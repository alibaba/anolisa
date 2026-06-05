use std::io::{IsTerminal, Write};
use std::path::Path;

use super::*;

pub(super) fn render_startup_banner<W: Write>(
    events: &[ShellEvent],
    adapter: &AdapterInstance,
    shell_label: &str,
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    if state.rendered_startup_banner || !startup_banner_enabled() {
        return Ok(());
    }

    let Some(event) = events
        .iter()
        .find(|event| event.kind == ShellEventKind::ShellReady)
    else {
        return Ok(());
    };

    state.rendered_startup_banner = true;
    let cwd = event.cwd.as_deref().unwrap_or("<unknown>");
    let startup_hook = evaluate_startup_hooks(cwd);
    let ai_disabled = std::env::var("COSH_SHELL_AI")
        .ok()
        .is_some_and(|v| v.eq_ignore_ascii_case("off"));
    let ai_line = if ai_disabled {
        "AI: disabled".to_string()
    } else {
        format!("AI context may be sent to the {} backend.", adapter.name())
    };
    write!(output, "\r\x1b[2K")?;
    let renderer = RatatuiInlineRenderer::for_terminal();
    renderer.write_banner(
        output,
        "cosh-shell",
        vec![
            String::new(),
            "\u{250c}\u{2500}\u{2510}\u{250c}\u{2500}\u{2510}\u{250c}\u{2500}\u{2510}\u{252c} \u{252c}".to_string(),
            "\u{2502}  \u{2502} \u{2502}\u{2514}\u{2500}\u{2510}\u{251c}\u{2500}\u{2524}  shell".to_string(),
            "\u{2514}\u{2500}\u{2518}\u{2514}\u{2500}\u{2518}\u{2514}\u{2500}\u{2518}\u{2534} \u{2534}".to_string(),
            String::new(),
            format!(
                "Adapter: {}   Shell: {shell_label}   Mode: {}",
                adapter.name(),
                state.approval_mode.label()
            ),
            ai_line,
            format!("cwd: {cwd}"),
            String::new(),
            "/help \u{00b7} /mode \u{00b7} /details \u{00b7} /skill".to_string(),
            startup_hook.summary,
        ],
        Some("Agent actions still require approval."),
    )?;
    if let Some(markdown) = startup_hook.markdown {
        renderer.write_notice(
            output,
            "Startup hooks",
            renderer.markdown_text_lines(&markdown),
            Some("Read-only startup checks."),
        )?;
    }
    writeln!(output)?;
    write!(output, "cosh-osc$ ")?;
    output.flush()
}

fn startup_banner_enabled() -> bool {
    match std::env::var("COSH_SHELL_STARTUP_BANNER") {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on" | "always"
        ),
        Err(_) => std::io::stdout().is_terminal(),
    }
}

struct StartupHookResult {
    summary: String,
    markdown: Option<String>,
}

fn evaluate_startup_hooks(cwd: &str) -> StartupHookResult {
    if !startup_hooks_enabled() {
        return StartupHookResult {
            summary: "Startup hooks: none configured.".to_string(),
            markdown: None,
        };
    }

    let mut findings = Vec::new();
    let cwd_path = Path::new(cwd);
    if cwd_path.join("Cargo.toml").is_file() {
        findings.push(
            "- Rust project detected from `Cargo.toml`; `/skill` can show project-oriented Agent capabilities."
                .to_string(),
        );
    }

    if findings.is_empty() {
        findings.push("- No startup findings from built-in read-only checks.".to_string());
    }

    StartupHookResult {
        summary: "Startup hooks: built-in read-only checks completed.".to_string(),
        markdown: Some(format!(
            "## Startup findings\n\n{}\n\n`cosh-shell` only inspected lightweight startup context.",
            findings.join("\n")
        )),
    }
}

fn startup_hooks_enabled() -> bool {
    std::env::var("COSH_SHELL_STARTUP_HOOKS")
        .ok()
        .is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on" | "builtin" | "built-in"
            )
        })
}
