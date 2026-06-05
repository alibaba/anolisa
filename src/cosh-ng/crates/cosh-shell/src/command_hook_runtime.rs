use super::*;

const MAX_COMMAND_HOOK_HINTS: usize = 32;

pub(super) fn record_command_result_hooks(blocks: &[CommandBlock], state: &mut InlineState) {
    for block in blocks {
        if !state.handled_command_hooks.insert(block.id.clone()) {
            continue;
        }

        let Some(prompt_hint) = prompt_hint_for_block(block) else {
            continue;
        };
        let finding_markdown = finding_markdown_for_block(block);

        state.command_hook_hints.push(RuntimeCommandHookHint {
            id: format!("hook-hint-{}", block.id),
            command_block_id: block.id.clone(),
            ended_at_ms: block.ended_at_ms,
            prompt_hint,
            finding_markdown,
        });
    }

    if state.command_hook_hints.len() > MAX_COMMAND_HOOK_HINTS {
        let drop_count = state.command_hook_hints.len() - MAX_COMMAND_HOOK_HINTS;
        state.command_hook_hints.drain(0..drop_count);
    }
}

pub(super) fn render_command_hook_findings<W: Write>(
    blocks: &[CommandBlock],
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let block_ids = blocks
        .iter()
        .map(|block| block.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    let renderer = RatatuiInlineRenderer::for_terminal();

    for hint in &state.command_hook_hints {
        if !block_ids.contains(hint.command_block_id.as_str())
            || !state.rendered_command_hook_findings.insert(hint.id.clone())
        {
            continue;
        }

        let Some(markdown) = hint.finding_markdown.as_deref() else {
            continue;
        };
        renderer.write_notice(
            output,
            "Command hook",
            renderer.markdown_text_lines(markdown),
            None,
        )?;
    }

    Ok(())
}

pub(super) fn command_hook_hints_for_blocks(
    state: &InlineState,
    blocks: &[CommandBlock],
) -> Vec<String> {
    let block_ids = blocks
        .iter()
        .map(|block| block.id.as_str())
        .collect::<std::collections::HashSet<_>>();

    state
        .command_hook_hints
        .iter()
        .filter(|hint| block_ids.contains(hint.command_block_id.as_str()))
        .map(|hint| {
            format!(
                "{} block={} ended_at_ms={} {}",
                hint.id, hint.command_block_id, hint.ended_at_ms, hint.prompt_hint
            )
        })
        .collect()
}

pub(super) fn command_hook_hints_for_block(
    state: &InlineState,
    block: &CommandBlock,
) -> Vec<String> {
    state
        .command_hook_hints
        .iter()
        .filter(|hint| hint.command_block_id == block.id)
        .map(|hint| {
            format!(
                "{} block={} ended_at_ms={} {}",
                hint.id, hint.command_block_id, hint.ended_at_ms, hint.prompt_hint
            )
        })
        .collect()
}

fn prompt_hint_for_block(block: &CommandBlock) -> Option<String> {
    if !should_analyze_failed_block(block) {
        return None;
    }

    let output_ref = block
        .output
        .terminal_output_ref
        .as_deref()
        .unwrap_or("<missing>");
    let command = block.command.trim();
    let category = if looks_like_test_or_build(command) {
        "test/build command failed"
    } else {
        "command failed"
    };

    Some(format!(
        "{category}; exit={}; output_ref={output_ref}; command={command}. Inspect output_ref before suggesting fixes or follow-up commands.",
        block.exit_code
    ))
}

fn finding_markdown_for_block(block: &CommandBlock) -> Option<String> {
    if !should_analyze_failed_block(block) {
        return None;
    }

    let output_ref = block
        .output
        .terminal_output_ref
        .as_deref()
        .unwrap_or("<missing>");
    let category = if looks_like_test_or_build(&block.command) {
        "test/build command"
    } else {
        "shell command"
    };

    Some(format!(
        "## Command result finding\n\n- `{}` exited with code `{}`.\n- Category: {category}.\n- Output ref: `{output_ref}`.\n\nAgent follow-up must inspect the output ref before claiming details.",
        block.command.trim(),
        block.exit_code
    ))
}

fn looks_like_test_or_build(command: &str) -> bool {
    let trimmed = command.trim();
    trimmed.starts_with("cargo test")
        || trimmed.starts_with("cargo build")
        || trimmed.starts_with("npm test")
        || trimmed.starts_with("pnpm test")
        || trimmed.starts_with("yarn test")
        || trimmed.starts_with("make test")
}
