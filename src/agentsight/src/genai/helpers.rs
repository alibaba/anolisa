//! GenAI Builder helper functions
//!
//! Pure helpers for LLM request classification, provider/model extraction,
//! user-query parsing and agent-name resolution. Logic preserved verbatim
//! from the original `builder.rs`; only visibility was widened to
//! `pub(super)` so siblings (`builder` / `call_builder`) can call these.

use super::GenAIBuilder;
use super::semantic::{LLMRequest, MessagePart};
use crate::analyzer::ParsedApiMessage;
use crate::config::default_cmdline_rules;
use crate::discovery::matcher::{CmdlineGlobMatcher, ProcessContext};

/// LLM call classification: Main (normal), Recap (compaction/summary), WebSearch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CallKind {
    Main,
    Recap,
    WebSearch,
}

impl CallKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            CallKind::Main => "main",
            CallKind::Recap => "recap",
            CallKind::WebSearch => "web_search",
        }
    }
}

/// Classify an LLM call based on request content (dual-path: system + first-user).
///
/// Conservative: unmatched → Main (zero false positives > recall).
/// Signatures are from real captures (case-sensitive .contains()).
pub(super) fn classify_call_kind(request: &LLMRequest) -> CallKind {
    // Collect system instructions text
    let system_text: String = request
        .messages
        .iter()
        .filter(|m| m.role == "system")
        .filter_map(|m| m.parts.first())
        .filter_map(|p| match p {
            MessagePart::Text { content } => Some(content.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");

    // ① Check system_instructions (Cosh patterns)
    if !system_text.is_empty() {
        // Cosh recap/compaction: both markers present
        if system_text.contains("summarizes internal chat history")
            && system_text.contains("<state_snapshot>")
        {
            return CallKind::Recap;
        }
        // Cosh recap/project-summary
        if system_text.contains("specialized context summarizer") {
            return CallKind::Recap;
        }
    }

    // ② Check first-user text (Claude Code patterns + Cosh tool-output)
    let first_user_text: Option<&str> = request
        .messages
        .iter()
        .filter(|m| m.role == "user")
        .filter_map(|m| m.parts.first())
        .filter_map(|p| match p {
            MessagePart::Text { content } => Some(content.as_str()),
            _ => None,
        })
        .next();

    if let Some(text) = first_user_text {
        // Cosh tool-output + Claude Code recap (both → Recap)
        if text.starts_with("Summarize the following tool output to be a maximum of")
            || (text
                .contains("Your task is to create a detailed summary of the conversation so far")
                && text.contains("Do NOT call any tools"))
        {
            return CallKind::Recap;
        }
        // Claude Code web_search
        if text.contains("Perform a web search for the query:") {
            return CallKind::WebSearch;
        }
    }

    // ③ Default: Main (conservative, zero false positives)
    CallKind::Main
}

/// Classify an LLM call from raw JSON strings (pending-write path).
///
/// `system_instructions` is the serialized JSON array of system messages.
/// `first_user_text` is the extracted first user message text.
///
/// This mirrors [`classify_call_kind`] but operates on raw strings from the
/// HTTP body rather than the structured `LLMRequest`, used in
/// `build_pending_from_request` where full semantic parsing has not occurred.
pub(super) fn classify_call_kind_from_raw(
    system_instructions: &Option<String>,
    first_user_text: &str,
) -> &'static str {
    let sys_text = system_instructions
        .as_deref()
        .and_then(|s| serde_json::from_str::<Vec<serde_json::Value>>(s).ok())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let c = m.get("content")?;
                    if let Some(s) = c.as_str() {
                        return Some(s.to_string());
                    }
                    if let Some(arr) = c.as_array() {
                        let text: String = arr
                            .iter()
                            .filter_map(|item| {
                                if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                                    item.get("text").and_then(|t| t.as_str())
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        if !text.is_empty() {
                            return Some(text);
                        }
                    }
                    None
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();

    if (sys_text.contains("summarizes internal chat history")
        && sys_text.contains("<state_snapshot>"))
        || sys_text.contains("specialized context summarizer")
        || first_user_text.starts_with("Summarize the following tool output to be a maximum of")
        || (first_user_text
            .contains("Your task is to create a detailed summary of the conversation so far")
            && first_user_text.contains("Do NOT call any tools"))
    {
        "recap"
    } else if first_user_text.contains("Perform a web search for the query:") {
        "web_search"
    } else {
        "main"
    }
}

impl GenAIBuilder {
    /// Check if the path indicates an LLM API call
    pub(super) fn is_llm_api_path(&self, path: &str) -> bool {
        path.contains("/v1/chat/completions")
            || path.contains("/v1/completions")
            || path.contains("/v1/messages")
            || path.contains("/v1/responses")
            || path.contains("/chat/completions")
            || path.contains("/completions")
            || path.contains("/api/v1/copilot/generate_copilot")
    }

    /// Check if request body contains SysOM POP API markers
    /// SysOM uses path "/" with action in body (llmParamString field)
    pub(super) fn is_sysom_pop_request(request_body: &Option<String>) -> bool {
        request_body
            .as_ref()
            .map(|b| b.contains("llmParamString"))
            .unwrap_or(false)
    }

    /// Extract provider from path
    pub(super) fn extract_provider_from_path(&self, path: &str) -> Option<String> {
        if path.contains("anthropic") || path.contains("/v1/messages") {
            Some("anthropic".to_string())
        } else if path.contains("/v1/chat/completions")
            || path.contains("/v1/completions")
            || path.contains("/v1/responses")
        {
            Some("openai".to_string())
        } else if path.contains("/api/v1/copilot/generate_copilot") {
            Some("sysom".to_string())
        } else {
            None
        }
    }

    /// Extract provider from request body (for POP API style requests)
    pub(super) fn extract_provider_from_body(request_body: &Option<String>) -> Option<String> {
        if Self::is_sysom_pop_request(request_body) {
            Some("sysom".to_string())
        } else {
            None
        }
    }

    /// Extract model from parsed message
    pub(super) fn extract_model_from_message(
        &self,
        message: &Option<ParsedApiMessage>,
    ) -> Option<String> {
        match message {
            Some(ParsedApiMessage::OpenAICompletion { request, .. }) => {
                request.as_ref().map(|r| r.model.clone())
            }
            Some(ParsedApiMessage::AnthropicMessage { request, .. }) => {
                request.as_ref().map(|r| r.model.clone())
            }
            Some(ParsedApiMessage::SysomMessage { request, .. }) => {
                request.as_ref().map(|r| r.params.model.clone())
            }
            _ => None,
        }
    }

    /// 从 HTTP request/response body 中直接提取 model 字段
    ///
    /// 优先从 request body 取（用户请求的 model），
    /// 如果没有则从 response body 取（SSE 响应中的 model）
    /// 对于 SysOM 请求，需要从 llmParamString 内嵌 JSON 中提取 model
    pub(super) fn extract_model_from_body(
        request_body: &Option<String>,
        response_body: &Option<String>,
    ) -> Option<String> {
        // 尝试从 request body 获取
        if let Some(body) = request_body {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
                // 标准 OpenAI/Anthropic 格式
                if let Some(model) = v.get("model").and_then(|m| m.as_str()) {
                    if !model.is_empty() {
                        return Some(model.to_string());
                    }
                }
                // SysOM 格式：model 嵌套在 llmParamString 中
                if let Some(lps) = v.get("llmParamString").and_then(|v| v.as_str()) {
                    if let Ok(inner) = serde_json::from_str::<serde_json::Value>(lps) {
                        if let Some(model) = inner.get("model").and_then(|m| m.as_str()) {
                            if !model.is_empty() {
                                return Some(model.to_string());
                            }
                        }
                    }
                }
            }
        }
        // 尝试从 response body 获取（SSE 响应是 JSON 数组，取第一个 chunk）
        if let Some(body) = response_body {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
                // 非 SSE: 直接是 JSON 对象
                if let Some(model) = v.get("model").and_then(|m| m.as_str()) {
                    if !model.is_empty() {
                        return Some(model.to_string());
                    }
                }
                // SSE: JSON 数组，取第一个 chunk 的 model
                if let Some(arr) = v.as_array() {
                    for chunk in arr {
                        if let Some(model) = chunk.get("model").and_then(|m| m.as_str()) {
                            if !model.is_empty() {
                                return Some(model.to_string());
                            }
                        }
                    }
                }
            }
        }
        None
    }

    /// 提取第一条有实际文本内容的 user message 的原始文本
    ///
    /// 仅返回含非空 `Text` 片段的首条 user message，供 `IdResolver`
    /// 生成 session_key 使用。跳过只含 tool_result 等的 user message。
    pub(super) fn extract_first_user_raw(request: &LLMRequest) -> Option<String> {
        request
            .messages
            .iter()
            .filter(|m| m.role == "user")
            .find_map(|m| {
                let text: String = m
                    .parts
                    .iter()
                    .filter_map(|p| match p {
                        MessagePart::Text { content } if !content.is_empty() => {
                            Some(content.as_str())
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if text.is_empty() { None } else { Some(text) }
            })
    }

    /// 提取最后一条有实际文本内容的 user message 的原始文本
    ///
    /// 跳过 Anthropic 格式中只包含 tool_result 的 user message
    pub(super) fn extract_last_user_raw(request: &LLMRequest) -> Option<String> {
        request
            .messages
            .iter()
            .rev()
            .filter(|m| m.role == "user")
            .find_map(|m| {
                let text: String = m
                    .parts
                    .iter()
                    .filter_map(|p| match p {
                        MessagePart::Text { content } if !content.is_empty() => {
                            Some(content.as_str())
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if text.is_empty() { None } else { Some(text) }
            })
    }

    /// 提取清理后的 user query（去除 metadata 前缀，用于展示）
    pub(super) fn extract_last_user_query(request: &LLMRequest) -> Option<String> {
        Self::extract_last_user_raw(request).map(|raw| Self::strip_user_query_prefix(&raw))
    }

    /// 去除 user message 中的 metadata 前缀，只保留用户实际输入的文本
    ///
    /// OpenClaw 等 Agent 会在 user message 前面加上元数据，格式如：
    /// ```text
    /// Sender (untrusted metadata):
    /// ```json
    /// {"label":"...", ...}
    /// ```
    ///
    /// [Tue 2026-03-31 17:19 GMT+8] 用户实际输入
    /// ```
    pub(super) fn strip_user_query_prefix(text: &str) -> String {
        // 查找最后一个 [timestamp] 模式，取其后的内容
        // 格式: [Day YYYY-MM-DD HH:MM TZ] 或 [Day, DD Mon YYYY HH:MM:SS TZ]
        if let Some(pos) = text.rfind(']') {
            // 确认 ] 前面有对应的 [
            if let Some(bracket_start) = text[..pos].rfind('[') {
                let bracket_content = &text[bracket_start + 1..pos];
                // 简单验证：方括号内包含数字（日期）和冒号（时间）
                if bracket_content.contains(':')
                    && bracket_content.chars().any(|c| c.is_ascii_digit())
                {
                    let after = text[pos + 1..].trim_start();
                    if !after.is_empty() {
                        return after.to_string();
                    }
                }
            }
        }
        text.to_string()
    }

    /// Resolve agent name from comm string only (no /proc access).
    /// Used for dead-PID drain where the process is already gone.
    pub(super) fn resolve_agent_name_from_comm(
        comm: &str,
        pid: u32,
        cache: &std::collections::HashMap<u32, String>,
    ) -> Option<String> {
        // First check the pid→agent_name cache (works even for dead processes)
        if let Some(name) = cache.get(&pid) {
            return Some(name.clone());
        }
        let ctx = ProcessContext {
            comm: comm.to_string(),
            cmdline_args: vec![],
            exe_path: String::new(),
        };
        default_cmdline_rules()
            .iter()
            .filter_map(CmdlineGlobMatcher::from_config)
            .find(|m| m.matches(&ctx))
            .map(|m| m.info().name.clone())
    }

    /// 通过进程名匹配 agent registry，返回已知 agent 名称
    pub(super) fn resolve_agent_name(
        comm: &str,
        pid: u32,
        cache: &std::collections::HashMap<u32, String>,
    ) -> Option<String> {
        // First check the pid→agent_name cache (works even for dead processes)
        if let Some(name) = cache.get(&pid) {
            return Some(name.clone());
        }
        // Read cmdline from /proc/{pid}/cmdline for accurate agent matching
        let cmdline_args = std::fs::read(format!("/proc/{pid}/cmdline"))
            .ok()
            .map(|data| {
                data.split(|&b| b == 0)
                    .filter(|s| !s.is_empty())
                    .map(|s| String::from_utf8_lossy(s).to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let exe_path = std::fs::read_link(format!("/proc/{pid}/exe"))
            .ok()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        let ctx = ProcessContext {
            comm: comm.to_string(),
            cmdline_args,
            exe_path,
        };
        default_cmdline_rules()
            .iter()
            .filter_map(CmdlineGlobMatcher::from_config)
            .find(|m| m.matches(&ctx))
            .map(|m| m.info().name.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::super::semantic::InputMessage;
    use super::*;
    use std::collections::HashMap;

    fn make_llm_request(messages: Vec<InputMessage>) -> LLMRequest {
        LLMRequest {
            messages,
            temperature: None,
            max_tokens: None,
            frequency_penalty: None,
            presence_penalty: None,
            top_p: None,
            top_k: None,
            seed: None,
            stop_sequences: None,
            stream: false,
            tools: None,
            raw_body: None,
        }
    }

    #[test]
    fn test_classify_cosh_compaction_recap() {
        let req = make_llm_request(vec![InputMessage {
            role: "system".to_string(),
            parts: vec![MessagePart::Text {
                content: "This summarizes internal chat history with <state_snapshot> data"
                    .to_string(),
            }],
            name: None,
        }]);
        assert_eq!(classify_call_kind(&req), CallKind::Recap);
    }

    #[test]
    fn test_classify_cosh_project_summary_recap() {
        let req = make_llm_request(vec![InputMessage {
            role: "system".to_string(),
            parts: vec![MessagePart::Text {
                content: "You are a specialized context summarizer for project files".to_string(),
            }],
            name: None,
        }]);
        assert_eq!(classify_call_kind(&req), CallKind::Recap);
    }

    #[test]
    fn test_classify_cosh_tool_output_recap() {
        let req = make_llm_request(vec![InputMessage {
            role: "user".to_string(),
            parts: vec![MessagePart::Text {
                content: "Summarize the following tool output to be a maximum of 500 tokens: ..."
                    .to_string(),
            }],
            name: None,
        }]);
        assert_eq!(classify_call_kind(&req), CallKind::Recap);
    }

    #[test]
    fn test_classify_claude_web_search() {
        let req = make_llm_request(vec![InputMessage {
            role: "user".to_string(),
            parts: vec![MessagePart::Text {
                content: "Perform a web search for the query: rust async programming".to_string(),
            }],
            name: None,
        }]);
        assert_eq!(classify_call_kind(&req), CallKind::WebSearch);
    }

    #[test]
    fn test_classify_claude_recap() {
        let req = make_llm_request(vec![InputMessage {
            role: "user".to_string(),
            parts: vec![MessagePart::Text {
                content: "Your task is to create a detailed summary of the conversation so far. Do NOT call any tools during this.".to_string(),
            }],
            name: None,
        }]);
        assert_eq!(classify_call_kind(&req), CallKind::Recap);
    }

    #[test]
    fn test_classify_normal_call_not_misclassified() {
        let req = make_llm_request(vec![
            InputMessage {
                role: "system".to_string(),
                parts: vec![MessagePart::Text {
                    content: "You are Copilot Shell, a helpful assistant.".to_string(),
                }],
                name: None,
            },
            InputMessage {
                role: "user".to_string(),
                parts: vec![MessagePart::Text {
                    content: "explain this code".to_string(),
                }],
                name: None,
            },
        ]);
        assert_eq!(classify_call_kind(&req), CallKind::Main);
    }

    #[test]
    fn test_classify_empty_request_is_main() {
        let req = make_llm_request(vec![]);
        assert_eq!(classify_call_kind(&req), CallKind::Main);
    }

    #[test]
    fn test_is_llm_api_path() {
        let builder = GenAIBuilder::new();
        assert!(builder.is_llm_api_path("/v1/chat/completions"));
        assert!(builder.is_llm_api_path("/v1/completions"));
        assert!(builder.is_llm_api_path("/v1/messages"));
        assert!(builder.is_llm_api_path("/api/v1/copilot/generate_copilot"));
        assert!(builder.is_llm_api_path("/proxy/v1/chat/completions"));
        assert!(!builder.is_llm_api_path("/api/health"));
        assert!(!builder.is_llm_api_path("/v1/models"));
    }

    #[test]
    fn test_is_sysom_pop_request() {
        assert!(GenAIBuilder::is_sysom_pop_request(&Some(
            r#"{"llmParamString":"{}"}"#.to_string()
        )));
        assert!(!GenAIBuilder::is_sysom_pop_request(&Some("{}".to_string())));
        assert!(!GenAIBuilder::is_sysom_pop_request(&None));
    }

    #[test]
    fn test_extract_provider_from_path() {
        let builder = GenAIBuilder::new();
        assert_eq!(
            builder.extract_provider_from_path("/v1/chat/completions"),
            Some("openai".to_string())
        );
        assert_eq!(
            builder.extract_provider_from_path("/v1/messages"),
            Some("anthropic".to_string())
        );
        assert_eq!(
            builder.extract_provider_from_path("/api/v1/copilot/generate_copilot"),
            Some("sysom".to_string())
        );
        assert_eq!(builder.extract_provider_from_path("/unknown"), None);
    }

    #[test]
    fn test_extract_provider_from_body() {
        assert_eq!(
            GenAIBuilder::extract_provider_from_body(&Some(
                r#"{"llmParamString":"{}"} "#.to_string()
            )),
            Some("sysom".to_string())
        );
        assert_eq!(
            GenAIBuilder::extract_provider_from_body(&Some("{}".to_string())),
            None
        );
    }

    #[test]
    fn test_extract_model_from_body_request() {
        let body = Some(r#"{"model": "gpt-4", "messages": []}"#.to_string());
        assert_eq!(
            GenAIBuilder::extract_model_from_body(&body, &None),
            Some("gpt-4".to_string())
        );
    }

    #[test]
    fn test_extract_model_from_body_sysom() {
        let body = Some(r#"{"llmParamString": "{\"model\":\"qwen-max\"}"} "#.to_string());
        assert_eq!(
            GenAIBuilder::extract_model_from_body(&body, &None),
            Some("qwen-max".to_string())
        );
    }

    #[test]
    fn test_extract_model_from_body_response() {
        let resp = Some(r#"{"model": "claude-3"}"#.to_string());
        assert_eq!(
            GenAIBuilder::extract_model_from_body(&None, &resp),
            Some("claude-3".to_string())
        );
    }

    #[test]
    fn test_extract_model_from_body_sse_array() {
        let resp = Some(r#"[{"model": "gpt-4o"}, {"model": "gpt-4o"}]"#.to_string());
        assert_eq!(
            GenAIBuilder::extract_model_from_body(&None, &resp),
            Some("gpt-4o".to_string())
        );
    }

    #[test]
    fn test_extract_model_from_body_none() {
        assert_eq!(GenAIBuilder::extract_model_from_body(&None, &None), None);
    }

    #[test]
    fn test_strip_user_query_prefix_with_timestamp() {
        let text = "Sender (untrusted metadata):\n```json\n{}\n```\n\n[Tue 2026-03-31 17:19 GMT+8] hello world";
        assert_eq!(GenAIBuilder::strip_user_query_prefix(text), "hello world");
    }

    #[test]
    fn test_strip_user_query_prefix_no_timestamp() {
        let text = "plain user input";
        assert_eq!(
            GenAIBuilder::strip_user_query_prefix(text),
            "plain user input"
        );
    }

    #[test]
    fn test_strip_user_query_prefix_bracket_no_datetime() {
        let text = "[not a timestamp] content";
        // No ':' and digit in bracket content -> returns original
        assert_eq!(
            GenAIBuilder::strip_user_query_prefix(text),
            "[not a timestamp] content"
        );
    }

    #[test]
    fn test_extract_last_user_query() {
        let req = LLMRequest {
            messages: vec![
                InputMessage {
                    role: "system".to_string(),
                    parts: vec![MessagePart::Text {
                        content: "sys".to_string(),
                    }],
                    name: None,
                },
                InputMessage {
                    role: "user".to_string(),
                    parts: vec![MessagePart::Text {
                        content: "[Mon 2026-01-01 10:00 GMT+8] hi".to_string(),
                    }],
                    name: None,
                },
            ],
            temperature: None,
            max_tokens: None,
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
        assert_eq!(
            GenAIBuilder::extract_last_user_query(&req),
            Some("hi".to_string())
        );
    }

    #[test]
    fn test_resolve_agent_name_from_comm_with_cache() {
        let mut cache = HashMap::new();
        cache.insert(42u32, "CachedAgent".to_string());
        let result = GenAIBuilder::resolve_agent_name_from_comm("unknown", 42, &cache);
        assert_eq!(result, Some("CachedAgent".to_string()));
    }

    #[test]
    fn test_resolve_agent_name_from_comm_no_match() {
        let cache = HashMap::new();
        let result = GenAIBuilder::resolve_agent_name_from_comm("random_process", 99, &cache);
        assert!(result.is_none());
    }

    // ─── classify_call_kind_from_raw tests ─────────────────────────────────────

    use super::classify_call_kind_from_raw;

    #[test]
    fn test_raw_classify_recap_cosh_compaction() {
        // system_instructions contains both markers
        let sys = Some(
            serde_json::to_string(&serde_json::json!([
                {"content": "This summarizes internal chat history and <state_snapshot> data"}
            ]))
            .unwrap(),
        );
        assert_eq!(classify_call_kind_from_raw(&sys, ""), "recap");
    }

    #[test]
    fn test_raw_classify_recap_specialized_summarizer() {
        let sys = Some(
            serde_json::to_string(&serde_json::json!([
                {"content": "You are a specialized context summarizer"}
            ]))
            .unwrap(),
        );
        assert_eq!(classify_call_kind_from_raw(&sys, ""), "recap");
    }

    #[test]
    fn test_raw_classify_recap_tool_output() {
        let first_user = "Summarize the following tool output to be a maximum of 500 tokens: ...";
        assert_eq!(classify_call_kind_from_raw(&None, first_user), "recap");
    }

    #[test]
    fn test_raw_classify_recap_claude_code() {
        let first_user = "Your task is to create a detailed summary of the conversation so far. Please Do NOT call any tools in this turn.";
        assert_eq!(classify_call_kind_from_raw(&None, first_user), "recap");
    }

    #[test]
    fn test_raw_classify_web_search() {
        let first_user = "Perform a web search for the query: rust async";
        assert_eq!(classify_call_kind_from_raw(&None, first_user), "web_search");
    }

    #[test]
    fn test_raw_classify_main_default() {
        assert_eq!(
            classify_call_kind_from_raw(&None, "explain this code"),
            "main"
        );
    }

    #[test]
    fn test_raw_classify_main_no_sys_instructions() {
        assert_eq!(classify_call_kind_from_raw(&None, ""), "main");
    }

    #[test]
    fn test_raw_classify_content_array_format() {
        // system_instructions with content as array of {type, text} objects
        let sys = Some(
            serde_json::to_string(&serde_json::json!([
                {"content": [{"type": "text", "text": "This summarizes internal chat history"}, {"type": "text", "text": "and <state_snapshot> data"}]}
            ]))
            .unwrap(),
        );
        assert_eq!(classify_call_kind_from_raw(&sys, ""), "recap");
    }

    #[test]
    fn test_raw_classify_invalid_json_sys_instructions() {
        // Invalid JSON in system_instructions → falls through to first_user_text
        let sys = Some("not valid json".to_string());
        assert_eq!(classify_call_kind_from_raw(&sys, ""), "main");
    }

    #[test]
    fn test_raw_classify_partial_recap_markers_not_enough() {
        // Only one of the two markers → not recap
        let sys = Some(
            serde_json::to_string(&serde_json::json!([
                {"content": "This summarizes internal chat history without snapshot"}
            ]))
            .unwrap(),
        );
        assert_eq!(classify_call_kind_from_raw(&sys, ""), "main");
    }

    #[test]
    fn test_call_kind_as_str() {
        assert_eq!(CallKind::Main.as_str(), "main");
        assert_eq!(CallKind::Recap.as_str(), "recap");
        assert_eq!(CallKind::WebSearch.as_str(), "web_search");
    }

    #[test]
    fn test_classify_with_non_text_parts_system() {
        // System message with only a ToolCall part → no text extracted → Main
        let req = make_llm_request(vec![InputMessage {
            role: "system".to_string(),
            parts: vec![MessagePart::ToolCall {
                id: None,
                name: "read_file".to_string(),
                arguments: None,
            }],
            name: None,
        }]);
        assert_eq!(classify_call_kind(&req), CallKind::Main);
    }

    #[test]
    fn test_classify_with_non_text_parts_user() {
        // User message with ToolCallResponse part → first_user_text is None → Main
        let req = make_llm_request(vec![InputMessage {
            role: "user".to_string(),
            parts: vec![MessagePart::ToolCallResponse {
                id: Some("tc-1".to_string()),
                response: serde_json::json!({"result": "ok"}),
            }],
            name: None,
        }]);
        assert_eq!(classify_call_kind(&req), CallKind::Main);
    }

    #[test]
    fn test_extract_model_from_message() {
        let builder = GenAIBuilder::new();
        let msg = Some(ParsedApiMessage::OpenAICompletion {
            request: Some(crate::analyzer::message::types::OpenAIRequest {
                model: "gpt-4-turbo".to_string(),
                messages: vec![],
                temperature: None,
                max_tokens: None,
                stream: None,
                top_p: None,
                n: None,
                stop: None,
                presence_penalty: None,
                frequency_penalty: None,
                user: None,
                tools: None,
                tool_choice: None,
                response_format: None,
                seed: None,
                logprobs: None,
                top_logprobs: None,
                parallel_tool_calls: None,
            }),
            response: None,
        });
        assert_eq!(
            builder.extract_model_from_message(&msg),
            Some("gpt-4-turbo".to_string())
        );
        assert_eq!(builder.extract_model_from_message(&None), None);
    }
}
