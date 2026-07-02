//! Shared helpers for tier1 / tier2 command handlers.
//!
//! Access to the skeleton-stable command inputs: [`FsLayout`],
//! [`InstalledState`], [`Catalog`], and repo configuration. Keep this module
//! thin — handlers compose these calls; we do not introduce a service layer here.

use std::path::{Path, PathBuf};

use anolisa_core::adapter::manager::AdapterManager;
use anolisa_core::{Catalog, CatalogLayers, InstalledState, ObjectStatus};
use anolisa_platform::fs_layout::FsLayout;

use crate::color::Palette;
use crate::context::{CliContext, InstallMode};
use crate::packaged;
use crate::repo_config::{RepoConfig, RepoConfigProvisioning};
use crate::response::CliError;

/// Subdirectory under `datadir` and `etc_dir` where component
/// manifests live (e.g. `share/anolisa/manifests`, `etc/anolisa/manifests`).
const MANIFESTS_SUBDIR: &str = "manifests";
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

/// Refuse a handler-level system-only path when called with user-mode context.
///
/// The dispatcher handles normal CLI entry. This guard protects direct calls
/// from tests and shared command helpers that bypass the dispatcher.
pub(crate) fn require_system_mode(
    ctx: &CliContext,
    command: &str,
    reason: &str,
    sudo_command: &str,
) -> Result<(), CliError> {
    if ctx.install_mode == InstallMode::System {
        return Ok(());
    }

    Err(CliError::InvalidArgument {
        command: command.to_string(),
        reason: format!("{reason}; run `{sudo_command}`"),
    })
}

/// Build a consistent package-transaction permission error.
pub(crate) fn package_permission_error(command: &str, bin: &str, action: &str) -> CliError {
    CliError::Runtime {
        command: command.to_string(),
        reason: format!("permission denied running {bin}; re-run the {action} with sudo"),
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
    match provisioning {
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
    }
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

/// Load `InstalledState` from the layout's `state_dir/installed.toml`.
/// A missing file yields `Default` — fresh installs are not an error.
pub fn load_installed_state(ctx: &CliContext, command: &str) -> Result<InstalledState, CliError> {
    let layout = resolve_layout(ctx);
    let path = layout.state_dir.join("installed.toml");
    InstalledState::load(&path).map_err(|err| CliError::InvalidArgument {
        command: command.to_string(),
        reason: format!(
            "failed to load installed state at {}: {err}",
            path.display()
        ),
    })
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

/// Load the layered catalog.
///
/// Layers (low → high precedence):
///   1. **bundled** — packaged manifests under `datadir/manifests` (the
///      install-time location). Falls back to the dev-tree manifests
///      (`CARGO_MANIFEST_DIR/../../manifests`) when the packaged location is
///      absent so `cargo run` in the source tree works without an install.
///   2. **overlay** — `manifests_overlay` (e.g. `/etc/anolisa/manifests` or
///      `~/.config/anolisa/manifests`) attached as the `system` or `user`
///      layer per `ctx.install_mode`. Optional: skipped when the directory
///      does not exist.
///
/// The overlay used to be passed as `bundled` with no system/user layers —
/// that meant any overlay completely replaced the in-tree catalog (and an
/// empty overlay produced an empty catalog). The proper Catalog contract is
/// that the bundled layer is always-present and overlays stack on top.
pub fn load_bundled_catalog(ctx: &CliContext, command: &str) -> Result<Catalog, CliError> {
    let layout = resolve_layout(ctx);
    let bundled = packaged_manifests_root(&layout)
        .or_else(dev_tree_manifests)
        .unwrap_or_else(|| layout.datadir.join(MANIFESTS_SUBDIR));

    let overlay = layout.manifests_overlay.clone();
    let overlay = overlay.is_dir().then_some(overlay);
    let (system, user) = match ctx.install_mode {
        InstallMode::System => (overlay, None),
        InstallMode::User => (None, overlay),
    };

    let layers = CatalogLayers {
        bundled,
        system,
        user,
    };
    Catalog::load(layers).map_err(|err| CliError::InvalidArgument {
        command: command.to_string(),
        reason: format!("failed to load catalog: {err}"),
    })
}

fn packaged_manifests_root(layout: &FsLayout) -> Option<PathBuf> {
    // Discover the packaged datadir (`<prefix>/share/anolisa/`) using
    // the shared probe in [`crate::packaged`] — that helper honors the
    // `ANOLISA_DATA_DIR` env override and binary-location lookup so a
    // user-mode CLI still finds the system-installed datadir under
    // `/usr/local/share/anolisa/` when one is staged by
    // `install-anolisa.sh`. Falls back to `layout.datadir` for the
    // pre-P1-A install layout.
    let datadir = packaged::packaged_datadir_root(layout).unwrap_or_else(|| layout.datadir.clone());
    let candidate = datadir.join(MANIFESTS_SUBDIR);
    candidate.is_dir().then_some(candidate)
}

fn dev_tree_manifests() -> Option<PathBuf> {
    let candidate = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("manifests");
    candidate.is_dir().then_some(candidate)
}

/// Wire-friendly label for an [`ObjectStatus`] value. Shared between the
/// `status` and `list` handlers so both surfaces speak the same vocabulary
/// (matches launch spec §7.1: `installed | degraded | disabled | failed |
/// adopted`). The `"not_installed"` label is produced separately by callers
/// when no `InstalledObject` exists at all.
pub(crate) fn object_status_str(status: ObjectStatus) -> &'static str {
    match status {
        ObjectStatus::Installed => "installed",
        ObjectStatus::Partial => "degraded",
        ObjectStatus::Disabled => "disabled",
        ObjectStatus::Failed => "failed",
        ObjectStatus::Adopted => "adopted",
    }
}

/// True iff the wire status label denotes a component that is actively
/// serving (i.e. `installed`, `degraded`, or `adopted`). Used by
/// `list --enabled` to exclude `disabled`/`failed`/`not_installed`.
pub(crate) fn status_is_enabled(status_label: &str) -> bool {
    matches!(status_label, "installed" | "degraded" | "adopted")
}

/// Build an [`AdapterManager`] for the active layout, shared between
/// `adapter` and `status` handlers.
pub(crate) fn build_adapter_manager(ctx: &CliContext) -> AdapterManager {
    use anolisa_core::adapter::manager::VisibleRoot;

    let layout = resolve_layout(ctx);
    let env = anolisa_env::EnvService::detect();
    let mut manager = AdapterManager::new(layout.clone(), Some(env.home), env.user);

    // Two independent datadir-discovery mechanisms are layered here:
    //
    //   packaged_datadir_root()  — runtime probe: env override → exe-sibling
    //                              `../share/anolisa/` → layout.datadir.
    //                              Discovers wherever the *running binary's*
    //                              packaged tree actually lives on disk.
    //
    //   layout.package_datadir() — FHS constant: `/usr/share/anolisa` (rebased
    //                              under prefix). Always present in system mode
    //                              so RPM-installed contracts are found even
    //                              when the binary is at `/usr/local/bin/`.
    //
    // Both are added (deduped by push_primary_datadir_root / contains-check)
    // because they cover different scenarios: the exe-sibling probe handles
    // relocated installs; the FHS constant handles cross-install-method
    // discovery (raw binary + RPM components).

    if ctx.install_mode == InstallMode::User {
        let system_layout = FsLayout::system(ctx.prefix.clone());
        let mut system_datadirs = vec![system_layout.datadir.clone()];
        if let Some(packaged) = packaged::packaged_datadir_root(&system_layout)
            && !system_datadirs.contains(&packaged)
        {
            system_datadirs.push(packaged);
        }
        if let Some(pkg_dd) = system_layout.package_datadir()
            && !system_datadirs.contains(&pkg_dd)
        {
            system_datadirs.push(pkg_dd);
        }
        manager.push_visible_root(VisibleRoot {
            state_dir: system_layout.state_dir,
            contract_datadir_roots: system_datadirs,
        });
    } else {
        if let Some(packaged) = packaged::packaged_datadir_root(&layout)
            && packaged != layout.datadir
        {
            manager.push_primary_datadir_root(packaged);
        }
        if let Some(pkg_dd) = layout.package_datadir() {
            manager.push_primary_datadir_root(pkg_dd);
        }
    }

    manager
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
pub fn migrate_v3_symlinks(state: &mut InstalledState, layout: &FsLayout) -> usize {
    use std::collections::HashMap;
    use std::fs;

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

    if state.schema_version >= 4 {
        return 0;
    }

    let mut migrated = 0usize;

    for obj in &mut state.objects {
        if obj.kind != ObjectKind::Component {
            continue;
        }
        let has_legacy = obj.files.iter().any(|f| f.kind == OwnedFileKind::File);
        if !has_legacy {
            continue;
        }

        if validate_component_path_segment(&obj.name, "migrate").is_err() {
            continue;
        }
        let manifest_path = layout
            .state_dir
            .join(INSTALLED_COMPONENT_MANIFESTS_SUBDIR)
            .join(&obj.name)
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
            let dest = match expand_layout_placeholders(
                dest_template,
                layout,
                &[("component", &obj.name)],
            ) {
                Ok(p) => p,
                Err(_) => continue,
            };
            let referent = match expand_layout_placeholders(
                referent_template,
                layout,
                &[("component", &obj.name)],
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

        for file in &mut obj.files {
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
            InstalledObject, InstalledState, ObjectKind, ObjectStatus, Ownership, SubscriptionScope,
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
        let mut state = InstalledState::default();
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

    /// `object_status_str` must cover every variant of `ObjectStatus` and
    /// produce the exact wire vocabulary the spec promises. If a new variant
    /// is added, this test forces us to extend the mapping.
    #[test]
    fn object_status_str_covers_full_vocabulary() {
        assert_eq!(object_status_str(ObjectStatus::Installed), "installed");
        assert_eq!(object_status_str(ObjectStatus::Partial), "degraded");
        assert_eq!(object_status_str(ObjectStatus::Disabled), "disabled");
        assert_eq!(object_status_str(ObjectStatus::Failed), "failed");
        assert_eq!(object_status_str(ObjectStatus::Adopted), "adopted");
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
            let mut state = v3_state();
            state.upsert_object(sample_object("tokenless", vec![owned]));

            let count = super::migrate_v3_symlinks(&mut state, &layout);
            assert_eq!(count, 1);

            let file = &state.objects[0].files[0];
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
            let mut state = v3_state();
            state.upsert_object(sample_object("tokenless", vec![owned]));

            let count = super::migrate_v3_symlinks(&mut state, &layout);
            assert_eq!(count, 0);
            assert_eq!(state.objects[0].files[0].kind, OwnedFileKind::File);
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
            let mut state = v3_state();
            state.upsert_object(sample_object("tokenless", vec![owned]));

            let count = super::migrate_v3_symlinks(&mut state, &layout);
            assert_eq!(count, 0);
            assert_eq!(state.objects[0].files[0].kind, OwnedFileKind::File);
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
            let mut state = v3_state();
            state.upsert_object(sample_object("tokenless", vec![owned]));

            let count = super::migrate_v3_symlinks(&mut state, &layout);
            assert_eq!(count, 0);
            assert_eq!(state.objects[0].files[0].kind, OwnedFileKind::File);
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
            let mut state = v3_state();
            state.upsert_object(sample_object("tokenless", vec![owned]));

            let count = super::migrate_v3_symlinks(&mut state, &layout);
            assert_eq!(count, 0);
            assert_eq!(state.objects[0].files[0].kind, OwnedFileKind::File);
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
            let mut state = v3_state();
            state.upsert_object(sample_object("../../../etc", vec![owned]));

            let count = super::migrate_v3_symlinks(&mut state, &layout);
            assert_eq!(count, 0);
        }

        #[test]
        #[cfg(unix)]
        fn migrate_skips_when_schema_v4() {
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
            let mut state = InstalledState::default();
            state.upsert_object(sample_object("tokenless", vec![owned]));
            assert_eq!(state.schema_version, 4);

            let count = super::migrate_v3_symlinks(&mut state, &layout);
            assert_eq!(count, 0);
            assert_eq!(state.objects[0].files[0].kind, OwnedFileKind::File);
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
