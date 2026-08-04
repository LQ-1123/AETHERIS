//! Local DICOM series state for the desktop viewer.

use crate::mpr::{MprMetadata, Plane, SourceSlice, Volume};
use dicom::core::Tag;
use dicom::dictionary_std::tags;
use dicom::object::{DefaultDicomObject, open_file};
use pacs_codec::{Frames, GrayLut, Photometric, Pipeline, VoiFunction};
use pacs_core::geometry::{SliceInput, Vec3, group_slices_by_orientation, sort_slices};
use pacs_core::spacing::{Confidence, PixelSpacing, Source, resolve};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use thiserror::Error;

pub type SeriesHandle = u64;
type FrameKey = (SeriesHandle, u32, u32);

const FRAME_CACHE_LIMIT: usize = 512 * 1024 * 1024;
const PREFETCH_RADIUS: u32 = 2;

#[derive(Clone)]
pub struct ViewerState {
    inner: Arc<Mutex<ViewerStateInner>>,
    mpr_cancelled: Arc<AtomicBool>,
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
    path: PathBuf,
    source_frame: u32,
    pipeline: Pipeline,
    rows: u32,
    cols: u32,
    bits_allocated: u16,
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
    frame_count: u32,
    patient_name: Option<String>,
    patient_id: Option<String>,
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
    data: HashMap<FrameKey, Vec<u8>>,
    access_queue: VecDeque<FrameKey>,
    total_bytes: usize,
    limit: usize,
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
    pub window_presets: Vec<WindowPreset>,
    pub spacing: SpacingInfo,
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
        Self {
            inner: Arc::new(Mutex::new(ViewerStateInner {
                next_handle: 1,
                series: HashMap::new(),
                frame_cache: FrameCache::new(FRAME_CACHE_LIMIT),
            })),
            mpr_cancelled: Arc::new(AtomicBool::new(false)),
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

    pub fn close(&self, handle: SeriesHandle) -> Result<(), ViewerError> {
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
                        frame.bits_allocated,
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
        for (logical, source, rows, cols, bits_allocated) in neighbours {
            let bytes = frames
                .frame(source)
                .map_err(|e| ViewerError::Dicom(e.to_string()))?
                .to_vec();
            let expected = rows as usize * cols as usize * usize::from(bits_allocated / 8);
            if bytes.len() != expected {
                return Err(ViewerError::Unsupported(format!(
                    "解码帧大小为 {} 字节，预期 {expected} 字节；当前仅支持单通道整数灰度",
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

    pub fn prepare_mpr(
        &self,
        handle: SeriesHandle,
        stack_index: u32,
        progress: impl Fn(usize, usize) + Sync,
    ) -> Result<MprMetadata, ViewerError> {
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
        window_center: f64,
        window_width: f64,
        voi_function: &str,
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
            .render_slice(
                plane,
                slice_index,
                window_center,
                window_width,
                voi_function,
            )
            .map_err(ViewerError::Unsupported)
    }

    pub fn close_mpr(&self, handle: SeriesHandle) -> Result<(), ViewerError> {
        let mut inner = self.lock();
        let series = inner
            .series
            .get_mut(&handle)
            .ok_or(ViewerError::UnknownHandle(handle))?;
        series.mpr = None;
        Ok(())
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
    fn new(limit: usize) -> Self {
        Self {
            data: HashMap::new(),
            access_queue: VecDeque::new(),
            total_bytes: 0,
            limit,
        }
    }

    fn get(&mut self, key: &FrameKey) -> Option<&Vec<u8>> {
        if self.data.contains_key(key) {
            self.access_queue.retain(|candidate| candidate != key);
            self.access_queue.push_back(*key);
            self.data.get(key)
        } else {
            None
        }
    }

    fn insert(&mut self, key: FrameKey, data: Vec<u8>) {
        let size = data.len();
        if let Some(old) = self.data.insert(key, data) {
            self.total_bytes -= old.len();
            self.access_queue.retain(|candidate| candidate != &key);
        }
        self.total_bytes += size;
        self.access_queue.push_back(key);

        while self.total_bytes > self.limit && self.data.len() > 1 {
            let Some(evict_key) = self.access_queue.pop_front() else {
                break;
            };
            if let Some(evicted) = self.data.remove(&evict_key) {
                self.total_bytes -= evicted.len();
            }
        }
    }

    fn remove_series(&mut self, handle: SeriesHandle) {
        self.data.retain(|key, value| {
            if key.0 == handle {
                self.total_bytes -= value.len();
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
    let mut plans = {
        let slices = parsed
            .iter()
            .map(|file| SliceInput {
                position: file.position.as_deref().unwrap_or(&[]),
                orientation: file.orientation.as_deref().unwrap_or(&[]),
            })
            .collect::<Vec<_>>();
        let orientation_groups = group_slices_by_orientation(&slices).map_err(geometry_error)?;
        let mut dimension_groups = Vec::<Vec<usize>>::new();
        for orientation_group in orientation_groups {
            let mut compatible_dimensions = Vec::<Vec<usize>>::new();
            for source_index in orientation_group {
                let file = &parsed[source_index];
                if let Some(group) = compatible_dimensions.iter_mut().find(|group| {
                    let reference = &parsed[group[0]];
                    reference.rows == file.rows
                        && reference.cols == file.cols
                        && reference.bits_allocated == file.bits_allocated
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
    };

    plans.sort_by(|left, right| {
        right
            .order
            .len()
            .cmp(&left.order.len())
            .then_with(|| left.first_source_index.cmp(&right.first_source_index))
    });
    let mut slots = parsed.into_iter().map(Some).collect::<Vec<_>>();
    Ok(plans
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
        .collect())
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
                frame_key,
                sop_instance_uid: file.sop_uid.clone(),
                source_frame,
                instance_number: file.instance_number,
                rows: file.rows,
                cols: file.cols,
                bits_allocated: file.bits_allocated,
                window_presets: window_presets(&file.pipeline),
                spacing: spacing_info(file.spacing),
            });
            frames.push(LoadedFrame {
                path: file.path.clone(),
                source_frame,
                pipeline: file.pipeline.clone(),
                rows: file.rows,
                cols: file.cols,
                bits_allocated: file.bits_allocated,
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
    if pipeline.photometric == Photometric::NotMonochrome {
        return Err(ViewerError::Unsupported(format!(
            "{} 不是 MONOCHROME1/MONOCHROME2 灰度影像",
            path.display()
        )));
    }
    let samples_per_pixel = integer_u16(&object, tags::SAMPLES_PER_PIXEL).unwrap_or(1);
    if samples_per_pixel != 1 {
        return Err(ViewerError::Unsupported(format!(
            "{} 的 SamplesPerPixel={samples_per_pixel}，当前仅支持单通道灰度",
            path.display()
        )));
    }
    let rows = required_u32(&object, tags::ROWS, "Rows", &path)?;
    let cols = required_u32(&object, tags::COLUMNS, "Columns", &path)?;
    let bits_allocated = integer_u16(&object, tags::BITS_ALLOCATED).unwrap_or(16);
    if !matches!(bits_allocated, 8 | 16) {
        return Err(ViewerError::Unsupported(format!(
            "{} 的 BitsAllocated={bits_allocated}，当前仅支持 8 或 16 位整数灰度",
            path.display()
        )));
    }
    let frame_count = integer_u32(&object, tags::NUMBER_OF_FRAMES)
        .unwrap_or(1)
        .max(1);

    Ok(ParsedFile {
        path,
        pipeline,
        spacing: resolve(&object),
        rows,
        cols,
        bits_allocated,
        frame_count,
        patient_name: text(&object, tags::PATIENT_NAME),
        patient_id: text(&object, tags::PATIENT_ID),
        study_date: text(&object, tags::STUDY_DATE),
        accession_number: text(&object, tags::ACCESSION_NUMBER),
        modality: text(&object, tags::MODALITY),
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
    use dicom::core::{DataElement, PrimitiveValue, VR};
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
                -600.0,
                1500.0,
                "LINEAR",
            )
            .unwrap();
        assert_eq!(bytes.len(), axial.rows as usize * axial.cols as usize);
    }

    #[test]
    fn rejects_multi_file_series_without_patient_geometry() {
        let directory = tempfile::tempdir().unwrap();
        let study = unique_uid();
        let series = unique_uid();
        let (first, _) = write_slice(&directory, &study, &series, 0);
        let sop = unique_uid();
        let mut object = ct_instance(&study, &series, &sop);
        object.remove_element(tags::IMAGE_POSITION_PATIENT);
        let second = directory.path().join("missing-position.dcm");
        object.write_to_file(&second).unwrap();

        let error = ViewerState::new()
            .open_series(vec![first, second])
            .unwrap_err();
        assert!(error.to_string().contains("无法按 ImagePositionPatient"));
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
    fn frame_cache_evicts_the_least_recently_used_entry() {
        let mut cache = FrameCache::new(8);
        cache.insert((1, 0, 0), vec![0; 4]);
        cache.insert((1, 0, 1), vec![1; 4]);
        assert!(cache.get(&(1, 0, 0)).is_some());
        cache.insert((1, 0, 2), vec![2; 4]);

        assert!(cache.get(&(1, 0, 0)).is_some());
        assert!(cache.get(&(1, 0, 1)).is_none());
        assert!(cache.get(&(1, 0, 2)).is_some());
        assert_eq!(cache.total_bytes, 8);
    }
}
