// Owner: types (shell handoff contract). Handoff request model, bypass
// prefix, and untracked-status token shared across approval, shell_host,
// activity, runtime, and ui.
use serde::{Deserialize, Serialize};

pub const SHELL_HANDOFF_BYPASS_PREFIX: &str = "COSH_SHELL_HANDOFF_BYPASS=1 ";

/// Status string for a shell handoff that reached a prompt boundary without
/// ever being tracked by a preexec marker (see specs/shell-handoff-preexec-loss).
/// Cross-owner contract consumed by activity, runtime evidence delivery, and ui.
pub(crate) const SHELL_HANDOFF_UNTRACKED_STATUS: &str = "completed_untracked";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellHandoffRequest {
    pub command: String,
    pub exact_preview: String,
    pub source: String,
    pub actor: String,
    pub approval_id: String,
    pub run_id: String,
    pub request_id: Option<String>,
    pub tool_use_id: Option<String>,
    pub created_at_ms: u64,
    pub preview_hash: String,
}

impl ShellHandoffRequest {
    pub fn new(
        command: impl Into<String>,
        exact_preview: impl Into<String>,
        source: impl Into<String>,
        actor: impl Into<String>,
        approval_id: impl Into<String>,
        run_id: impl Into<String>,
        created_at_ms: u64,
    ) -> Result<Self, String> {
        let exact_preview = exact_preview.into();
        let request = Self {
            command: command.into(),
            preview_hash: preview_hash(&exact_preview),
            exact_preview,
            source: source.into(),
            actor: actor.into(),
            approval_id: approval_id.into(),
            run_id: run_id.into(),
            request_id: None,
            tool_use_id: None,
            created_at_ms,
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.command.trim().is_empty() {
            return Err("empty shell handoff command".to_string());
        }
        if self.command.contains('\0') {
            return Err("shell handoff command contains NUL byte".to_string());
        }
        if self.command.chars().any(|ch| matches!(ch, '\n' | '\r')) {
            return Err(
                "shell handoff command contains newline; multiline handoff is not enabled"
                    .to_string(),
            );
        }
        if self
            .command
            .chars()
            .any(|ch| ch.is_control() && !matches!(ch, '\t'))
        {
            return Err("shell handoff command contains blocked control character".to_string());
        }
        if self.exact_preview.is_empty() {
            return Err("shell handoff preview is empty".to_string());
        }
        if self.approval_id.trim().is_empty() {
            return Err("shell handoff approval id is empty".to_string());
        }
        if self.run_id.trim().is_empty() {
            return Err("shell handoff run id is empty".to_string());
        }
        Ok(())
    }

    pub fn pty_bytes(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let mut bytes = self.command.as_bytes().to_vec();
        bytes.push(b'\n');
        Ok(bytes)
    }

    pub fn handoff_pty_bytes(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let mut bytes = SHELL_HANDOFF_BYPASS_PREFIX.as_bytes().to_vec();
        bytes.extend_from_slice(self.command.as_bytes());
        bytes.push(b'\n');
        Ok(bytes)
    }
}

fn preview_hash(value: &str) -> String {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("fnv1a64:{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::{ShellHandoffRequest, SHELL_HANDOFF_BYPASS_PREFIX};

    fn handoff(command: &str) -> Result<ShellHandoffRequest, String> {
        ShellHandoffRequest::new(
            command,
            format!("$ {command}"),
            "test",
            "user",
            "approval-1",
            "run-1",
            42,
        )
    }

    #[test]
    fn shell_handoff_rejects_empty_nul_newline_and_control_chars() {
        for command in [
            "",
            "printf '\0'",
            "printf one\nprintf two",
            "printf '\u{1b}[31mred'",
        ] {
            assert!(handoff(command).is_err(), "{command:?}");
        }
    }

    #[test]
    fn shell_handoff_allows_visible_command_and_tab_separator() {
        let request = handoff("printf\tok").expect("tab-separated command is visible input");

        assert_eq!(request.pty_bytes().unwrap(), b"printf\tok\n");
        assert_eq!(
            request.handoff_pty_bytes().unwrap(),
            format!("{SHELL_HANDOFF_BYPASS_PREFIX}printf\tok\n").as_bytes()
        );
        assert_eq!(request.preview_hash, "fnv1a64:7d74cbb1a6f6fb27");
    }
}
