//! STOW-RS multipart DICOM ingestion (PS3.18 section 10.5).

use std::sync::Arc;

use axum::extract::{Extension, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use multer::{Constraints, Multipart, SizeLimit};
use pacs_auth::service_accounts::{ApiScope, ServiceIdentity};
use pacs_auth::{AuthService, Identity, Permission};
use serde::Serialize;

use crate::WebState;

const MAX_PART_BYTES: usize = 1024 * 1024 * 1024;
const MAX_REQUEST_BYTES: u64 = 4 * 1024 * 1024 * 1024;

pub fn routes(state: WebState, auth: Arc<AuthService>) -> Router {
    Router::new()
        .route("/studies", post(store_instances))
        .with_state(state)
        .layer(axum::middleware::from_fn(move |request, next| {
            let auth = Arc::clone(&auth);
            async move { require_upload(auth, request, next).await }
        }))
}

async fn require_upload(auth: Arc<AuthService>, request: Request, next: Next) -> Response {
    let is_service_key = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split_once(' '))
        .is_some_and(|(scheme, token)| {
            scheme.eq_ignore_ascii_case("Bearer") && token.trim().starts_with("pacs_sk_")
        });
    if is_service_key {
        pacs_auth::service_accounts::require_api_scope(auth, ApiScope::Upload, request, next).await
    } else {
        pacs_auth::require(auth, Permission::UploadImages, request, next).await
    }
}

async fn store_instances(
    State(state): State<WebState>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
    request: Request,
) -> Result<Response, StowError> {
    let institution_id = user
        .as_ref()
        .map(|Extension(identity)| identity.institution_id)
        .or_else(|| {
            service
                .as_ref()
                .map(|Extension(identity)| identity.institution_id)
        })
        .ok_or(StowError::MissingIdentity)?;
    let boundary = multipart_boundary(request.headers().get(header::CONTENT_TYPE))?;
    let store = state.store.as_ref().ok_or(StowError::StorageUnavailable)?;
    let constraints = Constraints::new().size_limit(
        SizeLimit::new()
            .whole_stream(MAX_REQUEST_BYTES)
            .per_field(MAX_PART_BYTES as u64),
    );
    let mut multipart = Multipart::with_constraints(
        request.into_body().into_data_stream(),
        boundary,
        constraints,
    );
    let mut results = Vec::new();

    while let Some(field) = multipart.next_field().await.map_err(StowError::Multipart)? {
        let is_dicom = field.content_type().is_some_and(|content_type| {
            content_type.type_() == mime::APPLICATION && content_type.subtype() == "dicom"
        });
        if !is_dicom {
            results.push(PartResult::failure(
                None,
                None,
                "part 的 Content-Type 必须为 application/dicom",
            ));
            continue;
        }
        let bytes = field.bytes().await.map_err(StowError::Multipart)?;
        results.push(PartResult::from(
            crate::ingest::ingest_dicom(store, &state.pool, institution_id, &bytes).await,
        ));
    }

    if results.is_empty() {
        return Err(StowError::EmptyRequest);
    }
    let succeeded = results.iter().filter(|result| result.success).count();
    let status = match succeeded {
        0 => StatusCode::CONFLICT,
        count if count == results.len() => StatusCode::OK,
        _ => StatusCode::ACCEPTED,
    };
    audit_upload(&state, user.as_ref(), service.as_ref(), &results).await;
    Ok((status, Json(stow_response(&results))).into_response())
}

fn multipart_boundary(content_type: Option<&axum::http::HeaderValue>) -> Result<String, StowError> {
    let raw = content_type
        .and_then(|value| value.to_str().ok())
        .ok_or(StowError::UnsupportedMediaType)?;
    let parsed: mime::Mime = raw.parse().map_err(|_| StowError::UnsupportedMediaType)?;
    if parsed.type_() != mime::MULTIPART || parsed.subtype() != "related" {
        return Err(StowError::UnsupportedMediaType);
    }
    if parsed
        .get_param("type")
        .is_some_and(|value| value.as_str() != "application/dicom")
    {
        return Err(StowError::UnsupportedMediaType);
    }
    parsed
        .get_param(mime::BOUNDARY)
        .map(|value| value.as_str().to_owned())
        .filter(|value| !value.is_empty())
        .ok_or(StowError::MissingBoundary)
}

#[derive(Debug, Serialize)]
struct PartResult {
    sop_class_uid: Option<String>,
    sop_instance_uid: Option<String>,
    success: bool,
    duplicate: bool,
    error: Option<String>,
}

impl PartResult {
    fn failure(
        sop_class_uid: Option<String>,
        sop_instance_uid: Option<String>,
        error: impl Into<String>,
    ) -> Self {
        Self {
            sop_class_uid,
            sop_instance_uid,
            success: false,
            duplicate: false,
            error: Some(error.into()),
        }
    }
}

impl From<crate::ingest::IngestOutcome> for PartResult {
    fn from(value: crate::ingest::IngestOutcome) -> Self {
        let success = value.success();
        let duplicate = matches!(
            value.disposition,
            crate::ingest::IngestDisposition::Duplicate
        );
        Self {
            sop_class_uid: value.sop_class_uid,
            sop_instance_uid: value.sop_instance_uid,
            success,
            duplicate,
            error: value.error,
        }
    }
}

fn stow_response(results: &[PartResult]) -> serde_json::Value {
    let referenced: Vec<_> = results
        .iter()
        .filter(|result| result.success)
        .map(reference_item)
        .collect();
    let failed: Vec<_> = results
        .iter()
        .filter(|result| !result.success)
        .map(|result| {
            let mut item = reference_item(result);
            item["00081197"] = serde_json::json!({"vr": "US", "Value": [272]});
            item
        })
        .collect();
    let mut response = serde_json::Map::new();
    if !referenced.is_empty() {
        response.insert(
            "00081199".to_owned(),
            serde_json::json!({"vr": "SQ", "Value": referenced}),
        );
    }
    if !failed.is_empty() {
        response.insert(
            "00081198".to_owned(),
            serde_json::json!({"vr": "SQ", "Value": failed}),
        );
    }
    serde_json::Value::Object(response)
}

fn reference_item(result: &PartResult) -> serde_json::Value {
    let mut item = serde_json::Map::new();
    if let Some(uid) = &result.sop_class_uid {
        item.insert(
            "00081150".to_owned(),
            serde_json::json!({"vr": "UI", "Value": [uid]}),
        );
    }
    if let Some(uid) = &result.sop_instance_uid {
        item.insert(
            "00081155".to_owned(),
            serde_json::json!({"vr": "UI", "Value": [uid]}),
        );
    }
    serde_json::Value::Object(item)
}

async fn audit_upload(
    state: &WebState,
    user: Option<&Extension<Identity>>,
    service: Option<&Extension<ServiceIdentity>>,
    results: &[PartResult],
) {
    let success = results.iter().filter(|result| result.success).count();
    let mut entry = if let Some(Extension(identity)) = user {
        pacs_auth::audit::Entry::for_user(identity.user_id, &identity.username, identity.role)
    } else if let Some(Extension(identity)) = service {
        pacs_auth::audit::Entry::for_attempted_username(&identity.name)
    } else {
        return;
    };
    entry = entry.with_detail(serde_json::json!({
        "protocol": "stow-rs",
        "service_account": service.is_some(),
        "succeeded": success,
        "failed": results.len() - success,
        "duplicates": results.iter().filter(|result| result.duplicate).count()
    }));
    pacs_auth::audit::record(
        &state.pool,
        pacs_auth::audit::Action::StoreImages,
        if success == results.len() {
            pacs_auth::audit::Outcome::Success
        } else {
            pacs_auth::audit::Outcome::Failure
        },
        entry,
    )
    .await;
}

#[derive(Debug, thiserror::Error)]
enum StowError {
    #[error("Content-Type 必须为 multipart/related; type=application/dicom")]
    UnsupportedMediaType,
    #[error("multipart 请求缺少 boundary")]
    MissingBoundary,
    #[error("multipart 请求无 DICOM part")]
    EmptyRequest,
    #[error("影像存储未配置")]
    StorageUnavailable,
    #[error("认证中间件未提供调用方身份")]
    MissingIdentity,
    #[error("multipart 解析失败: {0}")]
    Multipart(#[source] multer::Error),
}

impl IntoResponse for StowError {
    fn into_response(self) -> Response {
        let status = match &self {
            Self::UnsupportedMediaType | Self::MissingBoundary => {
                StatusCode::UNSUPPORTED_MEDIA_TYPE
            }
            Self::EmptyRequest => StatusCode::BAD_REQUEST,
            Self::Multipart(
                multer::Error::FieldSizeExceeded { .. } | multer::Error::StreamSizeExceeded { .. },
            ) => StatusCode::PAYLOAD_TOO_LARGE,
            Self::Multipart(_) => StatusCode::BAD_REQUEST,
            Self::StorageUnavailable | Self::MissingIdentity => StatusCode::INTERNAL_SERVER_ERROR,
        };
        if status.is_server_error() {
            tracing::error!(error = %self, "STOW-RS 请求失败");
        }
        (
            status,
            [(header::CONTENT_TYPE, "application/dicom+json")],
            Json(serde_json::json!({"error": self.to_string()})),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quoted_related_boundary() {
        let value = axum::http::HeaderValue::from_static(
            "multipart/related; type=\"application/dicom\"; boundary=\"dicom-boundary\"",
        );
        assert_eq!(multipart_boundary(Some(&value)).unwrap(), "dicom-boundary");
    }

    #[test]
    fn rejects_form_data_and_missing_boundary() {
        let form = axum::http::HeaderValue::from_static("multipart/form-data; boundary=x");
        assert!(matches!(
            multipart_boundary(Some(&form)),
            Err(StowError::UnsupportedMediaType)
        ));
        let missing =
            axum::http::HeaderValue::from_static("multipart/related; type=\"application/dicom\"");
        assert!(matches!(
            multipart_boundary(Some(&missing)),
            Err(StowError::MissingBoundary)
        ));
    }

    #[test]
    fn response_separates_successes_and_failures() {
        let response = stow_response(&[
            PartResult {
                sop_class_uid: Some("1.2.3".to_owned()),
                sop_instance_uid: Some("1.2.4".to_owned()),
                success: true,
                duplicate: false,
                error: None,
            },
            PartResult::failure(None, None, "bad object"),
        ]);
        assert_eq!(response["00081199"]["Value"].as_array().unwrap().len(), 1);
        assert_eq!(response["00081198"]["Value"].as_array().unwrap().len(), 1);
    }
}
