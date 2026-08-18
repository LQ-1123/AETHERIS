//! Device grants, single-doctor claiming and immutable report versions.

use dicom::core::{DataElement, VR};
use dicom::dictionary_std::tags;
use pacs_core::extract_metadata;
use pacs_core::fixture::{ct_instance, unique_uid};
use pacs_db::{ApproveDevice, StorageRecord};
use sqlx::migrate::MigrateDatabase;
use sqlx::{PgPool, Postgres};
use uuid::Uuid;

async fn pool() -> Option<PgPool> {
    dotenvy::dotenv().ok();
    let Ok(url) = std::env::var("PACS_TEST_DATABASE_URL") else {
        return None;
    };
    if !Postgres::database_exists(&url).await.unwrap_or(false) {
        let _ = Postgres::create_database(&url).await;
    }
    let pool = pacs_db::connect(&url).await.unwrap();
    pacs_db::migrate(&pool).await.unwrap();
    Some(pool)
}

#[tokio::test]
async fn device_scope_claim_and_signed_report_are_enforced() {
    let Some(pool) = pool().await else { return };
    let suffix = Uuid::new_v4();
    let admin: i64 = sqlx::query_scalar(
        "INSERT INTO users(institution_id,username,password_hash,role) VALUES(1,$1,'x','admin') RETURNING id",
    ).bind(format!("admin-{suffix}")).fetch_one(&pool).await.unwrap();
    let doctor: i64 = sqlx::query_scalar(
        "INSERT INTO users(institution_id,username,password_hash,role) VALUES(1,$1,'x','radiologist') RETURNING id",
    ).bind(format!("doctor-{suffix}")).fetch_one(&pool).await.unwrap();
    let outsider: i64 = sqlx::query_scalar(
        "INSERT INTO users(institution_id,username,password_hash,role) VALUES(1,$1,'x','radiologist') RETURNING id",
    ).bind(format!("outsider-{suffix}")).fetch_one(&pool).await.unwrap();

    let study = unique_uid();
    let series = unique_uid();
    let sop = unique_uid();
    let mut object = ct_instance(&study, &series, &sop);
    object.put(DataElement::new(
        tags::PATIENT_ID,
        VR::LO,
        format!("CLINICAL-{suffix}"),
    ));
    let metadata = extract_metadata(&object).unwrap();
    pacs_db::ingest_instance(
        &pool,
        &metadata,
        StorageRecord {
            relative_path: "clinical/test.dcm",
            size: 100,
            sha256: &[7; 32],
        },
    )
    .await
    .unwrap();
    pacs_db::record_dimse_origin(&pool, 1, &sop, "CT1", "10.0.0.1")
        .await
        .unwrap();
    let pending = pacs_db::list_devices(&pool, 1, None).await.unwrap();
    let device = pending
        .into_iter()
        .find(|d| d.calling_ae_title == "CT1" && d.source_ip == "10.0.0.1")
        .unwrap();
    pacs_db::approve_device(
        &pool,
        1,
        device.id,
        ApproveDevice {
            name: "CT 1",
            modality_hint: Some("CT"),
        },
        admin,
    )
    .await
    .unwrap();
    pacs_db::replace_user_device_grants(&pool, 1, doctor, &[device.id], admin)
        .await
        .unwrap();

    assert!(
        pacs_db::can_access_series(&pool, 1, doctor, false, &series)
            .await
            .unwrap()
    );
    assert!(
        !pacs_db::can_access_series(&pool, 1, outsider, false, &series)
            .await
            .unwrap()
    );
    let queue = pacs_db::list_clinical_work(
        &pool,
        1,
        doctor,
        false,
        chrono::Utc::now().date_naive(),
        None,
    )
    .await
    .unwrap();
    let item = queue
        .into_iter()
        .find(|item| item.series_uid == series)
        .unwrap();
    pacs_db::claim_work_item(&pool, 1, item.id, doctor, item.revision)
        .await
        .unwrap();
    assert!(
        pacs_db::claim_work_item(&pool, 1, item.id, outsider, item.revision)
            .await
            .is_err()
    );

    let report = pacs_db::create_report(
        &pool,
        1,
        doctor,
        &study,
        std::slice::from_ref(&series),
        None,
        false,
    )
    .await
    .unwrap();
    let report = pacs_db::update_report_draft(
        &pool,
        1,
        report.id,
        doctor,
        report.revision,
        "双肺纹理清晰",
        "未见明确异常",
        None,
        None,
        false,
        false,
    )
    .await
    .unwrap();
    assert!(
        pacs_db::sign_report(&pool, 1, report.id, doctor, report.revision)
            .await
            .is_err(),
        "作者直签必须被数据层拒绝"
    );
    pacs_db::submit_report(&pool, 1, report.id, doctor, report.revision)
        .await
        .unwrap();
    pacs_db::start_report_review(&pool, 1, report.id, outsider, report.revision + 1)
        .await
        .unwrap();
    pacs_db::approve_report(
        &pool,
        1,
        report.id,
        outsider,
        report.revision + 2,
        false,
        None,
        None,
        None,
        Some("审核通过"),
    )
    .await
    .unwrap();
    let versions = pacs_db::list_report_versions(&pool, 1, report.id, doctor, false)
        .await
        .unwrap();
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].findings, "双肺纹理清晰");
    let audited: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM audit_log WHERE user_fk=$1 AND action='report_signed' AND detail->>'report_id'=$2)",
    )
    .bind(outsider)
    .bind(report.id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(audited, "签发和审计必须在同一事务内完成");
}
