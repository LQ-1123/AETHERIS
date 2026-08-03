//! 查看器状态管理
//!
//! # 设计
//!
//! - **句柄索引已打开的实例**，用递增整数作为句柄（不是 UUID，因为本地应用）
//! - **LRU 缓存帧数据**，上限 512 MiB，用双端队列跟踪访问顺序
//! - **锁粒度**：整个状态一把锁，因为操作都很快（查表、插入缓存）
//! - **错误处理**：锁中毒时继续运行（日志警告），单个坏实例不影响其他实例

use pacs_codec::{GrayLut, Pipeline};
use pacs_core::spacing::PixelSpacing;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use thiserror::Error;

/// 实例句柄，递增分配
pub type InstanceHandle = u64;

/// 帧缓存键：(实例句柄, 帧号)
type FrameKey = (InstanceHandle, u32);

/// 帧缓存上限：512 MiB
const FRAME_CACHE_LIMIT: usize = 512 * 1024 * 1024;

/// 查看器状态，所有打开的实例和缓存
#[derive(Clone)]
pub struct ViewerState {
    inner: Arc<Mutex<ViewerStateInner>>,
}

struct ViewerStateInner {
    /// 下一个分配的句柄
    next_handle: InstanceHandle,
    /// 已打开的实例
    instances: HashMap<InstanceHandle, LoadedInstance>,
    /// 帧数据缓存
    frame_cache: FrameCache,
}

/// 已加载的实例
#[allow(dead_code)]
struct LoadedInstance {
    path: PathBuf,
    pipeline: Pipeline,
    spacing: PixelSpacing,
    rows: u32,
    cols: u32,
    bits_allocated: u16,
    frame_count: u32,
}

/// LRU 帧缓存
struct FrameCache {
    /// 缓存的帧数据
    data: HashMap<FrameKey, Vec<u8>>,
    /// LRU 队列，最近访问的在队尾
    access_queue: VecDeque<FrameKey>,
    /// 当前占用字节数
    total_bytes: usize,
}

/// 查看器错误
#[derive(Error, Debug)]
pub enum ViewerError {
    #[error("未知的实例句柄: {0}")]
    UnknownHandle(InstanceHandle),

    #[error("I/O 错误: {0}")]
    Io(#[from] std::io::Error),

    #[error("DICOM 解析错误: {0}")]
    Dicom(String),

    #[error("帧号越界: {frame} >= {total}")]
    FrameOutOfBounds { frame: u32, total: u32 },
}

/// 发给前端的元数据
#[derive(Serialize, Deserialize, Debug)]
pub struct DisplayMetadata {
    pub handle: InstanceHandle,
    pub rows: u32,
    pub cols: u32,
    pub frame_count: u32,
    pub bits_allocated: u16,
    pub window_presets: Vec<WindowPreset>,
    pub spacing: SpacingInfo,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct WindowPreset {
    pub center: f64,
    pub width: f64,
    pub explanation: Option<String>,
    pub function: String, // "LINEAR" | "SIGMOID"
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SpacingInfo {
    pub confidence: String, // "accurate" | "detector" | "none"
    pub row_mm: Option<f64>,
    pub col_mm: Option<f64>,
    pub aspect_ratio: f64,
}

impl ViewerState {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(ViewerStateInner {
                next_handle: 1,
                instances: HashMap::new(),
                frame_cache: FrameCache::new(),
            })),
        }
    }

    /// 从锁中毒恢复
    fn lock(&self) -> std::sync::MutexGuard<'_, ViewerStateInner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl FrameCache {
    fn new() -> Self {
        Self {
            data: HashMap::new(),
            access_queue: VecDeque::new(),
            total_bytes: 0,
        }
    }

    /// 获取帧，更新 LRU
    fn get(&mut self, key: &FrameKey) -> Option<&Vec<u8>> {
        if self.data.contains_key(key) {
            // 移到队尾
            self.access_queue.retain(|k| k != key);
            self.access_queue.push_back(*key);
            self.data.get(key)
        } else {
            None
        }
    }

    /// 插入帧，必要时淘汰
    fn insert(&mut self, key: FrameKey, data: Vec<u8>) {
        let size = data.len();

        // 替换已有的
        if let Some(old) = self.data.insert(key, data) {
            self.total_bytes -= old.len();
            self.access_queue.retain(|k| k != &key);
        }

        self.total_bytes += size;
        self.access_queue.push_back(key);

        // LRU 淘汰
        while self.total_bytes > FRAME_CACHE_LIMIT && !self.access_queue.is_empty() {
            if let Some(evict_key) = self.access_queue.pop_front() {
                if let Some(evicted) = self.data.remove(&evict_key) {
                    self.total_bytes -= evicted.len();
                }
            }
        }
    }

    /// 清除某个实例的所有帧
    fn remove_instance(&mut self, handle: InstanceHandle) {
        self.data.retain(|k, v| {
            if k.0 == handle {
                self.total_bytes -= v.len();
                false
            } else {
                true
            }
        });
        self.access_queue.retain(|k| k.0 != handle);
    }
}

impl Default for ViewerState {
    fn default() -> Self {
        Self::new()
    }
}

impl ViewerState {
    /// 打开一个 DICOM 文件
    pub fn open(&self, path: PathBuf) -> Result<DisplayMetadata, ViewerError> {
        use dicom::object::open_file;
        use pacs_codec::Pipeline;
        use pacs_core::spacing::resolve;

        // 解析 DICOM（这是 I/O 和 CPU 密集操作，不持锁）
        let obj = open_file(&path).map_err(|e| ViewerError::Dicom(e.to_string()))?;

        // 提取显示管线
        let pipeline = Pipeline::from_object(&obj);

        // 提取间距
        let spacing = resolve(&obj);

        // 提取几何信息
        let rows = obj
            .element_by_name("Rows")
            .ok()
            .and_then(|e| e.to_int::<u32>().ok())
            .ok_or_else(|| ViewerError::Dicom("缺少 Rows".to_string()))?;

        let cols = obj
            .element_by_name("Columns")
            .ok()
            .and_then(|e| e.to_int::<u32>().ok())
            .ok_or_else(|| ViewerError::Dicom("缺少 Columns".to_string()))?;

        let bits_allocated = obj
            .element_by_name("BitsAllocated")
            .ok()
            .and_then(|e| e.to_int::<u16>().ok())
            .unwrap_or(16);

        let frame_count = obj
            .element_by_name("NumberOfFrames")
            .ok()
            .and_then(|e| e.to_int::<u32>().ok())
            .unwrap_or(1);

        // 分配句柄并存储
        let mut inner = self.lock();
        let handle = inner.next_handle;
        inner.next_handle += 1;

        inner.instances.insert(
            handle,
            LoadedInstance {
                path: path.clone(),
                pipeline: pipeline.clone(),
                spacing: spacing.clone(),
                rows,
                cols,
                bits_allocated,
                frame_count,
            },
        );

        Ok(DisplayMetadata {
            handle,
            rows,
            cols,
            frame_count,
            bits_allocated,
            window_presets: pipeline
                .windows
                .iter()
                .map(|w| WindowPreset {
                    center: w.center,
                    width: w.width,
                    explanation: w.explanation.clone(),
                    function: match w.function {
                        pacs_codec::VoiFunction::Linear => "LINEAR".to_string(),
                        pacs_codec::VoiFunction::LinearExact => "LINEAR_EXACT".to_string(),
                        pacs_codec::VoiFunction::Sigmoid => "SIGMOID".to_string(),
                    },
                })
                .collect(),
            spacing: match spacing {
                PixelSpacing::Physical(s) => {
                    let confidence_str = match s.confidence() {
                        pacs_core::spacing::Confidence::Calibrated => "accurate",
                        pacs_core::spacing::Confidence::DetectorPlane => "detector",
                        pacs_core::spacing::Confidence::None => "none",
                    };
                    SpacingInfo {
                        confidence: confidence_str.to_string(),
                        row_mm: Some(s.row_mm),
                        col_mm: Some(s.column_mm),
                        aspect_ratio: s.column_mm / s.row_mm,
                    }
                }
                PixelSpacing::PixelsOnly { aspect_ratio, .. } => SpacingInfo {
                    confidence: "none".to_string(),
                    row_mm: None,
                    col_mm: None,
                    aspect_ratio: aspect_ratio.row_over_column,
                },
            },
        })
    }

    /// 关闭实例
    pub fn close(&self, handle: InstanceHandle) -> Result<(), ViewerError> {
        let mut inner = self.lock();
        inner
            .instances
            .remove(&handle)
            .ok_or(ViewerError::UnknownHandle(handle))?;
        inner.frame_cache.remove_instance(handle);
        Ok(())
    }

    /// 获取帧数据（原始像素）
    pub fn get_frame_bytes(
        &self,
        handle: InstanceHandle,
        frame_index: u32,
    ) -> Result<Vec<u8>, ViewerError> {
        let key = (handle, frame_index);

        // 先查缓存
        {
            let mut inner = self.lock();
            if let Some(cached) = inner.frame_cache.get(&key) {
                return Ok(cached.clone());
            }
        }

        // 缓存未命中，解码（不持锁）
        let path = {
            let inner = self.lock();
            let inst = inner
                .instances
                .get(&handle)
                .ok_or(ViewerError::UnknownHandle(handle))?;

            if frame_index >= inst.frame_count {
                return Err(ViewerError::FrameOutOfBounds {
                    frame: frame_index,
                    total: inst.frame_count,
                });
            }

            inst.path.clone()
        };

        use dicom::object::open_file;
        use pacs_codec::Frames;

        let obj = open_file(&path).map_err(|e| ViewerError::Dicom(e.to_string()))?;
        let frames = Frames::decode(&obj).map_err(|e| ViewerError::Dicom(e.to_string()))?;
        // Frames::frame() 使用 1-based 索引
        let frame_data = frames
            .frame(frame_index + 1)
            .map_err(|e| ViewerError::Dicom(e.to_string()))?;

        // 插入缓存
        {
            let mut inner = self.lock();
            inner.frame_cache.insert(key, frame_data.to_vec());
        }

        Ok(frame_data.to_vec())
    }

    /// 生成查找表
    pub fn build_lut(
        &self,
        handle: InstanceHandle,
        window_center: Option<f64>,
        window_width: Option<f64>,
    ) -> Result<Vec<u8>, ViewerError> {
        let inner = self.lock();
        let inst = inner
            .instances
            .get(&handle)
            .ok_or(ViewerError::UnknownHandle(handle))?;

        let window = window_center.and_then(|c| {
            window_width.map(|w| pacs_codec::Window {
                center: c,
                width: w,
                explanation: Some("custom".to_string()),
                function: pacs_codec::VoiFunction::Linear,
            })
        });

        let lut = GrayLut::build(&inst.pipeline, window.as_ref(), Some(inst.bits_allocated));
        Ok(lut.table)
    }
}
