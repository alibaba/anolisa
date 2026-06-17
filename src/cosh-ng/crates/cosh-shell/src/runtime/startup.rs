use std::collections::HashSet;
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use crate::runtime::cli_args::RawShellKind;
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
            MessageId::StartupAdapterLine,
            &[
                ("adapter", adapter.name()),
                ("shell", shell_label),
                ("mode", state.approval_mode.label()),
            ],
        ),
        i18n.format(MessageId::StartupCwdLine, &[("cwd", cwd)]),
        i18n.t(MessageId::StartupCommandsLine).to_string(),
    ];
    if let Some(markdown) = startup_hook.markdown {
        body.push(String::new());
        body.push(startup_hook.summary);
        for line in renderer.markdown_text_lines(&markdown) {
            body.push(line);
        }
    }
    renderer.write_banner(output, i18n.t(MessageId::StartupTitle), body, None)?;
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
          raw [adapter] [--run]   Interactive mode with AI (adapters: fake, claude, co, qwen, cosh-tui)\n\
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
    use super::{extract_bootstrap_path, merge_path_lists};

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
}
