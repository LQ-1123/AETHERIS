//! Device-scoped clinical access, diagnostic work queue and reports.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::DbError;

pub async fn institution_today(pool: &PgPool, institution_id: i64) -> Result<NaiveDate, DbError> {
    sqlx::query_scalar("SELECT (now() AT TIME ZONE timezone)::date FROM institutions WHERE id=$1")
        .bind(institution_id)
        .fetch_optional(pool)
        .await?
        .ok_or(DbError::NotFound)
}

pub async fn can_access_series(
    pool: &PgPool,
    institution_id: i64,
    user_id: i64,
    is_admin: bool,
    series_uid: &str,
) -> Result<bool, DbError> {
    if is_admin {
        return Ok(true);
    }
    Ok(sqlx::query_scalar(
        r#"SELECT EXISTS(
             SELECT 1 FROM series se JOIN studies st ON st.id=se.study_fk
             JOIN dicom_devices d ON d.id=se.source_device_fk AND d.status='active'
             JOIN user_device_grants g ON g.device_fk=d.id AND g.user_fk=$2
             WHERE st.institution_id=$1 AND se.series_instance_uid=$3
               AND se.source_status='trusted')"#,
    )
    .bind(institution_id)
    .bind(user_id)
    .bind(series_uid)
    .fetch_one(pool)
    .await?)
}

pub async fn can_access_study(
    pool: &PgPool,
    institution_id: i64,
    user_id: i64,
    is_admin: bool,
    study_uid: &str,
) -> Result<bool, DbError> {
    if is_admin {
        return Ok(true);
    }
    Ok(sqlx::query_scalar(
        r#"SELECT EXISTS(
             SELECT 1 FROM studies st JOIN series se ON se.study_fk=st.id
             JOIN dicom_devices d ON d.id=se.source_device_fk AND d.status='active'
             JOIN user_device_grants g ON g.device_fk=d.id AND g.user_fk=$2
             WHERE st.institution_id=$1 AND st.study_instance_uid=$3
               AND se.source_status='trusted')"#,
    )
    .bind(institution_id)
    .bind(user_id)
    .bind(study_uid)
    .fetch_one(pool)
    .await?)
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct DicomDevice {
    pub id: Uuid,
    pub institution_id: i64,
    pub name: String,
    pub calling_ae_title: String,
    pub source_ip: String,
    pub modality_hint: Option<String>,
    pub status: String,
    pub first_seen_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub approved_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApproveDevice<'a> {
    pub name: &'a str,
    pub modality_hint: Option<&'a str>,
}

pub async fn observe_device(
    pool: &PgPool,
    institution_id: i64,
    ae_title: &str,
    source_ip: &str,
) -> Result<DicomDevice, DbError> {
    Ok(sqlx::query_as(
        r#"INSERT INTO dicom_devices
           (id,institution_id,name,calling_ae_title,source_ip)
           VALUES ($1,$2,$3,$4,$5)
           ON CONFLICT (institution_id,calling_ae_title,source_ip) DO UPDATE
             SET last_seen_at=now()
           RETURNING id,institution_id,name,calling_ae_title,source_ip,modality_hint,status,
                     first_seen_at,last_seen_at,approved_at"#,
    )
    .bind(Uuid::new_v4())
    .bind(institution_id)
    .bind(ae_title.trim())
    .bind(ae_title.trim())
    .bind(source_ip)
    .fetch_one(pool)
    .await?)
}

pub async fn list_devices(
    pool: &PgPool,
    institution_id: i64,
    status: Option<&str>,
) -> Result<Vec<DicomDevice>, DbError> {
    Ok(sqlx::query_as(
        r#"SELECT id,institution_id,name,calling_ae_title,source_ip,modality_hint,status,
                  first_seen_at,last_seen_at,approved_at
           FROM dicom_devices WHERE institution_id=$1 AND ($2::TEXT IS NULL OR status=$2)
           ORDER BY status,name,calling_ae_title,source_ip"#,
    )
    .bind(institution_id)
    .bind(status)
    .fetch_all(pool)
    .await?)
}

pub async fn approve_device(
    pool: &PgPool,
    institution_id: i64,
    id: Uuid,
    input: ApproveDevice<'_>,
    actor_id: i64,
) -> Result<DicomDevice, DbError> {
    let mut tx = pool.begin().await?;
    let device = sqlx::query_as(
        r#"UPDATE dicom_devices SET name=$3,modality_hint=$4,status='active',
                  approved_by=$5,approved_at=COALESCE(approved_at,now())
           WHERE institution_id=$1 AND id=$2
           RETURNING id,institution_id,name,calling_ae_title,source_ip,modality_hint,status,
                     first_seen_at,last_seen_at,approved_at"#,
    )
    .bind(institution_id)
    .bind(id)
    .bind(input.name.trim())
    .bind(input.modality_hint.map(str::trim))
    .bind(actor_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(DbError::NotFound)?;
    sqlx::query(
        "UPDATE series SET source_status='trusted' WHERE source_device_fk=$1 AND source_status='pending'",
    ).bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(device)
}

pub async fn set_device_status(
    pool: &PgPool,
    institution_id: i64,
    id: Uuid,
    status: &str,
) -> Result<(), DbError> {
    let changed =
        sqlx::query("UPDATE dicom_devices SET status=$3 WHERE institution_id=$1 AND id=$2")
            .bind(institution_id)
            .bind(id)
            .bind(status)
            .execute(pool)
            .await?
            .rows_affected();
    if changed == 0 {
        Err(DbError::NotFound)
    } else {
        Ok(())
    }
}

pub async fn user_device_grants(
    pool: &PgPool,
    institution_id: i64,
    user_id: i64,
) -> Result<Vec<Uuid>, DbError> {
    Ok(sqlx::query_scalar(
        r#"SELECT g.device_fk FROM user_device_grants g
           JOIN users u ON u.id=g.user_fk
           JOIN dicom_devices d ON d.id=g.device_fk
           WHERE u.id=$1 AND u.institution_id=$2 AND d.institution_id=$2
           ORDER BY g.device_fk"#,
    )
    .bind(user_id)
    .bind(institution_id)
    .fetch_all(pool)
    .await?)
}

pub async fn replace_user_device_grants(
    pool: &PgPool,
    institution_id: i64,
    user_id: i64,
    device_ids: &[Uuid],
    actor_id: i64,
) -> Result<Vec<Uuid>, DbError> {
    let mut tx = pool.begin().await?;
    let owns_user: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id=$1 AND institution_id=$2)")
            .bind(user_id)
            .bind(institution_id)
            .fetch_one(&mut *tx)
            .await?;
    if !owns_user {
        return Err(DbError::NotFound);
    }
    let valid: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM dicom_devices WHERE institution_id=$1 AND id=ANY($2)",
    )
    .bind(institution_id)
    .bind(device_ids)
    .fetch_one(&mut *tx)
    .await?;
    if valid != device_ids.len() as i64 {
        return Err(DbError::Invalid("设备不属于当前机构".to_owned()));
    }
    sqlx::query("DELETE FROM user_device_grants WHERE user_fk=$1")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    for device_id in device_ids {
        sqlx::query(
            "INSERT INTO user_device_grants(user_fk,device_fk,granted_by) VALUES($1,$2,$3)",
        )
        .bind(user_id)
        .bind(device_id)
        .bind(actor_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(device_ids.to_vec())
}

/// Attach the immutable first origin and initialize the Series work item.
pub async fn record_dimse_origin(
    pool: &PgPool,
    institution_id: i64,
    sop_uid: &str,
    ae_title: &str,
    source_ip: &str,
) -> Result<(), DbError> {
    let device = observe_device(pool, institution_id, ae_title, source_ip).await?;
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        r#"SELECT i.id instance_id,se.id series_id,se.source_device_fk,se.source_status
           FROM instances i JOIN series se ON se.id=i.series_fk
           JOIN studies st ON st.id=se.study_fk
           WHERE i.sop_instance_uid=$1 AND st.institution_id=$2"#,
    )
    .bind(sop_uid)
    .bind(institution_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(DbError::NotFound)?;
    let instance_id: i64 = row.try_get("instance_id")?;
    let series_id: i64 = row.try_get("series_id")?;
    let prior: Option<Uuid> = row.try_get("source_device_fk")?;
    sqlx::query(
        r#"INSERT INTO dicom_instance_origins
           (instance_fk,device_fk,calling_ae_title,source_ip,ingress_kind)
           VALUES($1,$2,$3,$4,'dimse') ON CONFLICT(instance_fk) DO NOTHING"#,
    )
    .bind(instance_id)
    .bind(device.id)
    .bind(ae_title)
    .bind(source_ip)
    .execute(&mut *tx)
    .await?;
    let source_status = if device.status == "active" {
        "trusted"
    } else {
        "pending"
    };
    if let Some(prior) = prior {
        if prior != device.id {
            sqlx::query("UPDATE series SET source_status='needs_review' WHERE id=$1")
                .bind(series_id)
                .execute(&mut *tx)
                .await?;
        }
    } else {
        sqlx::query("UPDATE series SET source_device_fk=$2,source_status=$3 WHERE id=$1")
            .bind(series_id)
            .bind(device.id)
            .bind(source_status)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query(
        r#"INSERT INTO diagnostic_work_items(id,institution_id,series_fk)
           VALUES($1,$2,$3) ON CONFLICT(institution_id,series_fk) DO NOTHING"#,
    )
    .bind(Uuid::new_v4())
    .bind(institution_id)
    .bind(series_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn resolve_series_source(
    pool: &PgPool,
    institution_id: i64,
    series_uid: &str,
    device_id: Uuid,
) -> Result<(), DbError> {
    let changed = sqlx::query(
        r#"UPDATE series se SET source_device_fk=$3,source_status='trusted'
           FROM studies st,dicom_devices d
           WHERE se.study_fk=st.id AND st.institution_id=$1 AND se.series_instance_uid=$2
             AND d.id=$3 AND d.institution_id=$1 AND d.status='active'"#,
    )
    .bind(institution_id)
    .bind(series_uid)
    .bind(device_id)
    .execute(pool)
    .await?
    .rows_affected();
    if changed == 0 {
        Err(DbError::NotFound)
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ClinicalWorkItem {
    pub id: Uuid,
    pub series_uid: String,
    pub study_uid: String,
    pub patient_id: String,
    pub patient_name: Option<String>,
    pub modality: Option<String>,
    pub series_description: Option<String>,
    pub device_id: Uuid,
    pub device_name: String,
    pub received_date: NaiveDate,
    pub status: String,
    pub assignee_id: Option<i64>,
    pub assignee_name: Option<String>,
    pub revision: i32,
}

pub async fn list_clinical_work(
    pool: &PgPool,
    institution_id: i64,
    user_id: i64,
    is_admin: bool,
    date: NaiveDate,
    status: Option<&str>,
) -> Result<Vec<ClinicalWorkItem>, DbError> {
    Ok(sqlx::query_as(
        r#"SELECT w.id,se.series_instance_uid series_uid,st.study_instance_uid study_uid,
                  p.patient_id,p.name patient_name,se.modality,se.description series_description,
                  d.id device_id,d.name device_name,(MIN(i.received_at) AT TIME ZONE ins.timezone)::date received_date,
                  w.status,w.assignee_fk assignee_id,u.display_name assignee_name,w.revision
           FROM diagnostic_work_items w JOIN series se ON se.id=w.series_fk
           JOIN studies st ON st.id=se.study_fk JOIN patients p ON p.id=st.patient_fk
           JOIN institutions ins ON ins.id=w.institution_id
           JOIN dicom_devices d ON d.id=se.source_device_fk AND d.status='active'
           JOIN instances i ON i.series_fk=se.id LEFT JOIN users u ON u.id=w.assignee_fk
           WHERE w.institution_id=$1 AND se.source_status='trusted'
             AND ($3 OR EXISTS(SELECT 1 FROM user_device_grants g
                               WHERE g.user_fk=$2 AND g.device_fk=d.id))
             AND ($5::TEXT IS NULL OR w.status=$5)
           GROUP BY w.id,se.id,st.id,p.id,d.id,u.id,ins.timezone
           HAVING (MIN(i.received_at) AT TIME ZONE ins.timezone)::date=$4
           ORDER BY MIN(i.received_at),w.id"#,
    ).bind(institution_id).bind(user_id).bind(is_admin).bind(date).bind(status)
      .fetch_all(pool).await?)
}

pub async fn claim_work_item(
    pool: &PgPool,
    institution_id: i64,
    work_id: Uuid,
    user_id: i64,
    expected_revision: i32,
) -> Result<(), DbError> {
    let changed = sqlx::query(
        r#"UPDATE diagnostic_work_items w SET status='claimed',assignee_fk=$3,
                  claimed_at=now(),revision=revision+1
           FROM series se,dicom_devices d
           WHERE w.id=$2 AND w.institution_id=$1 AND w.series_fk=se.id
             AND se.source_device_fk=d.id AND se.source_status='trusted' AND d.status='active'
             AND w.status='pending' AND w.revision=$4
             AND EXISTS(SELECT 1 FROM user_device_grants g
                        WHERE g.user_fk=$3 AND g.device_fk=d.id)"#,
    )
    .bind(institution_id)
    .bind(work_id)
    .bind(user_id)
    .bind(expected_revision)
    .execute(pool)
    .await?
    .rows_affected();
    if changed == 0 {
        Err(DbError::Conflict("工作项已被领取或版本已变化".to_owned()))
    } else {
        Ok(())
    }
}

pub async fn release_work_item(
    pool: &PgPool,
    institution_id: i64,
    work_id: Uuid,
    actor_id: i64,
    is_admin: bool,
    expected_revision: i32,
) -> Result<(), DbError> {
    let changed = sqlx::query(
        r#"UPDATE diagnostic_work_items SET status='pending',assignee_fk=NULL,claimed_at=NULL,
                  revision=revision+1
           WHERE id=$1 AND institution_id=$2 AND revision=$3
             AND status IN ('claimed','reporting') AND ($5 OR assignee_fk=$4)
             AND NOT EXISTS(SELECT 1 FROM diagnostic_reports r
                            JOIN diagnostic_report_series rs ON rs.report_fk=r.id
                            WHERE rs.series_fk=diagnostic_work_items.series_fk
                              AND r.status IN ('draft','amending'))"#,
    )
    .bind(work_id)
    .bind(institution_id)
    .bind(expected_revision)
    .bind(actor_id)
    .bind(is_admin)
    .execute(pool)
    .await?
    .rows_affected();
    if changed == 0 {
        Err(DbError::Conflict(
            "工作项不可释放、存在草稿或版本已变化".to_owned(),
        ))
    } else {
        Ok(())
    }
}

pub async fn assign_work_item(
    pool: &PgPool,
    institution_id: i64,
    work_id: Uuid,
    doctor_id: i64,
    expected_revision: i32,
) -> Result<(), DbError> {
    let changed = sqlx::query(
        r#"UPDATE diagnostic_work_items w SET status='claimed',assignee_fk=$3,claimed_at=now(),
                  revision=revision+1
           FROM series se,dicom_devices d,users u
           WHERE w.id=$1 AND w.institution_id=$2 AND w.revision=$4 AND w.series_fk=se.id
             AND se.source_device_fk=d.id AND se.source_status='trusted' AND d.status='active'
             AND u.id=$3 AND u.institution_id=$2 AND u.role='radiologist' AND u.is_active
             AND EXISTS(SELECT 1 FROM user_device_grants g WHERE g.user_fk=u.id AND g.device_fk=d.id)
             AND NOT EXISTS(SELECT 1 FROM diagnostic_reports r
                 JOIN diagnostic_report_series rs ON rs.report_fk=r.id
                 WHERE rs.series_fk=se.id AND r.status IN ('draft','amending'))"#,
    ).bind(work_id).bind(institution_id).bind(doctor_id).bind(expected_revision)
      .execute(pool).await?.rows_affected();
    if changed == 0 {
        Err(DbError::Conflict("无法转派给该医生或版本已变化".to_owned()))
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct DiagnosticReport {
    pub id: Uuid,
    pub study_uid: String,
    pub author_id: i64,
    pub status: String,
    pub findings: String,
    pub impression: String,
    pub recommendation: Option<String>,
    pub revision: i32,
    pub access_incomplete: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub async fn create_report(
    pool: &PgPool,
    institution_id: i64,
    author_id: i64,
    study_uid: &str,
    series_uids: &[String],
) -> Result<DiagnosticReport, DbError> {
    if series_uids.is_empty() {
        return Err(DbError::Invalid("报告至少覆盖一个序列".to_owned()));
    }
    let mut tx = pool.begin().await?;
    let study_id: i64 = sqlx::query_scalar(
        "SELECT id FROM studies WHERE institution_id=$1 AND study_instance_uid=$2",
    )
    .bind(institution_id)
    .bind(study_uid)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(DbError::NotFound)?;
    let rows = sqlx::query(
        r#"SELECT se.id,w.assignee_fk FROM series se
           JOIN diagnostic_work_items w ON w.series_fk=se.id AND w.institution_id=$1
           JOIN dicom_devices d ON d.id=se.source_device_fk AND d.status='active'
           JOIN user_device_grants g ON g.device_fk=d.id AND g.user_fk=$3
           WHERE se.study_fk=$2 AND se.series_instance_uid=ANY($4)
             AND se.source_status='trusted' AND w.assignee_fk=$3
             AND w.status IN ('claimed','reporting')"#,
    )
    .bind(institution_id)
    .bind(study_id)
    .bind(author_id)
    .bind(series_uids)
    .fetch_all(&mut *tx)
    .await?;
    if rows.len() != series_uids.len() {
        return Err(DbError::Invalid(
            "序列未全部获权或未由当前医生领取".to_owned(),
        ));
    }
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM series WHERE study_fk=$1")
        .bind(study_id)
        .fetch_one(&mut *tx)
        .await?;
    let id = Uuid::new_v4();
    let report: DiagnosticReport = sqlx::query_as(
        r#"INSERT INTO diagnostic_reports(id,institution_id,study_fk,author_fk,access_incomplete)
           VALUES($1,$2,$3,$4,$5)
           RETURNING id,$6::TEXT study_uid,author_fk author_id,status,findings,impression,
                     recommendation,revision,access_incomplete,created_at,updated_at"#,
    )
    .bind(id)
    .bind(institution_id)
    .bind(study_id)
    .bind(author_id)
    .bind(total != series_uids.len() as i64)
    .bind(study_uid)
    .fetch_one(&mut *tx)
    .await?;
    for row in rows {
        let series_id: i64 = row.try_get("id")?;
        sqlx::query("INSERT INTO diagnostic_report_series(report_fk,series_fk) VALUES($1,$2)")
            .bind(id)
            .bind(series_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE diagnostic_work_items SET status='reporting',revision=revision+1 WHERE series_fk=$1")
            .bind(series_id).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(report)
}

pub async fn list_reports(
    pool: &PgPool,
    institution_id: i64,
    user_id: i64,
    is_admin: bool,
    study_uid: &str,
) -> Result<Vec<DiagnosticReport>, DbError> {
    Ok(sqlx::query_as(
        r#"SELECT r.id,st.study_instance_uid study_uid,r.author_fk author_id,r.status,
                  r.findings,r.impression,r.recommendation,r.revision,r.access_incomplete,
                  r.created_at,r.updated_at
           FROM diagnostic_reports r JOIN studies st ON st.id=r.study_fk
           WHERE r.institution_id=$1 AND st.study_instance_uid=$2
             AND ($4 OR r.author_fk=$3 OR EXISTS(
               SELECT 1 FROM diagnostic_report_series rs JOIN series se ON se.id=rs.series_fk
               JOIN dicom_devices d ON d.id=se.source_device_fk AND d.status='active'
               JOIN user_device_grants g ON g.device_fk=d.id AND g.user_fk=$3
               WHERE rs.report_fk=r.id))
           ORDER BY r.updated_at DESC,r.id"#,
    )
    .bind(institution_id)
    .bind(study_uid)
    .bind(user_id)
    .bind(is_admin)
    .fetch_all(pool)
    .await?)
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ReportVersion {
    pub id: Uuid,
    pub report_id: Uuid,
    pub version_number: i32,
    pub findings: String,
    pub impression: String,
    pub recommendation: Option<String>,
    pub covered_series_uids: Vec<String>,
    pub access_incomplete: bool,
    pub amendment_reason: Option<String>,
    pub signed_by: i64,
    pub signed_at: DateTime<Utc>,
}

pub async fn list_report_versions(
    pool: &PgPool,
    institution_id: i64,
    report_id: Uuid,
    user_id: i64,
    is_admin: bool,
) -> Result<Vec<ReportVersion>, DbError> {
    Ok(sqlx::query_as(
        r#"SELECT v.id,v.report_fk report_id,v.version_number,v.findings,v.impression,
                  v.recommendation,v.covered_series_uids,v.access_incomplete,
                  v.amendment_reason,v.signed_by,v.signed_at
           FROM diagnostic_report_versions v JOIN diagnostic_reports r ON r.id=v.report_fk
           WHERE v.report_fk=$1 AND r.institution_id=$2 AND ($4 OR r.author_fk=$3 OR EXISTS(
             SELECT 1 FROM diagnostic_report_series rs JOIN series se ON se.id=rs.series_fk
             JOIN dicom_devices d ON d.id=se.source_device_fk AND d.status='active'
             JOIN user_device_grants g ON g.device_fk=d.id AND g.user_fk=$3
             WHERE rs.report_fk=r.id)) ORDER BY v.version_number"#,
    )
    .bind(report_id)
    .bind(institution_id)
    .bind(user_id)
    .bind(is_admin)
    .fetch_all(pool)
    .await?)
}

pub async fn begin_report_amendment(
    pool: &PgPool,
    institution_id: i64,
    report_id: Uuid,
    author_id: i64,
    reason: &str,
) -> Result<DiagnosticReport, DbError> {
    if reason.trim().is_empty() {
        return Err(DbError::Invalid("修订原因不能为空".to_owned()));
    }
    sqlx::query_as(
        r#"UPDATE diagnostic_reports r SET status='amending',author_fk=$3,
                  pending_amendment_reason=$4,revision=revision+1
           FROM studies st WHERE r.id=$1 AND r.institution_id=$2 AND r.status='signed'
             AND st.id=r.study_fk
             AND NOT EXISTS(
               SELECT 1 FROM diagnostic_report_series rs JOIN series se ON se.id=rs.series_fk
               JOIN dicom_devices d ON d.id=se.source_device_fk
               WHERE rs.report_fk=r.id AND (se.source_status<>'trusted' OR d.status<>'active'
                 OR NOT EXISTS(SELECT 1 FROM user_device_grants g
                               WHERE g.user_fk=$3 AND g.device_fk=d.id)))
           RETURNING r.id,st.study_instance_uid study_uid,r.author_fk author_id,r.status,
                     r.findings,r.impression,r.recommendation,r.revision,r.access_incomplete,
                     r.created_at,r.updated_at"#,
    )
    .bind(report_id)
    .bind(institution_id)
    .bind(author_id)
    .bind(reason.trim())
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DbError::Conflict("报告当前不可修订".to_owned()))
}

#[allow(clippy::too_many_arguments)]
pub async fn update_report_draft(
    pool: &PgPool,
    institution_id: i64,
    report_id: Uuid,
    author_id: i64,
    revision: i32,
    findings: &str,
    impression: &str,
    recommendation: Option<&str>,
) -> Result<DiagnosticReport, DbError> {
    sqlx::query_as(
        r#"UPDATE diagnostic_reports r SET findings=$5,impression=$6,recommendation=$7,
                  revision=revision+1
           FROM studies st WHERE r.id=$2 AND r.institution_id=$1 AND r.author_fk=$3
             AND r.revision=$4 AND r.status IN ('draft','amending') AND st.id=r.study_fk
           RETURNING r.id,st.study_instance_uid study_uid,r.author_fk author_id,r.status,
                     r.findings,r.impression,r.recommendation,r.revision,r.access_incomplete,
                     r.created_at,r.updated_at"#,
    )
    .bind(institution_id)
    .bind(report_id)
    .bind(author_id)
    .bind(revision)
    .bind(findings)
    .bind(impression)
    .bind(recommendation)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DbError::Conflict("报告版本已变化或不可编辑".to_owned()))
}

pub async fn sign_report(
    pool: &PgPool,
    institution_id: i64,
    report_id: Uuid,
    author_id: i64,
    revision: i32,
) -> Result<(), DbError> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        r#"SELECT r.findings,r.impression,r.recommendation,r.access_incomplete,
                  r.pending_amendment_reason,
                  COALESCE(MAX(v.version_number),0)+1 version_number
           FROM diagnostic_reports r LEFT JOIN diagnostic_report_versions v ON v.report_fk=r.id
           WHERE r.id=$1 AND r.institution_id=$2 AND r.author_fk=$3 AND r.revision=$4
             AND r.status IN ('draft','amending')
             AND NOT EXISTS(
               SELECT 1 FROM diagnostic_report_series rs JOIN series se ON se.id=rs.series_fk
               JOIN dicom_devices d ON d.id=se.source_device_fk
               JOIN diagnostic_work_items w ON w.series_fk=se.id AND w.institution_id=r.institution_id
               WHERE rs.report_fk=r.id AND (se.source_status<>'trusted' OR d.status<>'active'
                 OR w.assignee_fk<>$3 OR NOT EXISTS(
                   SELECT 1 FROM user_device_grants g WHERE g.user_fk=$3 AND g.device_fk=d.id)))
           GROUP BY r.id"#,
    ).bind(report_id).bind(institution_id).bind(author_id).bind(revision)
      .fetch_optional(&mut *tx).await?.ok_or_else(|| DbError::Conflict("报告版本已变化或不可签发".to_owned()))?;
    let findings: String = row.try_get("findings")?;
    let impression: String = row.try_get("impression")?;
    if findings.trim().is_empty() || impression.trim().is_empty() {
        return Err(DbError::Invalid("影像所见和诊断意见不能为空".to_owned()));
    }
    let series_uids: Vec<String> = sqlx::query_scalar(
        r#"SELECT se.series_instance_uid FROM diagnostic_report_series rs
           JOIN series se ON se.id=rs.series_fk WHERE rs.report_fk=$1 ORDER BY se.id"#,
    )
    .bind(report_id)
    .fetch_all(&mut *tx)
    .await?;
    sqlx::query(
        r#"INSERT INTO diagnostic_report_versions
           (id,report_fk,version_number,findings,impression,recommendation,
            covered_series_uids,access_incomplete,amendment_reason,signed_by)
           VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)"#,
    )
    .bind(Uuid::new_v4())
    .bind(report_id)
    .bind(row.try_get::<i32, _>("version_number")?)
    .bind(&findings)
    .bind(&impression)
    .bind(row.try_get::<Option<String>, _>("recommendation")?)
    .bind(&series_uids)
    .bind(row.try_get::<bool, _>("access_incomplete")?)
    .bind(row.try_get::<Option<String>, _>("pending_amendment_reason")?)
    .bind(author_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE diagnostic_reports SET status='signed',pending_amendment_reason=NULL,revision=revision+1 WHERE id=$1")
        .bind(report_id).execute(&mut *tx).await?;
    sqlx::query(
        r#"UPDATE diagnostic_work_items SET status='completed',completed_at=now(),revision=revision+1
           WHERE series_fk IN (SELECT series_fk FROM diagnostic_report_series WHERE report_fk=$1)"#,
    ).bind(report_id).execute(&mut *tx).await?;
    sqlx::query(
        r#"INSERT INTO audit_log(user_fk,username,action,outcome,study_instance_uid,detail)
           SELECT u.id,u.username,'report_signed','success',st.study_instance_uid,
                  jsonb_build_object('report_id',$1::TEXT)
           FROM users u JOIN diagnostic_reports r ON r.author_fk=u.id
           JOIN studies st ON st.id=r.study_fk WHERE r.id=$1 AND u.id=$2"#,
    )
    .bind(report_id)
    .bind(author_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}
