use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use chrono::Utc;
use pacs_auth::{AccessTokenCodec, AuthService, Role};
use pacs_core::fixture::{ct_instance, unique_uid};
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
        eprintln!("\n>>> 跳过 STOW-RS 数据库测试: 未设置 PACS_TEST_DATABASE_URL\n");
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
async fn authenticated_stow_stores_and_deduplicates_a_part10_instance() {
    let Some(pool) = pool().await else { return };
    let user_id: i64 = sqlx::query_scalar(
        "INSERT INTO users (institution_id, username, password_hash, role)
         VALUES (1, $1, 'unused', 'technician') RETURNING id",
    )
    .bind(format!("stow-test-{}", Uuid::new_v4()))
    .fetch_one(&pool)
    .await
    .unwrap();
    let secret = b"stow-test-jwt-secret-at-least-thirty-two-bytes";
    let token = AccessTokenCodec::new(secret)
        .unwrap()
        .issue(user_id, 1, "stow-test", Role::Technician, Utc::now())
        .unwrap();
    let auth = Arc::new(AuthService::new(pool.clone(), secret).unwrap());
    let temp = tempfile::tempdir().unwrap();
    let store = Arc::new(pacs_store::Store::open(temp.path()).await.unwrap());
    let app = pacs_web::dicomweb_routes(pacs_web::WebState::with_store(pool.clone(), store), auth);

    let study = unique_uid();
    let series = unique_uid();
    let sop = unique_uid();
    let object = ct_instance(&study, &series, &sop);
    let mut encoded = Vec::new();
    object.write_all(&mut encoded).unwrap();
    let body = multipart_body("stow-boundary", &encoded);

    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(
                Request::post("/studies")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(
                        header::CONTENT_TYPE,
                        "multipart/related; type=\"application/dicom\"; boundary=stow-boundary",
                    )
                    .body(Body::from(body.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM instances i
         JOIN series se ON se.id = i.series_fk
         JOIN studies st ON st.id = se.study_fk
         WHERE i.sop_instance_uid = $1 AND st.institution_id = 1",
    )
    .bind(&sop)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "重复 STOW 必须幂等");
}

fn multipart_body(boundary: &str, dicom: &[u8]) -> Vec<u8> {
    let mut body = format!("--{boundary}\r\nContent-Type: application/dicom\r\n\r\n").into_bytes();
    body.extend_from_slice(dicom);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}
