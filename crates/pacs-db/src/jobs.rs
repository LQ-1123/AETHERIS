//! Durable background jobs shared by import, export, routing, retrieval and lifecycle workers.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::DbError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    Import,
    Export,
    Route,
    Lifecycle,
    Retrieval,
}

impl JobKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Import => "import",
            Self::Export => "export",
            Self::Route => "route",
            Self::Lifecycle => "lifecycle",
            Self::Retrieval => "retrieval",
        }
    }

    fn parse(raw: &str) -> Result<Self, DbError> {
        match raw {
            "import" => Ok(Self::Import),
            "export" => Ok(Self::Export),
            "route" => Ok(Self::Route),
            "lifecycle" => Ok(Self::Lifecycle),
            "retrieval" => Ok(Self::Retrieval),
            _ => Err(DbError::Invalid(format!("未知后台任务类型 {raw:?}"))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Paused,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobItemStatus {
    Pending,
    Running,
    Succeeded,
    Skipped,
    Conflict,
    Failed,
    Cancelled,
}

impl JobItemStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Skipped => "skipped",
            Self::Conflict => "conflict",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    fn parse(raw: &str) -> Result<Self, DbError> {
        Self::ALL
            .into_iter()
            .find(|status| status.as_str() == raw)
            .ok_or_else(|| DbError::Invalid(format!("未知后台任务明细状态 {raw:?}")))
    }

    const ALL: [Self; 7] = [
        Self::Pending,
        Self::Running,
        Self::Succeeded,
        Self::Skipped,
        Self::Conflict,
        Self::Failed,
        Self::Cancelled,
    ];

    fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Skipped | Self::Conflict | Self::Failed | Self::Cancelled
        )
    }
}

impl JobStatus {
    fn parse(raw: &str) -> Result<Self, DbError> {
        match raw {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "paused" => Ok(Self::Paused),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(DbError::Invalid(format!("未知后台任务状态 {raw:?}"))),
        }
    }
}

#[derive(Debug, Clone)]
pub struct NewJob<'a> {
    pub id: Uuid,
    pub institution_id: i64,
    pub created_by: Option<i64>,
    pub kind: JobKind,
    pub idempotency_key: Option<&'a str>,
    pub payload: &'a Value,
    pub progress_total: i64,
    pub max_attempts: i32,
    /// Keep an upload job out of the worker queue until the client completes it.
    pub available_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackgroundJob {
    pub id: Uuid,
    pub institution_id: i64,
    pub created_by: Option<i64>,
    pub kind: JobKind,
    pub status: JobStatus,
    pub idempotency_key: Option<String>,
    pub payload: Value,
    pub result: Value,
    pub progress_completed: i64,
    pub progress_total: i64,
    pub attempts: i32,
    pub max_attempts: i32,
    pub available_at: DateTime<Utc>,
    pub lease_owner: Option<Uuid>,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub cancel_requested: bool,
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackgroundJobItem {
    pub id: i64,
    pub job_id: Uuid,
    pub item_key: String,
    pub status: JobItemStatus,
    pub attempts: i32,
    pub input: Value,
    pub result: Value,
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

/// Create a job, or return the existing job when the caller repeats an
/// institution/kind/idempotency-key tuple.
pub async fn create_job(pool: &PgPool, new: NewJob<'_>) -> Result<BackgroundJob, DbError> {
    if new.progress_total < 0 || new.max_attempts <= 0 {
        return Err(DbError::Invalid(
            "progress_total 不能为负数且 max_attempts 必须为正数".to_owned(),
        ));
    }
    if new.idempotency_key.is_some_and(|key| key.trim().is_empty()) {
        return Err(DbError::Invalid("幂等键不能为空".to_owned()));
    }

    let row = sqlx::query(
        r#"
        INSERT INTO background_jobs (
            id, institution_id, created_by, kind, idempotency_key,
            payload, progress_total, max_attempts, available_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, COALESCE($9, now()))
        ON CONFLICT (institution_id, kind, idempotency_key)
            WHERE idempotency_key IS NOT NULL
        DO UPDATE SET idempotency_key = EXCLUDED.idempotency_key
        RETURNING *
        "#,
    )
    .bind(new.id)
    .bind(new.institution_id)
    .bind(new.created_by)
    .bind(new.kind.as_str())
    .bind(new.idempotency_key)
    .bind(new.payload)
    .bind(new.progress_total)
    .bind(new.max_attempts)
    .bind(new.available_at)
    .fetch_one(pool)
    .await?;
    decode_job(&row)
}

/// Make a deferred queued job available after all upload files are durable.
pub async fn release_job(
    pool: &PgPool,
    institution_id: i64,
    id: Uuid,
) -> Result<BackgroundJob, DbError> {
    let row = sqlx::query(
        "UPDATE background_jobs
         SET available_at = now()
         WHERE institution_id = $1 AND id = $2 AND status = 'queued'
           AND cancel_requested = false
         RETURNING *",
    )
    .bind(institution_id)
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DbError::Conflict("任务不存在、已取消或已开始".to_owned()))?;
    decode_job(&row)
}

pub async fn get_job(
    pool: &PgPool,
    institution_id: i64,
    id: Uuid,
) -> Result<BackgroundJob, DbError> {
    let row = sqlx::query("SELECT * FROM background_jobs WHERE institution_id = $1 AND id = $2")
        .bind(institution_id)
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or(DbError::NotFound)?;
    decode_job(&row)
}

pub async fn list_jobs(
    pool: &PgPool,
    institution_id: i64,
    kind: JobKind,
    limit: i64,
) -> Result<Vec<BackgroundJob>, DbError> {
    let rows = sqlx::query(
        "SELECT * FROM background_jobs WHERE institution_id=$1 AND kind=$2
         ORDER BY created_at DESC,id LIMIT $3",
    )
    .bind(institution_id)
    .bind(kind.as_str())
    .bind(limit.clamp(1, 500))
    .fetch_all(pool)
    .await?;
    rows.iter().map(decode_job).collect()
}

/// Atomically lease the oldest runnable job of the requested kind.
pub async fn claim_job(
    pool: &PgPool,
    kind: JobKind,
    worker_id: Uuid,
    lease_for: Duration,
) -> Result<Option<BackgroundJob>, DbError> {
    if lease_for <= Duration::zero() {
        return Err(DbError::Invalid("任务租约必须大于零".to_owned()));
    }
    let row = sqlx::query(
        r#"
        WITH candidate AS (
            SELECT id
            FROM background_jobs
            WHERE kind = $1
              AND status = 'queued'
              AND cancel_requested = false
              AND available_at <= now()
              AND attempts < max_attempts
            ORDER BY available_at, created_at, id
            FOR UPDATE SKIP LOCKED
            LIMIT 1
        )
        UPDATE background_jobs j
        SET status = 'running',
            attempts = attempts + 1,
            lease_owner = $2,
            lease_expires_at = now() + ($3 * interval '1 millisecond'),
            started_at = COALESCE(started_at, now()),
            error_message = NULL
        FROM candidate
        WHERE j.id = candidate.id
        RETURNING j.*
        "#,
    )
    .bind(kind.as_str())
    .bind(worker_id)
    .bind(lease_for.num_milliseconds())
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(decode_job).transpose()
}

pub async fn heartbeat_job(
    pool: &PgPool,
    id: Uuid,
    worker_id: Uuid,
    lease_for: Duration,
) -> Result<bool, DbError> {
    if lease_for <= Duration::zero() {
        return Err(DbError::Invalid("任务租约必须大于零".to_owned()));
    }
    let result = sqlx::query(
        "UPDATE background_jobs
         SET lease_expires_at = now() + ($3 * interval '1 millisecond')
         WHERE id = $1 AND status = 'running' AND lease_owner = $2",
    )
    .bind(id)
    .bind(worker_id)
    .bind(lease_for.num_milliseconds())
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn update_job_progress(
    pool: &PgPool,
    id: Uuid,
    worker_id: Uuid,
    completed: i64,
    total: i64,
) -> Result<bool, DbError> {
    if completed < 0 || total < 0 || (total != 0 && completed > total) {
        return Err(DbError::Invalid("任务进度无效".to_owned()));
    }
    let result = sqlx::query(
        "UPDATE background_jobs
         SET progress_completed = $3, progress_total = $4
         WHERE id = $1 AND status = 'running' AND lease_owner = $2",
    )
    .bind(id)
    .bind(worker_id)
    .bind(completed)
    .bind(total)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Update both the generic processed/total counters and a kind-specific
/// progress snapshot. The latter lets callers expose richer counters (for
/// example DIMSE completed/failed/warning suboperations) without adding
/// columns to the shared queue.
pub async fn update_job_progress_with_result(
    pool: &PgPool,
    id: Uuid,
    worker_id: Uuid,
    completed: i64,
    total: i64,
    result: &Value,
) -> Result<bool, DbError> {
    if completed < 0 || total < 0 || (total != 0 && completed > total) {
        return Err(DbError::Invalid("任务进度无效".to_owned()));
    }
    let changed = sqlx::query(
        "UPDATE background_jobs
         SET progress_completed = $3, progress_total = $4, result = $5
         WHERE id = $1 AND status = 'running' AND lease_owner = $2",
    )
    .bind(id)
    .bind(worker_id)
    .bind(completed)
    .bind(total)
    .bind(result)
    .execute(pool)
    .await?;
    Ok(changed.rows_affected() == 1)
}

pub async fn complete_job(
    pool: &PgPool,
    id: Uuid,
    worker_id: Uuid,
    result: &Value,
) -> Result<bool, DbError> {
    let changed = sqlx::query(
        "UPDATE background_jobs
         SET status = CASE WHEN cancel_requested THEN 'cancelled' ELSE 'succeeded' END,
             result = $3, lease_owner = NULL, lease_expires_at = NULL,
             completed_at = now()
         WHERE id = $1 AND status = 'running' AND lease_owner = $2",
    )
    .bind(id)
    .bind(worker_id)
    .bind(result)
    .execute(pool)
    .await?;
    Ok(changed.rows_affected() == 1)
}

/// Fail the current attempt. `retry_at` requeues the job unless it has reached
/// max attempts or cancellation has been requested.
pub async fn fail_job(
    pool: &PgPool,
    id: Uuid,
    worker_id: Uuid,
    error: &str,
    retry_at: Option<DateTime<Utc>>,
) -> Result<bool, DbError> {
    let changed = sqlx::query(
        r#"
        UPDATE background_jobs
        SET status = CASE
                WHEN cancel_requested THEN 'cancelled'
                WHEN $4 IS NOT NULL AND attempts < max_attempts THEN 'queued'
                ELSE 'failed'
            END,
            available_at = COALESCE($4, available_at),
            lease_owner = NULL,
            lease_expires_at = NULL,
            error_message = $3,
            completed_at = CASE
                WHEN cancel_requested OR $4 IS NULL OR attempts >= max_attempts THEN now()
                ELSE NULL
            END
        WHERE id = $1 AND status = 'running' AND lease_owner = $2
        "#,
    )
    .bind(id)
    .bind(worker_id)
    .bind(error)
    .bind(retry_at)
    .execute(pool)
    .await?;
    Ok(changed.rows_affected() == 1)
}

/// Queued jobs cancel immediately. Running jobs receive a cooperative cancel
/// request and remain leased until the worker acknowledges it.
pub async fn request_job_cancellation(
    pool: &PgPool,
    institution_id: i64,
    id: Uuid,
) -> Result<BackgroundJob, DbError> {
    let row = sqlx::query(
        r#"
        UPDATE background_jobs
        SET cancel_requested = true,
            status = CASE WHEN status = 'queued' THEN 'cancelled' ELSE status END,
            completed_at = CASE WHEN status = 'queued' THEN now() ELSE completed_at END
        WHERE institution_id = $1 AND id = $2
          AND status IN ('queued', 'running')
        RETURNING *
        "#,
    )
    .bind(institution_id)
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or(DbError::Conflict("任务已结束或不存在，无法取消".to_owned()))?;
    decode_job(&row)
}

/// Recover expired leases. Jobs with attempts left are requeued; exhausted
/// jobs fail so they cannot remain stuck in `running` forever.
pub async fn recover_expired_jobs(pool: &PgPool) -> Result<u64, DbError> {
    let result = sqlx::query(
        r#"
        UPDATE background_jobs
        SET status = CASE
                WHEN cancel_requested THEN 'cancelled'
                WHEN attempts < max_attempts THEN 'queued'
                ELSE 'failed'
            END,
            available_at = now(),
            lease_owner = NULL,
            lease_expires_at = NULL,
            error_message = COALESCE(error_message, 'worker lease expired'),
            completed_at = CASE
                WHEN cancel_requested OR attempts >= max_attempts THEN now()
                ELSE NULL
            END
        WHERE status = 'running' AND lease_expires_at <= now()
        "#,
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

pub async fn add_job_item(
    pool: &PgPool,
    job_id: Uuid,
    item_key: &str,
    input: &Value,
) -> Result<BackgroundJobItem, DbError> {
    if item_key.trim().is_empty() {
        return Err(DbError::Invalid("任务明细键不能为空".to_owned()));
    }
    let row = sqlx::query(
        r#"
        INSERT INTO background_job_items (job_fk, item_key, input)
        VALUES ($1, $2, $3)
        ON CONFLICT (job_fk, item_key)
        DO UPDATE SET item_key = EXCLUDED.item_key
        RETURNING *
        "#,
    )
    .bind(job_id)
    .bind(item_key)
    .bind(input)
    .fetch_one(pool)
    .await?;
    decode_item(&row)
}

pub async fn start_job_item(
    pool: &PgPool,
    job_id: Uuid,
    item_key: &str,
) -> Result<BackgroundJobItem, DbError> {
    let row = sqlx::query(
        "UPDATE background_job_items
         SET status = 'running', attempts = attempts + 1,
             started_at = COALESCE(started_at, now()),
             completed_at = NULL, error_message = NULL
         WHERE job_fk = $1 AND item_key = $2 AND status IN ('pending', 'running', 'failed')
         RETURNING *",
    )
    .bind(job_id)
    .bind(item_key)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DbError::Conflict("任务明细不存在或当前状态不能开始".to_owned()))?;
    decode_item(&row)
}

pub async fn finish_job_item(
    pool: &PgPool,
    job_id: Uuid,
    item_key: &str,
    status: JobItemStatus,
    result: &Value,
    error_message: Option<&str>,
) -> Result<BackgroundJobItem, DbError> {
    if !status.is_terminal() {
        return Err(DbError::Invalid("完成任务明细必须使用终态".to_owned()));
    }
    let row = sqlx::query(
        "UPDATE background_job_items
         SET status = $3, result = $4, error_message = $5, completed_at = now()
         WHERE job_fk = $1 AND item_key = $2 AND status = 'running'
         RETURNING *",
    )
    .bind(job_id)
    .bind(item_key)
    .bind(status.as_str())
    .bind(result)
    .bind(error_message)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DbError::Conflict("任务明细不在运行状态".to_owned()))?;
    decode_item(&row)
}

pub async fn list_job_items(
    pool: &PgPool,
    institution_id: i64,
    job_id: Uuid,
) -> Result<Vec<BackgroundJobItem>, DbError> {
    let rows = sqlx::query(
        "SELECT item.* FROM background_job_items item
         JOIN background_jobs job ON job.id = item.job_fk
         WHERE job.institution_id = $1 AND job.id = $2
         ORDER BY item.id",
    )
    .bind(institution_id)
    .bind(job_id)
    .fetch_all(pool)
    .await?;
    rows.iter().map(decode_item).collect()
}

fn decode_job(row: &sqlx::postgres::PgRow) -> Result<BackgroundJob, DbError> {
    Ok(BackgroundJob {
        id: row.try_get("id")?,
        institution_id: row.try_get("institution_id")?,
        created_by: row.try_get("created_by")?,
        kind: JobKind::parse(row.try_get("kind")?)?,
        status: JobStatus::parse(row.try_get("status")?)?,
        idempotency_key: row.try_get("idempotency_key")?,
        payload: row.try_get("payload")?,
        result: row.try_get("result")?,
        progress_completed: row.try_get("progress_completed")?,
        progress_total: row.try_get("progress_total")?,
        attempts: row.try_get("attempts")?,
        max_attempts: row.try_get("max_attempts")?,
        available_at: row.try_get("available_at")?,
        lease_owner: row.try_get("lease_owner")?,
        lease_expires_at: row.try_get("lease_expires_at")?,
        cancel_requested: row.try_get("cancel_requested")?,
        error_message: row.try_get("error_message")?,
        created_at: row.try_get("created_at")?,
        started_at: row.try_get("started_at")?,
        completed_at: row.try_get("completed_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn decode_item(row: &sqlx::postgres::PgRow) -> Result<BackgroundJobItem, DbError> {
    Ok(BackgroundJobItem {
        id: row.try_get("id")?,
        job_id: row.try_get("job_fk")?,
        item_key: row.try_get("item_key")?,
        status: JobItemStatus::parse(row.try_get("status")?)?,
        attempts: row.try_get("attempts")?,
        input: row.try_get("input")?,
        result: row.try_get("result")?,
        error_message: row.try_get("error_message")?,
        created_at: row.try_get("created_at")?,
        started_at: row.try_get("started_at")?,
        completed_at: row.try_get("completed_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enum_values_match_the_migration() {
        let migration = [
            include_str!("../migrations/0009_background_jobs.sql"),
            include_str!("../migrations/0016_pause_purge_for_legal_hold.sql"),
            include_str!("../migrations/0033_retrieval_jobs.sql"),
        ]
        .concat();
        for kind in [
            JobKind::Import,
            JobKind::Export,
            JobKind::Route,
            JobKind::Lifecycle,
            JobKind::Retrieval,
        ] {
            assert!(migration.contains(&format!("'{}'", kind.as_str())));
        }
        for status in [
            "queued",
            "running",
            "paused",
            "succeeded",
            "failed",
            "cancelled",
        ] {
            assert!(migration.contains(&format!("'{status}'")));
            assert!(JobStatus::parse(status).is_ok());
        }
        for status in JobItemStatus::ALL {
            assert!(migration.contains(&format!("'{}'", status.as_str())));
            assert!(JobItemStatus::parse(status.as_str()).is_ok());
        }
    }
}
