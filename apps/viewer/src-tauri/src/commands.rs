//! Tauri IPC commands used by the viewer frontend.

use crate::mpr::{MprMetadata, PixelStatistics, Plane, RoiShape};
use crate::remote::{
    DownloadProgress, PatientSummary, RemoteState, RemoteUser, SeriesSummary, StudySummary,
};
use crate::state::{SeriesMetadata, ViewerState};
use serde::Serialize;
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
pub async fn measure_frame_roi(
    handle: u64,
    stack_index: u32,
    frame_index: u32,
    shape: RoiShape,
    start: [f64; 2],
    end: [f64; 2],
    state: State<'_, ViewerState>,
) -> Result<PixelStatistics, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.measure_frame_roi(handle, stack_index, frame_index, shape, start, end)
    })
    .await
    .map_err(|error| format!("像素统计任务失败: {error}"))?
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn measure_mpr_roi(
    handle: u64,
    plane: Plane,
    slice_index: u32,
    shape: RoiShape,
    start: [f64; 2],
    end: [f64; 2],
    state: State<'_, ViewerState>,
) -> Result<PixelStatistics, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.measure_mpr_roi(handle, plane, slice_index, shape, start, end)
    })
    .await
    .map_err(|error| format!("MPR 像素统计任务失败: {error}"))?
    .map_err(|error| error.to_string())
}

#[derive(Clone, Serialize)]
pub struct MprBuildProgress {
    completed: usize,
    total: usize,
}

#[tauri::command]
pub async fn prepare_mpr(
    handle: u64,
    stack_index: u32,
    app: AppHandle,
    state: State<'_, ViewerState>,
) -> Result<MprMetadata, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.prepare_mpr(handle, stack_index, |completed, total| {
            let _ = app.emit("mpr-build-progress", MprBuildProgress { completed, total });
        })
    })
    .await
    .map_err(|error| format!("MPR 构建任务失败: {error}"))?
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn render_mpr_slice(
    handle: u64,
    plane: Plane,
    slice_index: u32,
    window_center: f64,
    window_width: f64,
    voi_function: String,
    state: State<'_, ViewerState>,
) -> Result<tauri::ipc::Response, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.render_mpr_slice(
            handle,
            plane,
            slice_index,
            window_center,
            window_width,
            &voi_function,
        )
    })
    .await
    .map_err(|error| format!("MPR 切面任务失败: {error}"))?
    .map(tauri::ipc::Response::new)
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn close_mpr(handle: u64, state: State<'_, ViewerState>) -> Result<(), String> {
    state.close_mpr(handle).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn cancel_mpr_build(state: State<'_, ViewerState>) {
    state.cancel_mpr_build();
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
pub async fn list_shared_annotations(
    study_uid: String,
    series_uid: String,
    since: Option<String>,
    state: State<'_, RemoteState>,
) -> Result<serde_json::Value, String> {
    state
        .list_shared_annotations(&study_uid, &series_uid, since.as_deref())
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn create_shared_annotation(
    study_uid: String,
    series_uid: String,
    annotation: serde_json::Value,
    state: State<'_, RemoteState>,
) -> Result<serde_json::Value, String> {
    state
        .create_shared_annotation(&study_uid, &series_uid, annotation)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn update_shared_annotation(
    study_uid: String,
    series_uid: String,
    annotation_id: String,
    expected_revision: i64,
    geometry: serde_json::Value,
    deleted: bool,
    state: State<'_, RemoteState>,
) -> Result<serde_json::Value, String> {
    state
        .update_shared_annotation(
            &study_uid,
            &series_uid,
            &annotation_id,
            expected_revision,
            geometry,
            deleted,
        )
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn transform_schema(state: State<'_, RemoteState>) -> Result<serde_json::Value, String> {
    state
        .transform_schema()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn preview_clinical_transform(
    target_type: String,
    target_key: String,
    rules: serde_json::Value,
    reason: String,
    state: State<'_, RemoteState>,
) -> Result<serde_json::Value, String> {
    state
        .preview_clinical_transform(&target_type, &target_key, rules, &reason)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn confirm_transform(
    job_id: String,
    confirmation_token: String,
    state: State<'_, RemoteState>,
) -> Result<serde_json::Value, String> {
    state
        .confirm_transform(&job_id, &confirmation_token)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn transform_jobs(state: State<'_, RemoteState>) -> Result<serde_json::Value, String> {
    state
        .transform_jobs()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn instance_revisions_by_sop(
    sop_uid: String,
    state: State<'_, RemoteState>,
) -> Result<serde_json::Value, String> {
    state
        .instance_revisions_by_sop(&sop_uid)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn preview_rollback(
    logical_id: String,
    version_id: i64,
    reason: String,
    state: State<'_, RemoteState>,
) -> Result<serde_json::Value, String> {
    state
        .preview_rollback(&logical_id, version_id, &reason)
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
