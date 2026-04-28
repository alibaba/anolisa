//! SLS (Aliyun Log Service) uploader for GenAI semantic events
//!
//! This module provides a non-blocking uploader that sends GenAI semantic events
//! to Aliyun SLS via a background thread with its own tokio runtime.

use crossbeam_channel::{Sender, Receiver, bounded};
use std::collections::BTreeMap;
use std::thread;

use crate::config::AgentsightConfig;
use super::semantic::GenAISemanticEvent;
use super::exporter::GenAIExporter;

/// SLS uploader that sends GenAI events to Aliyun Log Service
///
/// Uses a background thread with a dedicated tokio runtime to handle
/// async SLS SDK calls without blocking the main sync pipeline.
pub struct SlsUploader {
    sender: Sender<Vec<GenAISemanticEvent>>,
}

impl SlsUploader {
    /// Create a new SLS uploader from configuration
    ///
    /// Spawns a background thread with a tokio runtime that consumes
    /// events from a channel and uploads them to SLS.
    pub fn new(config: &AgentsightConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let endpoint = config.sls_endpoint.clone()
            .ok_or("SLS endpoint not configured")?;
        let access_key_id = config.sls_access_key_id.clone()
            .ok_or("SLS access key ID not configured")?;
        let access_key_secret = config.sls_access_key_secret.clone()
            .ok_or("SLS access key secret not configured")?;
        let project = config.sls_project.clone()
            .ok_or("SLS project not configured")?;
        let logstore = config.sls_logstore.clone()
            .ok_or("SLS logstore not configured")?;

        // Bounded channel to apply backpressure if SLS is slow
        let (sender, receiver): (Sender<Vec<GenAISemanticEvent>>, Receiver<Vec<GenAISemanticEvent>>) =
            bounded(256);

        // Spawn background thread with its own tokio runtime
        thread::Builder::new()
            .name("sls-uploader".to_string())
            .spawn(move || {
                Self::run_upload_loop(endpoint, access_key_id, access_key_secret, project, logstore, receiver);
            })?;

        log::info!("SLS uploader started (project={}, logstore={})", 
            config.sls_project.as_deref().unwrap_or("?"),
            config.sls_logstore.as_deref().unwrap_or("?"));

        Ok(SlsUploader { sender })
    }

    /// Send events to the background upload thread (non-blocking)
    ///
    /// If the channel is full, the events are dropped with a warning log.
    pub fn send(&self, events: Vec<GenAISemanticEvent>) {
        if events.is_empty() {
            return;
        }
        match self.sender.try_send(events) {
            Ok(_) => {}
            Err(crossbeam_channel::TrySendError::Full(dropped)) => {
                log::warn!("SLS upload channel full, dropping {} events", dropped.len());
            }
            Err(crossbeam_channel::TrySendError::Disconnected(_)) => {
                log::error!("SLS upload thread has disconnected");
            }
        }
    }
}

impl GenAIExporter for SlsUploader {
    fn name(&self) -> &str {
        "aliyun-sls"
    }

    fn export(&self, events: &[GenAISemanticEvent]) {
        self.send(events.to_vec());
    }
}

impl SlsUploader {
    fn run_upload_loop(
        endpoint: String,
        access_key_id: String,
        access_key_secret: String,
        project: String,
        logstore: String,
        receiver: Receiver<Vec<GenAISemanticEvent>>,
    ) {
        use aliyun_log_rust_sdk::{Client, Config, FromConfig};

        // Build tokio runtime for this thread
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                log::error!("Failed to create tokio runtime for SLS uploader: {}", e);
                return;
            }
        };

        // Create SLS client
        let sls_config = match Config::builder()
            .endpoint(&endpoint)
            .access_key(&access_key_id, &access_key_secret)
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                log::error!("Failed to build SLS config: {}", e);
                return;
            }
        };

        let client = match Client::from_config(sls_config) {
            Ok(c) => c,
            Err(e) => {
                log::error!("Failed to create SLS client: {}", e);
                return;
            }
        };

        log::debug!("SLS upload thread started, waiting for events...");

        // Process events from channel
        while let Ok(events) = receiver.recv() {
            let log_group = Self::events_to_log_group(&events);

            rt.block_on(async {
                match client.put_logs(&project, &logstore)
                    .log_group(log_group)
                    .send()
                    .await
                {
                    Ok(_) => {
                        log::debug!("Uploaded {} GenAI events to SLS", events.len());
                    }
                    Err(e) => {
                        log::warn!("Failed to upload to SLS: {}", e);
                    }
                }
            });
        }

        log::info!("SLS upload thread exiting (channel closed)");
    }

    /// Convert GenAI semantic events to SLS LogGroup
    ///
    /// Thin wrapper: calls `events_to_flat_records()` then converts to protobuf LogGroup.
    fn events_to_log_group(events: &[GenAISemanticEvent]) -> aliyun_log_sdk_protobuf::LogGroup {
        use aliyun_log_sdk_protobuf::{Log, LogGroup};

        let mut log_group = LogGroup::new();
        let records = events_to_flat_records(events);

        for record in &records {
            let timestamp = record.get("__time__")
                .and_then(|t| t.parse::<u32>().ok())
                .unwrap_or_else(|| chrono::Utc::now().timestamp() as u32);
            let mut log = Log::from_unixtime(timestamp);

            for (key, value) in record {
                // Skip iLogtail reserved fields for PutLogs path
                if key.starts_with("__") {
                    continue;
                }
                log.add_content_kv(key, value);
            }

            log_group.add_log(log);
        }

        log_group
    }
}

/// 将 GenAI 语义事件转换为扁平化 key-value 记录
///
/// 返回 `Vec<BTreeMap<String, String>>`，每个 BTreeMap 代表一条日志记录。
/// 字段命名遵循 OTel GenAI 标准和 AgentSight 扩展规范。
/// 包含 iLogtail 保留字段：`__time__`、`__source__`、`__topic__`。
///
/// 此函数被 SLS PutLogs 上传器和 Logtail 文件导出器共享使用。
pub fn events_to_flat_records(events: &[GenAISemanticEvent]) -> Vec<BTreeMap<String, String>> {
    let hostname = super::instance_id::get_instance_id();
    let mut records = Vec::with_capacity(events.len());

    for event in events {
        let mut m = BTreeMap::new();
        let timestamp = chrono::Utc::now().timestamp();

        // iLogtail 保留字段
        m.insert("__time__".to_string(), timestamp.to_string());
        m.insert("__source__".to_string(), hostname.clone());
        m.insert("__topic__".to_string(), "agentsight".to_string());

        // 每条日志都写入 instance
        m.insert("instance".to_string(), hostname.clone());

        match event {
            GenAISemanticEvent::LLMCall(call) => {
                // ── OTel GenAI Required ──
                m.insert("gen_ai.provider.name".to_string(), call.provider.clone());
                m.insert("gen_ai.request.model".to_string(), call.model.clone());
                m.insert("gen_ai.operation.name".to_string(),
                    call.metadata.get("operation_name").cloned().unwrap_or_else(|| "chat".to_string()));

                // ── OTel GenAI Conditionally Required ──
                if let Some(ref error) = call.error {
                    m.insert("error.type".to_string(), error.clone());
                }
                if let Some(port) = call.metadata.get("server.port") {
                    m.insert("server.port".to_string(), port.clone());
                }

                // ── OTel GenAI Recommended ──
                if let Some(rid) = call.metadata.get("response_id") {
                    m.insert("gen_ai.response.id".to_string(), rid.clone());
                } else {
                    m.insert("gen_ai.response.id".to_string(), call.call_id.clone());
                }
                m.insert("gen_ai.response.model".to_string(), call.model.clone());
                if let Some(reason) = call.response.messages.first().and_then(|msg| msg.finish_reason.as_ref()) {
                    m.insert("gen_ai.response.finish_reasons".to_string(), format!("[\"{}\"]", reason));
                }
                if let Some(temp) = call.request.temperature {
                    m.insert("gen_ai.request.temperature".to_string(), temp.to_string());
                }
                if let Some(max) = call.request.max_tokens {
                    m.insert("gen_ai.request.max_tokens".to_string(), max.to_string());
                }
                if let Some(fp) = call.request.frequency_penalty {
                    m.insert("gen_ai.request.frequency_penalty".to_string(), fp.to_string());
                }
                if let Some(pp) = call.request.presence_penalty {
                    m.insert("gen_ai.request.presence_penalty".to_string(), pp.to_string());
                }
                if let Some(tp) = call.request.top_p {
                    m.insert("gen_ai.request.top_p".to_string(), tp.to_string());
                }
                if let Some(tk) = call.request.top_k {
                    m.insert("gen_ai.request.top_k".to_string(), tk.to_string());
                }
                if let Some(seed) = call.request.seed {
                    m.insert("gen_ai.request.seed".to_string(), seed.to_string());
                }
                if let Some(ref usage) = call.token_usage {
                    m.insert("gen_ai.usage.input_tokens".to_string(), usage.input_tokens.to_string());
                    m.insert("gen_ai.usage.output_tokens".to_string(), usage.output_tokens.to_string());
                    if let Some(cache_create) = usage.cache_creation_input_tokens {
                        m.insert("gen_ai.usage.cache_creation.input_tokens".to_string(), cache_create.to_string());
                    }
                    if let Some(cache_read) = usage.cache_read_input_tokens {
                        m.insert("gen_ai.usage.cache_read.input_tokens".to_string(), cache_read.to_string());
                    }
                }
                if let Some(addr) = call.metadata.get("server.address") {
                    m.insert("server.address".to_string(), addr.clone());
                }
                m.insert("gen_ai.output.type".to_string(), "text".to_string());

                // ── AgentSight extensions ──
                m.insert("agentsight.pid".to_string(), call.pid.to_string());
                m.insert("agentsight.process_name".to_string(), call.process_name.clone());
                if let Some(ref name) = call.agent_name {
                    m.insert("agentsight.agent.name".to_string(), name.clone());
                }
                m.insert("agentsight.duration_ns".to_string(), call.duration_ns.to_string());
                m.insert("agentsight.start_timestamp_ns".to_string(), call.start_timestamp_ns.to_string());
                m.insert("agentsight.end_timestamp_ns".to_string(), call.end_timestamp_ns.to_string());
                if let Some(method) = call.metadata.get("method") {
                    m.insert("agentsight.http.method".to_string(), method.clone());
                }
                if let Some(path) = call.metadata.get("path") {
                    m.insert("agentsight.http.path".to_string(), path.clone());
                }
                if let Some(status) = call.metadata.get("status_code") {
                    m.insert("agentsight.http.status_code".to_string(), status.clone());
                }
                if call.request.stream || call.metadata.get("is_sse").map(|v| v == "true").unwrap_or(false) {
                    m.insert("agentsight.stream".to_string(), "true".to_string());
                    if let Some(cnt) = call.metadata.get("sse_event_count") {
                        m.insert("agentsight.sse_event_count".to_string(), cnt.clone());
                    }
                }
                if let Some(rid) = call.metadata.get("response_id") {
                    m.insert("trace_id".to_string(), rid.clone());
                } else {
                    m.insert("trace_id".to_string(), call.call_id.clone());
                }
                if let Some(cid) = call.metadata.get("conversation_id") {
                    m.insert("gen_ai.conversation.id".to_string(), cid.clone());
                }
                if let Some(sid) = call.metadata.get("session_id") {
                    m.insert("gen_ai.session.id".to_string(), sid.clone());
                }
            }
            GenAISemanticEvent::ToolUse(tool) => {
                m.insert("gen_ai.operation.name".to_string(), "tool_use".to_string());
                m.insert("gen_ai.tool.name".to_string(), tool.tool_name.clone());
                if let Some(ref parent_id) = tool.parent_llm_call_id {
                    m.insert("gen_ai.response.id".to_string(), parent_id.clone());
                }
                m.insert("agentsight.tool.success".to_string(), tool.success.to_string());
                m.insert("agentsight.pid".to_string(), tool.pid.to_string());
                if let Some(ref dur) = tool.duration_ns {
                    m.insert("agentsight.duration_ns".to_string(), dur.to_string());
                }
                if let Some(ref error) = tool.error {
                    m.insert("error.type".to_string(), error.clone());
                }
            }
            GenAISemanticEvent::AgentInteraction(interaction) => {
                m.insert("gen_ai.operation.name".to_string(), "agent_interaction".to_string());
                m.insert("agentsight.agent.name".to_string(), interaction.agent_name.clone());
                m.insert("agentsight.agent.interaction_type".to_string(), interaction.interaction_type.clone());
                m.insert("agentsight.pid".to_string(), interaction.pid.to_string());
            }
            GenAISemanticEvent::StreamChunk(chunk) => {
                m.insert("gen_ai.operation.name".to_string(), "stream_chunk".to_string());
                m.insert("agentsight.stream.id".to_string(), chunk.stream_id.clone());
                m.insert("agentsight.stream.chunk_index".to_string(), chunk.chunk_index.to_string());
                m.insert("agentsight.pid".to_string(), chunk.pid.to_string());
            }
        }

        records.push(m);
    }

    records
}
