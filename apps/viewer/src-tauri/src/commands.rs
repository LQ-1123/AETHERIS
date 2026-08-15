//! Tauri IPC commands used by the viewer frontend.

use crate::ai::AiState;
use crate::mpr::{MprMetadata, MprRenderOptions, PixelStatistics, Plane, ProjectionMode, RoiShape};
use crate::remote::{
    DownloadProgress, PatientSummary, RemoteState, RemoteUser, SeriesSummary, StudySummary,
    UserWindowPreset,
};
use crate::state::{SeriesMetadata, ViewerState};
use pacs_ai::{SegmentationEngine, SegmentationRequest, SegmentationResult};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, State};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tauri::command]
pub async fn list_ai_models(
    state: State<'_, AiState>,
) -> Result<Vec<pacs_ai::RegisteredModelDescriptor>, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || state.catalog().map(|catalog| catalog.models))
        .await
        .map_err(|error| format!("AI 模型检查任务失败: {error}"))?
}

#[tauri::command]
pub async fn list_ai_catalog(state: State<'_, AiState>) -> Result<pacs_ai::AiCatalog, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || state.catalog())
        .await
        .map_err(|error| format!("AI 插件检查任务失败: {error}"))?
}

#[tauri::command]
pub async fn refresh_ai_plugins(state: State<'_, AiState>) -> Result<pacs_ai::AiCatalog, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || state.refresh_catalog())
        .await
        .map_err(|error| format!("AI 插件刷新任务失败: {error}"))?
}

#[tauri::command]
pub async fn check_ai_plugin(
    name: String,
    path: String,
    state: State<'_, AiState>,
) -> Result<pacs_ai::AiCatalog, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.check_plugin(&name, PathBuf::from(path).as_path())
    })
    .await
    .map_err(|error| format!("AI 插件检测任务失败: {error}"))?
}

#[tauri::command]
pub async fn add_ai_plugin(
    name: String,
    path: String,
    state: State<'_, AiState>,
) -> Result<pacs_ai::AiCatalog, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.add_plugin(&name, PathBuf::from(path).as_path())
    })
    .await
    .map_err(|error| format!("AI 插件保存任务失败: {error}"))?
}

#[tauri::command]
pub fn list_ai_plugin_configurations(
    state: State<'_, AiState>,
) -> Vec<crate::ai::AiPluginConfiguration> {
    state.configured_plugins()
}

#[tauri::command]
pub async fn run_ai_segmentation(
    handle: u64,
    stack_index: u32,
    model_id: String,
    app: AppHandle,
    viewer: State<'_, ViewerState>,
    ai: State<'_, AiState>,
) -> Result<SegmentationResult, String> {
    let series = viewer
        .ai_series_input(handle, stack_index)
        .map_err(|error| error.to_string())?;
    let ai = ai.inner().clone();
    let requested_model_id = model_id;
    let resolver = ai.clone();
    let resolved =
        tauri::async_runtime::spawn_blocking(move || resolver.resolve_model(&requested_model_id))
            .await
            .map_err(|error| format!("AI 模型路由任务失败: {error}"))??;
    let (job_id, cancellation) = ai.begin()?;
    let worker = resolved.worker;
    let registered_model_id = resolved.registered_id;
    let request = SegmentationRequest::new(job_id, resolved.model_id, series);
    let task = tauri::async_runtime::spawn_blocking(move || {
        let mut result = worker.segment(&request, &cancellation, &mut |progress| {
            let _ = app.emit("ai-segmentation-progress", progress);
        })?;
        result.model_id = registered_model_id;
        Ok::<_, pacs_ai::AiError>(result)
    })
    .await;
    ai.finish(job_id);
    let result = task.map_err(|error| format!("AI 分割任务失败: {error}"))?;
    result.map_err(|error| error.to_string())
}

#[tauri::command]
pub fn cancel_ai_segmentation(state: State<'_, AiState>) -> bool {
    state.cancel()
}

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
    request: RenderMprRequest,
    state: State<'_, ViewerState>,
) -> Result<tauri::ipc::Response, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let options = MprRenderOptions {
            window_center: request.window_center,
            window_width: request.window_width,
            voi_function: &request.voi_function,
            projection: request.projection,
            slab_thickness_mm: request.slab_thickness_mm,
        };
        state.render_mpr_slice(request.handle, request.plane, request.slice_index, &options)
    })
    .await
    .map_err(|error| format!("MPR 切面任务失败: {error}"))?
    .map(tauri::ipc::Response::new)
    .map_err(|error| error.to_string())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderMprRequest {
    handle: u64,
    plane: Plane,
    slice_index: u32,
    window_center: f64,
    window_width: f64,
    voi_function: String,
    projection: ProjectionMode,
    slab_thickness_mm: f64,
}

#[tauri::command]
pub fn begin_mpr_prefetch(state: State<'_, ViewerState>) -> usize {
    state.begin_mpr_prefetch()
}

#[tauri::command]
pub async fn prefetch_mpr_slices(
    request: PrefetchMprRequest,
    state: State<'_, ViewerState>,
) -> Result<usize, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let options = MprRenderOptions {
            window_center: request.window_center,
            window_width: request.window_width,
            voi_function: &request.voi_function,
            projection: request.projection,
            slab_thickness_mm: request.slab_thickness_mm,
        };
        state.prefetch_mpr_slices(
            request.handle,
            request.start_slices,
            &options,
            request.generation,
            |_, _| {},
        )
    })
    .await
    .map_err(|error| format!("MPR 切片预计算任务失败: {error}"))?
    .map_err(|error| error.to_string())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrefetchMprRequest {
    handle: u64,
    generation: usize,
    start_slices: [u32; 3],
    window_center: f64,
    window_width: f64,
    voi_function: String,
    projection: ProjectionMode,
    slab_thickness_mm: f64,
}

#[tauri::command]
pub fn cancel_mpr_prefetch(state: State<'_, ViewerState>) {
    state.cancel_mpr_prefetch();
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
pub async fn list_window_presets(
    state: State<'_, RemoteState>,
) -> Result<Vec<UserWindowPreset>, String> {
    state
        .list_window_presets()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn create_window_preset(
    modality: String,
    name: String,
    center: f64,
    width: f64,
    function: String,
    state: State<'_, RemoteState>,
) -> Result<UserWindowPreset, String> {
    state
        .create_window_preset(&modality, &name, center, width, &function)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn rename_window_preset(
    preset_id: i64,
    name: String,
    state: State<'_, RemoteState>,
) -> Result<UserWindowPreset, String> {
    state
        .rename_window_preset(preset_id, &name)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn delete_window_preset(
    preset_id: i64,
    state: State<'_, RemoteState>,
) -> Result<(), String> {
    state
        .delete_window_preset(preset_id)
        .await
        .map_err(|error| error.to_string())
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
pub async fn list_segmentation_projects(
    study_uid: String,
    series_uid: String,
    state: State<'_, RemoteState>,
) -> Result<serde_json::Value, String> {
    state
        .list_segmentation_projects(&study_uid, &series_uid)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn create_segmentation_project(
    study_uid: String,
    series_uid: String,
    input: serde_json::Value,
    state: State<'_, RemoteState>,
) -> Result<serde_json::Value, String> {
    state
        .create_segmentation_project(&study_uid, &series_uid, input)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn delete_segmentation_project(
    study_uid: String,
    series_uid: String,
    project_id: String,
    state: State<'_, RemoteState>,
) -> Result<(), String> {
    state
        .delete_segmentation_project(&study_uid, &series_uid, &project_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_segmentation_segments(
    study_uid: String,
    series_uid: String,
    project_id: String,
    tag: Option<String>,
    state: State<'_, RemoteState>,
) -> Result<serde_json::Value, String> {
    state
        .list_segmentation_segments(&study_uid, &series_uid, &project_id, tag.as_deref())
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn update_segmentation_segment_tags(
    study_uid: String,
    series_uid: String,
    project_id: String,
    segment_id: String,
    input: serde_json::Value,
    state: State<'_, RemoteState>,
) -> Result<serde_json::Value, String> {
    state
        .update_segmentation_segment_tags(&study_uid, &series_uid, &project_id, &segment_id, input)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_segmentation_masks(
    study_uid: String,
    series_uid: String,
    project_id: String,
    sop_instance_uid: String,
    frame_number: i32,
    state: State<'_, RemoteState>,
) -> Result<serde_json::Value, String> {
    state
        .list_segmentation_masks(
            &study_uid,
            &series_uid,
            &project_id,
            &sop_instance_uid,
            frame_number,
        )
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn upsert_segmentation_mask(
    study_uid: String,
    series_uid: String,
    project_id: String,
    segment_id: String,
    input: serde_json::Value,
    state: State<'_, RemoteState>,
) -> Result<serde_json::Value, String> {
    state
        .upsert_segmentation_mask(&study_uid, &series_uid, &project_id, &segment_id, input)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_segmentation_volume(
    study_uid: String,
    series_uid: String,
    project_id: String,
    segment_id: String,
    state: State<'_, RemoteState>,
) -> Result<serde_json::Value, String> {
    state
        .list_segmentation_volume(&study_uid, &series_uid, &project_id, &segment_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn upsert_segmentation_masks(
    study_uid: String,
    series_uid: String,
    project_id: String,
    segment_id: String,
    updates: serde_json::Value,
    state: State<'_, RemoteState>,
) -> Result<serde_json::Value, String> {
    state
        .upsert_segmentation_masks(&study_uid, &series_uid, &project_id, &segment_id, updates)
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

#[derive(Clone, Serialize)]
pub struct TransferProgress {
    phase: String,
    completed_bytes: u64,
    total_bytes: u64,
    completed_files: usize,
    total_files: usize,
    status: Option<String>,
}

#[tauri::command]
pub async fn import_to_pacs(
    paths: Vec<String>,
    app: AppHandle,
    state: State<'_, RemoteState>,
) -> Result<serde_json::Value, String> {
    let files = tauri::async_runtime::spawn_blocking(move || collect_upload_files(paths))
        .await
        .map_err(|error| format!("扫描导入文件失败: {error}"))??;
    if files.is_empty() {
        return Err("没有可上传的文件".to_owned());
    }
    let total_bytes = files
        .iter()
        .map(|(_, path)| std::fs::metadata(path).map(|v| v.len()).unwrap_or(0))
        .sum();
    let job = state.create_import().await.map_err(|e| e.to_string())?;
    state.begin_transfer(job).await;
    let result = async {
        let mut completed_bytes = 0u64;
        for (index, (name, path)) in files.iter().enumerate() {
            let size = tokio::fs::metadata(path)
                .await
                .map_err(|e| e.to_string())?
                .len();
            let upload = state
                .create_upload(job, name, size)
                .await
                .map_err(|e| e.to_string())?;
            let mut file = tokio::fs::File::open(path)
                .await
                .map_err(|e| e.to_string())?;
            let mut offset = 0u64;
            let mut buffer = vec![0u8; 8 * 1024 * 1024];
            loop {
                let read = file.read(&mut buffer).await.map_err(|e| e.to_string())?;
                if read == 0 {
                    break;
                }
                let chunk = buffer[..read].to_vec();
                let mut attempts = 0;
                loop {
                    match state.upload_chunk(job, upload, offset, chunk.clone()).await {
                        Ok(()) => break,
                        Err(_) if attempts < 3 => {
                            attempts += 1;
                            let server_offset = state
                                .upload_offset(job, upload)
                                .await
                                .map_err(|e| e.to_string())?;
                            if server_offset == offset + read as u64 {
                                break;
                            }
                            if server_offset != offset {
                                return Err(format!("服务端上传偏移异常: {server_offset}"));
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(250 * attempts))
                                .await;
                        }
                        Err(error) => return Err(error.to_string()),
                    }
                }
                offset += read as u64;
                completed_bytes += read as u64;
                let _ = app.emit(
                    "transfer-progress",
                    TransferProgress {
                        phase: "upload".to_owned(),
                        completed_bytes,
                        total_bytes,
                        completed_files: index,
                        total_files: files.len(),
                        status: None,
                    },
                );
            }
        }
        state
            .complete_import(job)
            .await
            .map_err(|e| e.to_string())?;
        poll_transfer(&state, "imports", job, &app, total_bytes, files.len()).await
    }
    .await;
    state.end_transfer().await;
    result
}

#[tauri::command]
pub async fn export_from_pacs(
    study_uid: String,
    series_uid: Option<String>,
    destination: String,
    app: AppHandle,
    state: State<'_, RemoteState>,
) -> Result<serde_json::Value, String> {
    let job = state
        .create_export(&study_uid, series_uid.as_deref())
        .await
        .map_err(|e| e.to_string())?;
    state.begin_transfer(job).await;
    let result = async {
        let value = poll_transfer(&state, "exports", job, &app, 0, 0).await?;
        let bytes = state
            .download_export(job)
            .await
            .map_err(|e| e.to_string())?;
        let target = PathBuf::from(destination);
        let part = target.with_extension("zip.part");
        let mut file = tokio::fs::File::create(&part)
            .await
            .map_err(|e| e.to_string())?;
        file.write_all(&bytes).await.map_err(|e| e.to_string())?;
        file.sync_all().await.map_err(|e| e.to_string())?;
        tokio::fs::rename(part, target)
            .await
            .map_err(|e| e.to_string())?;
        Ok(value)
    }
    .await;
    state.end_transfer().await;
    result
}

#[tauri::command]
pub async fn cancel_transfer(kind: String, state: State<'_, RemoteState>) -> Result<(), String> {
    state
        .cancel_transfer(&kind)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn router_get(
    path: String,
    state: State<'_, RemoteState>,
) -> Result<serde_json::Value, String> {
    state
        .router_get(&path)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn router_write(
    method: String,
    path: String,
    body: serde_json::Value,
    state: State<'_, RemoteState>,
) -> Result<serde_json::Value, String> {
    let method = reqwest::Method::from_bytes(method.as_bytes())
        .map_err(|_| "Router HTTP 方法无效".to_owned())?;
    if !matches!(method, reqwest::Method::POST | reqwest::Method::PUT) {
        return Err("Router 只允许 POST 或 PUT".to_owned());
    }
    state
        .router_write(method, &path, Some(body))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn router_delete(path: String, state: State<'_, RemoteState>) -> Result<(), String> {
    state
        .router_delete(&path)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn lifecycle_get(
    path: String,
    state: State<'_, RemoteState>,
) -> Result<serde_json::Value, String> {
    state
        .lifecycle_get(&path)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn lifecycle_write(
    method: String,
    path: String,
    body: serde_json::Value,
    state: State<'_, RemoteState>,
) -> Result<serde_json::Value, String> {
    let method = reqwest::Method::from_bytes(method.as_bytes())
        .map_err(|_| "生命周期 HTTP 方法无效".to_owned())?;
    if !matches!(method, reqwest::Method::POST | reqwest::Method::PUT) {
        return Err("生命周期接口只允许 POST 或 PUT".to_owned());
    }
    state
        .lifecycle_write(method, &path, Some(body))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn lifecycle_delete(path: String, state: State<'_, RemoteState>) -> Result<(), String> {
    state
        .lifecycle_delete(&path)
        .await
        .map_err(|error| error.to_string())
}

async fn poll_transfer(
    state: &RemoteState,
    kind: &str,
    job: uuid::Uuid,
    app: &AppHandle,
    total_bytes: u64,
    total_files: usize,
) -> Result<serde_json::Value, String> {
    loop {
        let value = state
            .transfer_status(kind, job)
            .await
            .map_err(|e| e.to_string())?;
        let status = value
            .pointer("/job/status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown");
        let completed = value
            .pointer("/job/progress_completed")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as usize;
        let total = value
            .pointer("/job/progress_total")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(total_files as u64) as usize;
        let _ = app.emit(
            "transfer-progress",
            TransferProgress {
                phase: "processing".to_owned(),
                completed_bytes: total_bytes,
                total_bytes,
                completed_files: completed,
                total_files: total,
                status: Some(status.to_owned()),
            },
        );
        match status {
            "succeeded" => return Ok(value),
            "failed" | "cancelled" => {
                return Err(value
                    .pointer("/job/error_message")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("任务未完成")
                    .to_owned());
            }
            _ => tokio::time::sleep(std::time::Duration::from_millis(750)).await,
        }
    }
}

fn collect_upload_files(paths: Vec<String>) -> Result<Vec<(String, PathBuf)>, String> {
    let mut result = Vec::new();
    for raw in paths {
        let path = PathBuf::from(raw);
        if path.is_file() {
            let name = path
                .file_name()
                .and_then(|v| v.to_str())
                .ok_or("文件名不是 UTF-8")?
                .to_owned();
            result.push((name, path));
        } else if path.is_dir() {
            collect_directory(&path, &path, &mut result)?;
        }
    }
    result.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(result)
}

fn collect_directory(
    root: &std::path::Path,
    directory: &std::path::Path,
    files: &mut Vec<(String, PathBuf)>,
) -> Result<(), String> {
    for entry in std::fs::read_dir(directory).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            collect_directory(root, &entry.path(), files)?;
        } else if kind.is_file() {
            let relative = entry
                .path()
                .strip_prefix(root)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            files.push((relative, entry.path()));
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn local_stack_info(
    state: State<'_, std::sync::Arc<crate::local::LocalStack>>,
) -> Result<Option<crate::local::LocalModeInfo>, String> {
    let stack = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || stack.ensure())
        .await
        .map_err(|e| format!("本地服务任务失败: {e}"))?
}
