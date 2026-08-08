//! Local AI plugin discovery and single-job lifecycle.

use std::env;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use pacs_ai::{
    AiCatalog, CancellationToken, LocalWorker, PluginRegistry, PluginRoot, PluginSource,
    ResolvedModel, WorkerConfig,
};
use serde::{Deserialize, Serialize};
use tauri::path::BaseDirectory;
use tauri::{AppHandle, Manager};
use uuid::Uuid;

#[derive(Clone)]
struct DiscoveryConfig {
    roots: Vec<PluginRoot>,
    configured_plugins: Vec<ConfiguredPlugin>,
    configured_plugins_path: Option<PathBuf>,
    python_override: Option<PathBuf>,
    legacy_worker: Option<(PathBuf, PathBuf)>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ConfiguredPlugin {
    name: String,
    path: PathBuf,
    #[serde(default)]
    id: String,
    #[serde(default)]
    version: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct AiPluginConfiguration {
    pub id: String,
    pub name: String,
    pub version: String,
    pub path: String,
}

#[derive(Clone)]
pub struct AiState {
    config: Arc<Mutex<DiscoveryConfig>>,
    registry: Arc<Mutex<PluginRegistry>>,
    active_job: Arc<Mutex<Option<Uuid>>>,
    cancellation: CancellationToken,
}

impl AiState {
    pub fn new(app: &AppHandle) -> Self {
        let python_override = env_path("PACS_AI_PYTHON").or_else(legacy_development_python);
        let legacy_worker = env_path("PACS_AI_WORKER").map(|script| {
            let python = python_executable(&script, python_override.as_deref());
            (python, script)
        });
        let mut roots = vec![PluginRoot::new(
            bundled_plugin_root(app),
            PluginSource::Bundled,
        )];
        let app_data_dir = app.path().app_data_dir().ok();
        if let Some(user_root) = app_data_dir.as_ref().map(|path| path.join("ai-plugins")) {
            if let Err(error) = std::fs::create_dir_all(&user_root) {
                tracing::warn!(%error, "无法创建用户 AI 插件目录");
            }
            roots.push(PluginRoot::new(user_root, PluginSource::User));
        }
        let configured_plugins_path = app_data_dir.map(|path| path.join("ai-plugin-paths.json"));
        let configured_plugins = configured_plugins_path
            .as_deref()
            .map(load_configured_plugins)
            .unwrap_or_default();
        let config = DiscoveryConfig {
            roots,
            configured_plugins,
            configured_plugins_path,
            python_override,
            legacy_worker,
        };
        Self {
            registry: Arc::new(Mutex::new(registry(&config))),
            config: Arc::new(Mutex::new(config)),
            active_job: Arc::new(Mutex::new(None)),
            cancellation: CancellationToken::default(),
        }
    }

    pub fn catalog(&self) -> Result<AiCatalog, String> {
        self.current_registry()
            .catalog()
            .map_err(|error| error.to_string())
    }

    pub fn refresh_catalog(&self) -> Result<AiCatalog, String> {
        if self.is_active() {
            return Err("AI 分割运行期间不能刷新插件".to_owned());
        }
        let config = self
            .config
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let refreshed = registry(&config);
        let catalog = refreshed
            .refresh_catalog()
            .map_err(|error| error.to_string())?;
        *self.registry.lock().unwrap_or_else(PoisonError::into_inner) = refreshed;
        Ok(catalog)
    }

    pub fn check_plugin(&self, name: &str, path: &Path) -> Result<AiCatalog, String> {
        let plugin = configured_plugin(name, path)?;
        let python_override = self
            .config
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .python_override
            .clone();
        PluginRegistry::discover(
            &[PluginRoot::direct(
                plugin.path,
                PluginSource::User,
                Some(plugin.name),
            )],
            python_override,
        )
        .refresh_catalog()
        .map_err(|error| error.to_string())
    }

    pub fn add_plugin(&self, name: &str, path: &Path) -> Result<AiCatalog, String> {
        if self.is_active() {
            return Err("AI 分割运行期间不能增加插件".to_owned());
        }
        let mut plugin = configured_plugin(name, path)?;
        let checked = self.check_plugin(&plugin.name, &plugin.path)?;
        let status = checked
            .plugins
            .first()
            .ok_or_else(|| "所选目录中没有 plugin.json".to_owned())?;
        if !status.available {
            return Err(status
                .unavailable_reason
                .clone()
                .unwrap_or_else(|| "插件不可用".to_owned()));
        }
        plugin.id = status.id.clone();
        plugin.version = status.version.clone();

        let mut config = self.config.lock().unwrap_or_else(PoisonError::into_inner);
        let existing_path = config
            .configured_plugins
            .iter()
            .any(|existing| existing.path == plugin.path);
        if !existing_path
            && self
                .current_registry()
                .catalog()
                .map_err(|error| error.to_string())?
                .plugins
                .iter()
                .any(|existing| existing.id == status.id)
        {
            return Err(format!("插件 ID {} 已存在", status.id));
        }

        let mut configured_plugins = config.configured_plugins.clone();
        if let Some(existing) = configured_plugins
            .iter_mut()
            .find(|existing| existing.path == plugin.path)
        {
            *existing = plugin;
        } else {
            if configured_plugins.len() >= 24 {
                return Err("最多可配置 24 个外部插件".to_owned());
            }
            configured_plugins.push(plugin);
        }
        save_configured_plugins(
            config.configured_plugins_path.as_deref(),
            &configured_plugins,
        )?;
        config.configured_plugins = configured_plugins;
        let refreshed = registry(&config);
        let catalog = refreshed
            .refresh_catalog()
            .map_err(|error| error.to_string())?;
        *self.registry.lock().unwrap_or_else(PoisonError::into_inner) = refreshed;
        Ok(catalog)
    }

    pub fn configured_plugins(&self) -> Vec<AiPluginConfiguration> {
        self.config
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .configured_plugins
            .iter()
            .map(|plugin| AiPluginConfiguration {
                id: plugin.id.clone(),
                name: plugin.name.clone(),
                version: plugin.version.clone(),
                path: plugin.path.to_string_lossy().into_owned(),
            })
            .collect()
    }

    pub fn resolve_model(&self, model_id: &str) -> Result<ResolvedModel, String> {
        self.current_registry()
            .resolve_model(model_id)
            .map_err(|error| error.to_string())
    }

    pub fn begin(&self) -> Result<(Uuid, CancellationToken), String> {
        let mut active = self
            .active_job
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if active.is_some() {
            return Err("已有 AI 分割任务正在运行".to_owned());
        }
        let job_id = Uuid::new_v4();
        self.cancellation.reset();
        *active = Some(job_id);
        Ok((job_id, self.cancellation.clone()))
    }

    pub fn finish(&self, job_id: Uuid) {
        let mut active = self
            .active_job
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if *active == Some(job_id) {
            *active = None;
        }
    }

    pub fn cancel(&self) -> bool {
        let active = self.is_active();
        if active {
            self.cancellation.cancel();
        }
        active
    }

    fn is_active(&self) -> bool {
        self.active_job
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    fn current_registry(&self) -> PluginRegistry {
        self.registry
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

fn registry(config: &DiscoveryConfig) -> PluginRegistry {
    if let Some((python, script)) = &config.legacy_worker {
        return PluginRegistry::legacy(LocalWorker::new(WorkerConfig::new(
            python.clone(),
            vec![script.to_string_lossy().into_owned()],
        )));
    }
    let mut roots = config.roots.clone();
    roots.extend(config.configured_plugins.iter().map(|plugin| {
        PluginRoot::direct(
            plugin.path.clone(),
            PluginSource::User,
            Some(plugin.name.clone()),
        )
    }));
    PluginRegistry::discover(&roots, config.python_override.clone())
}

fn configured_plugin(name: &str, path: &Path) -> Result<ConfiguredPlugin, String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 120 || name.chars().any(char::is_control) {
        return Err("插件名称不能为空且不能超过 120 个字符".to_owned());
    }
    if !path.is_absolute() {
        return Err("插件路径必须是绝对路径".to_owned());
    }
    let path = path
        .canonicalize()
        .map_err(|error| format!("无法访问插件目录: {error}"))?;
    if !path.is_dir() {
        return Err("插件路径不是目录".to_owned());
    }
    if !path.join("plugin.json").is_file() {
        return Err("所选目录中没有 plugin.json".to_owned());
    }
    Ok(ConfiguredPlugin {
        name: name.to_owned(),
        path,
        id: String::new(),
        version: String::new(),
    })
}

fn load_configured_plugins(path: &Path) -> Vec<ConfiguredPlugin> {
    let Ok(metadata) = std::fs::metadata(path) else {
        return Vec::new();
    };
    if metadata.len() > 64 * 1024 {
        tracing::warn!(path = %path.display(), "AI 插件路径配置文件过大，已忽略");
        return Vec::new();
    }
    match std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Vec<ConfiguredPlugin>>(&bytes).ok())
    {
        Some(plugins) => plugins.into_iter().take(24).collect(),
        None => {
            tracing::warn!(path = %path.display(), "无法读取 AI 插件路径配置，已忽略");
            Vec::new()
        }
    }
}

fn save_configured_plugins(
    configured_plugins_path: Option<&Path>,
    configured_plugins: &[ConfiguredPlugin],
) -> Result<(), String> {
    let Some(path) = configured_plugins_path else {
        return Err("无法确定应用数据目录".to_owned());
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("无法创建配置目录: {error}"))?;
    }
    let bytes = serde_json::to_vec_pretty(configured_plugins)
        .map_err(|error| format!("无法生成插件配置: {error}"))?;
    std::fs::write(path, bytes).map_err(|error| format!("无法保存插件配置: {error}"))
}

fn bundled_plugin_root(app: &AppHandle) -> PathBuf {
    if cfg!(debug_assertions) {
        return PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../ai-plugins");
    }
    app.path()
        .resolve("ai-plugins", BaseDirectory::Resource)
        .unwrap_or_else(|_| PathBuf::from("ai-plugins"))
}

fn env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn legacy_development_python() -> Option<PathBuf> {
    if !cfg!(debug_assertions) {
        return None;
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../ai-worker/.venv");
    let path = if cfg!(windows) {
        root.join("Scripts/python.exe")
    } else {
        root.join("bin/python")
    };
    path.is_file().then_some(path)
}

fn python_executable(script: &Path, override_path: Option<&Path>) -> PathBuf {
    if let Some(path) = override_path {
        return path.to_path_buf();
    }
    let environment = if cfg!(windows) {
        script
            .parent()
            .map(|path| path.join(".venv/Scripts/python.exe"))
    } else {
        script.parent().map(|path| path.join(".venv/bin/python"))
    };
    environment
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from(if cfg!(windows) { "python" } else { "python3" }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_worker_sibling_virtual_environment_when_present() {
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("worker.py");
        std::fs::write(&script, "").unwrap();
        let expected = if cfg!(windows) {
            directory.path().join(".venv/Scripts/python.exe")
        } else {
            directory.path().join(".venv/bin/python")
        };
        std::fs::create_dir_all(expected.parent().unwrap()).unwrap();
        std::fs::write(&expected, "").unwrap();
        assert_eq!(python_executable(&script, None), expected);
    }

    #[test]
    fn explicit_python_override_has_priority() {
        let override_path = Path::new("custom-python");
        assert_eq!(
            python_executable(Path::new("worker.py"), Some(override_path)),
            override_path
        );
    }

    #[test]
    fn validates_configured_plugin_directory() {
        let directory = tempfile::tempdir().unwrap();
        assert!(configured_plugin("Example", directory.path()).is_err());
        std::fs::write(directory.path().join("plugin.json"), "{}").unwrap();
        let plugin = configured_plugin(" Example ", directory.path()).unwrap();
        assert_eq!(plugin.name, "Example");
        assert!(plugin.path.is_absolute());
    }

    #[test]
    fn rejects_empty_configured_plugin_name() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("plugin.json"), "{}").unwrap();
        assert!(configured_plugin("  ", directory.path()).is_err());
    }
}
