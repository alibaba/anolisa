use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use crate::state::DaemonState;
use ws_ckpt_common::{decode_payload, encode_frame, ErrorCode, Request, Response};

/// Maximum frame size: 16 MB
const MAX_FRAME_SIZE: u32 = 16 * 1024 * 1024;

pub async fn run_listener(
    state: Arc<DaemonState>,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    // 1. Clean up residual socket file
    let _ = std::fs::remove_file(&state.socket_path);

    // 2. Ensure socket parent directory exists
    if let Some(parent) = state.socket_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .context("Failed to create socket parent directory")?;
    }

    // 3. Bind the Unix listener
    let listener = UnixListener::bind(&state.socket_path).context("Failed to bind Unix socket")?;
    info!("Listening on {:?}", state.socket_path);

    // 4. Set socket permissions to 0o666
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&state.socket_path, std::fs::Permissions::from_mode(0o666))
        .context("Failed to set socket permissions")?;

    // 5. Accept loop
    let mut join_set = JoinSet::new();

    loop {
        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((stream, _addr)) => {
                        let state = Arc::clone(&state);
                        join_set.spawn(async move {
                            if let Err(e) = handle_connection(stream, state).await {
                                error!("Connection error: {:#}", e);
                            }
                        });
                    }
                    Err(e) => {
                        error!("Accept error: {}", e);
                    }
                }
            }
            _ = cancel.cancelled() => {
                info!("Listener received cancellation signal");
                break;
            }
        }
    }

    // 7. Wait for in-flight tasks to complete (with timeout)
    info!("Waiting for in-flight connections to complete...");
    let drain = async { while join_set.join_next().await.is_some() {} };
    if tokio::time::timeout(Duration::from_secs(10), drain)
        .await
        .is_err()
    {
        error!("Timed out waiting for in-flight connections; aborting remaining tasks");
        join_set.abort_all();
    }

    // Clean up socket file
    let _ = std::fs::remove_file(&state.socket_path);
    info!("Listener shut down");
    Ok(())
}

async fn handle_connection(
    mut stream: tokio::net::UnixStream,
    state: Arc<DaemonState>,
) -> anyhow::Result<()> {
    // Read 4-byte LE length
    let len = stream
        .read_u32_le()
        .await
        .context("Failed to read frame length")?;

    // Validate frame size
    if len > MAX_FRAME_SIZE {
        let err_resp = Response::Error {
            code: ErrorCode::InternalError,
            message: format!("Frame too large: {} bytes (max {})", len, MAX_FRAME_SIZE),
        };
        let frame = encode_frame(&err_resp)?;
        stream.write_all(&frame).await?;
        anyhow::bail!("Frame too large: {} bytes", len);
    }

    // Read payload
    let mut payload = vec![0u8; len as usize];
    stream
        .read_exact(&mut payload)
        .await
        .context("Failed to read frame payload")?;

    // Decode request
    let request: Request = decode_payload(&payload).context("Failed to decode request")?;

    let agent_name = stream
        .peer_cred()
        .ok()
        .and_then(|cred| cred.pid().map(|p| p as u32))
        .map(crate::ops_log::detect_agent_name)
        .unwrap_or_else(|| "user".to_string());
    let ops_name = crate::ops_log::ops_name_from_request(&request);

    // Dispatch
    let response = crate::dispatcher::dispatch(&state, request).await;

    if let Some(name) = ops_name {
        crate::ops_log::log_operation(name, &agent_name, &response);
    }

    // Encode and write response
    let frame = encode_frame(&response).context("Failed to encode response")?;
    stream
        .write_all(&frame)
        .await
        .context("Failed to write response")?;

    Ok(())
}
