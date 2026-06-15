//! iLogtail 文件导出器
//!
//! 将 GenAI 语义事件以扁平化 JSON 格式写入指定路径，
//! 由 iLogtail 自动采集上传到 SLS，无需配置 AK/SK。
//!
//! 通过环境变量 `SLS_LOGTAIL_FILE` 指定输出路径。
//! 仅当该环境变量设置时才启用。

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use super::encrypt::MessageEncryptor;
use super::exporter::GenAIExporter;
use super::instance_id;
use super::semantic::GenAISemanticEvent;
use crate::interruption::types::InterruptionEvent;

/// 环境变量名称
pub const LOGTAIL_ENV_VAR: &str = "SLS_LOGTAIL_FILE";

/// 动态 Logtail 路径（由 config watcher 运行时设置）
static DYNAMIC_LOGTAIL_PATH: std::sync::RwLock<Option<String>> = std::sync::RwLock::new(None);

/// 设置动态 Logtail 输出路径（线程安全）。
///
/// 由 config watcher 在检测到 `runtime.sls_logtail_path` 变更时调用：
/// * 非空字符串 → 设置/更新动态路径，启用 SLS 上传；
/// * 空字符串    → 清空动态路径，已激活的 `LogtailExporter`（`dynamic=true`）
///   下次 `export()` 时检测到 `logtail_path() == None` 直接跳过，实现可逆暂停。
pub fn set_dynamic_logtail_path(path: &str) {
    if let Ok(mut guard) = DYNAMIC_LOGTAIL_PATH.write() {
        if path.is_empty() {
            if guard.is_some() {
                log::info!("Dynamic logtail path cleared (SLS uploads paused)");
            }
            *guard = None;
        } else {
            *guard = Some(path.to_string());
            log::info!("Dynamic logtail path set: {}", path);
        }
    }
}

/// 检查 Logtail 导出是否启用（环境变量 SLS_LOGTAIL_FILE 是否设置，或动态路径已配置）
pub fn logtail_enabled() -> bool {
    std::env::var(LOGTAIL_ENV_VAR).is_ok() || {
        DYNAMIC_LOGTAIL_PATH
            .read()
            .map(|g| g.is_some())
            .unwrap_or(false)
    }
}

/// 获取 Logtail 输出路径
///
/// 优先级：环境变量 `SLS_LOGTAIL_FILE` > 动态配置路径
pub fn logtail_path() -> Option<String> {
    // 环境变量优先
    if let Ok(p) = std::env::var(LOGTAIL_ENV_VAR) {
        return Some(p);
    }
    // 回退到动态配置路径
    DYNAMIC_LOGTAIL_PATH.read().ok().and_then(|g| g.clone())
}

/// iLogtail 文件导出器
///
/// 将 GenAI 事件以扁平化 JSON 格式（每行一条记录）写入指定路径，
/// 由 iLogtail 自动采集上传到 SLS。字段命名与 SLS PutLogs 完全一致。
/// 敏感消息字段使用 RSA+AES 混合加密保护。
pub struct LogtailExporter {
    path: PathBuf,
    encryptor: Option<MessageEncryptor>,
    /// 轨迹采集开关（对应 agentsight.json 的 `traceEnabled`）。
    /// 为 `false` 时，LLMCall 上传记录中的
    /// `gen_ai.system_instructions`、`gen_ai.input.messages`、
    /// `gen_ai.output.messages` 等对话内容字段被丢弃；
    /// token 数量、模型、提供商等元数据仍照常上传。
    trace_enabled: bool,
    /// 是否使用动态路径（来自 `runtime.sls_logtail_path` 配置）。
    /// 为 `true` 时每次 `export()` 调用 `logtail_path()` 取最新路径，
    /// 路径为空（被清空）则丢弃本批次，实现可逆的"暂停 / 恢复"语义；
    /// 为 `false` 时（环境变量启动模式）始终写入构造时锁定的 `path`。
    dynamic: bool,
}

impl LogtailExporter {
    /// 创建新的 Logtail 导出器
    ///
    /// 从环境变量 `SLS_LOGTAIL_FILE` 读取路径，自动创建父目录。
    /// 如果环境变量未设置，返回 `None`。
    ///
    /// `encryption_pem`：可选 RSA 公钥 PEM（通常来自 agentsight.json
    /// 的 `encryption.public_key`）。有值且解析成功则启用加密；
    /// 为 None 或解析失败则不加密。
    ///
    /// `trace_enabled`：轨迹采集开关。为 `false` 时不上传对话内容字段，
    /// 但保留 token 数量等元数据。
    pub fn new(encryption_pem: Option<&str>, trace_enabled: bool) -> Option<Self> {
        let path_str = logtail_path()?;
        let path = PathBuf::from(path_str);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let encryptor = encryption_pem.and_then(MessageEncryptor::from_pem);
        if encryptor.is_none() {
            log::info!("Logtail exporter: encryption disabled (no public key configured)");
        }
        if !trace_enabled {
            log::info!(
                "Logtail exporter: traceEnabled=false, conversation content fields (gen_ai.system_instructions, gen_ai.input.messages, gen_ai.output.messages) will NOT be uploaded"
            );
        }
        Some(LogtailExporter {
            path,
            encryptor,
            trace_enabled,
            dynamic: false,
        })
    }

    /// 从显式路径创建 Logtail 导出器（用于运行时动态激活）
    ///
    /// 与 `new()` 不同，不依赖环境变量，直接使用传入的路径。
    pub fn new_with_path(
        path_str: &str,
        encryption_pem: Option<&str>,
        trace_enabled: bool,
    ) -> Self {
        let path = PathBuf::from(path_str);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let encryptor = encryption_pem.and_then(MessageEncryptor::from_pem);
        if encryptor.is_none() {
            log::info!(
                "Logtail exporter (dynamic): encryption disabled (no public key configured)"
            );
        }
        if !trace_enabled {
            log::info!(
                "Logtail exporter (dynamic): traceEnabled=false, conversation content fields (gen_ai.system_instructions, gen_ai.input.messages, gen_ai.output.messages) will NOT be uploaded"
            );
        }
        LogtailExporter {
            path,
            encryptor,
            trace_enabled,
            dynamic: true,
        }
    }

    /// 返回导出文件路径
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// 将扁平化记录批量写入文件（append 模式）
    ///
    /// `dynamic=true` 时每次重新调用 `logtail_path()` 取最新路径；
    /// 若动态路径已被 `set_dynamic_logtail_path("")` 清空（暂停语义），
    /// 直接丢弃本批次，不报错。
    fn write_batch(&self, events: &[GenAISemanticEvent]) {
        let target_path: PathBuf = if self.dynamic {
            match logtail_path() {
                Some(p) if !p.is_empty() => {
                    let p = PathBuf::from(p);
                    if let Some(parent) = p.parent() {
                        std::fs::create_dir_all(parent).ok();
                    }
                    p
                }
                _ => return, // 动态路径已清空 → 暂停状态，丢弃本批次
            }
        } else {
            self.path.clone()
        };

        let records = events_to_flat_records(events, self.encryptor.as_ref(), self.trace_enabled);
        if records.is_empty() {
            return;
        }

        let file = match OpenOptions::new()
            .create(true)
            .append(true)
            .open(&target_path)
        {
            Ok(f) => f,
            Err(e) => {
                log::warn!("Failed to open logtail file {:?}: {}", target_path, e);
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
/// 敏感消息字段（system_instructions/input.messages/output.messages）使用混合加密保护。
///
/// `trace_enabled=false` 时跳过 LLMCall 中的对话内容字段
/// (`gen_ai.system_instructions`、`gen_ai.input.messages`、
/// `gen_ai.output.messages`)，token 数量等元数据仍上传。
pub fn events_to_flat_records(
    events: &[GenAISemanticEvent],
    encryptor: Option<&MessageEncryptor>,
    trace_enabled: bool,
) -> Vec<BTreeMap<String, String>> {
    let hostname = instance_id::get_instance_id();
    let uid = instance_id::get_owner_account_id();
    let mut records = Vec::with_capacity(events.len());

    for event in events {
        let mut m = BTreeMap::new();
        let timestamp = chrono::Utc::now().timestamp();

        // iLogtail 保留字段
        m.insert("__time__".to_string(), timestamp.to_string());
        m.insert("__source__".to_string(), hostname.to_string());
        m.insert("__topic__".to_string(), "agentsight".to_string());

        // 每条日志都写入 instance
        m.insert("instance".to_string(), hostname.to_string());

        // 写入 uid (owner-account-id)
        if !uid.is_empty() {
            m.insert("uid".to_string(), uid.to_string());
        }

        match event {
            GenAISemanticEvent::LLMCall(call) => {
                // ── OTel GenAI Required ──
                m.insert("gen_ai.provider.name".to_string(), call.provider.clone());
                m.insert("gen_ai.request.model".to_string(), call.model.clone());
                m.insert(
                    "gen_ai.operation.name".to_string(),
                    call.metadata
                        .get("operation_name")
                        .cloned()
                        .unwrap_or_else(|| "chat".to_string()),
                );

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
                if let Some(reason) = call
                    .response
                    .messages
                    .first()
                    .and_then(|msg| msg.finish_reason.as_ref())
                {
                    m.insert(
                        "gen_ai.response.finish_reasons".to_string(),
                        format!("[\"{}\"]", reason),
                    );
                }
                if let Some(temp) = call.request.temperature {
                    m.insert("gen_ai.request.temperature".to_string(), temp.to_string());
                }
                if let Some(max) = call.request.max_tokens {
                    m.insert("gen_ai.request.max_tokens".to_string(), max.to_string());
                }
                if let Some(fp) = call.request.frequency_penalty {
                    m.insert(
                        "gen_ai.request.frequency_penalty".to_string(),
                        fp.to_string(),
                    );
                }
                if let Some(pp) = call.request.presence_penalty {
                    m.insert(
                        "gen_ai.request.presence_penalty".to_string(),
                        pp.to_string(),
                    );
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
                    m.insert(
                        "gen_ai.usage.input_tokens".to_string(),
                        usage.input_tokens.to_string(),
                    );
                    m.insert(
                        "gen_ai.usage.output_tokens".to_string(),
                        usage.output_tokens.to_string(),
                    );
                    if let Some(cache_create) = usage.cache_creation_input_tokens {
                        m.insert(
                            "gen_ai.usage.cache_creation.input_tokens".to_string(),
                            cache_create.to_string(),
                        );
                    }
                    if let Some(cache_read) = usage.cache_read_input_tokens {
                        m.insert(
                            "gen_ai.usage.cache_read.input_tokens".to_string(),
                            cache_read.to_string(),
                        );
                    }
                }
                if let Some(addr) = call.metadata.get("server.address") {
                    m.insert("server.address".to_string(), addr.clone());
                }
                m.insert("gen_ai.output.type".to_string(), "text".to_string());

                // ── gen_ai.system_instructions (system role messages) ──
                // 受 trace_enabled 控制：system prompt 通常包含产品业务逻辑、
                // 工具说明等敏感配置，traceEnabled=false 时同样不上传。
                if trace_enabled {
                    let system_msgs: Vec<&super::semantic::InputMessage> = call
                        .request
                        .messages
                        .iter()
                        .filter(|msg| msg.role == "system")
                        .collect();
                    if !system_msgs.is_empty() {
                        if let Ok(json) = serde_json::to_string(&system_msgs) {
                            m.insert(
                                "gen_ai.system_instructions".to_string(),
                                MessageEncryptor::maybe_encrypt(encryptor, &json),
                            );
                        }
                    }
                }

                // ── gen_ai.input.messages (增量：只取最新一轮) ──
                // 仅在 trace_enabled=true 时上传对话内容。轨迹开关关闭时
                // 仅保留 token 数量等元数据，不上传用户输入。
                // 从后往前找最后一条 user message，取它及之后的所有非 system 消息
                if trace_enabled {
                    let latest_msgs =
                        super::semantic::latest_round_input_messages(&call.request.messages);
                    if !latest_msgs.is_empty() {
                        if let Ok(json) = serde_json::to_string(&latest_msgs) {
                            m.insert(
                                "gen_ai.input.messages".to_string(),
                                MessageEncryptor::maybe_encrypt(encryptor, &json),
                            );
                        }
                    }
                }

                // ── gen_ai.output.messages (parts-based with finish_reason) ──
                // 同样受 trace_enabled 控制，不上传模型响应内容。
                if trace_enabled && !call.response.messages.is_empty() {
                    if let Ok(json) = serde_json::to_string(&call.response.messages) {
                        m.insert(
                            "gen_ai.output.messages".to_string(),
                            MessageEncryptor::maybe_encrypt(encryptor, &json),
                        );
                    }
                }

                // ── 加密标记字段 ──
                if encryptor.is_some() {
                    m.insert("agentsight.encrypted".to_string(), "true".to_string());
                }

                // ── AgentSight extensions ──
                m.insert("agentsight.pid".to_string(), call.pid.to_string());
                m.insert(
                    "agentsight.process_name".to_string(),
                    call.process_name.clone(),
                );
                if let Some(ref name) = call.agent_name {
                    m.insert("agentsight.agent.name".to_string(), name.clone());
                }
                m.insert(
                    "agentsight.duration_ns".to_string(),
                    call.duration_ns.to_string(),
                );
                m.insert(
                    "agentsight.start_timestamp_ns".to_string(),
                    call.start_timestamp_ns.to_string(),
                );
                m.insert(
                    "agentsight.end_timestamp_ns".to_string(),
                    call.end_timestamp_ns.to_string(),
                );
                if let Some(method) = call.metadata.get("method") {
                    m.insert("agentsight.http.method".to_string(), method.clone());
                }
                if let Some(path) = call.metadata.get("path") {
                    m.insert("agentsight.http.path".to_string(), path.clone());
                }
                if let Some(status) = call.metadata.get("status_code") {
                    m.insert("agentsight.http.status_code".to_string(), status.clone());
                }
                if call.request.stream
                    || call
                        .metadata
                        .get("is_sse")
                        .map(|v| v == "true")
                        .unwrap_or(false)
                {
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
                m.insert(
                    "agentsight.tool.success".to_string(),
                    tool.success.to_string(),
                );
                m.insert("agentsight.pid".to_string(), tool.pid.to_string());
                if let Some(ref dur) = tool.duration_ns {
                    m.insert("agentsight.duration_ns".to_string(), dur.to_string());
                }
                if let Some(ref error) = tool.error {
                    m.insert("error.type".to_string(), error.clone());
                }
            }
            GenAISemanticEvent::AgentInteraction(interaction) => {
                m.insert(
                    "gen_ai.operation.name".to_string(),
                    "agent_interaction".to_string(),
                );
                m.insert(
                    "agentsight.agent.name".to_string(),
                    interaction.agent_name.clone(),
                );
                m.insert(
                    "agentsight.agent.interaction_type".to_string(),
                    interaction.interaction_type.clone(),
                );
                m.insert("agentsight.pid".to_string(), interaction.pid.to_string());
            }
            GenAISemanticEvent::StreamChunk(chunk) => {
                m.insert(
                    "gen_ai.operation.name".to_string(),
                    "stream_chunk".to_string(),
                );
                m.insert("agentsight.stream.id".to_string(), chunk.stream_id.clone());
                m.insert(
                    "agentsight.stream.chunk_index".to_string(),
                    chunk.chunk_index.to_string(),
                );
                m.insert("agentsight.pid".to_string(), chunk.pid.to_string());
            }
        }

        records.push(m);
    }

    records
}

/// 将中断事件转换为扁平化 key-value 记录
///
/// 通过 `gen_ai.operation.name = "interruption"` 与 LLMCall 记录区分。
/// 复用通用字段（instance/uid/pid/agent.name/session.id/conversation.id/trace_id/
/// start_timestamp_ns），并增加 `agentsight.interruption.*` 专属字段，便于在 SLS
/// 中以同一索引同时查询会话与中断事件。
pub fn interruption_events_to_flat_records(
    events: &[InterruptionEvent],
) -> Vec<BTreeMap<String, String>> {
    let hostname = instance_id::get_instance_id();
    let uid = instance_id::get_owner_account_id();
    let mut records = Vec::with_capacity(events.len());

    for event in events {
        let mut m = BTreeMap::new();
        let timestamp = chrono::Utc::now().timestamp();

        // iLogtail 保留字段
        m.insert("__time__".to_string(), timestamp.to_string());
        m.insert("__source__".to_string(), hostname.to_string());
        m.insert("__topic__".to_string(), "agentsight".to_string());

        m.insert("instance".to_string(), hostname.to_string());
        if !uid.is_empty() {
            m.insert("uid".to_string(), uid.to_string());
        }

        // 区分字段：与 LLMCall/ToolUse 等记录区分
        m.insert(
            "gen_ai.operation.name".to_string(),
            "interruption".to_string(),
        );

        // ── 复用 LLMCall 记录的关联字段 ──
        if let Some(pid) = event.pid {
            m.insert("agentsight.pid".to_string(), pid.to_string());
        }
        if let Some(ref name) = event.agent_name {
            m.insert("agentsight.agent.name".to_string(), name.clone());
        }
        if let Some(ref sid) = event.session_id {
            m.insert("gen_ai.session.id".to_string(), sid.clone());
        }
        if let Some(ref cid) = event.conversation_id {
            m.insert("gen_ai.conversation.id".to_string(), cid.clone());
        }
        if let Some(ref tid) = event.trace_id {
            m.insert("trace_id".to_string(), tid.clone());
        }
        m.insert(
            "agentsight.start_timestamp_ns".to_string(),
            event.occurred_at_ns.to_string(),
        );

        // ── 中断事件专属字段 ──
        m.insert(
            "agentsight.interruption.id".to_string(),
            event.interruption_id.clone(),
        );
        m.insert(
            "agentsight.interruption.type".to_string(),
            event.interruption_type.as_str().to_string(),
        );
        m.insert(
            "agentsight.interruption.severity".to_string(),
            event.severity.as_str().to_string(),
        );
        m.insert(
            "agentsight.interruption.resolved".to_string(),
            event.resolved.to_string(),
        );
        if let Some(ref detail) = event.detail {
            m.insert("agentsight.interruption.detail".to_string(), detail.clone());
        }
        if let Some(ref cid) = event.call_id {
            m.insert("agentsight.interruption.call_id".to_string(), cid.clone());
        }

        records.push(m);
    }

    records
}

/// 将中断事件批量导出到 iLogtail 文件（追加写入）
///
/// 仅在环境变量 `SLS_LOGTAIL_FILE` 设置时生效；否则为空操作。
/// 与 `LogtailExporter::write_batch` 写入同一文件，由 iLogtail 统一采集到 SLS。
pub fn export_interruption_events(events: &[InterruptionEvent]) {
    if events.is_empty() {
        return;
    }
    let path_str = match logtail_path() {
        Some(p) => p,
        None => return,
    };
    let path = PathBuf::from(path_str);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    let records = interruption_events_to_flat_records(events);
    if records.is_empty() {
        return;
    }

    let file = match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(f) => f,
        Err(e) => {
            log::warn!(
                "Failed to open logtail file {:?} for interruption export: {}",
                path,
                e
            );
            return;
        }
    };

    let mut writer = BufWriter::new(file);
    for record in &records {
        match serde_json::to_string(record) {
            Ok(json_line) => {
                if let Err(e) = writeln!(writer, "{}", json_line) {
                    log::warn!("Failed to write interruption logtail record: {}", e);
                    return;
                }
            }
            Err(e) => {
                log::warn!("Failed to serialize interruption logtail record: {}", e);
            }
        }
    }

    if let Err(e) = writer.flush() {
        log::warn!("Failed to flush logtail file (interruption): {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::genai::semantic::{
        InputMessage, LLMCall, LLMRequest, LLMResponse, MessagePart, OutputMessage, TokenUsage,
    };
    use std::collections::HashMap;

    /// 构造一个包含 user/assistant 对话与 token usage 的 LLMCall。
    fn make_full_llm_call() -> LLMCall {
        let request = LLMRequest {
            messages: vec![
                InputMessage {
                    role: "system".to_string(),
                    parts: vec![MessagePart::Text {
                        content: "you are helpful".to_string(),
                    }],
                    name: None,
                },
                InputMessage {
                    role: "user".to_string(),
                    parts: vec![MessagePart::Text {
                        content: "hello secret".to_string(),
                    }],
                    name: None,
                },
            ],
            temperature: Some(0.7),
            max_tokens: Some(1024),
            frequency_penalty: None,
            presence_penalty: None,
            top_p: None,
            top_k: None,
            seed: None,
            stop_sequences: None,
            stream: false,
            tools: None,
            raw_body: None,
        };
        let mut call = LLMCall::new(
            "call-trace-test".to_string(),
            1_000,
            "openai".to_string(),
            "gpt-4".to_string(),
            request,
            42,
            "test-proc".to_string(),
        );
        call.set_response(
            LLMResponse {
                messages: vec![OutputMessage {
                    role: "assistant".to_string(),
                    parts: vec![MessagePart::Text {
                        content: "sensitive reply".to_string(),
                    }],
                    name: None,
                    finish_reason: Some("stop".to_string()),
                }],
                streamed: false,
                raw_body: None,
            },
            5_000,
        );
        call.set_token_usage(TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
            total_tokens: 150,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        });
        call.metadata = HashMap::new();
        call
    }

    #[test]
    fn test_trace_enabled_true_includes_messages() {
        // 默认轨迹开启：system_instructions、input.messages、output.messages 均上传
        let event = GenAISemanticEvent::LLMCall(make_full_llm_call());
        let records = events_to_flat_records(&[event], None, true);
        assert_eq!(records.len(), 1);
        let r = &records[0];
        assert!(
            r.contains_key("gen_ai.system_instructions"),
            "system_instructions should be uploaded when traceEnabled=true"
        );
        assert!(
            r.contains_key("gen_ai.input.messages"),
            "input.messages should be uploaded when traceEnabled=true"
        );
        assert!(
            r.contains_key("gen_ai.output.messages"),
            "output.messages should be uploaded when traceEnabled=true"
        );
        // token 数量元数据也应存在
        assert_eq!(
            r.get("gen_ai.usage.input_tokens").map(String::as_str),
            Some("100")
        );
        assert_eq!(
            r.get("gen_ai.usage.output_tokens").map(String::as_str),
            Some("50")
        );
    }

    #[test]
    fn test_trace_enabled_false_drops_messages_keeps_token_metadata() {
        // 轨迹关闭：system_instructions、input.messages、output.messages 均不上传，
        // token 数量等元数据仍保留
        let event = GenAISemanticEvent::LLMCall(make_full_llm_call());
        let records = events_to_flat_records(&[event], None, false);
        assert_eq!(records.len(), 1);
        let r = &records[0];
        assert!(
            !r.contains_key("gen_ai.system_instructions"),
            "system_instructions must NOT be uploaded when traceEnabled=false"
        );
        assert!(
            !r.contains_key("gen_ai.input.messages"),
            "input.messages must NOT be uploaded when traceEnabled=false"
        );
        assert!(
            !r.contains_key("gen_ai.output.messages"),
            "output.messages must NOT be uploaded when traceEnabled=false"
        );

        // token 消耗与模型元数据仍需上传
        assert_eq!(
            r.get("gen_ai.usage.input_tokens").map(String::as_str),
            Some("100")
        );
        assert_eq!(
            r.get("gen_ai.usage.output_tokens").map(String::as_str),
            Some("50")
        );
        assert_eq!(
            r.get("gen_ai.provider.name").map(String::as_str),
            Some("openai")
        );
        assert_eq!(
            r.get("gen_ai.request.model").map(String::as_str),
            Some("gpt-4")
        );
        assert_eq!(r.get("agentsight.pid").map(String::as_str), Some("42"));
        assert_eq!(
            r.get("agentsight.duration_ns").map(String::as_str),
            Some("4000")
        );
        // 不允许任何对话内容字段泄漏：包括 .messages 结尾的字段以及 system_instructions
        for key in r.keys() {
            assert!(
                !key.ends_with(".messages") && key != "gen_ai.system_instructions",
                "unexpected conversation-content field leaked when traceEnabled=false: {}",
                key,
            );
        }
    }

    #[test]
    fn test_trace_enabled_false_does_not_affect_non_llmcall_events() {
        // 轨迹关闭对 ToolUse / AgentInteraction / StreamChunk 本身不增加过滤逻辑
        // （这些事件本来就不包含 input/output messages）
        use crate::genai::semantic::ToolUse;
        let tool = ToolUse {
            tool_use_id: "tu-1".to_string(),
            timestamp_ns: 0,
            tool_name: "shell".to_string(),
            arguments: serde_json::Value::Null,
            result: None,
            duration_ns: Some(1000),
            success: true,
            error: None,
            parent_llm_call_id: Some("parent-1".to_string()),
            pid: 7,
        };
        let event = GenAISemanticEvent::ToolUse(tool);
        let records = events_to_flat_records(&[event], None, false);
        assert_eq!(records.len(), 1);
        let r = &records[0];
        assert_eq!(
            r.get("gen_ai.operation.name").map(String::as_str),
            Some("tool_use")
        );
        assert_eq!(r.get("gen_ai.tool.name").map(String::as_str), Some("shell"));
        assert_eq!(r.get("agentsight.pid").map(String::as_str), Some("7"));
    }
}
