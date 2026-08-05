//! `pacs-store` 落盘行为的测试。

use std::path::Path;

use pacs_core::Uid;
use pacs_store::{InstanceKey, Store, StoreError, StoreOutcome, TEMP_DIR};

fn uid(s: &str) -> Uid {
    Uid::parse(s).expect("测试 UID 应合法")
}

struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    study: Uid,
    series: Uid,
    sop: Uid,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().expect("应能建临时目录");
        let store = Store::open(dir.path()).await.expect("应能打开存储");
        Self {
            _dir: dir,
            store,
            study: uid("1.2.826.0.1.3680043.8.498.1"),
            series: uid("1.2.826.0.1.3680043.8.498.2"),
            sop: uid("1.2.826.0.1.3680043.8.498.3"),
        }
    }

    fn key(&self) -> InstanceKey<'_> {
        InstanceKey {
            study: &self.study,
            series: &self.series,
            sop: &self.sop,
        }
    }
}

#[tokio::test]
async fn stores_a_file_and_reports_where() {
    let f = Fixture::new().await;
    let bytes = b"DICM-payload";

    let stored = f.store.store(f.key(), bytes).await.expect("应能落盘");

    assert_eq!(stored.outcome, StoreOutcome::Created);
    assert_eq!(stored.size, bytes.len() as u64);
    assert!(stored.relative_path.ends_with(&format!("{}.dcm", f.sop)));

    // 返回的相对路径必须能读回同样的内容 —— 这是数据库里存的东西
    let absolute = f.store.resolve(&stored.relative_path).expect("路径应合法");
    assert_eq!(tokio::fs::read(&absolute).await.unwrap(), bytes);
}

#[tokio::test]
async fn digest_matches_the_content() {
    let f = Fixture::new().await;
    let stored = f.store.store(f.key(), b"payload").await.unwrap();

    // 库里存的校验和要能用来验完整性,算错了就失去意义
    use sha2::{Digest, Sha256};
    let expected: [u8; 32] = Sha256::digest(b"payload").into();
    assert_eq!(stored.sha256, expected);
}

/// 设备重传同一个实例非常常见,必须幂等而不是报错。
#[tokio::test]
async fn identical_retransmission_is_idempotent() {
    let f = Fixture::new().await;
    let bytes = b"same-bytes";

    let first = f.store.store(f.key(), bytes).await.unwrap();
    let second = f.store.store(f.key(), bytes).await.unwrap();

    assert_eq!(first.outcome, StoreOutcome::Created);
    assert_eq!(second.outcome, StoreOutcome::AlreadyIdentical);
    assert_eq!(first.relative_path, second.relative_path);
    assert_eq!(first.sha256, second.sha256);
}

/// 同一个 UID 送来不同内容是发送方的 bug，不能覆盖不可变原始文件。
#[tokio::test]
async fn conflicting_content_is_rejected_and_original_is_preserved() {
    let f = Fixture::new().await;

    let first = f.store.store(f.key(), b"first-version").await.unwrap();
    let error = f
        .store
        .store(f.key(), b"second-version-longer")
        .await
        .unwrap_err();

    assert!(matches!(error, StoreError::ContentConflict { .. }));
    let absolute = f.store.resolve(&first.relative_path).unwrap();
    assert_eq!(tokio::fs::read(&absolute).await.unwrap(), b"first-version");
}

/// 长度相同但内容不同也必须拒绝，只比文件大小会漏掉冲突。
#[tokio::test]
async fn same_length_different_content_is_rejected() {
    let f = Fixture::new().await;

    f.store.store(f.key(), b"AAAA").await.unwrap();
    let error = f.store.store(f.key(), b"BBBB").await.unwrap_err();

    assert!(matches!(error, StoreError::ContentConflict { .. }));
}

#[tokio::test]
async fn derived_file_is_invisible_until_activated() {
    let f = Fixture::new().await;
    let job = uuid::Uuid::new_v4();
    let staged = f
        .store
        .stage_derived(job, f.key(), b"derived")
        .await
        .unwrap();
    assert!(matches!(
        f.store.resolve_for_read(&staged.relative_path).await,
        Err(StoreError::NotFound { .. })
    ));
    let stored = f.store.activate_staged(staged).await.unwrap();
    assert!(stored.relative_path.starts_with(&format!("derived/{job}/")));
    assert_eq!(
        f.store.read(&stored.relative_path).await.unwrap(),
        b"derived"
    );
}

#[tokio::test]
async fn failed_activation_cleanup_only_removes_derived_files() {
    let f = Fixture::new().await;
    let original = f.store.store(f.key(), b"original").await.unwrap();
    let staged = f
        .store
        .stage_derived(uuid::Uuid::new_v4(), f.key(), b"derived")
        .await
        .unwrap();
    let derived = f.store.activate_staged(staged).await.unwrap();

    f.store
        .remove_derived(&derived.relative_path)
        .await
        .unwrap();
    assert!(matches!(
        f.store.read(&derived.relative_path).await,
        Err(StoreError::NotFound { .. })
    ));
    assert!(matches!(
        f.store.remove_derived(&original.relative_path).await,
        Err(StoreError::PathEscape { .. })
    ));
    assert_eq!(
        f.store.read(&original.relative_path).await.unwrap(),
        b"original"
    );
}

/// 成功返回后不该有临时文件残留 —— 有的话说明 rename 那步没走完。
#[tokio::test]
async fn leaves_no_temporary_files_behind() {
    let f = Fixture::new().await;
    f.store.store(f.key(), b"payload").await.unwrap();

    let mut entries = tokio::fs::read_dir(f.store.root().join(TEMP_DIR))
        .await
        .unwrap();
    assert!(
        entries.next_entry().await.unwrap().is_none(),
        ".tmp/ 应该是空的"
    );
}

/// 崩溃恢复:临时文件的存在意味着那次落盘没走完,对应事务也没提交,清掉是安全的。
#[tokio::test]
async fn cleanup_removes_orphaned_temp_files() {
    let f = Fixture::new().await;
    let temp_dir = f.store.root().join(TEMP_DIR);
    for name in ["a.part", "b.part"] {
        tokio::fs::write(temp_dir.join(name), b"half-written")
            .await
            .unwrap();
    }
    // 已经落好的文件不能被误删
    let stored = f.store.store(f.key(), b"good").await.unwrap();

    assert_eq!(f.store.cleanup_temp().await.unwrap(), 2);
    assert_eq!(f.store.cleanup_temp().await.unwrap(), 0, "清理应可重复执行");
    assert!(
        f.store
            .resolve(&stored.relative_path)
            .map(|p| Path::new(&p).exists())
            .unwrap(),
        "已落盘的影像不该被清理动到"
    );
}

/// 同一序列的多个实例落进同一个目录,WADO 拉整个 series 才是顺序读。
#[tokio::test]
async fn instances_of_a_series_share_a_directory() {
    let f = Fixture::new().await;
    let other_sop = uid("1.2.826.0.1.3680043.8.498.4");

    let a = f.store.store(f.key(), b"one").await.unwrap();
    let b = f
        .store
        .store(
            InstanceKey {
                study: &f.study,
                series: &f.series,
                sop: &other_sop,
            },
            b"two",
        )
        .await
        .unwrap();

    let dir = |p: &str| p.rsplit_once('/').unwrap().0.to_owned();
    assert_eq!(dir(&a.relative_path), dir(&b.relative_path));
}

/// 数据库被写坏或迁移出错时,一个含 `..` 的路径不能让 WADO 读到存储根外面去。
#[tokio::test]
async fn resolve_rejects_paths_escaping_the_root() {
    let f = Fixture::new().await;

    for hostile in [
        "../etc/passwd",
        "/etc/passwd",
        "aa/../../../etc/passwd",
        "./x",
    ] {
        assert!(
            matches!(f.store.resolve(hostile), Err(StoreError::PathEscape { .. })),
            "应拒绝 {hostile:?}"
        );
    }
    // 正常的相对路径仍然放行
    assert!(f.store.resolve("ab/cd/1.2/3.4/5.6.dcm").is_ok());
}

/// 符号链接逃逸:组件检查放行,但 canonicalize 之后必须被挡住。
///
/// 这是 `resolve` 单靠分量检查挡不住的一类:`evil.dcm` 的每个分量都是
/// `Normal`,可它指向存储根之外。
#[tokio::test]
async fn resolve_for_read_blocks_symlink_escape() {
    let f = Fixture::new().await;

    // 在存储根外面放一个文件,再从根内建符号链接指向它
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("secret.txt");
    tokio::fs::write(&secret, b"root only").await.unwrap();

    let link = f.store.root().join("evil.dcm");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&secret, &link).unwrap();
    #[cfg(not(unix))]
    {
        eprintln!(">>> 非 Unix 平台跳过符号链接测试");
        return;
    }

    // 分量检查会放行 —— 这正是需要第二道的原因
    assert!(
        f.store.resolve("evil.dcm").is_ok(),
        "分量检查本来就挡不住符号链接,这里确认这一前提"
    );

    // 读路径必须拒绝
    let result = f.store.resolve_for_read("evil.dcm").await;
    assert!(
        matches!(result, Err(StoreError::PathEscape { .. })),
        "符号链接指向根外时必须拒绝,实际:{result:?}"
    );
    assert!(
        matches!(
            f.store.read("evil.dcm").await,
            Err(StoreError::PathEscape { .. })
        ),
        "read 同样要挡住"
    );
}

/// 数据库有记录、盘上没文件时要能区分出来 —— 那是存储与库不一致的信号。
#[tokio::test]
async fn resolve_for_read_reports_a_missing_file_distinctly() {
    let f = Fixture::new().await;

    let result = f.store.resolve_for_read("ab/cd/1.2/3.4/nope.dcm").await;
    assert!(
        matches!(result, Err(StoreError::NotFound { .. })),
        "文件不存在应回 NotFound 而不是 PathEscape 或一般 IO 错误,实际:{result:?}"
    );
}

/// 正常落盘的文件要能原样读回来。
#[tokio::test]
async fn read_returns_the_stored_bytes() {
    let f = Fixture::new().await;
    let bytes = b"DICM-and-then-some-payload".repeat(40);
    let stored = f
        .store
        .store(
            InstanceKey {
                study: &uid("1.2.826.0.1.3680043.8.498.700"),
                series: &uid("1.2.826.0.1.3680043.8.498.701"),
                sop: &uid("1.2.826.0.1.3680043.8.498.702"),
            },
            &bytes,
        )
        .await
        .expect("应能落盘");

    let read_back = f.store.read(&stored.relative_path).await.expect("应能读回");
    assert_eq!(read_back, bytes, "读回的字节必须与写入完全一致");
}

#[tokio::test]
async fn concurrent_stores_of_a_series_all_succeed() {
    let f = Fixture::new().await;
    let sops: Vec<Uid> = (0..32)
        .map(|i| uid(&format!("1.2.826.0.1.3680043.8.498.100.{i}")))
        .collect();

    let results = futures::future::join_all(sops.iter().map(|sop| {
        let store = f.store.clone();
        let (study, series) = (f.study.clone(), f.series.clone());
        async move {
            store
                .store(
                    InstanceKey {
                        study: &study,
                        series: &series,
                        sop,
                    },
                    sop.as_str().as_bytes(),
                )
                .await
        }
    }))
    .await;

    // 并发建同一棵目录树时,"目录已存在"不能被当成错误
    let paths: std::collections::HashSet<String> = results
        .into_iter()
        .map(|r| r.expect("并发落盘不该失败").relative_path)
        .collect();
    assert_eq!(paths.len(), 32, "32 个实例应落到 32 个不同路径");
}
