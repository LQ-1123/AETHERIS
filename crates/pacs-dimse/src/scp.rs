//! C-ECHO、C-STORE 与 C-FIND 的服务提供方(SCP)。

use std::future::Future;

use dicom::encoding::TransferSyntaxIndex;
use dicom::object::{FileMetaTableBuilder, InMemDicomObject};
use dicom::transfer_syntax::TransferSyntaxRegistry;
use dicom_ul::association::Association;
use dicom_ul::association::server::AsyncServerAssociation;
use pacs_core::InstanceMetadata;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::command::{self, CommandField, Status};
use crate::find::{self, FindHandler};
use crate::message::{self, DimseMessage, Ended, MessageError};

/// 一份收到的影像。
#[derive(Debug)]
pub struct IncomingInstance<'a> {
    /// 从数据集解析出的四层元数据。
    pub metadata: &'a InstanceMetadata,
    /// 完整的 Part-10 文件字节:前导 + `DICM` + 文件元信息 + 数据集。
    ///
    /// 数据集部分是**发送方原样送来的字节**,没有解码再重编码。存下来的文件
    /// 因此与发送端逐字节一致 —— 影像资料的保真性不该被我们的编码器改写。
    pub file_bytes: &'a [u8],
    /// 发送方的 AE Title。协议本身不认证,这个值可以伪造,只作审计线索。
    pub calling_ae_title: &'a str,
}

/// 落盘入库失败的原因,决定回给发送方的状态码。
#[derive(Debug, Error)]
pub enum StoreFailure {
    #[error("存储资源不足:{0}")]
    OutOfResources(String),
    #[error("处理失败:{0}")]
    Processing(String),
}

impl StoreFailure {
    fn status(&self) -> Status {
        match self {
            // 资源不足是可恢复的,发送方应当稍后重试;
            // 处理失败通常不可恢复,重试也是白搭。这个区分对发送方的重试策略有意义。
            Self::OutOfResources(_) => Status::REFUSED_OUT_OF_RESOURCES,
            Self::Processing(_) => Status::PROCESSING_FAILURE,
        }
    }
}

/// 收到影像后做什么。由 `pacsd` 实现:落盘 + 入库。
pub trait StoreHandler: Send + Sync {
    fn store(
        &self,
        instance: IncomingInstance<'_>,
    ) -> impl Future<Output = Result<(), StoreFailure>> + Send;
}

/// 处理一次 association 上的全部消息,直到对端释放或中止。
pub async fn serve<S, H>(
    mut association: AsyncServerAssociation<S>,
    handler: &H,
    max_dataset_bytes: usize,
) -> Result<(), MessageError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
    H: StoreHandler + FindHandler,
{
    let calling_ae_title = association.peer_ae_title().to_owned();
    tracing::info!(calling_ae_title, "association 已建立");

    loop {
        let message = match message::receive_message(&mut association, max_dataset_bytes).await? {
            Ok(message) => message,
            Err(Ended::Released) => {
                tracing::info!(calling_ae_title, "association 正常释放");
                return Ok(());
            }
            Err(Ended::Aborted) => return Ok(()),
        };

        let context_id = message.presentation_context_id;
        let response = match message.command.command_field() {
            Ok(CommandField::CEchoRq) => {
                tracing::debug!(calling_ae_title, "C-ECHO-RQ");
                command::c_echo_rsp(&message.command, Status::SUCCESS)
            }
            Ok(CommandField::CStoreRq) => {
                let status = handle_store(&association, &message, handler, &calling_ae_title).await;
                command::c_store_rsp(&message.command, status)
            }
            Ok(CommandField::CFindRq) => {
                // C-FIND 自己负责把整串响应发完(见 `find` 模块的响应序列说明),
                // 不像其他服务那样由这里统一发一条响应
                find::handle_find(
                    &mut association,
                    &message.command,
                    message.dataset.as_deref(),
                    context_id,
                    handler,
                    &calling_ae_title,
                )
                .await?;
                continue;
            }
            Ok(CommandField::CCancelRq) => {
                // 取消请求只会在某个操作进行中才有意义。能读到它说明那个操作
                // 已经收尾了(见 `find` 模块关于 C-CANCEL 支持程度的说明)——
                // 忽略即可,绝不能中止 association:那会把对端后续的查询一起打断。
                tracing::debug!(calling_ae_title, "收到已无对应操作的 C-CANCEL,忽略");
                continue;
            }
            Ok(other) => {
                // 走到这里说明协商时接受了我们还没实现的服务。中止而不是沉默:
                // 让对端立刻知道,好过让它等一个永远不来的响应。
                tracing::warn!(
                    ?other,
                    calling_ae_title,
                    "收到尚未实现的 DIMSE 服务,中止 association"
                );
                association.abort().await?;
                return Ok(());
            }
            Err(error) => {
                tracing::warn!(%error, calling_ae_title, "命令集无法识别,中止 association");
                association.abort().await?;
                return Ok(());
            }
        };

        message::send_command(&mut association, context_id, &response).await?;
    }
}

/// 处理一条 C-STORE-RQ,返回要回给发送方的状态。
///
/// 这里不返回 `Result`:无论成功失败都必须回一条 C-STORE-RSP,把错误变成状态码
/// 而不是让它冒泡中断 association —— 一份影像存不下不该拖垮后面几百份。
async fn handle_store<S, H>(
    association: &AsyncServerAssociation<S>,
    message: &DimseMessage,
    handler: &H,
    calling_ae_title: &str,
) -> Status
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
    H: StoreHandler,
{
    let Some(dataset_bytes) = message.dataset.as_deref() else {
        tracing::warn!(calling_ae_title, "C-STORE-RQ 没有数据集");
        return Status::CANNOT_UNDERSTAND;
    };

    // 数据集用的是该表示上下文协商出的传输语法,不是命令集那个隐式 VR LE
    let Some(transfer_syntax_uid) =
        negotiated_transfer_syntax(association, message.presentation_context_id)
    else {
        tracing::error!(
            context_id = message.presentation_context_id,
            "找不到该表示上下文的协商结果"
        );
        return Status::PROCESSING_FAILURE;
    };
    let Some(transfer_syntax) = TransferSyntaxRegistry.get(&transfer_syntax_uid) else {
        tracing::error!(%transfer_syntax_uid, "协商出的传输语法本地不支持");
        return Status::PROCESSING_FAILURE;
    };

    let object = match InMemDicomObject::read_dataset_with_ts(dataset_bytes, transfer_syntax) {
        Ok(object) => object,
        Err(error) => {
            tracing::warn!(%error, calling_ae_title, "数据集解析失败");
            return Status::CANNOT_UNDERSTAND;
        }
    };

    let meta = match FileMetaTableBuilder::new()
        .transfer_syntax(&transfer_syntax_uid)
        .media_storage_sop_class_uid(message.command.affected_sop_class_uid().unwrap_or_default())
        .media_storage_sop_instance_uid(
            message
                .command
                .affected_sop_instance_uid()
                .unwrap_or_default(),
        )
        .implementation_class_uid(dicom_ul::IMPLEMENTATION_CLASS_UID)
        .implementation_version_name(IMPLEMENTATION_VERSION_NAME)
        .build()
    {
        Ok(meta) => meta,
        Err(error) => {
            tracing::warn!(%error, calling_ae_title, "文件元信息构造失败");
            return Status::DATA_SET_DOES_NOT_MATCH_SOP_CLASS;
        }
    };

    // with_exact_meta 只是把元信息挂上去,不重新编码数据集
    let file_object = object.with_exact_meta(meta);
    let metadata = match pacs_core::extract_metadata(&file_object) {
        Ok(metadata) => metadata,
        Err(error) => {
            tracing::warn!(%error, calling_ae_title, "元数据提取失败");
            // UID 缺失或非法属于「参数值有问题」,不是我们处理不了
            return Status::INVALID_ARGUMENT_VALUE;
        }
    };

    let file_bytes = match part10_bytes(&file_object, dataset_bytes) {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::error!(%error, "文件元信息序列化失败");
            return Status::PROCESSING_FAILURE;
        }
    };

    match handler
        .store(IncomingInstance {
            metadata: &metadata,
            file_bytes: &file_bytes,
            calling_ae_title,
        })
        .await
    {
        Ok(()) => Status::SUCCESS,
        Err(failure) => {
            tracing::error!(
                %failure,
                calling_ae_title,
                sop_instance_uid = %metadata.instance.uid,
                "影像存储失败"
            );
            failure.status()
        }
    }
}

/// 拼出 Part-10 文件字节:前导 + `DICM` + 文件元信息 + 原始数据集。
///
/// 数据集直接拼接原始字节,不经过编码器 —— 存下来的和发送方送来的完全一致。
fn part10_bytes(
    file_object: &dicom::object::FileDicomObject<InMemDicomObject>,
    dataset_bytes: &[u8],
) -> Result<Vec<u8>, Box<dicom::object::WriteError>> {
    let mut bytes = Vec::with_capacity(dataset_bytes.len() + 1024);
    bytes.extend_from_slice(&[0_u8; 128]); // 前导
    bytes.extend_from_slice(b"DICM");
    file_object.write_meta(&mut bytes).map_err(Box::new)?;
    bytes.extend_from_slice(dataset_bytes);
    Ok(bytes)
}

fn negotiated_transfer_syntax<S>(
    association: &AsyncServerAssociation<S>,
    presentation_context_id: u8,
) -> Option<String> {
    association
        .presentation_contexts()
        .iter()
        .find(|context| context.id == presentation_context_id)
        .map(|context| context.transfer_syntax.clone())
}

const IMPLEMENTATION_VERSION_NAME: &str = "REMOTE_PACS_0.1";
