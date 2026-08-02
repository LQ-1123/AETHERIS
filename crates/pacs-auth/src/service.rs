//! 认证服务：登录、刷新、登出。

use anyhow::{Context, Result};
use chrono::Utc;
use sqlx::PgPool;
use thiserror::Error;

use crate::{
    AccessClaims, AccessTokenCodec, PasswordError, User, WeakPassword, audit, normalize_username,
    password, repository, token,
};

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("用户名或密码错误")]
    InvalidCredentials,
    #[error("账号已停用")]
    AccountDisabled,
    #[error("必须先修改密码")]
    MustChangePassword,
    #[error("令牌无效或已过期")]
    InvalidToken,
    #[error("令牌已被吊销")]
    TokenRevoked,
    #[error("检测到令牌重放，已吊销整条会话链")]
    TokenReplayed,
    #[error(transparent)]
    Password(#[from] PasswordError),
    #[error(transparent)]
    WeakPassword(#[from] WeakPassword),
    #[error(transparent)]
    Token(#[from] token::TokenError),
    #[error(transparent)]
    Repo(#[from] repository::RepoError),
    #[error("内部错误")]
    Internal(#[from] anyhow::Error),
}

pub struct AuthService {
    pool: PgPool,
    token_codec: AccessTokenCodec,
}

impl AuthService {
    pub fn new(pool: PgPool, jwt_secret: &[u8]) -> Result<Self> {
        let token_codec = AccessTokenCodec::new(jwt_secret).context("JWT 签名密钥初始化失败")?;
        Ok(Self { pool, token_codec })
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// 登录：用户名密码 → access token + refresh token。
    pub async fn login(
        &self,
        username: &str,
        password: &str,
        client_ip: Option<String>,
        user_agent: Option<String>,
    ) -> Result<(String, String, User), AuthError> {
        let normalized = normalize_username(username).map_err(|_| AuthError::InvalidCredentials)?;

        let (user, stored_hash) =
            match repository::find_by_username(&self.pool, &normalized).await? {
                Some(pair) => pair,
                None => {
                    // 用户不存在时也付出同等哈希代价，防时间侧信道
                    password::waste_time_like_a_real_verification();
                    audit::record(
                        &self.pool,
                        audit::Action::Login,
                        audit::Outcome::Failure,
                        audit::Entry::for_attempted_username(&normalized)
                            .with_detail(serde_json::json!({"reason": "用户不存在"})),
                    )
                    .await;
                    return Err(AuthError::InvalidCredentials);
                }
            };

        if !password::verify(password, &stored_hash)? {
            audit::record(
                &self.pool,
                audit::Action::Login,
                audit::Outcome::Failure,
                audit::Entry::for_user(user.id, &user.username, user.role)
                    .with_detail(serde_json::json!({"reason": "密码错误"})),
            )
            .await;
            return Err(AuthError::InvalidCredentials);
        }

        if !user.is_active {
            audit::record(
                &self.pool,
                audit::Action::Login,
                audit::Outcome::Denied,
                audit::Entry::for_user(user.id, &user.username, user.role)
                    .with_detail(serde_json::json!({"reason": "账号已停用"})),
            )
            .await;
            return Err(AuthError::AccountDisabled);
        }

        if user.must_change_password {
            audit::record(
                &self.pool,
                audit::Action::Login,
                audit::Outcome::Denied,
                audit::Entry::for_user(user.id, &user.username, user.role)
                    .with_detail(serde_json::json!({"reason": "必须先修改密码"})),
            )
            .await;
            return Err(AuthError::MustChangePassword);
        }

        let now = Utc::now();
        let access_token =
            self.token_codec
                .issue(user.id, user.institution_id, &user.username, user.role, now)?;

        let refresh = token::generate_refresh_token(now);
        repository::store_refresh_token(
            &self.pool,
            user.id,
            &refresh.hash,
            refresh.expires_at,
            user_agent.as_deref(),
            client_ip.as_deref(),
        )
        .await?;

        repository::touch_last_login(&self.pool, user.id).await?;

        audit::record(
            &self.pool,
            audit::Action::Login,
            audit::Outcome::Success,
            audit::Entry::for_user(user.id, &user.username, user.role),
        )
        .await;

        Ok((access_token, refresh.secret, user))
    }

    /// 刷新：refresh token → 新的 access token + 新的 refresh token（轮换）。
    pub async fn refresh(
        &self,
        refresh_token: &str,
        client_ip: Option<String>,
        user_agent: Option<String>,
    ) -> Result<(String, String, User), AuthError> {
        let token_hash = token::hash_refresh_token(refresh_token);
        let stored = repository::find_refresh_token(&self.pool, &token_hash)
            .await?
            .ok_or(AuthError::InvalidToken)?;

        let now = Utc::now();

        // 检测重放：已经被轮换掉却又被拿来用 → 泄露了
        if stored.is_replayed() {
            repository::revoke_token_chain(&self.pool, stored.id).await?;
            return Err(AuthError::TokenReplayed);
        }

        if !stored.is_usable(now) {
            return Err(if stored.revoked_at.is_some() {
                AuthError::TokenRevoked
            } else {
                AuthError::InvalidToken
            });
        }

        let user = repository::find_by_id(&self.pool, stored.user_fk)
            .await?
            .ok_or_else(|| anyhow::anyhow!("令牌对应的用户 {} 不存在", stored.user_fk))?;

        if !user.is_active {
            return Err(AuthError::AccountDisabled);
        }

        let access_token =
            self.token_codec
                .issue(user.id, user.institution_id, &user.username, user.role, now)?;

        let new_refresh = token::generate_refresh_token(now);
        repository::rotate_refresh_token(
            &self.pool,
            stored.id,
            user.id,
            &new_refresh.hash,
            new_refresh.expires_at,
            user_agent.as_deref(),
            client_ip.as_deref(),
        )
        .await?;

        audit::record(
            &self.pool,
            audit::Action::TokenRefresh,
            audit::Outcome::Success,
            audit::Entry::for_user(user.id, &user.username, user.role),
        )
        .await;

        Ok((access_token, new_refresh.secret, user))
    }

    /// 登出：吊销 refresh token。
    pub async fn logout(&self, refresh_token: &str, user: &User) -> Result<(), AuthError> {
        let token_hash = token::hash_refresh_token(refresh_token);
        if let Some(stored) = repository::find_refresh_token(&self.pool, &token_hash).await? {
            repository::revoke_token_chain(&self.pool, stored.id).await?;
        }

        audit::record(
            &self.pool,
            audit::Action::Logout,
            audit::Outcome::Success,
            audit::Entry::for_user(user.id, &user.username, user.role),
        )
        .await;

        Ok(())
    }

    /// 改密码（吊销该用户的全部会话）。
    pub async fn change_password(
        &self,
        user_id: i64,
        old_password: &str,
        new_password: &str,
    ) -> Result<(), AuthError> {
        let (user, stored_hash) = repository::find_by_username(
            &self.pool,
            &repository::find_by_id(&self.pool, user_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("用户 {} 不存在", user_id))?
                .username,
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("用户 {} 不存在", user_id))?;

        if !password::verify(old_password, &stored_hash)? {
            audit::record(
                &self.pool,
                audit::Action::PasswordChange,
                audit::Outcome::Failure,
                audit::Entry::for_user(user.id, &user.username, user.role)
                    .with_detail(serde_json::json!({"reason": "旧密码错误"})),
            )
            .await;
            return Err(AuthError::InvalidCredentials);
        }

        password::check_strength(new_password, &user.username)?;
        let new_hash = password::hash(new_password)?;

        repository::set_password(&self.pool, user_id, &new_hash, false).await?;

        audit::record(
            &self.pool,
            audit::Action::PasswordChange,
            audit::Outcome::Success,
            audit::Entry::for_user(user.id, &user.username, user.role),
        )
        .await;

        Ok(())
    }

    /// 验证 access token 并返回载荷。
    pub fn verify_access_token(&self, token: &str) -> Result<AccessClaims, AuthError> {
        self.token_codec
            .verify(token)
            .map_err(|_| AuthError::InvalidToken)
    }

    /// 从 claims 加载完整用户信息。
    pub async fn load_user(&self, claims: &AccessClaims) -> Result<User, AuthError> {
        repository::find_by_id(&self.pool, claims.sub)
            .await?
            .ok_or_else(|| anyhow::anyhow!("JWT 里的用户 {} 不存在", claims.sub).into())
    }
}
