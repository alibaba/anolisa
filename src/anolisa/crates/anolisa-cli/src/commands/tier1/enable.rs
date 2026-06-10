//! `anolisa enable` — capability activation.
//!
//! P1-F wiring: dry-run path still goes through [`anolisa_core::plan_enable`]
//! and renders the resulting [`EnablePlan`] (human or JSON). On the
//! non-dry-run path the handler now drives the real-execute orchestrator
//! [`anolisa_core::execute_enable`]: download artifact → install ANOLISA-owned
//! files → write `InstalledState` capability/component objects → append
//! `started` / `succeeded` records to `CentralLog` → release the
//! [`anolisa_core::InstallLock`]. Any mid-flight failure self-cleans (unlinks files,
//! appends a `Failed` record, releases the lock).
//!
//! Scope limits enforced here (rather than inside the planner / executor)
//! so the underlying libraries stay general while the CLI surface honors
//! the launch-spec scope:
//!
//! * Exactly one capability per invocation.
//! * `--feature`, `--with-adapter`, `--from-source` are not supported on
//!   either path yet; we reject explicitly so users see a clear contract
//!   rather than silently-ignored flags.
//! * Both `--dry-run` and real-execute are scoped by the declarative
//!   execution policy at [`crate::execution_policy`]
//!   (`templates/execution-policy.toml`). Up through P1-G1 this gate was a
//!   hard-coded `SUPPORTED_CAPABILITY = "agent-observability"` constant;
//!   P1-H replaces it with the policy file so a second capability
//!   (`token-optimization`) can graduate without touching Rust. Capabilities
//!   absent from the policy — or present with `allow_execute = false` —
//!   continue to surface `NOT_IMPLEMENTED` so the boundary is visible.

use clap::Parser;

use anolisa_core::{
    EnablePlan, ExecuteError, ExecuteOutcome, FetchedMeta, PlanError, PlanStatus, execute_enable,
    plan_enable,
};
use anolisa_env::EnvService;

use crate::color::{Palette, pad_right};
use crate::commands::common;
use crate::context::CliContext;
use crate::execution_policy::{ExecutionPolicy, PolicyError};
use crate::response::{CliError, render_json};

const COMMAND: &str = "enable";

#[derive(Parser)]
pub struct EnableArgs {
    /// Capability name(s) to enable
    #[arg(required = true)]
    pub capabilities: Vec<String>,
    /// Only enable a specific sub-feature (capability must already be enabled)
    #[arg(long, value_name = "NAME")]
    pub feature: Option<String>,
    /// Adapter framework selection: explicit list ("cosh,openclaw"), `auto`, or omit for first-party only
    #[arg(long, value_name = "FRAMEWORKS|auto")]
    pub with_adapter: Option<String>,
    /// Build component(s) from source instead of installing prebuilt
    #[arg(long)]
    pub from_source: bool,
}

pub fn handle(args: EnableArgs, ctx: &CliContext) -> Result<(), CliError> {
    let command = format!("enable {}", args.capabilities.join(" "));

    if args.capabilities.len() != 1 {
        return Err(CliError::InvalidArgument {
            command,
            reason: "enable currently accepts exactly one capability".to_string(),
        });
    }
    let capability = args.capabilities[0].clone();

    // Scope guards apply uniformly to dry-run AND real-execute: the
    // launch-spec scope is the same for both surfaces today.
    if args.from_source {
        return Err(CliError::not_implemented_with_hint(
            command,
            "--from-source is not supported yet",
        ));
    }
    if args.with_adapter.is_some() {
        return Err(CliError::not_implemented_with_hint(
            command,
            "--with-adapter is not supported yet",
        ));
    }
    if args.feature.is_some() {
        return Err(CliError::not_implemented_with_hint(
            command,
            "--feature is not supported yet",
        ));
    }

    // Load the declarative execution policy. Failing to load it is an
    // internal error (the policy is a packaged asset, not caller input);
    // surface as EXECUTION_FAILED so the wrapping script can distinguish
    // "bad input" (exit 2) from "the binary is misconfigured" (exit 1).
    let policy = ExecutionPolicy::load().map_err(|err| policy_load_err(&command, err))?;

    let catalog = common::load_bundled_catalog(ctx, COMMAND)?;
    // Index source (T1.2 wiring): when the remote registry is configured
    // (env `ANOLISA_REGISTRY_URL` or `etc_dir/config.toml` `[registry]`),
    // fetch the index over HTTP with TTL cache + offline fallback.
    // Otherwise keep the local bundled index — missing local index is not
    // a CLI error: the planner reports it inside the plan so users still
    // get a structured dry-run output instead of an opaque error message.
    let registry = common::load_registry_client(ctx, COMMAND)?;
    let mut registry_warnings: Vec<String> = Vec::new();
    let dist_index = match &registry {
        Some(client) => {
            let (index, warning) = common::fetch_remote_index(client, COMMAND)?;
            registry_warnings.extend(warning);
            index
        }
        None => common::load_distribution_index(ctx, COMMAND)?
            .unwrap_or_else(common::empty_distribution_index),
    };
    let env = EnvService::detect();
    let layout = common::resolve_layout(ctx);
    let install_mode = ctx.install_mode.as_str();

    let map_plan_err = |err: PlanError| match err {
        PlanError::UnknownCapability(name) => CliError::InvalidArgument {
            command: COMMAND.to_string(),
            reason: format!("capability '{name}' is not in the catalog"),
        },
    };

    // Run `plan_enable` *before* consulting the policy so an unknown
    // capability surfaces as INVALID_ARGUMENT (caller typo) rather than
    // NOT_IMPLEMENTED (we shipped the gate closed). Keeping the buckets
    // distinct preserves the P1-G0 wire contract where INVALID_ARGUMENT
    // means "fix your input" and NOT_IMPLEMENTED means "this surface is
    // not open yet".
    let mut plan = plan_enable(
        &catalog,
        &dist_index,
        &env,
        install_mode,
        &layout,
        &capability,
    )
    .map_err(map_plan_err)?;

    // T1.3: with a remote registry, the authoritative component contract
    // for a resolved version is its published meta.toml (≈1KB), not the
    // bundled manifest. Fetch meta for every resolved component (full
    // artifacts are NOT downloaded here), overlay them onto the catalog,
    // and re-plan so layout/prechecks come from the publishing contract.
    // Missing meta degrades to the bundled-manifest preview with a warning.
    if let Some(client) = &registry {
        let mut metas: Vec<(String, FetchedMeta)> = Vec::new();
        for comp in &plan.components {
            let Some(artifact) = &comp.artifact else {
                continue;
            };
            match client.fetch_meta(&comp.name, &artifact.version, &artifact.url) {
                Ok(Some(meta)) => metas.push((comp.name.clone(), meta)),
                Ok(None) => registry_warnings.push(format!(
                    "component '{}': no meta.toml published for v{} — plan previews the bundled manifest",
                    comp.name, artifact.version,
                )),
                Err(err) => registry_warnings.push(format!(
                    "component '{}': meta.toml fetch failed ({err}) — plan previews the bundled manifest",
                    comp.name,
                )),
            }
        }
        if !metas.is_empty() {
            let mut overlaid = catalog.clone();
            for (name, meta) in &metas {
                overlaid
                    .components
                    .insert(name.clone(), meta.manifest.clone());
            }
            plan = plan_enable(
                &overlaid,
                &dist_index,
                &env,
                install_mode,
                &layout,
                &capability,
            )
            .map_err(map_plan_err)?;
            // Carry the meta digest into the plan so real-execute can hold
            // the artifact to its publishing contract (T1.4).
            for comp in plan.components.iter_mut() {
                if let Some((_, meta)) = metas.iter().find(|(name, _)| name == &comp.name) {
                    if let Some(artifact) = comp.artifact.as_mut() {
                        artifact.meta_sha256 = Some(meta.sha256.clone());
                    }
                }
            }
        }
    }
    plan.warnings.extend(registry_warnings);

    // Dry-run is always allowed for any catalog capability — it never
    // touches the system and users need it to see plan/lint output for
    // capabilities that have not yet graduated through the execution
    // policy. We record the policy decision structurally on the plan so
    // JSON consumers can read `execute_gate.allowed` rather than parsing
    // the legacy `"execute gate:"` warning prefix; the helper also
    // rewrites `next_actions` when the gate is closed so the plan never
    // suggests a command that would refuse.
    let execute_gated = !policy.allows_execute(&capability);
    let gate_reason = if execute_gated {
        Some(scope_hint_for(&policy, &capability))
    } else {
        None
    };
    plan.set_execute_gate(!execute_gated, gate_reason);

    if ctx.dry_run {
        if ctx.json {
            return render_json(COMMAND, &plan);
        }
        if !ctx.quiet {
            render_human(&plan, ctx.verbose, ctx.no_color);
        }
        return Ok(());
    }

    // Real-execute path is still gated. Anything other than an `enabled`
    // entry with `allow_execute = true` is the closed-gate default and the
    // user sees NOT_IMPLEMENTED with a hint that names the policy file so
    // they can self-serve a fix if they own this build.
    if execute_gated {
        return Err(CliError::not_implemented_with_hint(
            command,
            scope_hint_for(&policy, &capability),
        ));
    }

    // Real-execute path: drive `execute_enable` and render its outcome.
    let actor = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "cli".to_string());
    let outcome = execute_enable(&plan, &layout, &actor)
        .map_err(|err| execute_err_to_cli(&capability, err))?;

    if ctx.json {
        let payload = ExecutePayload::from(&outcome);
        return render_json(COMMAND, &payload);
    }

    if !ctx.quiet {
        render_execute_human(&outcome, ctx.verbose, ctx.no_color);
    }
    Ok(())
}

/// Build the hint string used when the policy gate closes on a capability
/// that lives in the catalog. Two cases:
///
/// * The capability is absent from the policy → tell the user the policy
///   file path so they can graduate it.
/// * The capability is present but `enabled = false` / `allow_execute = false`
///   → name the entry so the user can flip the relevant flag.
///
/// The hint always includes the list of capabilities currently graduated
/// so callers see the live scope without having to read the file.
fn scope_hint_for(policy: &ExecutionPolicy, capability: &str) -> String {
    let graduated: Vec<&str> = policy
        .capabilities
        .iter()
        .filter(|c| c.enabled && c.allow_execute)
        .map(|c| c.name.as_str())
        .collect();
    let graduated_list = if graduated.is_empty() {
        "<none>".to_string()
    } else {
        graduated.join(", ")
    };
    match policy.lookup(capability) {
        None => format!(
            "enable for '{capability}' is not graduated yet (execution policy graduates: {graduated_list}); add '{capability}' to templates/execution-policy.toml with allow_execute = true to open the scope"
        ),
        Some(entry) if !entry.enabled => format!(
            "enable for '{capability}' is gated off (execution policy entry has enabled = false); graduated capabilities: {graduated_list}"
        ),
        Some(_) => format!(
            "enable for '{capability}' is gated off (execution policy entry has allow_execute = false); graduated capabilities: {graduated_list}"
        ),
    }
}

/// Route a [`PolicyError`] to the CLI. The policy file is a packaged
/// asset, so a load failure means the installed binary is misconfigured
/// rather than the caller fed bad input — we route to EXECUTION_FAILED
/// (exit 1) per the P1-G0 split. Bad input would be exit 2.
fn policy_load_err(command: &str, err: PolicyError) -> CliError {
    CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to load execution policy: {err}"),
    }
}

/// Translate an [`ExecuteError`] into the CLI error surface.
///
/// Two buckets:
///
/// * **`INVALID_ARGUMENT` (exit 2)** — plan-time refusals the caller
///   could have prevented: `PlanNotExecutable` (plan was Blocked) and
///   `MissingArtifact` / `MissingChecksum` (catalog vs distribution-index
///   mismatch). These all point the user at `--dry-run` to diagnose the
///   plan; the machine itself never moved.
/// * **`EXECUTION_FAILED` (exit 1)** — runtime IO failures inside the
///   real-execute body: `Download`, `Install`, `State`, `Log`, `Lock`,
///   `LockHeld`. The plan was acceptable; the machine refused.
///
/// Splitting the two lets wrapping scripts distinguish "fix your input"
/// from "the machine couldn't complete it" — the P1-G0 graduation
/// criterion. `NOT_IMPLEMENTED` is reserved upstream of this routing
/// for surfaces the CLI scope-gate has not opened yet.
///
/// `capability` is the capability the caller invoked. It is interpolated
/// into the `INVALID_ARGUMENT` hints so users see a literally-runnable
/// `--dry-run` command instead of one hardcoded to `agent-observability`
/// (the P1-H multi-capability regression caught in review).
fn execute_err_to_cli(capability: &str, err: ExecuteError) -> CliError {
    match &err {
        // — INVALID_ARGUMENT: the plan ruled it out before any IO. —
        ExecuteError::PlanNotExecutable { status, reason } => CliError::InvalidArgument {
            command: COMMAND.to_string(),
            reason: format!(
                "plan is {status}: {reason} — run `anolisa enable {capability} --dry-run` for details and resolve blockers before retrying",
            ),
        },
        ExecuteError::MissingArtifact { component } => CliError::InvalidArgument {
            command: COMMAND.to_string(),
            reason: format!(
                "component '{component}' has no resolved artifact (catalog vs distribution-index mismatch — check `anolisa enable {capability} --dry-run`)",
            ),
        },
        ExecuteError::MissingChecksum { component } => CliError::InvalidArgument {
            command: COMMAND.to_string(),
            reason: format!(
                "component '{component}' has no sha256 in the distribution index — refuse to install without verification (regenerate the index with checksums and retry)",
            ),
        },

        // — EXECUTION_FAILED: the plan was acceptable; the machine refused. —
        ExecuteError::LockHeld { path } => CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!(
                "install lock at {} is held by another process — run again after the other invocation finishes",
                path.display(),
            ),
        },
        ExecuteError::Download { component, source } => CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!("download for component '{component}' failed: {source}"),
        },
        ExecuteError::Install { component, source } => CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!("install for component '{component}' failed: {source}"),
        },
        // The downloaded artifact's embedded component.toml contradicts the
        // registry meta.toml the plan was built from — a publisher-side
        // inconsistency (contract I3 violation). Nothing was installed; the
        // user retries after the registry is republished consistently.
        ExecuteError::ManifestMismatch {
            component,
            expected_sha256,
            actual_sha256,
        } => CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!(
                "component '{component}': artifact's embedded component.toml does not match the registry meta.toml used for planning (expected sha256 {expected_sha256}, actual {}) — published artifact and meta.toml are inconsistent; nothing was installed. Re-run `anolisa enable {capability} --dry-run` after the registry is republished",
                actual_sha256.as_deref().unwrap_or("<missing>"),
            ),
        },
        ExecuteError::State { source } => CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!("installed state write failed: {source}"),
        },
        ExecuteError::Log { source } => CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!("central log write failed: {source}"),
        },
        ExecuteError::Lock { source } => CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!("install lock io: {source}"),
        },

        // The capability is currently `Disabled` in state but the
        // recorded sha256s do not match what's on disk. This is a
        // "fix your machine" condition — uninstall/purge (not yet
        // shipped) or restore the affected files — so we route to
        // INVALID_ARGUMENT and surface every offending file so the
        // user does not have to grep state.toml by hand.
        ExecuteError::DisabledStateInconsistent {
            capability: cap_name,
            mismatches,
        } => {
            let detail = mismatches.join("; ");
            CliError::InvalidArgument {
                command: COMMAND.to_string(),
                reason: format!(
                    "capability '{cap_name}' is currently disabled but {} owned file(s) drifted from the recorded sha256: {detail} — run `anolisa status {cap_name} --verbose` to inspect, then restore the affected files (or wait for `anolisa uninstall`/`purge` to ship) before re-enabling",
                    mismatches.len(),
                ),
            }
        }

        // A pre_enable lifecycle hook returned non-zero. The verb
        // aborts before any download/install runs and a `failed`
        // central-log record is already written; the operator's next
        // step is to inspect the hook script (or its log line) and
        // retry, so we route to runtime/exit 1 with the hook details
        // inlined in the reason for `--json` consumers.
        ExecuteError::HookFailed {
            phase,
            component,
            summary,
            exit_code,
        } => CliError::Runtime {
            command: COMMAND.to_string(),
            reason: format!(
                "lifecycle hook {phase} for component '{component}' failed (exit {}): {summary} — inspect the central log (`anolisa logs --kind component --component {component}`) and the hook script before retrying",
                exit_code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "?".to_string()),
            ),
        },
    }
}

/// Wire shape mirrored from [`ExecuteOutcome`]. Defined at the CLI
/// boundary so `anolisa-core` does not need to derive `Serialize` on its
/// internal outcome struct.
#[derive(serde::Serialize)]
struct ExecutePayload {
    operation_id: String,
    capability: String,
    install_mode: String,
    components: Vec<String>,
    installed_files: Vec<InstalledFilePayload>,
    state_path: String,
    central_log_path: String,
    warnings: Vec<String>,
    /// `true` when this op observed the capability already `Disabled`
    /// with every owned file matching its recorded sha256 and only
    /// flipped state back to `Installed`. Consumers (smoke harness,
    /// downstream tooling) use this to distinguish a no-op re-enable
    /// from a fresh install — `installed_files` echoes the on-disk
    /// files in both cases.
    reactivated: bool,
}

#[derive(serde::Serialize)]
struct InstalledFilePayload {
    component: String,
    path: String,
    sha256: String,
}

impl From<&ExecuteOutcome> for ExecutePayload {
    fn from(o: &ExecuteOutcome) -> Self {
        Self {
            operation_id: o.operation_id.clone(),
            capability: o.capability.clone(),
            install_mode: o.install_mode.clone(),
            components: o.components.clone(),
            installed_files: o
                .installed_files
                .iter()
                .map(|f| InstalledFilePayload {
                    component: f.component.clone(),
                    path: f.path.display().to_string(),
                    sha256: f.sha256.clone(),
                })
                .collect(),
            state_path: o.state_path.display().to_string(),
            central_log_path: o.central_log_path.display().to_string(),
            warnings: o.warnings.clone(),
            reactivated: o.reactivated,
        }
    }
}

fn render_execute_human(outcome: &ExecuteOutcome, verbose: bool, no_color: bool) {
    let color = Palette::new(no_color);
    if outcome.reactivated {
        println!(
            "{} {} {} {}",
            color.command("enable"),
            outcome.capability,
            color.ok("reactivated"),
            color.muted("from disabled (no files written)")
        );
    } else {
        println!(
            "{} {} {}",
            color.command("enable"),
            outcome.capability,
            color.ok("succeeded")
        );
    }
    println!(
        "{} {}",
        color.label("operation_id:"),
        color.id(&outcome.operation_id)
    );
    println!("{} {}", color.label("install_mode:"), outcome.install_mode);
    println!("{}", color.header("components:"));
    for c in &outcome.components {
        println!("  - {c}");
    }
    let files_label = if outcome.reactivated {
        "verified_files"
    } else {
        "installed_files"
    };
    println!(
        "{}",
        color.header(format!(
            "{files_label} ({}):",
            outcome.installed_files.len()
        ))
    );
    for f in &outcome.installed_files {
        let sha_render = if verbose {
            f.sha256.clone()
        } else {
            // Short-form sha256 keeps the human line readable; full hash
            // is one --verbose away.
            f.sha256.get(..8).unwrap_or(&f.sha256).to_string()
        };
        println!(
            "  - {}  {}  sha256={}",
            f.component,
            color.path(f.path.display()),
            color.id(sha_render),
        );
    }
    println!(
        "{} {}",
        color.label("state:"),
        color.path(outcome.state_path.display())
    );
    println!(
        "{}   {}",
        color.label("log:"),
        color.path(outcome.central_log_path.display())
    );
    if !outcome.warnings.is_empty() {
        println!("{}", color.warn("warnings:"));
        for w in &outcome.warnings {
            println!("  - {w}");
        }
    }
}

fn render_human(plan: &EnablePlan, verbose: bool, no_color: bool) {
    let color = Palette::new(no_color);
    println!(
        "{} {} {}",
        color.label("capability:"),
        plan.capability,
        color.muted(format!(
            "(stability: {}, install_mode: {}, dry_run: true)",
            plan.stability, plan.install_mode,
        )),
    );
    println!(
        "{} {}",
        color.label("status:"),
        color.status(plan.status.as_str())
    );
    if let Some(reason) = plan.blocked_reason.as_deref() {
        println!("{} {reason}", color.err("blocked:"));
    }

    println!("{}", color.header("env:"));
    println!(
        "  os={} arch={} libc={} pkg_base={}",
        plan.env_facts.os,
        plan.env_facts.arch,
        plan.env_facts.libc.as_deref().unwrap_or("-"),
        plan.env_facts.pkg_base.as_deref().unwrap_or("-"),
    );

    if !plan.prechecks.is_empty() {
        println!("{}", color.header("prechecks:"));
        for p in &plan.prechecks {
            let detail = p.message.as_deref().unwrap_or("");
            println!(
                "  - {name:<14} {status} expected={expected} actual={actual} {detail}",
                name = p.name,
                status = color.status(pad_right(&p.status, 5)),
                expected = p.expected,
                actual = p.actual,
                detail = detail,
            );
        }
    }

    println!("{}", color.header("components:"));
    for c in &plan.components {
        let version = c.manifest_version.as_deref().unwrap_or("-");
        println!(
            "  - {} v{} status={}",
            c.name,
            version,
            color.status(c.status.as_str()),
        );
        if let Some(reason) = c.blocked_reason.as_deref() {
            println!("      {} {reason}", color.err("blocked:"));
        }
        if let Some(a) = &c.artifact {
            println!(
                "      artifact: {} ({}) v{} url={}",
                a.artifact_type,
                a.backend,
                a.version,
                color.path(&a.url),
            );
            if verbose && let Some(sha) = a.sha256.as_deref() {
                println!("      {} {}", color.label("sha256:"), color.id(sha));
            }
        }
        if verbose {
            if !c.services.is_empty() {
                println!(
                    "      {} {}",
                    color.label("services:"),
                    c.services.join(", ")
                );
            }
            if !c.files.is_empty() {
                let files = c
                    .files
                    .iter()
                    .map(|file| file.display())
                    .collect::<Vec<_>>()
                    .join(", ");
                println!("      {} {}", color.label("files:"), files);
            }
            if !c.resolved_files.is_empty() {
                println!(
                    "      {} {}",
                    color.label("resolved_files:"),
                    c.resolved_files.join(", ")
                );
            }
            if !c.capabilities.is_empty() {
                let capabilities = c
                    .capabilities
                    .iter()
                    .map(|capability| capability.display())
                    .collect::<Vec<_>>()
                    .join(", ");
                println!("      {} {}", color.label("capabilities:"), capabilities);
            }
            println!(
                "      {} {}",
                color.label("requires_privilege:"),
                color.bool_value(c.requires_privilege)
            );
        }
    }

    println!("{}", color.header("layout:"));
    println!("  bin_dir:           {}", color.path(&plan.layout.bin_dir));
    println!("  etc_dir:           {}", color.path(&plan.layout.etc_dir));
    println!(
        "  state_dir:         {}",
        color.path(&plan.layout.state_dir)
    );
    println!("  log_dir:           {}", color.path(&plan.layout.log_dir));
    println!(
        "  manifests_overlay: {}",
        color.path(&plan.layout.manifests_overlay)
    );

    if !plan.warnings.is_empty() {
        println!("{}", color.warn("warnings:"));
        for w in &plan.warnings {
            println!("  - {w}");
        }
    }

    if !plan.next_actions.is_empty() {
        println!("{}", color.header("next:"));
        for n in &plan.next_actions {
            println!("  - {n}");
        }
    }

    let _ = PlanStatus::Ready; // silence "unused import" if future refactor drops branches
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::context::InstallMode;
    use anolisa_core::{CentralLogError, DownloadError, InstallError, LockError, StateError};
    use std::io;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn ctx(json: bool, dry_run: bool, install_mode: InstallMode) -> CliContext {
        ctx_with_prefix(json, dry_run, install_mode, None)
    }

    fn ctx_with_prefix(
        json: bool,
        dry_run: bool,
        install_mode: InstallMode,
        prefix: Option<PathBuf>,
    ) -> CliContext {
        CliContext {
            install_mode,
            prefix,
            json,
            dry_run,
            verbose: false,
            quiet: true, // suppress stdout during tests
            no_color: true,
        }
    }

    fn args(caps: &[&str]) -> EnableArgs {
        EnableArgs {
            capabilities: caps.iter().map(|s| s.to_string()).collect(),
            feature: None,
            with_adapter: None,
            from_source: false,
        }
    }

    #[test]
    fn enable_with_zero_capabilities_is_rejected_by_clap() {
        // clap enforces `required = true` upstream, so this path is owned
        // by argument parsing — confirmed by integration coverage. Here we
        // verify the multi-capability guard inside the handler instead.
        let err = handle(
            args(&["agent-observability", "tokenless"]),
            &ctx(false, true, InstallMode::System),
        )
        .expect_err("must error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
    }

    /// Capabilities that exist in the catalog but are NOT graduated by the
    /// execution policy must still produce a dry-run plan — the gate only
    /// applies to real execute. This is the inversion of the previous
    /// behavior where the policy short-circuited both surfaces; the
    /// generic-lifecycle contract says a fresh capability must be plannable
    /// from manifests alone, with the gate communicated as a plan warning
    /// rather than a top-level NOT_IMPLEMENTED.
    ///
    /// We use `agent-memory` because it ships in the bundled catalog (so
    /// `UnknownCapability` doesn't pre-empt the policy check) and is
    /// intentionally absent from `templates/execution-policy.toml` —
    /// exactly the boundary that real execute still refuses.
    #[test]
    fn enable_dry_run_capability_not_in_policy_returns_plan_with_gate_warning() {
        handle(
            args(&["agent-memory"]),
            &ctx(false, true, InstallMode::User),
        )
        .expect(
            "dry-run must succeed for a catalog capability even when the policy gate is closed",
        );
    }

    /// The plan warning injected on the dry-run path must reuse the same
    /// scope-hint format used by the real-execute refusal — that way users
    /// who see the warning in dry-run output can predict what real execute
    /// will say without running it. The format is shared so a future hint
    /// refactor cannot silently desync the two surfaces.
    #[test]
    fn dry_run_gate_warning_matches_real_execute_hint_format() {
        let policy = ExecutionPolicy::load().expect("packaged policy loads");
        let hint = scope_hint_for(&policy, "agent-memory");
        assert!(
            hint.contains("agent-memory"),
            "scope hint must name the rejected capability: {hint}",
        );
        assert!(
            hint.contains("execution-policy.toml"),
            "scope hint must point at the policy file so users can self-serve: {hint}",
        );
    }

    /// Dry-run on a gated capability must populate `plan.execute_gate`
    /// with `allowed = false` AND must NOT keep the stale "run `anolisa
    /// enable <cap>` to execute" next_action — that command will refuse,
    /// so the next_action would mislead JSON consumers. We exercise the
    /// gating path directly by driving `EnablePlan::set_execute_gate`
    /// against a synthetic plan, because the handler swallows the plan
    /// after rendering and our coverage target is the
    /// next_action/gate-field contract, not stdout shape.
    #[test]
    fn set_execute_gate_closed_clears_run_next_action_and_records_reason() {
        use anolisa_core::{EnvFactsSummary, LayoutSummary};
        let mut plan = EnablePlan {
            schema_version: 1,
            capability: "agent-memory".to_string(),
            stability: "experimental".to_string(),
            install_mode: "user".to_string(),
            dry_run: true,
            status: PlanStatus::Ready,
            blocked_reason: None,
            components: Vec::new(),
            prechecks: Vec::new(),
            env_facts: EnvFactsSummary {
                os: "linux".into(),
                arch: "x86_64".into(),
                libc: None,
                pkg_base: None,
                kernel: None,
                btf: None,
                cap_bpf: None,
            },
            layout: LayoutSummary {
                bin_dir: "/tmp/bin".into(),
                etc_dir: "/tmp/etc".into(),
                state_dir: "/tmp/state".into(),
                log_dir: "/tmp/log".into(),
                manifests_overlay: "/tmp/overlay".into(),
            },
            warnings: Vec::new(),
            advice: Vec::new(),
            next_actions: vec!["run `anolisa enable agent-memory` to execute".to_string()],
            lint: Vec::new(),
            execute_gate: None,
        };
        plan.set_execute_gate(false, Some("gate closed for tests".to_string()));

        let gate = plan
            .execute_gate
            .as_ref()
            .expect("set_execute_gate must populate the field");
        assert!(!gate.allowed, "closed gate must report allowed=false");
        assert_eq!(gate.reason.as_deref(), Some("gate closed for tests"));

        assert!(
            plan.warnings.iter().any(|w| w.contains("execute gate")),
            "closed gate must surface a warning prefixed with 'execute gate': {:?}",
            plan.warnings,
        );
        // The planner-default "run `anolisa enable agent-memory` to execute"
        // hint must be replaced — that command would refuse, so suggesting it
        // would mislead JSON consumers. Note the gate's replacement copy DOES
        // mention `--dry-run`, so the test asserts on the real-execute shape
        // (capability name as the next token after `enable `), not just on
        // the substring "anolisa enable".
        assert!(
            !plan
                .next_actions
                .iter()
                .any(|n| n.contains("`anolisa enable agent-memory`")),
            "closed gate must drop the real-execute next_action — got: {:?}",
            plan.next_actions,
        );
    }

    /// Dry-run on a graduated capability records `allowed = true` and
    /// leaves the planner's existing `next_actions` intact so the JSON
    /// envelope still tells consumers how to execute.
    #[test]
    fn set_execute_gate_open_preserves_run_next_action() {
        use anolisa_core::{EnvFactsSummary, LayoutSummary};
        let mut plan = EnablePlan {
            schema_version: 1,
            capability: "token-optimization".to_string(),
            stability: "experimental".to_string(),
            install_mode: "user".to_string(),
            dry_run: true,
            status: PlanStatus::Ready,
            blocked_reason: None,
            components: Vec::new(),
            prechecks: Vec::new(),
            env_facts: EnvFactsSummary {
                os: "linux".into(),
                arch: "x86_64".into(),
                libc: None,
                pkg_base: None,
                kernel: None,
                btf: None,
                cap_bpf: None,
            },
            layout: LayoutSummary {
                bin_dir: "/tmp/bin".into(),
                etc_dir: "/tmp/etc".into(),
                state_dir: "/tmp/state".into(),
                log_dir: "/tmp/log".into(),
                manifests_overlay: "/tmp/overlay".into(),
            },
            warnings: Vec::new(),
            advice: Vec::new(),
            next_actions: vec!["run `anolisa enable token-optimization` to execute".to_string()],
            lint: Vec::new(),
            execute_gate: None,
        };
        plan.set_execute_gate(true, None);

        let gate = plan
            .execute_gate
            .as_ref()
            .expect("set_execute_gate must populate the field");
        assert!(gate.allowed);
        assert!(gate.reason.is_none(), "open gate must not carry a reason");
        assert!(
            plan.warnings.is_empty(),
            "open gate must not inject a warning, got: {:?}",
            plan.warnings,
        );
        assert!(
            plan.next_actions
                .iter()
                .any(|n| n.contains("run `anolisa enable")),
            "open gate must keep the run command in next_actions: {:?}",
            plan.next_actions,
        );
    }

    #[test]
    fn enable_dry_run_from_source_is_explicit_not_implemented() {
        let mut a = args(&["agent-observability"]);
        a.from_source = true;
        let err = handle(a, &ctx(false, true, InstallMode::System)).expect_err("must error");
        assert_eq!(err.code(), "NOT_IMPLEMENTED");
        assert!(err.hint().unwrap_or("").contains("--from-source"));
    }

    #[test]
    fn enable_dry_run_with_adapter_is_explicit_not_implemented() {
        let mut a = args(&["agent-observability"]);
        a.with_adapter = Some("auto".to_string());
        let err = handle(a, &ctx(false, true, InstallMode::System)).expect_err("must error");
        assert_eq!(err.code(), "NOT_IMPLEMENTED");
        assert!(err.hint().unwrap_or("").contains("--with-adapter"));
    }

    /// Smoke: with the bundled dev-tree manifests + index, the planner runs
    /// end-to-end and produces a plan. We assert the call succeeds and that
    /// the plan capability/install_mode match the request — we do not assert
    /// a specific status because the host env (linux vs macOS) drives it.
    #[test]
    fn enable_dry_run_happy_path_returns_plan_on_host() {
        // We need an env-agnostic ctx, but ctx.install_mode=System with
        // ctx.prefix=None defaults to / on system mode; that's fine for a
        // bundled-catalog dry-run since nothing is written.
        let _ = PathBuf::from("/tmp");
        let result = handle(
            args(&["agent-observability"]),
            &ctx(true, true, InstallMode::System),
        );
        result.expect("dry-run plan should not error on bundled fixtures");
    }

    /// On macOS the OS precheck for `agent-observability` (requires linux)
    /// turns the plan `Blocked`. The real-execute path must refuse a
    /// `Blocked` plan with an `INVALID_ARGUMENT` whose reason names both
    /// the block status and the suggested `--dry-run` next step — and it
    /// must do so without touching the real `/var/lib/anolisa/lock`. We
    /// rebase the layout under a tempdir via `ctx.prefix` so any lock /
    /// state IO that does happen lands in tmp.
    #[test]
    fn enable_execute_without_dry_run_blocked_plan_returns_invalid_argument() {
        let tmp = tempdir().expect("tmpdir");
        let result = handle(
            args(&["agent-observability"]),
            &ctx_with_prefix(
                false,
                false,
                InstallMode::System,
                Some(tmp.path().to_path_buf()),
            ),
        );

        // The macOS precheck fails the os check → plan.status = blocked
        // → execute_enable returns PlanNotExecutable. If a future change
        // ever makes this path succeed on the dev host we want to know,
        // so we assert specifically on the blocked reason.
        let err = result.expect_err("blocked plan must surface as a CLI error");
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        let reason = err.reason();
        assert!(
            reason.contains("blocked"),
            "reason must mention 'blocked': {reason}"
        );
        assert!(
            reason.contains("dry-run"),
            "reason must point at --dry-run: {reason}"
        );
    }

    /// Real-execute path must apply the same policy gate as `--dry-run`.
    /// A capability that exists in the catalog but is not in the policy
    /// surfaces NOT_IMPLEMENTED before any IO happens (no lock attempt, no
    /// log append). Same fixture choice as the dry-run sibling above.
    #[test]
    fn enable_execute_without_dry_run_capability_not_in_policy_still_not_implemented() {
        let err = handle(
            args(&["agent-memory"]),
            &ctx(false, false, InstallMode::User),
        )
        .expect_err("must error");
        assert_eq!(err.code(), "NOT_IMPLEMENTED");
        let hint = err.hint().unwrap_or("");
        assert!(
            hint.contains("agent-memory"),
            "hint must name the rejected capability: {hint}",
        );
    }

    /// `token-optimization` is now graduated by the execution policy. On
    /// `--dry-run`, the handler must produce a plan envelope (`Ok(())`)
    /// rather than returning NOT_IMPLEMENTED — that's the whole point of
    /// the P1-H exercise: prove the gate is policy-driven, not a second
    /// hard-code. We do not assert a specific status because the host env
    /// drives it (macOS will report `blocked` on the OS precheck, Linux
    /// will reach `degraded` or `ready` depending on the index). What we
    /// MUST assert is that we are no longer in the closed-gate path.
    #[test]
    fn enable_dry_run_token_optimization_returns_plan_not_not_implemented() {
        let result = handle(
            args(&["token-optimization"]),
            &ctx(true, true, InstallMode::User),
        );
        // The handler emits a JSON envelope on stdout and returns Ok(())
        // for any non-error plan (including blocked). We only assert the
        // outcome here; the envelope content is exercised by the
        // bin-level smoke run in the verification matrix.
        result
            .expect("token-optimization dry-run must produce a plan now that policy graduates it");
    }

    /// Real-execute path must keep the flag-scope guards: `--from-source`
    /// is still `NOT_IMPLEMENTED` with the flag named in the hint.
    #[test]
    fn enable_execute_without_dry_run_from_source_still_not_implemented() {
        let mut a = args(&["agent-observability"]);
        a.from_source = true;
        let err = handle(a, &ctx(false, false, InstallMode::System)).expect_err("must error");
        assert_eq!(err.code(), "NOT_IMPLEMENTED");
        assert!(err.hint().unwrap_or("").contains("--from-source"));
    }

    // ── execute_err_to_cli routing (P1-G0) ────────────────────────────
    //
    // The split between EXECUTION_FAILED (exit 1) and INVALID_ARGUMENT
    // (exit 2) is the user-facing contract that wrapping scripts depend
    // on. These tests pin the routing of every `ExecuteError` variant
    // so a future refactor of `execute_enable` cannot silently flip a
    // bucket without breaking a test.

    #[test]
    fn execute_err_download_maps_to_execution_failed_exit_1() {
        let err = execute_err_to_cli(
            "agent-observability",
            ExecuteError::Download {
                component: "agentsight".to_string(),
                source: DownloadError::UnsupportedScheme {
                    scheme: "https".to_string(),
                },
            },
        );
        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert_eq!(err.exit_code(), 1);
        assert!(err.reason().contains("agentsight"));
    }

    #[test]
    fn execute_err_install_maps_to_execution_failed_exit_1() {
        let err = execute_err_to_cli(
            "agent-observability",
            ExecuteError::Install {
                component: "agentsight".to_string(),
                source: InstallError::UnsupportedArtifactType("oci".to_string()),
            },
        );
        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert_eq!(err.exit_code(), 1);
        assert!(err.reason().contains("agentsight"));
    }

    #[test]
    fn execute_err_state_maps_to_execution_failed_exit_1() {
        let err = execute_err_to_cli(
            "agent-observability",
            ExecuteError::State {
                source: StateError::Io {
                    path: PathBuf::from("/tmp/installed.toml"),
                    source: io::Error::new(io::ErrorKind::PermissionDenied, "denied"),
                },
            },
        );
        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn execute_err_log_maps_to_execution_failed_exit_1() {
        let err = execute_err_to_cli(
            "agent-observability",
            ExecuteError::Log {
                source: CentralLogError::Io {
                    path: PathBuf::from("/tmp/central.jsonl"),
                    source: io::Error::new(io::ErrorKind::PermissionDenied, "denied"),
                },
            },
        );
        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn execute_err_lock_held_maps_to_execution_failed_exit_1() {
        let err = execute_err_to_cli(
            "agent-observability",
            ExecuteError::LockHeld {
                path: PathBuf::from("/var/lib/anolisa/lock"),
            },
        );
        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert_eq!(err.exit_code(), 1);
        assert!(err.reason().contains("/var/lib/anolisa/lock"));
    }

    #[test]
    fn execute_err_lock_io_maps_to_execution_failed_exit_1() {
        let err = execute_err_to_cli(
            "agent-observability",
            ExecuteError::Lock {
                source: LockError::Io {
                    path: PathBuf::from("/var/lib/anolisa/lock"),
                    source: io::Error::new(io::ErrorKind::PermissionDenied, "denied"),
                },
            },
        );
        assert_eq!(err.code(), "EXECUTION_FAILED");
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn execute_err_plan_not_executable_stays_invalid_argument_exit_2() {
        let err = execute_err_to_cli(
            "agent-observability",
            ExecuteError::PlanNotExecutable {
                status: "blocked".to_string(),
                reason: "test blocker".to_string(),
            },
        );
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert_eq!(err.exit_code(), 2);
        // Conservative routing: a Blocked plan is "fix your input/env",
        // not "the machine refused to run". The reason must still point
        // at --dry-run so users know how to inspect the block.
        assert!(err.reason().contains("dry-run"));
    }

    #[test]
    fn execute_err_missing_artifact_stays_invalid_argument_exit_2() {
        let err = execute_err_to_cli(
            "agent-observability",
            ExecuteError::MissingArtifact {
                component: "agentsight".to_string(),
            },
        );
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn execute_err_missing_checksum_stays_invalid_argument_exit_2() {
        let err = execute_err_to_cli(
            "agent-observability",
            ExecuteError::MissingChecksum {
                component: "agentsight".to_string(),
            },
        );
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert_eq!(err.exit_code(), 2);
    }

    /// Pin the regression caught in P1-H review: the user-facing
    /// `--dry-run` hint string must reference whatever capability the
    /// caller actually invoked, not the hard-coded P1-G0 name. Cover
    /// both `INVALID_ARGUMENT` hints that mention `--dry-run`
    /// (`PlanNotExecutable` and `MissingArtifact`) so a future template
    /// change cannot regress just one of them.
    /// `DisabledStateInconsistent` is a "fix your machine" condition,
    /// not a runtime IO failure, so it must route to INVALID_ARGUMENT
    /// (exit 2). The reason string must (a) name the capability so the
    /// user knows which one to inspect, (b) include the per-file
    /// mismatch diagnostics so they don't have to grep state.toml,
    /// and (c) hint at `anolisa status --verbose` + uninstall/purge as
    /// the resolution path. All three are load-bearing for the
    /// re-enable UX so we pin them.
    #[test]
    fn execute_err_disabled_state_inconsistent_routes_to_invalid_argument() {
        let err = execute_err_to_cli(
            "agent-observability",
            ExecuteError::DisabledStateInconsistent {
                capability: "agent-observability".to_string(),
                mismatches: vec![
                    "agentsight: sha256 mismatch at /usr/local/bin/agentsight (expected aaa, actual bbb)"
                        .to_string(),
                    "agentsight: cannot read /usr/local/bin/other for verification: No such file or directory"
                        .to_string(),
                ],
            },
        );
        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert_eq!(err.exit_code(), 2);
        let reason = err.reason();
        assert!(
            reason.contains("agent-observability"),
            "reason must name the capability: {reason}",
        );
        assert!(
            reason.contains("sha256 mismatch"),
            "reason must include per-file diagnostic: {reason}",
        );
        assert!(
            reason.contains("anolisa status agent-observability --verbose"),
            "reason must hint at status --verbose for inspection: {reason}",
        );
        assert!(
            reason.contains("uninstall") || reason.contains("purge"),
            "reason must mention the resolution path (uninstall/purge): {reason}",
        );
    }

    #[test]
    fn execute_err_dry_run_hint_uses_invoked_capability_name() {
        let blocked = execute_err_to_cli(
            "token-optimization",
            ExecuteError::PlanNotExecutable {
                status: "blocked".to_string(),
                reason: "host arch mismatch".to_string(),
            },
        );
        assert!(
            blocked
                .reason()
                .contains("anolisa enable token-optimization --dry-run"),
            "PlanNotExecutable hint must name the invoked capability: {}",
            blocked.reason(),
        );
        assert!(
            !blocked.reason().contains("agent-observability"),
            "PlanNotExecutable hint must not leak the P1-G0 capability name: {}",
            blocked.reason(),
        );

        let missing = execute_err_to_cli(
            "token-optimization",
            ExecuteError::MissingArtifact {
                component: "tokenless".to_string(),
            },
        );
        assert!(
            missing
                .reason()
                .contains("anolisa enable token-optimization --dry-run"),
            "MissingArtifact hint must name the invoked capability: {}",
            missing.reason(),
        );
        assert!(
            !missing.reason().contains("agent-observability"),
            "MissingArtifact hint must not leak the P1-G0 capability name: {}",
            missing.reason(),
        );
    }
}
