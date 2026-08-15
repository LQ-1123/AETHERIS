use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};

use crate::DbError;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct UserWindowPreset {
    pub id: i64,
    pub modality: String,
    pub name: String,
    pub center: f64,
    pub width: f64,
    pub function: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub struct NewUserWindowPreset<'a> {
    pub institution_id: i64,
    pub user_id: i64,
    pub modality: &'a str,
    pub name: &'a str,
    pub center: f64,
    pub width: f64,
    pub function: &'a str,
}

pub async fn list_user_window_presets(
    pool: &PgPool,
    institution_id: i64,
    user_id: i64,
) -> Result<Vec<UserWindowPreset>, DbError> {
    sqlx::query_as::<_, UserWindowPreset>(
        "SELECT id, modality, name, window_center AS center, window_width AS width,
                voi_function AS function, created_at, updated_at
         FROM user_window_presets
         WHERE institution_id = $1 AND user_fk = $2
         ORDER BY modality, lower(name), id",
    )
    .bind(institution_id)
    .bind(user_id)
    .fetch_all(pool)
    .await
    .map_err(DbError::from)
}

pub async fn create_user_window_preset(
    pool: &PgPool,
    input: NewUserWindowPreset<'_>,
) -> Result<UserWindowPreset, DbError> {
    let record = sqlx::query_as::<_, UserWindowPreset>(
        "INSERT INTO user_window_presets (
             institution_id, user_fk, modality, name, window_center, window_width, voi_function
         )
         SELECT $1, u.id, $3, $4, $5, $6, $7
         FROM users u
         WHERE u.id = $2 AND u.institution_id = $1
         RETURNING id, modality, name, window_center AS center, window_width AS width,
                   voi_function AS function, created_at, updated_at",
    )
    .bind(input.institution_id)
    .bind(input.user_id)
    .bind(input.modality)
    .bind(input.name)
    .bind(input.center)
    .bind(input.width)
    .bind(input.function)
    .fetch_optional(pool)
    .await
    .map_err(map_unique_name)?;
    record.ok_or(DbError::NotFound)
}

pub async fn rename_user_window_preset(
    pool: &PgPool,
    institution_id: i64,
    user_id: i64,
    preset_id: i64,
    name: &str,
) -> Result<UserWindowPreset, DbError> {
    sqlx::query_as::<_, UserWindowPreset>(
        "UPDATE user_window_presets
         SET name = $4
         WHERE institution_id = $1 AND user_fk = $2 AND id = $3
         RETURNING id, modality, name, window_center AS center, window_width AS width,
                   voi_function AS function, created_at, updated_at",
    )
    .bind(institution_id)
    .bind(user_id)
    .bind(preset_id)
    .bind(name)
    .fetch_optional(pool)
    .await
    .map_err(map_unique_name)?
    .ok_or(DbError::NotFound)
}

pub async fn delete_user_window_preset(
    pool: &PgPool,
    institution_id: i64,
    user_id: i64,
    preset_id: i64,
) -> Result<(), DbError> {
    let result = sqlx::query(
        "DELETE FROM user_window_presets
         WHERE institution_id = $1 AND user_fk = $2 AND id = $3",
    )
    .bind(institution_id)
    .bind(user_id)
    .bind(preset_id)
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        Err(DbError::NotFound)
    } else {
        Ok(())
    }
}

fn map_unique_name(error: sqlx::Error) -> DbError {
    match &error {
        sqlx::Error::Database(database) if database.is_unique_violation() => {
            DbError::Conflict("同一模态下已存在同名窗预设".to_owned())
        }
        _ => DbError::Query(error),
    }
}
