//! Lifecycle plan for `uninstall` / `purge` of components.
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
//! `--json` without performing any IO. Execution lives in the new
//! planner/executor pipeline (`planner` + `owned_executor` /
//! `executor`); `purge` remains plan-only until manifest-driven
//! config/cache/state discovery lands.
//!
//! # Scope guarantees (hard rules)
//!
//! * `Uninstall` — removes only files where `owner ==
//!   FileOwner::Anolisa`; everything else is skipped or refused.
//! * `Purge` — `Uninstall` semantics + drops ANOLISA-owned config / cache
//!   fragments. `external_modified_files` always
//!   [`FileActionKind::Refuse`].
//!
//! This module also hosts [`prepare_backup`], the hardened
//! backup-to-rollback primitive shared with the owned executor's port
//! implementations.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::domain::{InstallationScope, ProviderBinding};
use crate::hooks::HookSpec;
use crate::manifest::ServiceScope;
use crate::state::{
    ExternalModifiedFile, FileOwner as StateFileOwner, ObjectKind, OwnedFile, ServiceRef,
};
use crate::state_store::StateStore;

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
    /// Manager scope, carried from the installed `ServiceRef` so the
    /// uninstall executor can drive user units via `systemctl --user`.
    #[serde(default)]
    pub scope: ServiceScope,
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
    pub fn for_component_uninstall(component: &str, store: &StateStore) -> Self {
        Self::build(
            LifecycleOperation::Uninstall,
            LifecycleTargetKind::Component,
            component,
            store,
        )
    }

    /// Build a `Purge` plan: `Uninstall` + remove ANOLISA-owned
    /// `etc_dir` / `cache_dir` / `state_dir` fragments. External
    /// modifications stay [`FileActionKind::Refuse`]. Execution remains
    /// gated by the purge guard.
    pub fn for_component_purge(component: &str, store: &StateStore) -> Self {
        Self::build(
            LifecycleOperation::Purge,
            LifecycleTargetKind::Component,
            component,
            store,
        )
    }

    fn build(
        operation: LifecycleOperation,
        target_kind: LifecycleTargetKind,
        target: &str,
        store: &StateStore,
    ) -> Self {
        let target_obj = store.find(ObjectKind::Component, target);
        let target_scope = target_obj.map(|installation| installation.scope);

        let mut components: Vec<ComponentLifecyclePlan> = Vec::new();
        let mut warnings: Vec<String> = Vec::new();

        if let Some(installation) = target_obj {
            // Only owned artifacts carry files/services the plan can act
            // on; a delegated record contributes nothing but its hooks.
            let (owned_files, external_files, service_refs): (
                &[OwnedFile],
                &[ExternalModifiedFile],
                &[ServiceRef],
            ) = match &installation.binding {
                ProviderBinding::Owned { artifact } => (
                    &artifact.files,
                    &artifact.external_modified_files,
                    &artifact.services,
                ),
                ProviderBinding::Delegated { .. } => (&[], &[], &[]),
            };
            let mut files: Vec<FileAction> = plan_owned_files(owned_files);
            files.extend(plan_external_files(external_files));
            let configs = if operation == LifecycleOperation::Purge {
                plan_purge_configs(owned_files)
            } else {
                Vec::new()
            };
            components.push(ComponentLifecyclePlan {
                name: target.to_string(),
                services: plan_services(service_refs),
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

        let phases = build_phases(operation, target, target_scope, &components);

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
            scope: s.scope,
            reason: Some(
                "stops and disables via systemd; user-scope units via `systemctl --user`; skipped on non-linux/container hosts"
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
            // The plan is built from installed state, which does not carry
            // the component contract, so build() cannot tell here whether a
            // script is declared for this phase — the executor resolves that
            // from the installed manifest at run time. Preview it as Execute
            // with a reason that names the condition.
            name: (*n).to_string(),
            mode: LifecycleMode::Execute,
            reason: Some(
                "runs the contract [[component.hooks]] script for this phase when declared"
                    .to_string(),
            ),
        })
        .collect()
}

fn build_phases(
    operation: LifecycleOperation,
    component: &str,
    scope: Option<InstallationScope>,
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
                        scope.map(|scope| scoped_lifecycle_command(scope, "repair", &c.name))
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
    // State-record removal is the one phase every *installed* target ends
    // with. When the target is absent, `components` is empty and the plan is
    // genuinely empty (see the "not installed — plan is empty" warning in
    // `build`); appending `remove_state` here would report a phantom removal
    // that contradicts that warning, so gate it on a present component.
    if !components.is_empty() {
        phases.push(LifecyclePhase {
            name: "remove_state".to_string(),
            action: "remove_object".to_string(),
            target: component.to_string(),
            mode: LifecycleMode::Execute,
            rollback_hint: scope.map(|scope| scoped_lifecycle_command(scope, "install", component)),
        });
    }

    phases
}

fn scoped_lifecycle_command(scope: InstallationScope, operation: &str, component: &str) -> String {
    match scope {
        InstallationScope::System => {
            format!("sudo anolisa --install-mode system {operation} {component}")
        }
        InstallationScope::User { .. } => {
            format!("anolisa --install-mode user {operation} {component}")
        }
    }
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

/// Failure surface for lifecycle planning and backup primitives.
#[derive(Debug, thiserror::Error)]
pub enum LifecycleError {
    /// Filesystem mutation failed while deleting or restoring a path.
    #[error("filesystem io failed for {path}: {source}")]
    Filesystem {
        /// Path involved in the failed filesystem operation.
        path: PathBuf,
        /// Original I/O error from the OS.
        #[source]
        source: std::io::Error,
    },
}

/// Contract-driven lifecycle hooks the caller pre-resolved from the
/// installed component manifest, grouped by phase.
///
/// The executor takes these as input rather than discovering them itself:
/// the CLI layer owns the installed-manifest path convention and reads back
/// each component's `[[component.hooks]]` (placeholder expansion + the real
/// `strict`/`timeout` already applied by
/// [`resolve_manifest_hooks`](crate::hooks::resolve_manifest_hooks)). A
/// caller with no manifest snapshot (older installs, RPM-delegated paths)
/// passes the [`Default`] empty value and the uninstall simply runs no
/// hooks.
#[derive(Debug, Default)]
pub struct ResolvedLifecycleHooks {
    /// Hooks to run before service-stop and file removal. A `strict = true`
    /// hook that fails aborts the uninstall and rolls back; `strict = false`
    /// (e.g. ws-ckpt's recover) only warns.
    pub pre_uninstall: Vec<HookSpec>,
    /// Hooks to run after the lock is released and removal has committed.
    /// Always best-effort — failures only warn.
    pub post_uninstall: Vec<HookSpec>,
}

/// What [`prepare_backup`] wrote at the backup path.
#[derive(Debug)]
pub enum BackupArtifact {
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
    pub fn into_sha256(self) -> Option<String> {
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
pub fn prepare_backup(src: &Path, backup: &Path) -> Result<Option<BackupArtifact>, LifecycleError> {
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

#[cfg(test)]
mod tests {
    use super::*;

    use crate::domain::InstallationScope;
    use crate::state::{
        ExternalModifiedFile, FileOwner as StateFileOwner, InstalledObject, InstalledState,
        ObjectKind, ObjectStatus, OwnedFile, OwnedFileKind, ServiceRef,
    };
    use anolisa_platform::fs_layout::FsLayout;
    use std::fs as std_fs;
    use std::path::Path;
    use tempfile::tempdir;

    fn fixture_layout(prefix: &Path) -> FsLayout {
        FsLayout::system(Some(prefix.to_path_buf()))
    }

    /// Legacy fixture kept as migration coverage: seed a v4 object and
    /// migrate it into a v5 store the planner consumes.
    fn seed_state_with_two_files(
        layout: &FsLayout,
        component: &str,
        owned_path: &Path,
        external_path: &Path,
    ) -> StateStore {
        std_fs::create_dir_all(&layout.state_dir).expect("mkdir state");
        let mut state = InstalledState::default();
        state.upsert_object(InstalledObject {
            kind: ObjectKind::Component,
            name: component.to_string(),
            version: "0.2.0".to_string(),
            status: ObjectStatus::Installed,
            manifest_digest: None,
            distribution_source: Some("file:///fake".to_string()),
            raw_package: None,
            install_backend: Some("raw".to_string()),
            ownership: None,
            rpm_metadata: None,
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
                kind: OwnedFileKind::File,
                referent: None,
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
                scope: ServiceScope::System,
            }],
            health: Vec::new(),
            provisioned_packages: Vec::new(),
        });
        state
            .save(&layout.state_dir.join("installed.toml"))
            .expect("seed state save");
        let migration =
            crate::state_migration::migrate_state(&state.objects, InstallationScope::System);
        assert!(
            migration.quarantined.is_empty(),
            "fixtures must migrate cleanly"
        );
        let mut store = StateStore::empty();
        store.installations = migration.active;
        store
    }

    #[test]
    fn plan_services_carries_scope_from_service_ref() {
        let refs = vec![
            ServiceRef {
                name: "agentsight.service".to_string(),
                manager: "systemd".to_string(),
                restartable: true,
                enabled: true,
                scope: ServiceScope::System,
            },
            ServiceRef {
                name: "anolisa-memory@alice.service".to_string(),
                manager: "systemd-user".to_string(),
                restartable: false,
                enabled: false,
                scope: ServiceScope::User,
            },
        ];
        let actions = plan_services(&refs);
        assert!(matches!(actions[0].scope, ServiceScope::System));
        assert!(matches!(actions[1].scope, ServiceScope::User));
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

        let remove_file = plan
            .phases
            .iter()
            .find(|phase| phase.name == "remove_file")
            .expect("remove_file phase");
        assert_eq!(
            remove_file.rollback_hint.as_deref(),
            Some("sudo anolisa --install-mode system repair agentsight"),
        );
        let remove_state = plan
            .phases
            .iter()
            .find(|phase| phase.name == "remove_state")
            .expect("remove_state phase");
        assert_eq!(
            remove_state.rollback_hint.as_deref(),
            Some("sudo anolisa --install-mode system install agentsight"),
        );
    }

    #[test]
    fn recovery_hint_preserves_user_scope() {
        assert_eq!(
            scoped_lifecycle_command(InstallationScope::User { uid: 1000 }, "repair", "cosh",),
            "anolisa --install-mode user repair cosh",
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

    /// #1471: an absent target must yield a *genuinely* empty plan — the
    /// "not installed" warning present, and neither a component slice nor
    /// any phase emitted. Guards the self-contradiction where the warning
    /// said "plan is empty" while a phantom `remove_state` phase remained.
    #[test]
    fn uninstall_absent_component_yields_empty_components_and_phases() {
        let empty = StateStore::empty();
        let plan = LifecyclePlan::for_component_uninstall("agentsight", &empty);

        assert!(
            plan.components.is_empty(),
            "absent component must produce no component slice",
        );
        assert!(
            plan.phases.is_empty(),
            "absent component must produce no phases (not even remove_state): {:?}",
            plan.phases,
        );
        assert!(
            plan.warnings
                .iter()
                .any(|w| w.contains("is not installed") && w.contains("plan is empty")),
            "the not-installed warning must be retained: {:?}",
            plan.warnings,
        );
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
