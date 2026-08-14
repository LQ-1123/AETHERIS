//! 本地完整栈：内嵌 PostgreSQL + pacsd，让 Viewer 在无任何外部环境的电脑上双击即用。
//!
//! 打包布局（app 的 Contents/Resources/local-stack/）：
//!   pacsd                    服务端二进制（macOS 编译）
//!   pgsql/bin|lib|share      PostgreSQL 14（依赖库已改相对路径、编译路径已打补丁）
//!
//! 首启把 local-stack 复制到数据目录（避免 app translocation 干扰），并按
//! postgres 的路径解析规则建软链：
//!   <data>/local-stack/pgsql          postgres 本体
//!   <data>/local-stack/pgdata         PostgreSQL 数据目录
//!   <data>/local-stack/lib  -> pgsql/lib     （postgres 按 cwd=pgdata 解析 ../lib）
//!   <data>/local-stack/share -> pgsql/share  （initdb 按 cwd=pgsql/bin 解析 ../share）

use serde::Serialize;
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};

/// 本地栈监听端口：避开 docker 方案的 8443/11112 与本机其他服务。
const PG_PORT: &str = "55432";
const HTTP_PORT: &str = "18443";
const DIMSE_PORT: &str = "11113";
const ADMIN_USER: &str = "admin";

#[derive(Debug, Clone, Serialize)]
pub struct LocalModeInfo {
    pub server_url: String,
    pub ca_cert_path: String,
    pub username: String,
    pub password: String,
}

pub struct LocalStack {
    data_dir: PathBuf,
    resource_stack: PathBuf, // <app>/Contents/Resources/local-stack
    pacsd_child: Mutex<Option<Child>>,
}

impl LocalStack {
    pub fn new(app: &AppHandle) -> Self {
        let data_dir = app
            .path()
            .app_data_dir()
            .unwrap_or_else(|_| std::env::temp_dir().join("aetheris-local"));
        let resource_stack = app
            .path()
            .resource_dir()
            .unwrap_or_else(|_| PathBuf::from("/nonexistent"))
            .join("local-stack");
        Self {
            data_dir,
            resource_stack,
            pacsd_child: Mutex::new(None),
        }
    }

    fn stack_dir(&self) -> PathBuf {
        self.data_dir.join("local-stack")
    }
    fn pgsql(&self) -> PathBuf {
        self.stack_dir().join("postgres")
    }
    fn pg_bin(&self) -> PathBuf {
        self.pgsql().join("bin")
    }
    fn pg_data(&self) -> PathBuf {
        self.stack_dir().join("pgdata")
    }
    fn logs_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }
    fn storage(&self) -> PathBuf {
        self.data_dir.join("storage")
    }
    fn admin_file(&self) -> PathBuf {
        self.data_dir.join("local-admin.env")
    }

    /// 启动本地完整栈。非打包版（没有 local-stack 资源）时返回 Ok(None)。
    pub fn ensure(&self) -> Result<Option<LocalModeInfo>, String> {
        if !self.resource_stack.join("pacsd").exists() {
            return Ok(None);
        }
        fs::create_dir_all(self.logs_dir()).map_err(|e| format!("创建日志目录失败: {e}"))?;
        fs::create_dir_all(self.storage()).map_err(|e| format!("创建存储目录失败: {e}"))?;
        self.prepare_stack()?;

        self.ensure_postgres()?;
        self.ensure_role_and_database()?;

        let secrets = self.ensure_secrets()?;
        let password = &secrets.0;
        let jwt = &secrets.1;

        if !port_open(HTTP_PORT) {
            self.create_admin(password)?;
            self.spawn_pacsd(password, jwt)?;
            wait_for_port(HTTP_PORT, Duration::from_secs(40))
                .map_err(|e| format!("pacsd 启动超时: {e}"))?;
        }

        let ca = self.storage().join("tls/ca.crt");
        if !ca.exists() {
            return Err("TLS 证书尚未生成（pacsd 未成功启动？）".into());
        }
        Ok(Some(LocalModeInfo {
            server_url: format!("https://127.0.0.1:{HTTP_PORT}"),
            ca_cert_path: ca.display().to_string(),
            username: ADMIN_USER.into(),
            password: password.clone(),
        }))
    }

    /// 退出时停止 pacsd 与 PostgreSQL（不删数据）。
    pub fn shutdown(&self) {
        if let Some(mut child) = self.pacsd_child.lock().ok().and_then(|mut g| g.take()) {
            let _ = child.kill();
            let _ = child.wait();
        }
        if self.pg_data().join("PG_VERSION").exists() {
            let _ = Command::new(self.pg_bin().join("pg_ctl"))
                .current_dir(&self.pg_bin())
                .args(["stop", "-m", "fast", "-D"])
                .arg(&self.pg_data())
                .status();
        }
    }

    // ---- 内部步骤 ----

    /// 把打包的 local-stack 复制到数据目录并建软链（幂等）。
    fn prepare_stack(&self) -> Result<(), String> {
        let stack = self.stack_dir();
        if !stack.join("pgsql").exists() {
            let parent = stack.parent().unwrap_or(&self.data_dir);
            fs::create_dir_all(parent).map_err(|e| format!("创建数据目录失败: {e}"))?;
            let status = Command::new("cp")
                .arg("-R")
                .arg(&self.resource_stack)
                .arg(parent)
                .status()
                .map_err(|e| format!("无法复制本地栈: {e}"))?;
            if !status.success() {
                return Err(format!("复制本地栈失败（退出码 {}）", status.code().unwrap_or(-1)));
            }
        }
        // 软链：postgres 不同进程按不同基准解析 ../lib 与 ../share（仅 macOS 布局需要；
        // Windows 侧由 aetheris-launcher 管理服务，viewer 内不内嵌栈）
        #[cfg(unix)]
        {
            let lib_link = stack.join("lib");
            if !lib_link.exists() {
                let _ = std::os::unix::fs::symlink("postgres/lib", &lib_link);
            }
            let share_link = stack.join("share");
            if !share_link.exists() {
                let _ = std::os::unix::fs::symlink("postgres/share", &share_link);
            }
        }
        Ok(())
    }

    fn ensure_postgres(&self) -> Result<(), String> {
        let pg_data = self.pg_data();
        let pg_bin = self.pg_bin();
        if !pg_data.join("PG_VERSION").exists() {
            fs::create_dir_all(&pg_data).map_err(|e| format!("创建数据目录失败: {e}"))?;
            tracing::info!("初始化本地 PostgreSQL 数据目录…");
            let status = Command::new(pg_bin.join("initdb"))
                .current_dir(&pg_bin)
                .args([
                    "-D",
                    pg_data.to_str().unwrap_or_default(),
                    "-U",
                    "pacs",
                    "--auth=trust",
                    "-E",
                    "UTF8",
                    "--locale=C",
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map_err(|e| format!("无法执行 initdb: {e}"))?;
            if !status.success() {
                return Err(format!("initdb 失败（退出码 {}）", status.code().unwrap_or(-1)));
            }
        }
        if !port_open(PG_PORT) {
            let log = self.logs_dir().join("postgres.log");
            let status = Command::new(pg_bin.join("pg_ctl"))
                .current_dir(&pg_bin)
                .args(["start", "-w", "-D"])
                .arg(&pg_data)
                .arg("-l")
                .arg(&log)
                .arg("-o")
                .arg(format!("-p {PG_PORT} -h 127.0.0.1"))
                .status()
                .map_err(|e| format!("无法启动 PostgreSQL: {e}"))?;
            if !status.success() {
                return Err(format!(
                    "PostgreSQL 启动失败（退出码 {}），日志: {}",
                    status.code().unwrap_or(-1),
                    log.display()
                ));
            }
            wait_for_port(PG_PORT, Duration::from_secs(20))
                .map_err(|e| format!("PostgreSQL 未就绪: {e}"))?;
        }
        Ok(())
    }

    fn ensure_role_and_database(&self) -> Result<(), String> {
        let pg_bin = self.pg_bin();
        let psql = pg_bin.join("psql");
        let check = Command::new(&psql)
            .current_dir(&pg_bin)
            .args([
                "-h", "127.0.0.1", "-p", PG_PORT, "-U", "pacs", "-d", "postgres",
                "-tAc", "SELECT 1 FROM pg_database WHERE datname='pacs'",
            ])
            .output()
            .map_err(|e| format!("查询数据库失败: {e}"))?;
        if String::from_utf8_lossy(&check.stdout).trim().is_empty() {
            tracing::info!("创建本地 pacs 数据库…");
            let status = Command::new(pg_bin.join("createdb"))
                .current_dir(&pg_bin)
                .args(["-h", "127.0.0.1", "-p", PG_PORT, "-U", "pacs", "pacs"])
                .status()
                .map_err(|e| format!("无法创建数据库: {e}"))?;
            if !status.success() {
                return Err(format!("创建 pacs 数据库失败（退出码 {}）", status.code().unwrap_or(-1)));
            }
        }
        Ok(())
    }

    /// 读取或生成管理员密码与 JWT 密钥（持久化到 local-admin.env）。
    fn ensure_secrets(&self) -> Result<(String, String), String> {
        let file = self.admin_file();
        let existing = fs::read_to_string(&file).unwrap_or_default();
        let mut password = String::new();
        let mut jwt = String::new();
        for line in existing.lines() {
            if let Some((k, v)) = line.split_once('=') {
                if k.trim() == "PACS_ADMIN_PASSWORD" {
                    password = v.trim().to_string();
                } else if k.trim() == "PACS_JWT_SECRET" {
                    jwt = v.trim().to_string();
                }
            }
        }
        if password.len() < 12 {
            password = random_password();
        }
        if jwt.len() < 32 {
            jwt = random_hex(48);
        }
        let _ = fs::write(&file, format!("PACS_ADMIN_PASSWORD={password}\nPACS_JWT_SECRET={jwt}\n"));
        Ok((password, jwt))
    }

    fn create_admin(&self, password: &str) -> Result<(), String> {
        let output = Command::new(self.stack_dir().join("pacsd"))
            .current_dir(&self.stack_dir())
            .args(["admin", "--username", ADMIN_USER, "--password", password])
            .env("DATABASE_URL", format!("postgres://pacs@127.0.0.1:{PG_PORT}/pacs"))
            .output()
            .map_err(|e| format!("无法执行 pacsd admin: {e}"))?;
        let text = String::from_utf8_lossy(&output.stdout).to_string()
            + &String::from_utf8_lossy(&output.stderr);
        if !output.status.success() && !text.contains("已存在") {
            return Err(format!("创建管理员失败: {text}"));
        }
        Ok(())
    }

    fn spawn_pacsd(&self, password: &str, jwt: &str) -> Result<(), String> {
        let log = self.logs_dir().join("pacsd.log");
        let log_file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
            .map_err(|e| format!("无法打开 pacsd 日志: {e}"))?;
        let child = Command::new(self.stack_dir().join("pacsd"))
            .current_dir(&self.stack_dir())
            .env("DATABASE_URL", format!("postgres://pacs@127.0.0.1:{PG_PORT}/pacs"))
            .env("PACS_STORAGE_ROOT", self.storage())
            .env("PACS_HTTP_BIND", format!("127.0.0.1:{HTTP_PORT}"))
            .env("PACS_DIMSE_BIND", format!("127.0.0.1:{DIMSE_PORT}"))
            .env("PACS_AE_TITLE", "AETHERIS_LOCAL")
            .env("PACS_JWT_SECRET", jwt)
            .env("RUST_LOG", "info,pacsd=debug")
            .stdout(Stdio::from(log_file.try_clone().map_err(|e| e.to_string())?))
            .stderr(Stdio::from(log_file))
            .spawn()
            .map_err(|e| format!("无法启动 pacsd: {e}"))?;
        if let Ok(mut guard) = self.pacsd_child.lock() {
            *guard = Some(child);
        }
        tracing::info!(password = %password, "本地 pacsd 已启动");
        Ok(())
    }
}

fn port_open(port: &str) -> bool {
    std::net::TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}").parse().expect("合法端口"),
        Duration::from_millis(250),
    )
    .is_ok()
}

fn wait_for_port(port: &str, timeout: Duration) -> Result<(), String> {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if port_open(port) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Err(format!("端口 {port} 在 {} 秒内未就绪", timeout.as_secs()))
}

/// 生成不包含 "admin"（密码规则：不能包含用户名）的 20 位随机字母数字密码。
fn random_password() -> String {
    loop {
        let bytes = random_bytes(20);
        let pw: String = bytes
            .iter()
            .map(|b| {
                const CHARS: &[u8] = b"abcdefghijkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
                CHARS[(*b as usize) % CHARS.len()] as char
            })
            .collect();
        if !pw.to_lowercase().contains("admin") {
            return pw;
        }
    }
}

fn random_hex(len: usize) -> String {
    random_bytes(len)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn random_bytes(len: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; len];
    if let Ok(mut f) = fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut bytes);
    }
    bytes
}
