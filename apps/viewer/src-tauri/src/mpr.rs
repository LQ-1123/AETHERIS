//! Orthogonal multi-planar reconstruction for a validated DICOM stack.

use pacs_codec::{Photometric, Pipeline, VoiFunction, Window};
use pacs_core::geometry::Vec3;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};

const ORIENTATION_TOLERANCE: f64 = 1e-4;
const SPACING_ABSOLUTE_TOLERANCE_MM: f64 = 0.1;
const SPACING_RELATIVE_TOLERANCE: f64 = 0.05;
const MAX_VOLUME_BYTES: usize = 768 * 1024 * 1024;
const SLICE_CACHE_LIMIT: usize = 192 * 1024 * 1024;
type SliceKey = (Plane, u32);

#[derive(Clone)]
pub struct SourceSlice {
    pub rows: u32,
    pub cols: u32,
    pub bits_allocated: u16,
    pub pipeline: Pipeline,
    pub position: Option<[f64; 3]>,
    pub orientation: Option<[f64; 6]>,
    pub row_spacing_mm: Option<f64>,
    pub col_spacing_mm: Option<f64>,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Plane {
    Axial,
    Coronal,
    Sagittal,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoiShape {
    Point,
    Rectangle,
    Ellipse,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct PixelStatistics {
    pub count: usize,
    pub mean: f64,
    pub standard_deviation: f64,
    pub minimum: f64,
    pub maximum: f64,
    pub area: Option<f64>,
    pub area_unit: Option<&'static str>,
    pub unit: Option<&'static str>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct MprMetadata {
    pub stack_index: u32,
    pub dimensions: [u32; 3],
    pub source_spacing_mm: [f64; 3],
    pub patient_bounds_min: [f64; 3],
    pub patient_bounds_max: [f64; 3],
    pub initial_crosshair: [f64; 3],
    pub planes: Vec<PlaneMetadata>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct PlaneMetadata {
    pub plane: Plane,
    pub rows: u32,
    pub cols: u32,
    pub slice_count: u32,
    pub pixel_spacing_mm: f64,
    pub slice_spacing_mm: f64,
    /// Patient position of image pixel (0, 0) on slice zero.
    pub origin: [f64; 3],
    /// Patient-space direction of increasing image x (columns).
    pub x_axis: [f64; 3],
    /// Patient-space direction of increasing image y (rows).
    pub y_axis: [f64; 3],
    /// Patient-space direction of increasing slice index.
    pub normal: [f64; 3],
}

pub struct Volume {
    stack_index: u32,
    rows: usize,
    cols: usize,
    slices: usize,
    row_spacing: f64,
    col_spacing: f64,
    slice_spacing: f64,
    origin: Vec3,
    row_direction: Vec3,
    column_direction: Vec3,
    normal: Vec3,
    values: Vec<f32>,
    pipeline: Pipeline,
    bounds_min: Vec3,
    bounds_max: Vec3,
    planes: [PlaneMetadata; 3],
    slice_cache: Mutex<SliceCache>,
}

struct SliceCache {
    data: HashMap<SliceKey, Arc<[f32]>>,
    access_queue: VecDeque<SliceKey>,
    total_bytes: usize,
    limit: usize,
}

struct SourceGeometry {
    origin: Vec3,
    row_direction: Vec3,
    column_direction: Vec3,
    normal: Vec3,
    cols: usize,
    rows: usize,
    slices: usize,
    col_spacing: f64,
    row_spacing: f64,
    slice_spacing: f64,
}

impl Volume {
    pub fn build(
        stack_index: u32,
        mut sources: Vec<SourceSlice>,
        mut cancelled: impl FnMut() -> bool,
        mut progress: impl FnMut(usize, usize),
    ) -> Result<Self, String> {
        if sources.len() < 2 {
            return Err("MPR 至少需要两张带患者空间几何信息的切片".to_owned());
        }
        let first = &sources[0];
        let orientation = first
            .orientation
            .ok_or_else(|| "第 1 张切片缺少 ImageOrientationPatient".to_owned())?;
        first
            .position
            .ok_or_else(|| "第 1 张切片缺少 ImagePositionPatient".to_owned())?;
        let row_direction = vec3(&orientation[0..3])
            .and_then(Vec3::normalized)
            .ok_or_else(|| "ImageOrientationPatient 的行方向无效".to_owned())?;
        let column_direction = vec3(&orientation[3..6])
            .and_then(Vec3::normalized)
            .ok_or_else(|| "ImageOrientationPatient 的列方向无效".to_owned())?;
        let normal = row_direction
            .cross(column_direction)
            .normalized()
            .ok_or_else(|| "ImageOrientationPatient 的两个方向平行".to_owned())?;
        let row_spacing = positive_spacing(first.row_spacing_mm, "PixelSpacing 行间距")?;
        let col_spacing = positive_spacing(first.col_spacing_mm, "PixelSpacing 列间距")?;
        let rows = first.rows as usize;
        let cols = first.cols as usize;
        let bits_allocated = first.bits_allocated;
        let pipeline = first.pipeline.clone();

        for (index, source) in sources.iter().enumerate() {
            if source.rows as usize != rows
                || source.cols as usize != cols
                || source.bits_allocated != bits_allocated
            {
                return Err(format!("第 {} 张切片的尺寸或像素位宽不一致", index + 1));
            }
            if !compatible_pipeline(&pipeline, &source.pipeline) {
                return Err(format!(
                    "第 {} 张切片的像素显示或 Rescale 参数不一致",
                    index + 1
                ));
            }
            let candidate = source
                .orientation
                .ok_or_else(|| format!("第 {} 张切片缺少 ImageOrientationPatient", index + 1))?;
            let candidate_row = vec3(&candidate[0..3])
                .and_then(Vec3::normalized)
                .ok_or_else(|| format!("第 {} 张切片的行方向无效", index + 1))?;
            let candidate_column = vec3(&candidate[3..6])
                .and_then(Vec3::normalized)
                .ok_or_else(|| format!("第 {} 张切片的列方向无效", index + 1))?;
            if 1.0 - candidate_row.dot(row_direction) > ORIENTATION_TOLERANCE
                || 1.0 - candidate_column.dot(column_direction) > ORIENTATION_TOLERANCE
            {
                return Err(format!("第 {} 张切片朝向与主堆栈不一致", index + 1));
            }
            let candidate_row_spacing =
                positive_spacing(source.row_spacing_mm, "PixelSpacing 行间距")?;
            let candidate_col_spacing =
                positive_spacing(source.col_spacing_mm, "PixelSpacing 列间距")?;
            if !close(candidate_row_spacing, row_spacing, 1e-5)
                || !close(candidate_col_spacing, col_spacing, 1e-5)
            {
                return Err(format!("第 {} 张切片的 PixelSpacing 不一致", index + 1));
            }
            if source.position.is_none() {
                return Err(format!("第 {} 张切片缺少 ImagePositionPatient", index + 1));
            }
        }

        sources.sort_by(|left, right| {
            projected_position(left, normal)
                .partial_cmp(&projected_position(right, normal))
                .unwrap_or(Ordering::Equal)
        });
        let positions = sources
            .iter()
            .map(|source| projected_position(source, normal))
            .collect::<Vec<_>>();
        let mut gaps = positions
            .windows(2)
            .map(|pair| pair[1] - pair[0])
            .collect::<Vec<_>>();
        if gaps.iter().any(|gap| !gap.is_finite() || *gap <= 1e-4) {
            return Err("序列包含重复或无效的切片位置，不能安全构建 MPR".to_owned());
        }
        gaps.sort_by(f64::total_cmp);
        let slice_spacing = gaps[gaps.len() / 2];
        let tolerance =
            SPACING_ABSOLUTE_TOLERANCE_MM.max(slice_spacing * SPACING_RELATIVE_TOLERANCE);
        if gaps
            .iter()
            .any(|gap| (gap - slice_spacing).abs() > tolerance)
        {
            return Err(format!(
                "切片间距不均匀（中位数 {slice_spacing:.3} mm），可能存在漏传切片"
            ));
        }

        let voxel_count = rows
            .checked_mul(cols)
            .and_then(|value| value.checked_mul(sources.len()))
            .ok_or_else(|| "体数据尺寸超过支持范围".to_owned())?;
        let bytes = voxel_count
            .checked_mul(std::mem::size_of::<f32>())
            .ok_or_else(|| "体数据尺寸超过支持范围".to_owned())?;
        if bytes > MAX_VOLUME_BYTES {
            return Err(format!(
                "体数据需要约 {:.0} MB，超过 768 MB 安全上限",
                bytes as f64 / (1024.0 * 1024.0)
            ));
        }

        let mut values = Vec::with_capacity(voxel_count);
        let expected = rows * cols * usize::from(bits_allocated / 8);
        let total = sources.len();
        for (index, source) in sources.iter().enumerate() {
            if cancelled() {
                return Err("已取消 MPR 构建".to_owned());
            }
            if source.bytes.len() != expected {
                return Err(format!(
                    "第 {} 张切片解码为 {} 字节，预期 {expected} 字节",
                    index + 1,
                    source.bytes.len()
                ));
            }
            decode_stored_values(&source.bytes, bits_allocated, &pipeline, &mut values);
            progress(index + 1, total);
        }

        let origin =
            vec3(&sources[0].position.expect("已验证位置")).expect("已验证患者位置为有限数值");
        let geometry = SourceGeometry {
            origin,
            row_direction,
            column_direction,
            normal,
            cols,
            rows,
            slices: sources.len(),
            col_spacing,
            row_spacing,
            slice_spacing,
        };
        let (bounds_min, bounds_max) = volume_bounds(&geometry);
        let output_spacing = row_spacing.min(col_spacing).min(slice_spacing);
        let planes = build_planes(bounds_min, bounds_max, output_spacing);

        Ok(Self {
            stack_index,
            rows,
            cols,
            slices: sources.len(),
            row_spacing,
            col_spacing,
            slice_spacing,
            origin,
            row_direction,
            column_direction,
            normal,
            values,
            pipeline,
            bounds_min,
            bounds_max,
            planes,
            slice_cache: Mutex::new(SliceCache::new(SLICE_CACHE_LIMIT)),
        })
    }

    pub fn metadata(&self) -> MprMetadata {
        MprMetadata {
            stack_index: self.stack_index,
            dimensions: [self.cols as u32, self.rows as u32, self.slices as u32],
            source_spacing_mm: [self.col_spacing, self.row_spacing, self.slice_spacing],
            patient_bounds_min: array(self.bounds_min),
            patient_bounds_max: array(self.bounds_max),
            initial_crosshair: [
                (self.bounds_min.x + self.bounds_max.x) / 2.0,
                (self.bounds_min.y + self.bounds_max.y) / 2.0,
                (self.bounds_min.z + self.bounds_max.z) / 2.0,
            ],
            planes: self.planes.to_vec(),
        }
    }

    pub fn render_slice(
        &self,
        plane: Plane,
        slice_index: u32,
        window_center: f64,
        window_width: f64,
        voi_function: &str,
    ) -> Result<Vec<u8>, String> {
        if !window_center.is_finite() || !window_width.is_finite() || window_width <= 0.0 {
            return Err("窗位必须有限且窗宽必须大于 0".to_owned());
        }
        let metadata = self
            .planes
            .iter()
            .find(|candidate| candidate.plane == plane)
            .expect("三个标准切面总是存在");
        if slice_index >= metadata.slice_count {
            return Err(format!(
                "MPR 切面越界: {slice_index} >= {}",
                metadata.slice_count
            ));
        }
        let function = match voi_function.trim().to_ascii_uppercase().as_str() {
            "LINEAR" => VoiFunction::Linear,
            "LINEAR_EXACT" => VoiFunction::LinearExact,
            "SIGMOID" => VoiFunction::Sigmoid,
            other => return Err(format!("未知 VOI 函数 {other}")),
        };
        let window = Window {
            center: window_center,
            width: window_width,
            explanation: Some("MPR".to_owned()),
            function,
        };
        let samples = self.resampled_slice(plane, slice_index, metadata);
        Ok(samples
            .iter()
            .map(|value| {
                if value.is_finite() {
                    self.pipeline.apply(f64::from(*value), Some(&window))
                } else {
                    0
                }
            })
            .collect())
    }

    pub fn measure_roi(
        &self,
        plane: Plane,
        slice_index: u32,
        shape: RoiShape,
        start: [f64; 2],
        end: [f64; 2],
    ) -> Result<PixelStatistics, String> {
        let metadata = self
            .planes
            .iter()
            .find(|candidate| candidate.plane == plane)
            .expect("三个标准切面总是存在");
        if slice_index >= metadata.slice_count {
            return Err(format!(
                "MPR 切面越界: {slice_index} >= {}",
                metadata.slice_count
            ));
        }
        let samples = self.resampled_slice(plane, slice_index, metadata);
        statistics_for_region(
            &samples,
            metadata.cols as usize,
            metadata.rows as usize,
            shape,
            start,
            end,
            self.pipeline.modality_lut.slope,
            self.pipeline.modality_lut.intercept,
            self.pipeline.modality_lut.unit,
            Some(metadata.pixel_spacing_mm * metadata.pixel_spacing_mm),
        )
    }

    fn resampled_slice(
        &self,
        plane: Plane,
        slice_index: u32,
        metadata: &PlaneMetadata,
    ) -> Arc<[f32]> {
        let key = (plane, slice_index);
        {
            let mut cache = self.slice_cache();
            if let Some(cached) = cache.get(&key) {
                return Arc::clone(cached);
            }
        }

        let samples: Arc<[f32]> = self.resample_slice(slice_index, metadata).into();
        let mut cache = self.slice_cache();
        if let Some(cached) = cache.get(&key) {
            return Arc::clone(cached);
        }
        cache.insert(key, Arc::clone(&samples));
        samples
    }

    fn resample_slice(&self, slice_index: u32, metadata: &PlaneMetadata) -> Vec<f32> {
        let patient_origin = add(
            vec3(&metadata.origin).expect("平面原点有效"),
            scale(
                vec3(&metadata.normal).expect("平面法向有效"),
                f64::from(slice_index) * metadata.slice_spacing_mm,
            ),
        );
        let origin = self.patient_to_voxel(patient_origin);
        let x_step = self.patient_vector_to_voxel(scale(
            vec3(&metadata.x_axis).expect("平面 x 轴有效"),
            metadata.pixel_spacing_mm,
        ));
        let y_step = self.patient_vector_to_voxel(scale(
            vec3(&metadata.y_axis).expect("平面 y 轴有效"),
            metadata.pixel_spacing_mm,
        ));
        let mut output = vec![f32::NAN; metadata.rows as usize * metadata.cols as usize];
        let mut row_origin = origin;
        for row in 0..metadata.rows as usize {
            let mut voxel = row_origin;
            for col in 0..metadata.cols as usize {
                if let Some(value) = self.sample_voxel(voxel.x, voxel.y, voxel.z) {
                    output[row * metadata.cols as usize + col] = value as f32;
                }
                voxel = add_voxel(voxel, x_step);
            }
            row_origin = add_voxel(row_origin, y_step);
        }
        output
    }

    fn patient_to_voxel(&self, patient: Vec3) -> VoxelPoint {
        let relative = subtract(patient, self.origin);
        VoxelPoint {
            x: relative.dot(self.row_direction) / self.col_spacing,
            y: relative.dot(self.column_direction) / self.row_spacing,
            z: relative.dot(self.normal) / self.slice_spacing,
        }
    }

    fn patient_vector_to_voxel(&self, vector: Vec3) -> VoxelPoint {
        VoxelPoint {
            x: vector.dot(self.row_direction) / self.col_spacing,
            y: vector.dot(self.column_direction) / self.row_spacing,
            z: vector.dot(self.normal) / self.slice_spacing,
        }
    }

    fn slice_cache(&self) -> std::sync::MutexGuard<'_, SliceCache> {
        self.slice_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn sample_voxel(&self, x: f64, y: f64, z: f64) -> Option<f64> {
        let maximum_x = (self.cols - 1) as f64;
        let maximum_y = (self.rows - 1) as f64;
        let maximum_z = (self.slices - 1) as f64;
        let epsilon = 1e-6;
        if x < -epsilon
            || y < -epsilon
            || z < -epsilon
            || x > maximum_x + epsilon
            || y > maximum_y + epsilon
            || z > maximum_z + epsilon
        {
            return None;
        }
        let x = x.clamp(0.0, maximum_x);
        let y = y.clamp(0.0, maximum_y);
        let z = z.clamp(0.0, maximum_z);
        let x0 = x.floor() as usize;
        let y0 = y.floor() as usize;
        let z0 = z.floor() as usize;
        let x1 = (x0 + 1).min(self.cols - 1);
        let y1 = (y0 + 1).min(self.rows - 1);
        let z1 = (z0 + 1).min(self.slices - 1);
        let tx = x - x0 as f64;
        let ty = y - y0 as f64;
        let tz = z - z0 as f64;
        let at = |slice: usize, row: usize, col: usize| -> f64 {
            self.values[(slice * self.rows + row) * self.cols + col] as f64
        };
        let c00 = lerp(at(z0, y0, x0), at(z0, y0, x1), tx);
        let c01 = lerp(at(z0, y1, x0), at(z0, y1, x1), tx);
        let c10 = lerp(at(z1, y0, x0), at(z1, y0, x1), tx);
        let c11 = lerp(at(z1, y1, x0), at(z1, y1, x1), tx);
        Some(lerp(lerp(c00, c01, ty), lerp(c10, c11, ty), tz))
    }
}

#[allow(clippy::too_many_arguments)]
pub fn statistics_for_region(
    samples: &[f32],
    cols: usize,
    rows: usize,
    shape: RoiShape,
    start: [f64; 2],
    end: [f64; 2],
    slope: f64,
    intercept: f64,
    unit: Option<&'static str>,
    pixel_area: Option<f64>,
) -> Result<PixelStatistics, String> {
    if cols == 0 || rows == 0 || samples.len() != cols * rows {
        return Err("像素缓冲区尺寸无效".to_owned());
    }
    if !start
        .iter()
        .chain(end.iter())
        .all(|value| value.is_finite())
    {
        return Err("测量坐标必须是有限数值".to_owned());
    }
    let mut values = Vec::new();
    if matches!(shape, RoiShape::Point) {
        let col = start[0].round().clamp(0.0, (cols - 1) as f64) as usize;
        let row = start[1].round().clamp(0.0, (rows - 1) as f64) as usize;
        let stored = f64::from(samples[row * cols + col]);
        if stored.is_finite() {
            values.push(stored * slope + intercept);
        }
    } else {
        let left = start[0].min(end[0]);
        let right = start[0].max(end[0]);
        let top = start[1].min(end[1]);
        let bottom = start[1].max(end[1]);
        let center_x = (left + right) / 2.0;
        let center_y = (top + bottom) / 2.0;
        let radius_x = (right - left) / 2.0;
        let radius_y = (bottom - top) / 2.0;
        let first_col = left.floor().max(0.0) as usize;
        let last_col = right.ceil().min((cols - 1) as f64) as usize;
        let first_row = top.floor().max(0.0) as usize;
        let last_row = bottom.ceil().min((rows - 1) as f64) as usize;
        for row in first_row..=last_row {
            for col in first_col..=last_col {
                let x = col as f64 + 0.5;
                let y = row as f64 + 0.5;
                let included = match shape {
                    RoiShape::Rectangle => x >= left && x <= right && y >= top && y <= bottom,
                    RoiShape::Ellipse if radius_x > 0.0 && radius_y > 0.0 => {
                        ((x - center_x) / radius_x).powi(2) + ((y - center_y) / radius_y).powi(2)
                            <= 1.0
                    }
                    _ => false,
                };
                if !included {
                    continue;
                }
                let stored = f64::from(samples[row * cols + col]);
                if stored.is_finite() {
                    values.push(stored * slope + intercept);
                }
            }
        }
    }
    if values.is_empty() {
        return Err("测量区域内没有有效像素".to_owned());
    }
    let count = values.len();
    let mean = values.iter().sum::<f64>() / count as f64;
    let variance = values
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f64>()
        / count as f64;
    let minimum = values.iter().copied().fold(f64::INFINITY, f64::min);
    let maximum = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let area = (!matches!(shape, RoiShape::Point))
        .then(|| pixel_area.map_or(count as f64, |value| value * count as f64));
    Ok(PixelStatistics {
        count,
        mean,
        standard_deviation: variance.sqrt(),
        minimum,
        maximum,
        area,
        area_unit: area.map(|_| if pixel_area.is_some() { "mm2" } else { "px2" }),
        unit,
    })
}

#[derive(Clone, Copy)]
struct VoxelPoint {
    x: f64,
    y: f64,
    z: f64,
}

impl SliceCache {
    fn new(limit: usize) -> Self {
        Self {
            data: HashMap::new(),
            access_queue: VecDeque::new(),
            total_bytes: 0,
            limit,
        }
    }

    fn get(&mut self, key: &SliceKey) -> Option<&Arc<[f32]>> {
        if self.data.contains_key(key) {
            self.access_queue.retain(|candidate| candidate != key);
            self.access_queue.push_back(*key);
            self.data.get(key)
        } else {
            None
        }
    }

    fn insert(&mut self, key: SliceKey, data: Arc<[f32]>) {
        let size = data.len() * std::mem::size_of::<f32>();
        if let Some(previous) = self.data.insert(key, data) {
            self.total_bytes -= previous.len() * std::mem::size_of::<f32>();
            self.access_queue.retain(|candidate| candidate != &key);
        }
        self.total_bytes += size;
        self.access_queue.push_back(key);
        while self.total_bytes > self.limit && self.data.len() > 1 {
            let Some(oldest) = self.access_queue.pop_front() else {
                break;
            };
            if let Some(removed) = self.data.remove(&oldest) {
                self.total_bytes -= removed.len() * std::mem::size_of::<f32>();
            }
        }
    }
}

fn compatible_pipeline(left: &Pipeline, right: &Pipeline) -> bool {
    left.modality_lut == right.modality_lut
        && left.photometric == right.photometric
        && left.bits_stored == right.bits_stored
        && left.signed == right.signed
        && left.photometric != Photometric::NotMonochrome
}

pub(crate) fn decode_stored_values(
    bytes: &[u8],
    bits_allocated: u16,
    pipeline: &Pipeline,
    output: &mut Vec<f32>,
) {
    let bits = pipeline.bits_stored.clamp(1, bits_allocated) as u32;
    let full_range = 1_u32 << bits;
    let mask = full_range - 1;
    let sign_bit = 1_u32 << (bits - 1);
    let convert = |raw: u32| -> f32 {
        let raw = raw & mask;
        if pipeline.signed && raw & sign_bit != 0 {
            (raw as i64 - full_range as i64) as f32
        } else {
            raw as f32
        }
    };
    if bits_allocated == 8 {
        output.extend(bytes.iter().map(|value| convert(u32::from(*value))));
    } else {
        output.extend(
            bytes
                .chunks_exact(2)
                .map(|pair| convert(u32::from(u16::from_le_bytes([pair[0], pair[1]])))),
        );
    }
}

fn build_planes(min: Vec3, max: Vec3, spacing: f64) -> [PlaneMetadata; 3] {
    let count = |length: f64| (length / spacing).ceil() as u32 + 1;
    let x_count = count(max.x - min.x);
    let y_count = count(max.y - min.y);
    let z_count = count(max.z - min.z);
    [
        PlaneMetadata {
            plane: Plane::Axial,
            rows: y_count,
            cols: x_count,
            slice_count: z_count,
            pixel_spacing_mm: spacing,
            slice_spacing_mm: spacing,
            origin: [max.x, min.y, min.z],
            x_axis: [-1.0, 0.0, 0.0],
            y_axis: [0.0, 1.0, 0.0],
            normal: [0.0, 0.0, 1.0],
        },
        PlaneMetadata {
            plane: Plane::Coronal,
            rows: z_count,
            cols: x_count,
            slice_count: y_count,
            pixel_spacing_mm: spacing,
            slice_spacing_mm: spacing,
            origin: [max.x, min.y, max.z],
            x_axis: [-1.0, 0.0, 0.0],
            y_axis: [0.0, 0.0, -1.0],
            normal: [0.0, 1.0, 0.0],
        },
        PlaneMetadata {
            plane: Plane::Sagittal,
            rows: z_count,
            cols: y_count,
            slice_count: x_count,
            pixel_spacing_mm: spacing,
            slice_spacing_mm: spacing,
            origin: [min.x, min.y, max.z],
            x_axis: [0.0, 1.0, 0.0],
            y_axis: [0.0, 0.0, -1.0],
            normal: [1.0, 0.0, 0.0],
        },
    ]
}

fn volume_bounds(geometry: &SourceGeometry) -> (Vec3, Vec3) {
    let mut minimum = Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut maximum = Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for z in [0.0, (geometry.slices - 1) as f64 * geometry.slice_spacing] {
        for y in [0.0, (geometry.rows - 1) as f64 * geometry.row_spacing] {
            for x in [0.0, (geometry.cols - 1) as f64 * geometry.col_spacing] {
                let point = add(
                    add(
                        add(geometry.origin, scale(geometry.row_direction, x)),
                        scale(geometry.column_direction, y),
                    ),
                    scale(geometry.normal, z),
                );
                minimum.x = minimum.x.min(point.x);
                minimum.y = minimum.y.min(point.y);
                minimum.z = minimum.z.min(point.z);
                maximum.x = maximum.x.max(point.x);
                maximum.y = maximum.y.max(point.y);
                maximum.z = maximum.z.max(point.z);
            }
        }
    }
    (minimum, maximum)
}

fn projected_position(source: &SourceSlice, normal: Vec3) -> f64 {
    vec3(&source.position.expect("调用前已验证位置"))
        .map(|position| position.dot(normal))
        .unwrap_or(f64::NAN)
}

fn positive_spacing(value: Option<f64>, name: &str) -> Result<f64, String> {
    value
        .filter(|candidate| candidate.is_finite() && *candidate > 0.0)
        .ok_or_else(|| format!("缺少或无法解析{name}"))
}

fn vec3(values: &[f64]) -> Option<Vec3> {
    let value = Vec3::new(*values.first()?, *values.get(1)?, *values.get(2)?);
    (value.x.is_finite() && value.y.is_finite() && value.z.is_finite()).then_some(value)
}

fn add(left: Vec3, right: Vec3) -> Vec3 {
    Vec3::new(left.x + right.x, left.y + right.y, left.z + right.z)
}

fn subtract(left: Vec3, right: Vec3) -> Vec3 {
    Vec3::new(left.x - right.x, left.y - right.y, left.z - right.z)
}

fn scale(value: Vec3, scalar: f64) -> Vec3 {
    Vec3::new(value.x * scalar, value.y * scalar, value.z * scalar)
}

fn add_voxel(left: VoxelPoint, right: VoxelPoint) -> VoxelPoint {
    VoxelPoint {
        x: left.x + right.x,
        y: left.y + right.y,
        z: left.z + right.z,
    }
}

fn array(value: Vec3) -> [f64; 3] {
    [value.x, value.y, value.z]
}

fn close(left: f64, right: f64, tolerance: f64) -> bool {
    (left - right).abs() <= tolerance
}

fn lerp(left: f64, right: f64, amount: f64) -> f64 {
    left + (right - left) * amount
}

#[cfg(test)]
mod tests {
    use super::*;
    use pacs_codec::{ModalityLut, Photometric};

    fn pipeline() -> Pipeline {
        Pipeline {
            modality_lut: ModalityLut::default(),
            windows: vec![Window {
                center: 50.0,
                width: 100.0,
                explanation: None,
                function: VoiFunction::Linear,
            }],
            photometric: Photometric::Monochrome2,
            bits_stored: 16,
            signed: false,
        }
    }

    fn source(z: f64, values: [u16; 4]) -> SourceSlice {
        SourceSlice {
            rows: 2,
            cols: 2,
            bits_allocated: 16,
            pipeline: pipeline(),
            position: Some([0.0, 0.0, z]),
            orientation: Some([1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
            row_spacing_mm: Some(1.0),
            col_spacing_mm: Some(1.0),
            bytes: values.into_iter().flat_map(u16::to_le_bytes).collect(),
        }
    }

    #[test]
    fn builds_patient_aligned_planes_and_samples_trilinearly() {
        let volume = Volume::build(
            0,
            vec![
                source(0.0, [0, 10, 20, 30]),
                source(1.0, [100, 110, 120, 130]),
            ],
            || false,
            |_, _| {},
        )
        .unwrap();
        assert_eq!(volume.metadata().dimensions, [2, 2, 2]);
        assert_eq!(volume.metadata().planes.len(), 3);
        assert!((volume.sample_voxel(0.5, 0.5, 0.5).unwrap() - 65.0).abs() < 1e-6);
        let plane = &volume.planes[0];
        let first = volume.resampled_slice(Plane::Axial, 0, plane);
        let second = volume.resampled_slice(Plane::Axial, 0, plane);
        assert!(Arc::ptr_eq(&first, &second), "再次访问同一切面应命中缓存");
    }

    #[test]
    fn rejects_duplicate_and_irregular_positions() {
        let duplicate = Volume::build(
            0,
            vec![source(0.0, [0; 4]), source(0.0, [0; 4])],
            || false,
            |_, _| {},
        )
        .err()
        .unwrap();
        assert!(duplicate.contains("重复"));

        let irregular = Volume::build(
            0,
            vec![
                source(0.0, [0; 4]),
                source(1.0, [0; 4]),
                source(3.0, [0; 4]),
            ],
            || false,
            |_, _| {},
        )
        .err()
        .unwrap();
        assert!(irregular.contains("间距不均匀"));
    }
}
#[test]
fn roi_statistics_apply_modality_rescale_and_area() {
    let samples = [0.0, 100.0, 200.0, 300.0];
    let stats = statistics_for_region(
        &samples,
        2,
        2,
        RoiShape::Rectangle,
        [0.0, 0.0],
        [2.0, 2.0],
        1.0,
        -100.0,
        Some("HU"),
        Some(0.25),
    )
    .unwrap();
    assert_eq!(stats.count, 4);
    assert!((stats.mean - 50.0).abs() < 1e-9);
    assert!((stats.minimum + 100.0).abs() < 1e-9);
    assert!((stats.maximum - 200.0).abs() < 1e-9);
    assert_eq!(stats.area, Some(1.0));
    assert_eq!(stats.area_unit, Some("mm2"));
    assert_eq!(stats.unit, Some("HU"));
}

#[test]
fn point_probe_ignores_invalid_mpr_samples() {
    let samples = [f32::NAN, 42.0];
    let error = statistics_for_region(
        &samples,
        2,
        1,
        RoiShape::Point,
        [0.0, 0.0],
        [0.0, 0.0],
        1.0,
        0.0,
        None,
        None,
    )
    .unwrap_err();
    assert!(error.contains("没有有效像素"));
}
