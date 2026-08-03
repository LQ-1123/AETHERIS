//! 断层序列的空间排序。
//!
//! # 为什么不能用 InstanceNumber
//!
//! `InstanceNumber` (0020,0013) 是 Type 2 —— 可以为空,而且标准没有要求它
//! 反映空间顺序。真实设备上它会:
//!
//! - **缺失或重复**。多回波、多期相序列里,同一个空间位置有多个实例,
//!   编号可能重复或交错。
//! - **反映采集顺序而非空间顺序**。螺旋扫描、往复扫描的采集顺序与解剖顺序不同。
//! - **被重建覆盖**。同一份原始数据的不同重建会各自重新编号。
//!
//! 按它排序的后果是**切片顺序错乱**:翻页时解剖结构跳跃,而医生会以为
//! 那是病变。更隐蔽的是只错几张 —— 那看起来像运动伪影。
//!
//! `SliceLocation` (0020,1041) 同样不可靠:它是可选的,符号约定由厂商自定,
//! 而且怎么算的没有标准规定。
//!
//! # 正确做法
//!
//! 切片法向量 = 行方向 × 列方向(`ImageOrientationPatient` 的两个方向余弦
//! 叉积),再把每张切片的 `ImagePositionPatient` 投影到该法向量上:
//!
//! ```text
//! normal = row_cosines × column_cosines
//! key    = position · normal
//! ```
//!
//! 这个投影值是切片在解剖轴上的真实坐标,单位毫米,与设备的编号习惯无关。

use crate::model::InstanceMeta;

/// 三维向量。只为本模块的几何计算服务,不追求通用。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    fn from_slice(values: &[f64]) -> Option<Self> {
        let (&x, &y, &z) = (values.first()?, values.get(1)?, values.get(2)?);
        (x.is_finite() && y.is_finite() && z.is_finite()).then_some(Self { x, y, z })
    }

    pub fn cross(self, other: Self) -> Self {
        Self {
            x: self.y * other.z - self.z * other.y,
            y: self.z * other.x - self.x * other.z,
            z: self.x * other.y - self.y * other.x,
        }
    }

    pub fn dot(self, other: Self) -> f64 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    pub fn length(self) -> f64 {
        self.dot(self).sqrt()
    }

    /// 归一化。长度为 0 时返回 `None` —— 那说明方向余弦是退化的。
    pub fn normalized(self) -> Option<Self> {
        let length = self.length();
        (length > EPSILON).then(|| Self {
            x: self.x / length,
            y: self.y / length,
            z: self.z / length,
        })
    }
}

/// 几何计算的容差。
///
/// 方向余弦是无量纲的单位向量分量,真实数据里的数值噪声在 1e-6 量级;
/// 而任何有意义的方向差异(哪怕 0.01 度)都远大于 1e-9。
const EPSILON: f64 = 1e-9;

/// 判定两组方向余弦是否算同一个平面朝向的容差。
///
/// 1e-4 对应约 0.006 度。同一序列内的切片朝向应当完全一致(逐位相同或只差
/// 浮点噪声);真实的机架倾斜、或混进来的定位像,差异都远大于此。
const ORIENTATION_TOLERANCE: f64 = 1e-4;

/// 一张切片的空间信息。
#[derive(Debug, Clone, PartialEq)]
pub struct SliceGeometry {
    /// 在法向量上的投影,单位毫米。这就是排序键。
    pub position_along_normal: f64,
    pub position: Vec3,
}

/// 排序的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct SortedSeries {
    /// 按 `position_along_normal` 升序排列的下标,指向调用方传入的切片数组。
    ///
    /// 返回下标而不是重排后的数据:调用方的元素可能很大(含文件路径、
    /// 完整属性),而且它往往需要知道"原来的第几张"来对应已下载的帧缓存。
    pub order: Vec<usize>,
    /// 切片法向量(已归一化)。
    ///
    /// 交给调用方是为了多平面重建和"翻转显示方向"—— 后者是显示偏好,
    /// 不该由排序函数替用户决定,所以这里固定升序,方向留给界面。
    pub normal: Vec3,
    /// 相邻切片的间距中位数,单位毫米。
    ///
    /// 用中位数而不是平均值:少数几个异常间距(漏传、定位像混入)会把平均值
    /// 拉偏,而中位数不受影响。
    pub median_spacing_mm: f64,
    /// 间距是否均匀。
    ///
    /// 不均匀不阻止排序 —— 顺序仍然是对的,但界面应当提示:
    /// 断层序列本该等距,不等距通常意味着**漏传了切片**。
    /// 在这种序列上做距离测量或三维重建会得出错误结果。
    pub spacing_is_regular: bool,
    /// 投影值相同的切片组数(多回波、多期相、或重复传输)。
    ///
    /// 非零时界面不该简单地按顺序翻页 —— 同一位置的多张图是不同的东西。
    pub duplicate_position_groups: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GeometryError {
    #[error("序列为空")]
    Empty,
    #[error("第 {index} 张切片缺少 ImagePositionPatient 或 ImageOrientationPatient")]
    MissingGeometry { index: usize },
    #[error("第 {index} 张切片的 ImageOrientationPatient 退化(两个方向平行或为零)")]
    DegenerateOrientation { index: usize },
    #[error("第 {index} 张切片的朝向与序列其余部分不同 —— 混入了定位像,或这不是同一个序列")]
    InconsistentOrientation { index: usize },
}

/// 一张切片的几何输入。
///
/// 做成独立结构而不是直接吃 [`InstanceMeta`],是为了让 Tauri 查看器能从
/// 本地文件直接构造(它不经过服务端的元数据模型)。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SliceInput<'a> {
    /// (0020,0032) ImagePositionPatient,三个值。
    pub position: &'a [f64],
    /// (0020,0037) ImageOrientationPatient,六个值:行方向三个 + 列方向三个。
    pub orientation: &'a [f64],
}

impl<'a> SliceInput<'a> {
    /// 从服务端的元数据模型取几何。
    pub fn from_meta(meta: &'a InstanceMeta) -> Option<Self> {
        Some(Self {
            position: meta.image_position_patient.as_deref()?,
            orientation: meta.image_orientation_patient.as_deref()?,
        })
    }
}

/// 按空间位置给一组切片排序。
///
/// 全部切片必须朝向一致 —— 朝向不同就没有共同的法向量可投影,
/// 硬排会得出无意义的顺序。混进定位像的序列会在这里报错,
/// 那正是希望的:让调用方先把定位像分出去,而不是排出一个错的顺序。
pub fn sort_slices(slices: &[SliceInput<'_>]) -> Result<SortedSeries, GeometryError> {
    if slices.is_empty() {
        return Err(GeometryError::Empty);
    }

    let reference = orientation_of(slices[0], 0)?;
    let normal = reference
        .row
        .cross(reference.column)
        .normalized()
        .ok_or(GeometryError::DegenerateOrientation { index: 0 })?;

    let mut geometries: Vec<(usize, f64)> = Vec::with_capacity(slices.len());
    for (index, slice) in slices.iter().enumerate() {
        let orientation = orientation_of(*slice, index)?;
        // 朝向必须一致。只比方向余弦本身,不比法向量 —— 法向量相同但行列
        // 互换(旋转 90 度)的两张图,像素是转过的,不能当成同一朝向。
        if !orientation.matches(&reference) {
            return Err(GeometryError::InconsistentOrientation { index });
        }
        let position =
            Vec3::from_slice(slice.position).ok_or(GeometryError::MissingGeometry { index })?;
        geometries.push((index, position.dot(normal)));
    }

    // 升序。NaN 已在 from_slice 里排除,所以 partial_cmp 不会失败;
    // 仍然用 total_cmp 而不是 unwrap,避免将来放宽校验时留下 panic 隐患。
    geometries.sort_by(|a, b| a.1.total_cmp(&b.1));

    let keys: Vec<f64> = geometries.iter().map(|(_, key)| *key).collect();
    let order: Vec<usize> = geometries.iter().map(|(index, _)| *index).collect();

    let gaps: Vec<f64> = keys.windows(2).map(|pair| pair[1] - pair[0]).collect();
    let duplicate_position_groups = count_duplicate_groups(&gaps);
    let median_spacing_mm = median_positive(&gaps);
    let spacing_is_regular = is_regular(&gaps, median_spacing_mm);

    Ok(SortedSeries {
        order,
        normal,
        median_spacing_mm,
        spacing_is_regular,
        duplicate_position_groups,
    })
}

/// 按兼容的 ImageOrientationPatient 将切片拆成独立图像堆栈。
///
/// 返回值中的下标指向原始输入，并保持各朝向首次出现的顺序。该函数只负责
/// 拆分朝向，不负责空间排序；调用方应继续对每一组调用 [`sort_slices`]。
/// 所有切片仍必须提供有效的位置和方向，避免把缺少几何信息的文件悄悄混入。
pub fn group_slices_by_orientation(
    slices: &[SliceInput<'_>],
) -> Result<Vec<Vec<usize>>, GeometryError> {
    if slices.is_empty() {
        return Err(GeometryError::Empty);
    }

    let mut groups: Vec<(Orientation, Vec<usize>)> = Vec::new();
    for (index, slice) in slices.iter().copied().enumerate() {
        let orientation = orientation_of(slice, index)?;
        Vec3::from_slice(slice.position).ok_or(GeometryError::MissingGeometry { index })?;

        if let Some((_, indices)) = groups
            .iter_mut()
            .find(|(reference, _)| orientation.matches(reference))
        {
            indices.push(index);
        } else {
            groups.push((orientation, vec![index]));
        }
    }

    Ok(groups.into_iter().map(|(_, indices)| indices).collect())
}

struct Orientation {
    row: Vec3,
    column: Vec3,
}

impl Orientation {
    fn matches(&self, other: &Self) -> bool {
        let close = |a: Vec3, b: Vec3| {
            (a.x - b.x).abs() < ORIENTATION_TOLERANCE
                && (a.y - b.y).abs() < ORIENTATION_TOLERANCE
                && (a.z - b.z).abs() < ORIENTATION_TOLERANCE
        };
        close(self.row, other.row) && close(self.column, other.column)
    }
}

fn orientation_of(slice: SliceInput<'_>, index: usize) -> Result<Orientation, GeometryError> {
    if slice.orientation.len() < 6 || slice.position.len() < 3 {
        return Err(GeometryError::MissingGeometry { index });
    }
    let row = Vec3::from_slice(&slice.orientation[0..3])
        .ok_or(GeometryError::MissingGeometry { index })?;
    let column = Vec3::from_slice(&slice.orientation[3..6])
        .ok_or(GeometryError::MissingGeometry { index })?;

    // 两个方向余弦都必须是非零向量,且不能平行 —— 平行的话叉积为零,
    // 没有法向量可用。标准要求它们正交,但不强求我们去校验正交性:
    // 轻微非正交(浮点误差、厂商粗糙实现)不影响排序,而平行是致命的。
    let row = row
        .normalized()
        .ok_or(GeometryError::DegenerateOrientation { index })?;
    let column = column
        .normalized()
        .ok_or(GeometryError::DegenerateOrientation { index })?;
    if row.cross(column).length() < EPSILON {
        return Err(GeometryError::DegenerateOrientation { index });
    }

    Ok(Orientation { row, column })
}

/// 投影值相同(间距近似 0)的相邻切片会形成"组"。
fn count_duplicate_groups(gaps: &[f64]) -> usize {
    let mut groups = 0;
    let mut in_group = false;
    for gap in gaps {
        // 用绝对容差:投影值单位是毫米,同一位置的两张图差值在 1e-6 量级,
        // 而真实的最薄切片(0.5mm)远大于 1e-3。
        if gap.abs() < 1e-3 {
            if !in_group {
                groups += 1;
                in_group = true;
            }
        } else {
            in_group = false;
        }
    }
    groups
}

/// 正间距的中位数。全是重复位置时返回 0。
fn median_positive(gaps: &[f64]) -> f64 {
    let mut positive: Vec<f64> = gaps.iter().copied().filter(|gap| *gap > 1e-3).collect();
    if positive.is_empty() {
        return 0.0;
    }
    positive.sort_by(f64::total_cmp);
    let middle = positive.len() / 2;
    if positive.len().is_multiple_of(2) {
        (positive[middle - 1] + positive[middle]) / 2.0
    } else {
        positive[middle]
    }
}

/// 间距是否均匀。
///
/// 容差取中位数的 1%,下限 1e-3 毫米。真实的漏传会让某个间距翻倍
/// (差 100%),远超容差;而重建产生的浮点噪声在 1e-6 量级。
fn is_regular(gaps: &[f64], median: f64) -> bool {
    if median <= 0.0 {
        // 没有有效间距(单张,或全是重复位置)—— 谈不上均匀不均匀,
        // 报 true 免得界面对单张图也弹提示。
        return true;
    }
    let tolerance = (median * 0.01).max(1e-3);
    gaps.iter()
        .filter(|gap| gap.abs() > 1e-3) // 重复位置单独由 duplicate_position_groups 反映
        .all(|gap| (gap - median).abs() <= tolerance)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 标准轴位:行沿 +x,列沿 +y,法向量为 +z。
    const AXIAL: [f64; 6] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0];

    fn axial_slices(z_values: &[f64]) -> (Vec<[f64; 3]>, [f64; 6]) {
        (z_values.iter().map(|z| [0.0, 0.0, *z]).collect(), AXIAL)
    }

    fn inputs<'a>(positions: &'a [[f64; 3]], orientation: &'a [f64; 6]) -> Vec<SliceInput<'a>> {
        positions
            .iter()
            .map(|position| SliceInput {
                position,
                orientation,
            })
            .collect()
    }

    #[test]
    fn axial_slices_sort_by_z() {
        // 刻意乱序输入
        let (positions, orientation) = axial_slices(&[5.0, 1.0, 3.0, 2.0, 4.0]);
        let sorted = sort_slices(&inputs(&positions, &orientation)).expect("应能排序");

        // 期望下标顺序:z=1(下标1) → 2(下标3) → 3(下标2) → 4(下标4) → 5(下标0)
        assert_eq!(sorted.order, vec![1, 3, 2, 4, 0]);
        assert_eq!(sorted.normal, Vec3::new(0.0, 0.0, 1.0));
        assert!((sorted.median_spacing_mm - 1.0).abs() < 1e-9);
        assert!(sorted.spacing_is_regular);
        assert_eq!(sorted.duplicate_position_groups, 0);
    }

    /// InstanceNumber 与空间顺序相反时,排序必须按空间来。
    ///
    /// 这是本模块存在的理由:设备按采集顺序编号,而采集是从头顶往下,
    /// 解剖坐标 z 却是往上增加的。
    #[test]
    fn spatial_order_ignores_acquisition_order() {
        // 假设采集顺序(即 InstanceNumber 顺序)是 z 递减
        let (positions, orientation) = axial_slices(&[100.0, 95.0, 90.0, 85.0]);
        let sorted = sort_slices(&inputs(&positions, &orientation)).unwrap();
        // 结果应当是 z 升序,即输入下标倒序
        assert_eq!(sorted.order, vec![3, 2, 1, 0]);
        assert!((sorted.median_spacing_mm - 5.0).abs() < 1e-9);
    }

    /// 冠状位:法向量应当是 y 方向而不是 z。
    #[test]
    fn coronal_orientation_yields_a_y_normal() {
        // 行沿 +x,列沿 +z → 法向量 = x × z = -y
        let orientation = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
        let positions = [[0.0, 3.0, 0.0], [0.0, 1.0, 0.0], [0.0, 2.0, 0.0]];
        let sorted = sort_slices(&inputs(&positions, &orientation)).unwrap();

        // 法向量是 -y,所以投影值 = -y 坐标,y 越大投影越小
        assert!(
            (sorted.normal.y - -1.0).abs() < 1e-9,
            "法向量应为 -y:{:?}",
            sorted.normal
        );
        // 投影升序 = y 降序 → y=3 在最前
        assert_eq!(sorted.order, vec![0, 2, 1]);
    }

    /// 斜位:法向量不是坐标轴,投影仍然要算对。
    #[test]
    fn oblique_orientation_projects_onto_the_true_normal() {
        // 行沿 +x,列在 yz 平面内 45 度
        let s = std::f64::consts::FRAC_1_SQRT_2;
        let orientation = [1.0, 0.0, 0.0, 0.0, s, s];
        // 法向量 = x × (0,s,s) = (0*s - 0*s, 0*0 - 1*s, 1*s - 0*0) = (0, -s, s)
        // 沿法向量等距移动:每步 (0, -s, s) * 2mm
        let positions = [
            [0.0, 0.0, 0.0],
            [0.0, -s * 2.0, s * 2.0],
            [0.0, -s * 4.0, s * 4.0],
        ];
        let sorted = sort_slices(&inputs(&positions, &orientation)).unwrap();
        assert_eq!(sorted.order, vec![0, 1, 2]);
        // 每步在法向量上的投影正好是 2mm
        assert!(
            (sorted.median_spacing_mm - 2.0).abs() < 1e-9,
            "斜位的间距应为 2mm,实际 {}",
            sorted.median_spacing_mm
        );
        assert!(sorted.spacing_is_regular);
    }

    /// 漏传切片会让某个间距翻倍 —— 必须报出来。
    ///
    /// 顺序仍然是对的,但在这种序列上做三维重建或跨切片测距会算错。
    #[test]
    fn a_missing_slice_makes_the_spacing_irregular() {
        // z = 1,2,3,5,6 —— 缺了 4
        let (positions, orientation) = axial_slices(&[1.0, 2.0, 3.0, 5.0, 6.0]);
        let sorted = sort_slices(&inputs(&positions, &orientation)).unwrap();

        assert_eq!(sorted.order, vec![0, 1, 2, 3, 4], "顺序本身仍然正确");
        assert!(
            !sorted.spacing_is_regular,
            "漏了一张切片,间距不该被判为均匀"
        );
        // 中位数仍是 1(多数间距是 1),不被那个 2 拉偏
        assert!(
            (sorted.median_spacing_mm - 1.0).abs() < 1e-9,
            "中位数应为 1,实际 {}",
            sorted.median_spacing_mm
        );
    }

    /// 浮点噪声不该被判成不均匀。
    #[test]
    fn floating_point_noise_still_counts_as_regular() {
        let (positions, orientation) = axial_slices(&[0.0, 2.5, 5.000000001, 7.499999998, 10.0]);
        let sorted = sort_slices(&inputs(&positions, &orientation)).unwrap();
        assert!(
            sorted.spacing_is_regular,
            "1e-9 量级的噪声不该触发不均匀提示"
        );
    }

    /// 同一位置的多张图(多回波、多期相、重复传输)要被识别。
    #[test]
    fn duplicate_positions_are_reported() {
        let (positions, orientation) = axial_slices(&[1.0, 1.0, 2.0, 3.0, 3.0]);
        let sorted = sort_slices(&inputs(&positions, &orientation)).unwrap();
        assert_eq!(
            sorted.duplicate_position_groups, 2,
            "z=1 和 z=3 各有一组重复"
        );
        // 重复位置不影响间距中位数的计算
        assert!((sorted.median_spacing_mm - 1.0).abs() < 1e-9);
    }

    /// 混进定位像(朝向不同)必须报错,而不是排出一个无意义的顺序。
    #[test]
    fn a_localizer_with_a_different_orientation_is_rejected() {
        let axial = AXIAL;
        let sagittal = [0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        let p0 = [0.0, 0.0, 1.0];
        let p1 = [0.0, 0.0, 2.0];
        let p2 = [0.0, 0.0, 3.0];

        let slices = vec![
            SliceInput {
                position: &p0,
                orientation: &axial,
            },
            SliceInput {
                position: &p1,
                orientation: &sagittal, // 定位像
            },
            SliceInput {
                position: &p2,
                orientation: &axial,
            },
        ];
        assert_eq!(
            sort_slices(&slices),
            Err(GeometryError::InconsistentOrientation { index: 1 })
        );
    }

    #[test]
    fn mixed_orientations_can_be_split_before_sorting() {
        let axial = AXIAL;
        let sagittal = [0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        let positions = [
            [0.0, 0.0, 3.0],
            [4.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [2.0, 0.0, 0.0],
        ];
        let slices = vec![
            SliceInput {
                position: &positions[0],
                orientation: &axial,
            },
            SliceInput {
                position: &positions[1],
                orientation: &sagittal,
            },
            SliceInput {
                position: &positions[2],
                orientation: &axial,
            },
            SliceInput {
                position: &positions[3],
                orientation: &sagittal,
            },
        ];

        assert_eq!(
            group_slices_by_orientation(&slices).unwrap(),
            vec![vec![0, 2], vec![1, 3]]
        );
    }

    #[test]
    fn orientation_grouping_still_rejects_missing_geometry() {
        let missing_position: [f64; 0] = [];
        let slices = [SliceInput {
            position: &missing_position,
            orientation: &AXIAL,
        }];

        assert_eq!(
            group_slices_by_orientation(&slices),
            Err(GeometryError::MissingGeometry { index: 0 })
        );
    }

    /// 法向量相同但行列互换(图像转了 90 度)也算朝向不一致。
    ///
    /// 两张图的平面确实是同一个,但像素是转过的 —— 当成同一朝向去显示,
    /// 翻页时图像会突然旋转。
    #[test]
    fn swapped_row_and_column_is_not_the_same_orientation() {
        let normal_axial = AXIAL;
        let rotated = [0.0, 1.0, 0.0, 1.0, 0.0, 0.0]; // 行列互换
        let p0 = [0.0, 0.0, 1.0];
        let p1 = [0.0, 0.0, 2.0];

        let slices = vec![
            SliceInput {
                position: &p0,
                orientation: &normal_axial,
            },
            SliceInput {
                position: &p1,
                orientation: &rotated,
            },
        ];
        assert!(
            matches!(
                sort_slices(&slices),
                Err(GeometryError::InconsistentOrientation { .. })
            ),
            "行列互换应被判为朝向不一致"
        );
    }

    #[test]
    fn degenerate_orientation_is_rejected() {
        // 行列平行 → 叉积为零,没有法向量
        let parallel = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        let p = [0.0, 0.0, 0.0];
        let slices = vec![SliceInput {
            position: &p,
            orientation: &parallel,
        }];
        assert_eq!(
            sort_slices(&slices),
            Err(GeometryError::DegenerateOrientation { index: 0 })
        );

        // 全零方向余弦
        let zeros = [0.0; 6];
        let slices = vec![SliceInput {
            position: &p,
            orientation: &zeros,
        }];
        assert_eq!(
            sort_slices(&slices),
            Err(GeometryError::DegenerateOrientation { index: 0 })
        );
    }

    #[test]
    fn missing_or_short_geometry_is_rejected() {
        let short_position = [0.0, 0.0];
        let slices = vec![SliceInput {
            position: &short_position,
            orientation: &AXIAL,
        }];
        assert_eq!(
            sort_slices(&slices),
            Err(GeometryError::MissingGeometry { index: 0 })
        );

        let short_orientation = [1.0, 0.0, 0.0];
        let position = [0.0, 0.0, 0.0];
        let slices = vec![SliceInput {
            position: &position,
            orientation: &short_orientation,
        }];
        assert_eq!(
            sort_slices(&slices),
            Err(GeometryError::MissingGeometry { index: 0 })
        );
    }

    #[test]
    fn nan_positions_are_rejected() {
        let nan_position = [0.0, 0.0, f64::NAN];
        let slices = vec![SliceInput {
            position: &nan_position,
            orientation: &AXIAL,
        }];
        assert_eq!(
            sort_slices(&slices),
            Err(GeometryError::MissingGeometry { index: 0 })
        );
    }

    #[test]
    fn empty_series_is_an_error() {
        assert_eq!(sort_slices(&[]), Err(GeometryError::Empty));
    }

    /// 单张切片:能排,但没有间距可言。
    #[test]
    fn a_single_slice_sorts_trivially() {
        let (positions, orientation) = axial_slices(&[42.0]);
        let sorted = sort_slices(&inputs(&positions, &orientation)).unwrap();
        assert_eq!(sorted.order, vec![0]);
        assert_eq!(sorted.median_spacing_mm, 0.0);
        assert!(
            sorted.spacing_is_regular,
            "单张图不该被判为间距不均匀 —— 界面会弹一个无意义的提示"
        );
    }

    /// 叉积的方向遵循右手定则,这决定了法向量的朝向。
    #[test]
    fn cross_product_follows_the_right_hand_rule() {
        let x = Vec3::new(1.0, 0.0, 0.0);
        let y = Vec3::new(0.0, 1.0, 0.0);
        let z = Vec3::new(0.0, 0.0, 1.0);
        assert_eq!(x.cross(y), z);
        assert_eq!(y.cross(z), x);
        assert_eq!(z.cross(x), y);
        // 反过来是负的
        assert_eq!(y.cross(x), Vec3::new(0.0, 0.0, -1.0));
    }

    /// 同一朝向用不同缩放表示时,不能被误判成朝向不一致。
    ///
    /// 标准要求方向余弦是单位向量,缩放过的本就不合规;但设备真送出来时,
    /// `[1,0,0,0,1,0]` 和 `[3,0,0,0,3,0]` 描述的是同一个平面朝向,
    /// 不该因为数值不同就拒绝整个序列。一致性比较必须在归一化之后做。
    #[test]
    fn the_same_orientation_at_different_scales_is_still_consistent() {
        let unit = AXIAL;
        let scaled = [3.0, 0.0, 0.0, 0.0, 3.0, 0.0];
        let p0 = [0.0, 0.0, 1.0];
        let p1 = [0.0, 0.0, 2.0];

        let slices = vec![
            SliceInput {
                position: &p0,
                orientation: &unit,
            },
            SliceInput {
                position: &p1,
                orientation: &scaled,
            },
        ];
        let sorted = sort_slices(&slices)
            .expect("同一朝向的不同缩放表示应当被接受 —— 一致性比较要在归一化之后做");
        assert_eq!(sorted.order, vec![0, 1]);
    }

    /// 未归一化的方向余弦(厂商偶尔送)也要能用。
    #[test]
    fn unnormalized_direction_cosines_are_normalized() {
        // 行列方向都放大 3 倍
        let scaled = [3.0, 0.0, 0.0, 0.0, 3.0, 0.0];
        let positions = [[0.0, 0.0, 0.0], [0.0, 0.0, 5.0]];
        let slices = inputs(&positions, &scaled);
        let sorted = sort_slices(&slices).unwrap();

        // 法向量必须是单位向量,否则投影值会被放大,间距算错
        assert!(
            (sorted.normal.length() - 1.0).abs() < 1e-9,
            "法向量应归一化,实际长度 {}",
            sorted.normal.length()
        );
        assert!(
            (sorted.median_spacing_mm - 5.0).abs() < 1e-9,
            "间距应为 5mm 而不是被缩放,实际 {}",
            sorted.median_spacing_mm
        );
    }
}
