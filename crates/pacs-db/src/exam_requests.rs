//! 检查申请单与管理员工作量聚合。

use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use sqlx::{PgPool, Postgres, QueryBuilder};
use uuid::Uuid;

use crate::DbError;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ExamRequest {
    pub id: Uuid,
    pub patient_id: String,
    pub patient_name: String,
    pub patient_birth_date: Option<NaiveDate>,
    pub patient_sex: Option<String>,
    pub modality: String,
    pub body_part: String,
    pub request_type: String,
    pub clinical_indication: String,
    pub requested_by_id: i64,
    pub requested_by_name: String,
    pub requested_at: DateTime<Utc>,
    pub scheduled_at: Option<DateTime<Utc>>,
    pub status: String,
    pub study_uid: Option<String>,
    pub study_date: Option<NaiveDate>,
    pub study_description: Option<String>,
    pub revision: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ExamRequestInput<'a> {
    pub patient_id: &'a str,
    pub patient_name: &'a str,
    pub patient_birth_date: Option<NaiveDate>,
    pub patient_sex: Option<&'a str>,
    pub modality: &'a str,
    pub body_part: &'a str,
    pub request_type: &'a str,
    pub clinical_indication: &'a str,
    pub scheduled_at: Option<DateTime<Utc>>,
}

/// 为已入库检查开具申请单时可填写的请求信息。
///
/// 患者快照和目标 Study 一律从服务端的已入库检查读取，避免客户端把申请单
/// 绑定到另一位患者或另一家机构的检查。
#[derive(Debug, Clone)]
pub struct ExistingStudyExamRequestInput<'a> {
    pub modality: &'a str,
    pub body_part: &'a str,
    pub request_type: &'a str,
    pub clinical_indication: &'a str,
    pub scheduled_at: Option<DateTime<Utc>>,
}

const EXAM_REQUEST_SELECT: &str = r#"SELECT er.id,er.patient_id,er.patient_name,er.patient_birth_date,er.patient_sex,
              er.modality,er.body_part,er.request_type,er.clinical_indication,
              er.requested_by requested_by_id,COALESCE(u.display_name,u.username) requested_by_name,
              er.requested_at,er.scheduled_at,er.status,st.study_instance_uid study_uid,
              st.study_date,st.description study_description,er.revision,er.created_at,er.updated_at
       FROM exam_requests er JOIN users u ON u.id=er.requested_by
       LEFT JOIN studies st ON st.id=er.study_fk"#;

fn validate(input: &ExamRequestInput<'_>) -> Result<(), DbError> {
    for (label, value, max) in [
        ("患者 ID", input.patient_id, 64),
        ("患者姓名", input.patient_name, 256),
    ] {
        let len = value.trim().chars().count();
        if len == 0 || len > max {
            return Err(DbError::Invalid(format!(
                "{label}不能为空且不能超过 {max} 个字符"
            )));
        }
    }
    validate_request_fields(
        input.modality,
        input.body_part,
        input.request_type,
        input.clinical_indication,
    )?;
    if input
        .patient_sex
        .is_some_and(|value| value.trim().is_empty() || value.trim().chars().count() > 16)
    {
        return Err(DbError::Invalid("患者性别格式无效".to_owned()));
    }
    Ok(())
}

fn validate_existing_study_input(input: &ExistingStudyExamRequestInput<'_>) -> Result<(), DbError> {
    validate_request_fields(
        input.modality,
        input.body_part,
        input.request_type,
        input.clinical_indication,
    )
}

fn validate_request_fields(
    modality: &str,
    body_part: &str,
    request_type: &str,
    clinical_indication: &str,
) -> Result<(), DbError> {
    for (label, value, max) in [
        ("检查模态", modality, 16),
        ("检查部位", body_part, 128),
        ("检查类型", request_type, 64),
        ("临床指征", clinical_indication, 4096),
    ] {
        let len = value.trim().chars().count();
        if len == 0 || len > max {
            return Err(DbError::Invalid(format!(
                "{label}不能为空且不能超过 {max} 个字符"
            )));
        }
    }
    Ok(())
}

pub async fn list_exam_requests(
    pool: &PgPool,
    institution_id: i64,
    status: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<ExamRequest>, DbError> {
    if let Some(status) = status {
        if !matches!(status, "pending" | "executed" | "completed") {
            return Err(DbError::Invalid("申请单状态无效".to_owned()));
        }
    }
    let mut query = QueryBuilder::<Postgres>::new(EXAM_REQUEST_SELECT);
    query.push(" WHERE er.institution_id=");
    query.push_bind(institution_id);
    query.push(" AND (");
    query.push_bind(status);
    query.push("::TEXT IS NULL OR er.status=");
    query.push_bind(status);
    query.push(") ORDER BY er.requested_at DESC,er.id LIMIT ");
    query.push_bind(limit.clamp(1, 200));
    query.push(" OFFSET ");
    query.push_bind(offset.max(0));
    Ok(query.build_query_as().fetch_all(pool).await?)
}

pub async fn exam_request_for_study(
    pool: &PgPool,
    institution_id: i64,
    study_uid: &str,
) -> Result<Option<ExamRequest>, DbError> {
    let mut query = QueryBuilder::<Postgres>::new(EXAM_REQUEST_SELECT);
    query.push(" WHERE er.institution_id=");
    query.push_bind(institution_id);
    query.push(" AND st.study_instance_uid=");
    query.push_bind(study_uid);
    Ok(query.build_query_as().fetch_optional(pool).await?)
}

pub async fn create_exam_request(
    pool: &PgPool,
    institution_id: i64,
    requested_by: i64,
    input: ExamRequestInput<'_>,
) -> Result<ExamRequest, DbError> {
    validate(&input)?;
    let id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO exam_requests(
             id,institution_id,patient_id,patient_name,patient_birth_date,patient_sex,
             modality,body_part,request_type,clinical_indication,requested_by,scheduled_at)
           SELECT $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,u.id,$12
           FROM users u WHERE u.id=$11 AND u.institution_id=$2 AND u.is_active"#,
    )
    .bind(id)
    .bind(institution_id)
    .bind(input.patient_id.trim())
    .bind(input.patient_name.trim())
    .bind(input.patient_birth_date)
    .bind(
        input
            .patient_sex
            .map(str::trim)
            .filter(|value| !value.is_empty()),
    )
    .bind(input.modality.trim().to_uppercase())
    .bind(input.body_part.trim())
    .bind(input.request_type.trim())
    .bind(input.clinical_indication.trim())
    .bind(requested_by)
    .bind(input.scheduled_at)
    .execute(pool)
    .await?
    .rows_affected()
    .eq(&1)
    .then_some(())
    .ok_or(DbError::NotFound)?;
    exam_request_by_id(pool, institution_id, id).await
}

/// 为已经入库的 Study 开具申请单并立即关联。
///
/// 创建和绑定在一个数据库事务内完成，因此不会留下“已创建但未绑定”的中间状态。
pub async fn create_exam_request_for_study(
    pool: &PgPool,
    institution_id: i64,
    requested_by: i64,
    study_uid: &str,
    input: ExistingStudyExamRequestInput<'_>,
) -> Result<ExamRequest, DbError> {
    validate_existing_study_input(&input)?;
    let id = Uuid::new_v4();
    let mut tx = pool.begin().await?;
    let created: Option<Uuid> = sqlx::query_scalar(
        r#"INSERT INTO exam_requests(
             id,institution_id,patient_id,patient_name,patient_birth_date,patient_sex,
             modality,body_part,request_type,clinical_indication,requested_by,scheduled_at,
             status,study_fk)
           SELECT $1,$2,p.patient_id,COALESCE(NULLIF(btrim(p.name),''),p.patient_id),
                  p.birth_date,p.sex,$4,$5,$6,$7,u.id,$9,
                  CASE WHEN EXISTS(
                    SELECT 1 FROM diagnostic_reports r WHERE r.study_fk=st.id AND r.status='signed'
                  ) THEN 'completed' ELSE 'executed' END,
                  st.id
           FROM studies st
           JOIN patients p ON p.id=st.patient_fk
           JOIN users u ON u.id=$8 AND u.institution_id=$2 AND u.is_active
           WHERE st.institution_id=$2 AND st.study_instance_uid=$3 AND st.storage_tier<>'quarantine'
             AND EXISTS(SELECT 1 FROM series se JOIN instances i ON i.series_fk=se.id
                        WHERE se.study_fk=st.id)
           RETURNING id"#,
    )
    .bind(id)
    .bind(institution_id)
    .bind(study_uid)
    .bind(input.modality.trim().to_uppercase())
    .bind(input.body_part.trim())
    .bind(input.request_type.trim())
    .bind(input.clinical_indication.trim())
    .bind(requested_by)
    .bind(input.scheduled_at)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|error| match error {
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            DbError::Conflict("该检查已绑定其他申请单".to_owned())
        }
        other => DbError::from(other),
    })?;
    if created.is_none() {
        return Err(DbError::NotFound);
    }
    tx.commit().await?;
    exam_request_by_id(pool, institution_id, id).await
}

pub async fn update_exam_request(
    pool: &PgPool,
    institution_id: i64,
    request_id: Uuid,
    expected_revision: i32,
    input: ExamRequestInput<'_>,
) -> Result<ExamRequest, DbError> {
    validate(&input)?;
    let changed = sqlx::query(
        r#"UPDATE exam_requests SET patient_id=$4,patient_name=$5,patient_birth_date=$6,
                  patient_sex=$7,modality=$8,body_part=$9,request_type=$10,
                  clinical_indication=$11,scheduled_at=$12,revision=revision+1
           WHERE id=$1 AND institution_id=$2 AND revision=$3 AND status='pending'"#,
    )
    .bind(request_id)
    .bind(institution_id)
    .bind(expected_revision)
    .bind(input.patient_id.trim())
    .bind(input.patient_name.trim())
    .bind(input.patient_birth_date)
    .bind(
        input
            .patient_sex
            .map(str::trim)
            .filter(|value| !value.is_empty()),
    )
    .bind(input.modality.trim().to_uppercase())
    .bind(input.body_part.trim())
    .bind(input.request_type.trim())
    .bind(input.clinical_indication.trim())
    .bind(input.scheduled_at)
    .execute(pool)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(DbError::Conflict(
            "申请单已执行、版本已变化或不存在".to_owned(),
        ));
    }
    exam_request_by_id(pool, institution_id, request_id).await
}

pub async fn bind_exam_request(
    pool: &PgPool,
    institution_id: i64,
    request_id: Uuid,
    study_uid: &str,
    expected_revision: i32,
) -> Result<ExamRequest, DbError> {
    let changed = sqlx::query(
        r#"UPDATE exam_requests er SET study_fk=st.id,
                  status=CASE WHEN EXISTS(
                    SELECT 1 FROM diagnostic_reports r WHERE r.study_fk=st.id AND r.status='signed'
                  ) THEN 'completed' ELSE 'executed' END,
                  revision=er.revision+1
           FROM studies st
           WHERE er.id=$1 AND er.institution_id=$2 AND er.status='pending' AND er.revision=$3
             AND st.institution_id=$2 AND st.study_instance_uid=$4 AND st.storage_tier<>'quarantine'
             AND EXISTS(SELECT 1 FROM series se JOIN instances i ON i.series_fk=se.id
                        WHERE se.study_fk=st.id)"#,
    )
    .bind(request_id)
    .bind(institution_id)
    .bind(expected_revision)
    .bind(study_uid)
    .execute(pool)
    .await
    .map_err(|error| match error {
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            DbError::Conflict("该检查已绑定其他申请单".to_owned())
        }
        other => DbError::from(other),
    })?
    .rows_affected();
    if changed == 0 {
        return Err(DbError::Conflict(
            "申请单已执行、版本已变化，或检查不属于当前机构".to_owned(),
        ));
    }
    exam_request_by_id(pool, institution_id, request_id).await
}

async fn exam_request_by_id(
    pool: &PgPool,
    institution_id: i64,
    request_id: Uuid,
) -> Result<ExamRequest, DbError> {
    let mut query = QueryBuilder::<Postgres>::new(EXAM_REQUEST_SELECT);
    query.push(" WHERE er.institution_id=");
    query.push_bind(institution_id);
    query.push(" AND er.id=");
    query.push_bind(request_id);
    query
        .build_query_as()
        .fetch_optional(pool)
        .await?
        .ok_or(DbError::NotFound)
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ExamRequestStudyCandidate {
    pub study_uid: String,
    pub patient_id: String,
    pub patient_name: Option<String>,
    pub study_date: Option<NaiveDate>,
    pub modalities: Vec<String>,
    pub description: Option<String>,
}

pub async fn list_exam_request_study_candidates(
    pool: &PgPool,
    institution_id: i64,
    query: &str,
    limit: i64,
) -> Result<Vec<ExamRequestStudyCandidate>, DbError> {
    let pattern = format!("%{}%", query.trim());
    Ok(sqlx::query_as(
        r#"SELECT st.study_instance_uid study_uid,p.patient_id,p.name patient_name,st.study_date,
                  COALESCE(array_agg(DISTINCT se.modality) FILTER (WHERE se.modality IS NOT NULL),'{}') modalities,
                  st.description
           FROM studies st JOIN patients p ON p.id=st.patient_fk
           LEFT JOIN series se ON se.study_fk=st.id
           WHERE st.institution_id=$1
             AND NOT EXISTS(SELECT 1 FROM exam_requests er WHERE er.study_fk=st.id)
             AND st.storage_tier<>'quarantine'
             AND EXISTS(SELECT 1 FROM series available JOIN instances i ON i.series_fk=available.id
                        WHERE available.study_fk=st.id)
             AND ($2='' OR p.patient_id ILIKE $3 OR COALESCE(p.name,'') ILIKE $3
                  OR st.study_instance_uid ILIKE $3)
           GROUP BY st.id,p.id ORDER BY st.study_date DESC NULLS LAST,st.id DESC LIMIT $4"#,
    )
    .bind(institution_id)
    .bind(query.trim())
    .bind(pattern)
    .bind(limit.clamp(1, 100))
    .fetch_all(pool)
    .await?)
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct WorkloadRow {
    pub user_id: i64,
    pub username: String,
    pub display_name: Option<String>,
    pub role: String,
    pub draft_reports: i64,
    pub submitted_reports: i64,
    pub under_review_reports: i64,
    pub signed_status_reports: i64,
    pub signed_reports: i64,
    pub reviews_completed: i64,
    pub reviewer_modifications: i64,
    pub exam_requests_created: i64,
}

pub async fn workload_report(
    pool: &PgPool,
    institution_id: i64,
    date_from: NaiveDate,
    date_to: NaiveDate,
) -> Result<Vec<WorkloadRow>, DbError> {
    if date_from > date_to {
        return Err(DbError::Invalid("起始日期不能晚于结束日期".to_owned()));
    }
    Ok(sqlx::query_as(
        r#"WITH bounds AS (
             SELECT ($2::DATE::TIMESTAMP AT TIME ZONE i.timezone) started_at,
                    (($3::DATE + 1)::TIMESTAMP AT TIME ZONE i.timezone) ended_at
             FROM institutions i WHERE i.id=$1
           )
           SELECT u.id user_id,u.username,u.display_name,u.role,
             COUNT(DISTINCT r.id) FILTER (WHERE r.status='draft')::BIGINT draft_reports,
             COUNT(DISTINCT r.id) FILTER (WHERE r.status='submitted')::BIGINT submitted_reports,
             COUNT(DISTINCT r.id) FILTER (WHERE r.status='under_review')::BIGINT under_review_reports,
             COUNT(DISTINCT r.id) FILTER (WHERE r.status='signed')::BIGINT signed_status_reports,
             COUNT(DISTINCT v.id)::BIGINT signed_reports,
             COUNT(DISTINCT approved.id)::BIGINT reviews_completed,
             COUNT(DISTINCT modified.id)::BIGINT reviewer_modifications,
             COUNT(DISTINCT er.id)::BIGINT exam_requests_created
           FROM users u CROSS JOIN bounds b
           LEFT JOIN diagnostic_reports r ON r.author_fk=u.id
             AND r.created_at>=b.started_at AND r.created_at<b.ended_at
           LEFT JOIN diagnostic_report_versions v ON v.report_fk IN (
             SELECT id FROM diagnostic_reports WHERE author_fk=u.id
           ) AND v.signed_at>=b.started_at AND v.signed_at<b.ended_at
           LEFT JOIN report_review_events approved ON approved.actor_fk=u.id AND approved.action='approved'
             AND approved.created_at>=b.started_at AND approved.created_at<b.ended_at
           LEFT JOIN report_review_events modified ON modified.action='reviewer_modified'
             AND modified.report_fk IN (SELECT id FROM diagnostic_reports WHERE author_fk=u.id)
             AND modified.created_at>=b.started_at AND modified.created_at<b.ended_at
           LEFT JOIN exam_requests er ON er.requested_by=u.id
             AND er.requested_at>=b.started_at AND er.requested_at<b.ended_at
           WHERE u.institution_id=$1 AND u.role IN ('radiologist','technician')
           GROUP BY u.id ORDER BY u.role,u.display_name NULLS LAST,u.username"#,
    )
    .bind(institution_id)
    .bind(date_from)
    .bind(date_to)
    .fetch_all(pool)
    .await?)
}
