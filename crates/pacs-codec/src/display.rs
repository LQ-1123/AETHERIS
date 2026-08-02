//! 显示管线:存储的像素值 → 屏幕上的灰度。
//!
//! # 顺序是规定的,不是偏好
//!
//! ```text
//! 存储值 → Modality LUT(Rescale) → VOI LUT(窗宽窗位) → Photometric 反转 → 输出
//! ```
//!
//! 这个顺序来自 PS3.3 C.11(各 LUT 的定义)与 PS3.4 N.2(灰度管线的串联次序),
//! 不能调换:
//!
//! - **Rescale 必须在窗宽窗位之前**。窗宽窗位的值是按物理单位给的
//!   (CT 的窗位 -600 指 -600 HU),而存储值要经 Rescale 才成为 HU。
//!   顺序反了,窗宽窗位就作用在原始存储值上 —— 一张肺窗会看起来全白或全黑。
//! - **Photometric 反转必须在最后**。它作用的是最终的显示灰度,
//!   放在 VOI 之前会让窗宽窗位的上下界反过来。
//!
//! # 三个具体的坑
//!
//! 1. **`MONOCHROME1` 是反的**(0 = 白)。X 光里很常见,漏判得到负片 ——
//!    而负片上的骨骼和空气恰好互换,是会导致误读的错误,不是"看起来怪"。
//! 2. **`WindowCenter`/`WindowWidth` 可以是多值**。取第一组,并把全部组
//!    暴露出去让界面切换(乳腺和胸片常带多个预设)。
//! 3. **`SIGMOID` 不是线性窗**。按线性算,在窗的两端偏差最大 ——
//!    而那正是判断边界的地方。

use dicom::core::Tag;
use dicom::dictionary_std::tags;
use dicom::object::DefaultDicomObject;

/// Modality LUT:把存储值变成有物理意义的值(CT 是 HU)。
///
/// 只实现 Rescale 形式。显式的 `ModalityLUTSequence`(查表形式)在 CT/MR/X 光
/// 上极少出现,主要用于 PET 的某些老设备;真遇到时按恒等处理并告警,
/// 至少能看到原始数据,而不是拿错误的 Rescale 去算。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModalityLut {
    pub slope: f64,
    pub intercept: f64,
    /// 物理单位,如 `HU`。仅用于界面显示。
    pub unit: Option<&'static str>,
}

impl Default for ModalityLut {
    /// 标准规定缺省是 slope=1、intercept=0(PS3.3 C.11.1.1.2),即恒等。
    fn default() -> Self {
        Self {
            slope: 1.0,
            intercept: 0.0,
            unit: None,
        }
    }
}

impl ModalityLut {
    pub fn apply(self, stored: f64) -> f64 {
        stored * self.slope + self.intercept
    }

    /// 是否是恒等变换 —— 界面可据此决定要不要显示"HU"之类的单位。
    pub fn is_identity(self) -> bool {
        self.slope == 1.0 && self.intercept == 0.0
    }
}

/// VOI LUT 的函数形式(0028,1056)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiFunction {
    /// 标准线性窗。注意公式里用的是 `w - 1` 和 `c - 0.5`(PS3.3 C.11.2.1.2)。
    Linear,
    /// `LINEAR_EXACT`:不做那两个偏移,直接用 c 和 w。
    ///
    /// 存在的原因是 `LINEAR` 的 `-1`/`-0.5` 偏移在窄窗上误差显著:
    /// w=2 时 `w-1=1`,斜率差一倍。
    LinearExact,
    /// S 形曲线,两端平滑过渡。
    Sigmoid,
}

/// 一组窗宽窗位。
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub center: f64,
    pub width: f64,
    /// (0028,1055) WindowCenterWidthExplanation,如 "LUNG"、"MEDIASTINUM"。
    pub explanation: Option<String>,
    pub function: VoiFunction,
}

impl Window {
    /// 把物理值映射到 `0.0..=1.0`。
    ///
    /// 返回归一化值而不是 0-255:归一化保留了精度,让后续要输出 16 位灰度
    /// 或做进一步处理时不必重算。转 8 位在最后一步做。
    pub fn apply(&self, value: f64) -> f64 {
        match self.function {
            VoiFunction::Linear => self.linear(value),
            VoiFunction::LinearExact => self.linear_exact(value),
            VoiFunction::Sigmoid => self.sigmoid(value),
        }
    }

    /// PS3.3 C.11.2.1.2 的 LINEAR 公式。
    ///
    /// 偏移量 `c - 0.5` 和 `w - 1` 是标准明文规定的,不是笔误 ——
    /// 它们让窗恰好覆盖 w 个离散灰阶。
    fn linear(&self, value: f64) -> f64 {
        // w < 1 时 w-1 ≤ 0,除法会得到无穷或反向映射。标准要求 w ≥ 1,
        // 但设备真的会送 0(通常是初始化没填)。退化成阈值函数:
        // 这是 w→1 的极限,比返回 NaN 让整张图变黑要合理。
        let effective_width = self.width - 1.0;
        if effective_width <= 0.0 {
            return if value <= self.center - 0.5 { 0.0 } else { 1.0 };
        }
        let lower = self.center - 0.5 - effective_width / 2.0;
        ((value - lower) / effective_width).clamp(0.0, 1.0)
    }

    /// LINEAR_EXACT:不做 LINEAR 的两个偏移。
    fn linear_exact(&self, value: f64) -> f64 {
        if self.width <= 0.0 {
            return if value <= self.center { 0.0 } else { 1.0 };
        }
        let lower = self.center - self.width / 2.0;
        ((value - lower) / self.width).clamp(0.0, 1.0)
    }

    /// PS3.3 C.11.2.1.3 的 SIGMOID 公式:`1 / (1 + exp(-4 (x - c) / w))`。
    ///
    /// 系数 4 让曲线在 `c ± w/2` 处大致对应线性窗的两端。
    /// 值域是开区间 (0, 1),永远到不了纯黑纯白 —— 这是 S 形的本意
    /// (保留极端值的层次),不是缺陷,所以不做 clamp 拉伸。
    fn sigmoid(&self, value: f64) -> f64 {
        if self.width == 0.0 {
            return if value <= self.center { 0.0 } else { 1.0 };
        }
        1.0 / (1.0 + (-4.0 * (value - self.center) / self.width).exp())
    }
}

/// 光度解释(0028,0004)里与灰度反转有关的部分。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Photometric {
    /// 0 = 白。灰度**反转**。
    Monochrome1,
    /// 0 = 黑。常规。
    Monochrome2,
    /// 彩色或其他不走灰度管线的形式(RGB、PALETTE COLOR、YBR_*)。
    NotMonochrome,
}

impl Photometric {
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_uppercase().as_str() {
            "MONOCHROME1" => Self::Monochrome1,
            "MONOCHROME2" => Self::Monochrome2,
            _ => Self::NotMonochrome,
        }
    }

    /// 是否要反转最终灰度。
    pub fn inverts(self) -> bool {
        self == Self::Monochrome1
    }
}

/// 一条完整的显示管线。
#[derive(Debug, Clone, PartialEq)]
pub struct Pipeline {
    pub modality_lut: ModalityLut,
    /// 全部窗宽窗位预设,至少一个。第一个是默认。
    ///
    /// 保留全部而不只留第一个:乳腺和胸片常带多个预设(如 "LUNG"、
    /// "MEDIASTINUM"),界面要能让医生切换。
    pub windows: Vec<Window>,
    pub photometric: Photometric,
    /// (0028,0100) BitsAllocated,决定存储值的位宽。
    pub bits_stored: u16,
    /// (0028,0103) PixelRepresentation:1 表示有符号补码。
    pub signed: bool,
}

impl Pipeline {
    /// 从 DICOM 属性解析出管线。
    ///
    /// 缺 `WindowCenter`/`WindowWidth` 时按 `BitsStored` 的全量程构造一个窗。
    /// 那不是好的默认(真实影像的有效范围通常远小于全量程),但**是安全的**:
    /// 至少整张图都可见。查看器可以在此基础上按实际像素范围自动窗宽 ——
    /// 那需要扫一遍像素,不属于本函数的职责。
    pub fn from_object(object: &DefaultDicomObject) -> Self {
        let modality_lut = modality_lut(object);
        let photometric =
            Photometric::parse(&text(object, tags::PHOTOMETRIC_INTERPRETATION).unwrap_or_default());
        let bits_stored = int(object, tags::BITS_STORED).unwrap_or(16) as u16;
        let signed = int(object, tags::PIXEL_REPRESENTATION).unwrap_or(0) == 1;

        let function = voi_function(object);
        let mut windows = windows(object, function);
        if windows.is_empty() {
            windows.push(full_range_window(
                modality_lut,
                bits_stored,
                signed,
                function,
            ));
        }

        Self {
            modality_lut,
            windows,
            photometric,
            bits_stored,
            signed,
        }
    }

    /// 默认窗(第一组)。
    pub fn default_window(&self) -> &Window {
        self.windows.first().expect("构造时保证至少有一个窗")
    }

    /// 把一个存储值走完整条管线,输出 8 位灰度。
    ///
    /// 用指定的窗;传 `None` 用默认窗。
    pub fn apply(&self, stored: f64, window: Option<&Window>) -> u8 {
        let window = window.unwrap_or_else(|| self.default_window());

        // 1. Modality LUT:存储值 → 物理值(HU)
        let physical = self.modality_lut.apply(stored);
        // 2. VOI:物理值 → 0.0..=1.0
        let normalized = window.apply(physical);
        // 3. Photometric:MONOCHROME1 反转
        let displayed = if self.photometric.inverts() {
            1.0 - normalized
        } else {
            normalized
        };
        // 4. 量化到 8 位。乘 255 再四舍五入 —— 乘 256 会让 1.0 溢出成 256。
        (displayed * 255.0).round().clamp(0.0, 255.0) as u8
    }
}

fn modality_lut(object: &DefaultDicomObject) -> ModalityLut {
    // 显式的 ModalityLUTSequence 优先级高于 Rescale(PS3.3 C.11.1.1.2),
    // 但我们没实现查表形式。有它就告警并按恒等处理 ——
    // 拿 Rescale 去算会得出错误的物理值,而恒等至少是"未变换的原始值"。
    if object.get(tags::MODALITY_LUT_SEQUENCE).is_some() {
        tracing::warn!("影像带显式 ModalityLUTSequence,本实现尚不支持查表形式,按恒等处理");
        return ModalityLut::default();
    }

    let slope = float(object, tags::RESCALE_SLOPE);
    let intercept = float(object, tags::RESCALE_INTERCEPT);
    // 两个都缺 → 恒等。只有一个 → 另一个取缺省值(标准的缺省是 1 和 0)。
    if slope.is_none() && intercept.is_none() {
        return ModalityLut::default();
    }

    // slope 为 0 会把整张图压成常数。那一定是设备填错了 ——
    // 按恒等处理并告警,比显示一张纯色图强。
    let slope = match slope {
        Some(value) if value != 0.0 && value.is_finite() => value,
        Some(bad) => {
            tracing::warn!(slope = bad, "RescaleSlope 不可用,按 1.0 处理");
            1.0
        }
        None => 1.0,
    };

    ModalityLut {
        slope,
        intercept: intercept.filter(|v| v.is_finite()).unwrap_or(0.0),
        unit: rescale_unit(object),
    }
}

/// (0028,1054) RescaleType,或按模态推断。
fn rescale_unit(object: &DefaultDicomObject) -> Option<&'static str> {
    if let Some(raw) = text(object, tags::RESCALE_TYPE) {
        let upper = raw.trim().to_ascii_uppercase();
        // 只认标准定义的值,不把设备自定义的字符串当单位传出去
        return match upper.as_str() {
            "HU" => Some("HU"),
            "US" => Some("US"), // Unspecified
            "OD" => Some("OD"), // 光密度
            _ => None,
        };
    }
    // CT 没有 RescaleType 时,按标准 HU 是默认(PS3.3 C.11.1.1.2)
    matches!(
        text(object, tags::MODALITY)
            .unwrap_or_default()
            .trim()
            .to_ascii_uppercase()
            .as_str(),
        "CT"
    )
    .then_some("HU")
}

fn voi_function(object: &DefaultDicomObject) -> VoiFunction {
    match text(object, tags::VOILUT_FUNCTION)
        .unwrap_or_default()
        .trim()
        .to_ascii_uppercase()
        .as_str()
    {
        "SIGMOID" => VoiFunction::Sigmoid,
        "LINEAR_EXACT" => VoiFunction::LinearExact,
        // 缺失或 LINEAR 都是线性(标准缺省是 LINEAR)
        _ => VoiFunction::Linear,
    }
}

/// 读出全部窗宽窗位组。
///
/// `WindowCenter` 和 `WindowWidth` 的多值必须**成对**取。两者长度不一致时
/// 只取能配上的那些 —— 错位配对会得出毫无意义的窗。
fn windows(object: &DefaultDicomObject, function: VoiFunction) -> Vec<Window> {
    let centers = floats(object, tags::WINDOW_CENTER);
    let widths = floats(object, tags::WINDOW_WIDTH);
    let explanations = strings(object, tags::WINDOW_CENTER_WIDTH_EXPLANATION);

    if centers.len() != widths.len() && !centers.is_empty() && !widths.is_empty() {
        tracing::warn!(
            centers = centers.len(),
            widths = widths.len(),
            "WindowCenter 与 WindowWidth 的值数不一致,只取能配对的部分"
        );
    }

    centers
        .iter()
        .zip(widths.iter())
        .enumerate()
        // 宽度为 0 或负、或非有限值的窗没有意义,跳过
        .filter(|(_, (center, width))| width.is_finite() && **width > 0.0 && center.is_finite())
        .map(|(index, (center, width))| Window {
            center: *center,
            width: *width,
            explanation: explanations.get(index).cloned(),
            function,
        })
        .collect()
}

/// 缺窗宽窗位时的兜底:覆盖 `BitsStored` 全量程。
fn full_range_window(
    modality_lut: ModalityLut,
    bits_stored: u16,
    signed: bool,
    function: VoiFunction,
) -> Window {
    let bits = bits_stored.clamp(1, 32);
    let (min_stored, max_stored) = if signed {
        let half = 2_f64.powi(i32::from(bits) - 1);
        (-half, half - 1.0)
    } else {
        (0.0, 2_f64.powi(i32::from(bits)) - 1.0)
    };

    // 经 Modality LUT 换算到物理值。slope 为负时上下界会互换,取 min/max 兜住。
    let a = modality_lut.apply(min_stored);
    let b = modality_lut.apply(max_stored);
    let (low, high) = (a.min(b), a.max(b));

    tracing::debug!(
        low,
        high,
        "影像没有 WindowCenter/WindowWidth,按存储位宽的全量程构造默认窗"
    );
    Window {
        center: (low + high) / 2.0,
        // 宽度至少 1,否则 LINEAR 公式的 w-1 会退化
        width: (high - low).max(1.0),
        explanation: Some("全量程(影像未提供窗宽窗位)".to_owned()),
        function,
    }
}

fn text(object: &DefaultDicomObject, tag: Tag) -> Option<String> {
    let raw = object.get(tag)?.to_str().ok()?;
    let trimmed = raw.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn float(object: &DefaultDicomObject, tag: Tag) -> Option<f64> {
    object.get(tag)?.to_float64().ok()
}

fn int(object: &DefaultDicomObject, tag: Tag) -> Option<i32> {
    object.get(tag)?.to_int::<i32>().ok()
}

fn floats(object: &DefaultDicomObject, tag: Tag) -> Vec<f64> {
    object
        .get(tag)
        .and_then(|element| element.to_multi_float64().ok())
        .unwrap_or_default()
}

fn strings(object: &DefaultDicomObject, tag: Tag) -> Vec<String> {
    object
        .get(tag)
        .and_then(|element| element.to_multi_str().ok())
        .map(|values| {
            values
                .iter()
                .map(|value| value.trim_end_matches([' ', '\0']).to_owned())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(center: f64, width: f64, function: VoiFunction) -> Window {
        Window {
            center,
            width,
            explanation: None,
            function,
        }
    }

    /// LINEAR 公式的偏移量是标准规定的,不能省。
    ///
    /// 窗心处应当恰好是中灰。用 `c-0.5`/`w-1` 与不用,在 w=2 这类窄窗上
    /// 差一倍斜率。
    #[test]
    fn linear_window_follows_the_standard_formula() {
        let w = window(0.0, 100.0, VoiFunction::Linear);
        // 下界 = c - 0.5 - (w-1)/2 = -0.5 - 49.5 = -50
        assert!(
            (w.apply(-50.0) - 0.0).abs() < 1e-9,
            "下界应为 0,实际 {}",
            w.apply(-50.0)
        );
        // 上界 = 下界 + (w-1) = 49
        assert!(
            (w.apply(49.0) - 1.0).abs() < 1e-9,
            "上界应为 1,实际 {}",
            w.apply(49.0)
        );
        // 窗心附近约中灰
        assert!(
            (w.apply(-0.5) - 0.5).abs() < 0.01,
            "窗心应约 0.5,实际 {}",
            w.apply(-0.5)
        );
    }

    /// LINEAR_EXACT 不做那两个偏移。
    #[test]
    fn linear_exact_omits_the_offsets() {
        let exact = window(0.0, 100.0, VoiFunction::LinearExact);
        // 下界 = c - w/2 = -50,上界 = +50
        assert!((exact.apply(-50.0) - 0.0).abs() < 1e-9);
        assert!((exact.apply(50.0) - 1.0).abs() < 1e-9);
        assert!((exact.apply(0.0) - 0.5).abs() < 1e-9, "窗心正好是 0.5");

        // 与 LINEAR 在窄窗上差别明显
        let linear = window(0.0, 2.0, VoiFunction::Linear);
        let exact_narrow = window(0.0, 2.0, VoiFunction::LinearExact);
        assert_ne!(
            linear.apply(0.4).to_bits(),
            exact_narrow.apply(0.4).to_bits(),
            "窄窗上两种公式必须给出不同结果,否则说明偏移没生效"
        );
    }

    /// SIGMOID 不是线性,而且值域是开区间。
    #[test]
    fn sigmoid_is_not_linear_and_never_saturates() {
        let s = window(0.0, 100.0, VoiFunction::Sigmoid);
        assert!((s.apply(0.0) - 0.5).abs() < 1e-9, "窗心处应恰为 0.5");

        // 单调递增
        assert!(s.apply(-10.0) < s.apply(0.0));
        assert!(s.apply(0.0) < s.apply(10.0));

        // 在窗附近不饱和 —— 这是 S 形的本意:保留极端值的层次,
        // 而线性窗在两端会被 clamp 成死黑死白。
        //
        // 只在 f64 有精度的范围内断言。数学上值域是开区间 (0,1),但
        // exp(-40) ≈ 4e-18,`1/(1+4e-18)` 舍入后正好是 1.0 —— 离窗心几个
        // 窗宽之外就无法用 f64 区分了。那不是实现缺陷,而是浮点的表示极限。
        let two_widths = 200.0;
        assert!(
            s.apply(-two_widths) > 0.0 && s.apply(two_widths) < 1.0,
            "两个窗宽之内不该饱和,实际 {} 和 {}",
            s.apply(-two_widths),
            s.apply(two_widths)
        );
        // 对照:线性窗在同一位置已经彻底饱和
        let linear_far = window(0.0, 100.0, VoiFunction::Linear);
        assert_eq!(linear_far.apply(-two_widths), 0.0);
        assert_eq!(linear_far.apply(two_widths), 1.0);

        // 与线性的差异集中在**窗的两端**,而不是中段:
        //
        //     x     linear   sigmoid    差
        //   -50     0.000     0.119   0.119   ← 最大
        //   -25     0.253     0.269   0.016
        //     0     0.505     0.500   0.005   ← 中段几乎一致
        //   +25     0.758     0.731   0.027
        //   +50     1.000     0.881   0.119   ← 最大
        //
        // 所以按线性算 SIGMOID 影像,偏差最大的地方正是窗边界 ——
        // 而那恰好是判断病灶边界的位置。中段一致反而让这个错误更难察觉。
        let linear = window(0.0, 100.0, VoiFunction::Linear);
        for edge in [-50.0, 50.0] {
            let delta = (s.apply(edge) - linear.apply(edge)).abs();
            assert!(
                delta > 0.1,
                "SIGMOID 与 LINEAR 在窗端 {edge} 应有显著差异,实际差 {delta:.4}"
            );
        }
        // 中段差异很小,不该拿来当区分依据
        assert!(
            (s.apply(0.0) - linear.apply(0.0)).abs() < 0.01,
            "窗心附近两者本就接近"
        );
    }

    /// 宽度退化时不能返回 NaN —— 那会让整张图变黑。
    #[test]
    fn degenerate_widths_degrade_to_a_threshold() {
        for function in [
            VoiFunction::Linear,
            VoiFunction::LinearExact,
            VoiFunction::Sigmoid,
        ] {
            let w = window(10.0, 0.0, function);
            let low = w.apply(-100.0);
            let high = w.apply(100.0);
            assert!(
                low.is_finite() && high.is_finite(),
                "{function:?} 不该产生 NaN"
            );
            assert!(low < high, "{function:?} 仍应单调");
        }
        // LINEAR 的 w=1 使 w-1=0,同样是退化点
        let w = window(10.0, 1.0, VoiFunction::Linear);
        assert!(w.apply(0.0).is_finite());
        assert!(w.apply(100.0).is_finite());
    }

    #[test]
    fn photometric_parsing_is_case_insensitive_and_padding_tolerant() {
        assert_eq!(Photometric::parse("MONOCHROME1"), Photometric::Monochrome1);
        assert_eq!(Photometric::parse("monochrome1 "), Photometric::Monochrome1);
        assert_eq!(Photometric::parse("MONOCHROME2"), Photometric::Monochrome2);
        assert_eq!(Photometric::parse("RGB"), Photometric::NotMonochrome);
        assert_eq!(Photometric::parse(""), Photometric::NotMonochrome);

        assert!(Photometric::Monochrome1.inverts());
        assert!(!Photometric::Monochrome2.inverts());
        assert!(!Photometric::NotMonochrome.inverts());
    }

    /// MONOCHROME1 得到的是负片的反面 —— 同一个输入,两种光度解释的输出应互补。
    #[test]
    fn monochrome1_inverts_the_output() {
        let make = |photometric| Pipeline {
            modality_lut: ModalityLut::default(),
            windows: vec![window(0.0, 100.0, VoiFunction::LinearExact)],
            photometric,
            bits_stored: 16,
            signed: false,
        };
        let normal = make(Photometric::Monochrome2);
        let inverted = make(Photometric::Monochrome1);

        for value in [-50.0, -25.0, 0.0, 25.0, 50.0] {
            let a = normal.apply(value, None);
            let b = inverted.apply(value, None);
            // 允许差 1:归一化值恰好落在 0.5 时,两边都 round(127.5) = 128,
            // 和是 256。这是四舍五入的真实行为,不是实现问题 ——
            // 一个灰阶的差异在临床上不可感知,为了让和恰好等于 255 去改动
            // 量化方式反而会引入偏移。
            let sum = u16::from(a) + u16::from(b);
            assert!(
                (255..=256).contains(&sum),
                "MONOCHROME1 与 MONOCHROME2 在 {value} 处应互补(得到 {a} 和 {b},和 {sum})"
            );
        }

        // 端点必须严格互补:纯黑对纯白,这里不允许有偏差
        assert_eq!(normal.apply(-1000.0, None), 0);
        assert_eq!(inverted.apply(-1000.0, None), 255);
        assert_eq!(normal.apply(1000.0, None), 255);
        assert_eq!(inverted.apply(1000.0, None), 0);
    }

    // —— 以下测 from_object 的解析路径 ——

    #[cfg(feature = "fixtures")]
    mod parsing {
        use super::super::*;
        use dicom::core::{DataElement, PrimitiveValue, VR};
        use dicom::object::{FileMetaTableBuilder, InMemDicomObject};

        fn multi(values: &[&str]) -> PrimitiveValue {
            PrimitiveValue::Strs(values.iter().map(|s| (*s).to_owned()).collect())
        }

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

        fn ds(tag: dicom::core::Tag, value: &str) -> dicom::object::mem::InMemElement {
            DataElement::new(tag, VR::DS, PrimitiveValue::from(value))
        }

        /// 夹具是标准 CT:Rescale -1024/1, 窗 -600/1500, MONOCHROME2。
        #[test]
        fn parses_the_ct_fixture() {
            use pacs_core::fixture::{ct_instance, unique_uid};
            let obj = ct_instance(&unique_uid(), &unique_uid(), &unique_uid());
            let pipeline = Pipeline::from_object(&obj);

            assert_eq!(pipeline.modality_lut.slope, 1.0);
            assert_eq!(pipeline.modality_lut.intercept, -1024.0);
            assert_eq!(pipeline.modality_lut.unit, Some("HU"));
            assert_eq!(pipeline.photometric, Photometric::Monochrome2);
            assert_eq!(pipeline.windows.len(), 1);
            assert_eq!(pipeline.default_window().center, -600.0);
            assert_eq!(pipeline.default_window().width, 1500.0);
            assert_eq!(pipeline.default_window().function, VoiFunction::Linear);
            assert!(!pipeline.signed, "夹具的 PixelRepresentation 是 0");
        }

        /// CT 的存储值必须先经 Rescale 才能和窗宽窗位对上。
        ///
        /// 这是最容易漏的一步:漏了之后窗位 -600 会作用在 0..4095 的存储值上,
        /// 整张图全白。
        #[test]
        fn rescale_runs_before_the_window() {
            let obj = object(vec![
                DataElement::new(tags::MODALITY, VR::CS, PrimitiveValue::from("CT")),
                ds(tags::RESCALE_INTERCEPT, "-1024"),
                ds(tags::RESCALE_SLOPE, "1"),
                ds(tags::WINDOW_CENTER, "-600"),
                ds(tags::WINDOW_WIDTH, "1500"),
                DataElement::new(
                    tags::PHOTOMETRIC_INTERPRETATION,
                    VR::CS,
                    PrimitiveValue::from("MONOCHROME2"),
                ),
            ]);
            let pipeline = Pipeline::from_object(&obj);

            // 存储值 424 → HU = 424 - 1024 = -600,正好是窗心 → 约中灰
            let at_center = pipeline.apply(424.0, None);
            assert!(
                (120..=136).contains(&at_center),
                "窗心处应约为中灰 128,实际 {at_center}"
            );

            // 存储值 0 → HU = -1024,远在窗下界(-1350)之外?
            // 下界 = -600 - 0.5 - 749.5 = -1350,所以 -1024 在窗内偏暗
            let air = pipeline.apply(0.0, None);
            assert!(air < at_center, "空气应比窗心暗,实际 {air} vs {at_center}");

            // 存储值 3024 → HU = 2000,远超窗上界 → 纯白
            assert_eq!(pipeline.apply(3024.0, None), 255, "致密骨应饱和成白");

            // 对照:如果漏了 Rescale,存储值 424 会被当成 424 HU,
            // 那在 -600±750 的窗里已经饱和成白 —— 与上面的中灰截然不同。
            let without_rescale = Pipeline {
                modality_lut: ModalityLut::default(),
                ..pipeline.clone()
            };
            assert_eq!(
                without_rescale.apply(424.0, None),
                255,
                "漏做 Rescale 的症状:本该中灰的组织变成纯白"
            );
        }

        /// 多组窗宽窗位要全部保留,并带上各自的说明。
        #[test]
        fn multiple_window_presets_are_all_kept() {
            let obj = object(vec![
                DataElement::new(tags::MODALITY, VR::CS, PrimitiveValue::from("CT")),
                DataElement::new(tags::WINDOW_CENTER, VR::DS, multi(&["-600", "50"])),
                DataElement::new(tags::WINDOW_WIDTH, VR::DS, multi(&["1500", "350"])),
                DataElement::new(
                    tags::WINDOW_CENTER_WIDTH_EXPLANATION,
                    VR::LO,
                    multi(&["LUNG", "MEDIASTINUM"]),
                ),
            ]);
            let pipeline = Pipeline::from_object(&obj);

            assert_eq!(pipeline.windows.len(), 2);
            assert_eq!(pipeline.windows[0].center, -600.0);
            assert_eq!(pipeline.windows[0].explanation.as_deref(), Some("LUNG"));
            assert_eq!(pipeline.windows[1].center, 50.0);
            assert_eq!(
                pipeline.windows[1].explanation.as_deref(),
                Some("MEDIASTINUM")
            );
            // 默认用第一组
            assert_eq!(pipeline.default_window().center, -600.0);

            // 切到第二组应当得到不同结果
            let lung = pipeline.apply(0.0, Some(&pipeline.windows[0]));
            let medi = pipeline.apply(0.0, Some(&pipeline.windows[1]));
            assert_ne!(lung, medi, "不同预设应产生不同灰度");
        }

        /// 窗心窗宽的值数不一致时只取能配对的,不错位配。
        #[test]
        fn mismatched_window_value_counts_only_pair_what_matches() {
            let obj = object(vec![
                DataElement::new(tags::WINDOW_CENTER, VR::DS, multi(&["-600", "50", "300"])),
                DataElement::new(tags::WINDOW_WIDTH, VR::DS, multi(&["1500", "350"])),
            ]);
            let pipeline = Pipeline::from_object(&obj);
            assert_eq!(pipeline.windows.len(), 2, "只有两组能配上");
            assert_eq!(pipeline.windows[0].width, 1500.0);
            assert_eq!(pipeline.windows[1].width, 350.0);
        }

        /// SIGMOID 要被识别出来。
        #[test]
        fn sigmoid_function_is_recognized() {
            let obj = object(vec![
                ds(tags::WINDOW_CENTER, "0"),
                ds(tags::WINDOW_WIDTH, "100"),
                DataElement::new(
                    tags::VOILUT_FUNCTION,
                    VR::CS,
                    PrimitiveValue::from("SIGMOID"),
                ),
            ]);
            let pipeline = Pipeline::from_object(&obj);
            assert_eq!(pipeline.default_window().function, VoiFunction::Sigmoid);

            // 缺失时按标准缺省 LINEAR
            let plain = object(vec![
                ds(tags::WINDOW_CENTER, "0"),
                ds(tags::WINDOW_WIDTH, "100"),
            ]);
            assert_eq!(
                Pipeline::from_object(&plain).default_window().function,
                VoiFunction::Linear
            );
        }

        /// MONOCHROME1 的 X 光要被识别 —— 漏判得到负片。
        #[test]
        fn monochrome1_radiograph_is_detected() {
            let obj = object(vec![
                DataElement::new(tags::MODALITY, VR::CS, PrimitiveValue::from("CR")),
                DataElement::new(
                    tags::PHOTOMETRIC_INTERPRETATION,
                    VR::CS,
                    PrimitiveValue::from("MONOCHROME1"),
                ),
                ds(tags::WINDOW_CENTER, "2048"),
                ds(tags::WINDOW_WIDTH, "4096"),
            ]);
            let pipeline = Pipeline::from_object(&obj);
            assert!(pipeline.photometric.inverts());
            // 低存储值在 MONOCHROME1 下应当偏白
            assert!(
                pipeline.apply(0.0, None) > 200,
                "MONOCHROME1 的 0 应接近白,实际 {}",
                pipeline.apply(0.0, None)
            );
        }

        /// 缺窗宽窗位时按全量程兜底,保证整张图可见。
        #[test]
        fn missing_window_falls_back_to_the_full_range() {
            let obj = object(vec![
                DataElement::new(tags::MODALITY, VR::CS, PrimitiveValue::from("CT")),
                DataElement::new(tags::BITS_STORED, VR::US, PrimitiveValue::from(12_u16)),
                ds(tags::RESCALE_INTERCEPT, "-1024"),
                ds(tags::RESCALE_SLOPE, "1"),
            ]);
            let pipeline = Pipeline::from_object(&obj);
            assert_eq!(pipeline.windows.len(), 1);

            // 12 位无符号 = 0..4095,经 Rescale = -1024..3071,中心约 1023.5
            let window = pipeline.default_window();
            assert!(
                (window.center - 1023.5).abs() < 1.0,
                "全量程窗心应约 1023.5,实际 {}",
                window.center
            );
            // 两个端点都要可见(不饱和到同一个值)
            assert_eq!(pipeline.apply(0.0, None), 0);
            assert_eq!(pipeline.apply(4095.0, None), 255);
        }

        /// 有符号像素的全量程要算对范围。
        #[test]
        fn signed_pixels_get_a_signed_full_range() {
            let obj = object(vec![
                DataElement::new(tags::BITS_STORED, VR::US, PrimitiveValue::from(16_u16)),
                DataElement::new(
                    tags::PIXEL_REPRESENTATION,
                    VR::US,
                    PrimitiveValue::from(1_u16),
                ),
            ]);
            let pipeline = Pipeline::from_object(&obj);
            assert!(pipeline.signed);
            // 16 位有符号 = -32768..32767,中心约 -0.5
            assert!(
                pipeline.default_window().center.abs() < 1.0,
                "有符号全量程的窗心应接近 0,实际 {}",
                pipeline.default_window().center
            );
            assert_eq!(pipeline.apply(-32768.0, None), 0);
            assert_eq!(pipeline.apply(32767.0, None), 255);
        }

        /// slope 为 0 会把整张图压成常数 —— 按 1.0 处理并保留 intercept。
        #[test]
        fn zero_rescale_slope_is_rejected() {
            let obj = object(vec![
                ds(tags::RESCALE_SLOPE, "0"),
                ds(tags::RESCALE_INTERCEPT, "-1024"),
            ]);
            let pipeline = Pipeline::from_object(&obj);
            assert_eq!(pipeline.modality_lut.slope, 1.0);
            assert_eq!(pipeline.modality_lut.intercept, -1024.0);
        }

        /// 只给 intercept 时 slope 取标准缺省 1.0,反之亦然。
        #[test]
        fn partial_rescale_uses_standard_defaults() {
            let only_intercept = object(vec![ds(tags::RESCALE_INTERCEPT, "-1024")]);
            let lut = Pipeline::from_object(&only_intercept).modality_lut;
            assert_eq!(lut.slope, 1.0);
            assert_eq!(lut.intercept, -1024.0);

            let only_slope = object(vec![ds(tags::RESCALE_SLOPE, "2")]);
            let lut = Pipeline::from_object(&only_slope).modality_lut;
            assert_eq!(lut.slope, 2.0);
            assert_eq!(lut.intercept, 0.0);

            // 都没有 → 恒等
            let neither = object(vec![]);
            assert!(Pipeline::from_object(&neither).modality_lut.is_identity());
        }

        /// 宽度为 0 或负的窗要被丢掉,不能当成有效预设。
        #[test]
        fn degenerate_windows_are_discarded() {
            let obj = object(vec![
                DataElement::new(tags::WINDOW_CENTER, VR::DS, multi(&["0", "50"])),
                DataElement::new(tags::WINDOW_WIDTH, VR::DS, multi(&["0", "350"])),
            ]);
            let pipeline = Pipeline::from_object(&obj);
            assert_eq!(pipeline.windows.len(), 1, "宽度 0 的那组应被丢掉");
            assert_eq!(pipeline.default_window().width, 350.0);

            // 全都无效 → 退回全量程兜底,而不是留一个空列表
            let all_bad = object(vec![
                ds(tags::WINDOW_CENTER, "0"),
                ds(tags::WINDOW_WIDTH, "-100"),
            ]);
            let fallback = Pipeline::from_object(&all_bad);
            assert_eq!(fallback.windows.len(), 1);
            assert!(fallback.default_window().width > 0.0);
        }
    }
}
