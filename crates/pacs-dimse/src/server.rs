//! DIMSE 监听器。

use std::net::SocketAddr;
use std::sync::Arc;

use dicom_ul::association::Association;
use dicom_ul::association::server::ServerAssociationOptions;
use thiserror::Error;
use tokio::net::{TcpListener, TcpStream};

use crate::find::FindHandler;
use crate::message::DEFAULT_MAX_DATASET_BYTES;
use crate::scp::{self, StoreHandler};
use crate::sop_class;

/// DIMSE 服务端配置。
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub bind: SocketAddr,
    /// 本端 AE Title。发送方的 Called AE Title 必须与之相符。
    pub ae_title: String,
    /// 单个数据集的字节上限,见 [`DEFAULT_MAX_DATASET_BYTES`]。
    pub max_dataset_bytes: usize,
    /// 单个 PDU 的最大长度。
    pub max_pdu_length: u32,
}

impl ServerConfig {
    pub fn new(bind: SocketAddr, ae_title: impl Into<String>) -> Self {
        Self {
            bind,
            ae_title: ae_title.into(),
            max_dataset_bytes: DEFAULT_MAX_DATASET_BYTES,
            // 16 KiB 是常见取值,大到不会让分帧开销明显,小到单连接内存可控
            max_pdu_length: 16_384,
        }
    }
}

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("无法监听 {bind}")]
    Bind {
        bind: SocketAddr,
        #[source]
        source: std::io::Error,
    },
    #[error("接受连接失败")]
    Accept {
        #[source]
        source: std::io::Error,
    },
}

/// 已经绑定好端口的 DIMSE 服务端。
///
/// 先绑定再运行,分两步是为了让调用方(和测试)能拿到实际端口 ——
/// 配 `:0` 让系统分配端口时,这是唯一的获知途径。
pub struct DimseServer {
    listener: TcpListener,
    config: ServerConfig,
}

impl DimseServer {
    pub async fn bind(config: ServerConfig) -> Result<Self, ServerError> {
        let listener =
            TcpListener::bind(config.bind)
                .await
                .map_err(|source| ServerError::Bind {
                    bind: config.bind,
                    source,
                })?;
        Ok(Self { listener, config })
    }

    /// 实际监听地址。配置里写 `:0` 时用它拿到系统分配的端口。
    pub fn local_addr(&self) -> Result<SocketAddr, std::io::Error> {
        self.listener.local_addr()
    }

    /// 接受连接并一直服务下去。每个 association 一个任务。
    pub async fn run<H>(self, handler: Arc<H>) -> Result<(), ServerError>
    where
        H: StoreHandler + FindHandler + 'static,
    {
        tracing::info!(
            bind = %self.config.bind,
            ae_title = %self.config.ae_title,
            "DIMSE 开始监听"
        );

        loop {
            let (socket, peer) = self
                .listener
                .accept()
                .await
                .map_err(|source| ServerError::Accept { source })?;

            let config = self.config.clone();
            let handler = Arc::clone(&handler);
            // 一个连接出问题不能影响其他连接,各自独立成任务
            tokio::spawn(async move {
                if let Err(error) = serve_connection(socket, peer, &config, handler.as_ref()).await
                {
                    tracing::warn!(%peer, %error, "association 异常结束");
                }
            });
        }
    }
}

async fn serve_connection<H>(
    socket: TcpStream,
    peer: SocketAddr,
    config: &ServerConfig,
    handler: &H,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    H: StoreHandler + FindHandler,
{
    let mut options = ServerAssociationOptions::new()
        // Called AE Title 必须匹配。这不是认证(AE Title 可以随便填),
        // 只是避免把发错地方的影像收下来。真正的访问控制在阶段 3/8。
        .accept_called_ae_title()
        .ae_title(config.ae_title.clone())
        .max_pdu_length(config.max_pdu_length)
        .with_abstract_syntax(sop_class::VERIFICATION);

    for uid in sop_class::STORAGE {
        options = options.with_abstract_syntax(*uid);
    }
    for uid in sop_class::FIND {
        options = options.with_abstract_syntax(*uid);
    }
    for uid in sop_class::TRANSFER_SYNTAXES {
        options = options.with_transfer_syntax(*uid);
    }

    let association = options.establish_async(socket).await?;
    let observed = scp::IncomingAssociation {
        calling_ae_title: association.peer_ae_title().trim().to_owned(),
        remote_addr: peer,
    };
    handler.association_opened(&observed).await;
    let result = scp::serve(association, handler, config.max_dataset_bytes, peer).await;
    handler.association_closed(&observed).await;
    result?;
    Ok(())
}
