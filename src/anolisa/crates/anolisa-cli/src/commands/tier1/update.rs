//! `anolisa update` — unified update surface.
//!
//! Three forms:
//! - `update <component>` - update one ANOLISA-managed component.
//! - `update self` - update the `anolisa` CLI binary only.
//! - `update all` - update every ANOLISA-managed runtime, osbase, and
//!   adapter object.
//!
//! The component is a positional argument; `self` / `all` are subcommands
//! (kept mutually exclusive with the positional via
//! `args_conflicts_with_subcommands`). A component literally named `self` or
//! `all` would be shadowed by the subcommand — those are reserved.
//!
//! Explicit invariant: `update all` does **not** include CLI self-update. The
//! binary swap never shares a transaction with component updates.
//!
//! `update <component>` implements the **RPM** update path (issue #959): for
//! `rpm-observed` and `rpm-managed` components it runs the flow
//! `rpmdb query -> dnf repo query -> dnf update -> refresh ANOLISA state`,
//! gated on root for the real run. It never switches backend —
//! ownership/`install_backend` are preserved. Raw components stay on the
//! not-yet-implemented planner boundary, and `update all` remains
//! `NOT_IMPLEMENTED` pending the raw distribution resolver.

use std::io::Read;
use std::path::Path;
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use clap::{Parser, Subcommand};
use serde::Serialize;

use anolisa_core::central_log::{CentralLog, LogKind, LogRecord, LogStatus, Severity};
use anolisa_core::lock::InstallLock;
use anolisa_core::self_update::{self, ProgressFn, SelfUpdateOutcome};
use anolisa_core::state::{ObjectKind, OperationRecord, Ownership};
use anolisa_platform::pkg_query::{PackageInfo, PackageQuery, PackageQueryError};
use anolisa_platform::pkg_transaction::{PackageTransaction, PackageTransactionError};
use anolisa_platform::privilege;
use anolisa_platform::rpm_query::RpmPackageQuery;
use anolisa_platform::rpm_transaction::RpmTransaction;

use crate::color::Palette;
use crate::commands::common;
use crate::context::CliContext;
use crate::repo_config::RepoConfig;
use crate::response::{self, CliError};

/// Command label for JSON envelopes and error routing.
const COMMAND: &str = "update";

const CLI_CHANGELOG_URL: &str = "https://agentic-os.sh/#anolisa-cli-changelog";

/// TEMPORARY bootstrap: published copy of `templates/repo.toml`.
///
/// Until install/register provisions the user-editable repo config,
/// `anolisa update` downloads this copy when `<etc_dir>/repo.toml` is
/// absent, so a host that has only the CLI binary still ends up with the
/// production backend configuration. Remove once repo.toml provisioning
/// moves into the install/register flow.
const DEFAULT_REPO_CONFIG_URL: &str =
    "https://anolisa.oss-cn-hangzhou.aliyuncs.com/anolisa-releases/anolisa/v1/repo.toml";

/// Hard cap on the downloaded config size; repo.toml is a few KiB, so
/// anything larger is a misconfigured URL, not a config.
const MAX_REPO_CONFIG_BYTES: u64 = 256 * 1024;

/// Arguments for the unified update command surface.
///
/// `anolisa update <component>` updates a single component directly; the
/// `self` and `all` subcommands cover the CLI binary and the (future) batch
/// update. `args_conflicts_with_subcommands` keeps the positional and the
/// subcommands mutually exclusive so `update foo self` is a parse error.
#[derive(Debug, Parser)]
#[command(args_conflicts_with_subcommands = true)]
pub struct UpdateArgs {
    /// Component to update (omit when using a `self` / `all` subcommand)
    #[arg(value_name = "COMPONENT")]
    pub component: Option<String>,
    /// Update the CLI binary (`self`) or every component (`all`) instead of a
    /// single component.
    #[command(subcommand)]
    pub command: Option<UpdateCommands>,
}

/// Update operations that intentionally keep CLI self-update and batch update
/// separate from a single-component update.
#[derive(Debug, Subcommand)]
pub enum UpdateCommands {
    /// Update the anolisa CLI binary only
    #[command(name = "self")]
    SelfBin,
    /// Update every ANOLISA-managed runtime, osbase, and adapter object.
    ///
    /// Does NOT include the CLI binary itself — use `anolisa update self`
    /// for that.
    All,
}

/// Dispatches the selected `anolisa update` form.
///
/// # Errors
///
/// Returns [`CliError`] when the selected update operation fails, no target is
/// given, or the operation is not implemented yet.
pub fn handle(args: UpdateArgs, ctx: &CliContext) -> Result<(), CliError> {
    // `args_conflicts_with_subcommands` guarantees `command` and `component`
    // are never both set, so a present subcommand always wins.
    //
    // Bootstrap the repo config only inside the branches that actually run an
    // update: with `component` now optional, a bare `anolisa update` (or a
    // not-yet-implemented `update all`) must fail validation without first
    // reaching out to the network or writing config.
    match (args.command, args.component) {
        (Some(UpdateCommands::SelfBin), _) => {
            bootstrap_repo_config(ctx);
            handle_self_update(ctx)
        }
        (Some(UpdateCommands::All), _) => Err(CliError::not_implemented_with_hint(
            "update all",
            "update planner / distribution resolver not implemented yet",
        )),
        (None, Some(component)) => {
            bootstrap_repo_config(ctx);
            handle_component_update(&component, ctx)
        }
        (None, None) => Err(CliError::InvalidArgument {
            command: COMMAND.to_string(),
            reason: "specify a component to update (e.g. `anolisa update <component>`), or use `anolisa update self` / `anolisa update all`".to_string(),
        }),
    }
}

// ── component update (#959): RPM-backed update for rpm-observed / rpm-managed ──

/// Wire shape for an `update <component>` result (`--json`) and its dry-run
/// preview.
#[derive(Serialize)]
struct ComponentUpdatePayload {
    component: String,
    package: String,
    /// Always `rpm`: update never switches backend. The raw path is a
    /// separate, not-yet-implemented branch.
    backend: &'static str,
    /// `rpm-observed` or `rpm-managed`; preserved across the update.
    ownership: &'static str,
    install_mode: String,
    /// EVR recorded before the update (rpmdb truth).
    from_version: String,
    /// EVR after the update; `None` on dry-run (nothing applied).
    #[serde(skip_serializing_if = "Option::is_none")]
    to_version: Option<String>,
    /// Whether the EVR actually changed (false on a no-op "already latest").
    updated: bool,
    dry_run: bool,
    /// Repo candidate EVRs surfaced in the dry-run preview (best-effort).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    available_candidates: Vec<String>,
    /// `None` on dry-run (nothing recorded).
    #[serde(skip_serializing_if = "Option::is_none")]
    operation_id: Option<String>,
    warnings: Vec<String>,
}

/// Dispatch `update <component>`: build the real rpm/dnf-backed query and
/// transaction, then route by recorded ownership.
fn handle_component_update(component: &str, ctx: &CliContext) -> Result<(), CliError> {
    let query = RpmPackageQuery::system();
    let txn = RpmTransaction::system();
    update_component_with_deps(component, ctx, &query, &txn, privilege::is_root())
}

/// Core of [`handle_component_update`] with the package query, transaction, and
/// root status injected so tests drive the RPM path without a live rpmdb/dnf or
/// real privileges.
fn update_component_with_deps(
    target: &str,
    ctx: &CliContext,
    query: &dyn PackageQuery,
    txn: &dyn PackageTransaction,
    is_root: bool,
) -> Result<(), CliError> {
    let command = format!("update {target}");
    let installed = common::load_installed_state(ctx, COMMAND)?;

    let obj = installed
        .find_object(ObjectKind::Component, target)
        .ok_or_else(|| CliError::InvalidArgument {
            command: command.clone(),
            reason: format!(
                "component '{target}' is not installed — nothing to update (run `anolisa status` to see what is installed, or `anolisa install {target}` to install it)"
            ),
        })?;

    match obj.effective_ownership() {
        // Raw update still needs the distribution-resolver planner; #959 only
        // wires the RPM path. Keep raw on the same not-implemented boundary as
        // before so behavior is unchanged for raw components.
        Ownership::RawManaged => Err(CliError::not_implemented_with_hint(
            command,
            "raw component update is not implemented yet (update planner / distribution resolver pending); only RPM-backed components update today",
        )),
        ownership @ (Ownership::RpmManaged | Ownership::RpmObserved) => {
            // Snapshot the package identity, then drop the immutable borrow so
            // the write path can re-acquire the lock and reload state.
            let package = match obj.rpm_metadata.as_ref().map(|m| m.package_name.clone()) {
                Some(p) if !p.is_empty() => p,
                _ => {
                    return Err(CliError::Runtime {
                        command,
                        reason: format!(
                            "component '{target}' is recorded as an RPM component but has no package metadata; run `anolisa repair {target}` to refresh it before updating"
                        ),
                    });
                }
            };
            update_rpm_component(
                target, &package, ownership, ctx, query, txn, is_root, &command,
            )
        }
    }
}

/// Execute the RPM update flow for one component:
/// `rpmdb query -> dnf repo query -> dnf update -> refresh ANOLISA state`.
/// Never switches backend; the dnf transaction is gated on root for real runs.
#[allow(clippy::too_many_arguments)]
fn update_rpm_component(
    component: &str,
    package: &str,
    ownership: Ownership,
    ctx: &CliContext,
    query: &dyn PackageQuery,
    txn: &dyn PackageTransaction,
    is_root: bool,
    command: &str,
) -> Result<(), CliError> {
    let mut warnings: Vec<String> = Vec::new();

    // 1. rpmdb query — the EVR we update from, and the truth source.
    let current = match query.query_installed(package) {
        Ok(Some(info)) => info,
        // State records the package but rpmdb no longer has it: a Missing drift.
        // The package is gone, so `repair` cannot refresh it; point at `forget`
        // (or reinstall) rather than running dnf blindly.
        Ok(None) => {
            return Err(CliError::Runtime {
                command: command.to_string(),
                reason: format!(
                    "RPM package '{package}' for component '{component}' is recorded in ANOLISA state but is not present in rpmdb — it may have been removed with `rpm -e`; run `anolisa forget {component}` to drop the stale state, or reinstall before updating"
                ),
            });
        }
        Err(PackageQueryError::CommandMissing { .. }) => {
            return Err(rpm_tooling_missing_error(command));
        }
        // Same name, several installed versions — a drift, not a clean update
        // target.
        Err(PackageQueryError::UnexpectedOutput { .. }) => {
            return Err(CliError::Runtime {
                command: command.to_string(),
                reason: format!(
                    "RPM package '{package}' has multiple installed versions; refusing to update an ambiguous package — resolve the duplicate first"
                ),
            });
        }
        Err(err) => return Err(rpm_query_err(err, command)),
    };
    let from_evr = current.version.to_string();

    // 2. dnf repo query — best-effort candidate enrichment. A repo-query failure
    //    must not block the update (dnf runs its own resolution); it only feeds
    //    the dry-run preview.
    let candidates = available_candidates(query, package, &current.arch, &mut warnings);

    let ownership_label = ownership.label();

    // 3. Dry-run preview — never touches the filesystem, never needs root.
    if ctx.dry_run {
        let payload = ComponentUpdatePayload {
            component: component.to_string(),
            package: package.to_string(),
            backend: "rpm",
            ownership: ownership_label,
            install_mode: ctx.install_mode.as_str().to_string(),
            from_version: from_evr,
            to_version: None,
            updated: false,
            dry_run: true,
            available_candidates: candidates,
            operation_id: None,
            warnings,
        };
        render_component_update(ctx, &payload);
        return Ok(());
    }

    // 4. Privilege gate — dnf transactions need root. Check up front so the user
    //    gets an actionable message instead of dnf's raw mid-transaction refusal.
    if !is_root {
        return Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "updating system RPM '{package}' requires root privileges; re-run with sudo: `sudo anolisa --install-mode system update {component}`"
            ),
        });
    }

    // 5. dnf update — delegate the file transaction.
    txn.update(package).map_err(|err| txn_err(err, command))?;

    // 6. Refresh ANOLISA state from rpmdb (authoritative post-update EVR).
    //
    // The dnf transaction already mutated rpmdb and cannot be rolled back, so a
    // failed re-read leaves the package updated but its new EVR unconfirmed. We
    // must not paper over that by recording the *old* EVR as a successful no-op
    // ("already up to date") — that hides a real change. Surface it as a failure
    // with a repair pointer instead; `persist_rpm_update` is never reached.
    let refreshed = match query.query_installed(package) {
        Ok(Some(info)) => info,
        Ok(None) => {
            return Err(CliError::Runtime {
                command: command.to_string(),
                reason: format!(
                    "dnf update of '{package}' succeeded but the package is no longer in rpmdb under that name (it may have been obsoleted or renamed); ANOLISA state for component '{component}' is now stale — run `anolisa repair {component}`"
                ),
            });
        }
        // Several installed versions after the update — a drift we cannot record
        // as a single EVR.
        Err(PackageQueryError::UnexpectedOutput { .. }) => {
            return Err(CliError::Runtime {
                command: command.to_string(),
                reason: format!(
                    "dnf update of '{package}' succeeded but rpmdb now reports multiple installed versions; ANOLISA state for component '{component}' is now stale — run `anolisa repair {component}`"
                ),
            });
        }
        Err(err) => {
            return Err(CliError::Runtime {
                command: command.to_string(),
                reason: format!(
                    "dnf update of '{package}' succeeded but reading the new version from rpmdb failed ({err}); ANOLISA state for component '{component}' is now stale — run `anolisa repair {component}`"
                ),
            });
        }
    };
    let to_evr = refreshed.version.to_string();
    let updated = to_evr != from_evr;

    let operation_id = persist_rpm_update(
        ctx, component, package, ownership, &refreshed, &to_evr, command, &warnings,
    )?;

    let payload = ComponentUpdatePayload {
        component: component.to_string(),
        package: package.to_string(),
        backend: "rpm",
        ownership: ownership_label,
        install_mode: ctx.install_mode.as_str().to_string(),
        from_version: from_evr,
        to_version: Some(to_evr),
        updated,
        dry_run: false,
        available_candidates: Vec::new(),
        operation_id: Some(operation_id),
        warnings,
    };
    render_component_update(ctx, &payload);
    Ok(())
}

/// Persist the refreshed RPM version under the install lock and append an audit
/// record. Ownership and `install_backend` are left untouched — update never
/// switches backend. Returns the operation id.
#[allow(clippy::too_many_arguments)]
fn persist_rpm_update(
    ctx: &CliContext,
    component: &str,
    package: &str,
    ownership: Ownership,
    refreshed: &PackageInfo,
    to_evr: &str,
    command: &str,
    warnings: &[String],
) -> Result<String, CliError> {
    let layout = common::resolve_layout(ctx);
    let _lock = InstallLock::acquire(&layout.lock_file).map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to acquire install lock: {err}"),
    })?;
    let mut state = common::load_installed_state(ctx, command)?;

    // Re-validate under the lock: the component must still exist and still be
    // RPM-owned. A concurrent uninstall or backend change between the pre-lock
    // read and here must not be clobbered by a stale update record.
    let obj = state
        .find_object_mut(ObjectKind::Component, component)
        .ok_or_else(|| CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "component '{component}' disappeared from state during update; no changes recorded"
            ),
        })?;
    if !obj.effective_ownership().is_rpm() {
        return Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "component '{component}' is no longer an RPM component in state; refusing to record an RPM update"
            ),
        });
    }

    // The package identity must also be unchanged under the lock. `dnf update`
    // ran against `package` (snapshotted before the lock); if a concurrent
    // operation re-pointed this component at a different RPM in the meantime,
    // writing the new EVR in place would graft package A's version onto package
    // B's metadata. Refuse rather than corrupt the record.
    let package_matches = obj
        .rpm_metadata
        .as_ref()
        .is_some_and(|m| m.package_name == package);
    if !package_matches {
        return Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "component '{component}' RPM package identity changed during update (expected package '{package}'); refusing to record an EVR against a different package — run `anolisa status {component}`"
            ),
        });
    }

    let now = now_iso8601();
    let lock_ts = Utc::now();
    let operation_id = format!(
        "op-update-{}-{}",
        lock_ts.format("%Y%m%d%H%M%S"),
        lock_ts.timestamp_subsec_nanos()
    );

    // Refresh the observed/managed version in place. ownership / install_backend
    // are deliberately untouched.
    obj.version = to_evr.to_string();
    obj.last_operation_id = Some(operation_id.clone());
    if let Some(meta) = obj.rpm_metadata.as_mut() {
        meta.evr = Some(to_evr.to_string());
        meta.arch = Some(refreshed.arch.clone());
    }

    state.operations.push(OperationRecord {
        id: operation_id.clone(),
        command: command.to_string(),
        status: "ok".to_string(),
        started_at: now.clone(),
        finished_at: Some(now.clone()),
    });

    let state_path = layout.state_dir.join("installed.toml");
    state.save(&state_path).map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to save state: {err}"),
    })?;

    // Audit log is best-effort: the update already persisted, so a log failure
    // downgrades to a warning instead of unwinding.
    let log = CentralLog::open(layout.central_log.clone());
    let record = LogRecord {
        kind: LogKind::Operation,
        operation_id: Some(operation_id.clone()),
        command: command.to_string(),
        source: "anolisa-cli".to_string(),
        component: Some(component.to_string()),
        severity: Severity::Info,
        message: format!(
            "updated RPM package {package} for component {component} to {to_evr} via dnf ({ownership_label})",
            ownership_label = ownership.label(),
        ),
        actor: "cli".to_string(),
        install_mode: Some(ctx.install_mode.as_str().to_string()),
        started_at: now.clone(),
        finished_at: Some(now),
        status: Some(LogStatus::Ok),
        objects: vec![component.to_string()],
        backup_ids: Vec::new(),
        warnings: warnings.to_vec(),
        details: serde_json::Value::Null,
    };
    if let Err(err) = log.append(&record) {
        eprintln!("warning: failed to write central log: {err}");
    }

    Ok(operation_id)
}

/// Best-effort repo candidate EVRs for `package`, filtered to the installed
/// arch (plus `noarch`). A query failure degrades to a warning and an empty
/// list — dnf still resolves candidates at update time.
fn available_candidates(
    query: &dyn PackageQuery,
    package: &str,
    arch: &str,
    warnings: &mut Vec<String>,
) -> Vec<String> {
    match query.query_available(package) {
        Ok(infos) => {
            let mut evrs: Vec<String> = infos
                .into_iter()
                .filter(|i| i.arch == arch || i.arch == "noarch")
                .map(|i| i.version.to_string())
                .collect();
            // Sort + dedup for deterministic output; this is a display list, not
            // a version-ordered ranking (rpmvercmp is dnf's job at update time).
            evrs.sort();
            evrs.dedup();
            evrs
        }
        Err(err) => {
            warnings.push(format!(
                "could not query available versions for '{package}': {err}; dnf will still resolve candidates at update time"
            ));
            Vec::new()
        }
    }
}

/// Human/JSON renderer for a component update result.
fn render_component_update(ctx: &CliContext, payload: &ComponentUpdatePayload) {
    if ctx.json {
        // Errors here are unreachable for a plain Serialize struct; ignore the
        // Result so an (already-persisted) update is not reported as failed.
        let _ = response::render_json(COMMAND, payload);
        return;
    }
    if ctx.quiet {
        return;
    }
    let color = Palette::new(ctx.no_color);
    if payload.dry_run {
        println!(
            "{} {} {} {}",
            color.command("update"),
            payload.component,
            color.muted(format!("({}, {})", payload.ownership, payload.package)),
            color.muted("(dry-run — nothing updated)"),
        );
        println!("{} {}", color.label("current:"), payload.from_version);
        if payload.available_candidates.is_empty() {
            println!(
                "{} {}",
                color.label("available:"),
                color.muted("no repo candidates reported"),
            );
        } else {
            println!(
                "{} {}",
                color.label("available:"),
                payload.available_candidates.join(", "),
            );
        }
        println!("  would run: dnf update -y {}", payload.package);
    } else if payload.updated {
        println!(
            "{} {} {} → {}",
            color.ok("✓ updated"),
            payload.component,
            payload.from_version,
            payload.to_version.as_deref().unwrap_or("-"),
        );
    } else {
        println!(
            "{} {} is already up to date ({})",
            color.ok("✓"),
            payload.component,
            payload.from_version,
        );
    }
    // Remind the operator that an observed row is a pre-existing system RPM.
    if payload.ownership == "rpm-observed" {
        println!(
            "    {} {} is a system RPM observed by ANOLISA; dnf owns the file transaction",
            color.label("note:"),
            payload.package,
        );
    }
    render_warnings(&payload.warnings, &color);
}

/// Map a [`PackageQueryError`] onto a CLI runtime error (the benign
/// not-installed / multi-version branches are split off by the caller).
fn rpm_query_err(err: PackageQueryError, command: &str) -> CliError {
    CliError::Runtime {
        command: command.to_string(),
        reason: format!("rpm query failed: {err}"),
    }
}

/// Warn-and-exit error when `rpm`/`dnf` is absent: an RPM component cannot be
/// updated without the package manager.
fn rpm_tooling_missing_error(command: &str) -> CliError {
    CliError::Runtime {
        command: command.to_string(),
        reason: "rpm/dnf not found: cannot update an RPM-backed component without the package manager. Install rpm/dnf and retry".to_string(),
    }
}

/// Map a [`PackageTransactionError`] onto a CLI runtime error with an actionable
/// hint.
fn txn_err(err: PackageTransactionError, command: &str) -> CliError {
    match err {
        PackageTransactionError::CommandMissing { .. } => rpm_tooling_missing_error(command),
        PackageTransactionError::PermissionDenied { command: bin } => CliError::Runtime {
            command: command.to_string(),
            reason: format!("permission denied running {bin}; re-run the update with sudo"),
        },
        PackageTransactionError::TransactionFailed { code, stderr, .. } => CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "dnf update failed (exit {}): {}",
                code.map(|c| c.to_string())
                    .unwrap_or_else(|| "signal".to_string()),
                stderr.trim(),
            ),
        },
    }
}

/// Render any accumulated warnings to stderr, one per line.
fn render_warnings(warnings: &[String], color: &Palette) {
    for w in warnings {
        eprintln!("{} {w}", color.warn("warning:"));
    }
}

/// RFC3339 UTC timestamp, seconds precision (matches the install path).
fn now_iso8601() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// TEMPORARY: make sure the user-editable repo config exists before any
/// update operation runs (see [`DEFAULT_REPO_CONFIG_URL`]).
///
/// Best-effort by design: every failure mode (network down, bad TOML,
/// unwritable etc dir) degrades to a stderr warning — `update self` and
/// component updates must not be blocked by config bootstrap. The
/// download is validated as a parseable [`RepoConfig`] before anything
/// lands on disk, and the write is tmp + rename so a crash cannot leave
/// a half-written config behind.
fn bootstrap_repo_config(ctx: &CliContext) {
    let layout = common::resolve_layout(ctx);
    let dest = layout.etc_dir.join("repo.toml");
    if dest.exists() {
        return;
    }
    let url = std::env::var("ANOLISA_REPO_CONFIG_URL")
        .unwrap_or_else(|_| DEFAULT_REPO_CONFIG_URL.to_string());
    if ctx.dry_run {
        if !ctx.quiet && !ctx.json {
            println!(
                "would download repo config from {url} to {} (not present locally)",
                dest.display()
            );
        }
        return;
    }
    match fetch_and_write_repo_config(&url, &dest) {
        Ok(()) => {
            if !ctx.quiet && !ctx.json {
                let color = Palette::new(ctx.no_color);
                println!(
                    "{} repo config was missing — downloaded {} to {}",
                    color.ok("✓"),
                    url,
                    color.path(dest.display().to_string()),
                );
            }
        }
        Err(reason) => {
            eprintln!("warning: repo config bootstrap skipped: {reason}");
        }
    }
}

/// Download, validate, and atomically install the repo config at `dest`.
fn fetch_and_write_repo_config(url: &str, dest: &Path) -> Result<(), String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .build();
    let response = agent
        .get(url)
        .call()
        .map_err(|err| format!("fetch {url}: {err}"))?;
    let mut body = String::new();
    response
        .into_reader()
        .take(MAX_REPO_CONFIG_BYTES)
        .read_to_string(&mut body)
        .map_err(|err| format!("read {url}: {err}"))?;

    // Refuse to install bytes that the CLI itself cannot parse — a bad
    // published config must not break every subsequent command.
    RepoConfig::from_toml_str(&body).map_err(|err| format!("downloaded config invalid: {err}"))?;

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("create {}: {err}", parent.display()))?;
    }
    let tmp = dest.with_extension("toml.tmp");
    std::fs::write(&tmp, &body).map_err(|err| format!("write {}: {err}", tmp.display()))?;
    std::fs::rename(&tmp, dest).map_err(|err| {
        let _ = std::fs::remove_file(&tmp);
        format!("rename to {}: {err}", dest.display())
    })?;
    Ok(())
}

/// Execute CLI self-update: fetch release manifest, compare versions,
/// download and atomically replace the running binary.
///
/// Also called from `anolisa self update` as a convenience alias.
///
/// # Errors
///
/// Returns [`CliError::Runtime`] when the manifest fetch, version check,
/// download, or binary replacement fails.
pub(in crate::commands) fn handle_self_update(ctx: &CliContext) -> Result<(), CliError> {
    let url = self_update::update_url();
    let current_version = env!("CARGO_PKG_VERSION");

    let progress_cb: Option<ProgressFn> = if !ctx.json && !ctx.quiet {
        Some(Box::new(move |downloaded: u64, total: Option<u64>| {
            render_progress(downloaded, total);
        }))
    } else {
        None
    };

    let result =
        self_update::check_and_update(&url, current_version, ctx.dry_run, progress_cb.as_ref());

    // Clear the progress line before any output (success or error).
    if progress_cb.is_some() {
        eprint!("\r\x1b[2K");
    }

    let outcome = result.map_err(|e| CliError::Runtime {
        command: "update self".to_string(),
        reason: e.to_string(),
    })?;

    if ctx.json {
        return render_json_outcome(&outcome, ctx.dry_run);
    }

    if ctx.quiet {
        return Ok(());
    }

    let color = Palette::new(ctx.no_color);
    match &outcome {
        SelfUpdateOutcome::AlreadyLatest { version } => {
            println!(
                "{} anolisa {} is already the latest version",
                color.ok("✓"),
                version
            );
        }
        SelfUpdateOutcome::UpdateAvailable { from, to } => {
            if ctx.dry_run {
                println!("{} update available: {} → {}", color.warn("⬆"), from, to);
                println!("  run without --dry-run to apply");
            } else {
                println!("{} anolisa updated: {} → {}", color.ok("✓"), from, to);
                println!("  view the changelog at {}", color.path(CLI_CHANGELOG_URL));
                eprintln!(
                    "  {} signature verification not yet implemented; \
                     update trust relies on HTTPS only",
                    color.warn("⚠")
                );
            }
        }
    }

    Ok(())
}

fn render_progress(downloaded: u64, total: Option<u64>) {
    match total {
        Some(t) if t > 0 => {
            let pct = (downloaded as f64 / t as f64 * 100.0).min(100.0);
            eprint!(
                "\r  downloading ... {:.1} / {:.1} MiB ({:.0}%)",
                downloaded as f64 / 1_048_576.0,
                t as f64 / 1_048_576.0,
                pct,
            );
        }
        _ => {
            eprint!(
                "\r  downloading ... {:.1} MiB",
                downloaded as f64 / 1_048_576.0,
            );
        }
    }
}

#[derive(Serialize)]
struct SelfUpdateData {
    current_version: String,
    latest_version: String,
    update_available: bool,
    updated: bool,
}

fn build_json_data(outcome: &SelfUpdateOutcome, dry_run: bool) -> SelfUpdateData {
    match outcome {
        SelfUpdateOutcome::AlreadyLatest { version } => SelfUpdateData {
            current_version: version.clone(),
            latest_version: version.clone(),
            update_available: false,
            updated: false,
        },
        SelfUpdateOutcome::UpdateAvailable { from, to } => SelfUpdateData {
            current_version: from.clone(),
            latest_version: to.clone(),
            update_available: true,
            updated: !dry_run,
        },
    }
}

fn render_json_outcome(outcome: &SelfUpdateOutcome, dry_run: bool) -> Result<(), CliError> {
    response::render_json("update self", build_json_data(outcome, dry_run))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serve one HTTP response on an ephemeral port and return its URL.
    fn serve_once(body: &'static str) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                use std::io::Write;
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
            }
        });
        format!("http://{addr}/repo.toml")
    }

    #[test]
    fn bootstrap_fetch_writes_valid_config() {
        let body = "schema_version = 1\ndefault_backend = \"raw\"\n\n[backends.raw]\nbase_url = \"https://example.com/v1/\"\n";
        let url = serve_once(body);
        let tmp = tempfile::tempdir().expect("tempdir");
        let dest = tmp.path().join("etc/repo.toml");

        fetch_and_write_repo_config(&url, &dest).expect("bootstrap ok");
        assert_eq!(std::fs::read_to_string(&dest).expect("read dest"), body);
        assert!(!dest.with_extension("toml.tmp").exists());
    }

    #[test]
    fn bootstrap_fetch_refuses_unparseable_config() {
        let url = serve_once("this is not a repo config");
        let tmp = tempfile::tempdir().expect("tempdir");
        let dest = tmp.path().join("etc/repo.toml");

        let err = fetch_and_write_repo_config(&url, &dest).expect_err("must refuse");
        assert!(err.contains("invalid"), "unexpected error: {err}");
        assert!(!dest.exists(), "invalid config must not land on disk");
    }

    #[test]
    fn json_dry_run_reports_available_but_not_updated() {
        let outcome = SelfUpdateOutcome::UpdateAvailable {
            from: "0.1.0".into(),
            to: "0.2.0".into(),
        };
        let data = build_json_data(&outcome, true);
        assert!(data.update_available);
        assert!(!data.updated);
    }

    #[test]
    fn json_real_update_reports_both_true() {
        let outcome = SelfUpdateOutcome::UpdateAvailable {
            from: "0.1.0".into(),
            to: "0.2.0".into(),
        };
        let data = build_json_data(&outcome, false);
        assert!(data.update_available);
        assert!(data.updated);
    }

    #[test]
    fn json_already_latest_reports_both_false() {
        let outcome = SelfUpdateOutcome::AlreadyLatest {
            version: "0.1.0".into(),
        };
        let data = build_json_data(&outcome, false);
        assert!(!data.update_available);
        assert!(!data.updated);
    }

    // ── component update (#959): RPM path ───────────────────────────────

    use std::cell::{Cell, RefCell};
    use std::path::PathBuf;

    use anolisa_core::state::{
        InstallMode as StateInstallMode, InstalledObject, InstalledState, ObjectStatus, RpmMetadata,
    };
    use anolisa_platform::pkg_query::PackageVersion;

    use crate::context::InstallMode;

    /// In-memory rpm world implementing **both** [`PackageQuery`] and
    /// [`PackageTransaction`], so one fake drives the whole update flow.
    ///
    /// `installed` mutates: a successful [`update`](PackageTransaction::update)
    /// applies `upgrade_to`, modelling rpmdb advancing after dnf runs — so the
    /// pre-update query and the post-update refresh return different EVRs.
    struct FakeRpm {
        package: String,
        installed: RefCell<Option<PackageInfo>>,
        available: Vec<PackageInfo>,
        /// PackageInfo the rpmdb holds after a successful update; `None` keeps
        /// the same version (a no-op "already latest").
        upgrade_to: Option<PackageInfo>,
        /// `false` makes the dnf transaction fail.
        update_succeeds: bool,
        /// `true` makes `query_installed` report a same-name multi-version drift.
        multi_version: bool,
        /// `true` makes the *post-update* `query_installed` report the package
        /// gone, modelling a failed rpmdb re-read after a successful dnf update.
        post_update_missing: bool,
        update_calls: Cell<usize>,
    }

    impl FakeRpm {
        fn new(package: &str, installed: Option<PackageInfo>) -> Self {
            Self {
                package: package.to_string(),
                installed: RefCell::new(installed),
                available: Vec::new(),
                upgrade_to: None,
                update_succeeds: true,
                multi_version: false,
                post_update_missing: false,
                update_calls: Cell::new(0),
            }
        }
        fn with_available(mut self, infos: Vec<PackageInfo>) -> Self {
            self.available = infos;
            self
        }
        fn upgrading_to(mut self, info: PackageInfo) -> Self {
            self.upgrade_to = Some(info);
            self
        }
        fn failing_update(mut self) -> Self {
            self.update_succeeds = false;
            self
        }
        fn multi_version(mut self) -> Self {
            self.multi_version = true;
            self
        }
        fn post_update_missing(mut self) -> Self {
            self.post_update_missing = true;
            self
        }
    }

    impl PackageQuery for FakeRpm {
        fn query_installed(&self, package: &str) -> Result<Option<PackageInfo>, PackageQueryError> {
            if package != self.package {
                return Ok(None);
            }
            // Simulate a failed post-update re-read: the package "vanishes" only
            // after dnf update has run, so the pre-update query still succeeds.
            if self.post_update_missing && self.update_calls.get() > 0 {
                return Ok(None);
            }
            if self.multi_version {
                return Err(PackageQueryError::UnexpectedOutput {
                    command: "rpm".to_string(),
                    detail: "2 installed versions".to_string(),
                });
            }
            Ok(self.installed.borrow().clone())
        }

        fn query_available(&self, package: &str) -> Result<Vec<PackageInfo>, PackageQueryError> {
            if package != self.package {
                return Ok(Vec::new());
            }
            Ok(self.available.clone())
        }
    }

    impl PackageTransaction for FakeRpm {
        fn install(&self, _package: &str) -> Result<(), PackageTransactionError> {
            // The update flow never installs; a call here is a routing bug.
            panic!("update path must not delegate a dnf install");
        }

        fn update(&self, package: &str) -> Result<(), PackageTransactionError> {
            self.update_calls.set(self.update_calls.get() + 1);
            assert_eq!(package, self.package, "update targeted the wrong package");
            if !self.update_succeeds {
                return Err(PackageTransactionError::TransactionFailed {
                    command: "dnf".to_string(),
                    operation: "update".to_string(),
                    code: Some(1),
                    stderr: "repo unreachable".to_string(),
                });
            }
            if let Some(next) = &self.upgrade_to {
                *self.installed.borrow_mut() = Some(next.clone());
            }
            Ok(())
        }
    }

    fn pkg_info(name: &str, version: &str, release: Option<&str>, arch: &str) -> PackageInfo {
        PackageInfo {
            name: name.to_string(),
            version: PackageVersion {
                epoch: None,
                version: version.to_string(),
                release: release.map(str::to_string),
            },
            arch: arch.to_string(),
            origin: None,
        }
    }

    fn ctx(prefix: PathBuf, install_mode: InstallMode, dry_run: bool) -> CliContext {
        CliContext {
            install_mode,
            prefix: Some(prefix),
            json: false,
            dry_run,
            verbose: false,
            quiet: true,
            no_color: true,
        }
    }

    /// Build an RPM-backed component object (observed or managed).
    fn rpm_object(
        component: &str,
        package: &str,
        evr: &str,
        ownership: Ownership,
        status: ObjectStatus,
    ) -> InstalledObject {
        InstalledObject {
            kind: ObjectKind::Component,
            name: component.to_string(),
            version: evr.to_string(),
            status,
            manifest_digest: None,
            distribution_source: None,
            install_backend: Some("rpm".to_string()),
            ownership: Some(ownership),
            rpm_metadata: Some(RpmMetadata {
                package_name: package.to_string(),
                evr: Some(evr.to_string()),
                arch: Some("x86_64".to_string()),
                source_repo: Some("@System".to_string()),
            }),
            installed_at: "2026-06-01T10:00:00Z".to_string(),
            last_operation_id: Some("op-prior".to_string()),
            managed: !matches!(ownership, Ownership::RpmObserved),
            adopted: matches!(ownership, Ownership::RpmObserved),
            subscription_scope: Default::default(),
            enabled_features: Vec::new(),
            component_refs: Vec::new(),
            files: Vec::new(),
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
        }
    }

    /// A raw-managed component object (no rpm metadata).
    fn raw_object(component: &str, version: &str) -> InstalledObject {
        InstalledObject {
            kind: ObjectKind::Component,
            name: component.to_string(),
            version: version.to_string(),
            status: ObjectStatus::Installed,
            manifest_digest: None,
            distribution_source: Some("https://example.com/x".to_string()),
            install_backend: Some("raw".to_string()),
            ownership: Some(Ownership::RawManaged),
            rpm_metadata: None,
            installed_at: "2026-06-01T10:00:00Z".to_string(),
            last_operation_id: None,
            managed: true,
            adopted: false,
            subscription_scope: Default::default(),
            enabled_features: Vec::new(),
            component_refs: Vec::new(),
            files: Vec::new(),
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
        }
    }

    /// Seed `installed.toml` for `ctx`'s scope with one object.
    fn seed(ctx: &CliContext, obj: InstalledObject) {
        let layout = common::resolve_layout(ctx);
        std::fs::create_dir_all(&layout.state_dir).expect("mkdir state");
        let mut state = InstalledState {
            install_mode: match ctx.install_mode {
                InstallMode::System => StateInstallMode::System,
                InstallMode::User => StateInstallMode::User,
            },
            prefix: layout.prefix.clone(),
            ..Default::default()
        };
        state.upsert_object(obj);
        state
            .save(&layout.state_dir.join("installed.toml"))
            .expect("seed state");
    }

    fn load_state(ctx: &CliContext) -> InstalledState {
        let layout = common::resolve_layout(ctx);
        InstalledState::load(&layout.state_dir.join("installed.toml")).expect("load state")
    }

    /// rpm-observed component, root, real run: dnf update runs, the EVR is
    /// refreshed from rpmdb, and ownership/backend are preserved.
    #[test]
    fn rpm_observed_update_refreshes_evr_and_keeps_ownership() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            rpm_object(
                "copilot-shell",
                "anolisa-copilot-shell",
                "2.2.0-1.al8",
                Ownership::RpmObserved,
                ObjectStatus::Adopted,
            ),
        );
        let rpm = FakeRpm::new(
            "anolisa-copilot-shell",
            Some(pkg_info(
                "anolisa-copilot-shell",
                "2.2.0",
                Some("1.al8"),
                "x86_64",
            )),
        )
        .upgrading_to(pkg_info(
            "anolisa-copilot-shell",
            "2.3.0",
            Some("1.al8"),
            "x86_64",
        ));

        update_component_with_deps("copilot-shell", &c, &rpm, &rpm, true).expect("update ok");
        assert_eq!(rpm.update_calls.get(), 1, "dnf update must run once");

        let obj = load_state(&c)
            .find_object(ObjectKind::Component, "copilot-shell")
            .cloned()
            .expect("component present");
        assert_eq!(obj.version, "2.3.0-1.al8", "version refreshed from rpmdb");
        assert_eq!(
            obj.ownership,
            Some(Ownership::RpmObserved),
            "ownership preserved"
        );
        assert_eq!(
            obj.install_backend.as_deref(),
            Some("rpm"),
            "backend not switched"
        );
        assert_eq!(obj.status, ObjectStatus::Adopted, "status unchanged");
        let meta = obj.rpm_metadata.expect("metadata");
        assert_eq!(meta.evr.as_deref(), Some("2.3.0-1.al8"));
        assert_ne!(obj.last_operation_id.as_deref(), Some("op-prior"));
    }

    /// rpm-managed component updates the same way (different ownership/status).
    #[test]
    fn rpm_managed_update_refreshes_evr() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            rpm_object(
                "copilot-shell",
                "anolisa-copilot-shell",
                "1.0.0-1.al8",
                Ownership::RpmManaged,
                ObjectStatus::Installed,
            ),
        );
        let rpm = FakeRpm::new(
            "anolisa-copilot-shell",
            Some(pkg_info(
                "anolisa-copilot-shell",
                "1.0.0",
                Some("1.al8"),
                "x86_64",
            )),
        )
        .upgrading_to(pkg_info(
            "anolisa-copilot-shell",
            "1.1.0",
            Some("1.al8"),
            "x86_64",
        ));

        update_component_with_deps("copilot-shell", &c, &rpm, &rpm, true).expect("update ok");

        let obj = load_state(&c)
            .find_object(ObjectKind::Component, "copilot-shell")
            .cloned()
            .expect("component present");
        assert_eq!(obj.version, "1.1.0-1.al8");
        assert_eq!(obj.ownership, Some(Ownership::RpmManaged));
        assert_eq!(obj.status, ObjectStatus::Installed);
    }

    /// Non-root real run is refused with an actionable message; dnf never runs
    /// and state is untouched.
    #[test]
    fn non_root_update_is_refused_without_running_dnf() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            rpm_object(
                "copilot-shell",
                "anolisa-copilot-shell",
                "2.2.0-1.al8",
                Ownership::RpmObserved,
                ObjectStatus::Adopted,
            ),
        );
        let rpm = FakeRpm::new(
            "anolisa-copilot-shell",
            Some(pkg_info(
                "anolisa-copilot-shell",
                "2.2.0",
                Some("1.al8"),
                "x86_64",
            )),
        );

        let err = update_component_with_deps("copilot-shell", &c, &rpm, &rpm, false)
            .expect_err("must refuse without root");
        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert!(
            err.reason().contains("root") && err.reason().contains("sudo"),
            "reason must point at sudo: {}",
            err.reason()
        );
        assert_eq!(rpm.update_calls.get(), 0, "dnf must not run without root");
        // State unchanged.
        assert_eq!(
            load_state(&c)
                .find_object(ObjectKind::Component, "copilot-shell")
                .and_then(|o| o.last_operation_id.clone())
                .as_deref(),
            Some("op-prior"),
        );
    }

    /// Dry-run previews the plan without running dnf, needing root, or writing
    /// state — even for a non-root caller.
    #[test]
    fn dry_run_previews_without_dnf_or_state_write() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, true);
        seed(
            &c,
            rpm_object(
                "copilot-shell",
                "anolisa-copilot-shell",
                "2.2.0-1.al8",
                Ownership::RpmObserved,
                ObjectStatus::Adopted,
            ),
        );
        let rpm = FakeRpm::new(
            "anolisa-copilot-shell",
            Some(pkg_info(
                "anolisa-copilot-shell",
                "2.2.0",
                Some("1.al8"),
                "x86_64",
            )),
        )
        .with_available(vec![pkg_info(
            "anolisa-copilot-shell",
            "2.3.0",
            Some("1.al8"),
            "x86_64",
        )]);

        update_component_with_deps("copilot-shell", &c, &rpm, &rpm, false).expect("dry-run ok");
        assert_eq!(rpm.update_calls.get(), 0, "dry-run must not run dnf");
        // Version stays at the seeded value.
        assert_eq!(
            load_state(&c)
                .find_object(ObjectKind::Component, "copilot-shell")
                .map(|o| o.version.clone())
                .as_deref(),
            Some("2.2.0-1.al8"),
        );
    }

    /// A component absent from state routes to INVALID_ARGUMENT (exit 2), not a
    /// runtime failure, and never runs dnf.
    #[test]
    fn unknown_component_routes_to_invalid_argument() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        let rpm = FakeRpm::new("anolisa-copilot-shell", None);
        let err = update_component_with_deps("copilot-shell", &c, &rpm, &rpm, true)
            .expect_err("absent component must error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert_eq!(err.exit_code(), 2);
        assert!(err.reason().contains("not installed"));
        assert_eq!(rpm.update_calls.get(), 0);
    }

    /// Regression: a bare `anolisa update` (no component, no subcommand) fails
    /// validation as INVALID_ARGUMENT. The positional surface lets `(None,
    /// None)` reach `handle()`, where the old top-of-function bootstrap would
    /// otherwise hit the network / write config before this error — so the
    /// bootstrap now lives inside the real-update branches and this path must
    /// short-circuit here. The fixed routing reaches no bootstrap, so the test
    /// makes no network call.
    #[test]
    fn bare_update_errors_before_any_bootstrap() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        // Non-dry-run system ctx: an unconditional bootstrap would try to fetch
        // and write etc_dir/repo.toml here.
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        let repo_toml = common::resolve_layout(&c).etc_dir.join("repo.toml");

        let err = handle(
            UpdateArgs {
                component: None,
                command: None,
            },
            &c,
        )
        .expect_err("bare `update` must fail validation");

        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert_eq!(err.exit_code(), 2);
        assert!(
            !repo_toml.exists(),
            "no repo config must be written for an invalid invocation: {} exists",
            repo_toml.display()
        );
    }

    /// State records the RPM but rpmdb no longer has it (rpm -e drift): refuse
    /// with a forget pointer rather than running dnf (the gone package cannot be
    /// refreshed by repair).
    #[test]
    fn missing_from_rpmdb_refuses_with_forget_hint() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            rpm_object(
                "copilot-shell",
                "anolisa-copilot-shell",
                "2.2.0-1.al8",
                Ownership::RpmObserved,
                ObjectStatus::Adopted,
            ),
        );
        // rpmdb reports nothing installed for the package.
        let rpm = FakeRpm::new("anolisa-copilot-shell", None);
        let err = update_component_with_deps("copilot-shell", &c, &rpm, &rpm, true)
            .expect_err("drift must error");
        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert!(
            err.reason().contains("forget"),
            "reason must point at forget: {}",
            err.reason()
        );
        assert_eq!(rpm.update_calls.get(), 0);
    }

    /// AC4: a user-mode raw override updates only the user raw component and
    /// never touches the shadowed system RPM. The raw path is not implemented,
    /// so it surfaces NOT_IMPLEMENTED — crucially without running dnf.
    #[test]
    fn user_raw_override_does_not_touch_system_rpm() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::User, false);
        seed(&c, raw_object("copilot-shell", "9.9.9"));
        let rpm = FakeRpm::new(
            "anolisa-copilot-shell",
            Some(pkg_info(
                "anolisa-copilot-shell",
                "2.2.0",
                Some("1.al8"),
                "x86_64",
            )),
        );
        let err = update_component_with_deps("copilot-shell", &c, &rpm, &rpm, true)
            .expect_err("raw update is not implemented");
        assert_eq!(err.code(), "NOT_IMPLEMENTED");
        assert_eq!(
            rpm.update_calls.get(),
            0,
            "user raw update must never run dnf on the system RPM"
        );
    }

    /// `dnf update` failure surfaces as EXECUTION_FAILED and does not refresh
    /// state (the version stays at its pre-update value).
    #[test]
    fn dnf_failure_surfaces_and_leaves_state_untouched() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            rpm_object(
                "copilot-shell",
                "anolisa-copilot-shell",
                "2.2.0-1.al8",
                Ownership::RpmObserved,
                ObjectStatus::Adopted,
            ),
        );
        let rpm = FakeRpm::new(
            "anolisa-copilot-shell",
            Some(pkg_info(
                "anolisa-copilot-shell",
                "2.2.0",
                Some("1.al8"),
                "x86_64",
            )),
        )
        .failing_update();

        let err = update_component_with_deps("copilot-shell", &c, &rpm, &rpm, true)
            .expect_err("dnf failure must propagate");
        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert!(err.reason().contains("dnf update failed"));
        assert_eq!(
            load_state(&c)
                .find_object(ObjectKind::Component, "copilot-shell")
                .map(|o| o.version.clone())
                .as_deref(),
            Some("2.2.0-1.al8"),
            "failed update must not refresh the recorded version"
        );
    }

    /// A same-name multi-version rpmdb is a drift, not an update target.
    #[test]
    fn multi_version_drift_is_refused() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            rpm_object(
                "copilot-shell",
                "anolisa-copilot-shell",
                "2.2.0-1.al8",
                Ownership::RpmObserved,
                ObjectStatus::Adopted,
            ),
        );
        let rpm = FakeRpm::new(
            "anolisa-copilot-shell",
            Some(pkg_info(
                "anolisa-copilot-shell",
                "2.2.0",
                Some("1.al8"),
                "x86_64",
            )),
        )
        .multi_version();
        let err = update_component_with_deps("copilot-shell", &c, &rpm, &rpm, true)
            .expect_err("multi-version must error");
        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert!(err.reason().contains("multiple installed versions"));
        assert_eq!(rpm.update_calls.get(), 0);
    }

    /// No-op update (already latest): dnf runs, EVR is unchanged, the result is
    /// reported as not-updated, and state still records the operation.
    #[test]
    fn already_latest_reports_no_change() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            rpm_object(
                "copilot-shell",
                "anolisa-copilot-shell",
                "2.3.0-1.al8",
                Ownership::RpmObserved,
                ObjectStatus::Adopted,
            ),
        );
        // upgrade_to is None => update() is a no-op; EVR stays the same.
        let rpm = FakeRpm::new(
            "anolisa-copilot-shell",
            Some(pkg_info(
                "anolisa-copilot-shell",
                "2.3.0",
                Some("1.al8"),
                "x86_64",
            )),
        );
        update_component_with_deps("copilot-shell", &c, &rpm, &rpm, true).expect("update ok");
        assert_eq!(rpm.update_calls.get(), 1);
        let obj = load_state(&c)
            .find_object(ObjectKind::Component, "copilot-shell")
            .cloned()
            .expect("present");
        assert_eq!(obj.version, "2.3.0-1.al8");
        // Operation still recorded (last_operation_id advanced from the seed).
        assert_ne!(obj.last_operation_id.as_deref(), Some("op-prior"));
    }

    /// dnf update applied, but the post-update rpmdb re-read cannot confirm the
    /// new EVR: surface a repair-pointing failure rather than recording the stale
    /// EVR as a no-op "already up to date", and leave the recorded version as-is.
    #[test]
    fn refresh_failure_after_successful_update_errors_and_leaves_state() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        seed(
            &c,
            rpm_object(
                "copilot-shell",
                "anolisa-copilot-shell",
                "2.2.0-1.al8",
                Ownership::RpmObserved,
                ObjectStatus::Adopted,
            ),
        );
        let rpm = FakeRpm::new(
            "anolisa-copilot-shell",
            Some(pkg_info(
                "anolisa-copilot-shell",
                "2.2.0",
                Some("1.al8"),
                "x86_64",
            )),
        )
        .post_update_missing();

        let err = update_component_with_deps("copilot-shell", &c, &rpm, &rpm, true)
            .expect_err("a failed post-update refresh must surface");
        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert!(
            err.reason().contains("repair"),
            "reason must point at repair: {}",
            err.reason()
        );
        assert_eq!(rpm.update_calls.get(), 1, "dnf update did run");
        // The recorded version is untouched: no stale EVR was written as success.
        assert_eq!(
            load_state(&c)
                .find_object(ObjectKind::Component, "copilot-shell")
                .map(|o| o.version.clone())
                .as_deref(),
            Some("2.2.0-1.al8"),
        );
    }

    /// Post-lock guard: if the component's recorded RPM package_name drifted
    /// while dnf ran, persist refuses rather than grafting the updated package's
    /// EVR onto a different package's metadata.
    #[test]
    fn persist_refuses_when_package_identity_changed_under_lock() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, false);
        // State records the component under package B...
        seed(
            &c,
            rpm_object(
                "copilot-shell",
                "anolisa-pkg-b",
                "1.0.0-1.al8",
                Ownership::RpmObserved,
                ObjectStatus::Adopted,
            ),
        );
        // ...but `dnf update` ran against package A (snapshotted before a
        // concurrent identity change). persist must refuse.
        let refreshed = pkg_info("anolisa-pkg-a", "2.0.0", Some("1.al8"), "x86_64");
        let err = persist_rpm_update(
            &c,
            "copilot-shell",
            "anolisa-pkg-a",
            Ownership::RpmObserved,
            &refreshed,
            "2.0.0-1.al8",
            "update copilot-shell",
            &[],
        )
        .expect_err("package identity drift must be refused");
        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert!(
            err.reason().contains("package identity changed"),
            "got: {}",
            err.reason()
        );
        // State untouched: still package B at its old EVR.
        let obj = load_state(&c)
            .find_object(ObjectKind::Component, "copilot-shell")
            .cloned()
            .expect("present");
        assert_eq!(obj.version, "1.0.0-1.al8");
        assert_eq!(
            obj.rpm_metadata.expect("meta").package_name,
            "anolisa-pkg-b"
        );
    }

    // ── CLI surface: `update <component>` is the direct form ────────────

    use clap::Parser;

    /// `update <component>` parses to the positional, with no subcommand.
    #[test]
    fn update_component_parses_as_positional() {
        let a = UpdateArgs::try_parse_from(["update", "copilot-shell"]).expect("parse");
        assert_eq!(a.component.as_deref(), Some("copilot-shell"));
        assert!(a.command.is_none());
    }

    /// `update self` parses to the self subcommand, not a component named
    /// "self" (subcommands take precedence over the positional).
    #[test]
    fn update_self_parses_as_subcommand() {
        let a = UpdateArgs::try_parse_from(["update", "self"]).expect("parse");
        assert!(matches!(a.command, Some(UpdateCommands::SelfBin)));
        assert!(a.component.is_none());
    }

    /// `update all` parses to the all subcommand.
    #[test]
    fn update_all_parses_as_subcommand() {
        let a = UpdateArgs::try_parse_from(["update", "all"]).expect("parse");
        assert!(matches!(a.command, Some(UpdateCommands::All)));
        assert!(a.component.is_none());
    }

    /// A positional and a subcommand are mutually exclusive.
    #[test]
    fn update_component_with_subcommand_is_a_parse_error() {
        UpdateArgs::try_parse_from(["update", "copilot-shell", "self"])
            .expect_err("positional + subcommand must conflict");
    }

    /// `update` with no target is an INVALID_ARGUMENT, not a panic or a silent
    /// no-op — the dispatcher needs a target. `dry_run` keeps the repo-config
    /// bootstrap from reaching the network in this unit test.
    #[test]
    fn update_with_no_target_is_invalid_argument() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let c = ctx(tmp.path().to_path_buf(), InstallMode::System, true);
        let args = UpdateArgs {
            component: None,
            command: None,
        };
        let err = handle(args, &c).expect_err("must require a target");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(err.reason().contains("specify a component"));
    }
}
