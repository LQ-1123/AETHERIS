//! 检查申请单机构边界、编辑/绑定状态机与工作量聚合。

use chrono::Utc;
use sqlx::migrate::MigrateDatabase;
use sqlx::{PgPool, Postgres};
use uuid::Uuid;

async fn pool() -> Option<PgPool> {
    dotenvy::dotenv().ok();
    let Ok(url) = std::env::var("PACS_TEST_DATABASE_URL") else {
        return None;
    };
    if !Postgres::database_exists(&url).await.unwrap_or(false) {
        let _ = Postgres::create_database(&url).await;
    }
    let pool = pacs_db::connect(&url).await.unwrap();
    pacs_db::migrate(&pool).await.unwrap();
    Some(pool)
}

#[tokio::test]
async fn exam_requests_are_tenant_scoped_bind_once_and_aggregate_workload() {
    let Some(pool) = pool().await else { return };
    let suffix = Uuid::new_v4();
    let foreign_institution: i64 =
        sqlx::query_scalar("INSERT INTO institutions(code,name) VALUES($1,$2) RETURNING id")
            .bind(format!("exam-foreign-{suffix}"))
            .bind("申请单测试外部机构")
            .fetch_one(&pool)
            .await
            .unwrap();
    let technician: i64 = sqlx::query_scalar(
        "INSERT INTO users(institution_id,username,password_hash,role) VALUES(1,$1,'x','technician') RETURNING id",
    )
    .bind(format!("exam-tech-{suffix}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let doctor: i64 = sqlx::query_scalar(
        "INSERT INTO users(institution_id,username,password_hash,role) VALUES(1,$1,'x','radiologist') RETURNING id",
    )
    .bind(format!("exam-doctor-{suffix}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let foreign_technician: i64 = sqlx::query_scalar(
        "INSERT INTO users(institution_id,username,password_hash,role) VALUES($1,$2,'x','technician') RETURNING id",
    )
    .bind(foreign_institution)
    .bind(format!("exam-foreign-tech-{suffix}"))
    .fetch_one(&pool)
    .await
    .unwrap();

    let request = pacs_db::create_exam_request(
        &pool,
        1,
        technician,
        pacs_db::ExamRequestInput {
            patient_id: "EXAM-001",
            patient_name: "王测试",
            patient_birth_date: None,
            patient_sex: Some("F"),
            modality: "ct",
            body_part: "胸部",
            request_type: "增强",
            clinical_indication: "胸痛，排除肺栓塞",
            scheduled_at: Some(Utc::now()),
        },
    )
    .await
    .unwrap();
    assert_eq!(request.modality, "CT");
    assert!(
        pacs_db::list_exam_requests(&pool, 1, Some("pending"), 200, 0)
            .await
            .unwrap()
            .iter()
            .any(|entry| entry.id == request.id)
    );
    assert!(
        pacs_db::list_exam_requests(&pool, foreign_institution, None, 20, 0)
            .await
            .unwrap()
            .is_empty()
    );

    let patient: i64 = sqlx::query_scalar(
        "INSERT INTO patients(institution_id,patient_id,name) VALUES(1,$1,$2) RETURNING id",
    )
    .bind(format!("EXAM-PATIENT-{suffix}"))
    .bind("王测试")
    .fetch_one(&pool)
    .await
    .unwrap();
    let study_uid = format!("1.2.826.0.1.3680043.9.8800.{}", suffix.simple());
    sqlx::query(
        "INSERT INTO studies(institution_id,patient_fk,study_instance_uid,study_date,description) VALUES(1,$1,$2,CURRENT_DATE,'胸部增强 CT')",
    )
    .bind(patient)
    .bind(&study_uid)
    .execute(&pool)
    .await
    .unwrap();
    let study_fk: i64 = sqlx::query_scalar("SELECT id FROM studies WHERE study_instance_uid=$1")
        .bind(&study_uid)
        .fetch_one(&pool)
        .await
        .unwrap();
    let series_fk: i64 = sqlx::query_scalar(
        "INSERT INTO series(study_fk,series_instance_uid,modality) VALUES($1,$2,'CT') RETURNING id",
    )
    .bind(study_fk)
    .bind(format!("1.2.826.0.1.3680043.9.8801.{}", suffix.simple()))
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO instances(series_fk,sop_instance_uid,transfer_syntax_uid,storage_path,file_size,file_sha256,logical_instance_id) VALUES($1,$2,'1.2.840.10008.1.2.1','exam/test.dcm',1,'\\x00',gen_random_uuid())",
    )
    .bind(series_fk)
    .bind(format!("1.2.826.0.1.3680043.9.8802.{}", suffix.simple()))
    .execute(&pool)
    .await
    .unwrap();
    let bound = pacs_db::bind_exam_request(&pool, 1, request.id, &study_uid, request.revision)
        .await
        .unwrap();
    assert_eq!(bound.status, "executed");
    assert!(
        pacs_db::update_exam_request(
            &pool,
            1,
            request.id,
            bound.revision,
            pacs_db::ExamRequestInput {
                patient_id: "EXAM-001",
                patient_name: "禁止修改",
                patient_birth_date: None,
                patient_sex: None,
                modality: "CT",
                body_part: "胸部",
                request_type: "增强",
                clinical_indication: "已执行后不可修改",
                scheduled_at: None,
            },
        )
        .await
        .is_err()
    );

    let second_request = pacs_db::create_exam_request(
        &pool,
        1,
        technician,
        pacs_db::ExamRequestInput {
            patient_id: "EXAM-002",
            patient_name: "重复绑定测试",
            patient_birth_date: None,
            patient_sex: None,
            modality: "CT",
            body_part: "胸部",
            request_type: "平扫",
            clinical_indication: "验证重复绑定与不存在 Study",
            scheduled_at: None,
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        pacs_db::bind_exam_request(
            &pool,
            1,
            second_request.id,
            &study_uid,
            second_request.revision,
        )
        .await,
        Err(pacs_db::DbError::Conflict(_))
    ));
    assert!(matches!(
        pacs_db::bind_exam_request(
            &pool,
            1,
            second_request.id,
            "1.2.826.0.1.3680043.9.9999.1",
            second_request.revision,
        )
        .await,
        Err(pacs_db::DbError::Conflict(_))
    ));
    let still_pending = pacs_db::list_exam_requests(&pool, 1, Some("pending"), 200, 0)
        .await
        .unwrap();
    assert!(
        still_pending
            .iter()
            .any(|entry| entry.id == second_request.id)
    );

    let report_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO diagnostic_reports(id,institution_id,study_fk,author_fk,status) VALUES($1,1,$2,$3,'signed')",
    )
    .bind(report_id)
    .bind(study_fk)
    .bind(doctor)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO diagnostic_report_versions(id,report_fk,version_number,findings,impression,covered_series_uids,access_incomplete,signed_by) VALUES($1,$2,1,'所见','意见','{}',false,$3)",
    )
    .bind(Uuid::new_v4())
    .bind(report_id)
    .bind(doctor)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO report_review_events(report_fk,actor_fk,action) VALUES($1,$2,'approved')",
    )
    .bind(report_id)
    .bind(doctor)
    .execute(&pool)
    .await
    .unwrap();

    let today = pacs_db::institution_today(&pool, 1).await.unwrap();
    let rows = pacs_db::workload_report(&pool, 1, today, today)
        .await
        .unwrap();
    let tech = rows.iter().find(|row| row.user_id == technician).unwrap();
    assert_eq!(tech.exam_requests_created, 2);
    let doctor_row = rows.iter().find(|row| row.user_id == doctor).unwrap();
    assert_eq!(doctor_row.signed_reports, 1);
    assert_eq!(doctor_row.reviews_completed, 1);

    let _ = foreign_technician;
}

#[tokio::test]
async fn existing_study_request_uses_server_patient_and_is_created_atomically() {
    let Some(pool) = pool().await else { return };
    let suffix = Uuid::new_v4();
    let technician: i64 = sqlx::query_scalar(
        "INSERT INTO users(institution_id,username,password_hash,role) VALUES(1,$1,'x','technician') RETURNING id",
    )
    .bind(format!("existing-study-tech-{suffix}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let patient_id = format!("EXISTING-{suffix}");
    let patient: i64 = sqlx::query_scalar(
        "INSERT INTO patients(institution_id,patient_id,name,birth_date,sex) VALUES(1,$1,'服务端患者','1980-02-03','F') RETURNING id",
    )
    .bind(&patient_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let study_uid = format!("1.2.826.0.1.3680043.9.8810.{}", suffix.simple());
    let study_fk: i64 = sqlx::query_scalar(
        "INSERT INTO studies(institution_id,patient_fk,study_instance_uid,study_date,description) VALUES(1,$1,$2,CURRENT_DATE,'已有胸部 CT') RETURNING id",
    )
    .bind(patient)
    .bind(&study_uid)
    .fetch_one(&pool)
    .await
    .unwrap();
    let series_fk: i64 = sqlx::query_scalar(
        "INSERT INTO series(study_fk,series_instance_uid,modality,body_part_examined) VALUES($1,$2,'CT','CHEST') RETURNING id",
    )
    .bind(study_fk)
    .bind(format!("1.2.826.0.1.3680043.9.8811.{}", suffix.simple()))
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO instances(series_fk,sop_instance_uid,transfer_syntax_uid,storage_path,file_size,file_sha256,logical_instance_id) VALUES($1,$2,'1.2.840.10008.1.2.1','exam/existing.dcm',1,'\\x00',gen_random_uuid())",
    )
    .bind(series_fk)
    .bind(format!("1.2.826.0.1.3680043.9.8812.{}", suffix.simple()))
    .execute(&pool)
    .await
    .unwrap();

    let created = pacs_db::create_exam_request_for_study(
        &pool,
        1,
        technician,
        &study_uid,
        pacs_db::ExistingStudyExamRequestInput {
            modality: "ct",
            body_part: "胸部",
            request_type: "增强",
            clinical_indication: "胸痛，排除肺栓塞",
            scheduled_at: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(created.patient_id, patient_id);
    assert_eq!(created.patient_name, "服务端患者");
    assert_eq!(
        created.patient_birth_date.unwrap().to_string(),
        "1980-02-03"
    );
    assert_eq!(created.patient_sex.as_deref(), Some("F"));
    assert_eq!(created.modality, "CT");
    assert_eq!(created.study_uid.as_deref(), Some(study_uid.as_str()));
    assert_eq!(created.status, "executed");

    let duplicate = pacs_db::create_exam_request_for_study(
        &pool,
        1,
        technician,
        &study_uid,
        pacs_db::ExistingStudyExamRequestInput {
            modality: "CT",
            body_part: "胸部",
            request_type: "平扫",
            clinical_indication: "重复申请不得写入",
            scheduled_at: None,
        },
    )
    .await;
    assert!(matches!(duplicate, Err(pacs_db::DbError::Conflict(_))));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM exam_requests WHERE study_fk=$1")
        .bind(study_fk)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);

    let empty_study_uid = format!("1.2.826.0.1.3680043.9.8813.{}", suffix.simple());
    sqlx::query(
        "INSERT INTO studies(institution_id,patient_fk,study_instance_uid) VALUES(1,$1,$2)",
    )
    .bind(patient)
    .bind(&empty_study_uid)
    .execute(&pool)
    .await
    .unwrap();
    let empty_result = pacs_db::create_exam_request_for_study(
        &pool,
        1,
        technician,
        &empty_study_uid,
        pacs_db::ExistingStudyExamRequestInput {
            modality: "CT",
            body_part: "胸部",
            request_type: "平扫",
            clinical_indication: "空检查不可申请",
            scheduled_at: None,
        },
    )
    .await;
    assert!(matches!(empty_result, Err(pacs_db::DbError::NotFound)));
}
