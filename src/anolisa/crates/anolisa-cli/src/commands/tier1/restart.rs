//! `anolisa restart <component>` — restart a component's systemd service units.
//!
//! Brings a component's services up (`systemctl restart` starts a stopped
//! unit), routing each through the [`anolisa_core::ServiceManager`] for its
//! scope. The handler:
//!
//!   1. Loads `installed.toml` and locates the component. Absent →
//!      `NOT_INSTALLED`.
//!   2. Collects the component's restartable `.service` units, by backend:
//!      - **raw** installs: the recorded `ServiceRef`s — ANOLISA drove the
//!        activation, so state is the source of truth;
//!      - **RPM** installs (rpm-observed / rpm-managed): live `rpm -ql`
//!        discovery, because the RPM path records no services and the package
//!        owns the unit files. Template (`foo@.service`) units cannot be
//!        expanded here — instances are runtime/user state, not package files —
//!        so they degrade to a per-user guidance note instead of a restart.
//!   3. Empty set splits two ways: a component that ships no service units at
//!      all → `INVALID_ARGUMENT`; one that ships only templates restart cannot
//!      expand → exit 0 with per-user guidance (the package is fine, the
//!      operator just has to pick an instance).
//!   4. Reloads each in-play scope's unit database once (best-effort), so a
//!      freshly-placed unit (a place-only install, or an RPM whose `%post` did
//!      not reload) is loadable before restart.
//!   5. Routes each unit through the manager for its own scope — system units
//!      via `systemctl`, user units via `systemctl --user`. A unit whose scope
//!      has no driver here (a user unit in a system-mode restart, non-Linux,
//!      container) is a per-unit `not_supported` skip, never mis-driven through
//!      another namespace.
//!   6. Calls `restart_service(unit)` per unit. Per-unit failures are collected
//!      as warnings; a unit systemctl refuses does NOT abort the whole op.
//!
//! The execute path holds the exclusive state-root lock from lookup through
//! service dispatch so restart cannot race teardown and the pending-recovery
//! gate stays authoritative. `--dry-run` reads the same state and journals
//! without creating or exclusively locking that root, matching
//! forget/repair previews, so a non-root system preview can render a plan
//! on a root-owned install.

use clap::Parser;

use anolisa_core::domain::{Installation, ProviderBinding};
use anolisa_core::facts::{JournalEvidence, pending_journal_for};
use anolisa_core::lock::InstallLock;
use anolisa_core::{
    ObjectKind, ServiceManager, ServiceScope, ServiceState,
    service_for_install_mode as service_factory,
    user_service_for_install_mode as user_service_factory,
};
use anolisa_env::EnvService;
use anolisa_platform::command::CommandRunner;
use anolisa_platform::pkg_query::PackageQueryError;
use anolisa_platform::rpm_query::RpmPackageQuery;

use crate::color::Palette;
use crate::commands::common;
use crate::commands::tier1::recovery::{LockedJournalGate, pending_operation_error};
use crate::commands::tier1::rpm_install;
use crate::context::CliContext;
use crate::response::{CliError, render_json};

const COMMAND: &str = "restart";

#[derive(Parser)]
pub struct RestartArgs {
    /// Component whose services to restart
    pub component: String,
}

pub fn handle(args: RestartArgs, ctx: &CliContext) -> Result<(), CliError> {
    let payload = compute_restart(&args.component, ctx)?;
    render_restart(&payload, ctx)
}

/// Resolve units and either preview or dispatch the restart.
///
/// `--dry-run` must not `daemon-reload` or `restart_service`. Those are live
/// systemd mutations; a preview that still dispatched would bounce units
/// while the operator asked only for the plan. Unsupported scopes stay
/// `not_supported` so the preview names the same units execute would skip.
fn compute_restart(input: &str, ctx: &CliContext) -> Result<RestartPayload, CliError> {
    compute_restart_with(input, ctx, None)
}

/// Resolve units and either preview or dispatch using the given managers.
///
/// Tests inject recording managers so a supported backend cannot hide a
/// dry-run dispatch behind the host's `not_supported` skip.
fn compute_restart_with(
    input: &str,
    ctx: &CliContext,
    managers: Option<(&dyn ServiceManager, &dyn ServiceManager)>,
) -> Result<RestartPayload, CliError> {
    let command = format!("restart {input}");

    let install_mode = ctx.install_mode.as_str();
    let layout = common::resolve_layout(ctx);
    // Preview only reads recorded units and journals. Creating or exclusively
    // locking `/var/lib/anolisa/lock` would fail on a root-owned install
    // before a non-root `--dry-run` could print the plan.
    let exclusive_lock = if ctx.dry_run {
        None
    } else {
        Some(
            InstallLock::acquire(&layout.lock_file).map_err(|err| CliError::Runtime {
                command: command.clone(),
                reason: format!("failed to acquire install lock: {err}"),
            })?,
        )
    };
    let (resolved, view) = common::resolve_mutation_target(input, ctx, &command)?;
    let journal_dir = rpm_install::journal_dir(&layout);
    let evidence = JournalEvidence::new(&journal_dir, &view.writable.state.operations);
    if let Some(ref lock) = exclusive_lock {
        let journal_gate = LockedJournalGate::load(lock, evidence, &command)?;
        journal_gate.ensure_clear(&resolved, &command)?;
    } else if let Some(path) =
        pending_journal_for(evidence, &resolved).map_err(|err| CliError::Runtime {
            command: command.clone(),
            reason: format!("failed to inspect operation journals: {err}"),
        })?
    {
        return Err(pending_operation_error(&command, &resolved, &path));
    }
    let comp = view
        .writable
        .state
        .find(ObjectKind::Component, &resolved)
        .ok_or_else(|| CliError::NotInstalled {
            command: command.clone(),
            reason: format!(
                "component '{resolved}' is not installed — nothing to restart (run `anolisa status` to see what is installed)"
            ),
        })?;

    // Units come from the component's recorded ServiceRefs (raw installs, where
    // ANOLISA drove activation) or from live `rpm -ql` discovery (RPM installs,
    // which record no services). Discovery may also return notes — e.g. a
    // template unit it cannot expand in this tier — which seed `warnings`.
    let (units, mut warnings) = collect_restart_units(comp, &resolved)?;

    if units.is_empty() {
        if warnings.is_empty() {
            // Ships no service units at all → nothing to restart, a usage error.
            return Err(CliError::InvalidArgument {
                command,
                reason: format!("component '{resolved}' has no restartable systemd service units"),
            });
        }
        // Ships only template units whose instances are per-user runtime state
        // restart cannot choose. Not an error: the package is fine,
        // so exit 0 and surface the per-user guidance already collected.
        return Ok(guidance_only_payload(
            &resolved,
            install_mode,
            warnings,
            ctx.dry_run,
        ));
    }

    // Restart routes each unit through the manager for its own scope — the
    // same per-scope partitioning uninstall uses. System units drive
    // `systemctl`, user units drive `systemctl --user`, so a mixed-scope
    // component never mis-drives a user unit through the system manager (or
    // vice versa): a unit whose scope has no driver here is a per-unit
    // `not_supported` skip rather than a wrong-namespace call.
    let detected_sys;
    let detected_user;
    let (sys_manager, user_manager) = match managers {
        Some(pair) => pair,
        None => {
            let env = EnvService::detect();
            detected_sys = service_factory(install_mode, &env);
            detected_user = user_service_factory(install_mode, &env);
            (detected_sys.as_ref(), detected_user.as_ref())
        }
    };

    // Summary fields describe the set of scopes actually present. The op is
    // "supported" if at least one unit's manager can drive it, and the label
    // combines the distinct namespaces in play (just one for the common
    // single-scope component).
    let used_sys = units.iter().any(|u| u.scope == ServiceScope::System);
    let used_user = units.iter().any(|u| u.scope == ServiceScope::User);
    let supported =
        (used_sys && sys_manager.supported()) || (used_user && user_manager.supported());
    let manager_label = match (used_sys, used_user) {
        (true, true) => format!("{}+{}", sys_manager.manager(), user_manager.manager()),
        (true, false) => sys_manager.manager().to_string(),
        (false, true) => user_manager.manager().to_string(),
        // `units` is non-empty (checked above), so at least one scope is used.
        (false, false) => unreachable!("restartable units present but no scope flagged"),
    };

    let results = if ctx.dry_run {
        preview_restart_results(&units, sys_manager, user_manager)
    } else {
        execute_restart_units(
            &units,
            sys_manager,
            user_manager,
            used_sys,
            used_user,
            &mut warnings,
        )
    };

    Ok(RestartPayload {
        component: resolved,
        install_mode: install_mode.to_string(),
        manager: manager_label,
        supported,
        dry_run: ctx.dry_run,
        units: results,
        warnings,
    })
}

fn render_restart(payload: &RestartPayload, ctx: &CliContext) -> Result<(), CliError> {
    if ctx.json {
        return render_json(COMMAND, payload);
    }

    if !ctx.quiet {
        if payload.units.is_empty() {
            let color = Palette::new(ctx.no_color);
            println!(
                "{} {} {}",
                color.command("restart"),
                payload.component,
                color.warn("no instances to restart")
            );
            for warning in &payload.warnings {
                eprintln!("{} {}", color.warn("guidance:"), warning);
            }
        } else {
            render_human(payload, ctx.no_color);
        }
    }
    Ok(())
}

/// Describe units execute would touch without contacting systemd.
fn preview_restart_results(
    units: &[RestartUnit],
    sys_manager: &dyn ServiceManager,
    user_manager: &dyn ServiceManager,
) -> Vec<RestartResult> {
    units
        .iter()
        .map(|unit| {
            let manager = manager_for_scope(unit.scope, sys_manager, user_manager);
            if !manager.supported() {
                unsupported_restart_result(unit, manager)
            } else {
                RestartResult {
                    component: unit.component.clone(),
                    unit: unit.unit.clone(),
                    state: PLANNED_STATE.to_string(),
                    changed: false,
                    manager: manager.manager().to_string(),
                    message: format!("dry-run: would restart {}", unit.unit),
                }
            }
        })
        .collect()
}

fn execute_restart_units(
    units: &[RestartUnit],
    sys_manager: &dyn ServiceManager,
    user_manager: &dyn ServiceManager,
    used_sys: bool,
    used_user: bool,
    warnings: &mut Vec<String>,
) -> Vec<RestartResult> {
    // Freshly-placed units (a place-only install, or an RPM whose %post did not
    // reload the manager) aren't loadable until the manager reloads its unit
    // database, so restart would otherwise fail "unit not found". Reload once
    // per in-play scope, best-effort — a reload failure only adds a warning.
    if used_sys && sys_manager.supported() {
        if let Err(err) = sys_manager.daemon_reload() {
            warnings.push(format!("daemon-reload (system scope) failed: {err}"));
        }
    }
    if used_user && user_manager.supported() {
        if let Err(err) = user_manager.daemon_reload() {
            warnings.push(format!("daemon-reload (user scope) failed: {err}"));
        }
    }

    let mut results: Vec<RestartResult> = Vec::with_capacity(units.len());
    for unit in units {
        let manager = manager_for_scope(unit.scope, sys_manager, user_manager);
        if !manager.supported() {
            // Quiet skip: this unit's scope has no driver here (a user unit
            // in a system-mode restart, container, non-Linux). Reported
            // `not_supported` per unit so the boundary is explicit and the
            // unit is never mis-driven through another namespace.
            results.push(unsupported_restart_result(unit, manager));
            continue;
        }
        match manager.restart_service(&unit.unit) {
            Ok(outcome) => {
                results.push(RestartResult {
                    component: unit.component.clone(),
                    unit: unit.unit.clone(),
                    state: outcome.state.as_str().to_string(),
                    changed: outcome.changed,
                    manager: outcome.manager,
                    message: outcome.message,
                });
                if matches!(outcome.state, ServiceState::Failed | ServiceState::Unknown) {
                    warnings.push(format!(
                        "{}/{} reports state '{}' after restart",
                        unit.component,
                        unit.unit,
                        outcome.state.as_str()
                    ));
                }
            }
            Err(err) => {
                let msg = format!("{err}");
                warnings.push(format!(
                    "service restart skipped for {}/{}: {msg}",
                    unit.component, unit.unit
                ));
                results.push(RestartResult {
                    component: unit.component.clone(),
                    unit: unit.unit.clone(),
                    state: "unknown".to_string(),
                    changed: false,
                    manager: manager.manager().to_string(),
                    message: msg,
                });
            }
        }
    }
    results
}

fn manager_for_scope<'a>(
    scope: ServiceScope,
    sys_manager: &'a dyn ServiceManager,
    user_manager: &'a dyn ServiceManager,
) -> &'a dyn ServiceManager {
    match scope {
        ServiceScope::System => sys_manager,
        ServiceScope::User => user_manager,
    }
}

fn unsupported_restart_result(unit: &RestartUnit, manager: &dyn ServiceManager) -> RestartResult {
    RestartResult {
        component: unit.component.clone(),
        unit: unit.unit.clone(),
        state: "not_supported".to_string(),
        changed: false,
        manager: manager.manager().to_string(),
        message: manager
            .unsupported_reason()
            .unwrap_or("service manager not supported in this environment")
            .to_string(),
    }
}

/// Absolute directories systemd searches for **system** unit files. A `.service`
/// placed directly in one of these is a system-scope unit.
const SYSTEM_UNIT_DIRS: &[&str] = &[
    "/usr/lib/systemd/system",
    "/usr/local/lib/systemd/system",
    "/etc/systemd/system",
    "/run/systemd/system",
];

/// Absolute directories systemd searches for **user** unit files (driven via
/// `systemctl --user`).
const USER_UNIT_DIRS: &[&str] = &[
    "/usr/lib/systemd/user",
    "/usr/local/lib/systemd/user",
    "/etc/systemd/user",
];

/// Collect a component's restartable units plus any discovery notes.
///
/// The unit source is the component's backend: raw installs read the recorded
/// `ServiceRef`s (ANOLISA owns activation); RPM installs discover live from
/// `rpm -ql` (the package owns the unit files and state records no services).
/// Returned notes (e.g. for un-expandable templates) are surfaced to the user.
fn collect_restart_units(
    comp: &Installation,
    component: &str,
) -> Result<(Vec<RestartUnit>, Vec<String>), CliError> {
    match &comp.binding {
        ProviderBinding::Delegated { package, .. } => discover_rpm_units(
            package.resolved_name(),
            component,
            &RpmPackageQuery::system(),
        ),
        // Owned: the recorded ServiceRefs (restartable is hardcoded true
        // today; the filter keeps the door open for an explicit opt-out
        // later).
        ProviderBinding::Owned { artifact } => {
            let units = artifact
                .services
                .iter()
                .filter(|svc| svc.restartable)
                .map(|svc| RestartUnit {
                    component: component.to_string(),
                    unit: svc.name.clone(),
                    scope: svc.scope,
                })
                .collect();
            Ok((units, Vec::new()))
        }
    }
}

/// Discover an RPM component's `.service` units from its file manifest.
///
/// `rpm -ql <pkg>` lists every owned path; [`classify_unit_files`] keeps the
/// `.service` files sitting directly in a systemd unit directory and infers
/// scope from that directory. Plain units become [`RestartUnit`]s; template
/// (`foo@.service`) units cannot be expanded here (their instances are runtime
/// state, not package files), so each yields a per-user guidance note instead.
///
/// Generic over the [`RpmPackageQuery`] runner so tests can inject a fake
/// `rpm -ql` listing; production passes [`RpmPackageQuery::system`].
///
/// # Errors
/// `Runtime` when the component records no RPM package name (refresh with
/// `repair`), or when `rpm -ql` fails (e.g. the recorded package vanished).
fn discover_rpm_units<R: CommandRunner>(
    package: Option<&str>,
    component: &str,
    query: &RpmPackageQuery<R>,
) -> Result<(Vec<RestartUnit>, Vec<String>), CliError> {
    let command = format!("restart {component}");
    let package = package.ok_or_else(|| CliError::Runtime {
            command: command.clone(),
            reason: format!(
                "component '{component}' is RPM-backed but its state records no package name; run `anolisa repair {component}` to refresh rpm metadata"
            ),
        })?;

    let paths = match query.list_files(package) {
        Ok(paths) => paths,
        // Tooling gone: match the rpm/dnf-missing handling repair/update/uninstall
        // use, so an RPM-backed restart fails with the same actionable message
        // rather than a generic "command not found".
        Err(PackageQueryError::CommandMissing { .. }) => {
            return Err(rpm_tooling_missing_error(&command));
        }
        Err(err) => {
            return Err(CliError::Runtime {
                command,
                reason: format!("could not list files for RPM package '{package}': {err}"),
            });
        }
    };

    let mut units = Vec::new();
    let mut notes = Vec::new();
    for (unit, scope) in classify_unit_files(&paths) {
        if is_template_unit(&unit) {
            notes.push(template_guidance(&unit, scope));
            continue;
        }
        units.push(RestartUnit {
            component: component.to_string(),
            unit,
            scope,
        });
    }
    Ok((units, notes))
}

/// Warn-and-exit error when `rpm`/`dnf` is absent: an RPM-backed component
/// cannot be restarted without the package manager to enumerate its units.
/// Mirrors the sibling helpers in `repair`/`update`/`uninstall` so the
/// tooling-missing message is uniform across tier-1 commands.
fn rpm_tooling_missing_error(command: &str) -> CliError {
    CliError::Runtime {
        command: command.to_string(),
        reason: "rpm/dnf not found: cannot restart an RPM-backed component without the package manager. Install rpm/dnf and retry".to_string(),
    }
}

/// Stable `manager` label for a guidance-only result. No manager is engaged
/// (the component ships only templates), so the wire field carries a sentinel
/// rather than an empty string — `""` would be ambiguous between "no manager"
/// and a missing field, whereas `none` reads clearly alongside the normal
/// `systemd` / `systemd-user` / `not-supported` labels.
const GUIDANCE_MANAGER: &str = "none";

/// Build the payload for a guidance-only outcome: no units, the
/// per-user guidance as warnings, the [`GUIDANCE_MANAGER`] sentinel, and
/// `supported = true` (the command completed; the package is healthy).
fn guidance_only_payload(
    component: &str,
    install_mode: &str,
    warnings: Vec<String>,
    dry_run: bool,
) -> RestartPayload {
    RestartPayload {
        component: component.to_string(),
        install_mode: install_mode.to_string(),
        manager: GUIDANCE_MANAGER.to_string(),
        supported: true,
        dry_run,
        units: Vec::new(),
        warnings,
    }
}

/// Keep the `.service` files that sit **directly** in a known systemd unit
/// directory, pairing each with the scope its directory implies.
///
/// Requiring a direct parent match rejects `*.target.wants/foo.service` enable
/// symlinks and `foo.service.d/` drop-ins, leaving only canonical unit files.
fn classify_unit_files(paths: &[String]) -> Vec<(String, ServiceScope)> {
    let mut out = Vec::new();
    for path in paths {
        let path = path.trim();
        if !path.ends_with(".service") {
            continue;
        }
        let Some((dir, file)) = path.rsplit_once('/') else {
            continue;
        };
        let scope = if SYSTEM_UNIT_DIRS.contains(&dir) {
            ServiceScope::System
        } else if USER_UNIT_DIRS.contains(&dir) {
            ServiceScope::User
        } else {
            continue;
        };
        out.push((file.to_string(), scope));
    }
    out
}

/// A systemd template unit names its instance after `@` and has no instance
/// before `.service` (e.g. `anolisa-memory@.service`).
fn is_template_unit(unit: &str) -> bool {
    unit.ends_with("@.service")
}

/// Per-user guidance for a template unit restart cannot expand.
///
/// A bare template is not restartable (`systemctl restart foo@.service` fails);
/// the operator must pick an instance. For user-scope templates that also means
/// running as the target user, plus linger to survive logout.
fn template_guidance(unit: &str, scope: ServiceScope) -> String {
    let base = unit.trim_end_matches("@.service");
    match scope {
        ServiceScope::User => format!(
            "{unit} is a per-user template; restart cannot expand its instances — enable one as the target user: `systemctl --user enable --now {base}@<user>.service` (and `loginctl enable-linger <user>` to keep it running after logout)"
        ),
        ServiceScope::System => format!(
            "{unit} is a systemd template; restart cannot pick an instance — start a concrete one with `systemctl start {base}@<instance>.service`"
        ),
    }
}

#[derive(Debug)]
struct RestartUnit {
    component: String,
    unit: String,
    scope: ServiceScope,
}

#[derive(Debug, Clone, serde::Serialize)]
struct RestartResult {
    component: String,
    unit: String,
    state: String,
    changed: bool,
    manager: String,
    message: String,
}

/// Wire state for a dry-run unit that execute would restart.
const PLANNED_STATE: &str = "planned";

#[derive(Debug, serde::Serialize)]
struct RestartPayload {
    component: String,
    install_mode: String,
    manager: String,
    supported: bool,
    dry_run: bool,
    units: Vec<RestartResult>,
    warnings: Vec<String>,
}

/// Banner verb for the human summary. Unsupported scopes keep `skipped`
/// even on `--dry-run`, so a preview does not look successful when every
/// unit would be refused.
fn human_banner_status(payload: &RestartPayload) -> &'static str {
    if !payload.supported {
        "skipped"
    } else if payload.dry_run {
        "planned"
    } else {
        "dispatched"
    }
}

fn render_human(payload: &RestartPayload, no_color: bool) {
    let color = Palette::new(no_color);
    let status = human_banner_status(payload);
    if status == "skipped" {
        println!(
            "{} {} {} {}",
            color.command("restart"),
            payload.component,
            color.warn(status),
            color.muted(format!("(manager={} unsupported)", payload.manager))
        );
    } else {
        println!(
            "{} {} {}",
            color.command("restart"),
            payload.component,
            color.ok(status)
        );
    }
    println!("{} {}", color.label("manager:"), payload.manager);
    if !payload.units.is_empty() {
        println!("{}", color.header("units:"));
        for r in &payload.units {
            println!(
                "  - {}/{} {} (changed={})",
                r.component,
                r.unit,
                color.status(&r.state),
                color.bool_value(r.changed),
            );
        }
    }
    for w in &payload.warnings {
        eprintln!("{} {}", color.warn("warning:"), w);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::commands::tier1::rpm_install;
    use crate::context::InstallMode;
    use anolisa_core::domain::{
        InstallationScope, LifecycleStatus, OwnedArtifact, ProviderBinding,
    };
    use anolisa_core::state::ServiceRef;
    use anolisa_core::state_store::StateStore;
    use anolisa_core::transaction::Transaction;
    use anolisa_core::{FakeServiceManager, ServiceOp};
    use anolisa_platform::command::CommandOutput;
    use anolisa_platform::fs_layout::FsLayout;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn ctx_with_prefix(install_mode: InstallMode, prefix: Option<PathBuf>) -> CliContext {
        let root = prefix
            .as_deref()
            .unwrap_or_else(|| std::path::Path::new("/tmp/anolisa-restart-validation"));
        // Identity resolution consults the component index for names absent
        // from state; a seeded local index keeps fixture names supported.
        if install_mode == InstallMode::System
            && let Some(prefix) = prefix.as_ref()
        {
            crate::commands::tier1::install::tests::seed_repo_config_with_index(
                &anolisa_platform::fs_layout::FsLayout::system(Some(prefix.clone())),
                crate::commands::tier1::install::tests::TEST_INDEX_COMPONENTS,
            );
        }
        crate::test_support::context_for_root(
            root,
            install_mode,
            prefix.clone(),
            Default::default(),
        )
    }

    fn ctx_with_options(
        install_mode: InstallMode,
        prefix: Option<PathBuf>,
        options: crate::test_support::TestContextOptions,
    ) -> CliContext {
        let root = prefix
            .as_deref()
            .unwrap_or_else(|| std::path::Path::new("/tmp/anolisa-restart-validation"));
        if install_mode == InstallMode::System
            && let Some(prefix) = prefix.as_ref()
        {
            crate::commands::tier1::install::tests::seed_repo_config_with_index(
                &anolisa_platform::fs_layout::FsLayout::system(Some(prefix.clone())),
                crate::commands::tier1::install::tests::TEST_INDEX_COMPONENTS,
            );
        }
        crate::test_support::context_for_root(root, install_mode, prefix.clone(), options)
    }

    const PROBE_UNIT: &str = "anolisa-restart-dry-run-probe.service";

    fn planted_restart_ctx(dry_run: bool) -> (tempfile::TempDir, CliContext) {
        let tmp = tempdir().expect("tmpdir");
        let prefix = tmp.path().to_path_buf();
        let layout = FsLayout::system(Some(prefix.clone()));
        plant_owned_restartable(&layout, "agentsight", PROBE_UNIT, ServiceScope::System);
        let ctx = ctx_with_options(
            InstallMode::System,
            Some(prefix),
            crate::test_support::TestContextOptions {
                dry_run,
                quiet: true,
                no_color: true,
                ..Default::default()
            },
        );
        (tmp, ctx)
    }

    fn plant_owned_restartable(layout: &FsLayout, name: &str, unit: &str, scope: ServiceScope) {
        let state_path = layout.state_dir.join("installed.toml");
        let mut store = StateStore::empty_for_layout(layout);
        store.upsert(Installation {
            kind: ObjectKind::Component,
            name: name.to_string(),
            scope: InstallationScope::System,
            binding: ProviderBinding::Owned {
                artifact: OwnedArtifact {
                    version: "1.0.0".to_string(),
                    distribution_source: None,
                    raw_package: None,
                    manifest_digest: None,
                    files: Vec::new(),
                    services: vec![ServiceRef {
                        name: unit.to_string(),
                        manager: "systemd".to_string(),
                        restartable: true,
                        enabled: true,
                        scope,
                    }],
                    external_modified_files: Vec::new(),
                    provisioned_packages: Vec::new(),
                },
            },
            status: LifecycleStatus::Installed,
            installed_at: "2026-07-21T00:00:00Z".to_string(),
            last_operation_id: None,
            subscription_scope: Default::default(),
            enabled_features: Vec::new(),
            health: Vec::new(),
        });
        store.save(&state_path).expect("save state");
    }

    /// Fake `CommandRunner` returning a canned `rpm -ql` listing (or a spawn
    /// error), so the RPM discovery path is exercised without a real `rpm`.
    struct FakeRpm {
        stdout: String,
        code: Option<i32>,
        spawn_err: Option<std::io::ErrorKind>,
    }

    impl CommandRunner for FakeRpm {
        fn run(&self, _program: &str, _args: &[&str]) -> std::io::Result<CommandOutput> {
            if let Some(kind) = self.spawn_err {
                return Err(std::io::Error::new(kind, "fake spawn failure"));
            }
            Ok(CommandOutput {
                code: self.code,
                stdout: self.stdout.clone(),
                stderr: String::new(),
            })
        }
    }

    fn fake_query(listing: &str) -> RpmPackageQuery<FakeRpm> {
        RpmPackageQuery::with_runner(FakeRpm {
            stdout: listing.to_string(),
            code: Some(0),
            spawn_err: None,
        })
    }

    #[test]
    fn restart_unknown_component_returns_not_installed() {
        let tmp = tempdir().expect("tmpdir");
        let err = handle(
            RestartArgs {
                component: "agentsight".to_string(),
            },
            &ctx_with_prefix(InstallMode::System, Some(tmp.path().to_path_buf())),
        )
        .expect_err("must error");
        assert_eq!(err.code(), "NOT_INSTALLED");
        assert_eq!(err.exit_code(), 2);
        assert!(
            err.reason().contains("not installed"),
            "reason must mention 'not installed': {}",
            err.reason()
        );
    }

    #[test]
    fn restart_refuses_a_pending_lifecycle_before_service_side_effects() {
        let tmp = tempdir().expect("tmpdir");
        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
        let state_path = layout.state_dir.join("installed.toml");
        let mut store = StateStore::empty_for_layout(&layout);
        store.upsert(Installation {
            kind: ObjectKind::Component,
            name: "agentsight".to_string(),
            scope: InstallationScope::System,
            binding: ProviderBinding::Owned {
                artifact: OwnedArtifact {
                    version: "1.0.0".to_string(),
                    distribution_source: None,
                    raw_package: None,
                    manifest_digest: None,
                    files: Vec::new(),
                    services: vec![ServiceRef {
                        name: "agentsight.service".to_string(),
                        manager: "systemd".to_string(),
                        restartable: true,
                        enabled: true,
                        scope: ServiceScope::System,
                    }],
                    external_modified_files: Vec::new(),
                    provisioned_packages: Vec::new(),
                },
            },
            status: LifecycleStatus::Installed,
            installed_at: "2026-07-21T00:00:00Z".to_string(),
            last_operation_id: None,
            subscription_scope: Default::default(),
            enabled_features: Vec::new(),
            health: Vec::new(),
        });
        store.save(&state_path).expect("save state");
        let _pending = Transaction::begin_with_subject(
            "uninstall",
            Some("agentsight"),
            state_path.clone(),
            &rpm_install::journal_dir(&layout),
        )
        .expect("begin pending uninstall");

        let err = handle(
            RestartArgs {
                component: "agentsight".to_string(),
            },
            &ctx_with_prefix(InstallMode::System, Some(tmp.path().to_path_buf())),
        )
        .expect_err("pending lifecycle must block restart");

        assert!(err.reason().contains("pending operation journal"), "{err}");
        assert!(err.reason().contains("anolisa repair agentsight"), "{err}");
    }

    #[test]
    fn classify_unit_files_keeps_direct_service_units_with_scope() {
        let paths = vec![
            // not a unit
            "/usr/local/bin/agentsight".to_string(),
            // system units in the canonical and FHS-local dirs
            "/usr/lib/systemd/system/agentsight.service".to_string(),
            "/usr/local/lib/systemd/system/ws-ckpt.service".to_string(),
            // user-scope template
            "/usr/lib/systemd/user/anolisa-memory@.service".to_string(),
            // enable symlink in a .wants subdir — not a canonical unit file
            "/etc/systemd/system/multi-user.target.wants/x.service".to_string(),
            // drop-in (.conf, not .service)
            "/usr/lib/systemd/system/foo.service.d/override.conf".to_string(),
            // non-.service unit type
            "/usr/lib/systemd/system/foo.socket".to_string(),
            // .service outside any known unit dir
            "/opt/random/bar.service".to_string(),
        ];
        assert_eq!(
            classify_unit_files(&paths),
            vec![
                ("agentsight.service".to_string(), ServiceScope::System),
                ("ws-ckpt.service".to_string(), ServiceScope::System),
                ("anolisa-memory@.service".to_string(), ServiceScope::User),
            ]
        );
    }

    #[test]
    fn is_template_unit_matches_only_bare_template() {
        assert!(is_template_unit("anolisa-memory@.service"));
        assert!(!is_template_unit("agentsight.service"));
        // A concrete instance is restartable directly, so it is not a template.
        assert!(!is_template_unit("anolisa-memory@alice.service"));
    }

    #[test]
    fn template_guidance_user_mentions_instance_and_linger() {
        let msg = template_guidance("anolisa-memory@.service", ServiceScope::User);
        assert!(msg.contains("anolisa-memory@<user>.service"), "{msg}");
        assert!(msg.contains("--user"), "{msg}");
        assert!(msg.contains("enable-linger"), "{msg}");
    }

    #[test]
    fn template_guidance_system_mentions_instance_not_user_bus() {
        let msg = template_guidance("getty@.service", ServiceScope::System);
        assert!(msg.contains("getty@<instance>.service"), "{msg}");
        assert!(!msg.contains("--user"), "{msg}");
    }

    #[test]
    fn rpm_tooling_missing_error_mentions_restart_and_tooling() {
        // P3: rpm/dnf absent must surface the same actionable message the other
        // tier-1 commands use, not a bare "command not found".
        let err = rpm_tooling_missing_error("restart agent-memory");
        assert!(
            err.reason().contains("rpm/dnf not found"),
            "{}",
            err.reason()
        );
        assert!(err.reason().contains("restart"), "{}", err.reason());
    }

    #[test]
    fn guidance_only_template_component_succeeds_not_errors() {
        // A component whose only units are templates is NOT an error — the
        // guidance-only path returns Ok with the per-user guidance.
        let ctx = ctx_with_prefix(InstallMode::System, None);
        let warnings = vec![template_guidance(
            "anolisa-memory@.service",
            ServiceScope::User,
        )];
        let res = render_restart(
            &guidance_only_payload("agent-memory", "system", warnings, ctx.dry_run),
            &ctx,
        );
        assert!(
            res.is_ok(),
            "templates-only restart must succeed, got {res:?}"
        );
    }

    #[test]
    fn guidance_only_payload_uses_stable_manager_label() {
        // Wire semantics: guidance-only carries a stable `none` sentinel (not an
        // empty string), no units, the guidance as warnings, and supported=true.
        let warnings = vec![template_guidance(
            "anolisa-memory@.service",
            ServiceScope::User,
        )];
        let payload = guidance_only_payload("agent-memory", "system", warnings, false);
        assert_eq!(payload.manager, "none");
        assert!(payload.supported);
        assert!(!payload.dry_run);
        assert!(payload.units.is_empty());
        assert_eq!(payload.warnings.len(), 1);
        assert!(
            payload.warnings[0].contains("--user"),
            "{}",
            payload.warnings[0]
        );
    }

    #[test]
    fn discover_rpm_units_splits_plain_units_and_templates() {
        // End-to-end over a fake `rpm -ql`: a plain system .service becomes a
        // restartable unit; a user template degrades to a per-user note; bins
        // and docs are ignored.
        let listing = [
            "/usr/local/bin/agent-memory",
            "/usr/lib/systemd/system/agentsight.service",
            "/usr/lib/systemd/user/anolisa-memory@.service",
            "/usr/share/doc/agent-memory/README",
        ]
        .join("\n");
        let (units, notes) =
            discover_rpm_units(Some("agent-memory"), "agent-memory", &fake_query(&listing))
                .expect("discovery ok");

        assert_eq!(units.len(), 1, "one plain unit expected: {units:?}");
        assert_eq!(units[0].unit, "agentsight.service");
        assert_eq!(units[0].scope, ServiceScope::System);
        assert_eq!(units[0].component, "agent-memory");

        assert_eq!(notes.len(), 1, "one template note expected: {notes:?}");
        assert!(
            notes[0].contains("anolisa-memory@<user>.service") && notes[0].contains("--user"),
            "{}",
            notes[0]
        );
    }

    #[test]
    fn discover_rpm_units_without_package_name_errors() {
        // RPM-backed but state lost the package name → actionable repair hint.
        let err = discover_rpm_units(None, "agent-memory", &fake_query(""))
            .expect_err("missing package name must error");
        assert!(err.reason().contains("repair"), "{}", err.reason());
    }

    #[test]
    fn discover_rpm_units_tooling_missing_maps_to_actionable_error() {
        // `rpm` absent (spawn NotFound) → the uniform rpm/dnf-not-found message,
        // not a generic "command not found".
        let query = RpmPackageQuery::with_runner(FakeRpm {
            stdout: String::new(),
            code: None,
            spawn_err: Some(std::io::ErrorKind::NotFound),
        });
        let err = discover_rpm_units(Some("agent-memory"), "agent-memory", &query)
            .expect_err("missing rpm tooling must error");
        assert!(
            err.reason().contains("rpm/dnf not found"),
            "{}",
            err.reason()
        );
    }

    #[test]
    fn dry_run_lists_recorded_units_without_dispatching() {
        // Inject supported recording managers so this cannot go green on a
        // host whose real manager is `not_supported` and therefore never
        // calls reload/restart even on the execute path.
        let (_tmp, ctx) = planted_restart_ctx(true);
        let sys = FakeServiceManager::new();
        let user = FakeServiceManager::with_scope(ServiceScope::User);

        let payload =
            compute_restart_with("agentsight", &ctx, Some((&sys, &user))).expect("dry-run preview");

        assert!(payload.dry_run, "preview must mark dry_run");
        assert!(payload.supported);
        assert_eq!(payload.units.len(), 1, "{:?}", payload.units);
        assert_eq!(payload.units[0].unit, PROBE_UNIT);
        assert_eq!(payload.units[0].state, PLANNED_STATE);
        assert!(!payload.units[0].changed);
        assert!(
            payload.units[0].message.contains("dry-run: would restart"),
            "{}",
            payload.units[0].message
        );
        assert!(
            sys.calls().is_empty(),
            "dry-run must not reload or restart: {:?}",
            sys.calls()
        );
        assert!(
            user.calls().is_empty(),
            "dry-run must not touch the user manager: {:?}",
            user.calls()
        );
        assert!(
            payload
                .warnings
                .iter()
                .all(|w| !w.contains("daemon-reload")),
            "dry-run must not daemon-reload: {:?}",
            payload.warnings
        );
    }

    #[test]
    fn execute_records_reload_and_restart_when_manager_is_supported() {
        let (_tmp, ctx) = planted_restart_ctx(false);
        let sys = FakeServiceManager::new();
        let user = FakeServiceManager::with_scope(ServiceScope::User);

        let payload =
            compute_restart_with("agentsight", &ctx, Some((&sys, &user))).expect("execute restart");

        assert!(!payload.dry_run);
        assert!(payload.supported);
        assert_eq!(payload.units.len(), 1, "{:?}", payload.units);
        assert_eq!(payload.units[0].unit, PROBE_UNIT);
        assert_ne!(payload.units[0].state, PLANNED_STATE);
        assert_eq!(
            sys.calls(),
            vec![
                (ServiceOp::DaemonReload, String::new()),
                (ServiceOp::Restart, PROBE_UNIT.to_string()),
            ]
        );
        assert!(
            user.calls().is_empty(),
            "system-only execute must not drive the user manager: {:?}",
            user.calls()
        );
    }

    #[test]
    fn dry_run_human_banner_stays_skipped_when_unsupported() {
        let unsupported = RestartPayload {
            component: "agentsight".to_string(),
            install_mode: "system".to_string(),
            manager: "not-supported".to_string(),
            supported: false,
            dry_run: true,
            units: vec![RestartResult {
                component: "agentsight".to_string(),
                unit: PROBE_UNIT.to_string(),
                state: "not_supported".to_string(),
                changed: false,
                manager: "not-supported".to_string(),
                message: "container".to_string(),
            }],
            warnings: Vec::new(),
        };
        assert_eq!(human_banner_status(&unsupported), "skipped");

        let planned = RestartPayload {
            supported: true,
            manager: "fake".to_string(),
            units: vec![RestartResult {
                manager: "fake".to_string(),
                state: PLANNED_STATE.to_string(),
                message: "dry-run: would restart".to_string(),
                ..unsupported.units[0].clone()
            }],
            ..unsupported
        };
        assert_eq!(human_banner_status(&planned), "planned");

        let dispatched = RestartPayload {
            dry_run: false,
            ..planned
        };
        assert_eq!(human_banner_status(&dispatched), "dispatched");
    }

    #[test]
    fn dry_run_still_refuses_an_absent_component() {
        let tmp = tempdir().expect("tmpdir");
        let ctx = ctx_with_options(
            InstallMode::System,
            Some(tmp.path().to_path_buf()),
            crate::test_support::TestContextOptions {
                dry_run: true,
                quiet: true,
                no_color: true,
                ..Default::default()
            },
        );
        let err = compute_restart("agentsight", &ctx).expect_err("absent target must fail");
        assert_eq!(err.code(), "NOT_INSTALLED");
    }

    #[test]
    fn dry_run_still_refuses_a_pending_lifecycle() {
        let tmp = tempdir().expect("tmpdir");
        let prefix = tmp.path().to_path_buf();
        let layout = FsLayout::system(Some(prefix.clone()));
        plant_owned_restartable(
            &layout,
            "agentsight",
            "anolisa-restart-dry-run-probe.service",
            ServiceScope::System,
        );
        let _pending = Transaction::begin_with_subject(
            "uninstall",
            Some("agentsight"),
            layout.state_dir.join("installed.toml"),
            &rpm_install::journal_dir(&layout),
        )
        .expect("begin pending uninstall");
        let ctx = ctx_with_options(
            InstallMode::System,
            Some(prefix),
            crate::test_support::TestContextOptions {
                dry_run: true,
                quiet: true,
                no_color: true,
                ..Default::default()
            },
        );

        let err = compute_restart("agentsight", &ctx)
            .expect_err("pending lifecycle must block the preview too");
        assert!(err.reason().contains("pending operation journal"), "{err}");
    }

    #[test]
    fn dry_run_does_not_create_the_exclusive_lock() {
        let (_tmp, ctx) = planted_restart_ctx(true);
        let layout = FsLayout::system(Some(ctx.prefix.clone().expect("planted prefix")));
        assert!(
            !layout.lock_file.exists(),
            "fixture must start without a lock file"
        );

        let sys = FakeServiceManager::new();
        let user = FakeServiceManager::with_scope(ServiceScope::User);
        let payload =
            compute_restart_with("agentsight", &ctx, Some((&sys, &user))).expect("dry-run preview");

        assert!(payload.dry_run);
        assert_eq!(payload.units[0].state, PLANNED_STATE);
        assert!(
            !layout.lock_file.exists(),
            "preview must not create {}",
            layout.lock_file.display()
        );
    }

    #[test]
    fn dry_run_previews_from_a_non_writable_system_state_root() {
        // Root ignores directory mode bits, so this models a normal
        // unprivileged preview against a root-owned `/var/lib/anolisa`.
        if anolisa_platform::privilege::effective_uid() == 0 {
            return;
        }

        let (tmp, dry_ctx) = planted_restart_ctx(true);
        let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
        let exec_ctx = ctx_with_options(
            InstallMode::System,
            Some(tmp.path().to_path_buf()),
            crate::test_support::TestContextOptions {
                dry_run: false,
                quiet: true,
                no_color: true,
                ..Default::default()
            },
        );
        let _restore = RestorePermissions::dir_mode(&layout.state_dir, 0o555, 0o755);

        let sys = FakeServiceManager::new();
        let user = FakeServiceManager::with_scope(ServiceScope::User);
        let payload = compute_restart_with("agentsight", &dry_ctx, Some((&sys, &user)))
            .expect("non-writable preview must still list units");
        assert!(payload.dry_run);
        assert_eq!(payload.units[0].unit, PROBE_UNIT);
        assert_eq!(payload.units[0].state, PLANNED_STATE);
        assert!(
            !layout.lock_file.exists(),
            "preview must not create the exclusive lock"
        );

        let err = compute_restart("agentsight", &exec_ctx)
            .expect_err("execute still needs a writable exclusive lock");
        assert!(
            err.reason().contains("failed to acquire install lock"),
            "{err}"
        );
    }

    struct RestorePermissions {
        path: PathBuf,
        restore: u32,
    }

    impl RestorePermissions {
        fn dir_mode(path: &std::path::Path, restrict: u32, restore: u32) -> Self {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(path)
                .expect("state dir metadata")
                .permissions();
            perms.set_mode(restrict);
            std::fs::set_permissions(path, perms).expect("restrict state dir");
            Self {
                path: path.to_path_buf(),
                restore,
            }
        }
    }

    impl Drop for RestorePermissions {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(metadata) = std::fs::metadata(&self.path) {
                let mut perms = metadata.permissions();
                perms.set_mode(self.restore);
                let _ = std::fs::set_permissions(&self.path, perms);
            }
        }
    }

    #[test]
    fn preview_marks_unsupported_scopes_without_planned() {
        let units = [RestartUnit {
            component: "agentsight".to_string(),
            unit: "agentsight.service".to_string(),
            scope: ServiceScope::System,
        }];
        let manager = anolisa_core::NotSupportedServiceManager::new(
            "container runtime detected — refusing to drive systemctl".to_string(),
        );
        let results = preview_restart_results(&units, &manager, &manager);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].state, "not_supported");
        assert!(!results[0].changed);
        assert_ne!(results[0].state, PLANNED_STATE);
    }
}
