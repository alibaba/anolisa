//! Shared helpers for tier1 / tier2 command handlers.
//!
//! Access to the skeleton-stable command inputs: [`FsLayout`], the v5
//! [`StateStore`], and repo configuration. Keep this module thin —
//! handlers compose these calls; we do not introduce a service layer here.

use std::path::PathBuf;

use anolisa_core::ObjectKind;
use anolisa_core::adapter::manager::{AdapterManager, VisibleRoot};
use anolisa_core::domain::InstallationScope;
use anolisa_core::facts::{JournalEvidence, JournalInventory};
use anolisa_core::state_store::StateStore;
use anolisa_platform::fs_layout::FsLayout;

use crate::color::Palette;
use crate::commands::state_view::{StateView, StateVisibility};
use crate::context::{CliContext, InstallMode};
use crate::packaged;
use crate::repo_config::{RepoConfig, RepoConfigProvisioning};
use crate::response::CliError;

/// State subdirectory where install stores the exact component contract
/// used for each installed component.
const INSTALLED_COMPONENT_MANIFESTS_SUBDIR: &str = "component-manifests";
/// Filename used for the locally persisted installed component contract.
const INSTALLED_COMPONENT_MANIFEST_FILE: &str = "component.toml";

/// Build the layout for the active install mode, honoring `--prefix`
/// (system-mode) and the current process user's home (user-mode).
pub fn resolve_layout(ctx: &CliContext) -> FsLayout {
    match ctx.install_mode {
        InstallMode::System => FsLayout::system(ctx.prefix.clone()),
        InstallMode::User => {
            let home = anolisa_env::EnvService::detect().home;
            FsLayout::user(home)
        }
    }
}

/// Build a consistent package-transaction permission error.
pub(crate) fn package_permission_error(command: &str, bin: &str, action: &str) -> CliError {
    CliError::Runtime {
        command: command.to_string(),
        reason: format!("permission denied running {bin}; re-run the {action} with sudo"),
    }
}

/// Renders an explicit-scope remediation command so diagnostics never rely
/// on the caller's default installation mode.
pub(crate) fn scoped_component_command(
    scope: InstallationScope,
    operation: &str,
    component: &str,
) -> String {
    let mode = match scope {
        InstallationScope::System => InstallMode::System,
        InstallationScope::User { .. } => InstallMode::User,
    };
    scoped_component_command_for_mode(mode, operation, component)
}

pub(crate) fn scoped_component_command_for_mode(
    mode: InstallMode,
    operation: &str,
    component: &str,
) -> String {
    match mode {
        InstallMode::System => {
            format!("sudo anolisa --install-mode system {operation} {component}")
        }
        InstallMode::User => {
            format!("anolisa --install-mode user {operation} {component}")
        }
    }
}

/// Build a consistent non-zero package-transaction error.
pub(crate) fn package_transaction_failed_error(
    command: &str,
    operation: &str,
    code: Option<i32>,
    stderr: &str,
) -> CliError {
    CliError::Runtime {
        command: command.to_string(),
        reason: format!(
            "dnf {operation} failed (exit {}): {}",
            code.map(|c| c.to_string())
                .unwrap_or_else(|| "signal".to_string()),
            stderr.trim(),
        ),
    }
}

/// Render repo config provisioning performed by commands that need repo access.
fn render_repo_config_provisioning(ctx: &CliContext, provisioning: &RepoConfigProvisioning) {
    if ctx.quiet || ctx.json {
        return;
    }
    let color = Palette::new(ctx.no_color);
    // Routed through `suspend_output` so these persistent lines cannot
    // interleave with a live activity spinner (issue #1452); a no-op when no
    // spinner is running, which is the case for most callers.
    crate::progress::suspend_output(|| match provisioning {
        RepoConfigProvisioning::Existing => {}
        RepoConfigProvisioning::Downloaded { url, dest } => {
            println!(
                "{} repo config was missing; downloaded {} to {}",
                color.ok("✓"),
                url,
                color.path(dest.display().to_string()),
            );
        }
        RepoConfigProvisioning::FetchedForDryRun { url, dest } => {
            println!(
                "fetched repo config from {url} for dry-run; would write to {}",
                color.path(dest.display().to_string()),
            );
        }
        RepoConfigProvisioning::DownloadedPersistFailed { url, dest, reason } => {
            eprintln!(
                "{} repo config fetched from {} but could not write to {}: {}",
                color.warn("⚠"),
                url,
                color.path(dest.display().to_string()),
                reason,
            );
        }
    });
}

/// Controls whether a failed persistence of downloaded repo config is fatal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RepoPersistPolicy {
    /// Mutating commands: persistence failure is an error.
    Require,
    /// Read-only / best-effort commands: warn and use in-memory config.
    BestEffort,
}

/// Enforce the persist policy against a load result.
///
/// Extracted so the policy branch can be unit-tested without a real
/// network fetch or filesystem setup.
fn enforce_repo_persist_policy(
    provisioning: &RepoConfigProvisioning,
    persist_policy: RepoPersistPolicy,
    command: &str,
) -> Result<(), CliError> {
    if persist_policy == RepoPersistPolicy::Require
        && let RepoConfigProvisioning::DownloadedPersistFailed { dest, reason, .. } = provisioning
    {
        return Err(CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "repo config downloaded but could not be written to {}: {reason}",
                dest.display()
            ),
        });
    }
    Ok(())
}

/// Load `repo.toml`, provisioning it if every local source is missing.
///
/// `persist_policy` governs what happens when a freshly-downloaded config
/// cannot be written to disk:
/// - [`RepoPersistPolicy::Require`]: command fails (use for mutating ops).
/// - [`RepoPersistPolicy::BestEffort`]: warn and return the in-memory config.
pub(crate) fn load_repo_config(
    ctx: &CliContext,
    layout: &FsLayout,
    command: &str,
    persist_policy: RepoPersistPolicy,
) -> Result<RepoConfig, CliError> {
    let repo_load = RepoConfig::load(layout, ctx.dry_run).map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: err.to_string(),
    })?;
    enforce_repo_persist_policy(&repo_load.provisioning, persist_policy, command)?;
    render_repo_config_provisioning(ctx, &repo_load.provisioning);
    Ok(repo_load.config)
}

/// Load the v5 [`StateStore`] for this context, migrating a legacy state file
/// in memory. A missing file is a fresh store.
pub fn load_state_store(ctx: &CliContext, command: &str) -> Result<StateStore, CliError> {
    let layout = resolve_layout(ctx);
    let path = layout.state_dir.join("installed.toml");
    StateStore::load_for_layout(&path, anolisa_platform::privilege::effective_uid(), &layout)
        .map_err(|err| CliError::Runtime {
            command: command.to_string(),
            reason: format!(
                "failed to load installed state at {}: {err}",
                path.display()
            ),
        })
}

fn reject_visible_non_writable_component_from_view(
    view: &StateView,
    command: &str,
    component: &str,
) -> Result<(), CliError> {
    view.reject_non_writable_component_mutation(command, component)
}

/// Resolve an install target without treating a read-only system record as
/// the installation being mutated.
///
/// Exact identities across the visible user-plus-system view still precede
/// repository aliases. The returned state view keeps planning scoped to its
/// writable root, so a user install can create a user record that shadows an
/// existing system installation without inheriting or changing it.
pub(crate) fn resolve_install_target(
    input: &str,
    ctx: &CliContext,
    command: &str,
) -> Result<(String, StateView, bool), CliError> {
    resolve_lifecycle_target(input, ctx, command, false)
}

/// Resolve a lifecycle target and reject a visible installation that the
/// current invocation cannot mutate.
///
/// Missing targets are returned unchanged for the planner's normal
/// `not-installed` path. Exact component names and package identities are
/// resolved from visible state before repository aliases are considered.
pub(crate) fn resolve_mutation_target(
    input: &str,
    ctx: &CliContext,
    command: &str,
) -> Result<(String, StateView), CliError> {
    let writable_state = load_state_store(ctx, command)?;
    let layout = resolve_layout(ctx);
    let journal_dir = layout.state_dir.join("journal");
    let inventory = JournalInventory::load(JournalEvidence::new(
        &journal_dir,
        &writable_state.operations,
    ))
    .map_err(|err| CliError::Runtime {
        command: command.to_string(),
        reason: format!("failed to inspect pending operation journals: {err}"),
    })?;
    let writable_journal_identity = inventory.recoverable_for(input).is_some();
    let view = StateView::load_with_writable_state(
        ctx,
        command,
        StateVisibility::UserPlusSystem,
        writable_state,
    )?;

    if writable_journal_identity {
        return Ok((input.to_string(), view));
    }
    resolve_lifecycle_target_from_view(input, command, true, view, || {
        component_alias_from_repo_index(input, ctx, command)
    })
    .map(|(component, view, _)| (component, view))
}

/// Resolve an adapter target across visible component and local receipt
/// identities without requiring the source component's state to be writable.
pub(crate) fn resolve_adapter_target(
    input: &str,
    ctx: &CliContext,
    command: &str,
) -> Result<(String, StateView), CliError> {
    let writable_state = load_state_store(ctx, command)?;
    let view = StateView::load_with_writable_state(
        ctx,
        command,
        StateVisibility::UserPlusSystem,
        writable_state,
    )?;
    resolve_adapter_target_from_view(input, command, view, || {
        component_alias_from_repo_index(input, ctx, command)
    })
}

fn resolve_adapter_target_from_view(
    input: &str,
    command: &str,
    view: StateView,
    alias: impl FnOnce() -> String,
) -> Result<(String, StateView), CliError> {
    let exact_identity = view.has_exact_component(input)
        || !view
            .writable
            .state
            .adapter_claims_for_component(input)
            .is_empty();
    if !exact_identity {
        view.reject_incomplete_alias_visibility(command, input)?;
    }
    let component = if exact_identity {
        input.to_string()
    } else {
        alias()
    };
    Ok((component, view))
}

fn resolve_lifecycle_target(
    input: &str,
    ctx: &CliContext,
    command: &str,
    reject_read_only: bool,
) -> Result<(String, StateView, bool), CliError> {
    let writable_state = load_state_store(ctx, command)?;
    let view = StateView::load_with_writable_state(
        ctx,
        command,
        StateVisibility::UserPlusSystem,
        writable_state,
    )?;
    resolve_lifecycle_target_from_view(input, command, reject_read_only, view, || {
        component_alias_from_repo_index(input, ctx, command)
    })
}

fn resolve_lifecycle_target_from_view(
    input: &str,
    command: &str,
    reject_read_only: bool,
    view: StateView,
    alias: impl FnOnce() -> String,
) -> Result<(String, StateView, bool), CliError> {
    let exact_identity = view.has_exact_component(input);
    if reject_read_only
        && let Some(component) = view.resolve_mutation_component_identity(command, input)?
    {
        return Ok((component, view, true));
    }
    // Install is allowed to create an independent writable-scope identity,
    // but incomplete visibility cannot prove that an alias is not an exact
    // name in the missing root. Pin the literal input instead of remapping it.
    let literal_identity = exact_identity || !view.unavailable_roots.is_empty();
    let component = if literal_identity {
        input.to_string()
    } else {
        alias()
    };
    if reject_read_only {
        reject_visible_non_writable_component_from_view(&view, command, &component)?;
    }
    Ok((component, view, literal_identity))
}

/// Resolve a user-supplied component name to the stable state key.
///
/// 1. Exact state match wins — a component installed under its literal name
///    is never re-mapped.
/// 2. Otherwise the repo-side component index is consulted for package-name
///    aliases (e.g., `copilot-shell` → `cosh`) via in-memory lookup only —
///    no rpmdb/dnf queries are triggered.
/// 3. Falls back to the literal input when resolution is ambiguous or the
///    component index is unavailable.
pub(crate) fn lookup_component_name_in_store(
    input: &str,
    store: &anolisa_core::state_store::StateStore,
    ctx: &CliContext,
    command: &str,
) -> String {
    if store.contains_record(ObjectKind::Component, input) {
        return input.to_string();
    }
    component_alias_from_repo_index(input, ctx, command)
}

/// Steps 2–3 of component-name resolution: consult the repo-side component
/// index for package-name aliases, falling back to the literal input.
fn component_alias_from_repo_index(input: &str, ctx: &CliContext, command: &str) -> String {
    let layout = resolve_layout(ctx);
    let repo_config = load_repo_config(ctx, &layout, command, RepoPersistPolicy::BestEffort).ok();
    let env = anolisa_env::EnvService::detect();
    let component_index = repo_config
        .as_ref()
        .and_then(|cfg| crate::resolution::load_optional_component_index(&layout, &env, cfg));

    crate::resolution::lookup_component_alias(input, component_index.as_ref())
        .unwrap_or_else(|| input.to_string())
}

/// Path for the component manifest saved as part of an installed component's
/// local state.
pub fn installed_component_manifest_path(
    layout: &FsLayout,
    component: &str,
    command: &str,
) -> Result<PathBuf, CliError> {
    Ok(
        installed_component_manifest_dir(layout, component, command)?
            .join(INSTALLED_COMPONENT_MANIFEST_FILE),
    )
}

/// Directory for the component manifest saved as part of an installed
/// component's local state.
pub fn installed_component_manifest_dir(
    layout: &FsLayout,
    component: &str,
    command: &str,
) -> Result<PathBuf, CliError> {
    validate_component_path_segment(component, command)?;
    Ok(layout
        .state_dir
        .join(INSTALLED_COMPONENT_MANIFESTS_SUBDIR)
        .join(component))
}

fn validate_component_path_segment(component: &str, command: &str) -> Result<(), CliError> {
    if component.trim().is_empty()
        || component == "."
        || component == ".."
        || component.contains('/')
        || component.contains('\\')
    {
        return Err(CliError::InvalidArgument {
            command: command.to_string(),
            reason: format!("component name '{component}' cannot be used as a local path segment"),
        });
    }
    Ok(())
}

/// Wire-friendly status label for a v5
/// [`Installation`](anolisa_core::domain::Installation), same vocabulary as
/// [`installation_status_str`] vocabulary: a delegated adopted/observed row
/// reports its management relation (the legacy state collapsed both into one
/// `adopted` status), any other row reports its lifecycle health.
pub(crate) fn installation_status_str(
    installation: &anolisa_core::domain::Installation,
) -> &'static str {
    use anolisa_core::domain::{LifecycleStatus, ManagementRelation, ProviderBinding};
    if installation.status == LifecycleStatus::Installed
        && let ProviderBinding::Delegated { relation, .. } = &installation.binding
    {
        match relation {
            ManagementRelation::Adopted { .. } => return "adopted",
            ManagementRelation::Observed => return "observed",
            ManagementRelation::Managed { .. } => {}
        }
    }
    match installation.status {
        LifecycleStatus::Installed => "installed",
        LifecycleStatus::Partial => "degraded",
        LifecycleStatus::Disabled => "disabled",
        LifecycleStatus::Failed => "failed",
    }
}

/// True iff the wire status label denotes a component that is actively
/// serving (i.e. `installed`, `degraded`, `adopted`, or `observed`). Used by
/// `list --enabled` to exclude `disabled`/`failed`/`not_installed`.
pub(crate) fn status_is_enabled(status_label: &str) -> bool {
    matches!(
        status_label,
        "installed" | "degraded" | "adopted" | "observed"
    )
}

/// Build an [`AdapterManager`] for the active layout, shared between
/// `adapter` and `status` handlers.
pub(crate) fn build_adapter_manager(ctx: &CliContext) -> AdapterManager {
    let (mut manager, layout) = new_adapter_manager(ctx);

    match StateView::load(ctx, "adapter", StateVisibility::UserPlusSystem) {
        Ok(view) => configure_adapter_manager(&mut manager, &view),
        Err(_) => {
            manager.set_visible_roots(vec![visible_root_for_adapter(&layout)]);
        }
    }

    manager
}

/// Build an adapter manager from the same state visibility snapshot used to
/// resolve a component-specific adapter target.
pub(crate) fn build_adapter_manager_from_view(
    ctx: &CliContext,
    view: &StateView,
) -> AdapterManager {
    let (mut manager, _) = new_adapter_manager(ctx);
    configure_adapter_manager(&mut manager, view);
    manager
}

fn new_adapter_manager(ctx: &CliContext) -> (AdapterManager, FsLayout) {
    let layout = resolve_layout(ctx);
    let env = anolisa_env::EnvService::detect();
    (
        AdapterManager::new(layout.clone(), Some(env.home), env.user),
        layout,
    )
}

fn configure_adapter_manager(manager: &mut AdapterManager, view: &StateView) {
    let roots = view
        .visible_roots
        .iter()
        .map(|root| visible_root_for_adapter(&root.layout))
        .collect();
    manager.set_visible_roots(roots);
    for warning in &view.warnings {
        manager.push_visibility_warning(warning.clone());
    }
}

fn visible_root_for_adapter(layout: &FsLayout) -> VisibleRoot {
    VisibleRoot {
        state_dir: layout.state_dir.clone(),
        contract_datadir_roots: adapter_contract_datadir_roots(layout),
    }
}

fn adapter_contract_datadir_roots(layout: &FsLayout) -> Vec<PathBuf> {
    // Two independent datadir-discovery mechanisms are layered here:
    //
    //   packaged_datadir_root()  — runtime probe: env override → exe-sibling
    //                              `../share/anolisa/` → layout.datadir.
    //                              Discovers wherever the *running binary's*
    //                              packaged tree actually lives on disk.
    //
    //   layout.package_datadir() — FHS constant: `/usr/share/anolisa` (rebased
    //                              under prefix). System roots use it so
    //                              RPM-installed contracts are found even when
    //                              the binary is at `/usr/local/bin/`.
    //
    // Both are added (deduped) because they cover different scenarios: the
    // exe-sibling probe handles relocated installs; the FHS constant handles
    // cross-install-method discovery (raw binary + RPM components).
    let mut roots = vec![layout.datadir.clone()];
    if let Some(packaged) = packaged::packaged_datadir_root(layout)
        && !roots.contains(&packaged)
    {
        roots.push(packaged);
    }
    if let Some(pkg_dd) = layout.package_datadir()
        && !roots.contains(&pkg_dd)
    {
        roots.push(pkg_dd);
    }
    roots
}

/// In-memory migration of pre-v4 symlink entries.
///
/// Loads each component's installed manifest, resolves its
/// `FileKind::Symlink` entries, and upgrades matching
/// `kind = File` `OwnedFile` entries to `kind = Symlink` with the
/// manifest-declared referent — but only when every disk-level
/// invariant holds (link exists, points at the manifest-declared
/// target, referent is a regular file, and any recorded sha256
/// matches the referent content).
///
/// Returns the number of entries migrated. Errors in individual
/// components are silently skipped (conservative: the entry stays
/// `kind = File` and the integrity probe reports `symlink_refused`).
pub fn migrate_v3_symlinks(store: &mut StateStore, layout: &FsLayout) -> usize {
    use std::collections::HashMap;
    use std::fs;

    use anolisa_core::domain::ProviderBinding;
    use anolisa_core::expand_layout_placeholders;
    use anolisa_core::manifest::{ComponentManifest, FileKind};
    use anolisa_core::path_safety::validate_owned_path;
    use anolisa_core::state::{ObjectKind, OwnedFileKind};
    use sha2::{Digest, Sha256};

    const MAX_MIGRATE_PROBE_BYTES: u64 = 256 * 1024 * 1024;

    fn hex_lower(bytes: &[u8]) -> String {
        bytes.iter().fold(String::new(), |mut s, b| {
            use std::fmt::Write;
            let _ = write!(s, "{b:02x}");
            s
        })
    }

    fn hash_file_streaming(path: &std::path::Path) -> std::io::Result<String> {
        use std::io::Read;
        let f = fs::File::open(path)?;
        let mut reader = std::io::BufReader::new(f);
        let mut hasher = Sha256::new();
        let mut buf = [0u8; 8 * 1024];
        let mut total: u64 = 0;
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > MAX_MIGRATE_PROBE_BYTES {
                return Err(std::io::Error::other(
                    "file exceeds migration probe ceiling",
                ));
            }
            hasher.update(&buf[..n]);
        }
        Ok(hex_lower(&hasher.finalize()))
    }

    // Pre-v4 symlink entries can only exist on records that came through the
    // legacy-file migration; a native v5 file was written by code that already
    // records symlinks as symlinks.
    if !store.migrated_from_legacy() {
        return 0;
    }

    let mut migrated = 0usize;

    for installation in &mut store.installations {
        if installation.kind != ObjectKind::Component {
            continue;
        }
        let name = installation.name.clone();
        let ProviderBinding::Owned { artifact } = &mut installation.binding else {
            continue;
        };
        let has_legacy = artifact.files.iter().any(|f| f.kind == OwnedFileKind::File);
        if !has_legacy {
            continue;
        }

        if validate_component_path_segment(&name, "migrate").is_err() {
            continue;
        }
        let manifest_path = layout
            .state_dir
            .join(INSTALLED_COMPONENT_MANIFESTS_SUBDIR)
            .join(&name)
            .join(INSTALLED_COMPONENT_MANIFEST_FILE);
        let toml_str = match fs::read_to_string(&manifest_path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let manifest = match ComponentManifest::from_toml_str(&toml_str) {
            Ok(m) => m,
            Err(_) => continue,
        };

        let mut expected: HashMap<PathBuf, PathBuf> = HashMap::new();
        for spec in &manifest.install.files {
            if spec.kind != FileKind::Symlink {
                continue;
            }
            let dest_template = match spec.install_path() {
                Some(t) => t,
                None => continue,
            };
            let referent_template = match spec.source.as_deref() {
                Some(t) => t,
                None => continue,
            };
            let dest =
                match expand_layout_placeholders(dest_template, layout, &[("component", &name)]) {
                    Ok(p) => p,
                    Err(_) => continue,
                };
            let referent = match expand_layout_placeholders(
                referent_template,
                layout,
                &[("component", &name)],
            ) {
                Ok(p) => p,
                Err(_) => continue,
            };
            if validate_owned_path(layout, &dest).is_err()
                || validate_owned_path(layout, &referent).is_err()
            {
                continue;
            }
            expected.insert(dest, referent);
        }
        if expected.is_empty() {
            continue;
        }

        for file in &mut artifact.files {
            if file.kind != OwnedFileKind::File {
                continue;
            }
            let Some(expected_referent) = expected.get(&file.path) else {
                continue;
            };
            let Ok(sym_meta) = fs::symlink_metadata(&file.path) else {
                continue;
            };
            if !sym_meta.file_type().is_symlink() {
                continue;
            }
            let Ok(actual_referent) = fs::read_link(&file.path) else {
                continue;
            };
            if actual_referent != *expected_referent {
                continue;
            }
            let Ok(ref_meta) = fs::symlink_metadata(expected_referent) else {
                continue;
            };
            if ref_meta.file_type().is_symlink() || !ref_meta.is_file() {
                continue;
            }
            if let Some(ref recorded_sha) = file.sha256 {
                if ref_meta.len() > MAX_MIGRATE_PROBE_BYTES {
                    continue;
                }
                let actual = match hash_file_streaming(expected_referent) {
                    Ok(h) => h,
                    Err(_) => continue,
                };
                if actual != *recorded_sha {
                    continue;
                }
            }
            file.kind = OwnedFileKind::Symlink;
            file.referent = Some(expected_referent.clone());
            file.sha256 = None;
            migrated += 1;
        }
    }

    migrated
}

#[cfg(test)]
mod tests {
    use super::*;

    use anolisa_core::adapter::claim::{AdapterClaim, ClaimStatus, DriverPayload, OpenClawClaim};
    use anolisa_core::state::{InstalledObject, ObjectStatus, Ownership, SubscriptionScope};

    use crate::commands::state_view::{ScopedStateRoot, StateScope, UnavailableStateRoot};

    #[test]
    fn remediation_commands_preserve_the_explicit_scope() {
        assert_eq!(
            scoped_component_command(InstallationScope::System, "repair", "cosh"),
            "sudo anolisa --install-mode system repair cosh"
        );
        assert_eq!(
            scoped_component_command(InstallationScope::User { uid: 1000 }, "repair", "cosh"),
            "anolisa --install-mode user repair cosh"
        );
    }

    fn test_component(name: &str) -> InstalledObject {
        InstalledObject {
            kind: ObjectKind::Component,
            name: name.to_string(),
            version: "1.0.0".to_string(),
            status: ObjectStatus::Installed,
            manifest_digest: None,
            distribution_source: None,
            raw_package: None,
            install_backend: Some("raw".to_string()),
            ownership: Some(Ownership::RawManaged),
            rpm_metadata: None,
            installed_at: "2026-01-01T00:00:00Z".to_string(),
            last_operation_id: None,
            managed: true,
            adopted: false,
            subscription_scope: SubscriptionScope::None,
            enabled_features: Vec::new(),
            component_refs: Vec::new(),
            files: Vec::new(),
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
            provisioned_packages: Vec::new(),
        }
    }

    fn state_with_objects(objects: Vec<InstalledObject>) -> StateStore {
        let migration = anolisa_core::state_migration::migrate_state(
            &objects,
            anolisa_core::domain::InstallationScope::System,
        );
        assert!(
            migration.quarantined.is_empty(),
            "fixtures must migrate cleanly"
        );
        let mut store = StateStore::empty();
        store.installations = migration.active;
        store
    }

    fn state_with_quarantined_object(mut object: InstalledObject) -> StateStore {
        object.install_backend = None;
        object.ownership = None;
        object.managed = false;
        let migration = anolisa_core::state_migration::migrate_state(
            &[object],
            anolisa_core::domain::InstallationScope::System,
        );
        assert!(migration.active.is_empty());
        assert_eq!(migration.quarantined.len(), 1);
        let mut store = StateStore::empty();
        store.quarantined = migration.quarantined;
        store
    }

    fn scoped_view(user_state: StateStore, system_state: StateStore) -> StateView {
        let user_root = ScopedStateRoot {
            scope: StateScope::User,
            layout: FsLayout::user_with_overrides(
                PathBuf::from("/tmp/anolisa-home"),
                None,
                None,
                Some(PathBuf::from("/tmp/anolisa-user-state")),
                None,
                None,
            ),
            state_path: PathBuf::from("/tmp/anolisa-user-state/installed.toml"),
            writable: true,
            state: user_state,
        };
        let system_root = ScopedStateRoot {
            scope: StateScope::System,
            layout: FsLayout::system(Some(PathBuf::from("/tmp/anolisa-system"))),
            state_path: PathBuf::from("/tmp/anolisa-system-state/installed.toml"),
            writable: false,
            state: system_state,
        };
        StateView {
            writable: user_root.clone(),
            visible_roots: vec![user_root, system_root],
            unavailable_roots: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn view_with_unavailable_system(user_state: StateStore) -> StateView {
        let mut view = scoped_view(user_state, StateStore::empty());
        let system_root = view.visible_roots.pop().expect("system root");
        view.unavailable_roots.push(UnavailableStateRoot {
            scope: StateScope::System,
            state_path: system_root.state_path,
            reason: "future state schema".to_string(),
        });
        view
    }

    fn test_adapter_claim(component: &str) -> AdapterClaim {
        AdapterClaim {
            claim_schema: 1,
            component: component.to_string(),
            framework: "openclaw".to_string(),
            plugin_id: None,
            adapter_type: None,
            enabled_at: "2026-01-01T00:00:00Z".to_string(),
            resource_root: PathBuf::from("/tmp/adapter-resource"),
            bundle_digest: None,
            driver_schema: 1,
            status: ClaimStatus::Enabled,
            resources: Vec::new(),
            driver_payload: DriverPayload::OpenClaw(OpenClawClaim {
                state_dir_resource: "state".to_string(),
                plugin_resource: "plugin".to_string(),
                skill_resources: Vec::new(),
                config_resources: Vec::new(),
            }),
        }
    }

    #[test]
    fn reject_visible_non_writable_component_blocks_read_only_system_view() {
        let view = scoped_view(
            StateStore::empty(),
            state_with_objects(vec![test_component("system-tool")]),
        );

        let err = reject_visible_non_writable_component_from_view(
            &view,
            "uninstall system-tool",
            "system-tool",
        )
        .expect_err("system component must be visible but read-only");

        match err {
            CliError::PermissionDenied { reason, hint, .. } => {
                assert!(reason.contains("system-tool"), "reason: {reason}");
                assert!(reason.contains("system-scope"), "reason: {reason}");
                assert!(
                    hint.as_deref()
                        .is_some_and(|h| h.contains("--install-mode system")),
                    "hint: {hint:?}",
                );
            }
            other => panic!("expected permission error, got {other:?}"),
        }
    }

    #[test]
    fn reject_visible_non_writable_component_keeps_missing_targets_unchanged() {
        let view = scoped_view(StateStore::empty(), StateStore::empty());

        reject_visible_non_writable_component_from_view(
            &view,
            "forget missing-tool",
            "missing-tool",
        )
        .expect("missing targets should continue into the existing not-installed path");
    }

    #[test]
    fn install_keeps_system_exact_identity_but_targets_writable_scope() {
        let view = scoped_view(
            state_with_objects(vec![test_component("cosh")]),
            state_with_objects(vec![test_component("legacy-name")]),
        );

        let (target, resolved_view, exact_identity) = resolve_lifecycle_target_from_view(
            "legacy-name",
            "install legacy-name",
            false,
            view,
            || "cosh".to_string(),
        )
        .expect("a system record must not block a user-scope install");

        assert_eq!(target, "legacy-name");
        assert!(exact_identity);
        assert!(
            resolved_view
                .writable
                .state
                .find(ObjectKind::Component, "cosh")
                .is_some(),
            "the existing user alias target remains a separate record",
        );
        assert!(
            resolved_view
                .writable
                .state
                .find(ObjectKind::Component, "legacy-name")
                .is_none(),
            "install planning must see the exact identity as fresh in user scope",
        );
    }

    #[test]
    fn mutation_rejects_system_exact_before_repo_alias() {
        let view = scoped_view(
            state_with_objects(vec![test_component("cosh")]),
            state_with_quarantined_object(test_component("legacy-name")),
        );
        let alias_consulted = std::cell::Cell::new(false);

        let err = resolve_lifecycle_target_from_view(
            "legacy-name",
            "forget legacy-name",
            true,
            view,
            || {
                alias_consulted.set(true);
                "cosh".to_string()
            },
        )
        .expect_err("the read-only system identity must win over a user alias target");

        assert_eq!(err.code(), "PERMISSION_DENIED");
        assert!(
            !alias_consulted.get(),
            "exact identity must skip alias lookup"
        );
        assert!(err.reason().contains("legacy-name"));
        assert!(err.reason().contains("system-scope"));
    }

    #[test]
    fn mutation_rejects_system_package_identity_before_repo_alias() {
        let mut system_component = test_component("legacy-name");
        system_component.raw_package = Some("copilot-shell".to_string());
        let view = scoped_view(
            state_with_objects(vec![test_component("cosh")]),
            state_with_objects(vec![system_component]),
        );

        let result = resolve_lifecycle_target_from_view(
            "copilot-shell",
            "forget copilot-shell",
            true,
            view,
            || "cosh".to_string(),
        );

        let err = result.expect_err("the visible system package identity must win");
        assert_eq!(err.code(), "PERMISSION_DENIED");
    }

    #[test]
    fn mutation_resolves_writable_package_identity_before_repo_alias() {
        let mut package_owner = test_component("user-tool");
        package_owner.raw_package = Some("legacy-name".to_string());
        let view = scoped_view(
            state_with_objects(vec![package_owner, test_component("cosh")]),
            StateStore::empty(),
        );
        let alias_consulted = std::cell::Cell::new(false);

        let (target, _, state_identity) = resolve_lifecycle_target_from_view(
            "legacy-name",
            "forget legacy-name",
            true,
            view,
            || {
                alias_consulted.set(true);
                "cosh".to_string()
            },
        )
        .expect("the writable package owner must be resolved directly");

        assert_eq!(target, "user-tool");
        assert!(state_identity);
        assert!(
            !alias_consulted.get(),
            "state identity must skip repo alias"
        );
    }

    #[test]
    fn mutation_rejects_package_identity_claimed_by_multiple_components() {
        let mut first = test_component("first");
        first.raw_package = Some("shared-package".to_string());
        let mut second = test_component("second");
        second.raw_package = Some("shared-package".to_string());
        let view = scoped_view(state_with_objects(vec![first, second]), StateStore::empty());

        let err = resolve_lifecycle_target_from_view(
            "shared-package",
            "forget shared-package",
            true,
            view,
            || "repo-target".to_string(),
        )
        .expect_err("ambiguous state package identity must fail closed");

        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(err.reason().contains("first"));
        assert!(err.reason().contains("second"));
    }

    #[test]
    fn mutation_includes_quarantine_in_package_identity_ambiguity() {
        let mut active = test_component("active");
        active.raw_package = Some("shared-package".to_string());
        let mut quarantine = state_with_quarantined_object(test_component("quarantined-system"));
        quarantine.quarantined[0].record.raw_package = Some("shared-package".to_string());
        let view = scoped_view(state_with_objects(vec![active]), quarantine);

        let err = resolve_lifecycle_target_from_view(
            "shared-package",
            "repair shared-package",
            true,
            view,
            || "repo-target".to_string(),
        )
        .expect_err("quarantined package claims must remain authoritative");

        assert_eq!(err.code(), "INVALID_ARGUMENT");
        assert!(err.reason().contains("active"));
        assert!(err.reason().contains("quarantined-system"));
    }

    #[test]
    fn adapter_target_keeps_visible_system_exact_before_repo_alias() {
        let view = scoped_view(
            state_with_objects(vec![test_component("cosh")]),
            state_with_objects(vec![test_component("legacy-name")]),
        );
        let alias_consulted = std::cell::Cell::new(false);

        let (target, _) = resolve_adapter_target_from_view(
            "legacy-name",
            "adapter enable legacy-name",
            view,
            || {
                alias_consulted.set(true);
                "cosh".to_string()
            },
        )
        .expect("system component must remain a visible adapter source");

        assert_eq!(target, "legacy-name");
        assert!(!alias_consulted.get());
    }

    #[test]
    fn adapter_target_rejects_alias_when_system_visibility_is_incomplete() {
        let view = view_with_unavailable_system(state_with_objects(vec![test_component("cosh")]));

        let err = resolve_adapter_target_from_view(
            "legacy-name",
            "adapter disable legacy-name",
            view,
            || "cosh".to_string(),
        )
        .expect_err("incomplete visibility must block adapter alias inference");

        assert!(err.reason().contains("visible state is incomplete"));
        assert!(err.reason().contains("legacy-name"));
    }

    #[test]
    fn adapter_target_keeps_exact_receipt_when_system_visibility_is_incomplete() {
        let mut user_state = state_with_objects(vec![test_component("cosh")]);
        user_state.upsert_adapter_claim(test_adapter_claim("legacy-name"));
        let view = view_with_unavailable_system(user_state);
        let alias_consulted = std::cell::Cell::new(false);

        let (target, _) = resolve_adapter_target_from_view(
            "legacy-name",
            "adapter disable legacy-name",
            view,
            || {
                alias_consulted.set(true);
                "cosh".to_string()
            },
        )
        .expect("an exact local receipt remains safely addressable");

        assert_eq!(target, "legacy-name");
        assert!(!alias_consulted.get());
    }

    /// Verify that `package_datadir()` is wired into the system-mode
    /// manager: an RPM-installed contract under `{prefix}/usr/share/anolisa`
    /// must be discoverable via scan when the primary datadir is
    /// `{prefix}/usr/local/share/anolisa`.
    ///
    /// This exercises the same wiring path as `build_adapter_manager()`
    /// for system mode without needing a full `CliContext`.
    #[test]
    fn system_mode_wiring_discovers_package_datadir_contract() {
        use anolisa_core::adapter::manager::AdapterManager;
        use anolisa_core::state::{
            InstallMode as StateInstallMode, InstalledObject, InstalledState, ObjectKind,
            ObjectStatus, Ownership, SubscriptionScope,
        };

        let tmp = tempfile::tempdir().expect("tempdir");
        let prefix = tmp.path().to_path_buf();
        let layout = FsLayout::system(Some(prefix));

        // Simulate the system-mode wiring from build_adapter_manager().
        let mut manager = AdapterManager::new(
            layout.clone(),
            Some(tmp.path().to_path_buf()),
            "test".into(),
        );
        if let Some(pkg_dd) = layout.package_datadir() {
            manager.push_primary_datadir_root(pkg_dd);
        }

        // Seed state: sec-core adopted.
        let state_dir = &layout.state_dir;
        let mut state = InstalledState {
            install_mode: StateInstallMode::System,
            prefix: layout.prefix.clone(),
            ..InstalledState::default()
        };
        state.upsert_object(InstalledObject {
            kind: ObjectKind::Component,
            name: "sec-core".to_string(),
            version: "0.1.0".to_string(),
            status: ObjectStatus::Adopted,
            manifest_digest: None,
            distribution_source: None,
            raw_package: None,
            install_backend: Some("rpm".to_string()),
            ownership: Some(Ownership::RpmObserved),
            rpm_metadata: None,
            installed_at: "2026-06-23T00:00:00Z".to_string(),
            last_operation_id: None,
            managed: false,
            adopted: true,
            subscription_scope: SubscriptionScope::None,
            enabled_features: Vec::new(),
            component_refs: Vec::new(),
            files: Vec::new(),
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
            provisioned_packages: Vec::new(),
        });
        std::fs::create_dir_all(state_dir).expect("mkdir state");
        state
            .save(&state_dir.join("installed.toml"))
            .expect("save state");

        // Write contract under the package datadir (NOT local datadir).
        let package_datadir = layout.package_datadir().expect("package_datadir");
        let contract_dir = package_datadir.join("components").join("sec-core");
        std::fs::create_dir_all(&contract_dir).expect("mkdir contract");
        std::fs::write(
            contract_dir.join("component.toml"),
            r#"
[component]
name = "sec-core"
version = "0.1.0"
layer = "runtime"

[[adapters]]
framework = "openclaw"
adapter_type = "plugin"
plugin_id = "sec-core"
dest = "{datadir}/adapters/sec-core/openclaw/"
"#,
        )
        .expect("write contract");

        let report = manager.scan().expect("scan");
        let entry = report
            .entries
            .iter()
            .find(|e| e.component == "sec-core" && e.framework == "openclaw");
        assert!(
            entry.is_some_and(|e| e.declared),
            "system-mode wiring must discover contract under package_datadir; \
             entries: {:?}, warnings: {:?}",
            report
                .entries
                .iter()
                .map(|e| (&e.component, &e.framework, e.declared))
                .collect::<Vec<_>>(),
            report.warnings,
        );
    }

    #[test]
    fn status_is_enabled_excludes_disabled_failed_and_unknown() {
        assert!(status_is_enabled("installed"));
        assert!(status_is_enabled("degraded"));
        assert!(status_is_enabled("adopted"));
        assert!(!status_is_enabled("disabled"));
        assert!(!status_is_enabled("failed"));
        assert!(!status_is_enabled("not_installed"));
        assert!(!status_is_enabled(""));
    }

    mod migrate_v3_symlinks_tests {
        use anolisa_core::state::{
            FileOwner, InstalledObject, InstalledState, ObjectKind, ObjectStatus, OwnedFile,
            OwnedFileKind, Ownership, SubscriptionScope,
        };
        use anolisa_core::state_store::StateStore;
        use anolisa_platform::fs_layout::FsLayout;
        use sha2::{Digest, Sha256};

        fn hex_lower(bytes: &[u8]) -> String {
            bytes.iter().fold(String::new(), |mut s, b| {
                use std::fmt::Write;
                let _ = write!(s, "{b:02x}");
                s
            })
        }

        fn sample_object(name: &str, files: Vec<OwnedFile>) -> InstalledObject {
            InstalledObject {
                kind: ObjectKind::Component,
                name: name.to_string(),
                version: "1.0.0".to_string(),
                status: ObjectStatus::Installed,
                manifest_digest: None,
                distribution_source: None,
                raw_package: None,
                install_backend: None,
                ownership: Some(Ownership::RawManaged),
                rpm_metadata: None,
                installed_at: "2026-01-01T00:00:00Z".to_string(),
                last_operation_id: None,
                managed: true,
                adopted: false,
                subscription_scope: SubscriptionScope::None,
                enabled_features: Vec::new(),
                component_refs: Vec::new(),
                files,
                external_modified_files: Vec::new(),
                services: Vec::new(),
                health: Vec::new(),
                provisioned_packages: Vec::new(),
            }
        }

        fn v3_state() -> InstalledState {
            InstalledState {
                schema_version: 3,
                ..Default::default()
            }
        }

        /// Persist a v3-shaped legacy state and load it back as the v5 store,
        /// so `migrated_from_legacy()` holds — the gate `migrate_v3_symlinks`
        /// checks in production.
        fn seed_v3_store(layout: &FsLayout, objects: Vec<InstalledObject>) -> StateStore {
            let mut state = v3_state();
            for object in objects {
                state.upsert_object(object);
            }
            std::fs::create_dir_all(&layout.state_dir).expect("mkdir state dir");
            let path = layout.state_dir.join("installed.toml");
            state.save(&path).expect("seed v3 state");
            StateStore::load(&path, 0).expect("load store")
        }

        fn owned_files<'a>(store: &'a StateStore, name: &str) -> &'a [OwnedFile] {
            match &store
                .find(ObjectKind::Component, name)
                .expect("component in store")
                .binding
            {
                anolisa_core::domain::ProviderBinding::Owned { artifact } => &artifact.files,
                other => panic!("expected owned component, found {other:?}"),
            }
        }

        fn write_manifest(layout: &FsLayout, component: &str, toml: &str) {
            let dir = layout
                .state_dir
                .join(super::INSTALLED_COMPONENT_MANIFESTS_SUBDIR)
                .join(component);
            std::fs::create_dir_all(&dir).expect("mkdir manifest dir");
            std::fs::write(dir.join(super::INSTALLED_COMPONENT_MANIFEST_FILE), toml)
                .expect("write manifest");
        }

        #[test]
        #[cfg(unix)]
        fn migrate_upgrades_manifest_declared_symlink() {
            let tmp = tempfile::tempdir().expect("tempdir");
            let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
            std::fs::create_dir_all(&layout.bin_dir).expect("mkdir bindir");
            std::fs::create_dir_all(&layout.libexec_dir).expect("mkdir libexecdir");

            let referent = layout.libexec_dir.join("tokenless").join("rtk");
            std::fs::create_dir_all(referent.parent().unwrap()).expect("mkdir referent parent");
            let payload = b"binary-payload";
            std::fs::write(&referent, payload).expect("write referent");

            let link = layout.bin_dir.join("rtk");
            std::os::unix::fs::symlink(&referent, &link).expect("symlink");

            let sha = hex_lower(&Sha256::digest(payload));

            write_manifest(
                &layout,
                "tokenless",
                r#"
[component]
name = "tokenless"
version = "1.0.0"
layer = "runtime"

[[install.files]]
source = "{libexecdir}/tokenless/rtk"
dest = "{bindir}/rtk"
type = "symlink"
"#,
            );

            let owned = OwnedFile {
                path: link.clone(),
                owner: FileOwner::Anolisa,
                sha256: Some(sha),
                kind: OwnedFileKind::File,
                referent: None,
            };
            let mut store = seed_v3_store(&layout, vec![sample_object("tokenless", vec![owned])]);

            let count = super::migrate_v3_symlinks(&mut store, &layout);
            assert_eq!(count, 1);

            let file = &owned_files(&store, "tokenless")[0];
            assert_eq!(file.kind, OwnedFileKind::Symlink);
            assert_eq!(file.referent.as_deref(), Some(referent.as_path()));
            assert!(file.sha256.is_none());
        }

        #[test]
        #[cfg(unix)]
        fn migrate_skips_when_disk_not_symlink() {
            let tmp = tempfile::tempdir().expect("tempdir");
            let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
            std::fs::create_dir_all(&layout.bin_dir).expect("mkdir bindir");
            std::fs::create_dir_all(&layout.libexec_dir).expect("mkdir libexecdir");

            let referent = layout.libexec_dir.join("tokenless").join("rtk");
            std::fs::create_dir_all(referent.parent().unwrap()).expect("mkdir referent parent");
            std::fs::write(&referent, b"binary").expect("write referent");

            let regular = layout.bin_dir.join("rtk");
            std::fs::write(&regular, b"regular-file").expect("write regular");

            write_manifest(
                &layout,
                "tokenless",
                r#"
[component]
name = "tokenless"
version = "1.0.0"
layer = "runtime"

[[install.files]]
source = "{libexecdir}/tokenless/rtk"
dest = "{bindir}/rtk"
type = "symlink"
"#,
            );

            let owned = OwnedFile {
                path: regular,
                owner: FileOwner::Anolisa,
                sha256: None,
                kind: OwnedFileKind::File,
                referent: None,
            };
            let mut store = seed_v3_store(&layout, vec![sample_object("tokenless", vec![owned])]);

            let count = super::migrate_v3_symlinks(&mut store, &layout);
            assert_eq!(count, 0);
            assert_eq!(
                owned_files(&store, "tokenless")[0].kind,
                OwnedFileKind::File
            );
        }

        #[test]
        #[cfg(unix)]
        fn migrate_skips_when_readlink_mismatches() {
            let tmp = tempfile::tempdir().expect("tempdir");
            let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
            std::fs::create_dir_all(&layout.bin_dir).expect("mkdir bindir");
            std::fs::create_dir_all(&layout.libexec_dir).expect("mkdir libexecdir");

            let expected_referent = layout.libexec_dir.join("tokenless").join("rtk");
            std::fs::create_dir_all(expected_referent.parent().unwrap()).expect("mkdir");
            std::fs::write(&expected_referent, b"binary").expect("write expected");

            let wrong_target = layout.libexec_dir.join("attacker").join("evil");
            std::fs::create_dir_all(wrong_target.parent().unwrap()).expect("mkdir");
            std::fs::write(&wrong_target, b"evil").expect("write evil");

            let link = layout.bin_dir.join("rtk");
            std::os::unix::fs::symlink(&wrong_target, &link).expect("symlink");

            write_manifest(
                &layout,
                "tokenless",
                r#"
[component]
name = "tokenless"
version = "1.0.0"
layer = "runtime"

[[install.files]]
source = "{libexecdir}/tokenless/rtk"
dest = "{bindir}/rtk"
type = "symlink"
"#,
            );

            let owned = OwnedFile {
                path: link,
                owner: FileOwner::Anolisa,
                sha256: None,
                kind: OwnedFileKind::File,
                referent: None,
            };
            let mut store = seed_v3_store(&layout, vec![sample_object("tokenless", vec![owned])]);

            let count = super::migrate_v3_symlinks(&mut store, &layout);
            assert_eq!(count, 0);
            assert_eq!(
                owned_files(&store, "tokenless")[0].kind,
                OwnedFileKind::File
            );
        }

        #[test]
        #[cfg(unix)]
        fn migrate_skips_when_sha256_mismatches() {
            let tmp = tempfile::tempdir().expect("tempdir");
            let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
            std::fs::create_dir_all(&layout.bin_dir).expect("mkdir bindir");
            std::fs::create_dir_all(&layout.libexec_dir).expect("mkdir libexecdir");

            let referent = layout.libexec_dir.join("tokenless").join("rtk");
            std::fs::create_dir_all(referent.parent().unwrap()).expect("mkdir");
            std::fs::write(&referent, b"correct-payload").expect("write referent");

            let link = layout.bin_dir.join("rtk");
            std::os::unix::fs::symlink(&referent, &link).expect("symlink");

            write_manifest(
                &layout,
                "tokenless",
                r#"
[component]
name = "tokenless"
version = "1.0.0"
layer = "runtime"

[[install.files]]
source = "{libexecdir}/tokenless/rtk"
dest = "{bindir}/rtk"
type = "symlink"
"#,
            );

            let owned = OwnedFile {
                path: link,
                owner: FileOwner::Anolisa,
                sha256: Some("deadbeefdeadbeef".to_string()),
                kind: OwnedFileKind::File,
                referent: None,
            };
            let mut store = seed_v3_store(&layout, vec![sample_object("tokenless", vec![owned])]);

            let count = super::migrate_v3_symlinks(&mut store, &layout);
            assert_eq!(count, 0);
            assert_eq!(
                owned_files(&store, "tokenless")[0].kind,
                OwnedFileKind::File
            );
        }

        #[test]
        fn migrate_skips_when_manifest_missing() {
            let tmp = tempfile::tempdir().expect("tempdir");
            let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
            std::fs::create_dir_all(&layout.bin_dir).expect("mkdir bindir");

            let owned = OwnedFile {
                path: layout.bin_dir.join("rtk"),
                owner: FileOwner::Anolisa,
                sha256: None,
                kind: OwnedFileKind::File,
                referent: None,
            };
            let mut store = seed_v3_store(&layout, vec![sample_object("tokenless", vec![owned])]);

            let count = super::migrate_v3_symlinks(&mut store, &layout);
            assert_eq!(count, 0);
            assert_eq!(
                owned_files(&store, "tokenless")[0].kind,
                OwnedFileKind::File
            );
        }

        #[test]
        fn migrate_skips_traversal_component_name() {
            let tmp = tempfile::tempdir().expect("tempdir");
            let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
            std::fs::create_dir_all(&layout.bin_dir).expect("mkdir bindir");

            let owned = OwnedFile {
                path: layout.bin_dir.join("rtk"),
                owner: FileOwner::Anolisa,
                sha256: None,
                kind: OwnedFileKind::File,
                referent: None,
            };
            let mut store =
                seed_v3_store(&layout, vec![sample_object("../../../etc", vec![owned])]);

            let count = super::migrate_v3_symlinks(&mut store, &layout);
            assert_eq!(count, 0);
        }

        #[test]
        #[cfg(unix)]
        /// A native v5 store (not derived from a legacy file) never runs the
        /// symlink upgrade — its records were written by code that already
        /// records symlinks as symlinks.
        #[allow(clippy::items_after_statements)]
        fn migrate_skips_native_v5_store() {
            let tmp = tempfile::tempdir().expect("tempdir");
            let layout = FsLayout::system(Some(tmp.path().to_path_buf()));
            std::fs::create_dir_all(&layout.bin_dir).expect("mkdir bindir");
            std::fs::create_dir_all(&layout.libexec_dir).expect("mkdir libexecdir");

            let referent = layout.libexec_dir.join("tokenless").join("rtk");
            std::fs::create_dir_all(referent.parent().unwrap()).expect("mkdir referent parent");
            let payload = b"binary-payload";
            std::fs::write(&referent, payload).expect("write referent");

            let link = layout.bin_dir.join("rtk");
            std::os::unix::fs::symlink(&referent, &link).expect("symlink");

            let sha = hex_lower(&Sha256::digest(payload));

            write_manifest(
                &layout,
                "tokenless",
                r#"
[component]
name = "tokenless"
version = "1.0.0"
layer = "runtime"

[[install.files]]
source = "{libexecdir}/tokenless/rtk"
dest = "{bindir}/rtk"
type = "symlink"
"#,
            );

            let owned = OwnedFile {
                path: link.clone(),
                owner: FileOwner::Anolisa,
                sha256: Some(sha),
                kind: OwnedFileKind::File,
                referent: None,
            };
            // Round-trip through a *v5* file: save the migrated store, then
            // reload it — the reload is a native v5 load.
            let store = seed_v3_store(&layout, vec![sample_object("tokenless", vec![owned])]);
            let path = layout.state_dir.join("installed.toml");
            store.save(&path).expect("persist v5 store");
            let mut store = StateStore::load(&path, 0).expect("reload v5 store");
            assert!(!store.migrated_from_legacy());

            let count = super::migrate_v3_symlinks(&mut store, &layout);
            assert_eq!(count, 0);
            assert_eq!(
                owned_files(&store, "tokenless")[0].kind,
                OwnedFileKind::File
            );
        }
    }

    mod persist_policy {
        use super::super::{
            RepoConfigProvisioning, RepoPersistPolicy, enforce_repo_persist_policy,
        };
        use crate::response::CliError;
        use std::path::PathBuf;

        fn persist_failed() -> RepoConfigProvisioning {
            RepoConfigProvisioning::DownloadedPersistFailed {
                url: "https://example.com/repo.toml".to_string(),
                dest: PathBuf::from("/etc/anolisa/repo.toml"),
                reason: "permission denied".to_string(),
            }
        }

        /// Require policy: DownloadedPersistFailed is escalated to CliError::Runtime
        /// carrying the dest path and underlying reason.
        #[test]
        fn require_policy_rejects_persist_failure() {
            let err =
                enforce_repo_persist_policy(&persist_failed(), RepoPersistPolicy::Require, "list")
                    .expect_err("Require must reject persist failure");
            match err {
                CliError::Runtime { command, reason } => {
                    assert_eq!(command, "list");
                    assert!(
                        reason.contains("/etc/anolisa/repo.toml"),
                        "reason should carry dest: {reason}"
                    );
                    assert!(
                        reason.contains("permission denied"),
                        "reason should carry inner cause: {reason}"
                    );
                }
                other => panic!("expected Runtime, got {other:?}"),
            }
        }

        /// BestEffort policy: DownloadedPersistFailed is tolerated (Ok).
        #[test]
        fn best_effort_policy_tolerates_persist_failure() {
            enforce_repo_persist_policy(&persist_failed(), RepoPersistPolicy::BestEffort, "list")
                .expect("BestEffort must accept persist failure");
        }

        /// Neither policy touches non-persist-failure provisioning states.
        #[test]
        fn non_persist_failure_states_are_always_ok() {
            let downloaded = RepoConfigProvisioning::Downloaded {
                url: "https://example.com/repo.toml".to_string(),
                dest: PathBuf::from("/etc/anolisa/repo.toml"),
            };
            let existing = RepoConfigProvisioning::Existing;
            let dry_run = RepoConfigProvisioning::FetchedForDryRun {
                url: "https://example.com/repo.toml".to_string(),
                dest: PathBuf::from("/etc/anolisa/repo.toml"),
            };
            for provisioning in [&downloaded, &existing, &dry_run] {
                enforce_repo_persist_policy(provisioning, RepoPersistPolicy::Require, "install")
                    .expect("Require must pass through non-persist-failure states");
                enforce_repo_persist_policy(provisioning, RepoPersistPolicy::BestEffort, "list")
                    .expect("BestEffort must pass through non-persist-failure states");
            }
        }
    }
}
