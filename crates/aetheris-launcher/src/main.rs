#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::env;
use std::fs::{self, OpenOptions};
use std::io;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let message = format!(
                "AETHERIS 启动失败：\n\n{error}\n\n详情请查看 ProgramData\\AETHERIS\\logs。\n"
            );
            eprintln!("{message}");
            let _ = show_error(&message);
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let install_dir = env::current_exe()
        .map_err(|e| format!("无法确定安装目录：{e}"))?
        .parent()
        .ok_or_else(|| "启动器路径没有父目录".to_owned())?
        .to_path_buf();
    let data_dir = program_data_dir()?.join("AETHERIS");
    let config_file = data_dir.join("server.env");
    if !config_file.is_file() {
        return Err("尚未完成初始化，请重新运行安装程序。".into());
    }
    let log_dir = data_dir.join("logs");
    fs::create_dir_all(&log_dir).map_err(|e| format!("无法创建日志目录：{e}"))?;

    let pg_bin = install_dir.join("postgres").join("bin");
    let pg_data = data_dir.join("postgres");
    if !port_open("127.0.0.1:55432") {
        let status = hidden_command(pg_bin.join("pg_ctl.exe"))
            .args(["start", "-w", "-D"])
            .arg(&pg_data)
            .arg("-l")
            .arg(log_dir.join("postgres.log"))
            .status()
            .map_err(|e| format!("无法启动内置 PostgreSQL：{e}"))?;
        if !status.success() {
            return Err(format!("内置 PostgreSQL 启动失败（退出码 {status}）"));
        }
    }

    if !port_open("127.0.0.1:8443") {
        let stdout = append_log(&log_dir.join("pacsd.log"))?;
        let stderr = stdout
            .try_clone()
            .map_err(|e| format!("无法打开服务日志：{e}"))?;
        hidden_command(install_dir.join("pacsd.exe"))
            .current_dir(&data_dir)
            .envs(read_env_file(&config_file)?)
            .stdout(stdout)
            .stderr(stderr)
            .spawn()
            .map_err(|e| format!("无法启动 PACS 服务：{e}"))?;
        wait_for_port("127.0.0.1:8443", Duration::from_secs(30))?;
    }

    Command::new(install_dir.join("AETHERIS.exe"))
        .spawn()
        .map_err(|e| format!("无法启动 Viewer：{e}"))?;
    Ok(())
}

fn program_data_dir() -> Result<PathBuf, String> {
    env::var_os("PROGRAMDATA")
        .map(PathBuf::from)
        .ok_or_else(|| "系统没有 PROGRAMDATA 环境变量".to_owned())
}

fn append_log(path: &Path) -> Result<fs::File, String> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("无法写入 {}：{e}", path.display()))
}

fn read_env_file(path: &Path) -> Result<Vec<(String, String)>, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("无法读取 {}：{e}", path.display()))?;
    text.lines()
        .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
        .map(|line| {
            line.split_once('=')
                .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        })
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| format!("{} 中存在无效配置行", path.display()))
}

fn port_open(address: &str) -> bool {
    let Ok(address) = address.parse::<SocketAddr>() else {
        return false;
    };
    TcpStream::connect_timeout(&address, Duration::from_millis(250)).is_ok()
}

fn wait_for_port(address: &str, timeout: Duration) -> Result<(), String> {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if port_open(address) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(250));
    }
    Err(format!("PACS 服务在 {} 秒内未就绪", timeout.as_secs()))
}

fn hidden_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    command.stdin(Stdio::null());
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

#[cfg(windows)]
fn show_error(message: &str) -> io::Result<()> {
    hidden_command("msg.exe")
        .args(["*", message])
        .status()
        .map(|_| ())
}

#[cfg(not(windows))]
fn show_error(_message: &str) -> io::Result<()> {
    Ok(())
}
