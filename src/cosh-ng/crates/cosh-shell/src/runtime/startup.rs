use std::collections::HashSet;
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::diagnostics::health::{
    record_startup_health_recommendations, HealthScanReport, HealthSeverity,
};
use crate::runtime::cli_args::RawShellKind;
use crate::runtime::prelude::*;
use crate::runtime::state::PendingInputGhostBinding;

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
const STARTUP_HEALTH_ROW_WAIT: Duration = Duration::from_millis(150);

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
    let renderer = RatatuiInlineRenderer::for_terminal().with_language(state.language);

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
            MessageId::StartupAdapterLine,
            &[
                ("adapter", adapter.name()),
                ("shell", shell_label),
                ("approval", state.approval_mode.label()),
                ("analysis", state.analysis_mode.label()),
            ],
        ),
        i18n.format(MessageId::StartupCwdLine, &[("cwd", cwd)]),
        i18n.t(MessageId::StartupCommandsLine).to_string(),
    ];
    state.startup_health.wait_ready(STARTUP_HEALTH_ROW_WAIT);
    if let Some(report) = state.startup_health.report.as_ref() {
        let prompt_ghost = startup_health_prompt_ghost(report, i18n);
        if health_uses_startup_row(report) {
            body.push(renderer.startup_section_separator_line());
            body.extend(renderer.health_startup_row_lines(HealthBannerModel { report }));
            state.startup_health.rendered = true;
            record_startup_health_recommendations(report);
        }
        set_startup_health_prompt_ghost(state, prompt_ghost);
    }
    if let Some(markdown) = startup_hook.markdown {
        body.push(String::new());
        body.push(startup_hook.summary);
        for line in renderer.markdown_text_lines(&markdown) {
            body.push(line);
        }
    }
    body.push(i18n.t(MessageId::StartupSwitchHint).to_string());
    renderer.write_banner(output, i18n.t(MessageId::StartupTitle), body, None)?;
    writeln!(output)?;
    restore_startup_prompt(state, output)?;
    output.flush()
}

pub(crate) fn render_startup_health_banner<W: Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    if !state.rendered_startup_banner || state.startup_health.rendered {
        return Ok(());
    }

    state.startup_health.poll_ready();
    let Some(report) = state.startup_health.report.as_ref() else {
        return Ok(());
    };

    state.startup_health.rendered = true;
    let prompt_ghost = startup_health_prompt_ghost(report, state.i18n());
    write!(output, "\r\x1b[2K")?;
    let renderer = RatatuiInlineRenderer::for_terminal().with_language(state.language);
    renderer.write_health_banner(output, HealthBannerModel { report })?;
    record_startup_health_recommendations(report);
    set_startup_health_prompt_ghost(state, prompt_ghost);
    writeln!(output)?;
    restore_startup_prompt(state, output)?;
    output.flush()
}

fn restore_startup_prompt<W: Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    if std::env::var("COSH_SHELL_ISOLATED").is_ok() {
        write!(output, "cosh-osc$ ")?;
    } else {
        state.trigger_pty_prompt = true;
    }
    Ok(())
}

fn startup_health_prompt_ghost(report: &HealthScanReport, i18n: crate::I18n) -> Option<String> {
    startup_prompt_ghost_allowed(report).then(|| primary_health_prompt_suggestion(report, i18n))?
}

fn set_startup_health_prompt_ghost(state: &mut InlineState, prompt: Option<String>) {
    state.pending_input_ghost = prompt.clone();
    state.pending_input_ghost_binding = prompt.map(|_| {
        PendingInputGhostBinding::AgentContext(AgentContextBinding::StartupHealthFollowUp)
    });
}

fn startup_prompt_ghost_allowed(report: &HealthScanReport) -> bool {
    if std::env::var("COSH_SHELL_ISOLATED").is_ok() || std::env::var("NO_COLOR").is_ok() {
        return false;
    }
    if std::env::var("TERM")
        .map(|term| term.eq_ignore_ascii_case("dumb"))
        .unwrap_or(false)
    {
        return false;
    }
    if std::env::var("COSH_SHELL_RENDER")
        .map(|mode| mode.eq_ignore_ascii_case("plain") || mode.eq_ignore_ascii_case("text"))
        .unwrap_or(false)
    {
        return false;
    }
    !matches!(
        report.overall_severity,
        HealthSeverity::Unavailable | HealthSeverity::Degraded
    )
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

fn evaluate_startup_hooks(cwd: &str, i18n: I18n) -> StartupHookResult {
    if !startup_hooks_enabled() {
        return StartupHookResult {
            summary: i18n.t(MessageId::StartupHooksNoneSummary).to_string(),
            markdown: None,
        };
    }

    let mut findings = Vec::new();
    let cwd_path = Path::new(cwd);
    if cwd_path.join("Cargo.toml").is_file() {
        findings.push(format!(
            "- {}",
            i18n.t(MessageId::StartupHooksRustProjectFinding)
        ));
    }

    if findings.is_empty() {
        findings.push(format!("- {}", i18n.t(MessageId::StartupHooksNoFindings)));
    }

    StartupHookResult {
        summary: i18n.t(MessageId::StartupHooksCompletedSummary).to_string(),
        markdown: Some(format!(
            "## {}\n\n{}\n\n{}",
            i18n.t(MessageId::StartupHooksFindingsHeading),
            findings.join("\n"),
            i18n.t(MessageId::StartupHooksReadOnlyNote)
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

pub(crate) fn bootstrap_process_path_from_shell(shell_kind: &RawShellKind, login: bool) {
    if std::env::var("COSH_SHELL_BOOTSTRAP_PATH").as_deref() == Ok("0") {
        return;
    }

    let shell = match shell_kind {
        RawShellKind::Bash => "bash",
        RawShellKind::Zsh => "zsh",
        _ => return,
    };
    let flags = if login { "-lic" } else { "-ic" };
    let Ok(output) = Command::new(shell)
        .arg(flags)
        .arg("printf '\\n__COSH_PATH_BEGIN__%s__COSH_PATH_END__\\n' \"$PATH\"")
        .env("COSH_SHELL_BOOTSTRAP_PATH", "0")
        .output()
    else {
        return;
    };
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let Some(path) = extract_bootstrap_path(&text) else {
        return;
    };
    let current = std::env::var("PATH").unwrap_or_default();
    let merged = merge_path_lists(&[
        path.as_str(),
        current.as_str(),
        "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
    ]);
    if merged != current {
        std::env::set_var("PATH", merged);
    }
}

pub(crate) fn passthrough_non_interactive(args: &[String]) -> Option<i32> {
    if args.get(1).map(String::as_str) == Some("--") {
        let Some(command) = args.get(2) else {
            eprintln!("cosh-shell: missing command after --");
            return Some(2);
        };
        let status = Command::new(command)
            .args(&args[3..])
            .status()
            .map(|s| s.code().unwrap_or(1))
            .unwrap_or_else(|err| {
                eprintln!("cosh-shell: exec {command} failed: {err}");
                126
            });
        return Some(status);
    }

    if args.iter().any(|a| a == "-c") {
        let shell = detect_passthrough_shell(args);
        let pass_args = passthrough_shell_args(args);
        let status = Command::new(&shell)
            .args(&pass_args)
            .status()
            .map(|s| s.code().unwrap_or(1))
            .unwrap_or_else(|err| {
                eprintln!("cosh-shell: exec {shell} failed: {err}");
                126
            });
        return Some(status);
    }

    if !std::io::stdin().is_terminal() {
        let shell = detect_passthrough_shell(args);
        let pass_args = passthrough_shell_args(args);
        let status = Command::new(&shell)
            .args(&pass_args)
            .stdin(Stdio::inherit())
            .status()
            .map(|s| s.code().unwrap_or(1))
            .unwrap_or_else(|err| {
                eprintln!("cosh-shell: exec {shell} failed: {err}");
                126
            });
        return Some(status);
    }

    None
}

pub(crate) fn passthrough_raw_non_interactive(args: &[String]) -> Option<i32> {
    let passthrough_args = raw_passthrough_args(args)?;
    passthrough_non_interactive(&passthrough_args)
}

fn raw_passthrough_args(args: &[String]) -> Option<Vec<String>> {
    if args.get(1).map(String::as_str) != Some("raw") {
        return None;
    }

    let mut out = vec![args[0].clone()];
    let mut skipped_adapter = false;
    let mut idx = 2;
    while idx < args.len() {
        let arg = &args[idx];
        match arg.as_str() {
            "--" => {
                out.push(arg.clone());
                out.extend(args[idx + 1..].iter().cloned());
                break;
            }
            "-c" => {
                out.extend(args[idx..].iter().cloned());
                break;
            }
            "--shell" => {
                out.push(arg.clone());
                if let Some(value) = args.get(idx + 1) {
                    out.push(value.clone());
                    idx += 2;
                } else {
                    idx += 1;
                }
            }
            "--isolated" | "--login" | "-l" => {
                out.push(arg.clone());
                idx += 1;
            }
            _ if arg.starts_with("--shell=") => {
                out.push(arg.clone());
                idx += 1;
            }
            _ if !arg.starts_with('-') && !skipped_adapter => {
                skipped_adapter = true;
                idx += 1;
            }
            _ => return None,
        }
    }

    let has_dash_c = out.iter().any(|arg| arg == "-c");
    let has_double_dash = out.get(1).map(String::as_str) == Some("--");
    (has_dash_c || has_double_dash).then_some(out)
}

fn detect_passthrough_shell(args: &[String]) -> String {
    for (i, arg) in args.iter().enumerate() {
        if arg == "--shell" {
            if let Some(val) = args.get(i + 1) {
                return val.clone();
            }
        }
        if let Some(val) = arg.strip_prefix("--shell=") {
            return val.to_string();
        }
    }
    std::env::var("COSH_SHELL_DEFAULT_SHELL").unwrap_or_else(|_| "bash".to_string())
}

fn passthrough_shell_args(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut iter = args.iter().skip(1).peekable();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--shell" => {
                let _ = iter.next();
            }
            "--isolated" => {}
            "--login" => out.push("-l".to_string()),
            _ if arg.starts_with("--shell=") => {}
            _ => out.push(arg.clone()),
        }
    }
    out
}

pub(crate) fn print_usage_help() {
    eprintln!(
        "Usage: cosh-shell [OPTIONS]\n\
         \n\
         AI-augmented interactive shell wrapper.\n\
         \n\
         Modes:\n\
          raw [adapter] [--run]   Interactive mode with AI (adapters: fake, claude, co, qwen, cosh-core)\n\
           demo                    Demo with synthetic events\n\
         \n\
         Options:\n\
           -c <command>            Execute command and exit (passthrough to bash/zsh)\n\
           -- <command> [args...]   Execute command directly and exit\n\
           --shell <shell>         Use specified shell (bash, zsh) [default: bash]\n\
           --isolated              Isolated mode: skip user rcfiles\n\
           --login, -l             Treat as login shell\n\
           --version               Print version\n\
           --help                  Print help"
    );
}

fn extract_bootstrap_path(text: &str) -> Option<String> {
    let start = text.rfind("__COSH_PATH_BEGIN__")? + "__COSH_PATH_BEGIN__".len();
    let rest = &text[start..];
    let end = rest.find("__COSH_PATH_END__")?;
    let path = rest[..end].trim();
    if path.is_empty() {
        None
    } else {
        Some(path.to_string())
    }
}

fn merge_path_lists(paths: &[&str]) -> String {
    let mut seen = HashSet::new();
    let mut merged = Vec::new();
    for path in paths {
        for item in path.split(':') {
            if item.is_empty() {
                continue;
            }
            if seen.insert(item.to_string()) {
                merged.push(item.to_string());
            }
        }
    }
    merged.join(":")
}

#[cfg(test)]
mod tests {
    use super::{extract_bootstrap_path, merge_path_lists, raw_passthrough_args};

    #[test]
    fn bootstrap_path_extracts_last_marked_value() {
        let text = "plugin noise\n__COSH_PATH_BEGIN__/a:/b__COSH_PATH_END__\n";
        assert_eq!(extract_bootstrap_path(text), Some("/a:/b".to_string()));
        assert_eq!(extract_bootstrap_path("plugin noise"), None);
    }

    #[test]
    fn bootstrap_path_merge_keeps_existing_and_common_dirs() {
        assert_eq!(
            merge_path_lists(&[
                "/opt/homebrew/bin:/usr/bin:/bin",
                "/usr/local/bin:/bin",
                "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
            ]),
            "/opt/homebrew/bin:/usr/bin:/bin:/usr/local/bin:/usr/sbin:/sbin"
        );
    }

    #[test]
    fn raw_passthrough_args_strips_raw_adapter_for_dash_c() {
        assert_eq!(
            raw_passthrough_args(&[
                "cosh-shell".to_string(),
                "raw".to_string(),
                "cosh-core".to_string(),
                "-c".to_string(),
                "echo ok".to_string()
            ]),
            Some(vec![
                "cosh-shell".to_string(),
                "-c".to_string(),
                "echo ok".to_string()
            ])
        );
    }

    #[test]
    fn raw_passthrough_args_preserves_shell_option() {
        assert_eq!(
            raw_passthrough_args(&[
                "cosh-shell".to_string(),
                "raw".to_string(),
                "--shell".to_string(),
                "bash".to_string(),
                "cosh-core".to_string(),
                "-c".to_string(),
                "echo ok".to_string()
            ]),
            Some(vec![
                "cosh-shell".to_string(),
                "--shell".to_string(),
                "bash".to_string(),
                "-c".to_string(),
                "echo ok".to_string()
            ])
        );
    }
}
