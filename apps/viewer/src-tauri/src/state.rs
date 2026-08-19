//! Local DICOM series state for the desktop viewer.

use crate::mpr::{
    MprMetadata, MprRenderOptions, PixelStatistics, Plane, RoiShape, SourceSlice, Volume,
    decode_stored_values, statistics_for_region,
};
use chrono::{Duration, NaiveDate, NaiveDateTime, NaiveTime};
use dicom::core::Tag;
use dicom::dictionary_std::{tags, uids};
use dicom::object::{DefaultDicomObject, open_file};
use pacs_ai::{SeriesInput as AiSeriesInput, SliceInput as AiSliceInput};
use pacs_codec::{Frames, GrayLut, Photometric, Pipeline, VoiFunction};
use pacs_core::geometry::{SliceInput, Vec3, group_slices_by_orientation, sort_slices};
use pacs_core::spacing::{Confidence, PixelSpacing, Source, resolve};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration as StdDuration, Instant};
use thiserror::Error;

pub type SeriesHandle = u64;
type FrameKey = (SeriesHandle, u32, u32);

const FRAME_CACHE_LIMIT: usize = 512 * 1024 * 1024;
const FRAME_CACHE_TTL: StdDuration = StdDuration::from_secs(3 * 60);
#[cfg(not(test))]
const FRAME_CACHE_SWEEP_INTERVAL: StdDuration = StdDuration::from_secs(5);
const PREFETCH_RADIUS: u32 = 2;

#[derive(Clone)]
pub struct ViewerState {
    inner: Arc<Mutex<ViewerStateInner>>,
    mpr_cancelled: Arc<AtomicBool>,
    mpr_prefetch_generation: Arc<AtomicUsize>,
}

struct ViewerStateInner {
    next_handle: SeriesHandle,
    series: HashMap<SeriesHandle, LoadedSeries>,
    frame_cache: FrameCache,
}

struct LoadedSeries {
    identity: SeriesIdentity,
    image_stacks: Vec<LoadedImageStack>,
    mpr: Option<Arc<Volume>>,
    /// 远程序列的下载目录。句柄关闭时随 `LoadedSeries` 一起删除。
    _temporary_directory: Option<tempfile::TempDir>,
}

struct SeriesIdentity {
    patient: PatientStudyInfo,
    study_uid: Option<String>,
    series_uid: Option<String>,
}

struct LoadedImageStack {
    summary: ImageStackMetadata,
    frames: Vec<LoadedFrame>,
    frame_metadata: Vec<FrameMetadata>,
    warnings: Vec<String>,
}

struct PreparedImageStack {
    files: Vec<ParsedFile>,
    normal: Option<Vec3>,
    warnings: Vec<String>,
}

struct ImageStackPlan {
    order: Vec<usize>,
    first_source_index: usize,
    normal: Vec3,
    warnings: Vec<String>,
}

#[derive(Clone)]
struct LoadedFrame {
    frame_key: String,
    sop_instance_uid: Option<String>,
    path: PathBuf,
    source_frame: u32,
    pipeline: Pipeline,
    rows: u32,
    cols: u32,
    bits_allocated: u16,
    pixel_format: PixelFormat,
    quantitative: QuantitativeInfo,
    position: Option<[f64; 3]>,
    orientation: Option<[f64; 6]>,
    row_spacing_mm: Option<f64>,
    col_spacing_mm: Option<f64>,
}

struct ParsedFile {
    path: PathBuf,
    pipeline: Pipeline,
    spacing: PixelSpacing,
    rows: u32,
    cols: u32,
    bits_allocated: u16,
    pixel_format: PixelFormat,
    photometric_interpretation: String,
    frame_count: u32,
    cine_rate_fps: Option<f64>,
    quantitative: QuantitativeInfo,
    laterality: Option<String>,
    view_position: Option<String>,
    patient_orientation: Vec<String>,
    patient_name: Option<String>,
    patient_id: Option<String>,
    patient_sex: Option<String>,
    patient_birth_date: Option<String>,
    study_date: Option<String>,
    accession_number: Option<String>,
    modality: Option<String>,
    study_description: Option<String>,
    series_description: Option<String>,
    study_uid: Option<String>,
    series_uid: Option<String>,
    sop_uid: Option<String>,
    instance_number: Option<i32>,
    position: Option<Vec<f64>>,
    orientation: Option<Vec<f64>>,
}

struct FrameCache {
    data: HashMap<FrameKey, CachedFrame>,
    access_queue: VecDeque<FrameKey>,
    total_bytes: usize,
    limit: usize,
    ttl: StdDuration,
}

struct CachedFrame {
    bytes: Vec<u8>,
    last_access: Instant,
}

#[derive(Error, Debug)]
pub enum ViewerError {
    #[error("没有选择 DICOM 文件")]
    EmptySelection,
    #[error("未知的序列句柄: {0}")]
    UnknownHandle(SeriesHandle),
    #[error("未知的图像组: {stack_index}")]
    UnknownImageStack { stack_index: u32 },
    #[error("I/O 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("DICOM 解析错误: {0}")]
    Dicom(String),
    #[error("帧号越界: {frame} >= {total}")]
    FrameOutOfBounds { frame: u32, total: u32 },
    #[error("不支持的影像: {0}")]
    Unsupported(String),
    #[error("所选文件不能组成一个序列: {0}")]
    InvalidSeries(String),
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct SeriesMetadata {
    pub handle: SeriesHandle,
    pub patient: PatientStudyInfo,
    pub study_uid: Option<String>,
    pub series_uid: Option<String>,
    pub active_stack: u32,
    pub image_stacks: Vec<ImageStackMetadata>,
    pub frames: Vec<FrameMetadata>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct ImageStackMetadata {
    pub index: u32,
    pub label: String,
    pub frame_count: u32,
    pub rows: u32,
    pub cols: u32,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct PatientStudyInfo {
    pub patient_name: Option<String>,
    pub patient_id: Option<String>,
    pub patient_sex: Option<String>,
    pub patient_birth_date: Option<String>,
    pub study_date: Option<String>,
    pub accession_number: Option<String>,
    pub modality: Option<String>,
    pub study_description: Option<String>,
    pub series_description: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct FrameMetadata {
    pub logical_index: u32,
    pub frame_key: String,
    pub sop_instance_uid: Option<String>,
    pub source_frame: u32,
    pub instance_number: Option<i32>,
    pub rows: u32,
    pub cols: u32,
    pub bits_allocated: u16,
    pub pixel_format: PixelFormat,
    pub photometric_interpretation: String,
    pub cine_rate_fps: Option<f64>,
    pub quantitative: QuantitativeInfo,
    pub laterality: Option<String>,
    pub view_position: Option<String>,
    pub patient_orientation: Vec<String>,
    /// ImagePositionPatient（患者坐标系中的首个体素中心位置）。
    pub position: Option<[f64; 3]>,
    /// ImageOrientationPatient（行方向 3 分量 + 列方向 3 分量）。
    pub orientation: Option<[f64; 6]>,
    pub window_presets: Vec<WindowPreset>,
    pub spacing: SpacingInfo,
}

#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PixelFormat {
    Gray8,
    Gray16,
    Rgb8,
}

impl PixelFormat {
    fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Gray8 => 1,
            Self::Gray16 => 2,
            Self::Rgb8 => 3,
        }
    }

    fn is_grayscale(self) -> bool {
        !matches!(self, Self::Rgb8)
    }
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct QuantitativeInfo {
    pub unit: Option<String>,
    pub suvbw_factor: Option<f64>,
    pub suvbw_status: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct WindowPreset {
    pub center: f64,
    pub width: f64,
    pub explanation: Option<String>,
    pub function: String,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct SpacingInfo {
    pub confidence: String,
    pub source: Option<String>,
    pub description: String,
    pub row_mm: Option<f64>,
    pub col_mm: Option<f64>,
    /// Horizontal pixel size divided by vertical pixel size.
    pub column_over_row: f64,
}

impl ViewerState {
    pub fn new() -> Self {
        let inner = Arc::new(Mutex::new(ViewerStateInner {
            next_handle: 1,
            series: HashMap::new(),
            frame_cache: FrameCache::new(FRAME_CACHE_LIMIT, FRAME_CACHE_TTL),
        }));
        start_cache_cleanup(&inner);
        Self {
            inner,
            mpr_cancelled: Arc::new(AtomicBool::new(false)),
            mpr_prefetch_generation: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ViewerStateInner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn open_series(&self, paths: Vec<PathBuf>) -> Result<SeriesMetadata, ViewerError> {
        self.open_series_with_owner(paths, None)
    }

    pub fn open_temporary_series(
        &self,
        paths: Vec<PathBuf>,
        directory: tempfile::TempDir,
    ) -> Result<SeriesMetadata, ViewerError> {
        self.open_series_with_owner(paths, Some(directory))
    }

    fn open_series_with_owner(
        &self,
        paths: Vec<PathBuf>,
        temporary_directory: Option<tempfile::TempDir>,
    ) -> Result<SeriesMetadata, ViewerError> {
        if paths.is_empty() {
            return Err(ViewerError::EmptySelection);
        }

        let parsed = paths
            .into_iter()
            .map(parse_file)
            .collect::<Result<Vec<_>, _>>()?;
        let first = parsed.first().expect("已检查输入非空");
        let identity = SeriesIdentity {
            patient: PatientStudyInfo {
                patient_name: first.patient_name.clone(),
                patient_id: first.patient_id.clone(),
                patient_sex: first.patient_sex.clone(),
                patient_birth_date: first.patient_birth_date.clone(),
                study_date: first.study_date.clone(),
                accession_number: first.accession_number.clone(),
                modality: first.modality.clone(),
                study_description: first.study_description.clone(),
                series_description: first.series_description.clone(),
            },
            study_uid: first.study_uid.clone(),
            series_uid: first.series_uid.clone(),
        };
        let prepared_stacks = prepare_image_stacks(parsed)?;
        let stack_count = prepared_stacks.len();
        let image_stacks = prepared_stacks
            .into_iter()
            .enumerate()
            .map(|(index, stack)| build_loaded_image_stack(stack, index, stack_count))
            .collect::<Result<Vec<_>, _>>()?;

        let mut inner = self.lock();
        let handle = inner.next_handle;
        inner.next_handle = inner
            .next_handle
            .checked_add(1)
            .ok_or_else(|| ViewerError::Unsupported("打开的序列句柄已经耗尽".to_owned()))?;
        let loaded = LoadedSeries {
            identity,
            image_stacks,
            mpr: None,
            _temporary_directory: temporary_directory,
        };
        let metadata = loaded.metadata(handle, 0)?;
        inner.series.insert(handle, loaded);

        Ok(metadata)
    }

    pub fn select_image_stack(
        &self,
        handle: SeriesHandle,
        stack_index: u32,
    ) -> Result<SeriesMetadata, ViewerError> {
        let inner = self.lock();
        inner
            .series
            .get(&handle)
            .ok_or(ViewerError::UnknownHandle(handle))?
            .metadata(handle, stack_index)
    }

    pub fn ai_series_input(
        &self,
        handle: SeriesHandle,
        stack_index: u32,
    ) -> Result<AiSeriesInput, ViewerError> {
        let inner = self.lock();
        let series = inner
            .series
            .get(&handle)
            .ok_or(ViewerError::UnknownHandle(handle))?;
        let stack = series
            .image_stacks
            .get(stack_index as usize)
            .ok_or(ViewerError::UnknownImageStack { stack_index })?;
        let modality = series.identity.patient.modality.clone();
        if !modality
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case("CT"))
        {
            return Err(ViewerError::Unsupported(
                "当前 AI 模型仅支持 CT 序列".to_owned(),
            ));
        }
        if stack.frames.len() < 2 {
            return Err(ViewerError::InvalidSeries(
                "AI 分割至少需要两张 CT 切片".to_owned(),
            ));
        }
        let first = &stack.frames[0];
        let mut paths = std::collections::HashSet::new();
        let mut slices = Vec::with_capacity(stack.frames.len());
        for (index, frame) in stack.frames.iter().enumerate() {
            if !frame.pixel_format.is_grayscale() {
                return Err(ViewerError::Unsupported(
                    "彩色影像不能使用当前 AI 分割模型".to_owned(),
                ));
            }
            if frame.rows != first.rows || frame.cols != first.cols {
                return Err(ViewerError::InvalidSeries(
                    "AI 分割要求所有切片尺寸一致".to_owned(),
                ));
            }
            if frame.source_frame != 1 || !paths.insert(frame.path.clone()) {
                return Err(ViewerError::Unsupported(
                    "当前 AI Worker 暂不支持多帧 DICOM".to_owned(),
                ));
            }
            if frame.position.is_none()
                || frame.orientation.is_none()
                || frame.row_spacing_mm.is_none()
                || frame.col_spacing_mm.is_none()
            {
                return Err(ViewerError::InvalidSeries(format!(
                    "第 {} 张切片缺少 AI 分割所需的患者空间几何信息",
                    index + 1
                )));
            }
            slices.push(AiSliceInput {
                source_index: index as u32,
                path: frame.path.clone(),
            });
        }
        Ok(AiSeriesInput {
            modality,
            rows: first.rows,
            cols: first.cols,
            slices,
        })
    }

    pub fn close(&self, handle: SeriesHandle) -> Result<(), ViewerError> {
        self.cancel_mpr_prefetch();
        let mut inner = self.lock();
        inner
            .series
            .remove(&handle)
            .ok_or(ViewerError::UnknownHandle(handle))?;
        inner.frame_cache.remove_series(handle);
        Ok(())
    }

    pub fn get_frame_bytes(
        &self,
        handle: SeriesHandle,
        stack_index: u32,
        logical_frame: u32,
    ) -> Result<Vec<u8>, ViewerError> {
        let key = (handle, stack_index, logical_frame);
        {
            let mut inner = self.lock();
            if let Some(cached) = inner.frame_cache.get(&key) {
                return Ok(cached.clone());
            }
        }

        let (requested, neighbours) = {
            let inner = self.lock();
            let series = inner
                .series
                .get(&handle)
                .ok_or(ViewerError::UnknownHandle(handle))?;
            let image_stack = series
                .image_stacks
                .get(stack_index as usize)
                .ok_or(ViewerError::UnknownImageStack { stack_index })?;
            let requested = image_stack
                .frames
                .get(logical_frame as usize)
                .ok_or(ViewerError::FrameOutOfBounds {
                    frame: logical_frame,
                    total: image_stack.frames.len() as u32,
                })?
                .clone();
            let neighbours = image_stack
                .frames
                .iter()
                .enumerate()
                .filter(|(index, frame)| {
                    frame.path == requested.path
                        && (*index as u32).abs_diff(logical_frame) <= PREFETCH_RADIUS
                })
                .map(|(index, frame)| {
                    (
                        index as u32,
                        frame.source_frame,
                        frame.rows,
                        frame.cols,
                        frame.pixel_format,
                    )
                })
                .collect::<Vec<_>>();
            (requested, neighbours)
        };

        let mut object =
            open_file(&requested.path).map_err(|e| ViewerError::Dicom(e.to_string()))?;
        pacs_core::normalize_file_text(&mut object);
        let frames = Frames::decode(&object).map_err(|e| ViewerError::Dicom(e.to_string()))?;
        let mut decoded = Vec::with_capacity(neighbours.len());
        for (logical, source, rows, cols, pixel_format) in neighbours {
            let bytes = if pixel_format.is_grayscale() {
                frames
                    .frame(source)
                    .map_err(|e| ViewerError::Dicom(e.to_string()))?
                    .to_vec()
            } else {
                frames
                    .rgb8_frame(source)
                    .map_err(|e| ViewerError::Dicom(e.to_string()))?
            };
            let expected = rows as usize * cols as usize * pixel_format.bytes_per_pixel();
            if bytes.len() != expected {
                return Err(ViewerError::Unsupported(format!(
                    "解码帧大小为 {} 字节，预期 {expected} 字节",
                    bytes.len()
                )));
            }
            decoded.push((logical, bytes));
        }

        let mut inner = self.lock();
        if !inner.series.contains_key(&handle) {
            return Err(ViewerError::UnknownHandle(handle));
        }
        for (logical, bytes) in decoded {
            inner
                .frame_cache
                .insert((handle, stack_index, logical), bytes);
        }
        inner
            .frame_cache
            .get(&key)
            .cloned()
            .ok_or_else(|| ViewerError::Dicom("解码结果中缺少请求帧".to_owned()))
    }

    pub fn build_lut(
        &self,
        handle: SeriesHandle,
        stack_index: u32,
        logical_frame: u32,
        window_center: f64,
        window_width: f64,
        voi_function: &str,
    ) -> Result<Vec<u8>, ViewerError> {
        if !window_center.is_finite() || !window_width.is_finite() || window_width <= 0.0 {
            return Err(ViewerError::Unsupported(
                "窗位必须有限且窗宽必须大于 0".to_owned(),
            ));
        }
        let inner = self.lock();
        let series = inner
            .series
            .get(&handle)
            .ok_or(ViewerError::UnknownHandle(handle))?;
        let image_stack = series
            .image_stacks
            .get(stack_index as usize)
            .ok_or(ViewerError::UnknownImageStack { stack_index })?;
        let frame = image_stack.frames.get(logical_frame as usize).ok_or(
            ViewerError::FrameOutOfBounds {
                frame: logical_frame,
                total: image_stack.frames.len() as u32,
            },
        )?;
        if !frame.pixel_format.is_grayscale() {
            return Err(ViewerError::Unsupported(
                "彩色影像不使用灰度窗宽窗位 LUT".to_owned(),
            ));
        }
        let function = match voi_function.trim().to_ascii_uppercase().as_str() {
            "LINEAR" => VoiFunction::Linear,
            "LINEAR_EXACT" => VoiFunction::LinearExact,
            "SIGMOID" => VoiFunction::Sigmoid,
            other => {
                return Err(ViewerError::Unsupported(format!("未知 VOI 函数 {other}")));
            }
        };
        let window = pacs_codec::Window {
            center: window_center,
            width: window_width,
            explanation: Some("custom".to_owned()),
            function,
        };
        Ok(GrayLut::build(&frame.pipeline, Some(&window), Some(frame.bits_allocated)).table)
    }

    pub fn measure_frame_roi(
        &self,
        handle: SeriesHandle,
        stack_index: u32,
        logical_frame: u32,
        shape: RoiShape,
        start: [f64; 2],
        end: [f64; 2],
    ) -> Result<PixelStatistics, ViewerError> {
        let (frame, pixel_area) = {
            let inner = self.lock();
            let series = inner
                .series
                .get(&handle)
                .ok_or(ViewerError::UnknownHandle(handle))?;
            let stack = series
                .image_stacks
                .get(stack_index as usize)
                .ok_or(ViewerError::UnknownImageStack { stack_index })?;
            let frame = stack.frames.get(logical_frame as usize).cloned().ok_or(
                ViewerError::FrameOutOfBounds {
                    frame: logical_frame,
                    total: stack.frames.len() as u32,
                },
            )?;
            let pixel_area = frame
                .row_spacing_mm
                .zip(frame.col_spacing_mm)
                .map(|(row, col)| row * col);
            (frame, pixel_area)
        };
        if !frame.pixel_format.is_grayscale() {
            return Err(ViewerError::Unsupported(
                "彩色影像暂不提供 ROI 像素统计".to_owned(),
            ));
        }
        let bytes = self.get_frame_bytes(handle, stack_index, logical_frame)?;
        let mut stored = Vec::with_capacity(frame.rows as usize * frame.cols as usize);
        decode_stored_values(&bytes, frame.bits_allocated, &frame.pipeline, &mut stored);
        let (slope, intercept, unit) = frame.quantitative.suvbw_factor.map_or_else(
            || {
                (
                    frame.pipeline.modality_lut.slope,
                    frame.pipeline.modality_lut.intercept,
                    frame
                        .quantitative
                        .unit
                        .as_deref()
                        .or(frame.pipeline.modality_lut.unit),
                )
            },
            |factor| {
                (
                    frame.pipeline.modality_lut.slope * factor,
                    frame.pipeline.modality_lut.intercept * factor,
                    Some("SUVbw"),
                )
            },
        );
        statistics_for_region(
            &stored,
            frame.cols as usize,
            frame.rows as usize,
            shape,
            start,
            end,
            slope,
            intercept,
            unit,
            pixel_area,
        )
        .map_err(ViewerError::Unsupported)
    }

    pub fn measure_mpr_roi(
        &self,
        handle: SeriesHandle,
        plane: Plane,
        slice_index: u32,
        shape: RoiShape,
        start: [f64; 2],
        end: [f64; 2],
    ) -> Result<PixelStatistics, ViewerError> {
        let volume = {
            let inner = self.lock();
            Arc::clone(
                inner
                    .series
                    .get(&handle)
                    .ok_or(ViewerError::UnknownHandle(handle))?
                    .mpr
                    .as_ref()
                    .ok_or_else(|| ViewerError::Unsupported("尚未构建 MPR 体数据".to_owned()))?,
            )
        };
        volume
            .measure_roi(plane, slice_index, shape, start, end)
            .map_err(ViewerError::Unsupported)
    }

    pub fn prepare_mpr(
        &self,
        handle: SeriesHandle,
        stack_index: u32,
        progress: impl Fn(usize, usize) + Sync,
    ) -> Result<MprMetadata, ViewerError> {
        self.cancel_mpr_prefetch();
        self.mpr_cancelled.store(false, Ordering::Release);
        let frames = {
            let inner = self.lock();
            let series = inner
                .series
                .get(&handle)
                .ok_or(ViewerError::UnknownHandle(handle))?;
            let stack = series
                .image_stacks
                .get(stack_index as usize)
                .ok_or(ViewerError::UnknownImageStack { stack_index })?;
            stack.frames.clone()
        };
        if frames.len() < 2 {
            return Err(ViewerError::InvalidSeries(
                "MPR 至少需要两张属于同一空间堆栈的切片".to_owned(),
            ));
        }
        if frames
            .iter()
            .any(|frame| !frame.pixel_format.is_grayscale())
        {
            return Err(ViewerError::Unsupported(
                "彩色影像不能构建灰度 MPR 体数据".to_owned(),
            ));
        }

        let frame_count = frames.len();
        let decoded = AtomicUsize::new(0);
        let sources = frames
            .into_par_iter()
            .enumerate()
            .map(|(index, frame)| {
                if self.mpr_cancelled.load(Ordering::Acquire) {
                    return Err(ViewerError::Unsupported("已取消 MPR 构建".to_owned()));
                }
                let bytes = self.get_frame_bytes(handle, stack_index, index as u32)?;
                let completed = decoded.fetch_add(1, Ordering::AcqRel) + 1;
                if completed == frame_count || completed.is_multiple_of(5) {
                    progress(completed, frame_count * 2);
                }
                Ok(SourceSlice {
                    frame_key: frame.frame_key,
                    sop_instance_uid: frame.sop_instance_uid,
                    source_frame: frame.source_frame,
                    rows: frame.rows,
                    cols: frame.cols,
                    bits_allocated: frame.bits_allocated,
                    pipeline: frame.pipeline,
                    position: frame.position,
                    orientation: frame.orientation,
                    row_spacing_mm: frame.row_spacing_mm,
                    col_spacing_mm: frame.col_spacing_mm,
                    bytes,
                })
            })
            .collect::<Result<Vec<_>, ViewerError>>()?;
        let cancelled = Arc::clone(&self.mpr_cancelled);
        let volume = Volume::build(
            stack_index,
            sources,
            move || cancelled.load(Ordering::Acquire),
            |completed, total| progress(frame_count + completed, frame_count + total),
        )
        .map_err(ViewerError::InvalidSeries)?;
        let metadata = volume.metadata();
        let mut inner = self.lock();
        let series = inner
            .series
            .get_mut(&handle)
            .ok_or(ViewerError::UnknownHandle(handle))?;
        series.mpr = Some(Arc::new(volume));
        inner.frame_cache.remove_series(handle);
        Ok(metadata)
    }

    pub fn render_mpr_slice(
        &self,
        handle: SeriesHandle,
        plane: Plane,
        slice_index: u32,
        options: &MprRenderOptions<'_>,
    ) -> Result<Vec<u8>, ViewerError> {
        let volume = {
            let inner = self.lock();
            Arc::clone(
                inner
                    .series
                    .get(&handle)
                    .ok_or(ViewerError::UnknownHandle(handle))?
                    .mpr
                    .as_ref()
                    .ok_or_else(|| ViewerError::Unsupported("尚未构建 MPR 体数据".to_owned()))?,
            )
        };
        volume
            .render_slice(plane, slice_index, options)
            .map_err(ViewerError::Unsupported)
    }

    pub fn begin_mpr_prefetch(&self) -> usize {
        self.mpr_prefetch_generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1)
    }

    pub fn prefetch_mpr_slices(
        &self,
        handle: SeriesHandle,
        start_slices: [u32; 3],
        options: &MprRenderOptions<'_>,
        generation: usize,
        progress: impl Fn(usize, usize),
    ) -> Result<usize, ViewerError> {
        let volume = {
            let inner = self.lock();
            Arc::clone(
                inner
                    .series
                    .get(&handle)
                    .ok_or(ViewerError::UnknownHandle(handle))?
                    .mpr
                    .as_ref()
                    .ok_or_else(|| ViewerError::Unsupported("尚未构建 MPR 体数据".to_owned()))?,
            )
        };
        let active_generation = Arc::clone(&self.mpr_prefetch_generation);
        volume
            .prefetch_rendered_slices(
                start_slices,
                options,
                move || active_generation.load(Ordering::Acquire) != generation,
                progress,
            )
            .map_err(ViewerError::Unsupported)
    }

    pub fn cancel_mpr_prefetch(&self) {
        self.mpr_prefetch_generation.fetch_add(1, Ordering::AcqRel);
    }

    pub fn close_mpr(&self, handle: SeriesHandle) -> Result<(), ViewerError> {
        self.cancel_mpr_prefetch();
        let mut inner = self.lock();
        let series = inner
            .series
            .get_mut(&handle)
            .ok_or(ViewerError::UnknownHandle(handle))?;
        series.mpr = None;
        Ok(())
    }

    pub fn get_volume_texture_bytes(&self, handle: SeriesHandle) -> Result<Vec<u8>, ViewerError> {
        let volume = {
            let inner = self.lock();
            Arc::clone(
                inner
                    .series
                    .get(&handle)
                    .ok_or(ViewerError::UnknownHandle(handle))?
                    .mpr
                    .as_ref()
                    .ok_or_else(|| ViewerError::Unsupported("尚未构建三维体数据".to_owned()))?,
            )
        };
        volume
            .volume_texture_bytes()
            .map_err(ViewerError::Unsupported)
    }

    pub fn cancel_mpr_build(&self) {
        self.mpr_cancelled.store(true, Ordering::Release);
    }
}

impl Default for ViewerState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(test))]
fn start_cache_cleanup(inner: &Arc<Mutex<ViewerStateInner>>) {
    let weak = Arc::downgrade(inner);
    let _cleanup_thread = std::thread::spawn(move || {
        loop {
            std::thread::sleep(FRAME_CACHE_SWEEP_INTERVAL);
            let Some(inner) = weak.upgrade() else {
                break;
            };
            let now = Instant::now();
            let mut inner = inner.lock().unwrap_or_else(PoisonError::into_inner);
            inner.frame_cache.remove_expired(now);
            for series in inner.series.values() {
                if let Some(volume) = &series.mpr {
                    volume.purge_expired_cache(now);
                }
            }
        }
    });
}

#[cfg(test)]
fn start_cache_cleanup(_inner: &Arc<Mutex<ViewerStateInner>>) {}

impl LoadedSeries {
    fn metadata(
        &self,
        handle: SeriesHandle,
        stack_index: u32,
    ) -> Result<SeriesMetadata, ViewerError> {
        let image_stack = self
            .image_stacks
            .get(stack_index as usize)
            .ok_or(ViewerError::UnknownImageStack { stack_index })?;
        Ok(SeriesMetadata {
            handle,
            patient: self.identity.patient.clone(),
            study_uid: self.identity.study_uid.clone(),
            series_uid: self.identity.series_uid.clone(),
            active_stack: stack_index,
            image_stacks: self
                .image_stacks
                .iter()
                .map(|stack| stack.summary.clone())
                .collect(),
            frames: image_stack.frame_metadata.clone(),
            warnings: image_stack.warnings.clone(),
        })
    }
}

impl FrameCache {
    fn new(limit: usize, ttl: StdDuration) -> Self {
        Self {
            data: HashMap::new(),
            access_queue: VecDeque::new(),
            total_bytes: 0,
            limit,
            ttl,
        }
    }

    fn get(&mut self, key: &FrameKey) -> Option<&Vec<u8>> {
        self.get_at(key, Instant::now())
    }

    fn get_at(&mut self, key: &FrameKey, now: Instant) -> Option<&Vec<u8>> {
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
        Some(&entry.bytes)
    }

    fn insert(&mut self, key: FrameKey, data: Vec<u8>) {
        self.insert_at(key, data, Instant::now());
    }

    fn insert_at(&mut self, key: FrameKey, data: Vec<u8>, now: Instant) {
        let size = data.len();
        if let Some(old) = self.data.insert(
            key,
            CachedFrame {
                bytes: data,
                last_access: now,
            },
        ) {
            self.total_bytes -= old.bytes.len();
            self.access_queue.retain(|candidate| candidate != &key);
        }
        self.total_bytes += size;
        self.access_queue.push_back(key);

        while self.total_bytes > self.limit && self.data.len() > 1 {
            let Some(evict_key) = self.access_queue.pop_front() else {
                break;
            };
            if let Some(evicted) = self.data.remove(&evict_key) {
                self.total_bytes -= evicted.bytes.len();
            }
        }
    }

    fn remove(&mut self, key: &FrameKey) {
        if let Some(removed) = self.data.remove(key) {
            self.total_bytes -= removed.bytes.len();
        }
        self.access_queue.retain(|candidate| candidate != key);
    }

    fn remove_expired(&mut self, now: Instant) {
        let ttl = self.ttl;
        let mut removed_bytes = 0;
        self.data.retain(|_, entry| {
            let keep = now.saturating_duration_since(entry.last_access) < ttl;
            if !keep {
                removed_bytes += entry.bytes.len();
            }
            keep
        });
        self.total_bytes -= removed_bytes;
        self.access_queue.retain(|key| self.data.contains_key(key));
    }

    fn remove_series(&mut self, handle: SeriesHandle) {
        self.data.retain(|key, value| {
            if key.0 == handle {
                self.total_bytes -= value.bytes.len();
                false
            } else {
                true
            }
        });
        self.access_queue.retain(|key| key.0 != handle);
    }
}

fn prepare_image_stacks(parsed: Vec<ParsedFile>) -> Result<Vec<PreparedImageStack>, ViewerError> {
    if parsed.len() == 1 {
        return Ok(vec![PreparedImageStack {
            files: parsed,
            normal: None,
            warnings: Vec::new(),
        }]);
    }

    validate_multi_file_series(&parsed)?;
    // Projection/localizer images are valid DICOM images but are not always
    // populated with patient-space geometry. Keep them viewable as standalone
    // stacks while retaining strict geometry checks for sortable stacks.
    let mut standalone_indices = Vec::new();
    let mut sortable_indices = Vec::new();
    for (index, file) in parsed.iter().enumerate() {
        let has_geometry = file
            .position
            .as_deref()
            .is_some_and(|values| values.len() >= 3)
            && file
                .orientation
                .as_deref()
                .is_some_and(|values| values.len() >= 6);
        if has_geometry {
            sortable_indices.push(index);
        } else {
            standalone_indices.push(index);
        }
    }

    let mut plans = {
        let slices = parsed
            .iter()
            .map(|file| SliceInput {
                position: file.position.as_deref().unwrap_or(&[]),
                orientation: file.orientation.as_deref().unwrap_or(&[]),
            })
            .collect::<Vec<_>>();
        let sortable_slices = sortable_indices
            .iter()
            .map(|&source_index| slices[source_index])
            .collect::<Vec<_>>();
        if sortable_slices.is_empty() {
            Vec::new()
        } else {
            let orientation_groups =
                group_slices_by_orientation(&sortable_slices).map_err(geometry_error)?;
            let mut dimension_groups = Vec::<Vec<usize>>::new();
            for orientation_group in orientation_groups {
                let orientation_group = orientation_group
                    .into_iter()
                    .map(|local_index| sortable_indices[local_index])
                    .collect::<Vec<_>>();
                let mut compatible_dimensions = Vec::<Vec<usize>>::new();
                for source_index in orientation_group {
                    let file = &parsed[source_index];
                    if let Some(group) = compatible_dimensions.iter_mut().find(|group| {
                        let reference = &parsed[group[0]];
                        reference.rows == file.rows
                            && reference.cols == file.cols
                            && reference.bits_allocated == file.bits_allocated
                            && reference.pixel_format == file.pixel_format
                    }) {
                        group.push(source_index);
                    } else {
                        compatible_dimensions.push(vec![source_index]);
                    }
                }
                dimension_groups.extend(compatible_dimensions);
            }

            dimension_groups
                .into_iter()
                .map(|indices| {
                    let group_slices = indices
                        .iter()
                        .map(|&source_index| slices[source_index])
                        .collect::<Vec<_>>();
                    let sorted = sort_slices(&group_slices).map_err(geometry_error)?;
                    let mut warnings = Vec::new();
                    if sorted.duplicate_position_groups > 0 {
                        warnings.push(format!(
                            "当前图像组包含 {} 组重复切片位置，请核对重建内容",
                            sorted.duplicate_position_groups
                        ));
                    }
                    if !sorted.spacing_is_regular {
                        warnings.push("当前图像组的切片间距不均匀，可能存在漏传切片".to_owned());
                    }
                    let order = sorted
                        .order
                        .into_iter()
                        .map(|local_index| indices[local_index])
                        .collect::<Vec<_>>();
                    Ok(ImageStackPlan {
                        first_source_index: *indices.first().expect("图像组不为空"),
                        order,
                        normal: sorted.normal,
                        warnings,
                    })
                })
                .collect::<Result<Vec<_>, ViewerError>>()?
        }
    };

    plans.sort_by(|left, right| {
        right
            .order
            .len()
            .cmp(&left.order.len())
            .then_with(|| left.first_source_index.cmp(&right.first_source_index))
    });
    let mut slots = parsed.into_iter().map(Some).collect::<Vec<_>>();
    let mut prepared = plans
        .into_iter()
        .map(|plan| PreparedImageStack {
            files: plan
                .order
                .into_iter()
                .map(|index| slots[index].take().expect("图像组索引必须唯一且有效"))
                .collect(),
            normal: Some(plan.normal),
            warnings: plan.warnings,
        })
        .collect::<Vec<_>>();
    for source_index in standalone_indices {
        let file = slots[source_index]
            .take()
            .expect("独立图像组索引必须唯一且有效");
        prepared.push(PreparedImageStack {
            files: vec![file],
            normal: None,
            warnings: vec![
                "该图像缺少 ImagePositionPatient/ImageOrientationPatient，仅作为单张图像显示，不能安全排序或用于 MPR"
                    .to_owned(),
            ],
        });
    }
    Ok(prepared)
}

fn geometry_error(error: pacs_core::geometry::GeometryError) -> ViewerError {
    ViewerError::InvalidSeries(format!(
        "无法按 ImagePositionPatient/ImageOrientationPatient 安全排序: {error}"
    ))
}

fn build_loaded_image_stack(
    prepared: PreparedImageStack,
    stack_index: usize,
    stack_count: usize,
) -> Result<LoadedImageStack, ViewerError> {
    let rows = prepared.files[0].rows;
    let cols = prepared.files[0].cols;
    let mut frames = Vec::new();
    let mut frame_metadata = Vec::new();
    for (file_index, file) in prepared.files.into_iter().enumerate() {
        for source_frame in 1..=file.frame_count {
            let logical_index = u32::try_from(frames.len())
                .map_err(|_| ViewerError::Unsupported("序列帧数超过支持范围".to_owned()))?;
            let frame_key = file.sop_uid.as_ref().map_or_else(
                || format!("local-{stack_index}-{file_index}#{source_frame}"),
                |uid| format!("{uid}#{source_frame}"),
            );
            frame_metadata.push(FrameMetadata {
                logical_index,
                frame_key: frame_key.clone(),
                sop_instance_uid: file.sop_uid.clone(),
                source_frame,
                instance_number: file.instance_number,
                rows: file.rows,
                cols: file.cols,
                bits_allocated: file.bits_allocated,
                pixel_format: file.pixel_format,
                photometric_interpretation: file.photometric_interpretation.clone(),
                cine_rate_fps: file.cine_rate_fps,
                quantitative: file.quantitative.clone(),
                laterality: file.laterality.clone(),
                view_position: file.view_position.clone(),
                patient_orientation: file.patient_orientation.clone(),
                position: fixed_array::<3>(file.position.as_deref()),
                orientation: fixed_array::<6>(file.orientation.as_deref()),
                window_presets: window_presets(&file.pipeline),
                spacing: spacing_info(file.spacing),
            });
            frames.push(LoadedFrame {
                frame_key,
                sop_instance_uid: file.sop_uid.clone(),
                path: file.path.clone(),
                source_frame,
                pipeline: file.pipeline.clone(),
                rows: file.rows,
                cols: file.cols,
                bits_allocated: file.bits_allocated,
                pixel_format: file.pixel_format,
                quantitative: file.quantitative.clone(),
                position: fixed_array::<3>(file.position.as_deref()),
                orientation: fixed_array::<6>(file.orientation.as_deref()),
                row_spacing_mm: physical_spacing(file.spacing).map(|spacing| spacing.0),
                col_spacing_mm: physical_spacing(file.spacing).map(|spacing| spacing.1),
            });
        }
    }

    let frame_count = u32::try_from(frames.len())
        .map_err(|_| ViewerError::Unsupported("序列帧数超过支持范围".to_owned()))?;
    let index = u32::try_from(stack_index)
        .map_err(|_| ViewerError::Unsupported("图像组数量超过支持范围".to_owned()))?;
    let label = image_stack_label(prepared.normal, frame_count);
    let mut warnings = prepared.warnings;
    if stack_count > 1 {
        warnings.insert(
            0,
            format!("该 DICOM Series 含 {stack_count} 个不同朝向或尺寸的图像组，已分开显示"),
        );
    }

    Ok(LoadedImageStack {
        summary: ImageStackMetadata {
            index,
            label,
            frame_count,
            rows,
            cols,
        },
        frames,
        frame_metadata,
        warnings,
    })
}

fn image_stack_label(normal: Option<Vec3>, frame_count: u32) -> String {
    let plane = normal.map_or("多帧影像", |normal| {
        let (x, y, z) = (normal.x.abs(), normal.y.abs(), normal.z.abs());
        if z >= x && z >= y {
            "轴位"
        } else if y >= x {
            "冠状位"
        } else {
            "矢状位"
        }
    });
    format!("{plane} · {frame_count} 帧")
}

fn parse_file(path: PathBuf) -> Result<ParsedFile, ViewerError> {
    let mut object = open_file(&path)
        .map_err(|error| ViewerError::Dicom(format!("{}: {error}", path.display())))?;
    pacs_core::normalize_file_text(&mut object);
    let pipeline = Pipeline::from_object(&object);
    let photometric_interpretation = text(&object, tags::PHOTOMETRIC_INTERPRETATION)
        .unwrap_or_default()
        .trim()
        .to_ascii_uppercase();
    let samples_per_pixel = integer_u16(&object, tags::SAMPLES_PER_PIXEL).unwrap_or(1);
    let rows = required_u32(&object, tags::ROWS, "Rows", &path)?;
    let cols = required_u32(&object, tags::COLUMNS, "Columns", &path)?;
    let bits_allocated = integer_u16(&object, tags::BITS_ALLOCATED).unwrap_or(16);
    if !matches!(bits_allocated, 8 | 16) {
        return Err(ViewerError::Unsupported(format!(
            "{} 的 BitsAllocated={bits_allocated}，当前仅支持 8 或 16 位像素",
            path.display()
        )));
    }
    let pixel_format = match photometric_interpretation.as_str() {
        "MONOCHROME1" | "MONOCHROME2" if samples_per_pixel == 1 => {
            if bits_allocated == 8 {
                PixelFormat::Gray8
            } else {
                PixelFormat::Gray16
            }
        }
        "PALETTE COLOR" if samples_per_pixel == 1 => PixelFormat::Rgb8,
        "RGB" if samples_per_pixel == 3 => PixelFormat::Rgb8,
        value if value.starts_with("YBR_") && samples_per_pixel == 3 => PixelFormat::Rgb8,
        value => {
            return Err(ViewerError::Unsupported(format!(
                "{} 的 PhotometricInterpretation={value:?}、SamplesPerPixel={samples_per_pixel} 不受支持",
                path.display()
            )));
        }
    };
    if pixel_format.is_grayscale() && pipeline.photometric == Photometric::NotMonochrome {
        return Err(ViewerError::Unsupported(format!(
            "{} 的灰度光度解释无法解析",
            path.display()
        )));
    }
    let frame_count = integer_u32(&object, tags::NUMBER_OF_FRAMES)
        .unwrap_or(1)
        .max(1);
    let modality = text(&object, tags::MODALITY);

    Ok(ParsedFile {
        path,
        pipeline,
        spacing: resolve(&object),
        rows,
        cols,
        bits_allocated,
        pixel_format,
        photometric_interpretation,
        frame_count,
        cine_rate_fps: cine_rate_fps(&object, frame_count, modality.as_deref()),
        quantitative: quantitative_info(&object),
        laterality: text(&object, tags::IMAGE_LATERALITY)
            .or_else(|| text(&object, tags::LATERALITY)),
        view_position: text(&object, tags::VIEW_POSITION),
        patient_orientation: multi_text(&object, tags::PATIENT_ORIENTATION),
        patient_name: text(&object, tags::PATIENT_NAME),
        patient_id: text(&object, tags::PATIENT_ID),
        patient_sex: text(&object, tags::PATIENT_SEX),
        patient_birth_date: text(&object, tags::PATIENT_BIRTH_DATE),
        study_date: text(&object, tags::STUDY_DATE),
        accession_number: text(&object, tags::ACCESSION_NUMBER),
        modality,
        study_description: text(&object, tags::STUDY_DESCRIPTION),
        series_description: text(&object, tags::SERIES_DESCRIPTION),
        study_uid: text(&object, tags::STUDY_INSTANCE_UID),
        series_uid: text(&object, tags::SERIES_INSTANCE_UID),
        sop_uid: text(&object, tags::SOP_INSTANCE_UID),
        instance_number: integer_i32(&object, tags::INSTANCE_NUMBER),
        position: float_values(&object, tags::IMAGE_POSITION_PATIENT),
        orientation: float_values(&object, tags::IMAGE_ORIENTATION_PATIENT),
    })
}

fn validate_multi_file_series(files: &[ParsedFile]) -> Result<(), ViewerError> {
    if files.iter().any(|file| file.frame_count != 1) {
        return Err(ViewerError::InvalidSeries(
            "多文件输入暂不允许混入多帧实例；请单独打开该多帧文件".to_owned(),
        ));
    }
    let first_study = files[0]
        .study_uid
        .as_deref()
        .ok_or_else(|| ViewerError::InvalidSeries("多文件序列缺少 StudyInstanceUID".to_owned()))?;
    let first_series = files[0]
        .series_uid
        .as_deref()
        .ok_or_else(|| ViewerError::InvalidSeries("多文件序列缺少 SeriesInstanceUID".to_owned()))?;
    if files.iter().any(|file| {
        file.study_uid.as_deref() != Some(first_study)
            || file.series_uid.as_deref() != Some(first_series)
    }) {
        return Err(ViewerError::InvalidSeries(
            "选择中包含不同 StudyInstanceUID 或 SeriesInstanceUID 的文件".to_owned(),
        ));
    }
    Ok(())
}

fn window_presets(pipeline: &Pipeline) -> Vec<WindowPreset> {
    pipeline
        .windows
        .iter()
        .map(|window| WindowPreset {
            center: window.center,
            width: window.width,
            explanation: window.explanation.clone(),
            function: match window.function {
                VoiFunction::Linear => "LINEAR",
                VoiFunction::LinearExact => "LINEAR_EXACT",
                VoiFunction::Sigmoid => "SIGMOID",
            }
            .to_owned(),
        })
        .collect()
}

fn spacing_info(spacing: PixelSpacing) -> SpacingInfo {
    match spacing {
        PixelSpacing::Physical(value) => SpacingInfo {
            confidence: match value.confidence() {
                Confidence::Calibrated => "calibrated",
                Confidence::DetectorPlane => "detector",
                Confidence::None => "none",
            }
            .to_owned(),
            source: Some(source_name(value.source).to_owned()),
            description: value.source.describe().to_owned(),
            row_mm: Some(value.row_mm),
            col_mm: Some(value.column_mm),
            column_over_row: value.column_mm / value.row_mm,
        },
        PixelSpacing::PixelsOnly {
            aspect_ratio,
            reason,
        } => SpacingInfo {
            confidence: "none".to_owned(),
            source: None,
            description: reason.to_owned(),
            row_mm: None,
            col_mm: None,
            column_over_row: 1.0 / aspect_ratio.row_over_column,
        },
    }
}

fn physical_spacing(spacing: PixelSpacing) -> Option<(f64, f64)> {
    match spacing {
        PixelSpacing::Physical(value) => Some((value.row_mm, value.column_mm)),
        PixelSpacing::PixelsOnly { .. } => None,
    }
}

fn fixed_array<const N: usize>(values: Option<&[f64]>) -> Option<[f64; N]> {
    values?.get(..N)?.try_into().ok()
}

fn source_name(source: Source) -> &'static str {
    match source {
        Source::PixelSpacing => "pixel-spacing",
        Source::CalibratedPixelSpacing => "calibrated-pixel-spacing",
        Source::ImagerPixelSpacing => "imager-pixel-spacing",
        Source::MagnificationCorrected => "magnification-corrected",
        Source::UltrasoundRegion => "ultrasound-region",
        Source::NominalScannedPixelSpacing => "nominal-scanned-pixel-spacing",
    }
}

fn text(object: &DefaultDicomObject, tag: Tag) -> Option<String> {
    pacs_core::utf8_text(object, tag)
}

fn multi_text(object: &DefaultDicomObject, tag: Tag) -> Vec<String> {
    object
        .get(tag)
        .and_then(|element| element.to_multi_str().ok())
        .map(|values| {
            values
                .iter()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn float(object: &DefaultDicomObject, tag: Tag) -> Option<f64> {
    object
        .get(tag)?
        .to_float64()
        .ok()
        .filter(|value| value.is_finite())
}

fn cine_rate_fps(
    object: &DefaultDicomObject,
    frame_count: u32,
    modality: Option<&str>,
) -> Option<f64> {
    let explicit = float(object, tags::RECOMMENDED_DISPLAY_FRAME_RATE_IN_FLOAT)
        .or_else(|| float(object, tags::RECOMMENDED_DISPLAY_FRAME_RATE))
        .or_else(|| float(object, tags::CINE_RATE))
        .filter(|fps| *fps > 0.0);
    if explicit.is_some() {
        return explicit;
    }
    if let Some(frame_time_ms) = float(object, tags::FRAME_TIME).filter(|value| *value > 0.0) {
        return Some(1000.0 / frame_time_ms);
    }
    (frame_count > 1 && modality.is_some_and(|value| value.eq_ignore_ascii_case("US")))
        .then_some(15.0)
}

fn quantitative_info(object: &DefaultDicomObject) -> QuantitativeInfo {
    let unit = text(object, tags::UNITS)
        .map(|value| value.trim().to_ascii_uppercase())
        .or_else(|| {
            Pipeline::from_object(object)
                .modality_lut
                .unit
                .map(str::to_owned)
        });
    let sop_class = text(object, tags::SOP_CLASS_UID).unwrap_or_default();
    let is_pet = matches!(
        sop_class.as_str(),
        uids::POSITRON_EMISSION_TOMOGRAPHY_IMAGE_STORAGE
            | uids::ENHANCED_PET_IMAGE_STORAGE
            | uids::LEGACY_CONVERTED_ENHANCED_PET_IMAGE_STORAGE
    );
    if !is_pet {
        return QuantitativeInfo {
            unit,
            suvbw_factor: None,
            suvbw_status: None,
        };
    }

    let unavailable = |reason: String| QuantitativeInfo {
        unit: unit.clone(),
        suvbw_factor: None,
        suvbw_status: Some(reason),
    };
    if unit.as_deref() != Some("BQML") {
        return unavailable("SUVbw 不可用：PET Units 必须为 BQML".to_owned());
    }
    let Some(weight_kg) = float(object, tags::PATIENT_WEIGHT).filter(|value| *value > 0.0) else {
        return unavailable("SUVbw 不可用：缺少有效 PatientWeight".to_owned());
    };
    let Some(item) = object
        .get(tags::RADIOPHARMACEUTICAL_INFORMATION_SEQUENCE)
        .and_then(|element| element.items())
        .and_then(|items| items.first())
    else {
        return unavailable("SUVbw 不可用：缺少 RadiopharmaceuticalInformationSequence".to_owned());
    };
    let item_float = |tag| {
        item.get(tag)
            .and_then(|element| element.to_float64().ok())
            .filter(|value| value.is_finite() && *value > 0.0)
    };
    let Some(total_dose_bq) = item_float(tags::RADIONUCLIDE_TOTAL_DOSE) else {
        return unavailable("SUVbw 不可用：缺少有效 RadionuclideTotalDose".to_owned());
    };
    let correction = text(object, tags::DECAY_CORRECTION)
        .unwrap_or_default()
        .trim()
        .to_ascii_uppercase();
    let corrected_dose_bq = if correction == "ADMIN" {
        total_dose_bq
    } else if correction == "START" {
        let Some(half_life_seconds) = item_float(tags::RADIONUCLIDE_HALF_LIFE) else {
            return unavailable("SUVbw 不可用：缺少有效 RadionuclideHalfLife".to_owned());
        };
        let Some(acquisition) = acquisition_datetime(object) else {
            return unavailable("SUVbw 不可用：缺少完整采集日期时间".to_owned());
        };
        let Some(injection) = injection_datetime(item, acquisition) else {
            return unavailable("SUVbw 不可用：缺少完整注射日期时间".to_owned());
        };
        let elapsed_seconds = (acquisition - injection).num_milliseconds() as f64 / 1000.0;
        if elapsed_seconds < 0.0 {
            return unavailable("SUVbw 不可用：注射时间晚于采集时间".to_owned());
        }
        total_dose_bq * 0.5_f64.powf(elapsed_seconds / half_life_seconds)
    } else {
        return unavailable("SUVbw 不可用：DecayCorrection 必须为 START 或 ADMIN".to_owned());
    };
    if !corrected_dose_bq.is_finite() || corrected_dose_bq <= 0.0 {
        return unavailable("SUVbw 不可用：衰变校正后的注射剂量无效".to_owned());
    }
    QuantitativeInfo {
        unit,
        suvbw_factor: Some(weight_kg * 1000.0 / corrected_dose_bq),
        suvbw_status: Some("SUVbw 可用".to_owned()),
    }
}

fn acquisition_datetime(object: &DefaultDicomObject) -> Option<NaiveDateTime> {
    text(object, tags::ACQUISITION_DATE_TIME)
        .and_then(|value| parse_dicom_datetime(&value))
        .or_else(|| {
            let date =
                text(object, tags::ACQUISITION_DATE).or_else(|| text(object, tags::SERIES_DATE))?;
            let time =
                text(object, tags::ACQUISITION_TIME).or_else(|| text(object, tags::SERIES_TIME))?;
            combine_dicom_date_time(&date, &time)
        })
}

fn injection_datetime(
    item: &dicom::object::InMemDicomObject,
    acquisition: NaiveDateTime,
) -> Option<NaiveDateTime> {
    let value = item
        .get(tags::RADIOPHARMACEUTICAL_START_DATE_TIME)
        .and_then(|element| element.to_str().ok())
        .map(|value| value.into_owned());
    if let Some(datetime) = value.and_then(|value| parse_dicom_datetime(&value)) {
        return Some(datetime);
    }
    let time = item
        .get(tags::RADIOPHARMACEUTICAL_START_TIME)?
        .to_str()
        .ok()?;
    let time = parse_dicom_time(&time)?;
    let candidate = acquisition.date().and_time(time);
    Some(if candidate > acquisition {
        candidate - Duration::days(1)
    } else {
        candidate
    })
}

fn parse_dicom_datetime(raw: &str) -> Option<NaiveDateTime> {
    let normalized = raw.trim().trim_end_matches('\0');
    let main = normalized.split(['+', '-']).next().unwrap_or(normalized);
    if main.len() < 14 {
        return None;
    }
    let date = NaiveDate::parse_from_str(&main[..8], "%Y%m%d").ok()?;
    let time = parse_dicom_time(&main[8..])?;
    Some(date.and_time(time))
}

fn combine_dicom_date_time(date: &str, time: &str) -> Option<NaiveDateTime> {
    let date = NaiveDate::parse_from_str(date.trim(), "%Y%m%d").ok()?;
    Some(date.and_time(parse_dicom_time(time)?))
}

fn parse_dicom_time(raw: &str) -> Option<NaiveTime> {
    let normalized = raw.trim().trim_end_matches('\0');
    let (whole, fraction) = normalized.split_once('.').unwrap_or((normalized, ""));
    if whole.len() < 6 {
        return None;
    }
    let hour = whole[..2].parse().ok()?;
    let minute = whole[2..4].parse().ok()?;
    let second = whole[4..6].parse().ok()?;
    let mut fractional = fraction
        .chars()
        .take(9)
        .filter(|character| character.is_ascii_digit())
        .collect::<String>();
    while fractional.len() < 9 {
        fractional.push('0');
    }
    let nanos = if fractional.is_empty() {
        0
    } else {
        fractional.parse().ok()?
    };
    NaiveTime::from_hms_nano_opt(hour, minute, second, nanos)
}

fn integer_u16(object: &DefaultDicomObject, tag: Tag) -> Option<u16> {
    object.get(tag)?.to_int::<u16>().ok()
}

fn integer_u32(object: &DefaultDicomObject, tag: Tag) -> Option<u32> {
    object.get(tag)?.to_int::<u32>().ok()
}

fn integer_i32(object: &DefaultDicomObject, tag: Tag) -> Option<i32> {
    object.get(tag)?.to_int::<i32>().ok()
}

fn required_u32(
    object: &DefaultDicomObject,
    tag: Tag,
    name: &str,
    path: &std::path::Path,
) -> Result<u32, ViewerError> {
    integer_u32(object, tag)
        .ok_or_else(|| ViewerError::Dicom(format!("{} 缺少或无法解析 {name}", path.display())))
}

fn float_values(object: &DefaultDicomObject, tag: Tag) -> Option<Vec<f64>> {
    let values = object.get(tag)?.to_multi_float64().ok()?;
    (!values.is_empty()).then_some(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mpr::ProjectionMode;
    use dicom::core::{DataElement, DicomValue, PrimitiveValue, VR, value::DataSetSequence};
    use dicom::object::InMemDicomObject;
    use pacs_core::fixture::{ct_instance, unique_uid};

    fn write_slice(
        directory: &tempfile::TempDir,
        study: &str,
        series: &str,
        z: i32,
    ) -> (PathBuf, String) {
        let sop = unique_uid();
        let mut object = ct_instance(study, series, &sop);
        object.put(DataElement::new(
            tags::IMAGE_POSITION_PATIENT,
            VR::DS,
            PrimitiveValue::Strs(vec!["0".to_owned(), "0".to_owned(), z.to_string()].into()),
        ));
        object.put(DataElement::new(
            tags::INSTANCE_NUMBER,
            VR::IS,
            z.to_string(),
        ));
        let path = directory.path().join(format!("{z}.dcm"));
        object.write_to_file(&path).expect("测试 DICOM 应能写出");
        (path, sop)
    }

    fn write_oriented_slice(
        directory: &tempfile::TempDir,
        study: &str,
        series: &str,
        name: &str,
        position: [f64; 3],
        orientation: [f64; 6],
    ) -> (PathBuf, String) {
        let sop = unique_uid();
        let mut object = ct_instance(study, series, &sop);
        object.put(DataElement::new(
            tags::IMAGE_POSITION_PATIENT,
            VR::DS,
            PrimitiveValue::Strs(
                position
                    .into_iter()
                    .map(|value| value.to_string())
                    .collect::<Vec<_>>()
                    .into(),
            ),
        ));
        object.put(DataElement::new(
            tags::IMAGE_ORIENTATION_PATIENT,
            VR::DS,
            PrimitiveValue::Strs(
                orientation
                    .into_iter()
                    .map(|value| value.to_string())
                    .collect::<Vec<_>>()
                    .into(),
            ),
        ));
        let path = directory.path().join(format!("{name}.dcm"));
        object.write_to_file(&path).expect("测试 DICOM 应能写出");
        (path, sop)
    }

    #[test]
    fn multi_file_series_is_sorted_by_geometry() {
        let directory = tempfile::tempdir().unwrap();
        let study = unique_uid();
        let series = unique_uid();
        let (high, high_sop) = write_slice(&directory, &study, &series, 20);
        let (low, low_sop) = write_slice(&directory, &study, &series, -10);

        let state = ViewerState::new();
        let metadata = state.open_series(vec![high, low]).unwrap();
        assert_eq!(metadata.frames.len(), 2);
        assert_eq!(
            metadata.frames[0].sop_instance_uid.as_deref(),
            Some(low_sop.as_str())
        );
        assert_eq!(
            metadata.frames[1].sop_instance_uid.as_deref(),
            Some(high_sop.as_str())
        );
        assert_eq!(metadata.active_stack, 0);
        assert_eq!(metadata.image_stacks.len(), 1);
    }

    #[test]
    fn localizer_and_main_stack_are_split_and_selectable() {
        let directory = tempfile::tempdir().unwrap();
        let study = unique_uid();
        let series = unique_uid();
        let axial = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        let sagittal = [0.0, 1.0, 0.0, 0.0, 0.0, -1.0];
        let (localizer, localizer_sop) = write_oriented_slice(
            &directory,
            &study,
            &series,
            "localizer",
            [0.0, 0.0, 0.0],
            sagittal,
        );
        let (high, _) = write_oriented_slice(
            &directory,
            &study,
            &series,
            "axial-high",
            [0.0, 0.0, 20.0],
            axial,
        );
        let (low, low_sop) = write_oriented_slice(
            &directory,
            &study,
            &series,
            "axial-low",
            [0.0, 0.0, -10.0],
            axial,
        );

        let state = ViewerState::new();
        let metadata = state.open_series(vec![localizer, high, low]).unwrap();
        assert_eq!(metadata.image_stacks.len(), 2);
        assert_eq!(metadata.frames.len(), 2, "最大的主堆栈应默认打开");
        assert_eq!(
            metadata.frames[0].sop_instance_uid.as_deref(),
            Some(low_sop.as_str())
        );
        assert!(metadata.warnings[0].contains("2 个不同朝向或尺寸"));

        let localizer_metadata = state.select_image_stack(metadata.handle, 1).unwrap();
        assert_eq!(localizer_metadata.active_stack, 1);
        assert_eq!(localizer_metadata.frames.len(), 1);
        assert_eq!(
            localizer_metadata.frames[0].sop_instance_uid.as_deref(),
            Some(localizer_sop.as_str())
        );
    }

    #[test]
    fn two_orthogonal_multi_slice_stacks_are_both_preserved() {
        let directory = tempfile::tempdir().unwrap();
        let study = unique_uid();
        let series = unique_uid();
        let axial = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        let coronal = [1.0, 0.0, 0.0, 0.0, 0.0, -1.0];
        let mut paths = Vec::new();
        for (name, position, orientation) in [
            ("axial-1", [0.0, 0.0, 1.0], axial),
            ("coronal-1", [0.0, 1.0, 0.0], coronal),
            ("axial-2", [0.0, 0.0, 2.0], axial),
            ("coronal-2", [0.0, 2.0, 0.0], coronal),
        ] {
            paths.push(
                write_oriented_slice(&directory, &study, &series, name, position, orientation).0,
            );
        }

        let state = ViewerState::new();
        let metadata = state.open_series(paths).unwrap();
        assert_eq!(metadata.image_stacks.len(), 2);
        assert_eq!(metadata.image_stacks[0].frame_count, 2);
        assert_eq!(metadata.image_stacks[1].frame_count, 2);
        assert_eq!(
            state
                .select_image_stack(metadata.handle, 1)
                .unwrap()
                .frames
                .len(),
            2
        );
    }

    #[test]
    fn duplicate_positions_remain_a_warning_inside_their_stack() {
        let directory = tempfile::tempdir().unwrap();
        let study = unique_uid();
        let series = unique_uid();
        let axial = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        let first = write_oriented_slice(
            &directory,
            &study,
            &series,
            "duplicate-a",
            [0.0, 0.0, 5.0],
            axial,
        )
        .0;
        let second = write_oriented_slice(
            &directory,
            &study,
            &series,
            "duplicate-b",
            [0.0, 0.0, 5.0],
            axial,
        )
        .0;

        let metadata = ViewerState::new().open_series(vec![first, second]).unwrap();
        assert!(metadata.warnings[0].contains("重复切片位置"));
    }

    #[test]
    fn mixed_series_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let study = unique_uid();
        let (first, _) = write_slice(&directory, &study, &unique_uid(), 0);
        let (second, _) = write_slice(&directory, &study, &unique_uid(), 1);

        let error = ViewerState::new()
            .open_series(vec![first, second])
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("不同 StudyInstanceUID 或 SeriesInstanceUID")
        );
    }

    #[test]
    fn opens_and_decodes_a_single_frame() {
        let directory = tempfile::tempdir().unwrap();
        let (path, _) = write_slice(&directory, &unique_uid(), &unique_uid(), 0);
        let state = ViewerState::new();
        let metadata = state.open_series(vec![path]).unwrap();

        assert_eq!(metadata.patient.patient_id.as_deref(), Some("PID-0001"));
        assert_eq!(metadata.frames[0].bits_allocated, 16);
        assert_eq!(
            state
                .get_frame_bytes(metadata.handle, metadata.active_stack, 0)
                .unwrap()
                .len(),
            4 * 4 * 2
        );
        assert_eq!(
            state
                .build_lut(
                    metadata.handle,
                    metadata.active_stack,
                    0,
                    -600.0,
                    1500.0,
                    "LINEAR",
                )
                .unwrap()
                .len(),
            65_536
        );
    }

    #[test]
    fn builds_and_renders_mpr_from_a_regular_stack() {
        let directory = tempfile::tempdir().unwrap();
        let study = unique_uid();
        let series = unique_uid();
        let paths = [0, 1, 2]
            .into_iter()
            .map(|z| write_slice(&directory, &study, &series, z).0)
            .collect();
        let state = ViewerState::new();
        let opened = state.open_series(paths).unwrap();
        let metadata = state
            .prepare_mpr(opened.handle, opened.active_stack, |_, _| {})
            .unwrap();
        assert_eq!(metadata.dimensions, [4, 4, 3]);
        let axial = metadata
            .planes
            .iter()
            .find(|plane| plane.plane == Plane::Axial)
            .unwrap();
        let bytes = state
            .render_mpr_slice(
                opened.handle,
                Plane::Axial,
                axial.slice_count / 2,
                &MprRenderOptions {
                    window_center: -600.0,
                    window_width: 1500.0,
                    voi_function: "LINEAR",
                    projection: ProjectionMode::Slice,
                    slab_thickness_mm: 1.0,
                },
            )
            .unwrap();
        assert_eq!(bytes.len(), axial.rows as usize * axial.cols as usize);
    }

    #[test]
    fn keeps_geometryless_files_as_standalone_images() {
        let directory = tempfile::tempdir().unwrap();
        let study = unique_uid();
        let series = unique_uid();
        let (first, _) = write_slice(&directory, &study, &series, 0);
        let sop = unique_uid();
        let mut object = ct_instance(&study, &series, &sop);
        object.remove_element(tags::IMAGE_POSITION_PATIENT);
        let second = directory.path().join("missing-position.dcm");
        object.write_to_file(&second).unwrap();

        let state = ViewerState::new();
        let metadata = state.open_series(vec![first, second]).unwrap();
        assert_eq!(metadata.image_stacks.len(), 2);
        assert_eq!(metadata.frames.len(), 1, "几何完整的主堆栈应默认打开");

        let standalone = state.select_image_stack(metadata.handle, 1).unwrap();
        assert_eq!(standalone.frames.len(), 1);
        assert!(
            standalone
                .warnings
                .iter()
                .any(|warning| warning.contains("仅作为单张图像显示"))
        );
    }

    #[test]
    fn supports_single_file_multiframe_images() {
        let directory = tempfile::tempdir().unwrap();
        let mut object = ct_instance(&unique_uid(), &unique_uid(), &unique_uid());
        object.put(DataElement::new(tags::NUMBER_OF_FRAMES, VR::IS, "2"));
        object.put(DataElement::new(
            tags::PIXEL_DATA,
            VR::OW,
            PrimitiveValue::U16(vec![0_u16; 4 * 4 * 2].into()),
        ));
        let path = directory.path().join("multiframe.dcm");
        object.write_to_file(&path).unwrap();

        let state = ViewerState::new();
        let metadata = state.open_series(vec![path]).unwrap();
        assert_eq!(metadata.frames.len(), 2);
        assert_eq!(metadata.frames[1].source_frame, 2);
        assert_eq!(
            state
                .get_frame_bytes(metadata.handle, metadata.active_stack, 1)
                .unwrap()
                .len(),
            4 * 4 * 2
        );
    }

    #[test]
    fn supports_eight_bit_grayscale_frames_and_luts() {
        let directory = tempfile::tempdir().unwrap();
        let mut object = ct_instance(&unique_uid(), &unique_uid(), &unique_uid());
        object.put(DataElement::new(
            tags::BITS_ALLOCATED,
            VR::US,
            PrimitiveValue::from(8_u16),
        ));
        object.put(DataElement::new(
            tags::BITS_STORED,
            VR::US,
            PrimitiveValue::from(8_u16),
        ));
        object.put(DataElement::new(
            tags::HIGH_BIT,
            VR::US,
            PrimitiveValue::from(7_u16),
        ));
        object.put(DataElement::new(
            tags::PIXEL_DATA,
            VR::OB,
            PrimitiveValue::U8(vec![0_u8; 4 * 4].into()),
        ));
        let path = directory.path().join("eight-bit.dcm");
        object.write_to_file(&path).unwrap();

        let state = ViewerState::new();
        let metadata = state.open_series(vec![path]).unwrap();
        assert_eq!(metadata.frames[0].bits_allocated, 8);
        assert_eq!(
            state
                .get_frame_bytes(metadata.handle, metadata.active_stack, 0)
                .unwrap()
                .len(),
            4 * 4
        );
        assert_eq!(
            state
                .build_lut(
                    metadata.handle,
                    metadata.active_stack,
                    0,
                    128.0,
                    256.0,
                    "LINEAR",
                )
                .unwrap()
                .len(),
            256
        );
    }

    #[test]
    fn opens_rgb_images_as_interleaved_rgb8() {
        let directory = tempfile::tempdir().unwrap();
        let mut object = ct_instance(&unique_uid(), &unique_uid(), &unique_uid());
        object.put(DataElement::new(tags::MODALITY, VR::CS, "US"));
        object.put(DataElement::new(
            tags::PHOTOMETRIC_INTERPRETATION,
            VR::CS,
            "RGB",
        ));
        object.put(DataElement::new(
            tags::SAMPLES_PER_PIXEL,
            VR::US,
            PrimitiveValue::from(3_u16),
        ));
        object.put(DataElement::new(
            tags::PLANAR_CONFIGURATION,
            VR::US,
            PrimitiveValue::from(0_u16),
        ));
        object.put(DataElement::new(
            tags::BITS_ALLOCATED,
            VR::US,
            PrimitiveValue::from(8_u16),
        ));
        object.put(DataElement::new(
            tags::BITS_STORED,
            VR::US,
            PrimitiveValue::from(8_u16),
        ));
        object.put(DataElement::new(
            tags::HIGH_BIT,
            VR::US,
            PrimitiveValue::from(7_u16),
        ));
        object.put(DataElement::new(
            tags::PIXEL_DATA,
            VR::OB,
            PrimitiveValue::U8([255_u8, 32, 16].repeat(4 * 4).into()),
        ));
        let path = directory.path().join("rgb.dcm");
        object.write_to_file(&path).unwrap();

        let state = ViewerState::new();
        let metadata = state.open_series(vec![path]).unwrap();
        assert_eq!(metadata.frames[0].pixel_format, PixelFormat::Rgb8);
        assert_eq!(metadata.frames[0].photometric_interpretation, "RGB");
        let bytes = state
            .get_frame_bytes(metadata.handle, metadata.active_stack, 0)
            .unwrap();
        assert_eq!(bytes.len(), 4 * 4 * 3);
        assert_eq!(&bytes[..3], &[255, 32, 16]);
        assert!(
            state
                .build_lut(
                    metadata.handle,
                    metadata.active_stack,
                    0,
                    128.0,
                    256.0,
                    "LINEAR",
                )
                .is_err()
        );
    }

    #[test]
    fn expands_palette_color_lookup_tables_to_rgb8() {
        let directory = tempfile::tempdir().unwrap();
        let mut object = ct_instance(&unique_uid(), &unique_uid(), &unique_uid());
        object.put(DataElement::new(
            tags::PHOTOMETRIC_INTERPRETATION,
            VR::CS,
            "PALETTE COLOR",
        ));
        object.put(DataElement::new(
            tags::BITS_ALLOCATED,
            VR::US,
            PrimitiveValue::from(8_u16),
        ));
        object.put(DataElement::new(
            tags::BITS_STORED,
            VR::US,
            PrimitiveValue::from(8_u16),
        ));
        object.put(DataElement::new(
            tags::HIGH_BIT,
            VR::US,
            PrimitiveValue::from(7_u16),
        ));
        let descriptor = PrimitiveValue::U16(vec![2_u16, 0, 8].into());
        for tag in [
            tags::RED_PALETTE_COLOR_LOOKUP_TABLE_DESCRIPTOR,
            tags::GREEN_PALETTE_COLOR_LOOKUP_TABLE_DESCRIPTOR,
            tags::BLUE_PALETTE_COLOR_LOOKUP_TABLE_DESCRIPTOR,
        ] {
            object.put(DataElement::new(tag, VR::US, descriptor.clone()));
        }
        object.put(DataElement::new(
            tags::RED_PALETTE_COLOR_LOOKUP_TABLE_DATA,
            VR::OW,
            PrimitiveValue::U16(vec![0_u16, 255].into()),
        ));
        object.put(DataElement::new(
            tags::GREEN_PALETTE_COLOR_LOOKUP_TABLE_DATA,
            VR::OW,
            PrimitiveValue::U16(vec![0_u16, 0].into()),
        ));
        object.put(DataElement::new(
            tags::BLUE_PALETTE_COLOR_LOOKUP_TABLE_DATA,
            VR::OW,
            PrimitiveValue::U16(vec![0_u16, 255].into()),
        ));
        object.put(DataElement::new(
            tags::PIXEL_DATA,
            VR::OB,
            PrimitiveValue::U8(
                (0..4 * 4)
                    .map(|index| (index % 2) as u8)
                    .collect::<Vec<_>>()
                    .into(),
            ),
        ));
        let path = directory.path().join("palette.dcm");
        object.write_to_file(&path).unwrap();

        let state = ViewerState::new();
        let metadata = state.open_series(vec![path]).unwrap();
        let bytes = state
            .get_frame_bytes(metadata.handle, metadata.active_stack, 0)
            .unwrap();
        assert_eq!(metadata.frames[0].pixel_format, PixelFormat::Rgb8);
        assert_eq!(&bytes[..6], &[0, 0, 0, 255, 0, 255]);
    }

    #[test]
    fn converts_ybr_full_to_rgb8() {
        let directory = tempfile::tempdir().unwrap();
        let mut object = ct_instance(&unique_uid(), &unique_uid(), &unique_uid());
        object.put(DataElement::new(
            tags::PHOTOMETRIC_INTERPRETATION,
            VR::CS,
            "YBR_FULL",
        ));
        object.put(DataElement::new(
            tags::SAMPLES_PER_PIXEL,
            VR::US,
            PrimitiveValue::from(3_u16),
        ));
        object.put(DataElement::new(
            tags::PLANAR_CONFIGURATION,
            VR::US,
            PrimitiveValue::from(0_u16),
        ));
        object.put(DataElement::new(
            tags::BITS_ALLOCATED,
            VR::US,
            PrimitiveValue::from(8_u16),
        ));
        object.put(DataElement::new(
            tags::BITS_STORED,
            VR::US,
            PrimitiveValue::from(8_u16),
        ));
        object.put(DataElement::new(
            tags::HIGH_BIT,
            VR::US,
            PrimitiveValue::from(7_u16),
        ));
        object.put(DataElement::new(
            tags::PIXEL_DATA,
            VR::OB,
            PrimitiveValue::U8([128_u8, 128, 128].repeat(4 * 4).into()),
        ));
        let path = directory.path().join("ybr.dcm");
        object.write_to_file(&path).unwrap();

        let state = ViewerState::new();
        let metadata = state.open_series(vec![path]).unwrap();
        let bytes = state
            .get_frame_bytes(metadata.handle, metadata.active_stack, 0)
            .unwrap();
        assert_eq!(metadata.frames[0].photometric_interpretation, "YBR_FULL");
        assert!(bytes[..3].iter().all(|value| value.abs_diff(128) <= 1));
    }

    #[test]
    fn derives_cine_rate_from_frame_time_and_has_us_fallback() {
        let mut timed = ct_instance(&unique_uid(), &unique_uid(), &unique_uid());
        timed.put(DataElement::new(tags::FRAME_TIME, VR::DS, "50"));
        assert_eq!(cine_rate_fps(&timed, 10, Some("US")), Some(20.0));

        let fallback = ct_instance(&unique_uid(), &unique_uid(), &unique_uid());
        assert_eq!(cine_rate_fps(&fallback, 10, Some("US")), Some(15.0));
        assert_eq!(cine_rate_fps(&fallback, 1, Some("US")), None);
    }

    fn pet_object() -> DefaultDicomObject {
        let mut object = ct_instance(&unique_uid(), &unique_uid(), &unique_uid());
        object.put(DataElement::new(
            tags::SOP_CLASS_UID,
            VR::UI,
            uids::POSITRON_EMISSION_TOMOGRAPHY_IMAGE_STORAGE,
        ));
        object.put(DataElement::new(tags::MODALITY, VR::CS, "PT"));
        object.put(DataElement::new(tags::UNITS, VR::CS, "BQML"));
        object.put(DataElement::new(tags::PATIENT_WEIGHT, VR::DS, "70"));
        object.put(DataElement::new(tags::DECAY_CORRECTION, VR::CS, "START"));
        object.put(DataElement::new(
            tags::ACQUISITION_DATE_TIME,
            VR::DT,
            "20240315143000",
        ));
        let radiopharmaceutical = InMemDicomObject::from_element_iter([
            DataElement::new(tags::RADIONUCLIDE_TOTAL_DOSE, VR::DS, "370000000"),
            DataElement::new(tags::RADIONUCLIDE_HALF_LIFE, VR::DS, "6586.2"),
            DataElement::new(
                tags::RADIOPHARMACEUTICAL_START_DATE_TIME,
                VR::DT,
                "20240315140000",
            ),
        ]);
        object.put(DataElement::new(
            tags::RADIOPHARMACEUTICAL_INFORMATION_SEQUENCE,
            VR::SQ,
            DicomValue::Sequence(DataSetSequence::from(vec![radiopharmaceutical])),
        ));
        object
    }

    #[test]
    fn calculates_pet_suvbw_when_required_metadata_is_complete() {
        let info = quantitative_info(&pet_object());
        let expected_decay = 370_000_000.0 * 0.5_f64.powf(1800.0 / 6586.2);
        let expected = 70_000.0 / expected_decay;
        assert_eq!(info.unit.as_deref(), Some("BQML"));
        assert!((info.suvbw_factor.unwrap() - expected).abs() < 1e-12);
        assert_eq!(info.suvbw_status.as_deref(), Some("SUVbw 可用"));
    }

    #[test]
    fn explains_why_pet_suvbw_is_unavailable() {
        let mut object = pet_object();
        object.remove_element(tags::PATIENT_WEIGHT);
        let info = quantitative_info(&object);
        assert_eq!(info.suvbw_factor, None);
        assert!(info.suvbw_status.unwrap().contains("PatientWeight"));
    }

    #[test]
    fn frame_cache_evicts_the_least_recently_used_entry() {
        let mut cache = FrameCache::new(8, FRAME_CACHE_TTL);
        cache.insert((1, 0, 0), vec![0; 4]);
        cache.insert((1, 0, 1), vec![1; 4]);
        assert!(cache.get(&(1, 0, 0)).is_some());
        cache.insert((1, 0, 2), vec![2; 4]);

        assert!(cache.get(&(1, 0, 0)).is_some());
        assert!(cache.get(&(1, 0, 1)).is_none());
        assert!(cache.get(&(1, 0, 2)).is_some());
        assert_eq!(cache.total_bytes, 8);
    }

    #[test]
    fn frame_cache_removes_entries_after_three_idle_minutes() {
        let now = Instant::now();
        let mut cache = FrameCache::new(8, FRAME_CACHE_TTL);
        cache.insert_at((1, 0, 0), vec![0; 4], now);

        assert!(
            cache
                .get_at(&(1, 0, 0), now + StdDuration::from_secs(179))
                .is_some()
        );
        cache.remove_expired(now + StdDuration::from_secs(180));
        assert_eq!(cache.total_bytes, 4, "访问应刷新缓存期限");

        cache.remove_expired(now + StdDuration::from_secs(359));
        assert_eq!(cache.total_bytes, 0);
        assert!(cache.data.is_empty());
    }
}
