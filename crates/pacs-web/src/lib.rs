//! DICOMweb HTTP 接口(axum)。
//!
//! 实现 QIDO-RS、WADO-RS(含 `/frames`)和 STOW-RS。
//! 全部端点经 [`pacs_auth::middleware`] 鉴权。
//!
//! # 当前不做的能力
//!
//! - **服务端渲染(`/rendered`、`/thumbnail`)**:查看器本来就要在本地解码
//!   (要脱离服务端打开本地文件),服务端再实现一套是重复。窗宽窗位是交互式的,
//!   服务端渲染反而丢掉了交互性。
//!
//! 这两项的取舍记录在此,避免后续误以为是漏做。

pub mod annotations;
pub mod ingest;
pub mod qido;
pub mod routes;
pub mod stow;
pub mod transfers;
pub mod transformations;
pub mod wado;
pub mod worklist;

pub use annotations::annotation_routes;
pub use routes::{WebState, dicomweb_routes};
pub use transfers::{start_transfer_worker, transfer_routes};
pub use transformations::{dicom_transformation_routes, start_transform_worker};
pub use wado::WadoError;
pub use worklist::worklist_routes;
