//! Orthogonal multi-planar reconstruction for a validated DICOM stack.

use pacs_codec::{Photometric, Pipeline, VoiFunction, Window};
use pacs_core::geometry::Vec3;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

const ORIENTATION_TOLERANCE: f64 = 1e-4;
const SPACING_ABSOLUTE_TOLERANCE_MM: f64 = 0.1;
const SPACING_RELATIVE_TOLERANCE: f64 = 0.05;
const MAX_VOLUME_BYTES: usize = 768 * 1024 * 1024;
const MAX_GPU_VOLUME_BYTES: usize = 256 * 1024 * 1024;
const SLICE_CACHE_LIMIT: usize = 192 * 1024 * 1024;
const RENDERED_SLICE_CACHE_LIMIT: usize = 256 * 1024 * 1024;
const MPR_CACHE_TTL: Duration = Duration::from_secs(3 * 60);
type SliceKey = (Plane, u32);

#[derive(Clone)]
pub struct SourceSlice {
    pub frame_key: String,
    pub sop_instance_uid: Option<String>,
    pub source_frame: u32,
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

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ProjectionMode {
    Slice,
    Mip,
    Minip,
}

pub struct MprRenderOptions<'a> {
    pub window_center: f64,
    pub window_width: f64,
    pub voi_function: &'a str,
    pub projection: ProjectionMode,
    pub slab_thickness_mm: f64,
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
    pub area_unit: Option<String>,
    pub unit: Option<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct MprMetadata {
    pub stack_index: u32,
    pub dimensions: [u32; 3],
    pub source_spacing_mm: [f64; 3],
    pub source_origin: [f64; 3],
    pub source_x_axis: [f64; 3],
    pub source_y_axis: [f64; 3],
    pub source_normal: [f64; 3],
    pub source_slices: Vec<MprSourceSlice>,
    pub patient_bounds_min: [f64; 3],
    pub patient_bounds_max: [f64; 3],
    pub initial_crosshair: [f64; 3],
    pub planes: Vec<PlaneMetadata>,
    pub volume_rendering: VolumeRenderingMetadata,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct VolumeRenderingMetadata {
    pub dimensions: [u32; 3],
    pub spacing_mm: [f64; 3],
    pub value_range: [f64; 2],
    pub byte_length: usize,
    pub available: bool,
    pub unavailable_reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct MprSourceSlice {
    pub frame_key: String,
    pub sop_instance_uid: Option<String>,
    pub frame_number: u32,
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
    physical_min: f64,
    physical_max: f64,
    bounds_min: Vec3,
    bounds_max: Vec3,
    planes: [PlaneMetadata; 3],
    source_slices: Vec<MprSourceSlice>,
    slice_cache: Mutex<TimedLruCache<SliceKey, f32>>,
    rendered_slice_cache: Mutex<TimedLruCache<RenderedSliceKey, u8>>,
}

struct TimedLruCache<K, T> {
    data: HashMap<K, TimedCacheEntry<T>>,
    access_queue: VecDeque<K>,
    total_bytes: usize,
    limit: usize,
    ttl: Duration,
}

struct TimedCacheEntry<T> {
    value: Arc<[T]>,
    last_access: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct RenderedSliceKey {
    plane: Plane,
    slice_index: u32,
    window_center: u64,
    window_width: u64,
    voi_function: u8,
    projection: ProjectionMode,
    slab_thickness_mm: u64,
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
        let (physical_min, physical_max) = values.iter().fold(
            (f64::INFINITY, f64::NEG_INFINITY),
            |(minimum, maximum), stored| {
                let physical = pipeline.modality_lut.apply(f64::from(*stored));
                (minimum.min(physical), maximum.max(physical))
            },
        );
        let output_spacing = row_spacing.min(col_spacing).min(slice_spacing);
        let planes = build_planes(bounds_min, bounds_max, output_spacing);
        let source_slices = sources
            .iter()
            .map(|source| MprSourceSlice {
                frame_key: source.frame_key.clone(),
                sop_instance_uid: source.sop_instance_uid.clone(),
                frame_number: source.source_frame,
            })
            .collect();

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
            physical_min,
            physical_max,
            bounds_min,
            bounds_max,
            planes,
            source_slices,
            slice_cache: Mutex::new(TimedLruCache::new(SLICE_CACHE_LIMIT, MPR_CACHE_TTL)),
            rendered_slice_cache: Mutex::new(TimedLruCache::new(
                RENDERED_SLICE_CACHE_LIMIT,
                MPR_CACHE_TTL,
            )),
        })
    }

    pub fn metadata(&self) -> MprMetadata {
        MprMetadata {
            stack_index: self.stack_index,
            dimensions: [self.cols as u32, self.rows as u32, self.slices as u32],
            source_spacing_mm: [self.col_spacing, self.row_spacing, self.slice_spacing],
            source_origin: array(self.origin),
            source_x_axis: array(self.row_direction),
            source_y_axis: array(self.column_direction),
            source_normal: array(self.normal),
            source_slices: self.source_slices.clone(),
            patient_bounds_min: array(self.bounds_min),
            patient_bounds_max: array(self.bounds_max),
            initial_crosshair: [
                (self.bounds_min.x + self.bounds_max.x) / 2.0,
                (self.bounds_min.y + self.bounds_max.y) / 2.0,
                (self.bounds_min.z + self.bounds_max.z) / 2.0,
            ],
            planes: self.planes.to_vec(),
            volume_rendering: self.volume_rendering_metadata(),
        }
    }

    pub fn volume_texture_bytes(&self) -> Result<Vec<u8>, String> {
        let metadata = self.volume_rendering_metadata();
        if !metadata.available {
            return Err(metadata
                .unavailable_reason
                .unwrap_or_else(|| "当前体数据不能用于 GPU 体渲染".to_owned()));
        }
        let width = self.physical_max - self.physical_min;
        let mut bytes = Vec::with_capacity(metadata.byte_length);
        for stored in &self.values {
            let physical = self.pipeline.modality_lut.apply(f64::from(*stored));
            let normalized = if width > f64::EPSILON {
                ((physical - self.physical_min) / width).clamp(0.0, 1.0)
            } else {
                0.0
            };
            bytes.extend_from_slice(&((normalized * 65_535.0).round() as u16).to_le_bytes());
        }
        Ok(bytes)
    }

    fn volume_rendering_metadata(&self) -> VolumeRenderingMetadata {
        let byte_length = self.values.len().saturating_mul(std::mem::size_of::<u16>());
        let reason = if byte_length > MAX_GPU_VOLUME_BYTES {
            Some(format!(
                "体纹理需要约 {:.0} MB，超过 256 MB GPU 上传上限",
                byte_length as f64 / (1024.0 * 1024.0)
            ))
        } else if !self.physical_min.is_finite() || !self.physical_max.is_finite() {
            Some("体数据没有有限的物理值范围".to_owned())
        } else {
            None
        };
        VolumeRenderingMetadata {
            dimensions: [self.cols as u32, self.rows as u32, self.slices as u32],
            spacing_mm: [self.col_spacing, self.row_spacing, self.slice_spacing],
            value_range: [self.physical_min, self.physical_max],
            byte_length,
            available: reason.is_none(),
            unavailable_reason: reason,
        }
    }

    pub fn render_slice(
        &self,
        plane: Plane,
        slice_index: u32,
        options: &MprRenderOptions<'_>,
    ) -> Result<Vec<u8>, String> {
        self.rendered_slice(plane, slice_index, options)
            .map(|bytes| bytes.as_ref().to_vec())
    }

    pub fn prefetch_rendered_slices(
        &self,
        start_slices: [u32; 3],
        options: &MprRenderOptions<'_>,
        mut cancelled: impl FnMut() -> bool,
        mut progress: impl FnMut(usize, usize),
    ) -> Result<usize, String> {
        let mut queues: [VecDeque<u32>; 3] = std::array::from_fn(|index| {
            centered_slice_order(self.planes[index].slice_count, start_slices[index])
        });
        let total = queues.iter().map(VecDeque::len).sum();
        let mut completed = 0;
        loop {
            let mut found = false;
            for (index, metadata) in self.planes.iter().enumerate() {
                let Some(slice_index) = queues[index].pop_front() else {
                    continue;
                };
                found = true;
                if cancelled() {
                    return Ok(completed);
                }
                self.rendered_slice(metadata.plane, slice_index, options)?;
                completed += 1;
                progress(completed, total);
            }
            if !found {
                return Ok(completed);
            }
        }
    }

    pub(crate) fn purge_expired_cache(&self, now: Instant) {
        self.slice_cache().remove_expired(now);
        self.rendered_slice_cache().remove_expired(now);
    }

    fn rendered_slice(
        &self,
        plane: Plane,
        slice_index: u32,
        options: &MprRenderOptions<'_>,
    ) -> Result<Arc<[u8]>, String> {
        if !options.window_center.is_finite()
            || !options.window_width.is_finite()
            || options.window_width <= 0.0
        {
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
        if !options.slab_thickness_mm.is_finite()
            || options.slab_thickness_mm <= 0.0
            || options.slab_thickness_mm > 200.0
        {
            return Err("Slab 厚度必须在 0 到 200 mm 之间".to_owned());
        }
        let (function, function_key) =
            match options.voi_function.trim().to_ascii_uppercase().as_str() {
                "LINEAR" => (VoiFunction::Linear, 0),
                "LINEAR_EXACT" => (VoiFunction::LinearExact, 1),
                "SIGMOID" => (VoiFunction::Sigmoid, 2),
                other => return Err(format!("未知 VOI 函数 {other}")),
            };
        let key = RenderedSliceKey {
            plane,
            slice_index,
            window_center: normalized_float_bits(options.window_center),
            window_width: normalized_float_bits(options.window_width),
            voi_function: function_key,
            projection: options.projection,
            slab_thickness_mm: if matches!(options.projection, ProjectionMode::Slice) {
                0
            } else {
                normalized_float_bits(options.slab_thickness_mm)
            },
        };
        {
            let mut cache = self.rendered_slice_cache();
            if let Some(cached) = cache.get(&key) {
                return Ok(Arc::clone(cached));
            }
        }
        let window = Window {
            center: options.window_center,
            width: options.window_width,
            explanation: Some("MPR".to_owned()),
            function,
        };
        let samples: Arc<[f32]> = match options.projection {
            ProjectionMode::Slice => self.resampled_slice(plane, slice_index, metadata),
            ProjectionMode::Mip | ProjectionMode::Minip => self
                .resample_slab(
                    slice_index,
                    metadata,
                    options.projection,
                    options.slab_thickness_mm,
                )
                .into(),
        };
        let rendered: Arc<[u8]> = samples
            .iter()
            .map(|value| {
                if value.is_finite() {
                    self.pipeline.apply(f64::from(*value), Some(&window))
                } else {
                    0
                }
            })
            .collect::<Vec<_>>()
            .into();
        let mut cache = self.rendered_slice_cache();
        if let Some(cached) = cache.get(&key) {
            return Ok(Arc::clone(cached));
        }
        cache.insert(key, Arc::clone(&rendered));
        Ok(rendered)
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

    fn resample_slab(
        &self,
        slice_index: u32,
        metadata: &PlaneMetadata,
        projection: ProjectionMode,
        slab_thickness_mm: f64,
    ) -> Vec<f32> {
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
        let normal_step =
            self.patient_vector_to_voxel(vec3(&metadata.normal).expect("平面法向有效"));
        let offsets = slab_offsets(slab_thickness_mm, metadata.slice_spacing_mm);
        let mut output = vec![f32::NAN; metadata.rows as usize * metadata.cols as usize];
        let mut row_origin = origin;
        for row in 0..metadata.rows as usize {
            let mut voxel = row_origin;
            for col in 0..metadata.cols as usize {
                let mut selected: Option<(f64, f64)> = None;
                for offset in &offsets {
                    let sample_point = add_voxel(voxel, scale_voxel(normal_step, *offset));
                    let Some(stored) =
                        self.sample_voxel(sample_point.x, sample_point.y, sample_point.z)
                    else {
                        continue;
                    };
                    let physical = self.pipeline.modality_lut.apply(stored);
                    let replace = selected.is_none_or(|(_, selected_physical)| match projection {
                        ProjectionMode::Mip => physical > selected_physical,
                        ProjectionMode::Minip => physical < selected_physical,
                        ProjectionMode::Slice => false,
                    });
                    if replace {
                        selected = Some((stored, physical));
                    }
                }
                if let Some((stored, _)) = selected {
                    output[row * metadata.cols as usize + col] = stored as f32;
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

    fn slice_cache(&self) -> std::sync::MutexGuard<'_, TimedLruCache<SliceKey, f32>> {
        self.slice_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn rendered_slice_cache(
        &self,
    ) -> std::sync::MutexGuard<'_, TimedLruCache<RenderedSliceKey, u8>> {
        self.rendered_slice_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn sample_voxel(&self, x: f64, y: f64, z: f64) -> Option<f64> {
        // MPR bounds are voxel edges, so the valid sample domain extends half
        // a voxel beyond the first/last center. Clamp there to the edge voxel
        // instead of producing a black half-voxel border.
        let minimum = -0.5;
        let maximum_x = self.cols as f64 - 0.5;
        let maximum_y = self.rows as f64 - 0.5;
        let maximum_z = self.slices as f64 - 0.5;
        let epsilon = 1e-6;
        if x < minimum - epsilon
            || y < minimum - epsilon
            || z < minimum - epsilon
            || x > maximum_x + epsilon
            || y > maximum_y + epsilon
            || z > maximum_z + epsilon
        {
            return None;
        }
        let x = x.clamp(0.0, (self.cols - 1) as f64);
        let y = y.clamp(0.0, (self.rows - 1) as f64);
        let z = z.clamp(0.0, (self.slices - 1) as f64);
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
    unit: Option<&str>,
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
        area_unit: area.map(|_| {
            if pixel_area.is_some() {
                "mm2".to_owned()
            } else {
                "px2".to_owned()
            }
        }),
        unit: unit.map(str::to_owned),
    })
}

#[derive(Clone, Copy)]
struct VoxelPoint {
    x: f64,
    y: f64,
    z: f64,
}

impl<K, T> TimedLruCache<K, T>
where
    K: Copy + Eq + Hash,
{
    fn new(limit: usize, ttl: Duration) -> Self {
        Self {
            data: HashMap::new(),
            access_queue: VecDeque::new(),
            total_bytes: 0,
            limit,
            ttl,
        }
    }

    fn get(&mut self, key: &K) -> Option<&Arc<[T]>> {
        self.get_at(key, Instant::now())
    }

    fn get_at(&mut self, key: &K, now: Instant) -> Option<&Arc<[T]>> {
        if self
            .data
            .get(key)
            .is_some_and(|entry| now.saturating_duration_since(entry.last_access) >= self.ttl)
        {
            self.remove(key);
            return None;
        }
        let entry = self.data.get_mut(key)?;
        entry.last_access = now;
        self.access_queue.retain(|candidate| candidate != key);
        self.access_queue.push_back(*key);
        Some(&entry.value)
    }

    fn insert(&mut self, key: K, value: Arc<[T]>) {
        self.insert_at(key, value, Instant::now());
    }

    fn insert_at(&mut self, key: K, value: Arc<[T]>, now: Instant) {
        let size = value.len() * std::mem::size_of::<T>();
        if let Some(previous) = self.data.insert(
            key,
            TimedCacheEntry {
                value,
                last_access: now,
            },
        ) {
            self.total_bytes -= previous.value.len() * std::mem::size_of::<T>();
            self.access_queue.retain(|candidate| candidate != &key);
        }
        self.total_bytes += size;
        self.access_queue.push_back(key);
        while self.total_bytes > self.limit && self.data.len() > 1 {
            let Some(oldest) = self.access_queue.pop_front() else {
                break;
            };
            if let Some(removed) = self.data.remove(&oldest) {
                self.total_bytes -= removed.value.len() * std::mem::size_of::<T>();
            }
        }
    }

    fn remove(&mut self, key: &K) {
        if let Some(removed) = self.data.remove(key) {
            self.total_bytes -= removed.value.len() * std::mem::size_of::<T>();
        }
        self.access_queue.retain(|candidate| candidate != key);
    }

    fn remove_expired(&mut self, now: Instant) {
        let ttl = self.ttl;
        let mut removed_bytes = 0;
        self.data.retain(|_, entry| {
            let keep = now.saturating_duration_since(entry.last_access) < ttl;
            if !keep {
                removed_bytes += entry.value.len() * std::mem::size_of::<T>();
            }
            keep
        });
        self.total_bytes -= removed_bytes;
        self.access_queue.retain(|key| self.data.contains_key(key));
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
    let grid = |minimum: f64, maximum: f64| {
        let length = (maximum - minimum).max(0.0);
        let count = (length / spacing).ceil().max(1.0) as u32;
        let sampled_span = f64::from(count.saturating_sub(1)) * spacing;
        let margin = ((length - sampled_span) / 2.0).max(0.0);
        (count, minimum + margin, maximum - margin)
    };
    let (x_count, x_min, x_max) = grid(min.x, max.x);
    let (y_count, y_min, _y_max) = grid(min.y, max.y);
    let (z_count, z_min, z_max) = grid(min.z, max.z);
    [
        PlaneMetadata {
            plane: Plane::Axial,
            rows: y_count,
            cols: x_count,
            slice_count: z_count,
            pixel_spacing_mm: spacing,
            slice_spacing_mm: spacing,
            origin: [x_max, y_min, z_min],
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
            origin: [x_max, y_min, z_max],
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
            origin: [x_min, y_min, z_max],
            x_axis: [0.0, 1.0, 0.0],
            y_axis: [0.0, 0.0, -1.0],
            normal: [1.0, 0.0, 0.0],
        },
    ]
}

fn centered_slice_order(slice_count: u32, requested_start: u32) -> VecDeque<u32> {
    let mut order = VecDeque::with_capacity(slice_count as usize);
    if slice_count == 0 {
        return order;
    }
    let start = requested_start.min(slice_count - 1);
    order.push_back(start);
    for distance in 1..slice_count {
        if let Some(previous) = start.checked_sub(distance) {
            order.push_back(previous);
        }
        let next = start + distance;
        if next < slice_count {
            order.push_back(next);
        }
    }
    order
}

fn normalized_float_bits(value: f64) -> u64 {
    if value == 0.0 { 0.0 } else { value }.to_bits()
}

fn volume_bounds(geometry: &SourceGeometry) -> (Vec3, Vec3) {
    let mut minimum = Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut maximum = Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    // DICOM positions locate voxel centers. Include half a voxel outside the
    // first and last centers so sparse stacks retain their full physical depth.
    for z in [
        -0.5 * geometry.slice_spacing,
        (geometry.slices as f64 - 0.5) * geometry.slice_spacing,
    ] {
        for y in [
            -0.5 * geometry.row_spacing,
            (geometry.rows as f64 - 0.5) * geometry.row_spacing,
        ] {
            for x in [
                -0.5 * geometry.col_spacing,
                (geometry.cols as f64 - 0.5) * geometry.col_spacing,
            ] {
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

fn scale_voxel(value: VoxelPoint, scalar: f64) -> VoxelPoint {
    VoxelPoint {
        x: value.x * scalar,
        y: value.y * scalar,
        z: value.z * scalar,
    }
}

fn slab_offsets(thickness_mm: f64, sample_spacing_mm: f64) -> Vec<f64> {
    if thickness_mm <= sample_spacing_mm {
        return vec![0.0];
    }
    let radius = thickness_mm / 2.0;
    let steps = (radius / sample_spacing_mm).ceil() as usize;
    let mut offsets = Vec::with_capacity(steps * 2 + 1);
    for index in 0..=steps * 2 {
        offsets.push(-radius + index as f64 * thickness_mm / (steps * 2) as f64);
    }
    offsets
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
            frame_key: format!("frame-{z}"),
            sop_instance_uid: Some(format!("1.2.3.{}", z as u32 + 1)),
            source_frame: 1,
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
    fn sparse_slices_keep_their_full_physical_depth() {
        let volume = Volume::build(
            0,
            vec![source(0.0, [0, 10, 20, 30]), source(5.0, [100, 110, 120, 130])],
            || false,
            |_, _| {},
        )
        .unwrap();
        let metadata = volume.metadata();
        assert_eq!(metadata.patient_bounds_min[2], -2.5);
        assert_eq!(metadata.patient_bounds_max[2], 7.5);
        let coronal = metadata
            .planes
            .iter()
            .find(|plane| plane.plane == Plane::Coronal)
            .unwrap();
        assert_eq!(coronal.rows, 10, "两张 5 mm 间隔的切片应覆盖完整的 10 mm 体厚");
        assert!(
            volume
                .resampled_slice(Plane::Coronal, 0, coronal)
                .iter()
                .all(|value| value.is_finite()),
            "体素边界内的重采样不应产生黑色边框"
        );
    }

    #[test]
    fn precomputes_every_plane_and_random_reads_hit_the_rendered_cache() {
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
        let options = MprRenderOptions {
            window_center: 50.0,
            window_width: 100.0,
            voi_function: "LINEAR",
            projection: ProjectionMode::Slice,
            slab_thickness_mm: 10.0,
        };
        let expected: usize = volume
            .planes
            .iter()
            .map(|plane| plane.slice_count as usize)
            .sum();
        let completed = volume
            .prefetch_rendered_slices([1, 1, 1], &options, || false, |_, _| {})
            .unwrap();

        assert_eq!(completed, expected);
        assert_eq!(volume.rendered_slice_cache().data.len(), expected);
        let first = volume.rendered_slice(Plane::Sagittal, 0, &options).unwrap();
        let second = volume.rendered_slice(Plane::Sagittal, 0, &options).unwrap();
        assert!(Arc::ptr_eq(&first, &second), "随机读取应直接命中渲染帧缓存");
        volume.purge_expired_cache(Instant::now());
    }

    #[test]
    fn timed_mpr_cache_expires_after_three_idle_minutes() {
        let now = Instant::now();
        let mut cache = TimedLruCache::<u32, u8>::new(8, MPR_CACHE_TTL);
        cache.insert_at(1, Arc::from([1, 2, 3, 4]), now);

        assert!(cache.get_at(&1, now + Duration::from_secs(179)).is_some());
        cache.remove_expired(now + Duration::from_secs(180));
        assert_eq!(cache.total_bytes, 4, "访问应刷新 MPR 缓存期限");

        cache.remove_expired(now + Duration::from_secs(359));
        assert_eq!(cache.total_bytes, 0);
        assert!(cache.data.is_empty());
    }

    #[test]
    fn centered_slice_order_prioritizes_nearby_random_reads() {
        assert_eq!(
            centered_slice_order(6, 3).into_iter().collect::<Vec<_>>(),
            vec![3, 2, 4, 1, 5, 0]
        );
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

    #[test]
    fn slab_mip_and_minip_select_extreme_physical_values() {
        let volume = Volume::build(
            0,
            vec![
                source(0.0, [10; 4]),
                source(1.0, [100; 4]),
                source(2.0, [50; 4]),
            ],
            || false,
            |_, _| {},
        )
        .unwrap();
        let axial = &volume.planes[0];
        let mip = volume.resample_slab(1, axial, ProjectionMode::Mip, 2.0);
        let minip = volume.resample_slab(1, axial, ProjectionMode::Minip, 2.0);
        assert!(mip.iter().all(|value| (*value - 100.0).abs() < 1e-6));
        assert!(minip.iter().all(|value| (*value - 10.0).abs() < 1e-6));
    }

    #[test]
    fn volume_texture_is_normalized_to_unsigned_sixteen_bit() {
        let volume = Volume::build(
            0,
            vec![source(0.0, [0, 50, 100, 25]), source(1.0, [100, 75, 0, 50])],
            || false,
            |_, _| {},
        )
        .unwrap();
        let metadata = volume.metadata().volume_rendering;
        assert!(metadata.available);
        assert_eq!(metadata.value_range, [0.0, 100.0]);
        let bytes = volume.volume_texture_bytes().unwrap();
        assert_eq!(bytes.len(), 2 * 2 * 2 * 2);
        let values = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        assert_eq!(values[0], 0);
        assert!((i32::from(values[1]) - 32_768).abs() <= 1);
        assert_eq!(values[2], u16::MAX);
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
    assert_eq!(stats.area_unit.as_deref(), Some("mm2"));
    assert_eq!(stats.unit.as_deref(), Some("HU"));
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
