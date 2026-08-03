//! PACS 查看器 - Tauri 2 桌面客户端
//!
//! 核心设计:
//! - 帧数据通过自定义协议 `pacs-frame://` 直传,不走 JSON IPC
//! - 查找表(LUT)让窗宽窗位交互只需查表,不重算管线
//! - 状态管理用句柄索引已打开的序列,带 LRU 缓存

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod protocol;
mod remote;
mod state;

use remote::RemoteState;
use state::ViewerState;

fn main() {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let state = ViewerState::new();
    let remote = RemoteState::new();

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .manage(remote)
        .invoke_handler(tauri::generate_handler![
            commands::open_series,
            commands::close_series,
            commands::build_lut,
            commands::remote_login,
            commands::remote_logout,
            commands::list_patients,
            commands::list_patient_studies,
            commands::list_study_series,
            commands::open_remote_series,
            commands::cancel_remote_download,
        ]);

    // 注册自定义协议
    let builder = protocol::register_protocol(builder);

    builder
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
