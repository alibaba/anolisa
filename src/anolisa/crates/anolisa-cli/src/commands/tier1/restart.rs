//! `anolisa restart <component>` — restart a component's systemd service units.
//!
//! Brings a component's services up (`systemctl restart` starts a stopped
//! unit), routing each through the [`anolisa_core::ServiceManager`] for its
//! scope. The handler:
//!
//!   1. Loads `installed.toml` and locates the component. Unknown →
//!      `INVALID_ARGUMENT`.
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
//! Restart is intentionally lock-free: it does not mutate `installed.toml` and
//! is safe to run concurrently with other ANOLISA invocations.

use clap::Parser;

use anolisa_core::{
    InstalledObject, InstalledState, ObjectKind, ServiceManager, ServiceScope, ServiceState,
    service_for_install_mode as service_factory,
    user_service_for_install_mode as user_service_factory,
};
use anolisa_env::EnvService;
use anolisa_platform::command::CommandRunner;
use anolisa_platform::pkg_query::PackageQueryError;
use anolisa_platform::rpm_query::RpmPackageQuery;

use crate::color::Palette;
use crate::commands::common;
use crate::context::CliContext;
use crate::response::{CliError, render_json};

const COMMAND: &str = "restart";

#[derive(Parser)]
pub struct RestartArgs {
    /// Component whose services to restart
    pub component: String,
}

pub fn handle(args: RestartArgs, ctx: &CliContext) -> Result<(), CliError> {
    let command = format!("restart {}", args.component);

    let layout = common::resolve_layout(ctx);
    let install_mode = ctx.install_mode.as_str();

    let state_path = layout.state_dir.join("installed.toml");
    let state = InstalledState::load(&state_path).map_err(|err| CliError::Runtime {
        command: command.clone(),
        reason: format!(
            "failed to load installed state at {}: {err}",
            state_path.display()
        ),
    })?;

    let resolved = common::lookup_component_name(&args.component, &state, ctx, &command);

    let comp = state
        .find_object(ObjectKind::Component, &resolved)
        .ok_or_else(|| CliError::InvalidArgument {
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
        return render_guidance_only(&resolved, install_mode, warnings, ctx);
    }

    let env = EnvService::detect();
    // Restart routes each unit through the manager for its own scope — the
    // same per-scope partitioning uninstall uses. System units drive
    // `systemctl`, user units drive `systemctl --user`, so a mixed-scope
    // component never mis-drives a user unit through the system manager (or
    // vice versa): a unit whose scope has no driver here is a per-unit
    // `not_supported` skip rather than a wrong-namespace call.
    let sys_manager = service_factory(install_mode, &env);
    let user_manager = user_service_factory(install_mode, &env);

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

    for u in &units {
        let manager: &dyn ServiceManager = match u.scope {
            ServiceScope::System => sys_manager.as_ref(),
            ServiceScope::User => user_manager.as_ref(),
        };
        if !manager.supported() {
            // Quiet skip: this unit's scope has no driver here (a user unit
            // in a system-mode restart, container, non-Linux). Reported
            // `not_supported` per unit so the boundary is explicit and the
            // unit is never mis-driven through another namespace.
            let reason = manager
                .unsupported_reason()
                .unwrap_or("service manager not supported in this environment")
                .to_string();
            results.push(RestartResult {
                component: u.component.clone(),
                unit: u.unit.clone(),
                state: "not_supported".to_string(),
                changed: false,
                manager: manager.manager().to_string(),
                message: reason,
            });
            continue;
        }
        match manager.restart_service(&u.unit) {
            Ok(outcome) => {
                results.push(RestartResult {
                    component: u.component.clone(),
                    unit: u.unit.clone(),
                    state: outcome.state.as_str().to_string(),
                    changed: outcome.changed,
                    manager: outcome.manager,
                    message: outcome.message,
                });
                if matches!(outcome.state, ServiceState::Failed | ServiceState::Unknown) {
                    warnings.push(format!(
                        "{}/{} reports state '{}' after restart",
                        u.component,
                        u.unit,
                        outcome.state.as_str()
                    ));
                }
            }
            Err(err) => {
                let msg = format!("{err}");
                warnings.push(format!(
                    "service restart skipped for {}/{}: {msg}",
                    u.component, u.unit
                ));
                results.push(RestartResult {
                    component: u.component.clone(),
                    unit: u.unit.clone(),
                    state: "unknown".to_string(),
                    changed: false,
                    manager: manager.manager().to_string(),
                    message: msg,
                });
            }
        }
    }

    if ctx.json {
        let payload = RestartPayload {
            component: resolved.clone(),
            install_mode: install_mode.to_string(),
            manager: manager_label.clone(),
            supported,
            units: results.clone(),
            warnings: warnings.clone(),
        };
        return render_json(COMMAND, &payload);
    }

    if !ctx.quiet {
        render_human(
            &resolved,
            &manager_label,
            supported,
            &results,
            &warnings,
            ctx.no_color,
        );
    }
    Ok(())
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
    comp: &InstalledObject,
    component: &str,
) -> Result<(Vec<RestartUnit>, Vec<String>), CliError> {
    if comp.effective_ownership().is_rpm() {
        discover_rpm_units(comp, component, &RpmPackageQuery::system())
    } else {
        // Raw: the recorded ServiceRefs (restartable is hardcoded true today;
        // the filter keeps the door open for an explicit opt-out later).
        let units = comp
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
    comp: &InstalledObject,
    component: &str,
    query: &RpmPackageQuery<R>,
) -> Result<(Vec<RestartUnit>, Vec<String>), CliError> {
    let command = format!("restart {component}");
    let package = comp
        .rpm_metadata
        .as_ref()
        .map(|m| m.package_name.as_str())
        .ok_or_else(|| CliError::Runtime {
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
) -> RestartPayload {
    RestartPayload {
        component: component.to_string(),
        install_mode: install_mode.to_string(),
        manager: GUIDANCE_MANAGER.to_string(),
        supported: true,
        units: Vec::new(),
        warnings,
    }
}

/// Successful guidance-only outcome: the component ships only
/// template units restart cannot expand, so there is nothing to drive but the
/// package is healthy. Exits 0 with the per-user guidance as warnings.
fn render_guidance_only(
    component: &str,
    install_mode: &str,
    warnings: Vec<String>,
    ctx: &CliContext,
) -> Result<(), CliError> {
    if ctx.json {
        let payload = guidance_only_payload(component, install_mode, warnings);
        return render_json(COMMAND, &payload);
    }
    if !ctx.quiet {
        let color = Palette::new(ctx.no_color);
        println!(
            "{} {} {}",
            color.command("restart"),
            component,
            color.warn("no instances to restart")
        );
        for w in &warnings {
            eprintln!("{} {}", color.warn("guidance:"), w);
        }
    }
    Ok(())
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

#[derive(serde::Serialize)]
struct RestartPayload {
    component: String,
    install_mode: String,
    manager: String,
    supported: bool,
    units: Vec<RestartResult>,
    warnings: Vec<String>,
}

fn render_human(
    component: &str,
    manager_label: &str,
    supported: bool,
    results: &[RestartResult],
    warnings: &[String],
    no_color: bool,
) {
    let color = Palette::new(no_color);
    if supported {
        println!(
            "{} {} {}",
            color.command("restart"),
            component,
            color.ok("dispatched")
        );
    } else {
        println!(
            "{} {} {} {}",
            color.command("restart"),
            component,
            color.warn("skipped"),
            color.muted(format!("(manager={manager_label} unsupported)"))
        );
    }
    println!("{} {}", color.label("manager:"), manager_label);
    if !results.is_empty() {
        println!("{}", color.header("units:"));
        for r in results {
            println!(
                "  - {}/{} {} (changed={})",
                r.component,
                r.unit,
                color.status(&r.state),
                color.bool_value(r.changed),
            );
        }
    }
    for w in warnings {
        eprintln!("{} {}", color.warn("warning:"), w);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::context::InstallMode;
    use anolisa_core::{ObjectStatus, Ownership, RpmMetadata};
    use anolisa_platform::command::CommandOutput;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn ctx_with_prefix(install_mode: InstallMode, prefix: Option<PathBuf>) -> CliContext {
        CliContext {
            install_mode,
            prefix,
            json: false,
            dry_run: false,
            verbose: false,
            quiet: true,
            no_color: true,
        }
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

    /// An rpm-observed component object; `package = None` omits rpm metadata.
    fn rpm_component(name: &str, package: Option<&str>) -> InstalledObject {
        InstalledObject {
            kind: ObjectKind::Component,
            name: name.to_string(),
            version: "1.0.0-1".to_string(),
            status: ObjectStatus::Adopted,
            manifest_digest: None,
            distribution_source: None,
            raw_package: None,
            install_backend: Some("rpm".to_string()),
            ownership: Some(Ownership::RpmObserved),
            rpm_metadata: package.map(|p| RpmMetadata {
                package_name: p.to_string(),
                evr: Some("1.0.0-1".to_string()),
                arch: Some("x86_64".to_string()),
                source_repo: Some("@System".to_string()),
            }),
            installed_at: "2026-06-01T10:00:00Z".to_string(),
            last_operation_id: None,
            managed: false,
            adopted: true,
            subscription_scope: Default::default(),
            enabled_features: Vec::new(),
            component_refs: Vec::new(),
            files: Vec::new(),
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
            provisioned_packages: Vec::new(),
        }
    }

    #[test]
    fn restart_unknown_component_returns_invalid_argument() {
        let tmp = tempdir().expect("tmpdir");
        let err = handle(
            RestartArgs {
                component: "agentsight".to_string(),
            },
            &ctx_with_prefix(InstallMode::System, Some(tmp.path().to_path_buf())),
        )
        .expect_err("must error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert_eq!(err.exit_code(), 2);
        assert!(
            err.reason().contains("not installed"),
            "reason must mention 'not installed': {}",
            err.reason()
        );
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
        let res = render_guidance_only("agent-memory", "system", warnings, &ctx);
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
        let payload = guidance_only_payload("agent-memory", "system", warnings);
        assert_eq!(payload.manager, "none");
        assert!(payload.supported);
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
        let comp = rpm_component("agent-memory", Some("agent-memory"));
        let (units, notes) =
            discover_rpm_units(&comp, "agent-memory", &fake_query(&listing)).expect("discovery ok");

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
        let comp = rpm_component("agent-memory", None);
        let err = discover_rpm_units(&comp, "agent-memory", &fake_query(""))
            .expect_err("missing package name must error");
        assert!(err.reason().contains("repair"), "{}", err.reason());
    }

    #[test]
    fn discover_rpm_units_tooling_missing_maps_to_actionable_error() {
        // `rpm` absent (spawn NotFound) → the uniform rpm/dnf-not-found message,
        // not a generic "command not found".
        let comp = rpm_component("agent-memory", Some("agent-memory"));
        let query = RpmPackageQuery::with_runner(FakeRpm {
            stdout: String::new(),
            code: None,
            spawn_err: Some(std::io::ErrorKind::NotFound),
        });
        let err = discover_rpm_units(&comp, "agent-memory", &query)
            .expect_err("missing rpm tooling must error");
        assert!(
            err.reason().contains("rpm/dnf not found"),
            "{}",
            err.reason()
        );
    }
}
