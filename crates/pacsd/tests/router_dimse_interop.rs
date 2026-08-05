//! Outbound Router interoperability against DCMTK storescp.

use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use pacs_core::fixture::{ct_instance, unique_uid};

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn dcmtk_available() -> bool {
    Command::new("storescp")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[tokio::test]
async fn router_echoes_and_stores_to_dcmtk() {
    if !dcmtk_available() {
        assert!(std::env::var_os("CI").is_none(), "CI 必须安装 DCMTK");
        eprintln!("跳过 Router DIMSE 互操作测试: 未安装 DCMTK storescp");
        return;
    }
    let output = tempfile::tempdir().unwrap();
    let port = free_port();
    let child = Command::new("storescp")
        .args([
            "-q",
            "-aet",
            "ROUTE_SCP",
            "-od",
            output.path().to_str().unwrap(),
            &port.to_string(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("应能启动 DCMTK storescp");
    let _guard = ChildGuard(child);

    let config = pacs_dimse::DimseClientConfig::new("127.0.0.1", port, "ROUTE_SCP", "REMOTE_PACS");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match pacs_dimse::c_echo(&config).await {
            Ok(()) => break,
            Err(error) if Instant::now() < deadline => {
                eprintln!("等待 storescp: {error}");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(error) => panic!("Router C-ECHO 未能连接 DCMTK: {error}"),
        }
    }

    let sop_uid = unique_uid();
    let object = ct_instance(&unique_uid(), &unique_uid(), &sop_uid);
    let mut bytes = Vec::new();
    object.write_all(&mut bytes).unwrap();
    pacs_dimse::c_store(&config, &bytes)
        .await
        .expect("Router C-STORE 应被 DCMTK 接受");

    let received = std::fs::read_dir(output.path())
        .unwrap()
        .filter_map(Result::ok)
        .find(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .expect("storescp 应写出收到的 DICOM")
        .path();
    let parsed = dicom::object::open_file(received).expect("DCMTK 写出的文件应可解析");
    let uid = parsed
        .element(dicom::dictionary_std::tags::SOP_INSTANCE_UID)
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(uid.trim(), sop_uid);
}
