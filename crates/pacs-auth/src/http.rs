//! HTTP 认证 API 路由。

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::{AuthError, AuthService, User, repository, token};

pub fn routes(auth_service: Arc<AuthService>) -> Router {
    Router::new()
        .route("/login", axum::routing::post(login))
        .route("/refresh", axum::routing::post(refresh))
        .route("/logout", axum::routing::post(logout))
        .route("/change-password", axum::routing::post(change_password))
        .route(
            "/password-reset-requests",
            axum::routing::post(request_password_reset),
        )
        .with_state(auth_service)
}

#[derive(Debug, Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Debug, Serialize)]
struct LoginResponse {
    access_token: String,
    refresh_token: String,
    user: UserInfo,
}

#[derive(Debug, Serialize)]
struct UserInfo {
    id: i64,
    username: String,
    display_name: Option<String>,
    role: String,
    institution_id: i64,
    institution_name: String,
}

impl UserInfo {
    fn from_user(user: User, institution_name: String) -> Self {
        Self {
            id: user.id,
            username: user.username,
            display_name: user.display_name,
            role: user.role.to_string(),
            institution_id: user.institution_id,
            institution_name,
        }
    }
}

async fn login(
    State(service): State<Arc<AuthService>>,
    Json(req): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, ApiError> {
    let (access_token, refresh_token, user) = service
        .login(&req.username, &req.password, None, None)
        .await?;
    let institution_name: String = sqlx::query_scalar("SELECT name FROM institutions WHERE id=$1")
        .bind(user.institution_id)
        .fetch_one(service.pool())
        .await
        .map_err(repository::RepoError::from)?;

    Ok(Json(LoginResponse {
        access_token,
        refresh_token,
        user: UserInfo::from_user(user, institution_name),
    }))
}

#[derive(Debug, Deserialize)]
struct RefreshRequest {
    refresh_token: String,
}

#[derive(Debug, Serialize)]
struct RefreshResponse {
    access_token: String,
    refresh_token: String,
}

async fn refresh(
    State(service): State<Arc<AuthService>>,
    Json(req): Json<RefreshRequest>,
) -> Result<Json<RefreshResponse>, ApiError> {
    let (access_token, refresh_token, _user) =
        service.refresh(&req.refresh_token, None, None).await?;

    Ok(Json(RefreshResponse {
        access_token,
        refresh_token,
    }))
}

#[derive(Debug, Deserialize)]
struct LogoutRequest {
    refresh_token: String,
}

async fn logout(
    State(service): State<Arc<AuthService>>,
    Json(req): Json<LogoutRequest>,
) -> Result<StatusCode, ApiError> {
    // TODO: 从 JWT 提取用户信息（需要 middleware）
    // 当前简化版：只吊销令牌，不记审计
    let token_hash = token::hash_refresh_token(&req.refresh_token);
    if let Some(stored) = repository::find_refresh_token(service.pool(), &token_hash).await? {
        repository::revoke_token_chain(service.pool(), stored.id).await?;
    }

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ChangePasswordRequest {
    username: String,
    old_password: String,
    new_password: String,
}

async fn change_password(
    State(service): State<Arc<AuthService>>,
    Json(req): Json<ChangePasswordRequest>,
) -> Result<StatusCode, ApiError> {
    let username = crate::normalize_username(&req.username)
        .map_err(|_| ApiError(AuthError::InvalidCredentials))?;
    let Some((user, stored_hash)) = repository::find_by_username(service.pool(), &username).await?
    else {
        crate::password::waste_time_like_a_real_verification();
        return Err(ApiError(AuthError::InvalidCredentials));
    };
    if !user.is_active
        || !crate::password::verify(&req.old_password, &stored_hash).map_err(AuthError::from)?
    {
        return Err(ApiError(AuthError::InvalidCredentials));
    }
    crate::password::check_strength(&req.new_password, &username).map_err(AuthError::from)?;
    let password_hash = crate::password::hash(&req.new_password).map_err(AuthError::from)?;
    repository::set_password(service.pool(), user.id, &password_hash, false).await?;
    crate::audit::record(
        service.pool(),
        crate::audit::Action::PasswordChange,
        crate::audit::Outcome::Success,
        crate::audit::Entry::for_user(user.id, &user.username, user.role),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
struct PasswordResetRequestBody {
    username: String,
    new_password: String,
}

async fn request_password_reset(
    State(service): State<Arc<AuthService>>,
    Json(req): Json<PasswordResetRequestBody>,
) -> Result<StatusCode, ApiError> {
    let username = crate::normalize_username(&req.username)
        .map_err(|_| ApiError(AuthError::InvalidCredentials))?;
    crate::password::check_strength(&req.new_password, &username).map_err(AuthError::from)?;
    // 无论账号是否存在都执行 Argon2，避免用响应耗时枚举有效用户名。
    let password_hash = crate::password::hash(&req.new_password).map_err(AuthError::from)?;
    let request_id =
        repository::submit_password_reset_request(service.pool(), &username, &password_hash)
            .await?;
    crate::audit::record(
        service.pool(),
        crate::audit::Action::PasswordResetRequested,
        crate::audit::Outcome::Success,
        crate::audit::Entry::for_attempted_username(&username).with_detail(
            serde_json::json!({"queued": request_id.is_some(), "request_id": request_id}),
        ),
    )
    .await;
    Ok(StatusCode::ACCEPTED)
}

/// API 错误响应。
#[derive(Debug)]
struct ApiError(AuthError);

impl From<AuthError> for ApiError {
    fn from(e: AuthError) -> Self {
        Self(e)
    }
}

impl From<repository::RepoError> for ApiError {
    fn from(e: repository::RepoError) -> Self {
        Self(AuthError::Repo(e))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self.0 {
            AuthError::InvalidCredentials => (StatusCode::UNAUTHORIZED, "用户名或密码错误"),
            AuthError::AccountDisabled => (StatusCode::FORBIDDEN, "账号已停用"),
            AuthError::MustChangePassword => (StatusCode::FORBIDDEN, "必须先修改密码"),
            AuthError::InvalidToken => (StatusCode::UNAUTHORIZED, "令牌无效或已过期"),
            AuthError::TokenRevoked => (StatusCode::UNAUTHORIZED, "令牌已被吊销"),
            AuthError::TokenReplayed => (StatusCode::UNAUTHORIZED, "检测到令牌重放"),
            AuthError::WeakPassword(_) => (StatusCode::BAD_REQUEST, "密码强度不足"),
            AuthError::Password(_)
            | AuthError::Token(_)
            | AuthError::Repo(_)
            | AuthError::Internal(_) => {
                tracing::error!(error = ?self.0, "API 内部错误");
                (StatusCode::INTERNAL_SERVER_ERROR, "内部错误")
            }
        };

        (status, Json(serde_json::json!({"error": message}))).into_response()
    }
}
