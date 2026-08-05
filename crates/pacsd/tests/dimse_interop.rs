//! DIMSE 互操作测试:用 DCMTK 的 `echoscu`/`storescu` 打真实流量。
//!
//! 这是阶段 2 的验收标准。自己写的客户端测自己写的服务端,只能证明两边对协议的
//! **误解是一致的**;DCMTK 是业界事实上的互操作基准,它认可才算真的通了。

use std::net::{SocketAddr, TcpListener};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use pacs_core::fixture::{ct_instance, unique_uid};

const CALLED_AE: &str = "REMOTE_PACS";
const CALLING_AE: &str = "TEST_SCU";

/// 保证测试结束(哪怕 panic)也把服务端进程收掉,不留孤儿。
struct ServerGuard(Child);

impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// 前置条件不满足时跳过;CI 里跳过等于没测,直接失败。
fn prerequisites() -> Option<String> {
    dotenvy::dotenv().ok();

    let in_ci = std::env::var_os("CI").is_some();

    let Ok(database_url) = std::env::var("PACS_TEST_DATABASE_URL") else {
        assert!(!in_ci, "CI 必须设置 PACS_TEST_DATABASE_URL");
        eprintln!("\n>>> 跳过 DIMSE 互操作测试:未设置 PACS_TEST_DATABASE_URL。\n");
        return None;
    };

    if !dcmtk_available() {
        assert!(!in_ci, "CI 必须安装 DCMTK,否则互操作无从验证");
        eprintln!("\n>>> 跳过 DIMSE 互操作测试:未找到 DCMTK(brew install dcmtk)。\n");
        return None;
    }
    Some(database_url)
}

fn dcmtk_available() -> bool {
    Command::new("echoscu")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// 借系统分配一个空闲端口,随即释放让服务端去绑。
///
/// 释放到重新绑定之间有窗口期,但测试串行跑时不会撞上;
/// 这比写死端口可靠 —— 写死的端口会被开发机上别的服务占掉。
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("应能取到空闲端口")
        .local_addr()
        .expect("应能读出端口")
        .port()
}

fn start_pacsd(database_url: &str, storage_root: &Path, port: u16) -> ServerGuard {
    let http_port = free_port();
    let child = Command::new(env!("CARGO_BIN_EXE_pacsd"))
        .env("DATABASE_URL", database_url)
        .env("PACS_STORAGE_ROOT", storage_root)
        .env("PACS_DIMSE_BIND", format!("127.0.0.1:{port}"))
        .env("PACS_HTTP_BIND", format!("127.0.0.1:{http_port}"))
        .env("PACS_AE_TITLE", CALLED_AE)
        .env("RUST_LOG", "info,pacsd=debug,pacs_dimse=debug")
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("应能启动 pacsd");
    let guard = ServerGuard(child);

    // 等端口真的能连上再往下走,否则先跑的 echoscu 会连到一个还没起来的服务
    let address: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if std::net::TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_ok() {
            return guard;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("pacsd 在 20 秒内没有监听 {address}");
}

fn run(program: &str, args: &[&str]) -> (bool, String) {
    let output = Command::new(program)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("无法执行 {program}:{error}"));
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (output.status.success(), combined)
}

/// C-ECHO:最小的「我在」握手,协议栈通不通全看它。
#[test]
fn echoscu_verifies_the_server() {
    let Some(database_url) = prerequisites() else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let port = free_port();
    let _server = start_pacsd(&database_url, storage.path(), port);

    let (ok, output) = run(
        "echoscu",
        &[
            "-aec",
            CALLED_AE,
            "-aet",
            CALLING_AE,
            "127.0.0.1",
            &port.to_string(),
        ],
    );
    assert!(ok, "echoscu 应成功,实际输出:\n{output}");
}

/// Called AE Title 不匹配时必须拒绝关联。
///
/// 这不是认证(AE Title 可以随便填),但能挡住配错目标的设备 ——
/// 把别人家的影像收进来是很实际的事故。
#[test]
fn association_is_rejected_for_wrong_called_ae_title() {
    let Some(database_url) = prerequisites() else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let port = free_port();
    let _server = start_pacsd(&database_url, storage.path(), port);

    let (ok, output) = run(
        "echoscu",
        &[
            "-aec",
            "WRONG_AE",
            "-aet",
            CALLING_AE,
            "127.0.0.1",
            &port.to_string(),
        ],
    );
    assert!(
        !ok,
        "Called AE Title 不匹配时不该建立关联,实际输出:\n{output}"
    );
}

/// C-STORE 全链路:storescu 发一份 CT → 落盘 + 入库,且能原样取回。
#[tokio::test]
async fn storescu_stores_an_instance_end_to_end() {
    let Some(database_url) = prerequisites() else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let port = free_port();
    let _server = start_pacsd(&database_url, storage.path(), port);

    // 造一份 CT 实例写到磁盘,交给 storescu 发送
    let (study_uid, series_uid, sop_uid) = (unique_uid(), unique_uid(), unique_uid());
    let object = ct_instance(&study_uid, &series_uid, &sop_uid);
    let source_dir = tempfile::tempdir().unwrap();
    let source_file = source_dir.path().join("instance.dcm");
    object
        .write_to_file(&source_file)
        .expect("应能写出 DICOM 文件");

    let (ok, output) = run(
        "storescu",
        &[
            "-aec",
            CALLED_AE,
            "-aet",
            CALLING_AE,
            "127.0.0.1",
            &port.to_string(),
            source_file.to_str().unwrap(),
        ],
    );
    assert!(ok, "storescu 应成功,实际输出:\n{output}");

    // —— 数据库侧 ——
    let pool = pacs_db::connect(&database_url).await.unwrap();
    let (storage_path, file_size, series_uid_db): (String, i64, String) = sqlx::query_as(
        "SELECT i.storage_path, i.file_size, s.series_instance_uid
         FROM instances i JOIN series s ON i.series_fk = s.id
         WHERE i.sop_instance_uid = $1",
    )
    .bind(&sop_uid)
    .fetch_one(&pool)
    .await
    .expect("实例应已入库");
    assert_eq!(series_uid_db, series_uid, "实例应挂在正确的序列下");

    // —— 存储侧 ——
    let absolute = storage.path().join(&storage_path);
    let on_disk = std::fs::read(&absolute)
        .unwrap_or_else(|error| panic!("盘上应有 {}:{error}", absolute.display()));
    assert_eq!(
        on_disk.len() as i64,
        file_size,
        "库里记的大小应与实际文件一致"
    );

    // 存下来的必须是能重新解析的合法 DICOM,且 UID 对得上 ——
    // 只检查文件存在是不够的,写坏的文件也「存在」。
    let reparsed = dicom::object::from_reader(std::io::Cursor::new(&on_disk))
        .expect("落盘的文件应当能重新解析");
    let metadata = pacs_core::extract_metadata(&reparsed).expect("应能提取元数据");
    assert_eq!(metadata.instance.uid.as_str(), sop_uid);
    assert_eq!(metadata.study.uid.as_str(), study_uid);
    assert_eq!(metadata.series.modality.as_deref(), Some("CT"));
    // 像素几何要完整保留,否则查看器拿不到图
    assert_eq!(metadata.instance.rows, Some(4));
    assert_eq!(metadata.instance.columns, Some(4));
}

/// 重传同一份影像:必须幂等,不能报错也不能产生第二条记录。
#[tokio::test]
async fn retransmission_over_dimse_is_idempotent() {
    let Some(database_url) = prerequisites() else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let port = free_port();
    let _server = start_pacsd(&database_url, storage.path(), port);

    let (study_uid, series_uid, sop_uid) = (unique_uid(), unique_uid(), unique_uid());
    let object = ct_instance(&study_uid, &series_uid, &sop_uid);
    let source_dir = tempfile::tempdir().unwrap();
    let source_file = source_dir.path().join("instance.dcm");
    object.write_to_file(&source_file).unwrap();

    let args = [
        "-aec",
        CALLED_AE,
        "-aet",
        CALLING_AE,
        "127.0.0.1",
        &port.to_string(),
        source_file.to_str().unwrap(),
    ];
    for attempt in 1..=2 {
        let (ok, output) = run("storescu", &args);
        assert!(ok, "第 {attempt} 次发送应成功,实际输出:\n{output}");
    }

    let pool = pacs_db::connect(&database_url).await.unwrap();
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM instances WHERE sop_instance_uid = $1")
            .bind(&sop_uid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 1, "重传不该产生第二条记录");
}

/// 一次关联里连发多份:这是设备推送整个序列的常态。
#[tokio::test]
async fn multiple_instances_in_one_association() {
    let Some(database_url) = prerequisites() else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let port = free_port();
    let _server = start_pacsd(&database_url, storage.path(), port);

    let (study_uid, series_uid) = (unique_uid(), unique_uid());
    let source_dir = tempfile::tempdir().unwrap();
    let mut sop_uids = Vec::new();
    let mut files = Vec::new();
    for index in 0..5 {
        let sop_uid = unique_uid();
        let path = source_dir.path().join(format!("{index}.dcm"));
        ct_instance(&study_uid, &series_uid, &sop_uid)
            .write_to_file(&path)
            .unwrap();
        sop_uids.push(sop_uid);
        files.push(path.to_str().unwrap().to_owned());
    }

    let mut args = vec![
        "-aec".to_owned(),
        CALLED_AE.to_owned(),
        "-aet".to_owned(),
        CALLING_AE.to_owned(),
        "127.0.0.1".to_owned(),
        port.to_string(),
    ];
    args.extend(files);
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    let (ok, output) = run("storescu", &borrowed);
    assert!(ok, "连发 5 份应成功,实际输出:\n{output}");

    let pool = pacs_db::connect(&database_url).await.unwrap();
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM instances WHERE sop_instance_uid = ANY($1)")
            .bind(&sop_uids)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 5, "5 份都应入库");

    // 聚合计数由入库事务重算,应当准确反映这个序列
    let instances: i32 =
        sqlx::query_scalar("SELECT number_of_instances FROM series WHERE series_instance_uid = $1")
            .bind(&series_uid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(instances, 5);
}
