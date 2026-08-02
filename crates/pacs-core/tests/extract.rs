//! 元数据提取的端到端测试:合成 DICOM 对象 → 四层结构化字段 + 属性 JSON。

use dicom::core::{DataElement, VR};
use dicom::dictionary_std::tags;
use pacs_core::fixture::{ct_instance, unique_uid};
use pacs_core::{ExtractError, extract_metadata};

#[test]
fn extracts_all_four_levels() {
    let (study, series, sop) = (unique_uid(), unique_uid(), unique_uid());
    let meta = extract_metadata(&ct_instance(&study, &series, &sop)).expect("夹具应能提取");

    assert_eq!(meta.patient.patient_id, "PID-0001");
    assert_eq!(meta.patient.name.as_deref(), Some("Doe^John^^^"));
    // 尾部空分量在规范化时去掉,原始值保持不动
    assert_eq!(meta.patient.name_normalized.as_deref(), Some("DOE^JOHN"));
    assert_eq!(meta.patient.sex.as_deref(), Some("M"));
    assert_eq!(
        meta.patient.birth_date,
        chrono::NaiveDate::from_ymd_opt(1980, 1, 15)
    );

    assert_eq!(meta.study.uid.as_str(), study);
    assert_eq!(
        meta.study.date,
        chrono::NaiveDate::from_ymd_opt(2024, 3, 15)
    );
    assert_eq!(meta.study.time, chrono::NaiveTime::from_hms_opt(14, 25, 30));
    assert_eq!(meta.study.accession_number.as_deref(), Some("ACC-42"));
    assert_eq!(meta.study.description.as_deref(), Some("CHEST CT"));

    assert_eq!(meta.series.uid.as_str(), series);
    assert_eq!(meta.series.modality.as_deref(), Some("CT"));
    assert_eq!(meta.series.number, Some(2));
    assert_eq!(meta.series.body_part_examined.as_deref(), Some("CHEST"));

    assert_eq!(meta.instance.uid.as_str(), sop);
    assert_eq!(meta.instance.number, Some(1));
    assert_eq!(meta.instance.rows, Some(4));
    assert_eq!(meta.instance.columns, Some(4));
    assert_eq!(
        meta.instance.transfer_syntax_uid.as_str(),
        "1.2.840.10008.1.2.1"
    );
    // CT 序列排序要靠它,不能丢
    assert_eq!(
        meta.instance.image_position_patient,
        Some(vec![-120.5, -130.0, -45.25])
    );
}

/// 夹具是在内存里拼出来的,真实数据要经过"编码写盘 → 解析读回"。
/// 这条路径上多值元素、数值 VR 的表示都可能变,提取结果必须一致 ——
/// 否则单测全绿而真实 C-STORE 进来的影像丢空间信息,CT 序列排序就错了。
#[test]
fn extraction_survives_a_file_round_trip() {
    let (study, series, sop) = (unique_uid(), unique_uid(), unique_uid());
    let original = ct_instance(&study, &series, &sop);

    let mut encoded = Vec::new();
    original.write_all(&mut encoded).expect("应能写出");
    let reparsed = dicom::object::from_reader(std::io::Cursor::new(&encoded)).expect("应能读回");

    assert_eq!(
        extract_metadata(&original).expect("内存对象应能提取"),
        extract_metadata(&reparsed).expect("读回的对象应能提取"),
        "写盘再读回后提取结果应完全一致"
    );
}

#[test]
fn attributes_use_dicom_json_model() {
    let obj = ct_instance(&unique_uid(), &unique_uid(), &unique_uid());
    let meta = extract_metadata(&obj).expect("夹具应能提取");

    // PS3.18 附录 F:键是 8 位十六进制标签,值带 vr 和 Value 数组
    assert_eq!(meta.study.attributes["00080020"]["vr"], "DA");
    assert_eq!(meta.study.attributes["00080020"]["Value"][0], "20240315");
    assert_eq!(meta.series.attributes["00080060"]["Value"][0], "CT");

    // 查看器渲染管线依赖的标签必须在实例层留下
    for (tag, what) in [
        ("00281052", "RescaleIntercept"),
        ("00281053", "RescaleSlope"),
        ("00281050", "WindowCenter"),
        ("00281051", "WindowWidth"),
        ("00280004", "PhotometricInterpretation"),
        ("00280030", "PixelSpacing"),
    ] {
        assert!(
            !meta.instance.attributes[tag].is_null(),
            "实例属性里应有 {what} ({tag})"
        );
    }

    // 分层是有意义的:检查层不该混进实例层的像素属性
    assert!(
        meta.study.attributes["00280010"].is_null(),
        "检查层不该有 Rows"
    );
    // 像素数据体积大,绝不进 JSON
    assert!(
        meta.instance.attributes["7FE00010"].is_null(),
        "属性里不该有 PixelData"
    );
}

#[test]
fn missing_study_uid_is_a_hard_error() {
    let mut obj = ct_instance(&unique_uid(), &unique_uid(), &unique_uid());
    obj.remove_element(tags::STUDY_INSTANCE_UID);

    assert_eq!(
        extract_metadata(&obj),
        Err(ExtractError::Missing {
            field: "StudyInstanceUID"
        })
    );
}

#[test]
fn malformed_uid_is_a_hard_error() {
    let mut obj = ct_instance(&unique_uid(), &unique_uid(), &unique_uid());
    // 路径穿越尝试:必须在入库前就被挡掉,而不是拿去拼文件路径
    obj.put(DataElement::new(
        tags::SERIES_INSTANCE_UID,
        VR::UI,
        "../../etc/passwd",
    ));

    assert!(matches!(
        extract_metadata(&obj),
        Err(ExtractError::InvalidUid {
            field: "SeriesInstanceUID",
            ..
        })
    ));
}

#[test]
fn imprecise_date_is_left_null_but_kept_raw() {
    let mut obj = ct_instance(&unique_uid(), &unique_uid(), &unique_uid());
    // 只有年份的出生日期在脱敏数据里很常见
    obj.put(DataElement::new(tags::PATIENT_BIRTH_DATE, VR::DA, "1980"));

    let meta = extract_metadata(&obj).expect("部分精度日期不该阻断入库");
    assert_eq!(meta.patient.birth_date, None, "不精确到日就不该编造日期");
    assert_eq!(
        meta.patient.attributes["00100030"]["Value"][0], "1980",
        "原始值仍应保留在属性里"
    );
}

#[test]
fn malformed_date_does_not_block_ingest() {
    let mut obj = ct_instance(&unique_uid(), &unique_uid(), &unique_uid());
    obj.put(DataElement::new(tags::STUDY_DATE, VR::DA, "NOT-A-DATE"));

    let meta = extract_metadata(&obj).expect("畸形日期不该丢掉整个检查");
    assert_eq!(meta.study.date, None);
    assert_eq!(meta.study.accession_number.as_deref(), Some("ACC-42"));
}

#[test]
fn empty_patient_id_is_allowed() {
    let mut obj = ct_instance(&unique_uid(), &unique_uid(), &unique_uid());
    // PatientID 是 Type 2,设备可以送一个空值
    obj.put(DataElement::new(tags::PATIENT_ID, VR::LO, ""));

    let meta = extract_metadata(&obj).expect("空 PatientID 是合法的");
    assert_eq!(meta.patient.patient_id, "");
}
