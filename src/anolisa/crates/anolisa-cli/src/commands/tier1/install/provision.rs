//! Runtime-dependency preflight and auto-provisioning for the `install`
//! command.

use anolisa_core::{
    ComponentManifest, DependencyResolver, DependencyStatus, ProvisionPlan, ProvisionStrategy,
    ResolverEnv,
};
use anolisa_platform::package_manager::detect_package_manager;

use crate::context::{CliContext, InstallMode};
use crate::response::CliError;

/// Project detected host facts onto the slice the dependency resolver needs.
pub(crate) fn resolver_env_from_facts(facts: &anolisa_env::EnvFacts) -> ResolverEnv {
    ResolverEnv {
        kernel: facts.kernel.clone(),
        // `os_id` (raw `/etc/os-release` ID) maps to the coarse rpm/deb family;
        // the legacy `EnvFacts::pkg_base` is Anolis-specific and unsuitable here.
        pkg_base: facts
            .os_id
            .as_deref()
            .and_then(anolisa_env::pkg_base_from_id),
        btf: facts.btf,
        cap_bpf: facts.cap_bpf,
    }
}

/// Runtime-dependency preflight shared by the fresh-install (`execute_raw`) and
/// update (`execute_raw_update`) paths. Probes every declared dependency
/// through the system resolver and returns the satisfied plan's (soft)
/// warnings, or an error listing every miss so the caller can refuse **before
/// touching the host**. Empty `runtime_deps` is a no-op. The RPM backend never
/// calls this — dnf owns its `Requires`, so a dependency is never resolved
/// twice. Pure probe: never mutates.
pub(crate) fn run_runtime_preflight(
    manifest: &ComponentManifest,
    env: &anolisa_env::EnvFacts,
    command: &str,
) -> Result<Vec<String>, CliError> {
    if manifest.runtime_deps.is_empty() {
        return Ok(Vec::new());
    }
    let plan = DependencyResolver::system()
        .resolve(&manifest.runtime_deps, &resolver_env_from_facts(env))
        .map_err(|err| CliError::Runtime {
            command: command.to_string(),
            reason: format!("invalid runtime dependency declaration: {err}"),
        })?;
    if !plan.is_satisfied() {
        return Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "missing runtime dependencies; no files were changed:\n  {}",
                plan.unsatisfied_lines().join("\n  ")
            ),
        });
    }
    Ok(plan.warnings)
}

/// Provision-aware dependency handling that replaces the old fail-fast
/// `run_runtime_preflight` in the `execute_raw` path.
///
/// Behavior depends on `ctx.install_mode`:
/// - **System**: auto-install missing system packages via the host package
///   manager, then re-verify only the provisioned deps. Manual-only deps
///   (e.g. `language-runtime` without a `packages` mapping) remain
///   non-blocking warnings. Unresolvable platform capabilities fail fast.
/// - **User**: report missing deps with remediation commands and return an
///   error (the caller should exit without modifying the host).
///
/// Returns the list of package names that were auto-installed (empty in user
/// mode or when all deps were already satisfied).
pub(crate) fn run_provision(
    manifest: &ComponentManifest,
    env: &anolisa_env::EnvFacts,
    ctx: &CliContext,
    command: &str,
    warnings: &mut Vec<String>,
) -> Result<Vec<String>, CliError> {
    if manifest.runtime_deps.is_empty() {
        return Ok(Vec::new());
    }

    let resolver_env = resolver_env_from_facts(env);
    let plan = DependencyResolver::system()
        .resolve(&manifest.runtime_deps, &resolver_env)
        .map_err(|err| CliError::Runtime {
            command: command.to_string(),
            reason: format!("invalid runtime dependency declaration: {err}"),
        })?;
    warnings.extend(plan.warnings.clone());

    // Classify the resolver results into a provision plan.
    let provision = ProvisionPlan::from_resolution(&plan, &manifest.runtime_deps, &resolver_env);

    // Unresolvable deps (platform capabilities) are always fatal.
    if provision.has_blockers() {
        let lines: Vec<String> = provision
            .unresolvable
            .iter()
            .map(|u| format!("  {} [unresolvable]: {}", u.name, u.reason))
            .collect();
        return Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "unsatisfiable platform requirements; no files were changed:\n{}",
                lines.join("\n")
            ),
        });
    }

    // If everything is satisfied, nothing to do.
    if provision.is_satisfied() {
        return Ok(Vec::new());
    }

    // Select strategy based on install mode.
    let strategy = select_provision_strategy(ctx);

    match strategy {
        ProvisionStrategy::ReportAndExit => {
            // User mode: report missing deps and exit.
            let mut lines = Vec::new();
            for pkg in &provision.installable {
                lines.push(format!("  {} (not installed)", pkg.name));
            }
            for dep in &provision.manual {
                lines.push(format!("  {} (manual): {}", dep.name, dep.hint));
            }

            let remediation_cmds: Vec<&str> = provision
                .installable
                .iter()
                .map(|p| p.remediation.as_str())
                .collect();

            let mut reason = format!(
                "missing system dependencies in user mode; no files were changed:\n{}",
                lines.join("\n")
            );
            if !remediation_cmds.is_empty() {
                reason.push_str(&format!(
                    "\n\nInstall them with:\n  {}\n\nThen retry:\n  anolisa install {}",
                    remediation_cmds.join("\n  "),
                    manifest.component.name
                ));
            }

            Err(CliError::Runtime {
                command: command.to_string(),
                reason,
            })
        }
        ProvisionStrategy::Auto => {
            // System mode: auto-install missing packages.
            if !provision.has_installable() {
                // Only manual deps remain; warn but continue.
                for dep in &provision.manual {
                    warnings.push(format!(
                        "dependency '{}' requires manual installation: {}",
                        dep.name, dep.hint
                    ));
                }
                return Ok(Vec::new());
            }

            let pkg_names = provision.installable_package_names();
            let pkg_base = resolver_env.pkg_base.as_deref();

            // Detect the host package manager.
            let mgr = detect_package_manager(pkg_base).map_err(|err| CliError::Runtime {
                command: command.to_string(),
                reason: format!(
                    "cannot auto-install dependencies: {err}; install manually:\n  {}",
                    provision
                        .installable
                        .iter()
                        .map(|p| p.remediation.as_str())
                        .collect::<Vec<_>>()
                        .join("\n  ")
                ),
            })?;

            // Execute the install.
            mgr.install(&pkg_names).map_err(|err| CliError::Runtime {
                command: command.to_string(),
                reason: format!("failed to install system dependencies: {err}"),
            })?;

            // Re-verify only the provisioned deps (manual deps stay as warnings).
            let recheck = DependencyResolver::system()
                .resolve(&manifest.runtime_deps, &resolver_env)
                .map_err(|err| CliError::Runtime {
                    command: command.to_string(),
                    reason: format!("dependency re-verification failed: {err}"),
                })?;
            let provisioned_dep_names: std::collections::HashSet<&str> = provision
                .installable
                .iter()
                .map(|p| p.name.as_str())
                .collect();
            let still_failed: Vec<String> = recheck
                .resolutions
                .iter()
                .filter(|r| !matches!(r.status, DependencyStatus::Resolved))
                .filter(|r| {
                    // Only fail on deps we actually tried to provision.
                    provisioned_dep_names.contains(r.name.as_str())
                })
                .map(|r| format!("{} [{}]", r.name, r.kind.as_str()))
                .collect();
            if !still_failed.is_empty() {
                let installed_names: Vec<String> =
                    pkg_names.iter().map(|s| s.to_string()).collect();
                let note = retained_packages_note(&installed_names);
                return Err(CliError::Runtime {
                    command: command.to_string(),
                    reason: format!(
                        "dependencies still unsatisfied after install:\n  {}{note}",
                        still_failed.join("\n  ")
                    ),
                });
            }

            // Warn about manual deps.
            for dep in &provision.manual {
                warnings.push(format!(
                    "dependency '{}' requires manual installation: {}",
                    dep.name, dep.hint
                ));
            }

            let installed: Vec<String> = pkg_names.iter().map(|s| s.to_string()).collect();
            Ok(installed)
        }
    }
}

/// Build the note suffix appended to error messages when system packages were
/// provisioned but the install did not complete. Returns an empty string when
/// no packages were installed.
pub(crate) fn retained_packages_note(provisioned: &[String]) -> String {
    if provisioned.is_empty() {
        String::new()
    } else {
        format!(
            "\n\nnote: system packages were installed and retained: {}",
            provisioned.join(", ")
        )
    }
}

/// Select provision strategy based on install mode.
pub(crate) fn select_provision_strategy(ctx: &CliContext) -> ProvisionStrategy {
    if ctx.install_mode == InstallMode::System {
        ProvisionStrategy::Auto
    } else {
        ProvisionStrategy::ReportAndExit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_packages_note_empty_when_no_packages() {
        assert_eq!(retained_packages_note(&[]), "");
    }

    #[test]
    fn retained_packages_note_lists_provisioned_packages() {
        let pkgs = vec!["nodejs".to_string(), "jq".to_string()];
        let note = retained_packages_note(&pkgs);
        assert!(note.contains("system packages were installed and retained"));
        assert!(note.contains("nodejs"));
        assert!(note.contains("jq"));
    }
}
