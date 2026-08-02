//! DIMSE 命令集的编解码(PS3.7)。
//!
//! # 一个容易踩的点
//!
//! **命令集永远用 Implicit VR Little Endian 编码,和数据集协商出来的传输语法
//! 无关**(PS3.7 §6.3.1)。也就是说同一次交互里,命令用隐式 VR,紧跟其后的
//! 数据集可能用 JPEG 2000 —— 两者的编码规则不同。按协商结果去解命令集,
//! 遇到显式 VR 的连接就会解出乱码。

use dicom::core::{DataElement, PrimitiveValue, VR};
use dicom::dictionary_std::{tags, uids};
use dicom::encoding::TransferSyntaxIndex;
use dicom::encoding::transfer_syntax::TransferSyntax;
use dicom::object::InMemDicomObject;
use dicom::transfer_syntax::TransferSyntaxRegistry;
use thiserror::Error;

/// (0000,0800) CommandDataSetType 的「没有数据集」取值。
///
/// 这个字段不是布尔量:`0x0101` 表示后面没有数据集,**其他任何值**都表示有。
/// 当成 0/1 判断会把带数据集的消息误判成不带。
pub const NO_DATA_SET: u16 = 0x0101;

/// 有数据集时我们填的值。标准只要求「不等于 0x0101」。
const DATA_SET_PRESENT: u16 = 0x0000;

/// C-FIND/C-MOVE/C-GET 响应里"带标识符数据集"的惯用取值。
///
/// 同样只要求不等于 `NO_DATA_SET`,但 dcm4che、DCMTK 都用 0x0102,
/// 跟着用能避开个别实现里按具体数值判断的怪逻辑。
const IDENTIFIER_PRESENT: u16 = 0x0102;

/// DIMSE 命令类型,取自 (0000,0100) CommandField。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum CommandField {
    CStoreRq = 0x0001,
    CStoreRsp = 0x8001,
    CGetRq = 0x0010,
    CGetRsp = 0x8010,
    CFindRq = 0x0020,
    CFindRsp = 0x8020,
    CMoveRq = 0x0021,
    CMoveRsp = 0x8021,
    CEchoRq = 0x0030,
    CEchoRsp = 0x8030,
    CCancelRq = 0x0FFF,
}

impl CommandField {
    pub fn from_u16(value: u16) -> Option<Self> {
        Some(match value {
            0x0001 => Self::CStoreRq,
            0x8001 => Self::CStoreRsp,
            0x0010 => Self::CGetRq,
            0x8010 => Self::CGetRsp,
            0x0020 => Self::CFindRq,
            0x8020 => Self::CFindRsp,
            0x0021 => Self::CMoveRq,
            0x8021 => Self::CMoveRsp,
            0x0030 => Self::CEchoRq,
            0x8030 => Self::CEchoRsp,
            0x0FFF => Self::CCancelRq,
            _ => return None,
        })
    }

    pub fn as_u16(self) -> u16 {
        self as u16
    }
}

/// DIMSE 状态码 (0000,0900)。
///
/// 用 newtype 而不是枚举:标准把大段区间(`A000-AFFF`、`C000-CFFF`)留给服务类
/// 自定义,穷举不了,而且收到未知状态码时要能原样转发而不是丢掉。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Status(pub u16);

impl Status {
    pub const SUCCESS: Self = Self(0x0000);
    pub const CANCEL: Self = Self(0xFE00);
    /// C-FIND/C-MOVE/C-GET 的中间响应:还有后续。
    pub const PENDING: Self = Self(0xFF00);
    /// 同上,但有些可选键没能支持。
    pub const PENDING_WITH_WARNING: Self = Self(0xFF01);

    // —— 失败 ——
    /// 资源不足(磁盘满、内存不够)。
    pub const REFUSED_OUT_OF_RESOURCES: Self = Self(0xA700);
    /// 数据集与 SOP Class 不匹配。
    pub const DATA_SET_DOES_NOT_MATCH_SOP_CLASS: Self = Self(0xA900);
    /// 无法理解的请求 —— 解析不了命令或数据集时用它。
    pub const CANNOT_UNDERSTAND: Self = Self(0xC000);
    /// 处理失败(我们用来表示落盘或入库出错)。
    pub const PROCESSING_FAILURE: Self = Self(0x0110);
    /// SOP Class 不被支持。
    pub const SOP_CLASS_NOT_SUPPORTED: Self = Self(0x0122);
    /// 参数值非法 —— 比如 UID 校验没过。
    pub const INVALID_ARGUMENT_VALUE: Self = Self(0x0115);
    /// C-FIND:标识符与 SOP Class 对不上(层级非法、缺 QueryRetrieveLevel)。
    ///
    /// 值与 [`Self::DATA_SET_DOES_NOT_MATCH_SOP_CLASS`] 相同 —— 标准对 C-STORE
    /// 和 C-FIND 用了同一个码点、两套措辞。分成两个常量是为了让调用处读起来
    /// 对得上各自的服务类。
    pub const IDENTIFIER_DOES_NOT_MATCH_SOP_CLASS: Self = Self(0xA900);
    /// C-FIND:无法处理这次查询(标识符解不开、数据库出错)。
    pub const UNABLE_TO_PROCESS: Self = Self(0xC000);

    pub fn is_success(self) -> bool {
        self.0 == 0x0000
    }

    pub fn is_pending(self) -> bool {
        matches!(self.0, 0xFF00 | 0xFF01)
    }

    pub fn is_cancel(self) -> bool {
        self.0 == 0xFE00
    }

    /// 警告:操作完成了,但有保留意见(PS3.7 附录 C)。
    ///
    /// 对发送方来说警告和成功一样意味着「收下了」,不该重传。
    pub fn is_warning(self) -> bool {
        matches!(self.0, 0x0001 | 0x0107 | 0x0116) || (0xB000..=0xBFFF).contains(&self.0)
    }

    /// 失败:既不是成功、警告、挂起,也不是取消。
    pub fn is_failure(self) -> bool {
        !self.is_success() && !self.is_warning() && !self.is_pending() && !self.is_cancel()
    }
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "0x{:04X}", self.0)
    }
}

#[derive(Debug, Error)]
pub enum CommandError {
    // dicom-rs 的读写错误结构体有一百多字节,直接放进枚举会让每个
    // `Result<_, CommandError>` 都背上这个体积。装箱把它挪到堆上,
    // 让成功路径保持轻量 —— 命令收发在每条消息上都要走一遍。
    #[error("命令集解码失败")]
    Decode {
        #[source]
        source: Box<dicom::object::ReadError>,
    },
    #[error("命令集编码失败")]
    Encode {
        #[source]
        source: Box<dicom::object::WriteError>,
    },
    #[error("命令集缺少必需字段 {field}")]
    Missing { field: &'static str },
    #[error("未知的 CommandField 0x{value:04X}")]
    UnknownCommandField { value: u16 },
}

/// 一条 DIMSE 命令。
///
/// 内部就是一个 DICOM 数据集(0000 组),用包装类型提供带类型的读写,
/// 免得每处都手写标签和 VR。
#[derive(Debug, Clone, PartialEq)]
pub struct Command(InMemDicomObject);

impl Command {
    /// 从命令 PDV 的字节解码。
    pub fn decode(bytes: &[u8]) -> Result<Self, CommandError> {
        let object =
            InMemDicomObject::read_dataset_with_ts(bytes, implicit_vr_le()).map_err(|source| {
                CommandError::Decode {
                    source: Box::new(source),
                }
            })?;
        Ok(Self(object))
    }

    /// 编码成命令 PDV 的字节。
    pub fn encode(&self) -> Result<Vec<u8>, CommandError> {
        let mut buffer = Vec::new();
        self.0
            .write_dataset_with_ts(&mut buffer, implicit_vr_le())
            .map_err(|source| CommandError::Encode {
                source: Box::new(source),
            })?;
        Ok(buffer)
    }

    pub fn command_field(&self) -> Result<CommandField, CommandError> {
        let raw = self
            .u16_at(tags::COMMAND_FIELD)
            .ok_or(CommandError::Missing {
                field: "CommandField",
            })?;
        CommandField::from_u16(raw).ok_or(CommandError::UnknownCommandField { value: raw })
    }

    pub fn message_id(&self) -> Option<u16> {
        self.u16_at(tags::MESSAGE_ID)
    }

    pub fn message_id_being_responded_to(&self) -> Option<u16> {
        self.u16_at(tags::MESSAGE_ID_BEING_RESPONDED_TO)
    }

    pub fn affected_sop_class_uid(&self) -> Option<String> {
        self.text_at(tags::AFFECTED_SOP_CLASS_UID)
    }

    pub fn affected_sop_instance_uid(&self) -> Option<String> {
        self.text_at(tags::AFFECTED_SOP_INSTANCE_UID)
    }

    pub fn status(&self) -> Option<Status> {
        self.u16_at(tags::STATUS).map(Status)
    }

    /// 后面是否跟着数据集。
    ///
    /// 缺少 CommandDataSetType 时按「有数据集」处理:宁可去读一个不存在的数据集
    /// (会读到空并报错),也不要把真实数据集漏掉 —— 漏掉会让后续消息的字节
    /// 全部错位,整条连接从此解不出东西。
    pub fn has_data_set(&self) -> bool {
        self.u16_at(tags::COMMAND_DATA_SET_TYPE)
            .is_none_or(|value| value != NO_DATA_SET)
    }

    fn u16_at(&self, tag: dicom::core::Tag) -> Option<u16> {
        self.0.get(tag)?.to_int::<u16>().ok()
    }

    fn text_at(&self, tag: dicom::core::Tag) -> Option<String> {
        let raw = self.0.get(tag)?.to_str().ok()?;
        let trimmed = raw.trim_matches(|c: char| c == '\0' || c.is_whitespace());
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    }

    /// 底层数据集,用于测试和调试。
    pub fn as_object(&self) -> &InMemDicomObject {
        &self.0
    }
}

/// C-ECHO-RSP。
pub fn c_echo_rsp(request: &Command, status: Status) -> Command {
    Command(InMemDicomObject::command_from_element_iter([
        element_ui(
            tags::AFFECTED_SOP_CLASS_UID,
            &request
                .affected_sop_class_uid()
                .unwrap_or_else(|| uids::VERIFICATION.to_owned()),
        ),
        element_us(tags::COMMAND_FIELD, CommandField::CEchoRsp.as_u16()),
        element_us(
            tags::MESSAGE_ID_BEING_RESPONDED_TO,
            request.message_id().unwrap_or(0),
        ),
        element_us(tags::COMMAND_DATA_SET_TYPE, NO_DATA_SET),
        element_us(tags::STATUS, status.0),
    ]))
}

/// C-STORE-RSP。
///
/// 响应里要回带请求中的 AffectedSOPClassUID 和 AffectedSOPInstanceUID,
/// 发送方靠它们把响应对上自己发的是哪一份影像。
pub fn c_store_rsp(request: &Command, status: Status) -> Command {
    let mut elements = vec![
        element_us(tags::COMMAND_FIELD, CommandField::CStoreRsp.as_u16()),
        element_us(
            tags::MESSAGE_ID_BEING_RESPONDED_TO,
            request.message_id().unwrap_or(0),
        ),
        element_us(tags::COMMAND_DATA_SET_TYPE, NO_DATA_SET),
        element_us(tags::STATUS, status.0),
    ];
    if let Some(uid) = request.affected_sop_class_uid() {
        elements.push(element_ui(tags::AFFECTED_SOP_CLASS_UID, &uid));
    }
    if let Some(uid) = request.affected_sop_instance_uid() {
        elements.push(element_ui(tags::AFFECTED_SOP_INSTANCE_UID, &uid));
    }
    // 命令集元素必须按标签升序,否则接收方的顺序解析器会读错
    elements.sort_by_key(|element| element.header().tag);
    Command(InMemDicomObject::command_from_element_iter(elements))
}

/// C-ECHO-RQ,目前只有测试用得到(阶段 8 做 C-MOVE 时会需要发起端)。
pub fn c_echo_rq(message_id: u16) -> Command {
    Command(InMemDicomObject::command_from_element_iter([
        element_ui(tags::AFFECTED_SOP_CLASS_UID, uids::VERIFICATION),
        element_us(tags::COMMAND_FIELD, CommandField::CEchoRq.as_u16()),
        element_us(tags::MESSAGE_ID, message_id),
        element_us(tags::COMMAND_DATA_SET_TYPE, NO_DATA_SET),
    ]))
}

/// C-STORE-RQ,测试用。
pub fn c_store_rq(message_id: u16, sop_class_uid: &str, sop_instance_uid: &str) -> Command {
    Command(InMemDicomObject::command_from_element_iter([
        element_ui(tags::AFFECTED_SOP_CLASS_UID, sop_class_uid),
        element_us(tags::COMMAND_FIELD, CommandField::CStoreRq.as_u16()),
        element_us(tags::MESSAGE_ID, message_id),
        element_us(tags::PRIORITY, 0x0000),
        element_us(tags::COMMAND_DATA_SET_TYPE, DATA_SET_PRESENT),
        element_ui(tags::AFFECTED_SOP_INSTANCE_UID, sop_instance_uid),
    ]))
}

/// C-FIND-RSP。
///
/// 一次 C-FIND 会回多条:每条命中回一条 `PENDING` 带标识符数据集,
/// 最后一条回 `SUCCESS`(或失败/取消状态)且**不带数据集**。
/// 对端靠"状态不再是 pending"判断查询结束,所以最后那条的状态码不能填错。
pub fn c_find_rsp(request: &Command, status: Status, has_identifier: bool) -> Command {
    let mut elements = vec![
        element_us(tags::COMMAND_FIELD, CommandField::CFindRsp.as_u16()),
        element_us(
            tags::MESSAGE_ID_BEING_RESPONDED_TO,
            request.message_id().unwrap_or(0),
        ),
        element_us(
            tags::COMMAND_DATA_SET_TYPE,
            if has_identifier {
                IDENTIFIER_PRESENT
            } else {
                NO_DATA_SET
            },
        ),
        element_us(tags::STATUS, status.0),
    ];
    if let Some(uid) = request.affected_sop_class_uid() {
        elements.push(element_ui(tags::AFFECTED_SOP_CLASS_UID, &uid));
    }
    elements.sort_by_key(|element| element.header().tag);
    Command(InMemDicomObject::command_from_element_iter(elements))
}

/// C-FIND-RQ,测试用。
pub fn c_find_rq(message_id: u16, sop_class_uid: &str) -> Command {
    Command(InMemDicomObject::command_from_element_iter([
        element_ui(tags::AFFECTED_SOP_CLASS_UID, sop_class_uid),
        element_us(tags::COMMAND_FIELD, CommandField::CFindRq.as_u16()),
        element_us(tags::MESSAGE_ID, message_id),
        element_us(tags::PRIORITY, 0x0000),
        element_us(tags::COMMAND_DATA_SET_TYPE, IDENTIFIER_PRESENT),
    ]))
}

fn element_us(tag: dicom::core::Tag, value: u16) -> dicom::object::mem::InMemElement {
    DataElement::new(tag, VR::US, PrimitiveValue::from(value))
}

fn element_ui(tag: dicom::core::Tag, value: &str) -> dicom::object::mem::InMemElement {
    DataElement::new(tag, VR::UI, PrimitiveValue::from(value))
}

/// 命令集专用的传输语法。见模块文档:与协商结果无关,固定隐式 VR LE。
fn implicit_vr_le() -> &'static TransferSyntax {
    TransferSyntaxRegistry
        .get(uids::IMPLICIT_VR_LITTLE_ENDIAN)
        .expect("传输语法注册表必然包含隐式 VR Little Endian")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_classification_covers_each_category() {
        assert!(Status::SUCCESS.is_success());
        assert!(Status::PENDING.is_pending());
        assert!(Status::PENDING_WITH_WARNING.is_pending());
        assert!(Status::CANCEL.is_cancel());

        // 警告区间:操作完成了,发送方不该重传
        assert!(Status(0xB000).is_warning());
        assert!(Status(0xBFFF).is_warning());
        assert!(!Status(0xB000).is_failure());

        // 失败区间
        assert!(Status::CANNOT_UNDERSTAND.is_failure());
        assert!(Status::REFUSED_OUT_OF_RESOURCES.is_failure());
        assert!(Status::PROCESSING_FAILURE.is_failure());

        // 四类互斥
        for raw in [0x0000, 0x0001, 0xB000, 0xA700, 0xC000, 0xFE00, 0xFF00] {
            let status = Status(raw);
            let categories = [
                status.is_success(),
                status.is_warning(),
                status.is_failure(),
                status.is_pending(),
                status.is_cancel(),
            ];
            assert_eq!(
                categories.iter().filter(|hit| **hit).count(),
                1,
                "{status} 应恰好归入一类"
            );
        }
    }

    #[test]
    fn command_field_round_trips() {
        for field in [
            CommandField::CStoreRq,
            CommandField::CStoreRsp,
            CommandField::CEchoRq,
            CommandField::CEchoRsp,
            CommandField::CFindRq,
            CommandField::CMoveRq,
            CommandField::CCancelRq,
        ] {
            assert_eq!(CommandField::from_u16(field.as_u16()), Some(field));
        }
        assert_eq!(CommandField::from_u16(0x1234), None);
    }

    #[test]
    fn echo_request_encodes_and_decodes() {
        let encoded = c_echo_rq(7).encode().expect("应能编码");
        let decoded = Command::decode(&encoded).expect("应能解码");

        assert_eq!(decoded.command_field().unwrap(), CommandField::CEchoRq);
        assert_eq!(decoded.message_id(), Some(7));
        assert_eq!(
            decoded.affected_sop_class_uid().as_deref(),
            Some(uids::VERIFICATION)
        );
        assert!(!decoded.has_data_set());
    }

    #[test]
    fn echo_response_answers_the_request() {
        let request = Command::decode(&c_echo_rq(42).encode().unwrap()).unwrap();
        let response = c_echo_rsp(&request, Status::SUCCESS);
        let decoded = Command::decode(&response.encode().unwrap()).unwrap();

        assert_eq!(decoded.command_field().unwrap(), CommandField::CEchoRsp);
        // 发送方靠这个字段把响应对上请求
        assert_eq!(decoded.message_id_being_responded_to(), Some(42));
        assert_eq!(decoded.status(), Some(Status::SUCCESS));
        assert!(!decoded.has_data_set());
    }

    #[test]
    fn store_response_echoes_back_the_instance_identity() {
        let request = Command::decode(
            &c_store_rq(3, uids::CT_IMAGE_STORAGE, "1.2.3.4")
                .encode()
                .unwrap(),
        )
        .unwrap();
        assert!(request.has_data_set(), "C-STORE-RQ 后面必然跟数据集");

        let response =
            Command::decode(&c_store_rsp(&request, Status::SUCCESS).encode().unwrap()).unwrap();
        assert_eq!(response.command_field().unwrap(), CommandField::CStoreRsp);
        assert_eq!(response.message_id_being_responded_to(), Some(3));
        assert_eq!(
            response.affected_sop_class_uid().as_deref(),
            Some(uids::CT_IMAGE_STORAGE)
        );
        assert_eq!(
            response.affected_sop_instance_uid().as_deref(),
            Some("1.2.3.4"),
            "发送方靠它判断是哪一份影像存成功了"
        );
        assert!(!response.has_data_set());
    }

    /// CommandDataSetType 不是布尔量,只有 0x0101 表示「没有数据集」。
    #[test]
    fn data_set_presence_uses_the_magic_null_value() {
        let with_data = Command(InMemDicomObject::command_from_element_iter([element_us(
            tags::COMMAND_DATA_SET_TYPE,
            0x0000,
        )]));
        assert!(with_data.has_data_set());

        // 非 0 也非 0x0101 —— 仍然表示有数据集
        let odd = Command(InMemDicomObject::command_from_element_iter([element_us(
            tags::COMMAND_DATA_SET_TYPE,
            0x0001,
        )]));
        assert!(odd.has_data_set());

        let without = Command(InMemDicomObject::command_from_element_iter([element_us(
            tags::COMMAND_DATA_SET_TYPE,
            NO_DATA_SET,
        )]));
        assert!(!without.has_data_set());

        // 字段缺失时按「有」处理,漏读数据集会让整条连接的字节错位
        let missing = Command(InMemDicomObject::command_from_element_iter([]));
        assert!(missing.has_data_set());
    }

    /// 命令集必须是隐式 VR:用显式 VR 解会得到垃圾,这正是最容易搞错的地方。
    #[test]
    fn command_set_is_encoded_as_implicit_vr() {
        let encoded = c_echo_rq(1).encode().unwrap();

        // 隐式 VR 的元素头是 4 字节标签 + 4 字节长度,不含 VR 字面量。
        // 第一个元素是 (0000,0000) CommandGroupLength,长度 4。
        assert_eq!(
            &encoded[0..4],
            &[0x00, 0x00, 0x00, 0x00],
            "组号元素号应为 0"
        );
        assert_eq!(
            &encoded[4..8],
            &[0x04, 0x00, 0x00, 0x00],
            "隐式 VR 下这里应是 4 字节长度,而不是 VR 字面量"
        );

        // 用显式 VR 解同一段字节应当失败或解出不同结果 —— 证明两者确实不兼容
        let as_explicit = InMemDicomObject::read_dataset_with_ts(
            &encoded[..],
            TransferSyntaxRegistry
                .get(uids::EXPLICIT_VR_LITTLE_ENDIAN)
                .unwrap(),
        );
        let misread = as_explicit.ok().and_then(|obj| {
            obj.get(tags::MESSAGE_ID)
                .and_then(|e| e.to_int::<u16>().ok())
        });
        assert_ne!(misread, Some(1), "按显式 VR 解不该恰好得到正确的 MessageID");
    }

    /// CommandGroupLength (0000,0000) 必须等于其后所有元素的字节数,
    /// 接收方按它判断命令集到哪儿结束。
    #[test]
    fn command_group_length_matches_the_payload() {
        let encoded = c_store_rq(1, uids::CT_IMAGE_STORAGE, "1.2.840.10008.1.1")
            .encode()
            .unwrap();

        let declared = u32::from_le_bytes([encoded[8], encoded[9], encoded[10], encoded[11]]);
        let actual = encoded.len() - 12; // 减去组长度元素自身(8 字节头 + 4 字节值)
        assert_eq!(declared as usize, actual, "组长度应等于其后实际字节数");
    }
}
