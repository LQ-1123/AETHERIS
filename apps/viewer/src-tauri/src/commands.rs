//! Tauri IPC commands used by the viewer frontend.

use crate::ai::AiState;
use crate::mpr::{MprMetadata, MprRenderOptions, PixelStatistics, Plane, ProjectionMode, RoiShape};
use crate::remote::{
    AdminUser, ClinicalWorkItem, DiagnosticReport, DicomDevice, DownloadProgress, ExamRequest,
    ExamRequestStudyCandidate, InstitutionSettings, PasswordResetRequest, PatientSummary,
    QueueStudyRow, RemoteState, RemoteUser, ReportReviewEvent, ReportTemplate, ReportVersion,
    SeriesSourceEntry, SeriesSummary, StudySummary, UserWindowPreset, WorkloadRow,
};
use crate::state::{SeriesMetadata, ViewerState};
use pacs_ai::{SegmentationEngine, SegmentationRequest, SegmentationResult};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, State};
use tokio::io::AsyncWriteExt;

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
pub async fn request_password_reset(
    server_url: String,
    ca_cert_path: String,
    username: String,
    new_password: String,
    state: State<'_, RemoteState>,
) -> Result<(), String> {
    state
        .request_password_reset(
            &server_url,
            PathBuf::from(ca_cert_path).as_path(),
            &username,
            &new_password,
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
pub async fn list_report_templates(
    modality: Option<String>,
    state: State<'_, RemoteState>,
) -> Result<Vec<ReportTemplate>, String> {
    state
        .list_report_templates(modality.as_deref())
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_reports(
    study_uid: String,
    state: State<'_, RemoteState>,
) -> Result<Vec<DiagnosticReport>, String> {
    state
        .list_reports(&study_uid)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn create_report(
    study_uid: String,
    series_uids: Vec<String>,
    template_payload: Option<serde_json::Value>,
    is_positive: bool,
    state: State<'_, RemoteState>,
) -> Result<DiagnosticReport, String> {
    state
        .create_report(&study_uid, series_uids, template_payload, is_positive)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn update_report_draft(
    report_id: String,
    revision: i32,
    findings: String,
    impression: String,
    recommendation: Option<String>,
    template_payload: Option<serde_json::Value>,
    is_positive: bool,
    clear_template_payload: bool,
    state: State<'_, RemoteState>,
) -> Result<DiagnosticReport, String> {
    state
        .update_report_draft(
            &report_id,
            revision,
            &findings,
            &impression,
            recommendation.as_deref(),
            template_payload,
            is_positive,
            clear_template_payload,
        )
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn sign_report(
    report_id: String,
    revision: i32,
    state: State<'_, RemoteState>,
) -> Result<(), String> {
    state
        .sign_report(&report_id, revision)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn submit_report(
    report_id: String,
    revision: i32,
    state: State<'_, RemoteState>,
) -> Result<(), String> {
    state
        .submit_report(&report_id, revision)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn start_report_review(
    report_id: String,
    revision: i32,
    state: State<'_, RemoteState>,
) -> Result<(), String> {
    state
        .start_report_review(&report_id, revision)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn approve_report(
    report_id: String,
    revision: i32,
    modified: bool,
    findings: Option<String>,
    impression: Option<String>,
    recommendation: Option<String>,
    review_comment: Option<String>,
    state: State<'_, RemoteState>,
) -> Result<(), String> {
    state
        .approve_report(
            &report_id,
            revision,
            modified,
            findings.as_deref(),
            impression.as_deref(),
            recommendation.as_deref(),
            review_comment.as_deref(),
        )
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_report_review_events(
    report_id: String,
    state: State<'_, RemoteState>,
) -> Result<Vec<ReportReviewEvent>, String> {
    state
        .list_report_review_events(&report_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn begin_report_amendment(
    report_id: String,
    reason: String,
    state: State<'_, RemoteState>,
) -> Result<DiagnosticReport, String> {
    state
        .begin_report_amendment(&report_id, &reason)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_report_versions(
    report_id: String,
    state: State<'_, RemoteState>,
) -> Result<Vec<ReportVersion>, String> {
    state
        .list_report_versions(&report_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_worklist(
    status: Option<String>,
    state: State<'_, RemoteState>,
) -> Result<Vec<ClinicalWorkItem>, String> {
    state
        .list_worklist(status.as_deref())
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_exam_requests(
    status: Option<String>,
    limit: u32,
    offset: u64,
    state: State<'_, RemoteState>,
) -> Result<Vec<ExamRequest>, String> {
    state
        .list_exam_requests(status.as_deref(), limit, offset)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn create_exam_request(
    patient_id: String,
    patient_name: String,
    patient_birth_date: Option<String>,
    patient_sex: Option<String>,
    modality: String,
    body_part: String,
    request_type: String,
    clinical_indication: String,
    scheduled_at: Option<String>,
    state: State<'_, RemoteState>,
) -> Result<ExamRequest, String> {
    state
        .create_exam_request(
            &patient_id,
            &patient_name,
            patient_birth_date.as_deref(),
            patient_sex.as_deref(),
            &modality,
            &body_part,
            &request_type,
            &clinical_indication,
            scheduled_at.as_deref(),
        )
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn create_exam_request_for_study(
    study_uid: String,
    modality: String,
    body_part: String,
    request_type: String,
    clinical_indication: String,
    scheduled_at: Option<String>,
    state: State<'_, RemoteState>,
) -> Result<ExamRequest, String> {
    state
        .create_exam_request_for_study(
            &study_uid,
            &modality,
            &body_part,
            &request_type,
            &clinical_indication,
            scheduled_at.as_deref(),
        )
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn update_exam_request(
    request_id: String,
    revision: i32,
    patient_id: String,
    patient_name: String,
    patient_birth_date: Option<String>,
    patient_sex: Option<String>,
    modality: String,
    body_part: String,
    request_type: String,
    clinical_indication: String,
    scheduled_at: Option<String>,
    state: State<'_, RemoteState>,
) -> Result<ExamRequest, String> {
    state
        .update_exam_request(
            &request_id,
            revision,
            &patient_id,
            &patient_name,
            patient_birth_date.as_deref(),
            patient_sex.as_deref(),
            &modality,
            &body_part,
            &request_type,
            &clinical_indication,
            scheduled_at.as_deref(),
        )
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn bind_exam_request(
    request_id: String,
    study_uid: String,
    revision: i32,
    state: State<'_, RemoteState>,
) -> Result<ExamRequest, String> {
    state
        .bind_exam_request(&request_id, &study_uid, revision)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn exam_request_for_study(
    study_uid: String,
    state: State<'_, RemoteState>,
) -> Result<Option<ExamRequest>, String> {
    state
        .exam_request_for_study(&study_uid)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_exam_request_study_candidates(
    query: String,
    limit: u32,
    state: State<'_, RemoteState>,
) -> Result<Vec<ExamRequestStudyCandidate>, String> {
    state
        .list_exam_request_study_candidates(&query, limit)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn workload_report(
    date_from: String,
    date_to: String,
    state: State<'_, RemoteState>,
) -> Result<Vec<WorkloadRow>, String> {
    state
        .workload_report(&date_from, &date_to)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn work_item_for_series(
    series_uid: String,
    state: State<'_, RemoteState>,
) -> Result<ClinicalWorkItem, String> {
    state
        .work_item_for_series(&series_uid)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn study_work_items(
    study_uid: String,
    state: State<'_, RemoteState>,
) -> Result<Vec<ClinicalWorkItem>, String> {
    state
        .study_work_items(&study_uid)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn claim_study(
    study_uid: String,
    state: State<'_, RemoteState>,
) -> Result<usize, String> {
    state
        .claim_study(&study_uid)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn release_study(study_uid: String, state: State<'_, RemoteState>) -> Result<(), String> {
    state
        .release_study(&study_uid)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn claim_work_item(
    work_id: String,
    revision: i32,
    state: State<'_, RemoteState>,
) -> Result<(), String> {
    state
        .claim_work_item(&work_id, revision)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn release_work_item(
    work_id: String,
    revision: i32,
    state: State<'_, RemoteState>,
) -> Result<(), String> {
    state
        .release_work_item(&work_id, revision)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn register_device(
    name: String,
    calling_ae_title: String,
    source_ip: String,
    modality_hint: Option<String>,
    state: State<'_, RemoteState>,
) -> Result<DicomDevice, String> {
    state
        .register_device(
            &name,
            &calling_ae_title,
            &source_ip,
            modality_hint.as_deref(),
        )
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_devices(
    status: Option<String>,
    state: State<'_, RemoteState>,
) -> Result<Vec<DicomDevice>, String> {
    state
        .list_devices(status.as_deref())
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn approve_device(
    device_id: String,
    name: String,
    modality_hint: Option<String>,
    state: State<'_, RemoteState>,
) -> Result<DicomDevice, String> {
    state
        .approve_device(&device_id, &name, modality_hint.as_deref())
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn set_device_status(
    device_id: String,
    status: String,
    state: State<'_, RemoteState>,
) -> Result<(), String> {
    state
        .set_device_status(&device_id, &status)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_series_sources(
    unattributed: bool,
    limit: u32,
    offset: u32,
    state: State<'_, RemoteState>,
) -> Result<Vec<SeriesSourceEntry>, String> {
    state
        .list_series_sources(unattributed, limit, offset)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn resolve_series_source(
    series_uid: String,
    device_id: String,
    state: State<'_, RemoteState>,
) -> Result<(), String> {
    state
        .resolve_series_source(&series_uid, &device_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_users(state: State<'_, RemoteState>) -> Result<Vec<AdminUser>, String> {
    state.list_users().await.map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn get_institution_settings(
    state: State<'_, RemoteState>,
) -> Result<InstitutionSettings, String> {
    state
        .institution_settings()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn update_institution_settings(
    review_required: bool,
    state: State<'_, RemoteState>,
) -> Result<InstitutionSettings, String> {
    state
        .update_institution_settings(review_required)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn create_user(
    username: String,
    display_name: Option<String>,
    role: String,
    temporary_password: String,
    state: State<'_, RemoteState>,
) -> Result<AdminUser, String> {
    state
        .create_user(
            &username,
            display_name.as_deref(),
            &role,
            &temporary_password,
        )
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn update_user(
    user_id: i64,
    display_name: Option<String>,
    role: Option<String>,
    is_active: Option<bool>,
    state: State<'_, RemoteState>,
) -> Result<AdminUser, String> {
    state
        .update_user(user_id, display_name.as_deref(), role.as_deref(), is_active)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_password_reset_requests(
    state: State<'_, RemoteState>,
) -> Result<Vec<PasswordResetRequest>, String> {
    state
        .list_password_reset_requests()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn review_password_reset_request(
    request_id: i64,
    approve: bool,
    state: State<'_, RemoteState>,
) -> Result<PasswordResetRequest, String> {
    state
        .review_password_reset_request(request_id, approve)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_user_permissions(
    user_id: i64,
    state: State<'_, RemoteState>,
) -> Result<Vec<String>, String> {
    state
        .list_user_permissions(user_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn replace_user_permissions(
    user_id: i64,
    permissions: Vec<String>,
    state: State<'_, RemoteState>,
) -> Result<Vec<String>, String> {
    state
        .replace_user_permissions(user_id, permissions)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_user_device_grants(
    user_id: i64,
    state: State<'_, RemoteState>,
) -> Result<Vec<String>, String> {
    state
        .list_user_device_grants(user_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn replace_user_device_grants(
    user_id: i64,
    device_ids: Vec<String>,
    state: State<'_, RemoteState>,
) -> Result<Vec<String>, String> {
    state
        .replace_user_device_grants(user_id, device_ids)
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
pub async fn list_queue_studies(
    query: String,
    modality: Option<String>,
    body_part: Option<String>,
    report_status: Option<String>,
    institution: Option<String>,
    date_from: Option<String>,
    date_to: Option<String>,
    sort: String,
    order: String,
    limit: u32,
    offset: u64,
    state: State<'_, RemoteState>,
) -> Result<Vec<QueueStudyRow>, String> {
    state
        .list_queue_studies(
            &query,
            modality.as_deref(),
            body_part.as_deref(),
            report_status.as_deref(),
            institution.as_deref(),
            date_from.as_deref(),
            date_to.as_deref(),
            &sort,
            &order,
            limit,
            offset,
        )
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

#[tauri::command]
pub async fn local_stack_info(
    state: State<'_, std::sync::Arc<crate::local::LocalStack>>,
) -> Result<Option<crate::local::LocalModeInfo>, String> {
    let stack = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || stack.ensure())
        .await
        .map_err(|e| format!("本地服务任务失败: {e}"))?
}
