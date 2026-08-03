//! QIDO-RS 集成测试:HTTPS + 认证 + 真实查询。
//!
//! 最要紧的一条是「未授权必须被拒绝」——一个不设防的查询接口等于把全部病人
//! 元数据公开。而且要**逐个层级单独测**:三条路由如果有一条漏挂中间件,
//! 只测 `/studies` 是发现不了的。
//!
//! 用 `curl -k` 而不是 Rust HTTP 客户端:证书是自签的,而这里要验的是
//! HTTP 语义(状态码、响应头、JSON 结构),不是 TLS 校验。

use std::net::{SocketAddr, TcpListener};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use dicom::core::{DataElement, VR};
use dicom::dictionary_std::tags;
use pacs_core::fixture::{ct_instance, unique_uid};

const CALLED_AE: &str = "REMOTE_PACS";
const CALLING_AE: &str = "TEST_SCU";
/// 测试专用签名密钥。必须同时给建账号和跑服务端 —— 两边不一致的话
/// 令牌验签必然失败,而症状是「密码明明对却登不上」。
const TEST_JWT_SECRET: &str = "integration-test-secret-long-enough-for-hs256";
const ADMIN_USER: &str = "qido.tester";
const ADMIN_PASSWORD: &str = "qido-test-password-1234";

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
        eprintln!("\n>>> 跳过 QIDO-RS 测试:未设置 PACS_TEST_DATABASE_URL。\n");
        return None;
    };
    for tool in ["curl", "storescu"] {
        if !tool_available(tool) {
            assert!(!in_ci, "CI 必须有 {tool}");
            eprintln!("\n>>> 跳过 QIDO-RS 测试:未找到 {tool}。\n");
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
        .expect("应能取到空闲端口")
        .local_addr()
        .expect("应能读出端口")
        .port()
}

/// 起一个服务端,返回 (guard, dimse_port, http_port)。
fn start_server(database_url: &str, storage_root: &Path) -> (ServerGuard, u16, u16) {
    let dimse_port = free_port();
    let http_port = free_port();

    // 账号必须先建好:建账号和服务端要用同一个 JWT 密钥
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
    // 账号已存在是正常的(多个测试共用一个库),其他失败才是问题
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

fn push_instance(dimse_port: u16, study: &str, series: &str, sop: &str) {
    push_instance_for_patient(dimse_port, study, series, sop, "PID-0001");
}

fn push_instance_for_patient(
    dimse_port: u16,
    study: &str,
    series: &str,
    sop: &str,
    patient_id: &str,
) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("instance.dcm");
    let mut object = ct_instance(study, series, sop);
    object.put(DataElement::new(tags::PATIENT_ID, VR::LO, patient_id));
    object.write_to_file(&file).unwrap();

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
}

/// 一次 HTTP 请求的结果。
struct HttpResponse {
    status: u16,
    body: String,
    headers: String,
}

fn request(http_port: u16, path: &str, token: Option<&str>) -> HttpResponse {
    let url = format!("https://127.0.0.1:{http_port}{path}");
    let mut args: Vec<String> = vec![
        "-k".into(), // 自签证书
        "-s".into(),
        "-D".into(),
        "-".into(), // 响应头写到 stdout
        "-o".into(),
        "/dev/stderr".into(), // 响应体单独走 stderr,方便分开取
        "-w".into(),
        "%{http_code}".into(),
        url,
    ];
    if let Some(token) = token {
        args.push("-H".into());
        args.push(format!("Authorization: Bearer {token}"));
    }

    let output = Command::new("curl")
        .args(&args)
        .output()
        .expect("应能执行 curl");
    let headers_and_code = String::from_utf8_lossy(&output.stdout).to_string();
    let body = String::from_utf8_lossy(&output.stderr).to_string();

    // -w 的输出追加在最后,取末尾三位数字
    let status = headers_and_code
        .trim_end()
        .chars()
        .rev()
        .take(3)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>()
        .parse::<u16>()
        .unwrap_or_else(|_| panic!("无法解析状态码,curl 输出:{headers_and_code}"));

    HttpResponse {
        status,
        body,
        headers: headers_and_code,
    }
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

/// 三条路由都必须拒绝未授权访问。
///
/// 逐条测而不是只测 `/studies`:漏挂中间件是「少一行」的错误,
/// 只测一条路由发现不了另外两条的缺口。
#[tokio::test]
async fn every_endpoint_rejects_unauthenticated_requests() {
    let Some(database_url) = prerequisites() else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let (_server, _dimse, http) = start_server(&database_url, storage.path());

    let uid = "1.2.840.10008.1.1";
    for path in [
        "/dicomweb/studies",
        &format!("/dicomweb/studies/{uid}/series"),
        &format!("/dicomweb/studies/{uid}/series/{uid}/instances"),
        "/api/patients",
        "/api/patients/1/studies",
        &format!("/api/studies/{uid}/series"),
    ] {
        // 完全没有令牌
        let response = request(http, path, None);
        assert_eq!(
            response.status, 401,
            "{path} 无令牌时应回 401,实际 {} —— 病人元数据被公开了",
            response.status
        );
        // RFC 6750:401 要带 WWW-Authenticate,客户端靠它知道去拿令牌
        assert!(
            response.headers.to_lowercase().contains("www-authenticate"),
            "{path} 的 401 应带 WWW-Authenticate 头:{}",
            response.headers
        );

        // 伪造的令牌
        let forged = request(http, path, Some("not.a.real.token"));
        assert_eq!(forged.status, 401, "{path} 伪造令牌时应回 401");
    }
}

/// DCMTK 推送后，应用工作列表按病人 → 检查 → 序列返回，而不是每个检查重复一位病人。
#[tokio::test]
async fn worklist_groups_dcmtk_uploads_into_patient_study_series() {
    let Some(database_url) = prerequisites() else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let (_server, dimse, http) = start_server(&database_url, storage.path());

    let patient_id = format!("WORKLIST-{}", unique_uid());
    let study_a = unique_uid();
    let study_b = unique_uid();
    let series_a = unique_uid();
    let series_b = unique_uid();
    push_instance_for_patient(dimse, &study_a, &series_a, &unique_uid(), &patient_id);
    push_instance_for_patient(dimse, &study_b, &series_b, &unique_uid(), &patient_id);

    let token = login(http);
    let patients = request(
        http,
        &format!("/api/patients?query={patient_id}&limit=10&offset=0"),
        Some(&token),
    );
    assert_eq!(patients.status, 200, "响应体:{}", patients.body);
    let rows: serde_json::Value = serde_json::from_str(&patients.body).unwrap();
    let rows = rows.as_array().expect("病人工作列表应为数组");
    assert_eq!(rows.len(), 1, "同一病人不能按检查重复:{}", patients.body);
    assert_eq!(rows[0]["patient_id"], patient_id);
    assert_eq!(rows[0]["study_count"], 2);
    let patient_db_id = rows[0]["id"].as_i64().expect("应返回病人数据库 ID");

    let studies = request(
        http,
        &format!("/api/patients/{patient_db_id}/studies"),
        Some(&token),
    );
    assert_eq!(studies.status, 200, "响应体:{}", studies.body);
    assert!(studies.body.contains(&study_a));
    assert!(studies.body.contains(&study_b));

    let series = request(
        http,
        &format!("/api/studies/{study_a}/series"),
        Some(&token),
    );
    assert_eq!(series.status, 200, "响应体:{}", series.body);
    assert!(series.body.contains(&series_a));
    assert!(!series.body.contains(&series_b));
}

/// 已认证用户不能通过任一读取接口探测或下载其他机构的数据。
#[tokio::test]
async fn qido_wado_and_worklist_enforce_institution_scope() {
    let Some(database_url) = prerequisites() else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let (_server, dimse, http) = start_server(&database_url, storage.path());

    let patient_id = format!("TENANT-{}", unique_uid());
    let (study, series, sop) = (unique_uid(), unique_uid(), unique_uid());
    push_instance_for_patient(dimse, &study, &series, &sop, &patient_id);

    let pool = pacs_db::connect(&database_url).await.unwrap();
    let institution_code = format!("tenant-{}", unique_uid());
    let institution_id: i64 =
        sqlx::query_scalar("INSERT INTO institutions (code, name) VALUES ($1, $2) RETURNING id")
            .bind(&institution_code)
            .bind(&institution_code)
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query("UPDATE patients SET institution_id = $1 WHERE patient_id = $2")
        .bind(institution_id)
        .bind(&patient_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE studies SET institution_id = $1 WHERE study_instance_uid = $2")
        .bind(institution_id)
        .bind(&study)
        .execute(&pool)
        .await
        .unwrap();

    let token = login(http);
    let qido = request(
        http,
        &format!("/dicomweb/studies?StudyInstanceUID={study}"),
        Some(&token),
    );
    assert_eq!(qido.status, 204, "跨机构 QIDO 不应返回检查:{}", qido.body);

    let wado = request(
        http,
        &format!("/dicomweb/studies/{study}/series/{series}/instances/{sop}"),
        Some(&token),
    );
    assert_eq!(wado.status, 404, "跨机构 WADO 应表现为未找到");

    let worklist = request(
        http,
        &format!("/api/patients?query={patient_id}"),
        Some(&token),
    );
    assert_eq!(worklist.status, 200);
    assert_eq!(worklist.body.trim(), "[]", "跨机构病人不应出现在工作列表");
}

/// 带有效令牌能查到刚推进去的检查,响应是 DICOM JSON Model。
#[tokio::test]
async fn study_query_returns_dicom_json() {
    let Some(database_url) = prerequisites() else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let (_server, dimse, http) = start_server(&database_url, storage.path());

    let (study, series, sop) = (unique_uid(), unique_uid(), unique_uid());
    push_instance(dimse, &study, &series, &sop);

    let token = login(http);
    let response = request(
        http,
        &format!("/dicomweb/studies?StudyInstanceUID={study}&includefield=StudyDescription"),
        Some(&token),
    );
    assert_eq!(response.status, 200, "响应体:{}", response.body);
    assert!(
        response.headers.contains("application/dicom+json"),
        "Content-Type 应是 application/dicom+json:{}",
        response.headers
    );

    let parsed: serde_json::Value = serde_json::from_str(&response.body)
        .unwrap_or_else(|_| panic!("响应不是 JSON:{}", response.body));
    let array = parsed.as_array().expect("DICOM JSON Model 是数组");
    assert_eq!(array.len(), 1, "应只命中一个检查:{}", response.body);

    // DICOM JSON Model:标签作键,值在 "Value" 数组里,并带 "vr"
    let entry = &array[0];
    assert_eq!(
        entry["0020000D"]["vr"], "UI",
        "StudyInstanceUID 的 VR 应是 UI:{entry}"
    );
    assert_eq!(
        entry["0020000D"]["Value"][0], study,
        "应回刚推进去的 StudyInstanceUID"
    );
    // QueryRetrieveLevel 是 DIMSE 的概念,不该出现在 QIDO 响应里
    assert!(
        entry.get("00080052").is_none(),
        "响应里不该有 QueryRetrieveLevel:{entry}"
    );
}

/// 查不到时回 204,不是 200 带空数组、也不是 500。
#[tokio::test]
async fn no_matches_returns_no_content() {
    let Some(database_url) = prerequisites() else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let (_server, _dimse, http) = start_server(&database_url, storage.path());

    let token = login(http);
    let response = request(
        http,
        "/dicomweb/studies?StudyInstanceUID=1.2.826.0.1.3680043.9.9999.888888",
        Some(&token),
    );
    assert_eq!(response.status, 204, "响应体:{}", response.body);
    assert!(response.body.trim().is_empty(), "204 不该有响应体");
}

/// 层级查询要被 URL 路径里的 UID 约束住。
#[tokio::test]
async fn hierarchical_queries_are_constrained_by_the_path() {
    let Some(database_url) = prerequisites() else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let (_server, dimse, http) = start_server(&database_url, storage.path());

    // 两个检查各一个序列
    let (study_a, series_a) = (unique_uid(), unique_uid());
    let (study_b, series_b) = (unique_uid(), unique_uid());
    push_instance(dimse, &study_a, &series_a, &unique_uid());
    push_instance(dimse, &study_b, &series_b, &unique_uid());

    let token = login(http);
    let response = request(
        http,
        &format!("/dicomweb/studies/{study_a}/series?includefield=SeriesInstanceUID"),
        Some(&token),
    );
    assert_eq!(response.status, 200, "响应体:{}", response.body);

    let parsed: serde_json::Value = serde_json::from_str(&response.body).unwrap();
    let array = parsed.as_array().unwrap();
    assert_eq!(array.len(), 1, "study_a 下只有一个序列:{}", response.body);
    assert_eq!(array[0]["0020000E"]["Value"][0], series_a);
    // 绝不能串到另一个检查的序列
    assert!(
        !response.body.contains(&series_b),
        "查 study_a 的序列不该返回 study_b 的:{}",
        response.body
    );
}

/// 路径里的 UID 覆盖同名查询参数,不与之并存。
///
/// 若两个条件用 AND 并存,这个请求会变成永远查不到的矛盾条件,
/// 而调用方收到空结果看不出哪里错了。
#[tokio::test]
async fn path_uid_overrides_a_conflicting_query_parameter() {
    let Some(database_url) = prerequisites() else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let (_server, dimse, http) = start_server(&database_url, storage.path());

    let (study_a, series_a) = (unique_uid(), unique_uid());
    let study_b = unique_uid();
    push_instance(dimse, &study_a, &series_a, &unique_uid());
    push_instance(dimse, &study_b, &unique_uid(), &unique_uid());

    let token = login(http);
    // 路径说 study_a,参数说 study_b —— 路径应当胜出
    let response = request(
        http,
        &format!("/dicomweb/studies/{study_a}/series?StudyInstanceUID={study_b}"),
        Some(&token),
    );
    assert_eq!(
        response.status, 200,
        "路径 UID 应覆盖冲突参数而不是产生空结果:{}",
        response.body
    );
    assert!(
        response.body.contains(&series_a),
        "应返回路径指定的检查下的序列:{}",
        response.body
    );
}

/// 畸形请求回 400,不是 500。
#[tokio::test]
async fn malformed_requests_are_rejected_with_400() {
    let Some(database_url) = prerequisites() else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let (_server, _dimse, http) = start_server(&database_url, storage.path());

    let token = login(http);
    for (path, why) in [
        ("/dicomweb/studies?NoSuchAttribute=x", "无法识别的参数"),
        ("/dicomweb/studies?limit=abc", "limit 不是数字"),
        ("/dicomweb/studies?limit=0", "limit 为 0"),
        ("/dicomweb/studies/not..a..uid/series", "路径 UID 非法"),
    ] {
        let response = request(http, path, Some(&token));
        assert_eq!(
            response.status, 400,
            "{why} 应回 400,实际 {}:{}",
            response.status, response.body
        );
    }
}

/// 未实现的参数通过 Warning 头告知,而不是静默接受。
#[tokio::test]
async fn unsupported_parameters_are_announced_in_a_warning_header() {
    let Some(database_url) = prerequisites() else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let (_server, dimse, http) = start_server(&database_url, storage.path());

    let (study, series, sop) = (unique_uid(), unique_uid(), unique_uid());
    push_instance(dimse, &study, &series, &sop);

    let token = login(http);
    let response = request(
        http,
        &format!("/dicomweb/studies?StudyInstanceUID={study}&fuzzymatching=true"),
        Some(&token),
    );
    assert_eq!(response.status, 200, "响应体:{}", response.body);
    assert!(
        response.headers.to_lowercase().contains("warning"),
        "fuzzymatching=true 未实现,应带 Warning 头:{}",
        response.headers
    );

    // fuzzymatching=false 等于没提要求,不该告警
    let quiet = request(
        http,
        &format!("/dicomweb/studies?StudyInstanceUID={study}&fuzzymatching=false"),
        Some(&token),
    );
    assert!(
        !quiet.headers.to_lowercase().contains("warning"),
        "fuzzymatching=false 不该告警:{}",
        quiet.headers
    );
}

/// limit 与 offset 要真的分页。
#[tokio::test]
async fn limit_and_offset_paginate() {
    let Some(database_url) = prerequisites() else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let (_server, dimse, http) = start_server(&database_url, storage.path());

    let study = unique_uid();
    for _ in 0..3 {
        push_instance(dimse, &study, &unique_uid(), &unique_uid());
    }

    let token = login(http);
    let all = request(
        http,
        &format!("/dicomweb/studies/{study}/series"),
        Some(&token),
    );
    let total = serde_json::from_str::<serde_json::Value>(&all.body)
        .unwrap()
        .as_array()
        .unwrap()
        .len();
    assert_eq!(total, 3);

    let first_page = request(
        http,
        &format!("/dicomweb/studies/{study}/series?limit=2"),
        Some(&token),
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&first_page.body)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        2,
        "limit=2 应只回两条"
    );

    let second_page = request(
        http,
        &format!("/dicomweb/studies/{study}/series?limit=2&offset=2"),
        Some(&token),
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&second_page.body)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1,
        "offset=2 之后只剩一条"
    );
}
