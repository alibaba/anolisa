//! End-to-end orchestrator for `anolisa enable <capability>`.
//!
//! Given an already-built [`EnablePlan`] and a resolved [`FsLayout`],
//! [`execute_enable`] performs the minimal real install sequence:
//!
//!   1. acquire the advisory install lock;
//!   2. append a `started` audit record to the central log;
//!   3. for each component: download the artifact to the cache, then
//!      install it under the ANOLISA-owned layout;
//!   4. persist the operation outcome to `installed.toml`;
//!   5. append a `succeeded` audit record and release the lock.
//!
//! Any failure in steps 4 onwards (and any failure during a per-component
//! download/install) triggers cleanup: ANOLISA-owned files installed by
//! this operation are unlinked, a `failed` audit record is appended best
//! effort, and the lock is released. The CLI wrapper (Sub-D) renders the
//! returned [`ExecuteOutcome`] or [`ExecuteError`] to the user.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use chrono::{SecondsFormat, Utc};

use anolisa_env::EnvService;
use anolisa_platform::fs_layout::FsLayout;

use crate::central_log::{CentralLog, CentralLogError, LogKind, LogRecord, LogStatus, Severity};
use crate::download::{DownloadCache, DownloadError};
use crate::enable_plan::{EnablePlan, PlanStatus};
use crate::health::{CheckEnv, CheckOutcome, CheckStatus, run_check};
use crate::hooks::{HookPhase, run_phase_hooks};
use crate::install_runner::{InstallError, InstallRunner, ResolvedInstallFile};
use crate::lock::{InstallLock, LockError};
use crate::service;
use crate::state::{
    FileOwner, HealthEntry, InstallMode, InstalledObject, InstalledState, ObjectKind, ObjectStatus,
    OperationRecord, OwnedFile, ServiceRef, StateError,
};

/// One file installed during this operation, tagged with its owning
/// component for later state-writing and cleanup.
#[derive(Debug, Clone)]
pub struct ExecuteInstalledFile {
    /// Component that owns the installed file in `installed.toml`.
    pub component: String,
    /// Absolute destination path written or reactivated.
    pub path: PathBuf,
    /// Lowercase sha256 of the bytes at [`path`](Self::path).
    pub sha256: String,
}

/// What [`execute_enable`] actually did. Sub-D renders this to the user.
#[derive(Debug, Clone)]
pub struct ExecuteOutcome {
    /// Stable operation id recorded in state and central log.
    pub operation_id: String,
    /// Capability requested by the user.
    pub capability: String,
    /// Install mode label copied from the plan.
    pub install_mode: String,
    /// Components installed or reactivated for the capability.
    pub components: Vec<String>,
    /// Files written by this operation, or existing files verified during
    /// reactivation.
    pub installed_files: Vec<ExecuteInstalledFile>,
    /// Resolved on-disk paths the user can inspect after success.
    pub state_path: PathBuf,
    /// Central log path that received operation audit records.
    pub central_log_path: PathBuf,
    /// Non-fatal warnings: any plan warnings + cleanup notes if any.
    pub warnings: Vec<String>,
    /// `true` when this op did not download or install any files — it
    /// observed the capability already in `Disabled` state with every
    /// `OwnedFile` still present and sha256-matching, and only flipped
    /// the state objects back to `Installed`. The CLI renders this as
    /// "reactivated from disabled" so operators can tell apart a fresh
    /// install (which wrote files) from a no-op re-enable (which did
    /// not). `installed_files` echoes the existing on-disk files for
    /// display continuity, NOT files newly written by this op.
    pub reactivated: bool,
}

/// Failure surface for [`execute_enable`]. Every variant represents a
/// clean abort: any files this operation installed before the failure
/// are unlinked, a `failed` central-log record has been appended on a
/// best-effort basis, and the install lock has been released.
#[derive(Debug, thiserror::Error)]
pub enum ExecuteError {
    /// Another ANOLISA process owns the install lock; no install work
    /// started for this request.
    #[error("install lock at {path} is held by another process")]
    LockHeld {
        /// Lock file path that could not be acquired.
        path: PathBuf,
    },
    /// The planner marked the request non-executable.
    #[error("plan status is '{status}' — refuse to execute (reason: {reason})")]
    PlanNotExecutable {
        /// Plan status label observed by the executor.
        status: String,
        /// Planner-provided blocked reason.
        reason: String,
    },
    /// Component reached execution without a resolved artifact.
    #[error("component '{component}': no artifact resolved")]
    MissingArtifact {
        /// Component whose plan was incomplete.
        component: String,
    },
    /// Component artifact omitted sha256; execution refuses unverified
    /// bytes even if the plan was otherwise ready.
    #[error(
        "component '{component}': resolved artifact has no sha256 — refusing to install without verification"
    )]
    MissingChecksum {
        /// Component whose artifact lacked a checksum.
        component: String,
    },
    /// Artifact fetch or checksum verification failed for one component.
    #[error("download failed for component '{component}': {source}")]
    Download {
        /// Component being fetched.
        component: String,
        /// Underlying downloader error.
        #[source]
        source: DownloadError,
    },
    /// The artifact's embedded `.anolisa/component.toml` does not match the
    /// registry `meta.toml` the plan was built from (contract I3 requires a
    /// byte-for-byte copy). Aborts before any file of this component is
    /// installed — the "planned A, installing B" guard (T1.4).
    #[error(
        "component '{component}': artifact component.toml does not match planning meta.toml (expected sha256 {expected_sha256}, actual {})",
        actual_sha256.as_deref().unwrap_or("<missing .anolisa/component.toml>")
    )]
    ManifestMismatch {
        /// Component whose artifact contradicts the planning metadata.
        component: String,
        /// sha256 of the registry meta.toml used at plan time.
        expected_sha256: String,
        /// sha256 of the artifact's embedded manifest, `None` when the
        /// artifact does not contain `.anolisa/component.toml` at all.
        actual_sha256: Option<String>,
    },
    /// Installing cached bytes into the ANOLISA-owned layout failed.
    #[error("install failed for component '{component}': {source}")]
    Install {
        /// Component being installed.
        component: String,
        /// Underlying install-runner error.
        #[source]
        source: InstallError,
    },
    /// `installed.toml` could not be loaded, saved, or restored.
    #[error("state write failed: {source}")]
    State {
        /// Underlying state-file error.
        #[source]
        source: StateError,
    },
    /// Central-log append failed; the audit trail is part of the execute
    /// contract, so this is terminal.
    #[error("central log write failed: {source}")]
    Log {
        /// Underlying JSONL log error.
        #[source]
        source: CentralLogError,
    },
    /// Non-contention lock failure such as parent directory or file I/O.
    #[error("lock io: {source}")]
    Lock {
        /// Underlying lock error with filesystem context.
        #[source]
        source: LockError,
    },
    /// The capability is currently `Disabled` in `InstalledState` but one
    /// or more of its `OwnedFile` paths are missing, unreadable, or no
    /// longer match the recorded sha256. ANOLISA refuses to silently
    /// "reactivate" in this case because the files on disk are not what
    /// state claims they are — the user must uninstall/purge (or
    /// manually fix the affected paths) before re-enabling. Routed to
    /// `INVALID_ARGUMENT` (exit 2) by the CLI wrapper: it is a "fix
    /// your machine" condition, not a runtime IO failure.
    ///
    /// `mismatches` carries one human-readable line per offending file
    /// so the CLI can render an actionable diagnostic without the user
    /// having to grep state by hand.
    #[error(
        "capability '{capability}' is disabled but {} owned file(s) no longer match recorded sha256 — cannot reactivate safely",
        mismatches.len()
    )]
    DisabledStateInconsistent {
        /// Disabled capability the user attempted to re-enable.
        capability: String,
        /// Human-readable mismatch diagnostics for each offending file.
        mismatches: Vec<String>,
    },
    /// A `pre_*` lifecycle hook failed. Routed to a clean abort: the
    /// install runner has not run yet (pre_enable runs before any
    /// download/install) so there are no installed files to clean up,
    /// no state.save to restore, just a `failed` audit record + lock
    /// release. CLI surfaces this through the runtime
    /// (execution-failed) bucket so operators can grep central log
    /// records by `command: "hook:pre_enable"` for hook-induced aborts.
    #[error("hook {phase} for component '{component}' failed (exit {exit_code:?}): {summary}")]
    HookFailed {
        /// Lifecycle phase whose strict hook failed.
        phase: String,
        /// Component that shipped the hook.
        component: String,
        /// One-line diagnostic captured by the hook runner.
        summary: String,
        /// Process exit code when the hook ran; `None` for skip/timeout
        /// paths that never produced one.
        exit_code: Option<i32>,
    },
}

/// Execute `plan` against `layout`. `actor` is recorded in every audit
/// record (typically `$USER`, falling back to `"cli"`).
///
/// On success returns an [`ExecuteOutcome`] describing every file that
/// was written plus the audit-log / state paths the user can inspect.
///
/// On any failure inside the execution body the function:
///   1. unlinks every ANOLISA-owned file already installed in this op,
///   2. appends a [`LogStatus::Failed`] central-log record (best effort),
///   3. releases the install lock,
///   4. returns the underlying error unchanged.
pub fn execute_enable(
    plan: &EnablePlan,
    layout: &FsLayout,
    actor: &str,
) -> Result<ExecuteOutcome, ExecuteError> {
    let state_path = layout.state_dir.join("installed.toml");

    // Step 1 — READ-ONLY reactivation preflight. We deliberately do not
    // touch the lock here: `InstallLock::acquire` would `create_dir_all`
    // the lock parent and write the lock file, breaking the long-held
    // "a blocked plan does not touch the filesystem" contract for the
    // common fresh-blocked-install case (no prior state on disk → no
    // capability to reactivate → user just wants a clean refusal).
    //
    // The preflight is intentionally racy: if it returns "candidate"
    // we re-verify under the lock below (TOCTOU defense). If it
    // returns "not a candidate" we trust it — the only way a
    // concurrent process could promote a not-installed capability to
    // `Disabled` between this check and the next instant is via
    // install→disable, which itself takes the lock and is the case
    // the user can already retry.
    let preflight_reactivation_candidate = match InstalledState::load(&state_path) {
        Ok(s) => s
            .find_object(ObjectKind::Capability, &plan.capability)
            .map(|c| c.status == ObjectStatus::Disabled)
            .unwrap_or(false),
        Err(_) => false,
    };

    // Step 2 — if the preflight has no reactivation candidate, apply
    // the blocked-plan gate before any IO. This preserves the prior
    // contract: a fresh blocked install creates no lock file, no
    // state dir, no log entries.
    if !preflight_reactivation_candidate && plan.status == PlanStatus::Blocked {
        return Err(ExecuteError::PlanNotExecutable {
            status: "blocked".to_string(),
            reason: plan.blocked_reason.clone().unwrap_or_default(),
        });
    }

    // Step 3 — acquire the install lock. Either we have a real install
    // to run, or we have a reactivation candidate that we still need
    // to verify authoritatively. Held by another process → clean
    // abort with no log entry; any other lock IO surfaces as
    // ExecuteError::Lock.
    let lock = match InstallLock::acquire(&layout.lock_file) {
        Ok(l) => l,
        Err(LockError::Held { path }) => return Err(ExecuteError::LockHeld { path }),
        Err(other) => return Err(ExecuteError::Lock { source: other }),
    };

    let central = CentralLog::open(layout.central_log.clone());

    // Step 4 — authoritative reactivation check INSIDE the lock. The
    // preflight in Step 1 was lock-free, so a concurrent disable /
    // enable / uninstall could have changed state between then and
    // now. Re-load and re-check; only commit to reactivation if the
    // capability is still `Disabled`. Anything else falls through to
    // the install flow (which re-applies the blocked-plan gate).
    if preflight_reactivation_candidate
        && let Ok(authoritative_state) = InstalledState::load(&state_path)
        && let Some(cap) = authoritative_state.find_object(ObjectKind::Capability, &plan.capability)
        && cap.status == ObjectStatus::Disabled
    {
        return reactivate_from_disabled(
            authoritative_state,
            plan,
            layout,
            actor,
            &central,
            state_path,
            lock,
        );
    }
    // A false-positive reactivation preflight falls through to the normal
    // blocked-plan gate. If the plan is blocked, there is no disabled
    // install to reactivate and we refuse below.

    // Step 5 — re-apply the blocked-plan gate. Only reachable when the
    // authoritative state did not yield a Disabled capability (either
    // preflight was a false positive due to a concurrent uninstall,
    // or the preflight already said "no candidate"). In both cases a
    // blocked plan must refuse without writing any state / log.
    if plan.status == PlanStatus::Blocked {
        drop(lock);
        return Err(ExecuteError::PlanNotExecutable {
            status: "blocked".to_string(),
            reason: plan.blocked_reason.clone().unwrap_or_default(),
        });
    }

    // Compute the operation id and started_at AFTER the lock is held so
    // concurrent invocations don't accidentally share timestamps.
    let started_at_utc = Utc::now();
    let started_at = started_at_utc.to_rfc3339_opts(SecondsFormat::Secs, true);
    let operation_id = build_operation_id(&started_at_utc);

    // Pre-compute "objects" list once — both started and succeeded/failed
    // records must agree on the touched-objects set.
    let mut objects: Vec<String> = Vec::with_capacity(1 + plan.components.len());
    objects.push(plan.capability.clone());
    for c in &plan.components {
        objects.push(c.name.clone());
    }

    // Step 3 — append the "started" record. Failure here means we have
    // nothing to clean up yet; just drop the lock and report.
    if let Err(err) = central.append(&started_record(
        &operation_id,
        plan,
        actor,
        &started_at,
        objects.clone(),
    )) {
        drop(lock);
        return Err(ExecuteError::Log { source: err });
    }

    // Step 3.5 — pre_enable hooks. Discovered from
    // `<datadir>/hooks/<component>/pre_enable.sh`; absent scripts are a
    // silent no-op. `pre_*` phases run as strict gates: if a hook fails,
    // the install short-circuits before any download/install IO so a
    // drain or pre-flight check can keep the lifecycle closed. Hook
    // logging is performed inside `run_phase_hooks` regardless — the
    // failed hook still gets its own `LogKind::Component` record.
    let component_names: Vec<String> = plan.components.iter().map(|c| c.name.clone()).collect();
    let pre_enable = run_phase_hooks(
        layout,
        &component_names,
        HookPhase::PreEnable,
        Some(&central),
        &operation_id,
        actor,
        &plan.install_mode,
        true,
    );
    if let Some(hf) = pre_enable.hard_failure.as_ref() {
        let err = ExecuteError::HookFailed {
            phase: hf.phase.as_str().to_string(),
            component: hf.component.clone(),
            summary: hf.summary(),
            exit_code: hf.exit_code,
        };
        return cleanup_and_fail(
            err,
            &[],
            &central,
            &operation_id,
            plan,
            actor,
            &started_at,
            objects.clone(),
            None,
            lock,
        );
    }

    let mut installed: Vec<ExecuteInstalledFile> = Vec::new();

    // Step 4 — per-component download + install.
    for c in &plan.components {
        let Some(artifact) = c.artifact.as_ref() else {
            let err = ExecuteError::MissingArtifact {
                component: c.name.clone(),
            };
            return cleanup_and_fail(
                err,
                &installed,
                &central,
                &operation_id,
                plan,
                actor,
                &started_at,
                objects.clone(),
                None,
                lock,
            );
        };

        // Hard guard: the planner now marks missing-sha256 plans Blocked,
        // but `execute_enable` is a public API — a hand-built plan could
        // still arrive with `artifact.sha256: None`. Refuse it here so the
        // download is never attempted without verification, regardless of
        // caller. This is defense-in-depth against bypassing the planner.
        let Some(expected_sha) = artifact.sha256.as_deref() else {
            let err = ExecuteError::MissingChecksum {
                component: c.name.clone(),
            };
            return cleanup_and_fail(
                err,
                &installed,
                &central,
                &operation_id,
                plan,
                actor,
                &started_at,
                objects.clone(),
                None,
                lock,
            );
        };

        let cache = DownloadCache::new(layout.cache_dir.clone());
        let cached = match cache.fetch(&artifact.url, Some(expected_sha)) {
            Ok(d) => d,
            Err(src) => {
                let err = ExecuteError::Download {
                    component: c.name.clone(),
                    source: src,
                };
                return cleanup_and_fail(
                    err,
                    &installed,
                    &central,
                    &operation_id,
                    plan,
                    actor,
                    &started_at,
                    objects.clone(),
                    None,
                    lock,
                );
            }
        };

        // T1.4: when the plan carries the planning-time meta.toml digest,
        // verify the artifact's embedded `.anolisa/component.toml` is the
        // byte-identical copy contract I3 promises — BEFORE installing any
        // file of this component. Only archive artifacts can embed the
        // manifest; other types skip the check (nothing to compare).
        if let Some(expected_sha) = artifact.meta_sha256.as_deref() {
            if is_archive_artifact(&artifact.artifact_type) {
                let actual = match embedded_manifest_sha256(&cached.cached_path) {
                    Ok(digest) => digest,
                    Err(source) => {
                        let err = ExecuteError::Download {
                            component: c.name.clone(),
                            source,
                        };
                        return cleanup_and_fail(
                            err,
                            &installed,
                            &central,
                            &operation_id,
                            plan,
                            actor,
                            &started_at,
                            objects.clone(),
                            None,
                            lock,
                        );
                    }
                };
                if actual.as_deref() != Some(expected_sha) {
                    let err = ExecuteError::ManifestMismatch {
                        component: c.name.clone(),
                        expected_sha256: expected_sha.to_string(),
                        actual_sha256: actual,
                    };
                    return cleanup_and_fail(
                        err,
                        &installed,
                        &central,
                        &operation_id,
                        plan,
                        actor,
                        &started_at,
                        objects.clone(),
                        None,
                        lock,
                    );
                }
            }
        }

        let runner = InstallRunner::new(layout);
        let resolved: Vec<ResolvedInstallFile> = c
            .resolved_files
            .iter()
            .enumerate()
            .map(|(idx, dest)| {
                let spec = c.files.get(idx);
                let kind = spec.map(|file| file.kind).unwrap_or_default();
                let mut source = spec.and_then(|file| file.source.clone());
                // A symlink's source is its referent — a layout template
                // like the dest, not an archive path. Expand it the same
                // way; an expansion failure leaves the braces in place and
                // the runner rejects it as UnresolvedTemplate.
                if kind == crate::manifest::FileKind::Symlink {
                    source = source.map(|s| {
                        crate::adapter::expand_layout_placeholders(
                            &s,
                            layout,
                            &[("component", c.name.as_str())],
                        )
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or(s)
                    });
                }
                ResolvedInstallFile {
                    source,
                    dest: PathBuf::from(dest),
                    mode: spec.and_then(|file| file.mode.clone()),
                    kind,
                }
            })
            .collect();
        let outcome =
            match runner.install_files(&artifact.artifact_type, &cached.cached_path, &resolved) {
                Ok(o) => o,
                Err(src) => {
                    let err = ExecuteError::Install {
                        component: c.name.clone(),
                        source: src,
                    };
                    return cleanup_and_fail(
                        err,
                        &installed,
                        &central,
                        &operation_id,
                        plan,
                        actor,
                        &started_at,
                        objects.clone(),
                        None,
                        lock,
                    );
                }
            };

        for f in outcome.files {
            installed.push(ExecuteInstalledFile {
                component: c.name.clone(),
                path: f.path,
                sha256: f.sha256,
            });
        }
    }

    // Step 4.6 — post-install health probes. Run after every component's
    // files are on disk but before state is persisted, so each verdict and any
    // degraded status land in the same `installed.toml` write. The probes that
    // execute here (binary_version / file_exists / command) only need files
    // present; systemd-active is Unsupported in this milestone, so running
    // before the best-effort service start (Step 7) costs nothing.
    //
    // A hard health failure NEVER fails enable: the bytes are installed and
    // verified, so we degrade the object to `Partial` and surface a warning
    // rather than rolling back a working install over a probe.
    let check_env = CheckEnv {
        layout,
        dry_run: false,
    };
    let checked_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let mut health_by_component: BTreeMap<String, Vec<HealthEntry>> = BTreeMap::new();
    let mut degraded_components: BTreeSet<String> = BTreeSet::new();
    let mut health_warnings: Vec<String> = Vec::new();
    for c in &plan.components {
        let Some(spec) = &c.health_check else {
            continue;
        };
        let outcome = run_check(spec, &check_env);
        // The root node's status is authoritative: an `any_of` can pass with a
        // Failed child, so "degraded" cannot be inferred from leaves alone.
        if outcome.status == CheckStatus::Failed {
            degraded_components.insert(c.name.clone());
            let detail = outcome
                .detail
                .clone()
                .unwrap_or_else(|| outcome.spec_label.clone());
            health_warnings.push(format!(
                "component '{}' health check failed: {detail}",
                c.name
            ));
        }
        let mut entries = Vec::new();
        collect_health_entries(&outcome, &checked_at, &mut entries);
        health_by_component.insert(c.name.clone(), entries);
    }
    let any_degraded = !degraded_components.is_empty();

    // Step 5 — persist state. `state_path` was bound at step 2a.
    let finished_at_utc = Utc::now();
    let finished_at = finished_at_utc.to_rfc3339_opts(SecondsFormat::Secs, true);

    // Snapshot the prior on-disk state so any failure from state.save()
    // onwards can restore the machine to its pre-op state. Without this
    // snapshot a successful state.save() followed by a failed succeeded-log
    // append would leave `installed.toml` claiming components are installed
    // while cleanup unlinks their files — the worst possible inconsistency
    // for a package manager. `None` means there was no prior file; cleanup
    // will remove anything this op wrote instead of restoring bytes.
    let prior_state_bytes: Option<Vec<u8>> = fs::read(&state_path).ok();

    let mut state = match InstalledState::load(&state_path) {
        Ok(s) => s,
        Err(src) => {
            return cleanup_and_fail(
                ExecuteError::State { source: src },
                &installed,
                &central,
                &operation_id,
                plan,
                actor,
                &started_at,
                objects.clone(),
                None,
                lock,
            );
        }
    };

    state.install_mode = match plan.install_mode.as_str() {
        "system" => InstallMode::System,
        _ => InstallMode::User,
    };
    state.prefix = layout.prefix.clone();

    let service_manager = if plan.install_mode == "system" {
        "systemd".to_string()
    } else {
        "systemd-user".to_string()
    };

    for c in &plan.components {
        let comp_files: Vec<OwnedFile> = installed
            .iter()
            .filter(|f| f.component == c.name)
            .map(|f| OwnedFile {
                path: f.path.clone(),
                owner: FileOwner::Anolisa,
                sha256: Some(f.sha256.clone()),
            })
            .collect();

        let comp_health = health_by_component.remove(&c.name).unwrap_or_default();
        let comp_status = if degraded_components.contains(&c.name) {
            ObjectStatus::Partial
        } else {
            ObjectStatus::Installed
        };

        state.upsert_object(InstalledObject {
            kind: ObjectKind::Component,
            name: c.name.clone(),
            version: c.manifest_version.clone().unwrap_or_default(),
            status: comp_status,
            // Verified above against the artifact's embedded component.toml
            // when the plan carried a meta digest; recorded so audit/status
            // can tie the installed component back to its publishing meta.
            manifest_digest: c
                .artifact
                .as_ref()
                .and_then(|a| a.meta_sha256.as_ref())
                .map(|d| format!("sha256:{d}")),
            distribution_source: c.artifact.as_ref().map(|a| a.url.clone()),
            install_backend: c.artifact.as_ref().map(|a| a.backend.clone()),
            installed_at: finished_at.clone(),
            last_operation_id: Some(operation_id.clone()),
            managed: true,
            adopted: false,
            subscription_scope: Default::default(),
            enabled_features: Vec::new(),
            component_refs: Vec::new(),
            files: comp_files,
            external_modified_files: Vec::new(),
            services: c
                .services
                .iter()
                .map(|svc| ServiceRef {
                    name: svc.clone(),
                    manager: service_manager.clone(),
                    restartable: true,
                    // Service enablement is out of scope for this milestone.
                    enabled: false,
                })
                .collect(),
            health: comp_health,
        });
    }

    state.upsert_object(InstalledObject {
        kind: ObjectKind::Capability,
        name: plan.capability.clone(),
        // Capability has no version field on the plan; use the stability
        // label so the on-disk record's version stays non-empty.
        version: plan.stability.clone(),
        // Roll a component health failure up to the capability so list/status
        // surfaces a degraded capability rather than a green one hiding a
        // Partial child.
        status: if any_degraded {
            ObjectStatus::Partial
        } else {
            ObjectStatus::Installed
        },
        manifest_digest: None,
        distribution_source: None,
        install_backend: None,
        installed_at: finished_at.clone(),
        last_operation_id: Some(operation_id.clone()),
        managed: true,
        adopted: false,
        subscription_scope: Default::default(),
        enabled_features: Vec::new(),
        component_refs: plan.components.iter().map(|c| c.name.clone()).collect(),
        files: Vec::new(),
        external_modified_files: Vec::new(),
        services: Vec::new(),
        health: Vec::new(),
    });

    state.append_operation(OperationRecord {
        id: operation_id.clone(),
        command: format!("enable {}", plan.capability),
        status: "ok".to_string(),
        started_at: started_at.clone(),
        finished_at: Some(finished_at.clone()),
    });

    if let Err(src) = state.save(&state_path) {
        // state.save uses tmp+rename so on failure the on-disk file is
        // usually the prior bytes already. Pass the snapshot anyway as
        // defense in depth — restoring known-good bytes is idempotent.
        return cleanup_and_fail(
            ExecuteError::State { source: src },
            &installed,
            &central,
            &operation_id,
            plan,
            actor,
            &started_at,
            objects.clone(),
            Some((state_path.clone(), prior_state_bytes.clone())),
            lock,
        );
    }

    // Step 6 — append the succeeded record. Even if state.save above
    // succeeded, a failure here is fatal: without the audit record the
    // user has no way to find this operation in `anolisa logs`. The
    // snapshot restore is load-bearing here — state.save just succeeded,
    // so the on-disk file currently claims this op completed; we must
    // roll it back before unlinking files.
    if let Err(src) = central.append(&succeeded_record(
        &operation_id,
        plan,
        actor,
        &started_at,
        &finished_at,
        objects.clone(),
    )) {
        return cleanup_and_fail(
            ExecuteError::Log { source: src },
            &installed,
            &central,
            &operation_id,
            plan,
            actor,
            &started_at,
            objects.clone(),
            Some((state_path.clone(), prior_state_bytes)),
            lock,
        );
    }

    // Drop the install lock BEFORE driving systemctl. State + audit
    // trail have already landed; the service-start phase below is
    // best-effort and doesn't need to keep the lock held while it
    // shells out (which can spawn non-trivial subprocesses).
    drop(lock);

    // Step 7 — best-effort start of every owned service unit. Service
    // failures NEVER fail enable: by the time we reach here the audit
    // trail already shows `succeeded` and `installed.toml` records the
    // capability as Installed. Any systemctl error is surfaced as a
    // warning on the outcome (and the audit trail will pick it up the
    // next time the operator runs `anolisa status`).
    let mut warnings = plan.warnings.clone();
    warnings.extend(pre_enable.warnings);
    // Health degradations were detected at Step 4.6; surface them on the
    // outcome so the CLI prints them even though enable itself succeeded.
    warnings.extend(health_warnings);
    let service_units: Vec<(String, String)> = plan
        .components
        .iter()
        .flat_map(|c| {
            c.services
                .iter()
                .map(|svc| (c.name.clone(), svc.clone()))
                .collect::<Vec<_>>()
        })
        .collect();
    if !service_units.is_empty() {
        let env = EnvService::detect();
        let manager = service::for_install_mode(&plan.install_mode, &env);
        if manager.supported() {
            for (component, unit) in &service_units {
                match manager.start_service(unit) {
                    Ok(_) => {
                        service::record_service_op(
                            Some(&central),
                            service::ServiceOp::Start,
                            component,
                            unit,
                            &operation_id,
                            actor,
                            &plan.install_mode,
                            None,
                        );
                    }
                    Err(err) => {
                        let err_msg = err.to_string();
                        warnings.push(format!(
                            "service start skipped for {component}/{unit}: {err_msg}",
                        ));
                        service::record_service_op(
                            Some(&central),
                            service::ServiceOp::Start,
                            component,
                            unit,
                            &operation_id,
                            actor,
                            &plan.install_mode,
                            Some(&err_msg),
                        );
                    }
                }
            }
        } else {
            let manager_name = manager.manager().to_string();
            let reason = manager.unsupported_reason().map(str::to_string);
            for (component, unit) in &service_units {
                service::record_service_op_unsupported(
                    Some(&central),
                    service::ServiceOp::Start,
                    component,
                    unit,
                    &operation_id,
                    actor,
                    &plan.install_mode,
                    &manager_name,
                    reason.as_deref(),
                );
            }
        }
    }

    // Step 8 — post_enable hooks. Run AFTER state.save + succeeded log
    // + service start so hooks see the final post-enable shape (binary
    // on disk, state flipped to Installed, units started). Hook
    // failures only warn — by this point the operation is already
    // recorded as `succeeded` in the central log; downgrading to
    // `failed` would lie about what is on disk.
    let post_enable = run_phase_hooks(
        layout,
        &component_names,
        HookPhase::PostEnable,
        Some(&central),
        &operation_id,
        actor,
        &plan.install_mode,
        false,
    );
    warnings.extend(post_enable.warnings);

    let outcome = ExecuteOutcome {
        operation_id,
        capability: plan.capability.clone(),
        install_mode: plan.install_mode.clone(),
        components: plan.components.iter().map(|c| c.name.clone()).collect(),
        installed_files: installed,
        state_path,
        central_log_path: layout.central_log.clone(),
        warnings,
        reactivated: false,
    };
    Ok(outcome)
}

/// Flatten a [`CheckOutcome`] tree into [`HealthEntry`] rows in pre-order —
/// the aggregate node first, then its children — so `anolisa status` can show
/// both the combined verdict and each leaf probe. `checked_at` is stamped
/// identically across the tree because every node was observed in one pass.
fn collect_health_entries(outcome: &CheckOutcome, checked_at: &str, out: &mut Vec<HealthEntry>) {
    out.push(HealthEntry {
        name: outcome.spec_label.clone(),
        status: outcome.status.as_str().to_string(),
        checked_at: checked_at.to_string(),
        reason: outcome.detail.clone(),
    });
    for child in &outcome.children {
        collect_health_entries(child, checked_at, out);
    }
}

/// True when the artifact type is a tar.gz archive — the only artifact
/// form that can embed `.anolisa/component.toml` for the T1.4 check.
/// Accepts the same spellings the index parser accepts.
fn is_archive_artifact(artifact_type: &str) -> bool {
    matches!(artifact_type, "tar_gz" | "tar.gz" | "tar")
}

/// Extract `.anolisa/component.toml` from a tar.gz artifact and return the
/// lowercase-hex sha256 of its bytes; `Ok(None)` when the archive has no
/// such entry. Entry paths are compared after stripping any leading `./`
/// (tar created with `-C dir .` prefixes every path that way).
///
/// # Errors
/// [`DownloadError::Io`] when the archive cannot be read or is not valid
/// gzip/tar — surfaced through the same bucket as other artifact IO.
fn embedded_manifest_sha256(
    artifact_path: &std::path::Path,
) -> Result<Option<String>, DownloadError> {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    let io_err = |source: std::io::Error| DownloadError::Io {
        path: artifact_path.to_path_buf(),
        source,
    };

    let file = std::fs::File::open(artifact_path).map_err(io_err)?;
    let gz = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(gz);
    for entry in archive.entries().map_err(io_err)? {
        let mut entry = entry.map_err(io_err)?;
        // Scope the path borrow so `read_to_end` can take `&mut entry`.
        let is_manifest = {
            let path = entry.path().map_err(io_err)?;
            let normalized = path.strip_prefix("./").unwrap_or(&path);
            normalized == std::path::Path::new(".anolisa/component.toml")
        };
        if is_manifest {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).map_err(io_err)?;
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            return Ok(Some(format!("{:x}", hasher.finalize())));
        }
    }
    Ok(None)
}

/// Cleanup helper invoked when any post-lock step fails. Unlinks every
/// file already installed in this operation, optionally rolls
/// `installed.toml` back to its pre-op bytes (only required when the
/// failure happened after `state.save()`), appends a `failed` audit
/// record (errors here are swallowed so the original failure surfaces),
/// drops the lock, and returns the original error.
///
/// `state_restore`:
///   * `None` — failure happened before `state.save()`; the state file
///     was never written by this op and must not be touched.
///   * `Some((path, Some(bytes)))` — restore `path` to `bytes` (the
///     pre-op snapshot).
///   * `Some((path, None))` — no prior state existed; remove `path`
///     entirely so the cleanup is a true rollback.
#[allow(clippy::too_many_arguments)]
fn cleanup_and_fail(
    err: ExecuteError,
    installed: &[ExecuteInstalledFile],
    central: &CentralLog,
    operation_id: &str,
    plan: &EnablePlan,
    actor: &str,
    started_at: &str,
    objects: Vec<String>,
    state_restore: Option<(PathBuf, Option<Vec<u8>>)>,
    lock: InstallLock,
) -> Result<ExecuteOutcome, ExecuteError> {
    for f in installed {
        let _ = fs::remove_file(&f.path);
    }
    if let Some((path, prior)) = state_restore {
        match prior {
            Some(bytes) => {
                // Best-effort restore: if the rewrite fails the failed
                // audit record will still be appended below and the user
                // sees the original error, which is the right signal.
                let _ = fs::write(&path, &bytes);
            }
            None => {
                let _ = fs::remove_file(&path);
            }
        }
    }
    let finished_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let _ = central.append(&failed_record(
        operation_id,
        plan,
        actor,
        started_at,
        &finished_at,
        objects,
        &err,
    ));
    drop(lock);
    Err(err)
}

/// Reactivate a capability that is currently `Disabled` in
/// `InstalledState`. Runs entirely inside the install lock held by the
/// caller. Sha256-verifies every `OwnedFile` (only ANOLISA-owned files
/// are checked — externally-modified or unmanaged files are skipped)
/// before flipping the capability + its components back to `Installed`;
/// any mismatch surfaces as [`ExecuteError::DisabledStateInconsistent`]
/// and writes nothing to disk or to the central log.
///
/// On the happy path:
///   1. append a `started` record whose message announces the
///      reactivation (so audit consumers can grep for "reactivating"
///      to distinguish from fresh installs);
///   2. snapshot `installed.toml` bytes for cleanup;
///   3. mutate the in-memory state: flip capability + each component
///      in `component_refs` from `Disabled` → `Installed`, refresh
///      `last_operation_id`, append an `OperationRecord { status = "ok" }`,
///      `state.save`;
///   4. append a `succeeded` record (message includes "reactivated
///      from disabled — no reinstall" so operators don't mistake this
///      for a fresh install in `anolisa logs`);
///   5. drop the lock and return [`ExecuteOutcome`] with
///      `reactivated = true` and `installed_files` echoing the
///      on-disk files (these files were NOT written by this op — they
///      are repeated for CLI rendering parity with the install path).
///
/// On the mismatch path we drop the lock immediately and return
/// `DisabledStateInconsistent` with one diagnostic line per offending
/// file — no audit log entry is written, matching the
/// `CapabilityNotInstalled` no-log convention on the disable side
/// (both are "your machine doesn't satisfy the precondition" errors,
/// not runtime IO failures).
fn reactivate_from_disabled(
    mut state: InstalledState,
    plan: &EnablePlan,
    layout: &FsLayout,
    actor: &str,
    central: &CentralLog,
    state_path: PathBuf,
    lock: InstallLock,
) -> Result<ExecuteOutcome, ExecuteError> {
    // Step 1 — collect refs + verify every ANOLISA-owned file. We use
    // the capability's `component_refs` (not `plan.components`) so the
    // verification matches what state actually claims is installed,
    // not what the current catalog/plan would install fresh. This
    // matters when the catalog has drifted between the original
    // install and the re-enable attempt.
    // `state` was loaded fresh by `execute_enable` (Step 2a) which
    // already located this capability on the same in-memory handle —
    // a None here means InstalledState's lookup disagrees with itself.
    // Mirrors the `expect` pattern used in disable_execute Step 3.
    let component_refs = state
        .find_object(ObjectKind::Capability, &plan.capability)
        .expect("capability located in execute_enable Step 2a must still be present")
        .component_refs
        .clone();

    let mut mismatches: Vec<String> = Vec::new();
    let mut verified_files: Vec<ExecuteInstalledFile> = Vec::new();
    for comp_name in &component_refs {
        let Some(comp) = state.find_object(ObjectKind::Component, comp_name) else {
            mismatches.push(format!(
                "{comp_name}: component object missing from installed state",
            ));
            continue;
        };
        for f in &comp.files {
            if f.owner != FileOwner::Anolisa {
                continue;
            }
            let Some(expected_sha) = f.sha256.clone() else {
                mismatches.push(format!(
                    "{}: no recorded sha256 for {} — refuse to reactivate without verification",
                    comp_name,
                    f.path.display(),
                ));
                continue;
            };
            match hash_file_sha256(&f.path) {
                Err(io_err) => mismatches.push(format!(
                    "{}: cannot read {} for verification: {io_err}",
                    comp_name,
                    f.path.display(),
                )),
                Ok(actual) if actual != expected_sha => mismatches.push(format!(
                    "{}: sha256 mismatch at {} (expected {}, actual {})",
                    comp_name,
                    f.path.display(),
                    expected_sha,
                    actual,
                )),
                Ok(_) => verified_files.push(ExecuteInstalledFile {
                    component: comp_name.clone(),
                    path: f.path.clone(),
                    sha256: expected_sha,
                }),
            }
        }
    }

    if !mismatches.is_empty() {
        drop(lock);
        return Err(ExecuteError::DisabledStateInconsistent {
            capability: plan.capability.clone(),
            mismatches,
        });
    }

    // Step 2 — all files verified. Mint op id, log started, snapshot
    // state for cleanup.
    let started_at_utc = Utc::now();
    let started_at = started_at_utc.to_rfc3339_opts(SecondsFormat::Secs, true);
    let operation_id = build_operation_id(&started_at_utc);

    let mut objects: Vec<String> = Vec::with_capacity(1 + component_refs.len());
    objects.push(plan.capability.clone());
    for c in &component_refs {
        objects.push(c.clone());
    }

    if let Err(src) = central.append(&reactivate_started_record(
        &operation_id,
        plan,
        actor,
        &started_at,
        objects.clone(),
    )) {
        drop(lock);
        return Err(ExecuteError::Log { source: src });
    }

    // Pre-enable hooks for reactivation. Strict — same gate semantics
    // as the fresh install path. A failed pre_enable here means the
    // capability stays Disabled and we abort with HookFailed before
    // touching state. The reactivate-started log line was already
    // written above; balance it with a failed audit record + lock drop.
    let pre_enable_reactivate = run_phase_hooks(
        layout,
        &component_refs,
        HookPhase::PreEnable,
        Some(central),
        &operation_id,
        actor,
        &plan.install_mode,
        true,
    );
    if let Some(hf) = pre_enable_reactivate.hard_failure.as_ref() {
        let err = ExecuteError::HookFailed {
            phase: hf.phase.as_str().to_string(),
            component: hf.component.clone(),
            summary: hf.summary(),
            exit_code: hf.exit_code,
        };
        return cleanup_and_fail(
            err,
            &[],
            central,
            &operation_id,
            plan,
            actor,
            &started_at,
            objects.clone(),
            None,
            lock,
        );
    }

    let prior_state_bytes: Option<Vec<u8>> = fs::read(&state_path).ok();
    let finished_at_utc = Utc::now();
    let finished_at = finished_at_utc.to_rfc3339_opts(SecondsFormat::Secs, true);

    // Step 3 — flip capability + components Disabled → Installed.
    // `expect` mirrors the disable_execute pattern: we already located
    // the capability on this same in-memory state above, so absence
    // here means InstalledState's index disagrees with itself.
    let cap = state
        .find_object_mut(ObjectKind::Capability, &plan.capability)
        .expect("capability located above must still be present on same state handle");
    cap.status = ObjectStatus::Installed;
    cap.last_operation_id = Some(operation_id.clone());

    let mut reactivated_components: Vec<String> = Vec::new();
    for comp_name in &component_refs {
        if let Some(comp) = state.find_object_mut(ObjectKind::Component, comp_name) {
            comp.status = ObjectStatus::Installed;
            comp.last_operation_id = Some(operation_id.clone());
            reactivated_components.push(comp_name.clone());
        }
    }

    state.append_operation(OperationRecord {
        id: operation_id.clone(),
        command: format!("enable {}", plan.capability),
        status: "ok".to_string(),
        started_at: started_at.clone(),
        finished_at: Some(finished_at.clone()),
    });

    if let Err(src) = state.save(&state_path) {
        // Reactivate path installed no new files, so `installed` is
        // empty — cleanup_and_fail will only restore prior state bytes
        // and append the failed audit record.
        return cleanup_and_fail(
            ExecuteError::State { source: src },
            &[],
            central,
            &operation_id,
            plan,
            actor,
            &started_at,
            objects.clone(),
            Some((state_path.clone(), prior_state_bytes.clone())),
            lock,
        );
    }

    // Surface every signal that would have refused a fresh install so
    // the operator can see what changed under them while the
    // capability was Disabled. `plan.warnings` covers the resolver-side
    // soft signals; `plan.blocked_reason` + failing `plan.prechecks`
    // cover the hard signals that turned the fresh plan Blocked. A
    // silent reactivation that just hides them would be the wrong UX:
    // the user installed on a host that satisfied the prereqs, the
    // host has since regressed, and they should know — even though we
    // proceeded because the binary on disk is still verifiable.
    // Computed BEFORE the succeeded-log append so the warnings make
    // it into the persisted audit record, not just the in-memory
    // outcome the CLI renders.
    let mut warnings = plan.warnings.clone();
    if plan.status == PlanStatus::Blocked {
        if let Some(reason) = plan.blocked_reason.as_deref() {
            warnings.push(format!(
                "reactivation proceeded despite blocked fresh plan: {reason}",
            ));
        }
        for p in plan.prechecks.iter().filter(|p| p.status == "fail") {
            let detail = p.message.as_deref().unwrap_or("");
            warnings.push(format!(
                "blocked precheck (overridden by reactivation): {} expected={} actual={} {detail}",
                p.name, p.expected, p.actual,
            ));
        }
    }
    warnings.push(
        "reactivated from disabled state — no files written (existing OwnedFile sha256s verified)"
            .to_string(),
    );
    warnings.extend(pre_enable_reactivate.warnings);

    if let Err(src) = central.append(&reactivate_succeeded_record(
        &operation_id,
        plan,
        actor,
        &started_at,
        &finished_at,
        objects.clone(),
        warnings.clone(),
    )) {
        return cleanup_and_fail(
            ExecuteError::Log { source: src },
            &[],
            central,
            &operation_id,
            plan,
            actor,
            &started_at,
            objects.clone(),
            Some((state_path.clone(), prior_state_bytes)),
            lock,
        );
    }

    // Drop the install lock BEFORE driving systemctl. State + audit
    // trail are already committed; service start is best-effort and
    // doesn't need to keep the lock held while it shells out.
    let mut reactivate_service_units: Vec<(String, String)> = Vec::new();
    for comp_name in &component_refs {
        if let Some(comp) = state.find_object(ObjectKind::Component, comp_name) {
            for svc in &comp.services {
                reactivate_service_units.push((comp_name.clone(), svc.name.clone()));
            }
        }
    }
    drop(lock);

    if !reactivate_service_units.is_empty() {
        let env = EnvService::detect();
        let manager = service::for_install_mode(&plan.install_mode, &env);
        if manager.supported() {
            for (component, unit) in &reactivate_service_units {
                match manager.start_service(unit) {
                    Ok(_) => {
                        service::record_service_op(
                            Some(central),
                            service::ServiceOp::Start,
                            component,
                            unit,
                            &operation_id,
                            actor,
                            &plan.install_mode,
                            None,
                        );
                    }
                    Err(err) => {
                        let err_msg = err.to_string();
                        warnings.push(format!(
                            "service start skipped for {component}/{unit}: {err_msg}",
                        ));
                        service::record_service_op(
                            Some(central),
                            service::ServiceOp::Start,
                            component,
                            unit,
                            &operation_id,
                            actor,
                            &plan.install_mode,
                            Some(&err_msg),
                        );
                    }
                }
            }
        } else {
            let manager_name = manager.manager().to_string();
            let reason = manager.unsupported_reason().map(str::to_string);
            for (component, unit) in &reactivate_service_units {
                service::record_service_op_unsupported(
                    Some(central),
                    service::ServiceOp::Start,
                    component,
                    unit,
                    &operation_id,
                    actor,
                    &plan.install_mode,
                    &manager_name,
                    reason.as_deref(),
                );
            }
        }
    }

    // Post-enable hooks for reactivation. Run last so they see the
    // capability fully back online (state Installed, services started).
    let post_enable_reactivate = run_phase_hooks(
        layout,
        &component_refs,
        HookPhase::PostEnable,
        Some(central),
        &operation_id,
        actor,
        &plan.install_mode,
        false,
    );
    warnings.extend(post_enable_reactivate.warnings);

    let outcome = ExecuteOutcome {
        operation_id,
        capability: plan.capability.clone(),
        install_mode: plan.install_mode.clone(),
        components: reactivated_components,
        installed_files: verified_files,
        state_path,
        central_log_path: layout.central_log.clone(),
        warnings,
        reactivated: true,
    };
    Ok(outcome)
}

/// Stream-hash a file with sha256 (lowercase hex), mirroring the
/// install / download verification path so the comparison is
/// byte-for-byte identical. 8 KiB chunks keep peak memory bounded.
fn hash_file_sha256(path: &std::path::Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    let mut f = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest.iter() {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    Ok(out)
}

fn reactivate_started_record(
    operation_id: &str,
    plan: &EnablePlan,
    actor: &str,
    started_at: &str,
    objects: Vec<String>,
) -> LogRecord {
    LogRecord {
        kind: LogKind::Operation,
        operation_id: Some(operation_id.to_string()),
        command: format!("enable {}", plan.capability),
        source: "anolisa-cli".to_string(),
        component: None,
        severity: Severity::Info,
        message: format!(
            "enable {} started (reactivating from disabled)",
            plan.capability,
        ),
        actor: actor.to_string(),
        install_mode: Some(plan.install_mode.clone()),
        started_at: started_at.to_string(),
        finished_at: None,
        status: None,
        objects,
        backup_ids: Vec::new(),
        warnings: Vec::new(),
        details: serde_json::Value::Null,
    }
}

fn reactivate_succeeded_record(
    operation_id: &str,
    plan: &EnablePlan,
    actor: &str,
    started_at: &str,
    finished_at: &str,
    objects: Vec<String>,
    warnings: Vec<String>,
) -> LogRecord {
    LogRecord {
        kind: LogKind::Operation,
        operation_id: Some(operation_id.to_string()),
        command: format!("enable {}", plan.capability),
        source: "anolisa-cli".to_string(),
        component: None,
        severity: Severity::Info,
        message: format!(
            "enable {} succeeded (reactivated from disabled — no reinstall)",
            plan.capability,
        ),
        actor: actor.to_string(),
        install_mode: Some(plan.install_mode.clone()),
        started_at: started_at.to_string(),
        finished_at: Some(finished_at.to_string()),
        status: Some(LogStatus::Ok),
        objects,
        backup_ids: Vec::new(),
        warnings,
        details: serde_json::Value::Null,
    }
}

fn started_record(
    operation_id: &str,
    plan: &EnablePlan,
    actor: &str,
    started_at: &str,
    objects: Vec<String>,
) -> LogRecord {
    LogRecord {
        kind: LogKind::Operation,
        operation_id: Some(operation_id.to_string()),
        command: format!("enable {}", plan.capability),
        source: "anolisa-cli".to_string(),
        component: None,
        severity: Severity::Info,
        message: format!("enable {} started", plan.capability),
        actor: actor.to_string(),
        install_mode: Some(plan.install_mode.clone()),
        started_at: started_at.to_string(),
        finished_at: None,
        status: None,
        objects,
        backup_ids: Vec::new(),
        warnings: Vec::new(),
        details: serde_json::Value::Null,
    }
}

fn succeeded_record(
    operation_id: &str,
    plan: &EnablePlan,
    actor: &str,
    started_at: &str,
    finished_at: &str,
    objects: Vec<String>,
) -> LogRecord {
    LogRecord {
        kind: LogKind::Operation,
        operation_id: Some(operation_id.to_string()),
        command: format!("enable {}", plan.capability),
        source: "anolisa-cli".to_string(),
        component: None,
        severity: Severity::Info,
        message: format!("enable {} succeeded", plan.capability),
        actor: actor.to_string(),
        install_mode: Some(plan.install_mode.clone()),
        started_at: started_at.to_string(),
        finished_at: Some(finished_at.to_string()),
        status: Some(LogStatus::Ok),
        objects,
        backup_ids: Vec::new(),
        warnings: plan.warnings.clone(),
        details: serde_json::Value::Null,
    }
}

fn failed_record(
    operation_id: &str,
    plan: &EnablePlan,
    actor: &str,
    started_at: &str,
    finished_at: &str,
    objects: Vec<String>,
    err: &ExecuteError,
) -> LogRecord {
    LogRecord {
        kind: LogKind::Operation,
        operation_id: Some(operation_id.to_string()),
        command: format!("enable {}", plan.capability),
        source: "anolisa-cli".to_string(),
        component: None,
        severity: Severity::Error,
        message: format!("enable {} failed: {err}", plan.capability),
        actor: actor.to_string(),
        install_mode: Some(plan.install_mode.clone()),
        started_at: started_at.to_string(),
        finished_at: Some(finished_at.to_string()),
        status: Some(LogStatus::Failed),
        objects,
        backup_ids: Vec::new(),
        warnings: Vec::new(),
        details: serde_json::Value::Null,
    }
}

/// `op-YYYYMMDDHHMMSS-<6-hex>` — sortable, unique per call, no new
/// crate deps. The 24-bit suffix is the low bits of the timestamp nanos
/// run through `DefaultHasher` so two calls inside the same second still
/// disambiguate.
fn build_operation_id(now: &chrono::DateTime<Utc>) -> String {
    let ts = now.format("%Y%m%d%H%M%S").to_string();
    let nanos = now.timestamp_nanos_opt().unwrap_or_else(|| now.timestamp());
    let mut hasher = DefaultHasher::new();
    nanos.hash(&mut hasher);
    let suffix = hasher.finish() & 0xff_ffff;
    format!("op-{ts}-{suffix:06x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::enable_plan::{
        ArtifactPlan, ComponentPlan, EnablePlan, EnvFactsSummary, LayoutSummary,
        PLAN_SCHEMA_VERSION,
    };
    use crate::health::CheckSpec;
    use crate::manifest::{EnvRequirements, FileKind, InstallFileSpec};
    use sha2::{Digest, Sha256};
    use std::fs as std_fs;
    use std::path::Path;
    use tempfile::tempdir;

    fn sha256_of(bytes: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(bytes);
        let out = h.finalize();
        let mut s = String::with_capacity(64);
        for b in out {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }

    fn write_payload_artifact(dir: &Path, name: &str, bytes: &[u8]) -> (String, String) {
        let p = dir.join(name);
        std_fs::write(&p, bytes).expect("write payload");
        let url = format!("file://{}", p.to_str().expect("utf8 path"));
        (url, sha256_of(bytes))
    }

    /// Build a tar.gz artifact embedding `.anolisa/component.toml` plus one
    /// payload at `bin/agentsight` — the publish-contract artifact shape the
    /// T1.4 consistency check inspects. Returns `(file_url, artifact_sha256)`.
    fn write_targz_artifact(
        dir: &Path,
        name: &str,
        manifest_bytes: &[u8],
        payload: &[u8],
    ) -> (String, String) {
        use flate2::{Compression, write::GzEncoder};
        let p = dir.join(name);
        let file = std_fs::File::create(&p).expect("create tar.gz");
        let enc = GzEncoder::new(file, Compression::default());
        let mut tar = tar::Builder::new(enc);

        let mut manifest_header = tar::Header::new_gnu();
        manifest_header.set_size(manifest_bytes.len() as u64);
        manifest_header.set_mode(0o644);
        manifest_header.set_cksum();
        tar.append_data(
            &mut manifest_header,
            ".anolisa/component.toml",
            manifest_bytes,
        )
        .expect("append manifest");

        let mut payload_header = tar::Header::new_gnu();
        payload_header.set_size(payload.len() as u64);
        payload_header.set_mode(0o755);
        payload_header.set_cksum();
        tar.append_data(&mut payload_header, "bin/agentsight", payload)
            .expect("append payload");

        let enc = tar.into_inner().expect("finish tar");
        enc.finish().expect("finish gz");
        let bytes = std_fs::read(&p).expect("read back");
        let url = format!("file://{}", p.to_str().expect("utf8 path"));
        (url, sha256_of(&bytes))
    }

    fn fixture_layout(prefix: &Path) -> FsLayout {
        FsLayout::system(Some(prefix.to_path_buf()))
    }

    /// T1.4 happy path: artifact's embedded component.toml matches the plan's
    /// meta digest → install proceeds and `installed.toml` records the digest.
    #[test]
    fn manifest_match_installs_and_records_digest() {
        let tmp = tempdir().expect("tmpdir");
        let layout = fixture_layout(tmp.path());
        let manifest_bytes: &[u8] = b"[component]\nname = \"agentsight\"\nversion = \"0.2.0\"\n";
        let (url, sha) = write_targz_artifact(tmp.path(), "a.tar.gz", manifest_bytes, b"payload");

        let mut artifact = artifact_plan(&url, &sha);
        artifact.artifact_type = "tar_gz".to_string();
        artifact.meta_sha256 = Some(sha256_of(manifest_bytes));
        let dest = layout.bin_dir.join("agentsight");
        let mut comp = fixture_component(
            "agentsight",
            Some(artifact),
            vec![dest.display().to_string()],
            PlanStatus::Ready,
        );
        comp.files[0].source = Some("bin/agentsight".to_string());
        let plan = fixture_plan(
            "agent-observability",
            vec![comp],
            PlanStatus::Ready,
            "system",
            &layout,
        );

        execute_enable(&plan, &layout, "tester").expect("matching manifest must install");
        assert!(dest.is_file(), "payload must be installed");
        let state =
            std_fs::read_to_string(layout.state_dir.join("installed.toml")).expect("state file");
        assert!(
            state.contains(&format!("sha256:{}", sha256_of(manifest_bytes))),
            "installed.toml must record the verified manifest digest:\n{state}",
        );
    }

    /// T1.4 guard: artifact's embedded component.toml differs from the plan's
    /// meta digest → abort with `ManifestMismatch`, nothing installed, no
    /// state written.
    #[test]
    fn manifest_mismatch_aborts_with_no_files() {
        let tmp = tempdir().expect("tmpdir");
        let layout = fixture_layout(tmp.path());
        let manifest_bytes: &[u8] = b"[component]\nname = \"agentsight\"\nversion = \"0.2.0\"\n";
        let (url, sha) = write_targz_artifact(tmp.path(), "a.tar.gz", manifest_bytes, b"payload");

        let mut artifact = artifact_plan(&url, &sha);
        artifact.artifact_type = "tar_gz".to_string();
        // Planning meta digest deliberately disagrees with the artifact.
        artifact.meta_sha256 = Some(sha256_of(b"a different meta.toml body"));
        let dest = layout.bin_dir.join("agentsight");
        let mut comp = fixture_component(
            "agentsight",
            Some(artifact),
            vec![dest.display().to_string()],
            PlanStatus::Ready,
        );
        comp.files[0].source = Some("bin/agentsight".to_string());
        let plan = fixture_plan(
            "agent-observability",
            vec![comp],
            PlanStatus::Ready,
            "system",
            &layout,
        );

        let err = execute_enable(&plan, &layout, "tester").expect_err("mismatch must abort");
        match err {
            ExecuteError::ManifestMismatch {
                component,
                expected_sha256,
                actual_sha256,
            } => {
                assert_eq!(component, "agentsight");
                assert_eq!(expected_sha256, sha256_of(b"a different meta.toml body"));
                assert_eq!(
                    actual_sha256.as_deref(),
                    Some(sha256_of(manifest_bytes).as_str())
                );
            }
            other => panic!("expected ManifestMismatch, got {other:?}"),
        }
        assert!(!dest.exists(), "no file may be installed on mismatch");
        assert!(
            !layout.state_dir.join("installed.toml").exists(),
            "state must not be written on mismatch",
        );
    }

    /// Make `InstalledState::save` fail without breaking the executor's
    /// pre-save IO (lock acquire, central-log append). Strategy: pre-
    /// create the lock file and the state dir, then chmod state_dir to
    /// 0o500 so the salted tmp-sibling create_new() inside `state.save`
    /// is the *only* operation that fails. Returns the original
    /// `state_dir` permissions so the caller can restore them after the
    /// failing call (otherwise tempdir cleanup may itself fail).
    #[cfg(unix)]
    fn sabotage_state_save_unix(layout: &FsLayout) -> std_fs::Permissions {
        use std::os::unix::fs::PermissionsExt;
        std_fs::create_dir_all(&layout.state_dir).expect("mkdir state_dir");
        std_fs::create_dir_all(&layout.log_dir).expect("mkdir log_dir");
        std_fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&layout.lock_file)
            .expect("pre-create lock file");
        let original = std_fs::metadata(&layout.state_dir).unwrap().permissions();
        let mut readonly = original.clone();
        readonly.set_mode(0o500);
        std_fs::set_permissions(&layout.state_dir, readonly).unwrap();
        original
    }

    #[cfg(unix)]
    fn restore_state_dir_perms(layout: &FsLayout, perms: std_fs::Permissions) {
        std_fs::set_permissions(&layout.state_dir, perms).unwrap();
    }

    fn fixture_component(
        name: &str,
        artifact: Option<ArtifactPlan>,
        resolved_files: Vec<String>,
        status: PlanStatus,
    ) -> ComponentPlan {
        ComponentPlan {
            name: name.to_string(),
            manifest_version: Some("0.2.0".to_string()),
            status,
            blocked_reason: None,
            artifact,
            services: vec!["agentsight.service".to_string()],
            files: vec![InstallFileSpec {
                kind: FileKind::Data,
                source: None,
                dest: Some("{bindir}/agentsight".to_string()),
                mode: None,
            }],
            resolved_files,
            capabilities: Vec::new(),
            requires_privilege: true,
            env_requirements: EnvRequirements::default(),
            health_check: None,
        }
    }

    fn fixture_plan(
        capability: &str,
        components: Vec<ComponentPlan>,
        status: PlanStatus,
        install_mode: &str,
        layout: &FsLayout,
    ) -> EnablePlan {
        EnablePlan {
            schema_version: PLAN_SCHEMA_VERSION,
            capability: capability.to_string(),
            stability: "stable".to_string(),
            install_mode: install_mode.to_string(),
            dry_run: false,
            status,
            blocked_reason: if status == PlanStatus::Blocked {
                Some("test blocker".to_string())
            } else {
                None
            },
            components,
            prechecks: Vec::new(),
            env_facts: EnvFactsSummary {
                os: "linux".to_string(),
                arch: "x86_64".to_string(),
                libc: Some("glibc".to_string()),
                pkg_base: Some("anolis23".to_string()),
                kernel: Some("6.6.0".to_string()),
                btf: Some(true),
                cap_bpf: Some(true),
            },
            layout: LayoutSummary {
                bin_dir: layout.bin_dir.display().to_string(),
                etc_dir: layout.etc_dir.display().to_string(),
                state_dir: layout.state_dir.display().to_string(),
                log_dir: layout.log_dir.display().to_string(),
                manifests_overlay: layout.manifests_overlay.display().to_string(),
            },
            warnings: Vec::new(),
            advice: Vec::new(),
            next_actions: Vec::new(),
            lint: Vec::new(),
            execute_gate: None,
        }
    }

    fn artifact_plan(url: &str, sha256: &str) -> ArtifactPlan {
        ArtifactPlan {
            artifact_type: "binary".to_string(),
            backend: "binary".to_string(),
            version: "0.2.0".to_string(),
            url: url.to_string(),
            sha256: Some(sha256.to_string()),
            signature: None,
            artifact_id: None,
            meta_sha256: None,
        }
    }

    fn read_log_lines(path: &Path) -> Vec<serde_json::Value> {
        let content = std_fs::read_to_string(path).expect("read log");
        content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).expect("parse log line"))
            .collect()
    }

    #[cfg(unix)]
    fn write_hook_script(layout: &FsLayout, component: &str, phase: &str, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        let dir = layout.datadir.join("hooks").join(component);
        std_fs::create_dir_all(&dir).expect("mkdir hook dir");
        let path = dir.join(format!("{phase}.sh"));
        std_fs::write(&path, body).expect("write hook");
        let mut perm = std_fs::metadata(&path).expect("stat hook").permissions();
        perm.set_mode(0o755);
        std_fs::set_permissions(&path, perm).expect("chmod hook");
    }

    /// Wires that pre_enable and post_enable hooks discovered under
    /// `<datadir>/hooks/<component>/<phase>.sh` actually run during a
    /// real install AND emit a `LogKind::Component` record per attempt
    /// to the central log. This is the contract that protects against
    /// the runner being module-only — if a future refactor stops
    /// invoking `run_phase_hooks`, this test fails immediately rather
    /// than silently dropping hook execution.
    #[test]
    #[cfg(unix)]
    fn enable_runs_pre_and_post_hooks_and_records_them_in_central_log() {
        let root = tempdir().expect("tempdir");
        let payloads = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());

        let payload = b"hook-fixture-bytes";
        let (url, sha) = write_payload_artifact(payloads.path(), "agentsight", payload);

        // Drop both hooks under the conventional path. Bodies are
        // distinct so we can confirm the right script ran for each
        // phase via the `details.script` field.
        write_hook_script(&layout, "agentsight", "pre_enable", "#!/bin/sh\nexit 0\n");
        write_hook_script(&layout, "agentsight", "post_enable", "#!/bin/sh\nexit 0\n");

        let dest = layout.bin_dir.join("agentsight");
        let comp = fixture_component(
            "agentsight",
            Some(artifact_plan(&url, &sha)),
            vec![dest.display().to_string()],
            PlanStatus::Ready,
        );
        let plan = fixture_plan(
            "agent-observability",
            vec![comp],
            PlanStatus::Ready,
            "system",
            &layout,
        );

        let outcome = execute_enable(&plan, &layout, "tester").expect("execute ok");

        // Both hook attempts must land as separate `kind=component`
        // records on the central log, distinct from the operation
        // started/succeeded pair.
        let lines = read_log_lines(&layout.central_log);
        // Filter on `command starts with "hook:"` so service-op
        // component records (start/stop, supported or unsupported skip)
        // do not pollute the assertion — they're the responsibility of
        // a separate test.
        let hook_lines: Vec<_> = lines
            .iter()
            .filter(|l| l.get("kind").and_then(|v| v.as_str()) == Some("component"))
            .filter(|l| {
                l.get("command")
                    .and_then(|v| v.as_str())
                    .map(|c| c.starts_with("hook:"))
                    .unwrap_or(false)
            })
            .collect();
        assert_eq!(
            hook_lines.len(),
            2,
            "expected pre+post enable hook log entries, got: {lines:?}",
        );
        let commands: Vec<&str> = hook_lines
            .iter()
            .map(|l| l.get("command").and_then(|v| v.as_str()).unwrap_or(""))
            .collect();
        assert!(
            commands.contains(&"hook:pre_enable") && commands.contains(&"hook:post_enable"),
            "hook records must name both phases: {commands:?}",
        );
        // Every hook record must point back at the operation we just ran
        // so `anolisa logs --op-id` correlates them with the parent verb.
        for hl in &hook_lines {
            assert_eq!(
                hl.get("operation_id").and_then(|v| v.as_str()),
                Some(outcome.operation_id.as_str()),
            );
            assert_eq!(
                hl.get("component").and_then(|v| v.as_str()),
                Some("agentsight"),
            );
        }
    }

    #[test]
    fn happy_path_single_binary_installs_writes_state_and_two_logs() {
        let root = tempdir().expect("tempdir");
        let payloads = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());

        let payload = b"fake-agentsight-binary-bytes";
        let (url, sha) = write_payload_artifact(payloads.path(), "agentsight", payload);

        let dest = layout.bin_dir.join("agentsight");
        let comp = fixture_component(
            "agentsight",
            Some(artifact_plan(&url, &sha)),
            vec![dest.display().to_string()],
            PlanStatus::Ready,
        );
        let plan = fixture_plan(
            "agent-observability",
            vec![comp],
            PlanStatus::Ready,
            "system",
            &layout,
        );

        let outcome = execute_enable(&plan, &layout, "tester").expect("execute ok");
        assert_eq!(outcome.installed_files.len(), 1);
        assert_eq!(outcome.installed_files[0].path, dest);
        assert_eq!(outcome.installed_files[0].sha256, sha);
        assert!(dest.exists(), "destination binary must exist");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std_fs::metadata(&dest).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o755);
        }

        // installed.toml — capability + component objects, one ok operation.
        let state_path = layout.state_dir.join("installed.toml");
        assert!(state_path.exists());
        let state = InstalledState::load(&state_path).expect("load state");
        let cap = state
            .find_object(ObjectKind::Capability, "agent-observability")
            .expect("capability object present");
        assert_eq!(cap.status, ObjectStatus::Installed);
        let agentsight = state
            .find_object(ObjectKind::Component, "agentsight")
            .expect("component object present");
        assert_eq!(agentsight.status, ObjectStatus::Installed);
        assert_eq!(agentsight.files.len(), 1);
        assert_eq!(agentsight.files[0].path, dest);
        assert_eq!(state.operations.len(), 1);
        assert_eq!(state.operations[0].status, "ok");
        assert_eq!(state.operations[0].id, outcome.operation_id);

        // central log — exactly 2 operation lines, both for this op,
        // second is "ok". Component-kind records (service:start /
        // hooks) are out of scope for this test; filter them out so
        // adding/removing those records doesn't break the
        // started/succeeded contract this test pins.
        let all_lines = read_log_lines(&layout.central_log);
        let lines: Vec<&serde_json::Value> = all_lines
            .iter()
            .filter(|l| l.get("kind").and_then(|v| v.as_str()) == Some("operation"))
            .collect();
        assert_eq!(
            lines.len(),
            2,
            "expected started + succeeded operation entries, got: {all_lines:?}",
        );
        for line in &lines {
            assert_eq!(
                line.get("operation_id").and_then(|v| v.as_str()),
                Some(outcome.operation_id.as_str()),
            );
        }
        assert!(lines[0].get("status").map(|v| v.is_null()).unwrap_or(true));
        assert_eq!(lines[1].get("status").and_then(|v| v.as_str()), Some("ok"),);
    }

    #[test]
    fn health_check_pass_records_entry_and_keeps_installed() {
        let root = tempdir().expect("tempdir");
        let payloads = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());

        let payload = b"fake-agentsight-binary-bytes";
        let (url, sha) = write_payload_artifact(payloads.path(), "agentsight", payload);

        let dest = layout.bin_dir.join("agentsight");
        let mut comp = fixture_component(
            "agentsight",
            Some(artifact_plan(&url, &sha)),
            vec![dest.display().to_string()],
            PlanStatus::Ready,
        );
        // A file_exists probe over the just-installed binary passes; the
        // `{bindir}` placeholder is expanded by the engine against the layout.
        comp.health_check = Some(CheckSpec::FileExists {
            path: "{bindir}/agentsight".to_string(),
            mode: None,
            owner: None,
        });
        let plan = fixture_plan(
            "agent-observability",
            vec![comp],
            PlanStatus::Ready,
            "system",
            &layout,
        );

        let outcome = execute_enable(&plan, &layout, "tester").expect("execute ok");
        assert!(
            outcome.warnings.iter().all(|w| !w.contains("health check")),
            "no health warning expected: {:?}",
            outcome.warnings,
        );

        let state =
            InstalledState::load(&layout.state_dir.join("installed.toml")).expect("load state");
        let agentsight = state
            .find_object(ObjectKind::Component, "agentsight")
            .expect("component object present");
        assert_eq!(agentsight.status, ObjectStatus::Installed);
        assert_eq!(agentsight.health.len(), 1, "one health entry recorded");
        assert_eq!(agentsight.health[0].status, "ok");
        let cap = state
            .find_object(ObjectKind::Capability, "agent-observability")
            .expect("capability object present");
        assert_eq!(cap.status, ObjectStatus::Installed);
    }

    #[test]
    fn health_check_failure_degrades_to_partial_with_warning() {
        let root = tempdir().expect("tempdir");
        let payloads = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());

        let payload = b"fake-agentsight-binary-bytes";
        let (url, sha) = write_payload_artifact(payloads.path(), "agentsight", payload);

        let dest = layout.bin_dir.join("agentsight");
        let mut comp = fixture_component(
            "agentsight",
            Some(artifact_plan(&url, &sha)),
            vec![dest.display().to_string()],
            PlanStatus::Ready,
        );
        // Probe a sibling path that was never installed → hard failure. The
        // install itself must stay committed: enable degrades, never rolls back.
        comp.health_check = Some(CheckSpec::FileExists {
            path: "{bindir}/never-installed".to_string(),
            mode: None,
            owner: None,
        });
        let plan = fixture_plan(
            "agent-observability",
            vec![comp],
            PlanStatus::Ready,
            "system",
            &layout,
        );

        let outcome = execute_enable(&plan, &layout, "tester").expect("execute still succeeds");
        // Bytes are on disk despite the failed probe.
        assert!(
            dest.exists(),
            "install must not roll back on health failure"
        );
        assert!(
            outcome
                .warnings
                .iter()
                .any(|w| w.contains("health check failed")),
            "expected a health warning: {:?}",
            outcome.warnings,
        );

        let state =
            InstalledState::load(&layout.state_dir.join("installed.toml")).expect("load state");
        let agentsight = state
            .find_object(ObjectKind::Component, "agentsight")
            .expect("component object present");
        assert_eq!(
            agentsight.status,
            ObjectStatus::Partial,
            "degraded, not failed",
        );
        assert_eq!(agentsight.health.len(), 1);
        assert_eq!(agentsight.health[0].status, "failed");
        let cap = state
            .find_object(ObjectKind::Capability, "agent-observability")
            .expect("capability object present");
        assert_eq!(
            cap.status,
            ObjectStatus::Partial,
            "degradation rolls up to the capability",
        );
        // The operation itself is still recorded ok — enable succeeded.
        assert_eq!(state.operations[0].status, "ok");
    }

    #[test]
    fn blocked_plan_is_rejected_with_no_side_effects() {
        let root = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());

        let comp = fixture_component(
            "agentsight",
            None,
            vec![layout.bin_dir.join("agentsight").display().to_string()],
            PlanStatus::Blocked,
        );
        let plan = fixture_plan(
            "agent-observability",
            vec![comp],
            PlanStatus::Blocked,
            "system",
            &layout,
        );

        let err = execute_enable(&plan, &layout, "tester").expect_err("must error");
        assert!(
            matches!(err, ExecuteError::PlanNotExecutable { ref status, .. } if status == "blocked"),
            "unexpected error: {err:?}",
        );

        // No log file, no state file, no install touched. Also no
        // lock file: a fresh blocked plan must short-circuit before
        // `InstallLock::acquire`, which would otherwise `create_dir_all`
        // and write `<state_dir>/lock`. This pins the read-only
        // reactivation preflight: when state has no Disabled candidate,
        // execute_enable must not touch the filesystem at all.
        assert!(!layout.central_log.exists());
        assert!(!layout.state_dir.join("installed.toml").exists());
        assert!(
            !layout.lock_file.exists(),
            "blocked fresh plan must not create the lock file (got {})",
            layout.lock_file.display(),
        );
    }

    #[test]
    fn lock_contention_returns_lock_held() {
        let root = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());

        // Hold the lock from the outside for the duration of execute_enable.
        let _held = InstallLock::acquire(&layout.lock_file).expect("hold lock");

        let comp = fixture_component(
            "agentsight",
            Some(artifact_plan("file:///does/not/matter", &"0".repeat(64))),
            vec![layout.bin_dir.join("agentsight").display().to_string()],
            PlanStatus::Ready,
        );
        let plan = fixture_plan(
            "agent-observability",
            vec![comp],
            PlanStatus::Ready,
            "system",
            &layout,
        );

        let err = execute_enable(&plan, &layout, "tester").expect_err("must error");
        match err {
            ExecuteError::LockHeld { path } => assert_eq!(path, layout.lock_file),
            other => panic!("expected LockHeld, got {other:?}"),
        }
        // No log, no state.
        assert!(!layout.central_log.exists());
        assert!(!layout.state_dir.join("installed.toml").exists());
    }

    #[test]
    fn checksum_mismatch_cleans_up_partial_install_and_writes_failed_log() {
        let root = tempdir().expect("tempdir");
        let payloads = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());

        let payload_a = b"good-component-a";
        let (url_a, sha_a) = write_payload_artifact(payloads.path(), "comp-a", payload_a);

        // Component B's artifact has correct content but wrong expected sha.
        let payload_b = b"good-component-b";
        let (url_b, _) = write_payload_artifact(payloads.path(), "comp-b", payload_b);
        let wrong_sha_b = "0".repeat(64);

        let dest_a = layout.bin_dir.join("comp-a");
        let dest_b = layout.bin_dir.join("comp-b");
        let comp_a = fixture_component(
            "comp-a",
            Some(artifact_plan(&url_a, &sha_a)),
            vec![dest_a.display().to_string()],
            PlanStatus::Ready,
        );
        let comp_b = fixture_component(
            "comp-b",
            Some(artifact_plan(&url_b, &wrong_sha_b)),
            vec![dest_b.display().to_string()],
            PlanStatus::Ready,
        );
        let plan = fixture_plan(
            "agent-observability",
            vec![comp_a, comp_b],
            PlanStatus::Ready,
            "system",
            &layout,
        );

        let err = execute_enable(&plan, &layout, "tester").expect_err("must error");
        match err {
            ExecuteError::Download {
                ref component,
                source: DownloadError::ChecksumMismatch { .. },
            } => assert_eq!(component, "comp-b"),
            other => panic!("expected Download/ChecksumMismatch on comp-b, got {other:?}"),
        }

        // Component A's file must have been unlinked by cleanup.
        assert!(!dest_a.exists(), "comp-a file must be cleaned up");
        assert!(!dest_b.exists(), "comp-b file was never installed");

        // No state file (failure before save).
        assert!(!layout.state_dir.join("installed.toml").exists());

        // Two log lines: started (info) + failed (status=failed).
        let lines = read_log_lines(&layout.central_log);
        assert_eq!(lines.len(), 2);
        let op_id = lines[0]
            .get("operation_id")
            .and_then(|v| v.as_str())
            .expect("op id on started");
        assert_eq!(
            lines[1].get("operation_id").and_then(|v| v.as_str()),
            Some(op_id),
        );
        assert_eq!(
            lines[1].get("status").and_then(|v| v.as_str()),
            Some("failed"),
        );
        assert_eq!(
            lines[1].get("severity").and_then(|v| v.as_str()),
            Some("error"),
        );
    }

    #[test]
    fn missing_artifact_returns_missing_artifact_error_with_no_install() {
        let root = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());

        let dest = layout.bin_dir.join("agentsight");
        // Degraded is still executable per spec; we just have no artifact.
        let comp = fixture_component(
            "agentsight",
            None,
            vec![dest.display().to_string()],
            PlanStatus::Degraded,
        );
        let plan = fixture_plan(
            "agent-observability",
            vec![comp],
            PlanStatus::Degraded,
            "system",
            &layout,
        );

        let err = execute_enable(&plan, &layout, "tester").expect_err("must error");
        match err {
            ExecuteError::MissingArtifact { ref component } => assert_eq!(component, "agentsight"),
            other => panic!("expected MissingArtifact, got {other:?}"),
        }

        assert!(!dest.exists());
        assert!(!layout.state_dir.join("installed.toml").exists());

        // Started + failed log entries (started written before the per-component loop).
        let lines = read_log_lines(&layout.central_log);
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[1].get("status").and_then(|v| v.as_str()),
            Some("failed"),
        );
    }

    /// Executor-level checksum hard guard: even though the planner now
    /// marks missing-sha256 plans Blocked, `execute_enable` is a public
    /// API — a hand-built plan could still arrive with `artifact.sha256:
    /// None`. The executor must refuse without touching the disk: no
    /// download, no install, no state file. The started/failed audit
    /// records are still expected (the lock was acquired and started was
    /// already written before we hit the per-component loop).
    #[test]
    fn missing_checksum_in_artifact_returns_missing_checksum_error_with_no_install() {
        let root = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());

        // Construct a plan that bypasses the planner's missing-sha guard:
        // status=Ready, artifact present, but sha256=None.
        let dest = layout.bin_dir.join("agentsight");
        let artifact_no_sha = ArtifactPlan {
            artifact_type: "binary".to_string(),
            backend: "binary".to_string(),
            version: "0.2.0".to_string(),
            url: "file:///does/not/matter".to_string(),
            sha256: None,
            signature: None,
            artifact_id: None,
            meta_sha256: None,
        };
        let comp = fixture_component(
            "agentsight",
            Some(artifact_no_sha),
            vec![dest.display().to_string()],
            PlanStatus::Ready,
        );
        let plan = fixture_plan(
            "agent-observability",
            vec![comp],
            PlanStatus::Ready,
            "system",
            &layout,
        );

        let err = execute_enable(&plan, &layout, "tester").expect_err("must error");
        match err {
            ExecuteError::MissingChecksum { ref component } => assert_eq!(component, "agentsight"),
            other => panic!("expected MissingChecksum, got {other:?}"),
        }

        // No file installed, no state file written.
        assert!(!dest.exists(), "no file may be installed without sha256");
        assert!(
            !layout.state_dir.join("installed.toml").exists(),
            "no state file may be created when the executor refuses to install",
        );

        // Started + failed audit records.
        let lines = read_log_lines(&layout.central_log);
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[1].get("status").and_then(|v| v.as_str()),
            Some("failed"),
        );
    }

    /// Regression for the snapshot+restore path: when `state.save()` fails
    /// AND there is a pre-op `installed.toml`, the prior file must remain
    /// intact (cleanup must not delete it). We force the save failure by
    /// pre-creating `installed.toml`'s `.tmp` sibling as a *directory* so
    /// `fs::write(&tmp, ...)` inside `InstalledState::save` errors out
    /// before the rename.
    ///
    /// The prior state is built with `InstalledState::default().save(...)`
    /// so the file is a *real* serialized state (not just a TOML comment).
    /// That way `InstalledState::load` actually parses a populated-shape
    /// document and the test exercises the real cleanup path — losing
    /// the prior state of an existing install is the worst-case failure
    /// for a package manager and is what this regression locks down.
    #[test]
    #[cfg(unix)]
    fn state_save_failure_restores_prior_installed_toml() {
        if nix::unistd::Uid::effective().is_root() {
            // CAP_DAC_OVERRIDE bypasses the `chmod 0o500` sabotage that
            // `sabotage_state_save_unix` relies on, so this regression
            // can only be exercised under an unprivileged uid.
            eprintln!("skipping state_save_failure_restores_prior_installed_toml under uid 0");
            return;
        }
        let root = tempdir().expect("tempdir");
        let payloads = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());

        let payload = b"new-agentsight-bytes";
        let (url, sha) = write_payload_artifact(payloads.path(), "agentsight", payload);

        // Build a valid prior installed.toml using the real serializer
        // and snapshot its bytes; that snapshot is what cleanup must
        // restore byte-for-byte after the failed save.
        let state_path = layout.state_dir.join("installed.toml");
        std_fs::create_dir_all(&layout.state_dir).unwrap();
        InstalledState::default()
            .save(&state_path)
            .expect("prior state save");
        let prior_bytes = std_fs::read(&state_path).expect("read prior bytes");

        // Trip InstalledState::save by chmod'ing `state_dir` to read-only
        // *after* pre-creating the lock file (so InstallLock::acquire can
        // still open the existing path). The salted tmp-sibling name in
        // P0-A means we cannot squat on a fixed `.installed.toml.tmp`
        // path anymore — but the executor's tmp create_new still requires
        // write perms on the parent directory, which 0o500 strips.
        let original_perms = sabotage_state_save_unix(&layout);

        let dest = layout.bin_dir.join("agentsight");
        let comp = fixture_component(
            "agentsight",
            Some(artifact_plan(&url, &sha)),
            vec![dest.display().to_string()],
            PlanStatus::Ready,
        );
        let plan = fixture_plan(
            "agent-observability",
            vec![comp],
            PlanStatus::Ready,
            "system",
            &layout,
        );

        let result = execute_enable(&plan, &layout, "tester");

        // Restore writable perms before assertions so reads / cleanup work.
        restore_state_dir_perms(&layout, original_perms);

        let err = result.expect_err("must fail at state.save");
        assert!(
            matches!(err, ExecuteError::State { .. }),
            "unexpected error: {err:?}",
        );

        // Prior installed.toml content is unchanged byte-for-byte.
        let after = std_fs::read(&state_path).expect("installed.toml still readable");
        assert_eq!(
            after, prior_bytes,
            "cleanup must restore the prior installed.toml byte-for-byte",
        );
        // Belt-and-suspenders: the restored bytes still parse as a valid
        // InstalledState — proof we did not leave a truncated/garbled file.
        let _: InstalledState = InstalledState::load(&state_path).expect("prior state reparses");

        // Installed binary was unlinked.
        assert!(!dest.exists(), "cleanup must unlink installed files");
        // A failed audit record was appended.
        let lines = read_log_lines(&layout.central_log);
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[1].get("status").and_then(|v| v.as_str()),
            Some("failed"),
        );
    }

    /// Same trip, but with NO pre-existing `installed.toml`. Cleanup must
    /// remove any state file this op wrote (here state.save fails before
    /// the rename so nothing is on disk anyway — the assertion is "still
    /// nothing", confirming the `None` snapshot branch is a no-op rather
    /// than accidentally writing an empty file).
    #[test]
    #[cfg(unix)]
    fn state_save_failure_no_prior_state_leaves_no_installed_toml() {
        if nix::unistd::Uid::effective().is_root() {
            // CAP_DAC_OVERRIDE bypasses the `chmod 0o500` sabotage that
            // `sabotage_state_save_unix` relies on, so this regression
            // can only be exercised under an unprivileged uid.
            eprintln!(
                "skipping state_save_failure_no_prior_state_leaves_no_installed_toml under uid 0"
            );
            return;
        }
        let root = tempdir().expect("tempdir");
        let payloads = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());

        let payload = b"new-agentsight-bytes";
        let (url, sha) = write_payload_artifact(payloads.path(), "agentsight", payload);

        // Same chmod-based sabotage as the with-prior variant. Pre-create
        // lock_file/state_dir/log_dir, then strip +w on state_dir so the
        // first-time `state.save` fails on its tmp sibling create_new.
        let original_perms = sabotage_state_save_unix(&layout);

        let dest = layout.bin_dir.join("agentsight");
        let comp = fixture_component(
            "agentsight",
            Some(artifact_plan(&url, &sha)),
            vec![dest.display().to_string()],
            PlanStatus::Ready,
        );
        let plan = fixture_plan(
            "agent-observability",
            vec![comp],
            PlanStatus::Ready,
            "system",
            &layout,
        );

        let result = execute_enable(&plan, &layout, "tester");

        restore_state_dir_perms(&layout, original_perms);

        let err = result.expect_err("must fail at state.save");
        assert!(
            matches!(err, ExecuteError::State { .. }),
            "unexpected error: {err:?}",
        );
        assert!(
            !layout.state_dir.join("installed.toml").exists(),
            "no installed.toml may leak from a failed first-time enable",
        );
        assert!(!dest.exists());
    }

    // ── Reactivate-from-Disabled (P1) ────────────────────────────────
    //
    // These tests drive a full install → disable → enable cycle. The
    // second `execute_enable` must detect the `Disabled` capability,
    // sha256-verify every owned file, and flip state back to `Installed`
    // *without* invoking the DownloadCache / InstallRunner. The mismatch
    // sibling proves the gate is sha256-driven: a corrupted file must
    // surface `DisabledStateInconsistent` with no audit log entry, so a
    // future bug that silently re-enables a tampered binary would break
    // a test.

    /// Happy path: install → disable → re-enable. The third call must
    /// return `reactivated = true`, not write any new files, flip both
    /// the capability and component back to `Installed`, append a single
    /// `OperationRecord`, and emit a started+succeeded log pair whose
    /// succeeded message mentions reactivation. The on-disk binary must
    /// be the *same inode* (mtime unchanged) the install left behind —
    /// the contract is "no reinstall", so a fresh write here would be a
    /// regression.
    #[test]
    fn enable_reactivates_from_disabled_when_all_files_match_sha256() {
        let root = tempdir().expect("tempdir");
        let payloads = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());

        let payload = b"reactivate-fixture-bytes";
        let (url, sha) = write_payload_artifact(payloads.path(), "agentsight", payload);

        let dest = layout.bin_dir.join("agentsight");
        let comp = fixture_component(
            "agentsight",
            Some(artifact_plan(&url, &sha)),
            vec![dest.display().to_string()],
            PlanStatus::Ready,
        );
        let plan = fixture_plan(
            "agent-observability",
            vec![comp],
            PlanStatus::Ready,
            "system",
            &layout,
        );

        let install = execute_enable(&plan, &layout, "tester").expect("install ok");
        assert!(
            !install.reactivated,
            "first install must not be a reactivation"
        );
        let install_mtime = std_fs::metadata(&dest)
            .expect("dest exists")
            .modified()
            .expect("mtime");

        // Disable the capability so the re-enable hits the reactivation
        // path rather than a no-op idempotent install.
        crate::execute_disable(&layout, "agent-observability", "tester", "system")
            .expect("disable ok");

        let state_path = layout.state_dir.join("installed.toml");
        let mid_state = InstalledState::load(&state_path).expect("load mid state");
        assert_eq!(
            mid_state
                .find_object(ObjectKind::Capability, "agent-observability")
                .expect("cap")
                .status,
            ObjectStatus::Disabled,
        );

        // Count only operation-kind records as the "logs_before"
        // baseline. Component-kind records (service:start/stop, hook
        // outcomes) are accounted for separately and would otherwise
        // make the started+succeeded delta hard to reason about.
        let logs_before = read_log_lines(&layout.central_log)
            .iter()
            .filter(|l| l.get("kind").and_then(|v| v.as_str()) == Some("operation"))
            .count();
        let ops_before = mid_state.operations.len();

        let outcome = execute_enable(&plan, &layout, "tester").expect("re-enable ok");
        assert!(
            outcome.reactivated,
            "second enable on disabled cap must reactivate"
        );
        // Reactivation echoes existing files for display continuity.
        assert_eq!(outcome.installed_files.len(), 1);
        assert_eq!(outcome.installed_files[0].path, dest);
        assert_eq!(outcome.installed_files[0].sha256, sha);

        // The on-disk binary must NOT have been rewritten.
        let post_mtime = std_fs::metadata(&dest)
            .expect("dest exists")
            .modified()
            .expect("mtime");
        assert_eq!(
            install_mtime, post_mtime,
            "reactivation must not rewrite the existing file (mtime drifted)",
        );

        // State: capability + component back to Installed, new
        // operation appended, last_operation_id refreshed.
        let final_state = InstalledState::load(&state_path).expect("load final state");
        let cap = final_state
            .find_object(ObjectKind::Capability, "agent-observability")
            .expect("cap");
        assert_eq!(cap.status, ObjectStatus::Installed);
        assert_eq!(
            cap.last_operation_id.as_deref(),
            Some(outcome.operation_id.as_str())
        );
        let comp = final_state
            .find_object(ObjectKind::Component, "agentsight")
            .expect("comp");
        assert_eq!(comp.status, ObjectStatus::Installed);
        assert_eq!(
            comp.last_operation_id.as_deref(),
            Some(outcome.operation_id.as_str())
        );
        assert_eq!(
            final_state.operations.len(),
            ops_before + 1,
            "exactly one new OperationRecord must be appended",
        );
        let last_op = final_state.operations.last().expect("op");
        assert_eq!(last_op.id, outcome.operation_id);
        assert_eq!(last_op.status, "ok");

        // Central log: two new operation lines (started + succeeded),
        // succeeded message must call out reactivation so operators
        // can grep. Filter out component-kind records to keep the
        // delta math focused on the verb-level audit pair.
        let all_lines = read_log_lines(&layout.central_log);
        let lines: Vec<&serde_json::Value> = all_lines
            .iter()
            .filter(|l| l.get("kind").and_then(|v| v.as_str()) == Some("operation"))
            .collect();
        assert_eq!(
            lines.len(),
            logs_before + 2,
            "started + succeeded for reactivation"
        );
        let new_lines = &lines[logs_before..];
        assert!(
            new_lines[0]
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .contains("reactivating from disabled"),
            "started message must announce reactivation: {:?}",
            new_lines[0],
        );
        assert!(
            new_lines[1]
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .contains("reactivated from disabled"),
            "succeeded message must confirm reactivation: {:?}",
            new_lines[1],
        );
        assert_eq!(
            new_lines[1].get("status").and_then(|v| v.as_str()),
            Some("ok"),
        );
    }

    /// Regression: a Disabled capability must still reactivate even
    /// when the *fresh* plan is `Blocked` (e.g. distribution-index
    /// dropped the artifact entry, or a host env precheck regressed
    /// after install). The reactivation path doesn't touch the
    /// resolver, so artifact-resolver blockers are irrelevant; and
    /// the files are sha256-verified, so the install-time prereqs
    /// have already been honored. This pins the order of the
    /// reactivation preflight vs. the blocked-plan gate so a future
    /// refactor cannot accidentally swap them back.
    #[test]
    fn enable_reactivates_disabled_capability_even_when_fresh_plan_is_blocked() {
        let root = tempdir().expect("tempdir");
        let payloads = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());

        let payload = b"reactivate-despite-blocked-plan";
        let (url, sha) = write_payload_artifact(payloads.path(), "agentsight", payload);

        let dest = layout.bin_dir.join("agentsight");
        let install_comp = fixture_component(
            "agentsight",
            Some(artifact_plan(&url, &sha)),
            vec![dest.display().to_string()],
            PlanStatus::Ready,
        );
        let install_plan = fixture_plan(
            "agent-observability",
            vec![install_comp],
            PlanStatus::Ready,
            "system",
            &layout,
        );
        execute_enable(&install_plan, &layout, "tester").expect("install ok");
        crate::execute_disable(&layout, "agent-observability", "tester", "system")
            .expect("disable ok");

        // Build a Blocked plan that mirrors what `plan_enable` would
        // produce after a real env regression: top-level
        // `blocked_reason`, a failing precheck (kernel too old), AND a
        // resolver soft signal in `plan.warnings`. The reactivation
        // path must surface all three categories — using only
        // `plan.warnings` would hide the hard signals that turned the
        // fresh plan Blocked. Without the preflight-before-gate order
        // this would surface as `PlanNotExecutable` instead of
        // reactivating.
        let blocked_comp = fixture_component(
            "agentsight",
            None,
            vec![dest.display().to_string()],
            PlanStatus::Blocked,
        );
        let mut blocked_plan = fixture_plan(
            "agent-observability",
            vec![blocked_comp],
            PlanStatus::Blocked,
            "system",
            &layout,
        );
        blocked_plan.blocked_reason = Some("kernel 5.10 below required 6.0".to_string());
        blocked_plan
            .prechecks
            .push(crate::enable_plan::PrecheckResult {
                name: "kernel_min".to_string(),
                status: "fail".to_string(),
                expected: "6.0".to_string(),
                actual: "5.10".to_string(),
                message: Some("host kernel regressed after install".to_string()),
            });
        blocked_plan
            .warnings
            .push("distribution-index missing artifact for host".to_string());

        let outcome = execute_enable(&blocked_plan, &layout, "tester").expect("must reactivate");
        assert!(
            outcome.reactivated,
            "blocked plan + disabled cap must still reactivate, not refuse",
        );
        assert_eq!(outcome.installed_files.len(), 1);
        assert_eq!(outcome.installed_files[0].path, dest);
        // All three categories of "you got dropped for a reason" must
        // surface in outcome.warnings: resolver soft signal
        // (plan.warnings), top-level blocker (plan.blocked_reason),
        // and failing precheck (plan.prechecks). A silent reactivation
        // that hid the hard signals would let the user think the host
        // still satisfies the prereqs, which it may not.
        assert!(
            outcome
                .warnings
                .iter()
                .any(|w| w.contains("distribution-index missing artifact")),
            "plan.warnings must be carried into outcome: {:?}",
            outcome.warnings,
        );
        assert!(
            outcome
                .warnings
                .iter()
                .any(|w| w.contains("kernel 5.10 below required 6.0")),
            "plan.blocked_reason must be surfaced in outcome: {:?}",
            outcome.warnings,
        );
        assert!(
            outcome
                .warnings
                .iter()
                .any(|w| w.contains("blocked precheck") && w.contains("kernel_min")),
            "failing prechecks must be surfaced in outcome: {:?}",
            outcome.warnings,
        );

        // Same warnings must also land in the persisted succeeded
        // log record — the in-memory outcome alone is not enough for
        // post-hoc audit consumers.
        let lines = read_log_lines(&layout.central_log);
        let succeeded = lines
            .iter()
            .rfind(|l| l.get("status").and_then(|s| s.as_str()) == Some("ok"))
            .expect("succeeded log line");
        let log_warnings: Vec<&str> = succeeded
            .get("warnings")
            .and_then(|w| w.as_array())
            .expect("warnings array")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(
            log_warnings
                .iter()
                .any(|w| w.contains("kernel 5.10 below required 6.0")),
            "blocked_reason must persist in central log warnings: {log_warnings:?}",
        );
        assert!(
            log_warnings
                .iter()
                .any(|w| w.contains("blocked precheck") && w.contains("kernel_min")),
            "failing precheck must persist in central log warnings: {log_warnings:?}",
        );

        let state =
            InstalledState::load(&layout.state_dir.join("installed.toml")).expect("load state");
        assert_eq!(
            state
                .find_object(ObjectKind::Capability, "agent-observability")
                .expect("cap")
                .status,
            ObjectStatus::Installed,
        );
    }

    /// Mismatch path: install → disable → corrupt the file → enable.
    /// The reactivation gate must refuse with `DisabledStateInconsistent`,
    /// write NO new central-log entries (matching the
    /// `CapabilityNotInstalled` no-log convention), and leave state
    /// untouched (capability stays `Disabled`).
    #[test]
    fn enable_refuses_reactivation_when_owned_file_sha256_mismatches() {
        let root = tempdir().expect("tempdir");
        let payloads = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());

        let payload = b"original-bytes";
        let (url, sha) = write_payload_artifact(payloads.path(), "agentsight", payload);

        let dest = layout.bin_dir.join("agentsight");
        let comp = fixture_component(
            "agentsight",
            Some(artifact_plan(&url, &sha)),
            vec![dest.display().to_string()],
            PlanStatus::Ready,
        );
        let plan = fixture_plan(
            "agent-observability",
            vec![comp],
            PlanStatus::Ready,
            "system",
            &layout,
        );

        execute_enable(&plan, &layout, "tester").expect("install ok");
        crate::execute_disable(&layout, "agent-observability", "tester", "system")
            .expect("disable ok");

        // Corrupt the on-disk binary so its sha256 no longer matches
        // the value recorded in state.
        std_fs::write(&dest, b"tampered-bytes").expect("corrupt dest");

        let state_path = layout.state_dir.join("installed.toml");
        let state_before = std_fs::read(&state_path).expect("snapshot state");
        let logs_before = read_log_lines(&layout.central_log).len();

        let err = execute_enable(&plan, &layout, "tester")
            .expect_err("reactivation must refuse on sha256 drift");
        match err {
            ExecuteError::DisabledStateInconsistent {
                capability,
                mismatches,
            } => {
                assert_eq!(capability, "agent-observability");
                assert_eq!(
                    mismatches.len(),
                    1,
                    "one corrupted file → one mismatch line: {mismatches:?}",
                );
                assert!(
                    mismatches[0].contains("sha256 mismatch"),
                    "diagnostic must name the mismatch: {}",
                    mismatches[0],
                );
                assert!(
                    mismatches[0].contains(dest.to_str().unwrap()),
                    "diagnostic must name the offending path: {}",
                    mismatches[0],
                );
            }
            other => panic!("expected DisabledStateInconsistent, got {other:?}"),
        }

        // No central-log entry written, no state mutation.
        assert_eq!(
            read_log_lines(&layout.central_log).len(),
            logs_before,
            "DisabledStateInconsistent must not append to central log",
        );
        assert_eq!(
            std_fs::read(&state_path).expect("read state"),
            state_before,
            "state must be byte-identical when reactivation refuses",
        );
        let post = InstalledState::load(&state_path).expect("load");
        assert_eq!(
            post.find_object(ObjectKind::Capability, "agent-observability")
                .expect("cap")
                .status,
            ObjectStatus::Disabled,
            "capability must remain Disabled when reactivation refuses",
        );
    }
}
