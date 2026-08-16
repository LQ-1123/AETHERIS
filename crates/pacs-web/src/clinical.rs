//! Public v1 administration and radiology workflow API.

use std::sync::Arc;

use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use chrono::NaiveDate;
use pacs_auth::{AuthService, Identity, Permission, Role};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::WebState;

pub fn routes(state: WebState, auth: Arc<AuthService>) -> Router {
    let admin_auth = Arc::clone(&auth);
    let admin = Router::new()
        .route("/roles", get(roles))
        .route("/users", get(users).post(create_user))
        .route("/users/{user_id}", patch(update_user))
        .route(
            "/users/{user_id}/device-grants",
            get(user_grants).put(replace_user_grants),
        )
        .route("/users/{user_id}/reset-password", post(reset_password))
        .route("/users/{user_id}/revoke-sessions", post(revoke_sessions))
        .route("/devices", get(devices).post(register_device))
        .route("/devices/{device_id}/approve", post(approve_device))
        .route("/devices/{device_id}", patch(update_device_status))
        .route("/series/{series_uid}/resolve-source", post(resolve_source))
        .route("/series-sources", get(series_sources))
        .route("/worklist/{work_id}/assign", post(assign))
        .with_state(state.clone())
        .layer(axum::middleware::from_fn(move |request, next| {
            let auth = Arc::clone(&admin_auth);
            async move { pacs_auth::require(auth, Permission::ManageUsers, request, next).await }
        }));

    let clinical_auth = Arc::clone(&auth);
    let clinical = Router::new()
        .route("/worklist", get(worklist))
        .route("/worklist/{work_id}/claim", post(claim))
        .route("/worklist/{work_id}/release", post(release))
        .route("/worklist/series/{series_uid}", get(work_item_by_series))
        .route(
            "/studies/{study_uid}/clinical-context",
            get(clinical_context),
        )
        .route("/reports", get(list_reports).post(create_report))
        .route("/reports/{report_id}/draft", put(update_draft))
        .route("/reports/{report_id}/sign", post(sign_report))
        .route("/reports/{report_id}/amendments", post(begin_amendment))
        .route("/reports/{report_id}/versions", get(report_versions))
        .route("/report-templates", get(report_templates))
        .with_state(state)
        .layer(axum::middleware::from_fn(move |request, next| {
            let auth = Arc::clone(&clinical_auth);
            async move { pacs_auth::require(auth, Permission::ViewImages, request, next).await }
        }));
    admin.merge(clinical)
}

#[derive(Serialize)]
struct RoleInfo {
    name: &'static str,
    permissions: Vec<&'static str>,
}

async fn roles() -> Json<Vec<RoleInfo>> {
    let permissions = [
        (Permission::ViewImages, "view_images"),
        (Permission::UploadImages, "upload_images"),
        (Permission::WriteReport, "write_report"),
        (Permission::ManageUsers, "manage_users"),
        (Permission::ViewAuditLog, "view_audit_log"),
        (Permission::DeleteImages, "delete_images"),
        (Permission::EditDicomTags, "edit_dicom_tags"),
        (Permission::ViewDicomRevisions, "view_dicom_revisions"),
    ];
    Json(
        Role::ALL
            .iter()
            .map(|role| RoleInfo {
                name: role.as_str(),
                permissions: permissions
                    .iter()
                    .filter_map(|(p, n)| role.can(*p).then_some(*n))
                    .collect(),
            })
            .collect(),
    )
}

async fn users(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
) -> Result<Json<Vec<pacs_auth::User>>, ApiError> {
    Ok(Json(
        pacs_auth::repository::list_users_for_institution(&state.pool, identity.institution_id)
            .await
            .map_err(ApiError::auth_repo)?,
    ))
}

#[derive(Deserialize)]
struct CreateUserRequest {
    username: String,
    display_name: Option<String>,
    role: Role,
    temporary_password: String,
    #[serde(default)]
    device_ids: Vec<Uuid>,
}

async fn create_user(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Json(req): Json<CreateUserRequest>,
) -> Result<(StatusCode, Json<pacs_auth::User>), ApiError> {
    let username = pacs_auth::normalize_username(&req.username)
        .map_err(|e| ApiError::bad("invalid_username", e.to_string()))?;
    pacs_auth::password::check_strength(&req.temporary_password, &username)
        .map_err(|e| ApiError::bad("weak_password", e.to_string()))?;
    let hash =
        pacs_auth::password::hash(&req.temporary_password).map_err(|_| ApiError::internal())?;
    let user = pacs_auth::repository::create_user_for_institution(
        &state.pool,
        identity.institution_id,
        pacs_auth::repository::NewUser {
            username: &username,
            display_name: req.display_name.as_deref(),
            password_hash: &hash,
            role: req.role,
            must_change_password: true,
        },
    )
    .await
    .map_err(ApiError::auth_repo)?;
    pacs_db::replace_user_device_grants(
        &state.pool,
        identity.institution_id,
        user.id,
        &req.device_ids,
        identity.user_id,
    )
    .await
    .map_err(ApiError::db)?;
    audit(
        &state,
        &identity,
        pacs_auth::audit::Action::UserCreated,
        serde_json::json!({"user_id":user.id,"role":user.role.as_str()}),
    )
    .await;
    Ok((StatusCode::CREATED, Json(user)))
}

#[derive(Deserialize)]
struct UpdateUserRequest {
    display_name: Option<String>,
    role: Option<Role>,
    is_active: Option<bool>,
}

async fn update_user(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(user_id): Path<i64>,
    Json(req): Json<UpdateUserRequest>,
) -> Result<Json<pacs_auth::User>, ApiError> {
    let current = pacs_auth::repository::find_by_id(&state.pool, user_id)
        .await
        .map_err(ApiError::auth_repo)?
        .filter(|user| user.institution_id == identity.institution_id)
        .ok_or_else(ApiError::not_found)?;
    let removes_admin = current.role == Role::Admin
        && (req.role.is_some_and(|role| role != Role::Admin) || req.is_active == Some(false));
    if removes_admin {
        let active_admins: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM users WHERE institution_id=$1 AND role='admin' AND is_active",
        )
        .bind(identity.institution_id)
        .fetch_one(&state.pool)
        .await
        .map_err(|error| {
            tracing::error!(%error, "统计管理员账号失败");
            ApiError::internal()
        })?;
        if active_admins <= 1 {
            return Err(ApiError {
                status: StatusCode::CONFLICT,
                code: "last_admin",
                message: "不能停用或降级机构内最后一个管理员".to_owned(),
            });
        }
    }
    sqlx::query(
        r#"UPDATE users SET display_name=COALESCE($3,display_name),
                  role=COALESCE($4,role),is_active=COALESCE($5,is_active)
           WHERE id=$1 AND institution_id=$2"#,
    )
    .bind(user_id)
    .bind(identity.institution_id)
    .bind(req.display_name.as_deref())
    .bind(req.role.map(Role::as_str))
    .bind(req.is_active)
    .execute(&state.pool)
    .await
    .map_err(|error| {
        tracing::error!(%error, "更新账号失败");
        ApiError::internal()
    })?;
    if req.is_active == Some(false) {
        pacs_auth::repository::revoke_all_for_user(&state.pool, user_id)
            .await
            .map_err(ApiError::auth_repo)?;
    }
    let updated = pacs_auth::repository::find_by_id(&state.pool, user_id)
        .await
        .map_err(ApiError::auth_repo)?
        .ok_or_else(ApiError::not_found)?;
    audit(
        &state,
        &identity,
        pacs_auth::audit::Action::UserModified,
        serde_json::json!({"user_id":user_id,"role":updated.role.as_str(),"is_active":updated.is_active}),
    )
    .await;
    Ok(Json(updated))
}

async fn user_grants(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(user_id): Path<i64>,
) -> Result<Json<Vec<Uuid>>, ApiError> {
    Ok(Json(
        pacs_db::user_device_grants(&state.pool, identity.institution_id, user_id)
            .await
            .map_err(ApiError::db)?,
    ))
}

#[derive(Deserialize)]
struct GrantsRequest {
    device_ids: Vec<Uuid>,
}
async fn replace_user_grants(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(user_id): Path<i64>,
    Json(req): Json<GrantsRequest>,
) -> Result<Json<Vec<Uuid>>, ApiError> {
    let grants = pacs_db::replace_user_device_grants(
        &state.pool,
        identity.institution_id,
        user_id,
        &req.device_ids,
        identity.user_id,
    )
    .await
    .map_err(ApiError::db)?;
    audit(
        &state,
        &identity,
        pacs_auth::audit::Action::DeviceGrantChanged,
        serde_json::json!({"user_id":user_id,"device_ids":grants}),
    )
    .await;
    Ok(Json(grants))
}

#[derive(Deserialize)]
struct PasswordRequest {
    temporary_password: String,
}
async fn reset_password(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(user_id): Path<i64>,
    Json(req): Json<PasswordRequest>,
) -> Result<StatusCode, ApiError> {
    let user = pacs_auth::repository::find_by_id(&state.pool, user_id)
        .await
        .map_err(ApiError::auth_repo)?
        .filter(|u| u.institution_id == identity.institution_id)
        .ok_or_else(ApiError::not_found)?;
    pacs_auth::password::check_strength(&req.temporary_password, &user.username)
        .map_err(|e| ApiError::bad("weak_password", e.to_string()))?;
    let hash =
        pacs_auth::password::hash(&req.temporary_password).map_err(|_| ApiError::internal())?;
    pacs_auth::repository::set_password(&state.pool, user_id, &hash, true)
        .await
        .map_err(ApiError::auth_repo)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn revoke_sessions(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(user_id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let user = pacs_auth::repository::find_by_id(&state.pool, user_id)
        .await
        .map_err(ApiError::auth_repo)?
        .filter(|u| u.institution_id == identity.institution_id)
        .ok_or_else(ApiError::not_found)?;
    pacs_auth::repository::revoke_all_for_user(&state.pool, user.id)
        .await
        .map_err(ApiError::auth_repo)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct DeviceQuery {
    status: Option<String>,
}
async fn devices(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Query(q): Query<DeviceQuery>,
) -> Result<Json<Vec<pacs_db::DicomDevice>>, ApiError> {
    Ok(Json(
        pacs_db::list_devices(&state.pool, identity.institution_id, q.status.as_deref())
            .await
            .map_err(ApiError::db)?,
    ))
}

#[derive(Deserialize)]
struct ApproveDeviceRequest {
    name: String,
    modality_hint: Option<String>,
}

#[derive(Deserialize)]
struct RegisterDeviceRequest {
    name: String,
    calling_ae_title: String,
    source_ip: String,
    modality_hint: Option<String>,
}
async fn register_device(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Json(req): Json<RegisterDeviceRequest>,
) -> Result<(StatusCode, Json<pacs_db::DicomDevice>), ApiError> {
    if req.name.trim().is_empty()
        || req.calling_ae_title.trim().is_empty()
        || req.source_ip.trim().is_empty()
    {
        return Err(ApiError::bad(
            "invalid_device_fields",
            "设备名称、AE Title 与来源 IP 不能为空",
        ));
    }
    let device = pacs_db::register_device(
        &state.pool,
        identity.institution_id,
        &req.name,
        &req.calling_ae_title,
        &req.source_ip,
        req.modality_hint.as_deref(),
    )
    .await
    .map_err(ApiError::db)?;
    audit(
        &state,
        &identity,
        pacs_auth::audit::Action::DeviceRegistered,
        serde_json::json!({
            "device_id":device.id,
            "calling_ae_title":device.calling_ae_title,
            "source_ip":device.source_ip,
        }),
    )
    .await;
    Ok((StatusCode::CREATED, Json(device)))
}

#[derive(Deserialize)]
struct SeriesSourcesQuery {
    unattributed: Option<bool>,
    limit: Option<i64>,
    offset: Option<i64>,
}
async fn series_sources(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Query(q): Query<SeriesSourcesQuery>,
) -> Result<Json<Vec<pacs_db::SeriesSourceEntry>>, ApiError> {
    Ok(Json(
        pacs_db::list_series_sources(
            &state.pool,
            identity.institution_id,
            q.unattributed.unwrap_or(true),
            q.limit.unwrap_or(100),
            q.offset.unwrap_or(0),
        )
        .await
        .map_err(ApiError::db)?,
    ))
}

async fn approve_device(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(device_id): Path<Uuid>,
    Json(req): Json<ApproveDeviceRequest>,
) -> Result<Json<pacs_db::DicomDevice>, ApiError> {
    if req.name.trim().is_empty() {
        return Err(ApiError::bad("invalid_device_name", "设备名称不能为空"));
    }
    let device = pacs_db::approve_device(
        &state.pool,
        identity.institution_id,
        device_id,
        pacs_db::ApproveDevice {
            name: &req.name,
            modality_hint: req.modality_hint.as_deref(),
        },
        identity.user_id,
    )
    .await
    .map_err(ApiError::db)?;
    audit(&state, &identity, pacs_auth::audit::Action::DeviceApproved,
        serde_json::json!({"device_id":device.id,"calling_ae_title":device.calling_ae_title,"source_ip":device.source_ip})).await;
    Ok(Json(device))
}

#[derive(Deserialize)]
struct DeviceStatusRequest {
    status: String,
}
async fn update_device_status(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(device_id): Path<Uuid>,
    Json(req): Json<DeviceStatusRequest>,
) -> Result<StatusCode, ApiError> {
    if !matches!(req.status.as_str(), "active" | "disabled") {
        return Err(ApiError::bad(
            "invalid_device_status",
            "状态必须是 active 或 disabled",
        ));
    }
    pacs_db::set_device_status(&state.pool, identity.institution_id, device_id, &req.status)
        .await
        .map_err(ApiError::db)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct ResolveSourceRequest {
    device_id: Uuid,
}
async fn resolve_source(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(series_uid): Path<String>,
    Json(req): Json<ResolveSourceRequest>,
) -> Result<StatusCode, ApiError> {
    pacs_core::Uid::parse(&series_uid)
        .map_err(|_| ApiError::bad("invalid_uid", "Series UID 无效"))?;
    pacs_db::resolve_series_source(
        &state.pool,
        identity.institution_id,
        &series_uid,
        req.device_id,
    )
    .await
    .map_err(ApiError::db)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct WorklistQuery {
    date: Option<NaiveDate>,
    status: Option<String>,
}
async fn worklist(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Query(q): Query<WorklistQuery>,
) -> Result<Json<Vec<pacs_db::ClinicalWorkItem>>, ApiError> {
    let date = match q.date {
        Some(date) => date,
        None => pacs_db::institution_today(&state.pool, identity.institution_id)
            .await
            .map_err(ApiError::db)?,
    };
    Ok(Json(
        pacs_db::list_clinical_work(
            &state.pool,
            identity.institution_id,
            identity.user_id,
            identity.role == Role::Admin,
            date,
            q.status.as_deref(),
        )
        .await
        .map_err(ApiError::db)?,
    ))
}

/// 按序列查工作项（报告面板用）：不受工作列表「仅当天」的日期过滤限制，
/// 历史入库的序列也能领取并撰写报告。
async fn work_item_by_series(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(series_uid): Path<String>,
) -> Result<Json<pacs_db::ClinicalWorkItem>, ApiError> {
    pacs_core::Uid::parse(&series_uid)
        .map_err(|_| ApiError::bad("invalid_uid", "Series UID 无效"))?;
    let item = pacs_db::work_item_for_series(
        &state.pool,
        identity.institution_id,
        identity.user_id,
        identity.role == Role::Admin,
        &series_uid,
    )
    .await
    .map_err(ApiError::db)?
    .ok_or(ApiError::not_found())?;
    Ok(Json(item))
}

#[derive(Serialize, sqlx::FromRow)]
struct ClinicalStudyHeader {
    patient_id: String,
    patient_name: Option<String>,
    study_uid: String,
    study_date: Option<NaiveDate>,
    description: Option<String>,
    hidden_series_count: i64,
}

#[derive(Serialize)]
struct ClinicalContext {
    #[serde(flatten)]
    header: ClinicalStudyHeader,
    access_incomplete: bool,
    series: Vec<pacs_db::SeriesSummary>,
    reports: Vec<pacs_db::DiagnosticReport>,
}

async fn clinical_context(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(study_uid): Path<String>,
) -> Result<Json<ClinicalContext>, ApiError> {
    pacs_core::Uid::parse(&study_uid)
        .map_err(|_| ApiError::bad("invalid_uid", "Study UID 无效"))?;
    let is_admin = identity.role == Role::Admin;
    if !pacs_db::can_access_study(
        &state.pool,
        identity.institution_id,
        identity.user_id,
        is_admin,
        &study_uid,
    )
    .await
    .map_err(ApiError::db)?
    {
        return Err(ApiError::not_found());
    }
    let series = pacs_db::list_study_series(
        &state.pool,
        identity.institution_id,
        identity.user_id,
        is_admin,
        &study_uid,
    )
    .await
    .map_err(ApiError::db)?;
    let visible = series.len() as i64;
    let header: ClinicalStudyHeader = sqlx::query_as(
        r#"SELECT p.patient_id,p.name patient_name,st.study_instance_uid study_uid,
                  st.study_date,st.description,
                  GREATEST(count(se.id)-$3,0)::BIGINT hidden_series_count
           FROM studies st JOIN patients p ON p.id=st.patient_fk
           LEFT JOIN series se ON se.study_fk=st.id
           WHERE st.institution_id=$1 AND st.study_instance_uid=$2
           GROUP BY p.id,st.id"#,
    )
    .bind(identity.institution_id)
    .bind(&study_uid)
    .bind(visible)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!(%e);
        ApiError::internal()
    })?
    .ok_or_else(ApiError::not_found)?;
    let reports = pacs_db::list_reports(
        &state.pool,
        identity.institution_id,
        identity.user_id,
        is_admin,
        &study_uid,
    )
    .await
    .map_err(ApiError::db)?;
    Ok(Json(ClinicalContext {
        access_incomplete: header.hidden_series_count > 0,
        header,
        series,
        reports,
    }))
}

#[derive(Deserialize)]
struct RevisionRequest {
    revision: i32,
}
async fn claim(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(work_id): Path<Uuid>,
    Json(req): Json<RevisionRequest>,
) -> Result<StatusCode, ApiError> {
    if identity.role != Role::Radiologist {
        return Err(ApiError::forbidden("radiologist_required"));
    }
    pacs_db::claim_work_item(
        &state.pool,
        identity.institution_id,
        work_id,
        identity.user_id,
        req.revision,
    )
    .await
    .map_err(ApiError::db)?;
    audit(
        &state,
        &identity,
        pacs_auth::audit::Action::WorkItemClaimed,
        serde_json::json!({"work_item_id":work_id}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

async fn release(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(work_id): Path<Uuid>,
    Json(req): Json<RevisionRequest>,
) -> Result<StatusCode, ApiError> {
    pacs_db::release_work_item(
        &state.pool,
        identity.institution_id,
        work_id,
        identity.user_id,
        identity.role == Role::Admin,
        req.revision,
    )
    .await
    .map_err(ApiError::db)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct AssignRequest {
    doctor_id: i64,
    revision: i32,
}
async fn assign(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(work_id): Path<Uuid>,
    Json(req): Json<AssignRequest>,
) -> Result<StatusCode, ApiError> {
    pacs_db::assign_work_item(
        &state.pool,
        identity.institution_id,
        work_id,
        req.doctor_id,
        req.revision,
    )
    .await
    .map_err(ApiError::db)?;
    audit(
        &state,
        &identity,
        pacs_auth::audit::Action::WorkItemAssigned,
        serde_json::json!({"work_item_id":work_id,"doctor_id":req.doctor_id}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct CreateReportRequest {
    study_uid: String,
    covered_series_uids: Vec<String>,
    template_payload: Option<serde_json::Value>,
}
#[derive(Deserialize)]
struct ReportsQuery {
    study_uid: String,
}
#[derive(Deserialize)]
struct TemplatesQuery {
    modality: Option<String>,
}

/// 结构化模板快照大小上限（1 MiB），超限直接 400。
const MAX_TEMPLATE_PAYLOAD_BYTES: usize = 1_048_576;

fn validate_template_payload(payload: &Option<serde_json::Value>) -> Result<(), ApiError> {
    let Some(payload) = payload else {
        return Ok(());
    };
    if serde_json::to_vec(payload).map(|v| v.len()).unwrap_or(usize::MAX) > MAX_TEMPLATE_PAYLOAD_BYTES
    {
        return Err(ApiError::bad(
            "template_payload_too_large",
            "template_payload 超过 1 MiB 上限",
        ));
    }
    Ok(())
}

async fn report_templates(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Query(q): Query<TemplatesQuery>,
) -> Result<Json<Vec<pacs_db::ReportTemplate>>, ApiError> {
    Ok(Json(
        pacs_db::list_report_templates(&state.pool, identity.institution_id, q.modality.as_deref())
            .await
            .map_err(ApiError::db)?,
    ))
}
async fn list_reports(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Query(q): Query<ReportsQuery>,
) -> Result<Json<Vec<pacs_db::DiagnosticReport>>, ApiError> {
    Ok(Json(
        pacs_db::list_reports(
            &state.pool,
            identity.institution_id,
            identity.user_id,
            identity.role == Role::Admin,
            &q.study_uid,
        )
        .await
        .map_err(ApiError::db)?,
    ))
}

async fn create_report(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Json(req): Json<CreateReportRequest>,
) -> Result<(StatusCode, Json<pacs_db::DiagnosticReport>), ApiError> {
    if identity.role != Role::Radiologist {
        return Err(ApiError::forbidden("radiologist_required"));
    }
    validate_template_payload(&req.template_payload)?;
    let report = pacs_db::create_report(
        &state.pool,
        identity.institution_id,
        identity.user_id,
        &req.study_uid,
        &req.covered_series_uids,
        req.template_payload,
    )
    .await
    .map_err(ApiError::db)?;
    audit(
        &state,
        &identity,
        pacs_auth::audit::Action::ReportDrafted,
        serde_json::json!({"report_id":report.id,"study_uid":req.study_uid}),
    )
    .await;
    Ok((StatusCode::CREATED, Json(report)))
}

#[derive(Deserialize)]
struct AmendmentRequest {
    reason: String,
}
async fn begin_amendment(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(report_id): Path<Uuid>,
    Json(req): Json<AmendmentRequest>,
) -> Result<Json<pacs_db::DiagnosticReport>, ApiError> {
    if identity.role != Role::Radiologist {
        return Err(ApiError::forbidden("radiologist_required"));
    }
    let report = pacs_db::begin_report_amendment(
        &state.pool,
        identity.institution_id,
        report_id,
        identity.user_id,
        &req.reason,
    )
    .await
    .map_err(ApiError::db)?;
    audit(
        &state,
        &identity,
        pacs_auth::audit::Action::ReportAmendmentStarted,
        serde_json::json!({"report_id":report_id,"reason":req.reason}),
    )
    .await;
    Ok(Json(report))
}

async fn report_versions(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(report_id): Path<Uuid>,
) -> Result<Json<Vec<pacs_db::ReportVersion>>, ApiError> {
    // Visibility is established by the report list/resource relationship; administrators can
    // always inspect institution history, while non-admin callers need at least report access.
    let versions = pacs_db::list_report_versions(
        &state.pool,
        identity.institution_id,
        report_id,
        identity.user_id,
        identity.role == Role::Admin,
    )
    .await
    .map_err(ApiError::db)?;
    Ok(Json(versions))
}

#[derive(Deserialize)]
struct DraftRequest {
    revision: i32,
    findings: String,
    impression: String,
    recommendation: Option<String>,
    template_payload: Option<serde_json::Value>,
}
async fn update_draft(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(report_id): Path<Uuid>,
    Json(req): Json<DraftRequest>,
) -> Result<Json<pacs_db::DiagnosticReport>, ApiError> {
    if identity.role != Role::Radiologist {
        return Err(ApiError::forbidden("radiologist_required"));
    }
    validate_template_payload(&req.template_payload)?;
    Ok(Json(
        pacs_db::update_report_draft(
            &state.pool,
            identity.institution_id,
            report_id,
            identity.user_id,
            req.revision,
            &req.findings,
            &req.impression,
            req.recommendation.as_deref(),
            req.template_payload,
        )
        .await
        .map_err(ApiError::db)?,
    ))
}

async fn sign_report(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(report_id): Path<Uuid>,
    Json(req): Json<RevisionRequest>,
) -> Result<StatusCode, ApiError> {
    if identity.role != Role::Radiologist {
        return Err(ApiError::forbidden("radiologist_required"));
    }
    pacs_db::sign_report(
        &state.pool,
        identity.institution_id,
        report_id,
        identity.user_id,
        req.revision,
    )
    .await
    .map_err(ApiError::db)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn audit(
    state: &WebState,
    identity: &Identity,
    action: pacs_auth::audit::Action,
    detail: serde_json::Value,
) {
    pacs_auth::audit::record(
        &state.pool,
        action,
        pacs_auth::audit::Outcome::Success,
        pacs_auth::audit::Entry::for_user(identity.user_id, &identity.username, identity.role)
            .with_detail(detail),
    )
    .await;
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
    fn forbidden(code: &'static str) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code,
            message: "权限不足".to_owned(),
        }
    }
    fn not_found() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "resource_not_found",
            message: "资源不存在".to_owned(),
        }
    }
    fn internal() -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal_error",
            message: "内部错误".to_owned(),
        }
    }
    fn auth_repo(error: pacs_auth::repository::RepoError) -> Self {
        match error {
            pacs_auth::repository::RepoError::UsernameTaken { .. } => Self {
                status: StatusCode::CONFLICT,
                code: "username_taken",
                message: error.to_string(),
            },
            _ => {
                tracing::error!(%error, "账号 API 数据库错误");
                Self::internal()
            }
        }
    }
    fn db(error: pacs_db::DbError) -> Self {
        match error {
            pacs_db::DbError::NotFound => Self::not_found(),
            pacs_db::DbError::Conflict(message) => Self {
                status: StatusCode::CONFLICT,
                code: "state_conflict",
                message,
            },
            pacs_db::DbError::Invalid(message) => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "validation_failed",
                message,
            },
            other => {
                tracing::error!(%other, "临床 API 数据库错误");
                Self::internal()
            }
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
