//! End-to-end orchestrator for `anolisa disable <capability>` (P1-I).
//!
//! **Scope.** Disable flips the capability object — and every component
//! listed in its `component_refs` — to `ObjectStatus::Disabled`, runs the
//! disable lifecycle hooks that are strict or best-effort by phase, stops
//! owned service units on a best-effort basis, and writes matching
//! `started` / `succeeded` records to the central log. It does NOT:
//!
//!   * delete files (`OwnedFile` paths stay on disk);
//!   * `systemctl disable` service units or remove unit files;
//!   * touch the distribution-index, the download cache, or the install
//!     runner;
//!   * take a backup or build a transaction frame.
//!
//! Full teardown (file removal, service disablement, external-config
//! rollback, and transaction-framed purge) is handled by uninstall/purge
//! paths rather than this logical disable surface.
//!
//! This module mirrors the lock / log / state sequencing of
//! [`crate::enable_execute::execute_enable`] but is much smaller: there is
//! no per-component IO loop because there are no side effects to perform.
//!
//! Sequence on the success path:
//!
//!   1. Best-effort pre-lock check that the capability exists. This is
//!      advisory: a typo short-circuits with `CapabilityNotInstalled`
//!      and **no audit-log noise**, but the authoritative read happens
//!      inside the lock at step 3 so a concurrent disable / uninstall
//!      cannot smuggle the wrong `was_disabled` / `component_refs` past
//!      us.
//!   2. Acquire the install lock (`state_dir/lock`).
//!   3. **Inside the lock**, load `installed.toml` and compute the
//!      authoritative `previous_status`, `was_disabled`, and
//!      `component_refs`. If the capability vanished between step 1 and
//!      step 3, fail with `CapabilityNotInstalled` and **still write
//!      nothing** to the central log — symmetric with the pre-lock
//!      typo path so a concurrent uninstaller does not produce
//!      ghost audit entries.
//!   4. Append the `started` record to the central log.
//!   5. Run strict `pre_disable` hooks before the state flip. A hard
//!      failure records `failed` and leaves state unchanged.
//!   6. Snapshot `installed.toml` bytes; mutate the in-memory state
//!      (`status = Disabled`, `last_operation_id = <op>`) for the
//!      capability + each referenced component; append an
//!      `OperationRecord { status = "ok" }`; `state.save`. Any failure
//!      after the snapshot restores the prior bytes (or removes the file
//!      if there was no prior).
//!   7. Append the `succeeded` record; release the lock.
//!   8. Stop owned service units best-effort, then run best-effort
//!      `post_disable` hooks. Failures after the state flip surface as
//!      warnings rather than reverting the disable.
//!
//! Idempotency: if the capability is already `Disabled` at step 3, we
//! still go through lock + `started` + `succeeded` (so the audit trail
//! records the request was received) but we do NOT mutate state and do
//! NOT append an `OperationRecord`. The outcome carries `changed:
//! false` so callers can suppress downstream side effects. Because
//! `was_disabled` is sampled inside the lock, the second of two
//! back-to-back disable calls always lands here even if the first call
//! finished microseconds earlier — `disable_second_sequential_call_sees_disabled_and_writes_no_operation_record`
//! pins the contract.

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io;
use std::path::PathBuf;

use chrono::{SecondsFormat, Utc};

use anolisa_env::EnvService;
use anolisa_platform::fs_layout::FsLayout;

use crate::central_log::{CentralLog, CentralLogError, LogKind, LogRecord, LogStatus, Severity};
use crate::hooks::{HookPhase, run_phase_hooks};
use crate::lock::{InstallLock, LockError};
use crate::service;
use crate::state::{InstalledState, ObjectKind, ObjectStatus, OperationRecord, StateError};

/// What [`execute_disable`] actually did. The CLI wrapper renders this to
/// the user.
#[derive(Debug, Clone)]
pub struct DisableOutcome {
    /// Stable operation id (`op-YYYYMMDDHHMMSS-<6hex>`).
    pub operation_id: String,
    /// Capability that was disabled.
    pub capability: String,
    /// Wire status the capability held BEFORE this operation. For the
    /// idempotent case this equals `status` (both "disabled").
    pub previous_status: String,
    /// Wire status the capability holds AFTER this operation. Always
    /// `"disabled"`.
    pub status: String,
    /// `true` when this op flipped state from non-disabled to disabled.
    /// `false` for the idempotent "already disabled" path.
    pub changed: bool,
    /// Component object names that were also flipped to `disabled`
    /// (subset of `capability.component_refs` that actually existed in
    /// state). Empty in the idempotent path.
    pub components: Vec<String>,
    /// `installed.toml` location for caller convenience.
    pub state_path: PathBuf,
    /// Central log location for caller convenience.
    pub central_log_path: PathBuf,
    /// Non-fatal warnings raised during the op. Today the only source
    /// is per-unit service-stop failures: disable still flips state
    /// even if `systemctl stop <unit>` fails, and the failure surfaces
    /// here for the CLI to render.
    pub warnings: Vec<String>,
}

/// Failure surface for [`execute_disable`]. Every variant represents a
/// clean abort: on the variants that ran inside the lock body we restore
/// the prior `installed.toml`, append a `failed` central-log record on a
/// best-effort basis, and release the lock before returning.
#[derive(Debug, thiserror::Error)]
pub enum DisableError {
    /// Capability is not present in `installed.toml`. Reported BEFORE
    /// taking the lock or writing any log entry — it is a CLI input
    /// error, not a runtime failure. Routed to `INVALID_ARGUMENT` (exit
    /// 2) by the CLI wrapper.
    #[error("capability '{capability}' is not installed")]
    CapabilityNotInstalled {
        /// Requested capability name.
        capability: String,
    },
    /// Another ANOLISA process owns the install lock; no state or log was
    /// written for this request.
    #[error("install lock at {path} is held by another process")]
    LockHeld {
        /// Lock file path that could not be acquired.
        path: PathBuf,
    },
    /// Non-contention lock failure such as parent directory or file I/O.
    #[error("lock io: {source}")]
    Lock {
        /// Underlying lock error with filesystem context.
        #[source]
        source: LockError,
    },
    /// `installed.toml` could not be loaded or saved after the operation
    /// entered the lock body.
    #[error("state write failed: {source}")]
    State {
        /// Underlying state-file error.
        #[source]
        source: StateError,
    },
    /// Central-log append failed; callers treat this as an operation
    /// failure because the audit record is part of the contract.
    #[error("central log write failed: {source}")]
    Log {
        /// Underlying JSONL log error.
        #[source]
        source: CentralLogError,
    },
    /// A `pre_disable` lifecycle hook failed. Aborts the verb before
    /// state is mutated and before any service stop runs — the
    /// capability stays in its prior status, the failed hook gets its
    /// own component log line, and a `failed` operation record balances
    /// the started log line. CLI surfaces this through the runtime
    /// (execution-failed) bucket.
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

/// Logical-disable orchestration. See module docs for scope and the exact
/// step sequence.
///
/// `actor` is recorded in every audit record (typically `$USER`, falling
/// back to `"cli"`).
///
/// `install_mode` is the CLI's mode label (`"system"` or `"user"`) and
/// is mirrored verbatim into every started / succeeded / failed central
/// log record so disable entries are symmetric with the
/// `install_mode`-tagged records `execute_enable` emits. This lets
/// `logs --install-mode <m>` / external audit pipelines filter both
/// halves of the lifecycle by the same field rather than having to
/// special-case disable.
pub fn execute_disable(
    layout: &FsLayout,
    capability_name: &str,
    actor: &str,
    install_mode: &str,
) -> Result<DisableOutcome, DisableError> {
    let state_path = layout.state_dir.join("installed.toml");

    // Step 1 — best-effort pre-lock typo check. Refuses an unknown
    // capability with `CapabilityNotInstalled` and writes nothing —
    // important so accidental misspellings don't pollute the audit
    // trail. This check is ADVISORY only: the authoritative read
    // happens inside the lock at step 3, which is the only place we
    // trust the `was_disabled` / `component_refs` snapshot from.
    //
    // A state-load error here is deliberately ignored: on a fresh
    // machine `installed.toml` does not exist, and the post-lock load
    // (step 3) will surface a real error if one is warranted.
    if let Ok(preflight) = InstalledState::load(&state_path)
        && preflight
            .find_object(ObjectKind::Capability, capability_name)
            .is_none()
    {
        return Err(DisableError::CapabilityNotInstalled {
            capability: capability_name.to_string(),
        });
    }

    // Step 2 — acquire the install lock. Held → return with no log entry.
    let lock = match InstallLock::acquire(&layout.lock_file) {
        Ok(l) => l,
        Err(LockError::Held { path }) => return Err(DisableError::LockHeld { path }),
        Err(other) => return Err(DisableError::Lock { source: other }),
    };

    // Step 3 — authoritative load INSIDE the lock. This is the only
    // place we treat `was_disabled` / `component_refs` / `previous_status`
    // as trustworthy: any concurrent disable / uninstall that raced us
    // is either already visible here or is blocked behind our lock.
    let mut state = match InstalledState::load(&state_path) {
        Ok(s) => s,
        Err(source) => {
            drop(lock);
            return Err(DisableError::State { source });
        }
    };
    let (previous_status, was_disabled, component_refs) =
        match state.find_object(ObjectKind::Capability, capability_name) {
            Some(cap_obj) => (
                object_status_wire(cap_obj.status).to_string(),
                cap_obj.status == ObjectStatus::Disabled,
                cap_obj.component_refs.clone(),
            ),
            None => {
                // Race: the capability existed at step 1 but is gone
                // now (someone else uninstalled it between our
                // pre-lock check and lock acquisition). Symmetric with
                // the typo path — return with NO log entry so a
                // concurrent uninstaller does not leave ghost audit
                // records on this side.
                drop(lock);
                return Err(DisableError::CapabilityNotInstalled {
                    capability: capability_name.to_string(),
                });
            }
        };

    let started_at_utc = Utc::now();
    let started_at = started_at_utc.to_rfc3339_opts(SecondsFormat::Secs, true);
    let operation_id = build_operation_id(&started_at_utc);

    // Audit `objects[]` mirrors what enable_execute writes: capability
    // first, then the component refs (whether they actually exist as
    // state objects or not — the audit trail records intent).
    let mut objects: Vec<String> = Vec::with_capacity(1 + component_refs.len());
    objects.push(capability_name.to_string());
    for c in &component_refs {
        objects.push(c.clone());
    }

    let central = CentralLog::open(layout.central_log.clone());

    // Step 4 — append the "started" record. If this fails, we have no
    // cleanup to do (no state mutation yet); just drop the lock.
    let started_msg = if was_disabled {
        format!("disable {capability_name} started (already disabled)")
    } else {
        format!("disable {capability_name} started")
    };
    if let Err(source) = central.append(&started_record(
        &operation_id,
        capability_name,
        actor,
        install_mode,
        &started_at,
        objects.clone(),
        &started_msg,
    )) {
        drop(lock);
        return Err(DisableError::Log { source });
    }

    // Step 4.5 — pre_disable hooks. Run BEFORE the state flip and
    // BEFORE service stop so a hook can drain in-flight work or notify
    // dependents while the capability is still nominally "installed".
    // Idempotent path skips hooks: rerunning pre_disable on an
    // already-disabled capability has no useful side effect and would
    // just spam the log.
    let pre_disable = if was_disabled {
        crate::hooks::HookRunResult {
            outcomes: Vec::new(),
            warnings: Vec::new(),
            hard_failure: None,
        }
    } else {
        run_phase_hooks(
            layout,
            &component_refs,
            HookPhase::PreDisable,
            Some(&central),
            &operation_id,
            actor,
            install_mode,
            true,
        )
    };

    if let Some(hf) = pre_disable.hard_failure.as_ref() {
        return cleanup_and_fail(
            DisableError::HookFailed {
                phase: "pre_disable".to_string(),
                component: hf.component.clone(),
                summary: hf.summary(),
                exit_code: hf.exit_code,
            },
            &central,
            &operation_id,
            capability_name,
            actor,
            install_mode,
            &started_at,
            objects.clone(),
            None,
            lock,
        );
    }

    // Collect every owned service unit while the state handle is
    // still authoritative. The actual systemctl stops run AFTER we
    // drop the lock, so we don't keep the install lock held while
    // shelling out (which spawns subprocesses that can otherwise
    // capture the lock fd via fork+exec). The idempotent
    // (already-disabled) path skips the stop step on the assumption
    // that prior disable already stopped them.
    let mut warnings: Vec<String> = pre_disable.warnings;
    let mut stop_units: Vec<(String, String)> = Vec::new();
    if !was_disabled {
        for comp_name in &component_refs {
            if let Some(comp) = state.find_object(ObjectKind::Component, comp_name) {
                for svc in &comp.services {
                    stop_units.push((comp_name.clone(), svc.name.clone()));
                }
            }
        }
    }

    // Idempotent path — already disabled. No state mutation, no
    // operation record. Append the succeeded record and return. The
    // outcome carries `changed: false` so the CLI can render
    // "already disabled" semantics without re-reading state.
    if was_disabled {
        let finished_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
        if let Err(source) = central.append(&succeeded_record(
            &operation_id,
            capability_name,
            actor,
            install_mode,
            &started_at,
            &finished_at,
            objects.clone(),
            &format!("disable {capability_name} succeeded (already disabled — no state change)"),
        )) {
            drop(lock);
            return Err(DisableError::Log { source });
        }
        let outcome = DisableOutcome {
            operation_id,
            capability: capability_name.to_string(),
            previous_status,
            status: "disabled".to_string(),
            changed: false,
            components: Vec::new(),
            state_path,
            central_log_path: layout.central_log.clone(),
            warnings,
        };
        drop(lock);
        // No service stops in the idempotent path: stop_units stays
        // empty when was_disabled, so we just return.
        return Ok(outcome);
    }

    // Active path — flip status to Disabled on the capability and on
    // every component that exists in state.

    // Snapshot prior state bytes for cleanup. `Some(bytes)` is the
    // pre-op file content; `None` means the file did not exist (which
    // shouldn't happen here because Step 3's `find_object` succeeded,
    // but we still encode the case for defense in depth).
    let prior_state_bytes: Option<Vec<u8>> = fs::read(&state_path).ok();

    // `state` from Step 3 is already loaded and mutable inside the
    // lock. Reloading here would re-read the file we just snapshotted
    // and reopen a TOCTOU window — use the existing handle.

    let finished_at_utc = Utc::now();
    let finished_at = finished_at_utc.to_rfc3339_opts(SecondsFormat::Secs, true);

    // Flip the capability object. Step 3 already located this object
    // on the same in-memory `state` handle, so `find_object_mut` here
    // cannot return `None` — any failure would mean InstalledState's
    // index disagrees with itself. The race window for "uninstalled
    // between preflight and lock" was closed by Step 3's
    // authoritative reload inside the lock.
    let cap = state
        .find_object_mut(ObjectKind::Capability, capability_name)
        .expect("capability located in Step 3 must still be in same in-memory state");
    cap.status = ObjectStatus::Disabled;
    cap.last_operation_id = Some(operation_id.clone());

    // Flip each referenced component that actually exists in state.
    // We deliberately do NOT clear `files` / `services` / `component_refs`
    // / `enabled_features`: P1-I is logical-disable, those fields stay
    // intact so a later `enable` (or `restart`) can re-use them and an
    // operator can still see what the disabled object had owned.
    let mut flipped: Vec<String> = Vec::new();
    for comp_name in &component_refs {
        if let Some(comp) = state.find_object_mut(ObjectKind::Component, comp_name) {
            comp.status = ObjectStatus::Disabled;
            comp.last_operation_id = Some(operation_id.clone());
            flipped.push(comp_name.clone());
        }
    }

    state.append_operation(OperationRecord {
        id: operation_id.clone(),
        command: format!("disable {capability_name}"),
        status: "ok".to_string(),
        started_at: started_at.clone(),
        finished_at: Some(finished_at.clone()),
    });

    if let Err(source) = state.save(&state_path) {
        return cleanup_and_fail(
            DisableError::State { source },
            &central,
            &operation_id,
            capability_name,
            actor,
            install_mode,
            &started_at,
            objects.clone(),
            Some((state_path.clone(), prior_state_bytes.clone())),
            lock,
        );
    }

    // Step 5 — succeeded record. Failure here MUST restore the prior
    // state bytes; state.save just succeeded, so the on-disk file
    // already reflects the new "disabled" status that we will not be
    // able to advertise in the audit log.
    if let Err(source) = central.append(&succeeded_record(
        &operation_id,
        capability_name,
        actor,
        install_mode,
        &started_at,
        &finished_at,
        objects.clone(),
        &format!("disable {capability_name} succeeded"),
    )) {
        return cleanup_and_fail(
            DisableError::Log { source },
            &central,
            &operation_id,
            capability_name,
            actor,
            install_mode,
            &started_at,
            objects.clone(),
            Some((state_path.clone(), prior_state_bytes)),
            lock,
        );
    }

    drop(lock);

    // Best-effort stop of every owned service unit AFTER releasing
    // the lock. Stop failures NEVER block disable — they surface as
    // warnings on the outcome. We accept the small race window where
    // the service could read a freshly-flipped `installed.toml`
    // before its `systemctl stop` actually lands; in alpha this is
    // fine, and it lets us avoid keeping the lock held while shelling
    // out to systemctl + fcntl-style probes.
    if !stop_units.is_empty() {
        let env = EnvService::detect();
        let manager = service::for_install_mode(install_mode, &env);
        if manager.supported() {
            for (component, unit) in &stop_units {
                match manager.stop_service(unit) {
                    Ok(_) => {
                        service::record_service_op(
                            Some(&central),
                            service::ServiceOp::Stop,
                            component,
                            unit,
                            &operation_id,
                            actor,
                            install_mode,
                            None,
                        );
                    }
                    Err(err) => {
                        let err_msg = err.to_string();
                        warnings.push(format!(
                            "service stop skipped for {component}/{unit}: {err_msg}",
                        ));
                        service::record_service_op(
                            Some(&central),
                            service::ServiceOp::Stop,
                            component,
                            unit,
                            &operation_id,
                            actor,
                            install_mode,
                            Some(&err_msg),
                        );
                    }
                }
            }
        } else {
            let manager_name = manager.manager().to_string();
            let reason = manager.unsupported_reason().map(str::to_string);
            for (component, unit) in &stop_units {
                service::record_service_op_unsupported(
                    Some(&central),
                    service::ServiceOp::Stop,
                    component,
                    unit,
                    &operation_id,
                    actor,
                    install_mode,
                    &manager_name,
                    reason.as_deref(),
                );
            }
        }
    }

    // post_disable hooks. Run AFTER the state flip + service stop so
    // hooks observe the final disabled shape (status flipped, units
    // stopped). Failures only warn — at this point the central log
    // already records `succeeded`.
    let post_disable = run_phase_hooks(
        layout,
        &component_refs,
        HookPhase::PostDisable,
        Some(&central),
        &operation_id,
        actor,
        install_mode,
        false,
    );
    warnings.extend(post_disable.warnings);

    let outcome = DisableOutcome {
        operation_id,
        capability: capability_name.to_string(),
        previous_status,
        status: "disabled".to_string(),
        changed: true,
        components: flipped,
        state_path,
        central_log_path: layout.central_log.clone(),
        warnings,
    };
    Ok(outcome)
}

/// Cleanup invoked when any post-lock step fails on the active path.
/// Restores the prior `installed.toml` bytes (or removes the file if
/// none existed), appends a best-effort `failed` central-log record,
/// drops the lock, and returns the original error.
#[allow(clippy::too_many_arguments)]
fn cleanup_and_fail(
    err: DisableError,
    central: &CentralLog,
    operation_id: &str,
    capability_name: &str,
    actor: &str,
    install_mode: &str,
    started_at: &str,
    objects: Vec<String>,
    state_restore: Option<(PathBuf, Option<Vec<u8>>)>,
    lock: InstallLock,
) -> Result<DisableOutcome, DisableError> {
    let mut cleanup_warnings = Vec::new();
    if let Some((path, prior)) = state_restore {
        restore_prior_state(&path, prior, &mut cleanup_warnings);
    }
    let finished_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let _ = central.append(&failed_record(
        operation_id,
        capability_name,
        actor,
        install_mode,
        started_at,
        &finished_at,
        objects,
        &err,
        cleanup_warnings,
    ));
    drop(lock);
    Err(err)
}

fn restore_prior_state(path: &PathBuf, prior: Option<Vec<u8>>, warnings: &mut Vec<String>) {
    match prior {
        Some(bytes) => {
            if let Err(source) = fs::write(path, &bytes) {
                warnings.push(format!(
                    "failed to restore prior installed state at {}: {source}",
                    path.display()
                ));
            }
        }
        None => {
            if let Err(source) = fs::remove_file(path)
                && source.kind() != io::ErrorKind::NotFound
            {
                warnings.push(format!(
                    "failed to remove newly-created installed state at {}: {source}",
                    path.display()
                ));
            }
        }
    }
}

fn started_record(
    operation_id: &str,
    capability_name: &str,
    actor: &str,
    install_mode: &str,
    started_at: &str,
    objects: Vec<String>,
    message: &str,
) -> LogRecord {
    LogRecord {
        kind: LogKind::Operation,
        operation_id: Some(operation_id.to_string()),
        command: format!("disable {capability_name}"),
        source: "anolisa-cli".to_string(),
        component: None,
        severity: Severity::Info,
        message: message.to_string(),
        actor: actor.to_string(),
        install_mode: Some(install_mode.to_string()),
        started_at: started_at.to_string(),
        finished_at: None,
        status: None,
        objects,
        backup_ids: Vec::new(),
        warnings: Vec::new(),
        details: serde_json::Value::Null,
    }
}

#[allow(clippy::too_many_arguments)]
fn succeeded_record(
    operation_id: &str,
    capability_name: &str,
    actor: &str,
    install_mode: &str,
    started_at: &str,
    finished_at: &str,
    objects: Vec<String>,
    message: &str,
) -> LogRecord {
    LogRecord {
        kind: LogKind::Operation,
        operation_id: Some(operation_id.to_string()),
        command: format!("disable {capability_name}"),
        source: "anolisa-cli".to_string(),
        component: None,
        severity: Severity::Info,
        message: message.to_string(),
        actor: actor.to_string(),
        install_mode: Some(install_mode.to_string()),
        started_at: started_at.to_string(),
        finished_at: Some(finished_at.to_string()),
        status: Some(LogStatus::Ok),
        objects,
        backup_ids: Vec::new(),
        warnings: Vec::new(),
        details: serde_json::Value::Null,
    }
}

#[allow(clippy::too_many_arguments)]
fn failed_record(
    operation_id: &str,
    capability_name: &str,
    actor: &str,
    install_mode: &str,
    started_at: &str,
    finished_at: &str,
    objects: Vec<String>,
    err: &DisableError,
    warnings: Vec<String>,
) -> LogRecord {
    LogRecord {
        kind: LogKind::Operation,
        operation_id: Some(operation_id.to_string()),
        command: format!("disable {capability_name}"),
        source: "anolisa-cli".to_string(),
        component: None,
        severity: Severity::Error,
        message: format!("disable {capability_name} failed: {err}"),
        actor: actor.to_string(),
        install_mode: Some(install_mode.to_string()),
        started_at: started_at.to_string(),
        finished_at: Some(finished_at.to_string()),
        status: Some(LogStatus::Failed),
        objects,
        backup_ids: Vec::new(),
        warnings,
        details: serde_json::Value::Null,
    }
}

/// Wire label for the status that lives on an [`InstalledObject`]. Kept
/// local rather than imported from the CLI because `anolisa-core` does
/// not depend on `anolisa-cli`. The labels match
/// `anolisa-cli::commands::common::object_status_str` verbatim.
fn object_status_wire(status: ObjectStatus) -> &'static str {
    match status {
        ObjectStatus::Installed => "installed",
        ObjectStatus::Partial => "degraded",
        ObjectStatus::Disabled => "disabled",
        ObjectStatus::Failed => "failed",
        ObjectStatus::Adopted => "adopted",
    }
}

/// `op-YYYYMMDDHHMMSS-<6-hex>` — sortable, unique per call, no new
/// crate deps. Copied verbatim from
/// `enable_execute::build_operation_id` so the two orchestrators emit
/// identical id shapes without disable_execute reaching across modules
/// (and without modifying enable_execute today).
///
/// TODO(owner: lifecycle-core, when: operation-id generation changes):
/// dedupe with `enable_execute::build_operation_id` by lifting both into
/// a shared `operation_id` module.
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

    use crate::state::{
        FileOwner, InstalledObject, InstalledState, ObjectKind, ObjectStatus, OwnedFile, ServiceRef,
    };
    use std::fs as std_fs;
    use std::path::Path;
    use tempfile::tempdir;

    fn fixture_layout(prefix: &Path) -> FsLayout {
        FsLayout::system(Some(prefix.to_path_buf()))
    }

    /// Seed `installed.toml` with a capability + component pair so the
    /// active disable path has something to flip. `cap_status` controls
    /// the initial capability status; the component starts in the same
    /// state for symmetry. `files` are preserved by disable so the
    /// "files retained" assertion has something to read back.
    fn seed_installed_state(
        layout: &FsLayout,
        capability: &str,
        component: &str,
        cap_status: ObjectStatus,
        bin_path: &Path,
    ) {
        std_fs::create_dir_all(&layout.state_dir).expect("mkdir state");
        let mut state = InstalledState::default();
        state.upsert_object(InstalledObject {
            kind: ObjectKind::Component,
            name: component.to_string(),
            version: "0.2.0".to_string(),
            status: cap_status,
            manifest_digest: None,
            distribution_source: Some("file:///fake".to_string()),
            install_backend: Some("raw".to_string()),
            installed_at: "2026-06-01T10:00:00Z".to_string(),
            last_operation_id: Some("op-prior".to_string()),
            managed: true,
            adopted: false,
            subscription_scope: Default::default(),
            enabled_features: Vec::new(),
            component_refs: Vec::new(),
            files: vec![OwnedFile {
                path: bin_path.to_path_buf(),
                owner: FileOwner::Anolisa,
                sha256: Some("0".repeat(64)),
            }],
            external_modified_files: Vec::new(),
            services: vec![ServiceRef {
                name: format!("{component}.service"),
                manager: "systemd".to_string(),
                restartable: true,
                enabled: false,
            }],
            health: Vec::new(),
        });
        state.upsert_object(InstalledObject {
            kind: ObjectKind::Capability,
            name: capability.to_string(),
            version: "stable".to_string(),
            status: cap_status,
            manifest_digest: None,
            distribution_source: None,
            install_backend: None,
            installed_at: "2026-06-01T10:00:00Z".to_string(),
            last_operation_id: Some("op-prior".to_string()),
            managed: true,
            adopted: false,
            subscription_scope: Default::default(),
            enabled_features: Vec::new(),
            component_refs: vec![component.to_string()],
            files: Vec::new(),
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
        });
        state
            .save(&layout.state_dir.join("installed.toml"))
            .expect("seed state save");
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

    /// pre_disable + post_disable scripts discovered under
    /// `<datadir>/hooks/<component>/<phase>.sh` must actually run
    /// during a real disable AND emit one `LogKind::Component` record
    /// per attempt. Pins the wiring contract so a future refactor that
    /// drops the `run_phase_hooks` calls fails immediately rather than
    /// silently skipping lifecycle hooks.
    #[test]
    #[cfg(unix)]
    fn disable_runs_pre_and_post_hooks_and_records_them_in_central_log() {
        let root = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());
        let bin_path = layout.bin_dir.join("agentsight");
        seed_installed_state(
            &layout,
            "agent-observability",
            "agentsight",
            ObjectStatus::Installed,
            &bin_path,
        );

        write_hook_script(&layout, "agentsight", "pre_disable", "#!/bin/sh\nexit 0\n");
        write_hook_script(&layout, "agentsight", "post_disable", "#!/bin/sh\nexit 0\n");

        let outcome = execute_disable(&layout, "agent-observability", "tester", "system")
            .expect("disable ok");

        let lines = read_log_lines(&layout.central_log);
        // Filter on `command starts with "hook:"` so service-op
        // component records (stop, supported or unsupported skip) do
        // not pollute the assertion — they're the responsibility of a
        // separate test.
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
            "expected pre+post disable hook log entries, got: {lines:?}",
        );
        let commands: Vec<&str> = hook_lines
            .iter()
            .map(|l| l.get("command").and_then(|v| v.as_str()).unwrap_or(""))
            .collect();
        assert!(
            commands.contains(&"hook:pre_disable") && commands.contains(&"hook:post_disable"),
            "hook records must name both phases: {commands:?}",
        );
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

    /// Counterpart to the active-path proof: when disable is the
    /// idempotent (already-disabled) no-op, hooks must NOT run. The
    /// rationale is that pre_disable for an already-disabled
    /// capability has no useful side effect — it would just spam the
    /// central log on every retry. The pin guards against an
    /// over-eager refactor that drops the `was_disabled` check inside
    /// the hook block.
    #[test]
    #[cfg(unix)]
    fn disable_idempotent_path_skips_hooks() {
        let root = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());
        let bin_path = layout.bin_dir.join("agentsight");
        seed_installed_state(
            &layout,
            "agent-observability",
            "agentsight",
            ObjectStatus::Disabled,
            &bin_path,
        );

        write_hook_script(&layout, "agentsight", "pre_disable", "#!/bin/sh\nexit 0\n");
        write_hook_script(&layout, "agentsight", "post_disable", "#!/bin/sh\nexit 0\n");

        let outcome = execute_disable(&layout, "agent-observability", "tester", "system")
            .expect("idempotent disable ok");
        assert!(!outcome.changed, "must take idempotent branch");

        let lines = read_log_lines(&layout.central_log);
        let hook_lines: Vec<_> = lines
            .iter()
            .filter(|l| l.get("kind").and_then(|v| v.as_str()) == Some("component"))
            .collect();
        assert!(
            hook_lines.is_empty(),
            "idempotent disable must not run lifecycle hooks: {hook_lines:?}",
        );
    }

    #[test]
    fn restore_prior_state_records_warning_when_restore_fails() {
        let root = tempdir().expect("tempdir");
        let missing_parent = root.path().join("missing").join("installed.toml");
        let mut warnings = Vec::new();

        restore_prior_state(&missing_parent, Some(b"prior".to_vec()), &mut warnings);

        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("failed to restore prior installed state"));
        assert!(warnings[0].contains("installed.toml"));
    }

    #[test]
    fn disable_success_marks_capability_and_components_disabled_and_writes_logs() {
        let root = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());
        let bin_path = layout.bin_dir.join("agentsight");
        seed_installed_state(
            &layout,
            "agent-observability",
            "agentsight",
            ObjectStatus::Installed,
            &bin_path,
        );

        let outcome = execute_disable(&layout, "agent-observability", "tester", "system")
            .expect("execute ok");
        assert_eq!(outcome.capability, "agent-observability");
        assert_eq!(outcome.previous_status, "installed");
        assert_eq!(outcome.status, "disabled");
        assert!(outcome.changed, "successful disable must report changed");
        assert_eq!(outcome.components, vec!["agentsight".to_string()]);
        assert!(outcome.operation_id.starts_with("op-"));

        // State: capability + component flipped to Disabled, files
        // retained, last_operation_id updated, OperationRecord appended.
        let state_path = layout.state_dir.join("installed.toml");
        let state = InstalledState::load(&state_path).expect("load state");
        let cap = state
            .find_object(ObjectKind::Capability, "agent-observability")
            .expect("capability present");
        assert_eq!(cap.status, ObjectStatus::Disabled);
        assert_eq!(
            cap.last_operation_id.as_deref(),
            Some(outcome.operation_id.as_str())
        );
        assert_eq!(cap.component_refs, vec!["agentsight".to_string()]);

        let comp = state
            .find_object(ObjectKind::Component, "agentsight")
            .expect("component present");
        assert_eq!(comp.status, ObjectStatus::Disabled);
        assert_eq!(
            comp.last_operation_id.as_deref(),
            Some(outcome.operation_id.as_str())
        );
        // P1-I keeps files / services intact — only the lifecycle status
        // moves. The CLI / lifecycle stages own actual teardown.
        assert_eq!(comp.files.len(), 1, "files must be retained");
        assert_eq!(comp.files[0].path, bin_path);
        assert_eq!(comp.services.len(), 1, "services must be retained");

        // Operation record was appended.
        assert!(
            state
                .operations
                .iter()
                .any(|op| op.id == outcome.operation_id && op.status == "ok"),
            "OperationRecord with op id must be present and ok"
        );

        // Central log: exactly two operation lines (started + succeeded),
        // same op id. Component-kind records (service:stop, hooks) are
        // covered by their own dedicated tests; filter them out here so
        // adding/removing those records doesn't break the started/
        // succeeded contract this test pins.
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
            assert_eq!(
                line.get("command").and_then(|v| v.as_str()),
                Some("disable agent-observability"),
            );
            // install_mode must be stamped on both records so audit
            // pipelines can filter disable + enable rows uniformly by
            // system / user; missing values were the gap caught in P1-I
            // review.
            assert_eq!(
                line.get("install_mode").and_then(|v| v.as_str()),
                Some("system"),
                "every disable log line must carry install_mode",
            );
        }
        assert!(lines[0].get("status").map(|v| v.is_null()).unwrap_or(true));
        assert_eq!(lines[1].get("status").and_then(|v| v.as_str()), Some("ok"));
    }

    #[test]
    fn disable_idempotent_when_already_disabled_returns_changed_false() {
        let root = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());
        let bin_path = layout.bin_dir.join("agentsight");
        seed_installed_state(
            &layout,
            "agent-observability",
            "agentsight",
            ObjectStatus::Disabled,
            &bin_path,
        );

        // Snapshot prior state bytes so we can confirm the idempotent
        // path did NOT mutate `installed.toml`.
        let state_path = layout.state_dir.join("installed.toml");
        let prior_bytes = std_fs::read(&state_path).expect("read prior");

        let outcome = execute_disable(&layout, "agent-observability", "tester", "system")
            .expect("execute ok (idempotent)");
        assert_eq!(outcome.previous_status, "disabled");
        assert_eq!(outcome.status, "disabled");
        assert!(
            !outcome.changed,
            "idempotent path must report changed=false"
        );
        assert!(
            outcome.components.is_empty(),
            "idempotent path reports no components flipped"
        );

        // State bytes unchanged: no OperationRecord appended, no
        // last_operation_id mutation, files retained.
        let after_bytes = std_fs::read(&state_path).expect("read after");
        assert_eq!(
            after_bytes, prior_bytes,
            "idempotent disable must not mutate installed.toml",
        );
        // Belt-and-suspenders: parse still works.
        let state = InstalledState::load(&state_path).expect("state still loads");
        let comp = state
            .find_object(ObjectKind::Component, "agentsight")
            .expect("component present");
        assert_eq!(comp.files.len(), 1, "files retained");

        // Central log has started + succeeded.
        let lines = read_log_lines(&layout.central_log);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].get("status").and_then(|v| v.as_str()), Some("ok"));
        // Succeeded message must mention "already disabled" so operators
        // can grep audit history for the no-op case.
        let succ_msg = lines[1]
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert!(
            succ_msg.contains("already disabled"),
            "succeeded message should advertise the no-op: {succ_msg}",
        );
    }

    #[test]
    fn disable_capability_not_installed_returns_error_and_writes_nothing() {
        let root = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());
        // Deliberately leave the layout empty — no state file, no log.

        let err = execute_disable(&layout, "agent-observability", "tester", "system")
            .expect_err("must error");
        match err {
            DisableError::CapabilityNotInstalled { ref capability } => {
                assert_eq!(capability, "agent-observability");
            }
            other => panic!("expected CapabilityNotInstalled, got {other:?}"),
        }

        assert!(
            !layout.state_dir.join("installed.toml").exists(),
            "no installed.toml may be created when capability is missing",
        );
        assert!(
            !layout.central_log.exists(),
            "no central log may be written when capability is missing",
        );
    }

    /// Two back-to-back disable calls on the same capability. The
    /// second call MUST observe `Disabled` (sampled inside the lock at
    /// Step 3) and take the idempotent branch — no second
    /// `OperationRecord`, `changed=false`, succeeded log mentions
    /// "already disabled". Pins the TOCTOU fix where the
    /// authoritative `was_disabled` snapshot moved inside the install
    /// lock so a finished first call cannot be mis-read as
    /// "still installed" by the second.
    #[test]
    fn disable_second_sequential_call_sees_disabled_and_writes_no_operation_record() {
        let root = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());
        let bin_path = layout.bin_dir.join("agentsight");
        seed_installed_state(
            &layout,
            "agent-observability",
            "agentsight",
            ObjectStatus::Installed,
            &bin_path,
        );

        // First call — active path.
        let first = execute_disable(&layout, "agent-observability", "tester", "system")
            .expect("first disable ok");
        assert!(first.changed, "first call must flip state");
        assert_eq!(first.previous_status, "installed");

        // Second call — must hit the idempotent branch.
        let second = execute_disable(&layout, "agent-observability", "tester", "system")
            .expect("second disable ok");
        assert!(
            !second.changed,
            "second call must observe Disabled and report changed=false",
        );
        assert_eq!(second.previous_status, "disabled");
        assert_eq!(second.status, "disabled");
        assert!(
            second.components.is_empty(),
            "idempotent path reports no components flipped",
        );
        assert_ne!(
            first.operation_id, second.operation_id,
            "each call must mint its own operation_id even when idempotent",
        );

        // Exactly one OperationRecord (from the first call) — the
        // second call must not append a second record. This is the
        // strict guarantee that protects against TOCTOU: even if a
        // pre-lock read raced and saw "installed", the post-lock
        // sample must catch up and suppress the mutation.
        let state_path = layout.state_dir.join("installed.toml");
        let state = InstalledState::load(&state_path).expect("load state");
        let our_ops: Vec<_> = state
            .operations
            .iter()
            .filter(|op| op.id == first.operation_id || op.id == second.operation_id)
            .collect();
        assert_eq!(
            our_ops.len(),
            1,
            "exactly one OperationRecord must exist for the two calls",
        );
        assert_eq!(our_ops[0].id, first.operation_id);

        // Central log has 4 operation records: started+succeeded for
        // the first call, started+succeeded for the idempotent second
        // call. Component-kind records (service:stop / hooks) are
        // intentionally excluded — this test pins the operation pair
        // shape, not the auxiliary auditing.
        let all_lines = read_log_lines(&layout.central_log);
        let lines: Vec<&serde_json::Value> = all_lines
            .iter()
            .filter(|l| l.get("kind").and_then(|v| v.as_str()) == Some("operation"))
            .collect();
        assert_eq!(
            lines.len(),
            4,
            "expected started+succeeded for both calls (got {})",
            lines.len(),
        );
        let last_msg = lines[3]
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert!(
            last_msg.contains("already disabled"),
            "second-call succeeded message must advertise the no-op: {last_msg}",
        );
    }

    #[test]
    fn disable_lock_contention_returns_lock_held() {
        let root = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());
        let bin_path = layout.bin_dir.join("agentsight");
        seed_installed_state(
            &layout,
            "agent-observability",
            "agentsight",
            ObjectStatus::Installed,
            &bin_path,
        );

        let _held = InstallLock::acquire(&layout.lock_file).expect("hold lock");

        let err = execute_disable(&layout, "agent-observability", "tester", "system")
            .expect_err("must error");
        match err {
            DisableError::LockHeld { path } => assert_eq!(path, layout.lock_file),
            other => panic!("expected LockHeld, got {other:?}"),
        }
        // No log because we never made it past lock acquisition.
        assert!(!layout.central_log.exists());
    }
}
