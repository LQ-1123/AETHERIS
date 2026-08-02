//! 认证与授权中间件。
//!
//! # 为什么挂在路由树上而不是写进每个 handler
//!
//! 权限检查写进 handler 的话,新增一条路由时**漏写就是默认放行** ——
//! 一个未设防的查询接口等于把全部病人元数据公开,而这种缺口在代码审阅里
//! 很难看出来(少一行,不是多一行)。挂在路由层上则相反:新路由默认继承保护,
//! 想要放行必须显式挂到另一棵子树上。
//!
//! # 只验签,不查库
//!
//! access token 是 JWT,15 分钟有效期,验签不需要数据库。这条热路径上每请求
//! 一次查库会成为瓶颈,而 15 分钟的吊销延迟已经由短 TTL 兜住(见 [`crate::token`])。
//! 需要完整 [`User`](crate::User) 的 handler 自己调 [`AuthService::load_user`]。

use std::sync::Arc;

use axum::extract::Request;
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::{AccessClaims, AuthService, Permission, Role};

/// 通过认证的调用方身份。
///
/// 由中间件放进请求扩展,handler 用 `Extension<Identity>` 取。
#[derive(Debug, Clone)]
pub struct Identity {
    pub user_id: i64,
    pub institution_id: i64,
    pub username: String,
    pub role: Role,
}

impl Identity {
    fn from_claims(claims: &AccessClaims) -> Option<Self> {
        Some(Self {
            user_id: claims.sub,
            institution_id: claims.institution_id,
            username: claims.username.clone(),
            role: claims.role().ok()?,
        })
    }
}

/// 要求调用方已认证且具备某项权限。
///
/// 用法:
/// ```ignore
/// Router::new()
///     .route("/studies", get(handler))
///     .with_state(state)
///     .layer(middleware::from_fn(move |req, next| {
///         require(auth.clone(), Permission::ViewImages, req, next)
///     }))
/// ```
pub async fn require(
    auth: Arc<AuthService>,
    permission: Permission,
    mut request: Request,
    next: Next,
) -> Response {
    let Some(token) = bearer_token(&request) else {
        // 401 带 WWW-Authenticate 是 RFC 6750 的要求,客户端靠它知道该去拿令牌
        return unauthorized("缺少 Bearer 令牌");
    };

    let claims = match auth.verify_access_token(token) {
        Ok(claims) => claims,
        Err(error) => {
            // 令牌无效的原因不回给调用方 —— 区分"签名错"和"已过期"会给
            // 攻击者提供试探反馈。日志里留全貌。
            tracing::debug!(%error, "access token 校验失败");
            return unauthorized("令牌无效或已过期");
        }
    };

    let Some(identity) = Identity::from_claims(&claims) else {
        // 角色字段无法识别:令牌是我们自己签的,出现这种情况说明角色枚举
        // 变更后有旧令牌还在流通。拒绝而不是当成最低权限 —— 猜测权限是危险的。
        tracing::warn!(role = %claims.role, "令牌里的角色无法识别");
        return unauthorized("令牌无效或已过期");
    };

    if !identity.role.can(permission) {
        tracing::info!(
            username = %identity.username,
            role = %identity.role,
            ?permission,
            "权限不足,拒绝访问"
        );
        // 403 而不是 404:调用方身份有效,只是权限不够。用 404 掩盖资源存在性
        // 在这里没有意义 —— 端点本身是公开文档化的。
        return forbidden();
    }

    request.extensions_mut().insert(identity);
    next.run(request).await
}

/// 从 `Authorization: Bearer <token>` 取令牌。
fn bearer_token(request: &Request) -> Option<&str> {
    let value = request
        .headers()
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    // 方案名大小写不敏感(RFC 7235 §2.1)
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("Bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then_some(token)
}

fn unauthorized(message: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Bearer")],
        axum::Json(serde_json::json!({ "error": message })),
    )
        .into_response()
}

fn forbidden() -> Response {
    (
        StatusCode::FORBIDDEN,
        axum::Json(serde_json::json!({ "error": "权限不足" })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;

    fn request_with(authorization: Option<&str>) -> Request {
        let mut builder = Request::builder().uri("/");
        if let Some(value) = authorization {
            builder = builder.header(header::AUTHORIZATION, value);
        }
        builder.body(Body::empty()).unwrap()
    }

    #[test]
    fn extracts_a_bearer_token() {
        let request = request_with(Some("Bearer abc.def.ghi"));
        assert_eq!(bearer_token(&request), Some("abc.def.ghi"));
    }

    /// 方案名大小写不敏感(RFC 7235 §2.1)—— 有客户端送 `bearer`。
    #[test]
    fn scheme_name_is_case_insensitive() {
        for scheme in ["Bearer", "bearer", "BEARER", "BeArEr"] {
            let request = request_with(Some(&format!("{scheme} tok")));
            assert_eq!(bearer_token(&request), Some("tok"), "应接受 {scheme}");
        }
    }

    #[test]
    fn rejects_other_schemes_and_malformed_values() {
        for value in [
            "Basic dXNlcjpwYXNz", // 不是 Bearer
            "Bearer",             // 没有令牌
            "Bearer ",            // 令牌为空
            "abc.def.ghi",        // 没有方案名
            "",
        ] {
            let request = request_with(Some(value));
            assert_eq!(bearer_token(&request), None, "应拒绝 {value:?}");
        }
        assert_eq!(bearer_token(&request_with(None)), None);
    }

    /// 权限矩阵在中间件这一层生效:Viewer 能查、不能上传。
    #[test]
    fn permission_matrix_is_enforced_through_the_role() {
        let viewer = Identity {
            user_id: 1,
            institution_id: 1,
            username: "v".into(),
            role: Role::Viewer,
        };
        assert!(viewer.role.can(Permission::ViewImages));
        assert!(!viewer.role.can(Permission::UploadImages));
        assert!(!viewer.role.can(Permission::DeleteImages));
    }
}
