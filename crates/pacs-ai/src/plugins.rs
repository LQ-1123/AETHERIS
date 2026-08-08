use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::thread;

use serde::{Deserialize, Serialize};

use crate::{
    AiError, LabelDescriptor, LocalWorker, ModelDescriptor, SegmentationEngine,
    WORKER_PROTOCOL_VERSION, WorkerConfig,
};

const MANIFEST_FILE: &str = "plugin.json";
const MAX_PLUGINS: usize = 32;
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const MAX_ID_LENGTH: usize = 96;
const MAX_TEXT_LENGTH: usize = 120;
const MAX_LAUNCH_ARGUMENTS: usize = 32;
const MAX_ARGUMENT_LENGTH: usize = 1_024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginManifest {
    pub manifest_version: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    pub worker_protocol: u32,
    pub launcher: PluginLauncher,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PluginLauncher {
    Python {
        script: String,
        #[serde(default)]
        args: Vec<String>,
    },
    Executable {
        path: String,
        #[serde(default)]
        args: Vec<String>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum PluginSource {
    Bundled,
    User,
    Legacy,
}

#[derive(Debug, Clone)]
pub struct PluginRoot {
    pub path: PathBuf,
    pub source: PluginSource,
    direct: bool,
    name_override: Option<String>,
}

impl PluginRoot {
    pub fn new(path: PathBuf, source: PluginSource) -> Self {
        Self {
            path,
            source,
            direct: false,
            name_override: None,
        }
    }

    pub fn direct(path: PathBuf, source: PluginSource, name_override: Option<String>) -> Self {
        Self {
            path,
            source,
            direct: true,
            name_override,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginStatus {
    pub id: String,
    pub name: String,
    pub version: String,
    pub source: PluginSource,
    pub available: bool,
    pub unavailable_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RegisteredModelDescriptor {
    pub id: String,
    pub plugin_id: String,
    pub plugin_name: String,
    pub plugin_version: String,
    pub model_id: String,
    pub display_name: String,
    pub version: String,
    pub description: String,
    pub supported_modalities: Vec<String>,
    pub labels: Vec<LabelDescriptor>,
    pub estimated_peak_memory_mb: u32,
    pub model_download_mb: u32,
    pub device: Option<String>,
    pub available: bool,
    pub unavailable_reason: Option<String>,
}

impl RegisteredModelDescriptor {
    fn new(plugin: &PluginEntry, model: ModelDescriptor) -> Self {
        Self {
            id: qualified_model_id(&plugin.manifest.id, &model.id),
            plugin_id: plugin.manifest.id.clone(),
            plugin_name: plugin.manifest.name.clone(),
            plugin_version: plugin.manifest.version.clone(),
            model_id: model.id,
            display_name: model.display_name,
            version: model.version,
            description: model.description,
            supported_modalities: model.supported_modalities,
            labels: model.labels,
            estimated_peak_memory_mb: model.estimated_peak_memory_mb,
            model_download_mb: model.model_download_mb,
            device: model.device,
            available: model.available,
            unavailable_reason: model.unavailable_reason,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AiCatalog {
    pub plugins: Vec<PluginStatus>,
    pub models: Vec<RegisteredModelDescriptor>,
}

#[derive(Clone)]
struct PluginEntry {
    manifest: PluginManifest,
    source: PluginSource,
    worker: Arc<LocalWorker>,
}

#[derive(Clone)]
pub struct ResolvedModel {
    pub registered_id: String,
    pub model_id: String,
    pub worker: Arc<LocalWorker>,
}

#[derive(Clone)]
pub struct PluginRegistry {
    entries: Arc<Vec<PluginEntry>>,
    invalid: Arc<Vec<PluginStatus>>,
    cache: Arc<RwLock<Option<AiCatalog>>>,
}

impl PluginRegistry {
    pub fn discover(roots: &[PluginRoot], python_override: Option<PathBuf>) -> Self {
        let mut entries = Vec::new();
        let mut invalid = Vec::new();
        let mut seen = HashSet::new();

        for root in roots {
            let mut directories = if root.direct {
                vec![root.path.clone()]
            } else {
                let Ok(children) = fs::read_dir(&root.path) else {
                    continue;
                };
                children
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .filter(|path| path.is_dir() && path.join(MANIFEST_FILE).is_file())
                    .collect::<Vec<_>>()
            };
            directories.sort();
            for directory in directories {
                if entries.len() + invalid.len() >= MAX_PLUGINS {
                    invalid.push(invalid_status(
                        "plugin-limit",
                        "插件数量超过 32 个",
                        root.source,
                    ));
                    break;
                }
                match load_plugin(
                    &directory,
                    root.source,
                    python_override.as_deref(),
                    root.name_override.as_deref(),
                ) {
                    Ok(entry) if seen.insert(entry.manifest.id.clone()) => entries.push(entry),
                    Ok(entry) => invalid.push(PluginStatus {
                        id: entry.manifest.id,
                        name: entry.manifest.name,
                        version: entry.manifest.version,
                        source: root.source,
                        available: false,
                        unavailable_reason: Some("插件 ID 重复，已保留优先级更高的插件".to_owned()),
                    }),
                    Err(error) => invalid.push(invalid_status(
                        directory
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("invalid-plugin"),
                        &error.to_string(),
                        root.source,
                    )),
                }
            }
        }
        Self::from_parts(entries, invalid)
    }

    pub fn legacy(worker: LocalWorker) -> Self {
        let entry = PluginEntry {
            manifest: PluginManifest {
                manifest_version: 1,
                id: "legacy.worker".to_owned(),
                name: "Legacy AI Worker".to_owned(),
                version: "1".to_owned(),
                worker_protocol: WORKER_PROTOCOL_VERSION,
                launcher: PluginLauncher::Python {
                    script: "worker.py".to_owned(),
                    args: Vec::new(),
                },
            },
            source: PluginSource::Legacy,
            worker: Arc::new(worker),
        };
        Self::from_parts(vec![entry], Vec::new())
    }

    fn from_parts(entries: Vec<PluginEntry>, invalid: Vec<PluginStatus>) -> Self {
        Self {
            entries: Arc::new(entries),
            invalid: Arc::new(invalid),
            cache: Arc::new(RwLock::new(None)),
        }
    }

    pub fn catalog(&self) -> Result<AiCatalog, AiError> {
        if let Some(catalog) = self
            .cache
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            return Ok(catalog);
        }
        self.refresh_catalog()
    }

    pub fn refresh_catalog(&self) -> Result<AiCatalog, AiError> {
        let handles = self
            .entries
            .iter()
            .cloned()
            .map(|entry| {
                thread::spawn(move || {
                    let result = entry.worker.models();
                    (entry, result)
                })
            })
            .collect::<Vec<_>>();

        let mut plugins = self.invalid.as_ref().clone();
        let mut models = Vec::new();
        for handle in handles {
            let (entry, result) = handle
                .join()
                .map_err(|_| AiError::WorkerFailed("AI 插件探测线程异常退出".to_owned()))?;
            match result {
                Ok(worker_models) => {
                    plugins.push(plugin_status(&entry, true, None));
                    models.extend(
                        worker_models
                            .into_iter()
                            .map(|model| RegisteredModelDescriptor::new(&entry, model)),
                    );
                }
                Err(error) => plugins.push(plugin_status(&entry, false, Some(error.to_string()))),
            }
        }
        plugins.sort_by(|left, right| {
            left.source
                .cmp(&right.source)
                .then_with(|| left.name.cmp(&right.name))
                .then_with(|| left.id.cmp(&right.id))
        });
        models.sort_by(|left, right| {
            left.plugin_name
                .cmp(&right.plugin_name)
                .then_with(|| left.display_name.cmp(&right.display_name))
                .then_with(|| left.id.cmp(&right.id))
        });
        let catalog = AiCatalog { plugins, models };
        *self
            .cache
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(catalog.clone());
        Ok(catalog)
    }

    pub fn resolve_model(&self, requested_id: &str) -> Result<ResolvedModel, AiError> {
        let catalog = self.catalog()?;
        let model = if requested_id.contains("::") {
            catalog.models.iter().find(|model| model.id == requested_id)
        } else {
            let mut matches = catalog
                .models
                .iter()
                .filter(|model| model.model_id == requested_id);
            let first = matches.next();
            if matches.next().is_some() {
                return Err(AiError::InvalidRequest(
                    "AI 模型 ID 在多个插件中重复，请使用完整模型 ID".to_owned(),
                ));
            }
            first
        }
        .ok_or_else(|| AiError::Unavailable("请求的 AI 模型不存在".to_owned()))?;
        if !model.available {
            return Err(AiError::Unavailable(
                model
                    .unavailable_reason
                    .clone()
                    .unwrap_or_else(|| "请求的 AI 模型不可用".to_owned()),
            ));
        }
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.manifest.id == model.plugin_id)
            .ok_or_else(|| AiError::Unavailable("AI 插件已失效，请刷新插件列表".to_owned()))?;
        Ok(ResolvedModel {
            registered_id: model.id.clone(),
            model_id: model.model_id.clone(),
            worker: Arc::clone(&entry.worker),
        })
    }
}

fn load_plugin(
    directory: &Path,
    source: PluginSource,
    python_override: Option<&Path>,
    name_override: Option<&str>,
) -> Result<PluginEntry, AiError> {
    let manifest_path = directory.join(MANIFEST_FILE);
    let metadata = fs::metadata(&manifest_path)?;
    if metadata.len() == 0 || metadata.len() > MAX_MANIFEST_BYTES {
        return Err(AiError::Protocol("插件 Manifest 大小无效".to_owned()));
    }
    let mut manifest: PluginManifest = serde_json::from_slice(&fs::read(&manifest_path)?)?;
    validate_manifest(&manifest)?;
    if let Some(name) = name_override {
        if !valid_text(name) {
            return Err(AiError::Protocol("插件显示名称无效".to_owned()));
        }
        manifest.name = name.to_owned();
    }
    let root = directory.canonicalize()?;
    let config = worker_config(&root, &manifest.launcher, python_override)?;
    Ok(PluginEntry {
        manifest,
        source,
        worker: Arc::new(LocalWorker::new(config)),
    })
}

fn validate_manifest(manifest: &PluginManifest) -> Result<(), AiError> {
    if manifest.manifest_version != 1 {
        return Err(AiError::Protocol("插件 Manifest 版本不兼容".to_owned()));
    }
    if manifest.worker_protocol != WORKER_PROTOCOL_VERSION {
        return Err(AiError::Protocol("插件 Worker 协议版本不兼容".to_owned()));
    }
    if !valid_id(&manifest.id) || !valid_text(&manifest.name) || !valid_text(&manifest.version) {
        return Err(AiError::Protocol("插件身份信息无效".to_owned()));
    }
    let args = match &manifest.launcher {
        PluginLauncher::Python { script, args } => {
            validate_relative_path(script)?;
            args
        }
        PluginLauncher::Executable { path, args } => {
            validate_relative_path(path)?;
            args
        }
    };
    if args.len() > MAX_LAUNCH_ARGUMENTS
        || args
            .iter()
            .any(|argument| argument.is_empty() || argument.len() > MAX_ARGUMENT_LENGTH)
    {
        return Err(AiError::Protocol("插件启动参数无效".to_owned()));
    }
    Ok(())
}

fn worker_config(
    root: &Path,
    launcher: &PluginLauncher,
    python_override: Option<&Path>,
) -> Result<WorkerConfig, AiError> {
    match launcher {
        PluginLauncher::Python { script, args } => {
            let script = resolve_relative_file(root, script)?;
            let python = plugin_python(root)
                .or_else(|| python_override.map(Path::to_path_buf))
                .unwrap_or_else(default_python);
            let mut command_args = vec![script.to_string_lossy().into_owned()];
            command_args.extend(args.iter().cloned());
            Ok(WorkerConfig::new(python, command_args))
        }
        PluginLauncher::Executable { path, args } => Ok(WorkerConfig::new(
            resolve_relative_file(root, path)?,
            args.clone(),
        )),
    }
}

fn plugin_python(root: &Path) -> Option<PathBuf> {
    let path = if cfg!(windows) {
        root.join(".venv/Scripts/python.exe")
    } else {
        root.join(".venv/bin/python")
    };
    path.is_file().then_some(path)
}

fn default_python() -> PathBuf {
    PathBuf::from(if cfg!(windows) { "python" } else { "python3" })
}

fn resolve_relative_file(root: &Path, relative: &str) -> Result<PathBuf, AiError> {
    validate_relative_path(relative)?;
    let path = root.join(relative);
    let canonical = path
        .canonicalize()
        .map_err(|_| AiError::Unavailable("插件入口文件不存在".to_owned()))?;
    if !canonical.starts_with(root) || !canonical.is_file() {
        return Err(AiError::Protocol("插件入口必须位于插件目录内".to_owned()));
    }
    Ok(canonical)
}

fn validate_relative_path(value: &str) -> Result<(), AiError> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(AiError::Protocol("插件入口路径无效".to_owned()));
    }
    Ok(())
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ID_LENGTH
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
}

fn valid_text(value: &str) -> bool {
    !value.trim().is_empty() && value.chars().count() <= MAX_TEXT_LENGTH
}

fn qualified_model_id(plugin_id: &str, model_id: &str) -> String {
    format!("{plugin_id}::{model_id}")
}

fn plugin_status(
    entry: &PluginEntry,
    available: bool,
    unavailable_reason: Option<String>,
) -> PluginStatus {
    PluginStatus {
        id: entry.manifest.id.clone(),
        name: entry.manifest.name.clone(),
        version: entry.manifest.version.clone(),
        source: entry.source,
        available,
        unavailable_reason,
    }
}

fn invalid_status(id: &str, reason: &str, source: PluginSource) -> PluginStatus {
    PluginStatus {
        id: id.to_owned(),
        name: id.to_owned(),
        version: String::new(),
        source,
        available: false,
        unavailable_reason: Some(reason.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_with_id(id: &str, launcher: PluginLauncher) -> PluginManifest {
        PluginManifest {
            manifest_version: 1,
            id: id.to_owned(),
            name: "Example model".to_owned(),
            version: "1.0.0".to_owned(),
            worker_protocol: 1,
            launcher,
        }
    }

    fn manifest(launcher: PluginLauncher) -> PluginManifest {
        manifest_with_id("org.example.model", launcher)
    }

    #[test]
    fn validates_manifest_identity_and_launcher() {
        let valid = manifest(PluginLauncher::Python {
            script: "worker.py".to_owned(),
            args: Vec::new(),
        });
        assert!(validate_manifest(&valid).is_ok());
        let mut invalid = valid.clone();
        invalid.id = "bad/id".to_owned();
        assert!(validate_manifest(&invalid).is_err());
        invalid = valid;
        invalid.launcher = PluginLauncher::Executable {
            path: "../worker".to_owned(),
            args: Vec::new(),
        };
        assert!(validate_manifest(&invalid).is_err());
    }

    #[test]
    fn rejects_entrypoints_escaping_the_plugin_directory() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("plugin");
        fs::create_dir(&root).unwrap();
        fs::write(directory.path().join("worker.py"), "").unwrap();
        let root = root.canonicalize().unwrap();
        assert!(resolve_relative_file(&root, "../worker.py").is_err());
    }

    #[test]
    fn qualified_model_ids_include_the_plugin_namespace() {
        assert_eq!(
            qualified_model_id("org.example.model", "fast"),
            "org.example.model::fast"
        );
    }

    #[test]
    fn bundled_plugins_win_duplicate_ids() {
        let bundled = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        for root in [bundled.path(), user.path()] {
            let plugin = root.join("example");
            fs::create_dir(&plugin).unwrap();
            fs::write(plugin.join("worker.py"), "").unwrap();
            fs::write(
                plugin.join(MANIFEST_FILE),
                serde_json::to_vec(&manifest(PluginLauncher::Python {
                    script: "worker.py".to_owned(),
                    args: Vec::new(),
                }))
                .unwrap(),
            )
            .unwrap();
        }
        let registry = PluginRegistry::discover(
            &[
                PluginRoot::new(bundled.path().to_path_buf(), PluginSource::Bundled),
                PluginRoot::new(user.path().to_path_buf(), PluginSource::User),
            ],
            None,
        );
        assert_eq!(registry.entries.len(), 1);
        assert_eq!(registry.invalid.len(), 1);
        assert_eq!(registry.entries[0].source, PluginSource::Bundled);
    }

    #[test]
    fn discovers_one_explicit_directory_and_applies_display_name() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("worker.py"), "").unwrap();
        fs::write(
            directory.path().join(MANIFEST_FILE),
            serde_json::to_vec(&manifest(PluginLauncher::Python {
                script: "worker.py".to_owned(),
                args: Vec::new(),
            }))
            .unwrap(),
        )
        .unwrap();

        let registry = PluginRegistry::discover(
            &[PluginRoot::direct(
                directory.path().to_path_buf(),
                PluginSource::User,
                Some("自定义显示名称".to_owned()),
            )],
            None,
        );

        assert_eq!(registry.entries.len(), 1);
        assert_eq!(registry.entries[0].manifest.name, "自定义显示名称");
    }

    #[cfg(unix)]
    #[test]
    fn isolates_failed_plugins_and_routes_namespaced_models() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::tempdir().unwrap();
        for (id, succeeds) in [
            ("org.example.first", true),
            ("org.example.second", true),
            ("org.example.broken", false),
        ] {
            let directory = root.path().join(id);
            fs::create_dir(&directory).unwrap();
            let script = directory.join("worker.sh");
            let contents = if succeeds {
                r#"#!/bin/sh
printf '%s\n' '{"protocol_version":1,"models":[{"id":"shared","display_name":"Shared","version":"1","description":"Test model","supported_modalities":["CT"],"labels":[{"id":"target","display_name":"Target","color":[1,2,3],"tags":["AI"]}],"estimated_peak_memory_mb":1,"model_download_mb":0,"device":"CPU","available":true,"unavailable_reason":null}]}'
"#
            } else {
                "#!/bin/sh\nexit 1\n"
            };
            fs::write(&script, contents).unwrap();
            let mut permissions = fs::metadata(&script).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&script, permissions).unwrap();
            fs::write(
                directory.join(MANIFEST_FILE),
                serde_json::to_vec(&manifest_with_id(
                    id,
                    PluginLauncher::Executable {
                        path: "worker.sh".to_owned(),
                        args: Vec::new(),
                    },
                ))
                .unwrap(),
            )
            .unwrap();
        }

        let registry = PluginRegistry::discover(
            &[PluginRoot::new(
                root.path().to_path_buf(),
                PluginSource::User,
            )],
            None,
        );
        let catalog = registry.refresh_catalog().unwrap();
        assert_eq!(catalog.models.len(), 2);
        assert_eq!(
            catalog
                .plugins
                .iter()
                .filter(|plugin| plugin.available)
                .count(),
            2
        );
        assert_eq!(
            registry
                .resolve_model("org.example.first::shared")
                .unwrap()
                .model_id,
            "shared"
        );
        assert!(registry.resolve_model("shared").is_err());
    }
}
