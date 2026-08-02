//! 像素解码、缩略图、帧提取。
//!
//! CPU 密集,必须经 `spawn_blocking`/rayon 调用,不得直接跑在 async executor 上。
//! 显示管线顺序:存储值 → Rescale → VOI(窗宽窗位) → Photometric 反转 → 输出。
//! `MONOCHROME1` 灰度反转、多值 WindowCenter、SIGMOID VOI 见计划"查看器陷阱"。
//! 本 crate 不依赖 DB,可被 Tauri 查看器复用(本地打开 DICOM 文件)。
//!
//! 目前只实现了帧提取([`frames`]),供 WADO-RS 的 `/frames` 使用。
//! 显示管线在阶段 6 随查看器一起做 —— 窗宽窗位是交互式的,
//! 放在客户端才有意义。

pub mod display;
pub mod frames;

pub use display::{ModalityLut, Photometric, Pipeline, VoiFunction, Window};
pub use frames::{FrameError, Frames};
