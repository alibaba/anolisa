//! Integration test: full IPC protocol round-trip over Unix Socket.
//!
//! Spins up a mock server on a temporary Unix Socket, sends a Request
//! from a "client", and verifies the Response comes back correctly.

use std::path::PathBuf;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

use ws_ckpt_common::{
    decode_payload, encode_frame, ChangeType, ConfigReport, DiffEntry, Request, Response,
    SnapshotEntry, SnapshotMeta, StatusReport, WorkspaceInfo,
};

/// Helper: create a temporary socket path using tempfile
fn temp_socket_path() -> PathBuf {
    let dir = tempfile::tempdir().expect("failed to create tempdir");
    // We leak the tempdir so it's not cleaned up during the test.
    // The OS will clean up /tmp on reboot.
    let path = dir.path().join("test.sock");
    std::mem::forget(dir);
    path
}

/// Server side: read one request frame, process it, send a response frame
async fn mock_server_handle(mut stream: tokio::net::UnixStream) {
    // 1. Read 4-byte LE length
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await.expect("read len");
    let len = u32::from_le_bytes(len_buf) as usize;

    // 2. Read payload
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).await.expect("read payload");

    // 3. Decode request
    let request: Request = decode_payload(&payload).expect("decode request");

    // 4. Build response based on request type
    let response = match request {
        Request::Init { workspace } => Response::InitOk {
            ws_id: format!("ws-{}", &workspace[..6.min(workspace.len())]),
        },
        Request::Checkpoint { .. } => Response::CheckpointOk {
            snapshot_id: "msg1-step0".to_string(),
        },
        Request::Rollback { to, .. } => Response::RollbackOk {
            from: "ws-test".to_string(),
            to,
        },
        Request::Delete { snapshot, .. } => Response::DeleteOk { target: snapshot },
        Request::List { .. } => Response::ListOk {
            snapshots: vec![SnapshotEntry {
                id: "abcdef1234567890abcdef1234567890abcdef12".to_string(),
                workspace: "/home/user/ws".to_string(),
                meta: SnapshotMeta {
                    message: Some("initial".to_string()),
                    metadata: None,
                    pinned: false,
                    created_at: chrono::Utc::now(),
                },
            }],
        },
        Request::Diff { .. } => Response::DiffOk {
            changes: vec![DiffEntry {
                path: "src/main.rs".to_string(),
                change_type: ChangeType::Modified,
                detail: None,
            }],
        },
        Request::Status { .. } => Response::StatusOk {
            report: StatusReport {
                uptime_secs: 42,
                workspaces: vec![WorkspaceInfo {
                    ws_id: "ws-test".to_string(),
                    path: "/tmp/ws".to_string(),
                    snapshot_count: 3,
                }],
                fs_total_bytes: 1_000_000_000,
                fs_used_bytes: 500_000_000,
            },
        },
        Request::Cleanup { .. } => Response::CleanupOk {
            removed: vec!["msg1-step0".to_string()],
        },
        Request::Config => Response::ConfigOk {
            config: ConfigReport {
                mount_path: "/mnt/btrfs-workspace".to_string(),
                socket_path: "/run/ws-ckpt/ws-ckpt.sock".to_string(),
                log_level: "info".to_string(),
                auto_cleanup: false,
                auto_cleanup_keep: 20,
                auto_cleanup_interval_secs: 600,
                health_check_interval_secs: 300,
                fs_warn_threshold_percent: 90.0,
                img_path: "/data/ws-ckpt/btrfs-data.img".to_string(),
                img_size: 30,
                img_max_percent: 40.0,
            },
        },
        Request::ReloadConfig => Response::ReloadConfigOk,
        Request::Recover { workspace } => Response::RecoverOk { workspace },
    };

    // 5. Encode and send response frame
    let frame = encode_frame(&response).expect("encode response");
    stream.write_all(&frame).await.expect("write response");
}

#[tokio::test]
async fn full_init_request_response_over_socket() {
    let socket_path = temp_socket_path();

    // Start server
    let listener = UnixListener::bind(&socket_path).expect("bind failed");
    let server_handle = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept failed");
        mock_server_handle(stream).await;
    });

    // Give server a moment to start
    tokio::task::yield_now().await;

    // Client connects
    let mut client = UnixStream::connect(&socket_path)
        .await
        .expect("connect failed");

    // Send Init request
    let request = Request::Init {
        workspace: "/tmp/my-workspace".to_string(),
    };
    let frame = encode_frame(&request).expect("encode request");
    client.write_all(&frame).await.expect("write request");

    // Read response
    let mut len_buf = [0u8; 4];
    client
        .read_exact(&mut len_buf)
        .await
        .expect("read resp len");
    let len = u32::from_le_bytes(len_buf) as usize;

    let mut payload = vec![0u8; len];
    client
        .read_exact(&mut payload)
        .await
        .expect("read resp payload");

    let response: Response = decode_payload(&payload).expect("decode response");

    // Verify
    match response {
        Response::InitOk { ws_id } => {
            assert!(ws_id.starts_with("ws-"));
        }
        _ => panic!("expected InitOk, got {:?}", response),
    }

    server_handle.await.unwrap();
}

#[tokio::test]
async fn full_checkpoint_request_response_over_socket() {
    let socket_path = temp_socket_path();

    let listener = UnixListener::bind(&socket_path).expect("bind");
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        mock_server_handle(stream).await;
    });

    tokio::task::yield_now().await;

    let mut client = UnixStream::connect(&socket_path).await.unwrap();

    let request = Request::Checkpoint {
        workspace: "/ws".to_string(),
        id: "msg1-step0".to_string(),
        message: Some("test message".to_string()),
        metadata: None,
        pin: true,
    };
    let frame = encode_frame(&request).unwrap();
    client.write_all(&frame).await.unwrap();

    let mut len_buf = [0u8; 4];
    client.read_exact(&mut len_buf).await.unwrap();
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut payload = vec![0u8; len];
    client.read_exact(&mut payload).await.unwrap();

    let response: Response = decode_payload(&payload).unwrap();
    match response {
        Response::CheckpointOk { snapshot_id } => {
            assert_eq!(snapshot_id, "msg1-step0");
        }
        _ => panic!("expected CheckpointOk, got {:?}", response),
    }

    server.await.unwrap();
}

#[tokio::test]
async fn full_rollback_request_response_over_socket() {
    let socket_path = temp_socket_path();

    let listener = UnixListener::bind(&socket_path).expect("bind");
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        mock_server_handle(stream).await;
    });

    tokio::task::yield_now().await;

    let mut client = UnixStream::connect(&socket_path).await.unwrap();

    let request = Request::Rollback {
        workspace: "/ws".to_string(),
        to: "msg1-step2".to_string(),
    };
    let frame = encode_frame(&request).unwrap();
    client.write_all(&frame).await.unwrap();

    let mut len_buf = [0u8; 4];
    client.read_exact(&mut len_buf).await.unwrap();
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut payload = vec![0u8; len];
    client.read_exact(&mut payload).await.unwrap();

    let response: Response = decode_payload(&payload).unwrap();
    match response {
        Response::RollbackOk { from, to } => {
            assert_eq!(from, "ws-test");
            assert_eq!(to, "msg1-step2");
        }
        _ => panic!("expected RollbackOk, got {:?}", response),
    }

    server.await.unwrap();
}

#[tokio::test]
async fn frame_length_prefix_matches_payload() {
    // Verify the frame protocol: first 4 bytes = LE payload length
    let request = Request::Delete {
        workspace: Some("/ws".to_string()),
        snapshot: "msg1-step0".to_string(),
        force: true,
    };
    let frame = encode_frame(&request).unwrap();

    let declared_len = u32::from_le_bytes(frame[..4].try_into().unwrap()) as usize;
    let actual_payload = &frame[4..];
    assert_eq!(declared_len, actual_payload.len());

    // Verify the payload can be decoded back
    let decoded: Request = decode_payload(actual_payload).unwrap();
    match decoded {
        Request::Delete { force, .. } => assert!(force),
        _ => panic!("expected Delete"),
    }
}

// ── Phase 2 protocol integration tests ──

#[tokio::test]
async fn full_list_request_response_over_socket() {
    let socket_path = temp_socket_path();

    let listener = UnixListener::bind(&socket_path).expect("bind");
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        mock_server_handle(stream).await;
    });

    tokio::task::yield_now().await;

    let mut client = UnixStream::connect(&socket_path).await.unwrap();

    let request = Request::List {
        workspace: Some("/tmp/ws".to_string()),
        format: Some("json".to_string()),
    };
    let frame = encode_frame(&request).unwrap();
    client.write_all(&frame).await.unwrap();

    let mut len_buf = [0u8; 4];
    client.read_exact(&mut len_buf).await.unwrap();
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut payload = vec![0u8; len];
    client.read_exact(&mut payload).await.unwrap();

    let response: Response = decode_payload(&payload).unwrap();
    match response {
        Response::ListOk { snapshots } => {
            assert_eq!(snapshots.len(), 1);
            assert_eq!(snapshots[0].id, "abcdef1234567890abcdef1234567890abcdef12");
        }
        _ => panic!("expected ListOk, got {:?}", response),
    }

    server.await.unwrap();
}

#[tokio::test]
async fn full_diff_request_response_over_socket() {
    let socket_path = temp_socket_path();

    let listener = UnixListener::bind(&socket_path).expect("bind");
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        mock_server_handle(stream).await;
    });

    tokio::task::yield_now().await;

    let mut client = UnixStream::connect(&socket_path).await.unwrap();

    let request = Request::Diff {
        workspace: "/tmp/ws".to_string(),
        from: "msg1-step0".to_string(),
        to: "msg2-step0".to_string(),
    };
    let frame = encode_frame(&request).unwrap();
    client.write_all(&frame).await.unwrap();

    let mut len_buf = [0u8; 4];
    client.read_exact(&mut len_buf).await.unwrap();
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut payload = vec![0u8; len];
    client.read_exact(&mut payload).await.unwrap();

    let response: Response = decode_payload(&payload).unwrap();
    match response {
        Response::DiffOk { changes } => {
            assert_eq!(changes.len(), 1);
            assert_eq!(changes[0].change_type, ChangeType::Modified);
        }
        _ => panic!("expected DiffOk, got {:?}", response),
    }

    server.await.unwrap();
}

#[tokio::test]
async fn full_status_request_response_over_socket() {
    let socket_path = temp_socket_path();

    let listener = UnixListener::bind(&socket_path).expect("bind");
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        mock_server_handle(stream).await;
    });

    tokio::task::yield_now().await;

    let mut client = UnixStream::connect(&socket_path).await.unwrap();

    let request = Request::Status { workspace: None };
    let frame = encode_frame(&request).unwrap();
    client.write_all(&frame).await.unwrap();

    let mut len_buf = [0u8; 4];
    client.read_exact(&mut len_buf).await.unwrap();
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut payload = vec![0u8; len];
    client.read_exact(&mut payload).await.unwrap();

    let response: Response = decode_payload(&payload).unwrap();
    match response {
        Response::StatusOk { report } => {
            assert_eq!(report.uptime_secs, 42);
            assert_eq!(report.workspaces.len(), 1);
            assert_eq!(report.workspaces[0].ws_id, "ws-test");
        }
        _ => panic!("expected StatusOk, got {:?}", response),
    }

    server.await.unwrap();
}

#[tokio::test]
async fn full_cleanup_request_response_over_socket() {
    let socket_path = temp_socket_path();

    let listener = UnixListener::bind(&socket_path).expect("bind");
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        mock_server_handle(stream).await;
    });

    tokio::task::yield_now().await;

    let mut client = UnixStream::connect(&socket_path).await.unwrap();

    let request = Request::Cleanup {
        workspace: "/tmp/ws".to_string(),
        keep: Some(10),
    };
    let frame = encode_frame(&request).unwrap();
    client.write_all(&frame).await.unwrap();

    let mut len_buf = [0u8; 4];
    client.read_exact(&mut len_buf).await.unwrap();
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut payload = vec![0u8; len];
    client.read_exact(&mut payload).await.unwrap();

    let response: Response = decode_payload(&payload).unwrap();
    match response {
        Response::CleanupOk { removed } => {
            assert_eq!(removed.len(), 1);
            assert_eq!(removed[0], "msg1-step0");
        }
        _ => panic!("expected CleanupOk, got {:?}", response),
    }

    server.await.unwrap();
}

#[tokio::test]
async fn full_reload_config_request_response_over_socket() {
    let socket_path = temp_socket_path();

    let listener = UnixListener::bind(&socket_path).expect("bind");
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        mock_server_handle(stream).await;
    });

    tokio::task::yield_now().await;

    let mut client = UnixStream::connect(&socket_path).await.unwrap();

    let request = Request::ReloadConfig;
    let frame = encode_frame(&request).unwrap();
    client.write_all(&frame).await.unwrap();

    let mut len_buf = [0u8; 4];
    client.read_exact(&mut len_buf).await.unwrap();
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut payload = vec![0u8; len];
    client.read_exact(&mut payload).await.unwrap();

    let response: Response = decode_payload(&payload).unwrap();
    assert!(matches!(response, Response::ReloadConfigOk));

    server.await.unwrap();
}

#[tokio::test]
async fn full_config_request_response_over_socket() {
    let socket_path = temp_socket_path();

    let listener = UnixListener::bind(&socket_path).expect("bind");
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        mock_server_handle(stream).await;
    });

    tokio::task::yield_now().await;

    let mut client = UnixStream::connect(&socket_path).await.unwrap();

    let request = Request::Config;
    let frame = encode_frame(&request).unwrap();
    client.write_all(&frame).await.unwrap();

    let mut len_buf = [0u8; 4];
    client.read_exact(&mut len_buf).await.unwrap();
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut payload = vec![0u8; len];
    client.read_exact(&mut payload).await.unwrap();

    let response: Response = decode_payload(&payload).unwrap();
    match response {
        Response::ConfigOk { config } => {
            assert_eq!(config.auto_cleanup_keep, 20);
            assert_eq!(config.auto_cleanup_interval_secs, 600);
        }
        _ => panic!("expected ConfigOk, got {:?}", response),
    }

    server.await.unwrap();
}
