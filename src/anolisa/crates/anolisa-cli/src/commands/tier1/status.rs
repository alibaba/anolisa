//! `anolisa status [COMPONENT]` — read-only view of installed components.
//!
//! Reads `installed.toml` via the shared [`crate::commands::common`] helper
//! and lists every `Component`-kind object, or filters down to a single
//! name. A missing state file is the expected fresh-install case and yields
//! an empty result; an unknown component name surfaces a synthetic
//! `not_installed` record rather than an error (launch spec §7.1).
//!
//! This handler does NOT consult the resolver — it reports state-on-disk
//! plus live read-only probes. Every persisted field in [`ComponentRecord`]
//! is projected straight from [`InstalledObject`]; the only synthesized data
//! are the integrity and manifest health entries layered on top.

use chrono::{SecondsFormat, Utc};
use clap::Parser;
use serde::Serialize;

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anolisa_core::path_safety::{PathBoundaryError, validate_owned_path};
use anolisa_core::{
    Catalog, HealthEntry, HealthSpec, InstalledObject, InstalledState, IntegrityStatus, ObjectKind,
    ServiceState, check_owned_file, service_for_install_mode as service_factory,
};

/// Wall-clock ceiling for a single manifest command-kind probe. `status`
/// is read-only; a hostile or buggy probe must not be able to hang the
/// CLI. 5s is generous for the smoke-test probes the spec describes
/// (`<bindir>/agentsight --help`) while keeping the worst case bounded.
const COMMAND_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Polling cadence for the command-probe wait loop. Mirrors the hook
/// runner — sub-second responsiveness for fast probes without burning
/// CPU.
const COMMAND_PROBE_POLL: Duration = Duration::from_millis(25);

/// Single-character glyphs that turn the command string into a shell
/// expression. Manifest probes are validated *not* to contain any of
/// these — we never run them through `sh -c`, so anything that requires
/// a shell to interpret is, by definition, not a valid probe.
const SHELL_METACHARS: &[char] = &[
    ';', '|', '&', '>', '<', '$', '`', '\\', '{', '}', '(', ')', '*', '?', '~', '!', '\n', '\r',
    '\'', '"',
];
use anolisa_env::EnvService;
use anolisa_platform::fs_layout::FsLayout;

use crate::color::{Palette, pad_right};
use crate::commands::common;
use crate::context::CliContext;
use crate::response::{CliError, render_json};

const COMMAND: &str = "status";

#[derive(Parser)]
pub struct StatusArgs {
    /// Show detail for a specific component (omit for aggregate view).
    pub component: Option<String>,
}

/// JSON-shaped record for a single component, used in both the wire
/// envelope and the human renderer. Fields are projected straight from
/// the matching [`InstalledObject`] on disk; optional/empty fields are
/// skipped when absent so synthetic `not_installed` records stay compact.
#[derive(Debug, Serialize, PartialEq, Eq)]
struct ComponentRecord {
    name: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    installed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_operation_id: Option<String>,
    /// Feature flags the install record marks as enabled.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    enabled_features: Vec<String>,
    /// Last-known health probe entries persisted in state. Empty until a
    /// background probe wires up — but still surfaced verbatim today so
    /// users see whatever the install runner recorded.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    health: Vec<HealthEntry>,
}

pub fn handle(args: StatusArgs, ctx: &CliContext) -> Result<(), CliError> {
    let state = common::load_installed_state(ctx, COMMAND)?;
    let layout = common::resolve_layout(ctx);
    // Catalog is best-effort: if manifests are missing or malformed, status
    // still reports state-on-disk plus the integrity probe. The manifest
    // health checks layer is purely additive — never an error path that
    // would mask a working install.
    let catalog = common::load_bundled_catalog(ctx, COMMAND).ok();
    let install_mode = ctx.install_mode.as_str();
    let records = select_components(
        &state,
        &layout,
        catalog.as_ref(),
        install_mode,
        args.component.as_deref(),
    );

    if ctx.json {
        let data = serde_json::json!({ "components": records });
        return render_json(COMMAND, data);
    }

    if !ctx.quiet {
        render_human(&records, ctx.verbose, ctx.no_color);
    }
    Ok(())
}

/// Pure selector: project [`InstalledState`] down to component records,
/// optionally filtered to a single name. Extracted so tests can exercise
/// the filtering/synthetic-not-installed logic without mocking
/// `CliContext` or touching the filesystem.
fn select_components(
    state: &InstalledState,
    layout: &FsLayout,
    catalog: Option<&Catalog>,
    install_mode: &str,
    name: Option<&str>,
) -> Vec<ComponentRecord> {
    let installed: Vec<&InstalledObject> = state
        .objects
        .iter()
        .filter(|o| o.kind == ObjectKind::Component)
        .collect();

    match name {
        None => installed
            .iter()
            .map(|o| record_from_object(layout, catalog, install_mode, o))
            .collect(),
        Some(target) => match installed.iter().find(|o| o.name == target) {
            Some(obj) => vec![record_from_object(layout, catalog, install_mode, obj)],
            None => vec![ComponentRecord {
                name: target.to_string(),
                status: "not_installed".to_string(),
                version: None,
                installed_at: None,
                last_operation_id: None,
                enabled_features: Vec::new(),
                health: Vec::new(),
            }],
        },
    }
}

fn record_from_object(
    layout: &FsLayout,
    catalog: Option<&Catalog>,
    install_mode: &str,
    obj: &InstalledObject,
) -> ComponentRecord {
    // Start from the state's last-known health entries, then layer the
    // live integrity probe on top. The integrity probe is authoritative
    // for owned-file existence and sha256; it can escalate the wire
    // status from `installed` to `degraded` or `failed` without us
    // touching the on-disk state.
    let base_status = common::object_status_str(obj.status).to_string();
    let mut health = obj.health.clone();
    let (integrity_entries, integrity_status) = integrity_probe(layout, obj, &base_status);
    health.extend(integrity_entries);

    // Layer manifest-driven health checks on top. Each entry can escalate
    // the wire status independently of integrity (a missing service unit
    // can fail an otherwise-clean install). Probes are skipped when no
    // catalog is loaded — fresh checkouts without a packaged catalog still
    // get integrity-only behavior.
    let manifest_status = if let Some(cat) = catalog {
        let (manifest_entries, escalated) =
            manifest_health_probe(layout, cat, install_mode, obj, &integrity_status);
        health.extend(manifest_entries);
        escalated
    } else {
        integrity_status
    };

    ComponentRecord {
        name: obj.name.clone(),
        status: manifest_status,
        version: Some(obj.version.clone()),
        installed_at: Some(obj.installed_at.clone()),
        last_operation_id: obj.last_operation_id.clone(),
        enabled_features: obj.enabled_features.clone(),
        health,
    }
}

/// Probe the integrity of every file owned by `component` and return
/// synthesized [`HealthEntry`] items plus the (possibly escalated) wire
/// status label.
///
/// Escalation rules (only move toward more-broken, never back):
/// - any [`IntegrityStatus::is_failure`] result → `"failed"`
/// - any [`IntegrityStatus::Unverified`] result on an otherwise-clean
///   component → `"degraded"`
/// - otherwise the base status (`installed`/`disabled`/etc) is preserved
///
/// Status is left untouched when the component is already `disabled`
/// or `not_installed`: probing a disabled component and demoting it
/// to `degraded` would be a regression in the meaning of `disabled`.
fn integrity_probe(
    layout: &FsLayout,
    component: &InstalledObject,
    base_status: &str,
) -> (Vec<HealthEntry>, String) {
    let mut entries: Vec<HealthEntry> = Vec::new();
    let mut had_failure = false;
    let mut had_unverified = false;
    let checked_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);

    for file in &component.files {
        let result = check_owned_file(layout, file);
        if result == IntegrityStatus::Skipped {
            continue;
        }
        if result.is_failure() {
            had_failure = true;
        } else if matches!(result, IntegrityStatus::Unverified) {
            had_unverified = true;
        }
        entries.push(HealthEntry {
            name: format!("integrity:{}", file.path.display()),
            status: result.label().to_string(),
            checked_at: checked_at.clone(),
            reason: None,
        });
    }

    // Only escalate from "installed"/"adopted" — keep "disabled"/"failed"
    // as-is so a disabled component does not get demoted by a stale
    // sha256 mismatch on disk.
    let escalated = match base_status {
        "installed" | "adopted" if had_failure => "failed".to_string(),
        "installed" | "adopted" if had_unverified => "degraded".to_string(),
        _ => base_status.to_string(),
    };
    (entries, escalated)
}

/// Look up the component's manifest in the layered catalog and run each
/// declared `[[health_checks]]` entry. Three kinds are supported today
/// (file/command/systemd); unknown kinds are reported verbatim with
/// `status = "unsupported_kind"` so a future probe doesn't get silently
/// dropped.
///
/// Escalation rules (status moves only toward more-broken):
/// - required check fails → `"failed"`
/// - optional check fails → `"degraded"`
/// - service backend not supported (user mode, container, non-Linux) → entry
///   marked `"not_supported"` and degrades to `"degraded"` (we can't prove
///   the unit is up, but we have no positive failure either)
/// - on `"disabled"`/`"failed"`/`"not_installed"` the wire status is left
///   alone — the same rationale as integrity_probe.
fn manifest_health_probe(
    layout: &FsLayout,
    catalog: &Catalog,
    install_mode: &str,
    component: &InstalledObject,
    base_status: &str,
) -> (Vec<HealthEntry>, String) {
    let mut entries: Vec<HealthEntry> = Vec::new();
    let mut had_failure = false;
    let mut had_degrade = false;
    let checked_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);

    // Lazily build the service manager — most checks won't need it, and
    // a `EnvService::detect()` call in user mode shells out to `uname -r`.
    let mut service_manager: Option<Box<dyn anolisa_core::ServiceManager>> = None;

    // No manifest in the catalog — silent. This is the out-of-tree
    // component case (the alpha contract); the integrity probe already
    // covers everything the state file knows about.
    if let Some(manifest) = catalog.component(&component.name) {
        for check in &manifest.health_checks {
            let optional = check.optional.unwrap_or(false);
            let entry_name = format!(
                "{}:{}:{}",
                component.name,
                check.kind,
                check.name.as_deref().unwrap_or("(unnamed)")
            );

            let outcome = match check.kind.as_str() {
                "file" => probe_file_check(layout, check),
                "command" => probe_command_check(layout, check),
                "systemd" => {
                    if service_manager.is_none() {
                        let env = EnvService::detect();
                        service_manager = Some(service_factory(install_mode, &env));
                    }
                    let mgr = service_manager.as_deref().expect("service manager built");
                    probe_systemd_check(mgr, check)
                }
                other => HealthOutcome {
                    label: "unsupported_kind".to_string(),
                    reason: Some(format!("manifest health kind '{other}' is not supported")),
                    state: HealthCheckState::Unsupported,
                },
            };

            match outcome.state {
                HealthCheckState::Ok => {}
                HealthCheckState::Unsupported => {
                    had_degrade = true;
                }
                HealthCheckState::Failed if optional => {
                    had_degrade = true;
                }
                HealthCheckState::Failed => {
                    had_failure = true;
                }
            }
            entries.push(HealthEntry {
                name: entry_name,
                status: outcome.label,
                checked_at: checked_at.clone(),
                reason: outcome.reason,
            });
        }
    }

    let escalated = match base_status {
        "installed" | "adopted" if had_failure => "failed".to_string(),
        "installed" | "adopted" if had_degrade => "degraded".to_string(),
        // Already escalated by the integrity probe — preserve "failed" /
        // "degraded" rather than letting a manifest "ok" downgrade it.
        _ => base_status.to_string(),
    };
    (entries, escalated)
}

#[derive(Debug)]
enum HealthCheckState {
    Ok,
    Failed,
    Unsupported,
}

#[derive(Debug)]
struct HealthOutcome {
    label: String,
    reason: Option<String>,
    state: HealthCheckState,
}

/// Resolve `{bindir}` / `{datadir}` / `{etcdir}` placeholders in a
/// manifest path. Manifests are written against logical roots so the
/// same string works in system and user mode; the layout supplies the
/// concrete path for the active install mode.
fn expand_layout_placeholders(input: &str, layout: &FsLayout) -> String {
    input
        .replace("{bindir}", &layout.bin_dir.display().to_string())
        .replace("{datadir}", &layout.datadir.display().to_string())
        .replace("{etcdir}", &layout.etc_dir.display().to_string())
        .replace("{statedir}", &layout.state_dir.display().to_string())
}

/// File-kind probe with two security guards layered on top of the
/// "does this file exist" question:
///
///   1. `validate_owned_path` — a manifest pointing the probe at
///      `/etc/passwd` or a `..`-traversal must NOT trigger a stat that
///      could leak existence to a passive attacker via timing or
///      surface a sensitive file in the wire output. External paths
///      degrade to `out_of_bounds` / `Unsupported` so the component
///      goes `degraded` (not `failed`) — the manifest is misauthored,
///      not the install.
///   2. `symlink_metadata` instead of `Path::exists()` — `exists()`
///      follows symlinks, which means a manifest whose `probe` resolves
///      to a path under `bin_dir` could still have someone plant
///      `<bin_dir>/probe -> /etc/shadow` and turn `status` into a
///      symlink-follow primitive. We treat symlinks themselves as
///      `unsupported_target` so probes have to be authored against
///      real files.
fn probe_file_check(layout: &FsLayout, spec: &HealthSpec) -> HealthOutcome {
    let raw = spec
        .probe
        .as_deref()
        .or(spec.command.as_deref())
        .unwrap_or("");
    if raw.is_empty() {
        return HealthOutcome {
            label: "invalid_check".to_string(),
            reason: Some("file check missing 'probe' (or 'command') path".to_string()),
            state: HealthCheckState::Failed,
        };
    }
    let expanded = expand_layout_placeholders(raw, layout);
    let path = std::path::Path::new(&expanded);
    if let Err(err) = validate_owned_path(layout, path) {
        return HealthOutcome {
            label: "out_of_bounds".to_string(),
            reason: Some(format!(
                "manifest probe path '{expanded}' rejected: {}",
                boundary_reason(&err)
            )),
            state: HealthCheckState::Unsupported,
        };
    }
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => HealthOutcome {
            label: "unsupported_target".to_string(),
            reason: Some(format!(
                "manifest probe path '{expanded}' is a symlink — refusing to follow"
            )),
            state: HealthCheckState::Unsupported,
        },
        Ok(meta) if !meta.file_type().is_file() => HealthOutcome {
            // Non-regular targets (directory, fifo, socket, char/block
            // device) cannot honestly satisfy a `kind = "file"` check.
            // Returning `ok` for a directory turned a misauthored
            // manifest into a silent green light; surfacing
            // `not_regular_file` makes the manifest bug visible in the
            // wire output without escalating the component to `failed`
            // (the install itself is fine — the manifest is wrong).
            label: "not_regular_file".to_string(),
            reason: Some(format!(
                "manifest probe path '{expanded}' is not a regular file"
            )),
            state: HealthCheckState::Unsupported,
        },
        Ok(_) => HealthOutcome {
            label: "ok".to_string(),
            reason: Some(format!("file present at {expanded}")),
            state: HealthCheckState::Ok,
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => HealthOutcome {
            label: "missing_file".to_string(),
            reason: Some(format!("expected file not present at {expanded}")),
            state: HealthCheckState::Failed,
        },
        Err(err) => HealthOutcome {
            label: "stat_error".to_string(),
            reason: Some(format!("stat failed for '{expanded}': {err}")),
            state: HealthCheckState::Failed,
        },
    }
}

fn boundary_reason(err: &PathBoundaryError) -> String {
    match err {
        PathBoundaryError::External { path } => {
            format!("'{}' is outside ANOLISA-owned roots", path.display())
        }
        PathBoundaryError::Traversal { path } => {
            format!("'{}' contains '.' or '..' segments", path.display())
        }
    }
}

/// Command-kind probe, hardened against the two attack surfaces a naïve
/// `sh -c <manifest_string>` exposes:
///
///   1. **Arbitrary shell**: the probe runs *without* a shell. We split
///      the command string on ASCII whitespace and run the first token
///      as the executable, the rest as argv. Anything containing shell
///      metacharacters (`;|&><$\` etc.) is refused — `status` is a
///      read-only verb and must not let a third-party manifest run a
///      pipeline, redirect to a file, or expand variables.
///   2. **Arbitrary executable**: the executable must be an absolute
///      path under an ANOLISA-owned root (after `{bindir}` placeholder
///      expansion). A manifest that probes via `/usr/bin/curl` or a
///      bare `true` is refused — the only commands `status` may run
///      are ones the framework itself shipped + path-safety vetted.
///   3. **Hang**: the spawned child is bounded by `COMMAND_PROBE_TIMEOUT`.
///      A runaway probe is killed and the entry surfaces as `timeout`,
///      escalating to `degraded` so the wire status reflects "couldn't
///      verify" rather than the misleading "passed".
fn probe_command_check(layout: &FsLayout, spec: &HealthSpec) -> HealthOutcome {
    let raw = spec.command.as_deref().unwrap_or("");
    if raw.is_empty() {
        return HealthOutcome {
            label: "invalid_check".to_string(),
            reason: Some("command check missing 'command' string".to_string()),
            state: HealthCheckState::Failed,
        };
    }
    let expanded = expand_layout_placeholders(raw, layout);

    if let Some(meta) = expanded.chars().find(|c| SHELL_METACHARS.contains(c)) {
        return HealthOutcome {
            label: "invalid_check".to_string(),
            reason: Some(format!(
                "manifest probe '{expanded}' contains shell metacharacter '{meta}' — \
                 commands run without a shell, declare a single executable + plain args",
            )),
            state: HealthCheckState::Unsupported,
        };
    }

    let mut tokens = expanded.split_ascii_whitespace();
    let exe = match tokens.next() {
        Some(e) => e,
        None => {
            return HealthOutcome {
                label: "invalid_check".to_string(),
                reason: Some("manifest probe is empty after placeholder expansion".to_string()),
                state: HealthCheckState::Failed,
            };
        }
    };
    let args: Vec<&str> = tokens.collect();

    let exe_path = std::path::Path::new(exe);
    if !exe_path.is_absolute() {
        return HealthOutcome {
            label: "out_of_bounds".to_string(),
            reason: Some(format!(
                "manifest probe executable '{exe}' is not absolute — declare \
                 the full `{{bindir}}/...` path",
            )),
            state: HealthCheckState::Unsupported,
        };
    }
    if let Err(err) = validate_owned_path(layout, exe_path) {
        return HealthOutcome {
            label: "out_of_bounds".to_string(),
            reason: Some(format!(
                "manifest probe executable '{exe}' rejected: {}",
                boundary_reason(&err)
            )),
            state: HealthCheckState::Unsupported,
        };
    }

    let mut child = match Command::new(exe_path)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(err) => {
            return HealthOutcome {
                label: "command_error".to_string(),
                reason: Some(format!("failed to spawn '{expanded}': {err}")),
                state: HealthCheckState::Failed,
            };
        }
    };

    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    return HealthOutcome {
                        label: "ok".to_string(),
                        reason: Some(format!("`{expanded}` exited 0")),
                        state: HealthCheckState::Ok,
                    };
                }
                return HealthOutcome {
                    label: "command_failed".to_string(),
                    reason: Some(format!(
                        "`{expanded}` exited with status {}",
                        status
                            .code()
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| "signal".to_string()),
                    )),
                    state: HealthCheckState::Failed,
                };
            }
            Ok(None) => {
                if started.elapsed() > COMMAND_PROBE_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    return HealthOutcome {
                        label: "timeout".to_string(),
                        reason: Some(format!(
                            "`{expanded}` exceeded {}s probe timeout",
                            COMMAND_PROBE_TIMEOUT.as_secs(),
                        )),
                        state: HealthCheckState::Unsupported,
                    };
                }
                std::thread::sleep(COMMAND_PROBE_POLL);
            }
            Err(err) => {
                return HealthOutcome {
                    label: "command_error".to_string(),
                    reason: Some(format!("wait failed for '{expanded}': {err}")),
                    state: HealthCheckState::Failed,
                };
            }
        }
    }
}

fn probe_systemd_check(
    manager: &dyn anolisa_core::ServiceManager,
    spec: &HealthSpec,
) -> HealthOutcome {
    let unit = match spec.unit.as_deref() {
        Some(u) if !u.is_empty() => u,
        _ => {
            return HealthOutcome {
                label: "invalid_check".to_string(),
                reason: Some("systemd check missing 'unit' name".to_string()),
                state: HealthCheckState::Failed,
            };
        }
    };
    if !manager.supported() {
        let reason = manager
            .unsupported_reason()
            .unwrap_or("service manager not supported in this environment")
            .to_string();
        return HealthOutcome {
            label: "not_supported".to_string(),
            reason: Some(reason),
            state: HealthCheckState::Unsupported,
        };
    }
    match manager.probe_service(unit) {
        Ok(outcome) => match outcome.state {
            ServiceState::Active => HealthOutcome {
                label: "ok".to_string(),
                reason: Some(format!("unit '{unit}' is active")),
                state: HealthCheckState::Ok,
            },
            ServiceState::NotInstalled => HealthOutcome {
                label: "not_installed".to_string(),
                reason: Some(format!("unit '{unit}' is not installed")),
                state: HealthCheckState::Failed,
            },
            ServiceState::NotSupported => HealthOutcome {
                label: "not_supported".to_string(),
                reason: outcome
                    .message
                    .is_empty()
                    .then(|| "service manager unsupported".to_string())
                    .or(Some(outcome.message.clone())),
                state: HealthCheckState::Unsupported,
            },
            other => HealthOutcome {
                label: other.as_str().to_string(),
                reason: Some(format!("unit '{unit}' state '{}'", other.as_str())),
                state: HealthCheckState::Failed,
            },
        },
        Err(err) => HealthOutcome {
            label: "probe_error".to_string(),
            reason: Some(format!("probe failed for '{unit}': {err}")),
            state: HealthCheckState::Failed,
        },
    }
}

fn render_human(records: &[ComponentRecord], verbose: bool, no_color: bool) {
    let color = Palette::new(no_color);
    if records.is_empty() {
        println!("{}", color.muted("no installed components"));
        return;
    }

    println!(
        "{}",
        color.header(format!(
            "{:<28}  {:<14}  {:<10}  {}",
            "NAME", "STATUS", "VERSION", "INSTALLED_AT"
        ))
    );
    for record in records {
        let version = record.version.as_deref().unwrap_or("-");
        let installed_at = record.installed_at.as_deref().unwrap_or("-");
        println!(
            "{name:<28}  {status:<14}  {version:<10}  {installed_at}",
            name = record.name,
            status = color.status(pad_right(&record.status, 14)),
            version = version,
            installed_at = color.muted(installed_at),
        );
        if verbose {
            if let Some(op) = record.last_operation_id.as_deref() {
                println!("    {} {}", color.label("last_operation_id:"), color.id(op));
            }
            if !record.enabled_features.is_empty() {
                println!(
                    "    {} {}",
                    color.label("enabled_features:"),
                    record.enabled_features.join(", ")
                );
            }
            for entry in &record.health {
                println!(
                    "    {} {} @ {}",
                    color.label(format!("health[{}]:", entry.name)),
                    color.status(&entry.status),
                    color.muted(&entry.checked_at)
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anolisa_core::{
        FileOwner, HealthEntry, InstalledObject, InstalledState, ObjectKind, ObjectStatus,
        OwnedFile, SubscriptionScope,
    };
    use std::path::{Path, PathBuf};

    /// Build a system-mode FsLayout rooted under `prefix` and pre-create
    /// `bin_dir` so the path-safety guard in [`anolisa_core::check_owned_file`]
    /// has a canonical root to anchor on. Tests place owned files under
    /// `layout.bin_dir` to stay inside the ANOLISA-owned roots.
    fn test_layout(prefix: &Path) -> FsLayout {
        let layout = FsLayout::system(Some(prefix.to_path_buf()));
        std::fs::create_dir_all(&layout.bin_dir).expect("mkdir bin_dir");
        layout
    }

    /// Convenience for tests that don't exercise integrity at all — they
    /// only care about projection/filtering and the layout will never be
    /// touched. Uses a throwaway prefix that we don't bother creating.
    fn dummy_layout() -> FsLayout {
        FsLayout::system(Some(PathBuf::from("/tmp/anolisa-status-tests-noop")))
    }

    /// Baseline component install record. Owned `files` default to empty
    /// so projection-only tests never touch the filesystem; integrity
    /// tests attach files explicitly before upserting.
    fn component_object(name: &str, version: &str, status: ObjectStatus) -> InstalledObject {
        InstalledObject {
            kind: ObjectKind::Component,
            name: name.to_string(),
            version: version.to_string(),
            status,
            manifest_digest: Some("sha256:abc".to_string()),
            distribution_source: Some("builtin".to_string()),
            install_backend: None,
            installed_at: "2026-06-01T10:00:00Z".to_string(),
            last_operation_id: Some("op-20260601-001".to_string()),
            managed: true,
            adopted: false,
            subscription_scope: SubscriptionScope::None,
            enabled_features: Vec::new(),
            component_refs: Vec::new(),
            files: Vec::new(),
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
        }
    }

    /// A missing `installed.toml` is the fresh-install case and must
    /// surface as an empty result, not an error. Verifies the helper
    /// stack (`InstalledState::load` -> `select_components`) collapses
    /// "no file" to "no components".
    #[test]
    fn missing_state_file_yields_empty_result() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("installed.toml");
        let state = InstalledState::load(&path).expect("missing file is not an error");
        let records = select_components(&state, &dummy_layout(), None, "system", None);
        assert!(records.is_empty());
    }

    #[test]
    fn unfiltered_listing_returns_all_components() {
        let mut state = InstalledState::default();
        state.upsert_object(component_object(
            "agentsight",
            "0.1.0",
            ObjectStatus::Installed,
        ));
        state.upsert_object(component_object(
            "tokenless",
            "0.2.0",
            ObjectStatus::Partial,
        ));

        let records = select_components(&state, &dummy_layout(), None, "system", None);
        assert_eq!(records.len(), 2);
        let names: Vec<&str> = records.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"agentsight"));
        assert!(names.contains(&"tokenless"));
        // Partial maps to the wire-friendly `degraded` label.
        let tokenless = records
            .iter()
            .find(|r| r.name == "tokenless")
            .expect("present");
        assert_eq!(tokenless.status, "degraded");
    }

    #[test]
    fn filter_miss_yields_synthetic_not_installed_record() {
        let mut state = InstalledState::default();
        state.upsert_object(component_object(
            "agentsight",
            "0.1.0",
            ObjectStatus::Installed,
        ));

        let records = select_components(&state, &dummy_layout(), None, "system", Some("ws-ckpt"));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].name, "ws-ckpt");
        assert_eq!(records[0].status, "not_installed");
        assert!(records[0].version.is_none());
        assert!(records[0].installed_at.is_none());
        assert!(records[0].last_operation_id.is_none());
        assert!(records[0].enabled_features.is_empty());
    }

    #[test]
    fn filter_hit_returns_stored_record() {
        let mut state = InstalledState::default();
        // No owned files -> integrity probe is a no-op so the
        // state-projected record passes through clean.
        let mut obj = component_object("agentsight", "0.3.1", ObjectStatus::Installed);
        obj.enabled_features = vec!["bpf-events".to_string()];
        obj.health = vec![HealthEntry {
            name: "binary".to_string(),
            status: "ok".to_string(),
            checked_at: "2026-06-01T10:01:00Z".to_string(),
            reason: None,
        }];
        state.upsert_object(obj);

        let records =
            select_components(&state, &dummy_layout(), None, "system", Some("agentsight"));
        assert_eq!(records.len(), 1);
        let only = &records[0];
        assert_eq!(only.name, "agentsight");
        assert_eq!(only.status, "installed");
        assert_eq!(only.version.as_deref(), Some("0.3.1"));
        assert_eq!(only.installed_at.as_deref(), Some("2026-06-01T10:00:00Z"));
        assert_eq!(only.last_operation_id.as_deref(), Some("op-20260601-001"));
        // State-projected fields must reach the wire record verbatim.
        assert_eq!(only.enabled_features, vec!["bpf-events"]);
        assert_eq!(only.health.len(), 1);
        assert_eq!(only.health[0].name, "binary");
        assert_eq!(only.health[0].status, "ok");
    }

    /// Component whose owned files are all present on disk with matching
    /// sha256 stays `installed` and the wire record gains one
    /// `integrity:<path>` health entry per file with `status = "ok"`.
    #[test]
    fn integrity_probe_present_file_with_matching_sha_keeps_installed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());
        let file_path = layout.bin_dir.join("agentsight");
        std::fs::write(&file_path, b"payload").expect("write");

        let mut state = InstalledState::default();
        let mut comp = component_object("agentsight", "0.1.0", ObjectStatus::Installed);
        comp.files = vec![OwnedFile {
            path: file_path.clone(),
            owner: FileOwner::Anolisa,
            sha256: Some(
                "239f59ed55e737c77147cf55ad0c1b030b6d7ee748a7426952f9b852d5a935e5".to_string(),
            ),
        }];
        state.upsert_object(comp);

        let records = select_components(&state, &layout, None, "system", Some("agentsight"));
        let only = &records[0];
        assert_eq!(only.status, "installed");
        // Exactly one integrity entry, status "ok", with the path in the name.
        let integrity: Vec<&HealthEntry> = only
            .health
            .iter()
            .filter(|h| h.name.starts_with("integrity:"))
            .collect();
        assert_eq!(integrity.len(), 1);
        assert_eq!(integrity[0].status, "ok");
        assert!(integrity[0].name.ends_with("agentsight"));
    }

    /// Missing owned file on disk escalates the component status to
    /// `"failed"` and emits a `missing_file` health entry. The original
    /// `installed` ObjectStatus is NOT mutated — escalation is purely
    /// at the wire layer (`status` field).
    #[test]
    fn integrity_probe_missing_file_escalates_to_failed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());
        let missing_path = layout.bin_dir.join("anolisa-integrity-missing");

        let mut state = InstalledState::default();
        let mut comp = component_object("agentsight", "0.1.0", ObjectStatus::Installed);
        comp.files = vec![OwnedFile {
            path: missing_path,
            owner: FileOwner::Anolisa,
            sha256: Some("deadbeef".to_string()),
        }];
        state.upsert_object(comp);

        let records = select_components(&state, &layout, None, "system", Some("agentsight"));
        let only = &records[0];
        assert_eq!(only.status, "failed", "missing file -> failed");
        let integrity = only
            .health
            .iter()
            .find(|h| h.name.starts_with("integrity:"))
            .expect("integrity entry present");
        assert_eq!(integrity.status, "missing_file");
    }

    /// Tampered file (sha256 mismatch) escalates to `"failed"` and
    /// emits a `sha256_mismatch` health entry — distinct from
    /// `missing_file` so the user can tell which kind of drift occurred.
    #[test]
    fn integrity_probe_sha_mismatch_escalates_to_failed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());
        let file_path = layout.bin_dir.join("agentsight");
        std::fs::write(&file_path, b"tampered-payload").expect("write");

        let mut state = InstalledState::default();
        let mut comp = component_object("agentsight", "0.1.0", ObjectStatus::Installed);
        comp.files = vec![OwnedFile {
            path: file_path,
            owner: FileOwner::Anolisa,
            sha256: Some(
                "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
            ),
        }];
        state.upsert_object(comp);

        let records = select_components(&state, &layout, None, "system", Some("agentsight"));
        let only = &records[0];
        assert_eq!(only.status, "failed", "sha mismatch -> failed");
        let integrity = only
            .health
            .iter()
            .find(|h| h.name.starts_with("integrity:"))
            .expect("integrity entry present");
        assert_eq!(integrity.status, "sha256_mismatch");
    }

    /// File exists but no sha256 was recorded -> degrade (not fail). We
    /// can't prove tampering either way; "degraded" signals the user
    /// should treat the install with skepticism without claiming it's
    /// broken.
    #[test]
    fn integrity_probe_unverified_file_degrades_status() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());
        let file_path = layout.bin_dir.join("agentsight");
        std::fs::write(&file_path, b"payload").expect("write");

        let mut state = InstalledState::default();
        let mut comp = component_object("agentsight", "0.1.0", ObjectStatus::Installed);
        comp.files = vec![OwnedFile {
            path: file_path,
            owner: FileOwner::Anolisa,
            sha256: None,
        }];
        state.upsert_object(comp);

        let records = select_components(&state, &layout, None, "system", Some("agentsight"));
        let only = &records[0];
        assert_eq!(only.status, "degraded");
        let integrity = only
            .health
            .iter()
            .find(|h| h.name.starts_with("integrity:"))
            .expect("integrity entry present");
        assert_eq!(integrity.status, "unverified");
    }

    /// A disabled component MUST stay disabled even if its owned files
    /// are gone — `disabled` is a deliberate state set by the user, not
    /// a drift signal we should overwrite from a sha probe.
    #[test]
    fn integrity_probe_does_not_escalate_disabled_component() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());
        let missing_path = layout.bin_dir.join("anolisa-integrity-still-disabled");

        let mut state = InstalledState::default();
        let mut comp = component_object("agentsight", "0.1.0", ObjectStatus::Disabled);
        comp.files = vec![OwnedFile {
            path: missing_path,
            owner: FileOwner::Anolisa,
            sha256: Some("deadbeef".to_string()),
        }];
        state.upsert_object(comp);

        let records = select_components(&state, &layout, None, "system", Some("agentsight"));
        let only = &records[0];
        assert_eq!(only.status, "disabled");
        // The integrity entry is still surfaced so users can see the drift,
        // even though the wire status stays disabled.
        let integrity = only
            .health
            .iter()
            .find(|h| h.name.starts_with("integrity:"))
            .expect("integrity entry present");
        assert_eq!(integrity.status, "missing_file");
    }

    /// A forged `installed.toml` pointing an `owner = anolisa` file at a
    /// path outside the ANOLISA-owned roots must be refused by `status`
    /// without any stat or read happening. We point at `/etc/shadow` —
    /// if the path-safety guard fell through, integrity would either
    /// open the file (worst case) or report `MissingFile` on a host where
    /// it doesn't exist. `out_of_bounds` is the only status that proves
    /// the guard fired before IO.
    #[test]
    fn integrity_probe_refuses_path_outside_owned_roots() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());

        let mut state = InstalledState::default();
        let mut comp = component_object("agentsight", "0.1.0", ObjectStatus::Installed);
        comp.files = vec![OwnedFile {
            path: PathBuf::from("/etc/shadow"),
            owner: FileOwner::Anolisa,
            sha256: Some("deadbeef".to_string()),
        }];
        state.upsert_object(comp);

        let records = select_components(&state, &layout, None, "system", Some("agentsight"));
        let only = &records[0];
        assert_eq!(only.status, "failed", "out-of-bounds path -> failed");
        let integrity = only
            .health
            .iter()
            .find(|h| h.name.starts_with("integrity:"))
            .expect("integrity entry present");
        assert_eq!(
            integrity.status, "out_of_bounds",
            "path-safety guard must fire before any stat",
        );
    }

    // -----------------------------------------------------------------
    // Manifest health probe tests
    // -----------------------------------------------------------------

    /// Build a temporary catalog with a single component manifest under
    /// `runtime/<name>.toml`. Returns the Catalog plus the tempdir guard
    /// (dropping the guard wipes the manifests).
    fn catalog_with_component(
        name: &str,
        component_toml: &str,
    ) -> (anolisa_core::Catalog, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let runtime_dir = tmp.path().join("runtime");
        std::fs::create_dir_all(&runtime_dir).expect("mkdir runtime");
        std::fs::write(runtime_dir.join(format!("{name}.toml")), component_toml)
            .expect("write component manifest");
        let catalog = anolisa_core::Catalog::load(anolisa_core::CatalogLayers::bundled_only(
            tmp.path().to_path_buf(),
        ))
        .expect("catalog loads");
        (catalog, tmp)
    }

    /// File-kind health check pointing at an existing file emits an `ok`
    /// entry with the path in the reason and leaves the wire status at
    /// `installed`.
    #[test]
    fn manifest_health_file_check_ok() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());
        let probe_path = layout.bin_dir.join("agentsight");
        std::fs::write(&probe_path, b"binary").expect("write probe binary");

        let manifest = format!(
            r#"
            [component]
            name = "agentsight"
            version = "0.1.0"

            [[health_checks]]
            name = "binary"
            kind = "file"
            probe = "{}"
        "#,
            probe_path.display()
        );
        let (catalog, _guard) = catalog_with_component("agentsight", &manifest);

        let mut state = InstalledState::default();
        state.upsert_object(component_object(
            "agentsight",
            "0.1.0",
            ObjectStatus::Installed,
        ));

        let records = select_components(
            &state,
            &layout,
            Some(&catalog),
            "system",
            Some("agentsight"),
        );
        let only = &records[0];
        assert_eq!(only.status, "installed");
        let entry = only
            .health
            .iter()
            .find(|h| h.name == "agentsight:file:binary")
            .expect("file health entry present");
        assert_eq!(entry.status, "ok");
        assert!(
            entry
                .reason
                .as_deref()
                .unwrap_or("")
                .contains("file present"),
            "reason mentions presence: {:?}",
            entry.reason
        );
    }

    /// Required (default) file-kind check on a missing file escalates to
    /// `failed` and emits a `missing_file` entry with a reason.
    #[test]
    fn manifest_health_file_check_required_missing_fails() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());
        let probe_path = layout.bin_dir.join("ghost-binary");

        let manifest = format!(
            r#"
            [component]
            name = "agentsight"
            version = "0.1.0"

            [[health_checks]]
            name = "binary"
            kind = "file"
            probe = "{}"
        "#,
            probe_path.display()
        );
        let (catalog, _guard) = catalog_with_component("agentsight", &manifest);

        let mut state = InstalledState::default();
        state.upsert_object(component_object(
            "agentsight",
            "0.1.0",
            ObjectStatus::Installed,
        ));

        let records = select_components(
            &state,
            &layout,
            Some(&catalog),
            "system",
            Some("agentsight"),
        );
        let only = &records[0];
        assert_eq!(only.status, "failed", "required missing file -> failed");
        let entry = only
            .health
            .iter()
            .find(|h| h.name == "agentsight:file:binary")
            .expect("file health entry present");
        assert_eq!(entry.status, "missing_file");
        assert!(entry.reason.is_some(), "reason must be populated");
    }

    /// Optional file-kind check on a missing file degrades (not fails).
    /// The same probe with `optional = true` must produce a degraded
    /// status — not a failure — proving the optional flag is consumed.
    #[test]
    fn manifest_health_file_check_optional_missing_degrades() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());
        let probe_path = layout.bin_dir.join("ghost-binary");

        let manifest = format!(
            r#"
            [component]
            name = "agentsight"
            version = "0.1.0"

            [[health_checks]]
            name = "binary"
            kind = "file"
            probe = "{}"
            optional = true
        "#,
            probe_path.display()
        );
        let (catalog, _guard) = catalog_with_component("agentsight", &manifest);

        let mut state = InstalledState::default();
        state.upsert_object(component_object(
            "agentsight",
            "0.1.0",
            ObjectStatus::Installed,
        ));

        let records = select_components(
            &state,
            &layout,
            Some(&catalog),
            "system",
            Some("agentsight"),
        );
        let only = &records[0];
        assert_eq!(only.status, "degraded", "optional missing file -> degraded");
        let entry = only
            .health
            .iter()
            .find(|h| h.name == "agentsight:file:binary")
            .expect("file health entry present");
        assert_eq!(entry.status, "missing_file");
    }

    /// Helper: write an executable shell script under `layout.bin_dir`.
    /// Manifest probes are required to point at executables under an
    /// ANOLISA-owned root, so tests that exercise the command path
    /// stage their probe scripts here rather than reaching for `/bin/true`.
    fn write_probe_script(layout: &FsLayout, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = layout.bin_dir.join(name);
        std::fs::write(&path, body).expect("write probe script");
        let mut perm = std::fs::metadata(&path).expect("stat").permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(&path, perm).expect("chmod");
        path
    }

    /// Command-kind check that exits 0 stays `ok`. The probe is an
    /// owned executable under `{bindir}` — the only kind of command
    /// path-safety lets us run.
    #[test]
    fn manifest_health_command_check_succeeds() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());
        let probe = write_probe_script(&layout, "probe-ok", "#!/bin/sh\nexit 0\n");

        let manifest = format!(
            r#"
            [component]
            name = "agentsight"
            version = "0.1.0"

            [[health_checks]]
            name = "self-check"
            kind = "command"
            command = "{}"
        "#,
            probe.display()
        );
        let (catalog, _guard) = catalog_with_component("agentsight", &manifest);

        let mut state = InstalledState::default();
        state.upsert_object(component_object(
            "agentsight",
            "0.1.0",
            ObjectStatus::Installed,
        ));

        let records = select_components(
            &state,
            &layout,
            Some(&catalog),
            "system",
            Some("agentsight"),
        );
        let only = &records[0];
        assert_eq!(only.status, "installed");
        let entry = only
            .health
            .iter()
            .find(|h| h.name == "agentsight:command:self-check")
            .expect("command health entry present");
        assert_eq!(entry.status, "ok");
    }

    /// Required command-kind check that exits non-zero escalates to
    /// `failed` and surfaces the exit status in the reason.
    #[test]
    fn manifest_health_command_check_failure_escalates() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());
        let probe = write_probe_script(&layout, "probe-fail", "#!/bin/sh\nexit 1\n");

        let manifest = format!(
            r#"
            [component]
            name = "agentsight"
            version = "0.1.0"

            [[health_checks]]
            name = "self-check"
            kind = "command"
            command = "{}"
        "#,
            probe.display()
        );
        let (catalog, _guard) = catalog_with_component("agentsight", &manifest);

        let mut state = InstalledState::default();
        state.upsert_object(component_object(
            "agentsight",
            "0.1.0",
            ObjectStatus::Installed,
        ));

        let records = select_components(
            &state,
            &layout,
            Some(&catalog),
            "system",
            Some("agentsight"),
        );
        let only = &records[0];
        assert_eq!(only.status, "failed");
        let entry = only
            .health
            .iter()
            .find(|h| h.name == "agentsight:command:self-check")
            .expect("command health entry present");
        assert_eq!(entry.status, "command_failed");
    }

    /// File-kind check pointing outside the ANOLISA-owned roots must be
    /// refused as `out_of_bounds` and degrade the wire status — never
    /// stat the path. The component MUST NOT escalate to `failed`
    /// (a misauthored probe is a manifest bug, not an install bug) but
    /// must surface clearly in the wire output.
    #[test]
    fn manifest_health_file_check_refuses_external_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());

        // /etc/passwd is outside every ANOLISA root regardless of host.
        let manifest = r#"
            [component]
            name = "agentsight"
            version = "0.1.0"

            [[health_checks]]
            name = "passwd"
            kind = "file"
            probe = "/etc/passwd"
        "#;
        let (catalog, _guard) = catalog_with_component("agentsight", manifest);

        let mut state = InstalledState::default();
        state.upsert_object(component_object(
            "agentsight",
            "0.1.0",
            ObjectStatus::Installed,
        ));

        let records = select_components(
            &state,
            &layout,
            Some(&catalog),
            "system",
            Some("agentsight"),
        );
        let only = &records[0];
        assert_eq!(
            only.status, "degraded",
            "external probe path -> degraded, never failed",
        );
        let entry = only
            .health
            .iter()
            .find(|h| h.name == "agentsight:file:passwd")
            .expect("file health entry present");
        assert_eq!(entry.status, "out_of_bounds");
    }

    /// File-kind check whose `probe` resolves to a symlink must NOT
    /// follow the link — `unsupported_target` is the only safe answer.
    /// Test wires a link that stays under `bin_dir` so `validate_owned_path`
    /// passes and `symlink_metadata` is the guard that fires; the more
    /// dangerous link-to-outside case is already caught by path-safety
    /// (covered by `..._refuses_external_path`).
    #[test]
    #[cfg(unix)]
    fn manifest_health_file_check_refuses_symlink_target() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());
        // Real file + sibling symlink pointing at it. Both live under
        // bin_dir, so path-safety passes; the symlink is the only thing
        // the probe should refuse.
        let real = layout.bin_dir.join("probe-target");
        std::fs::write(&real, b"binary").expect("write target");
        let link = layout.bin_dir.join("probe-link");
        symlink(&real, &link).expect("symlink");

        let manifest = format!(
            r#"
            [component]
            name = "agentsight"
            version = "0.1.0"

            [[health_checks]]
            name = "binary"
            kind = "file"
            probe = "{}"
        "#,
            link.display()
        );
        let (catalog, _guard) = catalog_with_component("agentsight", &manifest);

        let mut state = InstalledState::default();
        state.upsert_object(component_object(
            "agentsight",
            "0.1.0",
            ObjectStatus::Installed,
        ));

        let records = select_components(
            &state,
            &layout,
            Some(&catalog),
            "system",
            Some("agentsight"),
        );
        let only = &records[0];
        assert_eq!(only.status, "degraded");
        let entry = only
            .health
            .iter()
            .find(|h| h.name == "agentsight:file:binary")
            .expect("file health entry present");
        assert_eq!(entry.status, "unsupported_target");
    }

    /// File-kind check whose `probe` resolves to a directory (or any
    /// other non-regular file: fifo, socket, char/block device) must
    /// NOT return `ok`. Before the fix, `symlink_metadata` succeeded on
    /// a directory and the probe greenlit the install — turning a
    /// misauthored manifest (probe pointing at a parent directory
    /// instead of the binary) into a silent "everything is fine".
    /// `not_regular_file` makes the manifest bug visible while keeping
    /// the component `degraded` rather than `failed` (the install is
    /// fine; the manifest is wrong).
    #[test]
    fn manifest_health_file_check_refuses_directory_target() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());
        // Probe points at bin_dir itself — a real directory under an
        // ANOLISA-owned root. Path-safety passes; the regular-file
        // guard is the only thing left to refuse it.
        let target = layout.bin_dir.clone();
        std::fs::create_dir_all(&target).expect("mkdir target");

        let manifest = format!(
            r#"
            [component]
            name = "agentsight"
            version = "0.1.0"

            [[health_checks]]
            name = "binary"
            kind = "file"
            probe = "{}"
        "#,
            target.display()
        );
        let (catalog, _guard) = catalog_with_component("agentsight", &manifest);

        let mut state = InstalledState::default();
        state.upsert_object(component_object(
            "agentsight",
            "0.1.0",
            ObjectStatus::Installed,
        ));

        let records = select_components(
            &state,
            &layout,
            Some(&catalog),
            "system",
            Some("agentsight"),
        );
        let only = &records[0];
        assert_eq!(
            only.status, "degraded",
            "directory target must escalate the component to degraded, not ok",
        );
        let entry = only
            .health
            .iter()
            .find(|h| h.name == "agentsight:file:binary")
            .expect("file health entry present");
        assert_eq!(
            entry.status, "not_regular_file",
            "directory probe must surface as not_regular_file, not ok",
        );
    }

    /// Command-kind check that names a bare or PATH-resolved executable
    /// must be refused — the probe must declare an absolute path under
    /// an ANOLISA-owned root. `true` is a builtin every shell ships;
    /// the only way it would have run before this fix was through `sh -c`.
    #[test]
    fn manifest_health_command_check_refuses_bare_executable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());

        let manifest = r#"
            [component]
            name = "agentsight"
            version = "0.1.0"

            [[health_checks]]
            name = "bare"
            kind = "command"
            command = "true"
        "#;
        let (catalog, _guard) = catalog_with_component("agentsight", manifest);

        let mut state = InstalledState::default();
        state.upsert_object(component_object(
            "agentsight",
            "0.1.0",
            ObjectStatus::Installed,
        ));

        let records = select_components(
            &state,
            &layout,
            Some(&catalog),
            "system",
            Some("agentsight"),
        );
        let only = &records[0];
        assert_eq!(only.status, "degraded");
        let entry = only
            .health
            .iter()
            .find(|h| h.name == "agentsight:command:bare")
            .expect("command health entry present");
        assert_eq!(entry.status, "out_of_bounds");
    }

    /// Command-kind check pointing at an absolute external executable
    /// (e.g. `/bin/true`) must be refused with `out_of_bounds`. The
    /// `validate_owned_path` guard fires on the executable, never letting
    /// us run a third-party binary on the user's behalf during status.
    #[test]
    fn manifest_health_command_check_refuses_external_absolute_executable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());

        let manifest = r#"
            [component]
            name = "agentsight"
            version = "0.1.0"

            [[health_checks]]
            name = "host-true"
            kind = "command"
            command = "/bin/true"
        "#;
        let (catalog, _guard) = catalog_with_component("agentsight", manifest);

        let mut state = InstalledState::default();
        state.upsert_object(component_object(
            "agentsight",
            "0.1.0",
            ObjectStatus::Installed,
        ));

        let records = select_components(
            &state,
            &layout,
            Some(&catalog),
            "system",
            Some("agentsight"),
        );
        let only = &records[0];
        assert_eq!(only.status, "degraded");
        let entry = only
            .health
            .iter()
            .find(|h| h.name == "agentsight:command:host-true")
            .expect("command health entry present");
        assert_eq!(entry.status, "out_of_bounds");
    }

    /// Command-kind check containing a shell metacharacter (pipe, redirect,
    /// `;`, …) must be refused. `status` runs probes WITHOUT a shell, so
    /// any probe that needs one is a misauthored manifest, not a runnable
    /// command.
    #[test]
    fn manifest_health_command_check_refuses_shell_metacharacters() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());
        // Path is fine; the trailing `; rm -rf /` is what must trip the guard.
        let probe = write_probe_script(&layout, "probe-meta", "#!/bin/sh\nexit 0\n");

        let manifest = format!(
            r#"
            [component]
            name = "agentsight"
            version = "0.1.0"

            [[health_checks]]
            name = "metachar"
            kind = "command"
            command = "{} ; echo hax"
        "#,
            probe.display()
        );
        let (catalog, _guard) = catalog_with_component("agentsight", &manifest);

        let mut state = InstalledState::default();
        state.upsert_object(component_object(
            "agentsight",
            "0.1.0",
            ObjectStatus::Installed,
        ));

        let records = select_components(
            &state,
            &layout,
            Some(&catalog),
            "system",
            Some("agentsight"),
        );
        let only = &records[0];
        assert_eq!(only.status, "degraded");
        let entry = only
            .health
            .iter()
            .find(|h| h.name == "agentsight:command:metachar")
            .expect("command health entry present");
        assert_eq!(entry.status, "invalid_check");
    }

    /// systemd-kind check on a non-Linux / user-mode host degrades to
    /// `not_supported` rather than failing — we cannot prove the unit's
    /// state on a host without a service backend, but we don't have a
    /// positive failure either. user mode in particular short-circuits
    /// to NotSupported, which is a portable assertion across all CI
    /// platforms (linux, darwin, etc.).
    #[test]
    fn manifest_health_systemd_check_unsupported_degrades() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());

        let manifest = r#"
            [component]
            name = "agentsight"
            version = "0.1.0"

            [[health_checks]]
            name = "service"
            kind = "systemd"
            unit = "agentsight.service"
        "#;
        let (catalog, _guard) = catalog_with_component("agentsight", manifest);

        let mut state = InstalledState::default();
        state.upsert_object(component_object(
            "agentsight",
            "0.1.0",
            ObjectStatus::Installed,
        ));

        // user mode is the portable "no service backend" install_mode.
        let records =
            select_components(&state, &layout, Some(&catalog), "user", Some("agentsight"));
        let only = &records[0];
        assert_eq!(only.status, "degraded", "unsupported -> degraded");
        let entry = only
            .health
            .iter()
            .find(|h| h.name == "agentsight:systemd:service")
            .expect("systemd health entry present");
        assert_eq!(entry.status, "not_supported");
    }

    /// Manifest health probes layer on top of integrity — a failed file
    /// integrity check stays `failed` even when every manifest check
    /// reports `ok`. Order of escalation: integrity is authoritative
    /// downward, manifest is authoritative for additional escalation.
    #[test]
    fn manifest_health_does_not_downgrade_failed_integrity() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = test_layout(dir.path());
        let missing_owned = layout.bin_dir.join("missing-binary");
        let probe = write_probe_script(&layout, "probe-ok-integ", "#!/bin/sh\nexit 0\n");

        let manifest = format!(
            r#"
            [component]
            name = "agentsight"
            version = "0.1.0"

            [[health_checks]]
            name = "self-check"
            kind = "command"
            command = "{}"
        "#,
            probe.display()
        );
        let (catalog, _guard) = catalog_with_component("agentsight", &manifest);

        let mut state = InstalledState::default();
        let mut comp = component_object("agentsight", "0.1.0", ObjectStatus::Installed);
        comp.files = vec![OwnedFile {
            path: missing_owned,
            owner: FileOwner::Anolisa,
            sha256: Some("deadbeef".to_string()),
        }];
        state.upsert_object(comp);

        let records = select_components(
            &state,
            &layout,
            Some(&catalog),
            "system",
            Some("agentsight"),
        );
        let only = &records[0];
        assert_eq!(
            only.status, "failed",
            "integrity failure dominates over a clean manifest probe",
        );
        // Both entries must be present: integrity surfaced the missing
        // file, manifest surfaced the ok command.
        assert!(
            only.health
                .iter()
                .any(|h| h.name.starts_with("integrity:") && h.status == "missing_file"),
            "integrity entry present",
        );
        assert!(
            only.health
                .iter()
                .any(|h| h.name == "agentsight:command:self-check" && h.status == "ok"),
            "manifest entry present",
        );
    }
}
