use std::io::{IsTerminal, Write};
use std::path::Path;

use super::*;

const LOGO_LINES: &[&str] = &[
    "  ██████╗  ██████╗  ███████╗ ██╗  ██╗",
    " ██╔════╝ ██╔═══██╗ ██╔════╝ ██║  ██║",
    " ██║      ██║   ██║ ███████╗ ███████║",
    " ██║      ██║   ██║ ╚════██║ ██╔══██║",
    " ╚██████╗ ╚██████╔╝ ███████║ ██║  ██║",
    "  ╚═════╝  ╚═════╝  ╚══════╝ ╚═╝  ╚═╝",
];

const LOGO_COLORS: &[&str] = &[
    "\x1b[1;38;5;33m",
    "\x1b[1;38;5;33m",
    "\x1b[1;38;5;39m",
    "\x1b[1;38;5;39m",
    "\x1b[1;38;5;117m",
    "\x1b[1;38;5;117m",
];

const RESET: &str = "\x1b[0m";
const LOGO_MIN_WIDTH: u16 = 42;

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

    write!(output, "\r\x1b[2K")?;
    let renderer = RatatuiInlineRenderer::for_terminal();

    let term_width = ratatui::crossterm::terminal::size()
        .map(|(cols, _)| cols)
        .unwrap_or(80);

    if term_width >= LOGO_MIN_WIDTH {
        writeln!(output)?;
        for (i, line) in LOGO_LINES.iter().enumerate() {
            writeln!(output, "{}{}{}", LOGO_COLORS[i], line, RESET)?;
        }
        writeln!(output)?;
    }

    let mut body = vec![
        format!(
            "Adapter: {} \u{00b7} Shell: {shell_label} \u{00b7} Mode: {}",
            adapter.name(),
            state.approval_mode.user_mode_label()
        ),
        format!("cwd: {cwd}"),
        "/help \u{00b7} /mode \u{00b7} /explain".to_string(),
    ];
    if let Some(markdown) = startup_hook.markdown {
        body.push(String::new());
        body.push(startup_hook.summary);
        for line in renderer.markdown_text_lines(&markdown) {
            body.push(line);
        }
    }
    renderer.write_banner(output, "cosh-shell", body, None)?;
    writeln!(output)?;
    if std::env::var("COSH_SHELL_ISOLATED").is_ok() {
        write!(output, "cosh-osc$ ")?;
    } else {
        state.trigger_pty_prompt = true;
    }
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
