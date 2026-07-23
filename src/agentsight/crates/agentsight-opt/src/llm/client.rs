use anyhow::{Context, Result};
use rig_core::{
    client::CompletionClient,
    completion::{AssistantContent, CompletionModel, Message},
    providers::openai,
};
use std::sync::Arc;
use tokio::sync::Semaphore;

use super::types::*;

pub struct LlmClient {
    base_url: String,
    api_key: String,
    model: String,
    temperature: Option<f64>,
    semaphore: Arc<Semaphore>,
    max_retries: u32,
}

impl LlmClient {
    /// Get the configured model name.
    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn from_env() -> Result<Self> {
        let base_url =
            std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".into());
        let api_key = std::env::var("OPENAI_API_KEY").context("OPENAI_API_KEY not set")?;
        let model = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o".into());

        Ok(Self {
            base_url: base_url.trim_end_matches('/').into(),
            api_key,
            model,
            temperature: None,
            semaphore: Arc::new(Semaphore::new(5)),
            max_retries: 3,
        })
    }

    pub fn with_config(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            model: model.into(),
            temperature: None,
            semaphore: Arc::new(Semaphore::new(5)),
            max_retries: 3,
        }
    }

    pub fn set_model(&mut self, m: impl Into<String>) {
        self.model = m.into();
    }

    pub fn set_base_url(&mut self, url: impl Into<String>) {
        self.base_url = url.into().trim_end_matches('/').to_string();
    }

    pub fn set_api_key(&mut self, key: impl Into<String>) {
        self.api_key = key.into();
    }

    pub fn set_temperature(&mut self, t: f64) {
        self.temperature = Some(t);
    }

    pub fn set_concurrency(&mut self, n: usize) {
        self.semaphore = Arc::new(Semaphore::new(n));
    }

    /// Build a rig-core OpenAI Completions API client from current config.
    fn build_rig_client(&self) -> Result<openai::CompletionsClient> {
        openai::Client::builder()
            .api_key(&self.api_key)
            .base_url(&self.base_url)
            .build()
            .map(|c| c.completions_api())
            .map_err(|e| anyhow::anyhow!("Failed to build rig client: {e}"))
    }

    /// Convert our ChatMessage to rig-core Message.
    fn to_rig_messages(messages: &[ChatMessage]) -> Vec<Message> {
        messages
            .iter()
            .map(|m| match m.role.as_str() {
                "system" => Message::System {
                    content: m.content.clone(),
                },
                "assistant" => Message::Assistant {
                    id: None,
                    content: rig_core::OneOrMany::one(AssistantContent::text(&m.content)),
                },
                _ => Message::User {
                    content: rig_core::OneOrMany::one(rig_core::message::UserContent::text(
                        &m.content,
                    )),
                },
            })
            .collect()
    }

    pub async fn chat(&self, messages: Vec<ChatMessage>) -> Result<String> {
        self.call(messages, false, None).await
    }

    pub async fn chat_json(&self, messages: Vec<ChatMessage>) -> Result<String> {
        self.call(messages, true, None).await
    }

    /// Chat with a label for log attribution (e.g. "perf:fast_tool").
    pub async fn chat_labeled(&self, messages: Vec<ChatMessage>, label: &str) -> Result<String> {
        self.call(messages, false, Some(label)).await
    }

    /// Chat JSON with a label for log attribution.
    pub async fn chat_json_labeled(
        &self,
        messages: Vec<ChatMessage>,
        label: &str,
    ) -> Result<String> {
        self.call(messages, true, Some(label)).await
    }

    pub async fn chat_json_parsed<T: serde::de::DeserializeOwned>(
        &self,
        messages: Vec<ChatMessage>,
    ) -> Result<T> {
        self.chat_json_parsed_labeled(messages, None).await
    }

    /// Parse JSON response with a label for log attribution.
    pub async fn chat_json_parsed_labeled<T: serde::de::DeserializeOwned>(
        &self,
        messages: Vec<ChatMessage>,
        label: Option<&str>,
    ) -> Result<T> {
        const MAX_FIX_ATTEMPTS: usize = 2;

        let mut convo = messages;
        let mut raw = self.call(convo.clone(), true, label).await?;

        for attempt in 0..=MAX_FIX_ATTEMPTS {
            // Try direct parse first, then extract JSON from markdown/text wrapper.
            let cleaned = Self::extract_json(&raw);
            let err = match serde_json::from_str::<T>(&cleaned) {
                Ok(v) => return Ok(v),
                Err(e) => e,
            };

            let preview: String = cleaned.chars().take(300).collect();
            if attempt == MAX_FIX_ATTEMPTS {
                return Err(anyhow::anyhow!(err)).with_context(|| {
                    format!(
                        "JSON parse failed after {MAX_FIX_ATTEMPTS} fix attempts. Raw: {preview}"
                    )
                });
            }
            tracing::warn!(
                "JSON parse failed (fix attempt {}/{}): {err}; cleaned preview: {preview}",
                attempt + 1,
                MAX_FIX_ATTEMPTS
            );

            // Feed the concrete parse error back so the model knows exactly
            // what to fix (e.g. "missing field `intents`").
            convo.push(ChatMessage {
                role: "assistant".into(),
                content: raw.clone(),
            });
            convo.push(ChatMessage::user(format!(
                "你返回的 JSON 无法解析，解析错误：{err}。\
                 请严格遵循 system 提示中要求的顶层字段和 schema，\
                 修正上述错误后重新输出完整、合法的 JSON，不要包含任何解释或多余文本。"
            )));
            raw = self.call(convo.clone(), true, label).await?;
        }
        unreachable!("loop always returns")
    }

    /// Extract JSON from LLM output that may be wrapped in markdown code fences
    /// or surrounded by extra text.
    fn extract_json(raw: &str) -> String {
        let trimmed = raw.trim();

        // Strip markdown code fences: ```json ... ``` or ``` ... ```
        let stripped = if trimmed.starts_with("```") {
            let without_fence = trimmed
                .trim_start_matches("```json")
                .trim_start_matches("```JSON")
                .trim_start_matches("```");
            without_fence.trim_end_matches("```").trim()
        } else {
            trimmed
        };

        // Find outermost { ... } or [ ... ]
        if let Some(start) = stripped.find('{') {
            if let Some(end) = stripped.rfind('}') {
                if end > start {
                    return stripped[start..=end].to_string();
                }
            }
        }
        if let Some(start) = stripped.find('[') {
            if let Some(end) = stripped.rfind(']') {
                if end > start {
                    return stripped[start..=end].to_string();
                }
            }
        }

        stripped.to_string()
    }

    /// Call the LLM with retry on empty/truncated responses.
    ///
    /// Reasoning models (glm-5.2, deepseek-r1, …) can spend all output tokens on
    /// `reasoning_content`, leaving `content` empty.  We detect this and retry
    /// with an explicit `max_tokens` budget so the model allocates tokens to the
    /// actual answer.
    async fn call(
        &self,
        messages: Vec<ChatMessage>,
        json_mode: bool,
        label: Option<&str>,
    ) -> Result<String> {
        let _permit = self.semaphore.acquire().await?;

        let client = self.build_rig_client()?;
        let rig_messages = Self::to_rig_messages(&messages);
        let tag = label.unwrap_or("llm");

        let mut last_err = None;
        for attempt in 0..=self.max_retries {
            if attempt > 0 {
                let delay = std::time::Duration::from_millis(500 * 2u64.pow(attempt - 1));
                tracing::debug!(
                    "[{tag}] Retry {attempt}/{} after {delay:?}",
                    self.max_retries
                );
                tokio::time::sleep(delay).await;
            }

            // On retry after empty/truncated response, escalate max_tokens.
            let max_tokens = if attempt == 0 {
                None
            } else {
                Some(16384u64 * attempt as u64)
            };

            match self
                .do_request(&client, &rig_messages, json_mode, max_tokens, tag)
                .await
            {
                Ok(text) if text.trim().is_empty() => {
                    tracing::warn!(
                        "[{tag}] LLM returned empty content (attempt {attempt}/{}), will retry with max_tokens",
                        self.max_retries
                    );
                    last_err = Some(anyhow::anyhow!(
                        "LLM returned empty content (reasoning model token exhaustion suspected)"
                    ));
                }
                Ok(text) => return Ok(text),
                Err(e) => {
                    tracing::warn!("[{tag}] LLM call attempt {attempt} failed: {e}");
                    last_err = Some(e);
                }
            }
        }

        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("LLM call failed with no recorded error")))
    }

    async fn do_request(
        &self,
        client: &openai::CompletionsClient,
        messages: &[Message],
        json_mode: bool,
        max_tokens: Option<u64>,
        tag: &str,
    ) -> Result<String> {
        let model = client.completion_model(&self.model);

        // Split messages: system → preamble, rest → chat_history, last → prompt
        let (preamble, history, prompt) = Self::split_messages(messages);

        // Build request using the builder
        let mut builder = model.completion_request(prompt);

        if let Some(p) = preamble {
            builder = builder.preamble(p);
        }

        for msg in history {
            builder = builder.message(msg);
        }

        if let Some(temp) = self.temperature {
            builder = builder.temperature(temp);
        }

        if let Some(mt) = max_tokens {
            builder = builder.max_tokens(mt);
        }

        // Add JSON mode via additional_params if needed
        if json_mode {
            builder = builder.additional_params(serde_json::json!({
                "response_format": { "type": "json_object" }
            }));
        }

        let response = builder
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Completion failed: {e}"))?;

        // Log token usage
        tracing::debug!(
            "[{tag}] Tokens: input={} output={} total={}",
            response.usage.input_tokens,
            response.usage.output_tokens,
            response.usage.total_tokens
        );

        // Extract text from response
        let text = response
            .choice
            .iter()
            .filter_map(|c| match c {
                AssistantContent::Text(t) => Some(t.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");

        // Log reasoning content if present
        for c in response.choice.iter() {
            if let AssistantContent::Reasoning(r) = c {
                let full_text = r.display_text();
                let reasoning_chars = full_text.chars().count();
                if reasoning_chars > 0 {
                    tracing::debug!(
                        "[{tag}] Reasoning ({} chars):\n{}",
                        reasoning_chars,
                        full_text
                    );
                }
            }
        }

        Ok(text)
    }

    /// Split messages into (preamble, history, prompt).
    /// - First system message becomes preamble
    /// - Last message becomes the prompt
    /// - Everything in between becomes history
    fn split_messages(messages: &[Message]) -> (Option<String>, Vec<Message>, Message) {
        let mut preamble = None;
        let mut history = Vec::new();

        for (i, msg) in messages.iter().enumerate() {
            if i == 0 {
                if let Message::System { content } = msg {
                    preamble = Some(content.clone());
                    continue;
                }
            }
            if i < messages.len() - 1 {
                history.push(msg.clone());
            }
        }

        let prompt = messages.last().cloned().unwrap_or(Message::User {
            content: rig_core::OneOrMany::one(rig_core::message::UserContent::text("")),
        });

        (preamble, history, prompt)
    }
}
