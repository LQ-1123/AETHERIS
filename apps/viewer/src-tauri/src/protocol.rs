//! 自定义协议处理器 - pacs-frame://

use crate::state::ViewerState;
use tauri::http::{Request, Response};
use tauri::{Manager, Runtime};

/// 注册 pacs-frame:// 协议
///
/// 路径格式: pacs-frame://localhost/{handle}/{frame}
/// 返回该帧的原始像素字节（Uint16Array 可直接读取）
pub fn register_protocol<R: Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder.register_asynchronous_uri_scheme_protocol("pacs-frame", move |_ctx, request, responder| {
        let state = _ctx.app_handle().state::<ViewerState>();
        let state = state.inner().clone();

        tokio::spawn(async move {
            let response = handle_request(request, state).await;
            responder.respond(response);
        });
    })
}

async fn handle_request(request: Request<Vec<u8>>, state: ViewerState) -> Response<Vec<u8>> {
    // 解析路径: /handle/frame
    let path = request.uri().path();
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();

    if parts.len() != 2 {
        return Response::builder()
            .status(400)
            .body(b"Invalid path: expected /handle/frame".to_vec())
            .unwrap();
    }

    let handle: u64 = match parts[0].parse() {
        Ok(h) => h,
        Err(_) => {
            return Response::builder()
                .status(400)
                .body(format!("Invalid handle: {}", parts[0]).into_bytes())
                .unwrap();
        }
    };

    let frame: u32 = match parts[1].parse() {
        Ok(f) => f,
        Err(_) => {
            return Response::builder()
                .status(400)
                .body(format!("Invalid frame: {}", parts[1]).into_bytes())
                .unwrap();
        }
    };

    // 获取帧数据
    match state.get_frame_bytes(handle, frame) {
        Ok(data) => Response::builder()
            .status(200)
            .header("Content-Type", "application/octet-stream")
            .header("Content-Length", data.len().to_string())
            .body(data)
            .unwrap(),
        Err(e) => Response::builder()
            .status(404)
            .body(e.to_string().into_bytes())
            .unwrap(),
    }
}
