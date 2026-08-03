//! Tauri IPC commands used by the viewer frontend.

use crate::remote::{
    DownloadProgress, PatientSummary, RemoteState, RemoteUser, SeriesSummary, StudySummary,
};
use crate::state::{SeriesMetadata, ViewerState};
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, State};

#[tauri::command]
pub async fn open_series(
    paths: Vec<String>,
    state: State<'_, ViewerState>,
) -> Result<SeriesMetadata, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.open_series(paths.into_iter().map(PathBuf::from).collect())
    })
    .await
    .map_err(|error| format!("打开序列任务失败: {error}"))?
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn close_series(handle: u64, state: State<'_, ViewerState>) -> Result<(), String> {
    state.close(handle).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn select_image_stack(
    handle: u64,
    stack_index: u32,
    state: State<'_, ViewerState>,
) -> Result<SeriesMetadata, String> {
    state
        .select_image_stack(handle, stack_index)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn build_lut(
    handle: u64,
    stack_index: u32,
    frame_index: u32,
    window_center: f64,
    window_width: f64,
    voi_function: String,
    state: State<'_, ViewerState>,
) -> Result<tauri::ipc::Response, String> {
    state
        .build_lut(
            handle,
            stack_index,
            frame_index,
            window_center,
            window_width,
            &voi_function,
        )
        .map(tauri::ipc::Response::new)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn remote_login(
    server_url: String,
    ca_cert_path: String,
    username: String,
    password: String,
    state: State<'_, RemoteState>,
) -> Result<RemoteUser, String> {
    state
        .login(
            &server_url,
            PathBuf::from(ca_cert_path).as_path(),
            &username,
            &password,
        )
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn remote_logout(state: State<'_, RemoteState>) -> Result<(), String> {
    state.logout().await.map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_patients(
    query: String,
    limit: u32,
    offset: u64,
    state: State<'_, RemoteState>,
) -> Result<Vec<PatientSummary>, String> {
    state
        .list_patients(&query, limit, offset)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_patient_studies(
    patient_id: i64,
    state: State<'_, RemoteState>,
) -> Result<Vec<StudySummary>, String> {
    state
        .list_patient_studies(patient_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_study_series(
    study_uid: String,
    state: State<'_, RemoteState>,
) -> Result<Vec<SeriesSummary>, String> {
    state
        .list_study_series(&study_uid)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn open_remote_series(
    study_uid: String,
    series_uid: String,
    app: AppHandle,
    remote: State<'_, RemoteState>,
    viewer: State<'_, ViewerState>,
) -> Result<SeriesMetadata, String> {
    let remote = remote.inner().clone();
    let _download = remote.begin_download().map_err(|error| error.to_string())?;
    let instance_uids = remote
        .list_instance_uids(&study_uid, &series_uid)
        .await
        .map_err(|error| error.to_string())?;
    if instance_uids.is_empty() {
        return Err("该序列没有可下载的实例".to_owned());
    }

    let directory = tempfile::Builder::new()
        .prefix("remote-pacs-series-")
        .tempdir()
        .map_err(|error| format!("无法创建下载目录: {error}"))?;
    let total = instance_uids.len();
    let _ = app.emit(
        "remote-download-progress",
        DownloadProgress {
            downloaded: 0,
            total,
        },
    );
    let mut paths = Vec::with_capacity(total);
    for (index, sop_uid) in instance_uids.into_iter().enumerate() {
        remote
            .check_cancelled()
            .map_err(|error| error.to_string())?;
        let bytes = remote
            .download_instance(&study_uid, &series_uid, &sop_uid)
            .await
            .map_err(|error| error.to_string())?;
        remote
            .check_cancelled()
            .map_err(|error| error.to_string())?;
        let path = directory.path().join(format!("{sop_uid}.dcm"));
        tokio::fs::write(&path, bytes)
            .await
            .map_err(|error| format!("无法保存下载的实例: {error}"))?;
        paths.push(path);
        let _ = app.emit(
            "remote-download-progress",
            DownloadProgress {
                downloaded: index + 1,
                total,
            },
        );
    }

    remote
        .check_cancelled()
        .map_err(|error| error.to_string())?;
    let viewer = viewer.inner().clone();
    tauri::async_runtime::spawn_blocking(move || viewer.open_temporary_series(paths, directory))
        .await
        .map_err(|error| format!("打开远程序列任务失败: {error}"))?
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn cancel_remote_download(state: State<'_, RemoteState>) {
    state.cancel_download();
}
