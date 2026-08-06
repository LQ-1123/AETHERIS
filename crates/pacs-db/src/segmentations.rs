use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::DbError;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct SegmentationProject {
    pub id: Uuid,
    pub study_instance_uid: String,
    pub series_instance_uid: String,
    pub name: String,
    pub status: String,
    pub revision: i64,
    pub created_by: Option<i64>,
    pub modified_by: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct SegmentationSegment {
    pub id: Uuid,
    pub project_id: Uuid,
    pub segment_number: i32,
    pub label: String,
    pub description: Option<String>,
    pub color_r: i16,
    pub color_g: i16,
    pub color_b: i16,
    pub algorithm_type: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct SegmentationMask {
    pub segment_id: Uuid,
    pub sop_instance_uid: String,
    pub frame_number: i32,
    pub rows: i32,
    pub cols: i32,
    pub encoding: String,
    pub mask_data: Vec<u8>,
    pub revision: i64,
    pub modified_by: Option<i64>,
    pub updated_at: DateTime<Utc>,
}

pub struct NewSegmentationProject<'a> {
    pub id: Uuid,
    pub segment_id: Uuid,
    pub institution_id: i64,
    pub study_instance_uid: &'a str,
    pub series_instance_uid: &'a str,
    pub name: &'a str,
    pub segment_label: &'a str,
    pub color: [i16; 3],
    pub user_id: i64,
}

pub struct UpsertSegmentationMask<'a> {
    pub institution_id: i64,
    pub project_id: Uuid,
    pub segment_id: Uuid,
    pub sop_instance_uid: &'a str,
    pub frame_number: i32,
    pub rows: i32,
    pub cols: i32,
    pub mask_data: &'a [u8],
    pub expected_revision: i64,
    pub user_id: i64,
}

pub struct SegmentationMaskUpdate<'a> {
    pub sop_instance_uid: &'a str,
    pub frame_number: i32,
    pub rows: i32,
    pub cols: i32,
    pub mask_data: &'a [u8],
    pub expected_revision: i64,
}

pub async fn list_segmentation_projects(
    pool: &PgPool,
    institution_id: i64,
    study_uid: &str,
    series_uid: &str,
) -> Result<Vec<SegmentationProject>, DbError> {
    Ok(sqlx::query_as::<_, SegmentationProject>(
        "SELECT id, study_instance_uid, series_instance_uid, name, status, revision,
                created_by, modified_by, created_at, updated_at
         FROM segmentation_projects
         WHERE institution_id = $1 AND study_instance_uid = $2 AND series_instance_uid = $3
         ORDER BY updated_at DESC, id",
    )
    .bind(institution_id)
    .bind(study_uid)
    .bind(series_uid)
    .fetch_all(pool)
    .await?)
}

pub async fn create_segmentation_project(
    pool: &PgPool,
    input: NewSegmentationProject<'_>,
) -> Result<(SegmentationProject, SegmentationSegment), DbError> {
    let mut transaction = pool.begin().await?;
    let project = sqlx::query_as::<_, SegmentationProject>(
        "INSERT INTO segmentation_projects (
            id, institution_id, series_fk, study_instance_uid, series_instance_uid,
            name, created_by, modified_by
         )
         SELECT $1, $2, se.id, $3, $4, $5, $6, $6
         FROM series se JOIN studies st ON st.id = se.study_fk
         WHERE st.institution_id = $2 AND st.study_instance_uid = $3
           AND se.series_instance_uid = $4
         RETURNING id, study_instance_uid, series_instance_uid, name, status, revision,
                   created_by, modified_by, created_at, updated_at",
    )
    .bind(input.id)
    .bind(input.institution_id)
    .bind(input.study_instance_uid)
    .bind(input.series_instance_uid)
    .bind(input.name)
    .bind(input.user_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(DbError::NotFound)?;
    let segment = sqlx::query_as::<_, SegmentationSegment>(
        "INSERT INTO segmentation_segments (
            id, project_fk, segment_number, label, color_r, color_g, color_b, algorithm_type
         ) VALUES ($1, $2, 1, $3, $4, $5, $6, 'manual')
         RETURNING id, project_fk AS project_id, segment_number, label, description,
                   color_r, color_g, color_b, algorithm_type, created_at, updated_at",
    )
    .bind(input.segment_id)
    .bind(input.id)
    .bind(input.segment_label)
    .bind(input.color[0])
    .bind(input.color[1])
    .bind(input.color[2])
    .fetch_one(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok((project, segment))
}

pub async fn list_segmentation_segments(
    pool: &PgPool,
    institution_id: i64,
    project_id: Uuid,
) -> Result<Vec<SegmentationSegment>, DbError> {
    Ok(sqlx::query_as::<_, SegmentationSegment>(
        "SELECT s.id, s.project_fk AS project_id, s.segment_number, s.label, s.description,
                s.color_r, s.color_g, s.color_b, s.algorithm_type, s.created_at, s.updated_at
         FROM segmentation_segments s
         JOIN segmentation_projects p ON p.id = s.project_fk
         WHERE p.institution_id = $1 AND p.id = $2
         ORDER BY s.segment_number",
    )
    .bind(institution_id)
    .bind(project_id)
    .fetch_all(pool)
    .await?)
}

pub async fn list_segmentation_masks(
    pool: &PgPool,
    institution_id: i64,
    project_id: Uuid,
    sop_uid: &str,
    frame_number: i32,
) -> Result<Vec<SegmentationMask>, DbError> {
    Ok(sqlx::query_as::<_, SegmentationMask>(
        "SELECT m.segment_fk AS segment_id, m.sop_instance_uid, m.frame_number,
                m.rows, m.cols, m.encoding, m.mask_data, m.revision,
                m.modified_by, m.updated_at
         FROM segmentation_masks m
         JOIN segmentation_segments s ON s.id = m.segment_fk
         JOIN segmentation_projects p ON p.id = s.project_fk
         WHERE p.institution_id = $1 AND p.id = $2
           AND m.sop_instance_uid = $3 AND m.frame_number = $4
         ORDER BY s.segment_number",
    )
    .bind(institution_id)
    .bind(project_id)
    .bind(sop_uid)
    .bind(frame_number)
    .fetch_all(pool)
    .await?)
}

pub async fn list_segmentation_segment_masks(
    pool: &PgPool,
    institution_id: i64,
    project_id: Uuid,
    segment_id: Uuid,
) -> Result<Vec<SegmentationMask>, DbError> {
    Ok(sqlx::query_as::<_, SegmentationMask>(
        "SELECT m.segment_fk AS segment_id, m.sop_instance_uid, m.frame_number,
                m.rows, m.cols, m.encoding, m.mask_data, m.revision,
                m.modified_by, m.updated_at
         FROM segmentation_masks m
         JOIN segmentation_segments s ON s.id = m.segment_fk
         JOIN segmentation_projects p ON p.id = s.project_fk
         WHERE p.institution_id = $1 AND p.id = $2 AND s.id = $3
         ORDER BY m.sop_instance_uid, m.frame_number",
    )
    .bind(institution_id)
    .bind(project_id)
    .bind(segment_id)
    .fetch_all(pool)
    .await?)
}

pub async fn upsert_segmentation_masks_batch(
    pool: &PgPool,
    institution_id: i64,
    project_id: Uuid,
    segment_id: Uuid,
    updates: &[SegmentationMaskUpdate<'_>],
    user_id: i64,
) -> Result<Vec<SegmentationMask>, DbError> {
    if updates.is_empty() {
        return Err(DbError::Invalid("Mask 批量更新不能为空".to_owned()));
    }
    let mut transaction = pool.begin().await?;
    let owns_segment: bool = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM segmentation_segments s
            JOIN segmentation_projects p ON p.id = s.project_fk
            WHERE p.institution_id = $1 AND p.id = $2 AND s.id = $3
         )",
    )
    .bind(institution_id)
    .bind(project_id)
    .bind(segment_id)
    .fetch_one(&mut *transaction)
    .await?;
    if !owns_segment {
        return Err(DbError::NotFound);
    }

    let mut records = Vec::with_capacity(updates.len());
    for update in updates {
        let record = if update.expected_revision == 0 {
            sqlx::query_as::<_, SegmentationMask>(
                "INSERT INTO segmentation_masks (
                    segment_fk, sop_instance_uid, frame_number, rows, cols, mask_data, modified_by
                 )
                 SELECT $1, $2, $3, $4, $5, $6, $7
                 WHERE EXISTS (
                    SELECT 1 FROM segmentation_projects p
                    JOIN instances i ON i.series_fk = p.series_fk
                    WHERE p.id = $8 AND i.sop_instance_uid = $2
                 )
                 ON CONFLICT DO NOTHING
                 RETURNING segment_fk AS segment_id, sop_instance_uid, frame_number, rows, cols,
                           encoding, mask_data, revision, modified_by, updated_at",
            )
            .bind(segment_id)
            .bind(update.sop_instance_uid)
            .bind(update.frame_number)
            .bind(update.rows)
            .bind(update.cols)
            .bind(update.mask_data)
            .bind(user_id)
            .bind(project_id)
            .fetch_optional(&mut *transaction)
            .await?
        } else {
            sqlx::query_as::<_, SegmentationMask>(
                "UPDATE segmentation_masks SET
                    rows = $4, cols = $5, mask_data = $6,
                    revision = revision + 1, modified_by = $7
                 WHERE segment_fk = $1 AND sop_instance_uid = $2 AND frame_number = $3
                   AND revision = $8
                 RETURNING segment_fk AS segment_id, sop_instance_uid, frame_number, rows, cols,
                           encoding, mask_data, revision, modified_by, updated_at",
            )
            .bind(segment_id)
            .bind(update.sop_instance_uid)
            .bind(update.frame_number)
            .bind(update.rows)
            .bind(update.cols)
            .bind(update.mask_data)
            .bind(user_id)
            .bind(update.expected_revision)
            .fetch_optional(&mut *transaction)
            .await?
        };
        let Some(record) = record else {
            transaction.rollback().await?;
            return Err(DbError::Conflict(format!(
                "Mask {}#{} 已被其他用户修改，请重新加载",
                update.sop_instance_uid, update.frame_number
            )));
        };
        records.push(record);
    }
    sqlx::query(
        "UPDATE segmentation_projects SET revision = revision + 1, modified_by = $2
         WHERE id = $1",
    )
    .bind(project_id)
    .bind(user_id)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(records)
}

pub async fn upsert_segmentation_mask(
    pool: &PgPool,
    input: UpsertSegmentationMask<'_>,
) -> Result<SegmentationMask, DbError> {
    let record = if input.expected_revision == 0 {
        sqlx::query_as::<_, SegmentationMask>(
            "INSERT INTO segmentation_masks (
                segment_fk, sop_instance_uid, frame_number, rows, cols, mask_data, modified_by
             )
             SELECT s.id, $4, $5, $6, $7, $8, $9
             FROM segmentation_segments s
             JOIN segmentation_projects p ON p.id = s.project_fk
             JOIN instances i ON i.series_fk = p.series_fk AND i.sop_instance_uid = $4
             WHERE p.institution_id = $1 AND p.id = $2 AND s.id = $3
             ON CONFLICT DO NOTHING
             RETURNING segment_fk AS segment_id, sop_instance_uid, frame_number, rows, cols,
                       encoding, mask_data, revision, modified_by, updated_at",
        )
        .bind(input.institution_id)
        .bind(input.project_id)
        .bind(input.segment_id)
        .bind(input.sop_instance_uid)
        .bind(input.frame_number)
        .bind(input.rows)
        .bind(input.cols)
        .bind(input.mask_data)
        .bind(input.user_id)
        .fetch_optional(pool)
        .await?
    } else {
        sqlx::query_as::<_, SegmentationMask>(
            "UPDATE segmentation_masks m SET
                mask_data = $8, rows = $6, cols = $7, revision = m.revision + 1,
                modified_by = $9
             FROM segmentation_segments s, segmentation_projects p
             WHERE m.segment_fk = s.id AND s.project_fk = p.id
               AND p.institution_id = $1 AND p.id = $2 AND s.id = $3
               AND m.sop_instance_uid = $4 AND m.frame_number = $5
               AND m.revision = $10
             RETURNING m.segment_fk AS segment_id, m.sop_instance_uid, m.frame_number,
                       m.rows, m.cols, m.encoding, m.mask_data, m.revision,
                       m.modified_by, m.updated_at",
        )
        .bind(input.institution_id)
        .bind(input.project_id)
        .bind(input.segment_id)
        .bind(input.sop_instance_uid)
        .bind(input.frame_number)
        .bind(input.rows)
        .bind(input.cols)
        .bind(input.mask_data)
        .bind(input.user_id)
        .bind(input.expected_revision)
        .fetch_optional(pool)
        .await?
    };
    if let Some(record) = record {
        sqlx::query(
            "UPDATE segmentation_projects SET revision = revision + 1, modified_by = $2
             WHERE id = $1",
        )
        .bind(input.project_id)
        .bind(input.user_id)
        .execute(pool)
        .await?;
        return Ok(record);
    }
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM segmentation_masks m
            JOIN segmentation_segments s ON s.id = m.segment_fk
            JOIN segmentation_projects p ON p.id = s.project_fk
            WHERE p.institution_id = $1 AND p.id = $2 AND s.id = $3
              AND m.sop_instance_uid = $4 AND m.frame_number = $5
         )",
    )
    .bind(input.institution_id)
    .bind(input.project_id)
    .bind(input.segment_id)
    .bind(input.sop_instance_uid)
    .bind(input.frame_number)
    .fetch_one(pool)
    .await?;
    if exists {
        Err(DbError::Conflict(
            "Mask 已被其他用户修改，请刷新后重试".to_owned(),
        ))
    } else if input.expected_revision == 0 {
        Err(DbError::NotFound)
    } else {
        Err(DbError::Conflict("Mask 不存在或版本已变化".to_owned()))
    }
}
