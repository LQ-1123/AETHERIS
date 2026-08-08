//! Editable segmentation persistence and frame-level optimistic concurrency.

use dicom::core::{DataElement, VR};
use dicom::dictionary_std::tags;
use pacs_core::fixture::{ct_instance, unique_uid};
use pacs_core::{InstanceMetadata, extract_metadata};
use pacs_db::{
    DbError, NewSegmentationProject, SegmentationMaskUpdate, StorageRecord,
    UpdateSegmentationSegmentTags, create_segmentation_project, delete_segmentation_project,
    find_segmentation_segments_by_tag, ingest_instance, list_segmentation_projects,
    list_segmentation_segment_masks, list_segmentation_segments, update_segmentation_segment_tags,
    upsert_segmentation_masks_batch,
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
        eprintln!("\n>>> 跳过分割数据库测试: 未设置 PACS_TEST_DATABASE_URL\n");
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
    pacs_db::migrate(&pool).await.expect("分割迁移应能应用");
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
        format!("SEGMENTATION-{}", Uuid::new_v4()),
    ));
    extract_metadata(&object).unwrap()
}

#[tokio::test]
async fn segmentation_masks_are_sparse_batch_updated_and_versioned() {
    let pool = require_db!();
    let study_uid = unique_uid();
    let series_uid = unique_uid();
    let sop_uid = unique_uid();
    let metadata = metadata(&study_uid, &series_uid, &sop_uid);
    ingest_instance(
        &pool,
        &metadata,
        StorageRecord {
            relative_path: &format!("test/segmentations/{sop_uid}.dcm"),
            size: 1024,
            sha256: &[0x73; 32],
        },
    )
    .await
    .unwrap();
    let user_id: i64 = sqlx::query_scalar(
        "INSERT INTO users (institution_id, username, password_hash, role)
         VALUES (1, $1, 'unused', 'radiologist') RETURNING id",
    )
    .bind(format!("segmentation-{}", Uuid::new_v4()))
    .fetch_one(&pool)
    .await
    .unwrap();
    let project_id = Uuid::new_v4();
    let segment_id = Uuid::new_v4();
    let (project, segment) = create_segmentation_project(
        &pool,
        NewSegmentationProject {
            id: project_id,
            segment_id,
            institution_id: 1,
            study_instance_uid: &study_uid,
            series_instance_uid: &series_uid,
            name: "Manual mask",
            segment_label: "Lesion",
            segment_description: None,
            color: [55, 213, 216],
            algorithm_type: "manual",
            tags: &[],
            user_id,
        },
    )
    .await
    .unwrap();
    assert_eq!(project.id, project_id);
    assert_eq!(segment.id, segment_id);
    assert_eq!(segment.algorithm_type, "manual");
    assert!(segment.tags.is_empty());
    assert_eq!(
        list_segmentation_projects(&pool, 1, &study_uid, &series_uid)
            .await
            .unwrap()
            .len(),
        1
    );

    let tags = vec!["结节".to_owned(), "肺".to_owned()];
    let tagged = update_segmentation_segment_tags(
        &pool,
        UpdateSegmentationSegmentTags {
            institution_id: 1,
            project_id,
            segment_id,
            tags: &tags,
            user_id,
        },
    )
    .await
    .unwrap();
    assert_eq!(tagged.tags, tags);
    assert_eq!(
        find_segmentation_segments_by_tag(&pool, 1, project_id, "结节")
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        find_segmentation_segments_by_tag(&pool, 1, project_id, "肿块")
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        list_segmentation_segments(&pool, 1, project_id)
            .await
            .unwrap()
            .len(),
        1
    );

    let first_rle = [0_u32, 1, 3]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect::<Vec<_>>();
    let created = upsert_segmentation_masks_batch(
        &pool,
        1,
        project_id,
        segment_id,
        &[SegmentationMaskUpdate {
            sop_instance_uid: &sop_uid,
            frame_number: 1,
            rows: 2,
            cols: 2,
            mask_data: &first_rle,
            expected_revision: 0,
        }],
        user_id,
    )
    .await
    .unwrap();
    assert_eq!(created[0].revision, 1);
    assert_eq!(
        list_segmentation_segment_masks(&pool, 1, project_id, segment_id)
            .await
            .unwrap()
            .len(),
        1
    );

    let conflict = upsert_segmentation_masks_batch(
        &pool,
        1,
        project_id,
        segment_id,
        &[SegmentationMaskUpdate {
            sop_instance_uid: &sop_uid,
            frame_number: 1,
            rows: 2,
            cols: 2,
            mask_data: &first_rle,
            expected_revision: 0,
        }],
        user_id,
    )
    .await
    .unwrap_err();
    assert!(matches!(conflict, DbError::Conflict(_)));

    let empty_rle = 4_u32.to_le_bytes();
    let updated = upsert_segmentation_masks_batch(
        &pool,
        1,
        project_id,
        segment_id,
        &[SegmentationMaskUpdate {
            sop_instance_uid: &sop_uid,
            frame_number: 1,
            rows: 2,
            cols: 2,
            mask_data: &empty_rle,
            expected_revision: 1,
        }],
        user_id,
    )
    .await
    .unwrap();
    assert_eq!(updated[0].revision, 2);

    assert!(
        !delete_segmentation_project(&pool, 2, &study_uid, &series_uid, project_id)
            .await
            .unwrap()
    );
    assert!(
        !delete_segmentation_project(&pool, 1, &study_uid, &unique_uid(), project_id)
            .await
            .unwrap()
    );
    assert!(
        delete_segmentation_project(&pool, 1, &study_uid, &series_uid, project_id)
            .await
            .unwrap()
    );
    assert!(
        list_segmentation_projects(&pool, 1, &study_uid, &series_uid)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        list_segmentation_segments(&pool, 1, project_id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        list_segmentation_segment_masks(&pool, 1, project_id, segment_id)
            .await
            .unwrap()
            .is_empty()
    );
}
