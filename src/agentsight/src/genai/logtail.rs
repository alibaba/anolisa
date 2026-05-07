//! iLogtail 文件导出器
//!
//! 将 GenAI 语义事件以扁平化 JSON 格式写入指定路径，
//! 由 iLogtail 自动采集上传到 SLS，无需配置 AK/SK。
//!
//! 通过环境变量 `SLS_LOGTAIL_FILE` 指定输出路径。
//! 仅当该环境变量设置时才启用。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::fs::OpenOptions;
use std::io::{Write, BufWriter};

use super::semantic::GenAISemanticEvent;
use super::exporter::GenAIExporter;
use super::instance_id;

/// 环境变量名称
pub const LOGTAIL_ENV_VAR: &str = "SLS_LOGTAIL_FILE";

/// 检查 Logtail 导出是否启用（环境变量 SLS_LOGTAIL_FILE 是否设置）
pub fn logtail_enabled() -> bool {
    std::env::var(LOGTAIL_ENV_VAR).is_ok()
}

/// 获取 Logtail 输出路径（从环境变量读取）
pub fn logtail_path() -> Option<String> {
    std::env::var(LOGTAIL_ENV_VAR).ok()
}

/// iLogtail 文件导出器
///
/// 将 GenAI 事件以扁平化 JSON 格式（每行一条记录）写入指定路径，
/// 由 iLogtail 自动采集上传到 SLS。字段命名与 SLS PutLogs 完全一致。
pub struct LogtailExporter {
    path: PathBuf,
}

impl LogtailExporter {
    /// 创建新的 Logtail 导出器
    ///
    /// 从环境变量 `SLS_LOGTAIL_FILE` 读取路径，自动创建父目录。
    /// 如果环境变量未设置，返回 `None`。
    pub fn new() -> Option<Self> {
        let path_str = logtail_path()?;
        let path = PathBuf::from(path_str);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        Some(LogtailExporter { path })
    }

    /// 返回导出文件路径
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// 将扁平化记录批量写入文件（append 模式）
    fn write_batch(&self, events: &[GenAISemanticEvent]) {
        let records = events_to_flat_records(events);
        if records.is_empty() {
            return;
        }

        let file = match OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            Ok(f) => f,
            Err(e) => {
                log::warn!("Failed to open logtail file {:?}: {}", self.path, e);
                return;
            }
        };

        let mut writer = BufWriter::new(file);
        for record in &records {
            match serde_json::to_string(record) {
                Ok(json_line) => {
                    if let Err(e) = writeln!(writer, "{}", json_line) {
                        log::warn!("Failed to write logtail record: {}", e);
                        return;
                    }
                }
                Err(e) => {
                    log::warn!("Failed to serialize logtail record: {}", e);
                }
            }
        }

        if let Err(e) = writer.flush() {
            log::warn!("Failed to flush logtail file: {}", e);
        }
    }
}

impl GenAIExporter for LogtailExporter {
    fn name(&self) -> &str {
        "logtail-file"
    }

    fn export(&self, events: &[GenAISemanticEvent]) {
        self.write_batch(events);
    }
}

/// 将 GenAI 语义事件转换为扁平化 key-value 记录
///
/// 返回 `Vec<BTreeMap<String, String>>`，每个 BTreeMap 代表一条日志记录。
/// 字段命名遵循 OTel GenAI 标准和 AgentSight 扩展规范。
/// 包含 iLogtail 保留字段：`__time__`、`__source__`、`__topic__`。
///
/// 此函数被 Logtail 文件导出器使用，由 iLogtail 采集后上传到 SLS。
pub fn events_to_flat_records(events: &[GenAISemanticEvent]) -> Vec<BTreeMap<String, String>> {
    let hostname = instance_id::get_instance_id();
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
