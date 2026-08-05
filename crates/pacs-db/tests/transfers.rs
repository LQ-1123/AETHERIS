use chrono::{Duration, Utc};
use pacs_db::{ImportUpload, JobKind, NewJob, UploadStatus};
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
        eprintln!("\n>>> 跳过传输数据库测试: 未设置 PACS_TEST_DATABASE_URL\n");
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
async fn upload_requires_sequential_offsets_before_job_release() {
    let Some(pool) = pool().await else { return };
    let job_id = Uuid::new_v4();
    let deferred = Utc::now() + Duration::days(100);
    let payload = serde_json::json!({});
    let job = pacs_db::create_background_job(
        &pool,
        NewJob {
            id: job_id,
            institution_id: 1,
            created_by: None,
            kind: JobKind::Import,
            idempotency_key: None,
            payload: &payload,
            progress_total: 0,
            max_attempts: 3,
            available_at: Some(deferred),
        },
    )
    .await
    .unwrap();
    assert!(job.available_at > Utc::now() + Duration::days(99));

    let upload = pacs_db::create_import_upload(
        &pool,
        1,
        &ImportUpload {
            id: Uuid::new_v4(),
            job_id,
            relative_name: "folder/image.dcm".to_owned(),
            expected_size: 8,
            expected_sha256: None,
            received_size: 0,
            temp_name: format!("{}.part", Uuid::new_v4()),
            status: UploadStatus::Uploading,
            error_message: None,
        },
    )
    .await
    .unwrap();
    assert!(
        pacs_db::advance_upload(&pool, 1, upload.id, 1, 4)
            .await
            .is_err()
    );
    let partial = pacs_db::advance_upload(&pool, 1, upload.id, 0, 4)
        .await
        .unwrap();
    assert_eq!(partial.received_size, 4);
    assert!(
        pacs_db::mark_upload_ready(&pool, 1, upload.id)
            .await
            .is_err()
    );
    pacs_db::advance_upload(&pool, 1, upload.id, 4, 4)
        .await
        .unwrap();
    let ready = pacs_db::mark_upload_ready(&pool, 1, upload.id)
        .await
        .unwrap();
    assert_eq!(ready.status, UploadStatus::Ready);
    let released = pacs_db::release_background_job(&pool, 1, job_id)
        .await
        .unwrap();
    assert!(released.available_at <= Utc::now() + Duration::seconds(1));
}
