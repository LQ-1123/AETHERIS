//! Device-scoped clinical access, diagnostic work queue and reports.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
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

/// 手动注册设备（status='pending'，批准后 active 才可归属历史序列）。
/// 设备也可以由 DIMSE 入站观察自动创建；本函数服务于尚未接入或历史数据归属场景。
pub async fn register_device(
    pool: &PgPool,
    institution_id: i64,
    name: &str,
    calling_ae_title: &str,
    source_ip: &str,
    modality_hint: Option<&str>,
) -> Result<DicomDevice, DbError> {
    sqlx::query_as(
        r#"INSERT INTO dicom_devices(id,institution_id,name,calling_ae_title,source_ip,
                   modality_hint)
           VALUES($1,$2,$3,$4,$5,$6)
           RETURNING id,institution_id,name,calling_ae_title,source_ip,modality_hint,status,
                     first_seen_at,last_seen_at,approved_at"#,
    )
    .bind(Uuid::new_v4())
    .bind(institution_id)
    .bind(name.trim())
    .bind(calling_ae_title.trim())
    .bind(source_ip.trim())
    .bind(modality_hint)
    .fetch_one(pool)
    .await
    .map_err(|error| match error {
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            DbError::Invalid("设备已存在（同 AE Title + 来源 IP）".to_owned())
        }
        other => DbError::from(other),
    })
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct SeriesSourceEntry {
    pub series_uid: String,
    pub study_uid: String,
    pub patient_id: String,
    pub patient_name: Option<String>,
    pub modality: Option<String>,
    pub description: Option<String>,
    pub instance_count: i64,
    pub source_status: String,
    pub device_name: Option<String>,
}

/// 列出序列的来源归属状态。`unattributed=true` 时只返回待归属
/// （needs_review / legacy_unattributed）的序列，供管理员控制台批量归属。
pub async fn list_series_sources(
    pool: &PgPool,
    institution_id: i64,
    unattributed: bool,
    limit: i64,
    offset: i64,
) -> Result<Vec<SeriesSourceEntry>, DbError> {
    Ok(sqlx::query_as(
        r#"SELECT se.series_instance_uid series_uid,st.study_instance_uid study_uid,
                  p.patient_id,p.name patient_name,se.modality,se.description,
                  (SELECT count(*) FROM instances i WHERE i.series_fk=se.id) instance_count,
                  se.source_status,d.name device_name
           FROM series se
           JOIN studies st ON st.id=se.study_fk
           JOIN patients p ON p.id=st.patient_fk
           LEFT JOIN dicom_devices d ON d.id=se.source_device_fk
           WHERE st.institution_id=$1
             AND (NOT $2 OR se.source_status IN ('needs_review','legacy_unattributed'))
           ORDER BY se.id DESC,st.study_date DESC NULLS LAST
           LIMIT $3 OFFSET $4"#,
    )
    .bind(institution_id)
    .bind(unattributed)
    .bind(limit.clamp(1, 500))
    .bind(offset.max(0))
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

/// 按序列查工作项（供报告面板显示领取状态，不按日期过滤——历史数据也要能领）。
/// 可见性与工作列表一致：来源可信 + 设备已批准 + 用户有该设备授权（管理员除外）。
pub async fn work_item_for_series(
    pool: &PgPool,
    institution_id: i64,
    user_id: i64,
    is_admin: bool,
    series_uid: &str,
) -> Result<Option<ClinicalWorkItem>, DbError> {
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
           WHERE w.institution_id=$1 AND se.series_instance_uid=$2 AND se.source_status='trusted'
             AND ($4 OR EXISTS(SELECT 1 FROM user_device_grants g
                               WHERE g.user_fk=$3 AND g.device_fk=d.id))
           GROUP BY w.id,se.id,st.id,p.id,d.id,u.id,ins.timezone
           ORDER BY w.id"#,
    )
    .bind(institution_id)
    .bind(series_uid)
    .bind(user_id)
    .bind(is_admin)
    .fetch_optional(pool)
    .await?)
}

/// 列出某检查（Study）下所有序列的工作项（报告按检查一份，领取也按检查）。
/// 可见性与按序列版本一致：来源可信 + 设备已批准 + 用户有该设备授权（管理员除外）。
pub async fn study_work_items(
    pool: &PgPool,
    institution_id: i64,
    user_id: i64,
    is_admin: bool,
    study_uid: &str,
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
           WHERE w.institution_id=$1 AND st.study_instance_uid=$2 AND se.source_status='trusted'
             AND ($4 OR EXISTS(SELECT 1 FROM user_device_grants g
                               WHERE g.user_fk=$3 AND g.device_fk=d.id))
           GROUP BY w.id,se.id,st.id,p.id,d.id,u.id,ins.timezone
           ORDER BY se.id"#,
    )
    .bind(institution_id)
    .bind(study_uid)
    .bind(user_id)
    .bind(is_admin)
    .fetch_all(pool)
    .await?)
}

/// 领取检查下所有 pending 工作项（报告按检查，领取一次性覆盖全部序列）。
/// 返回成功领取的数量。
pub async fn claim_study(
    pool: &PgPool,
    institution_id: i64,
    study_uid: &str,
    user_id: i64,
) -> Result<usize, DbError> {
    let changed = sqlx::query(
        r#"UPDATE diagnostic_work_items w SET status='claimed',assignee_fk=$3,
                  claimed_at=now(),revision=revision+1
           FROM series se,studies st,dicom_devices d
           WHERE w.institution_id=$1 AND w.series_fk=se.id AND se.study_fk=st.id
             AND st.study_instance_uid=$2 AND se.source_device_fk=d.id
             AND se.source_status='trusted' AND d.status='active'
             AND w.status='pending'
             AND EXISTS(SELECT 1 FROM user_device_grants g
                        WHERE g.user_fk=$3 AND g.device_fk=d.id)"#,
    )
    .bind(institution_id)
    .bind(study_uid)
    .bind(user_id)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(changed as usize)
}

/// 释放检查下所有由当前用户领取的工作项（存在草稿/修订中报告时整体拒绝）。
pub async fn release_study(
    pool: &PgPool,
    institution_id: i64,
    study_uid: &str,
    actor_id: i64,
) -> Result<(), DbError> {
    let changed = sqlx::query(
        r#"UPDATE diagnostic_work_items w SET status='pending',assignee_fk=NULL,claimed_at=NULL,
                  revision=revision+1
           FROM series se,studies st
           WHERE w.institution_id=$1 AND w.series_fk=se.id AND se.study_fk=st.id
             AND st.study_instance_uid=$2
             AND w.status IN ('claimed','reporting') AND w.assignee_fk=$3
             AND NOT EXISTS(SELECT 1 FROM diagnostic_reports r
                            WHERE r.study_fk=st.id AND r.status IN ('draft','amending'))"#,
    )
    .bind(institution_id)
    .bind(study_uid)
    .bind(actor_id)
    .execute(pool)
    .await?
    .rows_affected();
    if changed == 0 {
        Err(DbError::Conflict(
            "无可释放的工作项，或存在草稿/修订中报告".to_owned(),
        ))
    } else {
        Ok(())
    }
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
    pub author_name: String,
    pub reviewer_id: Option<i64>,
    pub reviewer_name: Option<String>,
    pub status: String,
    pub findings: String,
    pub impression: String,
    pub recommendation: Option<String>,
    pub revision: i32,
    pub access_incomplete: bool,
    pub is_positive: bool,
    pub template_payload: Option<Value>,
    pub submitted_at: Option<DateTime<Utc>>,
    pub reviewed_at: Option<DateTime<Utc>>,
    pub review_comment: Option<String>,
    pub reviewer_modified: bool,
    pub review_required: bool,
    pub can_review: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub async fn create_report(
    pool: &PgPool,
    institution_id: i64,
    author_id: i64,
    study_uid: &str,
    series_uids: &[String],
    template_payload: Option<Value>,
    is_positive: bool,
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
    // 授权校验：覆盖的序列必须来源可信 + 设备已批准 + 医生有设备授权（不再要求领取）。
    let rows = sqlx::query(
        r#"SELECT se.id FROM series se
           JOIN dicom_devices d ON d.id=se.source_device_fk AND d.status='active'
           JOIN user_device_grants g ON g.device_fk=d.id AND g.user_fk=$3
           WHERE se.study_fk=$2 AND se.series_instance_uid=ANY($4)
             AND se.source_status='trusted'"#,
    )
    .bind(institution_id)
    .bind(study_id)
    .bind(author_id)
    .bind(series_uids)
    .fetch_all(&mut *tx)
    .await?;
    if rows.len() != series_uids.len() {
        return Err(DbError::Invalid("序列未全部获权".to_owned()));
    }
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM series WHERE study_fk=$1")
        .bind(study_id)
        .fetch_one(&mut *tx)
        .await?;
    let id = Uuid::new_v4();
    let report: DiagnosticReport = sqlx::query_as(
        r#"INSERT INTO diagnostic_reports(id,institution_id,study_fk,author_fk,access_incomplete,
                   template_payload,is_positive)
           VALUES($1,$2,$3,$4,$5,$6,$7)
           RETURNING id,$8::TEXT study_uid,author_fk author_id,
                     (SELECT COALESCE(display_name,username) FROM users WHERE id=author_fk) author_name,
                     reviewer_fk reviewer_id,NULL::TEXT reviewer_name,status,findings,impression,
                     recommendation,revision,access_incomplete,is_positive,template_payload,
                     submitted_at,reviewed_at,review_comment,false reviewer_modified,
                     (SELECT review_required FROM institutions WHERE id=$2) review_required,
                     false can_review,
                     created_at,updated_at"#,
    )
    .bind(id)
    .bind(institution_id)
    .bind(study_id)
    .bind(author_id)
    .bind(total != series_uids.len() as i64)
    .bind(&template_payload)
    .bind(is_positive)
    .bind(study_uid)
    .fetch_one(&mut *tx)
    .await
    .map_err(|error| match error {
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            DbError::Conflict("该检查已有报告".to_owned())
        }
        other => DbError::from(other),
    })?;
    for row in rows {
        let series_id: i64 = row.try_get("id")?;
        sqlx::query("INSERT INTO diagnostic_report_series(report_fk,series_fk) VALUES($1,$2)")
            .bind(id)
            .bind(series_id)
            .execute(&mut *tx)
            .await?;
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
        r#"SELECT r.id,st.study_instance_uid study_uid,r.author_fk author_id,
                  COALESCE(au.display_name,au.username) author_name,
                  r.reviewer_fk reviewer_id,COALESCE(ru.display_name,ru.username) reviewer_name,r.status,
                  r.findings,r.impression,r.recommendation,r.revision,r.access_incomplete,
                  r.is_positive,r.template_payload,r.submitted_at,r.reviewed_at,r.review_comment,
                  EXISTS(SELECT 1 FROM report_review_events e
                         WHERE e.report_fk=r.id AND e.action='reviewer_modified') reviewer_modified,
                  inst.review_required,
                  EXISTS(SELECT 1 FROM user_permission_grants pg
                         WHERE pg.user_fk=$3 AND pg.permission='review_report') can_review,
                  r.created_at,r.updated_at
           FROM diagnostic_reports r JOIN studies st ON st.id=r.study_fk
           JOIN institutions inst ON inst.id=r.institution_id
           JOIN users au ON au.id=r.author_fk LEFT JOIN users ru ON ru.id=r.reviewer_fk
           WHERE r.institution_id=$1 AND st.study_instance_uid=$2
             AND ($4 OR r.author_fk=$3 OR (
               r.status IN ('submitted','under_review') AND EXISTS(
                 SELECT 1 FROM user_permission_grants pg
                 WHERE pg.user_fk=$3 AND pg.permission='review_report'
               )
             ) OR EXISTS(
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
    pub is_positive: bool,
    pub amendment_reason: Option<String>,
    pub signed_by: i64,
    pub signed_at: DateTime<Utc>,
    pub reviewed_by: Option<i64>,
    pub reviewed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ReportReviewEvent {
    pub id: i64,
    pub report_id: Uuid,
    pub actor_id: i64,
    pub actor_name: String,
    pub action: String,
    pub comment: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ReportTemplate {
    pub id: Uuid,
    pub name: String,
    pub modality: String,
    pub body_part: Option<String>,
    pub version: i32,
    pub structure: Value,
    pub builtin: bool,
}

/// 列出机构内的报告模板；`modality` 为 None 时返回全部。
/// 仅供「新建报告时选模板」，历史报告的渲染不依赖本表（设计不变量 I1）。
pub async fn list_report_templates(
    pool: &PgPool,
    institution_id: i64,
    modality: Option<&str>,
) -> Result<Vec<ReportTemplate>, DbError> {
    Ok(sqlx::query_as(
        r#"SELECT id,name,modality,body_part,version,structure,builtin
           FROM report_templates
           WHERE institution_id=$1 AND ($2::text IS NULL OR modality=$2)
           ORDER BY modality,name"#,
    )
    .bind(institution_id)
    .bind(modality)
    .fetch_all(pool)
    .await?)
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
                  v.recommendation,v.covered_series_uids,v.access_incomplete,v.is_positive,
                  v.amendment_reason,v.signed_by,v.signed_at,v.reviewed_by,v.reviewed_at
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

pub async fn list_report_review_events(
    pool: &PgPool,
    institution_id: i64,
    report_id: Uuid,
    user_id: i64,
    is_admin: bool,
) -> Result<Vec<ReportReviewEvent>, DbError> {
    Ok(sqlx::query_as(
        r#"SELECT e.id,e.report_fk report_id,e.actor_fk actor_id,
                  COALESCE(u.display_name,u.username) actor_name,e.action,e.comment,e.created_at
           FROM report_review_events e
           JOIN diagnostic_reports r ON r.id=e.report_fk
           JOIN users u ON u.id=e.actor_fk
           WHERE e.report_fk=$1 AND r.institution_id=$2 AND ($4 OR r.author_fk=$3
             OR r.reviewer_fk=$3 OR EXISTS(
               SELECT 1 FROM diagnostic_report_series rs JOIN series se ON se.id=rs.series_fk
               JOIN dicom_devices d ON d.id=se.source_device_fk AND d.status='active'
               JOIN user_device_grants g ON g.device_fk=d.id AND g.user_fk=$3
               WHERE rs.report_fk=r.id))
           ORDER BY e.created_at,e.id"#,
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
           RETURNING r.id,st.study_instance_uid study_uid,r.author_fk author_id,
                     (SELECT COALESCE(display_name,username) FROM users WHERE id=r.author_fk) author_name,
                     r.reviewer_fk reviewer_id,
                     (SELECT COALESCE(display_name,username) FROM users WHERE id=r.reviewer_fk) reviewer_name,
                     r.status,
                     r.findings,r.impression,r.recommendation,r.revision,r.access_incomplete,
                     r.is_positive,r.template_payload,r.submitted_at,r.reviewed_at,r.review_comment,
                     EXISTS(SELECT 1 FROM report_review_events e
                            WHERE e.report_fk=r.id AND e.action='reviewer_modified') reviewer_modified,
                     (SELECT review_required FROM institutions WHERE id=$2) review_required,
                     false can_review,r.created_at,r.updated_at"#,
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
    template_payload: Option<Value>,
    is_positive: bool,
    clear_template_payload: bool,
) -> Result<DiagnosticReport, DbError> {
    // I2 派生缓存单向：结构化报告（payload 非空）的草稿更新必须携带 payload，
    // 文本列不得绕过 payload 被独立修改。
    let has_payload: bool = sqlx::query_scalar(
        "SELECT template_payload IS NOT NULL FROM diagnostic_reports WHERE id=$1 AND institution_id=$2",
    )
    .bind(report_id)
    .bind(institution_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DbError::Conflict("报告版本已变化或不可编辑".to_owned()))?;
    if has_payload && template_payload.is_none() && !clear_template_payload {
        return Err(DbError::Invalid(
            "结构化报告必须携带 template_payload".to_owned(),
        ));
    }
    sqlx::query_as(
        r#"UPDATE diagnostic_reports r SET findings=$5,impression=$6,recommendation=$7,
                  template_payload=CASE WHEN $10 THEN NULL ELSE COALESCE($8, r.template_payload) END,
                  is_positive=$9,
                  revision=revision+1
           FROM studies st WHERE r.id=$2 AND r.institution_id=$1 AND r.author_fk=$3
             AND r.revision=$4 AND r.status IN ('draft','amending') AND st.id=r.study_fk
           RETURNING r.id,st.study_instance_uid study_uid,r.author_fk author_id,
                     (SELECT COALESCE(display_name,username) FROM users WHERE id=r.author_fk) author_name,
                     r.reviewer_fk reviewer_id,
                     (SELECT COALESCE(display_name,username) FROM users WHERE id=r.reviewer_fk) reviewer_name,
                     r.status,
                     r.findings,r.impression,r.recommendation,r.revision,r.access_incomplete,
                     r.is_positive,r.template_payload,r.submitted_at,r.reviewed_at,r.review_comment,
                     EXISTS(SELECT 1 FROM report_review_events e
                            WHERE e.report_fk=r.id AND e.action='reviewer_modified') reviewer_modified,
                     (SELECT review_required FROM institutions WHERE id=$1) review_required,
                     false can_review,r.created_at,r.updated_at"#,
    )
    .bind(institution_id)
    .bind(report_id)
    .bind(author_id)
    .bind(revision)
    .bind(findings)
    .bind(impression)
    .bind(recommendation)
    .bind(&template_payload)
    .bind(is_positive)
    .bind(clear_template_payload)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DbError::Conflict("报告版本已变化或不可编辑".to_owned()))
}

/// 作者提交报告进入审核队列。提交与事件留痕必须同事务提交。
pub async fn submit_report(
    pool: &PgPool,
    institution_id: i64,
    report_id: Uuid,
    author_id: i64,
    revision: i32,
) -> Result<(), DbError> {
    let mut tx = pool.begin().await?;
    let updated = sqlx::query(
        r#"UPDATE diagnostic_reports r
           SET status='submitted',submitted_at=now(),reviewer_fk=NULL,reviewed_at=NULL,
               review_comment=NULL,revision=revision+1
           WHERE r.id=$1 AND r.institution_id=$2 AND r.author_fk=$3 AND r.revision=$4
             AND r.status IN ('draft','amending')
             AND btrim(r.findings)<>'' AND btrim(r.impression)<>''"#,
    )
    .bind(report_id)
    .bind(institution_id)
    .bind(author_id)
    .bind(revision)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(DbError::Conflict(
            "报告版本已变化、内容不完整或不可提交审核".to_owned(),
        ));
    }
    sqlx::query(
        "INSERT INTO report_review_events(report_fk,actor_fk,action) VALUES($1,$2,'submitted')",
    )
    .bind(report_id)
    .bind(author_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// 审核人领取一份待审核报告。数据库再次硬校验审核人不能是作者。
pub async fn start_report_review(
    pool: &PgPool,
    institution_id: i64,
    report_id: Uuid,
    reviewer_id: i64,
    revision: i32,
) -> Result<(), DbError> {
    let mut tx = pool.begin().await?;
    let updated = sqlx::query(
        r#"UPDATE diagnostic_reports
           SET status='under_review',reviewer_fk=$3,revision=revision+1
           WHERE id=$1 AND institution_id=$2 AND revision=$4 AND status='submitted'
             AND author_fk<>$3"#,
    )
    .bind(report_id)
    .bind(institution_id)
    .bind(reviewer_id)
    .bind(revision)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(DbError::Conflict(
            "报告已被领取、版本已变化或审核人与作者相同".to_owned(),
        ));
    }
    sqlx::query(
        r#"INSERT INTO report_review_events(report_fk,actor_fk,action)
           VALUES($1,$2,'review_started')"#,
    )
    .bind(report_id)
    .bind(reviewer_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// 审核通过并签发。若审核人修改内容，则内容、版本快照、事件与签发状态原子落库。
#[allow(clippy::too_many_arguments)]
pub async fn approve_report(
    pool: &PgPool,
    institution_id: i64,
    report_id: Uuid,
    reviewer_id: i64,
    revision: i32,
    modified: bool,
    findings: Option<&str>,
    impression: Option<&str>,
    recommendation: Option<&str>,
    review_comment: Option<&str>,
) -> Result<(), DbError> {
    if modified
        && (findings.is_none_or(|value| value.trim().is_empty())
            || impression.is_none_or(|value| value.trim().is_empty()))
    {
        return Err(DbError::Invalid(
            "修改后签发必须提供影像所见和诊断意见".to_owned(),
        ));
    }
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        r#"SELECT r.author_fk,r.findings,r.impression,r.recommendation,r.access_incomplete,
                  r.is_positive,r.pending_amendment_reason,
                  (SELECT COALESCE(MAX(v.version_number),0)+1
                   FROM diagnostic_report_versions v WHERE v.report_fk=r.id) version_number
           FROM diagnostic_reports r
           WHERE r.id=$1 AND r.institution_id=$2 AND r.reviewer_fk=$3 AND r.revision=$4
             AND r.status='under_review' AND r.author_fk<>$3
           FOR UPDATE OF r"#,
    )
    .bind(report_id)
    .bind(institution_id)
    .bind(reviewer_id)
    .bind(revision)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| DbError::Conflict("报告版本已变化、未由当前审核人领取或不可审核".to_owned()))?;

    let final_findings = if modified {
        findings.unwrap().to_owned()
    } else {
        row.try_get("findings")?
    };
    let final_impression = if modified {
        impression.unwrap().to_owned()
    } else {
        row.try_get("impression")?
    };
    let final_recommendation = if modified {
        recommendation.map(str::to_owned)
    } else {
        row.try_get::<Option<String>, _>("recommendation")?
    };
    if final_findings.trim().is_empty() || final_impression.trim().is_empty() {
        return Err(DbError::Invalid("影像所见和诊断意见不能为空".to_owned()));
    }
    let series_uids: Vec<String> = sqlx::query_scalar(
        r#"SELECT se.series_instance_uid FROM diagnostic_report_series rs
           JOIN series se ON se.id=rs.series_fk WHERE rs.report_fk=$1 ORDER BY se.id"#,
    )
    .bind(report_id)
    .fetch_all(&mut *tx)
    .await?;

    if modified {
        sqlx::query(
            r#"UPDATE diagnostic_reports SET findings=$2,impression=$3,recommendation=$4,
                      template_payload=NULL WHERE id=$1"#,
        )
        .bind(report_id)
        .bind(&final_findings)
        .bind(&final_impression)
        .bind(&final_recommendation)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        r#"INSERT INTO diagnostic_report_versions
           (id,report_fk,version_number,findings,impression,recommendation,
            covered_series_uids,access_incomplete,is_positive,amendment_reason,signed_by,
            reviewed_by,reviewed_at)
           VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$11,now())"#,
    )
    .bind(Uuid::new_v4())
    .bind(report_id)
    .bind(row.try_get::<i32, _>("version_number")?)
    .bind(&final_findings)
    .bind(&final_impression)
    .bind(&final_recommendation)
    .bind(&series_uids)
    .bind(row.try_get::<bool, _>("access_incomplete")?)
    .bind(row.try_get::<bool, _>("is_positive")?)
    .bind(row.try_get::<Option<String>, _>("pending_amendment_reason")?)
    .bind(reviewer_id)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"UPDATE diagnostic_reports SET status='signed',reviewed_at=now(),review_comment=$2,
                  pending_amendment_reason=NULL,revision=revision+1 WHERE id=$1"#,
    )
    .bind(report_id)
    .bind(
        review_comment
            .map(str::trim)
            .filter(|value| !value.is_empty()),
    )
    .execute(&mut *tx)
    .await?;
    if modified {
        sqlx::query(
            r#"INSERT INTO report_review_events(report_fk,actor_fk,action,comment)
               VALUES($1,$2,'reviewer_modified',$3)"#,
        )
        .bind(report_id)
        .bind(reviewer_id)
        .bind(review_comment)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        r#"INSERT INTO report_review_events(report_fk,actor_fk,action,comment)
           VALUES($1,$2,'approved',$3)"#,
    )
    .bind(report_id)
    .bind(reviewer_id)
    .bind(review_comment)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"UPDATE diagnostic_work_items SET status='completed',completed_at=now(),revision=revision+1
           WHERE series_fk IN (SELECT series_fk FROM diagnostic_report_series WHERE report_fk=$1)"#,
    )
    .bind(report_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"UPDATE exam_requests SET status='completed',revision=revision+1
           WHERE study_fk=(SELECT study_fk FROM diagnostic_reports WHERE id=$1)
             AND status='executed'"#,
    )
    .bind(report_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"INSERT INTO audit_log(user_fk,username,action,outcome,study_instance_uid,detail)
           SELECT u.id,u.username,'report_signed','success',st.study_instance_uid,
                  jsonb_build_object('report_id',$1::TEXT,'reviewed',true,'modified',$3)
           FROM users u JOIN diagnostic_reports r ON r.id=$1
           JOIN studies st ON st.id=r.study_fk WHERE u.id=$2"#,
    )
    .bind(report_id)
    .bind(reviewer_id)
    .bind(modified)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn sign_report(
    _pool: &PgPool,
    _institution_id: i64,
    _report_id: Uuid,
    _author_id: i64,
    _revision: i32,
) -> Result<(), DbError> {
    Err(DbError::Conflict(
        "报告必须提交审核，并由非作者审核人签发".to_owned(),
    ))
}
