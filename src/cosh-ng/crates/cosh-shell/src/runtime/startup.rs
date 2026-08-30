use std::collections::HashSet;
use std::ffi::OsString;
use std::io::{IsTerminal, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Command, ExitStatus};
use std::time::Duration;

use crate::diagnostics::health::{
    record_startup_health_recommendations, HealthFindingCategory, HealthScanReport, HealthSeverity,
};
use crate::raw_input::{PromptGhostCandidate, PromptGhostRoute};
use crate::recommendation::personal_context::discover_repo_context;
use crate::recommendation::personal_crypto::random_hex;
use crate::recommendation::personal_feedback::{FeedbackEvent, FrozenPromptBinding};
use crate::recommendation::personal_model::{
    CandidateEvidenceSummary, CandidateSource, ContextAffinity, FeedbackAction, ScopeKind,
    DISCLOSURE_VERSION,
};
use crate::recommendation::personal_planner::{
    plan_startup, HealthResolution, PlannerCandidate, PlannerContext,
};
use crate::runtime::cli_args::RawShellKind;
use crate::runtime::invocation::{
    classify_invocation, exec_shell, normalize_raw_invocation, Invocation,
};
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
const STARTUP_AUTH_HINT_WAIT: Duration = Duration::from_millis(150);

mod recommendations;
#[cfg(test)]
use recommendations::{
    append_startup_auth_hint, plan_startup_for_render, record_visible_personal_impressions,
    visible_personal_candidates, write_startup_suggestion_card,
};
pub(crate) use recommendations::{
    render_pending_recommendation_notice, render_startup_banner, render_startup_health_banner,
};

fn restore_startup_prompt<W: Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    crate::slash::prompt::write_shell_prompt(state, output)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartupSuggestionMode {
    Hidden,
    ReadOnly,
    Interactive,
}

fn startup_suggestion_mode(
    isolated: bool,
    term: Option<&str>,
    report: &HealthScanReport,
) -> StartupSuggestionMode {
    if !startup_suggestion_display_supported(isolated, term) {
        StartupSuggestionMode::Hidden
    } else if health_report_supports_interactive_suggestions(report) {
        StartupSuggestionMode::Interactive
    } else {
        StartupSuggestionMode::ReadOnly
    }
}

fn startup_suggestion_display_supported(isolated: bool, term: Option<&str>) -> bool {
    !isolated && !term.is_some_and(|term| term.eq_ignore_ascii_case("dumb"))
}

fn health_report_supports_interactive_suggestions(report: &HealthScanReport) -> bool {
    !report
        .findings
        .iter()
        .any(|finding| finding.category == HealthFindingCategory::CollectionGap)
        && !report.unavailable.iter().any(|item| {
            matches!(
                item.severity,
                HealthSeverity::Unavailable | HealthSeverity::Degraded
            )
        })
}

pub(crate) fn startup_banner_enabled() -> bool {
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
    // Documented `cosh-shell` extension: `-- <command> [args…]` executes the
    // command directly (no shell). The `/usr/bin/cosh` entry never reaches
    // this path; there `--` is handed to bash verbatim.
    if args.get(1).map(String::as_str) == Some("--") {
        let Some(command) = args.get(2) else {
            eprintln!("cosh-shell: missing command after --");
            return Some(2);
        };
        let status = Command::new(command)
            .args(&args[3..])
            .status()
            .map(passthrough_exit_code)
            .unwrap_or_else(|err| {
                let command = crate::evidence::redact_sensitive_text(command).0;
                let err = crate::evidence::redact_sensitive_text(&err.to_string()).0;
                eprintln!("cosh-shell: exec {command} failed: {err}");
                126
            });
        return Some(status);
    }

    let argv0 = OsString::from(args[0].as_str());
    let rest = args[1..].iter().map(OsString::from).collect::<Vec<_>>();
    let stdin_tty = std::io::stdin().is_terminal();
    let stdout_tty = std::io::stdout().is_terminal();
    let stderr_tty = std::io::stderr().is_terminal();
    match classify_invocation(&argv0, &rest, stdin_tty, stdout_tty, stderr_tty) {
        Invocation::ExecShell(plan) => Some(exec_shell(plan)),
        Invocation::Tui(_) => None,
    }
}

fn passthrough_exit_code(status: ExitStatus) -> i32 {
    status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
        .unwrap_or(1)
}

pub(crate) fn passthrough_raw_non_interactive(args: &[String]) -> Option<i32> {
    let rest = args[1..].iter().map(OsString::from).collect::<Vec<_>>();
    let normalized = normalize_raw_invocation(&rest)?;
    // Leading `--`: same documented direct-exec extension as the bare
    // `cosh-shell` surface.
    if normalized.first().and_then(|arg| arg.to_str()) == Some("--") {
        let mut forwarded = vec![args[0].clone()];
        forwarded.extend(
            normalized
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned()),
        );
        return passthrough_non_interactive(&forwarded);
    }
    // `raw` is an explicit TUI request, so the remaining `-c` candidate is
    // classified on argv shape alone (terminals assumed): piped drivers
    // must never be diverted away from an interactive session.
    let argv0 = OsString::from(args[0].as_str());
    match classify_invocation(&argv0, &normalized, true, true, true) {
        Invocation::ExecShell(plan) => Some(exec_shell(plan)),
        Invocation::Tui(_) => None,
    }
}

pub(crate) fn print_usage_help() {
    println!(
        "Usage: cosh-shell [OPTIONS]\n\
         \n\
         AI-augmented interactive shell wrapper.\n\
         \n\
         Modes:\n\
          raw [adapter] [--run]   Interactive mode with AI (adapters: fake, claude, co, qwen, cosh-core)\n\
          diagnostics export      Export a redacted diagnostic bundle\n\
           demo                    Demo with synthetic events\n\
         \n\
         Options:\n\
           -c <command>            Execute command and exit (passthrough to bash/zsh)\n\
           -- <command> [args...]   Execute command directly and exit\n\
           --shell <shell>         Use specified shell (bash, zsh) [default: bash]\n\
           --resume [session-id]   Open the session picker or resume a provider session\n\
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
#[path = "startup_tests.rs"]
mod tests;
