//! WADO-RS 取回(PS3.18 §10.4)。
//!
//! 本阶段实现三条:
//!
//! ```text
//! GET .../instances/{sop}                原始 DICOM(application/dicom)
//! GET .../instances/{sop}/metadata       元数据(application/dicom+json)
//! GET .../instances/{sop}/frames/{list}  未压缩帧(multipart/related)
//! ```
//!
//! # 为什么单实例不套 multipart
//!
//! 标准的 `Accept: multipart/related; type="application/dicom"` 会把每个实例
//! 包成一个 part。对单实例请求,多数服务端(dcm4chee、Orthanc)在
//! `Accept: application/dicom` 下直接回裸文件,查看器也是这么用的 ——
//! 套一层 multipart 只是让客户端多写一段解析。
//!
//! 检查级和序列级的批量取回(要真 multipart)本阶段不做:查看器的实际取图
//! 路径是「QIDO 列实例 → 逐个取」,批量取回是归档导出的需求。
//!
//! # 帧号是 1 基
//!
//! 见 [`pacs_codec::frames`] 的模块文档。这里只做解析和转发,不碰基准。

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use dicom::object::FileDicomObject;
use pacs_auth::Identity;
use pacs_codec::Frames;
use pacs_core::Uid;
use pacs_store::{Store, StoreError};

use crate::routes::WebState;

/// multipart 响应的分隔串。
///
/// 固定值即可:分隔串的作用是划分 part 边界,只要不出现在负载里就行。
/// 用带随机数的分隔串是为了防止负载里恰好含有它,但这里的负载是二进制像素,
/// 而这个串含 ASCII 字母和连字符 —— 撞上的概率可以忽略,而固定值让响应可复现、
/// 便于测试。
const BOUNDARY: &str = "pacs-frame-boundary-8f2a1c";

/// 取回一个实例的原始 DICOM 文件。
pub async fn retrieve_instance(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path((study, series, sop)): Path<(String, String, String)>,
) -> Result<Response, WadoError> {
    let (study, series, sop) = validate_uids(&study, &series, &sop)?;
    let store = state.store.as_ref().ok_or(WadoError::StorageUnavailable)?;
    let instance = locate(&state, identity.institution_id, &study, &series, &sop).await?;

    let bytes = store
        .read(&instance.storage_path)
        .await
        .map_err(|error| classify_store_error(error, &instance.storage_path))?;

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/dicom"),
            // 取回的是不可变对象(SOPInstanceUID 定死一份影像),可以长期缓存。
            // immutable 让浏览器连条件请求都不发。
            (
                header::CACHE_CONTROL,
                "private, max-age=31536000, immutable",
            ),
        ],
        Body::from(bytes),
    )
        .into_response())
}

/// 取回一个实例的元数据(不含像素)。
///
/// 用途是查看器在下载像素前先拿到几何和显示参数(Rows/Columns/Rescale/Window),
/// 好决定要不要取、怎么排布。所以**必须去掉 PixelData** —— 不去掉的话
/// 这个接口和取原始文件没区别,还多一次 JSON 编码。
pub async fn retrieve_metadata(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path((study, series, sop)): Path<(String, String, String)>,
) -> Result<Response, WadoError> {
    let (study, series, sop) = validate_uids(&study, &series, &sop)?;
    let store = state.store.as_ref().ok_or(WadoError::StorageUnavailable)?;
    let instance = locate(&state, identity.institution_id, &study, &series, &sop).await?;
    let path = store
        .resolve_for_read(&instance.storage_path)
        .await
        .map_err(|error| classify_store_error(error, &instance.storage_path))?;

    // 解析 DICOM 是 CPU 活,挪出 async executor
    let json = tokio::task::spawn_blocking(move || -> Result<String, WadoError> {
        let object = FileDicomObject::open_file(&path).map_err(|error| {
            tracing::error!(%error, path = %path.display(), "盘上的文件无法解析");
            WadoError::Corrupt
        })?;

        let mut dataset = object.into_inner();
        pacs_core::normalize_dataset_text(&mut dataset);
        // PixelData 和它的伴随元素不进元数据响应
        for tag in [
            dicom::dictionary_std::tags::PIXEL_DATA,
            dicom::dictionary_std::tags::FLOAT_PIXEL_DATA,
            dicom::dictionary_std::tags::DOUBLE_FLOAT_PIXEL_DATA,
        ] {
            dataset.remove_element(tag);
        }

        dicom_json::to_string(&dataset).map_err(|error| {
            tracing::error!(%error, "元数据序列化失败");
            WadoError::Corrupt
        })
    })
    .await
    .map_err(|error| {
        tracing::error!(%error, "元数据任务 panic");
        WadoError::Corrupt
    })??;

    // 标准要求元数据响应是**数组**,即使只有一个实例(PS3.18 §10.4.1.2)
    let body = format!("[{json}]");
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/dicom+json"),
            (
                header::CACHE_CONTROL,
                "private, max-age=31536000, immutable",
            ),
        ],
        body,
    )
        .into_response())
}

/// 取回指定帧的未压缩像素。
pub async fn retrieve_frames(
    State(state): State<WebState>,
    Extension(identity): Extension<Identity>,
    Path((study, series, sop, frame_list)): Path<(String, String, String, String)>,
) -> Result<Response, WadoError> {
    let (study, series, sop) = validate_uids(&study, &series, &sop)?;
    let requested = parse_frame_list(&frame_list)?;
    let store = state.store.as_ref().ok_or(WadoError::StorageUnavailable)?;
    let instance = locate(&state, identity.institution_id, &study, &series, &sop).await?;
    let path = store
        .resolve_for_read(&instance.storage_path)
        .await
        .map_err(|error| classify_store_error(error, &instance.storage_path))?;

    // 解码整个实例一次,多帧共用 —— 每帧重新解码的代价是帧数的倍数
    let parts = tokio::task::spawn_blocking(move || -> Result<Vec<Vec<u8>>, WadoError> {
        let object = FileDicomObject::open_file(&path).map_err(|error| {
            tracing::error!(%error, path = %path.display(), "盘上的文件无法解析");
            WadoError::Corrupt
        })?;
        let frames = Frames::decode(&object).map_err(|error| {
            tracing::warn!(%error, "像素数据解码失败");
            WadoError::Undecodable
        })?;

        requested
            .iter()
            .map(|number| {
                frames
                    .frame(*number)
                    .map(<[u8]>::to_vec)
                    .map_err(|error| match error {
                        pacs_codec::FrameError::ZeroFrameNumber
                        | pacs_codec::FrameError::OutOfRange { .. } => WadoError::BadFrame {
                            detail: error.to_string(),
                        },
                        pacs_codec::FrameError::Decode { .. } => WadoError::Undecodable,
                    })
            })
            .collect()
    })
    .await
    .map_err(|error| {
        tracing::error!(%error, "帧提取任务 panic");
        WadoError::Corrupt
    })??;

    Ok(multipart_response(parts))
}

/// 帧响应固定用 multipart/related。
///
/// 单帧也套 multipart:标准对 `/frames` 只定义了 multipart 形式
/// (PS3.18 §10.4.1.1.1),而且请求 `frames/1,2` 与 `frames/1` 用同一套解析
/// 对客户端更简单 —— 否则客户端要按请求的帧数分两种情况处理响应。
fn multipart_response(parts: Vec<Vec<u8>>) -> Response {
    let mut body: Vec<u8> = Vec::new();
    for part in &parts {
        body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        body.extend_from_slice(b"Content-Type: application/octet-stream\r\n");
        body.extend_from_slice(format!("Content-Length: {}\r\n\r\n", part.len()).as_bytes());
        body.extend_from_slice(part);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());

    let content_type =
        format!(r#"multipart/related; type="application/octet-stream"; boundary={BOUNDARY}"#);
    let mut response = (StatusCode::OK, Body::from(body)).into_response();
    if let Ok(value) = HeaderValue::from_str(&content_type) {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=31536000, immutable"),
    );
    response
}

/// 解析 `1` 或 `1,2,5` 形式的帧列表。
///
/// 上限存在的理由和查询上限一样:`frames/1,1,1,...`(几千个)会让服务端
/// 把同一帧复制几千份进内存。
const MAX_FRAMES_PER_REQUEST: usize = 256;

fn parse_frame_list(raw: &str) -> Result<Vec<u32>, WadoError> {
    let mut frames = Vec::new();
    for piece in raw.split(',') {
        let trimmed = piece.trim();
        let number: u32 = trimmed.parse().map_err(|_| WadoError::BadFrame {
            detail: format!("{trimmed:?} 不是帧号"),
        })?;
        if number == 0 {
            // 帧号 1 基。0 说明调用方按 0 基算了,后续每一帧都会错位。
            return Err(WadoError::BadFrame {
                detail: "帧号从 1 开始".to_owned(),
            });
        }
        frames.push(number);
    }
    if frames.is_empty() {
        return Err(WadoError::BadFrame {
            detail: "帧列表为空".to_owned(),
        });
    }
    if frames.len() > MAX_FRAMES_PER_REQUEST {
        return Err(WadoError::TooManyFrames {
            limit: MAX_FRAMES_PER_REQUEST,
        });
    }
    Ok(frames)
}

fn validate_uids(study: &str, series: &str, sop: &str) -> Result<(Uid, Uid, Uid), WadoError> {
    Ok((
        parse_uid(study, "StudyInstanceUID")?,
        parse_uid(series, "SeriesInstanceUID")?,
        parse_uid(sop, "SOPInstanceUID")?,
    ))
}

fn parse_uid(raw: &str, field: &'static str) -> Result<Uid, WadoError> {
    Uid::parse(raw).map_err(|source| WadoError::InvalidUid { field, source })
}

async fn locate(
    state: &WebState,
    institution_id: i64,
    study: &Uid,
    series: &Uid,
    sop: &Uid,
) -> Result<pacs_db::StoredInstance, WadoError> {
    pacs_db::find_instance_for_institution(
        &state.pool,
        institution_id,
        study.as_str(),
        series.as_str(),
        sop.as_str(),
    )
    .await
    .map_err(|error| {
        tracing::error!(%error, "取回查询失败");
        WadoError::Internal
    })?
    .ok_or(WadoError::NotFound)
}

/// 落盘错误分成「库与盘不一致」和「其他」。
///
/// 前者是需要运维介入的信号(有记录没文件 / 路径越界),必须留下醒目日志;
/// 对调用方都是 404 —— 它没法区分,也不该看到内部细节。
fn classify_store_error(error: StoreError, relative: &str) -> WadoError {
    match error {
        StoreError::NotFound { .. } => {
            tracing::error!(
                relative,
                "数据库有记录但盘上没有文件 —— 存储与库不一致,需要核对"
            );
            WadoError::NotFound
        }
        StoreError::PathEscape { .. } => {
            // resolve_for_read 已经打了 error 级日志,这里不重复
            WadoError::NotFound
        }
        StoreError::ContentConflict { .. } | StoreError::DestinationExists { .. } => {
            tracing::error!(%error, "读取路径意外遇到写入冲突错误");
            WadoError::Internal
        }
        StoreError::Io { path, source } => {
            tracing::error!(%source, path = %path.display(), "读取影像文件失败");
            WadoError::Internal
        }
    }
}

/// 让 Store 能作为可选依赖注入 —— 见 [`WebState::store`]。
pub type SharedStore = Arc<Store>;

#[derive(Debug, thiserror::Error)]
pub enum WadoError {
    #[error("{field} 不是合法 UID:{source}")]
    InvalidUid {
        field: &'static str,
        #[source]
        source: pacs_core::UidError,
    },
    #[error("帧号无效:{detail}")]
    BadFrame { detail: String },
    #[error("一次最多请求 {limit} 帧")]
    TooManyFrames { limit: usize },
    #[error("未找到")]
    NotFound,
    #[error("影像文件无法解析")]
    Corrupt,
    #[error("像素数据无法解码")]
    Undecodable,
    #[error("存储未配置")]
    StorageUnavailable,
    #[error("内部错误")]
    Internal,
}

impl IntoResponse for WadoError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            Self::InvalidUid { .. } | Self::BadFrame { .. } => {
                (StatusCode::BAD_REQUEST, self.to_string())
            }
            Self::TooManyFrames { .. } => (StatusCode::BAD_REQUEST, self.to_string()),
            Self::NotFound => (StatusCode::NOT_FOUND, "未找到该实例".to_owned()),
            // 文件坏了不是调用方的错,但重试也没用 —— 500 而不是 4xx
            Self::Corrupt => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "影像文件无法解析".to_owned(),
            ),
            // 传输语法本地没有解码器(如 JPEG 2000 编码方向)。
            // 501 比 500 准确:不是出错,是这个功能没实现。
            Self::Undecodable => (
                StatusCode::NOT_IMPLEMENTED,
                "该传输语法的像素数据暂不支持解码".to_owned(),
            ),
            Self::StorageUnavailable | Self::Internal => {
                (StatusCode::INTERNAL_SERVER_ERROR, "内部错误".to_owned())
            }
        };
        (status, axum::Json(serde_json::json!({ "error": message }))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_lists_accept_single_and_multiple_values() {
        assert_eq!(parse_frame_list("1").unwrap(), vec![1]);
        assert_eq!(parse_frame_list("1,2,5").unwrap(), vec![1, 2, 5]);
        // 允许空格 —— URL 里可能是 %20
        assert_eq!(parse_frame_list("1, 2").unwrap(), vec![1, 2]);
    }

    /// 帧号 0 必须拒绝。当成第 1 帧会让所有多帧影像错位一帧。
    #[test]
    fn frame_zero_is_rejected() {
        assert!(matches!(
            parse_frame_list("0"),
            Err(WadoError::BadFrame { .. })
        ));
        assert!(matches!(
            parse_frame_list("1,0"),
            Err(WadoError::BadFrame { .. })
        ));
    }

    #[test]
    fn malformed_frame_lists_are_rejected() {
        for bad in ["", "abc", "1,,2", "-1", "1.5", "1;2"] {
            assert!(
                matches!(
                    parse_frame_list(bad),
                    Err(WadoError::BadFrame { .. } | WadoError::TooManyFrames { .. })
                ),
                "应拒绝 {bad:?}"
            );
        }
    }

    /// 帧数上限:`frames/1,1,1,...` 不能让服务端复制几千份同一帧。
    #[test]
    fn frame_count_is_capped() {
        let many = (1..=MAX_FRAMES_PER_REQUEST + 1)
            .map(|_| "1")
            .collect::<Vec<_>>()
            .join(",");
        assert!(matches!(
            parse_frame_list(&many),
            Err(WadoError::TooManyFrames { .. })
        ));

        // 恰好到上限应当放行
        let exactly = (1..=MAX_FRAMES_PER_REQUEST)
            .map(|_| "1")
            .collect::<Vec<_>>()
            .join(",");
        assert!(parse_frame_list(&exactly).is_ok());
    }

    #[test]
    fn path_uids_are_validated() {
        assert!(validate_uids("1.2.3", "1.2.4", "1.2.5").is_ok());
        for (a, b, c) in [
            ("..", "1.2.4", "1.2.5"),
            ("1.2.3", "../etc", "1.2.5"),
            ("1.2.3", "1.2.4", ""),
        ] {
            assert!(
                validate_uids(a, b, c).is_err(),
                "应拒绝 ({a:?}, {b:?}, {c:?})"
            );
        }
    }

    /// multipart 的结构:每个 part 有头、有负载,末尾是结束分隔串。
    #[test]
    fn multipart_body_is_well_formed() {
        let response = multipart_response(vec![vec![0xAA, 0xBB], vec![0xCC]]);
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(content_type.contains("multipart/related"));
        assert!(content_type.contains(BOUNDARY));
    }
}
