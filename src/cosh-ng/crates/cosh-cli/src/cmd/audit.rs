//! `cosh audit` command surface.
//!
//! Replaces the original stub (which always returned `allowed: true` with a
//! `meta.warning` admitting it was not connected to a policy engine) with a
//! real PEP→PDP→log dispatcher. See `docs/audit-design.md` §5.

use std::path::PathBuf;
use std::time::Instant;

use clap::{Args, Subcommand};
use serde::Serialize;
use serde_json::json;

use cosh_platform::audit::{self, parse_action_string, LoadedPolicy, ParseError};
use cosh_platform::detect::Distro;
use cosh_types::audit::{
    Action, ActionSubsystem, Decision, LogEntry, LogSource, Outcome, Policy,
};
use cosh_types::error::{CoshError, ErrorCode};
use cosh_types::output::ResponseMeta;

use crate::{build_meta, build_meta_with_warning, print_failure, print_success};

#[derive(Subcommand)]
pub enum AuditCommands {
    /// Check whether an action is permitted under the active policy.
    Check(CheckArgs),
    /// View audit log entries for the current session (or filtered).
    Log(LogArgs),
    /// Inspect or validate audit policies.
    Policy {
        #[command(subcommand)]
        action: PolicyCommands,
    },
}

#[derive(Args, Debug, Clone)]
pub struct CheckArgs {
    /// Subsystem identifier (pkg / svc / checkpoint / shell / cosh).
    /// Required if no --action-string is given.
    #[arg(long)]
    subsystem: Option<String>,
    /// Operation name (install / start / exec / ...).
    /// Required when --subsystem is given.
    #[arg(long)]
    operation: Option<String>,
    /// Action target (package name, service name, command, ...).
    #[arg(long)]
    target: Option<String>,
    /// Argument key. Repeat for multiple args; pair with --arg-value.
    #[arg(long = "arg-key")]
    arg_key: Vec<String>,
    /// Argument value. Position-paired with --arg-key.
    #[arg(long = "arg-value")]
    arg_value: Vec<String>,
    /// Raw action string (parsed into a structured Action). Backwards-
    /// compatible aliases: --action.
    #[arg(long = "action-string", alias = "action")]
    action_string: Option<String>,
}

#[derive(Args, Debug, Clone)]
pub struct LogArgs {
    /// Filter by session ID.
    #[arg(long)]
    session: Option<String>,
    /// Filter by outcome (allow / deny / requireapproval — case-insensitive).
    #[arg(long)]
    outcome: Option<String>,
    /// Filter to entries newer than `now - <duration>`. Accepts e.g.
    /// "30s", "5m", "2h", "1d".
    #[arg(long)]
    since: Option<String>,
    /// Maximum number of entries to return (most recent first when set).
    #[arg(long)]
    limit: Option<usize>,
}

#[derive(Subcommand)]
pub enum PolicyCommands {
    /// Show the active policy.
    Show,
    /// List built-in presets (permissive / balanced / strict).
    List,
    /// Validate a policy TOML file without writing anything to disk.
    Validate {
        /// Path to a TOML policy file.
        path: PathBuf,
    },
    /// Explain how `<action>` would be evaluated under the active policy.
    Explain {
        /// Action string (parsed via the same rules as `audit check
        /// --action-string`).
        action: String,
    },
}

pub fn run(action: AuditCommands, distro: &Distro, start: Instant) -> i32 {
    match action {
        AuditCommands::Check(args) => run_check(args, distro, start),
        AuditCommands::Log(args) => run_log(args, distro, start),
        AuditCommands::Policy { action: pc } => run_policy(pc, distro, start),
    }
}

// ===========================================================================
// `cosh audit check`
// ===========================================================================

#[derive(Debug, Clone, Serialize)]
struct PolicyListEntry {
    name: String,
    default: Outcome,
    rules: usize,
}

#[derive(Debug, Clone, Serialize)]
struct PolicyShowResult {
    source: String,
    policy_version: String,
    policy: Policy,
}

#[derive(Debug, Clone, Serialize)]
struct PolicyValidateResult {
    valid: bool,
    rules: usize,
    default: Outcome,
}

#[derive(Debug, Clone, Serialize)]
struct PolicyExplainResult {
    action: Action,
    decision: Decision,
}

#[derive(Debug, Clone, Serialize)]
struct LogOutput {
    entries: Vec<LogEntry>,
    total: usize,
}

enum BuiltAction {
    Structured(Action),
    ParseDeny { reason: String, raw: String },
    Malformed(String),
}

fn build_action(args: &CheckArgs) -> BuiltAction {
    // Highest priority: raw string input. (Wins over per-field flags so
    // that `--action-string "..."` is unambiguously the input mode.)
    if let Some(s) = args.action_string.as_deref() {
        return match parse_action_string(s) {
            Ok(a) => BuiltAction::Structured(a),
            Err(e) => BuiltAction::ParseDeny {
                reason: format!("{}", e),
                raw: s.to_string(),
            },
        };
    }

    // Otherwise structural input: --subsystem + --operation [+ --target].
    let subsystem = match &args.subsystem {
        Some(s) if !s.is_empty() => s.clone(),
        _ => {
            return BuiltAction::Malformed(
                "missing required argument: --subsystem or --action-string".to_string(),
            );
        }
    };
    let operation = match &args.operation {
        Some(o) if !o.is_empty() => o.clone(),
        _ => {
            return BuiltAction::Malformed(
                "missing required argument: --operation (required with --subsystem)".to_string(),
            );
        }
    };
    if args.arg_key.len() != args.arg_value.len() {
        return BuiltAction::Malformed(format!(
            "--arg-key and --arg-value must be paired ({} keys vs {} values)",
            args.arg_key.len(),
            args.arg_value.len()
        ));
    }
    let arg_pairs: Vec<(String, String)> = args
        .arg_key
        .iter()
        .zip(args.arg_value.iter())
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    BuiltAction::Structured(Action {
        subsystem: ActionSubsystem::from_token(&subsystem),
        operation,
        target: args.target.clone(),
        args: arg_pairs,
        raw: None,
    })
}

fn run_check(args: CheckArgs, distro: &Distro, start: Instant) -> i32 {
    match build_action(&args) {
        BuiltAction::Structured(action) => {
            let (loaded, load_warning) = LoadedPolicy::load();
            run_check_evaluate(action, &loaded, load_warning, distro, start)
        }
        BuiltAction::ParseDeny { reason, raw } => {
            // Semantic parse failure → produce Deny with reason "parse failed".
            // The decision is still recorded (audit-design.md §4 last bullet).
            let (loaded, load_warning) = LoadedPolicy::load();
            let decision = Decision {
                outcome: Outcome::Deny,
                reason: format!("parse failed: {}", reason),
                matched_rule: None,
                policy_version: loaded.policy_version.clone(),
            };
            let synthetic = Action {
                subsystem: ActionSubsystem::Other("unparsed".to_string()),
                operation: "<unparsed>".to_string(),
                target: None,
                args: vec![],
                raw: Some(raw),
            };
            match audit::record_decision(synthetic, &decision, LogSource::Cli) {
                Ok(()) => print_success(
                    decision,
                    meta_with_optional_warning(distro, start, load_warning.as_deref()),
                ),
                Err(mut e) => {
                    if let Ok(v) = serde_json::to_value(&decision) {
                        e = e.with_details(json!({ "decision": v }));
                    }
                    print_failure(e, build_meta("audit", distro, start, false))
                }
            }
        }
        BuiltAction::Malformed(msg) => print_failure(
            CoshError::new(ErrorCode::AuditActionMalformed, msg, "audit")
                .with_hint("see `cosh audit check --help` for valid argument combinations"),
            build_meta("audit", distro, start, false),
        ),
    }
}

fn run_check_evaluate(
    action: Action,
    loaded: &LoadedPolicy,
    load_warning: Option<String>,
    distro: &Distro,
    start: Instant,
) -> i32 {
    match audit::check(action, LogSource::Cli, loaded) {
        Ok(decision) => print_success(
            decision,
            meta_with_optional_warning(distro, start, load_warning.as_deref()),
        ),
        Err(e) => print_failure(e, build_meta("audit", distro, start, false)),
    }
}

// ===========================================================================
// `cosh audit log`
// ===========================================================================

fn run_log(args: LogArgs, distro: &Distro, start: Instant) -> i32 {
    let path = audit::log::audit_log_path();
    let entries = match audit::log::read_entries(&path) {
        Ok(e) => e,
        Err(e) => return print_failure(e, build_meta("audit", distro, start, false)),
    };

    let filtered = match filter_log_entries(entries, &args) {
        Ok(es) => es,
        Err(e) => return print_failure(e, build_meta("audit", distro, start, false)),
    };
    let total = filtered.len();
    print_success(
        LogOutput { entries: filtered, total },
        build_meta("audit", distro, start, false),
    )
}

fn filter_log_entries(
    mut entries: Vec<LogEntry>,
    args: &LogArgs,
) -> Result<Vec<LogEntry>, CoshError> {
    if let Some(s) = &args.session {
        entries.retain(|e| &e.session_id == s);
    }
    if let Some(o) = &args.outcome {
        let want = parse_outcome_filter(o)?;
        entries.retain(|e| e.decision.outcome == want);
    }
    if let Some(d) = &args.since {
        let dur = parse_duration_filter(d)?;
        let cutoff = chrono::Utc::now() - dur;
        entries.retain(|e| e.timestamp >= cutoff);
    }
    if let Some(limit) = args.limit {
        if entries.len() > limit {
            // Keep most recent.
            let drop = entries.len() - limit;
            entries.drain(..drop);
        }
    }
    Ok(entries)
}

fn parse_outcome_filter(s: &str) -> Result<Outcome, CoshError> {
    match s.to_ascii_lowercase().as_str() {
        "allow" => Ok(Outcome::Allow),
        "deny" => Ok(Outcome::Deny),
        "requireapproval" | "approval" | "require-approval" => Ok(Outcome::RequireApproval),
        other => Err(CoshError::new(
            ErrorCode::InvalidInput,
            format!(
                "unknown outcome filter '{}': expected allow / deny / requireapproval",
                other
            ),
            "audit",
        )),
    }
}

fn parse_duration_filter(s: &str) -> Result<chrono::Duration, CoshError> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err(CoshError::new(
            ErrorCode::InvalidInput,
            "empty --since value",
            "audit",
        ));
    }
    let (num_str, unit) = match trimmed.chars().last() {
        Some(c) if c.is_ascii_alphabetic() => (&trimmed[..trimmed.len() - c.len_utf8()], c),
        _ => {
            return Err(CoshError::new(
                ErrorCode::InvalidInput,
                format!("invalid --since '{}': expected e.g. 30s, 5m, 2h, 1d", s),
                "audit",
            ));
        }
    };
    let n: i64 = num_str.parse().map_err(|_| {
        CoshError::new(
            ErrorCode::InvalidInput,
            format!("invalid --since '{}': numeric component is not a non-negative integer", s),
            "audit",
        )
    })?;
    if n < 0 {
        return Err(CoshError::new(
            ErrorCode::InvalidInput,
            format!("invalid --since '{}': must be non-negative", s),
            "audit",
        ));
    }
    let dur = match unit {
        's' => chrono::Duration::seconds(n),
        'm' => chrono::Duration::minutes(n),
        'h' => chrono::Duration::hours(n),
        'd' => chrono::Duration::days(n),
        other => {
            return Err(CoshError::new(
                ErrorCode::InvalidInput,
                format!("invalid --since unit '{}': expected s/m/h/d", other),
                "audit",
            ));
        }
    };
    Ok(dur)
}

// ===========================================================================
// `cosh audit policy ...`
// ===========================================================================

fn run_policy(action: PolicyCommands, distro: &Distro, start: Instant) -> i32 {
    match action {
        PolicyCommands::Show => run_policy_show(distro, start),
        PolicyCommands::List => run_policy_list(distro, start),
        PolicyCommands::Validate { path } => run_policy_validate(path, distro, start),
        PolicyCommands::Explain { action } => run_policy_explain(action, distro, start),
    }
}

fn run_policy_show(distro: &Distro, start: Instant) -> i32 {
    let (loaded, load_warning) = LoadedPolicy::load();
    let result = PolicyShowResult {
        source: loaded.source.label(),
        policy_version: loaded.policy_version.clone(),
        policy: loaded.policy.clone(),
    };
    print_success(
        result,
        meta_with_optional_warning(distro, start, load_warning.as_deref()),
    )
}

fn run_policy_list(distro: &Distro, start: Instant) -> i32 {
    let presets = audit::builtin::ALL.map(|p| {
        let loaded = audit::builtin::load(p);
        PolicyListEntry {
            name: p.name().to_string(),
            default: loaded.policy.default,
            rules: loaded.policy.rules.len(),
        }
    });
    print_success(
        json!({ "presets": presets, "total": presets.len() }),
        build_meta("audit", distro, start, false),
    )
}

fn run_policy_validate(path: PathBuf, distro: &Distro, start: Instant) -> i32 {
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            return print_failure(
                CoshError::new(
                    ErrorCode::AuditPolicyError,
                    format!("failed to read {}: {}", path.display(), e),
                    "audit",
                ),
                build_meta("audit", distro, start, false),
            );
        }
    };
    match audit::policy::validate_toml_bytes(&bytes) {
        Ok(p) => print_success(
            PolicyValidateResult {
                valid: true,
                rules: p.rules.len(),
                default: p.default,
            },
            build_meta("audit", distro, start, false),
        ),
        Err(msg) => print_failure(
            CoshError::new(
                ErrorCode::AuditPolicyError,
                format!("invalid policy at {}: {}", path.display(), msg),
                "audit",
            )
            .with_hint("see docs/audit-design.md §6 for valid syntax"),
            build_meta("audit", distro, start, false),
        ),
    }
}

fn run_policy_explain(action_str: String, distro: &Distro, start: Instant) -> i32 {
    let action = match parse_action_string(&action_str) {
        Ok(a) => a,
        Err(e) => match e {
            ParseError::Empty => {
                return print_failure(
                    CoshError::new(
                        ErrorCode::AuditActionMalformed,
                        "empty action string",
                        "audit",
                    ),
                    build_meta("audit", distro, start, false),
                );
            }
            other => {
                // Show the deny reason explicitly (this is what would be
                // recorded for a real `check` call).
                let (loaded, load_warning) = LoadedPolicy::load();
                let decision = Decision {
                    outcome: Outcome::Deny,
                    reason: format!("parse failed: {}", other),
                    matched_rule: None,
                    policy_version: loaded.policy_version.clone(),
                };
                let synth = Action {
                    subsystem: ActionSubsystem::Other("unparsed".to_string()),
                    operation: "<unparsed>".to_string(),
                    target: None,
                    args: vec![],
                    raw: Some(action_str),
                };
                return print_success(
                    PolicyExplainResult {
                        action: synth,
                        decision,
                    },
                    meta_with_optional_warning(distro, start, load_warning.as_deref()),
                );
            }
        },
    };
    let (loaded, load_warning) = LoadedPolicy::load();
    let decision = audit::evaluate(&action, &loaded);
    print_success(
        PolicyExplainResult { action, decision },
        meta_with_optional_warning(distro, start, load_warning.as_deref()),
    )
}

// ===========================================================================
// Shared helpers
// ===========================================================================

fn meta_with_optional_warning(
    distro: &Distro,
    start: Instant,
    warning: Option<&str>,
) -> ResponseMeta {
    match warning {
        Some(w) => build_meta_with_warning("audit", distro, start, false, w),
        None => build_meta("audit", distro, start, false),
    }
}

