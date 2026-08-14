//! pacsd 启动路径的测试:真的把二进制跑起来。
//!
//! 主程序把配置、数据库迁移、存储初始化和崩溃恢复串在一起,各部件都有自己的
//! 单测,但"串起来能不能起得来"只有真跑一次才知道 —— 少一个环境变量、
//! 迁移路径写错,单测都发现不了。

use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// 借系统分配一个空闲端口,随即释放让服务端去绑(与 dimse_interop 一致)。
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("应能取到空闲端口")
        .local_addr()
        .expect("应能读出端口")
        .port()
}

/// 正常配置下应当启动成功,并打印就绪日志。
///
/// pacsd 是常驻服务,不会自己退出 —— 不能像普通工具那样等它结束,
/// 而是边读日志边等"就绪"出现,然后主动停止。
#[test]
fn starts_up_against_a_real_database() {
    let Ok(database_url) = std::env::var("PACS_TEST_DATABASE_URL") else {
        // 本地缺配置时跳过是方便;CI 里跳过就等于没测,必须直接失败。
        assert!(
            std::env::var_os("CI").is_none(),
            "CI 环境必须设置 PACS_TEST_DATABASE_URL,启动测试不允许跳过"
        );
        eprintln!("
>>> 跳过 pacsd 启动测试:未设置 PACS_TEST_DATABASE_URL。
");
        return;
    };
    let storage = tempfile::tempdir().expect("应能建临时目录");

    // 用空闲端口,避免开发机上 11112/8443 被别的服务占用
    let dimse_port = free_port();
    let http_port = free_port();

    let mut child = Command::new(env!("CARGO_BIN_EXE_pacsd"))
        .env("DATABASE_URL", &database_url)
        .env("PACS_STORAGE_ROOT", storage.path())
        .env("PACS_DIMSE_BIND", format!("127.0.0.1:{dimse_port}"))
        .env("PACS_HTTP_BIND", format!("127.0.0.1:{http_port}"))
        .env("RUST_LOG", "info")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("应能启动 pacsd");

    // 后台线程把 stdout 逐行送进通道;主循环轮询:就绪日志出现即成功,
    // 提前退出即失败,超时则停止并报错。
    // 注意:tracing_subscriber::fmt 默认写 stdout(实测确认),不是 stderr。
    let stdout = child.stdout.take().expect("stdout 已接管");
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if tx.send(line.clone()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut log = String::new();
    loop {
        if let Some(status) = child.try_wait().expect("应能查询子进程状态") {
            panic!("pacsd 应常驻运行,却提前退出({status}),输出:
{log}");
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("pacsd 在 30 秒内未打印就绪日志,已停止。输出:
{log}");
        }
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                log.push_str(&line);
                if line.contains("pacsd 就绪") {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => continue,
        }
    }

    // 服务已确认就绪:停止它,收尾断言
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        log.contains("pacsd 就绪"),
        "应打印就绪日志,实际:
{log}"
    );
    // 存储根应当已经初始化好,包括临时目录
    assert!(storage.path().join(".tmp").is_dir(), "应建好 .tmp 目录");
}

/// 缺少必需配置时要明确报错,不能用默认值糊过去 ——
/// 悄悄连上一个没预期的数据库比起不来更糟。
#[test]
fn missing_database_url_fails_with_a_clear_message() {
    // 换到临时目录跑,免得向上找到仓库里的 .env 把变量补上
    let elsewhere = tempfile::tempdir().expect("应能建临时目录");

    let output = Command::new(env!("CARGO_BIN_EXE_pacsd"))
        .current_dir(elsewhere.path())
        .env_remove("DATABASE_URL")
        .output()
        .expect("应能启动 pacsd");

    assert!(!output.status.success(), "缺配置时不该成功退出");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("DATABASE_URL"),
        "报错应指出缺的是哪个变量,实际:
{stderr}"
    );
}
