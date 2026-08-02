//! C-FIND 互操作测试:用 DCMTK 的 `findscu` 打真实查询。
//!
//! 这是阶段 4 的验收标准。匹配语义是 C-FIND 最容易出错的部分 ——
//! 通配符、日期范围、层级归属,每一条都用真实客户端验一遍。
//!
//! 验证方式是让 `findscu -X` 把每条响应写成 `rsp0001.dcm`,再用 dicom-rs
//! 解出来断言。不看退出码:**查不到任何结果时 findscu 一样返回 0**,
//! 只看退出码的测试会把「一条都没匹配上」当成通过。

use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use dicom::object::FileDicomObject;
use pacs_core::fixture::{ct_instance, unique_uid};

const CALLED_AE: &str = "REMOTE_PACS";
const CALLING_AE: &str = "TEST_SCU";

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
        eprintln!("\n>>> 跳过 C-FIND 互操作测试:未设置 PACS_TEST_DATABASE_URL。\n");
        return None;
    };
    if !dcmtk_available() {
        assert!(!in_ci, "CI 必须安装 DCMTK,否则互操作无从验证");
        eprintln!("\n>>> 跳过 C-FIND 互操作测试:未找到 DCMTK(brew install dcmtk)。\n");
        return None;
    }
    Some(database_url)
}

fn dcmtk_available() -> bool {
    Command::new("findscu")
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

fn start_pacsd(database_url: &str, storage_root: &Path, port: u16) -> ServerGuard {
    let child = Command::new(env!("CARGO_BIN_EXE_pacsd"))
        .env("DATABASE_URL", database_url)
        .env("PACS_STORAGE_ROOT", storage_root)
        .env("PACS_DIMSE_BIND", format!("127.0.0.1:{port}"))
        .env("PACS_AE_TITLE", CALLED_AE)
        .env("RUST_LOG", "info,pacsd=debug,pacs_dimse=debug")
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("应能启动 pacsd");
    let guard = ServerGuard(child);

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

/// 造一份 CT 推给服务端,返回它的三个 UID。
fn push_instance(port: u16, study: &str, series: &str, sop: &str) {
    let source_dir = tempfile::tempdir().unwrap();
    let file = source_dir.path().join("instance.dcm");
    ct_instance(study, series, sop)
        .write_to_file(&file)
        .unwrap();

    let output = Command::new("storescu")
        .args([
            "-aec",
            CALLED_AE,
            "-aet",
            CALLING_AE,
            "127.0.0.1",
            &port.to_string(),
            file.to_str().unwrap(),
        ])
        .output()
        .expect("应能执行 storescu");
    assert!(
        output.status.success(),
        "storescu 应成功:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// 跑一次 findscu,返回解析好的响应标识符。
///
/// `root` 传 `-S`(Study Root)或 `-P`(Patient Root)。
fn findscu(
    port: u16,
    root: &str,
    keys: &[&str],
) -> Vec<FileDicomObject<dicom::object::InMemDicomObject>> {
    // 响应文件写进各自的临时目录:不隔离的话并行跑的测试会读到对方的 rsp0001.dcm
    let workdir = tempfile::tempdir().unwrap();

    let mut args: Vec<String> = vec![
        root.to_owned(),
        "-X".to_owned(), // 把每条响应写成 rsp0001.dcm、rsp0002.dcm……
        "-od".to_owned(),
        workdir.path().to_str().unwrap().to_owned(),
        "-aec".to_owned(),
        CALLED_AE.to_owned(),
        "-aet".to_owned(),
        CALLING_AE.to_owned(),
    ];
    for key in keys {
        args.push("-k".to_owned());
        args.push((*key).to_owned());
    }
    args.push("127.0.0.1".to_owned());
    args.push(port.to_string());

    let output = Command::new("findscu")
        .args(&args)
        .output()
        .expect("应能执行 findscu");
    assert!(
        output.status.success(),
        "findscu 不该以失败退出(查不到结果也应是 0):\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let mut files: Vec<PathBuf> = std::fs::read_dir(workdir.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("rsp") && name.ends_with(".dcm"))
        })
        .collect();
    files.sort();

    files
        .iter()
        .map(|path| FileDicomObject::open_file(path).expect("响应文件应能解析"))
        .collect()
}

/// 读一个字符串属性,去掉 DICOM 的补齐。
fn text(
    object: &FileDicomObject<dicom::object::InMemDicomObject>,
    tag: dicom::core::Tag,
) -> Option<String> {
    let raw = object.get(tag)?.to_str().ok()?;
    let trimmed = raw.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// STUDY 层查询:按 StudyInstanceUID 精确命中,并带回请求的返回键。
#[tokio::test]
async fn study_level_query_returns_the_requested_keys() {
    let Some(database_url) = prerequisites() else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let port = free_port();
    let _server = start_pacsd(&database_url, storage.path(), port);

    let (study, series, sop) = (unique_uid(), unique_uid(), unique_uid());
    push_instance(port, &study, &series, &sop);

    let responses = findscu(
        port,
        "-S",
        &[
            "QueryRetrieveLevel=STUDY",
            &format!("StudyInstanceUID={study}"),
            "PatientName",
            "PatientID",
            "StudyDate",
            "ModalitiesInStudy",
            "NumberOfStudyRelatedInstances",
        ],
    );

    assert_eq!(responses.len(), 1, "唯一键精确匹配应只回一条");
    let response = &responses[0];
    assert_eq!(
        text(response, dicom::dictionary_std::tags::STUDY_INSTANCE_UID).as_deref(),
        Some(study.as_str())
    );
    // 聚合列要真的算出来,不能是占位的 0
    assert_eq!(
        text(
            response,
            dicom::dictionary_std::tags::NUMBER_OF_STUDY_RELATED_INSTANCES
        )
        .as_deref(),
        Some("1")
    );
    assert_eq!(
        text(response, dicom::dictionary_std::tags::MODALITIES_IN_STUDY).as_deref(),
        Some("CT")
    );
    // 请求了就必须出现在响应里,哪怕库里是空值
    assert!(
        response
            .get(dicom::dictionary_std::tags::PATIENT_ID)
            .is_some(),
        "请求的返回键必须出现在响应里"
    );
}

/// 通配符匹配:`*` 应当命中,且不该退化成「匹配一切」。
#[tokio::test]
async fn wildcard_matching_on_patient_name() {
    let Some(database_url) = prerequisites() else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let port = free_port();
    let _server = start_pacsd(&database_url, storage.path(), port);

    let (study, series, sop) = (unique_uid(), unique_uid(), unique_uid());
    push_instance(port, &study, &series, &sop);

    // fixture 的 PatientName 是已知的,取出来做前缀
    let exact = findscu(
        port,
        "-S",
        &[
            "QueryRetrieveLevel=STUDY",
            &format!("StudyInstanceUID={study}"),
            "PatientName",
        ],
    );
    let name = text(&exact[0], dicom::dictionary_std::tags::PATIENT_NAME)
        .expect("fixture 应当有 PatientName");
    let prefix: String = name.chars().take(3).collect();

    // 前缀通配应当命中这个检查
    let hit = findscu(
        port,
        "-S",
        &[
            "QueryRetrieveLevel=STUDY",
            &format!("StudyInstanceUID={study}"),
            &format!("PatientName={prefix}*"),
        ],
    );
    assert_eq!(hit.len(), 1, "前缀通配 {prefix}* 应命中 {name}");

    // 匹配不上的通配不能回结果 —— 通配符实现错了最典型的症状就是「什么都匹配」
    let miss = findscu(
        port,
        "-S",
        &[
            "QueryRetrieveLevel=STUDY",
            &format!("StudyInstanceUID={study}"),
            "PatientName=ZZZNOSUCH*",
        ],
    );
    assert!(miss.is_empty(), "不匹配的通配不该回任何结果");
}

/// 日期范围:三种写法(闭区间、只有下界、只有上界)都要正确。
#[tokio::test]
async fn date_range_matching_covers_open_and_closed_forms() {
    let Some(database_url) = prerequisites() else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let port = free_port();
    let _server = start_pacsd(&database_url, storage.path(), port);

    let (study, series, sop) = (unique_uid(), unique_uid(), unique_uid());
    push_instance(port, &study, &series, &sop);

    let baseline = findscu(
        port,
        "-S",
        &[
            "QueryRetrieveLevel=STUDY",
            &format!("StudyInstanceUID={study}"),
            "StudyDate",
        ],
    );
    let Some(date) = text(&baseline[0], dicom::dictionary_std::tags::STUDY_DATE) else {
        eprintln!(">>> fixture 没有 StudyDate,跳过日期范围断言");
        return;
    };

    let uid_key = format!("StudyInstanceUID={study}");
    for (label, range) in [
        ("闭区间", format!("{date}-{date}")),
        ("只有下界", format!("{date}-")),
        ("只有上界", format!("-{date}")),
    ] {
        let responses = findscu(
            port,
            "-S",
            &[
                "QueryRetrieveLevel=STUDY",
                &uid_key,
                &format!("StudyDate={range}"),
            ],
        );
        assert_eq!(responses.len(), 1, "{label} {range} 应命中该检查");
    }

    // 区间落在检查日期之前,必须为空
    let before = findscu(
        port,
        "-S",
        &[
            "QueryRetrieveLevel=STUDY",
            &uid_key,
            "StudyDate=19000101-19000102",
        ],
    );
    assert!(before.is_empty(), "范围之外的日期不该命中");
}

/// SERIES 层:在指定检查下列出序列。
#[tokio::test]
async fn series_level_query_lists_series_under_a_study() {
    let Some(database_url) = prerequisites() else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let port = free_port();
    let _server = start_pacsd(&database_url, storage.path(), port);

    let study = unique_uid();
    let series_uids: Vec<String> = (0..3).map(|_| unique_uid()).collect();
    for series in &series_uids {
        push_instance(port, &study, series, &unique_uid());
    }

    let responses = findscu(
        port,
        "-S",
        &[
            "QueryRetrieveLevel=SERIES",
            &format!("StudyInstanceUID={study}"),
            "SeriesInstanceUID",
            "Modality",
            "NumberOfSeriesRelatedInstances",
        ],
    );
    assert_eq!(responses.len(), 3, "该检查下有三个序列");

    let returned: std::collections::HashSet<String> = responses
        .iter()
        .filter_map(|r| text(r, dicom::dictionary_std::tags::SERIES_INSTANCE_UID))
        .collect();
    for series in &series_uids {
        assert!(returned.contains(series), "{series} 应出现在结果里");
    }
    for response in &responses {
        assert_eq!(
            text(response, dicom::dictionary_std::tags::MODALITY).as_deref(),
            Some("CT")
        );
    }
}

/// IMAGE 层:在指定序列下列出实例。
#[tokio::test]
async fn image_level_query_lists_instances_under_a_series() {
    let Some(database_url) = prerequisites() else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let port = free_port();
    let _server = start_pacsd(&database_url, storage.path(), port);

    let (study, series) = (unique_uid(), unique_uid());
    let sop_uids: Vec<String> = (0..4).map(|_| unique_uid()).collect();
    for sop in &sop_uids {
        push_instance(port, &study, &series, sop);
    }

    let responses = findscu(
        port,
        "-S",
        &[
            "QueryRetrieveLevel=IMAGE",
            &format!("StudyInstanceUID={study}"),
            &format!("SeriesInstanceUID={series}"),
            "SOPInstanceUID",
            "Rows",
            "Columns",
        ],
    );
    assert_eq!(responses.len(), 4);

    let returned: std::collections::HashSet<String> = responses
        .iter()
        .filter_map(|r| text(r, dicom::dictionary_std::tags::SOP_INSTANCE_UID))
        .collect();
    for sop in &sop_uids {
        assert!(returned.contains(sop));
    }
    // Rows/Columns 是 US(二进制),不能当成字符串写出去
    let rows = responses[0]
        .get(dicom::dictionary_std::tags::ROWS)
        .and_then(|e| e.to_int::<u16>().ok());
    assert_eq!(rows, Some(4), "Rows 应作为二进制 US 回传");
}

/// Patient Root 的 PATIENT 层查询。
#[tokio::test]
async fn patient_root_supports_the_patient_level() {
    let Some(database_url) = prerequisites() else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let port = free_port();
    let _server = start_pacsd(&database_url, storage.path(), port);

    let (study, series, sop) = (unique_uid(), unique_uid(), unique_uid());
    push_instance(port, &study, &series, &sop);

    let baseline = findscu(
        port,
        "-S",
        &[
            "QueryRetrieveLevel=STUDY",
            &format!("StudyInstanceUID={study}"),
            "PatientID",
        ],
    );
    let Some(patient_id) = text(&baseline[0], dicom::dictionary_std::tags::PATIENT_ID) else {
        eprintln!(">>> fixture 的 PatientID 为空,跳过 PATIENT 层断言");
        return;
    };

    let responses = findscu(
        port,
        "-P", // Patient Root
        &[
            "QueryRetrieveLevel=PATIENT",
            &format!("PatientID={patient_id}"),
            "PatientName",
            "PatientBirthDate",
        ],
    );
    assert_eq!(responses.len(), 1, "PatientID 是唯一键,应只回一条");
    assert_eq!(
        text(&responses[0], dicom::dictionary_std::tags::PATIENT_ID).as_deref(),
        Some(patient_id.as_str())
    );
}

/// Study Root 没有 PATIENT 层,对它发 PATIENT 层查询必须被拒绝。
///
/// 这条不是吹毛求疵:把 PATIENT 层查询当 STUDY 层将就处理,会回一批对方
/// 根本没请求的记录,而对方无从分辨。
#[tokio::test]
async fn study_root_rejects_a_patient_level_query() {
    let Some(database_url) = prerequisites() else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let port = free_port();
    let _server = start_pacsd(&database_url, storage.path(), port);

    let (study, series, sop) = (unique_uid(), unique_uid(), unique_uid());
    push_instance(port, &study, &series, &sop);

    let workdir = tempfile::tempdir().unwrap();
    let output = Command::new("findscu")
        .args([
            "-S", // Study Root
            "-X",
            "-v", // 让 findscu 把收到的状态打出来
            "-od",
            workdir.path().to_str().unwrap(),
            "-aec",
            CALLED_AE,
            "-aet",
            CALLING_AE,
            "-k",
            "QueryRetrieveLevel=PATIENT",
            "-k",
            "PatientID",
            "127.0.0.1",
            &port.to_string(),
        ])
        .output()
        .expect("应能执行 findscu");

    let logged = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // 光断言"没有结果"不够:服务端错误地回一条空的 SUCCESS 时同样没有结果。
    // 必须确认状态码真的是 0xA900(DCMTK 把它显示为 DataSetDoesNotMatchSOPClass)。
    assert!(
        logged.contains("DataSetDoesNotMatchSOPClass"),
        "Study Root 的 PATIENT 层查询应回 0xA900,实际日志:\n{logged}"
    );

    let responses = std::fs::read_dir(workdir.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("rsp"))
        .count();
    assert_eq!(responses, 0, "被拒绝的查询不该返回任何结果:\n{logged}");
}

/// 查询值里字面的 `%` 不能当通配符 —— 否则 `%` 会匹配到一切。
///
/// 这条验的是端到端的翻译链路:`%` 经过 `wildcard_to_sql_like` 转义、
/// 绑进参数、被 Postgres 按字面量对待。
/// (转义符本身由 `wildcard_to_sql_like` 的单元测试盯着 —— Postgres 的 LIKE
/// 默认转义符恰好也是 `\`,所以这条集成测试分辨不出 `ESCAPE` 子句在不在。)
#[tokio::test]
async fn a_literal_percent_sign_does_not_match_everything() {
    let Some(database_url) = prerequisites() else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let port = free_port();
    let _server = start_pacsd(&database_url, storage.path(), port);

    let (study, series, sop) = (unique_uid(), unique_uid(), unique_uid());
    push_instance(port, &study, &series, &sop);

    // fixture 的病人叫 Doe^John,名字里没有 `%`
    let responses = findscu(
        port,
        "-S",
        &[
            "QueryRetrieveLevel=STUDY",
            &format!("StudyInstanceUID={study}"),
            "PatientName=%",
        ],
    );
    assert!(
        responses.is_empty(),
        "字面的 % 被当成了 SQL 通配符 —— 转义没生效"
    );

    // 对照:`*` 才是 DICOM 的通配符,应当命中
    let wildcard = findscu(
        port,
        "-S",
        &[
            "QueryRetrieveLevel=STUDY",
            &format!("StudyInstanceUID={study}"),
            "PatientName=*",
        ],
    );
    assert_eq!(wildcard.len(), 1, "DICOM 的 * 应当匹配到该病人");
}

/// STUDY 层收到 Series 层的键(`Modality`)时,忽略它并回 0xFF01,而不是崩。
///
/// 层级作用域一旦失守,拼出来的 SQL 会引用没 JOIN 进来的 `se.modality`,
/// Postgres 直接报错、整次查询变成 0xC000。真实客户端把 `Modality` 误用在
/// STUDY 层是常见笔误(标准里 STUDY 层该用 `ModalitiesInStudy`),
/// 不能因为这个笔误就返回失败。
#[tokio::test]
async fn a_lower_level_key_at_study_level_is_ignored_with_a_warning() {
    let Some(database_url) = prerequisites() else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let port = free_port();
    let _server = start_pacsd(&database_url, storage.path(), port);

    let (study, series, sop) = (unique_uid(), unique_uid(), unique_uid());
    push_instance(port, &study, &series, &sop);

    let workdir = tempfile::tempdir().unwrap();
    let output = Command::new("findscu")
        .args([
            "-S",
            "-X",
            "-v",
            "-od",
            workdir.path().to_str().unwrap(),
            "-aec",
            CALLED_AE,
            "-aet",
            CALLING_AE,
            "-k",
            "QueryRetrieveLevel=STUDY",
            "-k",
            &format!("StudyInstanceUID={study}"),
            // Series 层的键,STUDY 层没有对应列
            "-k",
            "Modality=CT",
            "127.0.0.1",
            &port.to_string(),
        ])
        .output()
        .expect("应能执行 findscu");

    let logged = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // 结果照回(不支持的键被忽略),而不是以失败收尾
    let responses = std::fs::read_dir(workdir.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("rsp"))
        .count();
    assert_eq!(
        responses, 1,
        "不支持的键应被忽略、检查照样返回,实际日志:\n{logged}"
    );
    assert!(
        !logged.contains("UnableToProcess") && !logged.contains("Failed"),
        "不该因为一个不支持的键就让整次查询失败:\n{logged}"
    );
    // 0xFF01 = Pending with warning。DCMTK 把它显示成
    // "Pending: WarningUnsupportedOptionalKeys" —— 正是"有键没能支持"的意思。
    assert!(
        logged.contains("WarningUnsupportedOptionalKeys"),
        "应以 0xFF01 告知对方有键未被支持,实际日志:\n{logged}"
    );
}

/// 查不到时要干净地回一条 SUCCESS,而不是报错或挂起。
#[tokio::test]
async fn a_query_with_no_matches_completes_successfully() {
    let Some(database_url) = prerequisites() else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let port = free_port();
    let _server = start_pacsd(&database_url, storage.path(), port);

    let responses = findscu(
        port,
        "-S",
        &[
            "QueryRetrieveLevel=STUDY",
            // 合法但库里必然没有的 UID
            "StudyInstanceUID=1.2.826.0.1.3680043.9.9999.999999999",
            "PatientName",
        ],
    );
    assert!(responses.is_empty(), "不该有命中");
}

/// C-ECHO 和 C-FIND 走同一条 association 都要能用 —— 查看器就是这么连的。
#[tokio::test]
async fn find_and_store_coexist_on_the_same_server() {
    let Some(database_url) = prerequisites() else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let port = free_port();
    let _server = start_pacsd(&database_url, storage.path(), port);

    let echo = Command::new("echoscu")
        .args([
            "-aec",
            CALLED_AE,
            "-aet",
            CALLING_AE,
            "127.0.0.1",
            &port.to_string(),
        ])
        .output()
        .expect("应能执行 echoscu");
    assert!(echo.status.success(), "加了 C-FIND 后 C-ECHO 仍须可用");

    let (study, series, sop) = (unique_uid(), unique_uid(), unique_uid());
    push_instance(port, &study, &series, &sop);
    let responses = findscu(
        port,
        "-S",
        &[
            "QueryRetrieveLevel=STUDY",
            &format!("StudyInstanceUID={study}"),
        ],
    );
    assert_eq!(responses.len(), 1);
}
