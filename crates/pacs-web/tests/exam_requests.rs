//! 检查申请单 API 的角色权限与机构隔离。

use std::sync::{Arc, OnceLock};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use chrono::Utc;
use pacs_auth::{AccessTokenCodec, AuthService, Role};
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
        return None;
    };
    let _guard = DB_SETUP.get_or_init(|| Mutex::new(())).lock().await;
    if !Postgres::database_exists(&url).await.unwrap_or(false) {
        Postgres::create_database(&url).await.unwrap();
    }
    let pool = pacs_db::connect(&url).await.unwrap();
    pacs_db::migrate(&pool).await.unwrap();
    Some(pool)
}

async fn token(
    pool: &PgPool,
    secret: &[u8],
    institution_id: i64,
    role: Role,
    prefix: &str,
) -> String {
    let username = format!("{prefix}-{}", Uuid::new_v4());
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO users(institution_id,username,password_hash,role) VALUES($1,$2,'x',$3) RETURNING id",
    )
    .bind(institution_id)
    .bind(&username)
    .bind(role.as_str())
    .fetch_one(pool)
    .await
    .unwrap();
    AccessTokenCodec::new(secret)
        .unwrap()
        .issue(id, institution_id, &username, role, Utc::now())
        .unwrap()
}

async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

async fn seeded_study(pool: &PgPool, institution_id: i64, stem: u32) -> String {
    let nonce = Uuid::new_v4().as_u128() % 1_000_000_000_000_000_000_000_000_u128;
    let patient: i64 = sqlx::query_scalar(
        "INSERT INTO patients(institution_id,patient_id,name) VALUES($1,$2,'API 已入库患者') RETURNING id",
    )
    .bind(institution_id)
    .bind(format!("API-EXISTING-{nonce}"))
    .fetch_one(pool)
    .await
    .unwrap();
    let study_uid = format!("1.2.826.0.1.3680043.9.{stem}.{nonce}");
    let study_fk: i64 = sqlx::query_scalar(
        "INSERT INTO studies(institution_id,patient_fk,study_instance_uid,study_date,description) VALUES($1,$2,$3,CURRENT_DATE,'API 已入库胸部 CT') RETURNING id",
    )
    .bind(institution_id)
    .bind(patient)
    .bind(&study_uid)
    .fetch_one(pool)
    .await
    .unwrap();
    let series_fk: i64 = sqlx::query_scalar(
        "INSERT INTO series(study_fk,series_instance_uid,modality) VALUES($1,$2,'CT') RETURNING id",
    )
    .bind(study_fk)
    .bind(format!("1.2.826.0.1.3680043.9.{}.{}", stem + 1, nonce))
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO instances(series_fk,sop_instance_uid,transfer_syntax_uid,storage_path,file_size,file_sha256,logical_instance_id) VALUES($1,$2,'1.2.840.10008.1.2.1','api/existing.dcm',1,'\\x00',gen_random_uuid())",
    )
    .bind(series_fk)
    .bind(format!("1.2.826.0.1.3680043.9.{}.{}", stem + 2, nonce))
    .execute(pool)
    .await
    .unwrap();
    study_uid
}

#[tokio::test]
async fn technician_creates_admin_reports_and_other_roles_are_restricted() {
    let Some(pool) = pool().await else { return };
    let secret = b"exam-request-api-secret-at-least-32";
    let suffix = Uuid::new_v4();
    let foreign: i64 =
        sqlx::query_scalar("INSERT INTO institutions(code,name) VALUES($1,$2) RETURNING id")
            .bind(format!("api-exam-foreign-{suffix}"))
            .bind("API 申请单外部机构")
            .fetch_one(&pool)
            .await
            .unwrap();
    let admin = token(&pool, secret, 1, Role::Admin, "exam-admin").await;
    let technician = token(&pool, secret, 1, Role::Technician, "exam-tech").await;
    let doctor = token(&pool, secret, 1, Role::Radiologist, "exam-doctor").await;
    let viewer = token(&pool, secret, 1, Role::Viewer, "exam-viewer").await;
    let foreign_technician = token(
        &pool,
        secret,
        foreign,
        Role::Technician,
        "exam-foreign-tech",
    )
    .await;
    let app: Router = pacs_web::clinical_routes(
        pacs_web::WebState::new(pool.clone()),
        Arc::new(AuthService::new(pool.clone(), secret).unwrap()),
    );

    let technician_study = seeded_study(&pool, 1, 8814).await;
    let technician_existing = app
        .clone()
        .oneshot(
            Request::post(format!("/exam-requests/study/{technician_study}"))
                .header(header::AUTHORIZATION, bearer(&technician))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "modality":"CT","body_part":"胸部","request_type":"增强",
                        "clinical_indication":"从检查队列直接开具"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(technician_existing.status(), StatusCode::CREATED);
    let technician_existing = json_body(technician_existing).await;
    assert_eq!(technician_existing["study_uid"], technician_study);
    assert_eq!(technician_existing["patient_name"], "API 已入库患者");
    assert_eq!(technician_existing["status"], "executed");

    let admin_study = seeded_study(&pool, 1, 8817).await;
    let admin_existing = app
        .clone()
        .oneshot(
            Request::post(format!("/exam-requests/study/{admin_study}"))
                .header(header::AUTHORIZATION, bearer(&admin))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "modality":"CT","body_part":"胸部","request_type":"平扫",
                        "clinical_indication":"管理员从检查队列开具"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(admin_existing.status(), StatusCode::CREATED);

    for denied in [&doctor, &viewer] {
        let response = app
            .clone()
            .oneshot(
                Request::post(format!("/exam-requests/study/{technician_study}"))
                    .header(header::AUTHORIZATION, bearer(denied))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "modality":"CT","body_part":"胸部","request_type":"平扫",
                            "clinical_indication":"角色无权开具"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    let foreign_existing = app
        .clone()
        .oneshot(
            Request::post(format!("/exam-requests/study/{technician_study}"))
                .header(header::AUTHORIZATION, bearer(&foreign_technician))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "modality":"CT","body_part":"胸部","request_type":"平扫",
                        "clinical_indication":"不得跨机构开具"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(foreign_existing.status(), StatusCode::NOT_FOUND);

    let created = app
        .clone()
        .oneshot(
            Request::post("/exam-requests")
                .header(header::AUTHORIZATION, bearer(&technician))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "patient_id":"API-EXAM-001","patient_name":"测试患者",
                        "modality":"CT","body_part":"胸部","request_type":"平扫",
                        "clinical_indication":"咳嗽发热三天"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let created = json_body(created).await;

    let overlong = app
        .clone()
        .oneshot(
            Request::post("/exam-requests")
                .header(header::AUTHORIZATION, bearer(&technician))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "patient_id":"TOO-LONG","patient_name":"超长校验",
                        "modality":"CT","body_part":"胸部","request_type":"平扫",
                        "clinical_indication":"x".repeat(4097)
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(overlong.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let invalid_study_uid = app
        .clone()
        .oneshot(
            Request::post(format!(
                "/exam-requests/{}/bind",
                created["id"].as_str().unwrap()
            ))
            .header(header::AUTHORIZATION, bearer(&technician))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({"study_uid":"not-a-dicom-uid","revision":1}).to_string(),
            ))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invalid_study_uid.status(), StatusCode::BAD_REQUEST);

    let foreign_list = app
        .clone()
        .oneshot(
            Request::get("/exam-requests")
                .header(header::AUTHORIZATION, bearer(&foreign_technician))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(foreign_list.status(), StatusCode::OK);
    assert!(json_body(foreign_list).await.as_array().unwrap().is_empty());

    let doctor_create = app
        .clone()
        .oneshot(
            Request::post("/exam-requests")
                .header(header::AUTHORIZATION, bearer(&doctor))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "patient_id":"DENIED","patient_name":"无权限",
                        "modality":"CT","body_part":"胸部","request_type":"平扫",
                        "clinical_indication":"无权限创建"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(doctor_create.status(), StatusCode::FORBIDDEN);

    let doctor_study_read = app
        .clone()
        .oneshot(
            Request::get("/exam-requests/study/1.2.3.4")
                .header(header::AUTHORIZATION, bearer(&doctor))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(doctor_study_read.status(), StatusCode::NOT_FOUND);
    let viewer_study_read = app
        .clone()
        .oneshot(
            Request::get("/exam-requests/study/1.2.3.4")
                .header(header::AUTHORIZATION, bearer(&viewer))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(viewer_study_read.status(), StatusCode::FORBIDDEN);

    let today = Utc::now().date_naive();
    let workload = app
        .clone()
        .oneshot(
            Request::get(format!("/workload?date_from={today}&date_to={today}"))
                .header(header::AUTHORIZATION, bearer(&admin))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(workload.status(), StatusCode::OK);
    let workload = json_body(workload).await;
    assert!(
        workload
            .as_array()
            .unwrap()
            .iter()
            .any(|row| { row["exam_requests_created"] == 2 && row["role"] == "technician" })
    );
    let technician_workload = app
        .clone()
        .oneshot(
            Request::get(format!("/workload?date_from={today}&date_to={today}"))
                .header(header::AUTHORIZATION, bearer(&technician))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(technician_workload.status(), StatusCode::FORBIDDEN);
    assert!(created["id"].is_string());
}
