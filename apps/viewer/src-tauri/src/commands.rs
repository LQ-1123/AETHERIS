//! Tauri IPC commands used by the viewer frontend.

use crate::state::{SeriesMetadata, ViewerState};
use std::path::PathBuf;
use tauri::State;

#[tauri::command]
pub async fn open_series(
    paths: Vec<String>,
    state: State<'_, ViewerState>,
) -> Result<SeriesMetadata, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.open_series(paths.into_iter().map(PathBuf::from).collect())
    })
    .await
    .map_err(|error| format!("打开序列任务失败: {error}"))?
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn close_series(handle: u64, state: State<'_, ViewerState>) -> Result<(), String> {
    state.close(handle).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn build_lut(
    handle: u64,
    frame_index: u32,
    window_center: f64,
    window_width: f64,
    voi_function: String,
    state: State<'_, ViewerState>,
) -> Result<tauri::ipc::Response, String> {
    state
        .build_lut(
            handle,
            frame_index,
            window_center,
            window_width,
            &voi_function,
        )
        .map(tauri::ipc::Response::new)
        .map_err(|error| error.to_string())
}
