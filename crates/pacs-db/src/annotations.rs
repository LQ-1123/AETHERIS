use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::DbError;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AnnotationRecord {
    pub id: Uuid,
    pub study_instance_uid: String,
    pub series_instance_uid: String,
    pub sop_instance_uid: Option<String>,
    pub frame_number: Option<i32>,
    pub coordinate_space: String,
    pub mpr_plane: Option<String>,
    pub schema_version: i32,
    pub kind: String,
    pub geometry: Value,
    pub revision: i64,
    pub created_by: Option<i64>,
    pub modified_by: Option<i64>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub struct NewAnnotation<'a> {
    pub id: Uuid,
    pub institution_id: i64,
    pub study_instance_uid: &'a str,
    pub series_instance_uid: &'a str,
    pub sop_instance_uid: Option<&'a str>,
    pub frame_number: Option<i32>,
    pub coordinate_space: &'a str,
    pub mpr_plane: Option<&'a str>,
    pub schema_version: i32,
    pub kind: &'a str,
    pub geometry: &'a Value,
    pub user_id: i64,
}

pub struct AnnotationUpdate<'a> {
    pub institution_id: i64,
    pub study_instance_uid: &'a str,
    pub series_instance_uid: &'a str,
    pub annotation_id: Uuid,
    pub expected_revision: i64,
    pub geometry: &'a Value,
    pub deleted: bool,
    pub user_id: i64,
}

pub async fn list_annotations(
    pool: &PgPool,
    institution_id: i64,
    study_uid: &str,
    series_uid: &str,
    since: Option<DateTime<Utc>>,
) -> Result<Vec<AnnotationRecord>, DbError> {
    let records = sqlx::query_as::<_, AnnotationRecord>(
        "SELECT id, study_instance_uid, series_instance_uid, sop_instance_uid, frame_number,
                coordinate_space, mpr_plane, schema_version, kind, geometry, revision,
                created_by, modified_by, deleted_at, created_at, updated_at
         FROM viewer_annotations
         WHERE institution_id = $1 AND study_instance_uid = $2 AND series_instance_uid = $3
           AND ($4::timestamptz IS NULL OR updated_at >= $4)
           AND ($4::timestamptz IS NOT NULL OR deleted_at IS NULL)
         ORDER BY updated_at, id LIMIT 10001",
    )
    .bind(institution_id)
    .bind(study_uid)
    .bind(series_uid)
    .bind(since)
    .fetch_all(pool)
    .await
    .map_err(DbError::from)?;
    if records.len() > 10_000 {
        return Err(DbError::TooManyResults { limit: 10_000 });
    }
    Ok(records)
}

pub async fn create_annotation(
    pool: &PgPool,
    input: NewAnnotation<'_>,
) -> Result<AnnotationRecord, DbError> {
    let record = sqlx::query_as::<_, AnnotationRecord>(
        "INSERT INTO viewer_annotations (
            id, institution_id, series_fk, study_instance_uid, series_instance_uid,
            sop_instance_uid, frame_number, coordinate_space, mpr_plane,
            schema_version, kind, geometry, created_by, modified_by
         )
         SELECT $1, $2, se.id, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $12
         FROM series se JOIN studies st ON st.id = se.study_fk
         WHERE st.institution_id = $2 AND st.study_instance_uid = $3
           AND se.series_instance_uid = $4
           AND ($7 = 'patient' OR EXISTS (
               SELECT 1 FROM instances i
               WHERE i.series_fk = se.id AND i.sop_instance_uid = $5
           ))
         RETURNING id, study_instance_uid, series_instance_uid, sop_instance_uid, frame_number,
                   coordinate_space, mpr_plane, schema_version, kind, geometry, revision,
                   created_by, modified_by, deleted_at, created_at, updated_at",
    )
    .bind(input.id)
    .bind(input.institution_id)
    .bind(input.study_instance_uid)
    .bind(input.series_instance_uid)
    .bind(input.sop_instance_uid)
    .bind(input.frame_number)
    .bind(input.coordinate_space)
    .bind(input.mpr_plane)
    .bind(input.schema_version)
    .bind(input.kind)
    .bind(input.geometry)
    .bind(input.user_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| match &error {
        sqlx::Error::Database(database) if database.is_unique_violation() => {
            DbError::Conflict("标注 ID 已存在".to_owned())
        }
        _ => DbError::Query(error),
    })?;
    record.ok_or(DbError::NotFound)
}

pub async fn update_annotation(
    pool: &PgPool,
    input: AnnotationUpdate<'_>,
) -> Result<AnnotationRecord, DbError> {
    let record = sqlx::query_as::<_, AnnotationRecord>(
        "UPDATE viewer_annotations
         SET geometry = $6, deleted_at = CASE WHEN $7 THEN COALESCE(deleted_at, now()) ELSE NULL END,
             modified_by = $8, revision = revision + 1
         WHERE institution_id = $1 AND study_instance_uid = $2 AND series_instance_uid = $3
           AND id = $4 AND revision = $5
         RETURNING id, study_instance_uid, series_instance_uid, sop_instance_uid, frame_number,
                   coordinate_space, mpr_plane, schema_version, kind, geometry, revision,
                   created_by, modified_by, deleted_at, created_at, updated_at",
    )
        .bind(input.institution_id)
        .bind(input.study_instance_uid)
        .bind(input.series_instance_uid)
        .bind(input.annotation_id)
        .bind(input.expected_revision)
        .bind(input.geometry)
        .bind(input.deleted)
        .bind(input.user_id)
        .fetch_optional(pool)
        .await?;
    if let Some(record) = record {
        return Ok(record);
    }
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM viewer_annotations
         WHERE institution_id = $1 AND study_instance_uid = $2
           AND series_instance_uid = $3 AND id = $4)",
    )
    .bind(input.institution_id)
    .bind(input.study_instance_uid)
    .bind(input.series_instance_uid)
    .bind(input.annotation_id)
    .fetch_one(pool)
    .await?;
    if exists {
        Err(DbError::Conflict(
            "标注已被其他用户修改，请刷新后重试".to_owned(),
        ))
    } else {
        Err(DbError::NotFound)
    }
}
