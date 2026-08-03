//! Local DICOM series state for the desktop viewer.

use dicom::core::Tag;
use dicom::dictionary_std::tags;
use dicom::object::{DefaultDicomObject, open_file};
use pacs_codec::{Frames, GrayLut, Photometric, Pipeline, VoiFunction};
use pacs_core::geometry::{SliceInput, sort_slices};
use pacs_core::spacing::{Confidence, PixelSpacing, Source, resolve};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use thiserror::Error;

pub type SeriesHandle = u64;
type FrameKey = (SeriesHandle, u32);

const FRAME_CACHE_LIMIT: usize = 512 * 1024 * 1024;
const PREFETCH_RADIUS: u32 = 2;

#[derive(Clone)]
pub struct ViewerState {
    inner: Arc<Mutex<ViewerStateInner>>,
}

struct ViewerStateInner {
    next_handle: SeriesHandle,
    series: HashMap<SeriesHandle, LoadedSeries>,
    frame_cache: FrameCache,
}

struct LoadedSeries {
    frames: Vec<LoadedFrame>,
    /// 远程序列的下载目录。句柄关闭时随 `LoadedSeries` 一起删除。
    _temporary_directory: Option<tempfile::TempDir>,
}

#[derive(Clone)]
struct LoadedFrame {
    path: PathBuf,
    source_frame: u32,
    pipeline: Pipeline,
    rows: u32,
    cols: u32,
    bits_allocated: u16,
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
    pub frames: Vec<FrameMetadata>,
    pub warnings: Vec<String>,
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

        let mut parsed = paths
            .into_iter()
            .map(parse_file)
            .collect::<Result<Vec<_>, _>>()?;
        let mut warnings = Vec::new();

        if parsed.len() > 1 {
            validate_multi_file_series(&parsed)?;
            let slices = parsed
                .iter()
                .map(|file| SliceInput {
                    position: file.position.as_deref().unwrap_or(&[]),
                    orientation: file.orientation.as_deref().unwrap_or(&[]),
                })
                .collect::<Vec<_>>();
            let sorted = sort_slices(&slices).map_err(|error| {
                ViewerError::InvalidSeries(format!(
                    "无法按 ImagePositionPatient/ImageOrientationPatient 安全排序: {error}"
                ))
            })?;
            if sorted.duplicate_position_groups > 0 {
                warnings.push(format!(
                    "序列包含 {} 组重复切片位置，请核对重建内容",
                    sorted.duplicate_position_groups
                ));
            }

            let mut slots = parsed.into_iter().map(Some).collect::<Vec<_>>();
            parsed = sorted
                .order
                .into_iter()
                .map(|index| slots[index].take().expect("排序索引必须唯一且有效"))
                .collect();
        }

        let first = parsed.first().expect("已检查输入非空");
        let patient = PatientStudyInfo {
            patient_name: first.patient_name.clone(),
            patient_id: first.patient_id.clone(),
            study_date: first.study_date.clone(),
            accession_number: first.accession_number.clone(),
            modality: first.modality.clone(),
            study_description: first.study_description.clone(),
            series_description: first.series_description.clone(),
        };
        let study_uid = first.study_uid.clone();
        let series_uid = first.series_uid.clone();

        let mut loaded_frames = Vec::new();
        let mut frame_metadata = Vec::new();
        for (file_index, file) in parsed.into_iter().enumerate() {
            for source_frame in 1..=file.frame_count {
                let logical_index = u32::try_from(loaded_frames.len())
                    .map_err(|_| ViewerError::Unsupported("序列帧数超过支持范围".to_owned()))?;
                let frame_key = file.sop_uid.as_ref().map_or_else(
                    || format!("local-{file_index}#{source_frame}"),
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
                loaded_frames.push(LoadedFrame {
                    path: file.path.clone(),
                    source_frame,
                    pipeline: file.pipeline.clone(),
                    rows: file.rows,
                    cols: file.cols,
                    bits_allocated: file.bits_allocated,
                });
            }
        }

        let mut inner = self.lock();
        let handle = inner.next_handle;
        inner.next_handle = inner
            .next_handle
            .checked_add(1)
            .ok_or_else(|| ViewerError::Unsupported("打开的序列句柄已经耗尽".to_owned()))?;
        inner.series.insert(
            handle,
            LoadedSeries {
                frames: loaded_frames,
                _temporary_directory: temporary_directory,
            },
        );

        Ok(SeriesMetadata {
            handle,
            patient,
            study_uid,
            series_uid,
            frames: frame_metadata,
            warnings,
        })
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
        logical_frame: u32,
    ) -> Result<Vec<u8>, ViewerError> {
        let key = (handle, logical_frame);
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
            let requested = series
                .frames
                .get(logical_frame as usize)
                .ok_or(ViewerError::FrameOutOfBounds {
                    frame: logical_frame,
                    total: series.frames.len() as u32,
                })?
                .clone();
            let neighbours = series
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

        let object = open_file(&requested.path).map_err(|e| ViewerError::Dicom(e.to_string()))?;
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
            inner.frame_cache.insert((handle, logical), bytes);
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
        let frame =
            series
                .frames
                .get(logical_frame as usize)
                .ok_or(ViewerError::FrameOutOfBounds {
                    frame: logical_frame,
                    total: series.frames.len() as u32,
                })?;
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
}

impl Default for ViewerState {
    fn default() -> Self {
        Self::new()
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

fn parse_file(path: PathBuf) -> Result<ParsedFile, ViewerError> {
    let object = open_file(&path)
        .map_err(|error| ViewerError::Dicom(format!("{}: {error}", path.display())))?;
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
    let raw = object.get(tag)?.to_str().ok()?;
    let trimmed =
        raw.trim_matches(|character: char| character == '\0' || character.is_whitespace());
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
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
            state.get_frame_bytes(metadata.handle, 0).unwrap().len(),
            4 * 4 * 2
        );
        assert_eq!(
            state
                .build_lut(metadata.handle, 0, -600.0, 1500.0, "LINEAR")
                .unwrap()
                .len(),
            65_536
        );
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
            state.get_frame_bytes(metadata.handle, 1).unwrap().len(),
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
            state.get_frame_bytes(metadata.handle, 0).unwrap().len(),
            4 * 4
        );
        assert_eq!(
            state
                .build_lut(metadata.handle, 0, 128.0, 256.0, "LINEAR")
                .unwrap()
                .len(),
            256
        );
    }

    #[test]
    fn frame_cache_evicts_the_least_recently_used_entry() {
        let mut cache = FrameCache::new(8);
        cache.insert((1, 0), vec![0; 4]);
        cache.insert((1, 1), vec![1; 4]);
        assert!(cache.get(&(1, 0)).is_some());
        cache.insert((1, 2), vec![2; 4]);

        assert!(cache.get(&(1, 0)).is_some());
        assert!(cache.get(&(1, 1)).is_none());
        assert!(cache.get(&(1, 2)).is_some());
        assert_eq!(cache.total_bytes, 8);
    }
}
