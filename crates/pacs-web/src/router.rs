//! DICOM routing management API and asynchronous delivery worker.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant};

use axum::extract::{Extension, Path, Query, Request, State};
use axum::http::{Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use chrono::{Duration, Utc};
use pacs_auth::service_accounts::{ApiScope, ServiceIdentity};
use pacs_auth::{AuthService, Identity, Permission};
use pacs_db::{BackgroundJob, JobKind, RouteDestination, RouteProtocol};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::WebState;

pub fn routes(state: WebState, auth: Arc<AuthService>) -> Router {
    Router::new()
        .route(
            "/router/destinations",
            get(list_destinations).post(create_destination),
        )
        .route(
            "/router/destinations/{id}",
            put(update_destination).delete(delete_destination),
        )
        .route("/router/destinations/{id}/test", post(test_destination))
        .route("/router/peers", get(list_observed_peers))
        .route("/router/rules", get(list_rules).post(create_rule))
        .route("/router/rules/{id}", put(update_rule).delete(delete_rule))
        .route("/router/send", post(send_scope))
        .route("/router/deliveries", get(list_deliveries))
        .route("/router/deliveries/{id}/replay", post(replay_delivery))
        .with_state(state)
        .layer(axum::middleware::from_fn(move |request, next| {
            let auth = Arc::clone(&auth);
            async move { require_route(auth, request, next).await }
        }))
}

async fn require_route(auth: Arc<AuthService>, request: Request, next: Next) -> Response {
    let is_service_key = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split_once(' '))
        .is_some_and(|(scheme, token)| {
            scheme.eq_ignore_ascii_case("Bearer") && token.trim().starts_with("pacs_sk_")
        });
    if is_service_key {
        pacs_auth::service_accounts::require_api_scope(auth, ApiScope::Route, request, next).await
    } else {
        pacs_auth::require(auth, Permission::ManageUsers, request, next).await
    }
}

fn institution(
    user: Option<&Extension<Identity>>,
    service: Option<&Extension<ServiceIdentity>>,
) -> Result<i64, RouterError> {
    user.map(|Extension(value)| value.institution_id)
        .or_else(|| service.map(|Extension(value)| value.institution_id))
        .ok_or(RouterError::MissingIdentity)
}

fn actor(user: Option<&Extension<Identity>>) -> Option<i64> {
    user.map(|Extension(value)| value.user_id)
}

#[derive(Debug, Deserialize)]
struct DestinationInput {
    name: String,
    protocol: RouteProtocol,
    #[serde(default = "default_true")]
    enabled: bool,
    host: Option<String>,
    port: Option<i32>,
    called_ae_title: Option<String>,
    calling_ae_title: Option<String>,
    #[serde(default)]
    use_tls: bool,
    stow_url: Option<String>,
    auth_token: Option<String>,
    ca_pem: Option<String>,
}

fn default_true() -> bool {
    true
}

impl DestinationInput {
    fn as_db(&self) -> pacs_db::RouteDestinationInput<'_> {
        let dimse = self.protocol == RouteProtocol::Dimse;
        pacs_db::RouteDestinationInput {
            name: &self.name,
            protocol: self.protocol,
            enabled: self.enabled,
            host: dimse.then_some(self.host.as_deref()).flatten(),
            port: dimse.then_some(self.port).flatten(),
            called_ae_title: dimse.then_some(self.called_ae_title.as_deref()).flatten(),
            calling_ae_title: dimse.then_some(self.calling_ae_title.as_deref()).flatten(),
            use_tls: dimse && self.use_tls,
            stow_url: (!dimse).then_some(self.stow_url.as_deref()).flatten(),
            auth_token: self.auth_token.as_deref(),
            ca_pem: self.ca_pem.as_deref(),
        }
    }
}

async fn list_destinations(
    State(state): State<WebState>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
) -> Result<Json<Vec<RouteDestination>>, RouterError> {
    Ok(Json(
        pacs_db::list_route_destinations(
            &state.pool,
            institution(user.as_ref(), service.as_ref())?,
        )
        .await?,
    ))
}

async fn create_destination(
    State(state): State<WebState>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
    Json(input): Json<DestinationInput>,
) -> Result<(StatusCode, Json<RouteDestination>), RouterError> {
    let destination = pacs_db::create_route_destination(
        &state.pool,
        institution(user.as_ref(), service.as_ref())?,
        input.as_db(),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(destination)))
}

async fn update_destination(
    State(state): State<WebState>,
    Path(id): Path<Uuid>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
    Json(input): Json<DestinationInput>,
) -> Result<Json<RouteDestination>, RouterError> {
    Ok(Json(
        pacs_db::update_route_destination(
            &state.pool,
            institution(user.as_ref(), service.as_ref())?,
            id,
            input.as_db(),
        )
        .await?,
    ))
}

async fn delete_destination(
    State(state): State<WebState>,
    Path(id): Path<Uuid>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
) -> Result<StatusCode, RouterError> {
    if pacs_db::delete_route_destination(
        &state.pool,
        institution(user.as_ref(), service.as_ref())?,
        id,
    )
    .await?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(RouterError::NotFound)
    }
}

async fn test_destination(
    State(state): State<WebState>,
    Path(id): Path<Uuid>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
) -> Result<Json<RouteDestination>, RouterError> {
    let institution_id = institution(user.as_ref(), service.as_ref())?;
    let destination = pacs_db::get_route_destination(&state.pool, institution_id, id).await?;
    let started = Instant::now();
    let result = check_destination(&destination).await;
    let latency = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
    let updated = pacs_db::record_destination_health(
        &state.pool,
        institution_id,
        id,
        result.is_ok(),
        latency,
        result.as_ref().err().map(String::as_str),
    )
    .await?;
    Ok(Json(updated))
}

#[derive(Debug, Deserialize)]
struct PeerQuery {
    limit: Option<i64>,
}

async fn list_observed_peers(
    State(state): State<WebState>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
    Query(query): Query<PeerQuery>,
) -> Result<Json<Vec<pacs_db::ObservedDicomPeer>>, RouterError> {
    Ok(Json(
        pacs_db::list_observed_dicom_peers(
            &state.pool,
            institution(user.as_ref(), service.as_ref())?,
            query.limit.unwrap_or(200),
        )
        .await?,
    ))
}

#[derive(Debug, Deserialize)]
struct RuleInput {
    destination_id: Uuid,
    name: String,
    #[serde(default = "default_priority")]
    priority: i32,
    #[serde(default = "default_true")]
    enabled: bool,
    source_ae_title: Option<String>,
    modality: Option<String>,
    body_part_examined: Option<String>,
    study_description: Option<String>,
    series_description: Option<String>,
    #[serde(default = "empty_object")]
    tag_matches: Value,
}

fn default_priority() -> i32 {
    100
}
fn empty_object() -> Value {
    json!({})
}

impl RuleInput {
    fn as_db(&self) -> pacs_db::RouteRuleInput<'_> {
        pacs_db::RouteRuleInput {
            destination_id: self.destination_id,
            name: &self.name,
            priority: self.priority,
            enabled: self.enabled,
            source_ae_title: self.source_ae_title.as_deref(),
            modality: self.modality.as_deref(),
            body_part_examined: self.body_part_examined.as_deref(),
            study_description: self.study_description.as_deref(),
            series_description: self.series_description.as_deref(),
            tag_matches: &self.tag_matches,
        }
    }
}

async fn list_rules(
    State(state): State<WebState>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
) -> Result<Json<Vec<pacs_db::RouteRule>>, RouterError> {
    Ok(Json(
        pacs_db::list_route_rules(&state.pool, institution(user.as_ref(), service.as_ref())?)
            .await?,
    ))
}
async fn create_rule(
    State(state): State<WebState>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
    Json(input): Json<RuleInput>,
) -> Result<(StatusCode, Json<pacs_db::RouteRule>), RouterError> {
    let value = pacs_db::create_route_rule(
        &state.pool,
        institution(user.as_ref(), service.as_ref())?,
        input.as_db(),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(value)))
}
async fn update_rule(
    State(state): State<WebState>,
    Path(id): Path<Uuid>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
    Json(input): Json<RuleInput>,
) -> Result<Json<pacs_db::RouteRule>, RouterError> {
    Ok(Json(
        pacs_db::update_route_rule(
            &state.pool,
            institution(user.as_ref(), service.as_ref())?,
            id,
            input.as_db(),
        )
        .await?,
    ))
}
async fn delete_rule(
    State(state): State<WebState>,
    Path(id): Path<Uuid>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
) -> Result<StatusCode, RouterError> {
    if pacs_db::delete_route_rule(
        &state.pool,
        institution(user.as_ref(), service.as_ref())?,
        id,
    )
    .await?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(RouterError::NotFound)
    }
}

#[derive(Debug, Deserialize)]
struct SendRequest {
    destination_id: Uuid,
    study_instance_uid: String,
    series_instance_uid: Option<String>,
}

async fn send_scope(
    State(state): State<WebState>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
    Json(input): Json<SendRequest>,
) -> Result<(StatusCode, Json<Value>), RouterError> {
    pacs_core::Uid::parse(&input.study_instance_uid)
        .map_err(|_| RouterError::BadRequest("StudyInstanceUID 无效".to_owned()))?;
    if let Some(series) = &input.series_instance_uid {
        pacs_core::Uid::parse(series)
            .map_err(|_| RouterError::BadRequest("SeriesInstanceUID 无效".to_owned()))?;
    }
    let institution_id = institution(user.as_ref(), service.as_ref())?;
    let sources = pacs_db::route_sources_for_scope(
        &state.pool,
        institution_id,
        &input.study_instance_uid,
        input.series_instance_uid.as_deref(),
    )
    .await?;
    if sources.is_empty() {
        return Err(RouterError::NotFound);
    }
    let mut jobs = Vec::new();
    for source in &sources {
        if let Some(job) = pacs_db::enqueue_route_delivery(
            &state.pool,
            source,
            input.destination_id,
            None,
            actor(user.as_ref()),
        )
        .await?
        {
            jobs.push(job);
        }
    }
    Ok((
        StatusCode::ACCEPTED,
        Json(
            json!({"queued":jobs.len(),"skipped_as_duplicate":sources.len()-jobs.len(),"job_ids":jobs}),
        ),
    ))
}

#[derive(Debug, Deserialize)]
struct DeliveryQuery {
    limit: Option<i64>,
}
async fn list_deliveries(
    State(state): State<WebState>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
    Query(query): Query<DeliveryQuery>,
) -> Result<Json<Vec<pacs_db::RouteDelivery>>, RouterError> {
    Ok(Json(
        pacs_db::list_route_deliveries(
            &state.pool,
            institution(user.as_ref(), service.as_ref())?,
            query.limit.unwrap_or(100),
        )
        .await?,
    ))
}
async fn replay_delivery(
    State(state): State<WebState>,
    Path(id): Path<Uuid>,
    user: Option<Extension<Identity>>,
    service: Option<Extension<ServiceIdentity>>,
) -> Result<(StatusCode, Json<Value>), RouterError> {
    let job = pacs_db::replay_route_delivery(
        &state.pool,
        institution(user.as_ref(), service.as_ref())?,
        id,
        actor(user.as_ref()),
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(json!({"job_id":job}))))
}

pub async fn enqueue_for_instance(
    pool: &sqlx::PgPool,
    institution_id: i64,
    sop_uid: &str,
    source_ae_title: Option<&str>,
) -> Result<usize, pacs_db::DbError> {
    let source = pacs_db::route_source_by_sop(pool, institution_id, sop_uid).await?;
    let rules = pacs_db::matching_route_rules(pool, &source, source_ae_title).await?;
    let mut destinations = HashSet::new();
    let mut queued = 0;
    for rule in rules {
        if destinations.insert(rule.destination_id)
            && pacs_db::enqueue_route_delivery(
                pool,
                &source,
                rule.destination_id,
                Some(rule.id),
                None,
            )
            .await?
            .is_some()
        {
            queued += 1;
        }
    }
    Ok(queued)
}

pub fn start_worker(state: WebState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let worker = Uuid::new_v4();
        let mut interval = tokio::time::interval(StdDuration::from_secs(1));
        loop {
            interval.tick().await;
            match pacs_db::claim_background_job(
                &state.pool,
                JobKind::Route,
                worker,
                Duration::minutes(5),
            )
            .await
            {
                Ok(Some(job)) => process_job(&state, worker, job).await,
                Ok(None) => {}
                Err(error) => tracing::error!(%error,"领取路由任务失败"),
            }
        }
    })
}

async fn process_job(state: &WebState, worker: Uuid, job: BackgroundJob) {
    let result = run_delivery(state, &job).await;
    match result {
        Ok(value) => {
            let _ = pacs_db::complete_background_job(&state.pool, job.id, worker, &value).await;
        }
        Err(error) => {
            let message = error.to_string();
            let delivery = job.payload["delivery_id"]
                .as_str()
                .and_then(|v| Uuid::parse_str(v).ok());
            if job.attempts < job.max_attempts {
                if let Some(id) = delivery {
                    let _ = pacs_db::retry_delivery(&state.pool, job.institution_id, id, &message)
                        .await;
                }
                let exponent = u32::try_from(job.attempts.saturating_sub(1))
                    .unwrap_or(0)
                    .min(6);
                let retry_at =
                    Utc::now() + Duration::seconds(5_i64.saturating_mul(2_i64.pow(exponent)));
                let _ = pacs_db::fail_background_job(
                    &state.pool,
                    job.id,
                    worker,
                    &message,
                    Some(retry_at),
                )
                .await;
            } else {
                if let Some(id) = delivery {
                    let _ = pacs_db::finish_delivery(
                        &state.pool,
                        job.institution_id,
                        id,
                        false,
                        Some(&message),
                    )
                    .await;
                }
                let _ =
                    pacs_db::fail_background_job(&state.pool, job.id, worker, &message, None).await;
            }
        }
    }
}

async fn run_delivery(state: &WebState, job: &BackgroundJob) -> Result<Value, RouterError> {
    let delivery_id = job.payload["delivery_id"]
        .as_str()
        .and_then(|v| Uuid::parse_str(v).ok())
        .ok_or_else(|| RouterError::BadRequest("路由任务缺少 delivery_id".to_owned()))?;
    pacs_db::mark_delivery_running(&state.pool, job.institution_id, delivery_id).await?;
    let (destination, source) =
        pacs_db::get_delivery_source(&state.pool, job.institution_id, delivery_id).await?;
    let store = state
        .store
        .as_ref()
        .ok_or(RouterError::StorageUnavailable)?;
    let bytes = store
        .read(&source.storage_path)
        .await
        .map_err(|e| RouterError::Transport(e.to_string()))?;
    let started = Instant::now();
    let result = send_to_destination(&destination, &bytes).await;
    let latency = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
    let _ = pacs_db::record_destination_health(
        &state.pool,
        job.institution_id,
        destination.id,
        result.is_ok(),
        latency,
        result.as_ref().err().map(String::as_str),
    )
    .await;
    result.map_err(RouterError::Transport)?;
    pacs_db::finish_delivery(&state.pool, job.institution_id, delivery_id, true, None).await?;
    Ok(
        json!({"delivery_id":delivery_id,"destination_id":destination.id,"sop_instance_uid":source.sop_uid,"protocol":destination.protocol}),
    )
}

async fn check_destination(destination: &RouteDestination) -> Result<(), String> {
    match destination.protocol {
        RouteProtocol::Dimse => pacs_dimse::c_echo(&dimse_config(destination)?)
            .await
            .map_err(|e| e.to_string()),
        RouteProtocol::Stow => {
            let client = stow_client(destination)?;
            let mut request = client.request(
                Method::OPTIONS,
                destination.stow_url.as_deref().ok_or("STOW URL 缺失")?,
            );
            if let Some(token) = &destination.auth_token {
                request = request.bearer_auth(token);
            }
            let response = request.send().await.map_err(|e| e.to_string())?;
            if response.status().is_success() || response.status() == StatusCode::METHOD_NOT_ALLOWED
            {
                Ok(())
            } else {
                Err(format!("STOW 测试返回 HTTP {}", response.status()))
            }
        }
    }
}

async fn send_to_destination(destination: &RouteDestination, bytes: &[u8]) -> Result<(), String> {
    match destination.protocol {
        RouteProtocol::Dimse => pacs_dimse::c_store(&dimse_config(destination)?, bytes)
            .await
            .map_err(|e| e.to_string()),
        RouteProtocol::Stow => send_stow(destination, bytes).await,
    }
}

fn dimse_config(destination: &RouteDestination) -> Result<pacs_dimse::DimseClientConfig, String> {
    let port = destination
        .port
        .and_then(|v| u16::try_from(v).ok())
        .ok_or("DIMSE 端口无效")?;
    let mut config = pacs_dimse::DimseClientConfig::new(
        destination.host.clone().ok_or("DIMSE 主机缺失")?,
        port,
        destination
            .called_ae_title
            .clone()
            .ok_or("Called AE Title 缺失")?,
        destination
            .calling_ae_title
            .clone()
            .ok_or("Calling AE Title 缺失")?,
    );
    config.use_tls = destination.use_tls;
    config.ca_pem = destination.ca_pem.clone();
    Ok(config)
}

fn stow_client(destination: &RouteDestination) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(StdDuration::from_secs(10))
        .timeout(StdDuration::from_secs(120));
    if let Some(pem) = &destination.ca_pem {
        let cert = reqwest::Certificate::from_pem(pem.as_bytes())
            .map_err(|e| format!("CA 证书无效: {e}"))?;
        builder = builder.add_root_certificate(cert);
    }
    builder.build().map_err(|e| e.to_string())
}

async fn send_stow(destination: &RouteDestination, bytes: &[u8]) -> Result<(), String> {
    let boundary = format!("remote-pacs-{}", Uuid::new_v4());
    let mut body = Vec::with_capacity(bytes.len() + 256);
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Type: application/dicom\r\n\r\n").as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let mut request = stow_client(destination)?
        .post(destination.stow_url.as_deref().ok_or("STOW URL 缺失")?)
        .header(
            header::CONTENT_TYPE,
            format!("multipart/related; type=\"application/dicom\"; boundary={boundary}"),
        )
        .body(body);
    if let Some(token) = &destination.auth_token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.map_err(|e| e.to_string())?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!(
            "STOW 发送返回 HTTP {}: {}",
            response.status(),
            response.text().await.unwrap_or_default()
        ))
    }
}

#[derive(Debug, thiserror::Error)]
enum RouterError {
    #[error("认证中间件未提供调用方身份")]
    MissingIdentity,
    #[error("{0}")]
    BadRequest(String),
    #[error("资源不存在")]
    NotFound,
    #[error("影像存储未配置")]
    StorageUnavailable,
    #[error("路由传输失败: {0}")]
    Transport(String),
    #[error("数据库操作失败")]
    Database(#[from] pacs_db::DbError),
}
impl IntoResponse for RouterError {
    fn into_response(self) -> Response {
        let status = match &self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::NotFound | Self::Database(pacs_db::DbError::NotFound) => StatusCode::NOT_FOUND,
            Self::Database(pacs_db::DbError::Conflict(_)) => StatusCode::CONFLICT,
            Self::Transport(_) => StatusCode::BAD_GATEWAY,
            Self::MissingIdentity | Self::StorageUnavailable | Self::Database(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        if status.is_server_error() {
            tracing::error!(error=%self,"DICOM Router 请求失败");
        }
        (status, Json(json!({"error":self.to_string()}))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Bytes;
    use axum::extract::Request;
    use axum::routing::post;
    #[test]
    fn stow_body_is_related_and_contains_part10() {
        let bytes = b"DICOM";
        let boundary = "b";
        let mut body = Vec::new();
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Type: application/dicom\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(bytes);
        assert!(body.windows(bytes.len()).any(|v| v == bytes));
    }

    fn stow_destination(url: String) -> RouteDestination {
        RouteDestination {
            id: Uuid::nil(),
            institution_id: 1,
            name: "STOW test".to_owned(),
            protocol: RouteProtocol::Stow,
            enabled: true,
            host: None,
            port: None,
            called_ae_title: None,
            calling_ae_title: None,
            use_tls: false,
            stow_url: Some(url),
            auth_token: Some("secret".to_owned()),
            ca_pem: None,
            has_auth_token: true,
            has_ca_certificate: false,
            status: "unknown".to_owned(),
            last_checked_at: None,
            last_success_at: None,
            last_latency_ms: None,
            last_error: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn stow_sender_uses_related_content_type_and_bearer_auth() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new().route(
            "/stow",
            post(|request: Request| async move {
                let content_type = request
                    .headers()
                    .get(header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default()
                    .to_owned();
                let authorization = request
                    .headers()
                    .get(header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default()
                    .to_owned();
                let body = axum::body::to_bytes(request.into_body(), 1024 * 1024)
                    .await
                    .unwrap();
                assert!(content_type.starts_with("multipart/related; type=\"application/dicom\""));
                assert_eq!(authorization, "Bearer secret");
                assert!(body.windows(8).any(|window| window == b"DICMdata"));
                StatusCode::OK
            }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let destination = stow_destination(format!("http://{address}/stow"));
        send_stow(&destination, &Bytes::from_static(b"DICMdata"))
            .await
            .unwrap();
        server.abort();
    }
}
