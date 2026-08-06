//! Institution-scoped persistence for resumable imports and ZIP exports.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::DbError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UploadStatus {
    Uploading,
    Ready,
    Failed,
}

impl UploadStatus {
    fn parse(value: &str) -> Result<Self, DbError> {
        match value {
            "uploading" => Ok(Self::Uploading),
            "ready" => Ok(Self::Ready),
            "failed" => Ok(Self::Failed),
            _ => Err(DbError::Invalid(format!("未知上传状态 {value:?}"))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportUpload {
    pub id: Uuid,
    pub job_id: Uuid,
    pub relative_name: String,
    pub expected_size: i64,
    #[serde(skip_serializing)]
    pub expected_sha256: Option<Vec<u8>>,
    pub received_size: i64,
    #[serde(skip_serializing)]
    pub temp_name: String,
    pub status: UploadStatus,
    pub error_message: Option<String>,
}

pub async fn create_import_upload(
    pool: &PgPool,
    institution_id: i64,
    upload: &ImportUpload,
) -> Result<ImportUpload, DbError> {
    if upload.relative_name.trim().is_empty() || upload.expected_size < 0 {
        return Err(DbError::Invalid(
            "上传文件名不能为空且大小不能为负数".to_owned(),
        ));
    }
    let row = sqlx::query(
        "INSERT INTO import_uploads (id, job_fk, relative_name, expected_size, expected_sha256, temp_name)
         SELECT $1, j.id, $3, $4, $5, $6 FROM background_jobs j
         WHERE j.id = $2 AND j.institution_id = $7 AND j.kind = 'import'
           AND j.status = 'queued' AND j.cancel_requested = false
         RETURNING import_uploads.*"
    ).bind(upload.id).bind(upload.job_id).bind(&upload.relative_name)
      .bind(upload.expected_size).bind(&upload.expected_sha256).bind(&upload.temp_name)
      .bind(institution_id).fetch_optional(pool).await?
      .ok_or_else(|| DbError::Conflict("导入任务不存在或已不能接收文件".to_owned()))?;
    decode_upload(&row)
}

pub async fn list_import_uploads(
    pool: &PgPool,
    institution_id: i64,
    job_id: Uuid,
) -> Result<Vec<ImportUpload>, DbError> {
    let rows = sqlx::query(
        "SELECT u.* FROM import_uploads u JOIN background_jobs j ON j.id = u.job_fk
         WHERE j.institution_id = $1 AND j.id = $2 ORDER BY u.created_at, u.id",
    )
    .bind(institution_id)
    .bind(job_id)
    .fetch_all(pool)
    .await?;
    rows.iter().map(decode_upload).collect()
}

/// Atomically reserve a sequential chunk. The caller writes only after this succeeds.
pub async fn advance_upload(
    pool: &PgPool,
    institution_id: i64,
    upload_id: Uuid,
    offset: i64,
    chunk_size: i64,
) -> Result<ImportUpload, DbError> {
    if offset < 0 || chunk_size <= 0 {
        return Err(DbError::Invalid("上传偏移或分块大小无效".to_owned()));
    }
    let row = sqlx::query(
        "UPDATE import_uploads u SET received_size = received_size + $4
         FROM background_jobs j WHERE j.id = u.job_fk AND j.institution_id = $1
           AND u.id = $2 AND u.status = 'uploading' AND u.received_size = $3
           AND u.received_size + $4 <= u.expected_size AND j.status = 'queued'
         RETURNING u.*",
    )
    .bind(institution_id)
    .bind(upload_id)
    .bind(offset)
    .bind(chunk_size)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DbError::Conflict("上传偏移不匹配、文件已完成或分块超出声明大小".to_owned()))?;
    decode_upload(&row)
}

pub async fn mark_upload_ready(
    pool: &PgPool,
    institution_id: i64,
    upload_id: Uuid,
) -> Result<ImportUpload, DbError> {
    set_upload_status(pool, institution_id, upload_id, "ready", None, true).await
}

pub async fn mark_upload_failed(
    pool: &PgPool,
    institution_id: i64,
    upload_id: Uuid,
    error: &str,
) -> Result<ImportUpload, DbError> {
    set_upload_status(
        pool,
        institution_id,
        upload_id,
        "failed",
        Some(error),
        false,
    )
    .await
}

async fn set_upload_status(
    pool: &PgPool,
    institution_id: i64,
    upload_id: Uuid,
    status: &str,
    error: Option<&str>,
    require_complete: bool,
) -> Result<ImportUpload, DbError> {
    let row = sqlx::query(
        "UPDATE import_uploads u SET status = $3, error_message = $4
         FROM background_jobs j WHERE j.id = u.job_fk AND j.institution_id = $1 AND u.id = $2
           AND u.status = 'uploading' AND (NOT $5 OR u.received_size = u.expected_size)
         RETURNING u.*",
    )
    .bind(institution_id)
    .bind(upload_id)
    .bind(status)
    .bind(error)
    .bind(require_complete)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DbError::Conflict("上传状态不能变更".to_owned()))?;
    decode_upload(&row)
}

#[derive(Debug, Clone)]
pub struct ExportSource {
    pub study_uid: String,
    pub series_uid: String,
    pub sop_uid: String,
    pub storage_path: String,
    pub file_size: i64,
    pub file_sha256: Vec<u8>,
}

pub async fn list_export_sources(
    pool: &PgPool,
    institution_id: i64,
    study_uid: &str,
    series_uid: Option<&str>,
) -> Result<Vec<ExportSource>, DbError> {
    let rows = sqlx::query(
        "SELECT st.study_instance_uid, se.series_instance_uid, i.sop_instance_uid,
                v.storage_path, v.file_size, v.file_sha256
         FROM instances i JOIN series se ON se.id = i.series_fk
         JOIN studies st ON st.id = se.study_fk JOIN patients p ON p.id = st.patient_fk
         JOIN dicom_instance_versions v ON v.id = i.current_version_id
         WHERE p.institution_id = $1 AND st.institution_id = $1
           AND st.storage_tier <> 'quarantine'
           AND st.study_instance_uid = $2 AND ($3::text IS NULL OR se.series_instance_uid = $3)
         ORDER BY se.series_instance_uid, i.instance_number NULLS LAST, i.sop_instance_uid",
    )
    .bind(institution_id)
    .bind(study_uid)
    .bind(series_uid)
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(ExportSource {
                study_uid: row.try_get("study_instance_uid")?,
                series_uid: row.try_get("series_instance_uid")?,
                sop_uid: row.try_get("sop_instance_uid")?,
                storage_path: row.try_get("storage_path")?,
                file_size: row.try_get("file_size")?,
                file_sha256: row.try_get("file_sha256")?,
            })
        })
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportArtifact {
    pub job_id: Uuid,
    #[serde(skip_serializing)]
    pub relative_path: String,
    pub file_size: i64,
    pub file_sha256: Vec<u8>,
    pub download_name: String,
    pub expires_at: DateTime<Utc>,
}

pub async fn save_export_artifact(pool: &PgPool, artifact: &ExportArtifact) -> Result<(), DbError> {
    sqlx::query("INSERT INTO export_artifacts (job_fk, relative_path, file_size, file_sha256, download_name, expires_at) VALUES ($1,$2,$3,$4,$5,$6)")
        .bind(artifact.job_id).bind(&artifact.relative_path).bind(artifact.file_size)
        .bind(&artifact.file_sha256).bind(&artifact.download_name).bind(artifact.expires_at)
        .execute(pool).await?;
    Ok(())
}

pub async fn find_export_artifact(
    pool: &PgPool,
    institution_id: i64,
    job_id: Uuid,
) -> Result<ExportArtifact, DbError> {
    let row = sqlx::query(
        "SELECT a.* FROM export_artifacts a JOIN background_jobs j ON j.id = a.job_fk
         WHERE j.institution_id = $1 AND j.id = $2 AND a.expires_at > now()",
    )
    .bind(institution_id)
    .bind(job_id)
    .fetch_optional(pool)
    .await?
    .ok_or(DbError::NotFound)?;
    Ok(ExportArtifact {
        job_id: row.try_get("job_fk")?,
        relative_path: row.try_get("relative_path")?,
        file_size: row.try_get("file_size")?,
        file_sha256: row.try_get("file_sha256")?,
        download_name: row.try_get("download_name")?,
        expires_at: row.try_get("expires_at")?,
    })
}

pub async fn purge_expired_export_artifacts(pool: &PgPool) -> Result<Vec<String>, DbError> {
    let rows = sqlx::query(
        "DELETE FROM export_artifacts WHERE expires_at <= now() RETURNING relative_path",
    )
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|row| row.try_get("relative_path").map_err(DbError::from))
        .collect()
}

fn decode_upload(row: &sqlx::postgres::PgRow) -> Result<ImportUpload, DbError> {
    Ok(ImportUpload {
        id: row.try_get("id")?,
        job_id: row.try_get("job_fk")?,
        relative_name: row.try_get("relative_name")?,
        expected_size: row.try_get("expected_size")?,
        expected_sha256: row.try_get("expected_sha256")?,
        received_size: row.try_get("received_size")?,
        temp_name: row.try_get("temp_name")?,
        status: UploadStatus::parse(row.try_get("status")?)?,
        error_message: row.try_get("error_message")?,
    })
}
