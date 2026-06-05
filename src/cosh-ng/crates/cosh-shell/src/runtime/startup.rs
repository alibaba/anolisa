use std::io::{IsTerminal, Write};
use std::path::Path;

use crate::runtime::prelude::*;

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

pub(crate) fn render_startup_banner<W: Write>(
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
    let i18n = state.i18n();
    let startup_hook = evaluate_startup_hooks(cwd, i18n);

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
        i18n.format(
            cosh_shell::MessageId::StartupAdapterLine,
            &[
                ("adapter", adapter.name()),
                ("shell", shell_label),
                ("mode", state.approval_mode.label()),
            ],
        ),
        i18n.format(cosh_shell::MessageId::StartupCwdLine, &[("cwd", cwd)]),
        i18n.t(cosh_shell::MessageId::StartupCommandsLine)
            .to_string(),
    ];
    if let Some(markdown) = startup_hook.markdown {
        body.push(String::new());
        body.push(startup_hook.summary);
        for line in renderer.markdown_text_lines(&markdown) {
            body.push(line);
        }
    }
    renderer.write_banner(
        output,
        i18n.t(cosh_shell::MessageId::StartupTitle),
        body,
        None,
    )?;
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

fn evaluate_startup_hooks(cwd: &str, i18n: cosh_shell::I18n) -> StartupHookResult {
    if !startup_hooks_enabled() {
        return StartupHookResult {
            summary: i18n
                .t(cosh_shell::MessageId::StartupHooksNoneSummary)
                .to_string(),
            markdown: None,
        };
    }

    let mut findings = Vec::new();
    let cwd_path = Path::new(cwd);
    if cwd_path.join("Cargo.toml").is_file() {
        findings.push(format!(
            "- {}",
            i18n.t(cosh_shell::MessageId::StartupHooksRustProjectFinding)
        ));
    }

    if findings.is_empty() {
        findings.push(format!(
            "- {}",
            i18n.t(cosh_shell::MessageId::StartupHooksNoFindings)
        ));
    }

    StartupHookResult {
        summary: i18n
            .t(cosh_shell::MessageId::StartupHooksCompletedSummary)
            .to_string(),
        markdown: Some(format!(
            "## {}\n\n{}\n\n{}",
            i18n.t(cosh_shell::MessageId::StartupHooksFindingsHeading),
            findings.join("\n"),
            i18n.t(cosh_shell::MessageId::StartupHooksReadOnlyNote)
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
