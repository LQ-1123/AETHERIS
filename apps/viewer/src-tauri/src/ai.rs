//! Local AI worker configuration and single-job lifecycle.

use std::env;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use pacs_ai::{CancellationToken, LocalWorker, WorkerConfig};
use tauri::path::BaseDirectory;
use tauri::{AppHandle, Manager};
use uuid::Uuid;

#[derive(Clone)]
pub struct AiState {
    worker: Arc<LocalWorker>,
    active_job: Arc<Mutex<Option<Uuid>>>,
    cancellation: CancellationToken,
}

impl AiState {
    pub fn new(app: &AppHandle) -> Self {
        let script = worker_script(app);
        let python = python_executable(&script);
        Self {
            worker: Arc::new(LocalWorker::new(WorkerConfig { python, script })),
            active_job: Arc::new(Mutex::new(None)),
            cancellation: CancellationToken::default(),
        }
    }

    pub fn worker(&self) -> Arc<LocalWorker> {
        Arc::clone(&self.worker)
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
        let active = self
            .active_job
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some();
        if active {
            self.cancellation.cancel();
        }
        active
    }
}

fn worker_script(app: &AppHandle) -> PathBuf {
    if let Some(path) = env::var_os("PACS_AI_WORKER").filter(|value| !value.is_empty()) {
        return PathBuf::from(path);
    }
    if cfg!(debug_assertions) {
        return PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../ai-worker/worker.py");
    }
    app.path()
        .resolve("ai-worker/worker.py", BaseDirectory::Resource)
        .unwrap_or_else(|_| PathBuf::from("ai-worker/worker.py"))
}

fn python_executable(script: &std::path::Path) -> PathBuf {
    if let Some(path) = env::var_os("PACS_AI_PYTHON").filter(|value| !value.is_empty()) {
        return PathBuf::from(path);
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
        .unwrap_or_else(|| PathBuf::from("python3"))
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
        assert_eq!(python_executable(&script), expected);
    }
}
