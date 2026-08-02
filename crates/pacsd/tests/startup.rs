//! pacsd 启动路径的测试:真的把二进制跑起来。
//!
//! 主程序把配置、数据库迁移、存储初始化和崩溃恢复串在一起,各部件都有自己的
//! 单测,但"串起来能不能起得来"只有真跑一次才知道 —— 少一个环境变量、
//! 迁移路径写错,单测都发现不了。

use std::process::Command;

/// 正常配置下应当启动成功,并打印就绪日志。
#[test]
fn starts_up_against_a_real_database() {
    let Ok(database_url) = std::env::var("PACS_TEST_DATABASE_URL") else {
        // 本地缺配置时跳过是方便;CI 里跳过就等于没测,必须直接失败。
        assert!(
            std::env::var_os("CI").is_none(),
            "CI 环境必须设置 PACS_TEST_DATABASE_URL,启动测试不允许跳过"
        );
        eprintln!("\n>>> 跳过 pacsd 启动测试:未设置 PACS_TEST_DATABASE_URL。\n");
        return;
    };
    let storage = tempfile::tempdir().expect("应能建临时目录");

    let output = Command::new(env!("CARGO_BIN_EXE_pacsd"))
        .env("DATABASE_URL", &database_url)
        .env("PACS_STORAGE_ROOT", storage.path())
        .env("RUST_LOG", "info")
        .output()
        .expect("应能启动 pacsd");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "pacsd 应正常退出,实际:{}\n{stderr}",
        output.status
    );
    assert!(
        stderr.contains("pacsd 就绪"),
        "应打印就绪日志,实际:\n{stderr}"
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
        "报错应指出缺的是哪个变量,实际:\n{stderr}"
    );
}
