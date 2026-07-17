//! SysOM Provider — Aliyun SysOM `generate_copilot_stream_response` API client.
//!
//! Uses ACS3-HMAC-SHA256 signing and parses the cumulative SSE stream format
//! into incremental `GenerateEvent`s compatible with `ContentGenerator` trait.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use futures::StreamExt;
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    ContentGenerator, GenerateConfig, GenerateEvent, GenerateStream, Message, ToolDeclaration,
};

type HmacSha256 = Hmac<Sha256>;

const DEFAULT_ENDPOINT: &str = "sysom.cn-hangzhou.aliyuncs.com";
const API_PATH: &str = "/api/v1/copilot/generate_copilot_stream_response";
const API_VERSION: &str = "2023-12-30";
const API_ACTION: &str = "GenerateCopilotStreamResponse";
const ECS_METADATA_ENDPOINT: &str = "http://100.100.100.200";
const ECS_RAM_ROLE_NAME: &str = "AliyunECSInstanceForSysomRole";
const CONSOLE_URL_TEMPLATE: &str =
    "https://alinux.console.aliyun.com/{regionId}/guide/cosh?instance={instanceId}";

/// Cache TTL for instance_id (3 hours).
const INSTANCE_ID_CACHE_TTL_SECS: u64 = 3 * 3600;
/// Connect timeout for ECS metadata service.
const METADATA_CONNECT_TIMEOUT: Duration = Duration::from_secs(1);
/// Read timeout for ECS metadata service.
const METADATA_READ_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EcsAuthChallenge {
    pub instance_id: String,
    pub console_url: String,
}

/// Credentials that can be refreshed at runtime (for STS).
#[derive(Debug, Clone)]
struct SysomCredentials {
    access_key_id: String,
    access_key_secret: String,
    security_token: Option<String>,
}

/// SysOM Provider that connects to Aliyun SysOM API with ACS3-HMAC-SHA256 signing.
pub struct SysomProvider {
    endpoint: String,
    credentials: RwLock<SysomCredentials>,
    is_sts: bool,
    cancelled: Arc<AtomicBool>,
    instance_id: Option<String>,
}

impl SysomProvider {
    pub fn new(access_key_id: &str, access_key_secret: &str, security_token: Option<&str>) -> Self {
        let is_sts = security_token.is_some();
        let instance_id = resolve_instance_id();
        Self {
            endpoint: DEFAULT_ENDPOINT.to_string(),
            credentials: RwLock::new(SysomCredentials {
                access_key_id: access_key_id.to_string(),
                access_key_secret: access_key_secret.to_string(),
                security_token: security_token.map(|s| s.to_string()),
            }),
            is_sts,
            cancelled: Arc::new(AtomicBool::new(false)),
            instance_id,
        }
    }

    pub fn from_ecs_ram_role() -> Self {
        let instance_id = resolve_instance_id();
        Self {
            endpoint: DEFAULT_ENDPOINT.to_string(),
            credentials: RwLock::new(SysomCredentials {
                access_key_id: String::new(),
                access_key_secret: String::new(),
                security_token: None,
            }),
            is_sts: true,
            cancelled: Arc::new(AtomicBool::new(false)),
            instance_id,
        }
    }

    pub fn with_endpoint(mut self, endpoint: &str) -> Self {
        self.endpoint = endpoint.to_string();
        self
    }

    /// Build the JSON request body for the SysOM API.
    fn build_request_body(
        &self,
        messages: &[Message],
        tools: &[ToolDeclaration],
        config: &GenerateConfig,
    ) -> Value {
        let mut safe_messages = messages.to_vec();
        crate::redaction::redact_messages(&mut safe_messages);
        let mut inner = serde_json::json!({
            "messages": safe_messages,
            "model": config.model,
            "stream": true,
            "use_dashscope": true,
            "version": 2,
            "max_tokens": config.max_tokens,
        });

        if let Some(temp) = config.temperature {
            inner["temperature"] = serde_json::json!(temp);
        }

        if !tools.is_empty() {
            let tool_defs: Vec<Value> = tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters,
                        }
                    })
                })
                .collect();
            inner["tools"] = serde_json::json!(tool_defs);
            inner["tool_choice"] = serde_json::json!("auto");
        }

        if let Some(extra) = &config.extra_params {
            if let (Some(inner_obj), Some(extra_obj)) = (inner.as_object_mut(), extra.as_object()) {
                for (k, v) in extra_obj {
                    inner_obj.insert(k.clone(), v.clone());
                }
            }
        }

        if let Some(ref id) = self.instance_id {
            inner["instance_id"] = serde_json::json!(id);
        }

        // Wrap in llmParamString
        serde_json::json!({
            "llmParamString": inner.to_string()
        })
    }

    /// Compute ACS3-HMAC-SHA256 authorization header.
    fn sign_request(
        &self,
        method: &str,
        pathname: &str,
        headers: &[(String, String)],
        hashed_payload: &str,
        creds: &SysomCredentials,
    ) -> String {
        let signature_algorithm = "ACS3-HMAC-SHA256";

        // Build canonical headers and signed headers list
        // Sort headers by lowercase key
        let mut sorted_headers: Vec<(String, String)> = headers
            .iter()
            .map(|(k, v)| (k.to_lowercase(), v.trim().to_string()))
            .collect();
        sorted_headers.sort_by(|a, b| a.0.cmp(&b.0));

        let canonical_headers: String = sorted_headers
            .iter()
            .map(|(k, v)| format!("{}:{}\n", k, v))
            .collect();

        let signed_headers: Vec<&str> = sorted_headers.iter().map(|(k, _)| k.as_str()).collect();
        let signed_headers_str = signed_headers.join(";");

        // Canonical request
        let canonical_request = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            method,
            pathname,
            "", // query string (empty)
            canonical_headers,
            signed_headers_str,
            hashed_payload,
        );

        // String to sign
        let hashed_canonical = hex_sha256(canonical_request.as_bytes());
        let string_to_sign = format!("{}\n{}", signature_algorithm, hashed_canonical);

        // Signature
        let signature = hex_hmac_sha256(
            creds.access_key_secret.as_bytes(),
            string_to_sign.as_bytes(),
        );

        format!(
            "{} Credential={},SignedHeaders={},Signature={}",
            signature_algorithm, creds.access_key_id, signed_headers_str, signature
        )
    }
}

#[async_trait]
impl ContentGenerator for SysomProvider {
    async fn generate(
        &self,
        messages: &[Message],
        tools: &[ToolDeclaration],
        config: &GenerateConfig,
    ) -> Result<GenerateStream, String> {
        self.cancelled.store(false, Ordering::SeqCst);

        let body = self.build_request_body(messages, tools, config);
        let body_bytes = serde_json::to_vec(&body).map_err(|e| format!("JSON serialize: {e}"))?;

        if self.is_sts
            && self.credentials.read().unwrap().security_token.is_none()
            && !self.refresh_sts_credentials().await
        {
            return Err("failed to refresh ECS RAM Role credentials".to_string());
        }

        // First attempt
        match self.do_streaming_request(&body_bytes).await {
            Ok(stream) => Ok(stream),
            Err(e) if self.is_sts && is_sts_error(&e) => {
                // STS credential expired — try to refresh from ECS metadata and retry once
                tracing::debug!("STS credential error, attempting refresh...");
                if self.refresh_sts_credentials().await {
                    tracing::debug!("STS credentials refreshed, retrying...");
                    self.do_streaming_request(&body_bytes).await
                } else {
                    Err(e)
                }
            }
            Err(e) => Err(e),
        }
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
}

impl SysomProvider {
    /// Send a streaming request using current credentials.
    async fn do_streaming_request(&self, body_bytes: &[u8]) -> Result<GenerateStream, String> {
        let creds = self.credentials.read().unwrap().clone();
        let url = format!("https://{}{}", self.endpoint, API_PATH);
        let timestamp = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let nonce = Uuid::new_v4().to_string();
        let hashed_payload = hex_sha256(body_bytes);

        // Build headers for signing
        let mut sign_headers: Vec<(String, String)> = vec![
            ("host".to_string(), self.endpoint.clone()),
            ("x-acs-version".to_string(), API_VERSION.to_string()),
            ("x-acs-action".to_string(), API_ACTION.to_string()),
            ("x-acs-date".to_string(), timestamp.clone()),
            ("x-acs-signature-nonce".to_string(), nonce.clone()),
            ("x-acs-content-sha256".to_string(), hashed_payload.clone()),
            (
                "content-type".to_string(),
                "application/json; charset=utf-8".to_string(),
            ),
        ];

        if let Some(ref token) = creds.security_token {
            sign_headers.push((
                "x-acs-accesskey-id".to_string(),
                creds.access_key_id.clone(),
            ));
            sign_headers.push(("x-acs-security-token".to_string(), token.clone()));
        }

        let authorization =
            self.sign_request("POST", API_PATH, &sign_headers, &hashed_payload, &creds);

        // Build reqwest request
        let client = reqwest::Client::new();
        let mut req = client
            .post(&url)
            .header("host", &self.endpoint)
            .header("x-acs-version", API_VERSION)
            .header("x-acs-action", API_ACTION)
            .header("x-acs-date", &timestamp)
            .header("x-acs-signature-nonce", &nonce)
            .header("x-acs-content-sha256", &hashed_payload)
            .header("content-type", "application/json; charset=utf-8")
            .header("accept", "text/event-stream")
            .header("x-sysom-invoke-source", "cosh")
            .header("Authorization", &authorization);

        if let Some(ref token) = creds.security_token {
            req = req
                .header("x-acs-accesskey-id", &creds.access_key_id)
                .header("x-acs-security-token", token);
        }

        let response = req
            .body(body_bytes.to_vec())
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {e}"))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response
                .text()
                .await
                .unwrap_or_else(|_| "unknown".to_string());
            return Err(format!("SysOM API error {status}: {text}"));
        }

        let cancelled = Arc::clone(&self.cancelled);
        let byte_stream = response.bytes_stream();

        // State for cumulative → incremental conversion
        let state = SseParseState::default();

        let event_stream = futures::stream::unfold(
            (byte_stream, String::new(), cancelled, state),
            |(mut stream, mut buf, cancelled, mut state)| async move {
                loop {
                    if cancelled.load(Ordering::SeqCst) || state.stream_ended {
                        return None;
                    }

                    // Try to extract a complete SSE event from buffer
                    if let Some(pos) = buf.find("\n\n") {
                        let event_block = buf[..pos].to_string();
                        buf = buf[pos + 2..].to_string();

                        if event_block.trim().is_empty() {
                            continue;
                        }

                        if let Some(event) = parse_sysom_sse_event(&event_block, &mut state) {
                            return Some((event, (stream, buf, cancelled, state)));
                        }
                        continue;
                    }

                    // Need more data
                    match stream.next().await {
                        Some(Ok(bytes)) => {
                            buf.push_str(&String::from_utf8_lossy(&bytes));
                        }
                        Some(Err(e)) => {
                            return Some((
                                GenerateEvent::Error(format!("stream error: {e}")),
                                (stream, buf, cancelled, state),
                            ));
                        }
                        None => {
                            // Stream ended — try to flush remaining buffer
                            if !buf.trim().is_empty() {
                                let event_block = buf.trim().to_string();
                                buf.clear();
                                if let Some(event) = parse_sysom_sse_event(&event_block, &mut state)
                                {
                                    return Some((event, (stream, buf, cancelled, state)));
                                }
                            }
                            // Emit final Usage if available
                            if let Some((prompt, completion, total)) = state.latest_usage.take() {
                                state.stream_ended = true;
                                return Some((
                                    GenerateEvent::Usage {
                                        prompt_tokens: prompt,
                                        completion_tokens: completion,
                                        total_tokens: total,
                                    },
                                    (stream, buf, cancelled, state),
                                ));
                            }
                            state.stream_ended = true;
                            return Some((
                                GenerateEvent::MessageEnd,
                                (stream, buf, cancelled, state),
                            ));
                        }
                    }
                }
            },
        );

        Ok(Box::pin(event_stream))
    }

    /// Check if an error indicates STS credential expiration.
    /// Refresh STS credentials from ECS metadata service.
    async fn refresh_sts_credentials(&self) -> bool {
        let url = format!(
            "{}/latest/meta-data/ram/security-credentials/{}",
            ECS_METADATA_ENDPOINT, ECS_RAM_ROLE_NAME
        );
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(2))
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap_or_default();

        let resp = match client.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("STS refresh failed: {e}");
                return false;
            }
        };
        let body: Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("STS refresh parse failed: {e}");
                return false;
            }
        };

        let ak = body.get("AccessKeyId").and_then(|v| v.as_str());
        let sk = body.get("AccessKeySecret").and_then(|v| v.as_str());
        let token = body.get("SecurityToken").and_then(|v| v.as_str());

        if let (Some(ak), Some(sk), Some(token)) = (ak, sk, token) {
            let mut creds = self.credentials.write().unwrap();
            creds.access_key_id = ak.to_string();
            creds.access_key_secret = sk.to_string();
            creds.security_token = Some(token.to_string());
            true
        } else {
            tracing::warn!("STS refresh: missing fields in response");
            false
        }
    }
}

/// Check if an error string contains STS-related error codes.
fn is_sts_error(error: &str) -> bool {
    error.contains("InvalidSecurityToken")
        || error.contains("SecurityTokenExpired")
        || error.contains("InvalidAccessKeyId")
}

// ---------------------------------------------------------------------------
// SSE parsing helpers
// ---------------------------------------------------------------------------

#[derive(Default)]
struct SseParseState {
    last_content_len: usize,
    last_tool_use_count: usize,
    /// Track accumulated arguments length per tool index
    last_tool_args_len: Vec<usize>,
    message_ended: bool,
    stream_ended: bool,
    /// Latest usage info (updated every frame, emitted at stream end)
    latest_usage: Option<(u32, u32, u32)>,
}

/// Parse a single SSE event block (lines between \n\n).
/// Returns a GenerateEvent if the block contains useful data.
fn parse_sysom_sse_event(block: &str, state: &mut SseParseState) -> Option<GenerateEvent> {
    if state.message_ended {
        return None;
    }

    let mut event_type = String::new();
    let mut data_str = String::new();

    for line in block.lines() {
        if let Some(val) = line.strip_prefix("event:") {
            event_type = val.trim().to_string();
        } else if let Some(val) = line.strip_prefix("data:") {
            data_str = val.trim().to_string();
        }
        // id: line is ignored
    }

    if event_type == "Failed" {
        state.message_ended = true;
        return Some(GenerateEvent::Error(format!(
            "SysOM stream failed: {}",
            data_str
        )));
    }

    if event_type != "OK" || data_str.is_empty() {
        return None;
    }

    let data: Value = serde_json::from_str(&data_str).ok()?;

    let choices = data.get("choices").and_then(|c| c.as_array());

    // --- Text delta (cumulative → incremental) ---
    if let Some(choices_arr) = choices {
        if let Some(message) = choices_arr.first().and_then(|c| c.get("message")) {
            let content = message
                .get("content")
                .and_then(|c| c.as_str())
                .unwrap_or("");
            if content.len() > state.last_content_len {
                let delta = &content[state.last_content_len..];
                state.last_content_len = content.len();
                return Some(GenerateEvent::TextDelta(delta.to_string()));
            }

            // --- Tool use (cumulative tool_use array) ---
            if let Some(tool_use) = message.get("tool_use").and_then(|t| t.as_array()) {
                if !tool_use.is_empty() {
                    // New tool call appeared
                    if tool_use.len() > state.last_tool_use_count {
                        let new_tc = &tool_use[state.last_tool_use_count];
                        state.last_tool_use_count = tool_use.len();
                        state.last_tool_args_len.push(0);

                        let id = new_tc
                            .get("id")
                            .and_then(|i| i.as_str())
                            .unwrap_or("")
                            .to_string();
                        let name = new_tc
                            .get("function")
                            .and_then(|f| f.get("name"))
                            .and_then(|n| n.as_str())
                            .unwrap_or("")
                            .to_string();
                        let index =
                            new_tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as u32;

                        return Some(GenerateEvent::ToolCallStart { index, id, name });
                    }

                    // Check for arguments growth on last tool call
                    let last_idx = tool_use.len() - 1;
                    if let Some(tc) = tool_use.get(last_idx) {
                        let args = tc
                            .get("function")
                            .and_then(|f| f.get("arguments"))
                            .and_then(|a| a.as_str())
                            .unwrap_or("");
                        let prev_len = state.last_tool_args_len.get(last_idx).copied().unwrap_or(0);
                        if args.len() > prev_len {
                            let delta = &args[prev_len..];
                            if let Some(slot) = state.last_tool_args_len.get_mut(last_idx) {
                                *slot = args.len();
                            }
                            return Some(GenerateEvent::ToolCallDelta {
                                index: last_idx as u32,
                                arguments_delta: delta.to_string(),
                            });
                        }
                    }
                }
            }
        }
    }

    // If no content/tool delta, check if this is the final frame
    // (no new content growth + has usage = stream is done)
    if let Some(usage) = data.get("usage").and_then(|u| u.as_object()) {
        // SysOM returns usage on every frame, but we only emit Usage event
        // when content has stopped growing (i.e., this parse produced no text delta above)
        let prompt = usage
            .get("prompt_tokens")
            .or_else(|| usage.get("input_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        let completion = usage
            .get("completion_tokens")
            .or_else(|| usage.get("output_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        let total = usage
            .get("total_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        // Store latest usage but don't emit yet — only emit when stream ends
        state.latest_usage = Some((prompt, completion, total));
    }

    None
}

// ---------------------------------------------------------------------------
// Instance ID resolution with local cache
// ---------------------------------------------------------------------------

/// Resolve instance_id: read from local cache if valid, otherwise fetch from
/// ECS metadata service and update the cache.
fn resolve_instance_id() -> Option<String> {
    let config_dir = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".copilot-shell");
    let cache_path = config_dir.join("instance_id");

    // Try reading from cache
    if let Ok(metadata) = std::fs::metadata(&cache_path) {
        if let Ok(modified) = metadata.modified() {
            let age = modified.elapsed().unwrap_or(Duration::from_secs(u64::MAX));
            if age < Duration::from_secs(INSTANCE_ID_CACHE_TTL_SECS) {
                // Cache is still valid
                let content = std::fs::read_to_string(&cache_path).unwrap_or_default();
                let trimmed = content.trim();
                if trimmed.is_empty() {
                    // Empty file = previously failed to fetch
                    return None;
                }
                return Some(trimmed.to_string());
            }
        }
    }

    // Cache miss or expired — fetch from metadata service
    let instance_id = fetch_instance_id_from_metadata();

    // Write cache (create parent dir if needed)
    if let Some(parent) = cache_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let content = instance_id.as_deref().unwrap_or("");
    if let Err(e) = std::fs::write(&cache_path, content) {
        tracing::debug!("failed to write instance_id cache: {e}");
    }

    instance_id
}

/// Fetch instance-id from ECS metadata service via raw TCP.
/// Returns None if not running on ECS or if the request fails.
fn fetch_instance_id_from_metadata() -> Option<String> {
    fetch_ecs_metadata_text("/latest/meta-data/instance-id").and_then(|body| {
        let instance_id = body.trim();
        if instance_id.starts_with("i-") {
            Some(instance_id.to_string())
        } else {
            None
        }
    })
}

pub fn detect_ecs_auth_challenge() -> Option<EcsAuthChallenge> {
    let instance_id = fetch_instance_id_from_metadata()?;
    let region_id = fetch_ecs_region_id().unwrap_or_else(|| "cn-hangzhou".to_string());
    Some(EcsAuthChallenge {
        console_url: generate_console_url(&instance_id, &region_id),
        instance_id,
    })
}

pub fn ecs_ram_role_credentials_available() -> bool {
    let path = format!(
        "/latest/meta-data/ram/security-credentials/{}",
        ECS_RAM_ROLE_NAME
    );
    fetch_ecs_metadata_text(&path)
        .map(|body| body.contains("AccessKeyId") && body.contains("SecurityToken"))
        .unwrap_or(false)
}

fn fetch_ecs_region_id() -> Option<String> {
    let zone_id = fetch_ecs_metadata_text("/latest/meta-data/zone-id")?;
    region_id_from_zone_id(zone_id.trim())
}

fn region_id_from_zone_id(zone_id: &str) -> Option<String> {
    if zone_id.is_empty() {
        return None;
    }
    if let Some(pos) = zone_id.rfind('-') {
        let suffix = &zone_id[pos + 1..];
        if suffix.len() == 1 && suffix.chars().all(|c| c.is_ascii_lowercase()) {
            return Some(zone_id[..pos].to_string());
        }
    }
    Some(zone_id.to_string())
}

fn generate_console_url(instance_id: &str, region_id: &str) -> String {
    CONSOLE_URL_TEMPLATE
        .replace("{regionId}", region_id)
        .replace("{instanceId}", instance_id)
}

fn fetch_ecs_metadata_text(path: &str) -> Option<String> {
    let addr: SocketAddr = "100.100.100.200:80".parse().ok()?;
    let mut stream = TcpStream::connect_timeout(&addr, METADATA_CONNECT_TIMEOUT).ok()?;
    stream.set_read_timeout(Some(METADATA_READ_TIMEOUT)).ok()?;
    stream
        .set_write_timeout(Some(METADATA_CONNECT_TIMEOUT))
        .ok()?;

    let request = format!("GET {path} HTTP/1.0\r\nHost: 100.100.100.200\r\n\r\n");
    stream.write_all(request.as_bytes()).ok()?;

    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;

    response.split("\r\n\r\n").nth(1).map(str::to_string)
}

// ---------------------------------------------------------------------------
// Crypto helpers
// ---------------------------------------------------------------------------

fn hex_sha256(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

fn hex_hmac_sha256(key: &[u8], data: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC can take key of any size");
    mac.update(data);
    hex::encode(mac.finalize().into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
