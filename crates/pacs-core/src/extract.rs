//! 从 DICOM 对象提取四层元数据。
//!
//! 容错策略是刻意分成两档的:
//!
//! - **三个 UID 和传输语法缺失或非法 → 硬错误**。它们是主键和解码依据,
//!   没有它们这个实例既存不进库也取不回来,必须让 C-STORE 回错误码而不是
//!   悄悄存一条残缺记录。
//! - **其余属性解析失败 → 记为 `None`,不阻断入库**。真实设备产出的头信息
//!   到处是空值和不合规格式,为了一个畸形的 StudyDate 丢掉整个检查是不可接受的。
//!   原始值仍然保留在该层的 `attributes` JSON 里,不会丢失。

use chrono::{NaiveDate, NaiveTime};
use dicom::core::value::Value as DicomValue;
use dicom::core::{DataElement, PrimitiveValue, Tag};
use dicom::dictionary_std::tags;
use dicom::object::mem::InMemElement;
use dicom::object::{DefaultDicomObject, InMemDicomObject};
use serde_json::Value;
use thiserror::Error;

use crate::attributes;
use crate::model::{
    InstanceMeta, InstanceMetadata, PatientMeta, SeriesMeta, StudyMeta, normalize_person_name,
};
use crate::text::{normalized_text_element, utf8_text};
use crate::uid::{Uid, UidError};

/// 元数据提取失败。只在缺少入库必需项时产生。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ExtractError {
    #[error("缺少必需属性 {field}")]
    Missing { field: &'static str },
    #[error("{field} 不是合法 UID:{source}")]
    InvalidUid {
        field: &'static str,
        #[source]
        source: UidError,
    },
}

/// 解析出四层元数据。
pub fn extract_metadata(obj: &DefaultDicomObject) -> Result<InstanceMetadata, ExtractError> {
    let study_uid = required_uid(obj, tags::STUDY_INSTANCE_UID, "StudyInstanceUID")?;
    let series_uid = required_uid(obj, tags::SERIES_INSTANCE_UID, "SeriesInstanceUID")?;
    let instance_uid = sop_instance_uid(obj)?;
    let transfer_syntax_uid = parse_uid(obj.meta().transfer_syntax(), "TransferSyntaxUID")?;

    let name = text(obj, tags::PATIENT_NAME);
    let patient = PatientMeta {
        // PatientID 是 Type 2:可以为空。空串表示"设备没给",不是错误。
        patient_id: text(obj, tags::PATIENT_ID).unwrap_or_default(),
        issuer_of_patient_id: text(obj, tags::ISSUER_OF_PATIENT_ID),
        name_normalized: name.as_deref().map(normalize_person_name),
        name,
        birth_date: date(obj, tags::PATIENT_BIRTH_DATE),
        sex: text(obj, tags::PATIENT_SEX),
        attributes: subset(obj, attributes::PATIENT),
    };

    let study = StudyMeta {
        uid: study_uid,
        date: date(obj, tags::STUDY_DATE),
        time: time(obj, tags::STUDY_TIME),
        accession_number: text(obj, tags::ACCESSION_NUMBER),
        study_id: text(obj, tags::STUDY_ID),
        description: text(obj, tags::STUDY_DESCRIPTION),
        referring_physician: text(obj, tags::REFERRING_PHYSICIAN_NAME),
        attributes: subset(obj, attributes::STUDY),
    };

    let series = SeriesMeta {
        uid: series_uid,
        number: int(obj, tags::SERIES_NUMBER),
        modality: text(obj, tags::MODALITY),
        description: text(obj, tags::SERIES_DESCRIPTION),
        body_part_examined: text(obj, tags::BODY_PART_EXAMINED),
        date: date(obj, tags::SERIES_DATE),
        time: time(obj, tags::SERIES_TIME),
        attributes: subset(obj, attributes::SERIES),
    };

    let instance = InstanceMeta {
        uid: instance_uid,
        sop_class_uid: optional_uid(obj, tags::SOP_CLASS_UID),
        number: int(obj, tags::INSTANCE_NUMBER),
        transfer_syntax_uid,
        rows: int(obj, tags::ROWS),
        columns: int(obj, tags::COLUMNS),
        number_of_frames: int(obj, tags::NUMBER_OF_FRAMES),
        image_position_patient: floats(obj, tags::IMAGE_POSITION_PATIENT),
        image_orientation_patient: floats(obj, tags::IMAGE_ORIENTATION_PATIENT),
        attributes: subset(obj, attributes::INSTANCE),
    };

    Ok(InstanceMetadata {
        patient,
        study,
        series,
        instance,
    })
}

/// SOPInstanceUID 取数据集里的值;数据集没有时回退到文件元信息。
///
/// 标准要求 (0008,0018) 与元信息的 MediaStorageSOPInstanceUID 一致。两者不一致
/// 说明文件被改写过或写入方有 bug,此时以数据集为准(和 dcm4che 一致)并告警。
fn sop_instance_uid(obj: &DefaultDicomObject) -> Result<Uid, ExtractError> {
    let from_meta = Uid::parse(obj.meta().media_storage_sop_instance_uid()).ok();

    let Some(from_dataset) = text(obj, tags::SOP_INSTANCE_UID) else {
        let uid = from_meta.ok_or(ExtractError::Missing {
            field: "SOPInstanceUID",
        })?;
        tracing::warn!(%uid, "数据集缺少 SOPInstanceUID,回退到文件元信息");
        return Ok(uid);
    };

    let uid = parse_uid(&from_dataset, "SOPInstanceUID")?;
    if from_meta.as_ref().is_some_and(|meta| meta != &uid) {
        tracing::warn!(
            dataset = %uid,
            meta = %from_meta.expect("刚判断过是 Some"),
            "SOPInstanceUID 与文件元信息不一致,以数据集为准"
        );
    }
    Ok(uid)
}

fn parse_uid(raw: &str, field: &'static str) -> Result<Uid, ExtractError> {
    Uid::parse(raw).map_err(|source| ExtractError::InvalidUid { field, source })
}

fn required_uid(
    obj: &DefaultDicomObject,
    tag: Tag,
    field: &'static str,
) -> Result<Uid, ExtractError> {
    let raw = text(obj, tag).ok_or(ExtractError::Missing { field })?;
    parse_uid(&raw, field)
}

fn optional_uid(obj: &DefaultDicomObject, tag: Tag) -> Option<Uid> {
    Uid::parse(&text(obj, tag)?).ok()
}

/// 读一个字符串属性。缺失、无法转字符串、或值为空白一律当作没有。
fn text(obj: &DefaultDicomObject, tag: Tag) -> Option<String> {
    utf8_text(obj, tag)
}

fn int(obj: &DefaultDicomObject, tag: Tag) -> Option<i32> {
    obj.get(tag)?.to_int::<i32>().ok()
}

fn floats(obj: &DefaultDicomObject, tag: Tag) -> Option<Vec<f64>> {
    let values = obj.get(tag)?.to_multi_float64().ok()?;
    (!values.is_empty()).then_some(values)
}

/// 只在日期精确到「日」时才落库列。
///
/// DICOM 允许部分精度(只有年、或年月)。把 `2024` 补成 `2024-01-01` 会凭空造出
/// 精度,并让 C-FIND 的日期范围匹配产生假命中 —— 宁可留 NULL,原始值在
/// `attributes` 里还在。
fn date(obj: &DefaultDicomObject, tag: Tag) -> Option<NaiveDate> {
    let value = obj.get(tag)?.to_date().ok()?;
    NaiveDate::from_ymd_opt(
        i32::from(*value.year()),
        u32::from(*value.month()?),
        u32::from(*value.day()?),
    )
}

/// 时间至少要精确到「分」;秒和小数秒缺失时按 0 处理。
///
/// 与日期不同,时间列只用于显示和同日排序,不参与范围匹配,补零的风险可以接受。
fn time(obj: &DefaultDicomObject, tag: Tag) -> Option<NaiveTime> {
    let value = obj.get(tag)?.to_time().ok()?;
    NaiveTime::from_hms_micro_opt(
        u32::from(*value.hour()),
        u32::from(*value.minute()?),
        u32::from(value.second().copied().unwrap_or(0)),
        value.fraction_micro().unwrap_or(0),
    )
}

/// 按标签清单抽出该层属性,序列化成 DICOM JSON Model(PS3.18 附录 F)。
fn subset(obj: &DefaultDicomObject, tags: &[Tag]) -> Value {
    let elements: Vec<_> = tags
        .iter()
        .filter_map(|tag| obj.get(*tag))
        .map(|element| normalized_text_element(obj, element))
        .map(|element| trim_padding(&element).unwrap_or(element))
        .collect();
    let subset = InMemDicomObject::from_element_iter(elements);
    dicom_json::to_value(subset).unwrap_or_else(|error| {
        // 属性子集存不下不该拖垮入库:结构化列和文件本身仍然是完整的。
        tracing::warn!(%error, "属性子集序列化失败,该层 attributes 置空");
        Value::Object(serde_json::Map::new())
    })
}

/// 去掉 DICOM 的偶数长度补齐,不是字符串值则返回 `None`。
///
/// DICOM 要求每个值占偶数字节,奇数时在尾部补一个空格(UI 补 NUL)。解析器会把
/// 补齐字符一并保留,直接序列化出去就成了 `"2 "` 而不是 `"2"` —— QIDO-RS 客户端
/// 拿去做等值比较会失配。PS3.18 F.2.3 要求 JSON 值不带补齐。
///
/// 只去尾部:LT/ST/UT 这类文本 VR 的前导空格是有意义的排版内容。
fn trim_padding(element: &InMemElement) -> Option<InMemElement> {
    let trimmed = |s: &str| s.trim_end_matches([' ', '\0']).to_owned();
    let value = match element.value() {
        DicomValue::Primitive(PrimitiveValue::Str(s)) => PrimitiveValue::Str(trimmed(s)),
        DicomValue::Primitive(PrimitiveValue::Strs(items)) => {
            PrimitiveValue::Strs(items.iter().map(|s| trimmed(s)).collect())
        }
        _ => return None,
    };
    Some(DataElement::new(element.header().tag, element.vr(), value))
}
