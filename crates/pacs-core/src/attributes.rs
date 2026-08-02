//! 每层保留哪些 DICOM 属性。
//!
//! 入库时按这几组标签,把该层的属性单独抽出来存成 DICOM JSON Model
//! (PS3.18 附录 F)。这样做的取舍:
//!
//! - **不存整份数据集**。一个 500 层的 CT,每个实例都塞一份完整头会重复上百倍;
//!   不在列表里的属性仍然留在磁盘上的 `.dcm` 文件里,WADO-RS 的 metadata
//!   端点按需从文件读。
//! - **选的是查询键和显示必需项**。QIDO-RS 各层级的返回属性、C-FIND 的匹配键、
//!   以及查看器渲染管线要用的窗宽窗位/Rescale/间距标定,都在里面。
//!
//! 需要新增查询键时改这里并重新入库,不用改表结构。

use dicom::core::Tag;
use dicom::dictionary_std::tags;

/// 病人层属性。
pub const PATIENT: &[Tag] = &[
    tags::SPECIFIC_CHARACTER_SET,
    tags::PATIENT_NAME,
    tags::PATIENT_ID,
    tags::ISSUER_OF_PATIENT_ID,
    tags::PATIENT_BIRTH_DATE,
    tags::PATIENT_SEX,
    tags::PATIENT_COMMENTS,
];

/// 检查层属性。
pub const STUDY: &[Tag] = &[
    tags::SPECIFIC_CHARACTER_SET,
    tags::STUDY_INSTANCE_UID,
    tags::STUDY_DATE,
    tags::STUDY_TIME,
    tags::ACCESSION_NUMBER,
    tags::STUDY_ID,
    tags::STUDY_DESCRIPTION,
    tags::REFERRING_PHYSICIAN_NAME,
    tags::NAME_OF_PHYSICIANS_READING_STUDY,
    tags::INSTITUTION_NAME,
    // 检查时点的病人状态,属于检查而不是病人 —— 病人年龄体重会变
    tags::PATIENT_AGE,
    tags::PATIENT_SIZE,
    tags::PATIENT_WEIGHT,
];

/// 序列层属性。
pub const SERIES: &[Tag] = &[
    tags::SPECIFIC_CHARACTER_SET,
    tags::SERIES_INSTANCE_UID,
    tags::SERIES_NUMBER,
    tags::MODALITY,
    tags::SERIES_DESCRIPTION,
    tags::SERIES_DATE,
    tags::SERIES_TIME,
    tags::BODY_PART_EXAMINED,
    tags::PROTOCOL_NAME,
    tags::LATERALITY,
    tags::PATIENT_POSITION,
    tags::FRAME_OF_REFERENCE_UID,
    tags::MANUFACTURER,
    tags::MANUFACTURER_MODEL_NAME,
    tags::STATION_NAME,
];

/// 实例层属性。
///
/// 除查询键外,这里刻意收齐了查看器渲染要用的全部标签 —— 列表页和窗宽窗位
/// 初值不该为了读这几个数去解一遍原始文件。
pub const INSTANCE: &[Tag] = &[
    tags::SPECIFIC_CHARACTER_SET,
    tags::SOP_CLASS_UID,
    tags::SOP_INSTANCE_UID,
    tags::INSTANCE_NUMBER,
    tags::IMAGE_TYPE,
    tags::CONTENT_DATE,
    tags::CONTENT_TIME,
    tags::ACQUISITION_NUMBER,
    // 像素几何
    tags::ROWS,
    tags::COLUMNS,
    tags::NUMBER_OF_FRAMES,
    tags::SAMPLES_PER_PIXEL,
    tags::PHOTOMETRIC_INTERPRETATION,
    tags::PLANAR_CONFIGURATION,
    tags::BITS_ALLOCATED,
    tags::BITS_STORED,
    tags::HIGH_BIT,
    tags::PIXEL_REPRESENTATION,
    // 显示管线:存储值 → Rescale → VOI → Photometric 反转
    tags::RESCALE_SLOPE,
    tags::RESCALE_INTERCEPT,
    tags::RESCALE_TYPE,
    tags::WINDOW_CENTER,
    tags::WINDOW_WIDTH,
    tags::WINDOW_CENTER_WIDTH_EXPLANATION,
    tags::VOILUT_FUNCTION,
    // 空间信息:序列排序与测距
    tags::IMAGE_POSITION_PATIENT,
    tags::IMAGE_ORIENTATION_PATIENT,
    tags::SLICE_THICKNESS,
    tags::SLICE_LOCATION,
    // 测距标定。X 光上 PixelSpacing 与 ImagerPixelSpacing 的关系决定了
    // 测量值能不能当成解剖真实距离,三个标定标签必须一起留下,
    // 否则查看器无法判断该不该标注"探测器平面,未校正"。
    tags::PIXEL_SPACING,
    tags::IMAGER_PIXEL_SPACING,
    tags::PIXEL_SPACING_CALIBRATION_TYPE,
    tags::PIXEL_SPACING_CALIBRATION_DESCRIPTION,
    tags::ESTIMATED_RADIOGRAPHIC_MAGNIFICATION_FACTOR,
    tags::DISTANCE_SOURCE_TO_DETECTOR,
    tags::DISTANCE_SOURCE_TO_PATIENT,
    // 投影 X 光的采集参数
    tags::VIEW_POSITION,
    tags::KVP,
    tags::EXPOSURE_TIME,
];
