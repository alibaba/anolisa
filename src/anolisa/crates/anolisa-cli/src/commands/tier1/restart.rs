//! `anolisa restart <component>` — restart owned service units.
//!
//! Best-effort restart of every `services[]` entry on the component
//! where `restartable = true`. The handler:
//!
//!   1. Loads `installed.toml` and locates the component. Unknown →
//!      `INVALID_ARGUMENT`.
//!   2. Collects the component's restartable service units.
//!   3. If the set is empty → `INVALID_ARGUMENT` (nothing to restart).
//!   4. Routes each unit through the [`anolisa_core::ServiceManager`] for
//!      its own scope — system units via `systemctl`, user units via
//!      `systemctl --user` — the same per-scope partitioning uninstall uses.
//!      A unit whose scope has no driver here (a user unit in a system-mode
//!      restart, non-Linux, container) is a per-unit `not_supported` skip,
//!      never mis-driven through another namespace.
//!   5. Calls `restart_service(unit)` per unit. Per-unit failures are
//!      collected as warnings on the outcome; a unit that systemctl
//!      refuses does NOT abort the whole op.
//!
//! Restart is intentionally lock-free: it does not mutate
//! `installed.toml` and it is safe to run concurrently with other
//! ANOLISA invocations. If we later add a "record last_restart_at"
//! field on `ServiceRef`, this handler will need to take the install
//! lock around the state write.

use clap::Parser;

use anolisa_core::{
    InstalledState, ObjectKind, ServiceManager, ServiceScope, ServiceState,
    service_for_install_mode as service_factory,
    user_service_for_install_mode as user_service_factory,
};
use anolisa_env::EnvService;

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

    let comp = state
        .find_object(ObjectKind::Component, &args.component)
        .ok_or_else(|| CliError::InvalidArgument {
            command: command.clone(),
            reason: format!(
                "component '{}' is not installed — nothing to restart (run `anolisa status` to see what is installed)",
                args.component
            ),
        })?;

    // A service with `restartable = false` (one-shot setup unit, timer,
    // etc.) is silently filtered out here — the manifest opts that unit
    // out of `restart` semantics explicitly.
    let units: Vec<RestartUnit> = comp
        .services
        .iter()
        .filter(|svc| svc.restartable)
        .map(|svc| RestartUnit {
            component: args.component.clone(),
            unit: svc.name.clone(),
            scope: svc.scope,
        })
        .collect();

    if units.is_empty() {
        return Err(CliError::InvalidArgument {
            command,
            reason: format!(
                "component '{}' has no restartable service units (no `services[]` with `restartable = true`)",
                args.component
            ),
        });
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

    let mut results: Vec<RestartResult> = Vec::with_capacity(units.len());
    let mut warnings: Vec<String> = Vec::new();

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
            component: args.component.clone(),
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
            &args.component,
            &manager_label,
            supported,
            &results,
            &warnings,
            ctx.no_color,
        );
    }
    Ok(())
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
}
