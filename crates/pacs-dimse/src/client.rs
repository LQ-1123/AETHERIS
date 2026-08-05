//! DIMSE SCU operations used by the routing engine.

use std::io::Cursor;
use std::time::Duration;

use dicom::dictionary_std::uids;
use dicom_ul::association::client::{AsyncClientAssociation, ClientAssociationOptions};
use dicom_ul::pdu::{PDataValue, PDataValueType, Pdu};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::command::{self, Command, CommandField};

const MAX_PDU_LENGTH: u32 = 16_384;
const PDV_HEADER_BYTES: usize = 6;

#[derive(Debug, Clone)]
pub struct DimseClientConfig {
    pub host: String,
    pub port: u16,
    pub called_ae_title: String,
    pub calling_ae_title: String,
    pub timeout: Duration,
    pub use_tls: bool,
    pub ca_pem: Option<String>,
}

impl DimseClientConfig {
    pub fn new(
        host: impl Into<String>,
        port: u16,
        called_ae_title: impl Into<String>,
        calling_ae_title: impl Into<String>,
    ) -> Self {
        Self {
            host: host.into(),
            port,
            called_ae_title: called_ae_title.into(),
            calling_ae_title: calling_ae_title.into(),
            timeout: Duration::from_secs(15),
            use_tls: false,
            ca_pem: None,
        }
    }
}

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("DIMSE TLS CA 证书无效: {0}")]
    InvalidTlsCertificate(String),
    #[error("DICOM 文件解析失败: {0}")]
    DicomRead(String),
    #[error("DICOM 数据集编码失败: {0}")]
    DicomWrite(String),
    #[error("DIMSE association 失败: {0}")]
    Association(String),
    #[error("目的地未接受 SOP Class {sop_class_uid} / Transfer Syntax {transfer_syntax_uid}")]
    PresentationContextRejected {
        sop_class_uid: String,
        transfer_syntax_uid: String,
    },
    #[error("DIMSE 命令编解码失败: {0}")]
    Command(#[from] crate::CommandError),
    #[error("目的地返回了意外的 PDU: {0}")]
    UnexpectedPdu(String),
    #[error("目的地返回了意外的 DIMSE 命令 {0:?}")]
    UnexpectedCommand(CommandField),
    #[error("DIMSE 响应缺少 Status")]
    MissingStatus,
    #[error("DIMSE 操作失败，状态 {0}")]
    FailedStatus(crate::Status),
}

pub async fn echo(config: &DimseClientConfig) -> Result<(), ClientError> {
    let options = options(config).with_presentation_context(
        uids::VERIFICATION,
        vec![
            uids::IMPLICIT_VR_LITTLE_ENDIAN,
            uids::EXPLICIT_VR_LITTLE_ENDIAN,
        ],
    );
    if config.use_tls {
        let association = configure_tls(config, options)?
            .establish_tls_async((config.host.as_str(), config.port))
            .await
            .map_err(association_error)?;
        echo_association(association).await
    } else {
        let association = options
            .establish_async((config.host.as_str(), config.port))
            .await
            .map_err(association_error)?;
        echo_association(association).await
    }
}

async fn echo_association<S>(mut association: AsyncClientAssociation<S>) -> Result<(), ClientError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let context = association
        .presentation_contexts()
        .iter()
        .find(|context| context.abstract_syntax == uids::VERIFICATION)
        .map(|context| context.id)
        .ok_or_else(|| ClientError::PresentationContextRejected {
            sop_class_uid: uids::VERIFICATION.to_owned(),
            transfer_syntax_uid: uids::IMPLICIT_VR_LITTLE_ENDIAN.to_owned(),
        })?;
    send_command(&mut association, context, &command::c_echo_rq(1)).await?;
    let response = receive_command(&mut association).await?;
    verify_response(&response, CommandField::CEchoRsp, 1)?;
    association.release().await.map_err(association_error)
}

pub async fn store(config: &DimseClientConfig, file_bytes: &[u8]) -> Result<(), ClientError> {
    let object = dicom::object::from_reader(Cursor::new(file_bytes))
        .map_err(|error| ClientError::DicomRead(error.to_string()))?;
    let trim_uid = |value: &str| {
        value
            .trim_matches(|character: char| character == '\0' || character.is_whitespace())
            .to_owned()
    };
    let sop_class_uid = trim_uid(&object.meta().media_storage_sop_class_uid);
    let sop_instance_uid = trim_uid(&object.meta().media_storage_sop_instance_uid);
    let transfer_syntax_uid = trim_uid(&object.meta().transfer_syntax);
    let mut dataset = Vec::new();
    object
        .write_dataset(&mut dataset)
        .map_err(|error| ClientError::DicomWrite(error.to_string()))?;

    let options = options(config)
        .with_presentation_context(sop_class_uid.as_str(), vec![transfer_syntax_uid.as_str()]);
    if config.use_tls {
        let association = configure_tls(config, options)?
            .establish_tls_async((config.host.as_str(), config.port))
            .await
            .map_err(association_error)?;
        store_association(
            association,
            &sop_class_uid,
            &sop_instance_uid,
            &transfer_syntax_uid,
            &dataset,
        )
        .await
    } else {
        let association = options
            .establish_async((config.host.as_str(), config.port))
            .await
            .map_err(association_error)?;
        store_association(
            association,
            &sop_class_uid,
            &sop_instance_uid,
            &transfer_syntax_uid,
            &dataset,
        )
        .await
    }
}

async fn store_association<S>(
    mut association: AsyncClientAssociation<S>,
    sop_class_uid: &str,
    sop_instance_uid: &str,
    transfer_syntax_uid: &str,
    dataset: &[u8],
) -> Result<(), ClientError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let context = association
        .presentation_contexts()
        .iter()
        .find(|context| {
            context.abstract_syntax == sop_class_uid
                && context.transfer_syntax == transfer_syntax_uid
        })
        .map(|context| context.id)
        .ok_or_else(|| ClientError::PresentationContextRejected {
            sop_class_uid: sop_class_uid.to_owned(),
            transfer_syntax_uid: transfer_syntax_uid.to_owned(),
        })?;
    let request = command::c_store_rq(1, sop_class_uid, sop_instance_uid);
    send_command_with_dataset(&mut association, context, &request, dataset).await?;
    let response = receive_command(&mut association).await?;
    verify_response(&response, CommandField::CStoreRsp, 1)?;
    association.release().await.map_err(association_error)
}

fn options(config: &DimseClientConfig) -> ClientAssociationOptions<'_> {
    ClientAssociationOptions::new()
        .calling_ae_title(&config.calling_ae_title)
        .called_ae_title(&config.called_ae_title)
        .max_pdu_length(MAX_PDU_LENGTH)
        .connection_timeout(config.timeout)
        .read_timeout(config.timeout)
        .write_timeout(config.timeout)
}

fn configure_tls<'a>(
    config: &'a DimseClientConfig,
    options: ClientAssociationOptions<'a>,
) -> Result<ClientAssociationOptions<'a>, ClientError> {
    let pem = config.ca_pem.as_deref().ok_or_else(|| {
        ClientError::InvalidTlsCertificate("启用 TLS 时必须提供 CA PEM".to_owned())
    })?;
    let mut reader = Cursor::new(pem.as_bytes());
    let mut roots = rustls::RootCertStore::empty();
    let mut count = 0;
    for certificate in rustls_pemfile::certs(&mut reader) {
        let certificate =
            certificate.map_err(|error| ClientError::InvalidTlsCertificate(error.to_string()))?;
        roots
            .add(certificate)
            .map_err(|error| ClientError::InvalidTlsCertificate(error.to_string()))?;
        count += 1;
    }
    if count == 0 {
        return Err(ClientError::InvalidTlsCertificate(
            "CA PEM 中没有证书".to_owned(),
        ));
    }
    let tls = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(options.tls_config(tls).server_name(&config.host))
}

fn verify_response(
    response: &Command,
    expected: CommandField,
    message_id: u16,
) -> Result<(), ClientError> {
    let field = response.command_field()?;
    if field != expected || response.message_id_being_responded_to() != Some(message_id) {
        return Err(ClientError::UnexpectedCommand(field));
    }
    let status = response.status().ok_or(ClientError::MissingStatus)?;
    if status.is_success() || status.is_warning() {
        Ok(())
    } else {
        Err(ClientError::FailedStatus(status))
    }
}

async fn send_command<S>(
    association: &mut AsyncClientAssociation<S>,
    presentation_context_id: u8,
    command: &Command,
) -> Result<(), ClientError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    association
        .send(&Pdu::PData {
            data: vec![PDataValue {
                presentation_context_id,
                value_type: PDataValueType::Command,
                is_last: true,
                data: command.encode()?,
            }],
        })
        .await
        .map_err(association_error)
}

async fn send_command_with_dataset<S>(
    association: &mut AsyncClientAssociation<S>,
    presentation_context_id: u8,
    command: &Command,
    dataset: &[u8],
) -> Result<(), ClientError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    send_command(association, presentation_context_id, command).await?;
    let chunk_size = (association.acceptor_max_pdu_length() as usize)
        .checked_sub(PDV_HEADER_BYTES)
        .filter(|size| *size > 0)
        .unwrap_or(1024);
    let mut chunks = dataset.chunks(chunk_size).peekable();
    if chunks.peek().is_none() {
        send_dataset_pdv(association, presentation_context_id, &[], true).await?;
    }
    while let Some(chunk) = chunks.next() {
        let is_last = chunks.peek().is_none();
        send_dataset_pdv(association, presentation_context_id, chunk, is_last).await?;
    }
    Ok(())
}

async fn send_dataset_pdv<S>(
    association: &mut AsyncClientAssociation<S>,
    presentation_context_id: u8,
    data: &[u8],
    is_last: bool,
) -> Result<(), ClientError>
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
        .await
        .map_err(association_error)
}

async fn receive_command<S>(
    association: &mut AsyncClientAssociation<S>,
) -> Result<Command, ClientError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let mut bytes = Vec::new();
    loop {
        match association.receive().await.map_err(association_error)? {
            Pdu::PData { data } => {
                for value in data {
                    if value.value_type != PDataValueType::Command {
                        return Err(ClientError::UnexpectedPdu(
                            "响应包含意外的数据集".to_owned(),
                        ));
                    }
                    bytes.extend_from_slice(&value.data);
                    if value.is_last {
                        return Command::decode(&bytes).map_err(ClientError::from);
                    }
                }
            }
            Pdu::AbortRQ { .. } => {
                return Err(ClientError::UnexpectedPdu("A-ABORT".to_owned()));
            }
            other => {
                return Err(ClientError::UnexpectedPdu(
                    other.short_description().to_string(),
                ));
            }
        }
    }
}

fn association_error(error: dicom_ul::association::Error) -> ClientError {
    ClientError::Association(error.to_string())
}
