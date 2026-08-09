//! Viewer 工作列表查询。
//!
//! 这些查询返回应用 JSON，而不是 DICOM 标识符。每个入口都要求机构 ID，
//! 防止调用方忘记租户边界。

use chrono::{NaiveDate, NaiveTime};
use serde::Serialize;
use sqlx::PgPool;

use crate::DbError;

type PatientRow = (
    i64,
    String,
    Option<String>,
    Option<String>,
    Option<NaiveDate>,
    Option<String>,
    i64,
    i64,
    i64,
    Option<NaiveDate>,
);
type StudyRow = (
    String,
    Option<NaiveDate>,
    Option<NaiveTime>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Vec<String>,
    i32,
    i32,
);
type SeriesRow = (
    String,
    Option<i32>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    i32,
);

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PatientSummary {
    pub id: i64,
    pub patient_id: String,
    pub issuer_of_patient_id: Option<String>,
    pub name: Option<String>,
    pub birth_date: Option<NaiveDate>,
    pub sex: Option<String>,
    pub study_count: i64,
    pub series_count: i64,
    pub instance_count: i64,
    pub latest_study_date: Option<NaiveDate>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct StudySummary {
    pub study_uid: String,
    pub study_date: Option<NaiveDate>,
    pub study_time: Option<NaiveTime>,
    pub accession_number: Option<String>,
    pub study_id: Option<String>,
    pub description: Option<String>,
    pub referring_physician: Option<String>,
    pub modalities: Vec<String>,
    pub series_count: i32,
    pub instance_count: i32,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SeriesSummary {
    pub series_uid: String,
    pub series_number: Option<i32>,
    pub modality: Option<String>,
    pub description: Option<String>,
    pub body_part_examined: Option<String>,
    pub protocol_name: Option<String>,
    pub instance_count: i32,
}

/// 搜索一个机构下的病人。搜索文本按字面量包含匹配，不把 `%`/`_` 当通配符。
pub async fn list_patients(
    pool: &PgPool,
    institution_id: i64,
    user_id: i64,
    is_admin: bool,
    query: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<PatientSummary>, DbError> {
    let normalized = pacs_core::normalize_person_name(query);
    let id_pattern = contains_pattern(query);
    let name_pattern = contains_pattern(&normalized);
    let rows: Vec<PatientRow> = sqlx::query_as(
        "SELECT p.id, p.patient_id, p.issuer_of_patient_id, p.name, p.birth_date, p.sex,
                COUNT(DISTINCT st.id)::BIGINT,
                COUNT(DISTINCT se.id)::BIGINT,
                COALESCE(SUM(se.number_of_instances), 0)::BIGINT,
                MAX(st.study_date)
         FROM patients p
         JOIN studies st ON st.patient_fk = p.id AND st.institution_id = $1
              AND st.storage_tier <> 'quarantine'
         JOIN series se ON se.study_fk=st.id
         LEFT JOIN dicom_devices d ON d.id=se.source_device_fk
         WHERE p.institution_id = $1
           AND ($3 OR (d.status='active' AND se.source_status='trusted'
                AND EXISTS(SELECT 1 FROM user_device_grants g
                           WHERE g.user_fk=$2 AND g.device_fk=d.id)))
           AND ($4 = '' OR p.patient_id ILIKE $5 ESCAPE '\\'
                OR p.name_normalized LIKE $6 ESCAPE '\\')
         GROUP BY p.id
         ORDER BY MAX(st.study_date) DESC NULLS LAST, p.patient_id, p.id
         LIMIT $7 OFFSET $8",
    )
    .bind(institution_id)
    .bind(user_id)
    .bind(is_admin)
    .bind(query)
    .bind(id_pattern)
    .bind(name_pattern)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                patient_id,
                issuer_of_patient_id,
                name,
                birth_date,
                sex,
                study_count,
                series_count,
                instance_count,
                latest_study_date,
            )| PatientSummary {
                id,
                patient_id,
                issuer_of_patient_id,
                name,
                birth_date,
                sex,
                study_count,
                series_count,
                instance_count,
                latest_study_date,
            },
        )
        .collect())
}

pub async fn list_patient_studies(
    pool: &PgPool,
    institution_id: i64,
    user_id: i64,
    is_admin: bool,
    patient_id: i64,
) -> Result<Vec<StudySummary>, DbError> {
    let rows: Vec<StudyRow> = sqlx::query_as(
        "SELECT st.study_instance_uid, st.study_date, st.study_time,
                st.accession_number, st.study_id, st.description,
                st.referring_physician, st.modalities,
                st.number_of_series, st.number_of_instances
         FROM studies st
         JOIN patients p ON st.patient_fk = p.id
         WHERE p.id = $1
           AND p.institution_id = $2
           AND st.institution_id = $2
           AND st.storage_tier <> 'quarantine'
           AND ($4 OR EXISTS(
                SELECT 1 FROM series visible
                JOIN dicom_devices d ON d.id=visible.source_device_fk AND d.status='active'
                JOIN user_device_grants g ON g.device_fk=d.id AND g.user_fk=$3
                WHERE visible.study_fk=st.id AND visible.source_status='trusted'))
         ORDER BY st.study_date DESC NULLS LAST,
                  st.study_time DESC NULLS LAST,
                  st.study_instance_uid",
    )
    .bind(patient_id)
    .bind(institution_id)
    .bind(user_id)
    .bind(is_admin)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(
                study_uid,
                study_date,
                study_time,
                accession_number,
                study_id,
                description,
                referring_physician,
                modalities,
                series_count,
                instance_count,
            )| StudySummary {
                study_uid,
                study_date,
                study_time,
                accession_number,
                study_id,
                description,
                referring_physician,
                modalities,
                series_count,
                instance_count,
            },
        )
        .collect())
}

pub async fn list_study_series(
    pool: &PgPool,
    institution_id: i64,
    user_id: i64,
    is_admin: bool,
    study_uid: &str,
) -> Result<Vec<SeriesSummary>, DbError> {
    let rows: Vec<SeriesRow> = sqlx::query_as(
        "SELECT se.series_instance_uid, se.series_number, se.modality,
                se.description, se.body_part_examined, se.protocol_name,
                se.number_of_instances
         FROM series se
         JOIN studies st ON se.study_fk = st.id
         JOIN patients p ON st.patient_fk = p.id
         WHERE st.study_instance_uid = $1
           AND st.institution_id = $2
           AND st.storage_tier <> 'quarantine'
           AND p.institution_id = $2
           AND ($4 OR (se.source_status='trusted' AND EXISTS(
                SELECT 1 FROM dicom_devices d WHERE d.id=se.source_device_fk
                  AND d.status='active' AND EXISTS(
                    SELECT 1 FROM user_device_grants g WHERE g.user_fk=$3 AND g.device_fk=d.id))))
         ORDER BY se.series_number NULLS LAST, se.series_instance_uid",
    )
    .bind(study_uid)
    .bind(institution_id)
    .bind(user_id)
    .bind(is_admin)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(
                series_uid,
                series_number,
                modality,
                description,
                body_part_examined,
                protocol_name,
                instance_count,
            )| SeriesSummary {
                series_uid,
                series_number,
                modality,
                description,
                body_part_examined,
                protocol_name,
                instance_count,
            },
        )
        .collect())
}

fn contains_pattern(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_text_is_treated_literally() {
        assert_eq!(contains_pattern(r"A%_\B"), r"%A\%\_\\B%");
    }
}
