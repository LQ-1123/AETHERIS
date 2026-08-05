use std::sync::Arc;

use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use pacs_auth::audit::{Action, Entry, Outcome, record as record_audit};
use pacs_auth::{AuthService, Identity, Permission};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use crate::routes::WebState;

pub fn annotation_routes(state: WebState, auth: Arc<AuthService>) -> Router {
    Router::new()
        .route(
            "/studies/{study_uid}/series/{series_uid}/annotations",
            get(list).post(create),
        )
        .route(
            "/studies/{study_uid}/series/{series_uid}/annotations/{annotation_id}",
            patch(update),
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
struct AnnotationPath {
    study_uid: String,
    series_uid: String,
    annotation_id: Uuid,
}

#[derive(Default, Deserialize)]
struct ListQuery {
    since: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
struct CreateAnnotation {
    id: Uuid,
    schema_version: i32,
    kind: String,
    coordinate_space: String,
    sop_instance_uid: Option<String>,
    frame_number: Option<i32>,
    mpr_plane: Option<String>,
    geometry: Value,
}

#[derive(Deserialize)]
struct UpdateAnnotation {
    expected_revision: i64,
    geometry: Value,
    #[serde(default)]
    deleted: bool,
}

async fn list(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(path): Path<SeriesPath>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<pacs_db::AnnotationRecord>>, AnnotationError> {
    validate_path(&path.study_uid, &path.series_uid)?;
    let records = pacs_db::list_annotations(
        &state.pool,
        identity.institution_id,
        &path.study_uid,
        &path.series_uid,
        query.since,
    )
    .await
    .map_err(AnnotationError::db)?;
    Ok(Json(records))
}

async fn create(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(path): Path<SeriesPath>,
    Json(request): Json<CreateAnnotation>,
) -> Result<(StatusCode, Json<pacs_db::AnnotationRecord>), AnnotationError> {
    validate_path(&path.study_uid, &path.series_uid)?;
    validate_annotation(
        request.schema_version,
        &request.kind,
        &request.coordinate_space,
        request.sop_instance_uid.as_deref(),
        request.frame_number,
        request.mpr_plane.as_deref(),
        &request.geometry,
    )?;
    let record = pacs_db::create_annotation(
        &state.pool,
        pacs_db::NewAnnotation {
            id: request.id,
            institution_id: identity.institution_id,
            study_instance_uid: &path.study_uid,
            series_instance_uid: &path.series_uid,
            sop_instance_uid: request.sop_instance_uid.as_deref(),
            frame_number: request.frame_number,
            coordinate_space: &request.coordinate_space,
            mpr_plane: request.mpr_plane.as_deref(),
            schema_version: request.schema_version,
            kind: &request.kind,
            geometry: &request.geometry,
            user_id: identity.user_id,
        },
    )
    .await
    .map_err(AnnotationError::db)?;
    audit(&state, &identity, &path, &record, Action::AnnotationCreated).await;
    Ok((StatusCode::CREATED, Json(record)))
}

async fn update(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(path): Path<AnnotationPath>,
    Json(request): Json<UpdateAnnotation>,
) -> Result<Json<pacs_db::AnnotationRecord>, AnnotationError> {
    validate_path(&path.study_uid, &path.series_uid)?;
    if request.expected_revision <= 0 || !request.geometry.is_object() {
        return Err(AnnotationError::BadRequest(
            "expected_revision 必须为正数且 geometry 必须是对象".to_owned(),
        ));
    }
    let record = pacs_db::update_annotation(
        &state.pool,
        pacs_db::AnnotationUpdate {
            institution_id: identity.institution_id,
            study_instance_uid: &path.study_uid,
            series_instance_uid: &path.series_uid,
            annotation_id: path.annotation_id,
            expected_revision: request.expected_revision,
            geometry: &request.geometry,
            deleted: request.deleted,
            user_id: identity.user_id,
        },
    )
    .await
    .map_err(AnnotationError::db)?;
    let action = if request.deleted {
        Action::AnnotationDeleted
    } else {
        Action::AnnotationUpdated
    };
    audit(
        &state,
        &identity,
        &SeriesPath {
            study_uid: path.study_uid,
            series_uid: path.series_uid,
        },
        &record,
        action,
    )
    .await;
    Ok(Json(record))
}

fn validate_path(study_uid: &str, series_uid: &str) -> Result<(), AnnotationError> {
    pacs_core::Uid::parse(study_uid)
        .map_err(|_| AnnotationError::BadRequest("StudyInstanceUID 无效".to_owned()))?;
    pacs_core::Uid::parse(series_uid)
        .map_err(|_| AnnotationError::BadRequest("SeriesInstanceUID 无效".to_owned()))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_annotation(
    schema_version: i32,
    kind: &str,
    coordinate_space: &str,
    sop_uid: Option<&str>,
    frame_number: Option<i32>,
    mpr_plane: Option<&str>,
    geometry: &Value,
) -> Result<(), AnnotationError> {
    if schema_version != 1 {
        return Err(AnnotationError::BadRequest(
            "只支持标注 schema_version=1".to_owned(),
        ));
    }
    if !matches!(
        kind,
        "length" | "arrow" | "ellipse_roi" | "rectangle_roi" | "angle" | "point_probe"
    ) || !geometry.is_object()
    {
        return Err(AnnotationError::BadRequest(
            "标注类型或 geometry 无效".to_owned(),
        ));
    }
    validate_geometry(kind, coordinate_space, geometry)?;
    match coordinate_space {
        "image" => {
            let sop_uid = sop_uid.ok_or_else(|| {
                AnnotationError::BadRequest("图像标注缺少 SOPInstanceUID".to_owned())
            })?;
            pacs_core::Uid::parse(sop_uid)
                .map_err(|_| AnnotationError::BadRequest("SOPInstanceUID 无效".to_owned()))?;
            if !frame_number.is_some_and(|value| value > 0) || mpr_plane.is_some() {
                return Err(AnnotationError::BadRequest("图像标注帧定位无效".to_owned()));
            }
        }
        "patient" => {
            if sop_uid.is_some()
                || frame_number.is_some()
                || !matches!(mpr_plane, Some("axial" | "coronal" | "sagittal"))
            {
                return Err(AnnotationError::BadRequest(
                    "MPR 标注患者空间定位无效".to_owned(),
                ));
            }
        }
        _ => return Err(AnnotationError::BadRequest("未知坐标空间".to_owned())),
    }
    Ok(())
}

fn validate_geometry(
    kind: &str,
    coordinate_space: &str,
    geometry: &Value,
) -> Result<(), AnnotationError> {
    let object = geometry
        .as_object()
        .ok_or_else(|| AnnotationError::BadRequest("geometry 必须是对象".to_owned()))?;
    let names: &[&str] = match kind {
        "point_probe" => &["point"],
        "angle" => &["start", "vertex", "end"],
        _ => &["start", "end"],
    };
    for name in names {
        let point = object
            .get(*name)
            .and_then(Value::as_object)
            .ok_or_else(|| AnnotationError::BadRequest(format!("geometry 缺少有效的 {name}")))?;
        for axis in if coordinate_space == "patient" {
            &["x", "y", "z"][..]
        } else {
            &["x", "y"][..]
        } {
            let value = point.get(*axis).and_then(Value::as_f64).ok_or_else(|| {
                AnnotationError::BadRequest(format!("geometry.{name}.{axis} 必须是数值"))
            })?;
            if !value.is_finite() || value.abs() > 10_000_000.0 {
                return Err(AnnotationError::BadRequest(format!(
                    "geometry.{name}.{axis} 超出支持范围"
                )));
            }
        }
    }
    Ok(())
}

async fn audit(
    state: &WebState,
    identity: &Identity,
    path: &SeriesPath,
    record: &pacs_db::AnnotationRecord,
    action: Action,
) {
    let mut entry = Entry::for_user(identity.user_id, &identity.username, identity.role)
        .with_study(&path.study_uid)
        .with_detail(serde_json::json!({
            "annotation_id": record.id,
            "annotation_kind": record.kind,
            "revision": record.revision,
        }));
    entry.series_instance_uid = Some(path.series_uid.clone());
    entry.sop_instance_uid = record.sop_instance_uid.clone();
    record_audit(&state.pool, action, Outcome::Success, entry).await;
}

#[derive(Debug, thiserror::Error)]
enum AnnotationError {
    #[error("{0}")]
    BadRequest(String),
    #[error("标注不存在或不属于当前序列")]
    NotFound,
    #[error("{0}")]
    Conflict(String),
    #[error("内部错误")]
    Internal,
}

impl AnnotationError {
    fn db(error: pacs_db::DbError) -> Self {
        match error {
            pacs_db::DbError::NotFound => Self::NotFound,
            pacs_db::DbError::Conflict(message) => Self::Conflict(message),
            other => {
                tracing::error!(%other, "共享标注数据库操作失败");
                Self::Internal
            }
        }
    }
}

impl IntoResponse for AnnotationError {
    fn into_response(self) -> Response {
        let status = match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let message = self.to_string();
        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_inconsistent_annotation_targets() {
        let geometry = serde_json::json!({
            "start": {"x": 1, "y": 2, "z": 3},
            "end": {"x": 4, "y": 5, "z": 6}
        });
        assert!(validate_annotation(1, "length", "image", None, Some(1), None, &geometry).is_err());
        assert!(
            validate_annotation(1, "length", "patient", None, None, Some("axial"), &geometry)
                .is_ok()
        );
        assert!(
            validate_annotation(2, "length", "patient", None, None, Some("axial"), &geometry)
                .is_err()
        );
    }

    #[test]
    fn validates_geometry_for_its_coordinate_space() {
        let image = serde_json::json!({"point": {"x": 1.0, "y": 2.0}});
        assert!(validate_geometry("point_probe", "image", &image).is_ok());
        assert!(validate_geometry("point_probe", "patient", &image).is_err());
        let patient = serde_json::json!({"point": {"x": 1.0, "y": 2.0, "z": 3.0}});
        assert!(validate_geometry("point_probe", "patient", &patient).is_ok());
    }
}
