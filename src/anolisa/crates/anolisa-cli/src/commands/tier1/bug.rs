//! `anolisa bug` — generate a local bug report with diagnostic context.
//!
//! The command is read-only. It gathers environment facts, component state,
//! and recent warn/error central-log records, then renders copyable Markdown
//! for the repository bug report issue form.

use clap::Parser;
use serde::Serialize;

use anolisa_core::{CentralLog, LogFilter, LogRecord, ObjectKind, Severity};

use crate::commands::common;
use crate::context::CliContext;
use crate::response::{CliError, render_json};

const COMMAND: &str = "bug";
const ISSUE_URL: &str = "https://github.com/alibaba/anolisa/issues/new?template=bug_report.yml";
const DEFAULT_LIMIT: usize = 20;
const MAX_LIMIT: usize = 100;

#[derive(Parser)]
pub struct BugArgs {
    /// Limit the report to one component.
    #[arg(long, value_name = "NAME")]
    pub component: Option<String>,
    /// Maximum number of recent warn/error log records to include.
    #[arg(long, value_name = "N")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct EnvironmentSummary {
    anolisa: String,
    install_mode: String,
    os: String,
    arch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    libc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    kernel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pkg_base: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    btf: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cap_bpf: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    container: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct ComponentSummary {
    name: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    installed_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct RecentLogSummary {
    started_at: String,
    severity: String,
    source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    component: Option<String>,
    command: String,
    message: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    objects: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct BugReportPayload {
    issue_url: String,
    markdown: String,
    environment: EnvironmentSummary,
    installed_components: Vec<ComponentSummary>,
    recent_logs: Vec<RecentLogSummary>,
}

pub fn handle(args: BugArgs, ctx: &CliContext) -> Result<(), CliError> {
    let limit = validate_limit(args.limit.unwrap_or(DEFAULT_LIMIT))?;
    let payload = build_payload(args.component.as_deref(), limit, ctx)?;

    if ctx.json {
        return render_json(COMMAND, payload);
    }

    if ctx.quiet {
        println!("{}", payload.markdown);
    } else {
        println!("Bug report markdown generated below.");
        println!("Paste it into:");
        println!("{}", payload.issue_url);
        println!();
        println!("---");
        println!("{}", payload.markdown);
    }
    Ok(())
}

fn build_payload(
    component: Option<&str>,
    limit: usize,
    ctx: &CliContext,
) -> Result<BugReportPayload, CliError> {
    let environment = collect_environment(ctx);
    let components = collect_components(component, ctx)?;
    let recent_logs = collect_recent_logs(component, limit, ctx)?;
    let markdown = render_markdown(&environment, &components, &recent_logs);

    Ok(BugReportPayload {
        issue_url: ISSUE_URL.to_string(),
        markdown,
        environment,
        installed_components: components,
        recent_logs,
    })
}

fn validate_limit(limit: usize) -> Result<usize, CliError> {
    if limit > MAX_LIMIT {
        return Err(CliError::InvalidArgument {
            command: COMMAND.to_string(),
            reason: format!("--limit must be <= {MAX_LIMIT}, got {limit}"),
        });
    }
    Ok(limit)
}

fn collect_environment(ctx: &CliContext) -> EnvironmentSummary {
    let facts = anolisa_env::EnvService::detect();
    EnvironmentSummary {
        anolisa: env!("CARGO_PKG_VERSION").to_string(),
        install_mode: ctx.install_mode.as_str().to_string(),
        os: facts.os,
        arch: facts.arch,
        libc: facts.libc,
        kernel: facts.kernel,
        pkg_base: facts.pkg_base,
        btf: facts.btf,
        cap_bpf: facts.cap_bpf,
        container: facts.container,
    }
}

fn collect_components(
    component: Option<&str>,
    ctx: &CliContext,
) -> Result<Vec<ComponentSummary>, CliError> {
    let state = common::load_installed_state(ctx, COMMAND)?;
    let all: Vec<ComponentSummary> = state
        .objects
        .iter()
        .filter(|o| o.kind == ObjectKind::Component)
        .map(|o| ComponentSummary {
            name: o.name.clone(),
            status: common::object_status_str(o.status).to_string(),
            installed_version: Some(o.version.clone()),
        })
        .collect();

    match component {
        Some(name) => {
            let matches: Vec<ComponentSummary> =
                all.into_iter().filter(|s| s.name == name).collect();
            if matches.is_empty() {
                return Err(CliError::InvalidArgument {
                    command: COMMAND.to_string(),
                    reason: format!("unknown component '{name}'"),
                });
            }
            Ok(matches)
        }
        None => Ok(all
            .into_iter()
            .filter(|s| common::status_is_enabled(&s.status))
            .collect()),
    }
}

fn collect_recent_logs(
    component: Option<&str>,
    limit: usize,
    ctx: &CliContext,
) -> Result<Vec<RecentLogSummary>, CliError> {
    let layout = common::resolve_layout(ctx);
    let log = CentralLog::open(layout.central_log.clone());
    let records = log
        .query(&LogFilter {
            severity_at_least: Some(Severity::Warn),
            object: component.map(|name| name.to_string()),
            limit: None,
            ..Default::default()
        })
        .map_err(|err| CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!(
                "failed to query central log at {}: {err}",
                layout.central_log.display()
            ),
        })?;

    Ok(take_recent_by_started_at(records, limit)
        .into_iter()
        .map(summarize_log)
        .collect())
}

fn take_recent_by_started_at(mut records: Vec<LogRecord>, limit: usize) -> Vec<LogRecord> {
    records.sort_by(|a, b| a.started_at.cmp(&b.started_at));
    let skip = records.len().saturating_sub(limit);
    records.into_iter().skip(skip).collect()
}

fn summarize_log(record: LogRecord) -> RecentLogSummary {
    RecentLogSummary {
        started_at: redact_sensitive(&record.started_at),
        severity: severity_str(record.severity).to_string(),
        source: redact_sensitive(&record.source),
        component: record.component.map(|v| redact_sensitive(&v)),
        command: redact_sensitive(&record.command),
        message: redact_sensitive(&record.message),
        objects: record
            .objects
            .into_iter()
            .map(|v| redact_sensitive(&v))
            .collect(),
        warnings: record
            .warnings
            .into_iter()
            .map(|v| redact_sensitive(&v))
            .collect(),
    }
}

fn render_markdown(
    env: &EnvironmentSummary,
    components: &[ComponentSummary],
    logs: &[RecentLogSummary],
) -> String {
    let mut out = String::new();
    out.push_str("## Description\n\n");
    out.push_str("<!-- Please fill in the issue details before submitting. -->\n\n");
    out.push_str("- Problem:\n");
    out.push_str("- Steps to reproduce:\n");
    out.push_str("- Expected behavior:\n");
    out.push_str("- Time observed:\n\n");

    out.push_str("## Environment\n\n");
    push_kv(&mut out, "anolisa", &env.anolisa);
    push_kv(&mut out, "install_mode", &env.install_mode);
    push_kv(&mut out, "os", &env.os);
    push_kv(&mut out, "arch", &env.arch);
    push_opt_kv(&mut out, "libc", env.libc.as_deref());
    push_opt_kv(&mut out, "kernel", env.kernel.as_deref());
    push_opt_kv(&mut out, "pkg_base", env.pkg_base.as_deref());
    push_opt_kv(&mut out, "btf", env.btf.map(bool_label));
    push_opt_kv(&mut out, "cap_bpf", env.cap_bpf.map(bool_label));
    push_opt_kv(&mut out, "container", env.container.as_deref());

    out.push_str("\n## Installed Components\n\n");
    if components.is_empty() {
        out.push_str("- none\n");
    } else {
        for comp in components {
            match comp.installed_version.as_deref() {
                Some(version) => out.push_str(&format!(
                    "- {}: {}, version {}\n",
                    comp.name, comp.status, version
                )),
                None => out.push_str(&format!("- {}: {}\n", comp.name, comp.status)),
            }
        }
    }

    out.push_str("\n## Recent Logs\n\n");
    if logs.is_empty() {
        out.push_str("- No warn/error central log records found.\n");
    } else {
        for log in logs {
            out.push_str(&format!(
                "- {} {} {}: {}\n",
                log.started_at, log.severity, log.source, log.message
            ));
            if !log.objects.is_empty() {
                out.push_str(&format!("  - objects: {}\n", log.objects.join(", ")));
            }
            if !log.warnings.is_empty() {
                out.push_str(&format!("  - warnings: {}\n", log.warnings.join("; ")));
            }
        }
    }
    out
}

fn push_kv(out: &mut String, key: &str, value: &str) {
    out.push_str(&format!("- {key}: {value}\n"));
}

fn push_opt_kv(out: &mut String, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        push_kv(out, key, value);
    }
}

fn bool_label(v: bool) -> &'static str {
    if v { "true" } else { "false" }
}

fn severity_str(sev: Severity) -> &'static str {
    match sev {
        Severity::Debug => "debug",
        Severity::Info => "info",
        Severity::Warn => "warn",
        Severity::Error => "error",
    }
}

fn redact_sensitive(input: &str) -> String {
    // This is intentionally keyword-based and biased toward over-redaction:
    // component logs are free-form, so safety matters more than exact parsing.
    const KEYS: &[&str] = &[
        "token",
        "secret",
        "password",
        "passwd",
        "credential",
        "api_key",
        "access_key",
        "private_key",
    ];

    let mut out = input.to_string();
    for key in KEYS {
        out = redact_key_values(&out, key);
    }
    out
}

fn redact_key_values(input: &str, key: &str) -> String {
    let lower = input.to_ascii_lowercase();
    let mut out = String::with_capacity(input.len());
    let mut pos = 0;

    while let Some(relative) = lower[pos..].find(key) {
        let key_start = pos + relative;
        let key_end = key_start + key.len();
        let mut cursor = key_end;
        while cursor < input.len() && is_space_or_quote(input.as_bytes()[cursor]) {
            cursor += 1;
        }
        if cursor >= input.len() || !matches!(input.as_bytes()[cursor], b'=' | b':') {
            out.push_str(&input[pos..key_end]);
            pos = key_end;
            continue;
        }

        cursor += 1;
        while cursor < input.len() && is_space_or_quote(input.as_bytes()[cursor]) {
            cursor += 1;
        }

        out.push_str(&input[pos..cursor]);
        out.push_str("<redacted>");

        let value_end = input[cursor..]
            .find(is_value_boundary)
            .map(|idx| cursor + idx)
            .unwrap_or(input.len());
        pos = value_end;
    }

    out.push_str(&input[pos..]);
    out
}

fn is_space_or_quote(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\'' | b'"')
}

fn is_value_boundary(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '\n' | '\r' | ',' | ';' | '&' | '"' | '\'')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_summary(name: &str, status: &str, version: Option<&str>) -> ComponentSummary {
        ComponentSummary {
            name: name.to_string(),
            status: status.to_string(),
            installed_version: version.map(|v| v.to_string()),
        }
    }

    fn filter_summaries(
        all: &[ComponentSummary],
        component: Option<&str>,
    ) -> Result<Vec<ComponentSummary>, CliError> {
        match component {
            Some(name) => {
                let matches: Vec<ComponentSummary> =
                    all.iter().filter(|s| s.name == name).cloned().collect();
                if matches.is_empty() {
                    return Err(CliError::InvalidArgument {
                        command: COMMAND.to_string(),
                        reason: format!("unknown component '{name}'"),
                    });
                }
                Ok(matches)
            }
            None => Ok(all
                .iter()
                .filter(|s| common::status_is_enabled(&s.status))
                .cloned()
                .collect()),
        }
    }

    fn log_record(started_at: &str, message: &str) -> LogRecord {
        LogRecord {
            kind: anolisa_core::LogKind::Operation,
            operation_id: Some("op-1".to_string()),
            command: "enable tokenless".to_string(),
            source: "anolisa-cli".to_string(),
            component: None,
            severity: Severity::Warn,
            message: message.to_string(),
            actor: "cli".to_string(),
            install_mode: Some("user".to_string()),
            started_at: started_at.to_string(),
            finished_at: None,
            status: None,
            objects: vec!["tokenless".to_string()],
            backup_ids: Vec::new(),
            warnings: Vec::new(),
            details: serde_json::Value::Null,
        }
    }

    #[test]
    fn default_component_report_includes_enabled_rows_only() {
        let all = vec![
            make_summary("agent-observability", "installed", Some("0.1.0")),
            make_summary("sandbox", "disabled", Some("0.1.0")),
            make_summary("tokenless", "not_installed", None),
        ];

        let caps = filter_summaries(&all, None).expect("summaries");

        assert_eq!(caps.len(), 1);
        assert_eq!(caps[0].name, "agent-observability");
    }

    #[test]
    fn component_filter_keeps_requested_row_even_when_disabled() {
        let all = vec![make_summary("sandbox", "disabled", Some("0.1.0"))];

        let caps = filter_summaries(&all, Some("sandbox")).expect("summaries");

        assert_eq!(caps.len(), 1);
        assert_eq!(caps[0].status, "disabled");
    }

    #[test]
    fn component_filter_rejects_unknown_name() {
        let all = vec![make_summary("sandbox", "installed", Some("0.1.0"))];

        let err = filter_summaries(&all, Some("missing")).expect_err("unknown component");

        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(err.reason().contains("unknown component"));
    }

    #[test]
    fn take_recent_preserves_last_records_in_order() {
        let records = vec![
            log_record("2026-06-01T10:00:00Z", "one"),
            log_record("2026-06-01T10:00:01Z", "two"),
            log_record("2026-06-01T10:00:02Z", "three"),
        ];

        let recent = take_recent_by_started_at(records, 2);

        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].message, "two");
        assert_eq!(recent[1].message, "three");
    }

    #[test]
    fn take_recent_sorts_by_started_at_before_limiting() {
        let records = vec![
            log_record("2026-06-01T10:00:02Z", "three"),
            log_record("2026-06-01T10:00:00Z", "one"),
            log_record("2026-06-01T10:00:01Z", "two"),
        ];

        let recent = take_recent_by_started_at(records, 2);

        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].message, "two");
        assert_eq!(recent[1].message, "three");
    }

    #[test]
    fn missing_central_log_yields_empty_recent_logs() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = CliContext {
            install_mode: crate::context::InstallMode::System,
            prefix: Some(tmp.path().to_path_buf()),
            json: false,
            dry_run: false,
            verbose: false,
            quiet: false,
            no_color: false,
        };

        let logs = collect_recent_logs(None, DEFAULT_LIMIT, &ctx).expect("missing log is ok");

        assert!(logs.is_empty());
        let env = EnvironmentSummary {
            anolisa: "0.1.0".to_string(),
            install_mode: "system".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            libc: None,
            kernel: None,
            pkg_base: None,
            btf: None,
            cap_bpf: None,
            container: None,
        };
        let markdown = render_markdown(&env, &[], &logs);
        assert!(markdown.contains("No warn/error central log records found."));
    }

    #[test]
    fn redacts_sensitive_key_values() {
        let input = "token=abc password: hunter2 url=https://x?a=1&access_key=ak";

        let redacted = redact_sensitive(input);

        assert!(!redacted.contains("abc"));
        assert!(!redacted.contains("hunter2"));
        assert!(!redacted.contains("ak"));
        assert!(redacted.contains("token=<redacted>"));
        assert!(redacted.contains("password: <redacted>"));
        assert!(redacted.contains("access_key=<redacted>"));
    }

    #[test]
    fn redaction_preserves_json_style_quotes() {
        let input = r#"{"token":"abc","ok":true}"#;

        let redacted = redact_sensitive(input);

        assert_eq!(redacted, r#"{"token":"<redacted>","ok":true}"#);
    }

    #[test]
    fn markdown_uses_redacted_log_messages() {
        let env = EnvironmentSummary {
            anolisa: "0.1.0".to_string(),
            install_mode: "user".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            libc: None,
            kernel: None,
            pkg_base: None,
            btf: None,
            cap_bpf: None,
            container: None,
        };
        let logs = vec![summarize_log(log_record(
            "2026-06-01T10:00:00Z",
            "failed with token=super-secret",
        ))];

        let markdown = render_markdown(&env, &[], &logs);

        assert!(markdown.contains("token=<redacted>"));
        assert!(!markdown.contains("super-secret"));
    }

    #[test]
    fn validate_limit_rejects_values_above_max() {
        let err = validate_limit(MAX_LIMIT + 1).expect_err("limit should fail");

        assert_eq!(err.code(), "INVALID_ARGUMENT");
    }
}
