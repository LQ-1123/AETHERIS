//! DIMSE 消息的收发。
//!
//! 一条 DIMSE 消息由一个命令集、外加可选的数据集组成,两者都被切成
//! P-DATA 的 PDV 片段传输。这一层负责把片段拼回消息、把消息拆成片段。
//!
//! 不用 `dicom-ul` 的 `receive_pdata()` 而是直接在 `receive()` 上组装,
//! 原因是一个 P-DATA PDU 里**可以同时装命令 PDV 和数据 PDV**(标准允许),
//! 自己组装才能两种都不漏,也才能对数据集大小设上限。

use dicom_ul::association::server::AsyncServerAssociation;
use dicom_ul::pdu::{PDataValue, PDataValueType, Pdu};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::command::{Command, CommandError};

/// 单个数据集的字节上限。
///
/// 数据集要整个读进内存才能落盘和算校验和,没有上限的话,一个恶意或故障的
/// 对端只要一直发不带结束标志的片段就能把服务端内存耗光。512 MiB 足够容纳
/// 大型多帧对象,又不至于让单个连接吃掉整台机器。
pub const DEFAULT_MAX_DATASET_BYTES: usize = 512 * 1024 * 1024;

/// 收到的一条完整 DIMSE 消息。
#[derive(Debug, Clone)]
pub struct DimseMessage {
    pub command: Command,
    /// 数据集原始字节,按该表示上下文协商出的传输语法编码。
    ///
    /// 保留原始字节而不是解析后再编码 —— 落盘时原样写入,存下来的就和发送方
    /// 送出的完全一致。
    pub dataset: Option<Vec<u8>>,
    /// 这条消息用的表示上下文,响应必须沿用同一个。
    pub presentation_context_id: u8,
}

#[derive(Debug, Error)]
pub enum MessageError {
    #[error("association 传输失败")]
    Transport {
        #[source]
        source: Box<dicom_ul::association::Error>,
    },
    #[error("命令集处理失败")]
    Command {
        #[from]
        source: CommandError,
    },
    #[error("数据集超过 {limit} 字节上限")]
    DatasetTooLarge { limit: usize },
    #[error("对端在消息中途断开")]
    Truncated,
    #[error("收到意料之外的 PDU:{description}")]
    UnexpectedPdu { description: String },
    #[error("命令集 {size} 字节,超过协商的最大 PDU 长度 {max_pdu_length}")]
    CommandTooLarge { size: usize, max_pdu_length: u32 },
}

impl From<dicom_ul::association::Error> for MessageError {
    fn from(source: dicom_ul::association::Error) -> Self {
        Self::Transport {
            source: Box::new(source),
        }
    }
}

/// 对端结束了这次 association。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// 收到 A-RELEASE-RQ,正常结束。
    Released,
    /// 收到 A-ABORT,异常结束。
    Aborted,
}

/// 收下一条 DIMSE 消息;对端结束 association 时返回 [`Ended`]。
pub async fn receive_message<S>(
    association: &mut AsyncServerAssociation<S>,
    max_dataset_bytes: usize,
) -> Result<Result<DimseMessage, Ended>, MessageError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let mut command_bytes = Vec::new();
    let mut dataset_bytes = Vec::new();
    let mut presentation_context_id = None;
    let mut command_done = false;
    let mut dataset_done = false;

    loop {
        let pdu = match association.receive().await {
            Ok(pdu) => pdu,
            // 连接被对端直接掐断:消息还没收全,当作截断处理
            Err(error) if !command_done => {
                tracing::debug!(%error, "接收 PDU 失败,消息未完整");
                return Err(MessageError::Truncated);
            }
            Err(error) => return Err(error.into()),
        };

        match pdu {
            Pdu::PData { data } => {
                for pdv in data {
                    presentation_context_id = Some(pdv.presentation_context_id);
                    match pdv.value_type {
                        PDataValueType::Command => {
                            command_bytes.extend_from_slice(&pdv.data);
                            command_done |= pdv.is_last;
                        }
                        PDataValueType::Data => {
                            // 上限要在写入之前判,判完再写就已经吃下这一片了
                            if dataset_bytes.len() + pdv.data.len() > max_dataset_bytes {
                                return Err(MessageError::DatasetTooLarge {
                                    limit: max_dataset_bytes,
                                });
                            }
                            dataset_bytes.extend_from_slice(&pdv.data);
                            dataset_done |= pdv.is_last;
                        }
                    }
                }
            }
            Pdu::ReleaseRQ => {
                association.send(&Pdu::ReleaseRP).await?;
                return Ok(Err(Ended::Released));
            }
            Pdu::AbortRQ { source } => {
                tracing::info!(?source, "对端中止了 association");
                return Ok(Err(Ended::Aborted));
            }
            other => {
                return Err(MessageError::UnexpectedPdu {
                    description: other.short_description().to_string(),
                });
            }
        }

        if !command_done {
            continue;
        }

        let command = Command::decode(&command_bytes)?;
        if command.has_data_set() && !dataset_done {
            continue;
        }

        return Ok(Ok(DimseMessage {
            dataset: command.has_data_set().then_some(dataset_bytes),
            command,
            presentation_context_id: presentation_context_id.expect("走到这里至少收过一个 PDV"),
        }));
    }
}

/// 发一条只有命令集的消息(各类 RSP 都是这种)。
pub async fn send_command<S>(
    association: &mut AsyncServerAssociation<S>,
    presentation_context_id: u8,
    command: &Command,
) -> Result<(), MessageError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let encoded = command.encode()?;

    // 命令集只有一两百字节,任何实际协商出的最大 PDU 长度都装得下。
    // 真装不下说明对端报了个荒谬的值,此时明确报错而不是发一个超长 PDU
    // 让对端去猜。
    let max_pdu_length = association.requestor_max_pdu_length();
    if encoded.len() + PDV_HEADER_BYTES > max_pdu_length as usize {
        return Err(MessageError::CommandTooLarge {
            size: encoded.len(),
            max_pdu_length,
        });
    }

    association
        .send(&Pdu::PData {
            data: vec![PDataValue {
                presentation_context_id,
                value_type: PDataValueType::Command,
                is_last: true,
                data: encoded,
            }],
        })
        .await?;
    Ok(())
}

/// 发一条命令集 + 数据集的消息(C-FIND-RSP 的每条命中都是这种)。
///
/// 数据集按协商出的最大 PDU 长度切片。切片是必需的:一个几百条命中的
/// C-FIND 标识符虽然不大,但 `max_pdu_length` 常被对端报成 16 KiB,
/// 超过就得分帧,否则对端会直接中止 association。
pub async fn send_command_with_dataset<S>(
    association: &mut AsyncServerAssociation<S>,
    presentation_context_id: u8,
    command: &Command,
    dataset: &[u8],
) -> Result<(), MessageError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    send_command(association, presentation_context_id, command).await?;

    let max_pdu_length = association.requestor_max_pdu_length() as usize;
    // 每个 PDU 至少要能装下 PDV 头加一个字节,否则切片永远推进不了。
    // 对端报了荒谬的小值时按 1 KiB 兜底,让分帧还能继续。
    let chunk_size = max_pdu_length
        .checked_sub(PDV_HEADER_BYTES)
        .filter(|size| *size > 0)
        .unwrap_or(1024);

    // 空数据集也要发一个 is_last 的空 PDV,否则对端会一直等下去
    let mut chunks = dataset.chunks(chunk_size).peekable();
    if chunks.peek().is_none() {
        return send_pdv(association, presentation_context_id, &[], true).await;
    }
    while let Some(chunk) = chunks.next() {
        let is_last = chunks.peek().is_none();
        send_pdv(association, presentation_context_id, chunk, is_last).await?;
    }
    Ok(())
}

async fn send_pdv<S>(
    association: &mut AsyncServerAssociation<S>,
    presentation_context_id: u8,
    data: &[u8],
    is_last: bool,
) -> Result<(), MessageError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    association
        .send(&Pdu::PData {
            data: vec![PDataValue {
                presentation_context_id,
                value_type: PDataValueType::Data,
                is_last,
                data: data.to_vec(),
            }],
        })
        .await?;
    Ok(())
}

/// PDV 头:4 字节长度 + 1 字节表示上下文号 + 1 字节消息控制头。
const PDV_HEADER_BYTES: usize = 6;
