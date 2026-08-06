use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::DbError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageTier {
    Hot,
    Cold,
    Quarantine,
}

impl StorageTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hot => "hot",
            Self::Cold => "cold",
            Self::Quarantine => "quarantine",
        }
    }

    fn parse(raw: &str) -> Result<Self, DbError> {
        match raw {
            "hot" => Ok(Self::Hot),
            "cold" => Ok(Self::Cold),
            "quarantine" => Ok(Self::Quarantine),
            _ => Err(DbError::Invalid(format!("未知存储层级 {raw:?}"))),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LifecyclePolicyInput<'a> {
    pub name: &'a str,
    pub priority: i32,
    pub enabled: bool,
    pub target_tier: StorageTier,
    pub modalities: &'a [String],
    pub study_date_before: Option<NaiveDate>,
    pub last_accessed_before: Option<DateTime<Utc>>,
    pub tag_matches: &'a Value,
    pub minimum_study_bytes: Option<i64>,
    pub minimum_storage_used_percent: Option<f64>,
    pub definition_signature: &'a [u8],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LifecyclePolicy {
    pub id: Uuid,
    pub institution_id: i64,
    pub name: String,
    pub priority: i32,
    pub enabled: bool,
    pub target_tier: StorageTier,
    pub modalities: Vec<String>,
    pub study_date_before: Option<NaiveDate>,
    pub last_accessed_before: Option<DateTime<Utc>>,
    pub tag_matches: Value,
    pub minimum_study_bytes: Option<i64>,
    pub minimum_storage_used_percent: Option<f64>,
    pub preview_current: bool,
    pub last_preview_at: Option<DateTime<Utc>>,
    pub last_preview: Value,
    pub last_run_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LifecycleStudy {
    pub study_instance_uid: String,
    pub patient_name: Option<String>,
    pub patient_id: String,
    pub study_date: Option<NaiveDate>,
    pub modalities: Vec<String>,
    pub storage_tier: StorageTier,
    pub last_accessed_at: Option<DateTime<Utc>>,
    pub storage_bytes: i64,
    pub legal_hold: bool,
}

#[derive(Debug, Clone)]
pub struct LifecycleFile {
    pub version_id: i64,
    pub storage_path: String,
    pub file_size: i64,
    pub file_sha256: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct PurgeFile {
    pub storage_kind: String,
    pub relative_path: String,
    pub file_size: i64,
    pub file_sha256: Vec<u8>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct LifecyclePathUpdate {
    pub version_id: i64,
    pub old_path: String,
    pub new_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LegalHold {
    pub id: Uuid,
    pub study_instance_uid: String,
    pub reason: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub released_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PurgeRequest {
    pub id: Uuid,
    pub study_instance_uid: String,
    pub reason: String,
    pub status: String,
    pub grace_until: Option<DateTime<Utc>>,
    pub grace_remaining_seconds: Option<i64>,
    pub job_id: Option<Uuid>,
    pub error_message: Option<String>,
    pub requested_at: DateTime<Utc>,
    pub approved_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LifecycleEvent {
    pub id: i64,
    pub study_instance_uid: String,
    pub action: String,
    pub from_tier: Option<String>,
    pub to_tier: Option<String>,
    pub job_id: Option<Uuid>,
    pub details: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LifecycleSummary {
    pub hot_studies: i64,
    pub cold_studies: i64,
    pub quarantine_studies: i64,
    pub hot_bytes: i64,
    pub cold_bytes: i64,
    pub quarantine_bytes: i64,
    pub active_legal_holds: i64,
    pub pending_purge_requests: i64,
}

pub async fn create_lifecycle_policy(
    pool: &PgPool,
    institution_id: i64,
    actor: Option<i64>,
    input: &LifecyclePolicyInput<'_>,
) -> Result<LifecyclePolicy, DbError> {
    validate_policy(input)?;
    if input.enabled {
        return Err(DbError::Conflict("新策略必须先预演才能启用".to_owned()));
    }
    let row = sqlx::query(
        r#"INSERT INTO dicom_lifecycle_policies
           (id,institution_id,name,priority,enabled,target_tier,modalities,study_date_before,
            last_accessed_before,tag_matches,minimum_study_bytes,minimum_storage_used_percent,created_by)
           VALUES ($1,$2,$3,$4,false,$5,$6,$7,$8,$9,$10,$11,$12) RETURNING *"#,
    )
    .bind(Uuid::new_v4())
    .bind(institution_id)
    .bind(input.name.trim())
    .bind(input.priority)
    .bind(input.target_tier.as_str())
    .bind(input.modalities)
    .bind(input.study_date_before)
    .bind(input.last_accessed_before)
    .bind(input.tag_matches)
    .bind(input.minimum_study_bytes)
    .bind(input.minimum_storage_used_percent)
    .bind(actor)
    .fetch_one(pool)
    .await?;
    decode_policy(&row, input.definition_signature)
}

pub async fn update_lifecycle_policy(
    pool: &PgPool,
    institution_id: i64,
    id: Uuid,
    input: &LifecyclePolicyInput<'_>,
) -> Result<LifecyclePolicy, DbError> {
    validate_policy(input)?;
    let row = sqlx::query(
        r#"UPDATE dicom_lifecycle_policies SET
           name=$3,priority=$4,enabled=$5,target_tier=$6,modalities=$7,study_date_before=$8,
           last_accessed_before=$9,tag_matches=$10,minimum_study_bytes=$11,
           minimum_storage_used_percent=$12,
           preview_signature=CASE WHEN preview_signature=$13 THEN preview_signature ELSE NULL END,
           last_preview_at=CASE WHEN preview_signature=$13 THEN last_preview_at ELSE NULL END,
           last_preview=CASE WHEN preview_signature=$13 THEN last_preview ELSE '{}'::jsonb END
           WHERE institution_id=$1 AND id=$2
             AND (NOT $5 OR preview_signature=$13)
           RETURNING *"#,
    )
    .bind(institution_id)
    .bind(id)
    .bind(input.name.trim())
    .bind(input.priority)
    .bind(input.enabled)
    .bind(input.target_tier.as_str())
    .bind(input.modalities)
    .bind(input.study_date_before)
    .bind(input.last_accessed_before)
    .bind(input.tag_matches)
    .bind(input.minimum_study_bytes)
    .bind(input.minimum_storage_used_percent)
    .bind(input.definition_signature)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DbError::Conflict("策略不存在，或当前定义尚未预演，不能启用".to_owned()))?;
    decode_policy(&row, input.definition_signature)
}

pub async fn delete_lifecycle_policy(
    pool: &PgPool,
    institution_id: i64,
    id: Uuid,
) -> Result<bool, DbError> {
    Ok(
        sqlx::query("DELETE FROM dicom_lifecycle_policies WHERE institution_id=$1 AND id=$2")
            .bind(institution_id)
            .bind(id)
            .execute(pool)
            .await?
            .rows_affected()
            == 1,
    )
}

pub async fn list_lifecycle_policies(
    pool: &PgPool,
    institution_id: i64,
) -> Result<Vec<LifecyclePolicy>, DbError> {
    let rows = sqlx::query(
        "SELECT * FROM dicom_lifecycle_policies WHERE institution_id=$1 ORDER BY priority,name",
    )
    .bind(institution_id)
    .fetch_all(pool)
    .await?;
    rows.iter().map(|row| decode_policy(row, &[])).collect()
}

pub async fn list_due_lifecycle_policies(
    pool: &PgPool,
    before: DateTime<Utc>,
) -> Result<Vec<LifecyclePolicy>, DbError> {
    let rows = sqlx::query(
        "SELECT * FROM dicom_lifecycle_policies
         WHERE enabled AND preview_signature IS NOT NULL
           AND (last_run_at IS NULL OR last_run_at<$1)
         ORDER BY institution_id,priority,id",
    )
    .bind(before)
    .fetch_all(pool)
    .await?;
    rows.iter().map(|row| decode_policy(row, &[])).collect()
}

pub async fn get_lifecycle_policy(
    pool: &PgPool,
    institution_id: i64,
    id: Uuid,
) -> Result<LifecyclePolicy, DbError> {
    let row =
        sqlx::query("SELECT * FROM dicom_lifecycle_policies WHERE institution_id=$1 AND id=$2")
            .bind(institution_id)
            .bind(id)
            .fetch_optional(pool)
            .await?
            .ok_or(DbError::NotFound)?;
    decode_policy(&row, &[])
}

pub async fn preview_lifecycle_policy(
    pool: &PgPool,
    institution_id: i64,
    policy: &LifecyclePolicy,
    storage_threshold_met: bool,
    limit: i64,
) -> Result<Vec<LifecycleStudy>, DbError> {
    if !storage_threshold_met {
        return Ok(Vec::new());
    }
    let quarantine = policy.target_tier == StorageTier::Quarantine;
    let rows = sqlx::query(
        r#"SELECT st.study_instance_uid,p.name AS patient_name,p.patient_id,
                  st.study_date,st.modalities,st.storage_tier,
                  st.last_accessed_at,COALESCE(SUM(v.file_size),0)::BIGINT AS storage_bytes,
                  EXISTS(SELECT 1 FROM dicom_legal_holds h
                         WHERE h.institution_id=st.institution_id
                           AND h.study_instance_uid=st.study_instance_uid
                           AND h.released_at IS NULL
                           AND (h.expires_at IS NULL OR h.expires_at>now())) AS legal_hold
           FROM studies st
           JOIN patients p ON p.id=st.patient_fk AND p.institution_id=st.institution_id
           JOIN series se ON se.study_fk=st.id
           JOIN instances i ON i.series_fk=se.id
           JOIN dicom_instance_versions v ON v.instance_fk=i.id
           WHERE st.institution_id=$1 AND st.storage_tier<>$2
             AND (cardinality($3::text[])=0 OR st.modalities && $3)
             AND ($4::date IS NULL OR st.study_date<$4)
             AND ($5::timestamptz IS NULL OR COALESCE(st.last_accessed_at,st.created_at)<$5)
             AND ($6::jsonb='{}'::jsonb OR
                  (COALESCE(st.attributes,'{}') || COALESCE(se.attributes,'{}') ||
                   COALESCE(i.attributes,'{}')) @> $6)
             AND (NOT $7 OR NOT EXISTS(
                  SELECT 1 FROM dicom_legal_holds h WHERE h.institution_id=st.institution_id
                    AND h.study_instance_uid=st.study_instance_uid AND h.released_at IS NULL
                    AND (h.expires_at IS NULL OR h.expires_at>now())))
           GROUP BY st.id,p.id
           HAVING ($8::bigint IS NULL OR SUM(v.file_size)>=$8)
           ORDER BY st.study_date NULLS FIRST,st.id LIMIT $9"#,
    )
    .bind(institution_id)
    .bind(policy.target_tier.as_str())
    .bind(&policy.modalities)
    .bind(policy.study_date_before)
    .bind(policy.last_accessed_before)
    .bind(&policy.tag_matches)
    .bind(quarantine)
    .bind(policy.minimum_study_bytes)
    .bind(limit.clamp(1, 10_000))
    .fetch_all(pool)
    .await?;
    rows.iter().map(decode_study).collect()
}

pub async fn record_lifecycle_preview(
    pool: &PgPool,
    institution_id: i64,
    id: Uuid,
    signature: &[u8],
    summary: &Value,
) -> Result<(), DbError> {
    let changed = sqlx::query(
        "UPDATE dicom_lifecycle_policies SET preview_signature=$3,last_preview_at=now(),last_preview=$4
         WHERE institution_id=$1 AND id=$2",
    )
    .bind(institution_id)
    .bind(id)
    .bind(signature)
    .bind(summary)
    .execute(pool)
    .await?;
    if changed.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

pub async fn mark_lifecycle_policy_run(
    pool: &PgPool,
    institution_id: i64,
    id: Uuid,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE dicom_lifecycle_policies SET last_run_at=now() WHERE institution_id=$1 AND id=$2",
    )
    .bind(institution_id)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_lifecycle_studies(
    pool: &PgPool,
    institution_id: i64,
    limit: i64,
) -> Result<Vec<LifecycleStudy>, DbError> {
    let rows = sqlx::query(
        r#"SELECT st.study_instance_uid,p.name AS patient_name,p.patient_id,
                  st.study_date,st.modalities,st.storage_tier,
                  st.last_accessed_at,COALESCE(SUM(v.file_size),0)::BIGINT AS storage_bytes,
                  EXISTS(SELECT 1 FROM dicom_legal_holds h
                         WHERE h.institution_id=st.institution_id
                           AND h.study_instance_uid=st.study_instance_uid
                           AND h.released_at IS NULL
                           AND (h.expires_at IS NULL OR h.expires_at>now())) AS legal_hold
           FROM studies st
           JOIN patients p ON p.id=st.patient_fk AND p.institution_id=st.institution_id
           LEFT JOIN series se ON se.study_fk=st.id
           LEFT JOIN instances i ON i.series_fk=se.id
           LEFT JOIN dicom_instance_versions v ON v.instance_fk=i.id
           WHERE st.institution_id=$1 GROUP BY st.id,p.id
           ORDER BY st.lifecycle_updated_at DESC,st.id LIMIT $2"#,
    )
    .bind(institution_id)
    .bind(limit.clamp(1, 1000))
    .fetch_all(pool)
    .await?;
    rows.iter().map(decode_study).collect()
}

pub async fn lifecycle_files_for_study(
    pool: &PgPool,
    institution_id: i64,
    study_uid: &str,
) -> Result<(StorageTier, Vec<LifecycleFile>), DbError> {
    let tier: String = sqlx::query_scalar(
        "SELECT storage_tier FROM studies WHERE institution_id=$1 AND study_instance_uid=$2",
    )
    .bind(institution_id)
    .bind(study_uid)
    .fetch_optional(pool)
    .await?
    .ok_or(DbError::NotFound)?;
    let rows = sqlx::query(
        r#"SELECT v.id,v.storage_path,v.file_size,v.file_sha256
           FROM dicom_instance_versions v JOIN instances i ON i.id=v.instance_fk
           JOIN series se ON se.id=i.series_fk JOIN studies st ON st.id=se.study_fk
           WHERE st.institution_id=$1 AND st.study_instance_uid=$2 ORDER BY v.id"#,
    )
    .bind(institution_id)
    .bind(study_uid)
    .fetch_all(pool)
    .await?;
    Ok((
        StorageTier::parse(&tier)?,
        rows.iter()
            .map(|row| {
                Ok(LifecycleFile {
                    version_id: row.try_get("id")?,
                    storage_path: row.try_get("storage_path")?,
                    file_size: row.try_get("file_size")?,
                    file_sha256: row.try_get("file_sha256")?,
                })
            })
            .collect::<Result<Vec<_>, DbError>>()?,
    ))
}

// These fields are kept explicit because every one participates in the same guarded transaction.
#[allow(clippy::too_many_arguments)]
pub async fn switch_study_storage_tier(
    pool: &PgPool,
    institution_id: i64,
    study_uid: &str,
    expected_tier: StorageTier,
    target_tier: StorageTier,
    updates: &[LifecyclePathUpdate],
    job_id: Uuid,
    actor: Option<i64>,
) -> Result<(), DbError> {
    if updates.is_empty() {
        return Err(DbError::Invalid("Study 没有可迁移的实例文件".to_owned()));
    }
    let mut tx = pool.begin().await?;
    let study_id = lock_study(&mut tx, institution_id, study_uid, expected_tier).await?;
    if target_tier == StorageTier::Quarantine {
        ensure_no_hold(&mut tx, institution_id, study_uid).await?;
    }
    for update in updates {
        let changed = sqlx::query(
            "UPDATE dicom_instance_versions SET storage_path=$4,storage_tier=$5
             WHERE id=$1 AND storage_path=$2 AND instance_fk IN (
               SELECT i.id FROM instances i JOIN series se ON se.id=i.series_fk WHERE se.study_fk=$3)",
        )
        .bind(update.version_id)
        .bind(&update.old_path)
        .bind(study_id)
        .bind(&update.new_path)
        .bind(target_tier.as_str())
        .execute(&mut *tx)
        .await?;
        if changed.rows_affected() != 1 {
            return Err(DbError::Conflict(
                "实例版本路径已被其他生命周期任务修改".to_owned(),
            ));
        }
    }
    sqlx::query(
        "UPDATE instances i SET storage_path=v.storage_path FROM dicom_instance_versions v
         WHERE i.current_version_id=v.id AND i.series_fk IN (SELECT id FROM series WHERE study_fk=$1)",
    )
    .bind(study_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE studies SET storage_tier=$2,lifecycle_updated_at=now() WHERE id=$1")
        .bind(study_id)
        .bind(target_tier.as_str())
        .execute(&mut *tx)
        .await?;
    append_event(
        &mut tx,
        institution_id,
        study_uid,
        tier_action(expected_tier, target_tier),
        Some(expected_tier),
        Some(target_tier),
        Some(job_id),
        actor,
        &serde_json::json!({"files": updates.len()}),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn record_study_access(
    pool: &PgPool,
    institution_id: i64,
    study_uid: &str,
) -> Result<(), DbError> {
    sqlx::query("UPDATE studies SET last_accessed_at=now() WHERE institution_id=$1 AND study_instance_uid=$2")
        .bind(institution_id).bind(study_uid).execute(pool).await?;
    Ok(())
}

pub async fn create_legal_hold(
    pool: &PgPool,
    institution_id: i64,
    study_uid: &str,
    reason: &str,
    expires_at: Option<DateTime<Utc>>,
    actor: Option<i64>,
) -> Result<LegalHold, DbError> {
    if reason.trim().is_empty() || expires_at.is_some_and(|value| value <= Utc::now()) {
        return Err(DbError::Invalid(
            "Legal Hold 原因不能为空，过期时间必须晚于当前时间".to_owned(),
        ));
    }
    let mut tx = pool.begin().await?;
    let purge = sqlx::query(
        "SELECT id,status,grace_until,job_fk FROM dicom_purge_requests
         WHERE institution_id=$1 AND study_instance_uid=$2
           AND status IN ('approved','executing') FOR UPDATE",
    )
    .bind(institution_id)
    .bind(study_uid)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(purge) = purge.as_ref() {
        let status: String = purge.try_get("status")?;
        if status == "executing" {
            return Err(DbError::Conflict(
                "Study 已进入物理清除阶段，无法再设置 Legal Hold".to_owned(),
            ));
        }
    }
    let study_id: i64 = sqlx::query_scalar(
        "SELECT id FROM studies WHERE institution_id=$1 AND study_instance_uid=$2 FOR UPDATE",
    )
    .bind(institution_id)
    .bind(study_uid)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(DbError::NotFound)?;
    let expired = sqlx::query(
        "UPDATE dicom_legal_holds SET released_at=expires_at
         WHERE institution_id=$1 AND study_instance_uid=$2 AND released_at IS NULL
           AND expires_at IS NOT NULL AND expires_at<=now()
         RETURNING id,expires_at",
    )
    .bind(institution_id)
    .bind(study_uid)
    .fetch_all(&mut *tx)
    .await?;
    for previous in expired {
        let hold_id: Uuid = previous.try_get("id")?;
        let expired_at: DateTime<Utc> = previous.try_get("expires_at")?;
        append_event(
            &mut tx,
            institution_id,
            study_uid,
            "legal_hold_released",
            None,
            None,
            None,
            None,
            &serde_json::json!({"hold_id":hold_id,"expired_at":expired_at,"automatic":true}),
        )
        .await?;
    }
    let active: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM dicom_legal_holds
         WHERE institution_id=$1 AND study_instance_uid=$2 AND released_at IS NULL)",
    )
    .bind(institution_id)
    .bind(study_uid)
    .fetch_one(&mut *tx)
    .await?;
    if active {
        return Err(DbError::Conflict("Study 已存在有效 Legal Hold".to_owned()));
    }
    let hold_id = Uuid::new_v4();
    let row = sqlx::query(
        "INSERT INTO dicom_legal_holds
         (id,institution_id,study_fk,study_instance_uid,reason,expires_at,created_by)
         VALUES ($1,$2,$3,$4,$5,$6,$7) RETURNING *",
    )
    .bind(hold_id)
    .bind(institution_id)
    .bind(study_id)
    .bind(study_uid)
    .bind(reason.trim())
    .bind(expires_at)
    .bind(actor)
    .fetch_one(&mut *tx)
    .await?;
    append_event(
        &mut tx,
        institution_id,
        study_uid,
        "legal_hold_created",
        None,
        None,
        None,
        actor,
        &serde_json::json!({"reason":reason.trim(),"expires_at":expires_at}),
    )
    .await?;
    if let Some(purge) = purge {
        let request_id: Uuid = purge.try_get("id")?;
        let job_id: Uuid = purge
            .try_get::<Option<Uuid>, _>("job_fk")?
            .ok_or_else(|| DbError::Conflict("已批准的清除申请缺少后台任务".to_owned()))?;
        let grace_until: Option<DateTime<Utc>> = purge.try_get("grace_until")?;
        let paused_at = Utc::now();
        let remaining_millis = grace_until
            .map(|value| (value - paused_at).num_milliseconds().max(0))
            .unwrap_or(0);
        let remaining_seconds = (remaining_millis + 999) / 1000;
        let request_changed = sqlx::query(
            "UPDATE dicom_purge_requests
             SET status='paused_hold',grace_until=NULL,grace_remaining_seconds=$3,
                 error_message=NULL
             WHERE institution_id=$1 AND id=$2 AND status='approved'",
        )
        .bind(institution_id)
        .bind(request_id)
        .bind(remaining_seconds)
        .execute(&mut *tx)
        .await?;
        if request_changed.rows_affected() != 1 {
            return Err(DbError::Conflict(
                "清除申请状态已变化，无法冻结宽限期".to_owned(),
            ));
        }
        let job_changed = sqlx::query(
            "UPDATE background_jobs
             SET status='paused',attempts=0,lease_owner=NULL,lease_expires_at=NULL,
                 error_message='因 Legal Hold 暂停',completed_at=NULL
             WHERE institution_id=$1 AND id=$2 AND kind='lifecycle'
               AND status IN ('queued','running','failed')",
        )
        .bind(institution_id)
        .bind(job_id)
        .execute(&mut *tx)
        .await?;
        if job_changed.rows_affected() != 1 {
            return Err(DbError::Conflict(
                "清除后台任务状态已变化，无法因 Legal Hold 暂停".to_owned(),
            ));
        }
        append_event(
            &mut tx,
            institution_id,
            study_uid,
            "purge_paused_hold",
            Some(StorageTier::Quarantine),
            None,
            Some(job_id),
            actor,
            &serde_json::json!({
                "request_id":request_id,
                "hold_id":hold_id,
                "remaining_grace_seconds":remaining_seconds
            }),
        )
        .await?;
    }
    tx.commit().await?;
    decode_hold(&row)
}

pub async fn release_legal_hold(
    pool: &PgPool,
    institution_id: i64,
    id: Uuid,
    actor: Option<i64>,
) -> Result<LegalHold, DbError> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        "UPDATE dicom_legal_holds SET released_at=now(),released_by=$3
         WHERE institution_id=$1 AND id=$2 AND released_at IS NULL RETURNING *",
    )
    .bind(institution_id)
    .bind(id)
    .bind(actor)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(DbError::Conflict("Legal Hold 不存在或已解除".to_owned()))?;
    let study_uid: String = row.try_get("study_instance_uid")?;
    append_event(
        &mut tx,
        institution_id,
        &study_uid,
        "legal_hold_released",
        None,
        None,
        None,
        actor,
        &serde_json::json!({"hold_id":id}),
    )
    .await?;
    let paused = sqlx::query(
        "SELECT id,job_fk,grace_remaining_seconds FROM dicom_purge_requests
         WHERE institution_id=$1 AND study_instance_uid=$2 AND status='paused_hold'
         FOR UPDATE",
    )
    .bind(institution_id)
    .bind(&study_uid)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(paused) = paused {
        let request_id: Uuid = paused.try_get("id")?;
        let job_id: Uuid = paused
            .try_get::<Option<Uuid>, _>("job_fk")?
            .ok_or_else(|| DbError::Conflict("暂停的清除申请缺少后台任务".to_owned()))?;
        let remaining_seconds = paused
            .try_get::<Option<i64>, _>("grace_remaining_seconds")?
            .unwrap_or(0)
            .max(0);
        let grace_until = Utc::now() + chrono::Duration::seconds(remaining_seconds);
        let request_changed = sqlx::query(
            "UPDATE dicom_purge_requests
             SET status='approved',grace_until=$3,grace_remaining_seconds=NULL,
                 error_message=NULL
             WHERE institution_id=$1 AND id=$2 AND status='paused_hold'",
        )
        .bind(institution_id)
        .bind(request_id)
        .bind(grace_until)
        .execute(&mut *tx)
        .await?;
        if request_changed.rows_affected() != 1 {
            return Err(DbError::Conflict(
                "暂停的清除申请状态已变化，无法恢复宽限期".to_owned(),
            ));
        }
        let job_changed = sqlx::query(
            "UPDATE background_jobs
             SET status='queued',attempts=0,available_at=$3,lease_owner=NULL,
                 lease_expires_at=NULL,error_message=NULL,completed_at=NULL
             WHERE institution_id=$1 AND id=$2 AND kind='lifecycle'
               AND status IN ('paused','failed')",
        )
        .bind(institution_id)
        .bind(job_id)
        .bind(grace_until)
        .execute(&mut *tx)
        .await?;
        if job_changed.rows_affected() != 1 {
            return Err(DbError::Conflict(
                "暂停的清除后台任务状态已变化，无法恢复".to_owned(),
            ));
        }
        append_event(
            &mut tx,
            institution_id,
            &study_uid,
            "purge_resumed_hold",
            Some(StorageTier::Quarantine),
            None,
            Some(job_id),
            actor,
            &serde_json::json!({
                "request_id":request_id,
                "hold_id":id,
                "remaining_grace_seconds":remaining_seconds,
                "grace_until":grace_until
            }),
        )
        .await?;
    }
    tx.commit().await?;
    decode_hold(&row)
}

pub async fn list_legal_holds(
    pool: &PgPool,
    institution_id: i64,
) -> Result<Vec<LegalHold>, DbError> {
    let rows = sqlx::query(
        "SELECT * FROM dicom_legal_holds WHERE institution_id=$1 ORDER BY created_at DESC",
    )
    .bind(institution_id)
    .fetch_all(pool)
    .await?;
    rows.iter().map(decode_hold).collect()
}

pub async fn create_purge_request(
    pool: &PgPool,
    institution_id: i64,
    study_uid: &str,
    reason: &str,
    actor: Option<i64>,
) -> Result<PurgeRequest, DbError> {
    if reason.trim().is_empty() {
        return Err(DbError::Invalid("清除原因不能为空".to_owned()));
    }
    let mut tx = pool.begin().await?;
    let study_id = lock_study(&mut tx, institution_id, study_uid, StorageTier::Quarantine).await?;
    ensure_no_hold(&mut tx, institution_id, study_uid).await?;
    let open: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM dicom_purge_requests
         WHERE institution_id=$1 AND study_instance_uid=$2
           AND status IN ('pending','approved','paused_hold','executing'))",
    )
    .bind(institution_id)
    .bind(study_uid)
    .fetch_one(&mut *tx)
    .await?;
    if open {
        return Err(DbError::Conflict("Study 已存在待处理的清除申请".to_owned()));
    }
    let row = sqlx::query(
        "INSERT INTO dicom_purge_requests
         (id,institution_id,study_fk,study_instance_uid,reason,requested_by)
         VALUES ($1,$2,$3,$4,$5,$6) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(institution_id)
    .bind(study_id)
    .bind(study_uid)
    .bind(reason.trim())
    .bind(actor)
    .fetch_one(&mut *tx)
    .await?;
    append_event(
        &mut tx,
        institution_id,
        study_uid,
        "purge_requested",
        Some(StorageTier::Quarantine),
        None,
        None,
        actor,
        &serde_json::json!({"reason":reason.trim()}),
    )
    .await?;
    tx.commit().await?;
    decode_purge(&row)
}

pub async fn approve_purge_request(
    pool: &PgPool,
    institution_id: i64,
    id: Uuid,
    grace_until: DateTime<Utc>,
    actor: Option<i64>,
) -> Result<PurgeRequest, DbError> {
    // A zero-hour grace period is valid for tests and urgent administrative purges.
    // Allow small request/transaction clock drift around "now".
    if grace_until < Utc::now() - chrono::Duration::minutes(1) {
        return Err(DbError::Invalid(
            "宽限期结束时间不能早于当前时间".to_owned(),
        ));
    }
    let mut tx = pool.begin().await?;
    let request = sqlx::query(
        "SELECT * FROM dicom_purge_requests WHERE institution_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(institution_id)
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(DbError::NotFound)?;
    let status: String = request.try_get("status")?;
    if status != "pending" {
        return Err(DbError::Conflict("只有待审批的清除申请可以批准".to_owned()));
    }
    let study_uid: String = request.try_get("study_instance_uid")?;
    lock_study(&mut tx, institution_id, &study_uid, StorageTier::Quarantine).await?;
    ensure_no_hold(&mut tx, institution_id, &study_uid).await?;
    let job_id = Uuid::new_v4();
    let payload =
        serde_json::json!({"operation":"purge","request_id":id,"study_instance_uid":study_uid});
    sqlx::query(
        "INSERT INTO background_jobs
         (id,institution_id,created_by,kind,idempotency_key,payload,progress_total,max_attempts,available_at)
         VALUES ($1,$2,$3,'lifecycle',$4,$5,1,3,$6)",
    )
    .bind(job_id)
    .bind(institution_id)
    .bind(actor)
    .bind(format!("purge:{id}"))
    .bind(payload)
    .bind(grace_until)
    .execute(&mut *tx)
    .await?;
    let row = sqlx::query(
        "UPDATE dicom_purge_requests SET status='approved',grace_until=$3,approved_by=$4,
         approved_at=now(),job_fk=$5 WHERE institution_id=$1 AND id=$2 RETURNING *",
    )
    .bind(institution_id)
    .bind(id)
    .bind(grace_until)
    .bind(actor)
    .bind(job_id)
    .fetch_one(&mut *tx)
    .await?;
    append_event(
        &mut tx,
        institution_id,
        &study_uid,
        "purge_approved",
        Some(StorageTier::Quarantine),
        None,
        Some(job_id),
        actor,
        &serde_json::json!({"grace_until":grace_until}),
    )
    .await?;
    tx.commit().await?;
    decode_purge(&row)
}

pub async fn reject_purge_request(
    pool: &PgPool,
    institution_id: i64,
    id: Uuid,
    actor: Option<i64>,
) -> Result<PurgeRequest, DbError> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        "UPDATE dicom_purge_requests SET status='rejected',completed_at=now()
         WHERE institution_id=$1 AND id=$2 AND status='pending' RETURNING *",
    )
    .bind(institution_id)
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| DbError::Conflict("只有待审批的清除申请可以拒绝".to_owned()))?;
    let study_uid: String = row.try_get("study_instance_uid")?;
    append_event(
        &mut tx,
        institution_id,
        &study_uid,
        "purge_rejected",
        Some(StorageTier::Quarantine),
        None,
        None,
        actor,
        &serde_json::json!({"request_id":id}),
    )
    .await?;
    tx.commit().await?;
    decode_purge(&row)
}

pub async fn list_purge_requests(
    pool: &PgPool,
    institution_id: i64,
) -> Result<Vec<PurgeRequest>, DbError> {
    let rows = sqlx::query(
        "SELECT * FROM dicom_purge_requests WHERE institution_id=$1 ORDER BY requested_at DESC",
    )
    .bind(institution_id)
    .fetch_all(pool)
    .await?;
    rows.iter().map(decode_purge).collect()
}

pub async fn begin_purge(
    pool: &PgPool,
    institution_id: i64,
    request_id: Uuid,
) -> Result<String, DbError> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        "SELECT r.study_instance_uid,r.status,r.grace_until,st.storage_tier
         FROM dicom_purge_requests r LEFT JOIN studies st ON st.id=r.study_fk
         WHERE r.institution_id=$1 AND r.id=$2 FOR UPDATE OF r",
    )
    .bind(institution_id)
    .bind(request_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(DbError::NotFound)?;
    let study_uid: String = row.try_get("study_instance_uid")?;
    let status: String = row.try_get("status")?;
    if status == "executing" {
        tx.commit().await?;
        return Ok(study_uid);
    }
    let grace: Option<DateTime<Utc>> = row.try_get("grace_until")?;
    let tier: Option<String> = row.try_get("storage_tier")?;
    if status != "approved"
        || grace.is_none_or(|value| value > Utc::now())
        || tier.as_deref() != Some("quarantine")
    {
        return Err(DbError::Conflict(
            "清除申请尚未批准、仍在宽限期或 Study 不在隔离区".to_owned(),
        ));
    }
    ensure_no_hold(&mut tx, institution_id, &study_uid).await?;
    sqlx::query("UPDATE dicom_purge_requests SET status='executing' WHERE id=$1")
        .bind(request_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(study_uid)
}

/// Persist every deletion target before removing the Study metadata. Retries use this manifest.
pub async fn commit_purge_metadata(
    pool: &PgPool,
    institution_id: i64,
    request_id: Uuid,
) -> Result<Vec<PurgeFile>, DbError> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        "SELECT r.study_instance_uid,r.status,st.id AS study_id
         FROM dicom_purge_requests r LEFT JOIN studies st ON st.id=r.study_fk
         WHERE r.institution_id=$1 AND r.id=$2 FOR UPDATE OF r",
    )
    .bind(institution_id)
    .bind(request_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(DbError::NotFound)?;
    let study_uid: String = row.try_get("study_instance_uid")?;
    let status: String = row.try_get("status")?;
    if status != "executing" {
        return Err(DbError::Conflict("清除申请不在执行状态".to_owned()));
    }
    let study_id: Option<i64> = row.try_get("study_id")?;
    if let Some(study_id) = study_id {
        let locked_study =
            lock_study(&mut tx, institution_id, &study_uid, StorageTier::Quarantine).await?;
        if locked_study != study_id {
            return Err(DbError::Conflict(
                "清除申请对应的 Study 已经变化".to_owned(),
            ));
        }
        ensure_no_hold(&mut tx, institution_id, &study_uid).await?;
        sqlx::query(
            r#"INSERT INTO dicom_purge_files
               (request_fk,storage_kind,relative_path,file_size,file_sha256)
               SELECT $1,'dicom',v.storage_path,v.file_size,v.file_sha256
               FROM dicom_instance_versions v JOIN instances i ON i.id=v.instance_fk
               JOIN series se ON se.id=i.series_fk WHERE se.study_fk=$2
               ON CONFLICT DO NOTHING"#,
        )
        .bind(request_id)
        .bind(study_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"INSERT INTO dicom_purge_files
               (request_fk,storage_kind,relative_path,file_size,file_sha256)
               SELECT $1,'export',a.relative_path,a.file_size,a.file_sha256
               FROM export_artifacts a JOIN background_jobs j ON j.id=a.job_fk
               WHERE j.institution_id=$2 AND j.kind='export' AND j.payload->>'study_instance_uid'=$3
               ON CONFLICT DO NOTHING"#,
        )
        .bind(request_id)
        .bind(institution_id)
        .bind(&study_uid)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "DELETE FROM background_jobs WHERE institution_id=$1 AND kind='export'
             AND payload->>'study_instance_uid'=$2",
        )
        .bind(institution_id)
        .bind(&study_uid)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM studies WHERE id=$1")
            .bind(study_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE dicom_purge_requests SET study_fk=NULL WHERE id=$1")
            .bind(request_id)
            .execute(&mut *tx)
            .await?;
    }
    let files = load_purge_files(&mut tx, request_id).await?;
    tx.commit().await?;
    Ok(files)
}

pub async fn mark_purge_file_deleted(
    pool: &PgPool,
    request_id: Uuid,
    storage_kind: &str,
    relative_path: &str,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE dicom_purge_files SET deleted_at=now()
         WHERE request_fk=$1 AND storage_kind=$2 AND relative_path=$3",
    )
    .bind(request_id)
    .bind(storage_kind)
    .bind(relative_path)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn finalize_purge(
    pool: &PgPool,
    institution_id: i64,
    request_id: Uuid,
    job_id: Uuid,
    actor: Option<i64>,
) -> Result<(), DbError> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        "SELECT study_instance_uid,status FROM dicom_purge_requests
         WHERE institution_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(institution_id)
    .bind(request_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(DbError::NotFound)?;
    let study_uid: String = row.try_get("study_instance_uid")?;
    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM dicom_purge_files WHERE request_fk=$1 AND deleted_at IS NULL",
    )
    .bind(request_id)
    .fetch_one(&mut *tx)
    .await?;
    if pending != 0 {
        return Err(DbError::Conflict(format!(
            "仍有 {pending} 个文件未物理清除"
        )));
    }
    sqlx::query(
        "UPDATE dicom_purge_requests SET status='completed',completed_at=now(),error_message=NULL
         WHERE id=$1 AND status='executing'",
    )
    .bind(request_id)
    .execute(&mut *tx)
    .await?;
    append_event(
        &mut tx,
        institution_id,
        &study_uid,
        "purged",
        Some(StorageTier::Quarantine),
        None,
        Some(job_id),
        actor,
        &serde_json::json!({"request_id":request_id}),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn record_purge_error(
    pool: &PgPool,
    institution_id: i64,
    request_id: Uuid,
    error: &str,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE dicom_purge_requests SET error_message=$3
         WHERE institution_id=$1 AND id=$2 AND status='executing'",
    )
    .bind(institution_id)
    .bind(request_id)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

async fn load_purge_files(
    tx: &mut Transaction<'_, Postgres>,
    request_id: Uuid,
) -> Result<Vec<PurgeFile>, DbError> {
    let rows = sqlx::query(
        "SELECT storage_kind,relative_path,file_size,file_sha256,deleted_at
         FROM dicom_purge_files WHERE request_fk=$1 ORDER BY storage_kind,relative_path",
    )
    .bind(request_id)
    .fetch_all(&mut **tx)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(PurgeFile {
                storage_kind: row.try_get("storage_kind")?,
                relative_path: row.try_get("relative_path")?,
                file_size: row.try_get("file_size")?,
                file_sha256: row.try_get("file_sha256")?,
                deleted_at: row.try_get("deleted_at")?,
            })
        })
        .collect()
}

pub async fn list_lifecycle_events(
    pool: &PgPool,
    institution_id: i64,
    limit: i64,
) -> Result<Vec<LifecycleEvent>, DbError> {
    let rows = sqlx::query(
        "SELECT * FROM dicom_lifecycle_events WHERE institution_id=$1 ORDER BY created_at DESC,id DESC LIMIT $2",
    ).bind(institution_id).bind(limit.clamp(1,1000)).fetch_all(pool).await?;
    rows.iter()
        .map(|row| {
            Ok(LifecycleEvent {
                id: row.try_get("id")?,
                study_instance_uid: row.try_get("study_instance_uid")?,
                action: row.try_get("action")?,
                from_tier: row.try_get("from_tier")?,
                to_tier: row.try_get("to_tier")?,
                job_id: row.try_get("job_fk")?,
                details: row.try_get("details")?,
                created_at: row.try_get("created_at")?,
            })
        })
        .collect()
}

pub async fn lifecycle_summary(
    pool: &PgPool,
    institution_id: i64,
) -> Result<LifecycleSummary, DbError> {
    let row = sqlx::query(
        r#"WITH study_sizes AS (
             SELECT st.id,st.storage_tier,COALESCE(SUM(v.file_size),0)::BIGINT bytes
             FROM studies st LEFT JOIN series se ON se.study_fk=st.id
             LEFT JOIN instances i ON i.series_fk=se.id
             LEFT JOIN dicom_instance_versions v ON v.instance_fk=i.id
             WHERE st.institution_id=$1 GROUP BY st.id
           ) SELECT
             COUNT(*) FILTER (WHERE storage_tier='hot')::BIGINT hot_studies,
             COUNT(*) FILTER (WHERE storage_tier='cold')::BIGINT cold_studies,
             COUNT(*) FILTER (WHERE storage_tier='quarantine')::BIGINT quarantine_studies,
             COALESCE(SUM(bytes) FILTER (WHERE storage_tier='hot'),0)::BIGINT hot_bytes,
             COALESCE(SUM(bytes) FILTER (WHERE storage_tier='cold'),0)::BIGINT cold_bytes,
             COALESCE(SUM(bytes) FILTER (WHERE storage_tier='quarantine'),0)::BIGINT quarantine_bytes,
             (SELECT COUNT(*) FROM dicom_legal_holds WHERE institution_id=$1 AND released_at IS NULL
               AND (expires_at IS NULL OR expires_at>now()))::BIGINT active_legal_holds,
             (SELECT COUNT(*) FROM dicom_purge_requests WHERE institution_id=$1
               AND status IN ('pending','approved','paused_hold','executing'))::BIGINT pending_purge_requests
           FROM study_sizes"#,
    ).bind(institution_id).fetch_one(pool).await?;
    Ok(LifecycleSummary {
        hot_studies: row.try_get("hot_studies")?,
        cold_studies: row.try_get("cold_studies")?,
        quarantine_studies: row.try_get("quarantine_studies")?,
        hot_bytes: row.try_get("hot_bytes")?,
        cold_bytes: row.try_get("cold_bytes")?,
        quarantine_bytes: row.try_get("quarantine_bytes")?,
        active_legal_holds: row.try_get("active_legal_holds")?,
        pending_purge_requests: row.try_get("pending_purge_requests")?,
    })
}

fn validate_policy(input: &LifecyclePolicyInput<'_>) -> Result<(), DbError> {
    if input.name.trim().is_empty() || !input.tag_matches.is_object() {
        return Err(DbError::Invalid(
            "策略名称不能为空，Tag 条件必须是 JSON 对象".to_owned(),
        ));
    }
    if input.target_tier == StorageTier::Hot {
        return Err(DbError::Invalid(
            "自动策略目标只能是冷层或隔离区".to_owned(),
        ));
    }
    if input.minimum_study_bytes.is_some_and(|value| value < 0)
        || input
            .minimum_storage_used_percent
            .is_some_and(|value| !(0.0..=100.0).contains(&value))
    {
        return Err(DbError::Invalid("存储占用条件超出有效范围".to_owned()));
    }
    Ok(())
}

fn decode_policy(
    row: &sqlx::postgres::PgRow,
    signature: &[u8],
) -> Result<LifecyclePolicy, DbError> {
    let preview_signature: Option<Vec<u8>> = row.try_get("preview_signature")?;
    Ok(LifecyclePolicy {
        id: row.try_get("id")?,
        institution_id: row.try_get("institution_id")?,
        name: row.try_get("name")?,
        priority: row.try_get("priority")?,
        enabled: row.try_get("enabled")?,
        target_tier: StorageTier::parse(row.try_get("target_tier")?)?,
        modalities: row.try_get("modalities")?,
        study_date_before: row.try_get("study_date_before")?,
        last_accessed_before: row.try_get("last_accessed_before")?,
        tag_matches: row.try_get("tag_matches")?,
        minimum_study_bytes: row.try_get("minimum_study_bytes")?,
        minimum_storage_used_percent: row.try_get("minimum_storage_used_percent")?,
        preview_current: preview_signature.is_some()
            && (signature.is_empty() || preview_signature.as_deref() == Some(signature)),
        last_preview_at: row.try_get("last_preview_at")?,
        last_preview: row.try_get("last_preview")?,
        last_run_at: row.try_get("last_run_at")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn decode_study(row: &sqlx::postgres::PgRow) -> Result<LifecycleStudy, DbError> {
    Ok(LifecycleStudy {
        study_instance_uid: row.try_get("study_instance_uid")?,
        patient_name: row.try_get("patient_name")?,
        patient_id: row.try_get("patient_id")?,
        study_date: row.try_get("study_date")?,
        modalities: row.try_get("modalities")?,
        storage_tier: StorageTier::parse(row.try_get("storage_tier")?)?,
        last_accessed_at: row.try_get("last_accessed_at")?,
        storage_bytes: row.try_get("storage_bytes")?,
        legal_hold: row.try_get("legal_hold")?,
    })
}

fn decode_hold(row: &sqlx::postgres::PgRow) -> Result<LegalHold, DbError> {
    Ok(LegalHold {
        id: row.try_get("id")?,
        study_instance_uid: row.try_get("study_instance_uid")?,
        reason: row.try_get("reason")?,
        expires_at: row.try_get("expires_at")?,
        created_at: row.try_get("created_at")?,
        released_at: row.try_get("released_at")?,
    })
}

fn decode_purge(row: &sqlx::postgres::PgRow) -> Result<PurgeRequest, DbError> {
    Ok(PurgeRequest {
        id: row.try_get("id")?,
        study_instance_uid: row.try_get("study_instance_uid")?,
        reason: row.try_get("reason")?,
        status: row.try_get("status")?,
        grace_until: row.try_get("grace_until")?,
        grace_remaining_seconds: row.try_get("grace_remaining_seconds")?,
        job_id: row.try_get("job_fk")?,
        error_message: row.try_get("error_message")?,
        requested_at: row.try_get("requested_at")?,
        approved_at: row.try_get("approved_at")?,
        completed_at: row.try_get("completed_at")?,
    })
}

async fn lock_study(
    tx: &mut Transaction<'_, Postgres>,
    institution_id: i64,
    study_uid: &str,
    expected: StorageTier,
) -> Result<i64, DbError> {
    sqlx::query_scalar(
        "SELECT id FROM studies WHERE institution_id=$1 AND study_instance_uid=$2 AND storage_tier=$3 FOR UPDATE",
    ).bind(institution_id).bind(study_uid).bind(expected.as_str()).fetch_optional(&mut **tx).await?
        .ok_or_else(|| DbError::Conflict("Study 不存在或存储层级已经变化".to_owned()))
}

async fn ensure_no_hold(
    tx: &mut Transaction<'_, Postgres>,
    institution_id: i64,
    study_uid: &str,
) -> Result<(), DbError> {
    let held: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM dicom_legal_holds WHERE institution_id=$1
         AND study_instance_uid=$2 AND released_at IS NULL AND (expires_at IS NULL OR expires_at>now()))",
    ).bind(institution_id).bind(study_uid).fetch_one(&mut **tx).await?;
    if held {
        Err(DbError::Conflict(
            "Study 存在有效 Legal Hold，禁止隔离或清除".to_owned(),
        ))
    } else {
        Ok(())
    }
}

fn tier_action(from: StorageTier, to: StorageTier) -> &'static str {
    match (from, to) {
        (_, StorageTier::Hot) => "restore_to_hot",
        (_, StorageTier::Cold) => "move_to_cold",
        (_, StorageTier::Quarantine) => "quarantine",
    }
}

#[allow(clippy::too_many_arguments)]
async fn append_event(
    tx: &mut Transaction<'_, Postgres>,
    institution_id: i64,
    study_uid: &str,
    action: &str,
    from: Option<StorageTier>,
    to: Option<StorageTier>,
    job: Option<Uuid>,
    actor: Option<i64>,
    details: &Value,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO dicom_lifecycle_events
         (institution_id,study_instance_uid,action,from_tier,to_tier,job_fk,actor_fk,details)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(institution_id)
    .bind(study_uid)
    .bind(action)
    .bind(from.map(StorageTier::as_str))
    .bind(to.map(StorageTier::as_str))
    .bind(job)
    .bind(actor)
    .bind(details)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
