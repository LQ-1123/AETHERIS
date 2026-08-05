//! 四层领域模型:Patient / Study / Series / Instance。
//!
//! 每层都是"结构化字段 + 一份 DICOM JSON 属性子集":结构化字段进独立数据库列,
//! 负责索引和 C-FIND 匹配;`attributes` 保留该层的原始属性(标准 DICOM JSON
//! Model),让 QIDO-RS 可以直接回传、新增查询键时不用改表。

use chrono::{NaiveDate, NaiveTime};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::uid::Uid;

/// 病人层。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PatientMeta {
    /// (0010,0020) PatientID。DICOM 里是 Type 2(可以存在但为空),
    /// 设备不提供时这里是空串。
    pub patient_id: String,
    /// (0010,0021) IssuerOfPatientID
    pub issuer_of_patient_id: Option<String>,
    /// (0010,0010) PatientName 原始值,保留 `^` 分隔与各字符集组。
    pub name: Option<String>,
    /// 匹配用的规范化姓名,见 [`normalize_person_name`]。
    pub name_normalized: Option<String>,
    /// (0010,0030) PatientBirthDate
    pub birth_date: Option<NaiveDate>,
    /// (0010,0040) PatientSex
    pub sex: Option<String>,
    pub attributes: Value,
}

/// 检查层。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StudyMeta {
    /// (0020,000D) StudyInstanceUID
    pub uid: Uid,
    /// (0008,0020) StudyDate
    pub date: Option<NaiveDate>,
    /// (0008,0030) StudyTime
    pub time: Option<NaiveTime>,
    /// (0008,0050) AccessionNumber
    pub accession_number: Option<String>,
    /// (0020,0010) StudyID
    pub study_id: Option<String>,
    /// (0008,1030) StudyDescription
    pub description: Option<String>,
    /// (0008,0090) ReferringPhysicianName
    pub referring_physician: Option<String>,
    pub attributes: Value,
}

/// 序列层。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeriesMeta {
    /// (0020,000E) SeriesInstanceUID
    pub uid: Uid,
    /// (0020,0011) SeriesNumber
    pub number: Option<i32>,
    /// (0008,0060) Modality
    pub modality: Option<String>,
    /// (0008,103E) SeriesDescription
    pub description: Option<String>,
    /// (0018,0015) BodyPartExamined
    pub body_part_examined: Option<String>,
    /// (0018,1030) ProtocolName
    pub protocol_name: Option<String>,
    /// (0008,0021) SeriesDate
    pub date: Option<NaiveDate>,
    /// (0008,0031) SeriesTime
    pub time: Option<NaiveTime>,
    pub attributes: Value,
}

/// 实例层。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstanceMeta {
    /// (0008,0018) SOPInstanceUID
    pub uid: Uid,
    /// (0008,0016) SOPClassUID
    pub sop_class_uid: Option<Uid>,
    /// (0020,0013) InstanceNumber
    pub number: Option<i32>,
    /// 文件元信息里的传输语法,决定像素数据怎么解码。
    pub transfer_syntax_uid: Uid,
    /// (0028,0010) Rows
    pub rows: Option<i32>,
    /// (0028,0011) Columns
    pub columns: Option<i32>,
    /// (0028,0008) NumberOfFrames
    pub number_of_frames: Option<i32>,
    /// (0020,0032) ImagePositionPatient —— CT 序列排序靠它,不靠 InstanceNumber。
    pub image_position_patient: Option<Vec<f64>>,
    /// (0020,0037) ImageOrientationPatient —— 与上者一起算切片法向量。
    pub image_orientation_patient: Option<Vec<f64>>,
    pub attributes: Value,
}

/// 从一个 DICOM 文件解析出的完整四层元数据。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstanceMetadata {
    pub patient: PatientMeta,
    pub study: StudyMeta,
    pub series: SeriesMeta,
    pub instance: InstanceMeta,
}

/// 把 DICOM PN 规范化成用于匹配的形式。
///
/// PN 的结构是 `姓^名^中间名^前缀^后缀`,最多三组(字母组=表意组=读音组)用 `=`
/// 分隔。这里只取字母组,去掉尾部的空分量再转大写:
///
/// - 去尾部空分量是必需的 —— `DOE^JOHN^^^` 和 `DOE^JOHN` 是同一个人,
///   不归一化的话精确匹配 `DOE^JOHN` 会漏掉前者。
/// - 转大写是为了大小写不敏感匹配。标准规定 PN 匹配大小写敏感,但实际设备和
///   人工录入的大小写不可靠,主流 PACS 都做不敏感匹配。
pub fn normalize_person_name(raw: &str) -> String {
    let alphabetic = raw.split('=').next().unwrap_or_default();
    let mut parts: Vec<&str> = alphabetic.split('^').map(str::trim).collect();
    while parts.last().is_some_and(|p| p.is_empty()) {
        parts.pop();
    }
    parts.join("^").to_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_trailing_empty_components() {
        assert_eq!(normalize_person_name("Doe^John^^^"), "DOE^JOHN");
        assert_eq!(normalize_person_name("Doe^John"), "DOE^JOHN");
        assert_eq!(normalize_person_name("Doe"), "DOE");
    }

    #[test]
    fn keeps_interior_empty_components() {
        // 中间名为空但有后缀,不能把中间的空位挤掉,否则分量语义就错位了
        assert_eq!(
            normalize_person_name("Doe^John^^Dr^III"),
            "DOE^JOHN^^DR^III"
        );
    }

    #[test]
    fn uses_alphabetic_group_only() {
        // 字母组=表意组=读音组;匹配只用字母组
        assert_eq!(
            normalize_person_name("Yamada^Tarou=山田^太郎=やまだ^たろう"),
            "YAMADA^TAROU"
        );
    }

    #[test]
    fn handles_empty_and_whitespace() {
        assert_eq!(normalize_person_name(""), "");
        assert_eq!(normalize_person_name("^^^^"), "");
        assert_eq!(normalize_person_name(" Doe ^ John "), "DOE^JOHN");
    }

    #[test]
    fn non_ascii_names_pass_through() {
        // 中文姓名没有大小写概念,应原样保留
        assert_eq!(normalize_person_name("张^三"), "张^三");
    }
}
