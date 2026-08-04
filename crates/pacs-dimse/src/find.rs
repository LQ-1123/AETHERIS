//! C-FIND 的服务提供方(PS3.4 C.4.1)。
//!
//! # 响应序列
//!
//! 一次 C-FIND-RQ 换来一串响应,不是一条:
//!
//! ```text
//! SCU ──C-FIND-RQ(标识符)──▶ SCP
//!     ◀──C-FIND-RSP 0xFF00 + 标识符──  第 1 条命中
//!     ◀──C-FIND-RSP 0xFF00 + 标识符──  第 2 条命中
//!     ◀────────  ……  ────────
//!     ◀──C-FIND-RSP 0x0000(无数据集)── 结束
//! ```
//!
//! 对端靠「状态不再是 pending」判断查询结束。最后那条的状态码填错(比如
//! 又填了 0xFF00),对端就会一直等下去直到超时。
//!
//! # C-CANCEL 的支持程度
//!
//! 标准允许 SCU 在响应流中途发 C-CANCEL-FIND-RQ 要求提前中止。要真正做到
//! 「边发边听」,得把 association 的读写两半拆开并发轮询,而 `dicom-ul` 的
//! `AsyncServerAssociation` 没有暴露拆分接口,`receive()` 也不是 cancel-safe
//! ——在 `select!` 里被丢弃可能已经吃掉半个 PDU,整条连接从此解不出东西。
//!
//! 所以这里的取舍是:结果集在发送前就已经全部取出(条数由
//! [`pacs_db::DEFAULT_LIMIT`] 封顶),发送阶段不等数据库、纯输出,通常几毫秒
//! 就走完。中途到达的 C-CANCEL 会留在套接字缓冲里,在下一轮循环被读到并
//! **忽略**(见 [`crate::scp::serve`])—— 对一个已经结束的操作回取消状态没有意义,
//! 但绝不能因此中止 association,那会把对端后续的查询一起打断。

use std::future::Future;

use dicom::encoding::TransferSyntaxIndex;
use dicom::object::InMemDicomObject;
use dicom::transfer_syntax::TransferSyntaxRegistry;
use dicom_ul::association::server::AsyncServerAssociation;
use pacs_core::query::{Query, QueryLevel};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::command::{self, Command, Status};
use crate::message::{self, MessageError};
use crate::sop_class;

/// 一次查询请求。
#[derive(Debug)]
pub struct FindRequest<'a> {
    pub query: &'a Query,
    /// 用的是哪个信息模型(Patient Root / Study Root)。
    pub sop_class_uid: &'a str,
    /// 发起方 AE Title。协议不认证,只作审计线索。
    pub calling_ae_title: &'a str,
}

/// 查询结果。
#[derive(Debug, Default)]
pub struct FindResponse {
    /// 每条一个响应标识符,按顺序作为 pending 响应发出。
    pub identifiers: Vec<InMemDicomObject>,
    /// 请求里有我们不支持的匹配键。为真时 pending 状态用 `0xFF01`。
    ///
    /// 这个信号不能省:不支持的键被忽略后,返回的结果会**多于**对方的本意,
    /// 而对方无从察觉。`0xFF01` 就是标准留给这种情况的告知手段。
    pub keys_unsupported: bool,
}

#[derive(Debug, Error)]
pub enum FindFailure {
    #[error("查询条件超出支持范围:{0}")]
    Unsupported(String),
    #[error("查询执行失败:{0}")]
    Processing(String),
}

impl FindFailure {
    fn status(&self) -> Status {
        match self {
            Self::Unsupported(_) => Status::IDENTIFIER_DOES_NOT_MATCH_SOP_CLASS,
            Self::Processing(_) => Status::UNABLE_TO_PROCESS,
        }
    }
}

/// 收到查询请求后做什么。由 `pacsd` 实现:查数据库。
pub trait FindHandler: Send + Sync {
    fn find(
        &self,
        request: FindRequest<'_>,
    ) -> impl Future<Output = Result<FindResponse, FindFailure>> + Send;
}

/// 处理一条 C-FIND-RQ:查询、流式回 pending、最后回一条结束状态。
///
/// 与 C-STORE 一样不返回 `Result<Status>` —— 无论成败都必须以一条非 pending
/// 的响应收尾,否则对端会一直等。只有真正发不出去(连接断了)才向上冒泡。
pub async fn handle_find<S, H>(
    association: &mut AsyncServerAssociation<S>,
    request_command: &Command,
    identifier_bytes: Option<&[u8]>,
    presentation_context_id: u8,
    handler: &H,
    calling_ae_title: &str,
) -> Result<(), MessageError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
    H: FindHandler,
{
    let sop_class_uid = request_command.affected_sop_class_uid().unwrap_or_default();

    // 数据集用该表示上下文协商出的传输语法,不是命令集那个隐式 VR LE
    let transfer_syntax_uid = negotiated_transfer_syntax(association, presentation_context_id);
    let transfer_syntax = transfer_syntax_uid
        .as_deref()
        .and_then(|uid| TransferSyntaxRegistry.get(uid));

    let outcome = match (identifier_bytes, transfer_syntax) {
        (None, _) => {
            tracing::warn!(calling_ae_title, "C-FIND-RQ 缺少标识符数据集");
            Err(Status::UNABLE_TO_PROCESS)
        }
        (_, None) => {
            tracing::error!(
                context_id = presentation_context_id,
                "找不到表示上下文的协商结果或本地不支持该传输语法"
            );
            Err(Status::UNABLE_TO_PROCESS)
        }
        (Some(bytes), Some(ts)) => {
            match InMemDicomObject::read_dataset_with_ts(bytes, ts) {
                Ok(mut object) => {
                    pacs_core::normalize_dataset_text(&mut object);
                    match Query::from_identifier(&object) {
                        Ok(query) => match validate_level(&sop_class_uid, query.level) {
                            Ok(()) => Ok(query),
                            Err(status) => Err(status),
                        },
                        Err(error) => {
                            tracing::warn!(%error, calling_ae_title, "标识符无法解析成查询");
                            // 层级缺失/非法属于「标识符和 SOP Class 对不上」
                            Err(Status::IDENTIFIER_DOES_NOT_MATCH_SOP_CLASS)
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, calling_ae_title, "标识符数据集解析失败");
                    Err(Status::UNABLE_TO_PROCESS)
                }
            }
        }
    };

    let query = match outcome {
        Ok(query) => query,
        Err(status) => {
            return respond_final(
                association,
                request_command,
                presentation_context_id,
                status,
            )
            .await;
        }
    };

    let response = match handler
        .find(FindRequest {
            query: &query,
            sop_class_uid: &sop_class_uid,
            calling_ae_title,
        })
        .await
    {
        Ok(response) => response,
        Err(failure) => {
            tracing::warn!(%failure, calling_ae_title, level = query.level.as_str(), "查询失败");
            return respond_final(
                association,
                request_command,
                presentation_context_id,
                failure.status(),
            )
            .await;
        }
    };

    let pending_status = if response.keys_unsupported {
        Status::PENDING_WITH_WARNING
    } else {
        Status::PENDING
    };
    // 上面已确认过传输语法存在,这里必然取得到
    let ts = transfer_syntax.expect("走到这里说明传输语法已解析成功");

    let matches = response.identifiers.len();
    for identifier in &response.identifiers {
        let mut encoded = Vec::new();
        if let Err(error) = identifier.write_dataset_with_ts(&mut encoded, ts) {
            tracing::error!(%error, "响应标识符编码失败,以失败状态收尾");
            return respond_final(
                association,
                request_command,
                presentation_context_id,
                Status::UNABLE_TO_PROCESS,
            )
            .await;
        }

        let rsp = command::c_find_rsp(request_command, pending_status, true);
        message::send_command_with_dataset(association, presentation_context_id, &rsp, &encoded)
            .await?;
    }

    tracing::debug!(
        calling_ae_title,
        level = query.level.as_str(),
        matches,
        keys_unsupported = response.keys_unsupported,
        "C-FIND 完成"
    );
    respond_final(
        association,
        request_command,
        presentation_context_id,
        Status::SUCCESS,
    )
    .await
}

/// 收尾响应:不带数据集,状态码告诉对方查询是怎么结束的。
async fn respond_final<S>(
    association: &mut AsyncServerAssociation<S>,
    request_command: &Command,
    presentation_context_id: u8,
    status: Status,
) -> Result<(), MessageError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let rsp = command::c_find_rsp(request_command, status, false);
    message::send_command(association, presentation_context_id, &rsp).await
}

/// 查询层级必须属于该信息模型。
///
/// Study Root 没有 PATIENT 层 —— 对它发 PATIENT 层查询是明确的协议错误,
/// 不能当成 STUDY 层去将就:那会返回一批对方根本没请求的东西。
fn validate_level(sop_class_uid: &str, level: QueryLevel) -> Result<(), Status> {
    let allowed = match sop_class_uid {
        sop_class::PATIENT_ROOT_FIND => matches!(
            level,
            QueryLevel::Patient | QueryLevel::Study | QueryLevel::Series | QueryLevel::Image
        ),
        sop_class::STUDY_ROOT_FIND => matches!(
            level,
            QueryLevel::Study | QueryLevel::Series | QueryLevel::Image
        ),
        // 协商阶段只接受上面两个,走到这里说明协商放进来了别的东西
        _ => false,
    };

    if allowed {
        Ok(())
    } else {
        tracing::warn!(
            sop_class_uid,
            level = level.as_str(),
            "查询层级不属于该信息模型"
        );
        Err(Status::IDENTIFIER_DOES_NOT_MATCH_SOP_CLASS)
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn study_root_rejects_the_patient_level() {
        // Study Root 从 STUDY 层起,没有 PATIENT 层
        assert_eq!(
            validate_level(sop_class::STUDY_ROOT_FIND, QueryLevel::Patient),
            Err(Status::IDENTIFIER_DOES_NOT_MATCH_SOP_CLASS)
        );
        for level in [QueryLevel::Study, QueryLevel::Series, QueryLevel::Image] {
            assert_eq!(validate_level(sop_class::STUDY_ROOT_FIND, level), Ok(()));
        }
    }

    #[test]
    fn patient_root_accepts_every_level() {
        for level in [
            QueryLevel::Patient,
            QueryLevel::Study,
            QueryLevel::Series,
            QueryLevel::Image,
        ] {
            assert_eq!(validate_level(sop_class::PATIENT_ROOT_FIND, level), Ok(()));
        }
    }

    /// 没协商过的 SOP Class 一律拒绝,不做「大概是这个意思」的猜测。
    #[test]
    fn unknown_information_model_is_rejected() {
        assert_eq!(
            validate_level("1.2.840.10008.5.1.4.1.2.3.1", QueryLevel::Study),
            Err(Status::IDENTIFIER_DOES_NOT_MATCH_SOP_CLASS)
        );
        assert_eq!(
            validate_level("", QueryLevel::Study),
            Err(Status::IDENTIFIER_DOES_NOT_MATCH_SOP_CLASS)
        );
    }
}
