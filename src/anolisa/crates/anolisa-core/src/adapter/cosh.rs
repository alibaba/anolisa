//! Cosh (copilot-shell) framework driver.
//!
//! Cosh discovers extensions by scanning well-known directories, so its
//! adapter is *extension*-typed (`adapter_type = "extension"`), not a
//! CLI-registered plugin: `enable` copies the component's extension tree
//! into `<cosh_home>/extensions/<plugin_id>/` and `disable` removes only
//! that directory. No framework CLI is spawned — enable/disable/status are
//! pure filesystem operations mediated by the Manager's
//! [`AdapterOps`](super::driver::AdapterOps).
//!
//! The per-user extension dir ANOLISA takes over is distinct from the
//! system-level auto-discovery tree an RPM may ship under
//! `/usr/share/anolisa/extensions/<component>/`; `disable` never touches
//! the latter, which is package-owned, not receipt-owned.
//!
//! Env contract (used by detection and home resolution, and by tests to
//! point at a scratch home): `COSH_BIN` overrides the detected binary name;
//! `COSH_HOME` overrides the cosh home directory (default
//! `<user_home>/.copilot-shell`).

use std::path::{Path, PathBuf};

use super::AdapterError;
use super::claim::{
    AdapterClaim, CLAIM_SCHEMA_VERSION, ClaimResource, ClaimResourceKind, ClaimStatus, CoshClaim,
    DRIVER_SCHEMA_VERSION, DriverPayload,
};
use super::driver::{
    AdapterBundle, AdapterCondition, AdapterConditionKind, AdapterStatusReport, AdapterSummary,
    ClaimResourceRef, ConditionStatus, DetectResult, DisableReport, DriverCtx, DriverPlan,
    FrameworkDriver, HostEnv, find_binary_in_path,
};
use super::util::{bool_status, digest_tree, now_iso8601};

/// Candidate binary names that indicate cosh is installed. `co` and
/// `copilot` are the short/legacy aliases of the `cosh` CLI.
const COSH_BINARIES: &[&str] = &["cosh", "co", "copilot"];

/// Native manifest inside a cosh extension bundle. Its presence is what
/// makes a directory a valid cosh extension.
const COSH_MANIFEST: &str = "cosh-extension.json";

/// Ownership marker ANOLISA drops inside the delivered extension directory.
/// Its presence proves the directory is ANOLISA-managed, so a re-enable may
/// safely replace it and disable may safely remove it — without that proof,
/// enable refuses to overwrite and disable leaves the directory alone, so a
/// user-installed extension of the same name is never destroyed.
const COSH_OWNERSHIP_MARKER: &str = ".anolisa-adapter";

/// Resource id used in Cosh receipts.
const RES_EXTENSION_DIR: &str = "cosh_extension_dir";

/// Cosh driver. Stateless; all per-operation context arrives via
/// [`DriverCtx`].
pub struct CoshDriver;

impl CoshDriver {
    /// Construct the driver.
    pub fn new() -> Self {
        Self
    }
}

impl Default for CoshDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameworkDriver for CoshDriver {
    fn name(&self) -> &'static str {
        "cosh"
    }

    fn detect(&self, env: &HostEnv) -> DetectResult {
        // A cosh CLI on PATH is the strong signal. Because the extension
        // model only drops files (no CLI needed to enable), an existing
        // cosh home is accepted as a weaker signal so a user who installed
        // cosh without leaving its launcher on PATH can still enable.
        if let Some(path) = detect_cosh_binary() {
            return DetectResult {
                detected: true,
                reason: format!("cosh CLI found at {}", path.display()),
            };
        }
        match cosh_home(env.user_home.as_deref()).filter(|h| h.exists()) {
            Some(home) => DetectResult {
                detected: true,
                reason: format!(
                    "cosh CLI not on PATH, but home {} exists (extension can still be delivered)",
                    home.display()
                ),
            },
            None => DetectResult {
                detected: false,
                reason: "cosh not detected: no cosh/co/copilot on PATH and no ~/.copilot-shell"
                    .to_string(),
            },
        }
    }

    fn allowed_external_roots(&self, ctx: &DriverCtx) -> Vec<PathBuf> {
        // The only external root Cosh writes is its own home directory.
        cosh_home(ctx.user_home.as_deref()).into_iter().collect()
    }

    fn read_bundle(&self, ctx: &DriverCtx) -> Result<AdapterBundle, AdapterError> {
        let root = &ctx.resource_root;
        if !root.is_dir() {
            return Err(AdapterError::BundleInvalid {
                root: root.clone(),
                reason: "resource root does not exist or is not a directory".to_string(),
            });
        }
        let manifest = ctx
            .declared_bundle_entry
            .as_deref()
            .unwrap_or(COSH_MANIFEST);
        if !root.join(manifest).is_file() {
            return Err(AdapterError::BundleInvalid {
                root: root.clone(),
                reason: format!("cosh extension manifest '{manifest}' missing from resource root"),
            });
        }
        // Extension id used for the destination directory name. Falls back
        // to the component name when the manifest declares no plugin id.
        let plugin_id = Some(
            ctx.declared_plugin_id
                .clone()
                .filter(|id| !id.is_empty())
                .unwrap_or_else(|| ctx.component.clone()),
        );
        Ok(AdapterBundle {
            resource_root: root.clone(),
            digest: digest_tree(root),
            plugin_id,
        })
    }

    fn plan_enable(
        &self,
        bundle: &AdapterBundle,
        ctx: &DriverCtx,
    ) -> Result<DriverPlan, AdapterError> {
        let dst = extension_dir(bundle, ctx)?;
        let actions = vec![format!(
            "deliver cosh extension from {} to {}",
            bundle.resource_root.display(),
            dst.display(),
        )];
        Ok(DriverPlan {
            framework: self.name().to_string(),
            component: ctx.component.clone(),
            actions,
            register_command: None,
        })
    }

    fn prepare_enable(
        &self,
        bundle: &AdapterBundle,
        ctx: &DriverCtx,
    ) -> Result<AdapterClaim, AdapterError> {
        let dst = extension_dir(bundle, ctx)?;
        let resources = vec![ClaimResource {
            id: RES_EXTENSION_DIR.to_string(),
            purpose: "cosh_extension_dir".to_string(),
            kind: ClaimResourceKind::ExternalPath { path: dst },
        }];
        Ok(AdapterClaim {
            claim_schema: CLAIM_SCHEMA_VERSION,
            component: ctx.component.clone(),
            framework: self.name().to_string(),
            plugin_id: bundle.plugin_id.clone(),
            adapter_type: ctx.adapter_type.clone(),
            enabled_at: now_iso8601(),
            resource_root: bundle.resource_root.clone(),
            bundle_digest: bundle.digest.clone(),
            driver_schema: DRIVER_SCHEMA_VERSION,
            status: ClaimStatus::Enabled,
            resources,
            driver_payload: DriverPayload::Cosh(CoshClaim {
                extension_dir_resource: RES_EXTENSION_DIR.to_string(),
            }),
        })
    }

    fn apply_enable(&self, claim: &AdapterClaim, ctx: &DriverCtx) -> Result<(), AdapterError> {
        let dst = claim_extension_dir(claim).ok_or_else(|| AdapterError::BundleInvalid {
            root: claim.resource_root.clone(),
            reason: "cosh receipt has no extension directory resource".to_string(),
        })?;
        // Never clobber a directory ANOLISA does not own. A first enable
        // onto a user-installed extension of the same name would otherwise
        // silently destroy the user's files (and a later disable would
        // remove them as if ANOLISA had created them). Only replace when the
        // directory is empty/absent or carries our ownership marker.
        if dst.exists() && !is_anolisa_owned(&dst) && !dir_is_empty(&dst) {
            return Err(AdapterError::InvalidAdapterInput {
                component: ctx.component.clone(),
                framework: self.name().to_string(),
                reason: format!(
                    "refusing to overwrite existing non-ANOLISA cosh extension at {} (no {COSH_OWNERSHIP_MARKER} marker); remove it manually to enable",
                    dst.display()
                ),
            });
        }
        // Replace any prior ANOLISA contents so a re-enable after a bundle
        // change does not leave stale files behind. The dir is inside the
        // receipt-declared external root, so remove_tree is bounded.
        ctx.ops.remove_tree(&dst)?;
        // Write the ownership marker *before* copying the bundle: if the
        // copy fails partway, the directory is already provably
        // ANOLISA-owned, so a later disable will remove the partial tree
        // instead of leaving it (and dropping the receipt). copy_tree
        // tolerates the pre-existing marker in the destination.
        ctx.ops.write_file(
            &dst.join(COSH_OWNERSHIP_MARKER),
            b"ANOLISA-managed cosh extension\n",
        )?;
        ctx.ops.copy_tree(&claim.resource_root, &dst)?;
        Ok(())
    }

    fn status(
        &self,
        claim: &AdapterClaim,
        ctx: &DriverCtx,
    ) -> Result<AdapterStatusReport, AdapterError> {
        let mut conditions = Vec::new();

        // 1. Framework detectable?
        let detect = self.detect(&HostEnv {
            user_home: ctx.user_home.clone(),
        });
        conditions.push(AdapterCondition {
            kind: AdapterConditionKind::FrameworkDetected,
            status: bool_status(detect.detected),
            reason: Some(detect.reason.clone()),
            resource: None,
        });

        // 2. Resource bundle still matches the enable-time digest?
        conditions.push(bundle_match_condition(claim));

        // 3. Extension tree still present, carrying its manifest AND our
        //    ownership marker? A dir/manifest without the marker is not
        //    ANOLISA-managed (user replaced it, or a marker write failed),
        //    which is a degraded state — not a healthy one. This is a
        //    reliable, read-only filesystem check, so verification is always
        //    supported for cosh.
        let (tree_present, tree_reason) = match claim_extension_dir(claim) {
            Some(dir) if !dir.is_dir() || !dir.join(COSH_MANIFEST).is_file() => (
                false,
                Some("cosh extension directory or manifest missing".to_string()),
            ),
            Some(dir) if !is_anolisa_owned(&dir) => (
                false,
                Some(format!(
                    "cosh extension directory is not ANOLISA-managed ({COSH_OWNERSHIP_MARKER} marker missing)"
                )),
            ),
            Some(_) => (true, None),
            None => (
                false,
                Some("receipt has no extension directory".to_string()),
            ),
        };
        conditions.push(AdapterCondition {
            kind: AdapterConditionKind::TreePresent,
            status: bool_status(tree_present),
            reason: tree_reason,
            resource: Some(ClaimResourceRef {
                id: RES_EXTENSION_DIR.to_string(),
            }),
        });
        conditions.push(AdapterCondition {
            kind: AdapterConditionKind::VerificationSupported,
            status: ConditionStatus::True,
            reason: None,
            resource: None,
        });

        let summary = summarize(claim.status, detect.detected, tree_present);
        Ok(AdapterStatusReport {
            summary,
            conditions,
        })
    }

    fn disable(
        &self,
        claim: &AdapterClaim,
        ctx: &DriverCtx,
    ) -> Result<DisableReport, AdapterError> {
        let mut messages = Vec::new();
        let mut cleanup_complete = true;

        match claim_extension_dir(claim) {
            Some(dir) if !dir.exists() => {
                messages.push(format!(
                    "cosh extension dir {} already absent",
                    dir.display()
                ));
            }
            // Only remove a directory we can prove ANOLISA created. If the
            // marker is gone (e.g. the user replaced the extension after
            // enable), leave the directory in place rather than deleting
            // files ANOLISA does not own — but do NOT report success: the
            // extension is still on disk (and may still be auto-discovered
            // by cosh), so keep the receipt as cleanup_failed for the
            // operator to resolve manually.
            Some(dir) if !is_anolisa_owned(&dir) => {
                cleanup_complete = false;
                messages.push(format!(
                    "cosh extension dir {} is not ANOLISA-managed ({COSH_OWNERSHIP_MARKER} marker missing); left in place — remove it manually, then re-run disable",
                    dir.display()
                ));
            }
            Some(dir) => match ctx.ops.remove_tree(&dir) {
                Ok(true) => messages.push(format!("removed cosh extension dir {}", dir.display())),
                Ok(false) => messages.push(format!(
                    "cosh extension dir {} already absent",
                    dir.display()
                )),
                Err(err) => {
                    cleanup_complete = false;
                    messages.push(format!(
                        "failed to remove cosh extension dir {}: {err}",
                        dir.display()
                    ));
                }
            },
            None => messages.push("receipt records no cosh extension directory".to_string()),
        }

        Ok(DisableReport {
            cleanup_complete,
            messages,
        })
    }
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

/// First cosh-family binary found on PATH, honoring the `COSH_BIN`
/// override (which may name any executable, not only the defaults).
fn detect_cosh_binary() -> Option<PathBuf> {
    if let Some(bin) = std::env::var("COSH_BIN").ok().filter(|s| !s.is_empty()) {
        return find_binary_in_path(&bin);
    }
    COSH_BINARIES
        .iter()
        .find_map(|name| find_binary_in_path(name))
}

/// True when `dir` carries ANOLISA's ownership marker, proving ANOLISA
/// created (and may replace/remove) it.
fn is_anolisa_owned(dir: &Path) -> bool {
    dir.join(COSH_OWNERSHIP_MARKER).is_file()
}

/// True when `dir` has no entries (a fresh, safe-to-replace destination).
/// A read error is treated as "not empty" so we fail closed and refuse to
/// clobber a directory we could not inspect.
fn dir_is_empty(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|mut entries| entries.next().is_none())
        .unwrap_or(false)
}

/// Resolve the cosh home directory: `COSH_HOME`, else
/// `<user_home>/.copilot-shell`. Trailing slashes are trimmed.
fn cosh_home(user_home: Option<&Path>) -> Option<PathBuf> {
    if let Some(h) = std::env::var_os("COSH_HOME") {
        let s = h.to_string_lossy();
        let trimmed = s.trim_end_matches('/');
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    user_home.map(|h| h.join(".copilot-shell"))
}

/// Destination extension directory `<cosh_home>/extensions/<plugin_id>`.
fn extension_dir(bundle: &AdapterBundle, ctx: &DriverCtx) -> Result<PathBuf, AdapterError> {
    let home = cosh_home(ctx.user_home.as_deref()).ok_or_else(|| AdapterError::FrameworkCli {
        program: "cosh".to_string(),
        reason: "cannot resolve cosh home (no $HOME and no COSH_HOME)".to_string(),
    })?;
    let id = bundle
        .plugin_id
        .clone()
        .unwrap_or_else(|| ctx.component.clone());
    Ok(home.join("extensions").join(id))
}

/// Extract the extension directory path from a receipt's external-path
/// resource.
fn claim_extension_dir(claim: &AdapterClaim) -> Option<PathBuf> {
    let id = match &claim.driver_payload {
        DriverPayload::Cosh(c) => c.extension_dir_resource.as_str(),
        _ => return None,
    };
    claim.resource(id).and_then(|res| match &res.kind {
        ClaimResourceKind::ExternalPath { path } => Some(path.clone()),
        _ => None,
    })
}

/// Build the `ResourceBundleMatches` condition by re-digesting the resource
/// root and comparing to the enable-time digest.
fn bundle_match_condition(claim: &AdapterClaim) -> AdapterCondition {
    let kind = AdapterConditionKind::ResourceBundleMatches;
    match (&claim.bundle_digest, digest_tree(&claim.resource_root)) {
        (Some(recorded), Some(current)) if recorded == &current => AdapterCondition {
            kind,
            status: ConditionStatus::True,
            reason: None,
            resource: None,
        },
        (Some(_), Some(_)) => AdapterCondition {
            kind,
            status: ConditionStatus::False,
            reason: Some("resource bundle changed since enable".to_string()),
            resource: None,
        },
        _ => AdapterCondition {
            kind,
            status: ConditionStatus::Unknown,
            reason: Some("no digest recorded or resource root unavailable".to_string()),
            resource: None,
        },
    }
}

/// Roll the detect and tree-present signals into a summary, honoring a
/// `cleanup_failed` receipt.
fn summarize(
    claim_status: ClaimStatus,
    framework_detected: bool,
    tree_present: bool,
) -> AdapterSummary {
    if claim_status == ClaimStatus::CleanupFailed {
        return AdapterSummary::CleanupFailed;
    }
    if !tree_present {
        return AdapterSummary::Degraded;
    }
    if !framework_detected {
        return AdapterSummary::Degraded;
    }
    AdapterSummary::Healthy
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::driver::{AdapterOps, CliOutput, FrameworkCommand};
    use std::sync::{Mutex, MutexGuard};

    /// Serializes cosh env mutation across tests in this module.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        _lock: MutexGuard<'static, ()>,
        saved_bin: Option<std::ffi::OsString>,
        saved_home: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn acquire() -> Self {
            let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let g = Self {
                _lock: lock,
                saved_bin: std::env::var_os("COSH_BIN"),
                saved_home: std::env::var_os("COSH_HOME"),
            };
            // SAFETY: guard holds ENV_LOCK for the test's duration.
            unsafe {
                std::env::remove_var("COSH_BIN");
                std::env::remove_var("COSH_HOME");
            }
            g
        }
        fn set_home(&self, home: &Path) {
            // SAFETY: guard holds ENV_LOCK.
            unsafe { std::env::set_var("COSH_HOME", home) }
        }
        /// Force binary detection to miss, isolating the test from any real
        /// cosh-family CLI on the host PATH.
        fn set_bin_absent(&self) {
            // SAFETY: guard holds ENV_LOCK.
            unsafe { std::env::set_var("COSH_BIN", "cosh-does-not-exist-xyz-123") }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: guard holds ENV_LOCK until restore completes.
            unsafe {
                match &self.saved_bin {
                    Some(v) => std::env::set_var("COSH_BIN", v),
                    None => std::env::remove_var("COSH_BIN"),
                }
                match &self.saved_home {
                    Some(v) => std::env::set_var("COSH_HOME", v),
                    None => std::env::remove_var("COSH_HOME"),
                }
            }
        }
    }

    /// Recording ops that apply real filesystem effects for copy/remove
    /// (so status/disable behavior can be asserted) but reject CLI/symlink
    /// calls the cosh driver must never make.
    struct FsOps;
    impl AdapterOps for FsOps {
        fn run_framework_cli(&self, _: FrameworkCommand) -> Result<CliOutput, AdapterError> {
            panic!("cosh driver must not spawn a framework CLI");
        }
        fn copy_tree(&self, src: &Path, dst: &Path) -> Result<(), AdapterError> {
            copy_dir(src, dst).map_err(|source| AdapterError::Io {
                path: dst.to_path_buf(),
                source,
            })
        }
        fn copy_file(&self, _: &Path, _: &Path) -> Result<(), AdapterError> {
            unimplemented!()
        }
        fn remove_tree(&self, path: &Path) -> Result<bool, AdapterError> {
            if !path.exists() {
                return Ok(false);
            }
            std::fs::remove_dir_all(path).map_err(|source| AdapterError::Io {
                path: path.to_path_buf(),
                source,
            })?;
            Ok(true)
        }
        fn write_file(&self, path: &Path, contents: &[u8]) -> Result<(), AdapterError> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|source| AdapterError::Io {
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
            std::fs::write(path, contents).map_err(|source| AdapterError::Io {
                path: path.to_path_buf(),
                source,
            })
        }
        fn create_symlink(&self, _: &Path, _: &Path) -> Result<(), AdapterError> {
            panic!("cosh driver must not create symlinks");
        }
    }

    fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let s = entry.path();
            let d = dst.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                copy_dir(&s, &d)?;
            } else {
                std::fs::copy(&s, &d)?;
            }
        }
        Ok(())
    }

    fn staged_resource_root(dir: &Path) -> PathBuf {
        let root = dir.join("common");
        std::fs::create_dir_all(root.join("hooks")).expect("mkdir");
        std::fs::write(root.join(COSH_MANIFEST), br#"{"name":"tokenless"}"#).expect("manifest");
        std::fs::write(root.join("hooks/run-hook.sh"), b"#!/bin/sh\n").expect("hook");
        root
    }

    fn ctx<'a>(
        resource_root: &Path,
        user_home: &Path,
        ops: &'a FsOps,
        layout: &'a anolisa_platform::fs_layout::FsLayout,
    ) -> DriverCtx<'a> {
        DriverCtx {
            component: "tokenless".to_string(),
            framework: "cosh".to_string(),
            layout,
            resource_root: resource_root.to_path_buf(),
            user_home: Some(user_home.to_path_buf()),
            declared_plugin_id: Some("tokenless".to_string()),
            adapter_type: Some("extension".to_string()),
            declared_skills: Vec::new(),
            declared_config: Vec::new(),
            declared_bundle_entry: None,
            dry_run: false,
            ops,
        }
    }

    #[test]
    fn enable_status_disable_copies_and_removes_only_extension_dir() {
        let guard = EnvGuard::acquire();
        let tmp = tempfile::tempdir().expect("tempdir");
        let user_home = tmp.path().join("home");
        std::fs::create_dir_all(&user_home).expect("home");
        let cosh = tmp.path().join("cosh-home");
        guard.set_home(&cosh);
        // A pre-existing sibling extension must survive disable.
        let sibling = cosh.join("extensions").join("other");
        std::fs::create_dir_all(&sibling).expect("sibling");
        std::fs::write(sibling.join("keep.txt"), b"keep").expect("keep");

        let resource_root = staged_resource_root(tmp.path());
        let ops = FsOps;
        let layout = anolisa_platform::fs_layout::FsLayout::user(user_home.clone());
        let ctx = ctx(&resource_root, &user_home, &ops, &layout);
        let driver = CoshDriver::new();

        let bundle = driver.read_bundle(&ctx).expect("read bundle");
        assert_eq!(bundle.plugin_id.as_deref(), Some("tokenless"));

        let claim = driver.prepare_enable(&bundle, &ctx).expect("claim");
        driver.apply_enable(&claim, &ctx).expect("apply");

        let ext_dir = cosh.join("extensions").join("tokenless");
        assert!(ext_dir.join(COSH_MANIFEST).is_file(), "manifest copied");
        assert!(ext_dir.join("hooks/run-hook.sh").is_file(), "tree copied");

        let report = driver.status(&claim, &ctx).expect("status");
        assert_eq!(report.summary, AdapterSummary::Healthy);
        assert!(
            report
                .conditions
                .iter()
                .any(|c| c.kind == AdapterConditionKind::TreePresent
                    && c.status == ConditionStatus::True)
        );

        let disabled = driver.disable(&claim, &ctx).expect("disable");
        assert!(disabled.cleanup_complete);
        assert!(!ext_dir.exists(), "extension dir removed");
        assert!(
            sibling.join("keep.txt").is_file(),
            "disable must not touch other extensions"
        );
    }

    #[test]
    fn enable_refuses_to_overwrite_non_anolisa_extension() {
        let guard = EnvGuard::acquire();
        let tmp = tempfile::tempdir().expect("tempdir");
        let user_home = tmp.path().join("home");
        std::fs::create_dir_all(&user_home).expect("home");
        let cosh = tmp.path().join("cosh-home");
        guard.set_home(&cosh);
        // A user-installed extension of the same name, WITHOUT our marker.
        let ext_dir = cosh.join("extensions").join("tokenless");
        std::fs::create_dir_all(&ext_dir).expect("ext dir");
        std::fs::write(ext_dir.join("user-file"), b"precious user data").expect("user file");

        let resource_root = staged_resource_root(tmp.path());
        let ops = FsOps;
        let layout = anolisa_platform::fs_layout::FsLayout::user(user_home.clone());
        let ctx = ctx(&resource_root, &user_home, &ops, &layout);
        let driver = CoshDriver::new();

        let bundle = driver.read_bundle(&ctx).expect("read bundle");
        let claim = driver.prepare_enable(&bundle, &ctx).expect("claim");
        let err = driver
            .apply_enable(&claim, &ctx)
            .expect_err("must refuse to clobber non-ANOLISA extension");
        assert!(
            matches!(err, AdapterError::InvalidAdapterInput { .. }),
            "got {err:?}"
        );
        // The user's file must be untouched.
        assert_eq!(
            std::fs::read_to_string(ext_dir.join("user-file")).expect("user file kept"),
            "precious user data"
        );
    }

    #[test]
    fn read_bundle_rejects_missing_manifest() {
        let _guard = EnvGuard::acquire();
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("common");
        std::fs::create_dir_all(&root).expect("mkdir");
        std::fs::write(root.join("stray.txt"), b"x").expect("write");
        let user_home = tmp.path().join("home");
        let ops = FsOps;
        let layout = anolisa_platform::fs_layout::FsLayout::user(user_home.clone());
        let ctx = ctx(&root, &user_home, &ops, &layout);
        let err = CoshDriver::new()
            .read_bundle(&ctx)
            .expect_err("missing manifest must fail");
        assert!(matches!(err, AdapterError::BundleInvalid { .. }));
    }

    #[test]
    fn detect_uses_home_when_cli_absent() {
        let guard = EnvGuard::acquire();
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tmp.path().join(".copilot-shell");
        std::fs::create_dir_all(&home).expect("home");
        guard.set_home(&home);
        guard.set_bin_absent();
        let result = CoshDriver::new().detect(&HostEnv {
            user_home: Some(tmp.path().to_path_buf()),
        });
        assert!(result.detected, "existing home is a weak detect signal");
    }

    #[test]
    fn detect_false_without_cli_or_home() {
        let guard = EnvGuard::acquire();
        guard.set_bin_absent();
        let tmp = tempfile::tempdir().expect("tempdir");
        let result = CoshDriver::new().detect(&HostEnv {
            user_home: Some(tmp.path().join("nonexistent-home")),
        });
        assert!(!result.detected);
    }
}
