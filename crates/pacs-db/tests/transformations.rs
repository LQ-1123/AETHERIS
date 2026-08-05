//! Transactional invariants for versioned DICOM transformations.

use chrono::{Duration, Utc};
use dicom::core::{DataElement, VR};
use dicom::dictionary_std::tags;
use pacs_core::fixture::{ct_instance, unique_uid};
use pacs_core::{InstanceMetadata, Uid, extract_metadata, normalize_person_name};
use pacs_db::{
    ActivatedVersion, DbError, NewPreviewJob, StorageRecord, TargetType, TransformMode,
    TransformSource, TransformTarget, activate_clinical_job, claim_job, create_preview_job,
    ingest_instance, list_revisions, queue_preview_job, select_transform_sources,
};
use serde_json::Value;
use sqlx::migrate::MigrateDatabase;
use sqlx::{PgPool, Postgres};
use uuid::Uuid;

async fn pool() -> Option<PgPool> {
    dotenvy::dotenv().ok();
    let Ok(url) = std::env::var("PACS_TEST_DATABASE_URL") else {
        assert!(
            std::env::var_os("CI").is_none(),
            "CI 必须设置 PACS_TEST_DATABASE_URL"
        );
        eprintln!("\n>>> 跳过转换数据库测试: 未设置 PACS_TEST_DATABASE_URL\n");
        return None;
    };
    if !Postgres::database_exists(&url).await.unwrap_or(false)
        && let Err(error) = Postgres::create_database(&url).await
    {
        assert!(
            Postgres::database_exists(&url).await.unwrap_or(false),
            "创建测试库失败: {error}"
        );
    }
    let pool = pacs_db::connect(&url).await.expect("应能连接测试库");
    pacs_db::migrate(&pool).await.expect("迁移应能应用");
    Some(pool)
}

macro_rules! require_db {
    () => {
        match pool().await {
            Some(pool) => pool,
            None => return,
        }
    };
}

fn fresh_metadata() -> InstanceMetadata {
    let mut object = ct_instance(&unique_uid(), &unique_uid(), &unique_uid());
    object.put(DataElement::new(
        tags::PATIENT_ID,
        VR::LO,
        format!("TRANSFORM-{}", Uuid::new_v4()),
    ));
    extract_metadata(&object).expect("夹具应能提取")
}

async fn ingest(pool: &PgPool, metadata: &InstanceMetadata, fill: u8) -> pacs_db::Ingested {
    let path = format!("test/{}/original.dcm", metadata.instance.uid);
    ingest_instance(
        pool,
        metadata,
        StorageRecord {
            relative_path: &path,
            size: 4096,
            sha256: &[fill; 32],
        },
    )
    .await
    .expect("测试实例应能入库")
}

async fn user(pool: &PgPool) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (institution_id, username, password_hash, role)
         VALUES (1, $1, 'unused-test-hash', 'admin') RETURNING id",
    )
    .bind(format!("transform-{}", Uuid::new_v4()))
    .fetch_one(pool)
    .await
    .expect("应能创建测试用户")
}

async fn running_job(
    pool: &PgPool,
    user_id: i64,
    mode: TransformMode,
    target: &TransformTarget,
    sources: &[TransformSource],
) -> Uuid {
    let id = Uuid::new_v4();
    let rules = Value::Array(Vec::new());
    let preview = serde_json::json!({});
    let token = [0x51; 32];
    create_preview_job(
        pool,
        NewPreviewJob {
            id,
            institution_id: 1,
            user_id,
            username: "transform-test",
            mode,
            target,
            rules: &rules,
            reason: "integration test change",
            confirmation_hash: &token,
            confirmation_expires_at: Utc::now() + Duration::minutes(5),
            preview: &preview,
            pixel_risk: "safe",
        },
        sources,
    )
    .await
    .expect("应能创建预览任务");
    queue_preview_job(pool, 1, user_id, id, &token)
        .await
        .expect("基础修订未变化时应能确认");
    claim_job(pool, 1, id).await.expect("应能领取任务");
    id
}

fn revised(
    source: TransformSource,
    mut metadata: InstanceMetadata,
    source_version_id: i64,
    name: &str,
    fill: u8,
) -> ActivatedVersion {
    metadata.patient.name = Some(name.to_owned());
    metadata.patient.name_normalized = Some(normalize_person_name(name));
    metadata.study.uid = Uid::generate();
    metadata.series.uid = Uid::generate();
    metadata.instance.uid = Uid::generate();
    ActivatedVersion {
        derivation_source_version_id: source_version_id,
        source,
        metadata,
        storage_path: format!("derived/{}/result.dcm", Uuid::new_v4()),
        file_size: 8192,
        file_sha256: [fill; 32],
        uid_map: serde_json::json!({}),
    }
}

#[tokio::test]
async fn activation_creates_a_revision_and_preserves_original_digest() {
    let pool = require_db!();
    let metadata = fresh_metadata();
    let ingested = ingest(&pool, &metadata, 0x11).await;
    let user_id = user(&pool).await;
    let target = TransformTarget {
        target_type: TargetType::Patient,
        key: ingested.patient_id.to_string(),
    };
    let sources = select_transform_sources(&pool, 1, &target).await.unwrap();
    let source = sources[0].clone();
    let job = running_job(
        &pool,
        user_id,
        TransformMode::ClinicalCorrection,
        &target,
        &sources,
    )
    .await;
    let output = revised(
        source.clone(),
        metadata,
        source.current_version_id,
        "修订^患者",
        0x22,
    );
    activate_clinical_job(
        &pool,
        job,
        1,
        user_id,
        "transform-test",
        TransformMode::ClinicalCorrection,
        "integration test change",
        &[output],
    )
    .await
    .expect("应原子激活新修订");

    let revisions = list_revisions(&pool, 1, source.logical_instance_id)
        .await
        .unwrap();
    assert_eq!(revisions.len(), 2);
    assert!(revisions[0].is_current);
    assert_eq!(revisions[0].version_number, 2);
    assert_eq!(revisions[1].file_sha256_hex, "11".repeat(32));
    assert_eq!(revisions[1].derivation_kind, "original");
    let name: Option<String> = sqlx::query_scalar("SELECT name FROM patients WHERE id = $1")
        .bind(ingested.patient_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(name.as_deref(), Some("修订^患者"));
}

#[tokio::test]
async fn stale_activation_cannot_overwrite_a_newer_revision() {
    let pool = require_db!();
    let metadata = fresh_metadata();
    let ingested = ingest(&pool, &metadata, 0x31).await;
    let user_id = user(&pool).await;
    let target = TransformTarget {
        target_type: TargetType::Study,
        key: metadata.study.uid.to_string(),
    };
    let sources = select_transform_sources(&pool, 1, &target).await.unwrap();
    let first_job = running_job(
        &pool,
        user_id,
        TransformMode::ClinicalCorrection,
        &target,
        &sources,
    )
    .await;
    let second_job = running_job(
        &pool,
        user_id,
        TransformMode::ClinicalCorrection,
        &target,
        &sources,
    )
    .await;
    let source = sources[0].clone();
    let first = revised(
        source.clone(),
        metadata.clone(),
        source.current_version_id,
        "FIRST^REVISION",
        0x32,
    );
    activate_clinical_job(
        &pool,
        first_job,
        1,
        user_id,
        "transform-test",
        TransformMode::ClinicalCorrection,
        "first revision",
        &[first],
    )
    .await
    .unwrap();
    let stale = revised(
        source.clone(),
        metadata,
        source.current_version_id,
        "STALE^REVISION",
        0x33,
    );
    assert!(matches!(
        activate_clinical_job(
            &pool,
            second_job,
            1,
            user_id,
            "transform-test",
            TransformMode::ClinicalCorrection,
            "stale revision",
            &[stale]
        )
        .await,
        Err(DbError::Conflict(_))
    ));
    let current: i32 = sqlx::query_scalar(
        "SELECT v.version_number FROM instances i
         JOIN dicom_instance_versions v ON v.id = i.current_version_id WHERE i.id = $1",
    )
    .bind(ingested.instance_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(current, 2);
}

#[tokio::test]
async fn patient_id_collision_rolls_back_the_whole_activation() {
    let pool = require_db!();
    let first_metadata = fresh_metadata();
    let second_metadata = fresh_metadata();
    let first = ingest(&pool, &first_metadata, 0x41).await;
    ingest(&pool, &second_metadata, 0x42).await;
    let user_id = user(&pool).await;
    let target = TransformTarget {
        target_type: TargetType::Patient,
        key: first.patient_id.to_string(),
    };
    let sources = select_transform_sources(&pool, 1, &target).await.unwrap();
    let job = running_job(
        &pool,
        user_id,
        TransformMode::ClinicalCorrection,
        &target,
        &sources,
    )
    .await;
    let source = sources[0].clone();
    let mut output = revised(
        source.clone(),
        first_metadata.clone(),
        source.current_version_id,
        "COLLISION^TEST",
        0x43,
    );
    output.metadata.patient.patient_id = second_metadata.patient.patient_id.clone();
    assert!(matches!(
        activate_clinical_job(
            &pool,
            job,
            1,
            user_id,
            "transform-test",
            TransformMode::ClinicalCorrection,
            "patient id collision",
            &[output]
        )
        .await,
        Err(DbError::Conflict(_))
    ));
    let current: Option<i64> =
        sqlx::query_scalar("SELECT current_version_id FROM instances WHERE id = $1")
            .bind(first.instance_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(current, Some(source.current_version_id));
}

#[tokio::test]
async fn rollback_derives_version_three_from_selected_history() {
    let pool = require_db!();
    let original = fresh_metadata();
    let ingested = ingest(&pool, &original, 0x61).await;
    let user_id = user(&pool).await;
    let mut target = TransformTarget {
        target_type: TargetType::Study,
        key: original.study.uid.to_string(),
    };
    let sources = select_transform_sources(&pool, 1, &target).await.unwrap();
    let original_version = sources[0].current_version_id;
    let correction_job = running_job(
        &pool,
        user_id,
        TransformMode::ClinicalCorrection,
        &target,
        &sources,
    )
    .await;
    let correction = revised(
        sources[0].clone(),
        original.clone(),
        original_version,
        "CORRECTED^NAME",
        0x62,
    );
    activate_clinical_job(
        &pool,
        correction_job,
        1,
        user_id,
        "transform-test",
        TransformMode::ClinicalCorrection,
        "correction before rollback",
        &[correction],
    )
    .await
    .unwrap();

    let logical_id: Uuid =
        sqlx::query_scalar("SELECT logical_instance_id FROM instances WHERE id = $1")
            .bind(ingested.instance_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    target = TransformTarget {
        target_type: TargetType::Instance,
        key: logical_id.to_string(),
    };
    let current_sources = select_transform_sources(&pool, 1, &target).await.unwrap();
    let rollback_job = running_job(
        &pool,
        user_id,
        TransformMode::Rollback,
        &target,
        &current_sources,
    )
    .await;
    let rollback = revised(
        current_sources[0].clone(),
        original,
        original_version,
        "DOE^JOHN",
        0x63,
    );
    activate_clinical_job(
        &pool,
        rollback_job,
        1,
        user_id,
        "transform-test",
        TransformMode::Rollback,
        "restore original metadata",
        &[rollback],
    )
    .await
    .unwrap();
    let revisions = list_revisions(&pool, 1, logical_id).await.unwrap();
    assert_eq!(revisions[0].version_number, 3);
    assert_eq!(revisions[0].source_version_id, Some(original_version));
    assert_eq!(revisions[0].derivation_kind, "rollback");
}

#[tokio::test]
async fn mandatory_audit_failure_rolls_back_activation() {
    let pool = require_db!();
    let metadata = fresh_metadata();
    let ingested = ingest(&pool, &metadata, 0x71).await;
    let user_id = user(&pool).await;
    let target = TransformTarget {
        target_type: TargetType::Patient,
        key: ingested.patient_id.to_string(),
    };
    let sources = select_transform_sources(&pool, 1, &target).await.unwrap();
    let source = sources[0].clone();
    let job = running_job(
        &pool,
        user_id,
        TransformMode::ClinicalCorrection,
        &target,
        &sources,
    )
    .await;
    let suffix = job.simple().to_string();
    let function_name = format!("reject_transform_audit_{suffix}");
    let trigger_name = format!("reject_transform_audit_trigger_{suffix}");
    let function_sql = format!(
        "CREATE FUNCTION {function_name}() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN
           IF NEW.action = 'dicom_transform_activate'
              AND NEW.detail->>'job_id' = '{job}' THEN
             RAISE EXCEPTION 'injected audit failure';
           END IF;
           RETURN NEW;
         END $$"
    );
    sqlx::query(sqlx::AssertSqlSafe(function_sql))
        .execute(&pool)
        .await
        .unwrap();
    let trigger_sql = format!(
        "CREATE TRIGGER {trigger_name} BEFORE INSERT ON audit_log
         FOR EACH ROW EXECUTE FUNCTION {function_name}()"
    );
    sqlx::query(sqlx::AssertSqlSafe(trigger_sql))
        .execute(&pool)
        .await
        .unwrap();

    let output = revised(
        source.clone(),
        metadata,
        source.current_version_id,
        "AUDIT^FAILURE",
        0x72,
    );
    let result = activate_clinical_job(
        &pool,
        job,
        1,
        user_id,
        "transform-test",
        TransformMode::ClinicalCorrection,
        "audit failure injection",
        &[output],
    )
    .await;
    let drop_trigger = format!("DROP TRIGGER {trigger_name} ON audit_log");
    sqlx::query(sqlx::AssertSqlSafe(drop_trigger))
        .execute(&pool)
        .await
        .unwrap();
    let drop_function = format!("DROP FUNCTION {function_name}()");
    sqlx::query(sqlx::AssertSqlSafe(drop_function))
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(result, Err(DbError::Query(_))));

    let (current, name): (Option<i64>, Option<String>) = sqlx::query_as(
        "SELECT i.current_version_id, p.name FROM instances i
         JOIN series se ON se.id = i.series_fk
         JOIN studies st ON st.id = se.study_fk
         JOIN patients p ON p.id = st.patient_fk WHERE i.id = $1",
    )
    .bind(ingested.instance_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(current, Some(source.current_version_id));
    assert_eq!(name.as_deref(), Some("Doe^John^^^"));
    let versions: i64 =
        sqlx::query_scalar("SELECT count(*) FROM dicom_instance_versions WHERE instance_fk = $1")
            .bind(ingested.instance_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(versions, 1);
}
