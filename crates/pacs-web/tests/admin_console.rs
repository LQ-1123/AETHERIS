use std::sync::{Arc, OnceLock};
use tokio::sync::Mutex;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use chrono::Utc;
use pacs_auth::{AccessTokenCodec, AuthService, Role};
use serde_json::{Value, json};
use sqlx::migrate::MigrateDatabase;
use sqlx::{PgPool, Postgres};
use tower::ServiceExt;
use uuid::Uuid;

/// 建库/迁移串行化；连接池每个测试自建（PgPool 不能跨 tokio runtime 共享）。
static DB_SETUP: OnceLock<Mutex<()>> = OnceLock::new();

async fn pool() -> Option<PgPool> {
    dotenvy::dotenv().ok();
    let Ok(url) = std::env::var("PACS_TEST_DATABASE_URL") else {
        assert!(
            std::env::var_os("CI").is_none(),
            "CI 必须设置 PACS_TEST_DATABASE_URL"
        );
        eprintln!("\n>>> 跳过管理员控制台 API 测试: 未设置 PACS_TEST_DATABASE_URL\n");
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

async fn admin_token(pool: &PgPool, secret: &[u8], prefix: &str) -> (String, i64) {
    let suffix = Uuid::new_v4();
    let username = format!("{prefix}-{suffix}");
    let user_id: i64 = sqlx::query_scalar(
        "INSERT INTO users (institution_id, username, password_hash, role)
         VALUES (1, $1, 'unused', 'admin') RETURNING id",
    )
    .bind(&username)
    .fetch_one(pool)
    .await
    .unwrap();
    let codec = AccessTokenCodec::new(secret).unwrap();
    let token = codec
        .issue(user_id, 1, &username, Role::Admin, Utc::now())
        .unwrap();
    (token, user_id)
}

/// 注册 → 批准 → 归属 → 列表过滤，全链路走真实 API。
#[tokio::test]
async fn device_registration_approval_and_series_attribution() {
    let Some(pool) = pool().await else { return };
    let secret = b"admin-console-test-secret-at-least-32";
    let (token, _admin_id) = admin_token(&pool, secret, "admin-console").await;
    let auth = Arc::new(AuthService::new(pool.clone(), secret).unwrap());
    let app = pacs_web::clinical_routes(pacs_web::WebState::new(pool.clone()), auth);

    // 建一条未归属序列（模拟历史数据）
    let suffix = Uuid::new_v4();
    let patient_id: i64 = sqlx::query_scalar(
        "INSERT INTO patients (institution_id, patient_id) VALUES (1, $1) RETURNING id",
    )
    .bind(format!("console-patient-{suffix}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let study_uid = format!("1.2.826.0.1.3680043.9.7434.{suffix}");
    let study_id: i64 = sqlx::query_scalar(
        "INSERT INTO studies (institution_id, patient_fk, study_instance_uid)
         VALUES (1, $1, $2) RETURNING id",
    )
    .bind(patient_id)
    .bind(&study_uid)
    .fetch_one(&pool)
    .await
    .unwrap();
    let numeric = suffix
        .to_string()
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect::<String>();
    let series_uid = format!("1.2.826.0.1.3680043.9.7435.{numeric}");
    sqlx::query(
        "INSERT INTO series (study_fk, series_instance_uid, modality)
         VALUES ($1, $2, 'CT')",
    )
    .bind(study_id)
    .bind(&series_uid)
    .execute(&pool)
    .await
    .unwrap();

    let bearer = format!("Bearer {token}");
    let ae_suffix = suffix.to_string().replace('-', "");
    let ae_title = format!("CT{}", &ae_suffix[..8]);

    // 注册设备 → 201 pending
    let registered = app
        .clone()
        .oneshot(
            Request::post("/devices")
                .header(header::AUTHORIZATION, &bearer)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "name": "测试 CT 机",
                        "calling_ae_title": ae_title,
                        "source_ip": "192.168.1.50",
                        "modality_hint": "CT"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let registered_status = registered.status();
    let registered_body = response_json(registered).await;
    assert_eq!(
        registered_status,
        StatusCode::CREATED,
        "注册设备应 201，实际 {registered_status} body={registered_body}"
    );
    let device = registered_body;
    assert_eq!(device["status"], "pending");
    let device_id = device["id"].as_str().unwrap().to_owned();

    // 重复注册 → 422
    let duplicate = app
        .clone()
        .oneshot(
            Request::post("/devices")
                .header(header::AUTHORIZATION, &bearer)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "name": "重复设备",
                        "calling_ae_title": ae_title,
                        "source_ip": "192.168.1.50"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(duplicate.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // 批准 → active
    let approved = app
        .clone()
        .oneshot(
            Request::post(format!("/devices/{device_id}/approve"))
                .header(header::AUTHORIZATION, &bearer)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"name": "测试 CT 机"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(approved.status(), StatusCode::OK);
    assert_eq!(response_json(approved).await["status"], "active");

    // 未归属列表包含该序列
    let unattributed = app
        .clone()
        .oneshot(
            Request::get("/series-sources?unattributed=true")
                .header(header::AUTHORIZATION, &bearer)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unattributed.status(), StatusCode::OK);
    let list = response_json(unattributed).await;
    let entry = list
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["series_uid"] == series_uid)
        .expect("未归属列表应包含新序列");
    assert_eq!(entry["source_status"], "legacy_unattributed");

    // 归属 → trusted；未归属列表不再包含
    let resolved = app
        .clone()
        .oneshot(
            Request::post(format!("/series/{series_uid}/resolve-source"))
                .header(header::AUTHORIZATION, &bearer)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"device_id": device_id}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resolved.status(), StatusCode::NO_CONTENT);
    let after = app
        .clone()
        .oneshot(
            Request::get("/series-sources?unattributed=true")
                .header(header::AUTHORIZATION, &bearer)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let after = response_json(after).await;
    assert!(
        !after
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["series_uid"] == series_uid),
        "归属后的序列不应再出现在未归属列表"
    );
}

/// 0023 回填语句幂等：对本用例独占的序列重复执行不产生重复工作项。
#[tokio::test]
async fn work_item_backfill_statement_is_idempotent() {
    let Some(pool) = pool().await else { return };
    let nonce = Uuid::new_v4().as_u128();
    let patient_fk: i64 = sqlx::query_scalar(
        "INSERT INTO patients(institution_id,patient_id) VALUES(1,$1) RETURNING id",
    )
    .bind(format!("backfill-{nonce}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let study_fk: i64 = sqlx::query_scalar(
        "INSERT INTO studies(institution_id,patient_fk,study_instance_uid) VALUES(1,$1,$2) RETURNING id",
    )
    .bind(patient_fk)
    .bind(format!("1.2.826.0.1.3680043.9.9900.{nonce}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let series_fk: i64 = sqlx::query_scalar(
        "INSERT INTO series(study_fk,series_instance_uid) VALUES($1,$2) RETURNING id",
    )
    .bind(study_fk)
    .bind(format!("1.2.826.0.1.3680043.9.9901.{nonce}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let statement = r#"INSERT INTO diagnostic_work_items (id, institution_id, series_fk)
                       SELECT gen_random_uuid(), st.institution_id, se.id
                       FROM series se JOIN studies st ON st.id = se.study_fk
                       WHERE se.id=$1
                       ON CONFLICT (institution_id, series_fk) DO NOTHING"#;
    let first = sqlx::query(statement)
        .bind(series_fk)
        .execute(&pool)
        .await
        .unwrap();
    let second = sqlx::query(statement)
        .bind(series_fk)
        .execute(&pool)
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM diagnostic_work_items WHERE institution_id=1 AND series_fk=$1",
    )
    .bind(series_fk)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(first.rows_affected(), 1, "首次回填应创建工作项");
    assert_eq!(second.rows_affected(), 0, "重复回填不得创建工作项");
    assert_eq!(count, 1, "同一序列只能存在一条工作项");
}

/// 按序列查工作项：不受「仅当天」日期过滤限制，且遵守设备授权可见性。
#[tokio::test]
async fn work_item_for_series_ignores_date_but_respects_grants() {
    let Some(pool) = pool().await else { return };
    let secret = b"admin-wi-test-secret-at-least-32-byt";
    let (admin_token, _admin_id) = admin_token(&pool, secret, "admin-wi").await;
    let auth = Arc::new(AuthService::new(pool.clone(), secret).unwrap());
    let app = pacs_web::clinical_routes(pacs_web::WebState::new(pool.clone()), auth);
    let admin_bearer = format!("Bearer {admin_token}");

    // 医生 + 设备 + 授权 + 历史序列（模拟几天前入库）
    let suffix = Uuid::new_v4();
    let ae = format!("WI{}", &suffix.to_string().replace('-', "")[..8]);
    let doctor_id: i64 = sqlx::query_scalar(
        "INSERT INTO users (institution_id, username, password_hash, role)
         VALUES (1, $1, 'unused', 'radiologist') RETURNING id",
    )
    .bind(format!("wi-doctor-{suffix}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let codec = AccessTokenCodec::new(secret).unwrap();
    let doctor_token = codec
        .issue(doctor_id, 1, "wi-doctor", Role::Radiologist, Utc::now())
        .unwrap();
    let doctor_bearer = format!("Bearer {doctor_token}");

    let device = app
        .clone()
        .oneshot(
            Request::post("/devices")
                .header(header::AUTHORIZATION, &admin_bearer)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "name": "历史设备",
                        "calling_ae_title": ae,
                        "source_ip": "10.1.1.9"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let device = response_json(device).await;
    let device_id = device["id"].as_str().unwrap().to_owned();
    let approved = app
        .clone()
        .oneshot(
            Request::post(format!("/devices/{device_id}/approve"))
                .header(header::AUTHORIZATION, &admin_bearer)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"name": "历史设备"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(approved.status(), StatusCode::OK);

    let patient_id: i64 = sqlx::query_scalar(
        "INSERT INTO patients (institution_id, patient_id) VALUES (1, $1) RETURNING id",
    )
    .bind(format!("wi-patient-{suffix}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let study_uid = format!("1.2.826.0.1.3680043.9.7436.{suffix}");
    let study_id: i64 = sqlx::query_scalar(
        "INSERT INTO studies (institution_id, patient_fk, study_instance_uid)
         VALUES (1, $1, $2) RETURNING id",
    )
    .bind(patient_id)
    .bind(&study_uid)
    .fetch_one(&pool)
    .await
    .unwrap();
    let numeric = suffix
        .to_string()
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect::<String>();
    let series_uid = format!("1.2.826.0.1.3680043.9.7437.{numeric}");
    let series_id: i64 = sqlx::query_scalar(
        "INSERT INTO series (study_fk, series_instance_uid, modality)
         VALUES ($1, $2, 'CT') RETURNING id",
    )
    .bind(study_id)
    .bind(&series_uid)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        r#"INSERT INTO instances (series_fk, sop_instance_uid, transfer_syntax_uid,
           storage_path, file_size, file_sha256, logical_instance_id)
           VALUES ($1, $2, '1.2.840.10008.1.2.1', '/test/legacy.dcm', 1, '\x00',
                   gen_random_uuid())"#,
    )
    .bind(series_id)
    .bind(format!("1.2.826.0.1.3680043.9.7438.{numeric}"))
    .execute(&pool)
    .await
    .unwrap();
    // 回填工作项（与 0023 同语句）
    sqlx::query(
        r#"INSERT INTO diagnostic_work_items (id, institution_id, series_fk)
           SELECT gen_random_uuid(), st.institution_id, se.id
           FROM series se JOIN studies st ON st.id = se.study_fk
           ON CONFLICT (institution_id, series_fk) DO NOTHING"#,
    )
    .execute(&pool)
    .await
    .unwrap();
    // 归属到设备
    let resolved = app
        .clone()
        .oneshot(
            Request::post(format!("/series/{series_uid}/resolve-source"))
                .header(header::AUTHORIZATION, &admin_bearer)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"device_id": device_id}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resolved.status(), StatusCode::NO_CONTENT);

    // 未授权的医生 → 404（不可见）
    let denied = app
        .clone()
        .oneshot(
            Request::get(format!("/worklist/series/{series_uid}"))
                .header(header::AUTHORIZATION, &doctor_bearer)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::NOT_FOUND);

    // 授权后 → 200 且能拿到工作项（不因入库日期过滤）
    let granted = app
        .clone()
        .oneshot(
            Request::put(format!("/users/{doctor_id}/device-grants"))
                .header(header::AUTHORIZATION, &admin_bearer)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"device_ids": [device_id]}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(granted.status(), StatusCode::OK);
    let found = app
        .clone()
        .oneshot(
            Request::get(format!("/worklist/series/{series_uid}"))
                .header(header::AUTHORIZATION, &doctor_bearer)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(found.status(), StatusCode::OK);
    let item = response_json(found).await;
    assert_eq!(item["series_uid"], series_uid);
    assert_eq!(item["status"], "pending");
}
#[tokio::test]
async fn user_device_grants_roundtrip() {
    let Some(pool) = pool().await else { return };
    let secret = b"admin-grants-test-secret-at-least-32";
    let (token, _admin_id) = admin_token(&pool, secret, "admin-grants").await;
    let auth = Arc::new(AuthService::new(pool.clone(), secret).unwrap());
    let app = pacs_web::clinical_routes(pacs_web::WebState::new(pool.clone()), auth);
    let bearer = format!("Bearer {token}");

    // 注册并批准两个设备
    let suffix = Uuid::new_v4().to_string().replace('-', "");
    let mut device_ids = Vec::new();
    for index in 0..2 {
        let ae_title = format!("G{index}{}", &suffix[..8]);
        let registered = app
            .clone()
            .oneshot(
                Request::post("/devices")
                    .header(header::AUTHORIZATION, &bearer)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "name": format!("授权测试机 {index}"),
                            "calling_ae_title": ae_title,
                            "source_ip": format!("10.0.0.{index}")
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let device = response_json(registered).await;
        let device_id = device["id"].as_str().unwrap().to_owned();
        let approved = app
            .clone()
            .oneshot(
                Request::post(format!("/devices/{device_id}/approve"))
                    .header(header::AUTHORIZATION, &bearer)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({"name": format!("授权测试机 {index}")}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(approved.status(), StatusCode::OK);
        device_ids.push(device_id);
    }

    // 目标用户（radiologist）
    let suffix = Uuid::new_v4();
    let doctor_id: i64 = sqlx::query_scalar(
        "INSERT INTO users (institution_id, username, password_hash, role)
         VALUES (1, $1, 'unused', 'radiologist') RETURNING id",
    )
    .bind(format!("grant-doctor-{suffix}"))
    .fetch_one(&pool)
    .await
    .unwrap();

    // PUT 授权第一个设备
    let put = app
        .clone()
        .oneshot(
            Request::put(format!("/users/{doctor_id}/device-grants"))
                .header(header::AUTHORIZATION, &bearer)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"device_ids": [device_ids[0].clone()]}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(put.status(), StatusCode::OK);

    // GET 往返一致
    let get = app
        .clone()
        .oneshot(
            Request::get(format!("/users/{doctor_id}/device-grants"))
                .header(header::AUTHORIZATION, &bearer)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let grants = response_json(get).await;
    let ids = grants
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item.as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec![device_ids[0].clone()]);

    // PUT 全量替换为第二个设备
    let replace = app
        .clone()
        .oneshot(
            Request::put(format!("/users/{doctor_id}/device-grants"))
                .header(header::AUTHORIZATION, &bearer)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"device_ids": [device_ids[1].clone()]}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(replace.status(), StatusCode::OK);
    let get = app
        .clone()
        .oneshot(
            Request::get(format!("/users/{doctor_id}/device-grants"))
                .header(header::AUTHORIZATION, &bearer)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let grants = response_json(get).await;
    let ids = grants
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item.as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec![device_ids[1].clone()], "PUT 应为全量替换语义");
}

#[tokio::test]
async fn institution_review_setting_is_persistent_and_admin_only() {
    let Some(pool) = pool().await else { return };
    let secret = b"institution-settings-test-secret-32";
    let suffix = Uuid::new_v4();
    let institution_id: i64 =
        sqlx::query_scalar("INSERT INTO institutions(code,name) VALUES($1,$2) RETURNING id")
            .bind(format!("settings-{suffix}"))
            .bind(format!("设置测试机构 {suffix}"))
            .fetch_one(&pool)
            .await
            .unwrap();
    let admin_username = format!("settings-admin-{suffix}");
    let admin_id: i64 = sqlx::query_scalar(
        "INSERT INTO users (institution_id, username, password_hash, role)
         VALUES ($1, $2, 'unused', 'admin') RETURNING id",
    )
    .bind(institution_id)
    .bind(&admin_username)
    .fetch_one(&pool)
    .await
    .unwrap();
    let doctor_username = format!("settings-doctor-{suffix}");
    let doctor_id: i64 = sqlx::query_scalar(
        "INSERT INTO users (institution_id, username, password_hash, role)
         VALUES ($1, $2, 'unused', 'radiologist') RETURNING id",
    )
    .bind(institution_id)
    .bind(&doctor_username)
    .fetch_one(&pool)
    .await
    .unwrap();
    let codec = AccessTokenCodec::new(secret).unwrap();
    let admin_token = codec
        .issue(
            admin_id,
            institution_id,
            &admin_username,
            Role::Admin,
            Utc::now(),
        )
        .unwrap();
    let doctor_token = codec
        .issue(
            doctor_id,
            institution_id,
            &doctor_username,
            Role::Radiologist,
            Utc::now(),
        )
        .unwrap();
    let auth = Arc::new(AuthService::new(pool.clone(), secret).unwrap());
    let app = pacs_web::clinical_routes(pacs_web::WebState::new(pool.clone()), auth);
    let admin_bearer = format!("Bearer {admin_token}");
    let doctor_bearer = format!("Bearer {doctor_token}");

    let initial = app
        .clone()
        .oneshot(
            Request::get("/institution/settings")
                .header(header::AUTHORIZATION, &admin_bearer)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(initial.status(), StatusCode::OK);
    assert_eq!(response_json(initial).await["review_required"], false);

    let updated = app
        .clone()
        .oneshot(
            Request::patch("/institution/settings")
                .header(header::AUTHORIZATION, &admin_bearer)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"review_required": true}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);
    assert_eq!(response_json(updated).await["review_required"], true);

    let persisted: bool =
        sqlx::query_scalar("SELECT review_required FROM institutions WHERE id=$1")
            .bind(institution_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(persisted, "设置应写入机构表并跨会话保留");

    for request in [
        Request::get("/institution/settings")
            .header(header::AUTHORIZATION, &doctor_bearer)
            .body(Body::empty())
            .unwrap(),
        Request::patch("/institution/settings")
            .header(header::AUTHORIZATION, &doctor_bearer)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({"review_required": false}).to_string()))
            .unwrap(),
    ] {
        let denied = app.clone().oneshot(request).await.unwrap();
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    }

    let unchanged: bool =
        sqlx::query_scalar("SELECT review_required FROM institutions WHERE id=$1")
            .bind(institution_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(unchanged, "非管理员请求不得修改设置");
}
