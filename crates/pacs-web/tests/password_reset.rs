use std::sync::{Arc, OnceLock};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use chrono::Utc;
use pacs_auth::{AccessTokenCodec, AuthError, AuthService, Role};
use serde_json::{Value, json};
use sqlx::migrate::MigrateDatabase;
use sqlx::{PgPool, Postgres};
use tokio::sync::Mutex;
use tower::ServiceExt;
use uuid::Uuid;

static DB_SETUP: OnceLock<Mutex<()>> = OnceLock::new();

async fn pool() -> Option<PgPool> {
    dotenvy::dotenv().ok();
    let Ok(url) = std::env::var("PACS_TEST_DATABASE_URL") else {
        assert!(
            std::env::var_os("CI").is_none(),
            "CI 必须设置 PACS_TEST_DATABASE_URL"
        );
        eprintln!("\n>>> 跳过密码重置审核测试: 未设置 PACS_TEST_DATABASE_URL\n");
        return None;
    };
    let slot = DB_SETUP.get_or_init(|| Mutex::new(()));
    let _guard = slot.lock().await;
    if !Postgres::database_exists(&url).await.unwrap_or(false) {
        Postgres::create_database(&url)
            .await
            .expect("应能创建测试库");
    }
    let setup_pool = pacs_db::connect(&url).await.expect("应能连接测试库");
    pacs_db::migrate(&setup_pool).await.expect("迁移应能应用");
    drop(setup_pool);
    drop(_guard);
    pacs_db::connect(&url).await.ok()
}

async fn response_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}

async fn create_user(pool: &PgPool, role: Role, password: &str, prefix: &str) -> (i64, String) {
    let username = format!("{prefix}-{}", Uuid::new_v4().simple());
    let password_hash = pacs_auth::password::hash(password).unwrap();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO users(institution_id,username,password_hash,role) VALUES(1,$1,$2,$3) RETURNING id",
    )
    .bind(&username)
    .bind(password_hash)
    .bind(role.as_str())
    .fetch_one(pool)
    .await
    .unwrap();
    (id, username)
}

/// 用户提交的新密码只有管理员批准后才生效；拒绝申请不会改变当前密码。
#[tokio::test]
async fn user_requested_password_reset_requires_admin_approval() {
    let Some(pool) = pool().await else { return };
    let secret = b"password-reset-test-secret-at-least-32";
    let old_password = "old-password-secure-2026";
    let new_password = "new-password-secure-2026";
    let rejected_password = "rejected-password-secure-2026";
    let (user_id, username) =
        create_user(&pool, Role::Radiologist, old_password, "reset-user").await;
    let (admin_id, admin_username) = create_user(
        &pool,
        Role::Admin,
        "admin-password-secure-2026",
        "reset-admin",
    )
    .await;
    let admin_token = AccessTokenCodec::new(secret)
        .unwrap()
        .issue(admin_id, 1, &admin_username, Role::Admin, Utc::now())
        .unwrap();
    let bearer = format!("Bearer {admin_token}");
    let auth = Arc::new(AuthService::new(pool.clone(), secret).unwrap());
    let app = Router::new()
        .nest("/auth", pacs_auth::http::routes(auth.clone()))
        .nest(
            "/api/v1",
            pacs_web::clinical_routes(pacs_web::WebState::new(pool.clone()), auth.clone()),
        );

    // 先建立一个真实 refresh token，用于验证批准后会话吊销。
    auth.login(&username, old_password, None, None)
        .await
        .expect("原密码应能登录");

    let submitted = app
        .clone()
        .oneshot(
            Request::post("/auth/password-reset-requests")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"username":username,"new_password":new_password}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(submitted.status(), StatusCode::ACCEPTED);

    // 未登录调用方不能查看申请；管理员看到的结构中也绝不包含密码哈希。
    let unauthenticated = app
        .clone()
        .oneshot(
            Request::get("/api/v1/password-reset-requests")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
    let listed = app
        .clone()
        .oneshot(
            Request::get("/api/v1/password-reset-requests")
                .header(header::AUTHORIZATION, &bearer)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let listed = response_json(listed).await;
    let request = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["username"] == username)
        .expect("管理员应看到申请");
    assert!(request.get("password_hash").is_none());
    let request_id = request["id"].as_i64().unwrap();

    assert!(matches!(
        auth.login(&username, new_password, None, None).await,
        Err(AuthError::InvalidCredentials)
    ));

    let approved = app
        .clone()
        .oneshot(
            Request::post(format!(
                "/api/v1/password-reset-requests/{request_id}/approve"
            ))
            .header(header::AUTHORIZATION, &bearer)
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(approved.status(), StatusCode::OK);
    let approved = response_json(approved).await;
    assert_eq!(approved["status"], "approved");
    assert!(matches!(
        auth.login(&username, old_password, None, None).await,
        Err(AuthError::InvalidCredentials)
    ));
    let revoked: bool = sqlx::query_scalar(
        "SELECT bool_and(revoked_at IS NOT NULL) FROM refresh_tokens WHERE user_fk=$1",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(revoked, "批准重置应吊销此前签发的 refresh token");
    auth.login(&username, new_password, None, None)
        .await
        .expect("批准后新密码应能登录");

    // 第二次申请被拒后，已生效密码保持不变。
    let resubmitted = app
        .clone()
        .oneshot(
            Request::post("/auth/password-reset-requests")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"username":username,"new_password":rejected_password}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resubmitted.status(), StatusCode::ACCEPTED);
    let second_id: i64 = sqlx::query_scalar(
        "SELECT id FROM password_reset_requests WHERE user_fk=$1 AND status='pending'",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let rejected = app
        .clone()
        .oneshot(
            Request::post(format!(
                "/api/v1/password-reset-requests/{second_id}/reject"
            ))
            .header(header::AUTHORIZATION, &bearer)
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::OK);
    auth.login(&username, new_password, None, None)
        .await
        .expect("拒绝申请后当前密码应保持有效");
    assert!(matches!(
        auth.login(&username, rejected_password, None, None).await,
        Err(AuthError::InvalidCredentials)
    ));
}

/// 不存在的用户名也返回相同的受理响应，但不会产生管理员可见记录。
#[tokio::test]
async fn unknown_username_is_accepted_without_creating_request() {
    let Some(pool) = pool().await else { return };
    let secret = b"password-reset-unknown-secret-at-least-32";
    let auth = Arc::new(AuthService::new(pool.clone(), secret).unwrap());
    let app = Router::new().nest("/auth", pacs_auth::http::routes(auth));
    let username = format!("missing-{}", Uuid::new_v4().simple());
    let response = app
        .oneshot(
            Request::post("/auth/password-reset-requests")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"username":username,"new_password":"unknown-user-password-2026"})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM password_reset_requests r JOIN users u ON u.id=r.user_fk WHERE u.username=$1",
    )
    .bind(username)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 0);
}
