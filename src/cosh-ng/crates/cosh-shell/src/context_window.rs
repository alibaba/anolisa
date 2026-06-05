use crate::tools::{classify_command_interaction, OutputStability};
use crate::types::CommandBlock;
use std::fs;

pub struct ContextWindowConfig {
    pub max_commands: usize,
    pub max_age_ms: u64,
    pub preview_lines_failed: usize,
    pub preview_lines_success: usize,
    pub preview_enabled: bool,
    pub token_budget: usize,
}

impl Default for ContextWindowConfig {
    fn default() -> Self {
        Self {
            max_commands: 10,
            max_age_ms: 30 * 60 * 1000,
            preview_lines_failed: 20,
            preview_lines_success: 5,
            preview_enabled: true,
            token_budget: 2000,
        }
    }
}

pub struct ContextEntry {
    pub block: CommandBlock,
    pub preview: Option<String>,
    pub age_label: String,
}

pub struct RelatedHistoryConfig {
    pub max_commands: usize,
    pub max_age_ms: u64,
    pub neighbor_radius: usize,
    pub recent_failed_commands: usize,
    pub related_command_ids: Vec<String>,
}

impl Default for RelatedHistoryConfig {
    fn default() -> Self {
        Self {
            max_commands: 5,
            max_age_ms: 30 * 60 * 1000,
            neighbor_radius: 2,
            recent_failed_commands: 2,
            related_command_ids: Vec::new(),
        }
    }
}

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
        crate::types::CommandStatus::Completed => "completed",
        crate::types::CommandStatus::Failed => "failed",
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

pub fn build_context_window(
    blocks: &[CommandBlock],
    before_ms: u64,
    config: &ContextWindowConfig,
) -> Vec<ContextEntry> {
    let mut entries: Vec<_> = blocks
        .iter()
        .filter(|b| b.ended_at_ms <= before_ms)
        .filter(|b| before_ms.saturating_sub(b.ended_at_ms) <= config.max_age_ms)
        .rev()
        .take(config.max_commands)
        .enumerate()
        .map(|(i, block)| {
            let is_recent_3 = i < 3;
            let preview = if config.preview_enabled {
                output_preview(block, is_recent_3, config)
            } else {
                None
            };
            let age = format_age(before_ms, block.ended_at_ms);
            ContextEntry {
                block: block.clone(),
                preview,
                age_label: age,
            }
        })
        .collect();
    entries.reverse();
    trim_to_token_budget(&mut entries, config.token_budget);
    entries
}

pub fn build_related_history_index(
    blocks: &[CommandBlock],
    anchor: &CommandBlock,
    config: &RelatedHistoryConfig,
) -> Vec<ContextEntry> {
    let Some(anchor_index) = blocks
        .iter()
        .position(|block| block.session_id == anchor.session_id && block.id == anchor.id)
    else {
        return Vec::new();
    };

    let mut selected = Vec::<usize>::new();
    for id in &config.related_command_ids {
        if let Some(index) = blocks.iter().position(|block| {
            block.session_id == anchor.session_id
                && block.id == *id
                && block.ended_at_ms <= anchor.ended_at_ms
                && block.id != anchor.id
        }) {
            push_unique(&mut selected, index);
        }
    }

    let neighbor_start = anchor_index.saturating_sub(config.neighbor_radius);
    for index in neighbor_start..anchor_index {
        push_unique(&mut selected, index);
    }

    for (index, block) in blocks.iter().enumerate().take(anchor_index).rev() {
        if selected
            .iter()
            .filter(|candidate| blocks[**candidate].exit_code != 0)
            .count()
            >= config.recent_failed_commands
        {
            break;
        }
        if block.exit_code != 0 {
            push_unique(&mut selected, index);
        }
    }

    for (index, block) in blocks.iter().enumerate().take(anchor_index).rev() {
        if selected.len() >= config.max_commands {
            break;
        }
        if block.cwd == anchor.cwd || block.end_cwd == anchor.end_cwd {
            push_unique(&mut selected, index);
        }
    }

    selected.retain(|index| {
        let block = &blocks[*index];
        anchor.ended_at_ms.saturating_sub(block.ended_at_ms) <= config.max_age_ms
    });
    selected.sort_unstable_by_key(|index| blocks[*index].ended_at_ms);
    if selected.len() > config.max_commands {
        let drop_count = selected.len() - config.max_commands;
        selected.drain(0..drop_count);
    }

    selected
        .into_iter()
        .map(|index| {
            let block = blocks[index].clone();
            ContextEntry {
                age_label: format_age(anchor.ended_at_ms, block.ended_at_ms),
                block,
                preview: None,
            }
        })
        .collect()
}

fn push_unique(values: &mut Vec<usize>, value: usize) {
    if !values.contains(&value) {
        values.push(value);
    }
}

fn trim_to_token_budget(entries: &mut Vec<ContextEntry>, budget: usize) {
    let chars_budget = budget * 4;
    let mut total_chars = 0;
    let mut keep = entries.len();
    for (i, entry) in entries.iter().enumerate().rev() {
        let entry_chars = entry.block.command.len()
            + entry.block.cwd.len()
            + entry.preview.as_ref().map_or(0, |p| p.len())
            + 60;
        total_chars += entry_chars;
        if total_chars > chars_budget {
            keep = entries.len() - i;
            break;
        }
    }
    if keep < entries.len() {
        let start = entries.len() - keep;
        entries.drain(..start);
    }
}

fn output_preview(
    block: &CommandBlock,
    is_recent: bool,
    config: &ContextWindowConfig,
) -> Option<String> {
    if classify_command_interaction(&block.command).output_stability
        == OutputStability::UnstableInteractive
    {
        return None;
    }

    let max_lines = if block.exit_code != 0 {
        config.preview_lines_failed
    } else if is_recent {
        config.preview_lines_success
    } else {
        return None;
    };
    let path = block.output.terminal_output_ref.as_deref()?;
    read_preview_lines(path, max_lines)
}

fn read_preview_lines(path: &str, max_lines: usize) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    let lines: Vec<&str> = content.lines().take(max_lines).collect();
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

fn format_age(now_ms: u64, then_ms: u64) -> String {
    let diff_s = now_ms.saturating_sub(then_ms) / 1000;
    if diff_s < 60 {
        format!("{diff_s}s ago")
    } else if diff_s < 3600 {
        format!("{}min ago", diff_s / 60)
    } else {
        format!("{}h ago", diff_s / 3600)
    }
}

pub fn format_context_prompt(entries: &[ContextEntry]) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let mut lines = vec![format!(
        "\n\nRecent shell context ({} commands):\n",
        entries.len()
    )];
    for entry in entries {
        let facts = provider_safe_command_facts(&entry.block);
        lines.push(format!(
            "[{}] {} | exit={} | cwd={} | {}",
            facts.id, facts.command, facts.exit_code, facts.cwd, entry.age_label
        ));
        lines.push(format!(
            "  output: {} bytes, id={}, stability={}",
            facts.output_bytes, facts.output_id, facts.output_stability
        ));
        if let Some(ref preview) = entry.preview {
            lines.push("  preview:".into());
            for pl in preview.lines().take(5) {
                lines.push(format!("    {pl}"));
            }
        }
    }
    lines.push(
        "\nterminal-output:// refs are cosh-shell evidence ids, not files. Ask for more output only through cosh-shell evidence requests."
            .into(),
    );
    lines.join("\n")
}

pub fn context_blocks_from_entries(entries: &[ContextEntry]) -> Vec<CommandBlock> {
    entries.iter().map(|e| e.block.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CommandStatus, OutputRefs};

    fn make_block(
        id: &str,
        exit_code: i32,
        ended_at_ms: u64,
        output_ref: Option<&str>,
    ) -> CommandBlock {
        CommandBlock {
            id: id.to_string(),
            session_id: "s1".to_string(),
            command: format!("cmd-{id}"),
            cwd: "/repo".to_string(),
            end_cwd: "/repo".to_string(),
            started_at_ms: ended_at_ms.saturating_sub(100),
            ended_at_ms,
            duration_ms: 100,
            exit_code,
            status: if exit_code == 0 {
                CommandStatus::Completed
            } else {
                CommandStatus::Failed
            },
            output: OutputRefs {
                terminal_output_ref: output_ref.map(ToString::to_string),
                terminal_output_bytes: 42,
            },
        }
    }

    #[test]
    fn filters_blocks_after_before_ms() {
        let blocks = vec![
            make_block("a", 0, 1000, None),
            make_block("b", 0, 2000, None),
            make_block("c", 0, 3000, None),
        ];
        let config = ContextWindowConfig::default();
        let entries = build_context_window(&blocks, 2500, &config);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].block.id, "a");
        assert_eq!(entries[1].block.id, "b");
    }

    #[test]
    fn filters_blocks_by_max_age() {
        let now = 50 * 60 * 1000; // 50 minutes
        let blocks = vec![
            make_block("old", 0, 1000, None),            // 49+ min ago
            make_block("recent", 0, now - 10_000, None), // 10s ago
        ];
        let config = ContextWindowConfig {
            max_age_ms: 60_000, // 1 minute
            ..Default::default()
        };
        let entries = build_context_window(&blocks, now, &config);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].block.id, "recent");
    }

    #[test]
    fn respects_max_commands() {
        let blocks: Vec<_> = (0..20)
            .map(|i| make_block(&format!("{i}"), 0, (i + 1) * 1000, None))
            .collect();
        let config = ContextWindowConfig {
            max_commands: 5,
            ..Default::default()
        };
        let entries = build_context_window(&blocks, 100_000, &config);
        assert_eq!(entries.len(), 5);
        assert_eq!(entries[0].block.id, "15");
        assert_eq!(entries[4].block.id, "19");
    }

    #[test]
    fn preview_disabled_returns_none() {
        let blocks = vec![make_block("a", 1, 1000, None)];
        let config = ContextWindowConfig {
            preview_enabled: false,
            ..Default::default()
        };
        let entries = build_context_window(&blocks, 2000, &config);
        assert!(entries[0].preview.is_none());
    }

    #[test]
    fn skips_preview_for_interactive_docker_exec() {
        let dir = temp_context_dir("interactive-docker");
        let output_ref = dir.path().join("docker-output.txt");
        fs::write(
            &output_ref,
            "What's next:\nTry Docker Debug for seamless debugging\n",
        )
        .unwrap();
        let mut block = make_block("docker", 0, 1000, Some(output_ref.to_str().unwrap()));
        block.command = "docker exec -it cosh-hook-anolis23-dev bash".to_string();

        let entries = build_context_window(&[block], 2000, &ContextWindowConfig::default());

        assert_eq!(entries.len(), 1);
        assert!(entries[0].preview.is_none());
        let prompt = format_context_prompt(&entries);
        assert!(
            prompt.contains("stability=unstable_interactive"),
            "{prompt}"
        );
        assert!(!prompt.contains("Try Docker Debug"), "{prompt}");
    }

    #[test]
    fn keeps_preview_for_non_interactive_docker_commands() {
        let dir = temp_context_dir("non-interactive-docker");
        let output_ref = dir.path().join("docker-output.txt");
        fs::write(&output_ref, "container-id\n").unwrap();
        let mut block = make_block("docker", 0, 1000, Some(output_ref.to_str().unwrap()));
        block.command = "docker ps".to_string();

        let entries = build_context_window(&[block], 2000, &ContextWindowConfig::default());

        assert_eq!(entries[0].preview.as_deref(), Some("container-id"));
        let prompt = format_context_prompt(&entries);
        assert!(prompt.contains("stability=stable_snapshot"), "{prompt}");
    }

    struct TempContextDir {
        path: std::path::PathBuf,
    }

    impl TempContextDir {
        fn path(&self) -> &std::path::Path {
            &self.path
        }
    }

    impl Drop for TempContextDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn temp_context_dir(name: &str) -> TempContextDir {
        let path = std::env::temp_dir().join(format!(
            "cosh-context-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        TempContextDir { path }
    }

    #[test]
    fn age_label_seconds() {
        assert_eq!(format_age(10_000, 5_000), "5s ago");
    }

    #[test]
    fn age_label_minutes() {
        assert_eq!(format_age(200_000, 10_000), "3min ago");
    }

    #[test]
    fn age_label_hours() {
        assert_eq!(format_age(25_200_000, 0), "7h ago");
    }

    #[test]
    fn format_context_prompt_empty() {
        assert_eq!(format_context_prompt(&[]), "");
    }

    #[test]
    fn format_context_prompt_includes_metadata() {
        let blocks = vec![make_block("x", 0, 1000, None)];
        let config = ContextWindowConfig {
            preview_enabled: false,
            ..Default::default()
        };
        let entries = build_context_window(&blocks, 2000, &config);
        let prompt = format_context_prompt(&entries);
        assert!(prompt.contains("Recent shell context (1 commands)"));
        assert!(prompt.contains("[x] cmd-x"));
        assert!(prompt.contains("exit=0"));
        assert!(prompt.contains("stability=stable_snapshot"));
        assert!(prompt.contains("terminal-output:// refs are cosh-shell evidence ids"));
        assert!(!prompt.contains("Use Read tool on output_ref paths"));
    }

    #[test]
    fn provider_safe_command_fact_line_uses_output_id_without_path() {
        let block = make_block("x", 0, 1000, Some("/tmp/internal-output-ref.txt"));

        let line = provider_safe_command_fact_line(&block);

        assert!(line.contains("command_id=x"), "{line}");
        assert!(line.contains("output_id=terminal-output://s1/x"), "{line}");
        assert!(line.contains("output_stability=stable_snapshot"), "{line}");
        assert!(!line.contains("/tmp/internal-output-ref.txt"), "{line}");
        assert!(!line.contains("output_ref"), "{line}");
    }

    #[test]
    fn provider_safe_command_facts_redact_secret_like_command_values() {
        let mut block = make_block("x", 0, 1000, None);
        block.command = "curl https://example.test/api?token=query-secret --password cli-secret -H Authorization: Bearer bearer-secret ghp_abcdefghijklmnopqrstuvwxyz123456"
            .to_string();

        let line = provider_safe_command_fact_line(&block);

        assert!(line.contains("token=<redacted>"), "{line}");
        assert!(line.contains("--password <redacted>"), "{line}");
        assert!(line.contains("Bearer <redacted>"), "{line}");
        assert!(line.contains("<redacted>"), "{line}");
        assert!(!line.contains("query-secret"), "{line}");
        assert!(!line.contains("cli-secret"), "{line}");
        assert!(!line.contains("bearer-secret"), "{line}");
        assert!(!line.contains("ghp_"), "{line}");
        assert!(!line.contains("abcdefghijklmnopqrstuvwxyz123456"), "{line}");
    }

    #[test]
    fn provider_safe_command_facts_hide_missing_internal_ref() {
        let block = make_block("x", 1, 1000, None);

        let facts = provider_safe_command_facts(&block);

        assert_eq!(facts.id, "x");
        assert_eq!(facts.status, "failed");
        assert_eq!(facts.exit_code, 1);
        assert_eq!(facts.output_bytes, 42);
        assert_eq!(facts.output_id, "<missing>");
        assert_eq!(facts.output_stability, "stable_snapshot");
    }

    #[test]
    fn context_blocks_from_entries_extracts_blocks() {
        let blocks = vec![
            make_block("a", 0, 1000, None),
            make_block("b", 1, 2000, None),
        ];
        let config = ContextWindowConfig::default();
        let entries = build_context_window(&blocks, 3000, &config);
        let extracted = context_blocks_from_entries(&entries);
        assert_eq!(extracted.len(), 2);
        assert_eq!(extracted[0].id, "a");
        assert_eq!(extracted[1].id, "b");
    }

    #[test]
    fn related_history_index_selects_facts_without_previews() {
        let mut setup = make_block("setup", 0, 1_000, Some("/tmp/setup.txt"));
        setup.cwd = "/repo".to_string();
        setup.end_cwd = "/repo".to_string();
        let mut other = make_block("other", 0, 2_000, Some("/tmp/other.txt"));
        other.cwd = "/other".to_string();
        other.end_cwd = "/other".to_string();
        let mut failed = make_block("failed", 2, 3_000, Some("/tmp/failed.txt"));
        failed.cwd = "/other".to_string();
        failed.end_cwd = "/other".to_string();
        let mut nearby = make_block("nearby", 0, 4_000, Some("/tmp/nearby.txt"));
        nearby.cwd = "/tmp".to_string();
        nearby.end_cwd = "/tmp".to_string();
        let mut anchor = make_block("anchor", 1, 5_000, Some("/tmp/anchor.txt"));
        anchor.cwd = "/repo".to_string();
        anchor.end_cwd = "/repo".to_string();
        let blocks = vec![
            setup.clone(),
            other,
            failed.clone(),
            nearby.clone(),
            anchor.clone(),
        ];
        let config = RelatedHistoryConfig {
            max_commands: 5,
            neighbor_radius: 1,
            recent_failed_commands: 1,
            related_command_ids: vec!["setup".to_string()],
            ..Default::default()
        };

        let entries = build_related_history_index(&blocks, &anchor, &config);
        let ids = entries
            .iter()
            .map(|entry| entry.block.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(ids, vec!["setup", "failed", "nearby"]);
        assert!(entries.iter().all(|entry| entry.preview.is_none()));
    }
}
