//! 自定义协议处理器 - pacs-frame://

use crate::state::ViewerState;
use tauri::http::{Request, Response};
use tauri::{Manager, Runtime};

/// 注册 pacs-frame:// 协议
///
/// 路径格式: pacs-frame://localhost/{handle}/{frame}
/// 返回该帧的原始像素字节（Uint16Array 可直接读取）
pub fn register_protocol<R: Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder.register_asynchronous_uri_scheme_protocol("pacs-frame", move |app, request, responder| {
        let state = app.app_handle().state::<ViewerState>();
        let state = state.inner().clone();

        // 使用 Tauri 的运行时而不是 tokio::spawn
        tauri::async_runtime::spawn(async move {
            let response = handle_request(request, state).await;
            responder.respond(response);
        });
    })
}

async fn handle_request(request: Request<Vec<u8>>, state: ViewerState) -> Response<Vec<u8>> {
    // 解析路径: /handle/frame
    let path = request.uri().path();
    eprintln!("pacs-frame protocol request: {}", path);

    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();

    if parts.len() != 2 {
        eprintln!("Invalid path format: expected /handle/frame, got {}", path);
        return Response::builder()
            .status(400)
            .body(b"Invalid path: expected /handle/frame".to_vec())
            .unwrap();
    }

    let handle: u64 = match parts[0].parse() {
        Ok(h) => {
            eprintln!("Parsed handle: {}", h);
            h
        }
        Err(e) => {
            eprintln!("Failed to parse handle '{}': {}", parts[0], e);
            return Response::builder()
                .status(400)
                .body(format!("Invalid handle: {}", parts[0]).into_bytes())
                .unwrap();
        }
    };

    let frame: u32 = match parts[1].parse() {
        Ok(f) => {
            eprintln!("Parsed frame: {}", f);
            f
        }
        Err(e) => {
            eprintln!("Failed to parse frame '{}': {}", parts[1], e);
            return Response::builder()
                .status(400)
                .body(format!("Invalid frame: {}", parts[1]).into_bytes())
                .unwrap();
        }
    };

    eprintln!("Getting frame bytes for handle={}, frame={}", handle, frame);

    // 获取帧数据
    match state.get_frame_bytes(handle, frame) {
        Ok(data) => {
            eprintln!("Successfully got {} bytes", data.len());
            Response::builder()
                .status(200)
                .header("Content-Type", "application/octet-stream")
                .header("Content-Length", data.len().to_string())
                .header("Access-Control-Allow-Origin", "*")
                .header("Access-Control-Allow-Methods", "GET")
                .body(data)
                .unwrap()
        }
        Err(e) => {
            eprintln!("Error getting frame: {}", e);
            Response::builder()
                .status(404)
                .header("Access-Control-Allow-Origin", "*")
                .body(e.to_string().into_bytes())
                .unwrap()
        }
    }
}
