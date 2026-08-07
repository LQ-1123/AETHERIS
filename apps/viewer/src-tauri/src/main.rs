//! AETHERIS Medical Imaging Cloud - Tauri 2 桌面客户端
//!
//! 核心设计:
//! - 帧数据通过自定义协议 `pacs-frame://` 直传,不走 JSON IPC
//! - 查找表(LUT)让窗宽窗位交互只需查表,不重算管线
//! - 状态管理用句柄索引已打开的序列,带 LRU 缓存

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod mpr;
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
            commands::select_image_stack,
            commands::build_lut,
            commands::measure_frame_roi,
            commands::measure_mpr_roi,
            commands::prepare_mpr,
            commands::render_mpr_slice,
            commands::close_mpr,
            commands::cancel_mpr_build,
            commands::remote_login,
            commands::remote_logout,
            commands::list_patients,
            commands::list_patient_studies,
            commands::list_study_series,
            commands::list_shared_annotations,
            commands::create_shared_annotation,
            commands::update_shared_annotation,
            commands::list_segmentation_projects,
            commands::create_segmentation_project,
            commands::list_segmentation_segments,
            commands::update_segmentation_segment_tags,
            commands::list_segmentation_masks,
            commands::upsert_segmentation_mask,
            commands::list_segmentation_volume,
            commands::upsert_segmentation_masks,
            commands::transform_schema,
            commands::preview_clinical_transform,
            commands::confirm_transform,
            commands::transform_jobs,
            commands::instance_revisions_by_sop,
            commands::preview_rollback,
            commands::open_remote_series,
            commands::cancel_remote_download,
            commands::import_to_pacs,
            commands::export_from_pacs,
            commands::cancel_transfer,
            commands::router_get,
            commands::router_write,
            commands::router_delete,
            commands::lifecycle_get,
            commands::lifecycle_write,
            commands::lifecycle_delete,
        ]);

    // 注册自定义协议
    let builder = protocol::register_protocol(builder);

    builder
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
