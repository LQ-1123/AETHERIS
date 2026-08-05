use chrono::{Duration, Utc};
use pacs_db::{
    JobItemStatus, JobKind, JobStatus, NewJob, add_background_job_item, claim_background_job,
    complete_background_job, create_background_job, fail_background_job,
    finish_background_job_item, get_background_job, list_background_job_items,
    request_job_cancellation, start_background_job_item, update_background_job_progress,
};
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
        eprintln!("\n>>> 跳过后台任务数据库测试: 未设置 PACS_TEST_DATABASE_URL\n");
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

fn new_job(id: Uuid, kind: JobKind, key: Option<&str>, max_attempts: i32) -> NewJob<'_> {
    NewJob {
        id,
        institution_id: 1,
        created_by: None,
        kind,
        idempotency_key: key,
        payload: &serde_json::Value::Null,
        progress_total: 2,
        max_attempts,
        available_at: None,
    }
}

#[tokio::test]
async fn idempotency_returns_the_original_job() {
    let Some(pool) = pool().await else { return };
    let key = format!("import-test-{}", Uuid::new_v4());
    let first = create_background_job(
        &pool,
        new_job(Uuid::new_v4(), JobKind::Import, Some(&key), 3),
    )
    .await
    .unwrap();
    let repeated = create_background_job(
        &pool,
        new_job(Uuid::new_v4(), JobKind::Import, Some(&key), 3),
    )
    .await
    .unwrap();
    assert_eq!(first.id, repeated.id);
    assert_eq!(repeated.status, JobStatus::Queued);
}

#[tokio::test]
async fn worker_lease_guards_progress_and_completion() {
    let Some(pool) = pool().await else { return };
    let job = create_background_job(&pool, new_job(Uuid::new_v4(), JobKind::Export, None, 3))
        .await
        .unwrap();
    let worker = Uuid::new_v4();
    let claimed = claim_background_job(&pool, JobKind::Export, worker, Duration::minutes(1))
        .await
        .unwrap()
        .expect("应领取任务");
    assert_eq!(claimed.id, job.id);
    assert_eq!(claimed.status, JobStatus::Running);
    assert_eq!(claimed.attempts, 1);
    assert!(
        !update_background_job_progress(&pool, job.id, Uuid::new_v4(), 1, 2)
            .await
            .unwrap()
    );
    assert!(
        update_background_job_progress(&pool, job.id, worker, 1, 2)
            .await
            .unwrap()
    );
    assert!(
        complete_background_job(&pool, job.id, worker, &json!({"created": 1}))
            .await
            .unwrap()
    );
    let completed = get_background_job(&pool, 1, job.id).await.unwrap();
    assert_eq!(completed.status, JobStatus::Succeeded);
    assert_eq!(completed.result["created"], 1);
}

#[tokio::test]
async fn item_details_are_idempotent_and_track_terminal_results() {
    let Some(pool) = pool().await else { return };
    let job = create_background_job(&pool, new_job(Uuid::new_v4(), JobKind::Lifecycle, None, 3))
        .await
        .unwrap();
    let first =
        add_background_job_item(&pool, job.id, "study/1.2.3", &json!({"study_uid": "1.2.3"}))
            .await
            .unwrap();
    let repeated = add_background_job_item(
        &pool,
        job.id,
        "study/1.2.3",
        &json!({"ignored_on_retry": true}),
    )
    .await
    .unwrap();
    assert_eq!(first.id, repeated.id);
    assert_eq!(repeated.input["study_uid"], "1.2.3");
    let running = start_background_job_item(&pool, job.id, "study/1.2.3")
        .await
        .unwrap();
    assert_eq!(running.status, JobItemStatus::Running);
    let finished = finish_background_job_item(
        &pool,
        job.id,
        "study/1.2.3",
        JobItemStatus::Skipped,
        &json!({"reason": "legal_hold"}),
        None,
    )
    .await
    .unwrap();
    assert_eq!(finished.status, JobItemStatus::Skipped);
    let items = list_background_job_items(&pool, 1, job.id).await.unwrap();
    assert_eq!(items, vec![finished]);
}

#[tokio::test]
async fn retry_and_cancellation_are_durable() {
    let Some(pool) = pool().await else { return };
    let job = create_background_job(&pool, new_job(Uuid::new_v4(), JobKind::Route, None, 2))
        .await
        .unwrap();
    let worker = Uuid::new_v4();
    claim_background_job(&pool, JobKind::Route, worker, Duration::minutes(1))
        .await
        .unwrap()
        .expect("应领取任务");
    assert!(
        fail_background_job(&pool, job.id, worker, "temporary", Some(Utc::now()))
            .await
            .unwrap()
    );
    assert_eq!(
        get_background_job(&pool, 1, job.id).await.unwrap().status,
        JobStatus::Queued
    );
    let cancelled = request_job_cancellation(&pool, 1, job.id).await.unwrap();
    assert_eq!(cancelled.status, JobStatus::Cancelled);
    assert!(
        claim_background_job(&pool, JobKind::Route, worker, Duration::minutes(1))
            .await
            .unwrap()
            .is_none()
    );
}
