//! Shared Viewer annotation persistence and optimistic concurrency invariants.

use chrono::{Duration, Utc};
use dicom::core::{DataElement, VR};
use dicom::dictionary_std::tags;
use pacs_core::fixture::{ct_instance, unique_uid};
use pacs_core::{InstanceMetadata, extract_metadata};
use pacs_db::{
    AnnotationUpdate, DbError, NewAnnotation, StorageRecord, create_annotation, ingest_instance,
    list_annotations, update_annotation,
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
        eprintln!("\n>>> 跳过共享标注数据库测试: 未设置 PACS_TEST_DATABASE_URL\n");
        return None;
    };
    if !Postgres::database_exists(&url).await.unwrap_or(false)
        && let Err(error) = Postgres::create_database(&url).await
    {
        assert!(
            Postgres::database_exists(&url).await.unwrap_or(false),
            "创建测试库失败: {error}"
        );
    }
    let pool = pacs_db::connect(&url).await.expect("应能连接测试库");
    pacs_db::migrate(&pool).await.expect("共享标注迁移应能应用");
    Some(pool)
}

macro_rules! require_db {
    () => {
        match pool().await {
            Some(pool) => pool,
            None => return,
        }
    };
}

fn metadata(study_uid: &str, series_uid: &str, sop_uid: &str) -> InstanceMetadata {
    let mut object = ct_instance(study_uid, series_uid, sop_uid);
    object.put(DataElement::new(
        tags::PATIENT_ID,
        VR::LO,
        format!("ANNOTATION-{}", Uuid::new_v4()),
    ));
    extract_metadata(&object).unwrap()
}

#[tokio::test]
async fn annotations_are_isolated_versioned_and_soft_deleted() {
    let pool = require_db!();
    let study_uid = unique_uid();
    let series_uid = unique_uid();
    let sop_uid = unique_uid();
    let metadata = metadata(&study_uid, &series_uid, &sop_uid);
    ingest_instance(
        &pool,
        &metadata,
        StorageRecord {
            relative_path: &format!("test/annotations/{sop_uid}.dcm"),
            size: 1024,
            sha256: &[0x61; 32],
        },
    )
    .await
    .unwrap();
    let user_id: i64 = sqlx::query_scalar(
        "INSERT INTO users (institution_id, username, password_hash, role)
         VALUES (1, $1, 'unused', 'radiologist') RETURNING id",
    )
    .bind(format!("annotation-{}", Uuid::new_v4()))
    .fetch_one(&pool)
    .await
    .unwrap();
    let id = Uuid::new_v4();
    let geometry = serde_json::json!({"start": {"x": 1.0, "y": 2.0}, "end": {"x": 5.0, "y": 8.0}});
    let created = create_annotation(
        &pool,
        NewAnnotation {
            id,
            institution_id: 1,
            study_instance_uid: &study_uid,
            series_instance_uid: &series_uid,
            sop_instance_uid: Some(&sop_uid),
            frame_number: Some(1),
            coordinate_space: "image",
            mpr_plane: None,
            schema_version: 1,
            kind: "length",
            geometry: &geometry,
            user_id,
        },
    )
    .await
    .unwrap();
    assert_eq!(created.revision, 1);
    assert_eq!(
        list_annotations(&pool, 1, &study_uid, &series_uid, None)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        list_annotations(&pool, 999, &study_uid, &series_uid, None)
            .await
            .unwrap()
            .is_empty()
    );

    let changed = serde_json::json!({"start": {"x": 2.0, "y": 3.0}, "end": {"x": 6.0, "y": 9.0}});
    let updated = update_annotation(
        &pool,
        AnnotationUpdate {
            institution_id: 1,
            study_instance_uid: &study_uid,
            series_instance_uid: &series_uid,
            annotation_id: id,
            expected_revision: 1,
            geometry: &changed,
            deleted: false,
            user_id,
        },
    )
    .await
    .unwrap();
    assert_eq!(updated.revision, 2);
    assert!(matches!(
        update_annotation(
            &pool,
            AnnotationUpdate {
                institution_id: 1,
                study_instance_uid: &study_uid,
                series_instance_uid: &series_uid,
                annotation_id: id,
                expected_revision: 1,
                geometry: &geometry,
                deleted: false,
                user_id,
            }
        )
        .await,
        Err(DbError::Conflict(_))
    ));

    let deleted = update_annotation(
        &pool,
        AnnotationUpdate {
            institution_id: 1,
            study_instance_uid: &study_uid,
            series_instance_uid: &series_uid,
            annotation_id: id,
            expected_revision: 2,
            geometry: &changed,
            deleted: true,
            user_id,
        },
    )
    .await
    .unwrap();
    assert!(deleted.deleted_at.is_some());
    assert!(
        list_annotations(&pool, 1, &study_uid, &series_uid, None)
            .await
            .unwrap()
            .is_empty()
    );
    let incremental = list_annotations(
        &pool,
        1,
        &study_uid,
        &series_uid,
        Some(Utc::now() - Duration::minutes(1)),
    )
    .await
    .unwrap();
    assert_eq!(incremental.len(), 1);
    assert!(incremental[0].deleted_at.is_some());
}
