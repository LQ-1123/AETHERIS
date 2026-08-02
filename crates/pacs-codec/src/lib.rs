//! 像素解码、缩略图、帧提取。
//!
//! CPU 密集,必须经 `spawn_blocking`/rayon 调用,不得直接跑在 async executor 上。
//! 显示管线顺序:存储值 → Rescale → VOI(窗宽窗位) → Photometric 反转 → 输出。
//! `MONOCHROME1` 灰度反转、多值 WindowCenter、SIGMOID VOI 见计划"查看器陷阱"。
//! 本 crate 不依赖 DB,可被 Tauri 查看器复用(本地打开 DICOM 文件)。
//!
//! 三块内容:
//!
//! - [`frames`] —— 帧提取,供 WADO-RS 的 `/frames` 使用。
//! - [`display`] —— 显示管线的解析与计算,标准规定的顺序在这里落实。
//! - [`lut`] —— 把整条管线预先算成查找表,让查看器的窗宽窗位交互
//!   既快又不必在 TypeScript 里重写一遍管线。
//!
//! 窗宽窗位本身是交互式的,所以**渲染发生在客户端**;但决定灰度的那套规则
//! 只在这里实现一次。

pub mod display;
pub mod frames;
pub mod lut;

pub use display::{ModalityLut, Photometric, Pipeline, VoiFunction, Window};
pub use frames::{FrameError, Frames};
pub use lut::{GrayLut, MAX_LUT_BITS};
