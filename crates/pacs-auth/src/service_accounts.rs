//! Service accounts and scoped API keys for machine-to-machine integrations.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration as StdDuration, Instant};

use argon2::password_hash::rand_core::{OsRng, RngCore};
use axum::extract::{Extension, Path, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::{AuthService, Identity, Permission};

const KEY_MARKER: &str = "pacs_sk_";
const LOOKUP_PREFIX_LEN: usize = 20;
const REQUESTS_PER_MINUTE: u32 = 120;
static API_RATE_LIMITER: OnceLock<RateLimiter> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApiScope {
    Search,
    Read,
    Upload,
    Export,
    Route,
    Admin,
}

impl ApiScope {
    pub const ALL: [Self; 6] = [
        Self::Search,
        Self::Read,
        Self::Upload,
        Self::Export,
        Self::Route,
        Self::Admin,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Read => "read",
            Self::Upload => "upload",
            Self::Export => "export",
            Self::Route => "route",
            Self::Admin => "admin",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|scope| scope.as_str() == raw)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ServiceIdentity {
    pub service_account_id: Uuid,
    pub institution_id: i64,
    pub name: String,
    pub scopes: Vec<ApiScope>,
}

impl ServiceIdentity {
    pub fn can(&self, required: ApiScope) -> bool {
        self.scopes.contains(&ApiScope::Admin) || self.scopes.contains(&required)
    }
}

#[derive(Debug, Clone, Serialize)]
struct ServiceAccountRecord {
    id: Uuid,
    institution_id: i64,
    name: String,
    scopes: Vec<ApiScope>,
    is_active: bool,
    expires_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    last_used_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
struct CreateAccountRequest {
    name: String,
    scopes: Vec<ApiScope>,
    expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
struct CreateKeyRequest {
    expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
struct CreatedAccountResponse {
    account: ServiceAccountRecord,
    key_id: Uuid,
    api_key: String,
}

#[derive(Debug, Serialize)]
struct CreatedKeyResponse {
    key_id: Uuid,
    api_key: String,
    expires_at: Option<DateTime<Utc>>,
}

pub fn management_routes(auth: Arc<AuthService>) -> Router {
    Router::new()
        .route("/service-accounts", get(list_accounts).post(create_account))
        .route("/service-accounts/{account_id}/keys", post(create_key))
        .route(
            "/service-accounts/{account_id}/keys/{key_id}",
            delete(revoke_key),
        )
        .route("/service-accounts/{account_id}", delete(deactivate_account))
        .with_state(auth.clone())
        .layer(axum::middleware::from_fn(move |request, next| {
            let auth = Arc::clone(&auth);
            async move { crate::require(auth, Permission::ManageUsers, request, next).await }
        }))
}

/// A small authenticated endpoint establishes the v1 machine-auth contract.
/// Feature routes added later inherit this middleware with their own scope.
pub fn service_routes(auth: Arc<AuthService>) -> Router {
    Router::new()
        .route("/service-auth/whoami", get(service_whoami))
        .with_state(auth.clone())
        .layer(axum::middleware::from_fn(move |request, next| {
            let auth = Arc::clone(&auth);
            async move { require_api_scope(auth, ApiScope::Read, request, next).await }
        }))
}

pub fn documentation_routes() -> Router {
    Router::new().route("/openapi.json", get(openapi_document))
}

pub async fn require_api_scope(
    auth: Arc<AuthService>,
    scope: ApiScope,
    mut request: Request,
    next: Next,
) -> Response {
    let Some(secret) = bearer_token(&request) else {
        return api_unauthorized("缺少服务账号 Bearer API Key");
    };
    if lookup_prefix(secret).is_none() {
        return api_unauthorized("API Key 无效、已过期或已吊销");
    }
    let identity = match authenticate_api_key(auth.pool(), secret).await {
        Ok(Some(identity)) => identity,
        Ok(None) => return api_unauthorized("API Key 无效、已过期或已吊销"),
        Err(error) => {
            tracing::error!(%error, "服务账号 API Key 校验失败");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    if let Err(retry_after) = API_RATE_LIMITER
        .get_or_init(|| RateLimiter::new(REQUESTS_PER_MINUTE, StdDuration::from_secs(60)))
        .check(&identity.service_account_id.to_string())
    {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, retry_after.to_string())],
            Json(serde_json::json!({"error": {"code": "rate_limited", "message": "API Key 请求频率超过限制"}})),
        )
            .into_response();
    }
    if !identity.can(scope) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": {"code": "insufficient_scope", "message": "API Key 权限范围不足"}})),
        )
            .into_response();
    }
    request.extensions_mut().insert(identity);
    next.run(request).await
}

async fn list_accounts(
    State(auth): State<Arc<AuthService>>,
    Extension(identity): Extension<Identity>,
) -> Result<Json<Vec<ServiceAccountRecord>>, ServiceApiError> {
    let rows = sqlx::query(
        "SELECT id, institution_id, name, scopes, is_active, expires_at, created_at, last_used_at
         FROM service_accounts WHERE institution_id = $1 ORDER BY created_at DESC",
    )
    .bind(identity.institution_id)
    .fetch_all(auth.pool())
    .await?;
    rows.iter()
        .map(decode_account)
        .collect::<Result<Vec<_>, _>>()
        .map(Json)
}

async fn create_account(
    State(auth): State<Arc<AuthService>>,
    Extension(identity): Extension<Identity>,
    Json(request): Json<CreateAccountRequest>,
) -> Result<(StatusCode, Json<CreatedAccountResponse>), ServiceApiError> {
    validate_name_and_scopes(&request.name, &request.scopes, request.expires_at)?;
    let account_id = Uuid::new_v4();
    let key_id = Uuid::new_v4();
    let key = generate_key();
    let hash = hash_key(&key);
    let prefix = lookup_prefix(&key).expect("生成的 key 必须含完整前缀");
    let scopes: Vec<&str> = request.scopes.iter().map(|scope| scope.as_str()).collect();
    let mut tx = auth.pool().begin().await?;
    let row = sqlx::query(
        "INSERT INTO service_accounts
         (id, institution_id, name, scopes, expires_at, created_by)
         VALUES ($1, $2, $3, $4, $5, $6)
         RETURNING id, institution_id, name, scopes, is_active, expires_at, created_at, last_used_at",
    )
    .bind(account_id)
    .bind(identity.institution_id)
    .bind(request.name.trim())
    .bind(&scopes)
    .bind(request.expires_at)
    .bind(identity.user_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(ServiceApiError::database)?;
    sqlx::query(
        "INSERT INTO service_api_keys
         (id, service_account_fk, key_prefix, secret_hash, expires_at)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(key_id)
    .bind(account_id)
    .bind(prefix)
    .bind(hash.as_slice())
    .bind(request.expires_at)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(CreatedAccountResponse {
            account: decode_account(&row)?,
            key_id,
            api_key: key,
        }),
    ))
}

async fn create_key(
    State(auth): State<Arc<AuthService>>,
    Extension(identity): Extension<Identity>,
    Path(account_id): Path<Uuid>,
    Json(request): Json<CreateKeyRequest>,
) -> Result<(StatusCode, Json<CreatedKeyResponse>), ServiceApiError> {
    if request.expires_at.is_some_and(|value| value <= Utc::now()) {
        return Err(ServiceApiError::bad_request("过期时间必须晚于当前时间"));
    }
    let allowed: bool = sqlx::query_scalar(
        "SELECT EXISTS(
             SELECT 1 FROM service_accounts
             WHERE id = $1 AND institution_id = $2 AND is_active
               AND (expires_at IS NULL OR expires_at > now())
         )",
    )
    .bind(account_id)
    .bind(identity.institution_id)
    .fetch_one(auth.pool())
    .await?;
    if !allowed {
        return Err(ServiceApiError::NotFound);
    }
    let key_id = Uuid::new_v4();
    let key = generate_key();
    let hash = hash_key(&key);
    let prefix = lookup_prefix(&key).expect("生成的 key 必须含完整前缀");
    sqlx::query(
        "INSERT INTO service_api_keys
         (id, service_account_fk, key_prefix, secret_hash, expires_at)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(key_id)
    .bind(account_id)
    .bind(prefix)
    .bind(hash.as_slice())
    .bind(request.expires_at)
    .execute(auth.pool())
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(CreatedKeyResponse {
            key_id,
            api_key: key,
            expires_at: request.expires_at,
        }),
    ))
}

async fn revoke_key(
    State(auth): State<Arc<AuthService>>,
    Extension(identity): Extension<Identity>,
    Path((account_id, key_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, ServiceApiError> {
    let changed = sqlx::query(
        "UPDATE service_api_keys k SET revoked_at = now()
         FROM service_accounts a
         WHERE k.id = $1 AND k.service_account_fk = $2
           AND a.id = k.service_account_fk AND a.institution_id = $3
           AND k.revoked_at IS NULL",
    )
    .bind(key_id)
    .bind(account_id)
    .bind(identity.institution_id)
    .execute(auth.pool())
    .await?;
    if changed.rows_affected() == 0 {
        return Err(ServiceApiError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn deactivate_account(
    State(auth): State<Arc<AuthService>>,
    Extension(identity): Extension<Identity>,
    Path(account_id): Path<Uuid>,
) -> Result<StatusCode, ServiceApiError> {
    let mut tx = auth.pool().begin().await?;
    let changed = sqlx::query(
        "UPDATE service_accounts SET is_active = false
         WHERE id = $1 AND institution_id = $2 AND is_active",
    )
    .bind(account_id)
    .bind(identity.institution_id)
    .execute(&mut *tx)
    .await?;
    if changed.rows_affected() == 0 {
        return Err(ServiceApiError::NotFound);
    }
    sqlx::query(
        "UPDATE service_api_keys SET revoked_at = now()
         WHERE service_account_fk = $1 AND revoked_at IS NULL",
    )
    .bind(account_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn service_whoami(Extension(identity): Extension<ServiceIdentity>) -> Json<ServiceIdentity> {
    Json(identity)
}

async fn openapi_document() -> Json<serde_json::Value> {
    let mut document = serde_json::json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Remote PACS API",
            "version": pacs_core::VERSION,
            "description": "External integration API. DICOMweb endpoints follow PS3.18."
        },
        "components": {
            "securitySchemes": {
                "serviceApiKey": {"type": "http", "scheme": "bearer", "bearerFormat": "pacs_sk_*"},
                "userAccessToken": {"type": "http", "scheme": "bearer", "bearerFormat": "JWT"}
            }
        },
        "paths": {
            "/api/v1/service-auth/whoami": {
                "get": {"security": [{"serviceApiKey": []}], "responses": {"200": {"description": "Service identity"}}}
            },
            "/api/v1/service-accounts": {
                "get": {"security": [{"userAccessToken": []}], "responses": {"200": {"description": "Service account list"}}},
                "post": {"security": [{"userAccessToken": []}], "responses": {"201": {"description": "Account and one-time API key"}}}
            },
            "/api/v1/router/destinations": {
                "get": {"summary": "List DICOM route destinations", "security": [{"serviceApiKey": []}, {"userAccessToken": []}], "responses": {"200": {"description": "Destination list and connection state"}}},
                "post": {"summary": "Register a station connection request", "security": [{"serviceApiKey": []}, {"userAccessToken": []}], "responses": {"201": {"description": "Pending station request created"}}}
            },
            "/api/v1/router/destinations/{id}/approve": {
                "post": {"summary": "Approve a station request and test connectivity", "security": [{"userAccessToken": []}], "responses": {"200": {"description": "Approved station and connection state"}}}
            },
            "/api/v1/router/node": {
                "get": {"summary": "Get the local PACS DIMSE AE and listening endpoint", "security": [{"serviceApiKey": []}, {"userAccessToken": []}], "responses": {"200": {"description": "Local DICOM node identity"}}}
            },
            "/api/v1/router/destinations/{id}/test": {
                "post": {"summary": "Run C-ECHO or STOW connectivity test", "security": [{"serviceApiKey": []}, {"userAccessToken": []}], "responses": {"200": {"description": "Updated connection state"}}}
            },
            "/api/v1/router/peers": {
                "get": {"summary": "List inbound DIMSE peers observed by Calling AE and source IP", "security": [{"serviceApiKey": []}, {"userAccessToken": []}], "responses": {"200": {"description": "Observed inbound device list"}}}
            },
            "/api/v1/router/series": {
                "get": {"summary": "List routable Study and Series selections", "security": [{"serviceApiKey": []}, {"userAccessToken": []}], "responses": {"200": {"description": "Recent routable series, scoped to the caller institution"}}}
            },
            "/api/v1/router/rules": {
                "get": {"summary": "List automatic route rules", "security": [{"serviceApiKey": []}, {"userAccessToken": []}], "responses": {"200": {"description": "Rule list"}}},
                "post": {"summary": "Create an automatic route rule", "security": [{"serviceApiKey": []}, {"userAccessToken": []}], "responses": {"201": {"description": "Rule created"}}}
            },
            "/api/v1/router/send": {
                "post": {"summary": "Queue a Study or Series for routing", "security": [{"serviceApiKey": []}, {"userAccessToken": []}], "responses": {"202": {"description": "Per-instance deliveries queued"}}}
            },
            "/api/v1/router/deliveries": {
                "get": {"summary": "List route delivery state and dead letters", "security": [{"serviceApiKey": []}, {"userAccessToken": []}], "responses": {"200": {"description": "Delivery list"}}}
            },
            "/api/v1/router/deliveries/{id}/replay": {
                "post": {"summary": "Replay a dead-letter delivery", "security": [{"serviceApiKey": []}, {"userAccessToken": []}], "responses": {"202": {"description": "Replay queued"}}}
            },
            "/dicomweb/studies": {
                "post": {
                    "summary": "STOW-RS Store Instances",
                    "security": [{"serviceApiKey": []}, {"userAccessToken": []}],
                    "requestBody": {"required": true, "content": {"multipart/related; type=application/dicom": {}}},
                    "responses": {
                        "200": {"description": "All instances stored"},
                        "202": {"description": "Partially stored"},
                        "409": {"description": "No instance stored"}
                    }
                }
            }
        }
    });
    document["paths"]
        .as_object_mut()
        .expect("OpenAPI paths must be an object")
        .extend(lifecycle_openapi_paths());
    Json(document)
}

fn lifecycle_openapi_paths() -> serde_json::Map<String, serde_json::Value> {
    let serde_json::Value::Object(paths) = serde_json::json!({
        "/api/v1/lifecycle/summary": {
            "get": {"summary": "Get storage tier and governance totals", "security": [{"serviceApiKey": []}, {"userAccessToken": []}], "responses": {"200": {"description": "Lifecycle summary"}}}
        },
        "/api/v1/lifecycle/policies": {
            "get": {"summary": "List lifecycle policies", "security": [{"serviceApiKey": []}, {"userAccessToken": []}], "responses": {"200": {"description": "Policy list"}}},
            "post": {"summary": "Create a disabled lifecycle policy", "security": [{"serviceApiKey": []}, {"userAccessToken": []}], "responses": {"201": {"description": "Policy created"}}}
        },
        "/api/v1/lifecycle/policies/{id}": {
            "put": {"summary": "Update, enable or disable a lifecycle policy", "security": [{"serviceApiKey": []}, {"userAccessToken": []}], "responses": {"200": {"description": "Policy updated"}, "409": {"description": "Current definition has not been previewed"}}},
            "delete": {"summary": "Delete a lifecycle policy", "security": [{"serviceApiKey": []}, {"userAccessToken": []}], "responses": {"204": {"description": "Policy deleted"}}}
        },
        "/api/v1/lifecycle/policies/{id}/preview": {
            "post": {"summary": "Preview policy matches and storage impact", "security": [{"serviceApiKey": []}, {"userAccessToken": []}], "responses": {"200": {"description": "Preview summary"}}}
        },
        "/api/v1/lifecycle/policies/{id}/run": {
            "post": {"summary": "Queue an enabled lifecycle policy", "security": [{"serviceApiKey": []}, {"userAccessToken": []}], "responses": {"202": {"description": "Lifecycle job queued"}}}
        },
        "/api/v1/lifecycle/studies": {
            "get": {"summary": "List Study storage tiers, sizes and Legal Hold state", "security": [{"serviceApiKey": []}, {"userAccessToken": []}], "responses": {"200": {"description": "Lifecycle Study list"}}}
        },
        "/api/v1/lifecycle/studies/{study_uid}/move": {
            "post": {"summary": "Queue a Study move to cold storage or quarantine", "security": [{"serviceApiKey": []}, {"userAccessToken": []}], "responses": {"202": {"description": "Lifecycle job queued"}}}
        },
        "/api/v1/lifecycle/studies/{study_uid}/restore": {
            "post": {"summary": "Queue a Study restore to hot storage", "security": [{"serviceApiKey": []}, {"userAccessToken": []}], "responses": {"202": {"description": "Restore job queued"}}}
        },
        "/api/v1/lifecycle/studies/{study_uid}/holds": {
            "post": {"summary": "Create a Study Legal Hold", "security": [{"serviceApiKey": []}, {"userAccessToken": []}], "responses": {"201": {"description": "Legal Hold created"}}}
        },
        "/api/v1/lifecycle/holds": {
            "get": {"summary": "List Study Legal Holds", "security": [{"serviceApiKey": []}, {"userAccessToken": []}], "responses": {"200": {"description": "Legal Hold list"}}}
        },
        "/api/v1/lifecycle/holds/{id}": {
            "delete": {"summary": "Release a Study Legal Hold", "security": [{"serviceApiKey": []}, {"userAccessToken": []}], "responses": {"200": {"description": "Legal Hold released"}}}
        },
        "/api/v1/lifecycle/purge-requests": {
            "get": {"summary": "List Study purge requests", "security": [{"serviceApiKey": []}, {"userAccessToken": []}], "responses": {"200": {"description": "Purge request list"}}},
            "post": {"summary": "Request purge for a quarantined Study", "security": [{"serviceApiKey": []}, {"userAccessToken": []}], "responses": {"201": {"description": "Purge request created"}}}
        },
        "/api/v1/lifecycle/purge-requests/{id}/approve": {
            "post": {"summary": "Approve purge with a grace period", "security": [{"serviceApiKey": []}, {"userAccessToken": []}], "responses": {"200": {"description": "Purge approved"}, "409": {"description": "Legal Hold or invalid request state"}}}
        },
        "/api/v1/lifecycle/purge-requests/{id}/reject": {
            "post": {"summary": "Reject a pending purge request", "security": [{"serviceApiKey": []}, {"userAccessToken": []}], "responses": {"200": {"description": "Purge rejected"}}}
        },
        "/api/v1/lifecycle/jobs": {
            "get": {"summary": "List lifecycle background jobs", "security": [{"serviceApiKey": []}, {"userAccessToken": []}], "responses": {"200": {"description": "Lifecycle job list"}}}
        },
        "/api/v1/lifecycle/events": {
            "get": {"summary": "List append-only lifecycle audit events", "security": [{"serviceApiKey": []}, {"userAccessToken": []}], "responses": {"200": {"description": "Lifecycle event list"}}}
        }
    }) else {
        unreachable!("lifecycle OpenAPI paths must be an object")
    };
    paths
}

async fn authenticate_api_key(
    pool: &PgPool,
    secret: &str,
) -> Result<Option<ServiceIdentity>, sqlx::Error> {
    let Some(prefix) = lookup_prefix(secret) else {
        return Ok(None);
    };
    let hash = hash_key(secret);
    let row = sqlx::query(
        r#"
        UPDATE service_api_keys k
        SET last_used_at = now()
        FROM service_accounts a
        WHERE k.key_prefix = $1 AND k.secret_hash = $2
          AND k.service_account_fk = a.id
          AND k.revoked_at IS NULL
          AND (k.expires_at IS NULL OR k.expires_at > now())
          AND a.is_active
          AND (a.expires_at IS NULL OR a.expires_at > now())
        RETURNING a.id, a.institution_id, a.name, a.scopes
        "#,
    )
    .bind(prefix)
    .bind(hash.as_slice())
    .fetch_optional(pool)
    .await?;
    if let Some(row) = &row {
        let account_id: Uuid = row.try_get("id")?;
        sqlx::query("UPDATE service_accounts SET last_used_at = now() WHERE id = $1")
            .bind(account_id)
            .execute(pool)
            .await?;
    }
    row.map(|row| {
        let raw_scopes: Vec<String> = row.try_get("scopes")?;
        Ok(ServiceIdentity {
            service_account_id: row.try_get("id")?,
            institution_id: row.try_get("institution_id")?,
            name: row.try_get("name")?,
            scopes: raw_scopes
                .iter()
                .filter_map(|scope| ApiScope::parse(scope))
                .collect(),
        })
    })
    .transpose()
}

fn validate_name_and_scopes(
    name: &str,
    scopes: &[ApiScope],
    expires_at: Option<DateTime<Utc>>,
) -> Result<(), ServiceApiError> {
    if !(3..=100).contains(&name.trim().chars().count()) {
        return Err(ServiceApiError::bad_request(
            "服务账号名称长度必须为 3–100 个字符",
        ));
    }
    if scopes.is_empty() {
        return Err(ServiceApiError::bad_request("至少选择一个 API scope"));
    }
    let unique: std::collections::HashSet<_> = scopes.iter().collect();
    if unique.len() != scopes.len() {
        return Err(ServiceApiError::bad_request("API scope 不能重复"));
    }
    if expires_at.is_some_and(|value| value <= Utc::now()) {
        return Err(ServiceApiError::bad_request("过期时间必须晚于当前时间"));
    }
    Ok(())
}

fn generate_key() -> String {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    format!("{KEY_MARKER}{}", URL_SAFE_NO_PAD.encode(bytes))
}

fn hash_key(key: &str) -> [u8; 32] {
    Sha256::digest(key.as_bytes()).into()
}

fn lookup_prefix(key: &str) -> Option<&str> {
    (key.starts_with(KEY_MARKER) && key.len() >= LOOKUP_PREFIX_LEN)
        .then(|| &key[..LOOKUP_PREFIX_LEN])
}

struct RateLimiter {
    entries: Mutex<HashMap<String, RateWindow>>,
    limit: u32,
    window: StdDuration,
}

struct RateWindow {
    started_at: Instant,
    count: u32,
}

impl RateLimiter {
    fn new(limit: u32, window: StdDuration) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            limit,
            window,
        }
    }

    fn check(&self, key: &str) -> Result<(), u64> {
        let now = Instant::now();
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if entries.len() > 10_000 {
            entries.retain(|_, entry| now.duration_since(entry.started_at) < self.window);
        }
        let entry = entries.entry(key.to_owned()).or_insert(RateWindow {
            started_at: now,
            count: 0,
        });
        let elapsed = now.duration_since(entry.started_at);
        if elapsed >= self.window {
            entry.started_at = now;
            entry.count = 0;
        }
        if entry.count >= self.limit {
            return Err(self.window.saturating_sub(elapsed).as_secs().max(1));
        }
        entry.count += 1;
        Ok(())
    }
}

fn bearer_token(request: &Request) -> Option<&str> {
    let value = request
        .headers()
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let (scheme, token) = value.split_once(' ')?;
    (scheme.eq_ignore_ascii_case("Bearer") && !token.trim().is_empty()).then(|| token.trim())
}

fn api_unauthorized(message: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Bearer")],
        Json(serde_json::json!({"error": {"code": "invalid_api_key", "message": message}})),
    )
        .into_response()
}

fn decode_account(row: &sqlx::postgres::PgRow) -> Result<ServiceAccountRecord, ServiceApiError> {
    let raw_scopes: Vec<String> = row.try_get("scopes")?;
    let scopes = raw_scopes
        .iter()
        .map(|scope| {
            ApiScope::parse(scope).ok_or_else(|| {
                ServiceApiError::Internal(format!("数据库包含未知 API scope {scope:?}"))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ServiceAccountRecord {
        id: row.try_get("id")?,
        institution_id: row.try_get("institution_id")?,
        name: row.try_get("name")?,
        scopes,
        is_active: row.try_get("is_active")?,
        expires_at: row.try_get("expires_at")?,
        created_at: row.try_get("created_at")?,
        last_used_at: row.try_get("last_used_at")?,
    })
}

#[derive(Debug, thiserror::Error)]
enum ServiceApiError {
    #[error("请求无效: {0}")]
    BadRequest(String),
    #[error("资源不存在")]
    NotFound,
    #[error("数据库操作失败")]
    Database(#[from] sqlx::Error),
    #[error("内部错误: {0}")]
    Internal(String),
}

impl ServiceApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self::BadRequest(message.into())
    }

    fn database(error: sqlx::Error) -> Self {
        if matches!(&error, sqlx::Error::Database(db) if db.is_unique_violation()) {
            Self::BadRequest("同一机构内服务账号名称不能重复".to_owned())
        } else {
            Self::Database(error)
        }
    }
}

impl IntoResponse for ServiceApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, "bad_request", message),
            Self::NotFound => (StatusCode::NOT_FOUND, "not_found", "资源不存在".to_owned()),
            Self::Database(error) => {
                tracing::error!(%error, "服务账号数据库操作失败");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "内部错误".to_owned(),
                )
            }
            Self::Internal(message) => {
                tracing::error!(%message, "服务账号数据无效");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "内部错误".to_owned(),
                )
            }
        };
        (
            status,
            Json(serde_json::json!({"error": {"code": code, "message": message}})),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_keys_are_prefixed_random_and_hashable() {
        let first = generate_key();
        let second = generate_key();
        assert!(first.starts_with(KEY_MARKER));
        assert_ne!(first, second);
        assert_ne!(hash_key(&first), hash_key(&second));
        assert_eq!(lookup_prefix(&first).map(str::len), Some(LOOKUP_PREFIX_LEN));
    }

    #[test]
    fn malformed_keys_have_no_lookup_prefix() {
        assert!(lookup_prefix("not-a-pacs-key").is_none());
        assert!(lookup_prefix("pacs_sk_short").is_none());
    }

    #[test]
    fn admin_scope_implies_every_scope() {
        let identity = ServiceIdentity {
            service_account_id: Uuid::nil(),
            institution_id: 1,
            name: "integration".to_owned(),
            scopes: vec![ApiScope::Admin],
        };
        for scope in ApiScope::ALL {
            assert!(identity.can(scope));
        }
    }

    #[test]
    fn scopes_match_database_constraint() {
        let migration = include_str!("../../pacs-db/migrations/0010_service_accounts.sql");
        for scope in ApiScope::ALL {
            assert!(migration.contains(&format!("'{}'", scope.as_str())));
        }
    }

    #[test]
    fn rate_limiter_isolated_by_key() {
        let limiter = RateLimiter::new(2, StdDuration::from_secs(60));
        assert!(limiter.check("key-a").is_ok());
        assert!(limiter.check("key-a").is_ok());
        assert!(limiter.check("key-a").is_err());
        assert!(limiter.check("key-b").is_ok());
    }
}
