//! 像素间距的判定:一次测距该不该给出毫米值。
//!
//! # 为什么这件事不能"尽力而为"
//!
//! 测距结果会进报告、影响临床判断。给出一个看起来权威、实际系统性偏差 20% 的
//! 毫米值,比明说"这张图测不了"危险得多 —— 后者医生会去找别的依据,
//! 前者他会直接采信。
//!
//! 所以判定分三档([`Confidence`]),对应的展示方式不同:
//!
//! | 档 | 何时 | 怎么显示 |
//! |----|------|----------|
//! | [`Confidence::Calibrated`] | 断层模态的 `PixelSpacing`、已标定的间距、超声区域 | 直接给毫米 |
//! | [`Confidence::DetectorPlane`] | 投影影像只有 `ImagerPixelSpacing` | 给毫米**并标注这是探测器平面值** |
//! | [`Confidence::None`] | 什么间距都没有 | 只给像素数,明说没有物理尺寸 |
//!
//! # 投影影像的偏差方向是「高估」
//!
//! 射线源 → 病灶 → 探测器。病灶投影到探测器上是**放大**的
//! (放大率 = SID / SOD > 1),`ImagerPixelSpacing` 描述的是探测器平面的间距,
//! 拿它换算得到的是那个放大影子的尺寸 —— **比真实解剖结构大**。
//! 胸片典型 SID 180cm、病灶中平面约 150cm,放大率约 1.2,即偏大两成。
//!
//! 方向很重要:说成"低估"会让医生以为真实病灶更大,判断正好反过来。
//!
//! 真实的放大率取决于病灶到探测器的距离,而那个距离头信息里没有 ——
//! 这不是我们偷懒,是投影成像固有的信息缺失。有些设备给
//! `EstimatedRadiographicMagnificationFactor`,有它就能校正。

use dicom::core::Tag;
use dicom::dictionary_std::tags;
use dicom::object::{DefaultDicomObject, InMemDicomObject};

/// 间距值的可信程度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    /// 病人平面的真实间距,测距可直接采信。
    Calibrated,
    /// 探测器平面的间距。有物理单位,但**系统性高估**真实尺寸,
    /// 必须在界面上标注,不能当作精确测量。
    DetectorPlane,
    /// 没有任何间距信息,只能报像素数。
    None,
}

/// 间距是从哪个属性得来的。
///
/// 保留来源而不只保留数值:界面上要向医生说明"这个毫米值凭什么可信",
/// 而且排查设备兼容问题时需要知道走的是哪条分支。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// (0028,0030) PixelSpacing,断层模态,病人平面。
    PixelSpacing,
    /// (0028,0030) 且 (0028,0A02) PixelSpacingCalibrationType 声明已标定。
    CalibratedPixelSpacing,
    /// (0018,1164) ImagerPixelSpacing,探测器平面。
    ImagerPixelSpacing,
    /// (0018,1164) 经 (0018,1114) EstimatedRadiographicMagnificationFactor 校正。
    MagnificationCorrected,
    /// 超声 (0018,6011) 区域序列里的 PhysicalDeltaX/Y。
    UltrasoundRegion,
    /// (0018,2010) NominalScannedPixelSpacing,核医学等模态用。
    NominalScannedPixelSpacing,
}

impl Source {
    /// 这个来源对应的可信度。
    ///
    /// 集中在一处,不散落到各分支 —— 散落之后就说不清哪些来源算可信了。
    pub fn confidence(self) -> Confidence {
        match self {
            // 断层影像的 PixelSpacing 就是病人平面的间距
            Self::PixelSpacing
            | Self::CalibratedPixelSpacing
            | Self::UltrasoundRegion
            | Self::NominalScannedPixelSpacing => Confidence::Calibrated,
            // 校正过的仍然只是估计值:EstimatedRadiographicMagnificationFactor
            // 里的 Estimated 是设备自己说的。但比未校正强得多,
            // 归入可信并在描述里说明它是估计。
            Self::MagnificationCorrected => Confidence::Calibrated,
            Self::ImagerPixelSpacing => Confidence::DetectorPlane,
        }
    }

    /// 给界面用的一句话说明。
    pub fn describe(self) -> &'static str {
        match self {
            Self::PixelSpacing => "来自 PixelSpacing(病人平面)",
            Self::CalibratedPixelSpacing => "来自已标定的 PixelSpacing",
            Self::ImagerPixelSpacing => "来自 ImagerPixelSpacing(探测器平面,会高估真实尺寸)",
            Self::MagnificationCorrected => "ImagerPixelSpacing 经设备给出的放大率估计校正",
            Self::UltrasoundRegion => "来自超声区域标定",
            Self::NominalScannedPixelSpacing => "来自 NominalScannedPixelSpacing",
        }
    }
}

/// 一张影像的间距判定结果。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spacing {
    /// 行间距(毫米):**相邻两行之间**的距离,对应垂直方向。
    ///
    /// DICOM 的 PixelSpacing 是 `行\列` 顺序(PS3.3 C.7.6.3.1.1),
    /// 与直觉的 x/y 相反。反了会让非正方像素的影像测出来长宽互换。
    pub row_mm: f64,
    /// 列间距(毫米):相邻两列之间的距离,对应水平方向。
    pub column_mm: f64,
    pub source: Source,
}

impl Spacing {
    pub fn confidence(self) -> Confidence {
        self.source.confidence()
    }
}

/// 没有间距时,至少要知道像素是不是正方的。
///
/// `PixelAspectRatio` 解决不了测距,但解决**显示**:非正方像素不按比例
/// 拉伸的话,圆形病灶会显示成椭圆,那是纯粹的视觉误导。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AspectRatio {
    /// 行/列 的比值。1.0 表示正方像素。
    pub row_over_column: f64,
}

impl Default for AspectRatio {
    fn default() -> Self {
        Self {
            row_over_column: 1.0,
        }
    }
}

/// 判定结果:要么有物理间距,要么只有像素。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PixelSpacing {
    /// 有物理间距。注意 [`Spacing::confidence`] 可能是 [`Confidence::DetectorPlane`],
    /// 那种情况界面必须标注。
    Physical(Spacing),
    /// 没有物理间距,只能按像素计数。
    ///
    /// `reason` 是给界面直接显示的原因说明 —— 医生需要知道"为什么这张图不能测",
    /// 而不是只看到一个灰掉的按钮。
    PixelsOnly {
        aspect_ratio: AspectRatio,
        reason: &'static str,
    },
}

impl PixelSpacing {
    pub fn confidence(self) -> Confidence {
        match self {
            Self::Physical(spacing) => spacing.confidence(),
            Self::PixelsOnly { .. } => Confidence::None,
        }
    }

    /// 能否给出可直接采信的毫米值。
    pub fn is_calibrated(self) -> bool {
        self.confidence() == Confidence::Calibrated
    }
}

/// 判定一张影像的像素间距。
///
/// 优先级从可信到不可信,取第一个成立的:
///
/// 1. 超声区域标定(超声的 PixelSpacing 常常缺失或无意义)
/// 2. `PixelSpacing` + `PixelSpacingCalibrationType` —— 设备明确说已标定
/// 3. `PixelSpacing`,且模态不是投影类 —— 断层影像的间距就是病人平面的
/// 4. `ImagerPixelSpacing` + 放大率 —— 可校正
/// 5. `PixelSpacing` 在投影影像上 —— 标准允许它表示某个中间平面,按探测器平面处理
/// 6. `ImagerPixelSpacing` 裸值 —— 探测器平面,要标注
/// 7. `NominalScannedPixelSpacing`
/// 8. 什么都没有 —— 只报像素
pub fn resolve(object: &DefaultDicomObject) -> PixelSpacing {
    let modality = text(object, tags::MODALITY).unwrap_or_default();
    let projection = is_projection_modality(&modality);

    // 1. 超声区域
    if let Some(spacing) = ultrasound_region_spacing(object) {
        return PixelSpacing::Physical(spacing);
    }

    let pixel_spacing = pair(object, tags::PIXEL_SPACING);
    let imager_spacing = pair(object, tags::IMAGER_PIXEL_SPACING);

    // 2. 明确声明已标定
    if let Some((row, column)) = pixel_spacing
        && text(object, tags::PIXEL_SPACING_CALIBRATION_TYPE).is_some()
    {
        return PixelSpacing::Physical(Spacing {
            row_mm: row,
            column_mm: column,
            source: Source::CalibratedPixelSpacing,
        });
    }

    // 3. 断层模态的 PixelSpacing
    if let Some((row, column)) = pixel_spacing
        && !projection
    {
        return PixelSpacing::Physical(Spacing {
            row_mm: row,
            column_mm: column,
            source: Source::PixelSpacing,
        });
    }

    // 4. 放大率校正。除以放大率把探测器平面的值折回病人平面 ——
    //    影子比实物大,所以校正后的间距更小。
    if let Some((row, column)) = imager_spacing
        && let Some(factor) = positive(object, tags::ESTIMATED_RADIOGRAPHIC_MAGNIFICATION_FACTOR)
    {
        return PixelSpacing::Physical(Spacing {
            row_mm: row / factor,
            column_mm: column / factor,
            source: Source::MagnificationCorrected,
        });
    }

    // 5. 投影影像上的 PixelSpacing。
    //
    //    标准(PS3.3 C.8.11.3.1.1)说在投影影像里 PixelSpacing 可以指某个
    //    未说明的中间平面,没有 CalibrationType 时无从判断是哪个平面 ——
    //    所以按探测器平面处理并标注,不当作精确值。
    if let Some((row, column)) = pixel_spacing {
        return PixelSpacing::Physical(Spacing {
            row_mm: row,
            column_mm: column,
            source: Source::ImagerPixelSpacing,
        });
    }

    // 6. 裸的 ImagerPixelSpacing
    if let Some((row, column)) = imager_spacing {
        return PixelSpacing::Physical(Spacing {
            row_mm: row,
            column_mm: column,
            source: Source::ImagerPixelSpacing,
        });
    }

    // 7. NominalScannedPixelSpacing
    if let Some((row, column)) = pair(object, tags::NOMINAL_SCANNED_PIXEL_SPACING) {
        return PixelSpacing::Physical(Spacing {
            row_mm: row,
            column_mm: column,
            source: Source::NominalScannedPixelSpacing,
        });
    }

    // 8. 无
    PixelSpacing::PixelsOnly {
        aspect_ratio: aspect_ratio(object),
        reason: if projection {
            "该投影影像没有提供像素间距,无法换算物理尺寸"
        } else {
            "该影像没有提供像素间距,无法换算物理尺寸"
        },
    }
}

/// 投影类模态:像素间距描述探测器平面,不是病人平面。
fn is_projection_modality(modality: &str) -> bool {
    matches!(
        modality.trim().to_ascii_uppercase().as_str(),
        // CR 计算机放射, DX 数字放射, MG 乳腺, XA 血管造影,
        // RF 透视, PX 全景牙片, IO 口内牙片, DR 数字放射(非标准但有设备用)
        "CR" | "DX" | "MG" | "XA" | "RF" | "PX" | "IO" | "DR"
    )
}

/// 从超声区域序列取标定。
///
/// 只在**恰好一个区域**且单位是厘米时采用。多区域影像(同屏显示 B 超加
/// 多普勒频谱)的每个区域标定不同,用错区域的标定会得出完全错误的尺寸 ——
/// 那种情况下退回像素,等阶段 6 的查看器按测量点落在哪个区域来选。
fn ultrasound_region_spacing(object: &DefaultDicomObject) -> Option<Spacing> {
    let regions = object.get(tags::SEQUENCE_OF_ULTRASOUND_REGIONS)?.items()?;
    if regions.len() != 1 {
        return None;
    }
    let region = regions.first()?;

    // PhysicalUnitsXDirection: 3 = 厘米(PS3.3 C.8.5.5.1.15)。
    // 其他取值(秒、赫兹、dB)不是长度,拿来测距是无意义的。
    const UNIT_CENTIMETERS: u16 = 3;
    let unit_x = item_int(region, tags::PHYSICAL_UNITS_X_DIRECTION)?;
    let unit_y = item_int(region, tags::PHYSICAL_UNITS_Y_DIRECTION)?;
    if unit_x != UNIT_CENTIMETERS || unit_y != UNIT_CENTIMETERS {
        return None;
    }

    let delta_x = item_float(region, tags::PHYSICAL_DELTA_X)?;
    let delta_y = item_float(region, tags::PHYSICAL_DELTA_Y)?;
    if !usable(delta_x) || !usable(delta_y) {
        return None;
    }

    // PhysicalDelta 的单位是厘米/像素,换成毫米。
    // X 是列方向、Y 是行方向 —— 和 PixelSpacing 的 `行\列` 顺序正好相反。
    Some(Spacing {
        row_mm: delta_y * 10.0,
        column_mm: delta_x * 10.0,
        source: Source::UltrasoundRegion,
    })
}

fn item_int(item: &InMemDicomObject, tag: Tag) -> Option<u16> {
    item.get(tag)?.to_int::<u16>().ok()
}

fn item_float(item: &InMemDicomObject, tag: Tag) -> Option<f64> {
    item.get(tag)?.to_float64().ok()
}

/// 读一个双值间距属性,返回 `(行, 列)`。
///
/// 只有两个值都可用才返回 —— 一个是 0 或负数的间距会让测距结果变成 0 或负,
/// 那比没有间距更糟:界面会显示一个荒谬的数字而不是"测不了"。
fn pair(object: &DefaultDicomObject, tag: Tag) -> Option<(f64, f64)> {
    let values = object.get(tag)?.to_multi_float64().ok()?;
    let (row, column) = (*values.first()?, *values.get(1)?);
    (usable(row) && usable(column)).then_some((row, column))
}

fn positive(object: &DefaultDicomObject, tag: Tag) -> Option<f64> {
    let value = object.get(tag)?.to_float64().ok()?;
    usable(value).then_some(value)
}

/// 间距必须是正的有限数。NaN、无穷、0、负数一律不可用。
fn usable(value: f64) -> bool {
    value.is_finite() && value > 0.0
}

fn aspect_ratio(object: &DefaultDicomObject) -> AspectRatio {
    let Some(values) = object
        .get(tags::PIXEL_ASPECT_RATIO)
        .and_then(|element| element.to_multi_float64().ok())
    else {
        return AspectRatio::default();
    };
    let (Some(row), Some(column)) = (values.first().copied(), values.get(1).copied()) else {
        return AspectRatio::default();
    };
    if !usable(row) || !usable(column) {
        return AspectRatio::default();
    }
    AspectRatio {
        row_over_column: row / column,
    }
}

fn text(object: &DefaultDicomObject, tag: Tag) -> Option<String> {
    crate::utf8_text(object, tag)
}

/// 一次测距的结果。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Measurement {
    /// 有物理长度。`caveat` 非空时界面必须把它显示出来。
    Millimeters {
        value: f64,
        source: Source,
        /// 探测器平面等需要提醒的情况下的说明文字。
        caveat: Option<&'static str>,
    },
    /// 只有像素距离。
    Pixels { value: f64, reason: &'static str },
}

/// 按两点的像素偏移算距离。
///
/// `d_column`/`d_row` 是列方向和行方向的像素差。分开传而不是传一个欧氏
/// 像素距离,因为非正方像素下两个方向的权重不同 —— 先算欧氏距离再乘一个
/// 间距会在非正方像素上算错。
pub fn distance(spacing: PixelSpacing, d_column: f64, d_row: f64) -> Measurement {
    match spacing {
        PixelSpacing::Physical(spacing) => {
            let x = d_column * spacing.column_mm;
            let y = d_row * spacing.row_mm;
            Measurement::Millimeters {
                value: (x * x + y * y).sqrt(),
                source: spacing.source,
                caveat: match spacing.confidence() {
                    Confidence::DetectorPlane => Some(
                        "该值取自探测器平面,受投影放大影响会大于真实尺寸;\
                         精确测量请使用已知尺寸的标记物校准",
                    ),
                    Confidence::Calibrated | Confidence::None => None,
                },
            }
        }
        // 像素距离不考虑纵横比:纵横比是显示时的拉伸系数,
        // 把它乘进像素距离会得到一个既不是像素也不是毫米的数。
        PixelSpacing::PixelsOnly { reason, .. } => Measurement::Pixels {
            value: (d_column * d_column + d_row * d_row).sqrt(),
            reason,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dicom::core::{DataElement, PrimitiveValue, VR};
    use dicom::object::{FileMetaTableBuilder, InMemDicomObject};

    fn multi(values: &[&str]) -> PrimitiveValue {
        PrimitiveValue::Strs(values.iter().map(|s| (*s).to_owned()).collect())
    }

    /// 造一个只含判定所需属性的最小对象。
    fn object(elements: Vec<dicom::object::mem::InMemElement>) -> DefaultDicomObject {
        InMemDicomObject::from_element_iter(elements)
            .with_meta(
                FileMetaTableBuilder::new()
                    .transfer_syntax(dicom::dictionary_std::uids::EXPLICIT_VR_LITTLE_ENDIAN)
                    .media_storage_sop_class_uid(dicom::dictionary_std::uids::CT_IMAGE_STORAGE)
                    .media_storage_sop_instance_uid("1.2.3")
                    .implementation_class_uid("2.25.1"),
            )
            .expect("测试对象应可构造")
    }

    fn modality(value: &str) -> dicom::object::mem::InMemElement {
        DataElement::new(tags::MODALITY, VR::CS, PrimitiveValue::from(value))
    }

    /// PixelSpacing 是「行\列」顺序,不是 x/y。反了会让非正方像素长宽互换。
    #[test]
    fn pixel_spacing_is_row_then_column() {
        let obj = object(vec![
            modality("CT"),
            DataElement::new(tags::PIXEL_SPACING, VR::DS, multi(&["0.5", "2.0"])),
        ]);
        let PixelSpacing::Physical(spacing) = resolve(&obj) else {
            panic!("CT 有 PixelSpacing 应判为有物理间距");
        };
        assert_eq!(spacing.row_mm, 0.5, "第一个值是行间距");
        assert_eq!(spacing.column_mm, 2.0, "第二个值是列间距");
    }

    #[test]
    fn cross_sectional_pixel_spacing_is_calibrated() {
        let obj = object(vec![
            modality("CT"),
            DataElement::new(tags::PIXEL_SPACING, VR::DS, multi(&["0.6836", "0.6836"])),
        ]);
        let resolved = resolve(&obj);
        assert_eq!(resolved.confidence(), Confidence::Calibrated);
        assert!(resolved.is_calibrated());

        // MR、PET 同样
        for m in ["MR", "PT", "NM"] {
            let obj = object(vec![
                modality(m),
                DataElement::new(tags::PIXEL_SPACING, VR::DS, multi(&["1.0", "1.0"])),
            ]);
            assert!(resolve(&obj).is_calibrated(), "{m} 应可信");
        }
    }

    /// 投影影像只有 ImagerPixelSpacing 时必须标注,不能当精确值。
    #[test]
    fn projection_with_only_imager_spacing_is_flagged() {
        let obj = object(vec![
            modality("DX"),
            DataElement::new(tags::IMAGER_PIXEL_SPACING, VR::DS, multi(&["0.14", "0.14"])),
        ]);
        let resolved = resolve(&obj);
        assert_eq!(resolved.confidence(), Confidence::DetectorPlane);
        assert!(!resolved.is_calibrated(), "探测器平面值不算标定");

        // 测距要带说明
        let Measurement::Millimeters { caveat, .. } = distance(resolved, 100.0, 0.0) else {
            panic!("应给出毫米值");
        };
        let caveat = caveat.expect("探测器平面必须带说明");
        assert!(
            caveat.contains("大于真实尺寸"),
            "说明里要讲清偏差方向是偏大:{caveat}"
        );
    }

    /// 放大率校正:影子比实物大,所以校正后的间距更小。
    #[test]
    fn magnification_factor_shrinks_the_spacing() {
        let obj = object(vec![
            modality("DX"),
            DataElement::new(tags::IMAGER_PIXEL_SPACING, VR::DS, multi(&["0.12", "0.12"])),
            DataElement::new(
                tags::ESTIMATED_RADIOGRAPHIC_MAGNIFICATION_FACTOR,
                VR::DS,
                PrimitiveValue::from("1.2"),
            ),
        ]);
        let PixelSpacing::Physical(spacing) = resolve(&obj) else {
            panic!("应有物理间距");
        };
        assert_eq!(spacing.source, Source::MagnificationCorrected);
        // 0.12 / 1.2 = 0.1:校正后更小,因为探测器上的影子被放大过
        assert!(
            (spacing.column_mm - 0.1).abs() < 1e-9,
            "实际 {}",
            spacing.column_mm
        );
        assert!(spacing.column_mm < 0.12, "校正后的间距必须小于探测器平面值");
        assert_eq!(resolve(&obj).confidence(), Confidence::Calibrated);
    }

    /// 明确声明已标定的 PixelSpacing 优先于模态判断。
    #[test]
    fn explicit_calibration_wins_over_modality_heuristics() {
        let obj = object(vec![
            modality("DX"), // 投影模态
            DataElement::new(tags::PIXEL_SPACING, VR::DS, multi(&["0.1", "0.1"])),
            DataElement::new(
                tags::PIXEL_SPACING_CALIBRATION_TYPE,
                VR::CS,
                PrimitiveValue::from("FIDUCIAL"),
            ),
        ]);
        let resolved = resolve(&obj);
        assert!(resolved.is_calibrated(), "设备说已用标记物标定,就该采信");
        let PixelSpacing::Physical(spacing) = resolved else {
            unreachable!()
        };
        assert_eq!(spacing.source, Source::CalibratedPixelSpacing);
    }

    /// 投影影像上没有 CalibrationType 的 PixelSpacing 不能当精确值 ——
    /// 标准允许它指某个未说明的中间平面。
    #[test]
    fn projection_pixel_spacing_without_calibration_type_is_flagged() {
        let obj = object(vec![
            modality("MG"),
            DataElement::new(tags::PIXEL_SPACING, VR::DS, multi(&["0.07", "0.07"])),
        ]);
        assert_eq!(resolve(&obj).confidence(), Confidence::DetectorPlane);
    }

    /// 0 或负的间距比没有间距更糟 —— 会显示出荒谬的数字。
    #[test]
    fn degenerate_spacing_values_are_rejected() {
        for bad in [
            multi(&["0", "0.5"]),
            multi(&["0.5", "0"]),
            multi(&["-1", "0.5"]),
            multi(&["nan", "0.5"]),
            multi(&["0.5"]), // 只有一个值
        ] {
            let obj = object(vec![
                modality("CT"),
                DataElement::new(tags::PIXEL_SPACING, VR::DS, bad.clone()),
            ]);
            assert!(
                matches!(resolve(&obj), PixelSpacing::PixelsOnly { .. }),
                "应拒绝 {bad:?}"
            );
        }
    }

    /// 放大率为 0 或负数时不能拿去做除法。
    #[test]
    fn degenerate_magnification_factor_falls_back_to_detector_plane() {
        for bad in ["0", "-1.2"] {
            let obj = object(vec![
                modality("DX"),
                DataElement::new(tags::IMAGER_PIXEL_SPACING, VR::DS, multi(&["0.12", "0.12"])),
                DataElement::new(
                    tags::ESTIMATED_RADIOGRAPHIC_MAGNIFICATION_FACTOR,
                    VR::DS,
                    PrimitiveValue::from(bad),
                ),
            ]);
            let PixelSpacing::Physical(spacing) = resolve(&obj) else {
                panic!("仍应有探测器平面的间距");
            };
            assert_eq!(
                spacing.source,
                Source::ImagerPixelSpacing,
                "放大率 {bad} 不可用"
            );
            assert_eq!(spacing.column_mm, 0.12, "不该被荒谬的放大率改动");
        }
    }

    /// 什么间距都没有:报像素,并给出可直接显示的原因。
    #[test]
    fn no_spacing_yields_pixels_with_a_reason() {
        let obj = object(vec![modality("OT")]);
        let PixelSpacing::PixelsOnly { reason, .. } = resolve(&obj) else {
            panic!("没有任何间距属性时应只报像素");
        };
        assert!(
            reason.contains("无法换算物理尺寸"),
            "原因要能直接显示:{reason}"
        );

        let Measurement::Pixels { value, .. } = distance(resolve(&obj), 3.0, 4.0) else {
            panic!("应回像素距离");
        };
        assert_eq!(value, 5.0, "3-4-5 直角三角形");
    }

    /// 没有间距时至少保留纵横比,让显示不至于把圆画成椭圆。
    #[test]
    fn aspect_ratio_is_preserved_when_spacing_is_absent() {
        let obj = object(vec![
            modality("OT"),
            DataElement::new(tags::PIXEL_ASPECT_RATIO, VR::IS, multi(&["2", "1"])),
        ]);
        let PixelSpacing::PixelsOnly { aspect_ratio, .. } = resolve(&obj) else {
            panic!("应只报像素");
        };
        assert_eq!(aspect_ratio.row_over_column, 2.0);

        // 缺失时默认正方
        let plain = object(vec![modality("OT")]);
        let PixelSpacing::PixelsOnly { aspect_ratio, .. } = resolve(&plain) else {
            unreachable!()
        };
        assert_eq!(aspect_ratio.row_over_column, 1.0);
    }

    /// 非正方像素:两个方向要各按自己的间距算,不能先求欧氏像素距离再乘一个值。
    #[test]
    fn distance_weights_each_axis_by_its_own_spacing() {
        let spacing = PixelSpacing::Physical(Spacing {
            row_mm: 2.0,
            column_mm: 0.5,
            source: Source::PixelSpacing,
        });

        // 纯水平 100 列 → 100 × 0.5 = 50mm
        let Measurement::Millimeters { value, .. } = distance(spacing, 100.0, 0.0) else {
            panic!()
        };
        assert!((value - 50.0).abs() < 1e-9, "实际 {value}");

        // 纯垂直 100 行 → 100 × 2.0 = 200mm
        let Measurement::Millimeters { value, .. } = distance(spacing, 0.0, 100.0) else {
            panic!()
        };
        assert!((value - 200.0).abs() < 1e-9, "实际 {value}");

        // 斜向:各轴分别加权后求欧氏。若先算像素欧氏(141.42)再乘任一间距,
        // 会得到 70.7 或 282.8,都不对。
        let Measurement::Millimeters { value, .. } = distance(spacing, 100.0, 100.0) else {
            panic!()
        };
        let expected = (50.0_f64 * 50.0 + 200.0 * 200.0).sqrt();
        assert!(
            (value - expected).abs() < 1e-9,
            "应为 {expected},实际 {value}"
        );
    }

    /// 造一个超声区域项。`unit` 为 3 表示厘米。
    fn us_region(delta_x: &str, delta_y: &str, unit: u16) -> InMemDicomObject {
        InMemDicomObject::from_element_iter([
            DataElement::new(
                tags::PHYSICAL_UNITS_X_DIRECTION,
                VR::US,
                PrimitiveValue::from(unit),
            ),
            DataElement::new(
                tags::PHYSICAL_UNITS_Y_DIRECTION,
                VR::US,
                PrimitiveValue::from(unit),
            ),
            DataElement::new(
                tags::PHYSICAL_DELTA_X,
                VR::FD,
                PrimitiveValue::from(delta_x.parse::<f64>().unwrap()),
            ),
            DataElement::new(
                tags::PHYSICAL_DELTA_Y,
                VR::FD,
                PrimitiveValue::from(delta_y.parse::<f64>().unwrap()),
            ),
        ])
    }

    fn with_regions(regions: Vec<InMemDicomObject>) -> DefaultDicomObject {
        object(vec![
            modality("US"),
            DataElement::new(
                tags::SEQUENCE_OF_ULTRASOUND_REGIONS,
                VR::SQ,
                dicom::core::DicomValue::Sequence(dicom::core::value::DataSetSequence::from(
                    regions,
                )),
            ),
        ])
    }

    /// 超声区域的 PhysicalDelta 单位是厘米,要换成毫米;
    /// 而且 X 是列方向、Y 是行方向 —— 与 PixelSpacing 的「行\列」顺序相反。
    #[test]
    fn ultrasound_region_delta_is_centimeters_and_x_is_the_column_axis() {
        let obj = with_regions(vec![us_region("0.02", "0.03", 3)]);
        let PixelSpacing::Physical(spacing) = resolve(&obj) else {
            panic!("单区域超声应能标定");
        };
        assert_eq!(spacing.source, Source::UltrasoundRegion);
        // 0.02 cm/px = 0.2 mm/px,X 对应列
        assert!(
            (spacing.column_mm - 0.2).abs() < 1e-9,
            "实际 {}",
            spacing.column_mm
        );
        assert!(
            (spacing.row_mm - 0.3).abs() < 1e-9,
            "实际 {}",
            spacing.row_mm
        );
        assert!(resolve(&obj).is_calibrated());
    }

    /// 多区域影像(同屏 B 超 + 多普勒频谱)每个区域标定不同,
    /// 用错区域会得出完全错误的尺寸 —— 退回像素。
    #[test]
    fn multiple_ultrasound_regions_fall_back_to_pixels() {
        let obj = with_regions(vec![
            us_region("0.02", "0.02", 3),
            us_region("0.05", "0.05", 3),
        ]);
        assert!(
            matches!(resolve(&obj), PixelSpacing::PixelsOnly { .. }),
            "多区域时不能猜用哪个区域的标定"
        );
    }

    /// 非长度单位的区域(多普勒频谱的秒 / 赫兹)不能拿来测距。
    #[test]
    fn ultrasound_regions_with_non_length_units_are_ignored() {
        // 单位 4 = 秒,不是长度
        let obj = with_regions(vec![us_region("0.02", "0.02", 4)]);
        assert!(
            matches!(resolve(&obj), PixelSpacing::PixelsOnly { .. }),
            "秒/赫兹这类单位不是长度,不能用于测距"
        );
    }

    /// 可信的间距不带 caveat —— 界面上不该给正常的 CT 测距挂警告。
    #[test]
    fn calibrated_measurements_carry_no_caveat() {
        let spacing = PixelSpacing::Physical(Spacing {
            row_mm: 0.5,
            column_mm: 0.5,
            source: Source::PixelSpacing,
        });
        let Measurement::Millimeters { caveat, .. } = distance(spacing, 10.0, 0.0) else {
            panic!()
        };
        assert_eq!(caveat, None);
    }
}
