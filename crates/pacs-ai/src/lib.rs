//! Versioned protocol and process adapter for local AI segmentation workers.
//!
//! Model runtimes stay outside the PACS process. This crate only exchanges a
//! local, short-lived manifest with a worker and validates its binary masks.

use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;
use uuid::Uuid;

pub const WORKER_PROTOCOL_VERSION: u32 = 1;
const MAX_RESULT_BYTES: u64 = 256 * 1024 * 1024;
const MAX_MASK_BYTES: usize = 64 * 1024 * 1024;
const MAX_INPUT_VOXELS: u64 = 320_000_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelDescriptor {
    pub id: String,
    pub display_name: String,
    pub version: String,
    pub description: String,
    pub supported_modalities: Vec<String>,
    pub labels: Vec<LabelDescriptor>,
    pub estimated_peak_memory_mb: u32,
    pub model_download_mb: u32,
    pub device: Option<String>,
    pub available: bool,
    pub unavailable_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LabelDescriptor {
    pub id: String,
    pub display_name: String,
    pub color: [u8; 3],
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SeriesInput {
    pub modality: Option<String>,
    pub rows: u32,
    pub cols: u32,
    pub slices: Vec<SliceInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SliceInput {
    pub source_index: u32,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SegmentationRequest {
    pub protocol_version: u32,
    pub job_id: Uuid,
    pub model_id: String,
    pub series: SeriesInput,
}

impl SegmentationRequest {
    pub fn new(job_id: Uuid, model_id: impl Into<String>, series: SeriesInput) -> Self {
        Self {
            protocol_version: WORKER_PROTOCOL_VERSION,
            job_id,
            model_id: model_id.into(),
            series,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SegmentationResult {
    pub protocol_version: u32,
    pub job_id: Uuid,
    pub model_id: String,
    pub elapsed_ms: u64,
    pub segments: Vec<SegmentResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SegmentResult {
    pub label: LabelDescriptor,
    pub voxel_count: u64,
    pub masks: Vec<MaskSlice>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MaskSlice {
    pub source_index: u32,
    pub rows: u32,
    pub cols: u32,
    pub encoding: String,
    pub data_base64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SegmentationProgress {
    pub job_id: Uuid,
    pub stage: String,
    pub completed: u32,
    pub total: u32,
    pub message: String,
}

#[derive(Debug, Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn reset(&self) {
        self.0.store(false, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

pub trait SegmentationEngine: Send + Sync {
    fn models(&self) -> Result<Vec<ModelDescriptor>, AiError>;

    fn segment(
        &self,
        request: &SegmentationRequest,
        cancellation: &CancellationToken,
        progress: &mut dyn FnMut(SegmentationProgress),
    ) -> Result<SegmentationResult, AiError>;
}

#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub python: PathBuf,
    pub script: PathBuf,
}

#[derive(Debug, Clone)]
pub struct LocalWorker {
    config: WorkerConfig,
}

impl LocalWorker {
    pub fn new(config: WorkerConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &WorkerConfig {
        &self.config
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.config.python);
        command.arg(&self.config.script);
        command
    }
}

impl SegmentationEngine for LocalWorker {
    fn models(&self) -> Result<Vec<ModelDescriptor>, AiError> {
        if !self.config.script.is_file() {
            return Err(AiError::Unavailable("AI Worker 文件不存在".to_owned()));
        }
        let output = self
            .command()
            .arg("--models")
            .stderr(Stdio::null())
            .output()
            .map_err(|error| AiError::Unavailable(format!("无法启动 AI Worker: {error}")))?;
        if !output.status.success() {
            return Err(AiError::Unavailable(
                "AI Worker 环境检查失败，请先安装本地推理依赖".to_owned(),
            ));
        }
        let catalog: ModelCatalog = last_json_line(&output.stdout)
            .ok_or_else(|| AiError::Protocol("AI Worker 未返回有效的模型列表".to_owned()))?;
        validate_catalog(catalog)
    }

    fn segment(
        &self,
        request: &SegmentationRequest,
        cancellation: &CancellationToken,
        progress: &mut dyn FnMut(SegmentationProgress),
    ) -> Result<SegmentationResult, AiError> {
        validate_request(request)?;
        if !self.config.script.is_file() {
            return Err(AiError::Unavailable("AI Worker 文件不存在".to_owned()));
        }

        let directory = tempfile::Builder::new()
            .prefix("remote-pacs-ai-")
            .tempdir()?;
        let request_path = directory.path().join("request.json");
        let result_path = directory.path().join("result.json");
        fs::write(&request_path, serde_json::to_vec(request)?)?;

        let mut child = self
            .command()
            .arg("--request")
            .arg(&request_path)
            .arg("--output")
            .arg(&result_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| AiError::Unavailable(format!("无法启动 AI Worker: {error}")))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AiError::Protocol("无法读取 AI Worker 进度".to_owned()))?;
        let (sender, receiver) = mpsc::channel();
        let reader = thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if line.len() <= 64 * 1024 {
                    let _ = sender.send(line);
                }
            }
        });

        let mut worker_error = None;
        let status = wait_for_worker(
            &mut child,
            request.job_id,
            cancellation,
            &receiver,
            progress,
            &mut worker_error,
        )?;
        let _ = reader.join();
        drain_events(&receiver, request.job_id, progress, &mut worker_error);

        if cancellation.is_cancelled() {
            return Err(AiError::Cancelled);
        }
        if !status.success() {
            return Err(AiError::WorkerFailed(
                worker_error.unwrap_or_else(|| "AI Worker 执行失败".to_owned()),
            ));
        }

        let metadata = fs::metadata(&result_path)
            .map_err(|_| AiError::Protocol("AI Worker 没有生成分割结果".to_owned()))?;
        if metadata.len() == 0 || metadata.len() > MAX_RESULT_BYTES {
            return Err(AiError::Protocol("AI 分割结果大小无效".to_owned()));
        }
        let result: SegmentationResult = serde_json::from_slice(&fs::read(result_path)?)?;
        validate_result(request, result)
    }
}

fn wait_for_worker(
    child: &mut Child,
    expected_job_id: Uuid,
    cancellation: &CancellationToken,
    receiver: &mpsc::Receiver<String>,
    progress: &mut dyn FnMut(SegmentationProgress),
    worker_error: &mut Option<String>,
) -> Result<std::process::ExitStatus, AiError> {
    let started = Instant::now();
    loop {
        drain_events(receiver, expected_job_id, progress, worker_error);
        if cancellation.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(AiError::Cancelled);
        }
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if started.elapsed() > Duration::from_secs(60 * 30) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(AiError::WorkerFailed(
                "AI 分割超过 30 分钟，任务已终止".to_owned(),
            ));
        }
        thread::sleep(Duration::from_millis(80));
    }
}

fn drain_events(
    receiver: &mpsc::Receiver<String>,
    expected_job_id: Uuid,
    progress: &mut dyn FnMut(SegmentationProgress),
    worker_error: &mut Option<String>,
) {
    while let Ok(line) = receiver.try_recv() {
        let Ok(event) = serde_json::from_str::<WorkerEvent>(&line) else {
            continue;
        };
        match event {
            WorkerEvent::Progress { progress: update } => {
                if expected_job_id.is_nil() || update.job_id == expected_job_id {
                    progress(update);
                }
            }
            WorkerEvent::Error { message } => *worker_error = Some(message),
        }
    }
}

#[derive(Debug, Deserialize)]
struct ModelCatalog {
    protocol_version: u32,
    models: Vec<ModelDescriptor>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WorkerEvent {
    Progress {
        #[serde(flatten)]
        progress: SegmentationProgress,
    },
    Error {
        message: String,
    },
}

fn last_json_line<T: DeserializeOwned>(bytes: &[u8]) -> Option<T> {
    String::from_utf8_lossy(bytes)
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str(line).ok())
}

fn validate_catalog(catalog: ModelCatalog) -> Result<Vec<ModelDescriptor>, AiError> {
    if catalog.protocol_version != WORKER_PROTOCOL_VERSION {
        return Err(AiError::Protocol(format!(
            "AI Worker 协议版本不兼容: {}",
            catalog.protocol_version
        )));
    }
    let mut ids = HashSet::new();
    for model in &catalog.models {
        if model.id.trim().is_empty()
            || model.display_name.trim().is_empty()
            || !ids.insert(model.id.as_str())
        {
            return Err(AiError::Protocol("AI Worker 模型定义无效".to_owned()));
        }
    }
    Ok(catalog.models)
}

fn validate_request(request: &SegmentationRequest) -> Result<(), AiError> {
    let voxel_count = u64::from(request.series.rows)
        .checked_mul(u64::from(request.series.cols))
        .and_then(|pixels| pixels.checked_mul(request.series.slices.len() as u64));
    if request.protocol_version != WORKER_PROTOCOL_VERSION
        || request.model_id.trim().is_empty()
        || request.series.rows == 0
        || request.series.cols == 0
        || request.series.slices.len() < 2
        || request.series.slices.len() > 2_048
        || voxel_count.is_none_or(|count| count > MAX_INPUT_VOXELS)
    {
        return Err(AiError::InvalidRequest("AI 分割请求无效".to_owned()));
    }
    let mut source_indices = HashSet::new();
    for slice in &request.series.slices {
        if !slice.path.is_file() || !source_indices.insert(slice.source_index) {
            return Err(AiError::InvalidRequest(
                "AI 输入切片不存在或索引重复".to_owned(),
            ));
        }
    }
    Ok(())
}

fn validate_result(
    request: &SegmentationRequest,
    result: SegmentationResult,
) -> Result<SegmentationResult, AiError> {
    if result.protocol_version != WORKER_PROTOCOL_VERSION
        || result.job_id != request.job_id
        || result.model_id != request.model_id
        || result.segments.is_empty()
    {
        return Err(AiError::Protocol("AI 分割结果与请求不匹配".to_owned()));
    }

    let expected_pixels = usize::try_from(request.series.rows)
        .ok()
        .and_then(|rows| {
            usize::try_from(request.series.cols)
                .ok()
                .and_then(|cols| rows.checked_mul(cols))
        })
        .ok_or_else(|| AiError::Protocol("AI Mask 尺寸溢出".to_owned()))?;
    let expected_indices = request
        .series
        .slices
        .iter()
        .map(|slice| slice.source_index)
        .collect::<HashSet<_>>();
    let mut label_ids = HashSet::new();

    for segment in &result.segments {
        if segment.label.id.trim().is_empty()
            || segment.label.display_name.trim().is_empty()
            || !label_ids.insert(segment.label.id.as_str())
            || segment.masks.len() != expected_indices.len()
        {
            return Err(AiError::Protocol("AI Segment 定义无效".to_owned()));
        }
        let mut mask_indices = HashSet::new();
        let mut voxel_count = 0_u64;
        for mask in &segment.masks {
            if mask.rows != request.series.rows
                || mask.cols != request.series.cols
                || mask.encoding != "rle-v1"
                || !expected_indices.contains(&mask.source_index)
                || !mask_indices.insert(mask.source_index)
            {
                return Err(AiError::Protocol("AI Mask 来源或尺寸无效".to_owned()));
            }
            let bytes = STANDARD
                .decode(&mask.data_base64)
                .map_err(|_| AiError::Protocol("AI Mask Base64 无效".to_owned()))?;
            voxel_count = voxel_count
                .checked_add(validate_rle(&bytes, expected_pixels)?)
                .ok_or_else(|| AiError::Protocol("AI Mask 体素计数溢出".to_owned()))?;
        }
        if voxel_count != segment.voxel_count {
            return Err(AiError::Protocol("AI Mask 体素计数不一致".to_owned()));
        }
    }
    Ok(result)
}

fn validate_rle(bytes: &[u8], expected_pixels: usize) -> Result<u64, AiError> {
    if bytes.is_empty() || !bytes.len().is_multiple_of(4) || bytes.len() > MAX_MASK_BYTES {
        return Err(AiError::Protocol("AI Mask RLE 长度无效".to_owned()));
    }
    let mut total = 0usize;
    let mut foreground = 0_u64;
    for (index, chunk) in bytes.chunks_exact(4).enumerate() {
        let run = u32::from_le_bytes(chunk.try_into().expect("four byte chunk")) as usize;
        if run == 0 && index != 0 {
            return Err(AiError::Protocol("AI Mask RLE 包含空游程".to_owned()));
        }
        total = total
            .checked_add(run)
            .ok_or_else(|| AiError::Protocol("AI Mask RLE 溢出".to_owned()))?;
        if total > expected_pixels {
            return Err(AiError::Protocol("AI Mask RLE 超出图像范围".to_owned()));
        }
        if index % 2 == 1 {
            foreground = foreground
                .checked_add(run as u64)
                .ok_or_else(|| AiError::Protocol("AI Mask 体素计数溢出".to_owned()))?;
        }
    }
    if total != expected_pixels {
        return Err(AiError::Protocol("AI Mask RLE 像素数不匹配".to_owned()));
    }
    Ok(foreground)
}

#[derive(Debug, Error)]
pub enum AiError {
    #[error("AI Worker 不可用: {0}")]
    Unavailable(String),
    #[error("{0}")]
    InvalidRequest(String),
    #[error("AI Worker 协议错误: {0}")]
    Protocol(String),
    #[error("{0}")]
    WorkerFailed(String),
    #[error("已取消 AI 分割")]
    Cancelled,
    #[error("AI 临时文件 I/O 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("AI JSON 错误: {0}")]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_rle_and_counts_foreground_voxels() {
        let bytes = [3_u32, 2, 3]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>();
        assert_eq!(validate_rle(&bytes, 8).unwrap(), 2);
        assert!(validate_rle(&bytes, 9).is_err());
    }

    #[test]
    fn permits_mask_starting_with_foreground() {
        let bytes = [0_u32, 4]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>();
        assert_eq!(validate_rle(&bytes, 4).unwrap(), 4);
    }

    #[test]
    fn reads_catalog_after_worker_log_lines() {
        let bytes = br#"worker log
{"protocol_version":1,"models":[]}
"#;
        let catalog: ModelCatalog = last_json_line(bytes).unwrap();
        assert_eq!(catalog.protocol_version, WORKER_PROTOCOL_VERSION);
    }
}
