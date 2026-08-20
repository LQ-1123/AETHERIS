//! C-MOVE/C-GET interoperability against DCMTK.
//!
//! The test deliberately uses a third-party SCU/SCP pair. A retrieve stack
//! tested only against itself can make matching mistakes on both sides and
//! still appear correct.

use std::net::{SocketAddr, TcpListener};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use pacs_core::fixture::{ct_instance, unique_uid};
use uuid::Uuid;

const CALLED_AE: &str = "REMOTE_PACS";

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn prerequisites() -> Option<String> {
    dotenvy::dotenv().ok();
    let in_ci = std::env::var_os("CI").is_some();
    let Ok(database_url) = std::env::var("PACS_TEST_DATABASE_URL") else {
        assert!(!in_ci, "CI 必须设置 PACS_TEST_DATABASE_URL");
        eprintln!("\n>>> 跳过 C-MOVE/C-GET 互操作测试:未设置 PACS_TEST_DATABASE_URL。\n");
        return None;
    };
    let missing: Vec<_> = ["storescu", "storescp", "movescu", "getscu"]
        .into_iter()
        .filter(|program| {
            !Command::new(program)
                .arg("--version")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        })
        .collect();
    if !missing.is_empty() {
        assert!(!in_ci, "CI 必须安装完整 DCMTK:缺少 {missing:?}");
        eprintln!("\n>>> 跳过 C-MOVE/C-GET 互操作测试:缺少 DCMTK {missing:?}。\n");
        return None;
    }
    Some(database_url)
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("应能取得空闲端口")
        .local_addr()
        .expect("应能读取空闲端口")
        .port()
}

fn wait_for_port(port: u16, process: &str) {
    let address: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if std::net::TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("{process} 在 20 秒内没有监听 {address}");
}

fn start_pacsd(database_url: &str, storage_root: &Path, port: u16) -> ChildGuard {
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
    let guard = ChildGuard(child);
    wait_for_port(port, "pacsd");
    guard
}

fn start_storescp(ae_title: &str, output: &Path, port: u16) -> ChildGuard {
    let child = Command::new("storescp")
        .args([
            "-aet",
            ae_title,
            "-od",
            output.to_str().unwrap(),
            "-ta",
            "10",
            "-td",
            "10",
            &port.to_string(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("应能启动 storescp");
    let guard = ChildGuard(child);
    wait_for_port(port, "storescp");
    guard
}

fn run(program: &str, args: &[String]) -> (bool, String) {
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

fn push_instance(port: u16, calling_ae: &str, file: &Path) {
    let (ok, output) = run(
        "storescu",
        &[
            "-aec".into(),
            CALLED_AE.into(),
            "-aet".into(),
            calling_ae.into(),
            "-ta".into(),
            "10".into(),
            "-td".into(),
            "10".into(),
            "127.0.0.1".into(),
            port.to_string(),
            file.to_str().unwrap().into(),
        ],
    );
    assert!(ok, "storescu 应成功:\n{output}");
}

async fn approve_peer(pool: &sqlx::PgPool, ae_title: &str, retrieval_port: Option<u16>) {
    let device = pacs_db::observe_device(pool, 1, ae_title, "127.0.0.1")
        .await
        .unwrap();
    pacs_db::set_device_status(pool, 1, device.id, "active")
        .await
        .unwrap();
    if let Some(port) = retrieval_port {
        pacs_db::configure_retrieval_source(
            pool,
            1,
            device.id,
            true,
            Some(i32::from(port)),
            false,
            None,
        )
        .await
        .unwrap();
    }
}

fn retrieve_args(
    calling_ae: &str,
    port: u16,
    study_uid: &str,
    move_destination: Option<&str>,
) -> Vec<String> {
    let mut args = vec![
        "-v".into(),
        "-S".into(),
        "-aec".into(),
        CALLED_AE.into(),
        "-aet".into(),
        calling_ae.into(),
    ];
    if let Some(destination) = move_destination {
        args.extend(["-aem".into(), destination.into()]);
    }
    args.extend([
        "-ta".into(),
        "10".into(),
        "-td".into(),
        "10".into(),
        "-k".into(),
        "QueryRetrieveLevel=STUDY".into(),
        "-k".into(),
        format!("StudyInstanceUID={study_uid}"),
        "127.0.0.1".into(),
        port.to_string(),
    ]);
    args
}

fn assert_received_sop(directory: &Path, expected_sop_uid: &str) {
    let received = std::fs::read_dir(directory)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .find_map(|entry| {
            let object = dicom::object::open_file(entry.path()).ok()?;
            let uid = object
                .element(dicom::dictionary_std::tags::SOP_INSTANCE_UID)
                .ok()?
                .to_str()
                .ok()?
                .trim()
                .to_owned();
            (uid == expected_sop_uid).then_some(uid)
        });
    assert_eq!(received.as_deref(), Some(expected_sop_uid));
}

#[tokio::test]
async fn dcmtk_move_get_and_destination_whitelist_interoperate() {
    let Some(database_url) = prerequisites() else {
        return;
    };
    let suffix = Uuid::new_v4().simple().to_string();
    let calling_ae = format!("RSCU{}", &suffix[..8]);
    let move_destination = format!("RDEST{}", &suffix[..8]);
    let unknown_destination = format!("NOPE{}", &suffix[..8]);
    let storage = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let move_output = tempfile::tempdir().unwrap();
    let get_output = tempfile::tempdir().unwrap();
    let pacs_port = free_port();
    let move_port = free_port();
    let _server = start_pacsd(&database_url, storage.path(), pacs_port);

    let study_uid = unique_uid();
    let series_uid = unique_uid();
    let sop_uid = unique_uid();
    let second_series_uid = unique_uid();
    let second_sop_uid = unique_uid();
    let source_file = source.path().join("retrieve-source.dcm");
    let second_source_file = source.path().join("retrieve-source-2.dcm");
    ct_instance(&study_uid, &series_uid, &sop_uid)
        .write_to_file(&source_file)
        .unwrap();
    ct_instance(&study_uid, &second_series_uid, &second_sop_uid)
        .write_to_file(&second_source_file)
        .unwrap();
    push_instance(pacs_port, &calling_ae, &source_file);
    push_instance(pacs_port, &calling_ae, &second_source_file);

    let pool = pacs_db::connect(&database_url).await.unwrap();
    pacs_db::migrate(&pool).await.unwrap();
    approve_peer(&pool, &calling_ae, None).await;
    approve_peer(&pool, &move_destination, Some(move_port)).await;

    let _storescp = start_storescp(&move_destination, move_output.path(), move_port);
    let move_args = retrieve_args(&calling_ae, pacs_port, &study_uid, Some(&move_destination));
    let (ok, output) = run("movescu", &move_args);
    assert!(ok, "movescu 应成功:\n{output}");
    assert!(
        output.to_ascii_lowercase().contains("success"),
        "movescu 应收到最终成功状态:\n{output}"
    );
    assert!(
        output
            .to_ascii_lowercase()
            .contains("received move response 1 (pending)"),
        "两个实例应产生带子操作计数的 Pending 响应:\n{output}"
    );
    assert_received_sop(move_output.path(), &sop_uid);
    assert_received_sop(move_output.path(), &second_sop_uid);

    let unknown_args = retrieve_args(
        &calling_ae,
        pacs_port,
        &study_uid,
        Some(&unknown_destination),
    );
    let (_, output) = run("movescu", &unknown_args);
    assert!(
        output
            .to_ascii_lowercase()
            .contains("movedestinationunknown"),
        "未知 Move Destination 应返回标准 0xA801:\n{output}"
    );

    let mut get_args = retrieve_args(&calling_ae, pacs_port, &study_uid, None);
    get_args.splice(
        get_args.len() - 2..get_args.len() - 2,
        ["-od".into(), get_output.path().to_str().unwrap().into()],
    );
    let (ok, output) = run("getscu", &get_args);
    assert!(ok, "getscu 应成功:\n{output}");
    assert!(
        output.to_ascii_lowercase().contains("success"),
        "getscu 应收到最终成功状态:\n{output}"
    );
    assert_received_sop(get_output.path(), &sop_uid);
    assert_received_sop(get_output.path(), &second_sop_uid);
}
