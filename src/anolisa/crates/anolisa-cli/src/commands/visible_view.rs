//! Visible installed-state merge model.
//!
//! Read-only view that merges user-scope and system-scope `installed.toml`
//! so that read commands (`list`, `status`) show every component visible
//! to the current user — regardless of which physical scope owns the record.
//!
//! ## Merge rules
//!
//! - User state has higher priority than system state.
//! - When the same component name exists in both scopes, the user record is
//!   **active** and the system record is **shadowed**.
//! - System state is always read best-effort: a missing or unreadable file
//!   is not an error — it simply yields no system records.
//! - `mutable_by_current_user` is `true` when the record's scope matches the
//!   current [`CliContext`] install mode, meaning the user can directly
//!   modify that record through a mutation command.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anolisa_core::state::{InstalledObject, InstalledState, ObjectKind};
use anolisa_platform::fs_layout::FsLayout;
use serde::Serialize;

use crate::context::{CliContext, InstallMode};
use crate::packaged;

/// Physical scope of an installed record.
///
/// Maps 1:1 to the on-disk `installed.toml` file the record came from:
/// `user` → `~/.local/state/anolisa/installed.toml`,
/// `system` → `/var/lib/anolisa/installed.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// Per-user (`file-hierarchy(7)`) install scope.
    User,
    /// System-wide FHS install scope.
    System,
}

impl Scope {
    /// Wire label (`"user"` or `"system"`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Scope::User => "user",
            Scope::System => "system",
        }
    }
}

/// Convert a CLI [`InstallMode`] to a [`Scope`].
fn install_mode_to_scope(mode: InstallMode) -> Scope {
    match mode {
        InstallMode::User => Scope::User,
        InstallMode::System => Scope::System,
    }
}

/// One visible record in the merged view, corresponding to a single
/// `(component, scope)` pair.
///
/// If the same component exists in both scopes, the user-scope record has
/// `active = true` and the system-scope record has
/// `active = false, shadowed_by = Some(Scope::User)`.
#[derive(Debug, Clone)]
pub struct VisibleRecord {
    /// Canonical component name.
    pub component: String,
    /// Physical scope this record comes from.
    pub scope: Scope,
    /// Path to the `installed.toml` file this record was loaded from.
    pub state_path: PathBuf,
    /// The underlying installed object from state.
    pub object: InstalledObject,
    /// Whether this is the active (highest-priority) record for the
    /// component. `true` for the user record when both scopes have the
    /// component; `true` for the sole record when only one scope has it.
    pub active: bool,
    /// When `active = false`, which scope's record shadows this one.
    pub shadowed_by: Option<Scope>,
    /// Whether the current user can directly modify this record through a
    /// mutation command. `true` when the record's scope matches
    /// `ctx.install_mode`.
    pub mutable_by_current_user: bool,
    /// Read-only datadir roots where component contracts / adapter payloads
    /// for this scope can be found. Does not imply the current user owns
    /// these files.
    #[allow(dead_code)] // Phase 2/3 will consume this for payload discovery
    pub payload_roots: Vec<PathBuf>,
}

/// Merged read-only view of user-scope and system-scope install state.
///
/// Built once per read command invocation via [`VisibleInstalledView::load`].
/// Commands query the view through [`records`](Self::records),
/// [`active`](Self::active), and
/// [`records_for`](Self::records_for) rather than loading
/// `installed.toml` themselves.
///
/// System-state read failures are non-fatal: the view is constructed with
/// whatever state was readable, and [`system_state_readable`] reports
/// whether the system `installed.toml` was successfully loaded.
///
/// [`system_state_readable`]: Self::system_state_readable
#[derive(Debug)]
pub struct VisibleInstalledView {
    records: Vec<VisibleRecord>,
    system_state_readable: bool,
    system_state_path: Option<PathBuf>,
}

impl VisibleInstalledView {
    /// Load both user-scope and system-scope state and merge them.
    ///
    /// User state is loaded from the user layout's `state_dir`; system state
    /// from the system layout's `state_dir`. A missing or unreadable system
    /// state file is not an error — it simply produces no system records.
    pub fn load(ctx: &CliContext) -> Self {
        let env = anolisa_env::EnvService::detect();
        let user_layout = FsLayout::user(env.home);
        let system_layout = FsLayout::system(ctx.prefix.clone());

        let user_state = load_state_from(&user_layout.state_dir);
        let (system_state, system_readable) = load_system_state(&system_layout.state_dir);

        let user_payload_roots = collect_payload_roots(&user_layout);
        let system_payload_roots = collect_payload_roots(&system_layout);

        let user_state_path = user_layout.state_dir.join("installed.toml");
        let system_state_path = system_layout.state_dir.join("installed.toml");

        let mut records = Vec::new();

        // Collect component names from both scopes (user first for stable
        // ordering).
        let mut seen_names: HashSet<String> = HashSet::new();

        // User records first — they have higher priority.
        for obj in user_state
            .objects
            .iter()
            .filter(|o| o.kind == ObjectKind::Component)
        {
            seen_names.insert(obj.name.clone());
            let active = true; // user record is always active unless a later
            // pass finds a conflict (it can't — user wins).
            records.push(VisibleRecord {
                component: obj.name.clone(),
                scope: Scope::User,
                state_path: user_state_path.clone(),
                object: obj.clone(),
                active,
                shadowed_by: None,
                mutable_by_current_user: install_mode_to_scope(ctx.install_mode) == Scope::User,
                payload_roots: user_payload_roots.clone(),
            });
        }

        // System records — shadowed when the same name exists in user state.
        for obj in system_state
            .objects
            .iter()
            .filter(|o| o.kind == ObjectKind::Component)
        {
            let shadowed = seen_names.contains(&obj.name);
            records.push(VisibleRecord {
                component: obj.name.clone(),
                scope: Scope::System,
                state_path: system_state_path.clone(),
                object: obj.clone(),
                active: !shadowed,
                shadowed_by: shadowed.then_some(Scope::User),
                mutable_by_current_user: install_mode_to_scope(ctx.install_mode) == Scope::System,
                payload_roots: system_payload_roots.clone(),
            });
        }

        Self {
            records,
            system_state_readable: system_readable,
            system_state_path: system_readable.then_some(system_state_path),
        }
    }

    /// Build a view from explicit state objects — used in tests and by
    /// [`load_with_current_state`].
    ///
    /// `user_state_path` / `system_state_path` are recorded on each record
    /// for diagnostics. `system_readable` indicates whether the system
    /// state file was loadable.
    pub(crate) fn from_states(
        user_state: &InstalledState,
        system_state: &InstalledState,
        user_state_path: PathBuf,
        system_state_path: PathBuf,
        system_readable: bool,
        ctx: &CliContext,
    ) -> Self {
        let mut records = Vec::new();
        let mut seen_names: HashSet<String> = HashSet::new();

        for obj in user_state
            .objects
            .iter()
            .filter(|o| o.kind == ObjectKind::Component)
        {
            seen_names.insert(obj.name.clone());
            records.push(VisibleRecord {
                component: obj.name.clone(),
                scope: Scope::User,
                state_path: user_state_path.clone(),
                object: obj.clone(),
                active: true,
                shadowed_by: None,
                mutable_by_current_user: install_mode_to_scope(ctx.install_mode) == Scope::User,
                payload_roots: Vec::new(),
            });
        }

        for obj in system_state
            .objects
            .iter()
            .filter(|o| o.kind == ObjectKind::Component)
        {
            let shadowed = seen_names.contains(&obj.name);
            records.push(VisibleRecord {
                component: obj.name.clone(),
                scope: Scope::System,
                state_path: system_state_path.clone(),
                object: obj.clone(),
                active: !shadowed,
                shadowed_by: shadowed.then_some(Scope::User),
                mutable_by_current_user: install_mode_to_scope(ctx.install_mode) == Scope::System,
                payload_roots: Vec::new(),
            });
        }

        Self {
            records,
            system_state_readable: system_readable,
            system_state_path: system_readable.then_some(system_state_path),
        }
    }

    /// Load the view, using a pre-loaded current-scope state instead of
    /// reading it from disk.
    ///
    /// Used by `list` and `status` which perform v3 symlink migration on
    /// the in-memory state before rendering. The other scope's state is
    /// loaded from disk and also migrated so that cross-scope pre-v4
    /// symlink entries are upgraded before the integrity probe.
    pub fn load_with_current_state(ctx: &CliContext, current_state: &InstalledState) -> Self {
        let env = anolisa_env::EnvService::detect();
        let user_layout = FsLayout::user(env.home);
        let system_layout = FsLayout::system(ctx.prefix.clone());

        let user_state_path = user_layout.state_dir.join("installed.toml");
        let system_state_path = system_layout.state_dir.join("installed.toml");

        match install_mode_to_scope(ctx.install_mode) {
            Scope::User => {
                let (mut system_state, system_readable) =
                    load_system_state(&system_layout.state_dir);
                if system_readable {
                    crate::commands::common::migrate_v3_symlinks(&mut system_state, &system_layout);
                }
                Self::from_states(
                    current_state,
                    &system_state,
                    user_state_path,
                    system_state_path,
                    system_readable,
                    ctx,
                )
            }
            Scope::System => {
                let mut user_state = load_state_from(&user_layout.state_dir);
                crate::commands::common::migrate_v3_symlinks(&mut user_state, &user_layout);
                Self::from_states(
                    &user_state,
                    current_state,
                    user_state_path,
                    system_state_path,
                    true,
                    ctx,
                )
            }
        }
    }

    /// All visible records (both scopes, active and shadowed).
    #[allow(dead_code)] // Phase 2 will consume this for scope guards
    pub fn records(&self) -> &[VisibleRecord] {
        &self.records
    }

    /// Only active records (one per component — the highest-priority scope).
    pub fn active_records(&self) -> impl Iterator<Item = &VisibleRecord> {
        self.records.iter().filter(|r| r.active)
    }

    /// The active record for `component`, if any.
    ///
    /// User scope takes priority over system scope.
    pub fn active(&self, component: &str) -> Option<&VisibleRecord> {
        self.records
            .iter()
            .find(|r| r.active && r.component == component)
    }

    /// All records for `component` across all scopes (active first).
    pub fn records_for(&self, component: &str) -> Vec<&VisibleRecord> {
        let mut matches: Vec<&VisibleRecord> = self
            .records
            .iter()
            .filter(|r| r.component == component)
            .collect();
        // Sort: active first, then by scope priority (User before System).
        matches.sort_by(|a, b| b.active.cmp(&a.active).then_with(|| a.scope.cmp(&b.scope)));
        matches
    }

    /// Whether the system `installed.toml` was successfully read.
    ///
    /// `false` when the file is missing, unreadable, or failed to parse.
    /// Read commands should emit a warning when this is `false`.
    pub fn system_state_readable(&self) -> bool {
        self.system_state_readable
    }

    /// Path to the system state file, if it was readable.
    pub fn system_state_path(&self) -> Option<&Path> {
        self.system_state_path.as_deref()
    }
}

// ── Loading helpers ──────────────────────────────────────────────────

/// Load `installed.toml` from a state directory.
///
/// A missing file yields `Default` (fresh-install case). A parse or IO
/// error also yields `Default` — callers rely on the merge producing
/// an empty record set rather than propagating errors.
fn load_state_from(state_dir: &Path) -> InstalledState {
    let path = state_dir.join("installed.toml");
    InstalledState::load(&path).unwrap_or_default()
}

/// Load system-scope `installed.toml`.
///
/// Returns `(state, was_readable)`. When the file is missing or fails to
/// load, returns `(Default, false)`.
fn load_system_state(system_state_dir: &Path) -> (InstalledState, bool) {
    let path = system_state_dir.join("installed.toml");
    if !path.exists() {
        return (InstalledState::default(), false);
    }
    match InstalledState::load(&path) {
        Ok(state) => (state, true),
        Err(_) => (InstalledState::default(), false),
    }
}

/// Collect datadir roots for payload discovery from a layout.
///
/// Includes the layout's primary `datadir` and, for system layouts, the
/// FHS package-managed datadir (`/usr/share/anolisa`).
fn collect_payload_roots(layout: &FsLayout) -> Vec<PathBuf> {
    let mut roots = vec![layout.datadir.clone()];
    if let Some(pkg) = packaged::packaged_datadir_root(layout) {
        if !roots.contains(&pkg) {
            roots.push(pkg);
        }
    }
    if let Some(pkg_dd) = layout.package_datadir() {
        if !roots.contains(&pkg_dd) {
            roots.push(pkg_dd);
        }
    }
    roots
}

// ── Mutation scope guard ───────────────────────────────────────────

/// The kind of mutation being attempted, used by
/// [`resolve_mutation_target`] to decide whether a missing component is
/// an error or a legitimate creation path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Install/Repair/Adopt used by future phases; Update/Uninstall/Forget are live
pub enum MutationOperation {
    /// `install` — may create a new record in the current scope.
    Install,
    /// `update` — requires an existing record in the current scope.
    Update,
    /// `uninstall` — requires an existing record in the current scope.
    Uninstall,
    /// `forget` — requires an existing record in the current scope.
    Forget,
    /// `repair` — requires an existing record in the current scope.
    Repair,
    /// `adopt` — system scope only; creates a new record.
    Adopt,
}

impl MutationOperation {
    /// Human-readable verb for error messages.
    const fn verb(self) -> &'static str {
        match self {
            MutationOperation::Install => "install",
            MutationOperation::Update => "update",
            MutationOperation::Uninstall => "uninstall",
            MutationOperation::Forget => "forget",
            MutationOperation::Repair => "repair",
            MutationOperation::Adopt => "adopt",
        }
    }
}

/// Result of [`resolve_mutation_target`] — tells mutation commands whether
/// they may proceed, must reject with a scope hint, or should treat the
/// target as absent.
#[derive(Debug)]
#[allow(dead_code, clippy::enum_variant_names)] // CurrentScope's record is consumed by future callers
pub enum MutationTarget<'a> {
    /// The component has a record in the current scope that the current
    /// user can modify. Proceed with the mutation.
    CurrentScope(&'a VisibleRecord),
    /// The component is absent from the current scope, but the operation
    /// allows creating a new record (e.g. `install` in user mode creating
    /// a user override). Callers that do not allow creation should treat
    /// this as "not installed".
    CreateInCurrentScope,
    /// The component exists only in a different scope. The command must
    /// reject and tell the user to switch scope.
    WrongScope(&'a VisibleRecord),
}

/// Unified scope guard for mutation commands.
///
/// Checks whether `component` is mutable in the current `ctx.install_mode`
/// by consulting the merged visible view. Returns one of three results:
///
/// - [`MutationTarget::CurrentScope`] — a record exists in the current
///   scope; proceed with the mutation.
/// - [`MutationTarget::WrongScope`] — the component only exists in other
///   scopes; the command must reject with a scope-switch hint.
/// - [`MutationTarget::CreateInCurrentScope`] — the component is not in
///   any scope; `install`/`adopt` may proceed, other operations should
///   report "not installed".
///
/// When the same component exists in both user and system scope, the
/// active record might be in a different scope (user scope shadows
/// system). This function still finds the current scope's record via
/// [`VisibleInstalledView::records_for`] so that legitimate mutations
/// are not blocked by the shadowing rule.
pub fn resolve_mutation_target<'a>(
    _operation: MutationOperation,
    component: &str,
    view: &'a VisibleInstalledView,
) -> MutationTarget<'a> {
    let records = view.records_for(component);

    if records.is_empty() {
        return MutationTarget::CreateInCurrentScope;
    }

    // Find a record in the current scope (mutable_by_current_user == true).
    if let Some(record) = records.iter().copied().find(|r| r.mutable_by_current_user) {
        return MutationTarget::CurrentScope(record);
    }

    // No record in the current scope; the component only exists in other scopes.
    MutationTarget::WrongScope(records[0])
}

/// Build the error message for a [`MutationTarget::WrongScope`] result.
///
/// Produces a user-facing message that explains the scope mismatch and
/// suggests the correct invocation:
///
/// ```text
/// component 'ws-ckpt' is installed in system scope; rerun with sudo and --install-mode system
/// ```
pub fn wrong_scope_reason(operation: MutationOperation, record: &VisibleRecord) -> String {
    let verb = operation.verb();
    match record.scope {
        Scope::System => {
            format!(
                "component '{}' is installed in system scope; \
                 rerun with `sudo anolisa --install-mode system {verb} '{}'`",
                record.component, record.component
            )
        }
        Scope::User => {
            format!(
                "component '{}' is installed in user scope; \
                 rerun with `anolisa --install-mode user {verb} '{}'`",
                record.component, record.component
            )
        }
    }
}

#[cfg(test)]
mod tests;
