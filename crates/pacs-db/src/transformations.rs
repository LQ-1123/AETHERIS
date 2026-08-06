//! Persistence and transactional activation for DICOM transformations.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use pacs_core::InstanceMetadata;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::DbError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransformMode {
    ClinicalCorrection,
    Rollback,
}

impl TransformMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClinicalCorrection => "clinical_correction",
            Self::Rollback => "rollback",
        }
    }

    fn parse(value: &str) -> Result<Self, DbError> {
        match value {
            "clinical_correction" => Ok(Self::ClinicalCorrection),
            "rollback" => Ok(Self::Rollback),
            _ => Err(DbError::Invalid(format!("未知转换模式 {value}"))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetType {
    Patient,
    Study,
    Series,
    Instance,
}

impl TargetType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Patient => "patient",
            Self::Study => "study",
            Self::Series => "series",
            Self::Instance => "instance",
        }
    }

    fn parse(value: &str) -> Result<Self, DbError> {
        match value {
            "patient" => Ok(Self::Patient),
            "study" => Ok(Self::Study),
            "series" => Ok(Self::Series),
            "instance" => Ok(Self::Instance),
            _ => Err(DbError::Invalid(format!("未知目标层级 {value}"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransformTarget {
    pub target_type: TargetType,
    /// Patient uses its database primary key; Study/Series use UID; Instance uses logical UUID.
    pub key: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct TransformSource {
    pub patient_pk: i64,
    pub patient_id: String,
    pub study_pk: i64,
    pub series_pk: i64,
    pub instance_pk: i64,
    pub logical_instance_id: Uuid,
    pub current_version_id: i64,
    pub version_number: i32,
    pub storage_path: String,
    pub file_sha256: Vec<u8>,
    pub study_instance_uid: String,
    pub series_instance_uid: String,
    pub sop_instance_uid: String,
    pub sop_class_uid: Option<String>,
    pub transfer_syntax_uid: String,
    pub apply_rules: bool,
    pub storage_tier: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct VersionSource {
    pub id: i64,
    pub logical_instance_id: Uuid,
    pub instance_pk: i64,
    pub version_number: i32,
    pub storage_path: String,
    pub file_sha256: Vec<u8>,
    pub study_instance_uid: String,
    pub series_instance_uid: String,
    pub sop_instance_uid: String,
    pub transfer_syntax_uid: String,
    pub is_current: bool,
}

#[derive(Debug, Clone, FromRow)]
pub struct UidAlias {
    pub study_pk: i64,
    pub series_pk: i64,
    pub logical_instance_id: Uuid,
    pub study_instance_uid: String,
    pub series_instance_uid: String,
    pub sop_instance_uid: String,
}

const SOURCE_SELECT: &str = r#"
    SELECT p.id AS patient_pk, p.patient_id,
           st.id AS study_pk, se.id AS series_pk, i.id AS instance_pk,
           i.logical_instance_id,
           v.id AS current_version_id, v.version_number, v.storage_path, v.file_sha256,
           v.study_instance_uid, v.series_instance_uid, v.sop_instance_uid,
           i.sop_class_uid, v.transfer_syntax_uid, st.storage_tier,
           CASE
             WHEN $3 = 'patient' THEN p.id::text = $2
             WHEN $3 = 'study' THEN st.study_instance_uid = $2
             WHEN $3 = 'series' THEN se.series_instance_uid = $2
             WHEN $3 = 'instance' THEN i.logical_instance_id::text = $2
             ELSE false
           END AS apply_rules
    FROM instances i
    JOIN dicom_instance_versions v ON v.id = i.current_version_id
    JOIN series se ON i.series_fk = se.id
    JOIN studies st ON se.study_fk = st.id
    JOIN patients p ON st.patient_fk = p.id
"#;

/// Select all files required for a consistent UID graph.
///
/// A Series or Instance edit expands to its whole Study because StudyInstanceUID is remapped for
/// every derived revision. `apply_rules` remains true only for the requested leaf target.
pub async fn select_transform_sources(
    pool: &PgPool,
    institution_id: i64,
    target: &TransformTarget,
) -> Result<Vec<TransformSource>, DbError> {
    let suffix = match target.target_type {
        TargetType::Patient => {
            " WHERE p.institution_id = $1 AND st.institution_id = $1 AND p.id::text = $2"
        }
        TargetType::Study => {
            " WHERE p.institution_id = $1 AND st.institution_id = $1 AND st.study_instance_uid = $2"
        }
        TargetType::Series => {
            r#"
            WHERE p.institution_id = $1 AND st.institution_id = $1
              AND st.id = (
                SELECT target_st.id FROM series target_se
                JOIN studies target_st ON target_se.study_fk = target_st.id
                JOIN patients target_p ON target_st.patient_fk = target_p.id
                WHERE target_se.series_instance_uid = $2
                  AND target_st.institution_id = $1 AND target_p.institution_id = $1
              )"#
        }
        TargetType::Instance => {
            r#"
            WHERE p.institution_id = $1 AND st.institution_id = $1
              AND st.id = (
                SELECT target_st.id FROM instances target_i
                JOIN series target_se ON target_i.series_fk = target_se.id
                JOIN studies target_st ON target_se.study_fk = target_st.id
                JOIN patients target_p ON target_st.patient_fk = target_p.id
                WHERE target_i.logical_instance_id::text = $2
                  AND target_st.institution_id = $1 AND target_p.institution_id = $1
              )"#
        }
    };
    let sql = format!("{SOURCE_SELECT}{suffix} ORDER BY st.id, se.id, i.id");
    // Both fragments are constants selected by `TargetType`; no request text is interpolated.
    let sources = sqlx::query_as::<_, TransformSource>(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(institution_id)
        .bind(&target.key)
        .bind(target.target_type.as_str())
        .fetch_all(pool)
        .await?;
    if sources.is_empty() {
        return Err(DbError::NotFound);
    }
    Ok(sources)
}

/// Return every historical UID belonging to the selected logical instances. This lets a new
/// revision repair references which still point at an older revision of the same Study graph.
pub async fn list_uid_aliases(
    pool: &PgPool,
    institution_id: i64,
    instance_ids: &[i64],
) -> Result<Vec<UidAlias>, DbError> {
    if instance_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as::<_, UidAlias>(
        r#"
        SELECT st.id AS study_pk, se.id AS series_pk, v.logical_instance_id,
               v.study_instance_uid, v.series_instance_uid, v.sop_instance_uid
        FROM dicom_instance_versions v
        JOIN instances i ON i.id = v.instance_fk
        JOIN series se ON se.id = i.series_fk
        JOIN studies st ON st.id = se.study_fk
        JOIN patients p ON p.id = st.patient_fk
        WHERE v.instance_fk = ANY($1)
          AND p.institution_id = $2 AND st.institution_id = $2
        "#,
    )
    .bind(instance_ids)
    .bind(institution_id)
    .fetch_all(pool)
    .await?)
}

pub async fn get_version_source(
    pool: &PgPool,
    institution_id: i64,
    logical_instance_id: Uuid,
    version_id: i64,
) -> Result<VersionSource, DbError> {
    sqlx::query_as::<_, VersionSource>(
        r#"
        SELECT v.id, v.logical_instance_id, v.instance_fk AS instance_pk,
               v.version_number, v.storage_path, v.file_sha256,
               v.study_instance_uid, v.series_instance_uid, v.sop_instance_uid,
               v.transfer_syntax_uid, (i.current_version_id = v.id) AS is_current
        FROM dicom_instance_versions v
        JOIN instances i ON i.id = v.instance_fk
        JOIN series se ON se.id = i.series_fk
        JOIN studies st ON st.id = se.study_fk
        JOIN patients p ON p.id = st.patient_fk
        WHERE v.id = $1 AND v.logical_instance_id = $2
          AND p.institution_id = $3 AND st.institution_id = $3
        "#,
    )
    .bind(version_id)
    .bind(logical_instance_id)
    .bind(institution_id)
    .fetch_optional(pool)
    .await?
    .ok_or(DbError::NotFound)
}

pub struct NewPreviewJob<'a> {
    pub id: Uuid,
    pub institution_id: i64,
    pub user_id: i64,
    pub username: &'a str,
    pub mode: TransformMode,
    pub target: &'a TransformTarget,
    pub rules: &'a Value,
    pub reason: &'a str,
    pub confirmation_hash: &'a [u8],
    pub confirmation_expires_at: DateTime<Utc>,
    pub preview: &'a Value,
    pub pixel_risk: &'a str,
}

pub async fn create_preview_job(
    pool: &PgPool,
    job: NewPreviewJob<'_>,
    sources: &[TransformSource],
) -> Result<(), DbError> {
    let mut tx = pool.begin().await?;
    let base_revisions = Value::Object(
        sources
            .iter()
            .map(|source| {
                (
                    source.logical_instance_id.to_string(),
                    Value::from(source.current_version_id),
                )
            })
            .collect(),
    );
    sqlx::query(
        r#"
        INSERT INTO dicom_transform_jobs (
            id, institution_id, created_by, username, mode, target_type, target_key,
            base_revisions, rules, reason, status,
            progress_total, confirmation_hash, confirmation_expires_at, preview, pixel_risk
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
            'previewed', $11, $12, $13, $14, $15
        )
        "#,
    )
    .bind(job.id)
    .bind(job.institution_id)
    .bind(job.user_id)
    .bind(job.username)
    .bind(job.mode.as_str())
    .bind(job.target.target_type.as_str())
    .bind(&job.target.key)
    .bind(base_revisions)
    .bind(job.rules)
    .bind(job.reason)
    .bind(i32::try_from(sources.len()).unwrap_or(i32::MAX))
    .bind(job.confirmation_hash)
    .bind(job.confirmation_expires_at)
    .bind(job.preview)
    .bind(job.pixel_risk)
    .execute(&mut *tx)
    .await?;

    for source in sources {
        sqlx::query(
            r#"
            INSERT INTO dicom_transform_items (
                job_fk, logical_instance_id, instance_fk, source_version_fk,
                source_path, status
            ) VALUES ($1, $2, $3, $4, $5, 'pending')
            "#,
        )
        .bind(job.id)
        .bind(source.logical_instance_id)
        .bind(source.instance_pk)
        .bind(source.current_version_id)
        .bind(&source.storage_path)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "INSERT INTO audit_log (user_fk, username, action, outcome, detail)
         VALUES ($1, $2, 'dicom_transform_preview', 'success', $3)",
    )
    .bind(job.user_id)
    .bind(job.username)
    .bind(serde_json::json!({
        "job_id": job.id,
        "mode": job.mode.as_str(),
        "target_type": job.target.target_type.as_str(),
        "target_key": job.target.key,
        "instances": sources.len()
    }))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Confirm a preview and atomically reject stale base revisions.
pub async fn queue_preview_job(
    pool: &PgPool,
    institution_id: i64,
    user_id: i64,
    job_id: Uuid,
    confirmation_hash: &[u8],
) -> Result<(), DbError> {
    let mut tx = pool.begin().await?;
    let result = sqlx::query(
        r#"
        UPDATE dicom_transform_jobs j
        SET status = 'queued', confirmation_hash = NULL, confirmation_expires_at = NULL
        WHERE j.id = $1
          AND j.institution_id = $2
          AND j.created_by = $3
          AND j.status = 'previewed'
          AND j.confirmation_hash = $4
          AND j.confirmation_expires_at > now()
          AND NOT EXISTS (
              SELECT 1
              FROM dicom_transform_items item
              JOIN instances i ON i.id = item.instance_fk
              WHERE item.job_fk = j.id AND i.current_version_id <> item.source_version_fk
          )
        "#,
    )
    .bind(job_id)
    .bind(institution_id)
    .bind(user_id)
    .bind(confirmation_hash)
    .execute(&mut *tx)
    .await?;
    if result.rows_affected() != 1 {
        return Err(DbError::Conflict(
            "预览已过期、确认 token 无效或基础修订已变化，请重新预览".to_owned(),
        ));
    }
    sqlx::query(
        "INSERT INTO audit_log (user_fk, username, action, outcome, detail)
         SELECT created_by, username, 'dicom_transform_confirm', 'success', $2
         FROM dicom_transform_jobs WHERE id = $1",
    )
    .bind(job_id)
    .bind(serde_json::json!({ "job_id": job_id }))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct JobRecord {
    pub id: Uuid,
    pub mode: TransformMode,
    pub target: TransformTarget,
    pub status: String,
    pub reason: String,
    pub rules: Value,
    pub progress_completed: i32,
    pub progress_total: i32,
    pub preview: Value,
    pub result_summary: Value,
    pub pixel_risk: String,
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(FromRow)]
struct JobRow {
    id: Uuid,
    mode: String,
    target_type: String,
    target_key: String,
    status: String,
    reason: String,
    rules: Value,
    progress_completed: i32,
    progress_total: i32,
    preview: Value,
    result_summary: Value,
    pixel_risk: String,
    error_message: Option<String>,
    created_at: DateTime<Utc>,
    started_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
}

impl TryFrom<JobRow> for JobRecord {
    type Error = DbError;

    fn try_from(row: JobRow) -> Result<Self, Self::Error> {
        Ok(Self {
            id: row.id,
            mode: TransformMode::parse(&row.mode)?,
            target: TransformTarget {
                target_type: TargetType::parse(&row.target_type)?,
                key: row.target_key,
            },
            status: row.status,
            reason: row.reason,
            rules: row.rules,
            progress_completed: row.progress_completed,
            progress_total: row.progress_total,
            preview: row.preview,
            result_summary: row.result_summary,
            pixel_risk: row.pixel_risk,
            error_message: row.error_message,
            created_at: row.created_at,
            started_at: row.started_at,
            completed_at: row.completed_at,
        })
    }
}

const JOB_COLUMNS: &str = r#"
    id, mode, target_type, target_key, status, reason, rules,
    progress_completed, progress_total, preview, result_summary, pixel_risk,
    error_message, created_at, started_at, completed_at
"#;

pub async fn get_job(
    pool: &PgPool,
    institution_id: i64,
    job_id: Uuid,
) -> Result<JobRecord, DbError> {
    let sql = format!(
        "SELECT {JOB_COLUMNS} FROM dicom_transform_jobs WHERE id = $1 AND institution_id = $2"
    );
    let row = sqlx::query_as::<_, JobRow>(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(job_id)
        .bind(institution_id)
        .fetch_optional(pool)
        .await?
        .ok_or(DbError::NotFound)?;
    row.try_into()
}

pub async fn list_jobs(
    pool: &PgPool,
    institution_id: i64,
    limit: i64,
) -> Result<Vec<JobRecord>, DbError> {
    let sql = format!(
        "SELECT {JOB_COLUMNS} FROM dicom_transform_jobs
         WHERE institution_id = $1 ORDER BY created_at DESC LIMIT $2"
    );
    sqlx::query_as::<_, JobRow>(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(institution_id)
        .bind(limit.clamp(1, 200))
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(TryInto::try_into)
        .collect()
}

#[derive(Debug, Clone, FromRow)]
pub struct RunnableJob {
    pub id: Uuid,
    pub institution_id: i64,
    pub user_id: i64,
    pub username: String,
}

pub async fn list_runnable_jobs(pool: &PgPool, limit: i64) -> Result<Vec<RunnableJob>, DbError> {
    Ok(sqlx::query_as::<_, RunnableJob>(
        r#"
        SELECT id, institution_id, created_by AS user_id, username
        FROM dicom_transform_jobs
        WHERE status = 'queued'
          AND created_by IS NOT NULL AND username IS NOT NULL
        ORDER BY created_at
        LIMIT $1
        "#,
    )
    .bind(limit.clamp(1, 32))
    .fetch_all(pool)
    .await?)
}

pub async fn recover_interrupted_jobs(
    pool: &PgPool,
    started_before: DateTime<Utc>,
) -> Result<u64, DbError> {
    let result = sqlx::query(
        r#"
        UPDATE dicom_transform_jobs
        SET status = 'queued', started_at = NULL, progress_completed = 0,
            error_message = '服务重启后自动恢复'
        WHERE status = 'running' AND started_at < $1
        "#,
    )
    .bind(started_before)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

pub async fn claim_job(
    pool: &PgPool,
    institution_id: i64,
    job_id: Uuid,
) -> Result<JobRecord, DbError> {
    let sql = format!(
        "UPDATE dicom_transform_jobs
         SET status = 'running', started_at = now(), error_message = NULL
         WHERE id = $1 AND institution_id = $2 AND status = 'queued'
         RETURNING {JOB_COLUMNS}"
    );
    let row = sqlx::query_as::<_, JobRow>(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(job_id)
        .bind(institution_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| DbError::Conflict("任务不再处于 queued 状态".to_owned()))?;
    row.try_into()
}

pub async fn job_sources(pool: &PgPool, job_id: Uuid) -> Result<Vec<TransformSource>, DbError> {
    let rows = sqlx::query_as::<_, TransformSource>(
        r#"
        SELECT p.id AS patient_pk, p.patient_id,
               st.id AS study_pk, se.id AS series_pk, i.id AS instance_pk,
               i.logical_instance_id,
               item.source_version_fk AS current_version_id,
               v.version_number, v.storage_path, v.file_sha256,
               v.study_instance_uid, v.series_instance_uid, v.sop_instance_uid,
               i.sop_class_uid, v.transfer_syntax_uid,
               CASE
                 WHEN j.target_type = 'patient' THEN p.id::text = j.target_key
                 WHEN j.target_type = 'study' THEN v.study_instance_uid = j.target_key
                 WHEN j.target_type = 'series' THEN v.series_instance_uid = j.target_key
                 WHEN j.target_type = 'instance' THEN i.logical_instance_id::text = j.target_key
                 ELSE false
               END AS apply_rules
        FROM dicom_transform_items item
        JOIN dicom_transform_jobs j ON j.id = item.job_fk
        JOIN instances i ON i.id = item.instance_fk
        JOIN dicom_instance_versions v ON v.id = item.source_version_fk
        JOIN series se ON i.series_fk = se.id
        JOIN studies st ON se.study_fk = st.id
        JOIN patients p ON st.patient_fk = p.id
        WHERE item.job_fk = $1
        ORDER BY st.id, se.id, i.id
        "#,
    )
    .bind(job_id)
    .fetch_all(pool)
    .await?;
    if rows.is_empty() {
        return Err(DbError::NotFound);
    }
    Ok(rows)
}

pub async fn update_job_progress(
    pool: &PgPool,
    job_id: Uuid,
    completed: usize,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE dicom_transform_jobs SET progress_completed = $2 WHERE id = $1 AND status = 'running'",
    )
    .bind(job_id)
    .bind(i32::try_from(completed).unwrap_or(i32::MAX))
    .execute(pool)
    .await?;
    Ok(())
}

pub struct ActivatedVersion {
    pub source: TransformSource,
    /// The revision used as the derivation input. Usually this is the current base revision; for
    /// rollback it is the selected historical revision while stale detection still uses
    /// `source.current_version_id`.
    pub derivation_source_version_id: i64,
    pub metadata: InstanceMetadata,
    pub storage_path: String,
    pub file_size: u64,
    pub file_sha256: [u8; 32],
    pub uid_map: Value,
}

/// Insert all new versions, update the clinical projection, and write mandatory audit in one
/// transaction. Any stale source version or audit failure rolls everything back.
#[allow(clippy::too_many_arguments)]
pub async fn activate_clinical_job(
    pool: &PgPool,
    job_id: Uuid,
    institution_id: i64,
    user_id: i64,
    username: &str,
    mode: TransformMode,
    reason: &str,
    outputs: &[ActivatedVersion],
) -> Result<(), DbError> {
    if outputs.is_empty() {
        return Err(DbError::Invalid("转换没有输出实例".to_owned()));
    }
    let mut tx = pool.begin().await?;

    for output in outputs {
        let current: Option<i64> =
            sqlx::query_scalar("SELECT current_version_id FROM instances WHERE id = $1 FOR UPDATE")
                .bind(output.source.instance_pk)
                .fetch_optional(&mut *tx)
                .await?
                .flatten();
        if current != Some(output.source.current_version_id) {
            return Err(DbError::Conflict(format!(
                "实例 {} 的基础修订已变化",
                output.source.logical_instance_id
            )));
        }
    }

    reject_patient_id_collisions(&mut tx, institution_id, outputs).await?;
    update_current_hierarchy(&mut tx, outputs).await?;

    for output in outputs {
        let metadata = &output.metadata;
        let snapshot = serde_json::to_value(metadata)
            .map_err(|error| DbError::Invalid(format!("元数据快照序列化失败: {error}")))?;
        let version_id = sqlx::query_scalar::<_, i64>(
            r#"
            INSERT INTO dicom_instance_versions (
                logical_instance_id, instance_fk, version_number, source_version_fk,
                transform_job_fk, derivation_kind,
                study_instance_uid, series_instance_uid, sop_instance_uid,
                source_sop_instance_uid, transfer_syntax_uid,
                storage_path, file_size, file_sha256, metadata_snapshot,
                created_by, reason
            ) VALUES (
                $1, $2, $3, $4, $5, $6,
                $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17
            ) RETURNING id
            "#,
        )
        .bind(output.source.logical_instance_id)
        .bind(output.source.instance_pk)
        .bind(output.source.version_number + 1)
        .bind(output.derivation_source_version_id)
        .bind(job_id)
        .bind(mode.as_str())
        .bind(metadata.study.uid.as_str())
        .bind(metadata.series.uid.as_str())
        .bind(metadata.instance.uid.as_str())
        .bind(&output.source.sop_instance_uid)
        .bind(metadata.instance.transfer_syntax_uid.as_str())
        .bind(&output.storage_path)
        .bind(i64::try_from(output.file_size).unwrap_or(i64::MAX))
        .bind(output.file_sha256.as_slice())
        .bind(snapshot)
        .bind(user_id)
        .bind(reason)
        .fetch_one(&mut *tx)
        .await?;

        let result = sqlx::query(
            r#"
            UPDATE instances SET
                series_fk = $2, sop_instance_uid = $3, sop_class_uid = $4,
                instance_number = $5, transfer_syntax_uid = $6,
                image_rows = $7, image_columns = $8, number_of_frames = $9,
                image_position_patient = $10, image_orientation_patient = $11,
                storage_path = $12, file_size = $13, file_sha256 = $14,
                attributes = $15, current_version_id = $16
            WHERE id = $1 AND current_version_id = $17
            "#,
        )
        .bind(output.source.instance_pk)
        .bind(output.source.series_pk)
        .bind(metadata.instance.uid.as_str())
        .bind(
            metadata
                .instance
                .sop_class_uid
                .as_ref()
                .map(|uid| uid.as_str()),
        )
        .bind(metadata.instance.number)
        .bind(metadata.instance.transfer_syntax_uid.as_str())
        .bind(metadata.instance.rows)
        .bind(metadata.instance.columns)
        .bind(metadata.instance.number_of_frames)
        .bind(metadata.instance.image_position_patient.as_deref())
        .bind(metadata.instance.image_orientation_patient.as_deref())
        .bind(&output.storage_path)
        .bind(i64::try_from(output.file_size).unwrap_or(i64::MAX))
        .bind(output.file_sha256.as_slice())
        .bind(&metadata.instance.attributes)
        .bind(version_id)
        .bind(output.source.current_version_id)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() != 1 {
            return Err(DbError::Conflict("激活时基础修订已变化".to_owned()));
        }

        sqlx::query(
            r#"
            UPDATE dicom_transform_items
            SET output_version_fk = $3, output_path = $4, uid_map = $5, status = 'activated'
            WHERE job_fk = $1 AND logical_instance_id = $2
            "#,
        )
        .bind(job_id)
        .bind(output.source.logical_instance_id)
        .bind(version_id)
        .bind(&output.storage_path)
        .bind(&output.uid_map)
        .execute(&mut *tx)
        .await?;
    }

    // Mandatory audit: this INSERT is intentionally not best-effort.
    sqlx::query(
        r#"
        INSERT INTO audit_log (user_fk, username, action, outcome, detail)
        VALUES ($1, $2, 'dicom_transform_activate', 'success', $3)
        "#,
    )
    .bind(user_id)
    .bind(username)
    .bind(serde_json::json!({
        "job_id": job_id,
        "mode": mode.as_str(),
        "reason": reason,
        "instances": outputs.len()
    }))
    .execute(&mut *tx)
    .await?;

    let completed = sqlx::query(
        r#"
        UPDATE dicom_transform_jobs
        SET status = 'succeeded', progress_completed = progress_total,
            completed_at = now(), result_summary = $2
        WHERE id = $1 AND institution_id = $3 AND status = 'running'
        "#,
    )
    .bind(job_id)
    .bind(serde_json::json!({ "activated_instances": outputs.len() }))
    .bind(institution_id)
    .execute(&mut *tx)
    .await?;
    if completed.rows_affected() != 1 {
        return Err(DbError::Conflict(
            "任务不再处于可激活的 running 状态".to_owned(),
        ));
    }

    tx.commit().await?;
    Ok(())
}

async fn reject_patient_id_collisions(
    tx: &mut Transaction<'_, Postgres>,
    institution_id: i64,
    outputs: &[ActivatedVersion],
) -> Result<(), DbError> {
    let mut checked = HashSet::new();
    for output in outputs {
        if !checked.insert(output.source.patient_pk) {
            continue;
        }
        let conflict: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM patients
             WHERE institution_id = $1 AND patient_id = $2 AND id <> $3",
        )
        .bind(institution_id)
        .bind(&output.metadata.patient.patient_id)
        .bind(output.source.patient_pk)
        .fetch_optional(&mut **tx)
        .await?;
        if conflict.is_some() {
            return Err(DbError::Conflict(format!(
                "PatientID {} 已属于另一位病人，不支持自动合并",
                output.metadata.patient.patient_id
            )));
        }
    }
    Ok(())
}

async fn update_current_hierarchy(
    tx: &mut Transaction<'_, Postgres>,
    outputs: &[ActivatedVersion],
) -> Result<(), DbError> {
    let mut patients = HashSet::new();
    let mut studies = HashSet::new();
    let mut series = HashSet::new();
    for output in outputs {
        let metadata = &output.metadata;
        if patients.insert(output.source.patient_pk) {
            sqlx::query(
                r#"
                UPDATE patients SET patient_id = $2, issuer_of_patient_id = $3,
                    name = $4, name_normalized = $5, birth_date = $6, sex = $7,
                    attributes = $8
                WHERE id = $1
                "#,
            )
            .bind(output.source.patient_pk)
            .bind(&metadata.patient.patient_id)
            .bind(&metadata.patient.issuer_of_patient_id)
            .bind(&metadata.patient.name)
            .bind(&metadata.patient.name_normalized)
            .bind(metadata.patient.birth_date)
            .bind(&metadata.patient.sex)
            .bind(&metadata.patient.attributes)
            .execute(&mut **tx)
            .await?;
        }
        if studies.insert(output.source.study_pk) {
            sqlx::query(
                r#"
                UPDATE studies SET study_instance_uid = $2, study_date = $3,
                    study_time = $4, accession_number = $5, study_id = $6,
                    description = $7, referring_physician = $8, attributes = $9
                WHERE id = $1
                "#,
            )
            .bind(output.source.study_pk)
            .bind(metadata.study.uid.as_str())
            .bind(metadata.study.date)
            .bind(metadata.study.time)
            .bind(&metadata.study.accession_number)
            .bind(&metadata.study.study_id)
            .bind(&metadata.study.description)
            .bind(&metadata.study.referring_physician)
            .bind(&metadata.study.attributes)
            .execute(&mut **tx)
            .await?;
        }
        if series.insert(output.source.series_pk) {
            sqlx::query(
                r#"
                UPDATE series SET series_instance_uid = $2, series_number = $3,
                    modality = $4, description = $5, body_part_examined = $6,
                    protocol_name = $7, series_date = $8, series_time = $9,
                    attributes = $10
                WHERE id = $1
                "#,
            )
            .bind(output.source.series_pk)
            .bind(metadata.series.uid.as_str())
            .bind(metadata.series.number)
            .bind(&metadata.series.modality)
            .bind(&metadata.series.description)
            .bind(&metadata.series.body_part_examined)
            .bind(&metadata.series.protocol_name)
            .bind(metadata.series.date)
            .bind(metadata.series.time)
            .bind(&metadata.series.attributes)
            .execute(&mut **tx)
            .await?;
        }
    }
    Ok(())
}

pub async fn mark_job_failed(pool: &PgPool, job_id: Uuid, message: &str) -> Result<(), DbError> {
    let mut tx = pool.begin().await?;
    let changed = sqlx::query(
        "UPDATE dicom_transform_jobs
         SET status = 'failed', error_message = $2, completed_at = now()
         WHERE id = $1 AND status IN ('queued', 'running')",
    )
    .bind(job_id)
    .bind(message)
    .execute(&mut *tx)
    .await?;
    if changed.rows_affected() == 1 {
        sqlx::query(
            "INSERT INTO audit_log (user_fk, username, action, outcome, detail)
             SELECT created_by, username, 'dicom_transform_execute', 'failure', $2
             FROM dicom_transform_jobs WHERE id = $1",
        )
        .bind(job_id)
        .bind(serde_json::json!({ "job_id": job_id, "error": message }))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct RevisionRecord {
    pub id: i64,
    pub logical_instance_id: Uuid,
    pub version_number: i32,
    pub source_version_id: Option<i64>,
    pub job_id: Option<Uuid>,
    pub derivation_kind: String,
    pub study_instance_uid: String,
    pub series_instance_uid: String,
    pub sop_instance_uid: String,
    pub storage_path: String,
    pub file_size: i64,
    pub file_sha256_hex: String,
    pub metadata_snapshot: Value,
    pub reason: String,
    pub created_at: DateTime<Utc>,
    pub is_current: bool,
}

pub async fn list_revisions(
    pool: &PgPool,
    institution_id: i64,
    logical_instance_id: Uuid,
) -> Result<Vec<RevisionRecord>, DbError> {
    let rows = sqlx::query_as::<_, RevisionRecord>(
        r#"
        SELECT v.id, v.logical_instance_id, v.version_number,
               v.source_version_fk AS source_version_id,
               v.transform_job_fk AS job_id, v.derivation_kind,
               v.study_instance_uid, v.series_instance_uid, v.sop_instance_uid,
               v.storage_path, v.file_size, encode(v.file_sha256, 'hex') AS file_sha256_hex,
               v.metadata_snapshot, v.reason, v.created_at,
               (i.current_version_id = v.id) AS is_current
        FROM dicom_instance_versions v
        JOIN instances i ON i.id = v.instance_fk
        JOIN series se ON i.series_fk = se.id
        JOIN studies st ON se.study_fk = st.id
        JOIN patients p ON st.patient_fk = p.id
        WHERE v.logical_instance_id = $1
          AND p.institution_id = $2 AND st.institution_id = $2
        ORDER BY v.version_number DESC
        "#,
    )
    .bind(logical_instance_id)
    .bind(institution_id)
    .fetch_all(pool)
    .await?;
    if rows.is_empty() {
        return Err(DbError::NotFound);
    }
    Ok(rows)
}

pub async fn logical_instance_id_for_current_sop(
    pool: &PgPool,
    institution_id: i64,
    sop_instance_uid: &str,
) -> Result<Uuid, DbError> {
    sqlx::query_scalar(
        r#"
        SELECT v.logical_instance_id
        FROM dicom_instance_versions v
        JOIN instances i ON i.id = v.instance_fk
        JOIN series se ON se.id = i.series_fk
        JOIN studies st ON st.id = se.study_fk
        JOIN patients p ON p.id = st.patient_fk
        WHERE v.sop_instance_uid = $1
          AND p.institution_id = $2 AND st.institution_id = $2
        "#,
    )
    .bind(sop_instance_uid)
    .bind(institution_id)
    .fetch_optional(pool)
    .await?
    .ok_or(DbError::NotFound)
}
