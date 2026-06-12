//! Lifecycle plan + executor for `uninstall` / `purge` of components.
//!
//! Both teardown verbs share a single data model — [`LifecyclePlan`] —
//! built from the questions every destructive verb must answer before
//! touching the system:
//!
//!   1. What files / services does this component own?
//!   2. Which of those files are ANOLISA-owned (safe to remove) vs.
//!      external (must be preserved)?
//!   3. What service-stop / hook phases would run, and which ones are
//!      shipped today vs. deferred?
//!   4. What is the blast radius — privilege, risk level, irreversible
//!      operations — and what rollback advice can we give if the user
//!      cancels mid-flight?
//!
//! The plan is *data-only*: callers can render it for `--dry-run` /
//! `--json` without performing any IO. Only the executor (when invoked
//! without `--dry-run`) actually mutates the filesystem and state.
//!
//! # Scope guarantees (hard rules)
//!
//! * `Uninstall` — removes only files where `owner ==
//!   FileOwner::Anolisa`; everything else is skipped or refused.
//! * `Purge` — `Uninstall` semantics + drops ANOLISA-owned config / cache
//!   fragments. `external_modified_files` always
//!   [`FileActionKind::Refuse`]. `--force` is wire-level only for now;
//!   the executor treats it as deferred follow-up work.
//!
//! No AgentSight-specific code lives here — the plan is shaped from
//! [`InstalledState`] alone, which is what `install_runner` already
//! writes.
//!
//! # Transaction integration
//!
//! `Uninstall` opens a [`crate::transaction::Transaction`] **inside** the
//! install lock, after the authoritative state load. `Transaction::begin`
//! mints the operation id, snapshots `installed.toml`, and writes an
//! empty journal under `state_dir/journal/<operation_id>.journal.toml`.
//! Each removable file is:
//!
//!   1. backed up to `state_dir/backups/<operation_id>/<idx>.bak`,
//!   2. recorded as a `Planned` step whose
//!      [`RollbackActionKind::RestoreFile`](crate::transaction::RollbackActionKind::RestoreFile)
//!      points at the backup (with sha256),
//!   3. unlinked, then
//!   4. flipped to `Done` on success.
//!
//! On any post-deletion failure (`state.save`, the `succeeded` log entry,
//! a `Transaction` error itself) the executor walks done steps in reverse
//! calling `tx.restore_file`, then `tx.restore_state` to put back the
//! pre-op `installed.toml` bytes, marks the failing step `Failed`, and
//! `tx.finish(RolledBack)`. Transaction errors propagate to the caller as
//! [`LifecycleError::Transaction`] — the executor does not swallow them.
//!
//! `Purge` keeps the legacy plan-only gate (`check_destructive_execute_gate`)
//! until manifest-driven config discovery lands; until then the verb still emits a structured plan via
//! `--dry-run` and refuses to execute.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};

use anolisa_env::EnvService;
use anolisa_platform::fs_layout::FsLayout;

use crate::central_log::{CentralLog, CentralLogError, LogKind, LogRecord, LogStatus, Severity};
use crate::hooks::{HookPhase, run_phase_hooks};
use crate::lock::{InstallLock, LockError};
use crate::service;
use crate::state::{
    ExternalModifiedFile, FileOwner as StateFileOwner, InstalledObject, InstalledState, ObjectKind,
    ObjectStatus, OperationRecord, OwnedFile, ServiceRef, StateError,
};
use crate::transaction::{
    RollbackAction, Transaction, TransactionError, TransactionOutcomeStatus, TransactionStep,
    TransactionStepStatus,
};

// ---------------------------------------------------------------------------
// Plan data model
// ---------------------------------------------------------------------------

/// Which teardown verb produced this plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleOperation {
    /// Remove ANOLISA-owned files for the component.
    Uninstall,
    /// Uninstall + drop ANOLISA-owned config / cache / state fragments.
    Purge,
}

impl LifecycleOperation {
    /// Wire label for the verb, used in audit-log records and JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Uninstall => "uninstall",
            Self::Purge => "purge",
        }
    }
}

/// Coarse blast-radius bucket. Used by CLI surfaces to gate confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    /// Logical or read-only change with no file removal.
    Low,
    /// Removes ANOLISA-owned files with transaction rollback support.
    Medium,
    /// Destructive cleanup with incomplete rollback coverage.
    High,
}

/// What a single planned phase will actually do at execute time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleMode {
    /// Will run for real on execute.
    Execute,
    /// Intentionally skipped (e.g. nothing to do, or scope-gated off).
    Skip,
    /// Recognized but not shipped yet — the plan records the intent so
    /// audit / preview is honest, but execute does not perform it.
    NotImplemented,
}

/// Whether a file is ANOLISA-owned (safe to remove) or external.
///
/// Mirrors [`crate::state::FileOwner`] but adds an `Unknown` variant for
/// plan-time files that the state file did not annotate (e.g. a future
/// manifest-only path that has not yet been recorded as installed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileOwner {
    /// Path is owned by ANOLISA and can be removed by lifecycle verbs.
    Anolisa,
    /// Path belongs to the user or another package and must be preserved.
    External,
    /// Ownership was not recorded; destructive verbs treat this
    /// conservatively.
    Unknown,
}

impl From<StateFileOwner> for FileOwner {
    fn from(value: StateFileOwner) -> Self {
        match value {
            StateFileOwner::Anolisa => Self::Anolisa,
            StateFileOwner::External => Self::External,
        }
    }
}

/// What the executor is allowed to do with a single file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileActionKind {
    /// Leave the file on disk (the default for non-ANOLISA files in
    /// `Uninstall` / `Purge`).
    Keep,
    /// Delete the file. Only valid when `owner ==
    /// FileOwner::Anolisa`.
    Remove,
    /// Move the file aside under the backup tree. Reserved for future
    /// use (e.g. on-error rollback recovery); the alpha executor never
    /// emits this variant.
    Backup,
    /// External modification that cannot be safely removed — the plan
    /// MUST surface it so operators understand the residue.
    Refuse,
}

/// One file slot in the plan, tying a path to its ownership + intended
/// action.
#[derive(Debug, Clone, Serialize)]
pub struct FileAction {
    /// Absolute path the action applies to.
    pub path: PathBuf,
    /// Ownership classification used to decide whether deletion is safe.
    pub owner: FileOwner,
    /// Planned executor behavior for this path.
    pub action: FileActionKind,
    /// Human-facing explanation for skipped or refused actions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Service-unit action the plan would take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceActionKind {
    /// `systemctl stop`. Not shipped in alpha.
    Stop,
    /// `systemctl disable`. Not shipped in alpha.
    Disable,
    /// Recorded but explicitly skipped (e.g. unit never installed).
    Skip,
    /// Recognized but not shipped yet (current alpha for stop/disable).
    NotImplemented,
}

/// Service-unit action surfaced in a lifecycle plan.
#[derive(Debug, Clone, Serialize)]
pub struct ServiceAction {
    /// Unit name as recorded in installed state.
    pub name: String,
    /// Planned behavior for the unit.
    pub action: ServiceActionKind,
    /// Explanation when a service action is skipped or deferred.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Hook (pre/post-uninstall, etc.) recorded in the plan.
#[derive(Debug, Clone, Serialize)]
pub struct HookAction {
    /// Hook phase name shown in the plan.
    pub name: String,
    /// Whether this hook would run, skip, or remain deferred.
    pub mode: LifecycleMode,
    /// Explanation when the hook does not execute.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Per-component slice of the plan.
#[derive(Debug, Clone, Serialize)]
pub struct ComponentLifecyclePlan {
    /// Component this plan slice describes.
    pub name: String,
    /// Service work associated with the component.
    pub services: Vec<ServiceAction>,
    /// Installed file actions for uninstall.
    pub files: Vec<FileAction>,
    /// Configuration / state fragments owned by ANOLISA (e.g. dropins
    /// the component wrote into `etc_dir`). Only populated for `Purge`.
    pub configs: Vec<FileAction>,
    /// Hook phases that would surround the component lifecycle.
    pub hooks: Vec<HookAction>,
}

/// A single ordered phase of the plan, used by the renderer to show
/// the user what will happen and in what order.
#[derive(Debug, Clone, Serialize)]
pub struct LifecyclePhase {
    /// Stable phase identifier (e.g. `"stop_services"`, `"remove_files"`).
    pub name: String,
    /// Human-readable verb (`"stop"`, `"remove"`, `"run_hook"`, ...).
    pub action: String,
    /// What the phase is acting on (component name, file path, etc.).
    pub target: String,
    /// Whether the executor will run, skip, or defer the phase.
    pub mode: LifecycleMode,
    /// Operator guidance for recovery if this phase fails mid-flight.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rollback_hint: Option<String>,
}

/// Installed-state object vocabulary targeted by a lifecycle plan.
///
/// Components are the only installable object today; the enum stays on
/// the wire as an extension point for future target kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleTargetKind {
    /// Component target used by `anolisa install` / `uninstall`.
    Component,
}

/// The full lifecycle plan for one installed object invocation.
#[derive(Debug, Clone, Serialize)]
pub struct LifecyclePlan {
    /// Lifecycle verb requested by the user.
    pub operation: LifecycleOperation,
    /// Installed-state object kind this plan targets.
    pub target_kind: LifecycleTargetKind,
    /// Component name the plan targets.
    pub component: String,
    /// Per-component plan slices.
    pub components: Vec<ComponentLifecyclePlan>,
    /// Ordered phases shown by dry-run renderers.
    pub phases: Vec<LifecyclePhase>,
    /// Confirmation bucket for the overall plan.
    pub risk: RiskLevel,
    /// `true` when executing the plan needs elevated privileges.
    pub requires_privilege: bool,
    /// Non-fatal planning warnings for the user.
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------------------
// Planner constructors
// ---------------------------------------------------------------------------

impl LifecyclePlan {
    /// Build an `Uninstall` plan for a component installed through
    /// `anolisa install`: every `OwnedFile` whose owner is ANOLISA
    /// becomes [`FileActionKind::Remove`]; external residue is surfaced
    /// as [`FileActionKind::Refuse`].
    pub fn for_component_uninstall(component: &str, installed_state: &InstalledState) -> Self {
        Self::build(
            LifecycleOperation::Uninstall,
            LifecycleTargetKind::Component,
            component,
            installed_state,
        )
    }

    /// Build a `Purge` plan: `Uninstall` + remove ANOLISA-owned
    /// `etc_dir` / `cache_dir` / `state_dir` fragments. External
    /// modifications stay [`FileActionKind::Refuse`]. Execution remains
    /// gated by the purge guard.
    pub fn for_component_purge(component: &str, installed_state: &InstalledState) -> Self {
        Self::build(
            LifecycleOperation::Purge,
            LifecycleTargetKind::Component,
            component,
            installed_state,
        )
    }

    fn build(
        operation: LifecycleOperation,
        target_kind: LifecycleTargetKind,
        target: &str,
        installed_state: &InstalledState,
    ) -> Self {
        let target_obj = installed_state.find_object(ObjectKind::Component, target);

        let mut components: Vec<ComponentLifecyclePlan> = Vec::new();
        let mut warnings: Vec<String> = Vec::new();

        if let Some(obj) = target_obj {
            let mut files: Vec<FileAction> = plan_owned_files(&obj.files);
            files.extend(plan_external_files(&obj.external_modified_files));
            let configs = if operation == LifecycleOperation::Purge {
                plan_purge_configs(&obj.files)
            } else {
                Vec::new()
            };
            components.push(ComponentLifecyclePlan {
                name: target.to_string(),
                services: plan_services(&obj.services),
                files,
                configs,
                // Hook execution is deferred to lifecycle teardown; record
                // the intent so audit / preview is honest.
                hooks: default_hooks_for(operation),
            });
        } else {
            warnings.push(format!(
                "component '{target}' is not installed — plan is empty"
            ));
        }

        let phases = build_phases(operation, target, &components);

        let requires_privilege = components
            .iter()
            .any(|c| c.files.iter().any(|f| f.action == FileActionKind::Remove));

        let risk = match operation {
            LifecycleOperation::Uninstall => RiskLevel::Medium,
            LifecycleOperation::Purge => RiskLevel::High,
        };

        Self {
            operation,
            target_kind,
            component: target.to_string(),
            components,
            phases,
            risk,
            requires_privilege,
            warnings,
        }
    }
}

fn plan_owned_files(files: &[OwnedFile]) -> Vec<FileAction> {
    files
        .iter()
        .map(|f| {
            let owner: FileOwner = f.owner.into();
            let (action, reason) = match owner {
                FileOwner::Anolisa => (FileActionKind::Remove, None),
                FileOwner::External => (
                    FileActionKind::Refuse,
                    Some("file marked external in state".to_string()),
                ),
                FileOwner::Unknown => (
                    FileActionKind::Keep,
                    Some("owner unknown — refusing to delete".to_string()),
                ),
            };
            FileAction {
                path: f.path.clone(),
                owner,
                action,
                reason,
            }
        })
        .collect()
}

fn plan_external_files(files: &[ExternalModifiedFile]) -> Vec<FileAction> {
    files
        .iter()
        .map(|f| FileAction {
            path: f.path.clone(),
            owner: FileOwner::External,
            // Uninstall / Purge refuse external modifications — the user
            // (or a future restore command) owns the cleanup decision.
            action: FileActionKind::Refuse,
            reason: Some("external modification recorded in state".to_string()),
        })
        .collect()
}

fn plan_services(services: &[ServiceRef]) -> Vec<ServiceAction> {
    services
        .iter()
        .map(|s| ServiceAction {
            name: s.name.clone(),
            action: ServiceActionKind::Stop,
            reason: Some(
                "stops via systemd in system mode; skipped on user/non-linux/container hosts"
                    .to_string(),
            ),
        })
        .collect()
}

/// Configuration fragments to drop on `Purge`. Today we only purge the
/// ANOLISA-owned files that already live under a state/etc/cache root —
/// the manifest schema work for separate config drop-ins is deferred,
/// so we surface the existing files via the `Remove` action and rely on
/// the executor to enforce ownership.
fn plan_purge_configs(files: &[OwnedFile]) -> Vec<FileAction> {
    files
        .iter()
        .filter(|f| f.owner == StateFileOwner::Anolisa)
        .filter(|f| is_config_or_state_path(&f.path))
        .map(|f| FileAction {
            path: f.path.clone(),
            owner: FileOwner::Anolisa,
            action: FileActionKind::Remove,
            reason: Some("ANOLISA-owned config/state fragment".to_string()),
        })
        .collect()
}

fn is_config_or_state_path(p: &Path) -> bool {
    let s = p.to_string_lossy();
    // Conservative match — only the ANOLISA-owned roots that
    // `install_runner` writes into qualify.
    s.contains("/etc/anolisa")
        || s.contains("/var/lib/anolisa")
        || s.contains("/var/cache/anolisa")
        || s.contains("/.config/anolisa")
        || s.contains("/.local/state/anolisa")
        || s.contains("/.cache/anolisa")
}

fn default_hooks_for(operation: LifecycleOperation) -> Vec<HookAction> {
    let names: &[&str] = match operation {
        LifecycleOperation::Uninstall => &["pre_uninstall", "post_uninstall"],
        LifecycleOperation::Purge => &["pre_uninstall", "post_uninstall", "post_purge"],
    };
    names
        .iter()
        .map(|n| HookAction {
            name: (*n).to_string(),
            mode: LifecycleMode::NotImplemented,
            reason: Some("hook execution not shipped in alpha".to_string()),
        })
        .collect()
}

fn build_phases(
    operation: LifecycleOperation,
    component: &str,
    components: &[ComponentLifecyclePlan],
) -> Vec<LifecyclePhase> {
    let mut phases: Vec<LifecyclePhase> = Vec::new();

    // Hook phases (intent only).
    for c in components {
        for h in &c.hooks {
            phases.push(LifecyclePhase {
                name: format!("hook_{}", h.name),
                action: "run_hook".to_string(),
                target: format!("{}:{}", c.name, h.name),
                mode: h.mode,
                rollback_hint: None,
            });
        }
    }

    // Service stop / disable phases (NotImplemented in alpha).
    for c in components {
        for s in &c.services {
            phases.push(LifecyclePhase {
                name: "stop_service".to_string(),
                action: match s.action {
                    ServiceActionKind::Stop => "stop",
                    ServiceActionKind::Disable => "disable",
                    ServiceActionKind::Skip => "skip",
                    ServiceActionKind::NotImplemented => "stop",
                }
                .to_string(),
                target: s.name.clone(),
                mode: match s.action {
                    ServiceActionKind::Skip => LifecycleMode::Skip,
                    ServiceActionKind::NotImplemented => LifecycleMode::NotImplemented,
                    _ => LifecycleMode::Execute,
                },
                rollback_hint: None,
            });
        }
    }

    // File phases.
    for c in components {
        for f in &c.files {
            phases.push(LifecyclePhase {
                name: "remove_file".to_string(),
                action: match f.action {
                    FileActionKind::Remove => "remove",
                    FileActionKind::Keep => "keep",
                    FileActionKind::Backup => "backup",
                    FileActionKind::Refuse => "refuse",
                }
                .to_string(),
                target: f.path.display().to_string(),
                mode: match f.action {
                    FileActionKind::Remove => LifecycleMode::Execute,
                    _ => LifecycleMode::Skip,
                },
                rollback_hint: match f.action {
                    FileActionKind::Remove => {
                        Some("anolisa install <component> (reinstall)".to_string())
                    }
                    _ => None,
                },
            });
        }
        if operation == LifecycleOperation::Purge {
            for f in &c.configs {
                phases.push(LifecyclePhase {
                    name: "remove_config".to_string(),
                    action: "remove".to_string(),
                    target: f.path.display().to_string(),
                    mode: LifecycleMode::Execute,
                    rollback_hint: None,
                });
            }
        }
    }
    phases.push(LifecyclePhase {
        name: "remove_state".to_string(),
        action: "remove_object".to_string(),
        target: component.to_string(),
        mode: LifecycleMode::Execute,
        rollback_hint: Some("anolisa install <component> (reinstall)".to_string()),
    });

    phases
}

// ---------------------------------------------------------------------------
// Journal (transaction soft dependency)
// ---------------------------------------------------------------------------
//
// Earlier revisions defined a `LifecycleJournal` trait + `NoopJournal` /
// `TransactionJournal` shims so the D-worktree could land in any order
// with this module. With `crate::transaction::Transaction` now stable
// the executor calls it directly instead — see [`execute_uninstall_or_purge`].
// The trait/impls were removed once the wiring landed; tests inspect
// transaction behaviour by reading the journal file from `journal_dir`.

// ---------------------------------------------------------------------------
// Executor
// ---------------------------------------------------------------------------

/// Outcome of executing a [`LifecyclePlan`].
#[derive(Debug, Clone)]
pub struct LifecycleOutcome {
    /// Stable operation id recorded in state, central log, and journal.
    pub operation_id: String,
    /// Lifecycle verb that was executed.
    pub operation: LifecycleOperation,
    /// Component name the operation targeted.
    pub component: String,
    /// Paths actually removed by this op (only populated for
    /// `Uninstall` / `Purge`).
    pub removed_files: Vec<PathBuf>,
    /// Files the plan flagged as `Refuse` (external) or `Keep` — i.e.
    /// not deleted, surfaced so the CLI can render an honest summary.
    pub skipped_files: Vec<PathBuf>,
    /// Whether the target object was removed from `installed.toml`.
    pub state_object_removed: bool,
    /// Non-fatal warnings raised AFTER the destructive ops succeeded.
    ///
    /// The canonical case is "journal finalize failed but state.save +
    /// succeeded log already landed": the system is uninstalled, the
    /// audit log shows it, and the only damage is a journal that did
    /// not record its terminal status. Returning `Err` there would flip
    /// a successful uninstall into `EXECUTION_FAILED` for automation —
    /// instead we surface the failure here and the CLI logs it as a
    /// warning while exiting `0`.
    pub warnings: Vec<String>,
    /// `installed.toml` path affected by this operation.
    pub state_path: PathBuf,
    /// Central log path that received operation audit records.
    pub central_log_path: PathBuf,
}

/// Failure surface for [`execute_plan`].
#[derive(Debug, thiserror::Error)]
pub enum LifecycleError {
    /// Component is absent from `installed.toml`; uninstall cannot infer
    /// files or services to remove.
    #[error("component '{component}' is not installed")]
    ComponentNotInstalled {
        /// Requested component name.
        component: String,
    },
    /// Executor does not implement this lifecycle verb.
    #[error("operation '{op}' is not supported by this executor")]
    UnsupportedOperation {
        /// Operation label refused by this executor.
        op: &'static str,
    },
    /// Another ANOLISA process owns the install lock; no destructive work
    /// started for this request.
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
    /// `installed.toml` could not be loaded, saved, or restored.
    #[error("state write failed: {source}")]
    State {
        /// Underlying state-file error.
        #[source]
        source: StateError,
    },
    /// Central-log append failed; the audit trail is part of the
    /// lifecycle contract.
    #[error("central log write failed: {source}")]
    Log {
        /// Underlying JSONL log error.
        #[source]
        source: CentralLogError,
    },
    /// Filesystem mutation failed while deleting or restoring a path.
    #[error("filesystem io failed for {path}: {source}")]
    Filesystem {
        /// Path involved in the failed filesystem operation.
        path: PathBuf,
        /// Original I/O error from the OS.
        #[source]
        source: std::io::Error,
    },
    /// Transaction journal or rollback operation failed.
    #[error("transaction failed: {source}")]
    Transaction {
        /// Underlying transaction error.
        #[source]
        source: TransactionError,
    },
    /// `Purge` is still plan-only until manifest-driven config /
    /// cache / state discovery ships. `Uninstall` is no longer gated —
    /// it goes through the transaction-backed executor below.
    #[error("{reason}")]
    ExecuteGated {
        /// Human-readable gate reason rendered by the CLI.
        reason: String,
    },
    /// A `pre_uninstall` lifecycle hook failed. Aborts the verb before
    /// any file delete runs — the transaction is rolled back, the
    /// component object stays on disk, and a `failed` operation record
    /// balances the started log line. CLI surfaces this through the
    /// runtime (execution-failed) bucket.
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

/// Execute a plan (`Uninstall` or `Purge`).
///
/// `actor` is recorded in every audit record; `install_mode` is mirrored
/// into the central-log records so audit pipelines can filter by mode.
///
/// The choice of "remove the component object vs. mark it removed":
/// the alpha state schema has no `Removed` `ObjectStatus`, so the
/// executor REMOVES the component object via
/// `InstalledState::remove_object` for `Uninstall` / `Purge` — the
/// smallest delta from the existing schema.
pub fn execute_plan(
    plan: &LifecyclePlan,
    layout: &FsLayout,
    actor: &str,
    install_mode: &str,
) -> Result<LifecycleOutcome, LifecycleError> {
    execute_uninstall_or_purge(plan, layout, actor, install_mode)
}

/// `Purge` is still plan-only. The verb declares "remove ANOLISA-owned
/// config / cache / state fragments on top of uninstall", and we don't
/// yet have manifest-driven discovery for those fragments — the planner
/// can only see what `installed.toml` already records as `OwnedFile`
/// entries, which the uninstall path already removes. Shipping execute
/// today would therefore offer no extra value over `uninstall` while
/// adding a strictly more dangerous wire surface.
///
/// `Uninstall` is NOT gated — it goes through the transaction-backed
/// executor below.
///
/// Dry-run is always allowed; it remains the supported way to preview
/// what `purge` would touch once the gate lifts.
///
/// **Lift conditions.** Remove the gate once the manifest schema gains
/// dedicated `[purge.config]` / `[purge.cache]` / `[purge.state]` blocks
/// and the planner consults them; the executor itself already has all
/// the transaction + rollback primitives it needs (it reuses the same
/// path as uninstall, just with the additional `configs` action list).
fn check_destructive_execute_gate(plan: &LifecyclePlan) -> Result<(), LifecycleError> {
    if plan.operation != LifecycleOperation::Purge {
        return Ok(());
    }
    Err(LifecycleError::ExecuteGated {
        reason: "purge execute is gated pending manifest-driven config/cache/state \
                 discovery; run with --dry-run to preview the plan, or use \
                 `anolisa uninstall <component>` for the file-removal subset"
            .to_string(),
    })
}

fn execute_uninstall_or_purge(
    plan: &LifecyclePlan,
    layout: &FsLayout,
    actor: &str,
    install_mode: &str,
) -> Result<LifecycleOutcome, LifecycleError> {
    let state_path = layout.state_dir.join("installed.toml");
    let target_name = plan.component.as_str();

    // Step 1 — best-effort pre-lock typo check. The preflight is
    // read-only; an unreadable state file counts as "not installed".
    let preflight_present = InstalledState::load(&state_path)
        .map(|s| s.find_object(ObjectKind::Component, target_name).is_some())
        .unwrap_or(false);
    if !preflight_present {
        return Err(LifecycleError::ComponentNotInstalled {
            component: target_name.to_string(),
        });
    }

    // Step 1.5 — Purge stays gated pending manifest-driven discovery.
    // Uninstall falls through.
    check_destructive_execute_gate(plan)?;

    // Step 2 — acquire install lock.
    let lock = match InstallLock::acquire(&layout.lock_file) {
        Ok(l) => l,
        Err(LockError::Held { path }) => return Err(LifecycleError::LockHeld { path }),
        Err(other) => return Err(LifecycleError::Lock { source: other }),
    };

    // Step 3 — authoritative load INSIDE the lock and rebuild the plan
    // against the live state. The plan we were handed was built outside
    // the lock; a concurrent install / uninstall could have mutated state
    // since then.
    let mut state = match InstalledState::load(&state_path) {
        Ok(s) => s,
        Err(source) => {
            drop(lock);
            return Err(LifecycleError::State { source });
        }
    };
    if state
        .find_object(ObjectKind::Component, target_name)
        .is_none()
    {
        drop(lock);
        return Err(LifecycleError::ComponentNotInstalled {
            component: target_name.to_string(),
        });
    }

    let live_plan = match plan.operation {
        LifecycleOperation::Uninstall => {
            LifecyclePlan::for_component_uninstall(target_name, &state)
        }
        LifecycleOperation::Purge => LifecyclePlan::for_component_purge(target_name, &state),
    };

    // Step 4 — open a Transaction inside the lock. Begin snapshots
    // installed.toml bytes, mints the operation_id, and writes an empty
    // journal. Errors propagate as LifecycleError::Transaction.
    let journal_dir = layout.state_dir.join("journal");
    let mut tx = match Transaction::begin(plan.operation.as_str(), state_path.clone(), &journal_dir)
    {
        Ok(t) => t,
        Err(source) => {
            drop(lock);
            return Err(LifecycleError::Transaction { source });
        }
    };
    let operation_id = tx.operation_id.clone();
    let started_at = tx.started_at.clone();
    let command = format!("{} {target_name}", plan.operation.as_str());

    let objects: Vec<String> = vec![target_name.to_string()];

    let central = CentralLog::open(layout.central_log.clone());

    // Step 5 — append the started record. Failure here is recoverable:
    // no destructive IO has happened, so we just finish the journal as
    // Failed and return.
    if let Err(source) = central.append(&started_record(
        &operation_id,
        &command,
        actor,
        install_mode,
        &started_at,
        objects.clone(),
        &format!("{command} started"),
    )) {
        let _ = tx.finish(TransactionOutcomeStatus::Failed);
        drop(lock);
        return Err(LifecycleError::Log { source });
    }

    // Step 5.25 — pre_uninstall hooks. Run BEFORE service-stop and
    // file-deletion so hooks can drain state, snapshot data, or notify
    // dependents while the component's binaries and services are still
    // in place.
    let hook_components: Vec<String> = vec![target_name.to_string()];
    let pre_uninstall = run_phase_hooks(
        layout,
        &hook_components,
        HookPhase::PreUninstall,
        Some(&central),
        &operation_id,
        actor,
        install_mode,
        true,
    );

    if let Some(hf) = pre_uninstall.hard_failure.as_ref() {
        return rollback_uninstall(
            LifecycleError::HookFailed {
                phase: "pre_uninstall".to_string(),
                component: hf.component.clone(),
                summary: hf.summary(),
                exit_code: hf.exit_code,
            },
            &mut tx,
            &central,
            &operation_id,
            &command,
            actor,
            install_mode,
            &started_at,
            objects,
            lock,
        );
    }

    // Step 5.5 — best-effort stop of every owned service unit BEFORE
    // the delete loop. Stopping a service before unlinking its binary
    // is the only sequence that lets a still-running daemon shut down
    // cleanly. Stop failures NEVER fail uninstall — they surface on
    // `LifecycleOutcome.warnings`.
    let mut warnings_pre_delete: Vec<String> = pre_uninstall.warnings;
    {
        let mut units: Vec<(String, String)> = Vec::new();
        for c in &live_plan.components {
            for s in &c.services {
                units.push((c.name.clone(), s.name.clone()));
            }
        }
        if !units.is_empty() {
            let env = EnvService::detect();
            let manager = service::for_install_mode(install_mode, &env);
            if manager.supported() {
                for (component, unit) in &units {
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
                            warnings_pre_delete.push(format!(
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
                for (component, unit) in &units {
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
    }

    // Step 6 — backup + delete every owned file flagged Remove. Files
    // that are skipped (Refuse / Keep / Unknown owner) are still
    // recorded as Skipped journal steps so the audit trail is honest
    // about what the executor saw.
    let mut removed_files: Vec<PathBuf> = Vec::new();
    let mut skipped_files: Vec<PathBuf> = Vec::new();
    let mut backup_idx: usize = 0;
    let backup_root = layout.backup_dir.join(&operation_id);

    for c in &live_plan.components {
        for f in &c.files {
            match (f.action, f.owner) {
                (FileActionKind::Remove, FileOwner::Anolisa) => {
                    // Boundary check: even though state claims this file
                    // is `owner = anolisa`, we require the *current*
                    // FsLayout's owned roots to contain it before we
                    // touch it. A forged or stale `installed.toml` that
                    // names `/etc/shadow` with `owner = anolisa` would
                    // otherwise turn uninstall into an arbitrary-delete
                    // primitive — install_runner already refuses to
                    // write outside these roots; uninstall must be
                    // symmetric. Skip + record so the operation still
                    // makes progress on legitimate files.
                    if let Err(boundary) = crate::path_safety::validate_owned_path(layout, &f.path)
                    {
                        if let Err(err) = record_skipped_step(
                            &mut tx,
                            "remove_file",
                            &f.path,
                            &format!(
                                "path outside ANOLISA-owned roots — refusing to delete: {boundary}"
                            ),
                        ) {
                            return rollback_uninstall(
                                err,
                                &mut tx,
                                &central,
                                &operation_id,
                                &command,
                                actor,
                                install_mode,
                                &started_at,
                                objects,
                                lock,
                            );
                        }
                        skipped_files.push(f.path.clone());
                        continue;
                    }
                    let backup_path = backup_root.join(format!("{backup_idx}.bak"));
                    backup_idx += 1;
                    match prepare_backup(&f.path, &backup_path) {
                        Ok(Some(artifact)) => {
                            let rb = RollbackAction::restore_file(
                                backup_path.clone(),
                                f.path.clone(),
                                artifact.into_sha256(),
                            );
                            let step = TransactionStep::planned(
                                "remove_file",
                                f.path.display().to_string(),
                                "remove",
                                Some(rb),
                            );
                            let idx = tx.steps.len();
                            if let Err(source) = tx.record_step(step) {
                                return rollback_uninstall(
                                    LifecycleError::Transaction { source },
                                    &mut tx,
                                    &central,
                                    &operation_id,
                                    &command,
                                    actor,
                                    install_mode,
                                    &started_at,
                                    objects,
                                    lock,
                                );
                            }
                            match fs::remove_file(&f.path) {
                                Ok(()) => {
                                    if let Err(source) = tx.mark_done(idx) {
                                        return rollback_uninstall(
                                            LifecycleError::Transaction { source },
                                            &mut tx,
                                            &central,
                                            &operation_id,
                                            &command,
                                            actor,
                                            install_mode,
                                            &started_at,
                                            objects,
                                            lock,
                                        );
                                    }
                                    removed_files.push(f.path.clone());
                                }
                                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                                    let _ = tx.mark_skipped(
                                        idx,
                                        "file vanished between backup and unlink",
                                    );
                                    skipped_files.push(f.path.clone());
                                }
                                Err(source) => {
                                    let _ = tx.mark_failed(idx, &source.to_string());
                                    return rollback_uninstall(
                                        LifecycleError::Filesystem {
                                            path: f.path.clone(),
                                            source,
                                        },
                                        &mut tx,
                                        &central,
                                        &operation_id,
                                        &command,
                                        actor,
                                        install_mode,
                                        &started_at,
                                        objects,
                                        lock,
                                    );
                                }
                            }
                        }
                        Ok(None) => {
                            // File already missing — idempotent path,
                            // record as skipped.
                            if let Err(err) = record_skipped_step(
                                &mut tx,
                                "remove_file",
                                &f.path,
                                "file already missing — idempotent",
                            ) {
                                return rollback_uninstall(
                                    err,
                                    &mut tx,
                                    &central,
                                    &operation_id,
                                    &command,
                                    actor,
                                    install_mode,
                                    &started_at,
                                    objects,
                                    lock,
                                );
                            }
                            skipped_files.push(f.path.clone());
                        }
                        Err(err) => {
                            return rollback_uninstall(
                                err,
                                &mut tx,
                                &central,
                                &operation_id,
                                &command,
                                actor,
                                install_mode,
                                &started_at,
                                objects,
                                lock,
                            );
                        }
                    }
                }
                _ => {
                    let reason = match f.action {
                        FileActionKind::Refuse => f
                            .reason
                            .clone()
                            .unwrap_or_else(|| "external — refused".to_string()),
                        _ => f.reason.clone().unwrap_or_else(|| "kept".to_string()),
                    };
                    if let Err(err) = record_skipped_step(&mut tx, "remove_file", &f.path, &reason)
                    {
                        return rollback_uninstall(
                            err,
                            &mut tx,
                            &central,
                            &operation_id,
                            &command,
                            actor,
                            install_mode,
                            &started_at,
                            objects,
                            lock,
                        );
                    }
                    skipped_files.push(f.path.clone());
                }
            }
        }

        // Purge: also remove ANOLISA-owned config / state fragments.
        // Today the gate above prevents this branch from executing for
        // Purge, but we leave the loop in place so the wiring is ready
        // when the gate lifts.
        if plan.operation == LifecycleOperation::Purge {
            for f in &c.configs {
                if f.action == FileActionKind::Remove && f.owner == FileOwner::Anolisa {
                    // Mirror the boundary check applied to `files`: a
                    // forged config path outside `FsLayout` must be
                    // skipped, never backed up + deleted.
                    if let Err(boundary) = crate::path_safety::validate_owned_path(layout, &f.path)
                    {
                        if let Err(err) = record_skipped_step(
                            &mut tx,
                            "remove_config",
                            &f.path,
                            &format!(
                                "path outside ANOLISA-owned roots — refusing to delete: {boundary}"
                            ),
                        ) {
                            return rollback_uninstall(
                                err,
                                &mut tx,
                                &central,
                                &operation_id,
                                &command,
                                actor,
                                install_mode,
                                &started_at,
                                objects,
                                lock,
                            );
                        }
                        skipped_files.push(f.path.clone());
                        continue;
                    }
                    let backup_path = backup_root.join(format!("{backup_idx}.bak"));
                    backup_idx += 1;
                    match prepare_backup(&f.path, &backup_path) {
                        Ok(Some(artifact)) => {
                            let rb = RollbackAction::restore_file(
                                backup_path.clone(),
                                f.path.clone(),
                                artifact.into_sha256(),
                            );
                            let step = TransactionStep::planned(
                                "remove_config",
                                f.path.display().to_string(),
                                "remove",
                                Some(rb),
                            );
                            let idx = tx.steps.len();
                            if let Err(source) = tx.record_step(step) {
                                return rollback_uninstall(
                                    LifecycleError::Transaction { source },
                                    &mut tx,
                                    &central,
                                    &operation_id,
                                    &command,
                                    actor,
                                    install_mode,
                                    &started_at,
                                    objects,
                                    lock,
                                );
                            }
                            match fs::remove_file(&f.path) {
                                Ok(()) => {
                                    if let Err(source) = tx.mark_done(idx) {
                                        return rollback_uninstall(
                                            LifecycleError::Transaction { source },
                                            &mut tx,
                                            &central,
                                            &operation_id,
                                            &command,
                                            actor,
                                            install_mode,
                                            &started_at,
                                            objects,
                                            lock,
                                        );
                                    }
                                    removed_files.push(f.path.clone());
                                }
                                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                                    let _ = tx.mark_skipped(idx, "file vanished");
                                    skipped_files.push(f.path.clone());
                                }
                                Err(source) => {
                                    let _ = tx.mark_failed(idx, &source.to_string());
                                    return rollback_uninstall(
                                        LifecycleError::Filesystem {
                                            path: f.path.clone(),
                                            source,
                                        },
                                        &mut tx,
                                        &central,
                                        &operation_id,
                                        &command,
                                        actor,
                                        install_mode,
                                        &started_at,
                                        objects,
                                        lock,
                                    );
                                }
                            }
                        }
                        Ok(None) => {
                            skipped_files.push(f.path.clone());
                        }
                        Err(err) => {
                            return rollback_uninstall(
                                err,
                                &mut tx,
                                &central,
                                &operation_id,
                                &command,
                                actor,
                                install_mode,
                                &started_at,
                                objects,
                                lock,
                            );
                        }
                    }
                }
            }
        }
    }

    // Step 7 — drop the component object from state.
    let _ = state.remove_object(ObjectKind::Component, target_name);

    // Step 7.5 — migrate away legacy capability objects. Releases that
    // predate the capability removal may have left `kind = "capability"`
    // rows; prune them on this write so old state files converge. A
    // Step 8 save failure restores the prior bytes, rolling this back too.
    let pruned_legacy = state.prune_legacy_capabilities();

    let finished_at_utc = Utc::now();
    let finished_at = finished_at_utc.to_rfc3339_opts(SecondsFormat::Secs, true);
    state.append_operation(OperationRecord {
        id: operation_id.clone(),
        command: command.clone(),
        status: "ok".to_string(),
        started_at: started_at.clone(),
        finished_at: Some(finished_at.clone()),
    });

    // Step 8 — persist state. On failure, restore every removed file
    // via the journal AND restore the prior installed.toml bytes.
    if let Err(source) = state.save(&state_path) {
        return rollback_uninstall(
            LifecycleError::State { source },
            &mut tx,
            &central,
            &operation_id,
            &command,
            actor,
            install_mode,
            &started_at,
            objects,
            lock,
        );
    }

    // Step 9 — append the succeeded record. On failure, the on-disk
    // state already reflects the new "uninstalled" status that we will
    // not be able to advertise — roll back files AND state so the
    // operator's view stays internally consistent.
    if let Err(source) = central.append(&succeeded_record(
        &operation_id,
        &command,
        actor,
        install_mode,
        &started_at,
        &finished_at,
        objects.clone(),
        &format!("{command} succeeded"),
    )) {
        return rollback_uninstall(
            LifecycleError::Log { source },
            &mut tx,
            &central,
            &operation_id,
            &command,
            actor,
            install_mode,
            &started_at,
            objects,
            lock,
        );
    }

    // Step 10 — finalise the journal. By the time we reach here, the
    // destructive ops have already succeeded, `installed.toml` reflects
    // the new "uninstalled" state, and the central log carries the
    // `succeeded` record. A finalize error therefore is NOT a uninstall
    // failure — only the journal failed to record its terminal status.
    // Returning `Err` here would surface as `EXECUTION_FAILED`
    // (CLI exit 1) on a system that is in fact already uninstalled,
    // confusing automation. Instead, append a warning-severity central
    // log record (best-effort) and return Ok with the warning surfaced
    // on the outcome so the CLI can render it.
    let mut warnings = finalize_journal_with_warnings(
        &mut tx,
        &central,
        &operation_id,
        &command,
        actor,
        install_mode,
        &started_at,
        &objects,
    );
    // Pre-delete service-stop warnings prepend journal warnings so the
    // operator sees them first — they happened earlier in the op and
    // are usually the more actionable signal (a still-running daemon
    // can keep its binary mapped after delete on Linux).
    let mut combined = warnings_pre_delete;
    combined.append(&mut warnings);
    if !pruned_legacy.is_empty() {
        let msg = format!(
            "pruned legacy capability state object(s) written by an older release: {}",
            pruned_legacy.join(", ")
        );
        // Best-effort audit trail; the prune already landed with Step 8.
        let _ = central.append(&warning_record(
            &operation_id,
            &command,
            actor,
            install_mode,
            &started_at,
            pruned_legacy,
            &msg,
        ));
        combined.push(msg);
    }
    let warnings = combined;

    // Best-effort cleanup of backups on the success path.
    let _ = fs::remove_dir_all(&backup_root);

    drop(lock);

    // Step 11 — post_uninstall hooks. Run AFTER the lock has been
    // released so cleanup scripts can do their own slow IO (rsync state
    // out, archive logs, notify external systems) without holding up
    // concurrent CLI calls. Failures only warn — by now the central
    // log already records `succeeded` and `installed.toml` reflects
    // removal; downgrading would lie about what is on disk.
    let post_uninstall = run_phase_hooks(
        layout,
        &hook_components,
        HookPhase::PostUninstall,
        Some(&central),
        &operation_id,
        actor,
        install_mode,
        false,
    );
    let mut warnings = warnings;
    warnings.extend(post_uninstall.warnings);

    Ok(LifecycleOutcome {
        operation_id,
        operation: plan.operation,
        component: target_name.to_string(),
        removed_files,
        skipped_files,
        state_object_removed: true,
        warnings,
        state_path,
        central_log_path: layout.central_log.clone(),
    })
}

/// Try to finalize `tx` as `Ok`. If the underlying journal write fails,
/// emit a warning-severity record into `central` (best-effort) and
/// return the warning string instead of propagating the error — this
/// runs only after `state.save` + `succeeded` log have landed, so
/// flipping the wire result to "failed" would lie about what is
/// actually on disk.
#[allow(clippy::too_many_arguments)]
fn finalize_journal_with_warnings(
    tx: &mut Transaction,
    central: &CentralLog,
    operation_id: &str,
    command: &str,
    actor: &str,
    install_mode: &str,
    started_at: &str,
    objects: &[String],
) -> Vec<String> {
    let mut warnings = Vec::new();
    if let Err(source) = tx.finish(TransactionOutcomeStatus::Ok) {
        let warning = format!("journal finalize failed: {source}");
        let _ = central.append(&warning_record(
            operation_id,
            command,
            actor,
            install_mode,
            started_at,
            objects.to_vec(),
            &warning,
        ));
        warnings.push(warning);
    }
    warnings
}

/// What [`prepare_backup`] wrote at the backup path.
#[derive(Debug)]
enum BackupArtifact {
    /// Regular file copied byte-for-byte; sha256 of those bytes.
    File {
        /// Content hash recorded on the `RestoreFile` rollback action.
        sha256: String,
    },
    /// Symlink reproduced as an identical link. The referent is never
    /// read through, so there is no byte hash to verify on restore.
    Symlink,
}

impl BackupArtifact {
    /// Hash to record on the rollback action; `None` for symlinks.
    fn into_sha256(self) -> Option<String> {
        match self {
            Self::File { sha256 } => Some(sha256),
            Self::Symlink => None,
        }
    }
}

/// Copy `src` to `backup` while streaming sha256 over the bytes.
///
/// The backup path is the rollback's single source of truth — every
/// `RestoreFile` step replays bytes from here, so this write must be at
/// least as hardened as install:
///
///   * A symlink at `src` (a managed `FileKind::Symlink` entry) is backed
///     up as a *link*: the referent path is reproduced, never read
///     through — bytes behind a link must not be copied as if they
///     belonged to the owned file. Regular files still open with
///     `O_NOFOLLOW` so a link racing in after the metadata check fails
///     the open instead of being followed.
///   * Backup leaf opened with `create_new` (+ `O_NOFOLLOW` on Unix) so
///     a pre-placed symlink or stale file at the backup path fails the
///     open instead of being followed or overwritten (`symlink(2)` gives
///     the same EEXIST guarantee on the link branch).
///   * Streaming read+hash so a multi-GB owned file does not have to fit
///     in RAM, and so the on-disk bytes match the recorded sha exactly.
///
/// Returns `Ok(None)` only if `src` is `NotFound`; other errors are
/// surfaced as [`LifecycleError::Filesystem`].
fn prepare_backup(src: &Path, backup: &Path) -> Result<Option<BackupArtifact>, LifecycleError> {
    use std::io::{Read, Write};

    match fs::symlink_metadata(src) {
        Ok(meta) if meta.file_type().is_symlink() => {
            let referent = fs::read_link(src).map_err(|source| LifecycleError::Filesystem {
                path: src.to_path_buf(),
                source,
            })?;
            if let Some(parent) = backup.parent()
                && !parent.as_os_str().is_empty()
                && let Err(source) = fs::create_dir_all(parent)
            {
                return Err(LifecycleError::Filesystem {
                    path: parent.to_path_buf(),
                    source,
                });
            }
            std::os::unix::fs::symlink(&referent, backup).map_err(|source| {
                LifecycleError::Filesystem {
                    path: backup.to_path_buf(),
                    source,
                }
            })?;
            return Ok(Some(BackupArtifact::Symlink));
        }
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(LifecycleError::Filesystem {
                path: src.to_path_buf(),
                source,
            });
        }
    }

    let mut src_opts = fs::OpenOptions::new();
    src_opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        src_opts.custom_flags(nix::libc::O_NOFOLLOW);
    }
    let mut src_f = match src_opts.open(src) {
        Ok(f) => f,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(LifecycleError::Filesystem {
                path: src.to_path_buf(),
                source,
            });
        }
    };

    if let Some(parent) = backup.parent()
        && !parent.as_os_str().is_empty()
        && let Err(source) = fs::create_dir_all(parent)
    {
        return Err(LifecycleError::Filesystem {
            path: parent.to_path_buf(),
            source,
        });
    }

    let mut backup_opts = fs::OpenOptions::new();
    backup_opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        backup_opts.custom_flags(nix::libc::O_NOFOLLOW);
    }
    let mut backup_f = match backup_opts.open(backup) {
        Ok(f) => f,
        Err(source) => {
            return Err(LifecycleError::Filesystem {
                path: backup.to_path_buf(),
                source,
            });
        }
    };

    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = match src_f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(source) => {
                let _ = fs::remove_file(backup);
                return Err(LifecycleError::Filesystem {
                    path: src.to_path_buf(),
                    source,
                });
            }
        };
        if let Err(source) = backup_f.write_all(&buf[..n]) {
            let _ = fs::remove_file(backup);
            return Err(LifecycleError::Filesystem {
                path: backup.to_path_buf(),
                source,
            });
        }
        hasher.update(&buf[..n]);
    }
    if let Err(source) = backup_f.sync_all() {
        let _ = fs::remove_file(backup);
        return Err(LifecycleError::Filesystem {
            path: backup.to_path_buf(),
            source,
        });
    }

    let out = hasher.finalize();
    let mut sha = String::with_capacity(64);
    for b in out {
        sha.push_str(&format!("{b:02x}"));
    }
    Ok(Some(BackupArtifact::File { sha256: sha }))
}

fn record_skipped_step(
    tx: &mut Transaction,
    phase: &str,
    target: &Path,
    reason: &str,
) -> Result<(), LifecycleError> {
    let step = TransactionStep::planned(phase, target.display().to_string(), "skip", None);
    let idx = tx.steps.len();
    tx.record_step(step)
        .map_err(|source| LifecycleError::Transaction { source })?;
    tx.mark_skipped(idx, reason)
        .map_err(|source| LifecycleError::Transaction { source })
}

/// Walk `tx.steps` in reverse, restoring every `Done` step's file via
/// its rollback action; restore `installed.toml` from the snapshot;
/// mark the failing step `Failed`; finish the journal as `RolledBack`;
/// emit a `failed` central-log record; drop the lock; return `err`.
#[allow(clippy::too_many_arguments)]
fn rollback_uninstall(
    err: LifecycleError,
    tx: &mut Transaction,
    central: &CentralLog,
    operation_id: &str,
    command: &str,
    actor: &str,
    install_mode: &str,
    started_at: &str,
    objects: Vec<String>,
    lock: InstallLock,
) -> Result<LifecycleOutcome, LifecycleError> {
    let mut warnings: Vec<String> = Vec::new();

    // Walk done steps in reverse so the original state is restored in
    // the opposite order it was mutated. Rollback errors are appended
    // as warnings on the failed log record but do not mask the original
    // error — the operator needs to see the root cause first.
    let mut idxs_done: Vec<usize> = Vec::new();
    for (idx, step) in tx.steps.iter().enumerate() {
        if step.status == TransactionStepStatus::Done {
            idxs_done.push(idx);
        }
    }
    for idx in idxs_done.into_iter().rev() {
        let rollback = tx.steps[idx].rollback.clone();
        if let Some(rb) = rollback {
            if let Err(source) = tx.restore_file(&rb) {
                warnings.push(format!("rollback restore_file failed: {source}"));
            } else if let Err(source) = tx.mark_rolled_back(idx) {
                warnings.push(format!("journal mark_rolled_back failed: {source}"));
            }
        }
    }

    if let Err(source) = tx.restore_state() {
        warnings.push(format!("rollback restore_state failed: {source}"));
    }

    let _ = tx.finish(TransactionOutcomeStatus::RolledBack);

    let finished_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let _ = central.append(&failed_record_with_warnings(
        operation_id,
        command,
        actor,
        install_mode,
        started_at,
        &finished_at,
        objects,
        &err,
        warnings,
    ));
    drop(lock);
    Err(err)
}

fn started_record(
    operation_id: &str,
    command: &str,
    actor: &str,
    install_mode: &str,
    started_at: &str,
    objects: Vec<String>,
    message: &str,
) -> LogRecord {
    LogRecord {
        kind: LogKind::Operation,
        operation_id: Some(operation_id.to_string()),
        command: command.to_string(),
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
    command: &str,
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
        command: command.to_string(),
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

/// Warning-severity log record. Used after a successful destructive op
/// when a *post-success* step (currently: journal finalize) failed
/// without invalidating the on-disk uninstall state.
#[allow(clippy::too_many_arguments)]
fn warning_record(
    operation_id: &str,
    command: &str,
    actor: &str,
    install_mode: &str,
    started_at: &str,
    objects: Vec<String>,
    message: &str,
) -> LogRecord {
    LogRecord {
        kind: LogKind::Operation,
        operation_id: Some(operation_id.to_string()),
        command: command.to_string(),
        source: "anolisa-cli".to_string(),
        component: None,
        severity: Severity::Warn,
        message: message.to_string(),
        actor: actor.to_string(),
        install_mode: Some(install_mode.to_string()),
        started_at: started_at.to_string(),
        finished_at: None,
        status: None,
        objects,
        backup_ids: Vec::new(),
        warnings: vec![message.to_string()],
        details: serde_json::Value::Null,
    }
}

#[allow(clippy::too_many_arguments)]
fn failed_record_with_warnings(
    operation_id: &str,
    command: &str,
    actor: &str,
    install_mode: &str,
    started_at: &str,
    finished_at: &str,
    objects: Vec<String>,
    err: &LifecycleError,
    warnings: Vec<String>,
) -> LogRecord {
    LogRecord {
        kind: LogKind::Operation,
        operation_id: Some(operation_id.to_string()),
        command: command.to_string(),
        source: "anolisa-cli".to_string(),
        component: None,
        severity: Severity::Error,
        message: format!("{command} failed: {err}"),
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

// Allow tests + callers to reuse object_status_wire if needed in the
// future. Kept private until a need surfaces.
#[allow(dead_code)]
fn object_status_wire(status: ObjectStatus) -> &'static str {
    match status {
        ObjectStatus::Installed => "installed",
        ObjectStatus::Partial => "degraded",
        ObjectStatus::Disabled => "disabled",
        ObjectStatus::Failed => "failed",
        ObjectStatus::Adopted => "adopted",
    }
}

// Unused but referenced via `_ =` to silence warnings until a follow-up
// surfaces InstalledObject in the public planning APIs.
#[allow(dead_code)]
fn touch_installed_object(_obj: &InstalledObject) {}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::state::{
        ExternalModifiedFile, FileOwner as StateFileOwner, InstalledObject, InstalledState,
        ObjectKind, ObjectStatus, OwnedFile, ServiceRef,
    };
    use std::fs as std_fs;
    use std::path::Path;
    use tempfile::tempdir;

    fn fixture_layout(prefix: &Path) -> FsLayout {
        FsLayout::system(Some(prefix.to_path_buf()))
    }

    fn seed_state_with_two_files(
        layout: &FsLayout,
        component: &str,
        owned_path: &Path,
        external_path: &Path,
    ) -> InstalledState {
        std_fs::create_dir_all(&layout.state_dir).expect("mkdir state");
        let mut state = InstalledState::default();
        state.upsert_object(InstalledObject {
            kind: ObjectKind::Component,
            name: component.to_string(),
            version: "0.2.0".to_string(),
            status: ObjectStatus::Installed,
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
                path: owned_path.to_path_buf(),
                owner: StateFileOwner::Anolisa,
                sha256: Some("0".repeat(64)),
            }],
            external_modified_files: vec![ExternalModifiedFile {
                path: external_path.to_path_buf(),
                owner: StateFileOwner::External,
                backup_id: "backup-prior".to_string(),
                sha256_before: Some("a".repeat(64)),
                sha256_after: Some("b".repeat(64)),
            }],
            services: vec![ServiceRef {
                name: format!("{component}.service"),
                manager: "systemd".to_string(),
                restartable: true,
                enabled: false,
            }],
            health: Vec::new(),
        });
        state
            .save(&layout.state_dir.join("installed.toml"))
            .expect("seed state save");
        state
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

    /// pre_uninstall + post_uninstall scripts discovered under
    /// `<datadir>/hooks/<component>/<phase>.sh` must actually run
    /// during a real uninstall AND emit a `LogKind::Component` record
    /// per attempt to the central log. This pins the wiring so a
    /// future refactor of `execute_uninstall_or_purge` cannot drop
    /// hook execution silently.
    #[test]
    #[cfg(unix)]
    fn uninstall_runs_pre_and_post_hooks_and_records_them_in_central_log() {
        let root = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());
        std_fs::create_dir_all(&layout.bin_dir).unwrap();
        let owned_path = layout.bin_dir.join("agentsight");
        std_fs::write(&owned_path, b"owned").unwrap();
        let external_path = layout.etc_dir.join("foreign.conf");
        std_fs::create_dir_all(external_path.parent().unwrap()).unwrap();
        std_fs::write(&external_path, b"external").unwrap();

        seed_state_with_two_files(&layout, "agentsight", &owned_path, &external_path);

        write_hook_script(
            &layout,
            "agentsight",
            "pre_uninstall",
            "#!/bin/sh\nexit 0\n",
        );
        write_hook_script(
            &layout,
            "agentsight",
            "post_uninstall",
            "#!/bin/sh\nexit 0\n",
        );

        let plan = LifecyclePlan::for_component_uninstall(
            "agentsight",
            &InstalledState::load(&layout.state_dir.join("installed.toml")).unwrap(),
        );
        let outcome = execute_plan(&plan, &layout, "tester", "system").expect("uninstall ok");

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
            "expected pre+post uninstall hook log entries, got: {lines:?}",
        );
        let commands: Vec<&str> = hook_lines
            .iter()
            .map(|l| l.get("command").and_then(|v| v.as_str()).unwrap_or(""))
            .collect();
        assert!(
            commands.contains(&"hook:pre_uninstall") && commands.contains(&"hook:post_uninstall"),
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

    #[test]
    fn uninstall_plan_remove_anolisa_refuse_external() {
        let root = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());
        let owned = layout.bin_dir.join("agentsight");
        let external = layout.etc_dir.join("third-party.toml");
        let state = seed_state_with_two_files(&layout, "agentsight", &owned, &external);

        let plan = LifecyclePlan::for_component_uninstall("agentsight", &state);
        assert_eq!(plan.operation, LifecycleOperation::Uninstall);
        assert_eq!(plan.risk, RiskLevel::Medium);
        // Service phases recorded as Stop (executed best-effort by the
        // ServiceManager; degrades to a quiet skip on unsupported hosts).
        for s in &plan.components[0].services {
            assert_eq!(s.action, ServiceActionKind::Stop);
        }
        let comp = &plan.components[0];
        let owned_action = comp
            .files
            .iter()
            .find(|f| f.path == owned)
            .expect("owned file in plan");
        assert_eq!(owned_action.action, FileActionKind::Remove);
        assert_eq!(owned_action.owner, FileOwner::Anolisa);
        let ext_action = comp
            .files
            .iter()
            .find(|f| f.path == external)
            .expect("external file in plan");
        assert_eq!(ext_action.action, FileActionKind::Refuse);
        assert_eq!(ext_action.owner, FileOwner::External);
    }

    #[test]
    fn uninstall_execute_removes_anolisa_owned_and_preserves_external() {
        // Success path through the transaction-backed executor:
        //
        //   * the ANOLISA-owned binary is unlinked,
        //   * the external file is preserved,
        //   * `installed.toml` drops the component,
        //   * the central log gains a started+succeeded pair,
        //   * a journal exists under `state_dir/journal/` whose terminal
        //     status is `Ok` and whose `remove_file` step is `Done`.
        let root = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());
        std_fs::create_dir_all(&layout.bin_dir).expect("mkdir bin");
        let owned = layout.bin_dir.join("agentsight");
        std_fs::write(&owned, b"binary content").expect("write owned");
        std_fs::create_dir_all(&layout.etc_dir).expect("mkdir etc");
        let external = layout.etc_dir.join("third-party.toml");
        std_fs::write(&external, b"third-party = true\n").expect("write external");

        let state = seed_state_with_two_files(&layout, "agentsight", &owned, &external);
        let plan = LifecyclePlan::for_component_uninstall("agentsight", &state);

        let outcome =
            execute_plan(&plan, &layout, "tester", "system").expect("uninstall must succeed");
        assert_eq!(outcome.operation, LifecycleOperation::Uninstall);
        assert!(outcome.state_object_removed);
        assert_eq!(outcome.removed_files, vec![owned.clone()]);
        // external + the second-pass external_modified_files entry
        // (seeded by `seed_state_with_two_files`) both surface as skipped.
        assert!(outcome.skipped_files.iter().any(|p| p == &external));

        assert!(!owned.exists(), "ANOLISA-owned file must be removed");
        assert!(external.exists(), "external file must be preserved");

        let after =
            InstalledState::load(&layout.state_dir.join("installed.toml")).expect("reload state");
        assert!(
            after
                .find_object(ObjectKind::Component, "agentsight")
                .is_none(),
            "component must be dropped",
        );

        // Operation-kind only — service:stop / hook component records
        // are tested separately and don't belong in this verb-shape pin.
        let all = read_log_lines(&layout.central_log);
        let lines: Vec<&serde_json::Value> = all
            .iter()
            .filter(|l| l.get("kind").and_then(|v| v.as_str()) == Some("operation"))
            .collect();
        assert_eq!(lines.len(), 2, "expect started + succeeded record");
        assert_eq!(
            lines[0].get("command").and_then(|v| v.as_str()),
            Some("uninstall agentsight"),
        );
        assert_eq!(lines[1].get("status").and_then(|v| v.as_str()), Some("ok"),);

        let journal_dir = layout.state_dir.join("journal");
        let journal_path = journal_dir.join(format!("{}.journal.toml", outcome.operation_id));
        let tx = crate::transaction::Transaction::load_journal(&journal_path)
            .expect("journal must round-trip");
        assert_eq!(
            tx.status,
            crate::transaction::TransactionOutcomeStatus::Ok,
            "terminal journal status must be Ok",
        );
        assert!(
            tx.steps.iter().any(|s| s.action == "remove"
                && s.target == owned.display().to_string()
                && s.status == TransactionStepStatus::Done),
            "journal must record the remove_file step as Done",
        );
    }

    #[test]
    fn uninstall_execute_lock_held_returns_lock_held() {
        let root = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());
        std_fs::create_dir_all(&layout.bin_dir).expect("mkdir bin");
        let owned = layout.bin_dir.join("agentsight");
        std_fs::write(&owned, b"binary").expect("write owned");
        let external = layout.etc_dir.join("third.toml");
        let state = seed_state_with_two_files(&layout, "agentsight", &owned, &external);
        let plan = LifecyclePlan::for_component_uninstall("agentsight", &state);

        // Hold the install lock from this test thread before invoking.
        let _held = crate::lock::InstallLock::acquire(&layout.lock_file)
            .expect("first acquire must succeed");
        let err = execute_plan(&plan, &layout, "tester", "system")
            .expect_err("must fail while lock is held");
        match err {
            LifecycleError::LockHeld { path } => assert_eq!(path, layout.lock_file),
            other => panic!("expected LockHeld, got {other:?}"),
        }
        assert!(owned.exists(), "lock-held failure must not touch files");
    }

    #[test]
    #[cfg(unix)]
    fn uninstall_execute_state_save_failure_rolls_back_files_and_state() {
        if nix::unistd::Uid::effective().is_root() {
            // CAP_DAC_OVERRIDE bypasses the `chmod 0o500` sabotage below,
            // so this regression can only be exercised under an
            // unprivileged uid.
            eprintln!(
                "skipping uninstall_execute_state_save_failure_rolls_back_files_and_state under uid 0"
            );
            return;
        }
        // Sabotage strategy: keep `state_dir` writable for everything the
        // executor needs (lock acquire, journal writes, backup writes),
        // then flip `state_dir` to 0o500 so the *only* operation that
        // fails is `state.save` — which must create a fresh tmp sibling
        // inside `state_dir`. Lock-file open succeeds because the lock
        // path is pre-created and Unix only requires +x on the parent
        // for opening an existing file. Journal/backup writes succeed
        // because their subdirs are pre-created and keep their default
        // 0o755 perms.
        //
        // The rollback must:
        //   * restore the deleted owned file from its backup, AND
        //   * restore the original `installed.toml` snapshot bytes.
        use std::os::unix::fs::PermissionsExt;

        let root = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());
        std_fs::create_dir_all(&layout.bin_dir).expect("mkdir bin");
        let owned = layout.bin_dir.join("agentsight");
        std_fs::write(&owned, b"binary content").expect("write owned");
        let external = layout.etc_dir.join("third.toml");
        let _state = seed_state_with_two_files(&layout, "agentsight", &owned, &external);

        // Re-load the state to build the plan against the on-disk bytes.
        let live_state =
            InstalledState::load(&layout.state_dir.join("installed.toml")).expect("load");
        let plan = LifecyclePlan::for_component_uninstall("agentsight", &live_state);

        // Pre-create everything the executor would otherwise create
        // inside `state_dir` so the chmod below cannot block it.
        std_fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&layout.lock_file)
            .expect("pre-create lock file");
        let journal_dir = layout.state_dir.join("journal");
        std_fs::create_dir_all(&journal_dir).expect("mkdir journal");
        std_fs::create_dir_all(&layout.backup_dir).expect("mkdir backups");
        std_fs::create_dir_all(&layout.log_dir).expect("mkdir log");

        // Drop write perm on `state_dir` so `state.save`'s tmp-sibling
        // create_new fails. Existing entries (installed.toml, lock,
        // journal/, backups/) keep their own perms and remain usable.
        let original = std_fs::metadata(&layout.state_dir).unwrap().permissions();
        let mut readonly = original.clone();
        readonly.set_mode(0o500);
        std_fs::set_permissions(&layout.state_dir, readonly).unwrap();

        let result = execute_plan(&plan, &layout, "tester", "system");

        // Restore writable perms so we can inspect on-disk state.
        std_fs::set_permissions(&layout.state_dir, original).unwrap();

        let err = result.expect_err("state.save sabotage must surface as error");
        assert!(
            matches!(err, LifecycleError::State { .. }),
            "expected State error, got {err:?}",
        );

        // Owned file restored from backup.
        assert!(
            owned.exists(),
            "rollback must restore the deleted owned file from backup",
        );
        let restored = std_fs::read(&owned).expect("read restored");
        assert_eq!(
            restored, b"binary content",
            "restored bytes must match backup bytes",
        );
        // installed.toml still names the component (snapshot restored).
        let after =
            InstalledState::load(&layout.state_dir.join("installed.toml")).expect("reload state");
        assert!(
            after
                .find_object(ObjectKind::Component, "agentsight")
                .is_some(),
            "snapshot restore must put the component back",
        );

        // Central log: started + failed entry. Operation-kind only —
        // service-stop component records are tested separately.
        let all = read_log_lines(&layout.central_log);
        let lines: Vec<&serde_json::Value> = all
            .iter()
            .filter(|l| l.get("kind").and_then(|v| v.as_str()) == Some("operation"))
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[1].get("status").and_then(|v| v.as_str()),
            Some("failed"),
        );
    }

    #[test]
    fn uninstall_dry_run_does_not_mutate_anything() {
        // "dry-run" is a CLI-level concept: the executor is never
        // invoked. Here we exercise the planner-only path and confirm
        // no IO occurs.
        let root = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());
        std_fs::create_dir_all(&layout.bin_dir).expect("mkdir bin");
        let owned = layout.bin_dir.join("agentsight");
        std_fs::write(&owned, b"keep me").expect("write owned");
        let external = layout.etc_dir.join("third.toml");
        let state = seed_state_with_two_files(&layout, "agentsight", &owned, &external);

        let plan = LifecyclePlan::for_component_uninstall("agentsight", &state);
        assert!(!plan.components.is_empty());
        assert!(
            owned.exists(),
            "dry-run planner must not touch the filesystem",
        );
        assert!(!layout.central_log.exists());
    }

    #[test]
    fn uninstall_on_not_installed_component_returns_component_not_installed() {
        let root = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());
        // No state on disk at all.
        let empty = InstalledState::default();
        let plan = LifecyclePlan::for_component_uninstall("agentsight", &empty);
        let err = execute_plan(&plan, &layout, "tester", "system").expect_err("must error");
        match err {
            LifecycleError::ComponentNotInstalled { component } => {
                assert_eq!(component, "agentsight");
            }
            other => panic!("expected ComponentNotInstalled, got {other:?}"),
        }
        assert!(!layout.central_log.exists());
    }

    #[test]
    fn purge_execute_is_gated_and_leaves_filesystem_untouched() {
        // Purge is the strictest form of destructive teardown and is
        // covered by the same gate as uninstall. The plan is built
        // normally so `--dry-run` works, but the executor must refuse
        // before touching anything.
        let root = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());
        std_fs::create_dir_all(&layout.bin_dir).expect("mkdir bin");
        std_fs::create_dir_all(&layout.etc_dir).expect("mkdir etc");
        let owned = layout.bin_dir.join("agentsight");
        std_fs::write(&owned, b"owned").expect("write owned");
        let external = layout.etc_dir.join("third-party.toml");
        std_fs::write(&external, b"third").expect("write external");

        let state = seed_state_with_two_files(&layout, "agentsight", &owned, &external);

        let plan = LifecyclePlan::for_component_purge("agentsight", &state);
        assert_eq!(plan.operation, LifecycleOperation::Purge);
        assert_eq!(plan.risk, RiskLevel::High);

        let err = execute_plan(&plan, &layout, "tester", "system")
            .expect_err("purge execute must be gated");
        match &err {
            LifecycleError::ExecuteGated { reason } => {
                assert!(
                    reason.contains("purge execute is gated"),
                    "gate reason must name the operation: {reason}",
                );
            }
            other => panic!("expected ExecuteGated, got {other:?}"),
        }

        assert!(owned.exists(), "owned must survive gated purge");
        assert!(external.exists(), "external must survive gated purge");
        assert!(
            !layout.central_log.exists(),
            "gated purge must NOT write to the central log",
        );
    }

    #[test]
    fn uninstall_filesystem_error_rolls_back_and_emits_failed_log() {
        // The owned "file" recorded in installed.toml is actually a
        // directory on disk. `fs::remove_file` returns EISDIR; the
        // executor must surface a `Filesystem` error, restore the prior
        // state from the snapshot (component still present), and emit a
        // failed central-log record — NOT a succeeded one.
        let root = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());
        std_fs::create_dir_all(&layout.bin_dir).expect("mkdir bin");
        let owned = layout.bin_dir.join("agentsight");
        std_fs::create_dir(&owned).expect("create owned as dir");
        let external = layout.etc_dir.join("third.toml");
        let state = seed_state_with_two_files(&layout, "agentsight", &owned, &external);

        let plan = LifecyclePlan::for_component_uninstall("agentsight", &state);
        let err = execute_plan(&plan, &layout, "tester", "system")
            .expect_err("EISDIR must surface as a Filesystem error");
        assert!(
            matches!(err, LifecycleError::Filesystem { .. }),
            "expected Filesystem error, got {err:?}",
        );

        // The directory survives (EISDIR was the symptom, not a partial
        // delete) and the snapshot rollback put the component back.
        assert!(owned.exists(), "directory must survive failed unlink");
        let after =
            InstalledState::load(&layout.state_dir.join("installed.toml")).expect("reload state");
        assert!(
            after
                .find_object(ObjectKind::Component, "agentsight")
                .is_some(),
            "snapshot rollback must restore the component object",
        );

        // Operation-kind only — service-stop / hook component records
        // are not the focus of this filesystem-rollback test.
        let all = read_log_lines(&layout.central_log);
        let lines: Vec<&serde_json::Value> = all
            .iter()
            .filter(|l| l.get("kind").and_then(|v| v.as_str()) == Some("operation"))
            .collect();
        assert_eq!(lines.len(), 2, "expect started + failed log records");
        assert_eq!(
            lines[1].get("status").and_then(|v| v.as_str()),
            Some("failed"),
        );
    }

    /// A component that declares ZERO `OwnedFile` entries must still
    /// pass through the executor cleanly: the journal opens, the state
    /// object is dropped, and the central log gets a started +
    /// succeeded pair. No file deletions, no rollback.
    #[test]
    fn uninstall_execute_with_no_removable_files_succeeds() {
        let root = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());
        std_fs::create_dir_all(&layout.state_dir).expect("mkdir state");

        // Seed an installed.toml whose component owns no files and no
        // external modifications — only state/log changes would happen
        // on uninstall.
        let mut state = InstalledState::default();
        state.upsert_object(InstalledObject {
            kind: ObjectKind::Component,
            name: "agentsight".to_string(),
            version: "0.2.0".to_string(),
            status: ObjectStatus::Installed,
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
            files: Vec::new(),
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
        });
        let state_path = layout.state_dir.join("installed.toml");
        state.save(&state_path).expect("seed state save");

        let plan = LifecyclePlan::for_component_uninstall("agentsight", &state);
        // Sanity: the plan has zero remove file actions.
        assert!(
            plan.components.iter().all(|c| c
                .files
                .iter()
                .all(|f| f.action != FileActionKind::Remove)
                && c.configs.iter().all(|f| f.action != FileActionKind::Remove)),
            "test premise broken: plan unexpectedly contains a Remove action",
        );

        let outcome =
            execute_plan(&plan, &layout, "tester", "system").expect("must succeed cleanly");
        assert!(outcome.removed_files.is_empty());
        assert!(outcome.state_object_removed);

        let after = InstalledState::load(&state_path).expect("reload state");
        assert!(
            after
                .find_object(ObjectKind::Component, "agentsight")
                .is_none(),
            "component must be dropped",
        );

        let lines = read_log_lines(&layout.central_log);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].get("status").and_then(|v| v.as_str()), Some("ok"),);
    }

    /// Legacy `kind = "capability"` rows from older releases must be
    /// pruned on the uninstall state write, surfaced as an outcome
    /// warning, and audited with a warn-severity central-log record.
    #[test]
    fn uninstall_execute_prunes_legacy_capability_objects() {
        let root = tempdir().expect("tempdir");
        let layout = fixture_layout(root.path());
        std_fs::create_dir_all(&layout.state_dir).expect("mkdir state");

        let mut state = InstalledState::default();
        state.upsert_object(InstalledObject {
            kind: ObjectKind::Component,
            name: "agentsight".to_string(),
            version: "0.2.0".to_string(),
            status: ObjectStatus::Installed,
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
            files: Vec::new(),
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
        });
        state.upsert_object(InstalledObject {
            kind: ObjectKind::Capability,
            name: "agent-observability".to_string(),
            version: "0.1.0".to_string(),
            status: ObjectStatus::Installed,
            manifest_digest: None,
            distribution_source: None,
            install_backend: None,
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
        });
        let state_path = layout.state_dir.join("installed.toml");
        state.save(&state_path).expect("seed state save");

        let plan = LifecyclePlan::for_component_uninstall("agentsight", &state);
        let outcome =
            execute_plan(&plan, &layout, "tester", "system").expect("must succeed cleanly");

        assert!(
            outcome
                .warnings
                .iter()
                .any(|w| w.contains("legacy capability") && w.contains("agent-observability")),
            "outcome must surface the prune as a warning: {:?}",
            outcome.warnings,
        );

        let after = InstalledState::load(&state_path).expect("reload state");
        assert!(
            after.objects.is_empty(),
            "both the component and the legacy capability row must be gone",
        );

        let lines = read_log_lines(&layout.central_log);
        assert!(
            lines.iter().any(|l| {
                l.get("severity").and_then(|v| v.as_str()) == Some("warn")
                    && l.get("message")
                        .and_then(|v| v.as_str())
                        .is_some_and(|m| m.contains("legacy capability"))
            }),
            "central log must carry a warn-severity prune record",
        );
    }

    /// A forged or stale `installed.toml` claims `owner = anolisa` for a
    /// path that lives outside the current FsLayout's owned roots. The
    /// executor must refuse to delete it (otherwise uninstall becomes an
    /// arbitrary-delete primitive), record a Skipped journal step, leave
    /// the file untouched on disk, and still proceed to drop the
    /// component object from state.
    #[test]
    fn uninstall_execute_refuses_forged_owner_outside_owned_roots() {
        let root = tempdir().expect("tempdir prefix");
        let outside = tempdir().expect("tempdir outside");
        let layout = fixture_layout(root.path());
        std_fs::create_dir_all(&layout.state_dir).expect("mkdir state");

        // Plant a real file outside the prefix to stand in for an
        // attacker-chosen target (e.g. `/etc/shadow`). It must still
        // exist after uninstall.
        let victim = outside.path().join("victim");
        std_fs::write(&victim, b"do not touch").expect("write victim");

        let mut state = InstalledState::default();
        state.upsert_object(InstalledObject {
            kind: ObjectKind::Component,
            name: "agentsight".to_string(),
            version: "0.2.0".to_string(),
            status: ObjectStatus::Installed,
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
                path: victim.clone(),
                owner: StateFileOwner::Anolisa,
                sha256: Some("0".repeat(64)),
            }],
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
        });
        let state_path = layout.state_dir.join("installed.toml");
        state.save(&state_path).expect("seed state save");

        let plan = LifecyclePlan::for_component_uninstall("agentsight", &state);
        let outcome =
            execute_plan(&plan, &layout, "tester", "system").expect("uninstall must succeed");

        assert!(
            victim.exists(),
            "forged-owner path outside owned roots must NOT be deleted",
        );
        assert!(
            outcome.removed_files.is_empty(),
            "no files should be reported as removed",
        );
        assert!(
            outcome.skipped_files.iter().any(|p| p == &victim),
            "forged path must surface in skipped_files",
        );
        assert!(outcome.state_object_removed);

        let after = InstalledState::load(&state_path).expect("reload state");
        assert!(
            after
                .find_object(ObjectKind::Component, "agentsight")
                .is_none(),
            "component must still be dropped",
        );

        let journal_dir = layout.state_dir.join("journal");
        let journal_path = journal_dir.join(format!("{}.journal.toml", outcome.operation_id));
        let tx = crate::transaction::Transaction::load_journal(&journal_path)
            .expect("journal must round-trip");
        assert_eq!(
            tx.status,
            crate::transaction::TransactionOutcomeStatus::Ok,
            "boundary skip must not flip terminal status",
        );
        let skipped = tx
            .steps
            .iter()
            .find(|s| s.target == victim.display().to_string())
            .expect("journal must record the forged path");
        assert_eq!(skipped.status, TransactionStepStatus::Skipped);
        let note = skipped.note.as_deref().unwrap_or("");
        assert!(
            note.contains("ANOLISA-owned roots"),
            "skip note must explain the boundary refusal, got {note:?}",
        );

        let lines = read_log_lines(&layout.central_log);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].get("status").and_then(|v| v.as_str()), Some("ok"));
    }

    /// `prepare_backup` must refuse to overwrite a pre-existing file at
    /// the backup leaf — `O_CREAT|O_EXCL` is what makes the backup the
    /// rollback's single source of truth, so a stale or hostile file
    /// already sitting at `<backup_root>/<idx>.bak` must fail the open
    /// rather than be silently replaced.
    #[test]
    fn prepare_backup_refuses_existing_backup_leaf() {
        let tmp = tempdir().expect("tempdir");
        let src = tmp.path().join("src");
        std_fs::write(&src, b"payload").expect("write src");
        let backup = tmp.path().join("backup.bak");
        std_fs::write(&backup, b"stale").expect("write stale backup");

        let err = prepare_backup(&src, &backup).expect_err("must refuse existing backup leaf");
        assert!(
            matches!(err, LifecycleError::Filesystem { ref path, .. } if path == &backup),
            "expected Filesystem error pointing at backup leaf, got {err:?}",
        );
        // Existing bytes preserved — we did not silently overwrite.
        let after = std_fs::read(&backup).expect("read backup");
        assert_eq!(after, b"stale");
    }

    /// A symlink planted at the backup leaf must fail the open instead
    /// of being followed. Without `O_NOFOLLOW`, an attacker who can
    /// write inside the backup root could redirect the backup writes
    /// onto an arbitrary file.
    #[test]
    #[cfg(unix)]
    fn prepare_backup_refuses_symlink_at_backup_leaf() {
        let tmp = tempdir().expect("tempdir");
        let src = tmp.path().join("src");
        std_fs::write(&src, b"payload").expect("write src");
        let victim = tmp.path().join("victim");
        std_fs::write(&victim, b"untouched").expect("write victim");
        let backup = tmp.path().join("backup.bak");
        std::os::unix::fs::symlink(&victim, &backup).expect("plant symlink");

        let err = prepare_backup(&src, &backup).expect_err("must refuse symlink at backup leaf");
        assert!(
            matches!(err, LifecycleError::Filesystem { ref path, .. } if path == &backup),
            "expected Filesystem error pointing at backup leaf, got {err:?}",
        );
        // Victim must NOT have been written to via the symlink.
        assert_eq!(std_fs::read(&victim).expect("read victim"), b"untouched");
    }

    /// A symlink at the source path is backed up as a *link* — the
    /// referent path is reproduced and its bytes are never read through,
    /// so a link pointing at content outside the owned roots cannot leak
    /// those bytes into the backup as if they belonged to the owned file.
    #[test]
    #[cfg(unix)]
    fn prepare_backup_copies_symlink_as_link() {
        let tmp = tempdir().expect("tempdir");
        let target = tmp.path().join("target");
        std_fs::write(&target, b"target bytes").expect("write target");
        let src = tmp.path().join("src");
        std::os::unix::fs::symlink(&target, &src).expect("plant src symlink");
        let backup = tmp.path().join("backup.bak");

        let artifact = prepare_backup(&src, &backup)
            .expect("backup ok")
            .expect("src exists");
        assert!(
            artifact.into_sha256().is_none(),
            "symlink backup must not record a byte hash"
        );
        let meta = std_fs::symlink_metadata(&backup).expect("backup exists");
        assert!(meta.file_type().is_symlink(), "backup must be a link");
        assert_eq!(std_fs::read_link(&backup).expect("read_link"), target);
    }

    /// A pre-placed file at the backup leaf must fail the symlink backup
    /// the same way `create_new` protects the regular-file branch.
    #[test]
    #[cfg(unix)]
    fn prepare_backup_symlink_refuses_existing_backup_leaf() {
        let tmp = tempdir().expect("tempdir");
        let target = tmp.path().join("target");
        std_fs::write(&target, b"target bytes").expect("write target");
        let src = tmp.path().join("src");
        std::os::unix::fs::symlink(&target, &src).expect("plant src symlink");
        let backup = tmp.path().join("backup.bak");
        std_fs::write(&backup, b"stale").expect("write stale backup");

        let err = prepare_backup(&src, &backup).expect_err("must refuse existing backup leaf");
        assert!(
            matches!(err, LifecycleError::Filesystem { ref path, .. } if path == &backup),
            "expected Filesystem error pointing at backup leaf, got {err:?}",
        );
        assert_eq!(std_fs::read(&backup).expect("read backup"), b"stale");
    }

    /// Once `state.save` and the `succeeded` log have landed, the
    /// uninstall is observable and on-disk-correct. A subsequent
    /// journal-finalize failure must NOT flip the wire result to
    /// `EXECUTION_FAILED` (which would tell automation "uninstall
    /// failed" on a system that is in fact uninstalled). Instead the
    /// helper records a warning and emits a `warn`-severity central log
    /// record — leaving the caller free to return `Ok` with the
    /// warning surfaced on `LifecycleOutcome.warnings`.
    #[test]
    fn finalize_journal_with_warnings_records_warning_when_finish_fails() {
        let tmp = tempdir().expect("tempdir");
        let state_dir = tmp.path().join("state");
        std_fs::create_dir_all(&state_dir).expect("mkdir state");
        let journal_dir = state_dir.join("journal");
        std_fs::create_dir_all(&journal_dir).expect("mkdir journal");
        let state_path = state_dir.join("installed.toml");

        let mut tx = crate::transaction::Transaction::begin("uninstall", state_path, &journal_dir)
            .expect("tx begin");

        // Force `tx.finish().persist()` to fail by repointing the
        // journal at a path whose parent is a regular file — the
        // `create_dir_all(parent)` inside `persist` then errors with
        // `NotADirectory`.
        let blocker = tmp.path().join("blocker");
        std_fs::write(&blocker, b"not a dir").expect("plant blocker");
        tx.journal_path = blocker.join("inner").join("journal.toml");

        let central_log_path = tmp.path().join("central.jsonl");
        let central = CentralLog::open(central_log_path.clone());

        let warnings = finalize_journal_with_warnings(
            &mut tx,
            &central,
            "op-test",
            "uninstall agentsight",
            "tester",
            "system",
            "2026-06-02T00:00:00Z",
            &["agentsight".to_string()],
        );

        assert_eq!(warnings.len(), 1, "expected exactly one warning");
        assert!(
            warnings[0].contains("journal finalize failed"),
            "warning text must explain the cause, got {:?}",
            warnings[0],
        );

        let lines = read_log_lines(&central_log_path);
        assert_eq!(lines.len(), 1, "central log must capture the warning");
        assert_eq!(
            lines[0].get("severity").and_then(|v| v.as_str()),
            Some("warn"),
        );
        assert_eq!(
            lines[0].get("operation_id").and_then(|v| v.as_str()),
            Some("op-test"),
        );
    }

    /// Streaming-hash sanity: a multi-chunk file's recorded sha matches
    /// the canonical sha256 of its bytes, and the backup contents are
    /// byte-identical to the source. Guards against off-by-one read
    /// loops.
    #[test]
    fn prepare_backup_streams_large_file_with_correct_sha() {
        let tmp = tempdir().expect("tempdir");
        let src = tmp.path().join("src");
        // Bigger than one read buffer (64 KiB) to exercise the loop.
        let payload: Vec<u8> = (0..200_000).map(|i| (i % 251) as u8).collect();
        std_fs::write(&src, &payload).expect("write src");
        let backup = tmp.path().join("nested").join("backup.bak");

        let sha = prepare_backup(&src, &backup)
            .expect("backup ok")
            .expect("expected sha for existing src")
            .into_sha256()
            .expect("regular file backup records a sha");

        let mut hasher = Sha256::new();
        hasher.update(&payload);
        let expected: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(sha, expected);
        assert_eq!(std_fs::read(&backup).expect("read backup"), payload);
    }
}
