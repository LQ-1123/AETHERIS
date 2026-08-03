//! Tauri 命令 - 前端调用的 IPC 接口

use crate::state::{DisplayMetadata, ViewerState};
use std::path::PathBuf;
use tauri::State;

/// 打开 DICOM 文件
#[tauri::command]
pub fn open_dicom(
    path: String,
    state: State<ViewerState>,
) -> Result<DisplayMetadata, String> {
    state
        .open(PathBuf::from(path))
        .map_err(|e| e.to_string())
}

/// 关闭实例
#[tauri::command]
pub fn close_instance(handle: u64, state: State<ViewerState>) -> Result<(), String> {
    state.close(handle).map_err(|e| e.to_string())
}

/// 生成查找表
#[tauri::command]
pub fn build_lut(
    handle: u64,
    window_center: Option<f64>,
    window_width: Option<f64>,
    state: State<ViewerState>,
) -> Result<Vec<u8>, String> {
    state
        .build_lut(handle, window_center, window_width)
        .map_err(|e| e.to_string())
}
