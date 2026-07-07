//! End-to-end adapter manager tests for the Cosh, Codex, and Claude Code
//! drivers.
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

    fn load_state(&self) -> anolisa_core::state::InstalledState {
        anolisa_core::state::InstalledState::load(&self.layout.state_dir.join("installed.toml"))
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
        EnableOutcome::Planned(_) => panic!("expected enabled"),
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
        EnableOutcome::Planned(plan) => {
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
        EnableOutcome::Planned(_) => panic!("expected enabled"),
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
    assert!(matches!(outcome, EnableOutcome::Planned(_)));
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
        &prefix,
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
        EnableOutcome::Planned(_) => panic!("expected enabled"),
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
        EnableOutcome::Planned(_) => panic!("expected enabled"),
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
