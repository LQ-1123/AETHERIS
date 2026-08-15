use std::sync::Arc;

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch};
use axum::{Json, Router};
use pacs_auth::{AuthService, Identity, Permission};
use serde::Deserialize;

use crate::WebState;

pub fn routes(state: WebState, auth: Arc<AuthService>) -> Router {
    Router::new()
        .route("/window-presets", get(list).post(create))
        .route(
            "/window-presets/{preset_id}",
            patch(rename).delete(delete_preset),
        )
        .with_state(state)
        .layer(axum::middleware::from_fn(move |request, next| {
            let auth = Arc::clone(&auth);
            async move { pacs_auth::require(auth, Permission::ViewImages, request, next).await }
        }))
}

#[derive(Deserialize)]
struct CreatePreset {
    modality: String,
    name: String,
    center: f64,
    width: f64,
    function: String,
}

#[derive(Deserialize)]
struct RenamePreset {
    name: String,
}

async fn list(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
) -> Result<Json<Vec<pacs_db::UserWindowPreset>>, WindowPresetError> {
    let records =
        pacs_db::list_user_window_presets(&state.pool, identity.institution_id, identity.user_id)
            .await
            .map_err(WindowPresetError::db)?;
    Ok(Json(records))
}

async fn create(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Json(request): Json<CreatePreset>,
) -> Result<(StatusCode, Json<pacs_db::UserWindowPreset>), WindowPresetError> {
    let name = normalized_name(&request.name)?;
    let modality = normalized_modality(&request.modality)?;
    validate_window(request.center, request.width, &request.function)?;
    let record = pacs_db::create_user_window_preset(
        &state.pool,
        pacs_db::NewUserWindowPreset {
            institution_id: identity.institution_id,
            user_id: identity.user_id,
            modality: &modality,
            name: &name,
            center: request.center,
            width: request.width,
            function: &request.function,
        },
    )
    .await
    .map_err(WindowPresetError::db)?;
    Ok((StatusCode::CREATED, Json(record)))
}

async fn rename(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(preset_id): Path<i64>,
    Json(request): Json<RenamePreset>,
) -> Result<Json<pacs_db::UserWindowPreset>, WindowPresetError> {
    if preset_id <= 0 {
        return Err(WindowPresetError::NotFound);
    }
    let name = normalized_name(&request.name)?;
    let record = pacs_db::rename_user_window_preset(
        &state.pool,
        identity.institution_id,
        identity.user_id,
        preset_id,
        &name,
    )
    .await
    .map_err(WindowPresetError::db)?;
    Ok(Json(record))
}

async fn delete_preset(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path(preset_id): Path<i64>,
) -> Result<StatusCode, WindowPresetError> {
    if preset_id <= 0 {
        return Err(WindowPresetError::NotFound);
    }
    pacs_db::delete_user_window_preset(
        &state.pool,
        identity.institution_id,
        identity.user_id,
        preset_id,
    )
    .await
    .map_err(WindowPresetError::db)?;
    Ok(StatusCode::NO_CONTENT)
}

fn normalized_name(value: &str) -> Result<String, WindowPresetError> {
    let name = value.trim();
    if name.is_empty() || name.chars().count() > 64 {
        return Err(WindowPresetError::BadRequest(
            "窗预设名称必须为 1 到 64 个字符".to_owned(),
        ));
    }
    Ok(name.to_owned())
}

fn normalized_modality(value: &str) -> Result<String, WindowPresetError> {
    let modality = value.trim().to_ascii_uppercase();
    if modality.is_empty()
        || modality.len() > 16
        || !modality
            .bytes()
            .all(|value| value.is_ascii_uppercase() || value.is_ascii_digit())
    {
        return Err(WindowPresetError::BadRequest(
            "影像模态必须为 1 到 16 个大写字母或数字".to_owned(),
        ));
    }
    Ok(modality)
}

fn validate_window(center: f64, width: f64, function: &str) -> Result<(), WindowPresetError> {
    if !center.is_finite() || !width.is_finite() || width <= 0.0 {
        return Err(WindowPresetError::BadRequest(
            "窗位必须为有限数，窗宽必须为有限正数".to_owned(),
        ));
    }
    if !matches!(function, "LINEAR" | "LINEAR_EXACT" | "SIGMOID") {
        return Err(WindowPresetError::BadRequest(
            "VOI Function 无效".to_owned(),
        ));
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
enum WindowPresetError {
    #[error("{0}")]
    BadRequest(String),
    #[error("窗预设不存在")]
    NotFound,
    #[error("{0}")]
    Conflict(String),
    #[error("内部错误")]
    Internal,
}

impl WindowPresetError {
    fn db(error: pacs_db::DbError) -> Self {
        match error {
            pacs_db::DbError::NotFound => Self::NotFound,
            pacs_db::DbError::Conflict(message) => Self::Conflict(message),
            other => {
                tracing::error!(%other, "用户窗预设数据库操作失败");
                Self::Internal
            }
        }
    }
}

impl IntoResponse for WindowPresetError {
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
    fn normalizes_names_and_modalities() {
        assert_eq!(normalized_name("  肺窗  ").unwrap(), "肺窗");
        assert_eq!(normalized_modality(" ct ").unwrap(), "CT");
        assert!(normalized_name("  ").is_err());
        assert!(normalized_modality("CT/MR").is_err());
    }

    #[test]
    fn rejects_invalid_windows() {
        assert!(validate_window(-600.0, 1500.0, "LINEAR").is_ok());
        assert!(validate_window(f64::NAN, 1500.0, "LINEAR").is_err());
        assert!(validate_window(40.0, 0.0, "LINEAR").is_err());
        assert!(validate_window(40.0, 400.0, "UNKNOWN").is_err());
    }
}
