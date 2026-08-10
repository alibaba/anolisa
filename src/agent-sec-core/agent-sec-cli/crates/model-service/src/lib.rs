//! Shared HTTP client for local model inference backends.
//!
//! Only Ollama is supported today.  Configuration is read from the same
//! environment variables the rest of the toolchain uses:
//!
//! - `AGENT_SEC_MODEL_SERVICE_BACKEND` (default `ollama`)
//! - `AGENT_SEC_MODEL_SERVICE_BASE_URL` (default `http://localhost:11434`)
//! - `AGENT_SEC_MODEL_SERVICE_TIMEOUT` seconds (default `30`)
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
    fn post(&self, path: &str, payload: Value) -> Result<Value, ModelServiceError> {
        let url = format!("{}{path}", self.base_url);
        let response = self
            .agent
            .post(&url)
            .set("Content-Type", "application/json")
            .send_json(payload)
            .map_err(|err| {
                ModelServiceError::Inference(format!("Ollama request failed (url={url}): {err}"))
            })?;
        response.into_json().map_err(|err| {
            ModelServiceError::Inference(format!("Ollama returned invalid JSON: {err}"))
        })
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
/// Returns [`ModelServiceError::Config`] for an unsupported backend name.  An
/// unparseable timeout silently falls back to the default, matching the
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
    let timeout_secs = std::env::var(ENV_TIMEOUT)
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .unwrap_or(DEFAULT_TIMEOUT_SECS);
    Ok(OllamaClient::new(
        base_url,
        Duration::from_secs(timeout_secs),
    ))
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
    use std::io::{BufRead, BufReader, Write};
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
}
