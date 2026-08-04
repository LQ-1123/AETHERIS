//! C-FIND 查询标识符的解析与匹配语义(PS3.4 C.2.2.2)。
//!
//! 这一层是纯逻辑,不碰数据库也不碰网络 —— 匹配语义是 C-FIND 最容易出错的
//! 地方,单独拆出来才能穷举测试。`pacs-db` 负责把 [`Query`] 翻成 SQL,
//! `pacs-dimse` 负责收发消息。
//!
//! # 五种匹配类型
//!
//! 标准定义的匹配类型不是任选的,**取决于该属性的 VR**:
//!
//! | 类型 | 形式 | 适用 VR |
//! |------|------|---------|
//! | 通配(Universal) | 零长值 | 全部 —— 只请求返回,不做过滤 |
//! | 单值(Single Value) | `CT` | 全部 |
//! | 通配符(Wild Card) | `ZHANG*`、`A?C` | 仅 AE/CS/LO/PN/SH |
//! | 范围(Range) | `20240101-20240131` | 仅 DA/TM/DT |
//! | UID 列表(List of UID) | `1.2.3\1.2.4` | 仅 UI |
//!
//! 把通配符用在 DA 上、或把 `-` 当成范围符号用在 LO 上,都会让查询悄悄
//! 匹配到错误的结果 —— 所以分类严格按 VR 走,而不是按值的形状猜。

use std::collections::BTreeMap;

use chrono::{NaiveDate, NaiveTime};
use dicom::core::{Tag, VR};
use dicom::object::InMemDicomObject;

/// 查询层级 (0008,0052) QueryRetrieveLevel。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum QueryLevel {
    Patient,
    Study,
    Series,
    Image,
}

impl QueryLevel {
    pub fn parse(raw: &str) -> Option<Self> {
        // 标准规定这些值全大写,但设备实际会送小写和带补齐空格的
        match raw.trim_matches(|c: char| c == '\0' || c.is_whitespace()) {
            s if s.eq_ignore_ascii_case("PATIENT") => Some(Self::Patient),
            s if s.eq_ignore_ascii_case("STUDY") => Some(Self::Study),
            s if s.eq_ignore_ascii_case("SERIES") => Some(Self::Series),
            // IMAGE 是标准写法;有些设备送 INSTANCE
            s if s.eq_ignore_ascii_case("IMAGE") || s.eq_ignore_ascii_case("INSTANCE") => {
                Some(Self::Image)
            }
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Patient => "PATIENT",
            Self::Study => "STUDY",
            Self::Series => "SERIES",
            Self::Image => "IMAGE",
        }
    }

    /// 层级深浅。查询层级不浅于某个属性所属的层级时,那个属性才可查 ——
    /// `pacs-db` 靠这个判断该属性的表在不在 JOIN 范围内。
    pub fn depth(self) -> u8 {
        match self {
            Self::Patient => 0,
            Self::Study => 1,
            Self::Series => 2,
            Self::Image => 3,
        }
    }
}

/// 一个键的匹配方式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchKey {
    /// 零长值:不过滤,只要求把这个属性放进响应。
    Universal,
    Single(String),
    /// 含 `*` 或 `?`。存的是原始 DICOM 形式,翻 SQL 时才转义。
    Wildcard(String),
    /// UI VR 的反斜杠分隔列表,命中任意一个即匹配。
    UidList(Vec<String>),
    /// 日期范围。两端都是 `None` 不会出现 —— 那种情况归为 [`MatchKey::Universal`]。
    DateRange {
        from: Option<NaiveDate>,
        to: Option<NaiveDate>,
    },
    TimeRange {
        from: Option<NaiveTime>,
        to: Option<NaiveTime>,
    },
}

/// 解析好的一次 C-FIND 请求。
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub level: QueryLevel,
    /// 请求里出现的全部键,含只作返回用的通配键。
    ///
    /// 用 `BTreeMap` 而不是 `HashMap`:响应数据集的元素必须按标签升序排列,
    /// 有序容器让构造响应时不用再排一次,也让测试的断言稳定。
    pub keys: BTreeMap<Tag, MatchKey>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QueryError {
    #[error("标识符缺少 QueryRetrieveLevel (0008,0052)")]
    MissingLevel,
    #[error("无法识别的 QueryRetrieveLevel:{raw:?}")]
    UnknownLevel { raw: String },
}

impl Query {
    /// 从 C-FIND-RQ 的标识符数据集解析。
    ///
    /// 无法解析的键一律降级为 [`MatchKey::Single`](MatchKey::Single) 或直接跳过,
    /// 不让整个查询失败 —— 一个畸形的可选键不该把整次查询打回错误。
    pub fn from_identifier(identifier: &InMemDicomObject) -> Result<Self, QueryError> {
        let raw_level = crate::utf8_text(
            identifier,
            dicom::dictionary_std::tags::QUERY_RETRIEVE_LEVEL,
        )
        .ok_or(QueryError::MissingLevel)?;
        let level = QueryLevel::parse(&raw_level).ok_or_else(|| QueryError::UnknownLevel {
            raw: raw_level.trim().to_owned(),
        })?;

        let mut keys = BTreeMap::new();
        for element in identifier.iter() {
            let tag = element.header().tag;
            // 跳过的三类:
            //   * 0000 组是命令集,不该出现在标识符里;
            //   * QueryRetrieveLevel 已单独取出;
            //   * SpecificCharacterSet 描述的是**请求本身的编码**,不是匹配键。
            //     当成匹配键会拿它去过滤病人,并且因为无对应列而白白把响应状态
            //     降成 0xFF01;
            //   * 奇数组号是私有标签,我们不认识它们的语义,不该拿去过滤。
            if tag.group() == 0x0000
                || tag == dicom::dictionary_std::tags::QUERY_RETRIEVE_LEVEL
                || tag == dicom::dictionary_std::tags::SPECIFIC_CHARACTER_SET
                || tag.group() % 2 == 1
            {
                continue;
            }

            let raw = crate::utf8_text(identifier, tag).unwrap_or_default();
            keys.insert(tag, classify(&raw, element.vr()));
        }

        Ok(Self { level, keys })
    }

    /// 只保留会实际参与过滤的键(丢掉纯返回键)。
    pub fn filters(&self) -> impl Iterator<Item = (Tag, &MatchKey)> {
        self.keys
            .iter()
            .filter(|(_, key)| **key != MatchKey::Universal)
            .map(|(tag, key)| (*tag, key))
    }
}

/// 按 VR 决定一个原始值属于哪种匹配。
///
/// 顺序有讲究:先判空(通配),再按 VR 分派。VR 不支持某种形式时**降级为单值**
/// 而不是报错 —— 比如 LO 里出现 `-`,那就是名字的一部分,不是范围。
pub fn classify(raw: &str, vr: VR) -> MatchKey {
    // DICOM 用尾部空格补到偶数长度,UI 用 NUL。补齐字符不是值的一部分。
    let value = raw.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    if value.is_empty() {
        return MatchKey::Universal;
    }

    match vr {
        VR::DA => parse_date_range(value).unwrap_or_else(|| MatchKey::Single(value.to_owned())),
        VR::TM => parse_time_range(value).unwrap_or_else(|| MatchKey::Single(value.to_owned())),
        // DT 的范围端点是 YYYYMMDDHHMMSS 连写,拆开后按日期部分匹配就够用了;
        // 完整的 DT 精度目前没有查询键需要。
        VR::DT => MatchKey::Single(value.to_owned()),
        VR::UI => {
            // UI 的多值分隔符是反斜杠。单个值也走这条路,列表长度为 1。
            let uids: Vec<String> = value
                .split('\\')
                .map(|s| s.trim_matches(|c: char| c == '\0' || c.is_whitespace()))
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect();
            match uids.len() {
                0 => MatchKey::Universal,
                1 => MatchKey::Single(uids.into_iter().next().expect("刚判断过长度为 1")),
                _ => MatchKey::UidList(uids),
            }
        }
        _ if supports_wildcard(vr) && value.contains(['*', '?']) => {
            MatchKey::Wildcard(value.to_owned())
        }
        _ => MatchKey::Single(value.to_owned()),
    }
}

/// 哪些 VR 允许通配符匹配(PS3.4 C.2.2.2.4)。
///
/// 标准是反着列的:DA/TM/DT/SL/SS/UL/US/FL/FD/OB/OW/UN/AT/DS/IS/AS/UI 不支持。
/// 这里正着列成白名单 —— 白名单在加新 VR 时默认拒绝,比黑名单默认放行安全。
fn supports_wildcard(vr: VR) -> bool {
    matches!(vr, VR::AE | VR::CS | VR::LO | VR::PN | VR::SH)
}

/// 解析 DA 范围 `<日期>-<日期>`,任一端可省略。
///
/// 返回 `None` 表示这不是范围(没有 `-`),调用方按单值处理。
fn parse_date_range(value: &str) -> Option<MatchKey> {
    let (lo, hi) = value.split_once('-')?;
    let from = parse_da(lo);
    let to = parse_da(hi);

    // `-` 两端都解析不出日期:要么是畸形值,要么根本不是范围。
    // 当成范围会退化成「全匹配」,把整库返回给对方,所以宁可交回单值处理。
    if from.is_none() && to.is_none() {
        return None;
    }
    // 只有一端解析成功、另一端非空 —— 说明那一端写坏了。同样不能当范围:
    // `20240101-2024xxxx` 若按 `>= 20240101` 处理,会多返回大量结果。
    if (from.is_none() && !lo.trim().is_empty()) || (to.is_none() && !hi.trim().is_empty()) {
        return None;
    }
    Some(MatchKey::DateRange { from, to })
}

fn parse_time_range(value: &str) -> Option<MatchKey> {
    let (lo, hi) = value.split_once('-')?;
    let from = parse_tm(lo);
    let to = parse_tm(hi);

    if from.is_none() && to.is_none() {
        return None;
    }
    if (from.is_none() && !lo.trim().is_empty()) || (to.is_none() && !hi.trim().is_empty()) {
        return None;
    }
    Some(MatchKey::TimeRange { from, to })
}

/// DA 是 `YYYYMMDD`。ACR-NEMA 的 `YYYY.MM.DD` 老格式也接受 —— 老设备还在用。
pub fn parse_da(raw: &str) -> Option<NaiveDate> {
    let s: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    if s.len() != 8 {
        return None;
    }
    NaiveDate::from_ymd_opt(
        s[0..4].parse().ok()?,
        s[4..6].parse().ok()?,
        s[6..8].parse().ok()?,
    )
}

/// TM 是 `HHMMSS.FFFFFF`,后面的分量可以逐级省略。
///
/// 省略的分量按 0 补:`14` 表示 14:00:00。范围查询里这是对的 ——
/// `0900-1700` 的意思就是 09:00:00 到 17:00:00。
pub fn parse_tm(raw: &str) -> Option<NaiveTime> {
    let trimmed = raw.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    if trimmed.is_empty() {
        return None;
    }
    let (main, fraction) = match trimmed.split_once('.') {
        Some((m, f)) => (m, f),
        None => (trimmed, ""),
    };
    // 老格式允许 HH:MM:SS
    let digits: String = main.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() || !digits.len().is_multiple_of(2) || digits.len() > 6 {
        return None;
    }

    let part = |index: usize| -> Option<u32> {
        digits
            .get(index * 2..index * 2 + 2)
            .map_or(Some(0), |s| s.parse().ok())
    };
    // 小数部分是「秒的小数」,右侧补零到 6 位才是微秒:`.5` 是 500000μs 不是 5μs
    let micros: u32 = if fraction.is_empty() {
        0
    } else {
        let padded: String = fraction
            .chars()
            .chain(std::iter::repeat('0'))
            .take(6)
            .collect();
        padded.parse().ok()?
    };

    NaiveTime::from_hms_micro_opt(part(0)?, part(1)?, part(2)?, micros)
}

/// 把 DICOM 通配符翻成 SQL `LIKE` 模式。
///
/// **必须先转义 SQL 的元字符再翻译 DICOM 的**,否则查询里字面的 `%` 会变成
/// 通配符:找 `50%` 会匹配到所有以 50 开头的值。转义符用 `\`,
/// 调用方的 SQL 要带 `ESCAPE '\'`。
pub fn wildcard_to_sql_like(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len() + 8);
    for ch in pattern.chars() {
        match ch {
            // SQL 元字符:先转义掉,让它们只能当字面量
            '%' | '_' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            // DICOM 元字符:翻成 SQL 的对应物
            '*' => out.push('%'),
            '?' => out.push('_'),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use dicom::core::{DataElement, PrimitiveValue};
    use dicom::dictionary_std::tags;

    #[test]
    fn empty_value_is_universal_matching() {
        assert_eq!(classify("", VR::LO), MatchKey::Universal);
        // 补齐用的空格和 NUL 不算值
        assert_eq!(classify("  ", VR::LO), MatchKey::Universal);
        assert_eq!(classify("\0", VR::UI), MatchKey::Universal);
    }

    #[test]
    fn wildcards_only_apply_to_permitted_vrs() {
        assert_eq!(
            classify("ZHANG*", VR::PN),
            MatchKey::Wildcard("ZHANG*".into())
        );
        assert_eq!(classify("C?", VR::CS), MatchKey::Wildcard("C?".into()));

        // UI 不支持通配符 —— `*` 是 UID 的非法字符,只能当字面量
        assert_eq!(classify("1.2.*", VR::UI), MatchKey::Single("1.2.*".into()));
        // IS/DS 也不支持
        assert_eq!(classify("1*", VR::IS), MatchKey::Single("1*".into()));
    }

    /// LO 里的 `-` 是名字的一部分,不能当成范围。
    #[test]
    fn hyphen_is_only_a_range_for_date_and_time_vrs() {
        assert_eq!(
            classify("2024-2025", VR::LO),
            MatchKey::Single("2024-2025".into())
        );
        assert_eq!(
            classify("20240101-20240131", VR::DA),
            MatchKey::DateRange {
                from: NaiveDate::from_ymd_opt(2024, 1, 1),
                to: NaiveDate::from_ymd_opt(2024, 1, 31),
            }
        );
    }

    #[test]
    fn open_ended_date_ranges_work_in_both_directions() {
        assert_eq!(
            classify("20240101-", VR::DA),
            MatchKey::DateRange {
                from: NaiveDate::from_ymd_opt(2024, 1, 1),
                to: None,
            }
        );
        assert_eq!(
            classify("-20240131", VR::DA),
            MatchKey::DateRange {
                from: None,
                to: NaiveDate::from_ymd_opt(2024, 1, 31),
            }
        );
    }

    /// 写坏的范围端点绝不能退化成「无上界」—— 那会把整库返回出去。
    #[test]
    fn malformed_range_falls_back_to_single_value() {
        assert_eq!(
            classify("20240101-2024XXXX", VR::DA),
            MatchKey::Single("20240101-2024XXXX".into())
        );
        assert_eq!(classify("-", VR::DA), MatchKey::Single("-".into()));
        // 单值日期不含 `-`,原样保留
        assert_eq!(
            classify("20240101", VR::DA),
            MatchKey::Single("20240101".into())
        );
    }

    #[test]
    fn uid_lists_split_on_backslash() {
        assert_eq!(
            classify("1.2.3\\1.2.4", VR::UI),
            MatchKey::UidList(vec!["1.2.3".into(), "1.2.4".into()])
        );
        // 单个 UID 不必包成列表
        assert_eq!(classify("1.2.3", VR::UI), MatchKey::Single("1.2.3".into()));
    }

    #[test]
    fn time_components_may_be_truncated() {
        assert_eq!(parse_tm("14"), NaiveTime::from_hms_opt(14, 0, 0));
        assert_eq!(parse_tm("1430"), NaiveTime::from_hms_opt(14, 30, 0));
        assert_eq!(parse_tm("143025"), NaiveTime::from_hms_opt(14, 30, 25));
        // 小数是「秒的小数」,`.5` = 500000 微秒
        assert_eq!(
            parse_tm("143025.5"),
            NaiveTime::from_hms_micro_opt(14, 30, 25, 500_000)
        );
        assert_eq!(
            parse_tm("143025.123456"),
            NaiveTime::from_hms_micro_opt(14, 30, 25, 123_456)
        );
        // 奇数位数不合法
        assert_eq!(parse_tm("143"), None);
    }

    #[test]
    fn old_style_dates_with_dots_are_accepted() {
        // ACR-NEMA 的 YYYY.MM.DD,老设备还在送
        assert_eq!(parse_da("2024.01.15"), NaiveDate::from_ymd_opt(2024, 1, 15));
        assert_eq!(parse_da("20240115"), NaiveDate::from_ymd_opt(2024, 1, 15));
        assert_eq!(parse_da("2024"), None);
    }

    /// 查询里字面的 `%` 不能变成通配符,否则会返回远多于预期的结果。
    #[test]
    fn sql_metacharacters_are_escaped_before_translation() {
        assert_eq!(wildcard_to_sql_like("ZHANG*"), "ZHANG%");
        assert_eq!(wildcard_to_sql_like("A?C"), "A_C");
        assert_eq!(wildcard_to_sql_like("50%"), "50\\%");
        assert_eq!(wildcard_to_sql_like("a_b"), "a\\_b");
        assert_eq!(wildcard_to_sql_like("back\\slash"), "back\\\\slash");
        // 混合:DICOM 的翻译,SQL 的转义
        assert_eq!(wildcard_to_sql_like("50%*"), "50\\%%");
    }

    fn identifier(
        level: &str,
        elements: Vec<dicom::object::mem::InMemElement>,
    ) -> InMemDicomObject {
        let mut all = vec![DataElement::new(
            tags::QUERY_RETRIEVE_LEVEL,
            VR::CS,
            PrimitiveValue::from(level),
        )];
        all.extend(elements);
        InMemDicomObject::from_element_iter(all)
    }

    #[test]
    fn parses_a_study_level_query() {
        let object = identifier(
            "STUDY",
            vec![
                DataElement::new(tags::PATIENT_NAME, VR::PN, PrimitiveValue::from("ZHANG*")),
                // 零长 = 请求返回该属性
                DataElement::new(tags::STUDY_DATE, VR::DA, PrimitiveValue::Empty),
                DataElement::new(
                    tags::ACCESSION_NUMBER,
                    VR::SH,
                    PrimitiveValue::from("A12345"),
                ),
            ],
        );

        let query = Query::from_identifier(&object).expect("应能解析");
        assert_eq!(query.level, QueryLevel::Study);
        assert_eq!(
            query.keys.get(&tags::PATIENT_NAME),
            Some(&MatchKey::Wildcard("ZHANG*".into()))
        );
        assert_eq!(
            query.keys.get(&tags::STUDY_DATE),
            Some(&MatchKey::Universal)
        );

        // 通配键不参与过滤,但仍要出现在响应里
        let filters: Vec<Tag> = query.filters().map(|(tag, _)| tag).collect();
        assert!(!filters.contains(&tags::STUDY_DATE));
        assert_eq!(filters.len(), 2);
    }

    #[test]
    fn level_is_required_and_validated() {
        let no_level = InMemDicomObject::from_element_iter([DataElement::new(
            tags::PATIENT_NAME,
            VR::PN,
            PrimitiveValue::from("X"),
        )]);
        assert_eq!(
            Query::from_identifier(&no_level),
            Err(QueryError::MissingLevel)
        );

        let bad = identifier("GALAXY", vec![]);
        assert!(matches!(
            Query::from_identifier(&bad),
            Err(QueryError::UnknownLevel { .. })
        ));
    }

    #[test]
    fn level_parsing_tolerates_case_and_padding() {
        assert_eq!(QueryLevel::parse("STUDY "), Some(QueryLevel::Study));
        assert_eq!(QueryLevel::parse("study"), Some(QueryLevel::Study));
        assert_eq!(QueryLevel::parse("IMAGE"), Some(QueryLevel::Image));
        // 有些设备送 INSTANCE 而不是标准的 IMAGE
        assert_eq!(QueryLevel::parse("INSTANCE"), Some(QueryLevel::Image));
        assert_eq!(QueryLevel::parse("FRAME"), None);
    }

    /// 私有标签(奇数组号)不该拿去过滤 —— 我们不知道它们的语义。
    #[test]
    fn private_tags_are_ignored() {
        let object = identifier(
            "STUDY",
            vec![DataElement::new(
                Tag(0x0009, 0x0010),
                VR::LO,
                PrimitiveValue::from("VENDOR"),
            )],
        );
        let query = Query::from_identifier(&object).unwrap();
        assert!(query.keys.is_empty(), "私有标签不该进入查询键");
    }

    /// SpecificCharacterSet 说的是请求怎么编码,不是「找字符集等于这个的病人」。
    #[test]
    fn specific_character_set_is_not_a_matching_key() {
        let object = identifier(
            "STUDY",
            vec![DataElement::new(
                tags::SPECIFIC_CHARACTER_SET,
                VR::CS,
                PrimitiveValue::from("ISO_IR 192"),
            )],
        );
        let query = Query::from_identifier(&object).unwrap();
        assert!(
            query.keys.is_empty(),
            "字符集声明不该被当成匹配键,否则会拿它去过滤,还会把状态降成 0xFF01"
        );
    }
}
