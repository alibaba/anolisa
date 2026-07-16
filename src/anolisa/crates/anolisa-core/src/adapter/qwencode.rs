//! Qwen Code framework driver.
//!
//! Qwen Code discovers extensions by scanning well-known directories, so
//! its adapter is *extension*-typed (`adapter_type = "extension"`), not a
//! CLI-registered plugin: `enable` copies the component's extension tree
//! into `<qwen_home>/extensions/<plugin_id>/` and `disable` removes only
//! that directory. No framework CLI is spawned — enable/disable/status
//! are pure filesystem operations mediated by the Manager's
//! [`AdapterOps`](super::driver::AdapterOps).
//!
//! The per-user extension dir ANOLISA takes over is distinct from the
//! system-level auto-discovery tree an RPM may ship under
//! `/usr/share/anolisa/extensions/<component>/`; `disable` never touches
//! the latter, which is package-owned, not receipt-owned.
//!
//! Env contract (used by detection and home resolution, and by tests to
//! point at a scratch home): `QWEN_BIN` overrides the detected binary
//! name; `QWEN_HOME` overrides the Qwen Code home directory (default
//! `<user_home>/.qwen`).

use std::path::{Path, PathBuf};

use super::AdapterError;
use super::claim::{
    AdapterClaim, CLAIM_SCHEMA_VERSION, ClaimResource, ClaimResourceKind, ClaimStatus,
    DRIVER_SCHEMA_VERSION, DriverPayload, QwenCodeClaim,
};
use super::driver::{
    AdapterBundle, AdapterCondition, AdapterConditionKind, AdapterStatusReport, AdapterSummary,
    ClaimResourceRef, ConditionStatus, DetectResult, DisableReport, DriverCtx, DriverPlan,
    FrameworkDriver, HostEnv, find_binary_in_path,
};
use super::util::{bool_status, digest_tree, now_iso8601};

/// Candidate binary names that indicate Qwen Code is installed.
const QWEN_BINARIES: &[&str] = &["qwen"];

/// Native manifest inside a Qwen Code extension bundle. Its presence is
/// what makes a directory a valid Qwen Code extension.
const QWEN_MANIFEST: &str = "qwen-extension.json";

/// Ownership marker ANOLISA drops inside the delivered extension
/// directory. Its presence proves the directory is ANOLISA-managed, so a
/// re-enable may safely replace it and disable may safely remove it —
/// without that proof, enable refuses to overwrite and disable leaves the
/// directory alone, so a user-installed extension of the same name is
/// never destroyed.
const QWEN_OWNERSHIP_MARKER: &str = ".anolisa-adapter";

/// Resource id used in QwenCode receipts.
const RES_EXTENSION_DIR: &str = "qwencode_extension_dir";

/// Qwen Code driver. Stateless; all per-operation context arrives via
/// [`DriverCtx`].
pub struct QwenCodeDriver;

impl QwenCodeDriver {
    /// Construct the driver.
    pub fn new() -> Self {
        Self
    }
}

impl Default for QwenCodeDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameworkDriver for QwenCodeDriver {
    fn name(&self) -> &'static str {
        "qwencode"
    }

    fn detect(&self, env: &HostEnv) -> DetectResult {
        // A qwen CLI on PATH is the strong signal. Because the extension
        // model only drops files (no CLI needed to enable), an existing
        // Qwen Code home is accepted as a weaker signal so a user who
        // installed Qwen Code without leaving its launcher on PATH can
        // still enable.
        if let Some(path) = detect_qwen_binary() {
            return DetectResult {
                detected: true,
                reason: format!("qwen CLI found at {}", path.display()),
            };
        }
        match qwen_home(env.user_home.as_deref()).filter(|h| h.exists()) {
            Some(home) => DetectResult {
                detected: true,
                reason: format!(
                    "qwen CLI not on PATH, but home {} exists (extension can still be delivered)",
                    home.display()
                ),
            },
            None => DetectResult {
                detected: false,
                reason: "qwen not detected: no qwen on PATH and no ~/.qwen".to_string(),
            },
        }
    }

    fn allowed_external_roots(&self, ctx: &DriverCtx) -> Vec<PathBuf> {
        // The only external root Qwen Code writes is its own home
        // directory.
        qwen_home(ctx.user_home.as_deref()).into_iter().collect()
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
            .unwrap_or(QWEN_MANIFEST);
        if !root.join(manifest).is_file() {
            return Err(AdapterError::BundleInvalid {
                root: root.clone(),
                reason: format!(
                    "Qwen Code extension manifest '{manifest}' missing from resource root"
                ),
            });
        }
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
            "deliver Qwen Code extension from {} to {}",
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
            purpose: "qwencode_extension_dir".to_string(),
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
            driver_payload: DriverPayload::QwenCode(QwenCodeClaim {
                extension_dir_resource: RES_EXTENSION_DIR.to_string(),
            }),
        })
    }

    fn apply_enable(&self, claim: &AdapterClaim, ctx: &DriverCtx) -> Result<(), AdapterError> {
        let dst = claim_extension_dir(claim).ok_or_else(|| AdapterError::BundleInvalid {
            root: claim.resource_root.clone(),
            reason: "Qwen Code receipt has no extension directory resource".to_string(),
        })?;
        // Never clobber a directory ANOLISA does not own. A first enable
        // onto a user-installed extension of the same name would otherwise
        // silently destroy the user's files. Only replace when the
        // directory is empty/absent or carries our ownership marker.
        if dst.exists() && !is_anolisa_owned(&dst) && !dir_is_empty(&dst) {
            return Err(AdapterError::InvalidAdapterInput {
                component: ctx.component.clone(),
                framework: self.name().to_string(),
                reason: format!(
                    "refusing to overwrite existing non-ANOLISA Qwen Code extension at {} (no {QWEN_OWNERSHIP_MARKER} marker); remove it manually to enable",
                    dst.display()
                ),
            });
        }
        ctx.ops.remove_tree(&dst)?;
        ctx.ops.write_file(
            &dst.join(QWEN_OWNERSHIP_MARKER),
            format!("anolisa:{}:{}", claim.component, claim.framework).as_bytes(),
        )?;
        ctx.ops.copy_tree(&claim.resource_root, &dst)?;
        Ok(())
    }

    fn status(
        &self,
        claim: &AdapterClaim,
        ctx: &DriverCtx,
    ) -> Result<AdapterStatusReport, AdapterError> {
        let detect = self.detect(&HostEnv {
            user_home: ctx.user_home.clone(),
        });
        let mut conditions = Vec::new();

        conditions.push(AdapterCondition {
            kind: AdapterConditionKind::FrameworkDetected,
            status: bool_status(detect.detected),
            reason: Some(detect.reason),
            resource: None,
        });

        let tree_present = match claim_extension_dir(claim) {
            Some(dir) => dir.is_dir() && dir.join(QWEN_MANIFEST).is_file(),
            None => false,
        };
        let tree_reason = if tree_present {
            None
        } else {
            Some("extension tree missing or manifest absent".to_string())
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
                    "Qwen Code extension dir {} already absent",
                    dir.display()
                ));
            }
            Some(dir) if !is_anolisa_owned(&dir) => {
                cleanup_complete = false;
                messages.push(format!(
                    "Qwen Code extension dir {} is not ANOLISA-managed ({QWEN_OWNERSHIP_MARKER} marker missing); left in place — remove it manually, then re-run disable",
                    dir.display()
                ));
            }
            Some(dir) => match ctx.ops.remove_tree(&dir) {
                Ok(true) => {
                    messages.push(format!("removed Qwen Code extension dir {}", dir.display()))
                }
                Ok(false) => messages.push(format!(
                    "Qwen Code extension dir {} already absent",
                    dir.display()
                )),
                Err(err) => {
                    cleanup_complete = false;
                    messages.push(format!(
                        "failed to remove Qwen Code extension dir {}: {err}",
                        dir.display()
                    ));
                }
            },
            None => messages.push("receipt records no Qwen Code extension directory".to_string()),
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

/// First qwen-family binary found on PATH, honoring the `QWEN_BIN`
/// override.
fn detect_qwen_binary() -> Option<PathBuf> {
    if let Some(bin) = std::env::var("QWEN_BIN").ok().filter(|s| !s.is_empty()) {
        return find_binary_in_path(&bin);
    }
    QWEN_BINARIES
        .iter()
        .find_map(|name| find_binary_in_path(name))
}

/// True when `dir` carries ANOLISA's ownership marker.
fn is_anolisa_owned(dir: &Path) -> bool {
    dir.join(QWEN_OWNERSHIP_MARKER).is_file()
}

/// True when `dir` has no entries.
fn dir_is_empty(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|mut entries| entries.next().is_none())
        .unwrap_or(false)
}

/// Resolve the Qwen Code home directory: `QWEN_HOME`, else
/// `<user_home>/.qwen`. Trailing slashes are trimmed.
fn qwen_home(user_home: Option<&Path>) -> Option<PathBuf> {
    if let Some(h) = std::env::var_os("QWEN_HOME") {
        let s = h.to_string_lossy();
        let trimmed = s.trim_end_matches('/');
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    user_home.map(|h| h.join(".qwen"))
}

/// Destination extension directory `<qwen_home>/extensions/<plugin_id>`.
fn extension_dir(bundle: &AdapterBundle, ctx: &DriverCtx) -> Result<PathBuf, AdapterError> {
    let home = qwen_home(ctx.user_home.as_deref()).ok_or_else(|| AdapterError::FrameworkCli {
        program: "qwen".to_string(),
        reason: "cannot resolve Qwen Code home (no $HOME and no QWEN_HOME)".to_string(),
    })?;
    let id = bundle
        .plugin_id
        .clone()
        .unwrap_or_else(|| ctx.component.clone());
    Ok(home.join("extensions").join(id))
}

/// Extract the extension directory path from a receipt's external-path
/// resources.
fn claim_extension_dir(claim: &AdapterClaim) -> Option<PathBuf> {
    claim.resources.iter().find_map(|r| {
        let ClaimResourceKind::ExternalPath { path } = &r.kind else {
            return None;
        };
        (r.id == RES_EXTENSION_DIR).then(|| path.clone())
    })
}

/// Roll signals into a summary. Healthy requires the framework detected
/// and the extension tree present.
fn summarize(claim_status: ClaimStatus, detected: bool, tree_present: bool) -> AdapterSummary {
    if claim_status == ClaimStatus::CleanupFailed {
        return AdapterSummary::CleanupFailed;
    }
    if !detected {
        return AdapterSummary::Degraded;
    }
    if tree_present {
        AdapterSummary::Healthy
    } else {
        AdapterSummary::Degraded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use crate::adapter::driver::{AdapterOps, CliOutput, FrameworkCommand};

    /// Serialize access to env vars across parallel tests.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        saved_bin: Option<String>,
        saved_home: Option<String>,
    }

    impl EnvGuard {
        fn acquire() -> Self {
            let lock = ENV_LOCK.lock().expect("env lock");
            let saved_bin = std::env::var("QWEN_BIN").ok();
            let saved_home = std::env::var("QWEN_HOME").ok();
            // Force absent by default; tests opt in.
            unsafe {
                std::env::set_var("QWEN_BIN", "/nonexistent/qwen");
            }
            unsafe {
                std::env::remove_var("QWEN_HOME");
            }
            Self {
                _lock: lock,
                saved_bin,
                saved_home,
            }
        }

        fn set_home(&self, path: &Path) {
            unsafe {
                std::env::set_var("QWEN_HOME", path);
            }
        }

        fn set_bin_present(&self, path: &Path) {
            unsafe {
                std::env::set_var("QWEN_BIN", path);
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.saved_bin {
                Some(v) => unsafe { std::env::set_var("QWEN_BIN", v) },
                None => unsafe { std::env::remove_var("QWEN_BIN") },
            }
            match &self.saved_home {
                Some(v) => unsafe { std::env::set_var("QWEN_HOME", v) },
                None => unsafe { std::env::remove_var("QWEN_HOME") },
            }
        }
    }

    struct FsOps;

    impl AdapterOps for FsOps {
        fn run_framework_cli(&self, _: FrameworkCommand) -> Result<CliOutput, AdapterError> {
            panic!("qwencode driver must not spawn a framework CLI");
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
        fn create_symlink(&self, _link: &Path, _target: &Path) -> Result<(), AdapterError> {
            unimplemented!()
        }
        fn read_file(&self, _path: &Path) -> Result<Option<Vec<u8>>, AdapterError> {
            unimplemented!()
        }
    }

    fn copy_dir(src: &Path, dst: &Path) -> Result<(), std::io::Error> {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let ty = entry.file_type()?;
            let dst_path = dst.join(entry.file_name());
            if ty.is_dir() {
                copy_dir(&entry.path(), &dst_path)?;
            } else {
                std::fs::copy(entry.path(), &dst_path)?;
            }
        }
        Ok(())
    }

    fn staged_resource_root(base: &Path) -> PathBuf {
        let dir = base.join("qwencode");
        std::fs::create_dir_all(&dir).expect("resource dir");
        std::fs::write(
            dir.join("qwen-extension.json"),
            br#"{"name":"tokenless","version":"0.6.0"}"#,
        )
        .expect("manifest");
        std::fs::create_dir_all(dir.join("hooks")).expect("hooks dir");
        std::fs::write(dir.join("hooks/run-hook.sh"), b"#!/bin/sh\n").expect("hook");
        dir
    }

    fn ctx<'a>(
        resource_root: &'a Path,
        user_home: &'a Path,
        ops: &'a dyn AdapterOps,
        layout: &'a anolisa_platform::fs_layout::FsLayout,
    ) -> DriverCtx<'a> {
        DriverCtx {
            component: "tokenless".to_string(),
            framework: "qwencode".to_string(),
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
    fn enable_status_disable_round_trip() {
        let guard = EnvGuard::acquire();
        let tmp = tempfile::tempdir().expect("tempdir");
        let user_home = tmp.path().join("home");
        std::fs::create_dir_all(&user_home).expect("home");
        let qwen = tmp.path().join("qwen-home");
        guard.set_home(&qwen);
        let sibling = qwen.join("extensions").join("other");
        std::fs::create_dir_all(&sibling).expect("sibling");
        std::fs::write(sibling.join("keep.txt"), b"keep").expect("keep");

        let resource_root = staged_resource_root(tmp.path());
        let ops = FsOps;
        let layout = anolisa_platform::fs_layout::FsLayout::user(user_home.clone());
        let ctx = ctx(&resource_root, &user_home, &ops, &layout);
        let driver = QwenCodeDriver::new();

        let bundle = driver.read_bundle(&ctx).expect("read bundle");
        assert_eq!(bundle.plugin_id.as_deref(), Some("tokenless"));

        let claim = driver.prepare_enable(&bundle, &ctx).expect("claim");
        driver.apply_enable(&claim, &ctx).expect("apply");

        let ext_dir = qwen.join("extensions").join("tokenless");
        assert!(ext_dir.join(QWEN_MANIFEST).is_file(), "manifest copied");
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
        let qwen = tmp.path().join("qwen-home");
        guard.set_home(&qwen);
        let ext_dir = qwen.join("extensions").join("tokenless");
        std::fs::create_dir_all(&ext_dir).expect("ext dir");
        std::fs::write(ext_dir.join("user-file"), b"precious user data").expect("user file");

        let resource_root = staged_resource_root(tmp.path());
        let ops = FsOps;
        let layout = anolisa_platform::fs_layout::FsLayout::user(user_home.clone());
        let ctx = ctx(&resource_root, &user_home, &ops, &layout);
        let driver = QwenCodeDriver::new();

        let bundle = driver.read_bundle(&ctx).expect("read bundle");
        let claim = driver.prepare_enable(&bundle, &ctx).expect("claim");
        let err = driver
            .apply_enable(&claim, &ctx)
            .expect_err("must refuse to clobber non-ANOLISA extension");
        assert!(
            matches!(err, AdapterError::InvalidAdapterInput { .. }),
            "got {err:?}"
        );
        assert_eq!(
            std::fs::read_to_string(ext_dir.join("user-file")).expect("user file kept"),
            "precious user data"
        );
    }

    #[test]
    fn read_bundle_rejects_missing_manifest() {
        let _guard = EnvGuard::acquire();
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("qwencode");
        std::fs::create_dir_all(&root).expect("mkdir");
        std::fs::write(root.join("stray.txt"), b"x").expect("write");
        let user_home = tmp.path().join("home");
        let ops = FsOps;
        let layout = anolisa_platform::fs_layout::FsLayout::user(user_home.clone());
        let ctx = ctx(&root, &user_home, &ops, &layout);
        let err = QwenCodeDriver::new()
            .read_bundle(&ctx)
            .expect_err("missing manifest must fail");
        assert!(matches!(err, AdapterError::BundleInvalid { .. }));
    }

    #[test]
    fn detect_uses_home_when_cli_absent() {
        let guard = EnvGuard::acquire();
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tmp.path().join(".qwen");
        std::fs::create_dir_all(&home).expect("home");
        guard.set_home(&home);
        let result = QwenCodeDriver::new().detect(&HostEnv {
            user_home: Some(tmp.path().to_path_buf()),
        });
        assert!(result.detected, "existing home is a weak detect signal");
    }

    #[test]
    fn detect_false_without_cli_or_home() {
        let guard = EnvGuard::acquire();
        guard.set_bin_present(Path::new("/nonexistent/qwen"));
        let tmp = tempfile::tempdir().expect("tempdir");
        let result = QwenCodeDriver::new().detect(&HostEnv {
            user_home: Some(tmp.path().join("nonexistent-home")),
        });
        assert!(!result.detected);
    }
}
