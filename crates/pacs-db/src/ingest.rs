//! 入库:把一个实例的四层元数据写进数据库。
//!
//! 整体在一个事务里,要么四层全部就位,要么什么都不留。这条约束是 C-STORE
//! 可靠性的后半段 —— 文件已经 fsync 落盘了(见 `pacs-store`),数据库这边
//! 半途失败会留下取不回来的孤儿文件。

use pacs_core::InstanceMetadata;
use sqlx::{PgPool, Postgres, Transaction};

use crate::DbError;

/// 已落盘文件的位置与校验信息。
#[derive(Debug, Clone, Copy)]
pub struct StorageRecord<'a> {
    /// 相对存储根的路径。
    pub relative_path: &'a str,
    pub size: u64,
    pub sha256: &'a [u8],
}

/// 入库结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ingested {
    pub patient_id: i64,
    pub study_id: i64,
    pub series_id: i64,
    pub instance_id: i64,
    /// 该实例是新增还是覆盖了已有记录(设备重传)。
    pub instance_created: bool,
}

/// 把一个实例写进数据库。
///
/// 同一个 SOPInstanceUID 重复入库是幂等的:设备重传很常见,不该报错。
pub async fn ingest_instance(
    pool: &PgPool,
    metadata: &InstanceMetadata,
    storage: StorageRecord<'_>,
) -> Result<Ingested, DbError> {
    ingest_instance_for_institution(pool, metadata, storage, 1).await
}

/// 把实例写入显式指定的机构。HTTP 上传必须使用认证身份里的机构,
/// 不能依赖数据库的默认值,否则多租户调用会静默写进默认机构。
pub async fn ingest_instance_for_institution(
    pool: &PgPool,
    metadata: &InstanceMetadata,
    storage: StorageRecord<'_>,
    institution_id: i64,
) -> Result<Ingested, DbError> {
    let mut tx = pool.begin().await?;

    validate_uid_ownership(&mut tx, metadata, institution_id).await?;

    // A sender may retransmit an original SOP after the clinical projection has advanced to a
    // derived UID. Match every immutable version, not just the current projection, so the
    // retransmission remains idempotent and cannot create a second logical instance.
    if let Some((patient_id, study_id, series_id, instance_id, archived_sha)) =
        find_instance_by_version_uid(&mut tx, metadata.instance.uid.as_str()).await?
    {
        if archived_sha.as_slice() != storage.sha256 {
            return Err(DbError::Conflict(format!(
                "SOPInstanceUID {} 已归档为不同内容",
                metadata.instance.uid
            )));
        }
        tx.commit().await?;
        return Ok(Ingested {
            patient_id,
            study_id,
            series_id,
            instance_id,
            instance_created: false,
        });
    }

    // Study/Series UIDs are remapped by immutable revisions. If a modality continues a Study
    // under one of its archived UIDs, attach the new SOP to the stable hierarchy without reverting
    // the current clinical projection to a historical UID.
    let historical_study = find_study_by_version_uid(&mut tx, metadata.study.uid.as_str()).await?;
    let (patient_id, study_id) = if let Some(ids) = historical_study {
        ids
    } else {
        let patient_id = upsert_patient(&mut tx, metadata, institution_id).await?;
        let study_id = upsert_study(&mut tx, metadata, patient_id, institution_id).await?;
        (patient_id, study_id)
    };
    let series_id = if historical_study.is_some() {
        match find_series_by_version_uid(&mut tx, study_id, metadata.series.uid.as_str()).await? {
            Some(series_id) => series_id,
            None => upsert_series(&mut tx, metadata, study_id).await?,
        }
    } else {
        upsert_series(&mut tx, metadata, study_id).await?
    };
    let (instance_id, instance_created) =
        upsert_instance(&mut tx, metadata, series_id, storage).await?;

    ensure_original_version(&mut tx, metadata, instance_id, storage).await?;

    refresh_counts(&mut tx, study_id, series_id).await?;

    tx.commit().await?;

    Ok(Ingested {
        patient_id,
        study_id,
        series_id,
        instance_id,
        instance_created,
    })
}

/// DICOM UID 理论上全局唯一,数据库也按全局唯一约束保存。若一个 UID 已属于
/// 另一机构,不能把调用方悄悄挂到那棵影像树上,也不能通过幂等响应泄露其存在。
async fn validate_uid_ownership(
    tx: &mut Transaction<'_, Postgres>,
    metadata: &InstanceMetadata,
    institution_id: i64,
) -> Result<(), DbError> {
    let owner: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT p.institution_id
        FROM dicom_instance_versions v
        JOIN instances i ON i.id = v.instance_fk
        JOIN series se ON se.id = i.series_fk
        JOIN studies st ON st.id = se.study_fk
        JOIN patients p ON p.id = st.patient_fk
        WHERE v.sop_instance_uid = $1
        LIMIT 1
        "#,
    )
    .bind(metadata.instance.uid.as_str())
    .fetch_optional(&mut **tx)
    .await?;
    if owner.is_some_and(|owner| owner != institution_id) {
        return Err(DbError::Conflict(
            "SOPInstanceUID 已属于其他机构".to_owned(),
        ));
    }

    let study_owner: Option<i64> =
        sqlx::query_scalar("SELECT institution_id FROM studies WHERE study_instance_uid = $1")
            .bind(metadata.study.uid.as_str())
            .fetch_optional(&mut **tx)
            .await?;
    if study_owner.is_some_and(|owner| owner != institution_id) {
        return Err(DbError::Conflict(
            "StudyInstanceUID 已属于其他机构".to_owned(),
        ));
    }

    let series_owner: Option<i64> = sqlx::query_scalar(
        "SELECT st.institution_id FROM series se
         JOIN studies st ON st.id = se.study_fk
         WHERE se.series_instance_uid = $1",
    )
    .bind(metadata.series.uid.as_str())
    .fetch_optional(&mut **tx)
    .await?;
    if series_owner.is_some_and(|owner| owner != institution_id) {
        return Err(DbError::Conflict(
            "SeriesInstanceUID 已属于其他机构".to_owned(),
        ));
    }
    Ok(())
}

async fn find_instance_by_version_uid(
    tx: &mut Transaction<'_, Postgres>,
    sop_instance_uid: &str,
) -> Result<Option<(i64, i64, i64, i64, Vec<u8>)>, DbError> {
    Ok(sqlx::query_as(
        r#"
        SELECT p.id, st.id, se.id, i.id, v.file_sha256
        FROM dicom_instance_versions v
        JOIN instances i ON i.id = v.instance_fk
        JOIN series se ON se.id = i.series_fk
        JOIN studies st ON st.id = se.study_fk
        JOIN patients p ON p.id = st.patient_fk
        WHERE v.sop_instance_uid = $1
        ORDER BY v.version_number DESC
        LIMIT 1
        "#,
    )
    .bind(sop_instance_uid)
    .fetch_optional(&mut **tx)
    .await?)
}

async fn find_study_by_version_uid(
    tx: &mut Transaction<'_, Postgres>,
    study_instance_uid: &str,
) -> Result<Option<(i64, i64)>, DbError> {
    Ok(sqlx::query_as(
        r#"
        SELECT p.id, st.id
        FROM dicom_instance_versions v
        JOIN instances i ON i.id = v.instance_fk
        JOIN series se ON se.id = i.series_fk
        JOIN studies st ON st.id = se.study_fk
        JOIN patients p ON p.id = st.patient_fk
        WHERE v.study_instance_uid = $1 AND st.study_instance_uid <> $1
        ORDER BY v.version_number DESC
        LIMIT 1
        "#,
    )
    .bind(study_instance_uid)
    .fetch_optional(&mut **tx)
    .await?)
}

async fn find_series_by_version_uid(
    tx: &mut Transaction<'_, Postgres>,
    study_id: i64,
    series_instance_uid: &str,
) -> Result<Option<i64>, DbError> {
    Ok(sqlx::query_scalar(
        r#"
        SELECT se.id
        FROM dicom_instance_versions v
        JOIN instances i ON i.id = v.instance_fk
        JOIN series se ON se.id = i.series_fk
        WHERE se.study_fk = $1 AND v.series_instance_uid = $2
          AND se.series_instance_uid <> $2
        ORDER BY v.version_number DESC
        LIMIT 1
        "#,
    )
    .bind(study_id)
    .bind(series_instance_uid)
    .fetch_optional(&mut **tx)
    .await?)
}

/// Register a newly received file as immutable version 1.
///
/// `instances.current_version_id` is nullable only to break the insert cycle between the
/// projection row and its first version. This function fills it in before the ingest transaction
/// commits. Existing/retransmitted instances already have a current version and are left alone.
async fn ensure_original_version(
    tx: &mut Transaction<'_, Postgres>,
    metadata: &InstanceMetadata,
    instance_id: i64,
    storage: StorageRecord<'_>,
) -> Result<(), DbError> {
    let current: Option<i64> =
        sqlx::query_scalar("SELECT current_version_id FROM instances WHERE id = $1")
            .bind(instance_id)
            .fetch_one(&mut **tx)
            .await?;
    if current.is_some() {
        return Ok(());
    }

    let snapshot = serde_json::to_value(metadata).unwrap_or_else(|error| {
        tracing::error!(%error, "入库元数据快照序列化失败");
        serde_json::Value::Object(Default::default())
    });
    let version_id = sqlx::query_scalar::<_, i64>(
        r#"
        INSERT INTO dicom_instance_versions (
            logical_instance_id, instance_fk, version_number, derivation_kind,
            study_instance_uid, series_instance_uid, sop_instance_uid,
            transfer_syntax_uid, storage_path, file_size, file_sha256,
            metadata_snapshot, reason
        )
        SELECT logical_instance_id, id, 1, 'original',
               $2, $3, $4, $5, $6, $7, $8, $9, 'received by C-STORE'
        FROM instances
        WHERE id = $1
        RETURNING id
        "#,
    )
    .bind(instance_id)
    .bind(metadata.study.uid.as_str())
    .bind(metadata.series.uid.as_str())
    .bind(metadata.instance.uid.as_str())
    .bind(metadata.instance.transfer_syntax_uid.as_str())
    .bind(storage.relative_path)
    .bind(i64::try_from(storage.size).unwrap_or(i64::MAX))
    .bind(storage.sha256)
    .bind(snapshot)
    .fetch_one(&mut **tx)
    .await?;

    sqlx::query("UPDATE instances SET current_version_id = $2 WHERE id = $1")
        .bind(instance_id)
        .bind(version_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// 各层的 upsert 都用 `COALESCE(EXCLUDED.x, 原值)` 合并。
///
/// 同一个检查的不同实例,头信息完整程度经常不一样(有的带 StudyDescription
/// 有的不带)。用 EXCLUDED 直接覆盖的话,后到的残缺实例会把先前存好的字段抹成
/// NULL。COALESCE 让新的非空值生效、空值不破坏已有数据。
async fn upsert_patient(
    tx: &mut Transaction<'_, Postgres>,
    metadata: &InstanceMetadata,
    institution_id: i64,
) -> Result<i64, DbError> {
    let patient = &metadata.patient;
    let id = sqlx::query_scalar::<_, i64>(
        r#"
        INSERT INTO patients (
            institution_id, patient_id, issuer_of_patient_id, name, name_normalized,
            birth_date, sex, attributes
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        ON CONFLICT (institution_id, patient_id) DO UPDATE SET
            issuer_of_patient_id = COALESCE(EXCLUDED.issuer_of_patient_id, patients.issuer_of_patient_id),
            name                 = COALESCE(EXCLUDED.name, patients.name),
            name_normalized      = COALESCE(EXCLUDED.name_normalized, patients.name_normalized),
            birth_date           = COALESCE(EXCLUDED.birth_date, patients.birth_date),
            sex                  = COALESCE(EXCLUDED.sex, patients.sex),
            attributes           = patients.attributes || EXCLUDED.attributes
        RETURNING id
        "#,
    )
    .bind(institution_id)
    .bind(&patient.patient_id)
    .bind(&patient.issuer_of_patient_id)
    .bind(&patient.name)
    .bind(&patient.name_normalized)
    .bind(patient.birth_date)
    .bind(&patient.sex)
    .bind(&patient.attributes)
    .fetch_one(&mut **tx)
    .await?;
    Ok(id)
}

async fn upsert_study(
    tx: &mut Transaction<'_, Postgres>,
    metadata: &InstanceMetadata,
    patient_id: i64,
    institution_id: i64,
) -> Result<i64, DbError> {
    let study = &metadata.study;
    let id = sqlx::query_scalar::<_, i64>(
        r#"
        INSERT INTO studies (
            patient_fk, institution_id, study_instance_uid, study_date, study_time,
            accession_number, study_id, description, referring_physician, attributes
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        ON CONFLICT (study_instance_uid) DO UPDATE SET
            study_date          = COALESCE(EXCLUDED.study_date, studies.study_date),
            study_time          = COALESCE(EXCLUDED.study_time, studies.study_time),
            accession_number    = COALESCE(EXCLUDED.accession_number, studies.accession_number),
            study_id            = COALESCE(EXCLUDED.study_id, studies.study_id),
            description         = COALESCE(EXCLUDED.description, studies.description),
            referring_physician = COALESCE(EXCLUDED.referring_physician, studies.referring_physician),
            attributes          = studies.attributes || EXCLUDED.attributes
        RETURNING id
        "#,
    )
    .bind(patient_id)
    .bind(institution_id)
    .bind(study.uid.as_str())
    .bind(study.date)
    .bind(study.time)
    .bind(&study.accession_number)
    .bind(&study.study_id)
    .bind(&study.description)
    .bind(&study.referring_physician)
    .bind(&study.attributes)
    .fetch_one(&mut **tx)
    .await?;
    Ok(id)
}

async fn upsert_series(
    tx: &mut Transaction<'_, Postgres>,
    metadata: &InstanceMetadata,
    study_id: i64,
) -> Result<i64, DbError> {
    let series = &metadata.series;
    let id = sqlx::query_scalar::<_, i64>(
        r#"
        INSERT INTO series (
            study_fk, series_instance_uid, series_number, modality,
            description, body_part_examined, protocol_name,
            series_date, series_time, attributes
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        ON CONFLICT (series_instance_uid) DO UPDATE SET
            series_number      = COALESCE(EXCLUDED.series_number, series.series_number),
            modality           = COALESCE(EXCLUDED.modality, series.modality),
            description        = COALESCE(EXCLUDED.description, series.description),
            body_part_examined = COALESCE(EXCLUDED.body_part_examined, series.body_part_examined),
            protocol_name      = COALESCE(EXCLUDED.protocol_name, series.protocol_name),
            series_date        = COALESCE(EXCLUDED.series_date, series.series_date),
            series_time        = COALESCE(EXCLUDED.series_time, series.series_time),
            attributes         = series.attributes || EXCLUDED.attributes
        RETURNING id
        "#,
    )
    .bind(study_id)
    .bind(series.uid.as_str())
    .bind(series.number)
    .bind(&series.modality)
    .bind(&series.description)
    .bind(&series.body_part_examined)
    .bind(&series.protocol_name)
    .bind(series.date)
    .bind(series.time)
    .bind(&series.attributes)
    .fetch_one(&mut **tx)
    .await?;
    Ok(id)
}

/// 实例层不做 COALESCE 合并:重传的是同一份影像的新副本,整行直接覆盖。
///
/// 返回值里的 `created` 用 `xmax = 0` 判断 —— 新插入的行 xmax 为 0,
/// 走 DO UPDATE 分支的行 xmax 是当前事务号。这个值只用于上报,
/// 计数不依赖它(见 [`refresh_counts`])。
async fn upsert_instance(
    tx: &mut Transaction<'_, Postgres>,
    metadata: &InstanceMetadata,
    series_id: i64,
    storage: StorageRecord<'_>,
) -> Result<(i64, bool), DbError> {
    let instance = &metadata.instance;
    let logical_instance_id = uuid::Uuid::new_v4();
    let row = sqlx::query_as::<_, (i64, bool)>(
        r#"
        INSERT INTO instances (
            series_fk, logical_instance_id, sop_instance_uid, sop_class_uid, instance_number,
            transfer_syntax_uid, image_rows, image_columns, number_of_frames,
            image_position_patient, image_orientation_patient,
            storage_path, file_size, file_sha256, attributes
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
        ON CONFLICT (sop_instance_uid) DO UPDATE SET
            series_fk                 = EXCLUDED.series_fk,
            sop_class_uid             = EXCLUDED.sop_class_uid,
            instance_number           = EXCLUDED.instance_number,
            transfer_syntax_uid       = EXCLUDED.transfer_syntax_uid,
            image_rows                = EXCLUDED.image_rows,
            image_columns             = EXCLUDED.image_columns,
            number_of_frames          = EXCLUDED.number_of_frames,
            image_position_patient    = EXCLUDED.image_position_patient,
            image_orientation_patient = EXCLUDED.image_orientation_patient,
            storage_path              = EXCLUDED.storage_path,
            file_size                 = EXCLUDED.file_size,
            file_sha256               = EXCLUDED.file_sha256,
            attributes                = EXCLUDED.attributes
        RETURNING id, (xmax = 0) AS created
        "#,
    )
    .bind(series_id)
    .bind(logical_instance_id)
    .bind(instance.uid.as_str())
    .bind(instance.sop_class_uid.as_ref().map(|uid| uid.as_str()))
    .bind(instance.number)
    .bind(instance.transfer_syntax_uid.as_str())
    .bind(instance.rows)
    .bind(instance.columns)
    .bind(instance.number_of_frames)
    .bind(instance.image_position_patient.as_deref())
    .bind(instance.image_orientation_patient.as_deref())
    .bind(storage.relative_path)
    .bind(i64::try_from(storage.size).unwrap_or(i64::MAX))
    .bind(storage.sha256)
    .bind(&instance.attributes)
    .fetch_one(&mut **tx)
    .await?;
    Ok(row)
}

/// 按实际行数重算聚合列。
///
/// 刻意用重算而不是 `count = count + 1`:增量在重传、事务回滚、并发入库下都会
/// 漂移,而漂移出来的 NumberOfStudyRelatedInstances 会直接出现在 C-FIND 响应里。
/// 代价是每次入库多两次索引扫描,对一个几百层的序列完全可以接受;
/// 真到了成为瓶颈的时候(阶段 2 建 benchmark 时验证),再换成增量 + 定期对账。
async fn refresh_counts(
    tx: &mut Transaction<'_, Postgres>,
    study_id: i64,
    series_id: i64,
) -> Result<(), DbError> {
    sqlx::query(
        r#"
        UPDATE series
        SET number_of_instances = (SELECT count(*) FROM instances WHERE series_fk = $1)
        WHERE id = $1
        "#,
    )
    .bind(series_id)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        r#"
        UPDATE studies SET
            number_of_series = (SELECT count(*) FROM series WHERE study_fk = $1),
            number_of_instances = (
                SELECT count(*) FROM instances i
                JOIN series s ON i.series_fk = s.id
                WHERE s.study_fk = $1
            ),
            modalities = COALESCE(
                (SELECT array_agg(DISTINCT modality ORDER BY modality)
                 FROM series WHERE study_fk = $1 AND modality IS NOT NULL),
                '{}'
            )
        WHERE id = $1
        "#,
    )
    .bind(study_id)
    .execute(&mut **tx)
    .await?;

    Ok(())
}
