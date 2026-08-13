//! Shared HTTP client for local model inference backends.
//!
//! Only Ollama is supported today.  Configuration is read from the same
//! environment variables the rest of the toolchain uses:
//!
//! - `AGENT_SEC_MODEL_SERVICE_BACKEND` (default `ollama`)
//! - `AGENT_SEC_MODEL_SERVICE_BASE_URL` (default `http://localhost:11434`)
//! - `AGENT_SEC_MODEL_SERVICE_TIMEOUT` seconds (default `30`, max `300`)
//!
//! Consumers (prompt-scanner, future code/pii scanners) inject a
//! [`ModelClient`] so their transport stays decoupled from this crate.

use std::time::Duration;

use serde_json::{json, Map, Value};
use thiserror::Error;

const ENV_BACKEND: &str = "AGENT_SEC_MODEL_SERVICE_BACKEND";
const ENV_BASE_URL: &str = "AGENT_SEC_MODEL_SERVICE_BASE_URL";
const ENV_TIMEOUT: &str = "AGENT_SEC_MODEL_SERVICE_TIMEOUT";

const DEFAULT_BACKEND: &str = "ollama";
const DEFAULT_BASE_URL: &str = "http://localhost:11434";
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Pause before the single retry of a transient failure; long enough for
/// an Ollama restart to finish binding, short enough to stay invisible
/// inside the middleware's per-scan budget.
const RETRY_BACKOFF: Duration = Duration::from_millis(200);

/// Upper bound for the configurable timeout; values beyond this would let a
/// single scan hang far longer than any caller expects.
const MAX_TIMEOUT_SECS: u64 = 300;

/// Errors raised by the model service client.
#[derive(Debug, Error)]
pub enum ModelServiceError {
    /// Invalid configuration (unsupported backend name, bad timeout).
    #[error("invalid model service configuration: {0}")]
    Config(String),

    /// The service is unreachable or returned an unusable response.
    #[error("model inference failed: {0}")]
    Inference(String),
}

/// Options forwarded to the backend's `options` field.
pub type ModelOptions = Map<String, Value>;

/// Parameters for a single-shot completion request.
#[derive(Debug, Clone)]
pub struct GenerateRequest<'a> {
    pub model: &'a str,
    pub prompt: &'a str,
    /// Bypass the server-side chat template; the caller supplies the
    /// fully templated prompt.
    pub raw: bool,
    /// Request per-token logprobs.
    pub logprobs: bool,
    /// How many alternatives per position to return (only when
    /// `logprobs` is set).
    pub top_logprobs: u32,
    pub options: ModelOptions,
}

/// Unified interface for local model inference services.
///
/// Implemented by [`OllamaClient`]; tests inject fakes.
pub trait ModelClient: Send + Sync {
    /// Whether `model` is available in the backend.
    ///
    /// Never fails: network errors are reported as `false` so callers can
    /// treat availability as a simple predicate.
    fn check_model(&self, model: &str) -> bool;

    /// Single-shot completion (`POST /api/generate`).
    ///
    /// # Errors
    ///
    /// Returns [`ModelServiceError::Inference`] when the service is
    /// unreachable or the response body is not valid JSON.
    fn generate(&self, request: &GenerateRequest<'_>) -> Result<Value, ModelServiceError>;

    /// Chat completion with structured messages (`POST /api/chat`).
    ///
    /// `logprobs` requests per-token log probabilities in the response;
    /// `top_logprobs` limits how many candidate tokens are returned at each
    /// position (ignored when `logprobs` is false).  Requires Ollama
    /// v0.12.11+; older versions silently omit the `logprobs` field and
    /// callers must treat that as "no confidence available".
    ///
    /// # Errors
    ///
    /// Returns [`ModelServiceError::Inference`] when the service is
    /// unreachable or the response body is not valid JSON.
    fn chat(
        &self,
        model: &str,
        messages: &[(&str, &str)],
        options: &ModelOptions,
        logprobs: bool,
        top_logprobs: u32,
    ) -> Result<Value, ModelServiceError>;
}

/// Ollama REST backend.
#[derive(Debug, Clone)]
pub struct OllamaClient {
    base_url: String,
    /// Built once and reused so repeated scans share the connection pool
    /// instead of paying a fresh TCP + TLS handshake per request.
    agent: ureq::Agent,
}

impl OllamaClient {
    /// Build a client for `base_url` with the given request timeout.
    ///
    /// A trailing slash in `base_url` is stripped so path concatenation
    /// never produces a double slash.
    ///
    /// `timeout` bounds connect, read and write alike.  Setting the connect
    /// phase explicitly matters: ureq defaults it to 30s, so leaving it
    /// unset would let a single request block for `timeout + 30s` when the
    /// host is unreachable rather than the configured budget.
    pub fn new(base_url: impl Into<String>, timeout: Duration) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(timeout)
            .timeout_read(timeout)
            .timeout_write(timeout)
            .build();
        OllamaClient { base_url, agent }
    }
}

impl ModelClient for OllamaClient {
    fn check_model(&self, model: &str) -> bool {
        let url = format!("{}/api/tags", self.base_url);
        let response = match self.agent.get(&url).call() {
            Ok(response) => response,
            Err(err) => {
                log::warn!("Ollama check_model failed (url={}): {err}", self.base_url);
                return false;
            }
        };
        let body: Value = match response.into_json() {
            Ok(body) => body,
            Err(err) => {
                log::warn!("Ollama check_model returned invalid JSON: {err}");
                return false;
            }
        };
        let names: Vec<&str> = body
            .get("models")
            .and_then(Value::as_array)
            .map(|models| {
                models
                    .iter()
                    .filter_map(|m| m.get("name").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        // Match the exact name or a name:tag prefix, so "warden" matches
        // "warden:latest" but not "warden-tmp".
        let prefix = format!("{model}:");
        let found = names
            .iter()
            .any(|name| *name == model || name.starts_with(&prefix));
        if found {
            log::info!("Model '{model}' verified in Ollama.");
        } else {
            log::warn!("Ollama reachable but model '{model}' not in: {names:?}");
        }
        found
    }

    fn generate(&self, request: &GenerateRequest<'_>) -> Result<Value, ModelServiceError> {
        let mut payload = Map::new();
        payload.insert("model".into(), json!(request.model));
        payload.insert("prompt".into(), json!(request.prompt));
        payload.insert("stream".into(), json!(false));
        payload.insert("raw".into(), json!(request.raw));
        if request.logprobs {
            payload.insert("logprobs".into(), json!(true));
            payload.insert("top_logprobs".into(), json!(request.top_logprobs));
        }
        if !request.options.is_empty() {
            payload.insert("options".into(), Value::Object(request.options.clone()));
        }
        self.post("/api/generate", Value::Object(payload))
    }

    fn chat(
        &self,
        model: &str,
        messages: &[(&str, &str)],
        options: &ModelOptions,
        logprobs: bool,
        top_logprobs: u32,
    ) -> Result<Value, ModelServiceError> {
        let messages: Vec<Value> = messages
            .iter()
            .map(|(role, content)| json!({"role": role, "content": content}))
            .collect();
        let mut payload = Map::new();
        payload.insert("model".into(), json!(model));
        payload.insert("messages".into(), Value::Array(messages));
        payload.insert("stream".into(), json!(false));
        if logprobs {
            payload.insert("logprobs".into(), json!(true));
            payload.insert("top_logprobs".into(), json!(top_logprobs));
        }
        if !options.is_empty() {
            payload.insert("options".into(), Value::Object(options.clone()));
        }
        self.post("/api/chat", Value::Object(payload))
    }
}

impl OllamaClient {
    /// POST `payload` to `path` and parse the JSON response body.
    ///
    /// Transient failures (see [`is_transient`]) are retried once after
    /// [`RETRY_BACKOFF`], since middleware callers issue a request per scan
    /// and a lone hiccup would otherwise fail the whole hook.
    fn post(&self, path: &str, payload: Value) -> Result<Value, ModelServiceError> {
        let url = format!("{}{path}", self.base_url);
        let response = match self.send(&url, &payload) {
            Err(err) if is_transient(&err) => {
                log::warn!("Ollama request failed (url={url}): {err}; retrying once");
                std::thread::sleep(RETRY_BACKOFF);
                self.send(&url, &payload)
            }
            attempt => attempt,
        }
        .map_err(|err| {
            ModelServiceError::Inference(format!("Ollama request failed (url={url}): {err}"))
        })?;
        response.into_json().map_err(|err| {
            ModelServiceError::Inference(format!("Ollama returned invalid JSON: {err}"))
        })
    }

    /// Single POST attempt, kept separate so `post` can retry it.
    // The large Err (ureq::Error embeds a Response) is consumed immediately
    // by `post`; boxing it would only obscure the transient-error check.
    #[allow(clippy::result_large_err)]
    fn send(&self, url: &str, payload: &Value) -> Result<ureq::Response, ureq::Error> {
        self.agent
            .post(url)
            .set("Content-Type", "application/json")
            .send_json(payload)
    }
}

/// Whether `err` is transient enough that one short-backoff retry can
/// realistically succeed: an HTTP 5xx or a failed connect (refused or
/// connect timeout, e.g. Ollama mid-restart).  Read timeouts map to
/// `ErrorKind::Io` and are deliberately excluded — retrying them would
/// double the caller's latency budget on slow inference instead of
/// masking a transient fault.
fn is_transient(err: &ureq::Error) -> bool {
    match err {
        ureq::Error::Status(code, _) => *code >= 500,
        ureq::Error::Transport(transport) => transport.kind() == ureq::ErrorKind::ConnectionFailed,
    }
}

/// Build a client from the environment.
///
/// A fresh client per call keeps configuration read-on-use, which suits the
/// one-process-per-scan CLI.  Requests issued through the same client still
/// share its connection pool — see [`OllamaClient::new`].
///
/// # Errors
///
/// Returns [`ModelServiceError::Config`] for an unsupported backend name or
/// a base URL without an `http://`/`https://` scheme.  An unparseable or
/// out-of-range timeout silently falls back to the default, matching the
/// tolerant behaviour of the surrounding tooling.
pub fn create_client() -> Result<Box<dyn ModelClient>, ModelServiceError> {
    Ok(Box::new(ollama_from_env()?))
}

/// Read the Ollama configuration from the environment.
fn ollama_from_env() -> Result<OllamaClient, ModelServiceError> {
    let backend = env_or(ENV_BACKEND, DEFAULT_BACKEND);
    if backend != DEFAULT_BACKEND {
        return Err(ModelServiceError::Config(format!(
            "Unsupported model service backend: {backend:?}"
        )));
    }
    let base_url = env_or(ENV_BASE_URL, DEFAULT_BASE_URL);
    validate_base_url(&base_url)?;
    let timeout_secs = timeout_secs_or_default(std::env::var(ENV_TIMEOUT).ok());
    Ok(OllamaClient::new(
        base_url,
        Duration::from_secs(timeout_secs),
    ))
}

/// Reject a base URL whose scheme is not `http://` or `https://`, and warn
/// when it targets a non-loopback host.  The URL comes from an environment
/// variable, so a hijacked value (compromised orchestration config, injected
/// `.env`) would silently exfiltrate every scanned prompt to that host.
fn validate_base_url(base_url: &str) -> Result<(), ModelServiceError> {
    if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
        return Err(ModelServiceError::Config(format!(
            "base_url must use http:// or https:// scheme: {base_url:?}"
        )));
    }
    if !is_loopback_url(base_url) {
        log::warn!(
            "Model service base_url points to a non-local host: {base_url}; \
             scanned prompts will be sent to it"
        );
    }
    Ok(())
}

/// Whether the URL's host is `localhost` or a loopback IP (any `127.x.x.x`
/// or `::1`).  Assumes a valid `http(s)://` prefix has already been checked.
fn is_loopback_url(base_url: &str) -> bool {
    let after_scheme = base_url
        .split_once("://")
        .map_or(base_url, |(_, rest)| rest);
    let authority = after_scheme.split('/').next().unwrap_or("");
    // Bracketed IPv6 keeps its colons inside `[...]`; otherwise the first
    // colon separates host from port.
    let host = match authority.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or(""),
        None => authority.split(':').next().unwrap_or(""),
    };
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Parse a timeout in seconds, falling back to [`DEFAULT_TIMEOUT_SECS`] when
/// the value is missing, unparseable, zero, or above [`MAX_TIMEOUT_SECS`].
fn timeout_secs_or_default(raw: Option<String>) -> u64 {
    raw.and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|secs| (1..=MAX_TIMEOUT_SECS).contains(secs))
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Spawn a minimal keep-alive HTTP/1.1 server that answers `expected`
    /// GET requests, returning its port and the accepted-connection counter.
    fn spawn_counting_server(
        expected: usize,
    ) -> (u16, Arc<AtomicUsize>, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("local addr").port();
        let accepted = Arc::new(AtomicUsize::new(0));
        let accepted_in_server = Arc::clone(&accepted);

        let handle = std::thread::spawn(move || {
            let mut served = 0;
            for stream in listener.incoming() {
                let mut stream = stream.expect("accept");
                accepted_in_server.fetch_add(1, Ordering::SeqCst);
                let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
                while served < expected {
                    let mut request_line = String::new();
                    if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
                        break; // client closed the connection
                    }
                    loop {
                        let mut header = String::new();
                        if reader.read_line(&mut header).unwrap_or(0) == 0 {
                            break;
                        }
                        if header == "\r\n" || header == "\n" {
                            break;
                        }
                    }
                    stream
                        .write_all(
                            b"HTTP/1.1 200 OK\r\n\
                              Content-Type: application/json\r\n\
                              Content-Length: 2\r\n\r\n{}",
                        )
                        .expect("write response");
                    stream.flush().ok();
                    served += 1;
                }
                if served >= expected {
                    break;
                }
            }
        });
        (port, accepted, handle)
    }

    /// Spawn a minimal HTTP/1.1 server that answers each request with the
    /// next status in `statuses` (body `{}`), returning its port and a
    /// served-request counter.  Exits once every status has been sent.
    fn spawn_scripted_server(
        statuses: &'static [u16],
    ) -> (u16, Arc<AtomicUsize>, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("local addr").port();
        let served = Arc::new(AtomicUsize::new(0));
        let served_in_server = Arc::clone(&served);

        let handle = std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.expect("accept");
                let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
                loop {
                    let mut request_line = String::new();
                    if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
                        break; // client closed the connection
                    }
                    let mut content_length = 0usize;
                    loop {
                        let mut header = String::new();
                        if reader.read_line(&mut header).unwrap_or(0) == 0 {
                            break;
                        }
                        if header == "\r\n" || header == "\n" {
                            break;
                        }
                        let lower = header.to_ascii_lowercase();
                        if let Some(value) = lower.strip_prefix("content-length:") {
                            content_length = value.trim().parse().unwrap_or(0);
                        }
                    }
                    // Consume the request body so the client never sees a
                    // reset while still writing.
                    let mut body = vec![0u8; content_length];
                    reader.read_exact(&mut body).ok();

                    let index = served_in_server.fetch_add(1, Ordering::SeqCst);
                    let status = statuses.get(index).copied().unwrap_or(200);
                    let reason = if status < 400 { "OK" } else { "Error" };
                    let response = format!(
                        "HTTP/1.1 {status} {reason}\r\n\
                         Content-Type: application/json\r\n\
                         Content-Length: 2\r\n\r\n{{}}"
                    );
                    stream
                        .write_all(response.as_bytes())
                        .expect("write response");
                    stream.flush().ok();
                    if index + 1 >= statuses.len() {
                        return;
                    }
                }
            }
        });
        (port, served, handle)
    }

    /// Minimal generate request for retry tests.
    fn generate_request(prompt: &str) -> GenerateRequest<'_> {
        GenerateRequest {
            model: "warden",
            prompt,
            raw: true,
            logprobs: false,
            top_logprobs: 0,
            options: Map::new(),
        }
    }

    #[test]
    fn timeout_in_range_is_used() {
        assert_eq!(timeout_secs_or_default(Some("45".into())), 45);
        assert_eq!(timeout_secs_or_default(Some("1".into())), 1);
        assert_eq!(
            timeout_secs_or_default(Some(MAX_TIMEOUT_SECS.to_string())),
            MAX_TIMEOUT_SECS
        );
    }

    #[test]
    fn timeout_out_of_range_falls_back_to_default() {
        assert_eq!(
            timeout_secs_or_default(Some("0".into())),
            DEFAULT_TIMEOUT_SECS
        );
        assert_eq!(
            timeout_secs_or_default(Some((MAX_TIMEOUT_SECS + 1).to_string())),
            DEFAULT_TIMEOUT_SECS
        );
    }

    #[test]
    fn timeout_missing_or_unparseable_falls_back_to_default() {
        assert_eq!(timeout_secs_or_default(None), DEFAULT_TIMEOUT_SECS);
        assert_eq!(
            timeout_secs_or_default(Some("not-a-number".into())),
            DEFAULT_TIMEOUT_SECS
        );
        assert_eq!(
            timeout_secs_or_default(Some("-5".into())),
            DEFAULT_TIMEOUT_SECS
        );
    }

    #[test]
    fn base_url_without_http_scheme_is_rejected() {
        for bad in [
            "ftp://localhost:11434",
            "file:///etc/passwd",
            "localhost:11434",
            "//attacker.example",
        ] {
            assert!(
                matches!(validate_base_url(bad), Err(ModelServiceError::Config(_))),
                "{bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn base_url_with_http_scheme_is_accepted() {
        assert!(validate_base_url("http://localhost:11434").is_ok());
        assert!(validate_base_url("https://model.internal:8443").is_ok());
    }

    #[test]
    fn loopback_detection_matches_local_hosts_only() {
        assert!(is_loopback_url("http://localhost:11434"));
        assert!(is_loopback_url("http://127.0.0.1:11434"));
        assert!(is_loopback_url("http://127.1.2.3:11434/api"));
        assert!(is_loopback_url("http://[::1]:11434"));
        assert!(!is_loopback_url("http://attacker.example:11434"));
        assert!(!is_loopback_url("http://10.0.0.5:11434"));
        assert!(!is_loopback_url("http://[2001:db8::1]:11434"));
    }

    #[test]
    fn base_url_trailing_slash_is_stripped() {
        let client = OllamaClient::new("http://localhost:11434/", Duration::from_secs(1));
        assert_eq!(client.base_url, "http://localhost:11434");
    }

    #[test]
    fn requests_reuse_one_pooled_connection() {
        let (port, accepted, server) = spawn_counting_server(2);
        let client = OllamaClient::new(format!("http://127.0.0.1:{port}"), Duration::from_secs(5));
        client.check_model("a");
        client.check_model("b");
        server.join().expect("server thread");

        assert_eq!(
            accepted.load(Ordering::SeqCst),
            1,
            "two requests must share one pooled connection"
        );
    }

    #[test]
    fn unreachable_service_reports_model_missing() {
        // Port 1 is never a live Ollama; check_model must not propagate.
        let client = OllamaClient::new("http://127.0.0.1:1", Duration::from_millis(50));
        assert!(!client.check_model("qwen3guard:0.6b"));
    }

    #[test]
    fn unreachable_service_generate_is_inference_error() {
        let client = OllamaClient::new("http://127.0.0.1:1", Duration::from_millis(50));
        let request = GenerateRequest {
            model: "warden",
            prompt: "hi",
            raw: true,
            logprobs: true,
            top_logprobs: 10,
            options: Map::new(),
        };
        assert!(matches!(
            client.generate(&request),
            Err(ModelServiceError::Inference(_))
        ));
    }

    #[test]
    fn transient_5xx_is_retried_once_and_succeeds() {
        let (port, served, server) = spawn_scripted_server(&[500, 200]);
        let client = OllamaClient::new(format!("http://127.0.0.1:{port}"), Duration::from_secs(5));
        let result = client.generate(&generate_request("hi"));
        server.join().expect("server thread");

        assert!(result.is_ok(), "retry after a 500 must succeed: {result:?}");
        assert_eq!(served.load(Ordering::SeqCst), 2, "exactly one retry");
    }

    #[test]
    fn persistent_5xx_fails_after_single_retry() {
        let (port, served, server) = spawn_scripted_server(&[500, 500]);
        let client = OllamaClient::new(format!("http://127.0.0.1:{port}"), Duration::from_secs(5));
        let result = client.generate(&generate_request("hi"));
        server.join().expect("server thread");

        assert!(matches!(result, Err(ModelServiceError::Inference(_))));
        assert_eq!(served.load(Ordering::SeqCst), 2, "one retry, then give up");
    }

    #[test]
    fn client_error_is_not_retried() {
        let (port, served, server) = spawn_scripted_server(&[400]);
        let client = OllamaClient::new(format!("http://127.0.0.1:{port}"), Duration::from_secs(5));
        let result = client.generate(&generate_request("hi"));
        server.join().expect("server thread");

        assert!(matches!(result, Err(ModelServiceError::Inference(_))));
        assert_eq!(served.load(Ordering::SeqCst), 1, "4xx must not be retried");
    }
}
