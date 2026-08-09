//! 桌面 Viewer 使用的病人工作列表 API。

use std::sync::Arc;

use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use pacs_auth::{AuthService, Identity, Permission};
use serde::Deserialize;

use crate::routes::WebState;

const DEFAULT_PAGE_SIZE: u32 = 50;
const MAX_PAGE_SIZE: u32 = 100;
const MAX_QUERY_CHARS: usize = 128;

pub fn worklist_routes(state: WebState, auth: Arc<AuthService>) -> Router {
    Router::new()
        .route("/patients", get(patients))
        .route("/patients/{patient_id}/studies", get(patient_studies))
        .route("/studies/{study_uid}/series", get(study_series))
        .with_state(state)
        .layer(axum::middleware::from_fn(move |request, next| {
            let auth = Arc::clone(&auth);
            async move { pacs_auth::require(auth, Permission::ViewImages, request, next).await }
        }))
}

#[derive(Debug, Deserialize)]
struct PatientParams {
    #[serde(default)]
    query: String,
    limit: Option<u32>,
    offset: Option<u64>,
}

async fn patients(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Query(params): Query<PatientParams>,
) -> Result<Json<Vec<pacs_db::PatientSummary>>, WorklistError> {
    if params.query.chars().count() > MAX_QUERY_CHARS {
        return Err(WorklistError::BadRequest(format!(
            "搜索内容不能超过 {MAX_QUERY_CHARS} 个字符"
        )));
    }
    let limit = params.limit.unwrap_or(DEFAULT_PAGE_SIZE);
    if limit == 0 || limit > MAX_PAGE_SIZE {
        return Err(WorklistError::BadRequest(format!(
            "limit 必须在 1..={MAX_PAGE_SIZE} 之间"
        )));
    }
    let offset = i64::try_from(params.offset.unwrap_or(0))
        .map_err(|_| WorklistError::BadRequest("offset 超出范围".to_owned()))?;
    let rows = pacs_db::list_patients(
        &state.pool,
        identity.institution_id,
        identity.user_id,
        identity.role == pacs_auth::Role::Admin,
        params.query.trim(),
        i64::from(limit),
        offset,
    )
    .await
    .map_err(WorklistError::db)?;
    Ok(Json(rows))
}

async fn patient_studies(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(patient_id): Path<i64>,
) -> Result<Json<Vec<pacs_db::StudySummary>>, WorklistError> {
    if patient_id <= 0 {
        return Err(WorklistError::BadRequest("病人 ID 无效".to_owned()));
    }
    let rows = pacs_db::list_patient_studies(
        &state.pool,
        identity.institution_id,
        identity.user_id,
        identity.role == pacs_auth::Role::Admin,
        patient_id,
    )
    .await
    .map_err(WorklistError::db)?;
    Ok(Json(rows))
}

async fn study_series(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(study_uid): Path<String>,
) -> Result<Json<Vec<pacs_db::SeriesSummary>>, WorklistError> {
    pacs_core::Uid::parse(&study_uid)
        .map_err(|_| WorklistError::BadRequest("StudyInstanceUID 无效".to_owned()))?;
    let rows = pacs_db::list_study_series(
        &state.pool,
        identity.institution_id,
        identity.user_id,
        identity.role == pacs_auth::Role::Admin,
        &study_uid,
    )
    .await
    .map_err(WorklistError::db)?;
    Ok(Json(rows))
}

#[derive(Debug, thiserror::Error)]
enum WorklistError {
    #[error("{0}")]
    BadRequest(String),
    #[error("内部错误")]
    Internal,
}

impl WorklistError {
    fn db(error: pacs_db::DbError) -> Self {
        tracing::error!(%error, "工作列表查询失败");
        Self::Internal
    }
}

impl IntoResponse for WorklistError {
    fn into_response(self) -> Response {
        let status = match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let message = self.to_string();
        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}
