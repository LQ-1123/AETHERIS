//! DICOMweb HTTP 接口(axum)。
//!
//! 本阶段实现**读取侧**:QIDO-RS(查询)与 WADO-RS(取回,含 `/frames`)。
//! 全部端点经 [`pacs_auth::middleware`] 鉴权。
//!
//! # 本阶段不做的两件事
//!
//! - **STOW-RS(上传)**:multipart 解析、UID 冲突处理、部分成功的 207 响应
//!   各有自己的复杂度,和读取侧没有共用逻辑。读取侧先跑通,查看器就能开工。
//! - **服务端渲染(`/rendered`、`/thumbnail`)**:查看器本来就要在本地解码
//!   (要脱离服务端打开本地文件),服务端再实现一套是重复。窗宽窗位是交互式的,
//!   服务端渲染反而丢掉了交互性。
//!
//! 这两项的取舍记录在此,避免后续误以为是漏做。

pub mod qido;
pub mod routes;
pub mod transformations;
pub mod wado;
pub mod worklist;

pub use routes::{WebState, dicomweb_routes};
pub use transformations::{dicom_transformation_routes, start_transform_worker};
pub use wado::WadoError;
pub use worklist::worklist_routes;
