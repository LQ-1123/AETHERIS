use chrono::{Duration, Utc};
use pacs_core::fixture::{ct_instance, unique_uid};
use pacs_db::{
    IngestPreflight, JobKind, JobStatus, LifecyclePathUpdate, LifecyclePolicyInput, NewJob,
    StorageRecord, StorageTier,
};
use pacs_store::{InstanceKey, Store};
use serde_json::json;
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
        eprintln!("跳过生命周期数据库测试: 未设置 PACS_TEST_DATABASE_URL");
        return None;
    };
    if !Postgres::database_exists(&url).await.unwrap_or(false) {
        Postgres::create_database(&url).await.unwrap();
    }
    let pool = pacs_db::connect(&url).await.unwrap();
    pacs_db::migrate(&pool).await.expect("生命周期迁移应能应用");
    Some(pool)
}

async fn ingest(pool: &PgPool) -> (String, String, Vec<u8>) {
    let (study, series, sop) = (unique_uid(), unique_uid(), unique_uid());
    let object = ct_instance(&study, &series, &sop);
    let mut bytes = Vec::new();
    object.write_all(&mut bytes).unwrap();
    let metadata = pacs_core::extract_metadata(&object).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).await.unwrap();
    let stored = store
        .store(
            InstanceKey {
                study: &metadata.study.uid,
                series: &metadata.series.uid,
                sop: &metadata.instance.uid,
            },
            &bytes,
        )
        .await
        .unwrap();
    pacs_db::ingest_instance_for_institution(
        pool,
        &metadata,
        StorageRecord {
            relative_path: &stored.relative_path,
            size: stored.size,
            sha256: &stored.sha256,
        },
        1,
    )
    .await
    .unwrap();
    (study, stored.relative_path, stored.sha256.to_vec())
}

fn policy<'a>(
    name: &'a str,
    modalities: &'a [String],
    tags: &'a serde_json::Value,
    signature: &'a [u8],
    enabled: bool,
) -> LifecyclePolicyInput<'a> {
    LifecyclePolicyInput {
        name,
        priority: 10,
        enabled,
        target_tier: StorageTier::Cold,
        modalities,
        study_date_before: None,
        last_accessed_before: None,
        tag_matches: tags,
        minimum_study_bytes: Some(1),
        minimum_storage_used_percent: None,
        definition_signature: signature,
    }
}

#[tokio::test]
async fn policy_requires_a_preview_of_the_current_definition() {
    let Some(pool) = pool().await else { return };
    let name = format!("lifecycle-{}", Uuid::new_v4());
    let modalities = vec!["CT".to_owned()];
    let tags = json!({});
    let signature = [1_u8; 32];
    let created = pacs_db::create_lifecycle_policy(
        &pool,
        1,
        None,
        &policy(&name, &modalities, &tags, &signature, false),
    )
    .await
    .unwrap();
    assert!(!created.enabled);
    assert!(!created.preview_current);

    let preview = json!({"matched_studies":0,"matched_bytes":0});
    pacs_db::record_lifecycle_preview(&pool, 1, created.id, &signature, &preview)
        .await
        .unwrap();
    let enabled = pacs_db::update_lifecycle_policy(
        &pool,
        1,
        created.id,
        &policy(&name, &modalities, &tags, &signature, true),
    )
    .await
    .unwrap();
    assert!(enabled.enabled && enabled.preview_current);

    let changed_signature = [2_u8; 32];
    let error = pacs_db::update_lifecycle_policy(
        &pool,
        1,
        created.id,
        &policy(&name, &modalities, &tags, &changed_signature, true),
    )
    .await
    .unwrap_err();
    assert!(matches!(error, pacs_db::DbError::Conflict(_)));
    pacs_db::delete_lifecycle_policy(&pool, 1, created.id)
        .await
        .unwrap();
}

#[tokio::test]
async fn lifecycle_studies_include_patient_display_tags() {
    let Some(pool) = pool().await else { return };
    let (study_uid, _, _) = ingest(&pool).await;

    let studies = pacs_db::list_lifecycle_studies(&pool, 1, 1000)
        .await
        .unwrap();
    let study = studies
        .iter()
        .find(|study| study.study_instance_uid == study_uid)
        .expect("刚入库的 Study 应出现在生命周期列表中");

    assert_eq!(study.patient_name.as_deref(), Some("Doe^John^^^"));
    assert_eq!(study.patient_id, "PID-0001");
}

#[tokio::test]
async fn legal_hold_freezes_and_resumes_the_purge_grace_period() {
    let Some(pool) = pool().await else { return };
    let (study_uid, _, _) = ingest(&pool).await;
    sqlx::query(
        "UPDATE studies SET storage_tier='quarantine'
         WHERE institution_id=1 AND study_instance_uid=$1",
    )
    .bind(&study_uid)
    .execute(&pool)
    .await
    .unwrap();

    let request = pacs_db::create_purge_request(&pool, 1, &study_uid, "保留期届满", None)
        .await
        .unwrap();
    let approved = pacs_db::approve_purge_request(
        &pool,
        1,
        request.id,
        Utc::now() + Duration::minutes(90),
        None,
    )
    .await
    .unwrap();
    let job_id = approved.job_id.unwrap();

    let hold = pacs_db::create_legal_hold(&pool, 1, &study_uid, "诉讼保全", None, None)
        .await
        .unwrap();
    let paused = pacs_db::list_purge_requests(&pool, 1)
        .await
        .unwrap()
        .into_iter()
        .find(|value| value.id == request.id)
        .unwrap();
    assert_eq!(paused.status, "paused_hold");
    assert!(paused.grace_until.is_none());
    let remaining = paused.grace_remaining_seconds.unwrap();
    assert!((5_398..=5_400).contains(&remaining));
    let paused_job = pacs_db::get_background_job(&pool, 1, job_id).await.unwrap();
    assert_eq!(paused_job.status, JobStatus::Paused);
    assert_eq!(
        paused_job.error_message.as_deref(),
        Some("因 Legal Hold 暂停")
    );
    assert!(pacs_db::begin_purge(&pool, 1, request.id).await.is_err());

    pacs_db::release_legal_hold(&pool, 1, hold.id, None)
        .await
        .unwrap();
    let resumed = pacs_db::list_purge_requests(&pool, 1)
        .await
        .unwrap()
        .into_iter()
        .find(|value| value.id == request.id)
        .unwrap();
    assert_eq!(resumed.status, "approved");
    assert!(resumed.grace_remaining_seconds.is_none());
    let resumed_until = resumed.grace_until.unwrap();
    let resumed_seconds = (resumed_until - Utc::now()).num_seconds();
    assert!((remaining - resumed_seconds).abs() <= 1);
    let resumed_job = pacs_db::get_background_job(&pool, 1, job_id).await.unwrap();
    assert_eq!(resumed_job.status, JobStatus::Queued);
    assert!(
        (resumed_job.available_at - resumed_until)
            .num_milliseconds()
            .abs()
            <= 1
    );
    assert!(resumed_job.error_message.is_none());

    let events = pacs_db::list_lifecycle_events(&pool, 1, 1000)
        .await
        .unwrap();
    assert!(events.iter().any(|event| {
        event.study_instance_uid == study_uid && event.action == "purge_paused_hold"
    }));
    assert!(events.iter().any(|event| {
        event.study_instance_uid == study_uid && event.action == "purge_resumed_hold"
    }));
}

#[tokio::test]
async fn legal_hold_blocks_quarantine_and_purge_is_study_scoped() {
    let Some(pool) = pool().await else { return };
    let (study_uid, old_path, sha256) = ingest(&pool).await;
    let hold = pacs_db::create_legal_hold(&pool, 1, &study_uid, "诉讼保全", None, None)
        .await
        .unwrap();
    let files = pacs_db::lifecycle_files_for_study(&pool, 1, &study_uid)
        .await
        .unwrap()
        .1;
    let quarantine_path = format!("quarantine/{old_path}");
    let job = pacs_db::create_background_job(
        &pool,
        NewJob {
            id: Uuid::new_v4(),
            institution_id: 1,
            created_by: None,
            kind: JobKind::Lifecycle,
            idempotency_key: None,
            payload: &json!({"operation":"move"}),
            progress_total: 1,
            max_attempts: 3,
            available_at: None,
        },
    )
    .await
    .unwrap();
    let updates = vec![LifecyclePathUpdate {
        version_id: files[0].version_id,
        old_path: old_path.clone(),
        new_path: quarantine_path.clone(),
    }];
    let blocked = pacs_db::switch_study_storage_tier(
        &pool,
        1,
        &study_uid,
        StorageTier::Hot,
        StorageTier::Quarantine,
        &updates,
        job.id,
        None,
    )
    .await
    .unwrap_err();
    assert!(matches!(blocked, pacs_db::DbError::Conflict(_)));

    pacs_db::release_legal_hold(&pool, 1, hold.id, None)
        .await
        .unwrap();
    pacs_db::switch_study_storage_tier(
        &pool,
        1,
        &study_uid,
        StorageTier::Hot,
        StorageTier::Quarantine,
        &updates,
        job.id,
        None,
    )
    .await
    .unwrap();
    let request = pacs_db::create_purge_request(&pool, 1, &study_uid, "保留期届满", None)
        .await
        .unwrap();
    let approved = pacs_db::approve_purge_request(&pool, 1, request.id, Utc::now(), None)
        .await
        .unwrap();
    let purge_job = approved.job_id.unwrap();
    pacs_db::begin_purge(&pool, 1, request.id).await.unwrap();
    let purge_files = pacs_db::commit_purge_metadata(&pool, 1, request.id)
        .await
        .unwrap();
    assert_eq!(purge_files.len(), 1);
    assert_eq!(purge_files[0].relative_path, quarantine_path);
    assert_eq!(purge_files[0].file_sha256, sha256);
    assert!(
        pacs_db::lifecycle_files_for_study(&pool, 1, &study_uid)
            .await
            .is_err()
    );
    pacs_db::mark_purge_file_deleted(&pool, request.id, "dicom", &quarantine_path)
        .await
        .unwrap();
    pacs_db::finalize_purge(&pool, 1, request.id, purge_job, None)
        .await
        .unwrap();
    let requests = pacs_db::list_purge_requests(&pool, 1).await.unwrap();
    assert_eq!(
        requests
            .iter()
            .find(|value| value.id == request.id)
            .unwrap()
            .status,
        "completed"
    );

    let immutable =
        sqlx::query("UPDATE dicom_lifecycle_events SET details='{}' WHERE study_instance_uid=$1")
            .bind(&study_uid)
            .execute(&pool)
            .await;
    assert!(immutable.is_err(), "生命周期审计记录必须禁止修改");
}

#[tokio::test]
async fn cold_retransmission_is_idempotent_and_quarantine_hides_the_study() {
    let Some(pool) = pool().await else { return };
    let (study_uid, hot_path, sha256) = ingest(&pool).await;
    let (patient_id, series_uid, sop_uid): (i64, String, String) = sqlx::query_as(
        "SELECT st.patient_fk,se.series_instance_uid,i.sop_instance_uid
         FROM studies st JOIN series se ON se.study_fk=st.id JOIN instances i ON i.series_fk=se.id
         WHERE st.institution_id=1 AND st.study_instance_uid=$1",
    )
    .bind(&study_uid)
    .fetch_one(&pool)
    .await
    .unwrap();
    let files = pacs_db::lifecycle_files_for_study(&pool, 1, &study_uid)
        .await
        .unwrap()
        .1;
    let job = pacs_db::create_background_job(
        &pool,
        NewJob {
            id: Uuid::new_v4(),
            institution_id: 1,
            created_by: None,
            kind: JobKind::Lifecycle,
            idempotency_key: None,
            payload: &json!({"operation":"move"}),
            progress_total: 1,
            max_attempts: 3,
            available_at: None,
        },
    )
    .await
    .unwrap();
    let cold_path = format!("cold/{hot_path}");
    pacs_db::switch_study_storage_tier(
        &pool,
        1,
        &study_uid,
        StorageTier::Hot,
        StorageTier::Cold,
        &[LifecyclePathUpdate {
            version_id: files[0].version_id,
            old_path: hot_path.clone(),
            new_path: cold_path.clone(),
        }],
        job.id,
        None,
    )
    .await
    .unwrap();

    let duplicate = ct_instance(&study_uid, &series_uid, &sop_uid);
    let duplicate_metadata = pacs_core::extract_metadata(&duplicate).unwrap();
    assert_eq!(
        pacs_db::preflight_instance_for_institution(&pool, &duplicate_metadata, &sha256, 1)
            .await
            .unwrap(),
        IngestPreflight::Duplicate
    );
    let new_object = ct_instance(&study_uid, &unique_uid(), &unique_uid());
    let new_metadata = pacs_core::extract_metadata(&new_object).unwrap();
    let blocked = pacs_db::preflight_instance_for_institution(&pool, &new_metadata, &[9_u8; 32], 1)
        .await
        .unwrap_err();
    assert!(matches!(blocked, pacs_db::DbError::Conflict(_)));
    assert!(
        pacs_db::find_instance_for_institution(&pool, 1, &study_uid, &series_uid, &sop_uid)
            .await
            .unwrap()
            .is_some(),
        "冷层 Study 仍可读取"
    );

    let quarantine_path = format!("quarantine/{hot_path}");
    pacs_db::switch_study_storage_tier(
        &pool,
        1,
        &study_uid,
        StorageTier::Cold,
        StorageTier::Quarantine,
        &[LifecyclePathUpdate {
            version_id: files[0].version_id,
            old_path: cold_path,
            new_path: quarantine_path,
        }],
        job.id,
        None,
    )
    .await
    .unwrap();
    assert!(
        pacs_db::find_instance_for_institution(&pool, 1, &study_uid, &series_uid, &sop_uid)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        pacs_db::list_export_sources(&pool, 1, &study_uid, None)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        pacs_db::route_sources_for_scope(&pool, 1, &study_uid, None)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        pacs_db::list_patient_studies(&pool, 1, 0, true, patient_id)
            .await
            .unwrap()
            .iter()
            .all(|study| study.study_uid != study_uid)
    );
}

#[tokio::test]
async fn an_expired_legal_hold_can_be_replaced_with_an_audited_hold() {
    let Some(pool) = pool().await else { return };
    let (study_uid, _, _) = ingest(&pool).await;
    let first = pacs_db::create_legal_hold(
        &pool,
        1,
        &study_uid,
        "短期保全",
        Some(Utc::now() + Duration::hours(1)),
        None,
    )
    .await
    .unwrap();
    sqlx::query("UPDATE dicom_legal_holds SET expires_at=now()-interval '1 hour' WHERE id=$1")
        .bind(first.id)
        .execute(&pool)
        .await
        .unwrap();

    let replacement = pacs_db::create_legal_hold(&pool, 1, &study_uid, "继续保全", None, None)
        .await
        .unwrap();
    assert_ne!(first.id, replacement.id);
    let holds = pacs_db::list_legal_holds(&pool, 1).await.unwrap();
    assert!(
        holds
            .iter()
            .find(|hold| hold.id == first.id)
            .unwrap()
            .released_at
            .is_some()
    );
    let events = pacs_db::list_lifecycle_events(&pool, 1, 1000)
        .await
        .unwrap();
    assert!(events.iter().any(|event| {
        event.study_instance_uid == study_uid
            && event.action == "legal_hold_released"
            && event.details["automatic"] == true
    }));
    pacs_db::release_legal_hold(&pool, 1, replacement.id, None)
        .await
        .unwrap();
}
