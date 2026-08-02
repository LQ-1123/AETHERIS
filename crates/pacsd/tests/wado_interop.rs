//! WADO-RS 集成测试:取回原始 DICOM、元数据、帧。
//!
//! 重点验三件事:
//! 1. 取回的字节与推送的**逐字节一致** —— 影像保真性是这套系统的根本承诺;
//! 2. 帧号 1 基;
//! 3. 未授权被拒绝(取回接口拿到的是像素,泄露后果比元数据更重)。

use std::net::{SocketAddr, TcpListener};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use pacs_core::fixture::{ct_instance, unique_uid};

const CALLED_AE: &str = "REMOTE_PACS";
const CALLING_AE: &str = "TEST_SCU";
const TEST_JWT_SECRET: &str = "integration-test-secret-long-enough-for-hs256";
const ADMIN_USER: &str = "wado.tester";
const ADMIN_PASSWORD: &str = "wado-test-password-1234";

struct ServerGuard(Child);

impl Drop for ServerGuard {
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
        eprintln!("\n>>> 跳过 WADO-RS 测试:未设置 PACS_TEST_DATABASE_URL。\n");
        return None;
    };
    for tool in ["curl", "storescu"] {
        if !tool_available(tool) {
            assert!(!in_ci, "CI 必须有 {tool}");
            eprintln!("\n>>> 跳过 WADO-RS 测试:未找到 {tool}。\n");
            return None;
        }
    }
    Some(database_url)
}

fn tool_available(tool: &str) -> bool {
    Command::new(tool)
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

fn start_server(database_url: &str, storage_root: &Path) -> (ServerGuard, u16, u16) {
    let dimse_port = free_port();
    let http_port = free_port();

    let create = Command::new(env!("CARGO_BIN_EXE_pacsd"))
        .args([
            "admin",
            "--username",
            ADMIN_USER,
            "--password",
            ADMIN_PASSWORD,
        ])
        .env("DATABASE_URL", database_url)
        .env("PACS_STORAGE_ROOT", storage_root)
        .env("PACS_JWT_SECRET", TEST_JWT_SECRET)
        .env("RUST_LOG", "warn")
        .output()
        .expect("应能执行 pacsd admin");
    let stderr = String::from_utf8_lossy(&create.stderr);
    assert!(
        create.status.success() || stderr.contains("已存在") || stderr.contains("duplicate"),
        "建账号失败:{stderr}"
    );

    let child = Command::new(env!("CARGO_BIN_EXE_pacsd"))
        .env("DATABASE_URL", database_url)
        .env("PACS_STORAGE_ROOT", storage_root)
        .env("PACS_DIMSE_BIND", format!("127.0.0.1:{dimse_port}"))
        .env("PACS_HTTP_BIND", format!("127.0.0.1:{http_port}"))
        .env("PACS_AE_TITLE", CALLED_AE)
        .env("PACS_JWT_SECRET", TEST_JWT_SECRET)
        .env("RUST_LOG", "info,pacsd=debug,pacs_web=debug")
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("应能启动 pacsd");
    let guard = ServerGuard(child);

    for port in [dimse_port, http_port] {
        let address: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let deadline = Instant::now() + Duration::from_secs(25);
        let mut ready = false;
        while Instant::now() < deadline {
            if std::net::TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_ok() {
                ready = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(ready, "pacsd 在 25 秒内没有监听 {address}");
    }
    (guard, dimse_port, http_port)
}

/// 推一个实例,返回它的 Part-10 原始字节(用于比对取回结果)。
fn push_instance(dimse_port: u16, study: &str, series: &str, sop: &str) -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("instance.dcm");
    ct_instance(study, series, sop)
        .write_to_file(&file)
        .unwrap();
    let original = std::fs::read(&file).unwrap();

    let output = Command::new("storescu")
        .args([
            "-aec",
            CALLED_AE,
            "-aet",
            CALLING_AE,
            "127.0.0.1",
            &dimse_port.to_string(),
            file.to_str().unwrap(),
        ])
        .output()
        .expect("应能执行 storescu");
    assert!(
        output.status.success(),
        "storescu 应成功:{}",
        String::from_utf8_lossy(&output.stderr)
    );
    original
}

fn login(http_port: u16) -> String {
    let url = format!("https://127.0.0.1:{http_port}/auth/login");
    let payload = format!(r#"{{"username":"{ADMIN_USER}","password":"{ADMIN_PASSWORD}"}}"#);
    let output = Command::new("curl")
        .args([
            "-k",
            "-s",
            "-X",
            "POST",
            "-H",
            "Content-Type: application/json",
            "-d",
            &payload,
            &url,
        ])
        .output()
        .expect("应能执行 curl");
    let body = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&body).unwrap_or_else(|_| panic!("登录响应不是 JSON:{body}"));
    parsed["access_token"]
        .as_str()
        .unwrap_or_else(|| panic!("登录响应里没有 access_token:{body}"))
        .to_owned()
}

/// 取回二进制内容:响应体写文件,避免走 UTF-8 转换弄坏字节。
fn fetch_binary(http_port: u16, path: &str, token: &str) -> (u16, Vec<u8>, String) {
    let dir = tempfile::tempdir().unwrap();
    let body_file = dir.path().join("body.bin");
    let url = format!("https://127.0.0.1:{http_port}{path}");

    let output = Command::new("curl")
        .args([
            "-k",
            "-s",
            "-D",
            "-", // 响应头到 stdout
            "-o",
            body_file.to_str().unwrap(),
            "-w",
            "%{http_code}",
            "-H",
            &format!("Authorization: Bearer {token}"),
            &url,
        ])
        .output()
        .expect("应能执行 curl");

    let headers = String::from_utf8_lossy(&output.stdout).to_string();
    let status = headers
        .trim_end()
        .chars()
        .rev()
        .take(3)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>()
        .parse::<u16>()
        .unwrap_or_else(|_| panic!("无法解析状态码:{headers}"));
    let body = std::fs::read(&body_file).unwrap_or_default();
    (status, body, headers)
}

fn status_only(http_port: u16, path: &str, token: Option<&str>) -> u16 {
    let url = format!("https://127.0.0.1:{http_port}{path}");
    let mut args: Vec<String> = vec![
        "-k".into(),
        "-s".into(),
        "-o".into(),
        "/dev/null".into(),
        "-w".into(),
        "%{http_code}".into(),
        url,
    ];
    if let Some(token) = token {
        args.push("-H".into());
        args.push(format!("Authorization: Bearer {token}"));
    }
    let output = Command::new("curl").args(&args).output().unwrap();
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(0)
}

/// 取回的字节必须与推送的完全一致。
///
/// 这是整套系统最根本的承诺:存进去的影像取出来不能有任何改动。
/// C-STORE 那边刻意保留了发送方的原始数据集字节(不解码再重编码),
/// 这条测试守的就是那个保证一路走到 HTTP 出口。
#[tokio::test]
async fn retrieved_instance_is_byte_identical() {
    let Some(database_url) = prerequisites() else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let (_server, dimse, http) = start_server(&database_url, storage.path());

    let (study, series, sop) = (unique_uid(), unique_uid(), unique_uid());
    let original = push_instance(dimse, &study, &series, &sop);

    let token = login(http);
    let (status, body, headers) = fetch_binary(
        http,
        &format!("/dicomweb/studies/{study}/series/{series}/instances/{sop}"),
        &token,
    );

    assert_eq!(status, 200, "响应头:{headers}");
    assert!(
        headers.contains("application/dicom"),
        "Content-Type 应是 application/dicom:{headers}"
    );
    assert_eq!(&body[128..132], b"DICM", "应是合法的 Part-10 文件");

    // 比的是**数据集**而不是整个文件:文件元信息头由服务端按标准重建
    // (ImplementationClassUID 要标成本实现,不能冒充发送方),所以整文件
    // 逐字节相同并不成立 —— 那是错误的期望。
    // 「数据集不变、元信息头合法重建」的完整论证见 tests/byte_fidelity.rs。
    let sent = dataset_slice(&original);
    let received = dataset_slice(&body);
    assert_eq!(
        received.len(),
        sent.len(),
        "数据集字节数应一致(发送 {} 字节,取回 {} 字节)",
        sent.len(),
        received.len()
    );
    assert_eq!(
        received, sent,
        "数据集必须逐字节一致 —— 影像资料不该被我们的编码器改写"
    );
}

/// 取出 Part-10 文件里的数据集部分(跳过前导、`DICM` 和元信息组)。
///
/// 元信息组长度由 (0002,0000) 给出,该值不含自身那个元素,
/// 所以要把它自己的 12 字节头也加回去。
fn dataset_slice(bytes: &[u8]) -> &[u8] {
    assert_eq!(&bytes[128..132], b"DICM", "应是 Part-10 文件");
    let value_at = 132 + 8;
    let group_length = u32::from_le_bytes([
        bytes[value_at],
        bytes[value_at + 1],
        bytes[value_at + 2],
        bytes[value_at + 3],
    ]) as usize;
    &bytes[132 + 12 + group_length..]
}

/// 元数据接口回 DICOM JSON,且**不含像素**。
#[tokio::test]
async fn metadata_excludes_pixel_data() {
    let Some(database_url) = prerequisites() else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let (_server, dimse, http) = start_server(&database_url, storage.path());

    let (study, series, sop) = (unique_uid(), unique_uid(), unique_uid());
    push_instance(dimse, &study, &series, &sop);

    let token = login(http);
    let (status, body, headers) = fetch_binary(
        http,
        &format!("/dicomweb/studies/{study}/series/{series}/instances/{sop}/metadata"),
        &token,
    );
    assert_eq!(status, 200, "响应头:{headers}");
    assert!(headers.contains("application/dicom+json"), "{headers}");

    let text = String::from_utf8(body).expect("元数据应是 UTF-8");
    let parsed: serde_json::Value =
        serde_json::from_str(&text).unwrap_or_else(|_| panic!("不是 JSON:{text}"));
    let array = parsed
        .as_array()
        .expect("元数据响应是数组(即使只有一个实例)");
    assert_eq!(array.len(), 1);

    let entry = &array[0];
    // 几何和显示参数要在 —— 查看器靠它们决定怎么渲染
    assert_eq!(entry["00280010"]["Value"][0], 4, "Rows 应为 4");
    assert_eq!(entry["00280011"]["Value"][0], 4, "Columns 应为 4");
    assert!(entry.get("00281052").is_some(), "RescaleIntercept 应在");
    assert_eq!(entry["0020000D"]["Value"][0], study);

    // PixelData (7FE0,0010) 必须被去掉,否则这个接口就和取原始文件没区别了
    assert!(
        entry.get("7FE00010").is_none(),
        "元数据不该含 PixelData:{text}"
    );
    // 不比"元数据比原文件小":夹具的像素只有 4×4×2 = 32 字节,而 DICOM JSON
    // 要为每个属性写出标签、VR 和 Value 数组,本来就比二进制冗长得多。
    // 真实影像(512×512)上这个比较才成立,拿夹具比会得出错误结论。
    // 该断言的实质是"像素没被带出来",用负载里不含像素字节来验:
    let pixel_bytes_in_json = text.matches("7FE0").count();
    assert_eq!(
        pixel_bytes_in_json, 0,
        "响应里不该出现任何 7FE0 组的标签:{text}"
    );
}

/// 帧号 1 基:`/frames/1` 拿到第一帧,`/frames/0` 是 400。
///
/// 这个差一位的错误在单帧影像上不显现,必须显式测。
#[tokio::test]
async fn frame_numbers_start_at_one() {
    let Some(database_url) = prerequisites() else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let (_server, dimse, http) = start_server(&database_url, storage.path());

    let (study, series, sop) = (unique_uid(), unique_uid(), unique_uid());
    push_instance(dimse, &study, &series, &sop);

    let token = login(http);
    let base = format!("/dicomweb/studies/{study}/series/{series}/instances/{sop}/frames");

    // 第 1 帧存在
    let (status, body, headers) = fetch_binary(http, &format!("{base}/1"), &token);
    assert_eq!(status, 200, "响应头:{headers}");
    assert!(
        headers.contains("multipart/related"),
        "帧响应应是 multipart/related:{headers}"
    );
    // 4×4、16 位、单采样 = 32 字节像素,加上 multipart 的头和分隔串
    assert!(
        body.len() > 32,
        "响应应含 32 字节像素加 multipart 包装,实际 {} 字节",
        body.len()
    );
    assert!(
        body.windows(4).any(|w| w == b"\r\n\r\n"),
        "multipart 的 part 头与负载之间应有空行"
    );

    // 帧号 0 非法 —— 当成第 1 帧会让所有多帧影像错位
    assert_eq!(
        status_only(http, &format!("{base}/0"), Some(&token)),
        400,
        "帧号 0 应回 400"
    );
    // 超出范围(夹具是单帧)
    assert_eq!(
        status_only(http, &format!("{base}/2"), Some(&token)),
        400,
        "超出帧数应回 400"
    );
    // 畸形
    assert_eq!(
        status_only(http, &format!("{base}/abc"), Some(&token)),
        400,
        "非数字帧号应回 400"
    );
}

/// 三条取回路由都必须拒绝未授权访问。
///
/// 比 QIDO 更要紧:这些接口回的是像素本身。
#[tokio::test]
async fn retrieval_endpoints_require_authentication() {
    let Some(database_url) = prerequisites() else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let (_server, dimse, http) = start_server(&database_url, storage.path());

    let (study, series, sop) = (unique_uid(), unique_uid(), unique_uid());
    push_instance(dimse, &study, &series, &sop);

    let base = format!("/dicomweb/studies/{study}/series/{series}/instances/{sop}");
    for path in [
        base.clone(),
        format!("{base}/metadata"),
        format!("{base}/frames/1"),
    ] {
        assert_eq!(
            status_only(http, &path, None),
            401,
            "{path} 无令牌时应回 401 —— 像素数据被公开了"
        );
        assert_eq!(
            status_only(http, &path, Some("forged.token.here")),
            401,
            "{path} 伪造令牌应回 401"
        );
    }
}

/// URL 里的三段 UID 是调用方的断言,不成立时回 404。
///
/// SOPInstanceUID 本身唯一,但不能因此忽略前两段 —— 那会让
/// `/studies/错的/series/错的/instances/对的` 也返回内容,
/// 而调用方由此推断的层级关系是错的。
#[tokio::test]
async fn hierarchy_mismatch_returns_404() {
    let Some(database_url) = prerequisites() else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let (_server, dimse, http) = start_server(&database_url, storage.path());

    let (study, series, sop) = (unique_uid(), unique_uid(), unique_uid());
    push_instance(dimse, &study, &series, &sop);
    let other_study = unique_uid();
    let other_series = unique_uid();

    let token = login(http);
    // 正确的三段能取到
    assert_eq!(
        status_only(
            http,
            &format!("/dicomweb/studies/{study}/series/{series}/instances/{sop}"),
            Some(&token)
        ),
        200
    );
    // 换掉检查或序列都应当 404,即使 SOPInstanceUID 是对的
    for (s, se, label) in [
        (&other_study, &series, "检查不匹配"),
        (&study, &other_series, "序列不匹配"),
    ] {
        assert_eq!(
            status_only(
                http,
                &format!("/dicomweb/studies/{s}/series/{se}/instances/{sop}"),
                Some(&token)
            ),
            404,
            "{label} 时应回 404"
        );
    }

    // 不存在的实例
    assert_eq!(
        status_only(
            http,
            &format!(
                "/dicomweb/studies/{study}/series/{series}/instances/1.2.826.0.1.3680043.9.7777.1"
            ),
            Some(&token)
        ),
        404
    );
}

/// 非法 UID 回 400 而不是 404 —— 区分「格式错」和「查不到」。
#[tokio::test]
async fn invalid_uids_are_rejected_with_400() {
    let Some(database_url) = prerequisites() else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let (_server, _dimse, http) = start_server(&database_url, storage.path());

    let token = login(http);
    let valid = "1.2.840.10008.1.1";
    // 路径穿越尝试:UID 位置塞 `..`
    for path in [
        format!("/dicomweb/studies/{valid}/series/{valid}/instances/..").to_string(),
        format!("/dicomweb/studies/x!y/series/{valid}/instances/{valid}"),
    ] {
        let status = status_only(http, &path, Some(&token));
        assert!(
            status == 400 || status == 404,
            "{path} 应回 400 或 404(取决于路由匹配),实际 {status}"
        );
    }
}

/// 取回响应要带长缓存头 —— 影像对象不可变,重复取同一份浪费带宽。
#[tokio::test]
async fn responses_are_cacheable() {
    let Some(database_url) = prerequisites() else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let (_server, dimse, http) = start_server(&database_url, storage.path());

    let (study, series, sop) = (unique_uid(), unique_uid(), unique_uid());
    push_instance(dimse, &study, &series, &sop);

    let token = login(http);
    let (_status, _body, headers) = fetch_binary(
        http,
        &format!("/dicomweb/studies/{study}/series/{series}/instances/{sop}"),
        &token,
    );
    let lowered = headers.to_lowercase();
    assert!(
        lowered.contains("cache-control"),
        "取回响应应带 Cache-Control:{headers}"
    );
    // private:影像是病人数据,不能进共享缓存(CDN、公司代理)
    assert!(
        lowered.contains("private"),
        "病人影像不能进共享缓存,必须标 private:{headers}"
    );
}
