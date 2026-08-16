//! AETHERIS Medical Imaging Cloud - Tauri 2 桌面客户端
//!
//! 核心设计:
//! - 帧数据通过自定义协议 `pacs-frame://` 直传,不走 JSON IPC
//! - 查找表(LUT)让窗宽窗位交互只需查表,不重算管线
//! - 状态管理用句柄索引已打开的序列,带 LRU 缓存

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ai;
mod commands;
mod local;
mod mpr;
mod protocol;
mod remote;
mod state;

use ai::AiState;
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
        .setup(|app| {
            use tauri::Manager as _;
            app.manage(AiState::new(app.handle()));
            // 本地完整栈（内嵌 PostgreSQL + pacsd），双击即用模式
            app.manage(std::sync::Arc::new(local::LocalStack::new(app.handle())));
            Ok(())
        })
        .manage(state)
        .manage(remote)
        .invoke_handler(tauri::generate_handler![
            commands::list_ai_models,
            commands::list_ai_catalog,
            commands::refresh_ai_plugins,
            commands::check_ai_plugin,
            commands::add_ai_plugin,
            commands::list_ai_plugin_configurations,
            commands::run_ai_segmentation,
            commands::cancel_ai_segmentation,
            commands::open_series,
            commands::close_series,
            commands::select_image_stack,
            commands::build_lut,
            commands::measure_frame_roi,
            commands::measure_mpr_roi,
            commands::prepare_mpr,
            commands::render_mpr_slice,
            commands::begin_mpr_prefetch,
            commands::prefetch_mpr_slices,
            commands::cancel_mpr_prefetch,
            commands::close_mpr,
            commands::cancel_mpr_build,
            commands::remote_login,
            commands::remote_logout,
            commands::list_window_presets,
            commands::create_window_preset,
            commands::rename_window_preset,
            commands::delete_window_preset,
            commands::list_report_templates,
            commands::list_reports,
            commands::create_report,
            commands::update_report_draft,
            commands::sign_report,
            commands::begin_report_amendment,
            commands::list_report_versions,
            commands::list_worklist,
            commands::work_item_for_series,
            commands::claim_work_item,
            commands::release_work_item,
            commands::register_device,
            commands::list_devices,
            commands::approve_device,
            commands::set_device_status,
            commands::list_series_sources,
            commands::resolve_series_source,
            commands::list_users,
            commands::list_user_device_grants,
            commands::replace_user_device_grants,
            commands::list_patients,
            commands::list_patient_studies,
            commands::list_study_series,
            commands::list_shared_annotations,
            commands::create_shared_annotation,
            commands::update_shared_annotation,
            commands::list_segmentation_projects,
            commands::create_segmentation_project,
            commands::delete_segmentation_project,
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
            commands::local_stack_info,
        ]);

    // 注册自定义协议
    let builder = protocol::register_protocol(builder);

    builder
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            // 退出时停止本地 pacsd 与 PostgreSQL（数据保留）
            if let tauri::RunEvent::Exit = event {
                use tauri::Manager as _;
                app_handle
                    .state::<std::sync::Arc<local::LocalStack>>()
                    .shutdown();
            }
        });
}
