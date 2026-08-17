//! 报告独立小窗：上下文快照托管 + 开窗/聚焦/更新命令。

use std::sync::Mutex;

use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

pub const REPORT_WINDOW_LABEL: &str = "report";

/// 主窗 ↔ 报告窗共享的上下文快照（当前检查/序列 + 患者 + 当前用户）。
#[derive(Default)]
pub struct ReportWindowState {
    context: Mutex<Option<Value>>,
}

/// 打开（或聚焦）报告窗，并把上下文写入快照后推给报告窗。
#[tauri::command]
pub async fn open_report_window(
    app: AppHandle,
    state: State<'_, ReportWindowState>,
    context: Value,
) -> Result<(), String> {
    *state.context.lock().unwrap() = Some(context.clone());
    if let Some(window) = app.get_webview_window(REPORT_WINDOW_LABEL) {
        let _ = window.emit("report-context", &context);
        let _ = window.set_focus();
        return Ok(());
    }
    let window = WebviewWindowBuilder::new(
        &app,
        REPORT_WINDOW_LABEL,
        WebviewUrl::App("index.html?mode=report".into()),
    )
    .title("诊断报告")
    .inner_size(470.0, 800.0)
    .min_inner_size(400.0, 560.0)
    .resizable(true)
    .build()
    .map_err(|error| error.to_string())?;
    // 报告窗启动后自行 get_report_context 拉取，这里不再推送，避免监听未就绪。
    let _ = window.emit("report-context", &context);
    Ok(())
}

/// 主窗切换/打开序列时更新快照并推送给已打开的报告窗。
#[tauri::command]
pub async fn update_report_context(
    app: AppHandle,
    state: State<'_, ReportWindowState>,
    context: Value,
) -> Result<(), String> {
    *state.context.lock().unwrap() = Some(context.clone());
    if let Some(window) = app.get_webview_window(REPORT_WINDOW_LABEL) {
        let _ = window.emit("report-context", &context);
    }
    Ok(())
}

/// 报告窗启动时主动拉取当前上下文（兜底，防事件丢失）。
#[tauri::command]
pub async fn get_report_context(
    state: State<'_, ReportWindowState>,
) -> Result<Option<Value>, String> {
    Ok(state.context.lock().unwrap().clone())
}
