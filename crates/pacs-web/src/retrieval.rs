//! Administrator API for querying an external PACS and pulling a study with C-MOVE.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration as StdDuration;

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use dicom::core::{DataElement, PrimitiveValue, VR};
use dicom::dictionary_std::tags;
use dicom::object::InMemDicomObject;
use pacs_auth::{AuthService, Identity, Permission};
use pacs_db::{BackgroundJob, JobKind, NewJob};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::WebState;

pub fn routes(state: WebState, auth: Arc<AuthService>) -> Router {
    Router::new()
        .route("/retrieval/sources", get(sources))
        .route("/retrieval/sources/{device_id}", put(configure_source))
        .route("/retrieval/sources/{device_id}/query", post(query_source))
        .route("/retrieval/sources/{device_id}/move", post(move_study))
        .route("/retrieval/jobs", get(list_jobs))
        .route("/retrieval/jobs/{job_id}", get(get_job))
        .route("/retrieval/jobs/{job_id}/cancel", post(cancel_job))
        .with_state(state)
        .layer(axum::middleware::from_fn(move |request, next| {
            let auth = Arc::clone(&auth);
            async move { pacs_auth::require(auth, Permission::ManageUsers, request, next).await }
        }))
}

async fn sources(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
) -> Result<Json<Vec<pacs_db::DicomDevice>>, ApiError> {
    Ok(Json(
        pacs_db::list_retrieval_sources(&state.pool, identity.institution_id)
            .await
            .map_err(ApiError::db)?,
    ))
}

#[derive(Debug, Deserialize)]
struct ConfigureSource {
    enabled: bool,
    port: Option<i32>,
    #[serde(default)]
    use_tls: bool,
    ca_pem: Option<String>,
}

async fn configure_source(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(device_id): Path<Uuid>,
    Json(request): Json<ConfigureSource>,
) -> Result<Json<pacs_db::DicomDevice>, ApiError> {
    Ok(Json(
        pacs_db::configure_retrieval_source(
            &state.pool,
            identity.institution_id,
            device_id,
            request.enabled,
            request.port,
            request.use_tls,
            request.ca_pem.as_deref(),
        )
        .await
        .map_err(ApiError::db)?,
    ))
}

#[derive(Debug, Default, Deserialize)]
struct QuerySource {
    patient_id: Option<String>,
    accession_number: Option<String>,
    study_date_from: Option<String>,
    study_date_to: Option<String>,
    modality: Option<String>,
}

#[derive(Debug, Serialize)]
struct RemoteStudy {
    study_instance_uid: String,
    patient_id: Option<String>,
    patient_name: Option<String>,
    study_date: Option<String>,
    accession_number: Option<String>,
    modalities: Option<String>,
    description: Option<String>,
}

async fn query_source(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(device_id): Path<Uuid>,
    Json(request): Json<QuerySource>,
) -> Result<Json<Vec<RemoteStudy>>, ApiError> {
    let device = pacs_db::retrieval_source(&state.pool, identity.institution_id, device_id)
        .await
        .map_err(ApiError::db)?;
    let config = client_config(&state, &device)?;
    let identifier = find_identifier(&request)?;
    let matches = pacs_dimse::c_find(&config, pacs_dimse::sop_class::STUDY_ROOT_FIND, &identifier)
        .await
        .map_err(ApiError::dimse)?;
    Ok(Json(
        matches
            .iter()
            .filter_map(|object| {
                Some(RemoteStudy {
                    study_instance_uid: pacs_core::utf8_text(object, tags::STUDY_INSTANCE_UID)?,
                    patient_id: pacs_core::utf8_text(object, tags::PATIENT_ID),
                    patient_name: pacs_core::utf8_text(object, tags::PATIENT_NAME),
                    study_date: pacs_core::utf8_text(object, tags::STUDY_DATE),
                    accession_number: pacs_core::utf8_text(object, tags::ACCESSION_NUMBER),
                    modalities: pacs_core::utf8_text(object, tags::MODALITIES_IN_STUDY),
                    description: pacs_core::utf8_text(object, tags::STUDY_DESCRIPTION),
                })
            })
            .collect(),
    ))
}

#[derive(Debug, Deserialize)]
struct MoveStudy {
    study_instance_uid: String,
}

async fn move_study(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(device_id): Path<Uuid>,
    Json(request): Json<MoveStudy>,
) -> Result<(StatusCode, Json<BackgroundJob>), ApiError> {
    pacs_core::Uid::parse(&request.study_instance_uid)
        .map_err(|_| ApiError::bad("invalid_study_uid", "StudyInstanceUID 无效"))?;
    let device = pacs_db::retrieval_source(&state.pool, identity.institution_id, device_id)
        .await
        .map_err(ApiError::db)?;
    // Validate the current source and local DIMSE configuration before a job
    // is accepted. The worker resolves them again at execution time so a
    // disabled source cannot be used by a queued job.
    let _ = client_config(&state, &device)?;
    let _ = local_ae_title(&state)?;
    let payload = json!({
        "device_id": device.id,
        "source_name": device.name,
        "source_ae_title": device.calling_ae_title,
        "study_instance_uid": request.study_instance_uid,
    });
    let job = pacs_db::create_background_job(
        &state.pool,
        NewJob {
            id: Uuid::new_v4(),
            institution_id: identity.institution_id,
            created_by: Some(identity.user_id),
            kind: JobKind::Retrieval,
            idempotency_key: None,
            payload: &payload,
            progress_total: 0,
            max_attempts: 1,
            available_at: None,
        },
    )
    .await
    .map_err(ApiError::db)?;
    Ok((StatusCode::ACCEPTED, Json(job)))
}

async fn list_jobs(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
) -> Result<Json<Vec<BackgroundJob>>, ApiError> {
    Ok(Json(
        pacs_db::list_background_jobs(
            &state.pool,
            identity.institution_id,
            JobKind::Retrieval,
            100,
        )
        .await
        .map_err(ApiError::db)?,
    ))
}

async fn get_job(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(job_id): Path<Uuid>,
) -> Result<Json<BackgroundJob>, ApiError> {
    let job = pacs_db::get_background_job(&state.pool, identity.institution_id, job_id)
        .await
        .map_err(ApiError::db)?;
    ensure_retrieval_job(&job)?;
    Ok(Json(job))
}

async fn cancel_job(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(job_id): Path<Uuid>,
) -> Result<Json<BackgroundJob>, ApiError> {
    let job = pacs_db::get_background_job(&state.pool, identity.institution_id, job_id)
        .await
        .map_err(ApiError::db)?;
    ensure_retrieval_job(&job)?;
    Ok(Json(
        pacs_db::request_job_cancellation(&state.pool, identity.institution_id, job_id)
            .await
            .map_err(ApiError::db)?,
    ))
}

fn ensure_retrieval_job(job: &BackgroundJob) -> Result<(), ApiError> {
    if job.kind == JobKind::Retrieval {
        Ok(())
    } else {
        Err(ApiError::bad(
            "not_retrieval_job",
            "任务不属于外部 PACS 拉取",
        ))
    }
}

fn client_config(
    state: &WebState,
    device: &pacs_db::DicomDevice,
) -> Result<pacs_dimse::DimseClientConfig, ApiError> {
    let port = device
        .retrieval_port
        .and_then(|port| u16::try_from(port).ok())
        .ok_or_else(|| ApiError::bad("retrieval_port_missing", "外部 PACS 未配置端口"))?;
    let local_ae = local_ae_title(state)?;
    let mut config = pacs_dimse::DimseClientConfig::new(
        device.source_ip.clone(),
        port,
        device.calling_ae_title.clone(),
        local_ae,
    );
    config.use_tls = device.retrieval_use_tls;
    config.ca_pem = device.retrieval_ca_pem.clone();
    Ok(config)
}

fn local_ae_title(state: &WebState) -> Result<String, ApiError> {
    state
        .dicom_node
        .as_ref()
        .map(|node| node.ae_title.clone())
        .ok_or_else(|| ApiError::bad("dicom_node_unavailable", "本机 DIMSE 节点未配置"))
}

/// Start the durable external-PACS retrieval worker.
///
/// A single worker processes retrieval jobs serially. Each remote C-MOVE may
/// still run the peer's C-STORE suboperations concurrently; serial job pickup
/// prevents administrators from accidentally opening an unbounded number of
/// long-lived associations from one node.
pub fn start_worker(state: WebState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let worker = Uuid::new_v4();
        let mut interval = tokio::time::interval(StdDuration::from_secs(1));
        loop {
            interval.tick().await;
            match pacs_db::claim_background_job(
                &state.pool,
                JobKind::Retrieval,
                worker,
                chrono::Duration::minutes(2),
            )
            .await
            {
                Ok(Some(job)) => process_job(&state, worker, job).await,
                Ok(None) => {}
                Err(error) => tracing::error!(%error, "领取外部 PACS 拉取任务失败"),
            }
        }
    })
}

async fn process_job(state: &WebState, worker: Uuid, job: BackgroundJob) {
    match run_job(state, worker, &job).await {
        Ok(result) => {
            let value = move_result_json(result);
            if let Err(error) =
                pacs_db::complete_background_job(&state.pool, job.id, worker, &value).await
            {
                tracing::error!(%error, job_id=%job.id, "完成外部 PACS 拉取任务失败");
            }
        }
        Err(error) => {
            if !matches!(
                &error,
                WorkerError::Dimse(pacs_dimse::ClientError::Cancelled)
            ) {
                tracing::error!(%error, job_id=%job.id, "外部 PACS 拉取任务失败");
            }
            if let Err(db_error) =
                pacs_db::fail_background_job(&state.pool, job.id, worker, &error.to_string(), None)
                    .await
            {
                tracing::error!(%db_error, job_id=%job.id, "记录外部 PACS 拉取失败状态失败");
            }
        }
    }
}

async fn run_job(
    state: &WebState,
    worker: Uuid,
    job: &BackgroundJob,
) -> Result<pacs_dimse::MoveResult, WorkerError> {
    let device_id = payload_uuid(&job.payload, "device_id")?;
    let study_uid = payload_string(&job.payload, "study_instance_uid")?;
    pacs_core::Uid::parse(study_uid)
        .map_err(|_| WorkerError::InvalidPayload("StudyInstanceUID 无效"))?;
    let device = pacs_db::retrieval_source(&state.pool, job.institution_id, device_id).await?;
    let config =
        client_config(state, &device).map_err(|error| WorkerError::Config(error.message))?;
    let destination = local_ae_title(state).map_err(|error| WorkerError::Config(error.message))?;
    let identifier = study_identifier(study_uid);
    let cancelled = Arc::new(AtomicBool::new(false));
    let (progress_tx, mut progress_rx) =
        tokio::sync::watch::channel(pacs_dimse::MoveResult::default());
    let operation = pacs_dimse::c_move_controlled(
        &config,
        pacs_dimse::sop_class::STUDY_ROOT_MOVE,
        &destination,
        &identifier,
        Some(progress_tx),
        Arc::clone(&cancelled),
    );
    tokio::pin!(operation);
    let mut cancellation_poll = tokio::time::interval(StdDuration::from_millis(250));
    let mut heartbeat = tokio::time::interval(StdDuration::from_secs(30));
    let mut progress_open = true;

    loop {
        tokio::select! {
            result = &mut operation => {
                let progress = *progress_rx.borrow();
                persist_progress(state, worker, job.id, progress).await?;
                return result.map_err(WorkerError::Dimse);
            }
            changed = progress_rx.changed(), if progress_open => {
                if changed.is_err() {
                    progress_open = false;
                } else {
                    let progress = *progress_rx.borrow();
                    persist_progress(state, worker, job.id, progress).await?;
                }
            }
            _ = cancellation_poll.tick() => {
                let current = pacs_db::get_background_job(&state.pool, job.institution_id, job.id).await?;
                if current.cancel_requested {
                    cancelled.store(true, Ordering::Release);
                }
            }
            _ = heartbeat.tick() => {
                if !pacs_db::heartbeat_background_job(
                    &state.pool,
                    job.id,
                    worker,
                    chrono::Duration::minutes(2),
                ).await? {
                    return Err(WorkerError::LostLease);
                }
            }
        }
    }
}

async fn persist_progress(
    state: &WebState,
    worker: Uuid,
    job_id: Uuid,
    progress: pacs_dimse::MoveResult,
) -> Result<(), WorkerError> {
    let processed =
        i64::from(progress.completed) + i64::from(progress.failed) + i64::from(progress.warning);
    let total = processed + i64::from(progress.remaining);
    let value = move_result_json(progress);
    if pacs_db::update_background_job_progress_with_result(
        &state.pool,
        job_id,
        worker,
        processed,
        total,
        &value,
    )
    .await?
    {
        Ok(())
    } else {
        Err(WorkerError::LostLease)
    }
}

fn move_result_json(result: pacs_dimse::MoveResult) -> Value {
    json!({
        "remaining": result.remaining,
        "completed": result.completed,
        "failed": result.failed,
        "warning": result.warning,
    })
}

fn payload_uuid(payload: &Value, field: &'static str) -> Result<Uuid, WorkerError> {
    payload
        .get(field)
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or(WorkerError::InvalidPayload(field))
}

fn payload_string<'a>(payload: &'a Value, field: &'static str) -> Result<&'a str, WorkerError> {
    payload
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or(WorkerError::InvalidPayload(field))
}

#[derive(Debug, thiserror::Error)]
enum WorkerError {
    #[error(transparent)]
    Db(#[from] pacs_db::DbError),
    #[error("拉取任务载荷无效或缺少字段: {0}")]
    InvalidPayload(&'static str),
    #[error("拉取任务配置无效: {0}")]
    Config(String),
    #[error(transparent)]
    Dimse(pacs_dimse::ClientError),
    #[error("拉取任务租约已失效")]
    LostLease,
}

fn find_identifier(request: &QuerySource) -> Result<InMemDicomObject, ApiError> {
    let mut elements = vec![
        text(tags::QUERY_RETRIEVE_LEVEL, VR::CS, "STUDY"),
        text(tags::STUDY_INSTANCE_UID, VR::UI, ""),
        text(
            tags::PATIENT_ID,
            VR::LO,
            request.patient_id.as_deref().unwrap_or(""),
        ),
        text(tags::PATIENT_NAME, VR::PN, ""),
        text(
            tags::ACCESSION_NUMBER,
            VR::SH,
            request.accession_number.as_deref().unwrap_or(""),
        ),
        text(tags::STUDY_DESCRIPTION, VR::LO, ""),
        text(
            tags::MODALITIES_IN_STUDY,
            VR::CS,
            request.modality.as_deref().unwrap_or(""),
        ),
    ];
    let study_date = match (
        request
            .study_date_from
            .as_deref()
            .filter(|value| !value.is_empty()),
        request
            .study_date_to
            .as_deref()
            .filter(|value| !value.is_empty()),
    ) {
        (Some(from), Some(to)) => format!("{}-{}", compact_date(from)?, compact_date(to)?),
        (Some(from), None) => format!("{}-", compact_date(from)?),
        (None, Some(to)) => format!("-{}", compact_date(to)?),
        (None, None) => String::new(),
    };
    elements.push(text(tags::STUDY_DATE, VR::DA, &study_date));
    elements.sort_by_key(|element| element.header().tag);
    Ok(InMemDicomObject::from_element_iter(elements))
}

fn study_identifier(study_uid: &str) -> InMemDicomObject {
    InMemDicomObject::from_element_iter([
        text(tags::QUERY_RETRIEVE_LEVEL, VR::CS, "STUDY"),
        text(tags::STUDY_INSTANCE_UID, VR::UI, study_uid),
    ])
}

fn text(tag: dicom::core::Tag, vr: VR, value: &str) -> dicom::object::mem::InMemElement {
    DataElement::new(tag, vr, PrimitiveValue::from(value.to_owned()))
}

fn compact_date(value: &str) -> Result<String, ApiError> {
    let compact: String = value.chars().filter(char::is_ascii_digit).collect();
    if compact.len() == 8 && pacs_core::query::parse_da(&compact).is_some() {
        Ok(compact)
    } else {
        Err(ApiError::bad(
            "invalid_study_date",
            "检查日期必须为 YYYY-MM-DD",
        ))
    }
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    fn bad(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code,
            message: message.into(),
        }
    }

    fn db(error: pacs_db::DbError) -> Self {
        match error {
            pacs_db::DbError::NotFound => Self {
                status: StatusCode::NOT_FOUND,
                code: "resource_not_found",
                message: "资源不存在".to_owned(),
            },
            pacs_db::DbError::Invalid(message) => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "validation_failed",
                message,
            },
            pacs_db::DbError::Conflict(message) => Self {
                status: StatusCode::CONFLICT,
                code: "conflict",
                message,
            },
            other => {
                tracing::error!(%other, "外部 PACS API 数据库错误");
                Self {
                    status: StatusCode::INTERNAL_SERVER_ERROR,
                    code: "internal_error",
                    message: "内部错误".to_owned(),
                }
            }
        }
    }

    fn dimse(error: pacs_dimse::ClientError) -> Self {
        tracing::warn!(%error, "外部 PACS DIMSE 操作失败");
        Self {
            status: StatusCode::BAD_GATEWAY,
            code: "dimse_operation_failed",
            message: error.to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({"error":{"code":self.code,"message":self.message}})),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_identifier_encodes_study_filters() {
        let identifier = find_identifier(&QuerySource {
            patient_id: Some("P-100".to_owned()),
            accession_number: None,
            study_date_from: Some("2026-08-01".to_owned()),
            study_date_to: Some("2026-08-20".to_owned()),
            modality: Some("CT".to_owned()),
        })
        .unwrap();
        assert_eq!(
            pacs_core::utf8_text(&identifier, tags::QUERY_RETRIEVE_LEVEL).as_deref(),
            Some("STUDY")
        );
        assert_eq!(
            pacs_core::utf8_text(&identifier, tags::STUDY_DATE).as_deref(),
            Some("20260801-20260820")
        );
        assert_eq!(
            pacs_core::utf8_text(&identifier, tags::MODALITIES_IN_STUDY).as_deref(),
            Some("CT")
        );
    }
}
