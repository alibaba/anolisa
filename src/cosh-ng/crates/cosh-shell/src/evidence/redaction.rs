use crate::tools::{classify_command_interaction, OutputStability};
use crate::types::{CommandBlock, CommandStatus};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCommandFacts {
    pub id: String,
    pub command: String,
    pub cwd: String,
    pub end_cwd: String,
    pub status: &'static str,
    pub exit_code: i32,
    pub duration_ms: u64,
    pub output_bytes: u64,
    pub output_id: String,
    pub output_stability: &'static str,
}

pub fn redact_provider_command_text(command: &str) -> String {
    let mut redacted = Vec::new();
    let mut redact_next = false;
    for token in command.split_whitespace() {
        if redact_next {
            redacted.push("<redacted>".to_string());
            redact_next = false;
            continue;
        }

        let lower = token.to_ascii_lowercase();
        if is_sensitive_value_flag(&lower) || lower == "bearer" {
            redacted.push(token.to_string());
            redact_next = true;
            continue;
        }
        let (token, changed) = redact_sensitive_assignment_token(token);
        if changed {
            redacted.push(token);
            continue;
        }
        if is_key_like_token(&token) {
            redacted.push("<redacted>".to_string());
        } else {
            redacted.push(token.to_string());
        }
    }
    redacted.join(" ")
}

pub fn terminal_output_id(shell_session_id: &str, command_id: &str) -> String {
    format!("terminal-output://{shell_session_id}/{command_id}")
}

pub fn provider_safe_command_facts(block: &CommandBlock) -> ProviderCommandFacts {
    let status = match block.status {
        CommandStatus::Completed => "completed",
        CommandStatus::Failed => "failed",
    };
    let output_id = if block.output.terminal_output_ref.is_some() {
        terminal_output_id(&block.session_id, &block.id)
    } else {
        "<missing>".to_string()
    };
    let output_stability = match classify_command_interaction(&block.command).output_stability {
        OutputStability::StableSnapshot => "stable_snapshot",
        OutputStability::UnstableInteractive => "unstable_interactive",
    };
    ProviderCommandFacts {
        id: block.id.clone(),
        command: redact_provider_command_text(&block.command),
        cwd: block.cwd.clone(),
        end_cwd: block.end_cwd.clone(),
        status,
        exit_code: block.exit_code,
        duration_ms: block.duration_ms,
        output_bytes: block.output.terminal_output_bytes,
        output_id,
        output_stability,
    }
}

fn is_sensitive_value_flag(lower_token: &str) -> bool {
    matches!(
        lower_token,
        "--password"
            | "--passwd"
            | "--token"
            | "--secret"
            | "--api-key"
            | "--apikey"
            | "--access-token"
            | "--authorization"
    )
}

fn redact_sensitive_assignment_token(token: &str) -> (String, bool) {
    let Some(eq_pos) = token.find('=') else {
        return (token.to_string(), false);
    };
    let key_start = token[..eq_pos]
        .rfind(|ch: char| ['?', '&', '-'].contains(&ch))
        .map(|idx| idx + 1)
        .unwrap_or(0);
    let key = token[key_start..eq_pos].to_ascii_lowercase();
    if !matches!(
        key.as_str(),
        "password"
            | "passwd"
            | "token"
            | "secret"
            | "api_key"
            | "apikey"
            | "access_token"
            | "authorization"
    ) {
        return (token.to_string(), false);
    }

    let value_start = eq_pos + 1;
    let value_end = token[value_start..]
        .find('&')
        .map(|idx| value_start + idx)
        .unwrap_or(token.len());
    let mut redacted = String::new();
    redacted.push_str(&token[..value_start]);
    redacted.push_str("<redacted>");
    redacted.push_str(&token[value_end..]);
    (redacted, true)
}

fn is_key_like_token(token: &str) -> bool {
    token.starts_with("ghp_")
        || token.starts_with("github_pat_")
        || token.starts_with("sk-")
        || (token.starts_with("AKIA")
            && token.len() >= 20
            && token
                .chars()
                .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit()))
}

pub fn provider_safe_command_fact_line(block: &CommandBlock) -> String {
    let facts = provider_safe_command_facts(block);
    format!(
        "command_id={id}; command={command}; cwd={cwd}; end_cwd={end_cwd}; status={status}; exit_code={exit_code}; duration_ms={duration_ms}; output_bytes={output_bytes}; output_id={output_id}; output_stability={output_stability}",
        id = facts.id,
        command = facts.command,
        cwd = facts.cwd,
        end_cwd = facts.end_cwd,
        status = facts.status,
        exit_code = facts.exit_code,
        duration_ms = facts.duration_ms,
        output_bytes = facts.output_bytes,
        output_id = facts.output_id,
        output_stability = facts.output_stability,
    )
}
