//! QIDO-RS 的查询参数翻译(PS3.18 §10.6)。
//!
//! # 复用 C-FIND 的匹配语义,不另写一套
//!
//! QIDO-RS 和 C-FIND 是同一套匹配语义的两个门面:URL 参数 `PatientName=ZHANG*`
//! 和 C-FIND 标识符里的同名键要产生完全一样的结果。所以这一层只做**翻译** ——
//! 把 URL 参数拼成一个标识符数据集,交给 [`pacs_core::Query`] 去分类、
//! 交给 `pacs_db::find` 去查。
//!
//! 两套独立实现迟早会在边角上分叉(`20240101-` 算不算合法范围、`%` 要不要转义),
//! 而分叉的症状是「网页上查得到的检查,DICOM 客户端查不到」—— 这种问题极难定位。
//!
//! # 属性名的三种写法
//!
//! 标准允许用关键字、八位十六进制标签、带逗号的标签指同一个属性,都要认:
//!
//! ```text
//! ?PatientName=ZHANG*      关键字
//! ?00100010=ZHANG*         十六进制标签
//! ?0010,0010=ZHANG*        带逗号(标准没要求,但客户端会送)
//! ```

use dicom::core::dictionary::{DataDictionary, DataDictionaryEntry};
use dicom::core::{DataElement, PrimitiveValue, Tag, VR};
use dicom::dictionary_std::StandardDataDictionary;
use dicom::object::InMemDicomObject;
use pacs_core::query::{Query, QueryError, QueryLevel};

/// 分页与返回控制参数。这些不是匹配键,不能拿去过滤。
///
/// 把它们当匹配键的后果是静默的:`limit` 在字典里查不到标签,
/// 于是变成「不支持的键」被忽略,分页就失效了。
const RESERVED: &[&str] = &[
    "limit",
    "offset",
    "includefield",
    "fuzzymatching",
    "orderby",
];

/// 解析好的一次 QIDO-RS 请求。
#[derive(Debug, Clone, PartialEq)]
pub struct QidoRequest {
    pub query: Query,
    pub limit: Option<usize>,
    pub offset: usize,
    /// 出现过但我们没实现的参数(`fuzzymatching=true`、`orderby`)。
    ///
    /// 不静默接受:调用方以为模糊匹配生效了、结果却是精确匹配,
    /// 它无从察觉。响应里用 `Warning` 头告知。
    pub unsupported_params: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QidoError {
    #[error("无法识别的查询参数 {name:?}")]
    UnknownParameter { name: String },
    #[error("参数 {name} 需要一个非负整数,收到 {value:?}")]
    NotANumber { name: &'static str, value: String },
    #[error("limit 不能为 0")]
    ZeroLimit,
    #[error(transparent)]
    Query(#[from] QueryError),
}

/// 从 URL 查询参数构造请求。
///
/// `level` 由路由决定(`/studies` → STUDY),不从参数里读 —— 让调用方自己指定
/// 层级会和 URL 路径产生矛盾。
pub fn parse(level: QueryLevel, params: &[(String, String)]) -> Result<QidoRequest, QidoError> {
    let mut elements = vec![DataElement::new(
        dicom::dictionary_std::tags::QUERY_RETRIEVE_LEVEL,
        VR::CS,
        PrimitiveValue::from(level.as_str()),
    )];
    let mut limit = None;
    let mut offset = 0_usize;
    let mut unsupported_params = Vec::new();

    for (name, value) in params {
        let lowered = name.to_ascii_lowercase();
        match lowered.as_str() {
            "limit" => {
                let parsed = parse_number("limit", value)?;
                if parsed == 0 {
                    // limit=0 几乎总是调用方算错了(比如 `pageSize * 0`)。
                    // 回空集会让这个 bug 表现为"没有数据",报错才能让它现形。
                    return Err(QidoError::ZeroLimit);
                }
                limit = Some(parsed);
            }
            "offset" => offset = parse_number("offset", value)?,
            "includefield" => {
                for field in value.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                    if field.eq_ignore_ascii_case("all") {
                        // `includefield=all` 要求返回该层全部属性。我们的列表是
                        // 固定的,忠实做法是把已知列全部作为返回键加进去。
                        for (tag, vr) in returnable_columns(level) {
                            elements.push(DataElement::new(tag, vr, PrimitiveValue::Empty));
                        }
                    } else if let Some((tag, vr)) = resolve_attribute(field) {
                        // 零长值 = 通配匹配 = 只请求返回,不参与过滤
                        elements.push(DataElement::new(tag, vr, PrimitiveValue::Empty));
                    } else {
                        return Err(QidoError::UnknownParameter {
                            name: field.to_owned(),
                        });
                    }
                }
            }
            // fuzzymatching=false 等于没要求,不必告警
            "fuzzymatching" if is_falsy(value) => {}
            other if RESERVED.contains(&other) => {
                unsupported_params.push(name.clone());
            }
            _ => {
                let Some((tag, vr)) = resolve_attribute(name) else {
                    // 拒绝而不是忽略:`?NoSuchKey=x` 被忽略后就成了无条件查询,
                    // 把整个库回给调用方,而它以为自己过滤了。
                    return Err(QidoError::UnknownParameter { name: name.clone() });
                };
                elements.push(DataElement::new(tag, vr, text_value(value)));
            }
        }
    }

    elements.sort_by_key(|element| element.header().tag);
    elements.dedup_by_key(|element| element.header().tag);
    let identifier = InMemDicomObject::from_element_iter(elements);

    Ok(QidoRequest {
        query: Query::from_identifier(&identifier)?,
        limit,
        offset,
        unsupported_params,
    })
}

fn parse_number(name: &'static str, raw: &str) -> Result<usize, QidoError> {
    raw.trim()
        .parse::<usize>()
        .map_err(|_| QidoError::NotANumber {
            name,
            value: raw.to_owned(),
        })
}

fn is_falsy(raw: &str) -> bool {
    matches!(raw.trim().to_ascii_lowercase().as_str(), "false" | "0" | "")
}

/// 空串要成为「零长元素」(通配匹配),不是「值为空串」。
///
/// `?StudyDate=` 的意思是"把 StudyDate 返回给我",不是"找 StudyDate 等于空串的"。
fn text_value(raw: &str) -> PrimitiveValue {
    if raw.is_empty() {
        PrimitiveValue::Empty
    } else {
        PrimitiveValue::from(raw)
    }
}

/// 把属性名解析成标签和 VR,认三种写法。
fn resolve_attribute(name: &str) -> Option<(Tag, VR)> {
    let trimmed = name.trim();
    if let Some(tag) = parse_tag_literal(trimmed) {
        // 标签形式:VR 从字典查;查不到的(私有标签)按 UN 处理,
        // 后续会因为没有对应列而被当作不支持的键。
        let vr = StandardDataDictionary
            .by_tag(tag)
            .map_or(VR::UN, |entry| entry.vr().relaxed());
        return Some((tag, vr));
    }
    // 绑成局部变量:by_name 返回的 entry 借用字典,直接链式调用会让临时值提前析构
    let dictionary = StandardDataDictionary;
    let entry = dictionary.by_name(trimmed)?;
    Some((entry.tag(), entry.vr().relaxed()))
}

/// `00100010` 或 `0010,0010`。
fn parse_tag_literal(raw: &str) -> Option<Tag> {
    let compact: String = raw.chars().filter(|c| *c != ',').collect();
    if compact.len() != 8 || !compact.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let group = u16::from_str_radix(&compact[0..4], 16).ok()?;
    let element = u16::from_str_radix(&compact[4..8], 16).ok()?;
    Some(Tag(group, element))
}

/// `includefield=all` 时该层要返回的属性。
///
/// 与 `pacs_db::find` 的列表保持一致 —— 那边没有的列,请求了也回不出值。
fn returnable_columns(level: QueryLevel) -> Vec<(Tag, VR)> {
    use dicom::dictionary_std::tags;
    let mut columns: Vec<(Tag, VR)> = vec![
        (tags::PATIENT_ID, VR::LO),
        (tags::PATIENT_NAME, VR::PN),
        (tags::PATIENT_BIRTH_DATE, VR::DA),
        (tags::PATIENT_SEX, VR::CS),
    ];
    if level.depth() >= QueryLevel::Study.depth() {
        columns.extend([
            (tags::STUDY_INSTANCE_UID, VR::UI),
            (tags::STUDY_DATE, VR::DA),
            (tags::STUDY_TIME, VR::TM),
            (tags::ACCESSION_NUMBER, VR::SH),
            (tags::STUDY_ID, VR::SH),
            (tags::STUDY_DESCRIPTION, VR::LO),
            (tags::REFERRING_PHYSICIAN_NAME, VR::PN),
            (tags::MODALITIES_IN_STUDY, VR::CS),
            (tags::NUMBER_OF_STUDY_RELATED_SERIES, VR::IS),
            (tags::NUMBER_OF_STUDY_RELATED_INSTANCES, VR::IS),
        ]);
    }
    if level.depth() >= QueryLevel::Series.depth() {
        columns.extend([
            (tags::SERIES_INSTANCE_UID, VR::UI),
            (tags::SERIES_NUMBER, VR::IS),
            (tags::MODALITY, VR::CS),
            (tags::SERIES_DESCRIPTION, VR::LO),
            (tags::BODY_PART_EXAMINED, VR::CS),
            (tags::SERIES_DATE, VR::DA),
            (tags::SERIES_TIME, VR::TM),
            (tags::NUMBER_OF_SERIES_RELATED_INSTANCES, VR::IS),
        ]);
    }
    if level.depth() >= QueryLevel::Image.depth() {
        columns.extend([
            (tags::SOP_INSTANCE_UID, VR::UI),
            (tags::SOP_CLASS_UID, VR::UI),
            (tags::INSTANCE_NUMBER, VR::IS),
            (tags::ROWS, VR::US),
            (tags::COLUMNS, VR::US),
            (tags::NUMBER_OF_FRAMES, VR::IS),
        ]);
    }
    columns
}

#[cfg(test)]
mod tests {
    use super::*;
    use dicom::dictionary_std::tags;
    use pacs_core::query::MatchKey;

    fn params(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn attribute_names_accept_all_three_forms() {
        for name in ["PatientName", "00100010", "0010,0010"] {
            let request = parse(QueryLevel::Study, &params(&[(name, "ZHANG*")]))
                .unwrap_or_else(|e| panic!("{name} 应能解析:{e}"));
            assert_eq!(
                request.query.keys.get(&tags::PATIENT_NAME),
                Some(&MatchKey::Wildcard("ZHANG*".into())),
                "{name} 应解析成 PatientName 的通配匹配"
            );
        }
    }

    /// URL 里的日期范围必须走 C-FIND 那套分类,成为 `DateRange` 而不是字面串。
    #[test]
    fn date_ranges_reuse_the_cfind_semantics() {
        let request = parse(
            QueryLevel::Study,
            &params(&[("StudyDate", "20240101-20240131")]),
        )
        .unwrap();
        assert!(matches!(
            request.query.keys.get(&tags::STUDY_DATE),
            Some(MatchKey::DateRange { .. })
        ));
    }

    #[test]
    fn empty_value_becomes_a_return_key_not_an_empty_string_match() {
        let request = parse(QueryLevel::Study, &params(&[("StudyDate", "")])).unwrap();
        assert_eq!(
            request.query.keys.get(&tags::STUDY_DATE),
            Some(&MatchKey::Universal),
            "`?StudyDate=` 的意思是把它返回给我,不是找值等于空串的记录"
        );
    }

    #[test]
    fn limit_and_offset_are_not_matching_keys() {
        let request = parse(
            QueryLevel::Study,
            &params(&[("limit", "50"), ("offset", "100")]),
        )
        .unwrap();
        assert_eq!(request.limit, Some(50));
        assert_eq!(request.offset, 100);
        // 只剩层级本身,没有把 limit/offset 当成属性
        assert!(request.query.keys.is_empty());
    }

    /// 拼错的参数名必须报错。忽略它等于把整个库返回给调用方。
    #[test]
    fn unknown_parameters_are_rejected() {
        let error = parse(QueryLevel::Study, &params(&[("NoSuchKey", "x")])).unwrap_err();
        assert_eq!(
            error,
            QidoError::UnknownParameter {
                name: "NoSuchKey".into()
            }
        );
    }

    #[test]
    fn malformed_numbers_are_rejected() {
        assert!(matches!(
            parse(QueryLevel::Study, &params(&[("limit", "abc")])),
            Err(QidoError::NotANumber { name: "limit", .. })
        ));
        // limit=0 几乎总是调用方算错了,回空集会掩盖这个 bug
        assert_eq!(
            parse(QueryLevel::Study, &params(&[("limit", "0")])),
            Err(QidoError::ZeroLimit)
        );
    }

    #[test]
    fn includefield_adds_return_keys() {
        let request = parse(
            QueryLevel::Study,
            &params(&[("includefield", "StudyDescription,AccessionNumber")]),
        )
        .unwrap();
        assert_eq!(
            request.query.keys.get(&tags::STUDY_DESCRIPTION),
            Some(&MatchKey::Universal)
        );
        assert_eq!(
            request.query.keys.get(&tags::ACCESSION_NUMBER),
            Some(&MatchKey::Universal)
        );
    }

    #[test]
    fn includefield_all_expands_to_the_level_columns() {
        let study = parse(QueryLevel::Study, &params(&[("includefield", "all")])).unwrap();
        assert!(study.query.keys.contains_key(&tags::STUDY_DESCRIPTION));
        assert!(study.query.keys.contains_key(&tags::MODALITIES_IN_STUDY));
        // STUDY 层不该带 Series 层的列
        assert!(!study.query.keys.contains_key(&tags::MODALITY));

        let series = parse(QueryLevel::Series, &params(&[("includefield", "all")])).unwrap();
        assert!(series.query.keys.contains_key(&tags::MODALITY));
    }

    /// 未实现的参数要报出来,不能静默接受 —— 调用方察觉不到自己的参数被忽略。
    #[test]
    fn unsupported_parameters_are_reported() {
        let request = parse(
            QueryLevel::Study,
            &params(&[("fuzzymatching", "true"), ("orderby", "StudyDate")]),
        )
        .unwrap();
        assert_eq!(request.unsupported_params.len(), 2);

        // fuzzymatching=false 等于没提要求,不该告警
        let quiet = parse(QueryLevel::Study, &params(&[("fuzzymatching", "false")])).unwrap();
        assert!(quiet.unsupported_params.is_empty());
    }

    #[test]
    fn parameter_names_are_case_insensitive_for_reserved_words() {
        let request = parse(QueryLevel::Study, &params(&[("LIMIT", "10")])).unwrap();
        assert_eq!(request.limit, Some(10));
    }
}
