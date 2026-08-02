//! 测试用的合成 DICOM 对象。
//!
//! 由 `fixtures` feature 开启,供本 crate 及 `pacs-store`/`pacs-db` 的测试使用。
//! 用程序生成而不是往仓库里塞 `.dcm` 样例文件:测试不依赖外部数据、不会
//! 误传真实病人影像,而且要构造异常头(缺 UID、畸形日期)时改几行就行。

use dicom::core::{DataElement, PrimitiveValue, VR};
use dicom::dictionary_std::{tags, uids};
use dicom::object::{DefaultDicomObject, FileMetaTableBuilder, InMemDicomObject};
use uuid::Uuid;

/// 生成一个全局唯一的 UID。
///
/// 用 PS3.5 §B.2 的 UUID 派生法:`2.25.` 加 UUID 的十进制整数值。
/// 无需申请组织根,且天然不重复 —— 并行测试各写各的行,互不干扰。
pub fn unique_uid() -> String {
    format!("2.25.{}", Uuid::new_v4().as_u128())
}

/// 多值字符串。
///
/// 用 `&str` 直接建元素会得到单个 `Str`,而真实文件解析出来的是按 `\` 拆开的
/// `Strs` —— 夹具必须和解析器保持一致,否则测出来的是夹具而不是产品代码。
fn multi(values: &[&str]) -> PrimitiveValue {
    PrimitiveValue::Strs(values.iter().map(|s| (*s).to_owned()).collect())
}

/// 一个最小但结构完整的 CT 实例,像素数据是 4×4 的 16 位灰度占位。
///
/// 头信息覆盖了四层各自的结构化字段和查看器渲染要用的标签。
pub fn ct_instance(study_uid: &str, series_uid: &str, sop_uid: &str) -> DefaultDicomObject {
    ct_instance_sized(study_uid, series_uid, sop_uid, 4)
}

/// 同上,但可以指定边长,用于需要真实数据量的场景(如吞吐基准)。
///
/// `side = 512` 对应真实 CT 断层的 512×512×16bit ≈ 512 KiB,
/// 这个量级下磁盘和数据库的开销占比才接近生产环境 —— 拿 4×4 测吞吐
/// 量到的几乎全是固定开销,得出的数字没有参考价值。
pub fn ct_instance_sized(
    study_uid: &str,
    series_uid: &str,
    sop_uid: &str,
    side: u16,
) -> DefaultDicomObject {
    let obj = InMemDicomObject::from_element_iter([
        DataElement::new(tags::SPECIFIC_CHARACTER_SET, VR::CS, "ISO_IR 192"),
        // 病人层
        DataElement::new(tags::PATIENT_NAME, VR::PN, "Doe^John^^^"),
        DataElement::new(tags::PATIENT_ID, VR::LO, "PID-0001"),
        DataElement::new(tags::PATIENT_BIRTH_DATE, VR::DA, "19800115"),
        DataElement::new(tags::PATIENT_SEX, VR::CS, "M"),
        // 检查层
        DataElement::new(tags::STUDY_INSTANCE_UID, VR::UI, study_uid),
        DataElement::new(tags::STUDY_DATE, VR::DA, "20240315"),
        DataElement::new(tags::STUDY_TIME, VR::TM, "142530"),
        DataElement::new(tags::ACCESSION_NUMBER, VR::SH, "ACC-42"),
        DataElement::new(tags::STUDY_ID, VR::SH, "S1"),
        DataElement::new(tags::STUDY_DESCRIPTION, VR::LO, "CHEST CT"),
        DataElement::new(tags::REFERRING_PHYSICIAN_NAME, VR::PN, "Smith^Jane"),
        // 序列层
        DataElement::new(tags::SERIES_INSTANCE_UID, VR::UI, series_uid),
        DataElement::new(tags::SERIES_NUMBER, VR::IS, "2"),
        DataElement::new(tags::MODALITY, VR::CS, "CT"),
        DataElement::new(tags::SERIES_DESCRIPTION, VR::LO, "AXIAL"),
        DataElement::new(tags::BODY_PART_EXAMINED, VR::CS, "CHEST"),
        // 实例层
        DataElement::new(tags::SOP_CLASS_UID, VR::UI, uids::CT_IMAGE_STORAGE),
        DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, sop_uid),
        DataElement::new(tags::INSTANCE_NUMBER, VR::IS, "1"),
        DataElement::new(
            tags::IMAGE_POSITION_PATIENT,
            VR::DS,
            multi(&["-120.5", "-130.0", "-45.25"]),
        ),
        DataElement::new(
            tags::IMAGE_ORIENTATION_PATIENT,
            VR::DS,
            multi(&["1", "0", "0", "0", "1", "0"]),
        ),
        DataElement::new(tags::PIXEL_SPACING, VR::DS, multi(&["0.6836", "0.6836"])),
        DataElement::new(tags::SLICE_THICKNESS, VR::DS, "5.0"),
        // 显示管线:CT 必须有 Rescale 才能得到正确的 HU
        DataElement::new(tags::RESCALE_INTERCEPT, VR::DS, "-1024"),
        DataElement::new(tags::RESCALE_SLOPE, VR::DS, "1"),
        DataElement::new(tags::RESCALE_TYPE, VR::LO, "HU"),
        DataElement::new(tags::WINDOW_CENTER, VR::DS, "-600"),
        DataElement::new(tags::WINDOW_WIDTH, VR::DS, "1500"),
        // 像素几何
        DataElement::new(tags::SAMPLES_PER_PIXEL, VR::US, PrimitiveValue::from(1_u16)),
        DataElement::new(tags::PHOTOMETRIC_INTERPRETATION, VR::CS, "MONOCHROME2"),
        DataElement::new(tags::ROWS, VR::US, PrimitiveValue::from(side)),
        DataElement::new(tags::COLUMNS, VR::US, PrimitiveValue::from(side)),
        DataElement::new(tags::BITS_ALLOCATED, VR::US, PrimitiveValue::from(16_u16)),
        DataElement::new(tags::BITS_STORED, VR::US, PrimitiveValue::from(16_u16)),
        DataElement::new(tags::HIGH_BIT, VR::US, PrimitiveValue::from(15_u16)),
        DataElement::new(
            tags::PIXEL_REPRESENTATION,
            VR::US,
            PrimitiveValue::from(0_u16),
        ),
        DataElement::new(
            tags::PIXEL_DATA,
            VR::OW,
            PrimitiveValue::U16(vec![0_u16; usize::from(side) * usize::from(side)].into()),
        ),
    ]);

    obj.with_meta(
        FileMetaTableBuilder::new()
            .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
            .implementation_class_uid("2.25.1")
            .implementation_version_name("REMOTE_PACS_TEST"),
    )
    .expect("夹具的文件元信息应当可构造")
}

/// 一组同属一个序列的实例,SOPInstanceUID 各不相同。
pub fn ct_series(study_uid: &str, series_uid: &str, count: usize) -> Vec<DefaultDicomObject> {
    (0..count)
        .map(|_| ct_instance(study_uid, series_uid, &unique_uid()))
        .collect()
}
