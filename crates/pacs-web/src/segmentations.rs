use std::sync::Arc;

use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, put};
use axum::{Json, Router};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use pacs_auth::audit::{Action, Entry, Outcome, record as record_audit};
use pacs_auth::{AuthService, Identity, Permission};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::routes::WebState;

pub fn segmentation_routes(state: WebState, auth: Arc<AuthService>) -> Router {
    Router::new()
        .route(
            "/studies/{study_uid}/series/{series_uid}/segmentations",
            get(list_projects).post(create_project),
        )
        .route(
            "/studies/{study_uid}/series/{series_uid}/segmentations/{project_id}",
            delete(delete_project),
        )
        .route(
            "/studies/{study_uid}/series/{series_uid}/segmentations/{project_id}/segments",
            get(list_segments),
        )
        .route(
            "/studies/{study_uid}/series/{series_uid}/segmentations/{project_id}/segments/{segment_id}",
            axum::routing::patch(update_segment),
        )
        .route(
            "/studies/{study_uid}/series/{series_uid}/segmentations/{project_id}/masks",
            get(list_masks),
        )
        .route(
            "/studies/{study_uid}/series/{series_uid}/segmentations/{project_id}/segments/{segment_id}/mask",
            put(upsert_mask),
        )
        .route(
            "/studies/{study_uid}/series/{series_uid}/segmentations/{project_id}/segments/{segment_id}/masks",
            get(list_segment_masks).put(upsert_masks_batch),
        )
        .with_state(state)
        .layer(axum::middleware::from_fn(move |request, next| {
            let auth = Arc::clone(&auth);
            async move { pacs_auth::require(auth, Permission::ViewImages, request, next).await }
        }))
}

#[derive(Deserialize)]
struct SeriesPath {
    study_uid: String,
    series_uid: String,
}

#[derive(Deserialize)]
struct ProjectPath {
    study_uid: String,
    series_uid: String,
    project_id: Uuid,
}

#[derive(Deserialize)]
struct SegmentPath {
    study_uid: String,
    series_uid: String,
    project_id: Uuid,
    segment_id: Uuid,
}

#[derive(Deserialize)]
struct CreateProjectRequest {
    id: Uuid,
    segment_id: Uuid,
    name: String,
    segment_label: String,
    segment_description: Option<String>,
    color: [i16; 3],
    #[serde(default = "default_algorithm_type")]
    algorithm_type: String,
    #[serde(default)]
    tags: Vec<String>,
}

fn default_algorithm_type() -> String {
    "manual".to_owned()
}

#[derive(Serialize)]
struct CreatedProject {
    project: pacs_db::SegmentationProject,
    segment: pacs_db::SegmentationSegment,
}

#[derive(Deserialize)]
struct MaskQuery {
    sop_instance_uid: String,
    frame_number: i32,
}

#[derive(Default, Deserialize)]
struct SegmentQuery {
    tag: Option<String>,
}

#[derive(Deserialize)]
struct UpdateSegmentRequest {
    tags: Vec<String>,
}

#[derive(Deserialize)]
struct UpsertMaskRequest {
    sop_instance_uid: String,
    frame_number: i32,
    rows: i32,
    cols: i32,
    encoding: String,
    data_base64: String,
    expected_revision: i64,
}

#[derive(Deserialize)]
struct UpsertMasksRequest {
    updates: Vec<UpsertMaskRequest>,
}

struct DecodedMaskUpdate {
    sop_instance_uid: String,
    frame_number: i32,
    rows: i32,
    cols: i32,
    mask_data: Vec<u8>,
    expected_revision: i64,
}

#[derive(Serialize)]
struct MaskResponse {
    segment_id: Uuid,
    sop_instance_uid: String,
    frame_number: i32,
    rows: i32,
    cols: i32,
    encoding: String,
    data_base64: String,
    revision: i64,
    modified_by: Option<i64>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<pacs_db::SegmentationMask> for MaskResponse {
    fn from(value: pacs_db::SegmentationMask) -> Self {
        Self {
            segment_id: value.segment_id,
            sop_instance_uid: value.sop_instance_uid,
            frame_number: value.frame_number,
            rows: value.rows,
            cols: value.cols,
            encoding: value.encoding,
            data_base64: STANDARD.encode(value.mask_data),
            revision: value.revision,
            modified_by: value.modified_by,
            updated_at: value.updated_at,
        }
    }
}

async fn list_projects(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(path): Path<SeriesPath>,
) -> Result<Json<Vec<pacs_db::SegmentationProject>>, SegmentationError> {
    validate_series(&path.study_uid, &path.series_uid)?;
    authorize(&state, &identity, &path.series_uid).await?;
    Ok(Json(
        pacs_db::list_segmentation_projects(
            &state.pool,
            identity.institution_id,
            &path.study_uid,
            &path.series_uid,
        )
        .await
        .map_err(SegmentationError::db)?,
    ))
}

async fn create_project(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(path): Path<SeriesPath>,
    Json(request): Json<CreateProjectRequest>,
) -> Result<(StatusCode, Json<CreatedProject>), SegmentationError> {
    validate_series(&path.study_uid, &path.series_uid)?;
    authorize(&state, &identity, &path.series_uid).await?;
    let name = request.name.trim();
    let label = request.segment_label.trim();
    let description = request
        .segment_description
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if name.is_empty()
        || name.chars().count() > 120
        || label.is_empty()
        || label.chars().count() > 120
    {
        return Err(SegmentationError::BadRequest(
            "分割项目名称和 Segment Label 必须为 1–120 个字符".to_owned(),
        ));
    }
    if request
        .color
        .iter()
        .any(|component| !(0..=255).contains(component))
    {
        return Err(SegmentationError::BadRequest(
            "显示颜色分量必须为 0–255".to_owned(),
        ));
    }
    if description.is_some_and(|value| value.chars().count() > 1_024) {
        return Err(SegmentationError::BadRequest(
            "Segment 描述不能超过 1024 个字符".to_owned(),
        ));
    }
    if !matches!(
        request.algorithm_type.as_str(),
        "manual" | "semiautomatic" | "automatic"
    ) {
        return Err(SegmentationError::BadRequest(
            "Segment 算法类型无效".to_owned(),
        ));
    }
    let tags = normalize_tags(request.tags)?;
    let (project, segment) = pacs_db::create_segmentation_project(
        &state.pool,
        pacs_db::NewSegmentationProject {
            id: request.id,
            segment_id: request.segment_id,
            institution_id: identity.institution_id,
            study_instance_uid: &path.study_uid,
            series_instance_uid: &path.series_uid,
            name,
            segment_label: label,
            segment_description: description,
            color: request.color,
            algorithm_type: &request.algorithm_type,
            tags: &tags,
            user_id: identity.user_id,
        },
    )
    .await
    .map_err(SegmentationError::db)?;
    audit(
        &state,
        &identity,
        &path,
        Action::SegmentationCreated,
        serde_json::json!({
            "project_id": project.id,
            "segment_id": segment.id,
            "algorithm_type": segment.algorithm_type,
        }),
        None,
    )
    .await;
    Ok((
        StatusCode::CREATED,
        Json(CreatedProject { project, segment }),
    ))
}

async fn delete_project(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(path): Path<ProjectPath>,
) -> Result<StatusCode, SegmentationError> {
    validate_series(&path.study_uid, &path.series_uid)?;
    authorize(&state, &identity, &path.series_uid).await?;
    let deleted = pacs_db::delete_segmentation_project(
        &state.pool,
        identity.institution_id,
        &path.study_uid,
        &path.series_uid,
        path.project_id,
    )
    .await
    .map_err(SegmentationError::db)?;
    if !deleted {
        return Err(SegmentationError::NotFound);
    }
    audit(
        &state,
        &identity,
        &SeriesPath {
            study_uid: path.study_uid,
            series_uid: path.series_uid,
        },
        Action::SegmentationDeleted,
        serde_json::json!({ "project_id": path.project_id }),
        None,
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_segments(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(path): Path<ProjectPath>,
    Query(query): Query<SegmentQuery>,
) -> Result<Json<Vec<pacs_db::SegmentationSegment>>, SegmentationError> {
    validate_series(&path.study_uid, &path.series_uid)?;
    authorize(&state, &identity, &path.series_uid).await?;
    let tag = query
        .tag
        .as_deref()
        .map(str::trim)
        .filter(|tag| !tag.is_empty());
    if tag.is_some_and(|tag| tag.chars().count() > 40) {
        return Err(SegmentationError::BadRequest(
            "查询 Tag 不能超过 40 个字符".to_owned(),
        ));
    }
    let segments = if let Some(tag) = tag {
        pacs_db::find_segmentation_segments_by_tag(
            &state.pool,
            identity.institution_id,
            path.project_id,
            tag,
        )
        .await
    } else {
        pacs_db::list_segmentation_segments(&state.pool, identity.institution_id, path.project_id)
            .await
    };
    Ok(Json(segments.map_err(SegmentationError::db)?))
}

async fn update_segment(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(path): Path<SegmentPath>,
    Json(request): Json<UpdateSegmentRequest>,
) -> Result<Json<pacs_db::SegmentationSegment>, SegmentationError> {
    validate_series(&path.study_uid, &path.series_uid)?;
    authorize(&state, &identity, &path.series_uid).await?;
    let tags = normalize_tags(request.tags)?;
    let segment = pacs_db::update_segmentation_segment_tags(
        &state.pool,
        pacs_db::UpdateSegmentationSegmentTags {
            institution_id: identity.institution_id,
            project_id: path.project_id,
            segment_id: path.segment_id,
            tags: &tags,
            user_id: identity.user_id,
        },
    )
    .await
    .map_err(SegmentationError::db)?;
    audit(
        &state,
        &identity,
        &SeriesPath {
            study_uid: path.study_uid,
            series_uid: path.series_uid,
        },
        Action::SegmentationTagsUpdated,
        serde_json::json!({
            "project_id": path.project_id,
            "segment_id": path.segment_id,
            "tags": segment.tags,
        }),
        None,
    )
    .await;
    Ok(Json(segment))
}

fn normalize_tags(tags: Vec<String>) -> Result<Vec<String>, SegmentationError> {
    let mut normalized = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for tag in tags {
        let tag = tag.trim();
        if tag.is_empty() || !seen.insert(tag.to_owned()) {
            continue;
        }
        if tag.chars().count() > 40 {
            return Err(SegmentationError::BadRequest(
                "单个 Tag 不能超过 40 个字符".to_owned(),
            ));
        }
        normalized.push(tag.to_owned());
    }
    if normalized.len() > 16 {
        return Err(SegmentationError::BadRequest(
            "一个 Mask 最多设置 16 个 Tag".to_owned(),
        ));
    }
    Ok(normalized)
}

async fn list_masks(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(path): Path<ProjectPath>,
    Query(query): Query<MaskQuery>,
) -> Result<Json<Vec<MaskResponse>>, SegmentationError> {
    validate_series(&path.study_uid, &path.series_uid)?;
    authorize(&state, &identity, &path.series_uid).await?;
    validate_mask_target(&query.sop_instance_uid, query.frame_number, 1, 1)?;
    let masks = pacs_db::list_segmentation_masks(
        &state.pool,
        identity.institution_id,
        path.project_id,
        &query.sop_instance_uid,
        query.frame_number,
    )
    .await
    .map_err(SegmentationError::db)?;
    Ok(Json(masks.into_iter().map(MaskResponse::from).collect()))
}

async fn upsert_mask(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(path): Path<SegmentPath>,
    Json(request): Json<UpsertMaskRequest>,
) -> Result<Json<MaskResponse>, SegmentationError> {
    validate_series(&path.study_uid, &path.series_uid)?;
    authorize(&state, &identity, &path.series_uid).await?;
    validate_mask_target(
        &request.sop_instance_uid,
        request.frame_number,
        request.rows,
        request.cols,
    )?;
    if request.encoding != "rle-v1" || request.expected_revision < 0 {
        return Err(SegmentationError::BadRequest(
            "只支持 rle-v1，expected_revision 不能为负数".to_owned(),
        ));
    }
    let data = STANDARD
        .decode(request.data_base64.as_bytes())
        .map_err(|_| SegmentationError::BadRequest("Mask Base64 无效".to_owned()))?;
    validate_rle(&data, request.rows as usize * request.cols as usize)?;
    let mask = pacs_db::upsert_segmentation_mask(
        &state.pool,
        pacs_db::UpsertSegmentationMask {
            institution_id: identity.institution_id,
            project_id: path.project_id,
            segment_id: path.segment_id,
            sop_instance_uid: &request.sop_instance_uid,
            frame_number: request.frame_number,
            rows: request.rows,
            cols: request.cols,
            mask_data: &data,
            expected_revision: request.expected_revision,
            user_id: identity.user_id,
        },
    )
    .await
    .map_err(SegmentationError::db)?;
    audit(
        &state,
        &identity,
        &SeriesPath {
            study_uid: path.study_uid,
            series_uid: path.series_uid,
        },
        Action::SegmentationMaskUpdated,
        serde_json::json!({
            "project_id": path.project_id,
            "segment_id": path.segment_id,
            "revision": mask.revision,
            "rows": mask.rows,
            "cols": mask.cols,
        }),
        Some(mask.sop_instance_uid.clone()),
    )
    .await;
    Ok(Json(mask.into()))
}

async fn list_segment_masks(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(path): Path<SegmentPath>,
) -> Result<Json<Vec<MaskResponse>>, SegmentationError> {
    validate_series(&path.study_uid, &path.series_uid)?;
    authorize(&state, &identity, &path.series_uid).await?;
    let masks = pacs_db::list_segmentation_segment_masks(
        &state.pool,
        identity.institution_id,
        path.project_id,
        path.segment_id,
    )
    .await
    .map_err(SegmentationError::db)?;
    Ok(Json(masks.into_iter().map(MaskResponse::from).collect()))
}

async fn upsert_masks_batch(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(path): Path<SegmentPath>,
    Json(request): Json<UpsertMasksRequest>,
) -> Result<Json<Vec<MaskResponse>>, SegmentationError> {
    validate_series(&path.study_uid, &path.series_uid)?;
    authorize(&state, &identity, &path.series_uid).await?;
    if request.updates.is_empty() || request.updates.len() > 2048 {
        return Err(SegmentationError::BadRequest(
            "一次 Mask 批量更新必须包含 1–2048 个来源层".to_owned(),
        ));
    }
    let mut seen = std::collections::HashSet::new();
    let mut decoded = Vec::with_capacity(request.updates.len());
    for update in request.updates {
        validate_mask_target(
            &update.sop_instance_uid,
            update.frame_number,
            update.rows,
            update.cols,
        )?;
        if update.encoding != "rle-v1" || update.expected_revision < 0 {
            return Err(SegmentationError::BadRequest(
                "只支持 rle-v1，expected_revision 不能为负数".to_owned(),
            ));
        }
        if !seen.insert((update.sop_instance_uid.clone(), update.frame_number)) {
            return Err(SegmentationError::BadRequest(
                "批量更新包含重复来源层".to_owned(),
            ));
        }
        let data = STANDARD
            .decode(update.data_base64.as_bytes())
            .map_err(|_| SegmentationError::BadRequest("Mask Base64 无效".to_owned()))?;
        validate_rle(&data, update.rows as usize * update.cols as usize)?;
        decoded.push(DecodedMaskUpdate {
            sop_instance_uid: update.sop_instance_uid,
            frame_number: update.frame_number,
            rows: update.rows,
            cols: update.cols,
            mask_data: data,
            expected_revision: update.expected_revision,
        });
    }
    let inputs = decoded
        .iter()
        .map(|update| pacs_db::SegmentationMaskUpdate {
            sop_instance_uid: &update.sop_instance_uid,
            frame_number: update.frame_number,
            rows: update.rows,
            cols: update.cols,
            mask_data: &update.mask_data,
            expected_revision: update.expected_revision,
        })
        .collect::<Vec<_>>();
    let records = pacs_db::upsert_segmentation_masks_batch(
        &state.pool,
        identity.institution_id,
        path.project_id,
        path.segment_id,
        &inputs,
        identity.user_id,
    )
    .await
    .map_err(SegmentationError::db)?;
    audit(
        &state,
        &identity,
        &SeriesPath {
            study_uid: path.study_uid,
            series_uid: path.series_uid,
        },
        Action::SegmentationMaskUpdated,
        serde_json::json!({
            "project_id": path.project_id,
            "segment_id": path.segment_id,
            "updated_slices": records.len(),
        }),
        None,
    )
    .await;
    Ok(Json(records.into_iter().map(MaskResponse::from).collect()))
}

fn validate_series(study_uid: &str, series_uid: &str) -> Result<(), SegmentationError> {
    pacs_core::Uid::parse(study_uid)
        .map_err(|_| SegmentationError::BadRequest("StudyInstanceUID 无效".to_owned()))?;
    pacs_core::Uid::parse(series_uid)
        .map_err(|_| SegmentationError::BadRequest("SeriesInstanceUID 无效".to_owned()))?;
    Ok(())
}

fn validate_mask_target(
    sop_uid: &str,
    frame_number: i32,
    rows: i32,
    cols: i32,
) -> Result<(), SegmentationError> {
    pacs_core::Uid::parse(sop_uid)
        .map_err(|_| SegmentationError::BadRequest("SOPInstanceUID 无效".to_owned()))?;
    if frame_number <= 0 || rows <= 0 || cols <= 0 || rows > 65_535 || cols > 65_535 {
        return Err(SegmentationError::BadRequest(
            "Mask 帧号或尺寸无效".to_owned(),
        ));
    }
    Ok(())
}

fn validate_rle(data: &[u8], pixel_count: usize) -> Result<(), SegmentationError> {
    if data.is_empty() || !data.len().is_multiple_of(4) || data.len() > 64 * 1024 * 1024 {
        return Err(SegmentationError::BadRequest(
            "Mask RLE 长度无效".to_owned(),
        ));
    }
    let mut total = 0usize;
    for (index, bytes) in data.chunks_exact(4).enumerate() {
        let run = u32::from_le_bytes(bytes.try_into().expect("四字节分块")) as usize;
        if run == 0 && index != 0 {
            return Err(SegmentationError::BadRequest(
                "Mask RLE 包含空游程".to_owned(),
            ));
        }
        total = total
            .checked_add(run)
            .ok_or_else(|| SegmentationError::BadRequest("Mask RLE 溢出".to_owned()))?;
        if total > pixel_count {
            return Err(SegmentationError::BadRequest(
                "Mask RLE 超出图像范围".to_owned(),
            ));
        }
    }
    if total != pixel_count {
        return Err(SegmentationError::BadRequest(
            "Mask RLE 像素总数与图像尺寸不符".to_owned(),
        ));
    }
    Ok(())
}

async fn audit(
    state: &WebState,
    identity: &Identity,
    path: &SeriesPath,
    action: Action,
    detail: serde_json::Value,
    sop_uid: Option<String>,
) {
    let mut entry = Entry::for_user(identity.user_id, &identity.username, identity.role)
        .with_study(&path.study_uid)
        .with_detail(detail);
    entry.series_instance_uid = Some(path.series_uid.clone());
    entry.sop_instance_uid = sop_uid;
    record_audit(&state.pool, action, Outcome::Success, entry).await;
}

async fn authorize(
    state: &WebState,
    identity: &Identity,
    series_uid: &str,
) -> Result<(), SegmentationError> {
    let allowed = pacs_db::can_access_series(
        &state.pool,
        identity.institution_id,
        identity.user_id,
        identity.role == pacs_auth::Role::Admin,
        series_uid,
    )
    .await
    .map_err(SegmentationError::db)?;
    if allowed {
        Ok(())
    } else {
        Err(SegmentationError::NotFound)
    }
}

#[derive(Debug, thiserror::Error)]
enum SegmentationError {
    #[error("{0}")]
    BadRequest(String),
    #[error("分割项目、Segment 或来源影像不存在")]
    NotFound,
    #[error("{0}")]
    Conflict(String),
    #[error("内部错误")]
    Internal,
}

impl SegmentationError {
    fn db(error: pacs_db::DbError) -> Self {
        match error {
            pacs_db::DbError::NotFound => Self::NotFound,
            pacs_db::DbError::Conflict(message) => Self::Conflict(message),
            other => {
                tracing::error!(%other, "分割数据库操作失败");
                Self::Internal
            }
        }
    }
}

impl IntoResponse for SegmentationError {
    fn into_response(self) -> Response {
        let status = match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let message = self.to_string();
        (status, Json(serde_json::json!({"error": message}))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_binary_rle_pixel_count() {
        let valid = [3_u32, 2, 3]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>();
        assert!(validate_rle(&valid, 8).is_ok());
        assert!(validate_rle(&valid, 9).is_err());
    }

    #[test]
    fn permits_initial_zero_run_for_masks_starting_inside() {
        let valid = [0_u32, 4]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>();
        assert!(validate_rle(&valid, 4).is_ok());
    }

    #[test]
    fn normalizes_and_validates_segment_tags() {
        let tags = normalize_tags(vec![
            " 结节 ".to_owned(),
            "肺".to_owned(),
            "结节".to_owned(),
            " ".to_owned(),
        ])
        .unwrap();
        assert_eq!(tags, ["结节", "肺"]);
        assert!(normalize_tags(vec!["x".repeat(41)]).is_err());
        assert!(normalize_tags((0..17).map(|value| value.to_string()).collect()).is_err());
    }
}
