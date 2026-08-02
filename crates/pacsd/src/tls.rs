//! 自签 TLS 证书生成。
//!
//! 用于本地部署场景。生成的证书包括：
//! - CA 根证书（客户端需要信任这个）
//! - 服务端证书（由 CA 签发）
//!
//! 证书写入 `{storage_root}/tls/` 目录。

use anyhow::{Context, Result};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair, SanType,
};
use std::path::{Path, PathBuf};
use tokio::fs;

const CA_CERT_FILE: &str = "ca.crt";
const CA_KEY_FILE: &str = "ca.key";
const SERVER_CERT_FILE: &str = "server.crt";
const SERVER_KEY_FILE: &str = "server.key";

pub struct TlsCerts {
    pub ca_cert_path: PathBuf,
    pub server_cert_path: PathBuf,
    pub server_key_path: PathBuf,
}

impl TlsCerts {
    /// 检查证书是否存在，不存在则生成。
    pub async fn ensure(storage_root: &Path) -> Result<Self> {
        let tls_dir = storage_root.join("tls");
        fs::create_dir_all(&tls_dir)
            .await
            .with_context(|| format!("无法创建 TLS 目录 {}", tls_dir.display()))?;

        let ca_cert_path = tls_dir.join(CA_CERT_FILE);
        let ca_key_path = tls_dir.join(CA_KEY_FILE);
        let server_cert_path = tls_dir.join(SERVER_CERT_FILE);
        let server_key_path = tls_dir.join(SERVER_KEY_FILE);

        let all_exist = ca_cert_path.exists()
            && ca_key_path.exists()
            && server_cert_path.exists()
            && server_key_path.exists();

        if !all_exist {
            tracing::info!("生成自签 TLS 证书到 {}", tls_dir.display());
            generate_certs(
                &ca_cert_path,
                &ca_key_path,
                &server_cert_path,
                &server_key_path,
            )
            .await?;
        } else {
            tracing::info!("使用已有 TLS 证书 {}", tls_dir.display());
        }

        Ok(Self {
            ca_cert_path,
            server_cert_path,
            server_key_path,
        })
    }
}

async fn generate_certs(
    ca_cert_path: &Path,
    ca_key_path: &Path,
    server_cert_path: &Path,
    server_key_path: &Path,
) -> Result<()> {
    // 生成 CA 根证书
    let mut ca_params = CertificateParams::default();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let mut ca_dn = DistinguishedName::new();
    ca_dn.push(DnType::CommonName, "PACS Self-Signed CA");
    ca_dn.push(DnType::OrganizationName, "PACS");
    ca_params.distinguished_name = ca_dn;

    let ca_key = KeyPair::generate()?;
    let ca_cert = ca_params.self_signed(&ca_key)?;

    fs::write(ca_cert_path, ca_cert.pem())
        .await
        .context("写入 CA 证书失败")?;
    fs::write(ca_key_path, ca_key.serialize_pem())
        .await
        .context("写入 CA 私钥失败")?;

    // 生成服务端证书（由 CA 签发）
    let mut server_params = CertificateParams::default();
    let mut server_dn = DistinguishedName::new();
    server_dn.push(DnType::CommonName, "localhost");
    server_params.distinguished_name = server_dn;
    server_params.subject_alt_names = vec![
        SanType::DnsName("localhost".try_into()?),
        SanType::IpAddress(std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1))),
        SanType::IpAddress(std::net::IpAddr::V6(std::net::Ipv6Addr::new(
            0, 0, 0, 0, 0, 0, 0, 1,
        ))),
    ];

    let server_key = KeyPair::generate()?;
    let server_cert = server_params.signed_by(&server_key, &ca_cert, &ca_key)?;

    fs::write(server_cert_path, server_cert.pem())
        .await
        .context("写入服务端证书失败")?;
    fs::write(server_key_path, server_key.serialize_pem())
        .await
        .context("写入服务端私钥失败")?;

    tracing::info!("TLS 证书已生成");
    tracing::warn!("客户端需要信任 CA 证书: {}", ca_cert_path.display());

    Ok(())
}
