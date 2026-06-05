use crate::types::CommandBlock;
use std::fs;

pub struct ContextWindowConfig {
    pub max_commands: usize,
    pub max_age_ms: u64,
    pub preview_lines_failed: usize,
    pub preview_lines_success: usize,
    pub preview_enabled: bool,
}

impl Default for ContextWindowConfig {
    fn default() -> Self {
        Self {
            max_commands: 10,
            max_age_ms: 30 * 60 * 1000,
            preview_lines_failed: 20,
            preview_lines_success: 5,
            preview_enabled: true,
        }
    }
}

pub struct ContextEntry {
    pub block: CommandBlock,
    pub preview: Option<String>,
    pub age_label: String,
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
    entries
}

fn output_preview(
    block: &CommandBlock,
    is_recent: bool,
    config: &ContextWindowConfig,
) -> Option<String> {
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
        lines.push(format!(
            "[{}] {} | exit={} | cwd={} | {}",
            entry.block.id, entry.block.command, entry.block.exit_code, entry.block.cwd,
            entry.age_label
        ));
        lines.push(format!(
            "  output: {} bytes, ref={}",
            entry.block.output.terminal_output_bytes,
            entry
                .block
                .output
                .terminal_output_ref
                .as_deref()
                .unwrap_or("<none>")
        ));
        if let Some(ref preview) = entry.preview {
            lines.push("  preview:".into());
            for pl in preview.lines().take(5) {
                lines.push(format!("    {pl}"));
            }
        }
    }
    lines.push("\nUse Read tool on output_ref paths for full output.".into());
    lines.join("\n")
}

pub fn context_blocks_from_entries(entries: &[ContextEntry]) -> Vec<CommandBlock> {
    entries.iter().map(|e| e.block.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CommandStatus, OutputRefs};

    fn make_block(id: &str, exit_code: i32, ended_at_ms: u64, output_ref: Option<&str>) -> CommandBlock {
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
            make_block("old", 0, 1000, None),         // 49+ min ago
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
        assert!(prompt.contains("Use Read tool on output_ref paths"));
    }

    #[test]
    fn context_blocks_from_entries_extracts_blocks() {
        let blocks = vec![make_block("a", 0, 1000, None), make_block("b", 1, 2000, None)];
        let config = ContextWindowConfig::default();
        let entries = build_context_window(&blocks, 3000, &config);
        let extracted = context_blocks_from_entries(&entries);
        assert_eq!(extracted.len(), 2);
        assert_eq!(extracted[0].id, "a");
        assert_eq!(extracted[1].id, "b");
    }
}
