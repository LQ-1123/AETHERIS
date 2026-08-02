//! 入库路径的集成测试。跑在真实 Postgres 上 —— 这些 SQL 只有真库能验证。
//!
//! 需要 `PACS_TEST_DATABASE_URL`(见 `.env.example`)。未设置时测试会跳过并
//! 打印醒目提示:CI 里一定设置,所以漏跑不会悄悄溜过去。
//!
//! 隔离靠 UID 唯一:每个测试用自己的 UUID 派生 UID 和 PatientID,
//! 因此可以并行跑,也不需要清库。

use dicom::core::{DataElement, VR};
use dicom::dictionary_std::tags;
use pacs_core::fixture::{ct_instance, unique_uid};
use pacs_core::{InstanceMetadata, extract_metadata};
use pacs_db::{StorageRecord, ingest_instance};
use pacs_store::{InstanceKey, Store};
use sqlx::migrate::MigrateDatabase;
use sqlx::{PgPool, Postgres};

/// 连上测试库并确保迁移已应用;没配测试库就返回 `None`。
///
/// 库不存在会自动建 —— 开发者和 CI 只需要给一个连接串,不用先手动 createdb。
/// 少一个手工步骤,就少一次"忘了建库导致测试静默跳过"。
async fn pool() -> Option<PgPool> {
    dotenvy::dotenv().ok();
    let Ok(url) = std::env::var("PACS_TEST_DATABASE_URL") else {
        // 本地缺配置时跳过是方便;CI 里跳过就等于没测,必须直接失败。
        assert!(
            std::env::var_os("CI").is_none(),
            "CI 环境必须设置 PACS_TEST_DATABASE_URL,数据库测试不允许跳过"
        );
        eprintln!(
            "\n>>> 跳过数据库测试:未设置 PACS_TEST_DATABASE_URL。\
             \n>>> 这些测试覆盖入库 SQL,本地请照 .env.example 配置后重跑。\n"
        );
        return None;
    };
    if !Postgres::database_exists(&url).await.unwrap_or(false) {
        // 测试是并行跑的,多个任务会同时发现库不存在并抢着建,只有一个能成功。
        // 其余收到的"已存在"不是失败 —— 重新确认一次库在不在即可。
        if let Err(error) = Postgres::create_database(&url).await {
            assert!(
                Postgres::database_exists(&url).await.unwrap_or(false),
                "创建测试库失败(连接账号需要 CREATEDB 权限):{error}"
            );
        }
    }
    let pool = pacs_db::connect(&url).await.expect("应能连上测试库");
    pacs_db::migrate(&pool).await.expect("迁移应能应用");
    Some(pool)
}

/// 每次调用都产出一份 UID 与 PatientID 全新的元数据,测试之间互不干扰。
fn fresh_metadata(study: &str, series: &str) -> InstanceMetadata {
    let mut obj = ct_instance(study, series, &unique_uid());
    obj.put(DataElement::new(
        tags::PATIENT_ID,
        VR::LO,
        format!("PID-{}", uuid::Uuid::new_v4()),
    ));
    extract_metadata(&obj).expect("夹具应能提取")
}

fn storage() -> StorageRecord<'static> {
    StorageRecord {
        relative_path: "ab/cd/study/series/instance.dcm",
        size: 4096,
        sha256: &[0x42; 32],
    }
}

/// 没配测试库就直接结束该测试(已在 [`pool`] 里打印提示)。
macro_rules! require_db {
    () => {
        match pool().await {
            Some(pool) => pool,
            None => return,
        }
    };
}

#[tokio::test]
async fn ingests_all_four_levels() {
    let pool = require_db!();
    let (study_uid, series_uid) = (unique_uid(), unique_uid());
    let metadata = fresh_metadata(&study_uid, &series_uid);

    let ingested = ingest_instance(&pool, &metadata, storage())
        .await
        .expect("应能入库");
    assert!(ingested.instance_created);

    let (name, birth): (Option<String>, Option<chrono::NaiveDate>) =
        sqlx::query_as("SELECT name_normalized, birth_date FROM patients WHERE id = $1")
            .bind(ingested.patient_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(name.as_deref(), Some("DOE^JOHN"));
    assert_eq!(birth, chrono::NaiveDate::from_ymd_opt(1980, 1, 15));

    let (uid, date, accession): (String, Option<chrono::NaiveDate>, Option<String>) =
        sqlx::query_as(
            "SELECT study_instance_uid, study_date, accession_number FROM studies WHERE id = $1",
        )
        .bind(ingested.study_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(uid, study_uid);
    assert_eq!(date, chrono::NaiveDate::from_ymd_opt(2024, 3, 15));
    assert_eq!(accession.as_deref(), Some("ACC-42"));

    let (modality, study_fk): (Option<String>, i64) =
        sqlx::query_as("SELECT modality, study_fk FROM series WHERE id = $1")
            .bind(ingested.series_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(modality.as_deref(), Some("CT"));
    assert_eq!(study_fk, ingested.study_id, "序列应挂在刚建的检查下");

    let (path, size, position): (String, i64, Option<Vec<f64>>) = sqlx::query_as(
        "SELECT storage_path, file_size, image_position_patient FROM instances WHERE id = $1",
    )
    .bind(ingested.instance_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(path, "ab/cd/study/series/instance.dcm");
    assert_eq!(size, 4096);
    // CT 序列排序的依据,必须原样存下来
    assert_eq!(position, Some(vec![-120.5, -130.0, -45.25]));
}

/// 设备重传同一实例很常见,必须幂等 —— 不报错,也不产生第二行。
#[tokio::test]
async fn retransmission_is_idempotent() {
    let pool = require_db!();
    let (study_uid, series_uid) = (unique_uid(), unique_uid());
    let metadata = fresh_metadata(&study_uid, &series_uid);

    let first = ingest_instance(&pool, &metadata, storage()).await.unwrap();
    let second = ingest_instance(&pool, &metadata, storage()).await.unwrap();

    assert_eq!(first.instance_id, second.instance_id);
    assert!(first.instance_created);
    assert!(!second.instance_created, "第二次应识别为覆盖而不是新增");

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM instances WHERE series_fk = $1")
        .bind(first.series_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "重传不该多出一行");

    let instances: i32 =
        sqlx::query_scalar("SELECT number_of_instances FROM studies WHERE id = $1")
            .bind(first.study_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(instances, 1, "计数是重算的,重传后不该漂移到 2");
}

/// C-FIND 的 ModalitiesInStudy / NumberOfStudyRelated* 返回键靠这些聚合列。
#[tokio::test]
async fn aggregates_counts_and_modalities_across_series() {
    let pool = require_db!();
    let study_uid = unique_uid();
    let (series_a, series_b) = (unique_uid(), unique_uid());

    let mut last = None;
    for series in [&series_a, &series_a, &series_b] {
        last = Some(
            ingest_instance(&pool, &fresh_metadata(&study_uid, series), storage())
                .await
                .unwrap(),
        );
    }
    let ingested = last.unwrap();

    let (series_count, instance_count, modalities): (i32, i32, Vec<String>) = sqlx::query_as(
        "SELECT number_of_series, number_of_instances, modalities FROM studies WHERE id = $1",
    )
    .bind(ingested.study_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(series_count, 2);
    assert_eq!(instance_count, 3);
    assert_eq!(modalities, vec!["CT".to_owned()]);

    let per_series: i32 =
        sqlx::query_scalar("SELECT number_of_instances FROM series WHERE series_instance_uid = $1")
            .bind(&series_a)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(per_series, 2);
}

/// 同一检查的不同实例头信息完整度不同。后到的残缺实例不能把已存好的字段抹掉。
#[tokio::test]
async fn sparse_instance_does_not_erase_existing_fields() {
    let pool = require_db!();
    let (study_uid, series_uid) = (unique_uid(), unique_uid());

    let complete = fresh_metadata(&study_uid, &series_uid);
    ingest_instance(&pool, &complete, storage()).await.unwrap();

    // 第二个实例没带 StudyDescription 和 AccessionNumber
    let mut obj = ct_instance(&study_uid, &series_uid, &unique_uid());
    obj.remove_element(tags::STUDY_DESCRIPTION);
    obj.remove_element(tags::ACCESSION_NUMBER);
    let sparse = extract_metadata(&obj).unwrap();
    let ingested = ingest_instance(&pool, &sparse, storage()).await.unwrap();

    let (description, accession): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT description, accession_number FROM studies WHERE id = $1")
            .bind(ingested.study_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        description.as_deref(),
        Some("CHEST CT"),
        "已有的检查描述不该被残缺实例抹成 NULL"
    );
    assert_eq!(accession.as_deref(), Some("ACC-42"));
}

#[tokio::test]
async fn attributes_are_stored_as_queryable_jsonb() {
    let pool = require_db!();
    let metadata = fresh_metadata(&unique_uid(), &unique_uid());
    let ingested = ingest_instance(&pool, &metadata, storage()).await.unwrap();

    // 直接用 JSON 路径查 —— 阶段 5 的 QIDO-RS 就靠这个回属性
    let window_center: Option<String> = sqlx::query_scalar(
        "SELECT attributes #>> '{00281050,Value,0}' FROM instances WHERE id = $1",
    )
    .bind(ingested.instance_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(window_center.as_deref(), Some("-600"));
}

/// 删检查要能把底下的序列和实例一并带走,不留悬挂行。
#[tokio::test]
async fn deleting_a_study_cascades() {
    let pool = require_db!();
    let metadata = fresh_metadata(&unique_uid(), &unique_uid());
    let ingested = ingest_instance(&pool, &metadata, storage()).await.unwrap();

    sqlx::query("DELETE FROM studies WHERE id = $1")
        .bind(ingested.study_id)
        .execute(&pool)
        .await
        .unwrap();

    let orphans: i64 = sqlx::query_scalar("SELECT count(*) FROM instances WHERE id = $1")
        .bind(ingested.instance_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(orphans, 0);
}

/// 阶段 1 的交付标准:一个 DICOM 文件既正确落盘、又正确入库,两者对得上。
#[tokio::test]
async fn end_to_end_file_lands_on_disk_and_in_database() {
    let pool = require_db!();
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).await.unwrap();

    let (study_uid, series_uid, sop_uid) = (unique_uid(), unique_uid(), unique_uid());
    let obj = ct_instance(&study_uid, &series_uid, &sop_uid);
    let mut encoded = Vec::new();
    obj.write_all(&mut encoded).unwrap();
    let metadata = extract_metadata(&obj).unwrap();

    // 顺序和 C-STORE 一致:先落盘(已 fsync),再提交数据库事务
    let stored = store
        .store(
            InstanceKey {
                study: &metadata.study.uid,
                series: &metadata.series.uid,
                sop: &metadata.instance.uid,
            },
            &encoded,
        )
        .await
        .expect("应能落盘");

    let ingested = ingest_instance(
        &pool,
        &metadata,
        StorageRecord {
            relative_path: &stored.relative_path,
            size: stored.size,
            sha256: &stored.sha256,
        },
    )
    .await
    .expect("应能入库");

    // 库里存的路径必须能把文件原样取回来 —— WADO 就是这么取的
    let (path, size, digest): (String, i64, Vec<u8>) =
        sqlx::query_as("SELECT storage_path, file_size, file_sha256 FROM instances WHERE id = $1")
            .bind(ingested.instance_id)
            .fetch_one(&pool)
            .await
            .unwrap();

    let absolute = store.resolve(&path).expect("库里的路径应合法");
    let from_disk = tokio::fs::read(&absolute).await.expect("应能读回文件");
    assert_eq!(from_disk, encoded, "读回的字节应与写入的完全一致");
    assert_eq!(size as usize, encoded.len());

    use sha2::{Digest, Sha256};
    assert_eq!(
        digest,
        Sha256::digest(&encoded).to_vec(),
        "库里的校验和应能验证盘上文件的完整性"
    );

    // 再解析一遍读回的文件,确认 UID 三元组与库里的记录一致
    let reparsed = dicom::object::from_reader(std::io::Cursor::new(&from_disk)).unwrap();
    let round_tripped = extract_metadata(&reparsed).unwrap();
    assert_eq!(round_tripped.instance.uid.as_str(), sop_uid);
    assert_eq!(round_tripped.study.uid.as_str(), study_uid);
}
