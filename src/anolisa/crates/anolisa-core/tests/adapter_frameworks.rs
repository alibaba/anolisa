//! End-to-end adapter manager tests for the Cosh, Codex, Claude Code, and
//! Qoder drivers.
//!
//! Each test drives the real [`AdapterManager`] against a staged component
//! contract + resource bundle. Codex and Claude Code use shell-script fake
//! CLIs that record their argv (so we can assert the exact framework
//! commands ANOLISA issues) and keep enough state for `status` to verify.
//! Cosh is CLI-less: its enable/disable are pure filesystem operations, so
//! the test asserts it copies and removes only its own extension directory.
//!
//! All three drivers read process-global env (`CODEX_BIN`, `CLAUDE_BIN`,
//! `COSH_HOME`, `XDG_DATA_HOME`, …), so every test serializes on
//! [`ENV_LOCK`], starts from a cleared env, and restores it on exit.
#![cfg(unix)]

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use anolisa_core::adapter::AdapterError;
use anolisa_core::adapter::claim::{ClaimResourceKind, ClaimStatus};
use anolisa_core::adapter::driver::{AdapterConditionKind, AdapterSummary, ConditionStatus};
use anolisa_core::adapter::manager::{AdapterManager, EnableOutcome};
use anolisa_platform::fs_layout::FsLayout;

const COMPONENT: &str = "tokenless";

/// Serializes process-global env mutation across tests.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Env keys every test clears on entry and restores on drop, so a test
/// never observes another test's half-applied contract.
const MANAGED_ENV: &[&str] = &[
    "CODEX_BIN",
    "CLAUDE_BIN",
    "COSH_BIN",
    "COSH_HOME",
    "XDG_DATA_HOME",
    "FAKE_CODEX_LOG",
    "FAKE_CODEX_STATE",
    "FAKE_CODEX_FAIL",
    "FAKE_CLAUDE_LOG",
    "FAKE_CLAUDE_STATE",
    "QODERCLI_BIN",
    "FAKE_QODER_LOG",
    "FAKE_QODER_CACHE",
    "FAKE_QODER_FAIL",
];

struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    saved: Vec<(&'static str, Option<OsString>)>,
}

impl EnvGuard {
    fn acquire() -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let saved = MANAGED_ENV
            .iter()
            .map(|k| (*k, std::env::var_os(k)))
            .collect();
        let guard = Self { _lock: lock, saved };
        for k in MANAGED_ENV {
            // SAFETY: guard holds ENV_LOCK for the whole test.
            unsafe { std::env::remove_var(k) };
        }
        guard
    }

    fn set(&self, key: &str, value: &Path) {
        // SAFETY: guard holds ENV_LOCK.
        unsafe { std::env::set_var(key, value) };
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (k, v) in &self.saved {
            // SAFETY: guard holds ENV_LOCK until restore completes.
            unsafe {
                match v {
                    Some(val) => std::env::set_var(k, val),
                    None => std::env::remove_var(k),
                }
            }
        }
    }
}

/// A staged world: system-mode layout under a temp prefix, a seeded
/// `installed.toml` + component contract for one framework, and the
/// framework's resource bundle.
struct World {
    _root: tempfile::TempDir,
    prefix: PathBuf,
    layout: FsLayout,
    user_home: PathBuf,
    resource_root: PathBuf,
}

impl World {
    fn manager(&self) -> AdapterManager {
        AdapterManager::new(
            self.layout.clone(),
            Some(self.user_home.clone()),
            "tester".to_string(),
        )
    }

    fn load_state(&self) -> anolisa_core::state_store::StateStore {
        anolisa_core::state_store::StateStore::load(
            &self.layout.state_dir.join("installed.toml"),
            anolisa_platform::privilege::effective_uid(),
        )
        .expect("load state")
    }
}

/// Stage a component contract declaring `framework`/`adapter_type` with the
/// given `dest`, plus the resource bundle written by `stage_bundle`.
fn stage(framework: &str, adapter_type: &str, dest: &str, stage_bundle: impl Fn(&Path)) -> World {
    let root = tempfile::tempdir().expect("tempdir");
    let prefix = root.path().to_path_buf();
    let layout = FsLayout::system(Some(prefix.clone()));
    let user_home = prefix.join("home");
    std::fs::create_dir_all(&user_home).expect("home");

    // Resolve the resource root the same way the manager will (expand the
    // dest against the system datadir) and populate it.
    let resource_root = expand_dest(dest, &layout.datadir);
    std::fs::create_dir_all(&resource_root).expect("resource root");
    stage_bundle(&resource_root);

    seed_component(&layout, &prefix, framework, adapter_type, dest);

    World {
        _root: root,
        prefix,
        layout,
        user_home,
        resource_root,
    }
}

/// Seed `installed.toml` (component installed) plus the component contract
/// declaring one adapter with the given `framework`/`adapter_type`/`dest`.
fn seed_component(
    layout: &FsLayout,
    prefix: &Path,
    framework: &str,
    adapter_type: &str,
    dest: &str,
) {
    let state_path = layout.state_dir.join("installed.toml");
    std::fs::create_dir_all(state_path.parent().unwrap()).expect("state dir");
    std::fs::write(
        &state_path,
        format!(
            r#"schema_version = 2
updated_at = "2026-07-04T00:00:00Z"
install_mode = "system"
prefix = "{prefix}"
anolisa_version = "0.1.20"

[[objects]]
kind = "component"
name = "{COMPONENT}"
version = "0.6.0"
status = "installed"
install_backend = "raw"
ownership = "raw_managed"
installed_at = "2026-07-04T00:00:00Z"
"#,
            prefix = prefix.display(),
        ),
    )
    .expect("seed state");

    let manifest_path = layout
        .state_dir
        .join("component-manifests")
        .join(COMPONENT)
        .join("component.toml");
    std::fs::create_dir_all(manifest_path.parent().unwrap()).expect("manifest dir");
    std::fs::write(
        &manifest_path,
        format!(
            r#"[component]
name = "{COMPONENT}"
version = "0.6.0"

[component.layout]
modes = ["system"]

[[adapters]]
framework = "{framework}"
adapter_type = "{adapter_type}"
plugin_id = "{COMPONENT}"
dest = "{dest}"
"#
        ),
    )
    .expect("seed contract");
}

/// Minimal `{datadir}`/`{component}` expansion for staging (the manager's
/// real expansion is exercised separately).
fn expand_dest(dest: &str, datadir: &Path) -> PathBuf {
    let expanded = dest
        .replace("{datadir}", &datadir.to_string_lossy())
        .replace("{component}", COMPONENT);
    PathBuf::from(expanded)
}

fn write_exec(path: &Path, body: &str) {
    std::fs::write(path, body).expect("write script");
    let mut perms = std::fs::metadata(path).expect("meta").permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).expect("chmod");
}

// ---------------------------------------------------------------------------
// Cosh
// ---------------------------------------------------------------------------

fn stage_cosh_bundle(root: &Path) {
    std::fs::create_dir_all(root.join("hooks")).expect("hooks");
    std::fs::write(root.join("cosh-extension.json"), br#"{"name":"tokenless"}"#).expect("manifest");
    std::fs::write(root.join("hooks/run-hook.sh"), b"#!/bin/sh\n").expect("hook");
}

#[test]
fn cosh_enable_status_disable_touches_only_extension_dir() {
    let guard = EnvGuard::acquire();
    let world = stage(
        "cosh",
        "extension",
        "{datadir}/adapters/{component}/common/",
        stage_cosh_bundle,
    );
    let cosh_home = world.prefix.join("cosh-home");
    std::fs::create_dir_all(&cosh_home).expect("cosh home");
    guard.set("COSH_HOME", &cosh_home);
    // A sibling extension owned by someone else must survive disable.
    let sibling = cosh_home.join("extensions").join("other");
    std::fs::create_dir_all(&sibling).expect("sibling");
    std::fs::write(sibling.join("keep.txt"), b"keep").expect("keep");

    let manager = world.manager();
    let claim = match manager
        .enable(COMPONENT, Some("cosh"), false)
        .expect("enable")
    {
        EnableOutcome::Enabled(c) => *c,
        EnableOutcome::Planned { .. } => panic!("expected enabled"),
    };
    assert_eq!(claim.adapter_type.as_deref(), Some("extension"));

    let ext_dir = cosh_home.join("extensions").join("tokenless");
    assert!(
        ext_dir.join("cosh-extension.json").is_file(),
        "extension copied"
    );
    assert!(ext_dir.join("hooks/run-hook.sh").is_file(), "tree copied");

    let status = manager.status(Some(COMPONENT)).expect("status");
    assert_eq!(status.entries[0].report.summary, AdapterSummary::Healthy);
    assert!(
        status.entries[0]
            .report
            .conditions
            .iter()
            .any(|c| c.kind == AdapterConditionKind::TreePresent
                && c.status == ConditionStatus::True)
    );

    let disabled = manager
        .disable(COMPONENT, Some("cosh"), false)
        .expect("disable");
    assert!(disabled.claim_removed);
    assert!(!ext_dir.exists(), "extension dir removed");
    assert!(
        sibling.join("keep.txt").is_file(),
        "disable must not touch sibling extensions"
    );
    assert!(
        world
            .load_state()
            .find_adapter_claim(COMPONENT, "cosh")
            .is_none(),
        "receipt gone after disable"
    );
}

#[test]
fn cosh_dry_run_enable_writes_nothing() {
    let guard = EnvGuard::acquire();
    let world = stage(
        "cosh",
        "extension",
        "{datadir}/adapters/{component}/common/",
        stage_cosh_bundle,
    );
    let cosh_home = world.prefix.join("cosh-home");
    std::fs::create_dir_all(&cosh_home).expect("cosh home");
    guard.set("COSH_HOME", &cosh_home);

    let manager = world.manager();
    let outcome = manager
        .enable(COMPONENT, Some("cosh"), true)
        .expect("dry-run");
    match outcome {
        EnableOutcome::Planned { plan, .. } => {
            assert_eq!(plan.framework, "cosh");
            assert!(
                plan.actions
                    .iter()
                    .any(|a| a.contains("deliver cosh extension"))
            );
        }
        EnableOutcome::Enabled(_) => panic!("dry-run must not enable"),
    }
    assert!(
        !cosh_home.join("extensions").join("tokenless").exists(),
        "dry-run must not write the extension dir"
    );
    assert!(
        world
            .load_state()
            .find_adapter_claim(COMPONENT, "cosh")
            .is_none(),
        "dry-run must not persist a receipt"
    );
}

#[test]
fn cosh_disable_keeps_receipt_when_ownership_marker_missing() {
    let guard = EnvGuard::acquire();
    let world = stage(
        "cosh",
        "extension",
        "{datadir}/adapters/{component}/common/",
        stage_cosh_bundle,
    );
    let cosh_home = world.prefix.join("cosh-home");
    std::fs::create_dir_all(&cosh_home).expect("cosh home");
    guard.set("COSH_HOME", &cosh_home);

    let manager = world.manager();
    manager
        .enable(COMPONENT, Some("cosh"), false)
        .expect("enable");
    let ext_dir = cosh_home.join("extensions").join("tokenless");

    // Simulate the ownership marker going missing (user replaced the
    // extension, or a marker write failed after copy).
    std::fs::remove_file(ext_dir.join(".anolisa-adapter")).expect("remove marker");

    // Status must degrade, not report healthy, when ownership is unprovable.
    let status = manager.status(Some(COMPONENT)).expect("status");
    assert_eq!(status.entries[0].report.summary, AdapterSummary::Degraded);

    // Disable must NOT delete a dir it cannot prove it owns, and must NOT
    // report success (the extension is still on disk / auto-discoverable),
    // so the receipt is kept as cleanup_failed.
    let disabled = manager
        .disable(COMPONENT, Some("cosh"), false)
        .expect("disable runs");
    assert!(
        !disabled.claim_removed,
        "receipt kept when ownership unprovable"
    );
    assert!(!disabled.report.cleanup_complete);
    assert!(
        ext_dir.exists(),
        "non-ANOLISA-owned dir must be left in place"
    );
    let claim = world
        .load_state()
        .find_adapter_claim(COMPONENT, "cosh")
        .cloned()
        .expect("receipt kept");
    assert_eq!(claim.status, ClaimStatus::CleanupFailed);
}

// ---------------------------------------------------------------------------
// Codex
// ---------------------------------------------------------------------------

fn stage_codex_bundle(root: &Path) {
    std::fs::create_dir_all(root.join(".codex-plugin")).expect("codex-plugin");
    std::fs::write(
        root.join(".codex-plugin/plugin.json"),
        br#"{"name":"tokenless"}"#,
    )
    .expect("plugin.json");
    std::fs::write(root.join("README.md"), b"codex plugin\n").expect("readme");
}

/// Fake `codex` CLI: appends each argv line to `$FAKE_CODEX_LOG` and keeps
/// marketplace/plugin registries under `$FAKE_CODEX_STATE` so `list`
/// reflects prior `add`/`remove` calls.
fn write_fake_codex(dir: &Path) -> PathBuf {
    let path = dir.join("codex");
    write_exec(
        &path,
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$FAKE_CODEX_LOG"
st="$FAKE_CODEX_STATE"; mkdir -p "$st" 2>/dev/null
if [ "$1" = "plugin" ] && [ "$2" = "marketplace" ]; then
  case "$3" in
    add)
      name=$(sed -n 's/.*"name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$4/.agents/plugins/marketplace.json" | head -n1)
      echo "$name" >> "$st/marketplaces" ;;
    remove)
      [ "$FAKE_CODEX_FAIL" = "remove" ] && { echo "remove boom" >&2; exit 1; }
      [ -f "$st/marketplaces" ] && { grep -vx "$4" "$st/marketplaces" > "$st/m.tmp" 2>/dev/null || true; mv "$st/m.tmp" "$st/marketplaces" 2>/dev/null || true; } ;;
    list) cat "$st/marketplaces" 2>/dev/null || true ;;
  esac
  exit 0
fi
if [ "$1" = "plugin" ]; then
  case "$2" in
    add) echo "$3" >> "$st/plugins" ;;
    remove)
      # FAKE_CODEX_FAIL=remove: fail without removing, so the driver's
      # post-remove verification still finds the plugin registered.
      [ "$FAKE_CODEX_FAIL" = "remove" ] && { echo "remove boom" >&2; exit 1; }
      [ -f "$st/plugins" ] && { grep -vx "$3" "$st/plugins" > "$st/p.tmp" 2>/dev/null || true; mv "$st/p.tmp" "$st/plugins" 2>/dev/null || true; } ;;
    list) cat "$st/plugins" 2>/dev/null || true ;;
  esac
  exit 0
fi
exit 0
"#,
    );
    path
}

fn apply_codex_env(guard: &EnvGuard, world: &World, fake_bin: &Path) -> (PathBuf, PathBuf) {
    let xdg = world.prefix.join("xdg-data");
    std::fs::create_dir_all(&xdg).expect("xdg");
    let log = world.prefix.join("codex.log");
    let state = world.prefix.join("codex-state");
    guard.set("CODEX_BIN", fake_bin);
    guard.set("XDG_DATA_HOME", &xdg);
    guard.set("FAKE_CODEX_LOG", &log);
    guard.set("FAKE_CODEX_STATE", &state);
    let marketplace_root = xdg.join("anolisa").join("codex-marketplace");
    (log, marketplace_root)
}

#[test]
fn codex_enable_records_argv_and_builds_marketplace() {
    let guard = EnvGuard::acquire();
    let world = stage(
        "codex",
        "plugin",
        "{datadir}/adapters/{component}/codex/",
        stage_codex_bundle,
    );
    let fake = write_fake_codex(&world.prefix);
    let (log, marketplace_root) = apply_codex_env(&guard, &world, &fake);

    let manager = world.manager();
    let claim = match manager
        .enable(COMPONENT, Some("codex"), false)
        .expect("enable")
    {
        EnableOutcome::Enabled(c) => *c,
        EnableOutcome::Planned { .. } => panic!("expected enabled"),
    };

    // Marketplace layout on disk.
    let manifest = marketplace_root.join(".agents/plugins/marketplace.json");
    assert!(manifest.is_file(), "marketplace.json written");
    let symlink = marketplace_root.join("tokenless");
    assert_eq!(
        std::fs::read_link(&symlink).expect("symlink"),
        world.resource_root,
        "symlink points at the resource root"
    );

    // Recorded argv: exactly the marketplace-add and plugin-add commands.
    let log_text = std::fs::read_to_string(&log).expect("codex log");
    assert!(
        log_text
            .lines()
            .any(|l| l == format!("plugin marketplace add {}", marketplace_root.display())),
        "must run `plugin marketplace add <root>`: {log_text}"
    );
    assert!(
        log_text
            .lines()
            .any(|l| l == "plugin add tokenless@anolisa-tokenless"),
        "must run `plugin add tokenless@anolisa-tokenless`: {log_text}"
    );

    // Receipt carries the marketplace + symlink + plugin resources.
    assert!(claim.resources.iter().any(|r| matches!(
        &r.kind,
        ClaimResourceKind::FrameworkMarketplace { marketplace, .. } if marketplace == "anolisa-tokenless"
    )));
    assert!(
        claim
            .resources
            .iter()
            .any(|r| matches!(&r.kind, ClaimResourceKind::Symlink { .. }))
    );

    let status = manager.status(Some(COMPONENT)).expect("status");
    assert_eq!(status.entries[0].report.summary, AdapterSummary::Healthy);

    let disabled = manager
        .disable(COMPONENT, Some("codex"), false)
        .expect("disable");
    assert!(disabled.claim_removed);
    assert!(
        !marketplace_root.exists(),
        "marketplace dir removed on disable"
    );
    let log_text = std::fs::read_to_string(&log).expect("codex log");
    assert!(
        log_text
            .lines()
            .any(|l| l == "plugin remove tokenless@anolisa-tokenless"),
        "disable must run `plugin remove`: {log_text}"
    );
    assert!(
        log_text
            .lines()
            .any(|l| l == "plugin marketplace remove anolisa-tokenless"),
        "disable must run `plugin marketplace remove`: {log_text}"
    );
}

#[test]
fn codex_dry_run_enable_writes_nothing() {
    let guard = EnvGuard::acquire();
    let world = stage(
        "codex",
        "plugin",
        "{datadir}/adapters/{component}/codex/",
        stage_codex_bundle,
    );
    let fake = write_fake_codex(&world.prefix);
    let (log, marketplace_root) = apply_codex_env(&guard, &world, &fake);

    let manager = world.manager();
    let outcome = manager
        .enable(COMPONENT, Some("codex"), true)
        .expect("dry-run");
    assert!(matches!(outcome, EnableOutcome::Planned { .. }));
    assert!(
        !marketplace_root.exists(),
        "dry-run must not create marketplace dir"
    );
    assert!(
        !log.exists(),
        "dry-run must not invoke the codex CLI (no log file)"
    );
    assert!(
        world
            .load_state()
            .find_adapter_claim(COMPONENT, "codex")
            .is_none(),
        "dry-run must not persist a receipt"
    );
}

#[test]
fn codex_forged_symlink_target_rejected_by_status() {
    let guard = EnvGuard::acquire();
    let world = stage(
        "codex",
        "plugin",
        "{datadir}/adapters/{component}/codex/",
        stage_codex_bundle,
    );
    let fake = write_fake_codex(&world.prefix);
    apply_codex_env(&guard, &world, &fake);

    let manager = world.manager();
    manager
        .enable(COMPONENT, Some("codex"), false)
        .expect("enable");

    // Tamper: repoint the symlink resource's target at /etc.
    let state_path = world.layout.state_dir.join("installed.toml");
    let mut state = world.load_state();
    {
        let claim = state
            .adapter_claims
            .iter_mut()
            .find(|c| c.component == COMPONENT)
            .expect("claim");
        for res in &mut claim.resources {
            if let ClaimResourceKind::Symlink { target, .. } = &mut res.kind {
                *target = PathBuf::from("/etc/cron.d/evil");
            }
        }
    }
    state.save(&state_path).expect("save tampered state");

    let err = manager
        .status(Some(COMPONENT))
        .expect_err("forged symlink target must be rejected");
    assert!(
        matches!(err, AdapterError::ClaimValidation(_)),
        "got {err:?}"
    );
}

#[test]
fn codex_forged_resource_root_and_symlink_target_rejected() {
    let guard = EnvGuard::acquire();
    let world = stage(
        "codex",
        "plugin",
        "{datadir}/adapters/{component}/codex/",
        stage_codex_bundle,
    );
    let fake = write_fake_codex(&world.prefix);
    apply_codex_env(&guard, &world, &fake);

    let manager = world.manager();
    manager
        .enable(COMPONENT, Some("codex"), false)
        .expect("enable");

    // Forge BOTH the receipt's resource_root and the symlink target to
    // /etc. The symlink target is validated against the trusted layout, not
    // the receipt, so this must still be rejected.
    let state_path = world.layout.state_dir.join("installed.toml");
    let mut state = world.load_state();
    {
        let claim = state
            .adapter_claims
            .iter_mut()
            .find(|c| c.component == COMPONENT)
            .expect("claim");
        claim.resource_root = PathBuf::from("/etc");
        for res in &mut claim.resources {
            if let ClaimResourceKind::Symlink { target, .. } = &mut res.kind {
                *target = PathBuf::from("/etc/cron.d/evil");
            }
        }
    }
    state.save(&state_path).expect("save tampered state");

    let err = manager
        .status(Some(COMPONENT))
        .expect_err("forged resource_root must not authorize a forged symlink target");
    assert!(
        matches!(err, AdapterError::ClaimValidation(_)),
        "got {err:?}"
    );
}

#[test]
fn codex_disable_keeps_receipt_when_cli_removal_fails() {
    let guard = EnvGuard::acquire();
    let world = stage(
        "codex",
        "plugin",
        "{datadir}/adapters/{component}/codex/",
        stage_codex_bundle,
    );
    let fake = write_fake_codex(&world.prefix);
    apply_codex_env(&guard, &world, &fake);

    let manager = world.manager();
    manager
        .enable(COMPONENT, Some("codex"), false)
        .expect("enable");

    // Force the codex CLI removal commands to fail without deregistering.
    guard.set("FAKE_CODEX_FAIL", Path::new("remove"));
    let disabled = manager
        .disable(COMPONENT, Some("codex"), false)
        .expect("disable runs");
    assert!(
        !disabled.claim_removed,
        "receipt must be kept when CLI deregistration fails"
    );
    assert!(!disabled.report.cleanup_complete);
    let claim = world
        .load_state()
        .find_adapter_claim(COMPONENT, "codex")
        .cloned()
        .expect("receipt kept");
    assert_eq!(claim.status, ClaimStatus::CleanupFailed);
}

/// Regression: when the resource bundle is resolved from a packaged datadir
/// registered via `push_primary_datadir_root` (exe-sibling `/usr/share`
/// differing from the install prefix's `{datadir}`), the codex plugin
/// symlink target lives outside the primary layout roots. Enable must still
/// succeed — the Manager trusts its configured datadir roots for symlink
/// target validation.
#[test]
fn codex_enable_succeeds_with_bundle_under_packaged_datadir() {
    let guard = EnvGuard::acquire();
    let root = tempfile::tempdir().expect("tempdir");
    let prefix = root.path().to_path_buf();
    // Install prefix layout (its datadir is prefix/usr/local/share/anolisa).
    let layout = FsLayout::system(Some(prefix.join("install")));
    let user_home = prefix.join("home");
    std::fs::create_dir_all(&user_home).expect("home");

    // Packaged datadir distinct from layout.datadir — where the bundle lives.
    let packaged_datadir = prefix.join("pkg").join("usr").join("share").join("anolisa");
    let resource_root = packaged_datadir
        .join("adapters")
        .join(COMPONENT)
        .join("codex");
    std::fs::create_dir_all(&resource_root).expect("resource root");
    stage_codex_bundle(&resource_root);

    // Contract dest expands against {datadir}; the bundle exists only under
    // the packaged datadir, so resolution lands there.
    seed_component(
        &layout,
        &layout.prefix,
        "codex",
        "plugin",
        "{datadir}/adapters/{component}/codex/",
    );

    let fake = write_fake_codex(&prefix);
    let xdg = prefix.join("xdg-data");
    std::fs::create_dir_all(&xdg).expect("xdg");
    guard.set("CODEX_BIN", &fake);
    guard.set("XDG_DATA_HOME", &xdg);
    guard.set("FAKE_CODEX_LOG", &prefix.join("codex.log"));
    guard.set("FAKE_CODEX_STATE", &prefix.join("codex-state"));

    let mut manager = AdapterManager::new(layout, Some(user_home), "tester".to_string());
    manager.push_primary_datadir_root(packaged_datadir);

    let claim = match manager
        .enable(COMPONENT, Some("codex"), false)
        .expect("enable must succeed with bundle under a packaged datadir")
    {
        EnableOutcome::Enabled(c) => *c,
        EnableOutcome::Planned { .. } => panic!("expected enabled"),
    };
    // The symlink target points at the packaged-datadir bundle, outside the
    // install-prefix layout roots — the very case that previously failed.
    assert!(claim.resources.iter().any(|r| matches!(
        &r.kind,
        ClaimResourceKind::Symlink { target, .. } if target == &resource_root
    )));

    // status re-validates the receipt; it must not reject the packaged-datadir
    // symlink target either.
    let status = manager.status(Some(COMPONENT)).expect("status");
    assert_eq!(status.entries[0].report.summary, AdapterSummary::Healthy);
}

// ---------------------------------------------------------------------------
// Claude Code
// ---------------------------------------------------------------------------

fn stage_claude_bundle(root: &Path) {
    std::fs::create_dir_all(root.join(".claude-plugin")).expect("claude-plugin");
    // Written multi-line with the top-level name on its own line so the
    // fake CLI's line-based `sed` reads the marketplace name (not the nested
    // plugin name) — mirrors the real multi-line manifest.
    std::fs::write(
        root.join(".claude-plugin/marketplace.json"),
        b"{\n  \"name\": \"anolisa-tokenless\",\n  \"plugins\": [{ \"name\": \"tokenless\", \"source\": \"./\" }]\n}\n",
    )
    .expect("marketplace.json");
    std::fs::write(
        root.join(".claude-plugin/plugin.json"),
        br#"{"name":"tokenless","version":"0.6.0"}"#,
    )
    .expect("plugin.json");
}

/// Fake `claude` CLI: records argv and keeps marketplace/plugin registries.
fn write_fake_claude(dir: &Path) -> PathBuf {
    let path = dir.join("claude");
    write_exec(
        &path,
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$FAKE_CLAUDE_LOG"
st="$FAKE_CLAUDE_STATE"; mkdir -p "$st" 2>/dev/null
if [ "$1" = "plugin" ]; then
  case "$2" in
    validate) exit 0 ;;
    marketplace)
      case "$3" in
        add)
          name=$(sed -n 's/.*"name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$4/.claude-plugin/marketplace.json" | head -n1)
          echo "$name" >> "$st/marketplaces" ;;
        remove) [ -f "$st/marketplaces" ] && { grep -vx "$4" "$st/marketplaces" > "$st/m.tmp" 2>/dev/null || true; mv "$st/m.tmp" "$st/marketplaces" 2>/dev/null || true; } ;;
        list) cat "$st/marketplaces" 2>/dev/null || true ;;
      esac ;;
    install) echo "$3" >> "$st/plugins" ;;
    uninstall) [ -f "$st/plugins" ] && { grep -vx "$3" "$st/plugins" > "$st/p.tmp" 2>/dev/null || true; mv "$st/p.tmp" "$st/plugins" 2>/dev/null || true; } ;;
    list) cat "$st/plugins" 2>/dev/null || true ;;
  esac
  exit 0
fi
exit 0
"#,
    );
    path
}

fn apply_claude_env(guard: &EnvGuard, world: &World, fake_bin: &Path) -> PathBuf {
    let log = world.prefix.join("claude.log");
    let state = world.prefix.join("claude-state");
    guard.set("CLAUDE_BIN", fake_bin);
    guard.set("FAKE_CLAUDE_LOG", &log);
    guard.set("FAKE_CLAUDE_STATE", &state);
    log
}

#[test]
fn claude_code_enable_records_validate_marketplace_and_install() {
    let guard = EnvGuard::acquire();
    let world = stage(
        "claude-code",
        "plugin",
        "{datadir}/adapters/{component}/claude-code/",
        stage_claude_bundle,
    );
    let fake = write_fake_claude(&world.prefix);
    let log = apply_claude_env(&guard, &world, &fake);

    let manager = world.manager();
    let claim = match manager
        .enable(COMPONENT, Some("claude-code"), false)
        .expect("enable")
    {
        EnableOutcome::Enabled(c) => *c,
        EnableOutcome::Planned { .. } => panic!("expected enabled"),
    };
    assert_eq!(claim.plugin_id.as_deref(), Some("tokenless"));

    let log_text = std::fs::read_to_string(&log).expect("claude log");
    assert!(
        log_text
            .lines()
            .any(|l| l == format!("plugin validate {}", world.resource_root.display())),
        "must validate the bundle: {log_text}"
    );
    assert!(
        log_text
            .lines()
            .any(|l| l == format!("plugin marketplace add {}", world.resource_root.display())),
        "must add the marketplace: {log_text}"
    );
    assert!(
        log_text
            .lines()
            .any(|l| l == "plugin install tokenless@anolisa-tokenless"),
        "must install the plugin: {log_text}"
    );

    let status = manager.status(Some(COMPONENT)).expect("status");
    assert_eq!(status.entries[0].report.summary, AdapterSummary::Healthy);

    let disabled = manager
        .disable(COMPONENT, Some("claude-code"), false)
        .expect("disable");
    assert!(disabled.claim_removed);
    let log_text = std::fs::read_to_string(&log).expect("claude log");
    assert!(
        log_text
            .lines()
            .any(|l| l == "plugin uninstall tokenless@anolisa-tokenless"),
        "disable must uninstall the plugin: {log_text}"
    );
    assert!(
        log_text
            .lines()
            .any(|l| l == "plugin marketplace remove anolisa-tokenless"),
        "disable must remove the marketplace: {log_text}"
    );
}

#[test]
fn claude_code_disable_without_cli_keeps_receipt() {
    let guard = EnvGuard::acquire();
    let world = stage(
        "claude-code",
        "plugin",
        "{datadir}/adapters/{component}/claude-code/",
        stage_claude_bundle,
    );
    let fake = write_fake_claude(&world.prefix);
    apply_claude_env(&guard, &world, &fake);

    let manager = world.manager();
    manager
        .enable(COMPONENT, Some("claude-code"), false)
        .expect("enable");

    // Point CLAUDE_BIN at a missing path: disable cannot run the CLI and
    // must NOT hand-edit settings.json, so it keeps the receipt.
    guard.set("CLAUDE_BIN", &world.prefix.join("no-such-claude"));
    let disabled = manager
        .disable(COMPONENT, Some("claude-code"), false)
        .expect("disable runs");
    assert!(!disabled.claim_removed, "receipt kept when CLI absent");
    assert!(!disabled.report.cleanup_complete);
    let claim = world
        .load_state()
        .find_adapter_claim(COMPONENT, "claude-code")
        .cloned()
        .expect("receipt kept");
    assert_eq!(claim.status, ClaimStatus::CleanupFailed);
}

/// A receipt missing its marketplace resource (malformed/forged) must not
/// drive `plugin uninstall` / `marketplace remove` against a name derived
/// from context: status degrades and disable keeps the receipt without
/// running any CLI.
#[test]
fn claude_code_fails_closed_without_marketplace_resource() {
    let guard = EnvGuard::acquire();
    let world = stage(
        "claude-code",
        "plugin",
        "{datadir}/adapters/{component}/claude-code/",
        stage_claude_bundle,
    );
    let fake = write_fake_claude(&world.prefix);
    let log = apply_claude_env(&guard, &world, &fake);

    let manager = world.manager();
    manager
        .enable(COMPONENT, Some("claude-code"), false)
        .expect("enable");

    // Tamper: drop the FrameworkMarketplace resource, leaving the payload's
    // dangling reference — as a forged/malformed receipt would.
    let state_path = world.layout.state_dir.join("installed.toml");
    let mut state = world.load_state();
    {
        let claim = state
            .adapter_claims
            .iter_mut()
            .find(|c| c.component == COMPONENT)
            .expect("claim");
        claim
            .resources
            .retain(|r| !matches!(r.kind, ClaimResourceKind::FrameworkMarketplace { .. }));
    }
    state.save(&state_path).expect("save tampered state");

    // status must not report healthy, and must not run the CLI.
    let log_before = std::fs::read_to_string(&log).unwrap_or_default();
    let status = manager.status(Some(COMPONENT)).expect("status");
    assert_eq!(status.entries[0].report.summary, AdapterSummary::Degraded);

    // disable must keep the receipt and run no CLI removal.
    let disabled = manager
        .disable(COMPONENT, Some("claude-code"), false)
        .expect("disable runs");
    assert!(!disabled.claim_removed, "malformed receipt must be kept");
    assert!(!disabled.report.cleanup_complete);
    let log_after = std::fs::read_to_string(&log).unwrap_or_default();
    assert_eq!(
        log_before, log_after,
        "no framework CLI must run for a receipt with no marketplace resource"
    );
    assert!(
        world
            .load_state()
            .find_adapter_claim(COMPONENT, "claude-code")
            .is_some(),
        "receipt kept for manual resolution"
    );
}

// ---------------------------------------------------------------------------
// Qoder
// ---------------------------------------------------------------------------

fn stage_qoder_bundle(root: &Path) {
    std::fs::create_dir_all(root.join(".qoder-plugin")).expect("qoder-plugin");
    std::fs::write(
        root.join(".qoder-plugin/plugin.json"),
        br#"{"name":"tokenless","version":"0.6.0"}"#,
    )
    .expect("plugin.json");
    // Hooks carry the ${QODER_TOKENLESS_HOOKS} placeholder and tokenless-*
    // hook names, mirroring the shipped bundle.
    std::fs::write(
        root.join("hooks.json"),
        br#"{
  "hooks": {
    "PreToolUse": [
      { "matcher": "", "hooks": [
        { "type": "command", "name": "tokenless-rewrite",
          "command": "python3 ${QODER_TOKENLESS_HOOKS}/rewrite_hook.py" } ] }
    ],
    "PostToolUse": [
      { "matcher": "", "hooks": [
        { "type": "command", "name": "tokenless-compress-response",
          "command": "python3 ${QODER_TOKENLESS_HOOKS}/compress_response_hook.py" } ] }
    ]
  }
}
"#,
    )
    .expect("hooks.json");
}

/// Fake `qodercli`: records each argv line to `$FAKE_QODER_LOG` and mirrors
/// qodercli's plugin cache under `$FAKE_QODER_CACHE` so the driver's
/// cache-based removal check reflects prior install/uninstall calls.
/// `$FAKE_QODER_FAIL=uninstall` fails the uninstall without clearing the
/// cache, so the driver cannot confirm removal.
fn write_fake_qodercli(dir: &Path) -> PathBuf {
    let path = dir.join("qodercli");
    write_exec(
        &path,
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$FAKE_QODER_LOG"
cache="$FAKE_QODER_CACHE"
if [ "$1" = "plugins" ]; then
  case "$2" in
    install) mkdir -p "$cache/tokenless" 2>/dev/null ;;
    uninstall)
      [ "$FAKE_QODER_FAIL" = "uninstall" ] && { echo "uninstall boom" >&2; exit 1; }
      rm -rf "$cache/$3" 2>/dev/null || true ;;
    list) ;;
  esac
  exit 0
fi
exit 0
"#,
    );
    path
}

/// Returns `(log, settings_path, cache_dir, staging_symlink)`.
fn apply_qoder_env(
    guard: &EnvGuard,
    world: &World,
    fake_bin: &Path,
) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let xdg = world.prefix.join("xdg-data");
    std::fs::create_dir_all(&xdg).expect("xdg");
    let log = world.prefix.join("qoder.log");
    let cache = world
        .user_home
        .join(".qoder")
        .join("plugins")
        .join("cache")
        .join("local");
    guard.set("QODERCLI_BIN", fake_bin);
    guard.set("XDG_DATA_HOME", &xdg);
    guard.set("FAKE_QODER_LOG", &log);
    guard.set("FAKE_QODER_CACHE", &cache);
    let settings = world.user_home.join(".qoder").join("settings.json");
    let staging = xdg.join("anolisa").join("qoder-plugins").join("tokenless");
    (log, settings, cache, staging)
}

fn read_json(path: &Path) -> serde_json::Value {
    let text = std::fs::read_to_string(path).expect("read settings.json");
    serde_json::from_str(&text).expect("parse settings.json")
}

fn hook_names(settings: &serde_json::Value, event: &str) -> Vec<String> {
    settings["hooks"][event]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|e| e["hooks"][0]["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn hook_command(settings: &serde_json::Value, event: &str, name: &str) -> Option<String> {
    settings["hooks"][event].as_array().and_then(|arr| {
        arr.iter().find_map(|entry| {
            let hook = entry["hooks"].as_array()?.first()?;
            (hook["name"].as_str()? == name)
                .then(|| hook["command"].as_str().map(str::to_string))
                .flatten()
        })
    })
}

fn enabled_plugins(settings: &serde_json::Value) -> Vec<String> {
    settings["plugins"]["enabled"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn qoder_enable_installs_writes_receipt_and_merges_settings() {
    let guard = EnvGuard::acquire();
    let world = stage(
        "qoder",
        "plugin",
        "{datadir}/adapters/{component}/qoder/",
        stage_qoder_bundle,
    );
    let fake = write_fake_qodercli(&world.prefix);
    let (log, settings, _cache, staging) = apply_qoder_env(&guard, &world, &fake);

    let manager = world.manager();
    let claim = match manager
        .enable(COMPONENT, Some("qoder"), false)
        .expect("enable")
    {
        EnableOutcome::Enabled(c) => *c,
        EnableOutcome::Planned { .. } => panic!("expected enabled"),
    };
    assert_eq!(claim.plugin_id.as_deref(), Some("tokenless"));

    // Recorded argv: install from the plugin-named staging symlink.
    let log_text = std::fs::read_to_string(&log).expect("qoder log");
    assert!(
        log_text
            .lines()
            .any(|l| l == format!("plugins install {}", staging.display())),
        "must run `plugins install <staging>`: {log_text}"
    );

    // settings.json merged: our hooks + tokenless@local, and the placeholder
    // was expanded to an absolute path.
    let cfg = read_json(&settings);
    assert!(hook_names(&cfg, "PreToolUse").contains(&"tokenless-rewrite".to_string()));
    assert!(hook_names(&cfg, "PostToolUse").contains(&"tokenless-compress-response".to_string()));
    assert!(enabled_plugins(&cfg).contains(&"tokenless@local".to_string()));
    let cmd = cfg["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
        .as_str()
        .expect("command");
    assert!(
        !cmd.contains("${QODER_TOKENLESS_HOOKS}"),
        "placeholder expanded: {cmd}"
    );

    // Receipt carries the plugin + settings resources, no argv/script.
    assert!(claim.resources.iter().any(|r| matches!(
        &r.kind,
        ClaimResourceKind::FrameworkPlugin { framework, plugin_id }
            if framework == "qoder" && plugin_id == "tokenless"
    )));
    assert!(claim.resources.iter().any(|r| matches!(
        &r.kind,
        ClaimResourceKind::ExternalPath { path } if path == &settings
    )));

    let status = manager.status(Some(COMPONENT)).expect("status");
    assert_eq!(status.entries[0].report.summary, AdapterSummary::Healthy);

    let disabled = manager
        .disable(COMPONENT, Some("qoder"), false)
        .expect("disable");
    assert!(disabled.claim_removed);
    let log_text = std::fs::read_to_string(&log).expect("qoder log");
    assert!(
        log_text.lines().any(|l| l == "plugins uninstall tokenless"),
        "disable must run `plugins uninstall tokenless`: {log_text}"
    );
    // settings.json pruned of our entries; file itself preserved.
    let cfg = read_json(&settings);
    assert!(!enabled_plugins(&cfg).contains(&"tokenless@local".to_string()));
    assert!(hook_names(&cfg, "PreToolUse").is_empty());
    assert!(
        world
            .load_state()
            .find_adapter_claim(COMPONENT, "qoder")
            .is_none(),
        "receipt gone after disable"
    );
}

#[test]
fn qoder_enable_preserves_existing_user_settings() {
    let guard = EnvGuard::acquire();
    let world = stage(
        "qoder",
        "plugin",
        "{datadir}/adapters/{component}/qoder/",
        stage_qoder_bundle,
    );
    let fake = write_fake_qodercli(&world.prefix);
    let (_log, settings, _cache, _staging) = apply_qoder_env(&guard, &world, &fake);

    // Pre-existing user settings.json with the user's own theme, hook, and
    // enabled plugin.
    std::fs::create_dir_all(settings.parent().unwrap()).expect("mkdir .qoder");
    std::fs::write(
        &settings,
        br#"{
  "theme": "dark",
  "hooks": { "PreToolUse": [
    { "hooks": [ { "type": "command", "name": "user-audit" } ] },
    { "hooks": [
      { "type": "command", "name": "tokenless-my-custom-audit",
        "command": "python3 /user/audit.py" } ] } ] },
  "plugins": { "enabled": ["other@local"], "registry": "corp" }
}"#,
    )
    .expect("seed settings");

    let manager = world.manager();
    manager
        .enable(COMPONENT, Some("qoder"), false)
        .expect("enable");

    let cfg = read_json(&settings);
    assert_eq!(cfg["theme"], "dark", "user setting preserved");
    assert_eq!(
        cfg["plugins"]["registry"], "corp",
        "user plugin cfg preserved"
    );
    let pre = hook_names(&cfg, "PreToolUse");
    assert!(pre.contains(&"user-audit".to_string()), "user hook kept");
    assert!(
        pre.contains(&"tokenless-my-custom-audit".to_string()),
        "user hook with tokenless prefix kept"
    );
    assert!(
        pre.contains(&"tokenless-rewrite".to_string()),
        "our hook added"
    );
    let enabled = enabled_plugins(&cfg);
    assert!(enabled.contains(&"other@local".to_string()));
    assert!(enabled.contains(&"tokenless@local".to_string()));

    // Disable prunes only ANOLISA-managed entries.
    manager
        .disable(COMPONENT, Some("qoder"), false)
        .expect("disable");
    let cfg = read_json(&settings);
    assert_eq!(cfg["theme"], "dark");
    assert_eq!(cfg["plugins"]["registry"], "corp");
    let pre = hook_names(&cfg, "PreToolUse");
    assert!(
        pre.contains(&"user-audit".to_string()),
        "user hook survives prune"
    );
    assert!(
        pre.contains(&"tokenless-my-custom-audit".to_string()),
        "user tokenless-prefix hook survives prune"
    );
    assert!(
        !pre.contains(&"tokenless-rewrite".to_string()),
        "our hook pruned"
    );
    assert!(enabled_plugins(&cfg).contains(&"other@local".to_string()));
    assert!(!enabled_plugins(&cfg).contains(&"tokenless@local".to_string()));
}

#[test]
fn qoder_enable_replaces_same_named_hook_body() {
    let guard = EnvGuard::acquire();
    let world = stage(
        "qoder",
        "plugin",
        "{datadir}/adapters/{component}/qoder/",
        stage_qoder_bundle,
    );
    let fake = write_fake_qodercli(&world.prefix);
    let (_log, settings, _cache, _staging) = apply_qoder_env(&guard, &world, &fake);

    std::fs::create_dir_all(settings.parent().unwrap()).expect("mkdir .qoder");
    std::fs::write(
        &settings,
        br#"{
  "hooks": { "PreToolUse": [
    { "matcher": "", "hooks": [
      { "type": "command", "name": "tokenless-rewrite",
        "command": "python3 /user/rewrite.py" } ] } ] },
  "plugins": { "enabled": [] }
}"#,
    )
    .expect("seed settings");

    let manager = world.manager();
    manager
        .enable(COMPONENT, Some("qoder"), false)
        .expect("enable");

    let cfg = read_json(&settings);
    let pre = hook_names(&cfg, "PreToolUse");
    assert_eq!(
        pre.iter()
            .filter(|name| *name == "tokenless-rewrite")
            .count(),
        1,
        "same-name hook is replaced instead of duplicated"
    );
    let command = hook_command(&cfg, "PreToolUse", "tokenless-rewrite").expect("command");
    assert!(
        command.contains("rewrite_hook.py"),
        "managed hook body restored: {command}"
    );
    assert_eq!(
        manager.status(Some(COMPONENT)).expect("status").entries[0]
            .report
            .summary,
        AdapterSummary::Healthy
    );
}

#[test]
fn qoder_enable_leaves_non_object_settings_untouched() {
    let guard = EnvGuard::acquire();
    let world = stage(
        "qoder",
        "plugin",
        "{datadir}/adapters/{component}/qoder/",
        stage_qoder_bundle,
    );
    let fake = write_fake_qodercli(&world.prefix);
    let (log, settings, _cache, _staging) = apply_qoder_env(&guard, &world, &fake);

    std::fs::create_dir_all(settings.parent().unwrap()).expect("mkdir .qoder");
    std::fs::write(&settings, br#"["user-placeholder"]"#).expect("seed settings");

    let manager = world.manager();
    let err = manager
        .enable(COMPONENT, Some("qoder"), false)
        .expect_err("non-object settings must fail closed");
    assert!(
        matches!(err, AdapterError::SettingsUnparseable { .. }),
        "{err:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&settings).expect("settings untouched"),
        r#"["user-placeholder"]"#
    );
    assert!(
        !log.exists(),
        "enable must fail before invoking qodercli when settings cannot be merged"
    );
}

#[test]
fn qoder_dry_run_enable_writes_nothing() {
    let guard = EnvGuard::acquire();
    let world = stage(
        "qoder",
        "plugin",
        "{datadir}/adapters/{component}/qoder/",
        stage_qoder_bundle,
    );
    let fake = write_fake_qodercli(&world.prefix);
    let (log, settings, _cache, staging) = apply_qoder_env(&guard, &world, &fake);

    let manager = world.manager();
    let outcome = manager
        .enable(COMPONENT, Some("qoder"), true)
        .expect("dry-run");
    assert!(matches!(outcome, EnableOutcome::Planned { .. }));
    assert!(!log.exists(), "dry-run must not invoke qodercli (no log)");
    assert!(!settings.exists(), "dry-run must not write settings.json");
    assert!(
        !staging.exists(),
        "dry-run must not create the staging symlink"
    );
    assert!(
        world
            .load_state()
            .find_adapter_claim(COMPONENT, "qoder")
            .is_none(),
        "dry-run must not persist a receipt"
    );
}

#[test]
fn qoder_status_degraded_when_managed_entry_missing() {
    let guard = EnvGuard::acquire();
    let world = stage(
        "qoder",
        "plugin",
        "{datadir}/adapters/{component}/qoder/",
        stage_qoder_bundle,
    );
    let fake = write_fake_qodercli(&world.prefix);
    let (_log, settings, _cache, _staging) = apply_qoder_env(&guard, &world, &fake);

    let manager = world.manager();
    manager
        .enable(COMPONENT, Some("qoder"), false)
        .expect("enable");
    assert_eq!(
        manager.status(Some(COMPONENT)).expect("status").entries[0]
            .report
            .summary,
        AdapterSummary::Healthy
    );

    // Drop tokenless@local from plugins.enabled: status must degrade, not
    // stay healthy off the (unreliable) plugin registry.
    let mut cfg = read_json(&settings);
    cfg["plugins"]["enabled"] = serde_json::json!([]);
    std::fs::write(&settings, serde_json::to_vec_pretty(&cfg).unwrap()).expect("rewrite settings");

    let status = manager.status(Some(COMPONENT)).expect("status");
    assert_eq!(status.entries[0].report.summary, AdapterSummary::Degraded);
    // Plugin registration is reported Unknown (never faked from qodercli list).
    assert!(
        status.entries[0]
            .report
            .conditions
            .iter()
            .any(|c| c.kind == AdapterConditionKind::PluginRegistered
                && c.status == ConditionStatus::Unknown)
    );
}

#[test]
fn qoder_disable_is_idempotent() {
    let guard = EnvGuard::acquire();
    let world = stage(
        "qoder",
        "plugin",
        "{datadir}/adapters/{component}/qoder/",
        stage_qoder_bundle,
    );
    let fake = write_fake_qodercli(&world.prefix);
    apply_qoder_env(&guard, &world, &fake);

    let manager = world.manager();
    manager
        .enable(COMPONENT, Some("qoder"), false)
        .expect("enable");

    let first = manager
        .disable(COMPONENT, Some("qoder"), false)
        .expect("first disable");
    assert!(first.claim_removed);
    assert!(first.report.cleanup_complete);

    // Second disable with no receipt is a clean no-op.
    let second = manager
        .disable(COMPONENT, Some("qoder"), false)
        .expect("second disable");
    assert!(!second.claim_removed);
    assert!(second.report.cleanup_complete, "idempotent no-op");
}

#[test]
fn qoder_disable_keeps_receipt_when_uninstall_fails() {
    let guard = EnvGuard::acquire();
    let world = stage(
        "qoder",
        "plugin",
        "{datadir}/adapters/{component}/qoder/",
        stage_qoder_bundle,
    );
    let fake = write_fake_qodercli(&world.prefix);
    apply_qoder_env(&guard, &world, &fake);

    let manager = world.manager();
    manager
        .enable(COMPONENT, Some("qoder"), false)
        .expect("enable");

    // Fail uninstall without clearing the cache: the driver cannot confirm
    // removal, so cleanup is incomplete and the receipt is kept.
    guard.set("FAKE_QODER_FAIL", Path::new("uninstall"));
    let disabled = manager
        .disable(COMPONENT, Some("qoder"), false)
        .expect("disable runs");
    assert!(!disabled.claim_removed);
    assert!(!disabled.report.cleanup_complete);
    let claim = world
        .load_state()
        .find_adapter_claim(COMPONENT, "qoder")
        .cloned()
        .expect("receipt kept");
    assert_eq!(claim.status, ClaimStatus::CleanupFailed);
}

#[test]
fn qoder_disable_without_cli_keeps_receipt() {
    let guard = EnvGuard::acquire();
    let world = stage(
        "qoder",
        "plugin",
        "{datadir}/adapters/{component}/qoder/",
        stage_qoder_bundle,
    );
    let fake = write_fake_qodercli(&world.prefix);
    apply_qoder_env(&guard, &world, &fake);

    let manager = world.manager();
    manager
        .enable(COMPONENT, Some("qoder"), false)
        .expect("enable");

    // Point QODERCLI_BIN at a missing binary: disable cannot deregister, so
    // it keeps the receipt rather than pruning settings and faking success.
    guard.set("QODERCLI_BIN", &world.prefix.join("no-such-qodercli"));
    let disabled = manager
        .disable(COMPONENT, Some("qoder"), false)
        .expect("disable runs");
    assert!(!disabled.claim_removed, "receipt kept when CLI absent");
    assert!(!disabled.report.cleanup_complete);
    let claim = world
        .load_state()
        .find_adapter_claim(COMPONENT, "qoder")
        .cloned()
        .expect("receipt kept");
    assert_eq!(claim.status, ClaimStatus::CleanupFailed);
}

#[test]
fn qoder_disable_fails_closed_on_unparseable_settings() {
    let guard = EnvGuard::acquire();
    let world = stage(
        "qoder",
        "plugin",
        "{datadir}/adapters/{component}/qoder/",
        stage_qoder_bundle,
    );
    let fake = write_fake_qodercli(&world.prefix);
    let (_log, settings, _cache, _staging) = apply_qoder_env(&guard, &world, &fake);

    let manager = world.manager();
    manager
        .enable(COMPONENT, Some("qoder"), false)
        .expect("enable");

    // Corrupt settings.json: disable must not overwrite it and must report
    // cleanup incomplete, keeping the receipt.
    std::fs::write(&settings, b"{ this is not json").expect("corrupt settings");
    let disabled = manager
        .disable(COMPONENT, Some("qoder"), false)
        .expect("disable runs");
    assert!(!disabled.claim_removed);
    assert!(!disabled.report.cleanup_complete);
    // The unparseable file was left byte-for-byte untouched.
    assert_eq!(
        std::fs::read_to_string(&settings).expect("read"),
        "{ this is not json"
    );
    let claim = world
        .load_state()
        .find_adapter_claim(COMPONENT, "qoder")
        .cloned()
        .expect("receipt kept");
    assert_eq!(claim.status, ClaimStatus::CleanupFailed);
}

#[test]
fn qoder_forged_settings_path_rejected_by_status() {
    let guard = EnvGuard::acquire();
    let world = stage(
        "qoder",
        "plugin",
        "{datadir}/adapters/{component}/qoder/",
        stage_qoder_bundle,
    );
    let fake = write_fake_qodercli(&world.prefix);
    apply_qoder_env(&guard, &world, &fake);

    let manager = world.manager();
    manager
        .enable(COMPONENT, Some("qoder"), false)
        .expect("enable");

    // Tamper: repoint the settings resource at ~/.ssh, then /etc. Both are
    // outside the driver's allowed roots, so claim validation must reject the
    // receipt before status can act on it.
    for forged in ["/home/attacker/.ssh/authorized_keys", "/etc/cron.d/evil"] {
        let state_path = world.layout.state_dir.join("installed.toml");
        let mut state = world.load_state();
        {
            let claim = state
                .adapter_claims
                .iter_mut()
                .find(|c| c.component == COMPONENT)
                .expect("claim");
            for res in &mut claim.resources {
                if let ClaimResourceKind::ExternalPath { path } = &mut res.kind {
                    *path = PathBuf::from(forged);
                }
            }
        }
        state.save(&state_path).expect("save tampered state");

        let err = manager
            .status(Some(COMPONENT))
            .expect_err("forged settings path must be rejected");
        assert!(
            matches!(err, AdapterError::ClaimValidation(_)),
            "got {err:?} for {forged}"
        );
    }
}

#[test]
fn qoder_status_degraded_when_one_managed_hook_removed() {
    let guard = EnvGuard::acquire();
    let world = stage(
        "qoder",
        "plugin",
        "{datadir}/adapters/{component}/qoder/",
        stage_qoder_bundle,
    );
    let fake = write_fake_qodercli(&world.prefix);
    let (_log, settings, _cache, _staging) = apply_qoder_env(&guard, &world, &fake);

    let manager = world.manager();
    manager
        .enable(COMPONENT, Some("qoder"), false)
        .expect("enable");
    assert_eq!(
        manager.status(Some(COMPONENT)).expect("status").entries[0]
            .report
            .summary,
        AdapterSummary::Healthy
    );

    // Remove one of the two managed hooks (tokenless-compress-response) while
    // keeping tokenless-rewrite and tokenless@local. Status must degrade:
    // partial hook drift is not healthy.
    let mut cfg = read_json(&settings);
    cfg["hooks"]
        .as_object_mut()
        .expect("hooks obj")
        .remove("PostToolUse");
    std::fs::write(&settings, serde_json::to_vec_pretty(&cfg).unwrap()).expect("rewrite settings");

    let status = manager.status(Some(COMPONENT)).expect("status");
    assert_eq!(status.entries[0].report.summary, AdapterSummary::Degraded);
    // The still-present tokenless@local means plugin entry is fine; the
    // JsonKeysPresent condition is what flipped to False.
    assert!(
        status.entries[0]
            .report
            .conditions
            .iter()
            .any(|c| c.kind == AdapterConditionKind::JsonKeysPresent
                && c.status == ConditionStatus::False)
    );
}

#[test]
fn qoder_status_degraded_when_plugin_resource_missing() {
    let guard = EnvGuard::acquire();
    let world = stage(
        "qoder",
        "plugin",
        "{datadir}/adapters/{component}/qoder/",
        stage_qoder_bundle,
    );
    let fake = write_fake_qodercli(&world.prefix);
    apply_qoder_env(&guard, &world, &fake);

    let manager = world.manager();
    manager
        .enable(COMPONENT, Some("qoder"), false)
        .expect("enable");

    // Drop the FrameworkPlugin resource, leaving the payload's dangling
    // reference — as a forged/malformed receipt would. Status must fail
    // closed (degraded), never healthy.
    let state_path = world.layout.state_dir.join("installed.toml");
    let mut state = world.load_state();
    {
        let claim = state
            .adapter_claims
            .iter_mut()
            .find(|c| c.component == COMPONENT)
            .expect("claim");
        claim
            .resources
            .retain(|r| !matches!(r.kind, ClaimResourceKind::FrameworkPlugin { .. }));
    }
    state.save(&state_path).expect("save tampered state");

    let status = manager.status(Some(COMPONENT)).expect("status");
    assert_eq!(status.entries[0].report.summary, AdapterSummary::Degraded);
}

#[test]
fn qoder_disable_fails_closed_when_settings_resource_missing() {
    let guard = EnvGuard::acquire();
    let world = stage(
        "qoder",
        "plugin",
        "{datadir}/adapters/{component}/qoder/",
        stage_qoder_bundle,
    );
    let fake = write_fake_qodercli(&world.prefix);
    let (log, _settings, _cache, _staging) = apply_qoder_env(&guard, &world, &fake);

    let manager = world.manager();
    manager
        .enable(COMPONENT, Some("qoder"), false)
        .expect("enable");

    // Drop the settings ExternalPath resource: disable must not run the CLI
    // or touch settings against a ctx-derived default; it keeps the receipt.
    let state_path = world.layout.state_dir.join("installed.toml");
    let mut state = world.load_state();
    {
        let claim = state
            .adapter_claims
            .iter_mut()
            .find(|c| c.component == COMPONENT)
            .expect("claim");
        claim
            .resources
            .retain(|r| !matches!(r.kind, ClaimResourceKind::ExternalPath { .. }));
    }
    state.save(&state_path).expect("save tampered state");

    let log_before = std::fs::read_to_string(&log).unwrap_or_default();
    let disabled = manager
        .disable(COMPONENT, Some("qoder"), false)
        .expect("disable runs");
    assert!(!disabled.claim_removed, "malformed receipt must be kept");
    assert!(!disabled.report.cleanup_complete);
    let log_after = std::fs::read_to_string(&log).unwrap_or_default();
    assert_eq!(
        log_before, log_after,
        "no qodercli command may run for a receipt missing its settings resource"
    );
    let claim = world
        .load_state()
        .find_adapter_claim(COMPONENT, "qoder")
        .cloned()
        .expect("receipt kept");
    assert_eq!(claim.status, ClaimStatus::CleanupFailed);
}

#[test]
fn qoder_disable_uses_receipt_hook_specs_when_bundle_removed() {
    let guard = EnvGuard::acquire();
    let world = stage(
        "qoder",
        "plugin",
        "{datadir}/adapters/{component}/qoder/",
        stage_qoder_bundle,
    );
    let fake = write_fake_qodercli(&world.prefix);
    let (_log, settings, _cache, _staging) = apply_qoder_env(&guard, &world, &fake);

    let manager = world.manager();
    manager
        .enable(COMPONENT, Some("qoder"), false)
        .expect("enable");

    std::fs::remove_dir_all(&world.resource_root).expect("remove bundle");
    let disabled = manager
        .disable(COMPONENT, Some("qoder"), false)
        .expect("disable runs");
    assert!(
        disabled.claim_removed,
        "receipt removed after complete cleanup"
    );
    assert!(disabled.report.cleanup_complete);

    let cfg = read_json(&settings);
    assert!(!hook_names(&cfg, "PreToolUse").contains(&"tokenless-rewrite".to_string()));
    assert!(!hook_names(&cfg, "PostToolUse").contains(&"tokenless-compress-response".to_string()));
    assert!(!enabled_plugins(&cfg).contains(&"tokenless@local".to_string()));
}

#[test]
fn qoder_forged_resource_root_does_not_change_hook_ownership() {
    let guard = EnvGuard::acquire();
    let world = stage(
        "qoder",
        "plugin",
        "{datadir}/adapters/{component}/qoder/",
        stage_qoder_bundle,
    );
    let fake = write_fake_qodercli(&world.prefix);
    let (_log, settings, _cache, _staging) = apply_qoder_env(&guard, &world, &fake);

    let manager = world.manager();
    manager
        .enable(COMPONENT, Some("qoder"), false)
        .expect("enable");

    let forged_root = world.prefix.join("forged-qoder-root");
    std::fs::create_dir_all(&forged_root).expect("forged root");
    std::fs::write(
        forged_root.join("hooks.json"),
        br#"{
  "hooks": {
    "PreToolUse": [
      { "hooks": [
        { "type": "command", "name": "tokenless-my-custom-audit",
          "command": "python3 /attacker/audit.py" } ] }
    ]
  }
}"#,
    )
    .expect("forged hooks");

    let mut cfg = read_json(&settings);
    cfg["hooks"]["PreToolUse"]
        .as_array_mut()
        .expect("pre hooks")
        .push(serde_json::json!({
            "hooks": [{
                "type": "command",
                "name": "tokenless-my-custom-audit",
                "command": "python3 /attacker/audit.py"
            }]
        }));
    std::fs::write(&settings, serde_json::to_vec_pretty(&cfg).unwrap()).expect("rewrite settings");

    let state_path = world.layout.state_dir.join("installed.toml");
    let mut state = world.load_state();
    {
        let claim = state
            .adapter_claims
            .iter_mut()
            .find(|c| c.component == COMPONENT)
            .expect("claim");
        claim.resource_root = forged_root;
    }
    state.save(&state_path).expect("save tampered state");

    let status = manager.status(Some(COMPONENT)).expect("status");
    assert!(
        status.entries[0]
            .report
            .conditions
            .iter()
            .any(|c| c.kind == AdapterConditionKind::JsonKeysPresent
                && c.status == ConditionStatus::True),
        "settings verification must use receipt specs, not forged resource_root hooks.json"
    );

    let disabled = manager
        .disable(COMPONENT, Some("qoder"), false)
        .expect("disable runs");
    assert!(disabled.claim_removed);
    assert!(disabled.report.cleanup_complete);

    let cfg = read_json(&settings);
    let pre = hook_names(&cfg, "PreToolUse");
    assert!(
        pre.contains(&"tokenless-my-custom-audit".to_string()),
        "forged resource_root hook is not treated as ANOLISA-owned"
    );
    assert!(!pre.contains(&"tokenless-rewrite".to_string()));
}

#[test]
fn qoder_forged_settings_redirect_within_qoder_home_rejected() {
    let guard = EnvGuard::acquire();
    let world = stage(
        "qoder",
        "plugin",
        "{datadir}/adapters/{component}/qoder/",
        stage_qoder_bundle,
    );
    let fake = write_fake_qodercli(&world.prefix);
    let (log, _settings, _cache, _staging) = apply_qoder_env(&guard, &world, &fake);

    let manager = world.manager();
    manager
        .enable(COMPONENT, Some("qoder"), false)
        .expect("enable");

    // Forge the settings resource to another file *inside* ~/.qoder. It still
    // passes the Manager's allowed-root check (the whole ~/.qoder is allowed),
    // so the driver must reject it by pinning the path to settings.json.
    let decoy = world.user_home.join(".qoder").join("other.json");
    std::fs::write(&decoy, b"{\"user\":\"data\"}").expect("seed decoy");
    let state_path = world.layout.state_dir.join("installed.toml");
    let mut state = world.load_state();
    {
        let claim = state
            .adapter_claims
            .iter_mut()
            .find(|c| c.component == COMPONENT)
            .expect("claim");
        for res in &mut claim.resources {
            if let ClaimResourceKind::ExternalPath { path } = &mut res.kind {
                *path = decoy.clone();
            }
        }
    }
    state.save(&state_path).expect("save tampered state");

    // status: the redirect is not an outright validation error (same root),
    // but the driver must fail closed to Degraded, never Healthy.
    let status = manager.status(Some(COMPONENT)).expect("status");
    assert_eq!(status.entries[0].report.summary, AdapterSummary::Degraded);

    // disable: must not run the CLI or touch the decoy file.
    let log_before = std::fs::read_to_string(&log).unwrap_or_default();
    let disabled = manager
        .disable(COMPONENT, Some("qoder"), false)
        .expect("disable runs");
    assert!(
        !disabled.claim_removed,
        "receipt kept for redirected settings"
    );
    assert!(!disabled.report.cleanup_complete);
    let log_after = std::fs::read_to_string(&log).unwrap_or_default();
    assert_eq!(
        log_before, log_after,
        "no qodercli command may run for a redirected settings resource"
    );
    assert_eq!(
        std::fs::read_to_string(&decoy).expect("read decoy"),
        "{\"user\":\"data\"}",
        "the redirected file must be left untouched"
    );
}
