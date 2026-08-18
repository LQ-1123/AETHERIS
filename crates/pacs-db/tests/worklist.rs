//! 高级队列查询的真实 Postgres 覆盖。

use dicom::core::{DataElement, VR};
use dicom::dictionary_std::tags;
use pacs_core::extract_metadata;
use pacs_core::fixture::{ct_instance, unique_uid};
use pacs_db::{
    Ingested, QueueFilter, QueueSort, StorageRecord, ingest_instance, list_queue_studies,
};
use sqlx::migrate::MigrateDatabase;
use sqlx::{PgPool, Postgres};
use uuid::Uuid;

async fn pool() -> Option<PgPool> {
    dotenvy::dotenv().ok();
    let Ok(url) = std::env::var("PACS_TEST_DATABASE_URL") else {
        assert!(
            std::env::var_os("CI").is_none(),
            "CI 必须设置 PACS_TEST_DATABASE_URL"
        );
        eprintln!("\n>>> 跳过高级队列数据库测试: 未设置 PACS_TEST_DATABASE_URL\n");
        return None;
    };
    if !Postgres::database_exists(&url).await.unwrap_or(false) {
        let _ = Postgres::create_database(&url).await;
    }
    let pool = pacs_db::connect(&url).await.expect("应能连接测试库");
    pacs_db::migrate(&pool).await.expect("迁移应能应用");
    Some(pool)
}

fn filter<'a>(query: &'a str) -> QueueFilter<'a> {
    QueueFilter {
        query,
        modality: None,
        body_part: None,
        report_status: None,
        institution: None,
        date_from: None,
        date_to: None,
    }
}

#[allow(clippy::too_many_arguments)]
async fn ingest_queue_instance(
    pool: &PgPool,
    study_uid: &str,
    series_uid: &str,
    patient_id: &str,
    patient_name: &str,
    study_date: &str,
    modality: &str,
    body_part: &str,
    institution: Option<&str>,
    marker: u8,
) -> Ingested {
    let sop_uid = unique_uid();
    let mut object = ct_instance(study_uid, series_uid, &sop_uid);
    for element in [
        DataElement::new(tags::PATIENT_ID, VR::LO, patient_id),
        DataElement::new(tags::PATIENT_NAME, VR::PN, patient_name),
        DataElement::new(tags::STUDY_DATE, VR::DA, study_date),
        DataElement::new(tags::MODALITY, VR::CS, modality),
        DataElement::new(tags::BODY_PART_EXAMINED, VR::CS, body_part),
    ] {
        object.put(element);
    }
    if let Some(institution) = institution {
        object.put(DataElement::new(
            tags::INSTITUTION_NAME,
            VR::LO,
            institution,
        ));
    }
    let metadata = extract_metadata(&object).expect("夹具应能提取");
    let relative_path = format!("queue/{sop_uid}.dcm");
    ingest_instance(
        pool,
        &metadata,
        StorageRecord {
            relative_path: &relative_path,
            size: 100,
            sha256: &[marker; 32],
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn queue_filters_json_institution_and_report_states() {
    let Some(pool) = pool().await else { return };
    let suffix = Uuid::new_v4();
    let patient_id = format!("QUEUE-{suffix}");
    let patient_name = format!("Queue^{suffix}");
    let study_uid = unique_uid();
    let series_uid = unique_uid();
    let author: i64 = sqlx::query_scalar(
        "INSERT INTO users(institution_id,username,password_hash,role)
         VALUES(1,$1,'unused','radiologist') RETURNING id",
    )
    .bind(format!("queue-author-{suffix}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let other_user: i64 = sqlx::query_scalar(
        "INSERT INTO users(institution_id,username,password_hash,role)
         VALUES(1,$1,'unused','radiologist') RETURNING id",
    )
    .bind(format!("queue-other-{suffix}"))
    .fetch_one(&pool)
    .await
    .unwrap();

    let ingested = ingest_queue_instance(
        &pool,
        &study_uid,
        &series_uid,
        &patient_id,
        &patient_name,
        "20240315",
        "CT",
        "CHEST",
        Some("Queue Hospital"),
        9,
    )
    .await;

    // Put the first series behind a granted local device, then add a second
    // series from a granted device belonging to another institution. A bad
    // cross-tenant grant must not broaden the non-admin queue.
    let visible_device = Uuid::new_v4();
    let hidden_device = Uuid::new_v4();
    let foreign_institution: i64 =
        sqlx::query_scalar("INSERT INTO institutions(code,name) VALUES($1,$2) RETURNING id")
            .bind(format!("queue-foreign-{suffix}"))
            .bind(format!("Queue foreign institution {suffix}"))
            .fetch_one(&pool)
            .await
            .unwrap();
    let device_token = suffix.simple().to_string();
    for (device, label, institution_id) in [
        (&visible_device, "visible", 1),
        (&hidden_device, "hidden", foreign_institution),
    ] {
        sqlx::query(
            "INSERT INTO dicom_devices(id,institution_id,name,calling_ae_title,source_ip,status)
             VALUES($1,$2,$3,$4,$5,'active')",
        )
        .bind(device)
        .bind(institution_id)
        .bind(format!("Queue {label} device"))
        .bind(format!("Q{}{}", &label[..1], &device_token[..8]))
        .bind(format!(
            "192.0.2.{}",
            if *device == visible_device { 1 } else { 2 }
        ))
        .execute(&pool)
        .await
        .unwrap();
    }
    let second_series_uid = unique_uid();
    ingest_queue_instance(
        &pool,
        &study_uid,
        &second_series_uid,
        &patient_id,
        &patient_name,
        "20240315",
        "MR",
        "HEAD",
        None,
        10,
    )
    .await;
    sqlx::query(
        "UPDATE series SET source_device_fk=$1,source_status='trusted'
         WHERE series_instance_uid=$2",
    )
    .bind(visible_device)
    .bind(&series_uid)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE series SET source_device_fk=$1,source_status='trusted'
         WHERE series_instance_uid=$2",
    )
    .bind(hidden_device)
    .bind(&second_series_uid)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_device_grants(user_fk,device_fk,granted_by)
         VALUES($1,$2,$1),($1,$3,$1)",
    )
    .bind(author)
    .bind(visible_device)
    .bind(hidden_device)
    .execute(&pool)
    .await
    .unwrap();

    let rows = list_queue_studies(
        &pool,
        1,
        author,
        true,
        filter(&patient_id),
        QueueSort::StudyDate,
        true,
        20,
        0,
    )
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].patient_key, ingested.patient_id);
    assert_eq!(rows[0].patient_id, patient_id);
    assert_eq!(rows[0].modalities, vec!["CT", "MR"]);
    assert_eq!(rows[0].body_parts, vec!["CHEST", "HEAD"]);
    assert_eq!(rows[0].series_count, 2);
    assert_eq!(rows[0].institution_name.as_deref(), Some("Queue Hospital"));
    assert_eq!(rows[0].report_status, "pending");
    let name_query = format!("Queue^{suffix}");
    let name_rows = list_queue_studies(
        &pool,
        1,
        author,
        true,
        filter(&name_query),
        QueueSort::StudyDate,
        true,
        20,
        0,
    )
    .await
    .unwrap();
    assert_eq!(name_rows.len(), 1, "姓名应支持包含匹配");

    let visible_rows = list_queue_studies(
        &pool,
        1,
        author,
        false,
        filter(&patient_id),
        QueueSort::StudyDate,
        true,
        20,
        0,
    )
    .await
    .unwrap();
    assert_eq!(visible_rows.len(), 1);
    assert_eq!(visible_rows[0].modalities, vec!["CT"]);
    assert_eq!(visible_rows[0].series_count, 1);
    assert_eq!(visible_rows[0].body_parts, vec!["CHEST"]);

    assert!(
        list_queue_studies(
            &pool,
            1,
            other_user,
            false,
            filter(&patient_id),
            QueueSort::StudyDate,
            true,
            20,
            0,
        )
        .await
        .unwrap()
        .is_empty(),
        "没有任何设备授权的用户不能看到检查行"
    );
    assert!(
        list_queue_studies(
            &pool,
            foreign_institution,
            author,
            true,
            filter(&patient_id),
            QueueSort::StudyDate,
            true,
            20,
            0,
        )
        .await
        .unwrap()
        .is_empty(),
        "机构边界不能被管理员角色绕过"
    );

    let mut hidden_modality_filter = filter(&patient_id);
    hidden_modality_filter.modality = Some("MR");
    assert!(
        list_queue_studies(
            &pool,
            1,
            author,
            false,
            hidden_modality_filter,
            QueueSort::StudyDate,
            true,
            20,
            0,
        )
        .await
        .unwrap()
        .is_empty(),
        "普通用户不能用隐藏序列的模态命中检查"
    );
    let mut hidden_body_filter = filter(&patient_id);
    hidden_body_filter.body_part = Some("HEAD");
    assert!(
        list_queue_studies(
            &pool,
            1,
            author,
            false,
            hidden_body_filter,
            QueueSort::StudyDate,
            true,
            20,
            0,
        )
        .await
        .unwrap()
        .is_empty()
    );

    let mut by_metadata = filter(&patient_id);
    by_metadata.modality = Some("CT");
    by_metadata.body_part = Some("CHEST");
    by_metadata.institution = Some("Queue Hospital");
    by_metadata.date_from = Some(chrono::NaiveDate::from_ymd_opt(2024, 3, 15).unwrap());
    by_metadata.date_to = by_metadata.date_from;
    assert_eq!(
        list_queue_studies(
            &pool,
            1,
            author,
            true,
            by_metadata,
            QueueSort::Institution,
            false,
            20,
            0,
        )
        .await
        .unwrap()
        .len(),
        1
    );
    let mut wrong_date = filter(&patient_id);
    wrong_date.date_from = Some(chrono::NaiveDate::from_ymd_opt(2024, 3, 16).unwrap());
    assert!(
        list_queue_studies(
            &pool,
            1,
            author,
            true,
            wrong_date,
            QueueSort::StudyDate,
            false,
            20,
            0,
        )
        .await
        .unwrap()
        .is_empty()
    );

    let mut pending_filter = filter(&patient_id);
    pending_filter.report_status = Some("pending");
    assert_eq!(
        list_queue_studies(
            &pool,
            1,
            author,
            true,
            pending_filter,
            QueueSort::StudyDate,
            true,
            20,
            0,
        )
        .await
        .unwrap()
        .len(),
        1
    );

    sqlx::query(
        "INSERT INTO diagnostic_reports(id,institution_id,study_fk,author_fk,status)
         VALUES($1,1,$2,$3,'draft')",
    )
    .bind(Uuid::new_v4())
    .bind(ingested.study_id)
    .bind(author)
    .execute(&pool)
    .await
    .unwrap();
    let writing = list_queue_studies(
        &pool,
        1,
        author,
        true,
        filter(&patient_id),
        QueueSort::ReportStatus,
        true,
        20,
        0,
    )
    .await
    .unwrap();
    assert_eq!(writing[0].report_status, "writing");
    let mut writing_filter = filter(&patient_id);
    writing_filter.report_status = Some("writing");
    assert_eq!(
        list_queue_studies(
            &pool,
            1,
            author,
            true,
            writing_filter,
            QueueSort::ReportStatus,
            true,
            20,
            0,
        )
        .await
        .unwrap()
        .len(),
        1
    );
    let locked = list_queue_studies(
        &pool,
        1,
        other_user,
        true,
        filter(&patient_id),
        QueueSort::StudyDate,
        true,
        20,
        0,
    )
    .await
    .unwrap();
    assert_eq!(locked[0].report_status, "locked");
    let mut locked_filter = filter(&patient_id);
    locked_filter.report_status = Some("locked");
    assert_eq!(
        list_queue_studies(
            &pool,
            1,
            other_user,
            true,
            locked_filter,
            QueueSort::ReportStatus,
            true,
            20,
            0,
        )
        .await
        .unwrap()
        .len(),
        1
    );

    sqlx::query("UPDATE diagnostic_reports SET status='submitted' WHERE study_fk=$1")
        .bind(ingested.study_id)
        .execute(&pool)
        .await
        .unwrap();
    let submitted_for_author = list_queue_studies(
        &pool,
        1,
        author,
        true,
        filter(&patient_id),
        QueueSort::ReportStatus,
        true,
        20,
        0,
    )
    .await
    .unwrap();
    let submitted_for_other = list_queue_studies(
        &pool,
        1,
        other_user,
        true,
        filter(&patient_id),
        QueueSort::ReportStatus,
        true,
        20,
        0,
    )
    .await
    .unwrap();
    assert_eq!(submitted_for_author[0].report_status, "writing");
    assert_eq!(submitted_for_other[0].report_status, "locked");

    sqlx::query(
        "UPDATE diagnostic_reports SET status='signed'
         WHERE study_fk=$1",
    )
    .bind(ingested.study_id)
    .execute(&pool)
    .await
    .unwrap();
    let signed = list_queue_studies(
        &pool,
        1,
        author,
        true,
        filter(&patient_id),
        QueueSort::StudyDate,
        true,
        20,
        0,
    )
    .await
    .unwrap();
    assert_eq!(signed[0].report_status, "signed");
    let mut signed_filter = filter(&patient_id);
    signed_filter.report_status = Some("signed");
    assert_eq!(
        list_queue_studies(
            &pool,
            1,
            author,
            true,
            signed_filter,
            QueueSort::ReportStatus,
            true,
            20,
            0,
        )
        .await
        .unwrap()
        .len(),
        1
    );

    // Keep the shared integration database repeatable. The study cascade removes
    // its series, instances, and report snapshots before the two test users go.
    sqlx::query("DELETE FROM patients WHERE id = $1")
        .bind(ingested.patient_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id IN ($1, $2)")
        .bind(author)
        .bind(other_user)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM dicom_devices WHERE id IN ($1, $2)")
        .bind(visible_device)
        .bind(hidden_device)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM institutions WHERE id = $1")
        .bind(foreign_institution)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn queue_sorting_and_pagination_are_applied_by_the_database() {
    let Some(pool) = pool().await else { return };
    let suffix = Uuid::new_v4();
    let query = format!("QUEUE-SORT-{suffix}");
    let cases = [
        ("C", "QueueSort^Charlie", "20250103", "US", "Zulu Hospital"),
        ("A", "QueueSort^Alpha", "20230101", "CT", "Alpha Hospital"),
        ("B", "QueueSort^Bravo", "20240102", "MR", "Metro Hospital"),
    ];
    let mut created = Vec::new();
    for (index, (label, name, date, modality, institution)) in cases.iter().enumerate() {
        let study_uid = unique_uid();
        let ingested = ingest_queue_instance(
            &pool,
            &study_uid,
            &unique_uid(),
            &format!("{query}-{label}"),
            name,
            date,
            modality,
            "CHEST",
            Some(institution),
            20 + u8::try_from(index).unwrap(),
        )
        .await;
        created.push((study_uid, ingested.patient_id));
    }

    let page = |sort, descending, offset| {
        list_queue_studies(
            &pool,
            1,
            0,
            true,
            filter(&query),
            sort,
            descending,
            1,
            offset,
        )
    };
    let oldest = page(QueueSort::StudyDate, false, 0).await.unwrap();
    let middle = page(QueueSort::StudyDate, false, 1).await.unwrap();
    let newest = page(QueueSort::StudyDate, true, 0).await.unwrap();
    assert_eq!(oldest[0].patient_name.as_deref(), Some("QueueSort^Alpha"));
    assert_eq!(middle[0].patient_name.as_deref(), Some("QueueSort^Bravo"));
    assert_eq!(newest[0].patient_name.as_deref(), Some("QueueSort^Charlie"));

    let by_name = list_queue_studies(
        &pool,
        1,
        0,
        true,
        filter(&query),
        QueueSort::PatientName,
        false,
        10,
        0,
    )
    .await
    .unwrap();
    assert_eq!(
        by_name
            .iter()
            .map(|row| row.patient_name.as_deref().unwrap())
            .collect::<Vec<_>>(),
        vec!["QueueSort^Alpha", "QueueSort^Bravo", "QueueSort^Charlie"]
    );

    let by_modality = list_queue_studies(
        &pool,
        1,
        0,
        true,
        filter(&query),
        QueueSort::Modality,
        false,
        10,
        0,
    )
    .await
    .unwrap();
    assert_eq!(
        by_modality
            .iter()
            .map(|row| row.modalities[0].as_str())
            .collect::<Vec<_>>(),
        vec!["CT", "MR", "US"]
    );

    let by_institution = list_queue_studies(
        &pool,
        1,
        0,
        true,
        filter(&query),
        QueueSort::Institution,
        false,
        10,
        0,
    )
    .await
    .unwrap();
    assert_eq!(
        by_institution
            .iter()
            .map(|row| row.institution_name.as_deref().unwrap())
            .collect::<Vec<_>>(),
        vec!["Alpha Hospital", "Metro Hospital", "Zulu Hospital"]
    );

    for (_, patient_id) in created {
        sqlx::query("DELETE FROM patients WHERE id = $1")
            .bind(patient_id)
            .execute(&pool)
            .await
            .unwrap();
    }
}
