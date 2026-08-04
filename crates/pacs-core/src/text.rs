//! DICOM 字符集解码与应用层 UTF-8 规范化。
//!
//! dicom-rs 会在解析数据集时根据 SpecificCharacterSet 解码大多数文本。本模块在
//! 其上补齐三件事:多值 ISO-2022 声明、缺失/不支持声明的确定性降级、以及坏字节
//! 清理。原始 DICOM 文件不在这里改写；只有内存对象和随后写入数据库/JSON 的文本
//! 会被规范化。

use dicom::core::value::Value as DicomValue;
use dicom::core::{DataElement, PrimitiveValue, Tag, VR};
use dicom::dictionary_std::tags;
use dicom::encoding::text::{SpecificCharacterSet, TextCodec};
use dicom::object::mem::InMemElement;
use dicom::object::{DefaultDicomObject, InMemDicomObject};

/// 一次数据集文本规范化的结果。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TextNormalizationReport {
    /// 实际检查过的、受 SpecificCharacterSet 影响的文本元素数量。
    pub text_elements: usize,
    /// 因字符集补救而改变的值数量。
    pub corrected_values: usize,
    /// 含坏字节、非法控制字符或不支持字符集而使用 U+FFFD 的值数量。
    pub values_with_replacement: usize,
    /// 声明缺失或错误，但原字节能被严格验证为 UTF-8 的值数量。
    pub assumed_utf8_values: usize,
    /// 声明缺失或与原始字节不符，最终使用 UTF-8/GB18030 兜底的值数量。
    pub fallback_decoded_values: usize,
    /// 数据集中是否出现了当前不支持的字符集声明。
    pub unsupported_character_set: bool,
}

impl TextNormalizationReport {
    pub fn has_warnings(self) -> bool {
        self.values_with_replacement > 0
            || self.assumed_utf8_values > 0
            || self.fallback_decoded_values > 0
            || self.unsupported_character_set
    }
}

/// 把一个已解析的数据集中的文本统一成 Rust UTF-8 字符串。
///
/// 该函数会把内存对象的 SpecificCharacterSet 改为 `ISO_IR 192`，保证对象之后若被
/// 编码为 C-FIND 响应或 DICOM JSON，声明与实际 UTF-8 内容一致。调用者若需要保留
/// 文件逐字节一致性，应继续保存接收时的原始字节，而不是重新编码此对象。
pub fn normalize_dataset_text(object: &mut InMemDicomObject) -> TextNormalizationReport {
    let mut report = TextNormalizationReport::default();
    normalize_object(object, None, true, &mut report);
    if report.has_warnings() {
        tracing::warn!(
            corrected_values = report.corrected_values,
            replacement_values = report.values_with_replacement,
            assumed_utf8_values = report.assumed_utf8_values,
            fallback_decoded_values = report.fallback_decoded_values,
            unsupported_character_set = report.unsupported_character_set,
            "DICOM 文本已规范化为 UTF-8"
        );
    }
    report
}

/// [`normalize_dataset_text`] 的 Part 10 文件对象版本。
pub fn normalize_file_text(object: &mut DefaultDicomObject) -> TextNormalizationReport {
    normalize_dataset_text(object)
}

/// 读取一个 DICOM 文本属性，保证返回值不会包含解码器的八进制转义或非法控制符。
///
/// 正常的生产解析入口仍应先调用 [`normalize_dataset_text`]；这个函数也可安全用于
/// 测试夹具和调用方自行构造的内存对象。
pub fn utf8_text(object: &InMemDicomObject, tag: Tag) -> Option<String> {
    let decoder = DicomTextDecoder::for_object(object, None);
    let element = object.get(tag)?;
    let decoded = decoder.decode_element(element)?;
    if decoded.had_replacement || decoded.assumed_utf8 || decoded.used_fallback {
        tracing::warn!(
            tag = %tag,
            replacement = decoded.had_replacement,
            assumed_utf8 = decoded.assumed_utf8,
            used_fallback = decoded.used_fallback,
            "DICOM 文本属性需要字符集补救"
        );
    }
    let trimmed = decoded
        .value
        .trim_matches(|character: char| character == '\0' || character.is_whitespace());
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// 规范化一个元素的副本，供 DICOM JSON 子集序列化使用。
pub fn normalized_text_element(object: &InMemDicomObject, element: &InMemElement) -> InMemElement {
    let decoder = DicomTextDecoder::for_object(object, None);
    decoder
        .normalized_element(element)
        .map(|normalized| normalized.element)
        .unwrap_or_else(|| element.clone())
}

#[derive(Debug, Clone)]
enum DecodePlan {
    /// dicom-rs 已按单一声明完成解码；仍会用可逆编码恢复原字节做兜底验证。
    Declared { charset: SpecificCharacterSet },
    /// 默认字符集只允许 ASCII。非 ASCII 是发送方未声明或声明错误。
    StrictDefault { declaration_missing: bool },
    /// 多值 ISO-2022 声明需要从 dicom-rs 可逆的中间字符串重新解码。
    Redecode {
        charset: SpecificCharacterSet,
        korean_iso_2022: bool,
    },
    /// 声明无法识别，先生成安全替换结果，再交给保守兜底做交叉验证。
    Unsupported,
}

#[derive(Debug, Clone)]
struct DicomTextDecoder {
    plan: DecodePlan,
    unsupported: bool,
}

#[derive(Debug)]
struct DecodedElement {
    value: String,
    primitive: PrimitiveValue,
    changed: bool,
    had_replacement: bool,
    assumed_utf8: bool,
    used_fallback: bool,
}

#[derive(Debug)]
struct NormalizedElement {
    element: InMemElement,
    changed: bool,
    had_replacement: bool,
    assumed_utf8: bool,
    used_fallback: bool,
}

#[derive(Debug)]
struct DecodedValue {
    value: String,
    changed: bool,
    had_replacement: bool,
    assumed_utf8: bool,
    used_fallback: bool,
}

impl DicomTextDecoder {
    fn for_object(object: &InMemDicomObject, inherited: Option<&Self>) -> Self {
        let Some(declarations) = character_set_declarations(object) else {
            return inherited.cloned().unwrap_or(Self {
                plan: DecodePlan::StrictDefault {
                    declaration_missing: true,
                },
                unsupported: false,
            });
        };

        let declarations: Vec<_> = declarations
            .into_iter()
            .map(|value| value.trim().to_owned())
            .collect();
        if declarations.len() > 1
            && let Some((charset, korean_iso_2022)) = extension_charset(&declarations)
        {
            return Self {
                plan: DecodePlan::Redecode {
                    charset,
                    korean_iso_2022,
                },
                unsupported: false,
            };
        }

        let first = declarations.first().map(String::as_str).unwrap_or("");
        if first.is_empty() || is_default_character_set(first) {
            return Self {
                plan: DecodePlan::StrictDefault {
                    declaration_missing: false,
                },
                unsupported: false,
            };
        }
        if let Some(charset) = SpecificCharacterSet::from_code(first) {
            Self {
                plan: DecodePlan::Declared { charset },
                unsupported: false,
            }
        } else {
            Self {
                plan: DecodePlan::Unsupported,
                unsupported: true,
            }
        }
    }

    fn decode_element(&self, element: &InMemElement) -> Option<DecodedElement> {
        if !is_textual_vr(element.vr()) {
            return None;
        }
        let decode = |value: &str| {
            if uses_specific_character_set(element.vr()) {
                self.decode_value(value)
            } else {
                sanitize(value, false)
            }
        };
        let primitive = element.value().primitive()?;
        match primitive {
            PrimitiveValue::Str(value) => {
                let decoded = decode(value);
                Some(DecodedElement {
                    value: decoded.value.clone(),
                    primitive: PrimitiveValue::Str(decoded.value),
                    changed: decoded.changed,
                    had_replacement: decoded.had_replacement,
                    assumed_utf8: decoded.assumed_utf8,
                    used_fallback: decoded.used_fallback,
                })
            }
            PrimitiveValue::Strs(values) => {
                let decoded: Vec<_> = values.iter().map(|value| decode(value)).collect();
                let changed = decoded.iter().any(|value| value.changed);
                let had_replacement = decoded.iter().any(|value| value.had_replacement);
                let assumed_utf8 = decoded.iter().any(|value| value.assumed_utf8);
                let used_fallback = decoded.iter().any(|value| value.used_fallback);
                let strings: Vec<String> = decoded.into_iter().map(|value| value.value).collect();
                Some(DecodedElement {
                    value: strings.join("\\"),
                    primitive: PrimitiveValue::Strs(strings.into()),
                    changed,
                    had_replacement,
                    assumed_utf8,
                    used_fallback,
                })
            }
            _ => None,
        }
    }

    fn normalized_element(&self, element: &InMemElement) -> Option<NormalizedElement> {
        let decoded = self.decode_element(element)?;
        Some(NormalizedElement {
            element: DataElement::new(element.header().tag, element.vr(), decoded.primitive),
            changed: decoded.changed,
            had_replacement: decoded.had_replacement,
            assumed_utf8: decoded.assumed_utf8,
            used_fallback: decoded.used_fallback,
        })
    }

    fn decode_value(&self, source: &str) -> DecodedValue {
        let (primary, declared_charset) = match &self.plan {
            DecodePlan::Declared { charset } => (sanitize(source, false), Some(charset)),
            DecodePlan::StrictDefault {
                declaration_missing,
            } => (decode_default(source, *declaration_missing), None),
            DecodePlan::Redecode {
                charset,
                korean_iso_2022,
            } => (redecode(source, charset, *korean_iso_2022), Some(charset)),
            DecodePlan::Unsupported => (replace_undeclared_non_ascii(source), None),
        };
        decode_with_fallback(source, primary, declared_charset)
    }
}

fn normalize_object(
    object: &mut InMemDicomObject,
    inherited: Option<&DicomTextDecoder>,
    root: bool,
    report: &mut TextNormalizationReport,
) {
    let had_own_declaration = object.get(tags::SPECIFIC_CHARACTER_SET).is_some();
    let decoder = DicomTextDecoder::for_object(object, inherited);
    report.unsupported_character_set |= decoder.unsupported;

    let element_tags: Vec<Tag> = object.iter().map(|element| element.header().tag).collect();
    for tag in element_tags {
        if tag == tags::SPECIFIC_CHARACTER_SET {
            continue;
        }

        let normalized = object
            .get(tag)
            .and_then(|element| decoder.normalized_element(element));
        if let Some(normalized) = normalized {
            report.text_elements += 1;
            report.corrected_values += usize::from(normalized.changed);
            report.values_with_replacement += usize::from(normalized.had_replacement);
            report.assumed_utf8_values += usize::from(normalized.assumed_utf8);
            report.fallback_decoded_values += usize::from(normalized.used_fallback);
            object.put(normalized.element);
            continue;
        }

        object.update_value(tag, |value| {
            if let DicomValue::Sequence(sequence) = value {
                for item in sequence.items_mut() {
                    normalize_object(item, Some(&decoder), false, report);
                }
            }
        });
    }

    if root || had_own_declaration {
        object.convert_to_utf8();
    }
}

fn uses_specific_character_set(vr: VR) -> bool {
    matches!(
        vr,
        VR::LO | VR::LT | VR::PN | VR::SH | VR::ST | VR::UC | VR::UT
    )
}

fn is_textual_vr(vr: VR) -> bool {
    matches!(
        vr,
        VR::AE
            | VR::AS
            | VR::CS
            | VR::DA
            | VR::DS
            | VR::DT
            | VR::IS
            | VR::LO
            | VR::LT
            | VR::PN
            | VR::SH
            | VR::ST
            | VR::UC
            | VR::UI
            | VR::UR
            | VR::UT
            | VR::TM
    )
}

fn character_set_declarations(object: &InMemDicomObject) -> Option<Vec<String>> {
    let value = object
        .get(tags::SPECIFIC_CHARACTER_SET)?
        .value()
        .primitive()?;
    match value {
        PrimitiveValue::Str(value) => Some(value.split('\\').map(str::to_owned).collect()),
        PrimitiveValue::Strs(values) => Some(values.iter().cloned().collect()),
        PrimitiveValue::Empty => Some(vec![String::new()]),
        _ => None,
    }
}

fn is_default_character_set(code: &str) -> bool {
    matches!(
        code.trim(),
        "Default" | "ISO_IR_6" | "ISO_IR 6" | "ISO 2022 IR 6"
    )
}

fn extension_charset(declarations: &[String]) -> Option<(SpecificCharacterSet, bool)> {
    // ISO-2022-JP 负责解释转义序列，优先于同一声明中的单字节日文集。
    if let Some(charset) = declarations
        .iter()
        .find(|code| matches!(code.trim(), "ISO_IR_87" | "ISO_IR 87" | "ISO 2022 IR 87"))
        .and_then(|code| SpecificCharacterSet::from_code(code))
    {
        return Some((charset, false));
    }
    if let Some(charset) = declarations
        .iter()
        .find(|code| matches!(code.trim(), "ISO_IR_149" | "ISO_IR 149" | "ISO 2022 IR 149"))
        .and_then(|code| SpecificCharacterSet::from_code(code))
    {
        return Some((charset, true));
    }
    declarations
        .iter()
        .skip(1)
        .find_map(|code| SpecificCharacterSet::from_code(code).map(|charset| (charset, false)))
}

fn decode_default(source: &str, declaration_missing: bool) -> DecodedValue {
    let Some(raw) = recover_bytes(source) else {
        return replace_undeclared_non_ascii(source);
    };
    if raw.iter().all(u8::is_ascii) {
        return sanitize(source, false);
    }

    // 没有声明时先接受能被严格证明为 UTF-8 的输入；GB18030 候选会在后续经过
    // 替换字符与 CJK 可信度检查，避免无条件猜测患者姓名。
    if declaration_missing && let Ok(decoded) = String::from_utf8(raw.clone()) {
        let mut value = sanitize(&decoded, true);
        value.changed = value.changed || decoded != source;
        value.assumed_utf8 = true;
        value.used_fallback = true;
        return value;
    }

    let lossy = String::from_utf8_lossy(&raw).into_owned();
    let mut value = sanitize(&lossy, true);
    value.changed = true;
    value.had_replacement = true;
    value
}

fn redecode(source: &str, charset: &SpecificCharacterSet, korean_iso_2022: bool) -> DecodedValue {
    let Some(mut raw) = recover_bytes(source) else {
        return sanitize(source, false);
    };
    if korean_iso_2022 && raw.contains(&0x1B) {
        raw = korean_iso_2022_to_cp949(&raw);
    }
    match charset.decode(&raw) {
        Ok(decoded) => {
            let mut value = sanitize(&decoded, true);
            value.changed = value.changed || decoded != source;
            value
        }
        Err(_) => DecodedValue {
            value: "�".to_owned(),
            changed: true,
            had_replacement: true,
            assumed_utf8: false,
            used_fallback: false,
        },
    }
}

/// 对声明字符集的结果做保守交叉验证。
///
/// dicom-rs 已经在解析时应用了 SpecificCharacterSet。声明错误但恰好能解码时，
/// 得到的 Unicode 看起来可能完全“合法”，所以这里先按同一声明反向编码来恢复原始
/// 字节，再尝试两种本项目实际会遇到的互操作兜底：严格 UTF-8 与 GB18030。
fn decode_with_fallback(
    source: &str,
    primary: DecodedValue,
    declared_charset: Option<&SpecificCharacterSet>,
) -> DecodedValue {
    let Some(raw) = recover_bytes(source)
        .or_else(|| declared_charset.and_then(|charset| recover_declared_bytes(source, charset)))
    else {
        return primary;
    };
    if raw.iter().all(u8::is_ascii) {
        return primary;
    }

    // 严格 UTF-8 自带完整的字节合法性校验。若它能无损解码，与声明结果冲突时
    // 优先 UTF-8，可修复“实际 UTF-8、却声明为 GBK/Latin-1”的常见设备错误。
    if let Ok(decoded) = String::from_utf8(raw.clone()) {
        let mut candidate = sanitize(&decoded, true);
        if !candidate.had_replacement && candidate.value != primary.value {
            candidate.assumed_utf8 = true;
            candidate.used_fallback = true;
            return candidate;
        }
    }

    let gb18030 = SpecificCharacterSet::from_code("GB18030").expect("dicom-rs 必须支持 GB18030");
    let Ok(decoded) = gb18030.decode(&raw) else {
        return primary;
    };
    let mut candidate = sanitize(&decoded, true);
    if candidate.value != primary.value
        && !candidate.had_replacement
        && should_prefer_gb18030(&primary, &candidate.value)
    {
        candidate.used_fallback = true;
        return candidate;
    }

    primary
}

fn should_prefer_gb18030(primary: &DecodedValue, candidate: &str) -> bool {
    let cjk_characters = candidate
        .chars()
        .filter(|character| is_cjk(*character))
        .count();
    if cjk_characters < 2 {
        return false;
    }
    if primary.had_replacement {
        return true;
    }

    // 单字节西欧字符集会把 GBK 的每个原始字节都映射成 U+0080..U+00FF，形成
    // “ËÎÔÆ»Ô”这类高密度扩展 Latin-1。只有 GB18030 候选能组成至少两个汉字时
    // 才覆盖声明结果，避免把正常的 Günther 等姓名误判成中文。
    let non_ascii = primary
        .value
        .chars()
        .filter(|character| !character.is_ascii())
        .count();
    let extended_latin = primary
        .value
        .chars()
        .filter(|character| matches!(*character as u32, 0x80..=0xFF))
        .count();
    extended_latin >= 2 && extended_latin * 2 >= non_ascii.max(1)
}

fn is_cjk(character: char) -> bool {
    matches!(
        character as u32,
        0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF
            | 0x20000..=0x2FA1F
    )
}

/// 将 DICOM 的 ISO 2022 IR 149 七位代码扩展还原成 CP949/EUC-KR 字节。
fn korean_iso_2022_to_cp949(source: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(source.len());
    let mut index = 0;
    let mut korean = false;
    while index < source.len() {
        if source.get(index..index + 4) == Some(b"\x1B$)C") {
            index += 4;
            continue;
        }
        match source[index] {
            0x0E => {
                korean = true;
                index += 1;
            }
            0x0F => {
                korean = false;
                index += 1;
            }
            byte if korean && (0x21..=0x7E).contains(&byte) => {
                output.push(byte | 0x80);
                index += 1;
            }
            byte => {
                output.push(byte);
                index += 1;
            }
        }
    }
    output
}

/// dicom-rs 的解码错误陷阱会把坏字节写成 `\ooo`；ISO-8859-1 中间结果则可由
/// U+0000..U+00FF 无损还原。二者都在这里恢复成原始字节供补救路径使用。
fn recover_bytes(source: &str) -> Option<Vec<u8>> {
    let chars: Vec<char> = source.chars().collect();
    let mut bytes = Vec::with_capacity(chars.len());
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '\\' && index + 3 < chars.len() {
            let digits = &chars[index + 1..index + 4];
            if digits.iter().all(|value| matches!(value, '0'..='7')) {
                let value = digits.iter().fold(0u16, |acc, digit| {
                    acc * 8 + digit.to_digit(8).expect("已验证为八进制") as u16
                });
                if value <= u8::MAX as u16 {
                    bytes.push(value as u8);
                    index += 4;
                    continue;
                }
            }
        }
        let code = chars[index] as u32;
        bytes.push(u8::try_from(code).ok()?);
        index += 1;
    }
    Some(bytes)
}

/// 按声明字符集反向编码，并把 dicom-rs 的 `\ooo` 解码陷阱逐字节拼回去。
///
/// 错误声明可能让一部分字节成功解成 Unicode、另一部分变成八进制转义。整串调用
/// `encode` 会被转义部分阻断，因此必须分段恢复才能重新尝试真实字符集。
fn recover_declared_bytes(source: &str, charset: &SpecificCharacterSet) -> Option<Vec<u8>> {
    let chars: Vec<char> = source.chars().collect();
    let mut bytes = Vec::with_capacity(source.len());
    let mut plain = String::new();
    let mut index = 0;

    let flush_plain = |plain: &mut String, bytes: &mut Vec<u8>| -> Option<()> {
        if !plain.is_empty() {
            bytes.extend(charset.encode(plain).ok()?);
            plain.clear();
        }
        Some(())
    };

    while index < chars.len() {
        if chars[index] == '\\' && index + 3 < chars.len() {
            let digits = &chars[index + 1..index + 4];
            if digits.iter().all(|value| matches!(value, '0'..='7')) {
                let value = digits.iter().fold(0u16, |acc, digit| {
                    acc * 8 + digit.to_digit(8).expect("已验证为八进制") as u16
                });
                if value <= u8::MAX as u16 {
                    flush_plain(&mut plain, &mut bytes)?;
                    bytes.push(value as u8);
                    index += 4;
                    continue;
                }
            }
        }
        plain.push(chars[index]);
        index += 1;
    }
    flush_plain(&mut plain, &mut bytes)?;
    Some(bytes)
}

fn replace_undeclared_non_ascii(source: &str) -> DecodedValue {
    let mut changed = false;
    let mut output = String::with_capacity(source.len());
    for character in source.chars() {
        if character.is_ascii() {
            output.push(character);
        } else {
            output.push('�');
            changed = true;
        }
    }
    let mut value = sanitize(&output, changed);
    value.changed |= changed;
    value.had_replacement |= changed;
    value
}

fn sanitize(source: &str, already_changed: bool) -> DecodedValue {
    let chars: Vec<char> = source.chars().collect();
    let mut output = String::with_capacity(source.len());
    let mut index = 0;
    let mut replacement = false;
    while index < chars.len() {
        if chars[index] == '\\' && index + 3 < chars.len() {
            let digits = &chars[index + 1..index + 4];
            if digits.iter().all(|value| matches!(value, '0'..='7')) {
                output.push('�');
                replacement = true;
                index += 4;
                continue;
            }
        }

        let character = chars[index];
        if character == '�'
            || (character.is_control() && !matches!(character, '\t' | '\n' | '\r' | '\0'))
        {
            output.push('�');
            replacement = true;
        } else {
            output.push(character);
        }
        index += 1;
    }
    DecodedValue {
        changed: already_changed || replacement || output != source,
        value: output,
        had_replacement: replacement,
        assumed_utf8: false,
        used_fallback: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dicom::encoding::TransferSyntaxIndex;
    use dicom::object::InMemDicomObject;
    use dicom::transfer_syntax::TransferSyntaxRegistry;

    fn explicit_element(tag: Tag, vr: &[u8; 2], value: &[u8]) -> Vec<u8> {
        assert!(value.len() <= u16::MAX as usize);
        let mut bytes = Vec::with_capacity(8 + value.len() + 1);
        bytes.extend_from_slice(&tag.group().to_le_bytes());
        bytes.extend_from_slice(&tag.element().to_le_bytes());
        bytes.extend_from_slice(vr);
        let padded_len = value.len() + value.len() % 2;
        bytes.extend_from_slice(&(padded_len as u16).to_le_bytes());
        bytes.extend_from_slice(value);
        if value.len() % 2 == 1 {
            bytes.push(b' ');
        }
        bytes
    }

    fn parse_dataset(character_set: Option<&[u8]>, patient_name: &[u8]) -> InMemDicomObject {
        let mut bytes = Vec::new();
        if let Some(character_set) = character_set {
            bytes.extend(explicit_element(
                tags::SPECIFIC_CHARACTER_SET,
                b"CS",
                character_set,
            ));
        }
        bytes.extend(explicit_element(tags::PATIENT_NAME, b"PN", patient_name));
        let ts = TransferSyntaxRegistry
            .get("1.2.840.10008.1.2.1")
            .expect("显式 VR 小端应存在");
        InMemDicomObject::read_dataset_with_ts(bytes.as_slice(), ts).expect("测试数据集应能解析")
    }

    #[test]
    fn decodes_utf8_gb18030_gbk_and_latin1() {
        for (charset, raw, expected) in [
            ("ISO_IR 192", "张金德".as_bytes(), "张金德"),
            ("GB18030", b"\xD5\xC5\xBD\xF0\xB5\xC2".as_slice(), "张金德"),
            ("GBK", b"\xD5\xC5\xBD\xF0\xB5\xC2".as_slice(), "张金德"),
            ("ISO_IR 100", b"G\xFCnther^Hans".as_slice(), "Günther^Hans"),
        ] {
            let object = parse_dataset(Some(charset.as_bytes()), raw);
            assert_eq!(
                utf8_text(&object, tags::PATIENT_NAME).as_deref(),
                Some(expected)
            );
        }
    }

    #[test]
    fn decodes_japanese_and_korean() {
        let japanese = parse_dataset(Some(b"ISO 2022 IR 87"), b"\x1B$B;3ED\x1B(B^\x1B$BB@O:");
        assert_eq!(
            utf8_text(&japanese, tags::PATIENT_NAME).as_deref(),
            Some("山田^太郎")
        );

        let korean = parse_dataset(Some(b"ISO 2022 IR 149"), b"\xB1\xE8\xC8\xF1\xC1\xDF");
        assert_eq!(
            utf8_text(&korean, tags::PATIENT_NAME).as_deref(),
            Some("김희중")
        );
    }

    #[test]
    fn decodes_multi_value_iso_2022_declarations() {
        let japanese = parse_dataset(Some(b"\\ISO 2022 IR 87"), b"\x1B$B;3ED\x1B(B^\x1B$BB@O:");
        assert_eq!(
            utf8_text(&japanese, tags::PATIENT_NAME).as_deref(),
            Some("山田^太郎")
        );

        // ESC $ ) C 指定 KS X 1001，SO/SI 在韩文与 ASCII 之间切换。
        let korean = parse_dataset(
            Some(b"\\ISO 2022 IR 149"),
            b"Hong^Gildong=\x1B$)C\x0EH+1f5?\x0F",
        );
        assert_eq!(
            utf8_text(&korean, tags::PATIENT_NAME).as_deref(),
            Some("Hong^Gildong=홍길동")
        );
    }

    #[test]
    fn missing_charset_falls_back_to_utf8_or_gb18030() {
        let utf8 = parse_dataset(None, "张金德".as_bytes());
        assert_eq!(
            utf8_text(&utf8, tags::PATIENT_NAME).as_deref(),
            Some("张金德")
        );

        let gbk = parse_dataset(None, b"\xCB\xCE\xD4\xC6\xBB\xD4");
        assert_eq!(
            utf8_text(&gbk, tags::PATIENT_NAME).as_deref(),
            Some("宋云辉")
        );
    }

    #[test]
    fn fallback_overrides_wrong_or_unsupported_declarations() {
        let latin1_claiming_gbk = parse_dataset(Some(b"ISO_IR 100"), b"\xCB\xCE\xD4\xC6\xBB\xD4");
        assert_eq!(
            utf8_text(&latin1_claiming_gbk, tags::PATIENT_NAME).as_deref(),
            Some("宋云辉")
        );

        let gb18030_claiming_utf8 = parse_dataset(Some(b"GB18030"), "张金德".as_bytes());
        assert_eq!(
            utf8_text(&gb18030_claiming_utf8, tags::PATIENT_NAME).as_deref(),
            Some("张金德")
        );

        let unsupported = parse_dataset(Some(b"VENDOR_UNKNOWN"), "张金德".as_bytes());
        assert_eq!(
            utf8_text(&unsupported, tags::PATIENT_NAME).as_deref(),
            Some("张金德")
        );
    }

    #[test]
    fn invalid_input_never_surfaces_mojibake_or_octal_escapes() {
        let invalid = parse_dataset(Some(b"ISO_IR 192"), b"A\xFFB");
        assert_eq!(
            utf8_text(&invalid, tags::PATIENT_NAME).as_deref(),
            Some("A�B")
        );
    }

    #[test]
    fn normalization_marks_dataset_as_utf8_and_json_is_clean() {
        let mut object = parse_dataset(Some(b"GB18030"), b"\xD5\xC5\xBD\xF0\xB5\xC2");
        let report = normalize_dataset_text(&mut object);
        assert_eq!(report.values_with_replacement, 0);
        assert_eq!(
            object
                .get(tags::SPECIFIC_CHARACTER_SET)
                .unwrap()
                .to_str()
                .unwrap(),
            "ISO_IR 192"
        );
        let json = dicom_json::to_string(&object).expect("规范化对象应能序列化");
        assert!(json.contains("张金德"));
        assert!(!json.contains("å¼"));
    }

    #[test]
    fn normalization_reports_gb18030_fallback() {
        let mut object = parse_dataset(None, b"\xCB\xCE\xD4\xC6\xBB\xD4");
        let report = normalize_dataset_text(&mut object);
        assert_eq!(report.fallback_decoded_values, 1);
        assert_eq!(report.values_with_replacement, 0);
        assert_eq!(
            object.get(tags::PATIENT_NAME).unwrap().to_str().unwrap(),
            "宋云辉"
        );
    }
}
