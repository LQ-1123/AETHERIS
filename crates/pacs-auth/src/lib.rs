//! 账号、访问控制与审计。
//!
//! - 密码用 argon2id 哈希([`password`])
//! - 短命 access token(JWT) + 可吊销 refresh token([`token`])
//! - 角色权限矩阵集中在 [`model::Role::can`]
//! - 审计日志写数据库([`audit`])

pub mod audit;
pub mod http;
pub mod middleware;
pub mod model;
pub mod password;
pub mod repository;
pub mod service;
pub mod service_accounts;
pub mod token;

pub use middleware::{Identity, require};
pub use model::{
    InvalidUsername, PasswordResetRequest, Permission, Role, User, normalize_username,
};
pub use password::{PasswordError, WeakPassword};
pub use service::{AuthError, AuthService};
pub use service_accounts::{ApiScope, ServiceIdentity};
pub use token::{AccessClaims, AccessTokenCodec, RefreshToken, TokenError};
