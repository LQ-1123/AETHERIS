//! 自定义协议处理器 - pacs-frame://

use crate::state::ViewerState;
use tauri::http::{Request, Response};
use tauri::{Manager, Runtime};

/// 注册 pacs-frame:// 协议
///
/// 路径格式: pacs-frame://localhost/{handle}/{stack}/{frame}
/// 返回该帧的原始像素字节（Uint16Array 可直接读取）
pub fn register_protocol<R: Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder.register_asynchronous_uri_scheme_protocol(
        "pacs-frame",
        move |app, request, responder| {
            let state = app.app_handle().state::<ViewerState>();
            let state = state.inner().clone();

            // 使用 Tauri 的运行时而不是 tokio::spawn
            tauri::async_runtime::spawn(async move {
                let response = handle_request(request, state).await;
                responder.respond(response);
            });
        },
    )
}

async fn handle_request(request: Request<Vec<u8>>, state: ViewerState) -> Response<Vec<u8>> {
    // 解析路径: /handle/stack/frame
    let path = request.uri().path();
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();

    if parts.len() != 3 {
        return Response::builder()
            .status(400)
            .body(b"Invalid path: expected /handle/stack/frame".to_vec())
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

    let stack: u32 = match parts[1].parse() {
        Ok(value) => value,
        Err(_) => {
            return Response::builder()
                .status(400)
                .body(format!("Invalid stack: {}", parts[1]).into_bytes())
                .unwrap();
        }
    };

    let frame: u32 = match parts[2].parse() {
        Ok(f) => f,
        Err(_) => {
            return Response::builder()
                .status(400)
                .body(format!("Invalid frame: {}", parts[2]).into_bytes())
                .unwrap();
        }
    };

    // Pixel decoding is CPU intensive and must not block Tauri's async runtime.
    let decoded =
        tauri::async_runtime::spawn_blocking(move || state.get_frame_bytes(handle, stack, frame))
            .await;
    match decoded {
        Ok(Ok(data)) => Response::builder()
            .status(200)
            .header("Content-Type", "application/octet-stream")
            .header("Content-Length", data.len().to_string())
            .header("Access-Control-Allow-Origin", "*")
            .header("Access-Control-Allow-Methods", "GET")
            .body(data)
            .unwrap(),
        Ok(Err(error)) => Response::builder()
            .status(404)
            .header("Access-Control-Allow-Origin", "*")
            .body(error.to_string().into_bytes())
            .unwrap(),
        Err(error) => Response::builder()
            .status(500)
            .header("Access-Control-Allow-Origin", "*")
            .body(format!("帧解码任务失败: {error}").into_bytes())
            .unwrap(),
    }
}
