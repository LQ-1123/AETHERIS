//! Pure DICOM tag transformation engine.
//!
//! This module does not know about HTTP, Postgres, or storage paths. It transforms an in-memory
//! Part 10 object while enforcing clinical correction invariants: protected pixel/identity
//! fields cannot be edited directly, UID references are remapped consistently, and PixelData is
//! byte-identical.

use std::collections::HashMap;

use chrono::NaiveDate;
use dicom::core::dictionary::{DataDictionary, DataDictionaryEntry};
use dicom::core::header::Header;
use dicom::core::value::{DataSetSequence, Value as DicomValue};
use dicom::core::{DataElement, PrimitiveValue, Tag, VR};
use dicom::dictionary_std::{StandardDataDictionary, tags};
use dicom::object::{DefaultDicomObject, InMemDicomObject};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{InstanceMetadata, extract_metadata, normalize_file_text};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TagScope {
    Patient,
    Study,
    Series,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManualTagSpec {
    pub keyword: &'static str,
    pub tag: Tag,
    pub vr: VR,
    pub scope: TagScope,
}

const MANUAL_TAGS: &[ManualTagSpec] = &[
    ManualTagSpec {
        keyword: "PatientName",
        tag: tags::PATIENT_NAME,
        vr: VR::PN,
        scope: TagScope::Patient,
    },
    ManualTagSpec {
        keyword: "PatientID",
        tag: tags::PATIENT_ID,
        vr: VR::LO,
        scope: TagScope::Patient,
    },
    ManualTagSpec {
        keyword: "IssuerOfPatientID",
        tag: tags::ISSUER_OF_PATIENT_ID,
        vr: VR::LO,
        scope: TagScope::Patient,
    },
    ManualTagSpec {
        keyword: "PatientBirthDate",
        tag: tags::PATIENT_BIRTH_DATE,
        vr: VR::DA,
        scope: TagScope::Patient,
    },
    ManualTagSpec {
        keyword: "PatientSex",
        tag: tags::PATIENT_SEX,
        vr: VR::CS,
        scope: TagScope::Patient,
    },
    ManualTagSpec {
        keyword: "AccessionNumber",
        tag: tags::ACCESSION_NUMBER,
        vr: VR::SH,
        scope: TagScope::Study,
    },
    ManualTagSpec {
        keyword: "StudyID",
        tag: tags::STUDY_ID,
        vr: VR::SH,
        scope: TagScope::Study,
    },
    ManualTagSpec {
        keyword: "StudyDescription",
        tag: tags::STUDY_DESCRIPTION,
        vr: VR::LO,
        scope: TagScope::Study,
    },
    ManualTagSpec {
        keyword: "ReferringPhysicianName",
        tag: tags::REFERRING_PHYSICIAN_NAME,
        vr: VR::PN,
        scope: TagScope::Study,
    },
    ManualTagSpec {
        keyword: "SeriesDescription",
        tag: tags::SERIES_DESCRIPTION,
        vr: VR::LO,
        scope: TagScope::Series,
    },
    ManualTagSpec {
        keyword: "SeriesNumber",
        tag: tags::SERIES_NUMBER,
        vr: VR::IS,
        scope: TagScope::Series,
    },
    ManualTagSpec {
        keyword: "BodyPartExamined",
        tag: tags::BODY_PART_EXAMINED,
        vr: VR::CS,
        scope: TagScope::Series,
    },
    ManualTagSpec {
        keyword: "ProtocolName",
        tag: tags::PROTOCOL_NAME,
        vr: VR::LO,
        scope: TagScope::Series,
    },
];

pub fn manual_tag_specs() -> &'static [ManualTagSpec] {
    MANUAL_TAGS
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum RuleAction {
    Replace { value: String },
    Remove,
    Empty,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagRule {
    /// DICOM keyword or `(gggg,eeee)` tag expression.
    pub tag: String,
    #[serde(flatten)]
    pub action: RuleAction,
    #[serde(default)]
    pub recursive: bool,
}

#[derive(Debug, Clone, Default)]
pub struct TransformContext {
    /// Mapping from every source UID in the target graph to its derived UID.
    pub uid_map: HashMap<String, String>,
    pub derivation_description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagDiff {
    pub tag: String,
    pub keyword: String,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    pub action: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PixelRiskLevel {
    Safe,
    ReviewRequired,
    Blocking,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PixelRisk {
    pub level: PixelRiskLevel,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct TransformOutcome {
    pub metadata: InstanceMetadata,
    pub diffs: Vec<TagDiff>,
    pub pixel_sha256: Option<[u8; 32]>,
    pub pixel_risk: PixelRisk,
}

#[derive(Debug, Error)]
pub enum TransformError {
    #[error("未知 DICOM 标签 {0}")]
    UnknownTag(String),
    #[error("标签 {0} 不在临床手工修改白名单中")]
    NotWhitelisted(String),
    #[error("标签 {tag} 属于 {actual:?} 层，不能用于 {requested:?} 层修改")]
    WrongScope {
        tag: String,
        actual: TagScope,
        requested: TagScope,
    },
    #[error("标签 {tag} 的值无效: {reason}")]
    InvalidValue { tag: String, reason: String },
    #[error("受保护标签 {0} 不允许直接修改")]
    ProtectedTag(String),
    #[error("转换改变了 PixelData")]
    PixelDataChanged,
    #[error("转换后元数据无效: {0}")]
    InvalidMetadata(#[from] crate::ExtractError),
}

pub fn validate_manual_rules(rules: &[TagRule], scope: TagScope) -> Result<(), TransformError> {
    if rules.is_empty() {
        return Err(TransformError::InvalidValue {
            tag: "rules".to_owned(),
            reason: "至少需要一项修改".to_owned(),
        });
    }
    for rule in rules {
        let spec = MANUAL_TAGS
            .iter()
            .find(|spec| spec.keyword == rule.tag || format_tag(spec.tag) == rule.tag)
            .ok_or_else(|| TransformError::NotWhitelisted(rule.tag.clone()))?;
        if spec.scope != scope {
            return Err(TransformError::WrongScope {
                tag: spec.keyword.to_owned(),
                actual: spec.scope,
                requested: scope,
            });
        }
        if rule.recursive {
            return Err(TransformError::InvalidValue {
                tag: spec.keyword.to_owned(),
                reason: "手工修改不允许递归规则".to_owned(),
            });
        }
        match &rule.action {
            RuleAction::Replace { value } => validate_text_value(spec.keyword, spec.vr, value)?,
            RuleAction::Empty | RuleAction::Remove => {}
        }
    }
    Ok(())
}

/// Apply tag rules and the task-wide UID graph to a Part 10 object.
///
/// `apply_rules` is false for instances pulled into a clinical transformation only because their
/// enclosing Study UID must be remapped. These instances receive UID/source derivation changes but
/// not the user-selected field edits.
pub fn apply_transform(
    object: &mut DefaultDicomObject,
    rules: &[TagRule],
    context: &TransformContext,
    apply_rules: bool,
) -> Result<TransformOutcome, TransformError> {
    let source_sop = object
        .get(tags::SOP_INSTANCE_UID)
        .and_then(|element| element.to_str().ok())
        .map(|value| trim_dicom(&value).to_owned());
    let source_class = object
        .get(tags::SOP_CLASS_UID)
        .and_then(|element| element.to_str().ok())
        .map(|value| trim_dicom(&value).to_owned());
    let pixel_before = pixel_data_sha256(object);

    // All derived files are encoded as declared UTF-8 text. This also applies the resilient
    // charset fallback before a user value is compared or replaced.
    normalize_file_text(object);

    let mut diffs = Vec::new();
    if apply_rules {
        for rule in rules {
            apply_rule(object, rule, &mut diffs)?;
        }
    }
    remap_uids(object, &context.uid_map, &mut diffs);

    if let (Some(source_sop), Some(source_class)) = (source_sop, source_class) {
        let item = InMemDicomObject::from_element_iter([
            DataElement::new(tags::REFERENCED_SOP_CLASS_UID, VR::UI, source_class),
            DataElement::new(tags::REFERENCED_SOP_INSTANCE_UID, VR::UI, source_sop),
        ]);
        object.put(DataElement::new(
            tags::SOURCE_IMAGE_SEQUENCE,
            VR::SQ,
            DataSetSequence::from(vec![item]),
        ));
    }
    object.put_str(
        tags::DERIVATION_DESCRIPTION,
        VR::ST,
        if context.derivation_description.trim().is_empty() {
            "remote_pacs derived revision"
        } else {
            context.derivation_description.trim()
        },
    );

    if let Some(new_sop) = object
        .get(tags::SOP_INSTANCE_UID)
        .and_then(|element| element.to_str().ok())
        .map(|value| trim_dicom(&value).to_owned())
    {
        object.update_meta(|meta| meta.media_storage_sop_instance_uid = new_sop);
    }

    let pixel_after = pixel_data_sha256(object);
    if pixel_before != pixel_after {
        return Err(TransformError::PixelDataChanged);
    }
    let pixel_risk = classify_pixel_risk(object);
    let metadata = extract_metadata(object)?;
    Ok(TransformOutcome {
        metadata,
        diffs,
        pixel_sha256: pixel_after,
        pixel_risk,
    })
}

fn apply_rule(
    object: &mut InMemDicomObject,
    rule: &TagRule,
    diffs: &mut Vec<TagDiff>,
) -> Result<(), TransformError> {
    let dictionary = StandardDataDictionary;
    let entry = dictionary
        .by_expr(&rule.tag)
        .ok_or_else(|| TransformError::UnknownTag(rule.tag.clone()))?;
    let tag = entry.tag();
    if protected_tag(tag) {
        return Err(TransformError::ProtectedTag(rule.tag.clone()));
    }

    apply_rule_here(
        object,
        tag,
        entry.alias(),
        entry.vr().relaxed(),
        rule,
        diffs,
    )?;
    if rule.recursive {
        let sequence_tags: Vec<Tag> = object
            .iter()
            .filter(|element| element.value().items().is_some())
            .map(|element| element.tag())
            .collect();
        for sequence_tag in sequence_tags {
            let mut nested_error = None;
            object.update_value(sequence_tag, |value| {
                if let Some(items) = value.items_mut() {
                    for item in items {
                        if nested_error.is_none() {
                            nested_error = apply_rule(item, rule, diffs).err();
                        }
                    }
                }
            });
            if let Some(error) = nested_error {
                return Err(error);
            }
        }
    }
    Ok(())
}

fn apply_rule_here(
    object: &mut InMemDicomObject,
    tag: Tag,
    keyword: &str,
    dictionary_vr: VR,
    rule: &TagRule,
    diffs: &mut Vec<TagDiff>,
) -> Result<(), TransformError> {
    let old = element_text(object, tag);
    let existing_vr = object
        .get(tag)
        .map(|element| element.vr())
        .unwrap_or(dictionary_vr);
    match &rule.action {
        RuleAction::Replace { value } => {
            validate_text_value(keyword, existing_vr, value)?;
            object.put_str(tag, existing_vr, value.clone());
        }
        RuleAction::Remove => {
            object.remove_element(tag);
        }
        RuleAction::Empty => {
            object.put(DataElement::new(tag, existing_vr, PrimitiveValue::Empty));
        }
    }
    let new = element_text(object, tag);
    if old != new {
        diffs.push(TagDiff {
            tag: format_tag(tag),
            keyword: keyword.to_owned(),
            old_value: old,
            new_value: new,
            action: action_name(&rule.action).to_owned(),
        });
    }
    Ok(())
}

fn remap_uids(
    object: &mut InMemDicomObject,
    uid_map: &HashMap<String, String>,
    diffs: &mut Vec<TagDiff>,
) {
    let uid_tags: Vec<Tag> = object
        .iter()
        .filter(|element| element.vr() == VR::UI)
        .map(|element| element.tag())
        .collect();
    for tag in uid_tags {
        remap_uid_element(object, tag, uid_map, diffs);
    }
    let sequence_tags: Vec<Tag> = object
        .iter()
        .filter(|element| element.value().items().is_some())
        .map(|element| element.tag())
        .collect();
    for tag in sequence_tags {
        object.update_value(tag, |value| {
            if let Some(items) = value.items_mut() {
                for item in items {
                    remap_uids(item, uid_map, diffs);
                }
            }
        });
    }
}

fn remap_uid_element(
    object: &mut InMemDicomObject,
    tag: Tag,
    uid_map: &HashMap<String, String>,
    diffs: &mut Vec<TagDiff>,
) {
    let Some(element) = object.get(tag) else {
        return;
    };
    let old = element_text(object, tag);
    let Some(primitive) = element.value().primitive() else {
        return;
    };
    let mapped = match primitive {
        PrimitiveValue::Str(value) => {
            let source = trim_dicom(value);
            uid_map
                .get(source)
                .map(|value| PrimitiveValue::Str(value.clone()))
        }
        PrimitiveValue::Strs(values) => {
            let mut changed = false;
            let output: Vec<String> = values
                .iter()
                .map(|value| {
                    let source = trim_dicom(value);
                    if let Some(mapped) = uid_map.get(source) {
                        changed = true;
                        mapped.clone()
                    } else {
                        source.to_owned()
                    }
                })
                .collect();
            changed.then(|| PrimitiveValue::Strs(output.into()))
        }
        _ => None,
    };
    if let Some(mapped) = mapped {
        object.put(DataElement::new(tag, VR::UI, mapped));
        let new = element_text(object, tag);
        if old != new {
            let dictionary = StandardDataDictionary;
            diffs.push(TagDiff {
                tag: format_tag(tag),
                keyword: dictionary
                    .by_tag(tag)
                    .map(|entry| entry.alias().to_owned())
                    .unwrap_or_else(|| "UID".to_owned()),
                old_value: old,
                new_value: new,
                action: "uid_remap".to_owned(),
            });
        }
    }
}

fn validate_text_value(keyword: &str, vr: VR, value: &str) -> Result<(), TransformError> {
    if value.contains('\\') {
        return invalid(keyword, "该字段只允许单值，不能包含反斜杠");
    }
    if value
        .chars()
        .any(|character| character == '\0' || character.is_control())
    {
        return invalid(keyword, "不能包含控制字符");
    }
    let max = match vr {
        VR::AE | VR::CS | VR::DS | VR::SH => Some(16),
        VR::AS => Some(4),
        VR::DA => Some(8),
        VR::DT => Some(26),
        VR::IS => Some(12),
        VR::LO => Some(64),
        VR::LT => Some(10_240),
        VR::PN => Some(192),
        VR::ST => Some(1_024),
        VR::UI => Some(64),
        _ => None,
    };
    if max.is_some_and(|max| value.chars().count() > max) {
        return invalid(
            keyword,
            &format!("长度超过 {max} 个字符", max = max.unwrap()),
        );
    }
    match vr {
        VR::DA if !value.is_empty() => {
            if value.len() != 8 || NaiveDate::parse_from_str(value, "%Y%m%d").is_err() {
                return invalid(keyword, "日期必须为有效的 YYYYMMDD");
            }
        }
        VR::IS if !value.is_empty() => {
            value
                .parse::<i32>()
                .map_err(|_| TransformError::InvalidValue {
                    tag: keyword.to_owned(),
                    reason: "必须是整数".to_owned(),
                })?;
        }
        VR::CS => {
            if value.chars().any(|character| {
                !(character.is_ascii_uppercase()
                    || character.is_ascii_digit()
                    || matches!(character, ' ' | '_'))
            }) {
                return invalid(keyword, "CS 只允许大写 ASCII 字母、数字、空格和下划线");
            }
            if keyword == "PatientSex" && !matches!(value, "" | "M" | "F" | "O") {
                return invalid(keyword, "PatientSex 只允许 M、F、O 或空值");
            }
        }
        VR::PN
            if value.split('=').count() > 3
                || value.split('=').any(|group| group.split('^').count() > 5) =>
        {
            return invalid(keyword, "PN 最多三组字符表示，每组最多五个分量");
        }
        _ => {}
    }
    Ok(())
}

fn invalid<T>(keyword: &str, reason: &str) -> Result<T, TransformError> {
    Err(TransformError::InvalidValue {
        tag: keyword.to_owned(),
        reason: reason.to_owned(),
    })
}

fn protected_tag(tag: Tag) -> bool {
    matches!(
        tag,
        tags::SOP_CLASS_UID
            | tags::PIXEL_DATA
            | tags::ROWS
            | tags::COLUMNS
            | tags::SAMPLES_PER_PIXEL
            | tags::PHOTOMETRIC_INTERPRETATION
            | tags::PLANAR_CONFIGURATION
            | tags::BITS_ALLOCATED
            | tags::BITS_STORED
            | tags::HIGH_BIT
            | tags::PIXEL_REPRESENTATION
    )
}

fn action_name(action: &RuleAction) -> &'static str {
    match action {
        RuleAction::Replace { .. } => "replace",
        RuleAction::Remove => "remove",
        RuleAction::Empty => "empty",
    }
}

fn element_text(object: &InMemDicomObject, tag: Tag) -> Option<String> {
    object
        .get(tag)
        .and_then(|element| element.to_str().ok())
        .map(|value| trim_dicom(&value).to_owned())
}

fn trim_dicom(value: &str) -> &str {
    value.trim_matches([' ', '\0'])
}

fn format_tag(tag: Tag) -> String {
    format!("({:04X},{:04X})", tag.group(), tag.element())
}

/// SHA-256 over the semantic PixelData value, including fragment boundaries and offsets.
pub fn pixel_data_sha256(object: &InMemDicomObject) -> Option<[u8; 32]> {
    let value = object.get(tags::PIXEL_DATA)?.value();
    let mut hash = Sha256::new();
    match value {
        DicomValue::Primitive(value) => {
            hash.update(b"primitive\0");
            hash.update(value.to_bytes().as_ref());
        }
        DicomValue::PixelSequence(sequence) => {
            hash.update(b"encapsulated\0");
            for offset in sequence.offset_table() {
                hash.update(offset.to_le_bytes());
            }
            for fragment in sequence.fragments() {
                hash.update((fragment.len() as u64).to_le_bytes());
                hash.update(fragment);
            }
        }
        DicomValue::Sequence(_) => return None,
    }
    Some(hash.finalize().into())
}

pub fn classify_pixel_risk(object: &InMemDicomObject) -> PixelRisk {
    let mut reasons = Vec::new();
    match element_text(object, tags::BURNED_IN_ANNOTATION)
        .map(|value| value.to_ascii_uppercase())
        .as_deref()
    {
        Some("NO") => {}
        Some("YES") => reasons.push("BurnedInAnnotation=YES".to_owned()),
        Some(value) => reasons.push(format!("BurnedInAnnotation 值无法识别: {value}")),
        None => reasons.push("缺少 BurnedInAnnotation".to_owned()),
    }
    if object.iter().any(|element| {
        let tag = element.tag();
        (0x6000..=0x60ff).contains(&tag.group()) && tag.element() == 0x3000
    }) {
        reasons.push("存在 OverlayData".to_owned());
    }
    PixelRisk {
        level: if reasons.is_empty() {
            PixelRiskLevel::Safe
        } else {
            PixelRiskLevel::Blocking
        },
        reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture;

    fn replace(tag: &str, value: &str) -> TagRule {
        TagRule {
            tag: tag.to_owned(),
            action: RuleAction::Replace {
                value: value.to_owned(),
            },
            recursive: false,
        }
    }

    #[test]
    fn manual_rules_enforce_scope_and_vr() {
        assert!(
            validate_manual_rules(&[replace("PatientName", "张^三")], TagScope::Patient).is_ok()
        );
        assert!(matches!(
            validate_manual_rules(&[replace("StudyDescription", "CT")], TagScope::Patient),
            Err(TransformError::WrongScope { .. })
        ));
        assert!(
            validate_manual_rules(
                &[replace("PatientBirthDate", "20240230")],
                TagScope::Patient
            )
            .is_err()
        );
        assert!(
            validate_manual_rules(&[replace("PatientSex", "UNKNOWN")], TagScope::Patient).is_err()
        );
        assert!(validate_manual_rules(&[replace("PixelData", "x")], TagScope::Patient).is_err());
    }

    #[test]
    fn correction_remaps_uids_and_preserves_pixels() {
        let study = fixture::unique_uid();
        let series = fixture::unique_uid();
        let sop = fixture::unique_uid();
        let mut object = fixture::ct_instance(&study, &series, &sop);
        let before = pixel_data_sha256(&object);
        let new_study = crate::Uid::generate().into_string();
        let new_series = crate::Uid::generate().into_string();
        let new_sop = crate::Uid::generate().into_string();
        let context = TransformContext {
            uid_map: HashMap::from([
                (study, new_study.clone()),
                (series, new_series.clone()),
                (sop.clone(), new_sop.clone()),
            ]),
            derivation_description: "clinical correction".to_owned(),
        };
        let outcome = apply_transform(
            &mut object,
            &[replace("PatientName", "张^三")],
            &context,
            true,
        )
        .unwrap();

        assert_eq!(outcome.metadata.patient.name.as_deref(), Some("张^三"));
        assert_eq!(outcome.metadata.study.uid.as_str(), new_study);
        assert_eq!(outcome.metadata.series.uid.as_str(), new_series);
        assert_eq!(outcome.metadata.instance.uid.as_str(), new_sop);
        assert_eq!(object.meta().media_storage_sop_instance_uid(), new_sop);
        assert_eq!(before, pixel_data_sha256(&object));
        assert_eq!(before, outcome.pixel_sha256);
    }

    #[test]
    fn uid_references_inside_sequences_are_updated() {
        let study = fixture::unique_uid();
        let series = fixture::unique_uid();
        let sop = fixture::unique_uid();
        let referenced = fixture::unique_uid();
        let mapped = crate::Uid::generate().into_string();
        let mut object = fixture::ct_instance(&study, &series, &sop);
        let item = InMemDicomObject::from_element_iter([DataElement::new(
            tags::REFERENCED_SOP_INSTANCE_UID,
            VR::UI,
            referenced.clone(),
        )]);
        object.put(DataElement::new(
            tags::REFERENCED_IMAGE_SEQUENCE,
            VR::SQ,
            DataSetSequence::from(vec![item]),
        ));
        let context = TransformContext {
            uid_map: HashMap::from([(referenced, mapped.clone())]),
            ..TransformContext::default()
        };
        apply_transform(&mut object, &[], &context, false).unwrap();
        let items = object
            .get(tags::REFERENCED_IMAGE_SEQUENCE)
            .unwrap()
            .items()
            .unwrap();
        assert_eq!(
            trim_dicom(
                &items[0]
                    .get(tags::REFERENCED_SOP_INSTANCE_UID)
                    .unwrap()
                    .to_str()
                    .unwrap()
            ),
            mapped
        );
    }

    #[test]
    fn pixel_risk_blocks_missing_yes_and_overlays() {
        let mut object = fixture::ct_instance("1.2.3", "1.2.4", "1.2.5");
        assert_eq!(classify_pixel_risk(&object).level, PixelRiskLevel::Blocking);
        object.put_str(tags::BURNED_IN_ANNOTATION, VR::CS, "NO");
        assert_eq!(classify_pixel_risk(&object).level, PixelRiskLevel::Safe);
        object.put(DataElement::new(
            Tag(0x6000, 0x3000),
            VR::OW,
            PrimitiveValue::U8(vec![0, 1].into()),
        ));
        assert_eq!(classify_pixel_risk(&object).level, PixelRiskLevel::Blocking);
    }
}
