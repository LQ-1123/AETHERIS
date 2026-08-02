//! PACS 查看器 - Tauri 2 桌面客户端
//!
//! 核心设计:
//! - 帧数据通过自定义协议 `pacs-frame://` 直传,不走 JSON IPC
//! - 查找表(LUT)让窗宽窗位交互只需查表,不重算管线
//! - 状态管理用句柄索引已打开的实例,带 LRU 缓存

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
