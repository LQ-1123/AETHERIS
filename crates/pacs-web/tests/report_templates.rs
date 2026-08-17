use std::sync::{Arc, Mutex, OnceLock};

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use chrono::Utc;
use pacs_auth::{AccessTokenCodec, AuthService, Role};
use serde_json::{Value, json};
use sqlx::migrate::MigrateDatabase;
use sqlx::{PgPool, Postgres};
use tower::ServiceExt;
use uuid::Uuid;

/// 建库/迁移串行化（两个测试并发跑时避免同时建库），连接池则每个测试自建——
/// PgPool 不能跨 tokio runtime 共享（前一个测试的 runtime 关停后池不可用）。
static DB_SETUP: OnceLock<Mutex<()>> = OnceLock::new();

async fn pool() -> Option<PgPool> {
    dotenvy::dotenv().ok();
    let Ok(url) = std::env::var("PACS_TEST_DATABASE_URL") else {
        assert!(
            std::env::var_os("CI").is_none(),
            "CI 必须设置 PACS_TEST_DATABASE_URL"
        );
        eprintln!("\n>>> 跳过报告模板 API 测试: 未设置 PACS_TEST_DATABASE_URL\n");
        return None;
    };
    let slot = DB_SETUP.get_or_init(|| Mutex::new(()));
    let _guard = slot.lock().unwrap();
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

async fn radiologist_token(pool: &PgPool, secret: &[u8], prefix: &str) -> (String, i64) {
    let suffix = Uuid::new_v4();
    let username = format!("{prefix}-{suffix}");
    let user_id: i64 = sqlx::query_scalar(
        "INSERT INTO users (institution_id, username, password_hash, role)
         VALUES (1, $1, 'unused', 'radiologist') RETURNING id",
    )
    .bind(&username)
    .fetch_one(pool)
    .await
    .unwrap();
    let codec = AccessTokenCodec::new(secret).unwrap();
    let token = codec
        .issue(user_id, 1, &username, Role::Radiologist, Utc::now())
        .unwrap();
    (token, user_id)
}

#[tokio::test]
async fn report_templates_are_seeded_and_filterable() {
    let Some(pool) = pool().await else { return };
    let secret = b"report-tpl-test-secret-at-least-32-byte";
    let (token, _user_id) = radiologist_token(&pool, secret, "report-tpl").await;
    let auth = Arc::new(AuthService::new(pool.clone(), secret).unwrap());
    let app = pacs_web::clinical_routes(pacs_web::WebState::new(pool), auth);

    let unauthorized = app
        .clone()
        .oneshot(
            Request::get("/report-templates")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let listed = app
        .clone()
        .oneshot(
            Request::get("/report-templates")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let templates = response_json(listed).await;
    let templates = templates.as_array().expect("应返回模板数组");
    assert!(
        templates.len() >= 5,
        "迁移应内置至少 5 个种子模板，实际 {}",
        templates.len()
    );
    for template in templates {
        assert!(template["builtin"].as_bool().unwrap_or(false));
        assert_eq!(template["structure"]["schema_version"], 1);
        // I5：section.id 固定枚举
        for section in template["structure"]["sections"].as_array().unwrap() {
            let id = section["id"].as_str().expect("section.id 缺失");
            assert!(
                matches!(id, "findings" | "impression" | "recommendation"),
                "非法 section.id: {id}"
            );
        }
    }

    let ct_only = app
        .clone()
        .oneshot(
            Request::get("/report-templates?modality=CT")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ct_only.status(), StatusCode::OK);
    let ct_only = response_json(ct_only).await;
    for template in ct_only.as_array().unwrap() {
        assert_eq!(template["modality"], "CT");
    }
}

/// I2 派生缓存单向：结构化报告（payload 非空）的草稿更新必须携带 payload，
/// 缺 payload → 422；携带 payload → 200。
#[tokio::test]
async fn structured_draft_update_requires_payload() {
    let Some(pool) = pool().await else { return };
    let secret = b"report-i2-test-secret-at-least-32-byte";
    let (token, user_id) = radiologist_token(&pool, secret, "report-i2").await;
    let auth = Arc::new(AuthService::new(pool.clone(), secret).unwrap());
    let app = pacs_web::clinical_routes(pacs_web::WebState::new(pool.clone()), auth);

    let suffix = Uuid::new_v4();
    let patient_id: i64 = sqlx::query_scalar(
        "INSERT INTO patients (institution_id, patient_id) VALUES (1, $1) RETURNING id",
    )
    .bind(format!("i2-patient-{suffix}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let study_uid = format!("1.2.826.0.1.3680043.9.7433.{suffix}");
    let study_id: i64 = sqlx::query_scalar(
        "INSERT INTO studies (institution_id, patient_fk, study_instance_uid)
         VALUES (1, $1, $2) RETURNING id",
    )
    .bind(patient_id)
    .bind(&study_uid)
    .fetch_one(&pool)
    .await
    .unwrap();
    let report_id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO diagnostic_reports
           (id,institution_id,study_fk,author_fk,status,findings,impression,template_payload)
           VALUES($1,1,$2,$3,'draft','所见','印象','{"schema_version":1,"sections":[],"values":{}}'::jsonb)"#,
    )
    .bind(report_id)
    .bind(study_id)
    .bind(user_id)
    .execute(&pool)
    .await
    .unwrap();

    let without_payload = app
        .clone()
        .oneshot(
            Request::put(format!("/reports/{report_id}/draft"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "revision": 1,
                        "findings": "绕过 payload 直接改文本",
                        "impression": "不应成功",
                        "recommendation": null
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let without_status = without_payload.status();
    let without_body = response_json(without_payload).await;
    assert_eq!(
        without_status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "结构化报告缺 payload 的草稿更新必须 422，实际 {without_status} body={without_body}"
    );

    let with_payload = app
        .clone()
        .oneshot(
            Request::put(format!("/reports/{report_id}/draft"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "revision": 1,
                        "findings": "所见（由 payload 渲染）",
                        "impression": "印象",
                        "recommendation": null,
                        "template_payload": {
                            "schema_version": 1,
                            "sections": [],
                            "values": {}
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(with_payload.status(), StatusCode::OK);
    let report = response_json(with_payload).await;
    assert!(report["template_payload"].is_object());
    assert_eq!(report["findings"], "所见（由 payload 渲染）");
}
