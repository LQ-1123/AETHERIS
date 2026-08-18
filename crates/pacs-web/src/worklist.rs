//! 桌面 Viewer 使用的病人工作列表 API。

use std::sync::Arc;

use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::NaiveDate;
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
        .route("/queue/studies", get(queue_studies))
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

#[derive(Debug, Deserialize)]
struct QueueParams {
    #[serde(default)]
    query: String,
    modality: Option<String>,
    body_part: Option<String>,
    report_status: Option<String>,
    institution: Option<String>,
    date_from: Option<String>,
    date_to: Option<String>,
    sort: Option<String>,
    order: Option<String>,
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

/// `GET /queue/studies`：按检查粒度返回高级工作队列。
async fn queue_studies(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Query(params): Query<QueueParams>,
) -> Result<Json<Vec<pacs_db::QueueStudyRow>>, WorklistError> {
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
    let date_from = optional_queue_date(params.date_from.as_deref(), "date_from")?;
    let date_to = optional_queue_date(params.date_to.as_deref(), "date_to")?;
    if matches!((date_from, date_to), (Some(from), Some(to)) if from > to) {
        return Err(WorklistError::BadRequest(
            "起始日期不能晚于结束日期".to_owned(),
        ));
    }
    let sort = parse_queue_sort(params.sort.as_deref())?;
    let descending = parse_queue_order(params.order.as_deref())?;
    let modality = optional_queue_text(params.modality.as_deref());
    let body_part = optional_queue_text(params.body_part.as_deref());
    let institution = optional_queue_text(params.institution.as_deref());
    let report_status = optional_report_status(params.report_status.as_deref())?;

    let rows = pacs_db::list_queue_studies(
        &state.pool,
        identity.institution_id,
        identity.user_id,
        identity.role == pacs_auth::Role::Admin,
        pacs_db::QueueFilter {
            query: params.query.trim(),
            modality,
            body_part,
            report_status,
            institution,
            date_from,
            date_to,
        },
        sort,
        descending,
        i64::from(limit),
        offset,
    )
    .await
    .map_err(WorklistError::db)?;
    Ok(Json(rows))
}

/// 队列日期接受 URL 常用的 ISO 日期，也接受 DICOM 工作列表常见的 `YYYYMMDD`。
fn optional_queue_date(raw: Option<&str>, field: &str) -> Result<Option<NaiveDate>, WorklistError> {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let parsed = NaiveDate::parse_from_str(raw, "%Y-%m-%d")
        .or_else(|_| NaiveDate::parse_from_str(raw, "%Y%m%d"))
        .map_err(|_| {
            WorklistError::BadRequest(format!(
                "{field} 日期格式无效（应为 YYYY-MM-DD 或 YYYYMMDD）"
            ))
        })?;
    Ok(Some(parsed))
}

fn optional_queue_text(raw: Option<&str>) -> Option<&str> {
    raw.map(str::trim).filter(|value| !value.is_empty())
}

fn optional_report_status(raw: Option<&str>) -> Result<Option<&str>, WorklistError> {
    let Some(value) = optional_queue_text(raw) else {
        return Ok(None);
    };
    if matches!(
        value,
        "pending" | "writing" | "locked" | "submitted" | "under_review" | "signed"
    ) {
        Ok(Some(value))
    } else {
        Err(WorklistError::BadRequest(
            "report_status 必须是 pending、writing、locked、submitted、under_review 或 signed"
                .to_owned(),
        ))
    }
}

fn parse_queue_sort(raw: Option<&str>) -> Result<pacs_db::QueueSort, WorklistError> {
    let value = raw.unwrap_or("study_date");
    let sort = match value {
        "study_date" => pacs_db::QueueSort::StudyDate,
        "patient_name" => pacs_db::QueueSort::PatientName,
        "modality" => pacs_db::QueueSort::Modality,
        "report_status" => pacs_db::QueueSort::ReportStatus,
        "institution" => pacs_db::QueueSort::Institution,
        _ => {
            return Err(WorklistError::BadRequest("sort 参数无效".to_owned()));
        }
    };
    Ok(sort)
}

fn parse_queue_order(raw: Option<&str>) -> Result<bool, WorklistError> {
    match raw.unwrap_or("desc") {
        "desc" => Ok(true),
        "asc" => Ok(false),
        _ => Err(WorklistError::BadRequest(
            "order 必须是 asc 或 desc".to_owned(),
        )),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, header};
    use chrono::Utc;
    use pacs_auth::{AccessTokenCodec, Role};
    use sqlx::postgres::PgPoolOptions;
    use tower::ServiceExt;

    #[test]
    fn queue_sort_is_a_closed_whitelist() {
        assert_eq!(
            parse_queue_sort(None).unwrap(),
            pacs_db::QueueSort::StudyDate
        );
        assert_eq!(
            parse_queue_sort(Some("patient_name")).unwrap(),
            pacs_db::QueueSort::PatientName
        );
        assert!(parse_queue_sort(Some("study_date DESC; DROP TABLE studies")).is_err());
    }

    #[test]
    fn queue_order_defaults_to_desc_and_rejects_other_values() {
        assert!(parse_queue_order(None).unwrap());
        assert!(!parse_queue_order(Some("asc")).unwrap());
        assert!(parse_queue_order(Some("DESC")).is_err());
    }

    #[test]
    fn queue_filters_validate_status_and_date_range_values() {
        assert_eq!(
            optional_report_status(Some("locked")).unwrap(),
            Some("locked")
        );
        assert_eq!(optional_report_status(Some("")).unwrap(), None);
        assert!(optional_report_status(Some("draft")).is_err());
        assert_eq!(
            optional_queue_date(Some("2026-08-18"), "date_from")
                .unwrap()
                .unwrap(),
            NaiveDate::from_ymd_opt(2026, 8, 18).unwrap()
        );
        assert_eq!(
            optional_queue_date(Some("20260818"), "date_from")
                .unwrap()
                .unwrap(),
            NaiveDate::from_ymd_opt(2026, 8, 18).unwrap()
        );
        assert!(optional_queue_date(Some("18/08/2026"), "date_from").is_err());
    }

    #[tokio::test]
    async fn queue_endpoint_requires_auth_and_returns_400_for_invalid_params() {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://localhost/worklist_validation_test")
            .unwrap();
        let secret = b"worklist-validation-secret-at-least-32-bytes";
        let token = AccessTokenCodec::new(secret)
            .unwrap()
            .issue(1, 1, "queue-test", Role::Radiologist, Utc::now())
            .unwrap();
        let auth = Arc::new(AuthService::new(pool.clone(), secret).unwrap());
        let app = worklist_routes(WebState::new(pool), auth);

        let unauthenticated = app
            .clone()
            .oneshot(Request::get("/queue/studies").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

        let long_query = "x".repeat(MAX_QUERY_CHARS + 1);
        let invalid_uris = [
            "/queue/studies?sort=unknown".to_owned(),
            "/queue/studies?order=DESC".to_owned(),
            "/queue/studies?limit=0".to_owned(),
            format!("/queue/studies?limit={}", MAX_PAGE_SIZE + 1),
            format!("/queue/studies?query={long_query}"),
            "/queue/studies?offset=18446744073709551615".to_owned(),
            "/queue/studies?date_from=2026-08-19&date_to=2026-08-18".to_owned(),
            "/queue/studies?date_from=18%2F08%2F2026".to_owned(),
            "/queue/studies?report_status=draft".to_owned(),
        ];
        for uri in invalid_uris {
            let response = app
                .clone()
                .oneshot(
                    Request::get(&uri)
                        .header(header::AUTHORIZATION, format!("Bearer {token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "URI: {uri}");
        }
    }
}
