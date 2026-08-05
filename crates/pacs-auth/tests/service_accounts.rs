use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use chrono::Utc;
use pacs_auth::{AccessTokenCodec, AuthService, Role};
use serde_json::{Value, json};
use sqlx::migrate::MigrateDatabase;
use sqlx::{PgPool, Postgres};
use tower::ServiceExt;
use uuid::Uuid;

async fn pool() -> Option<PgPool> {
    dotenvy::dotenv().ok();
    let Ok(url) = std::env::var("PACS_TEST_DATABASE_URL") else {
        assert!(
            std::env::var_os("CI").is_none(),
            "CI 必须设置 PACS_TEST_DATABASE_URL"
        );
        eprintln!("\n>>> 跳过服务账号数据库测试: 未设置 PACS_TEST_DATABASE_URL\n");
        return None;
    };
    if !Postgres::database_exists(&url).await.unwrap_or(false) {
        Postgres::create_database(&url)
            .await
            .expect("应能创建测试库");
    }
    let pool = pacs_db::connect(&url).await.expect("应能连接测试库");
    pacs_db::migrate(&pool).await.expect("迁移应能应用");
    Some(pool)
}

#[tokio::test]
async fn api_key_is_shown_once_authenticates_and_revokes() {
    let Some(pool) = pool().await else { return };
    let user_id: i64 = sqlx::query_scalar(
        "INSERT INTO users (institution_id, username, password_hash, role)
         VALUES (1, $1, 'unused', 'admin') RETURNING id",
    )
    .bind(format!("service-test-{}", Uuid::new_v4()))
    .fetch_one(&pool)
    .await
    .unwrap();
    let secret = b"service-account-test-jwt-secret-at-least-32-bytes";
    let token = AccessTokenCodec::new(secret)
        .unwrap()
        .issue(user_id, 1, "service-test-admin", Role::Admin, Utc::now())
        .unwrap();
    let auth = Arc::new(AuthService::new(pool.clone(), secret).unwrap());
    let app = pacs_auth::service_accounts::management_routes(auth.clone())
        .merge(pacs_auth::service_accounts::service_routes(auth));

    let response = app
        .clone()
        .oneshot(
            Request::post("/service-accounts")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "name": format!("integration-{}", Uuid::new_v4()),
                        "scopes": ["read", "upload"]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let created: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
            .unwrap();
    let api_key = created["api_key"].as_str().unwrap();
    let account_id = created["account"]["id"].as_str().unwrap();
    let key_id = created["key_id"].as_str().unwrap();
    assert!(api_key.starts_with("pacs_sk_"));

    let authenticated = app
        .clone()
        .oneshot(
            Request::get("/service-auth/whoami")
                .header(header::AUTHORIZATION, format!("Bearer {api_key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(authenticated.status(), StatusCode::OK);

    let revoked = app
        .clone()
        .oneshot(
            Request::delete(format!("/service-accounts/{account_id}/keys/{key_id}"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revoked.status(), StatusCode::NO_CONTENT);

    let rejected = app
        .oneshot(
            Request::get("/service-auth/whoami")
                .header(header::AUTHORIZATION, format!("Bearer {api_key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);
}
