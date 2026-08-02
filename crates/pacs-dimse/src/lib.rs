//! 自研 DIMSE 服务类。
//!
//! `dicom-ul` 只提供传输层:association 协商、PDU 读写、P-DATA 分帧。
//! 命令集的组装解析、消息的收发、各服务类的状态机都在本 crate 实现。

pub mod command;
pub mod find;
pub mod message;
pub mod scp;
pub mod server;
pub mod sop_class;

pub use command::{Command, CommandError, CommandField, Status};
pub use find::{FindFailure, FindHandler, FindRequest, FindResponse};
pub use message::{DimseMessage, Ended, MessageError};
pub use scp::{IncomingInstance, StoreFailure, StoreHandler};
pub use server::{DimseServer, ServerConfig, ServerError};
