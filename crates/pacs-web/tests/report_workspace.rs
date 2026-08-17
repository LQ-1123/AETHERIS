//! B2-2 报告工作台集成测试：`is_positive` 阳性标记与 `clear_template_payload`
//! 富文本迁移开关的端到端行为。
//!
//! 覆盖：
//! - `is_positive` 在 create / draft 上的往返
//! - sign 将 `is_positive` 写入不可变版本快照
//! - I2 规则：结构化报告草稿更新必须携带 template_payload，唯一例外是 clear
//! - clear 只影响 payload，不影响本次提交的 findings/impression/is_positive

use std::sync::{Arc, Mutex, OnceLock};

use axum::Router;
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
        eprintln!("\n>>> 跳过报告工作台 API 测试: 未设置 PACS_TEST_DATABASE_URL\n");
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

/// 只建 patient + study，返回 study_fk（供直接 SQL 插入 draft 报告用）。
async fn insert_study(pool: &PgPool, prefix: &str) -> i64 {
    let suffix = Uuid::new_v4();
    let patient_id: i64 = sqlx::query_scalar(
        "INSERT INTO patients (institution_id, patient_id) VALUES (1, $1) RETURNING id",
    )
    .bind(format!("{prefix}-patient-{suffix}"))
    .fetch_one(pool)
    .await
    .unwrap();
    let study_uid = format!("1.2.826.0.1.3680043.9.7450.{suffix}");
    sqlx::query_scalar(
        "INSERT INTO studies (institution_id, patient_fk, study_instance_uid)
         VALUES (1, $1, $2) RETURNING id",
    )
    .bind(patient_id)
    .bind(&study_uid)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// 完整链路：注册设备 → 批准 → 授权医生 → SQL 建 patient/study/series/instance →
/// 回填工作项 → resolve-source → 医生领取工作项。
/// 返回 (study_uid, series_uid)，供 create_report 使用。
#[allow(clippy::too_many_arguments)]
async fn setup_claimed_series(
    app: &Router,
    pool: &PgPool,
    admin_bearer: &str,
    doctor_bearer: &str,
    doctor_id: i64,
) -> (String, String) {
    let suffix = Uuid::new_v4();
    let ae_suffix = suffix.to_string().replace('-', "");
    let ae_title = format!("RW{}", &ae_suffix[..8]);

    // 注册设备 → 201
    let registered = app
        .clone()
        .oneshot(
            Request::post("/devices")
                .header(header::AUTHORIZATION, admin_bearer)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "name": "报告工作台测试机",
                        "calling_ae_title": ae_title,
                        "source_ip": "10.30.0.7"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(registered.status(), StatusCode::CREATED, "注册设备应 201");
    let device = response_json(registered).await;
    let device_id = device["id"].as_str().unwrap().to_owned();

    // 批准 → active
    let approved = app
        .clone()
        .oneshot(
            Request::post(format!("/devices/{device_id}/approve"))
                .header(header::AUTHORIZATION, admin_bearer)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"name": "报告工作台测试机"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(approved.status(), StatusCode::OK, "批准设备应 200");

    // 授权医生
    let granted = app
        .clone()
        .oneshot(
            Request::put(format!("/users/{doctor_id}/device-grants"))
                .header(header::AUTHORIZATION, admin_bearer)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"device_ids": [device_id]}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(granted.status(), StatusCode::OK, "授权医生应 200");

    // SQL 建 patient/study/series/instance
    let patient_id: i64 = sqlx::query_scalar(
        "INSERT INTO patients (institution_id, patient_id) VALUES (1, $1) RETURNING id",
    )
    .bind(format!("rw-patient-{suffix}"))
    .fetch_one(pool)
    .await
    .unwrap();
    let study_uid = format!("1.2.826.0.1.3680043.9.7441.{suffix}");
    let study_id: i64 = sqlx::query_scalar(
        "INSERT INTO studies (institution_id, patient_fk, study_instance_uid)
         VALUES (1, $1, $2) RETURNING id",
    )
    .bind(patient_id)
    .bind(&study_uid)
    .fetch_one(pool)
    .await
    .unwrap();
    let numeric = suffix
        .to_string()
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect::<String>();
    let series_uid = format!("1.2.826.0.1.3680043.9.7442.{numeric}");
    let series_id: i64 = sqlx::query_scalar(
        "INSERT INTO series (study_fk, series_instance_uid, modality)
         VALUES ($1, $2, 'CT') RETURNING id",
    )
    .bind(study_id)
    .bind(&series_uid)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        r#"INSERT INTO instances (series_fk, sop_instance_uid, transfer_syntax_uid,
           storage_path, file_size, file_sha256, logical_instance_id)
           VALUES ($1, $2, '1.2.840.10008.1.2.1', '/test/legacy.dcm', 1, '\x00',
                   gen_random_uuid())"#,
    )
    .bind(series_id)
    .bind(format!("1.2.826.0.1.3680043.9.7443.{numeric}"))
    .execute(pool)
    .await
    .unwrap();

    // 回填工作项（与 0023 同语句，幂等）
    sqlx::query(
        r#"INSERT INTO diagnostic_work_items (id, institution_id, series_fk)
           SELECT gen_random_uuid(), st.institution_id, se.id
           FROM series se JOIN studies st ON st.id = se.study_fk
           ON CONFLICT (institution_id, series_fk) DO NOTHING"#,
    )
    .execute(pool)
    .await
    .unwrap();

    // 归属到设备 → trusted
    let resolved = app
        .clone()
        .oneshot(
            Request::post(format!("/series/{series_uid}/resolve-source"))
                .header(header::AUTHORIZATION, admin_bearer)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"device_id": device_id}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resolved.status(),
        StatusCode::NO_CONTENT,
        "resolve-source 应 204"
    );

    // 医生领取工作项
    let item = app
        .clone()
        .oneshot(
            Request::get(format!("/worklist/series/{series_uid}"))
                .header(header::AUTHORIZATION, doctor_bearer)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(item.status(), StatusCode::OK, "按序列查工作项应 200");
    let item = response_json(item).await;
    let work_id = item["id"].as_str().unwrap().to_owned();
    let revision = item["revision"].as_i64().unwrap();

    let claimed = app
        .clone()
        .oneshot(
            Request::post(format!("/worklist/{work_id}/claim"))
                .header(header::AUTHORIZATION, doctor_bearer)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"revision": revision}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(claimed.status(), StatusCode::NO_CONTENT, "领取工作项应 204");

    (study_uid, series_uid)
}

/// is_positive 经 create（true）→ draft（false）往返。
#[tokio::test]
async fn is_positive_roundtrips_through_create_and_draft() {
    let Some(pool) = pool().await else { return };
    let secret = b"report-pos-secret-at-least-32-byte";
    let (admin_token, _admin_id) = admin_token(&pool, secret, "report-pos-admin").await;
    let (doctor_token, doctor_id) = radiologist_token(&pool, secret, "report-pos").await;
    let auth = Arc::new(AuthService::new(pool.clone(), secret).unwrap());
    let app = pacs_web::clinical_routes(pacs_web::WebState::new(pool.clone()), auth);
    let admin_bearer = format!("Bearer {admin_token}");
    let doctor_bearer = format!("Bearer {doctor_token}");

    let (study_uid, series_uid) =
        setup_claimed_series(&app, &pool, &admin_bearer, &doctor_bearer, doctor_id).await;

    // 创建报告：is_positive:true，template_payload 省略
    let created = app
        .clone()
        .oneshot(
            Request::post("/reports")
                .header(header::AUTHORIZATION, &doctor_bearer)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "study_uid": study_uid,
                        "covered_series_uids": [series_uid],
                        "is_positive": true
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED, "创建报告应 201");
    let report = response_json(created).await;
    assert_eq!(
        report["is_positive"], true,
        "create 返回 is_positive 应为 true"
    );
    let report_id = report["id"].as_str().unwrap().to_owned();
    let revision = report["revision"].as_i64().unwrap();

    // 草稿更新 is_positive:false
    let updated = app
        .clone()
        .oneshot(
            Request::put(format!("/reports/{report_id}/draft"))
                .header(header::AUTHORIZATION, &doctor_bearer)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "revision": revision,
                        "findings": "影像所见：正常",
                        "impression": "诊断意见：无异常",
                        "is_positive": false
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK, "草稿更新应 200");
    let report = response_json(updated).await;
    assert_eq!(
        report["is_positive"], false,
        "draft 返回 is_positive 应为 false"
    );
}

/// sign 把报告的 is_positive 写入不可变版本快照。
#[tokio::test]
async fn sign_report_snapshots_is_positive() {
    let Some(pool) = pool().await else { return };
    let secret = b"report-sign-secret-at-least-32-byte";
    let (admin_token, _admin_id) = admin_token(&pool, secret, "report-sign-admin").await;
    let (doctor_token, doctor_id) = radiologist_token(&pool, secret, "report-sign").await;
    let auth = Arc::new(AuthService::new(pool.clone(), secret).unwrap());
    let app = pacs_web::clinical_routes(pacs_web::WebState::new(pool.clone()), auth);
    let admin_bearer = format!("Bearer {admin_token}");
    let doctor_bearer = format!("Bearer {doctor_token}");

    let (study_uid, series_uid) =
        setup_claimed_series(&app, &pool, &admin_bearer, &doctor_bearer, doctor_id).await;

    // 建报告（is_positive 默认 false）
    let created = app
        .clone()
        .oneshot(
            Request::post("/reports")
                .header(header::AUTHORIZATION, &doctor_bearer)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "study_uid": study_uid,
                        "covered_series_uids": [series_uid]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let report = response_json(created).await;
    let report_id = report["id"].as_str().unwrap().to_owned();
    let revision = report["revision"].as_i64().unwrap();

    // 设 is_positive:true 并填写 findings/impression（签发前提）
    let updated = app
        .clone()
        .oneshot(
            Request::put(format!("/reports/{report_id}/draft"))
                .header(header::AUTHORIZATION, &doctor_bearer)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "revision": revision,
                        "findings": "影像所见：右肺上叶结节",
                        "impression": "诊断意见：建议随访",
                        "is_positive": true
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);
    let report = response_json(updated).await;
    assert_eq!(report["is_positive"], true);
    let revision = report["revision"].as_i64().unwrap();

    // 签发
    let signed = app
        .clone()
        .oneshot(
            Request::post(format!("/reports/{report_id}/sign"))
                .header(header::AUTHORIZATION, &doctor_bearer)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"revision": revision}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(signed.status(), StatusCode::NO_CONTENT, "签发应 204");

    // 版本快照应含 is_positive == true
    let versions = app
        .clone()
        .oneshot(
            Request::get(format!("/reports/{report_id}/versions"))
                .header(header::AUTHORIZATION, &doctor_bearer)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(versions.status(), StatusCode::OK);
    let versions = response_json(versions).await;
    let v1 = versions
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["version_number"] == 1)
        .expect("应存在 v1 版本快照");
    assert_eq!(v1["is_positive"], true, "v1 快照的 is_positive 应为 true");
}

/// I2：结构化报告（template_payload 非空）草稿更新缺 payload → 422；
/// clear_template_payload=true 例外 → 200 且 payload 置 NULL。
#[tokio::test]
async fn structured_draft_requires_payload_unless_cleared() {
    let Some(pool) = pool().await else { return };
    let secret = b"report-clear-secret-at-least-32-byte";
    let (token, user_id) = radiologist_token(&pool, secret, "report-clear").await;
    let auth = Arc::new(AuthService::new(pool.clone(), secret).unwrap());
    let app = pacs_web::clinical_routes(pacs_web::WebState::new(pool.clone()), auth);
    let bearer = format!("Bearer {token}");

    let study_id = insert_study(&pool, "clear").await;
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

    // 不带 payload、不带 clear → 422（I2 仍生效）
    let denied = app
        .clone()
        .oneshot(
            Request::put(format!("/reports/{report_id}/draft"))
                .header(header::AUTHORIZATION, &bearer)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "revision": 1,
                        "findings": "绕过 payload 改文本",
                        "impression": "不应成功"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let denied_status = denied.status();
    let denied_body = response_json(denied).await;
    assert_eq!(
        denied_status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "结构化报告缺 payload 的草稿更新应 422，实际 {denied_status} body={denied_body}"
    );

    // 带 clear_template_payload:true 且 template_payload 省略 → 200 且 payload == null
    let cleared = app
        .clone()
        .oneshot(
            Request::put(format!("/reports/{report_id}/draft"))
                .header(header::AUTHORIZATION, &bearer)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "revision": 1,
                        "findings": "转富文本后的所见",
                        "impression": "转富文本后的印象",
                        "clear_template_payload": true
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let cleared_status = cleared.status();
    let report = response_json(cleared).await;
    assert_eq!(
        cleared_status,
        StatusCode::OK,
        "clear 例外应 200，body={report}"
    );
    assert!(
        report["template_payload"].is_null(),
        "clear 后 template_payload 应为 null，实际 {}",
        report["template_payload"]
    );
}

/// clear 只清 payload：findings/impression 仍为本次提交文本，is_positive 正常往返。
#[tokio::test]
async fn clear_flag_only_affects_payload_not_text() {
    let Some(pool) = pool().await else { return };
    let secret = b"report-clear2-secret-at-least-32-byte";
    let (token, user_id) = radiologist_token(&pool, secret, "report-clear2").await;
    let auth = Arc::new(AuthService::new(pool.clone(), secret).unwrap());
    let app = pacs_web::clinical_routes(pacs_web::WebState::new(pool.clone()), auth);
    let bearer = format!("Bearer {token}");

    let study_id = insert_study(&pool, "clear2").await;
    let report_id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO diagnostic_reports
           (id,institution_id,study_fk,author_fk,status,findings,impression,template_payload)
           VALUES($1,1,$2,$3,'draft','旧所见','旧印象','{"schema_version":1,"sections":[],"values":{}}'::jsonb)"#,
    )
    .bind(report_id)
    .bind(study_id)
    .bind(user_id)
    .execute(&pool)
    .await
    .unwrap();

    let updated = app
        .clone()
        .oneshot(
            Request::put(format!("/reports/{report_id}/draft"))
                .header(header::AUTHORIZATION, &bearer)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "revision": 1,
                        "findings": "新所见",
                        "impression": "新印象",
                        "is_positive": true,
                        "clear_template_payload": true
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);
    let report = response_json(updated).await;
    assert!(
        report["template_payload"].is_null(),
        "clear 后 template_payload 应为 null"
    );
    assert_eq!(report["findings"], "新所见", "findings 应为本次提交文本");
    assert_eq!(
        report["impression"], "新印象",
        "impression 应为本次提交文本"
    );
    assert_eq!(report["is_positive"], true, "is_positive 应正常往返为 true");
}

/// 报告按检查一份：study 级领取/释放覆盖该检查下全部序列。
#[tokio::test]
async fn study_level_claim_and_release_cover_all_series() {
    let Some(pool) = pool().await else { return };
    let secret = b"report-study-test-secret-at-least-32";
    let (admin_token, _admin_id) = admin_token(&pool, secret, "report-study").await;
    let (doctor_token, doctor_id) = radiologist_token(&pool, secret, "report-study-doc").await;
    let auth = Arc::new(AuthService::new(pool.clone(), secret).unwrap());
    let app = pacs_web::clinical_routes(pacs_web::WebState::new(pool.clone()), auth);
    let admin_bearer = format!("Bearer {admin_token}");
    let doctor_bearer = format!("Bearer {doctor_token}");

    // 一台设备 + 授权医生
    let suffix = Uuid::new_v4();
    let ae_suffix = suffix.to_string().replace('-', "");
    let ae_title = format!("ST{}", &ae_suffix[..8]);
    let device = app.clone().oneshot(Request::post("/devices")
        .header(header::AUTHORIZATION, &admin_bearer)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({"name":"study测试机","calling_ae_title":ae_title,"source_ip":"10.40.0.9"}).to_string())).unwrap()).await.unwrap();
    let device_id = response_json(device).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let approved = app
        .clone()
        .oneshot(
            Request::post(format!("/devices/{device_id}/approve"))
                .header(header::AUTHORIZATION, &admin_bearer)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"name":"study测试机"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(approved.status(), StatusCode::OK);
    let granted = app
        .clone()
        .oneshot(
            Request::put(format!("/users/{doctor_id}/device-grants"))
                .header(header::AUTHORIZATION, &admin_bearer)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"device_ids":[device_id]}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(granted.status(), StatusCode::OK);

    // 一个 study 下建 2 个 series
    let patient_id: i64 = sqlx::query_scalar(
        "INSERT INTO patients (institution_id, patient_id) VALUES (1, $1) RETURNING id",
    )
    .bind(format!("study-patient-{suffix}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let numeric = suffix
        .to_string()
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect::<String>();
    let study_uid = format!("1.2.826.0.1.3680043.9.7451.{numeric}");
    let study_id: i64 = sqlx::query_scalar(
        "INSERT INTO studies (institution_id, patient_fk, study_instance_uid) VALUES (1, $1, $2) RETURNING id",
    ).bind(patient_id).bind(&study_uid).fetch_one(&pool).await.unwrap();
    let mut series_uids = Vec::new();
    for index in 0..2 {
        let series_uid = format!("1.2.826.0.1.3680043.9.7452.{numeric}.{index}");
        let series_id: i64 = sqlx::query_scalar(
            "INSERT INTO series (study_fk, series_instance_uid, modality) VALUES ($1, $2, 'CT') RETURNING id",
        ).bind(study_id).bind(&series_uid).fetch_one(&pool).await.unwrap();
        sqlx::query(r#"INSERT INTO instances (series_fk, sop_instance_uid, transfer_syntax_uid,
            storage_path, file_size, file_sha256, logical_instance_id)
            VALUES ($1, $2, '1.2.840.10008.1.2.1', '/test/legacy.dcm', 1, '\x00', gen_random_uuid())"#)
            .bind(series_id).bind(format!("1.2.826.0.1.3680043.9.7453.{numeric}.{index}"))
            .execute(&pool).await.unwrap();
        series_uids.push(series_uid);
    }
    // 回填工作项
    sqlx::query(
        r#"INSERT INTO diagnostic_work_items (id, institution_id, series_fk)
        SELECT gen_random_uuid(), st.institution_id, se.id
        FROM series se JOIN studies st ON st.id = se.study_fk
        ON CONFLICT (institution_id, series_fk) DO NOTHING"#,
    )
    .execute(&pool)
    .await
    .unwrap();
    // 归属两个序列
    for series_uid in &series_uids {
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
    }

    // 1) study 工作项列表：2 个，均 pending
    let list = app
        .clone()
        .oneshot(
            Request::get(format!("/worklist/study/{study_uid}"))
                .header(header::AUTHORIZATION, &doctor_bearer)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let items = response_json(list).await;
    let items = items.as_array().unwrap();
    assert_eq!(items.len(), 2, "study 下应有 2 个工作项");
    assert!(items.iter().all(|i| i["status"] == "pending"));

    // 2) study 领取：返回 2，两个都 claimed by doctor
    let claim = app
        .clone()
        .oneshot(
            Request::post(format!("/worklist/study/{study_uid}/claim"))
                .header(header::AUTHORIZATION, &doctor_bearer)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(claim.status(), StatusCode::OK);
    assert_eq!(response_json(claim).await, 2, "应领取 2 个工作项");

    let list = app
        .clone()
        .oneshot(
            Request::get(format!("/worklist/study/{study_uid}"))
                .header(header::AUTHORIZATION, &doctor_bearer)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let items = response_json(list).await;
    let items = items.as_array().unwrap();
    assert!(
        items
            .iter()
            .all(|i| i["status"] == "claimed" && i["assignee_id"] == doctor_id)
    );

    // 3) study 释放：204，回到 pending
    let release = app
        .clone()
        .oneshot(
            Request::post(format!("/worklist/study/{study_uid}/release"))
                .header(header::AUTHORIZATION, &doctor_bearer)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(release.status(), StatusCode::NO_CONTENT);
    let list = app
        .clone()
        .oneshot(
            Request::get(format!("/worklist/study/{study_uid}"))
                .header(header::AUTHORIZATION, &doctor_bearer)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let items = response_json(list).await;
    assert!(
        items
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["status"] == "pending")
    );
}
