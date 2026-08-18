//! Viewer 工作列表查询。
//!
//! 这些查询返回应用 JSON，而不是 DICOM 标识符。每个入口都要求机构 ID，
//! 防止调用方忘记租户边界。

use chrono::{NaiveDate, NaiveTime};
use serde::Serialize;
use sqlx::{PgPool, Postgres, QueryBuilder};

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
    i64,
    i64,
    i64,
    i64,
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
    String,
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
    /// 报告状态计数（按检查）：待书写 / 我书写中 / 他人锁定 / 已签发。
    pub pending_studies: i64,
    pub writing_studies: i64,
    pub locked_studies: i64,
    pub signed_studies: i64,
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
    /// 报告状态：pending（待书写）| writing（书写中）| signed（已签发）。
    pub report_status: String,
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

/// 高级队列查询的可选过滤条件。
///
/// 字符串切片由 HTTP 层或调用方持有，查询本身不复制过滤条件。`query` 使用
/// 患者 ID 和规范化姓名的字面量包含匹配；其余字段都是精确匹配。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QueueFilter<'a> {
    pub query: &'a str,
    pub modality: Option<&'a str>,
    pub body_part: Option<&'a str>,
    pub report_status: Option<&'a str>,
    pub institution: Option<&'a str>,
    pub date_from: Option<NaiveDate>,
    pub date_to: Option<NaiveDate>,
}

/// 队列表允许的排序列。
///
/// 这个枚举在进入 SQL 之前解析完成，避免把 URL 中的排序字段直接拼进查询。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum QueueSort {
    #[default]
    StudyDate,
    PatientName,
    Modality,
    ReportStatus,
    Institution,
}

impl QueueSort {
    /// URL 使用的稳定名称。HTTP 层负责将用户输入解析为这个枚举。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StudyDate => "study_date",
            Self::PatientName => "patient_name",
            Self::Modality => "modality",
            Self::ReportStatus => "report_status",
            Self::Institution => "institution",
        }
    }
}

/// 高级队列中的一行（一个检查）。
#[derive(Debug, Clone, Serialize, PartialEq, sqlx::FromRow)]
pub struct QueueStudyRow {
    /// 机构内患者主键，仅用于继续查询该患者的检查列表。
    pub patient_key: i64,
    pub study_uid: String,
    pub patient_id: String,
    pub patient_name: Option<String>,
    pub patient_sex: Option<String>,
    pub patient_birth_date: Option<NaiveDate>,
    pub study_date: Option<NaiveDate>,
    pub study_time: Option<NaiveTime>,
    pub modalities: Vec<String>,
    pub description: Option<String>,
    pub body_parts: Vec<String>,
    pub report_status: String,
    pub institution_name: Option<String>,
    /// 管理员为检查的全部序列，普通用户为其可见序列。
    pub series_count: i32,
}

/// 按检查列出当前用户可见的高级队列。
///
/// `is_admin` 只影响序列来源可见性；机构边界始终由 `institution_id` 约束。
/// 所有动态值都通过 `QueryBuilder::push_bind` 绑定，唯一直接进入 SQL 的值是
/// [`QueueSort`] 提供的静态排序表达式和方向。
#[allow(clippy::too_many_arguments)]
pub async fn list_queue_studies(
    pool: &PgPool,
    institution_id: i64,
    user_id: i64,
    is_admin: bool,
    filter: QueueFilter<'_>,
    sort: QueueSort,
    descending: bool,
    limit: i64,
    offset: i64,
) -> Result<Vec<QueueStudyRow>, DbError> {
    let normalized = pacs_core::normalize_person_name(filter.query.trim());
    let id_pattern = contains_pattern(filter.query.trim());
    let name_pattern = contains_pattern(&normalized);

    let mut query = QueryBuilder::<Postgres>::new(
        r#"SELECT p.id AS patient_key,
                  st.study_instance_uid AS study_uid,
                  p.patient_id,
                  p.name AS patient_name,
                  p.sex AS patient_sex,
                  p.birth_date AS patient_birth_date,
                  st.study_date,
                  st.study_time,
                  COALESCE(
                      array_agg(DISTINCT se.modality ORDER BY se.modality)
                          FILTER (WHERE se.modality IS NOT NULL),
                      ARRAY[]::TEXT[]
                  ) AS modalities,
                  st.description,
                  COALESCE(
                      array_agg(DISTINCT se.body_part_examined ORDER BY se.body_part_examined)
                          FILTER (WHERE se.body_part_examined IS NOT NULL),
                      ARRAY[]::TEXT[]
                  ) AS body_parts,
                  CASE WHEN r.id IS NULL THEN 'pending'
                       WHEN r.status = 'signed' THEN 'signed'
                       WHEN r.status = 'submitted' THEN 'submitted'
                       WHEN r.status = 'under_review' THEN 'under_review'
                       WHEN r.author_fk = "#,
    );
    query.push_bind(user_id);
    query.push(
        r#" THEN 'writing'
                       ELSE 'locked' END AS report_status,
                  st.attributes->'00080080'->'Value'->>0 AS institution_name,
                  COUNT(DISTINCT se.id)::INTEGER AS series_count
           FROM studies st
           JOIN patients p ON p.id = st.patient_fk
           JOIN series se ON se.study_fk = st.id
           LEFT JOIN diagnostic_reports r
                  ON r.study_fk = st.id AND r.institution_id = st.institution_id
           WHERE st.institution_id = "#,
    );
    query.push_bind(institution_id);
    query.push(r#" AND p.institution_id = "#);
    query.push_bind(institution_id);
    query.push(" AND st.storage_tier <> 'quarantine'");

    // A non-admin sees a study only when at least one of its series comes from
    // an active, trusted device granted to that user. Filtering the joined
    // series also keeps modality/body-part aggregation and series_count within
    // the visible set.
    if !is_admin {
        query.push(
            r#" AND se.source_status = 'trusted'
               AND EXISTS (
                   SELECT 1 FROM dicom_devices d
                   WHERE d.id = se.source_device_fk
                     AND d.institution_id = st.institution_id
                     AND d.status = 'active'
                     AND EXISTS (
                         SELECT 1 FROM user_device_grants g
                         WHERE g.user_fk = "#,
        );
        query.push_bind(user_id);
        query.push(" AND g.device_fk = d.id))");
    }

    query.push(
        r#" AND (
               "#,
    );
    query.push_bind(filter.query.trim());
    query.push(
        r#" = ''
               OR p.patient_id ILIKE "#,
    );
    query.push_bind(&id_pattern);
    query.push(
        r#" ESCAPE '\'
               OR p.name_normalized LIKE "#,
    );
    query.push_bind(&name_pattern);
    query.push(r#" ESCAPE '\')"#);

    if let Some(modality) = filter
        .modality
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if is_admin {
            // Administrators see every series, so the maintained study-level
            // aggregate can use the existing GIN index.
            query.push(" AND st.modalities @> ARRAY[");
            query.push_bind(modality);
            query.push("]::TEXT[]");
        } else {
            query.push(
                r#" AND EXISTS (
                       SELECT 1 FROM series modality_filter
                       WHERE modality_filter.study_fk = st.id
                         AND modality_filter.modality = "#,
            );
            query.push_bind(modality);
            query.push(
                r#" AND modality_filter.source_status = 'trusted'
                         AND EXISTS (
                             SELECT 1 FROM dicom_devices modality_device
                             WHERE modality_device.id = modality_filter.source_device_fk
                               AND modality_device.institution_id = st.institution_id
                               AND modality_device.status = 'active'
                               AND EXISTS (
                                   SELECT 1 FROM user_device_grants modality_grant
                                   WHERE modality_grant.user_fk = "#,
            );
            query.push_bind(user_id);
            query.push(" AND modality_grant.device_fk = modality_device.id))");
            query.push(")");
        }
    }
    if let Some(body_part) = filter
        .body_part
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        query.push(
            r#" AND EXISTS (
                       SELECT 1 FROM series body_filter
                       WHERE body_filter.study_fk = st.id
                         AND body_filter.body_part_examined = "#,
        );
        query.push_bind(body_part);
        if !is_admin {
            query.push(
                r#" AND body_filter.source_status = 'trusted'
                         AND EXISTS (
                             SELECT 1 FROM dicom_devices body_device
                             WHERE body_device.id = body_filter.source_device_fk
                               AND body_device.institution_id = st.institution_id
                               AND body_device.status = 'active'
                               AND EXISTS (
                                   SELECT 1 FROM user_device_grants body_grant
                                   WHERE body_grant.user_fk = "#,
            );
            query.push_bind(user_id);
            query.push(" AND body_grant.device_fk = body_device.id))");
        }
        query.push(")");
    }
    if let Some(institution) = filter
        .institution
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        query.push(" AND st.attributes->'00080080'->'Value'->>0 = ");
        query.push_bind(institution);
    }
    if let Some(date_from) = filter.date_from {
        query.push(" AND st.study_date >= ");
        query.push_bind(date_from);
    }
    if let Some(date_to) = filter.date_to {
        query.push(" AND st.study_date <= ");
        query.push_bind(date_to);
    }
    if let Some(report_status) = filter
        .report_status
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        query.push(" AND ");
        push_report_status_case(&mut query, user_id);
        query.push(" = ");
        query.push_bind(report_status);
    }

    query.push(" GROUP BY st.id, p.id, r.id, st.attributes ORDER BY ");
    match sort {
        QueueSort::StudyDate => {
            query.push("st.study_date");
        }
        QueueSort::PatientName => {
            query.push("p.name_normalized");
        }
        QueueSort::Modality => {
            query.push(if is_admin {
                "st.modalities[1]"
            } else {
                "MIN(se.modality)"
            });
        }
        QueueSort::ReportStatus => {
            push_report_status_case(&mut query, user_id);
        }
        QueueSort::Institution => {
            query.push("st.attributes->'00080080'->'Value'->>0");
        }
    }
    query.push(if descending { " DESC" } else { " ASC" });
    query.push(" NULLS LAST, st.study_instance_uid ASC LIMIT ");
    query.push_bind(limit);
    query.push(" OFFSET ");
    query.push_bind(offset);

    Ok(query
        .build_query_as::<QueueStudyRow>()
        .fetch_all(pool)
        .await?)
}

/// Append the six-state report expression. Each occurrence binds the current
/// user independently because `QueryBuilder` numbers parameters as it builds.
fn push_report_status_case(query: &mut QueryBuilder<Postgres>, user_id: i64) {
    query.push("CASE WHEN r.id IS NULL THEN 'pending' WHEN r.status = 'signed' THEN 'signed' WHEN r.status = 'submitted' THEN 'submitted' WHEN r.status = 'under_review' THEN 'under_review' WHEN r.author_fk = ");
    query.push_bind(user_id);
    query.push(" THEN 'writing' ELSE 'locked' END");
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
                MAX(st.study_date),
                COUNT(DISTINCT st.id) FILTER (WHERE r.id IS NULL)::BIGINT,
                COUNT(DISTINCT st.id) FILTER (WHERE r.status IN ('draft','amending') AND r.author_fk = $2)::BIGINT,
                COUNT(DISTINCT st.id) FILTER (WHERE r.status IN ('draft','amending') AND r.author_fk <> $2)::BIGINT,
                COUNT(DISTINCT st.id) FILTER (WHERE r.status = 'signed')::BIGINT
         FROM patients p
         JOIN studies st ON st.patient_fk = p.id AND st.institution_id = $1
              AND st.storage_tier <> 'quarantine'
         JOIN series se ON se.study_fk=st.id
         LEFT JOIN dicom_devices d ON d.id=se.source_device_fk
         LEFT JOIN diagnostic_reports r ON r.study_fk=st.id
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
                pending_studies,
                writing_studies,
                locked_studies,
                signed_studies,
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
                pending_studies,
                writing_studies,
                locked_studies,
                signed_studies,
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
                st.number_of_series, st.number_of_instances,
                CASE WHEN r.id IS NULL THEN 'pending'
                     WHEN r.status = 'signed' THEN 'signed'
                     ELSE 'writing' END
         FROM studies st
         JOIN patients p ON st.patient_fk = p.id
         LEFT JOIN diagnostic_reports r ON r.study_fk = st.id
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
                report_status,
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
                report_status,
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
