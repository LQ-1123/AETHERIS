//! C-MOVE/C-GET 的检索服务类。
//!
//! 该模块只负责 DIMSE 状态机。实例如何从数据库和对象存储读出、Move
//! Destination 如何解析到网络地址，都通过 [`RetrieveHandler`] 注入；协议层
//! 因此不会绕过机构边界，也不会获得任意外连能力。

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use dicom::encoding::TransferSyntaxIndex;
use dicom::object::InMemDicomObject;
use dicom::transfer_syntax::TransferSyntaxRegistry;
use dicom_ul::association::server::AsyncServerAssociation;
use pacs_core::query::{Query, QueryLevel};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::client::{self, DimseClientConfig};
use crate::command::{self, Command, CommandField, Status};
use crate::message::{self, Ended, MessageError};
use crate::sop_class;

#[derive(Debug)]
pub struct RetrieveRequest<'a> {
    pub query: &'a Query,
    pub sop_class_uid: &'a str,
    pub calling_ae_title: &'a str,
    pub remote_addr: SocketAddr,
}

/// 一个可被 C-STORE 子操作发送的原始实例。
#[derive(Debug, Clone)]
pub struct RetrievedInstance {
    pub sop_class_uid: String,
    pub sop_instance_uid: String,
    pub transfer_syntax_uid: String,
    pub file_bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct MoveDestination {
    pub ae_title: String,
    pub config: DimseClientConfig,
}

#[derive(Debug, Error)]
pub enum RetrieveFailure {
    #[error("检索条件超出支持范围:{0}")]
    Unsupported(String),
    #[error("检索执行失败:{0}")]
    Processing(String),
    #[error("Move Destination 未在白名单中:{0}")]
    DestinationUnknown(String),
}

impl RetrieveFailure {
    fn status(&self) -> Status {
        match self {
            Self::Unsupported(_) => Status::IDENTIFIER_DOES_NOT_MATCH_SOP_CLASS,
            Self::Processing(_) => Status::UNABLE_TO_PROCESS,
            // PS3.4 C.4.2: Move Destination Unknown.
            Self::DestinationUnknown(_) => Status::MOVE_DESTINATION_UNKNOWN,
        }
    }
}

/// 检索处理器。实现者必须在 `move_destination` 中做白名单校验；返回 None
/// 表示拒绝该 AE，而不是尝试把 AE 当作 host 解析。
pub trait RetrieveHandler: Send + Sync {
    fn retrieve(
        &self,
        request: RetrieveRequest<'_>,
    ) -> impl Future<Output = Result<Vec<RetrievedInstance>, RetrieveFailure>> + Send;

    fn move_destination(
        &self,
        calling_ae_title: &str,
        destination: &str,
    ) -> impl Future<Output = Result<Option<MoveDestination>, RetrieveFailure>> + Send;
}

pub struct RetrieveMessage<'a> {
    pub command: &'a Command,
    pub identifier_bytes: Option<&'a [u8]>,
    pub presentation_context_id: u8,
    pub calling_ae_title: &'a str,
    pub remote_addr: SocketAddr,
    pub max_dataset_bytes: usize,
    pub max_suboperations: usize,
}

#[derive(Debug, Clone, Copy, Default)]
struct SuboperationCounts {
    remaining: u16,
    completed: u16,
    failed: u16,
    warning: u16,
}

/// 处理 C-MOVE-RQ：查询、白名单解析、逐项 C-STORE 推送和计数响应。
pub async fn handle_move<S, H>(
    association: &mut AsyncServerAssociation<S>,
    handler: &H,
    message: RetrieveMessage<'_>,
) -> Result<(), MessageError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
    H: RetrieveHandler,
{
    let request_command = message.command;
    let presentation_context_id = message.presentation_context_id;
    let destination = request_command.move_destination().unwrap_or_default();
    let destination_config = match handler
        .move_destination(message.calling_ae_title, &destination)
        .await
    {
        Ok(Some(destination)) => destination,
        Ok(None) => {
            return respond_final(
                association,
                request_command,
                presentation_context_id,
                Status::MOVE_DESTINATION_UNKNOWN,
                SuboperationCounts::default(),
            )
            .await;
        }
        Err(failure) => {
            return respond_final(
                association,
                request_command,
                presentation_context_id,
                failure.status(),
                SuboperationCounts::default(),
            )
            .await;
        }
    };

    let query = match parse_query(
        association,
        request_command,
        message.identifier_bytes,
        presentation_context_id,
    )
    .await?
    {
        Ok(query) => query,
        Err(status) => {
            return respond_final(
                association,
                request_command,
                presentation_context_id,
                status,
                SuboperationCounts::default(),
            )
            .await;
        }
    };
    if query.level != QueryLevel::Study {
        return respond_final(
            association,
            request_command,
            presentation_context_id,
            Status::IDENTIFIER_DOES_NOT_MATCH_SOP_CLASS,
            SuboperationCounts::default(),
        )
        .await;
    }

    let instances = match handler
        .retrieve(RetrieveRequest {
            query: &query,
            sop_class_uid: request_command
                .affected_sop_class_uid()
                .as_deref()
                .unwrap_or_default(),
            calling_ae_title: message.calling_ae_title,
            remote_addr: message.remote_addr,
        })
        .await
    {
        Ok(instances) => instances,
        Err(failure) => {
            return respond_final(
                association,
                request_command,
                presentation_context_id,
                failure.status(),
                SuboperationCounts::default(),
            )
            .await;
        }
    };

    let total = instances.len() as u16;
    let mut remaining = total;
    let mut completed = 0_u16;
    let mut failed = 0_u16;
    let warning = 0_u16;
    let semaphore = Arc::new(tokio::sync::Semaphore::new(
        message.max_suboperations.max(1),
    ));
    let mut tasks = tokio::task::JoinSet::new();
    for instance in instances {
        let permit = Arc::clone(&semaphore);
        let config = destination_config.config.clone();
        tasks.spawn(async move {
            let _permit = permit
                .acquire_owned()
                .await
                .expect("检索并发信号量不应关闭");
            let result = client::store(&config, &instance.file_bytes).await;
            (instance.sop_instance_uid, result)
        });
    }
    while let Some(joined) = tasks.join_next().await {
        let (sop_instance_uid, result) = match joined {
            Ok(outcome) => outcome,
            Err(error) => {
                failed = failed.saturating_add(1);
                remaining = remaining.saturating_sub(1);
                tracing::warn!(%error, "C-MOVE 子操作任务异常结束");
                if remaining > 0 {
                    respond_move(
                        association,
                        request_command,
                        presentation_context_id,
                        Status::PENDING,
                        SuboperationCounts {
                            remaining,
                            completed,
                            failed,
                            warning,
                        },
                    )
                    .await?;
                }
                continue;
            }
        };
        remaining = remaining.saturating_sub(1);
        match result {
            Ok(()) => completed = completed.saturating_add(1),
            Err(error) => {
                failed = failed.saturating_add(1);
                tracing::warn!(
                    destination = %destination_config.ae_title,
                    sop_instance_uid = %sop_instance_uid,
                    %error,
                    "C-MOVE C-STORE 子操作失败"
                );
            }
        }
        if remaining > 0 {
            respond_move(
                association,
                request_command,
                presentation_context_id,
                Status::PENDING,
                SuboperationCounts {
                    remaining,
                    completed,
                    failed,
                    warning,
                },
            )
            .await?;
        }
    }

    let status = if failed == 0 && warning == 0 {
        Status::SUCCESS
    } else if completed > 0 {
        Status(0xB000)
    } else {
        Status::PROCESSING_FAILURE
    };
    respond_move(
        association,
        request_command,
        presentation_context_id,
        status,
        SuboperationCounts {
            remaining: 0,
            completed,
            failed,
            warning,
        },
    )
    .await?;
    Ok(())
}

/// C-GET-RQ：沿当前 association 发出 C-STORE 子操作。
pub async fn handle_get<S, H>(
    association: &mut AsyncServerAssociation<S>,
    handler: &H,
    message: RetrieveMessage<'_>,
) -> Result<(), MessageError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
    H: RetrieveHandler,
{
    let request_command = message.command;
    let presentation_context_id = message.presentation_context_id;
    let query = match parse_query(
        association,
        request_command,
        message.identifier_bytes,
        presentation_context_id,
    )
    .await?
    {
        Ok(query) => query,
        Err(status) => {
            return respond_get(
                association,
                request_command,
                presentation_context_id,
                status,
                SuboperationCounts::default(),
            )
            .await;
        }
    };
    if query.level != QueryLevel::Study {
        return respond_get(
            association,
            request_command,
            presentation_context_id,
            Status::IDENTIFIER_DOES_NOT_MATCH_SOP_CLASS,
            SuboperationCounts::default(),
        )
        .await;
    }
    let instances = match handler
        .retrieve(RetrieveRequest {
            query: &query,
            sop_class_uid: request_command
                .affected_sop_class_uid()
                .as_deref()
                .unwrap_or_default(),
            calling_ae_title: message.calling_ae_title,
            remote_addr: message.remote_addr,
        })
        .await
    {
        Ok(instances) => instances,
        Err(failure) => {
            return respond_get(
                association,
                request_command,
                presentation_context_id,
                failure.status(),
                SuboperationCounts::default(),
            )
            .await;
        }
    };

    let total = instances.len() as u16;
    let mut remaining = total;
    let mut completed = 0_u16;
    let mut failed = 0_u16;
    for instance in instances {
        let context = association.presentation_contexts().iter().find(|context| {
            context.abstract_syntax == instance.sop_class_uid
                && context.transfer_syntax == instance.transfer_syntax_uid
        });
        let Some(context) = context else {
            failed = failed.saturating_add(1);
            remaining = remaining.saturating_sub(1);
            continue;
        };
        let Some(ts) = TransferSyntaxRegistry.get(&instance.transfer_syntax_uid) else {
            failed = failed.saturating_add(1);
            remaining = remaining.saturating_sub(1);
            continue;
        };
        let dataset = match dicom::object::from_reader(std::io::Cursor::new(&instance.file_bytes)) {
            Ok(file) => {
                let mut bytes = Vec::new();
                if file.write_dataset_with_ts(&mut bytes, ts).is_ok() {
                    bytes
                } else {
                    Vec::new()
                }
            }
            Err(_) => Vec::new(),
        };
        if dataset.is_empty() && !instance.file_bytes.is_empty() {
            failed = failed.saturating_add(1);
        } else {
            let store_rq = command::c_store_rq(
                completed.saturating_add(failed).saturating_add(1),
                &instance.sop_class_uid,
                &instance.sop_instance_uid,
            );
            message::send_command_with_dataset(association, context.id, &store_rq, &dataset)
                .await?;
            match message::receive_message(association, message.max_dataset_bytes).await? {
                Ok(response) if response.command.command_field()? == CommandField::CStoreRsp => {
                    if response
                        .command
                        .status()
                        .is_some_and(|status| status.is_success() || status.is_warning())
                    {
                        completed = completed.saturating_add(1);
                    } else {
                        failed = failed.saturating_add(1);
                    }
                }
                Ok(_) | Err(Ended::Released | Ended::Aborted) => {
                    failed = failed.saturating_add(1);
                }
            }
        }
        remaining = remaining.saturating_sub(1);
        if remaining > 0 {
            respond_get(
                association,
                request_command,
                presentation_context_id,
                Status::PENDING,
                SuboperationCounts {
                    remaining,
                    completed,
                    failed,
                    warning: 0,
                },
            )
            .await?;
        }
    }
    let status = if failed == 0 {
        Status::SUCCESS
    } else if completed > 0 {
        Status(0xB000)
    } else {
        Status::PROCESSING_FAILURE
    };
    respond_get(
        association,
        request_command,
        presentation_context_id,
        status,
        SuboperationCounts {
            remaining: 0,
            completed,
            failed,
            warning: 0,
        },
    )
    .await?;
    Ok(())
}

async fn parse_query<S>(
    association: &AsyncServerAssociation<S>,
    request_command: &Command,
    identifier_bytes: Option<&[u8]>,
    presentation_context_id: u8,
) -> Result<Result<Query, Status>, MessageError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    match identifier_bytes {
        Some(bytes) => {
            let ts_uid = association
                .presentation_contexts()
                .iter()
                .find(|context| context.id == presentation_context_id)
                .map(|context| context.transfer_syntax.clone());
            let Some(ts) = ts_uid
                .as_deref()
                .and_then(|uid| TransferSyntaxRegistry.get(uid))
            else {
                return Ok(Err(Status::UNABLE_TO_PROCESS));
            };
            let Ok(mut object) = InMemDicomObject::read_dataset_with_ts(bytes, ts) else {
                return Ok(Err(Status::UNABLE_TO_PROCESS));
            };
            pacs_core::normalize_dataset_text(&mut object);
            let Ok(query) = Query::from_identifier(&object) else {
                return Ok(Err(Status::IDENTIFIER_DOES_NOT_MATCH_SOP_CLASS));
            };
            let sop = request_command.affected_sop_class_uid().unwrap_or_default();
            let valid = match sop.as_str() {
                sop_class::PATIENT_ROOT_FIND
                | sop_class::PATIENT_ROOT_MOVE
                | sop_class::PATIENT_ROOT_GET => true,
                sop_class::STUDY_ROOT_FIND
                | sop_class::STUDY_ROOT_MOVE
                | sop_class::STUDY_ROOT_GET => query.level != QueryLevel::Patient,
                _ => false,
            };
            if valid {
                return Ok(Ok(query));
            }
            Ok(Err(Status::IDENTIFIER_DOES_NOT_MATCH_SOP_CLASS))
        }
        None => Ok(Err(Status::UNABLE_TO_PROCESS)),
    }
}

async fn respond_final<S>(
    association: &mut AsyncServerAssociation<S>,
    request: &Command,
    context: u8,
    status: Status,
    counts: SuboperationCounts,
) -> Result<(), MessageError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    respond_move(association, request, context, status, counts).await
}

async fn respond_move<S>(
    association: &mut AsyncServerAssociation<S>,
    request: &Command,
    context: u8,
    status: Status,
    counts: SuboperationCounts,
) -> Result<(), MessageError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let rsp = command::c_retrieve_rsp(
        request,
        CommandField::CMoveRsp,
        status,
        counts.remaining,
        counts.completed,
        counts.failed,
        counts.warning,
    );
    message::send_command(association, context, &rsp).await
}

async fn respond_get<S>(
    association: &mut AsyncServerAssociation<S>,
    request: &Command,
    context: u8,
    status: Status,
    counts: SuboperationCounts,
) -> Result<(), MessageError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let rsp = command::c_retrieve_rsp(
        request,
        CommandField::CGetRsp,
        status,
        counts.remaining,
        counts.completed,
        counts.failed,
        counts.warning,
    );
    message::send_command(association, context, &rsp).await
}
