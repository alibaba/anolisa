use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    PreToolUse,
    PostToolUse,
}

impl HookEvent {
    pub fn as_str(&self) -> &'static str {
        match self {
            HookEvent::PreToolUse => "PreToolUse",
            HookEvent::PostToolUse => "PostToolUse",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum HookDecision {
    Approve,
    Block(String),
    Ask,
    Passthrough,
}

pub struct HookSystem {
    enabled: bool,
}

impl HookSystem {
    pub fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    pub async fn fire(
        &self,
        event: HookEvent,
        tool_name: &str,
        tool_input: &Value,
    ) -> HookDecision {
        if !self.enabled {
            return HookDecision::Passthrough;
        }

        let payload = serde_json::json!({
            "event": event.as_str(),
            "tool_name": tool_name,
            "tool_input": tool_input,
        });

        self.execute_hooks(&payload).await
    }

    async fn execute_hooks(&self, payload: &Value) -> HookDecision {
        let hooks = self.find_hooks(payload);
        if hooks.is_empty() {
            return HookDecision::Passthrough;
        }

        for hook_cmd in hooks {
            match self.run_hook_command(&hook_cmd, payload).await {
                Ok(decision) if decision != HookDecision::Passthrough => return decision,
                Ok(_) => continue,
                Err(e) => {
                    eprintln!("[cosh-core] Hook error: {e}");
                    continue;
                }
            }
        }

        HookDecision::Passthrough
    }

    fn find_hooks(&self, _payload: &Value) -> Vec<String> {
        Vec::new()
    }

    async fn run_hook_command(
        &self,
        command: &str,
        payload: &Value,
    ) -> Result<HookDecision, String> {
        use tokio::process::Command;
        use tokio::io::AsyncWriteExt;

        let mut child = Command::new("sh")
            .arg("-c")
            .arg(command)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to spawn hook: {e}"))?;

        if let Some(mut stdin) = child.stdin.take() {
            let payload_str = serde_json::to_string(payload).unwrap_or_default();
            let _ = stdin.write_all(payload_str.as_bytes()).await;
            let _ = stdin.shutdown().await;
        }

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            child.wait_with_output(),
        )
        .await
        .map_err(|_| "Hook timed out".to_string())?
        .map_err(|e| format!("Hook execution failed: {e}"))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        Self::parse_hook_output(&stdout)
    }

    fn parse_hook_output(output: &str) -> Result<HookDecision, String> {
        let trimmed = output.trim();
        if trimmed.is_empty() {
            return Ok(HookDecision::Passthrough);
        }

        if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
            match v.get("decision").and_then(|d| d.as_str()) {
                Some("approve") => return Ok(HookDecision::Approve),
                Some("block") => {
                    let reason = v
                        .get("reason")
                        .and_then(|r| r.as_str())
                        .unwrap_or("blocked by hook")
                        .to_string();
                    return Ok(HookDecision::Block(reason));
                }
                Some("ask") => return Ok(HookDecision::Ask),
                _ => {}
            }
        }

        Ok(HookDecision::Passthrough)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_event_str() {
        assert_eq!(HookEvent::PreToolUse.as_str(), "PreToolUse");
        assert_eq!(HookEvent::PostToolUse.as_str(), "PostToolUse");
    }

    #[test]
    fn parse_approve_output() {
        let result =
            HookSystem::parse_hook_output(r#"{"decision":"approve"}"#).unwrap();
        assert_eq!(result, HookDecision::Approve);
    }

    #[test]
    fn parse_block_output() {
        let result =
            HookSystem::parse_hook_output(r#"{"decision":"block","reason":"unsafe"}"#)
                .unwrap();
        assert_eq!(result, HookDecision::Block("unsafe".to_string()));
    }

    #[test]
    fn parse_ask_output() {
        let result =
            HookSystem::parse_hook_output(r#"{"decision":"ask"}"#).unwrap();
        assert_eq!(result, HookDecision::Ask);
    }

    #[test]
    fn parse_empty_output() {
        let result = HookSystem::parse_hook_output("").unwrap();
        assert_eq!(result, HookDecision::Passthrough);
    }

    #[test]
    fn parse_invalid_json_passthrough() {
        let result = HookSystem::parse_hook_output("not json").unwrap();
        assert_eq!(result, HookDecision::Passthrough);
    }

    #[tokio::test]
    async fn disabled_hook_returns_passthrough() {
        let hs = HookSystem::new(false);
        let decision = hs
            .fire(HookEvent::PreToolUse, "shell", &serde_json::json!({}))
            .await;
        assert_eq!(decision, HookDecision::Passthrough);
    }

    #[tokio::test]
    async fn enabled_hook_no_hooks_returns_passthrough() {
        let hs = HookSystem::new(true);
        let decision = hs
            .fire(HookEvent::PreToolUse, "shell", &serde_json::json!({}))
            .await;
        assert_eq!(decision, HookDecision::Passthrough);
    }
}
