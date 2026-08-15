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
        eprintln!("\n>>> 跳过用户窗预设 API 测试: 未设置 PACS_TEST_DATABASE_URL\n");
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

async fn response_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}

#[tokio::test]
async fn authenticated_users_manage_only_their_own_presets() {
    let Some(pool) = pool().await else { return };
    let suffix = Uuid::new_v4();
    let first_user: i64 = sqlx::query_scalar(
        "INSERT INTO users (institution_id, username, password_hash, role)
         VALUES (1, $1, 'unused', 'radiologist') RETURNING id",
    )
    .bind(format!("window-api-a-{suffix}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let second_user: i64 = sqlx::query_scalar(
        "INSERT INTO users (institution_id, username, password_hash, role)
         VALUES (1, $1, 'unused', 'radiologist') RETURNING id",
    )
    .bind(format!("window-api-b-{suffix}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let secret = b"window-preset-api-test-secret-at-least-32-bytes";
    let codec = AccessTokenCodec::new(secret).unwrap();
    let first_token = codec
        .issue(first_user, 1, "window-api-a", Role::Radiologist, Utc::now())
        .unwrap();
    let second_token = codec
        .issue(
            second_user,
            1,
            "window-api-b",
            Role::Radiologist,
            Utc::now(),
        )
        .unwrap();
    let auth = Arc::new(AuthService::new(pool.clone(), secret).unwrap());
    let app = pacs_web::window_preset_routes(pacs_web::WebState::new(pool), auth);

    let unauthorized = app
        .clone()
        .oneshot(Request::get("/window-presets").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let created = app
        .clone()
        .oneshot(
            Request::post("/window-presets")
                .header(header::AUTHORIZATION, format!("Bearer {first_token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "modality": "ct",
                        "name": "  我的肺窗  ",
                        "center": -600,
                        "width": 1500,
                        "function": "LINEAR"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let created = response_json(created).await;
    assert_eq!(created["modality"], "CT");
    assert_eq!(created["name"], "我的肺窗");
    let preset_id = created["id"].as_i64().unwrap();

    let first_list = app
        .clone()
        .oneshot(
            Request::get("/window-presets")
                .header(header::AUTHORIZATION, format!("Bearer {first_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response_json(first_list).await.as_array().unwrap().len(), 1);
    let second_list = app
        .clone()
        .oneshot(
            Request::get("/window-presets")
                .header(header::AUTHORIZATION, format!("Bearer {second_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        response_json(second_list)
            .await
            .as_array()
            .unwrap()
            .is_empty()
    );

    let duplicate = app
        .clone()
        .oneshot(
            Request::post("/window-presets")
                .header(header::AUTHORIZATION, format!("Bearer {first_token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "modality": "CT",
                        "name": "我的肺窗",
                        "center": -500,
                        "width": 1400,
                        "function": "LINEAR"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);

    let hidden = app
        .clone()
        .oneshot(
            Request::delete(format!("/window-presets/{preset_id}"))
                .header(header::AUTHORIZATION, format!("Bearer {second_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(hidden.status(), StatusCode::NOT_FOUND);

    let renamed = app
        .clone()
        .oneshot(
            Request::patch(format!("/window-presets/{preset_id}"))
                .header(header::AUTHORIZATION, format!("Bearer {first_token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "name": "胸部" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(renamed.status(), StatusCode::OK);
    assert_eq!(response_json(renamed).await["name"], "胸部");

    let deleted = app
        .oneshot(
            Request::delete(format!("/window-presets/{preset_id}"))
                .header(header::AUTHORIZATION, format!("Bearer {first_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
}
