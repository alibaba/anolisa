use super::*;

use futures::FutureExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream as AsyncTcpStream};
use tokio::sync::oneshot;

const TEST_READ_TIMEOUT: Duration = Duration::from_secs(1);
const TEST_COMPLETION_BOUND: Duration = Duration::from_secs(5);
const TEST_WATCHDOG: Duration = Duration::from_secs(10);
const CHUNK_INTERVAL: Duration = Duration::from_millis(250);
const KEEPALIVE_CHUNKS: usize = 12;
const FIRST_EVENT: &str = "data: first\n\n";
const KEEPALIVE_EVENT: &str = "data: keep-alive\n\n";
const DONE_EVENT: &str = "data: [DONE]\n\n";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StreamingScenario {
    SilentBeforeHeaders,
    SilentAfterHeaders,
    SilentAfterFirstChunk,
    Healthy,
}

async fn write_sse_chunk(socket: &mut AsyncTcpStream, event: &str) -> std::io::Result<()> {
    let chunk = format!("{:x}\r\n{event}\r\n", event.len());
    socket.write_all(chunk.as_bytes()).await
}

async fn serve_streaming_scenario(
    listener: TcpListener,
    scenario: StreamingScenario,
    request_received: oneshot::Sender<()>,
) -> std::io::Result<()> {
    let (mut socket, _) = listener.accept().await?;
    socket.set_nodelay(true)?;
    let mut request = Vec::new();
    let mut buffer = [0; 1024];
    while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
        let read = socket.read(&mut buffer).await?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "client closed before sending request headers",
            ));
        }
        request.extend_from_slice(&buffer[..read]);
    }
    let _ = request_received.send(());

    if scenario != StreamingScenario::SilentBeforeHeaders {
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                  Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
            )
            .await?;
    }
    if matches!(
        scenario,
        StreamingScenario::SilentAfterFirstChunk | StreamingScenario::Healthy
    ) {
        write_sse_chunk(&mut socket, FIRST_EVENT).await?;
    }
    if scenario == StreamingScenario::Healthy {
        for _ in 0..KEEPALIVE_CHUNKS {
            tokio::time::sleep(CHUNK_INTERVAL).await;
            write_sse_chunk(&mut socket, KEEPALIVE_EVENT).await?;
        }
        write_sse_chunk(&mut socket, DONE_EVENT).await?;
        socket.write_all(b"0\r\n\r\n").await?;
        socket.shutdown().await?;
    } else {
        std::future::pending::<()>().await;
    }
    drop(socket);
    Ok(())
}

async fn check_streaming_scenario(scenario: StreamingScenario) {
    let mut server = None;
    let outcome = tokio::time::timeout(
        TEST_WATCHDOG,
        std::panic::AssertUnwindSafe(async {
            let listener = TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind streaming fixture");
            let endpoint = endpoint::ResolvedEndpoint {
                host: listener.local_addr().expect("fixture address").to_string(),
                scheme: "http".to_string(),
                // Isolate this loopback fixture from host proxies; this is not a VPC probe.
                origin: endpoint::EndpointOrigin::VpcProxy,
            };
            let client =
                build_streaming_client(&endpoint, Duration::from_secs(3), TEST_READ_TIMEOUT)
                    .expect("build streaming client");
            let (request_received_tx, request_received_rx) = oneshot::channel();
            server = Some(tokio::spawn(serve_streaming_scenario(
                listener,
                scenario,
                request_received_tx,
            )));

            let started = std::time::Instant::now();
            let request = client.post(endpoint.base_url());
            let (received, response) = tokio::join!(request_received_rx, request.send());
            received.expect("fixture must accept the connection and receive request headers");

            if scenario == StreamingScenario::SilentBeforeHeaders {
                let error = response.expect_err("silent response headers must time out");
                assert!(error.is_timeout(), "expected a read timeout, got {error:?}");
            } else {
                let mut response = response.expect("receive SSE response headers");
                assert_eq!(response.status(), reqwest::StatusCode::OK);
                assert_eq!(response.headers()["content-type"], "text/event-stream");

                if scenario == StreamingScenario::Healthy {
                    let body = response.text().await.expect("healthy SSE must finish");
                    let expected = format!(
                        "{FIRST_EVENT}{}{DONE_EVENT}",
                        KEEPALIVE_EVENT.repeat(KEEPALIVE_CHUNKS)
                    );
                    assert_eq!(body, expected);
                    assert!(
                        started.elapsed() > TEST_READ_TIMEOUT,
                        "healthy stream must outlive a total request timeout"
                    );
                } else {
                    if scenario == StreamingScenario::SilentAfterFirstChunk {
                        let mut first = Vec::new();
                        while first.len() < FIRST_EVENT.len() {
                            let chunk = response
                                .chunk()
                                .await
                                .expect("read the first SSE event")
                                .expect("first SSE event must not be EOF");
                            first.extend_from_slice(&chunk);
                        }
                        assert_eq!(first, FIRST_EVENT.as_bytes());
                    }
                    let error = response
                        .chunk()
                        .await
                        .expect_err("stalled SSE body must time out, not reach EOF");
                    assert!(error.is_timeout(), "expected a read timeout, got {error:?}");
                }
            }
            assert!(
                started.elapsed() < TEST_COMPLETION_BOUND,
                "{scenario:?} exceeded {TEST_COMPLETION_BOUND:?}"
            );
        })
        .catch_unwind(),
    )
    .await;

    // Reap the server even when an assertion panics or the watchdog expires.
    if let Some(server) = server {
        server.abort();
        match server.await {
            Ok(result) => result.expect("streaming fixture failed"),
            Err(error) => assert!(error.is_cancelled(), "fixture task failed: {error}"),
        }
    }
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(panic)) => std::panic::resume_unwind(panic),
        Err(error) => panic!("{scenario:?} exceeded watchdog {TEST_WATCHDOG:?}: {error}"),
    }
}

#[tokio::test]
async fn streaming_client_bounds_a_silent_server() {
    check_streaming_scenario(StreamingScenario::SilentBeforeHeaders).await;
}

#[tokio::test]
async fn streaming_client_bounds_silence_after_sse_headers() {
    check_streaming_scenario(StreamingScenario::SilentAfterHeaders).await;
}

#[tokio::test]
async fn streaming_client_bounds_silence_after_first_chunk() {
    check_streaming_scenario(StreamingScenario::SilentAfterFirstChunk).await;
}

#[tokio::test]
async fn streaming_client_preserves_a_healthy_long_stream() {
    check_streaming_scenario(StreamingScenario::Healthy).await;
}

#[test]
fn region_id_strips_single_zone_suffix() {
    assert_eq!(
        region_id_from_zone_id("cn-hangzhou-j").as_deref(),
        Some("cn-hangzhou")
    );
    assert_eq!(
        region_id_from_zone_id("cn-beijing").as_deref(),
        Some("cn-beijing")
    );
    assert_eq!(region_id_from_zone_id(""), None);
}

#[test]
fn generate_console_url_uses_region_and_instance_id() {
    assert_eq!(
        generate_console_url("i-test123", "cn-hangzhou"),
        "https://alinux.console.aliyun.com/cn-hangzhou/guide/cosh?instance=i-test123"
    );
}

#[test]
fn build_request_preserves_user_provided_secrets() {
    let provider = SysomProvider {
        configured_endpoint: String::new(),
        credentials: std::sync::RwLock::new(SysomCredentials {
            access_key_id: "test-id".to_string(),
            access_key_secret: "test-secret".to_string(),
            security_token: None,
        }),
        is_sts: false,
        cancelled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        instance_id: None,
    };
    let secret = "short-provider-secret";
    let messages = vec![Message::user(&format!("api_key={secret}"))];

    let body = provider.build_request_body(&messages, &[], &GenerateConfig::default());
    let payload = body.to_string();

    assert!(payload.contains(secret), "{payload}");
    assert!(!payload.contains("<redacted>"), "{payload}");
}

fn test_provider() -> SysomProvider {
    SysomProvider {
        configured_endpoint: String::new(),
        credentials: std::sync::RwLock::new(SysomCredentials {
            access_key_id: "test-id".to_string(),
            access_key_secret: "test-secret".to_string(),
            security_token: None,
        }),
        is_sts: false,
        cancelled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        instance_id: None,
    }
}

/// Parses the inner request the SysOM wrapper actually sends.
fn wire_inner(body: &serde_json::Value) -> serde_json::Value {
    serde_json::from_str(
        body["llmParamString"]
            .as_str()
            .expect("llmParamString is a string"),
    )
    .expect("inner request is JSON")
}

#[test]
fn extra_params_cannot_raise_the_wire_output_cap() {
    // Regression (#2240): the compaction budget reserves `O` from the same
    // resolver that produced `max_tokens`, so a wire request must never be
    // allowed to spend more than the reserve — not even via extra_params.
    let provider = test_provider();
    let config = GenerateConfig {
        max_tokens: 16_384,
        extra_params: Some(serde_json::json!({
            "max_tokens": 65_536u32,
            "max_completion_tokens": 65_536u32,
        })),
        ..GenerateConfig::default()
    };

    let inner = wire_inner(&provider.build_request_body(&[], &[], &config));

    assert_eq!(inner["max_tokens"], 16_384);
    assert_eq!(inner["max_completion_tokens"], 16_384);
}

#[test]
fn extra_params_may_still_lower_the_wire_output_cap() {
    // Asking for less than the reserve is always safe and is preserved.
    let provider = test_provider();
    let config = GenerateConfig {
        max_tokens: 16_384,
        extra_params: Some(serde_json::json!({"max_tokens": 512u32})),
        ..GenerateConfig::default()
    };

    let inner = wire_inner(&provider.build_request_body(&[], &[], &config));

    assert_eq!(inner["max_tokens"], 512);
}
