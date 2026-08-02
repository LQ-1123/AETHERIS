//! pacsd:PACS 服务端主程序。

mod admin;
mod config;
mod store_handler;
mod tls;

use std::sync::Arc;

use anyhow::{Context, Result};
use axum::Router;
use clap::{Parser, Subcommand};

use crate::config::Config;
use crate::store_handler::PacsStoreHandler;

#[derive(Parser)]
#[command(name = "pacsd", version = pacs_core::VERSION)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// 创建管理员账号(账号体系的引导入口)
    Admin {
        /// 用户名(小写字母、数字、. _ -,至少 3 个字符)
        #[arg(short, long)]
        username: String,
        /// 密码(至少 12 个字符)
        #[arg(short, long)]
        password: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    let config = Config::from_env()?;

    let pool = pacs_db::connect(&config.database_url)
        .await
        .context("连接数据库失败")?;
    pacs_db::migrate(&pool)
        .await
        .context("应用数据库迁移失败")?;

    match cli.command {
        Some(Command::Admin { username, password }) => {
            admin::create_admin(&pool, &username, &password).await?;
            return Ok(());
        }
        None => {
            // 无子命令:启动服务
        }
    }

    for addr in config.binds_beyond_loopback() {
        tracing::warn!(
            %addr,
            "监听地址超出回环。DIMSE 和 HTTP 现已启用认证,但请注意网络安全"
        );
    }

    let store = pacs_store::Store::open(&config.storage_root)
        .await
        .context("打开影像存储根失败")?;
    let removed = store.cleanup_temp().await.context("清理临时文件失败")?;

    // TLS 证书
    let tls_certs = tls::TlsCerts::ensure(&config.storage_root)
        .await
        .context("TLS 证书初始化失败")?;

    // 认证服务
    // 变量名跟随 `PACS_` 前缀的统一约定(见 .env.example)。
    // 名字和配置文件对不上会让配好的密钥被静默忽略 —— debug 构建悄悄回退到
    // 开发默认值,release 构建则在用户明明配了变量的情况下 panic。
    let jwt_secret = std::env::var("PACS_JWT_SECRET").unwrap_or_else(|_| {
        let default = "dev-secret-DO-NOT-USE-IN-PRODUCTION";
        if cfg!(debug_assertions) {
            tracing::warn!(
                "PACS_JWT_SECRET 未设置，使用开发默认值（生产环境请用 `openssl rand -base64 48` 生成）"
            );
            default.to_string()
        } else {
            panic!("生产构建必须设置 PACS_JWT_SECRET 环境变量");
        }
    });
    let auth_service = Arc::new(
        pacs_auth::AuthService::new(pool.clone(), jwt_secret.as_bytes())
            .context("认证服务初始化失败")?,
    );

    // HTTP 路由。/auth 不鉴权(登录本身就是取令牌的入口),
    // /dicomweb 整棵子树要求 ViewImages。
    let http_app = Router::new()
        .nest("/auth", pacs_auth::http::routes(auth_service.clone()))
        .nest(
            "/dicomweb",
            pacs_web::dicomweb_routes(
                // Store 里只有一个 PathBuf,克隆便宜;包进 Arc 是让 WADO 的
                // handler 和 DIMSE 的 store handler 共用同一份存储根配置
                pacs_web::WebState::with_store(pool.clone(), Arc::new(store.clone())),
                auth_service.clone(),
            ),
        )
        .fallback(|| async { axum::http::StatusCode::NOT_FOUND });

    // DIMSE 监听
    let dimse = pacs_dimse::DimseServer::bind(pacs_dimse::ServerConfig::new(
        config.dimse_bind,
        &config.ae_title,
    ))
    .await
    .with_context(|| format!("DIMSE 无法监听 {}", config.dimse_bind))?;

    let dimse_handler = Arc::new(PacsStoreHandler::new(store, pool));

    tracing::info!(
        version = pacs_core::VERSION,
        storage_root = %config.storage_root.display(),
        ae_title = %config.ae_title,
        dimse_bind = %config.dimse_bind,
        http_bind = %config.http_bind,
        recovered_temp_files = removed,
        ca_cert = %tls_certs.ca_cert_path.display(),
        "pacsd 就绪（HTTPS 已启用）"
    );

    // 同时运行 DIMSE 和 HTTPS
    let dimse_task = tokio::spawn(async move {
        if let Err(e) = dimse.run(dimse_handler).await {
            tracing::error!(error = ?e, "DIMSE 服务异常退出");
        }
    });

    let https_task = tokio::spawn(async move {
        let tls_config = axum_server::tls_rustls::RustlsConfig::from_pem_file(
            &tls_certs.server_cert_path,
            &tls_certs.server_key_path,
        )
        .await
        .context("加载 TLS 证书失败")
        .unwrap();

        axum_server::bind_rustls(config.http_bind, tls_config)
            .serve(http_app.into_make_service())
            .await
            .context("HTTPS 服务异常退出")
            .unwrap();
    });

    tokio::select! {
        _ = dimse_task => {},
        _ = https_task => {},
    }

    Ok(())
}
