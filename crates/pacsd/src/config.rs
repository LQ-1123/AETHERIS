//! pacsd 的运行配置,全部来自环境变量(`.env` 或进程环境)。

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct Config {
    /// 服务端独占的数据库连接串。客户端拿不到也用不上它。
    pub database_url: String,
    pub storage_root: PathBuf,
    pub dimse_bind: SocketAddr,
    pub ae_title: String,
    pub http_bind: SocketAddr,
}

impl Config {
    /// 从环境变量读取。缺少必填项直接报错,不用默认值糊过去。
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            database_url: required("DATABASE_URL")?,
            storage_root: PathBuf::from(optional("PACS_STORAGE_ROOT", "./data/storage")),
            dimse_bind: parse_bind("PACS_DIMSE_BIND", "127.0.0.1:11112")?,
            ae_title: optional("PACS_AE_TITLE", "REMOTE_PACS"),
            http_bind: parse_bind("PACS_HTTP_BIND", "127.0.0.1:8443")?,
        })
    }

    /// 监听地址是否超出了本机回环。
    ///
    /// DIMSE 协议本身没有认证(AE Title 可以随便填),DICOMweb 在账号体系完成前
    /// 也没有。绑到回环以外就等于把影像库暴露给整个网络,所以要在启动日志里
    /// 显著告警,而不是让它悄悄发生。
    pub fn binds_beyond_loopback(&self) -> Vec<SocketAddr> {
        [self.dimse_bind, self.http_bind]
            .into_iter()
            .filter(|addr| !addr.ip().is_loopback())
            .collect()
    }
}

fn required(key: &str) -> Result<String> {
    std::env::var(key).with_context(|| format!("缺少必需的环境变量 {key},请参照 .env.example 配置"))
}

fn optional(key: &str, fallback: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| fallback.to_owned())
}

fn parse_bind(key: &str, fallback: &str) -> Result<SocketAddr> {
    let raw = optional(key, fallback);
    raw.parse()
        .with_context(|| format!("{key} 的值 {raw:?} 不是合法的监听地址"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(dimse: &str, http: &str) -> Config {
        Config {
            database_url: "postgres://localhost/x".into(),
            storage_root: PathBuf::from("/tmp/x"),
            dimse_bind: dimse.parse().expect("测试地址应合法"),
            ae_title: "TEST".into(),
            http_bind: http.parse().expect("测试地址应合法"),
        }
    }

    #[test]
    fn loopback_binds_raise_no_warning() {
        assert!(
            config("127.0.0.1:11112", "127.0.0.1:8443")
                .binds_beyond_loopback()
                .is_empty()
        );
        assert!(
            config("[::1]:11112", "[::1]:8443")
                .binds_beyond_loopback()
                .is_empty()
        );
    }

    /// 绑到回环之外就等于把无认证的影像库暴露出去,一个都不能漏报。
    #[test]
    fn exposed_binds_are_all_reported() {
        let exposed = config("0.0.0.0:11112", "192.168.1.10:8443").binds_beyond_loopback();
        assert_eq!(exposed.len(), 2, "两个监听都超出回环,都要报");

        // 只有一个暴露时也不能漏
        let one = config("127.0.0.1:11112", "0.0.0.0:8443").binds_beyond_loopback();
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].port(), 8443);

        // IPv6 的 :: 等价于 0.0.0.0,同样是全网暴露
        assert_eq!(
            config("[::]:11112", "127.0.0.1:8443")
                .binds_beyond_loopback()
                .len(),
            1
        );
    }

    #[test]
    fn bad_bind_address_is_rejected() {
        assert!(parse_bind("PACS_TEST_UNSET_BIND", "not-an-address").is_err());
        // 少了端口也不行 —— 别默默用一个我们没打算用的端口
        assert!(parse_bind("PACS_TEST_UNSET_BIND", "127.0.0.1").is_err());
        assert!(parse_bind("PACS_TEST_UNSET_BIND", "127.0.0.1:11112").is_ok());
    }
}
