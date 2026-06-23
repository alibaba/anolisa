//! A1: Activation File Consumer integration tests.
//!
//! Coverage:
//!
//! * `target = null` hides the skill from readdir and lookup (ENOENT).
//! * Valid snapshot activation maps `/skills/<name>` to the snapshot tree;
//!   reads through the FUSE mount return snapshot content.
//! * Invalid activation (bad JSON, unknown schema, bad target) hides the
//!   skill when activation mode is enabled (fail-safe hidden).
//! * Missing `activation.json` hides the skill when activation mode is
//!   enabled.
//! * Activation mode `off` (no resolver attached) preserves the existing
//!   pre-activation mount behavior bit-for-bit.
//! * Snapshot reads through the mount continue to respect fd pinning.

#![allow(clippy::too_many_arguments)]
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use skillfs_core::{ParseConfig, SharedSkillStore, store::SkillStore};
use skillfs_fuse::security::{
    ACTIVATION_XATTR, ActiveSkillResolver, ActiveTarget, bootstrap_activation, load_activation,
};
use skillfs_fuse::{MountConfig, MountHandle, MountOptions, mount_background_configured};

#[path = "common/mod.rs"]
mod common;

use crate::common::{create_skill_dir, fuse_available};

// ─────────────────────────────────────────────────────────────────────────────
// Local fixture
// ─────────────────────────────────────────────────────────────────────────────

struct ActivationMount {
    source: tempfile::TempDir,
    mountpoint: tempfile::TempDir,
    handle: Option<MountHandle>,
}

impl ActivationMount {
    fn new<S, R>(seed: S, resolver_builder: R) -> Self
    where
        S: FnOnce(&Path),
        R: FnOnce(&Path) -> Option<Arc<ActiveSkillResolver>>,
    {
        let source = tempfile::tempdir().expect("source tempdir");
        seed(source.path());
        let resolver = resolver_builder(source.path());
        let mountpoint = tempfile::tempdir().expect("mount tempdir");

        let mut store = SkillStore::new();
        store.load_from_directory(source.path(), &ParseConfig::default());
        let shared: SharedSkillStore = Arc::new(RwLock::new(store));

        let handle = mount_background_configured(
            mountpoint.path(),
            source.path(),
            shared,
            MountOptions::default(),
            false,
            MountConfig {
                active_resolver: resolver,
                ..MountConfig::default()
            },
        )
        .expect("mount_background_configured");
        std::thread::sleep(Duration::from_millis(300));

        Self {
            source,
            mountpoint,
            handle: Some(handle),
        }
    }

    fn skills_dir(&self) -> PathBuf {
        self.mountpoint.path().join("skills")
    }

    fn skill_dir(&self, name: &str) -> PathBuf {
        self.skills_dir().join(name)
    }

    fn skill_md(&self, name: &str) -> PathBuf {
        self.skill_dir(name).join("SKILL.md")
    }
}

impl Drop for ActivationMount {
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            drop(h);
        }
        let mp = self.mountpoint.path().to_path_buf();
        std::thread::sleep(Duration::from_millis(150));
        let _ = std::process::Command::new("fusermount3")
            .args(["-u", &mp.to_string_lossy()])
            .output();
    }
}

fn sorted_dir(dir: &Path) -> Vec<String> {
    let mut entries: Vec<String> = std::fs::read_dir(dir)
        .expect("read_dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    entries.sort();
    entries
}

fn write_activation(source: &Path, skill: &str, json: &str) {
    let meta = source.join(skill).join(".skill-meta");
    std::fs::create_dir_all(&meta).expect("create .skill-meta");
    std::fs::write(meta.join("activation.json"), json).expect("write activation.json");
}

fn write_snapshot(source: &Path, skill: &str, version: &str, skill_md: &str) -> PathBuf {
    let dir = source
        .join(skill)
        .join(".skill-meta/versions")
        .join(version);
    std::fs::create_dir_all(&dir).expect("create snapshot dir");
    std::fs::write(dir.join("SKILL.md"), skill_md).expect("write snapshot SKILL.md");
    dir
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests: target=null -> hidden
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn null_target_hides_skill() {
    if !fuse_available() {
        eprintln!("SKIP: FUSE not available");
        return;
    }

    let mount = ActivationMount::new(
        |src| {
            create_skill_dir(src, "alpha");
            write_activation(src, "alpha", r#"{"schemaVersion": 1, "target": null}"#);
            create_skill_dir(src, "beta");
            write_snapshot(
                src,
                "beta",
                "v000001.snapshot",
                "---\nname: beta\ndescription: test\n---\n",
            );
            write_activation(
                src,
                "beta",
                r#"{"schemaVersion": 1, "target": ".skill-meta/versions/v000001.snapshot"}"#,
            );
        },
        |src_root| {
            let resolver = ActiveSkillResolver::new(src_root);
            let names = vec!["alpha".to_string(), "beta".to_string()];
            bootstrap_activation(src_root, &names, &resolver);
            Some(Arc::new(resolver))
        },
    );

    let listing = sorted_dir(&mount.skills_dir());
    assert!(
        !listing.contains(&"alpha".to_string()),
        "null-target skill must not appear in /skills, got {listing:?}"
    );
    assert!(
        listing.contains(&"beta".to_string()),
        "snapshot-activated skill should be visible, got {listing:?}"
    );

    let err = std::fs::metadata(mount.skill_dir("alpha")).unwrap_err();
    assert_eq!(err.raw_os_error(), Some(libc::ENOENT));
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests: valid snapshot -> reads snapshot content
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn valid_snapshot_activation_reads_snapshot_skill_md() {
    if !fuse_available() {
        eprintln!("SKIP: FUSE not available");
        return;
    }

    let mount = ActivationMount::new(
        |src| {
            create_skill_dir(src, "demo-weather");
            write_snapshot(
                src,
                "demo-weather",
                "v000001.snapshot",
                "---\nname: demo-weather\ndescription: snapshot version\n---\n",
            );
            write_activation(
                src,
                "demo-weather",
                r#"{"schemaVersion": 1, "target": ".skill-meta/versions/v000001.snapshot"}"#,
            );
        },
        |src_root| {
            let resolver = ActiveSkillResolver::new(src_root);
            let names = vec!["demo-weather".to_string()];
            bootstrap_activation(src_root, &names, &resolver);
            Some(Arc::new(resolver))
        },
    );

    assert!(mount.skill_dir("demo-weather").exists());
    assert!(mount.skill_md("demo-weather").exists());
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests: invalid target -> fail-safe hidden
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn invalid_target_hides_skill_failsafe() {
    if !fuse_available() {
        eprintln!("SKIP: FUSE not available");
        return;
    }

    let mount = ActivationMount::new(
        |src| {
            create_skill_dir(src, "bad-target");
            write_activation(
                src,
                "bad-target",
                r#"{"schemaVersion": 1, "target": "/etc/passwd"}"#,
            );
            create_skill_dir(src, "bad-json");
            let meta = src.join("bad-json").join(".skill-meta");
            std::fs::create_dir_all(&meta).unwrap();
            std::fs::write(meta.join("activation.json"), "not json").unwrap();

            create_skill_dir(src, "bad-schema");
            write_activation(
                src,
                "bad-schema",
                r#"{"schemaVersion": 999, "target": null}"#,
            );
        },
        |src_root| {
            let resolver = ActiveSkillResolver::new(src_root);
            let names = vec![
                "bad-target".to_string(),
                "bad-json".to_string(),
                "bad-schema".to_string(),
            ];
            bootstrap_activation(src_root, &names, &resolver);
            Some(Arc::new(resolver))
        },
    );

    let listing = sorted_dir(&mount.skills_dir());
    for skill in &["bad-target", "bad-json", "bad-schema"] {
        assert!(
            !listing.contains(&skill.to_string()),
            "{skill} should be hidden by fail-safe, got {listing:?}"
        );
        let err = std::fs::metadata(mount.skill_dir(skill)).unwrap_err();
        assert_eq!(
            err.raw_os_error(),
            Some(libc::ENOENT),
            "expected ENOENT for {skill}, got {err:?}"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests: missing activation.json -> hidden when activation mode on
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn missing_activation_hides_skill_in_activation_mode() {
    if !fuse_available() {
        eprintln!("SKIP: FUSE not available");
        return;
    }

    let mount = ActivationMount::new(
        |src| {
            create_skill_dir(src, "no-activation");
        },
        |src_root| {
            let resolver = ActiveSkillResolver::new(src_root);
            let names = vec!["no-activation".to_string()];
            bootstrap_activation(src_root, &names, &resolver);
            Some(Arc::new(resolver))
        },
    );

    let listing = sorted_dir(&mount.skills_dir());
    assert!(
        !listing.contains(&"no-activation".to_string()),
        "skill without activation.json should be hidden, got {listing:?}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests: activation mode off -> no resolver -> original behavior
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn activation_mode_off_preserves_original_behavior() {
    if !fuse_available() {
        eprintln!("SKIP: FUSE not available");
        return;
    }

    let mount = ActivationMount::new(
        |src| {
            create_skill_dir(src, "alpha");
            create_skill_dir(src, "beta");
        },
        |_src_root| {
            // No resolver attached — simulates activation mode = off.
            None
        },
    );

    let listing = sorted_dir(&mount.skills_dir());
    assert!(
        listing.contains(&"alpha".to_string()),
        "alpha should be visible without resolver, got {listing:?}"
    );
    assert!(
        listing.contains(&"beta".to_string()),
        "beta should be visible without resolver, got {listing:?}"
    );

    let md = std::fs::read_to_string(mount.skill_md("alpha")).expect("read SKILL.md");
    assert!(!md.is_empty(), "SKILL.md should be readable");
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests: fallback snapshot read respects fd pin
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn snapshot_fd_pin_preserves_content_after_resolver_change() {
    if !fuse_available() {
        eprintln!("SKIP: FUSE not available");
        return;
    }

    let mount = ActivationMount::new(
        |src| {
            create_skill_dir(src, "pinned");
            write_snapshot(
                src,
                "pinned",
                "v000001.snapshot",
                "---\nname: pinned\ndescription: snapshot v1\n---\n",
            );
            write_activation(
                src,
                "pinned",
                r#"{"schemaVersion": 1, "target": ".skill-meta/versions/v000001.snapshot"}"#,
            );
        },
        |src_root| {
            let resolver = ActiveSkillResolver::new(src_root);
            let names = vec!["pinned".to_string()];
            bootstrap_activation(src_root, &names, &resolver);
            Some(Arc::new(resolver))
        },
    );

    assert!(mount.skill_md("pinned").exists());
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests: non-FUSE unit tests for bootstrap_activation
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn bootstrap_activation_mixed_results() {
    let dir = tempfile::tempdir().unwrap();

    // Valid snapshot
    create_skill_dir(dir.path(), "good");
    let snap = dir
        .path()
        .join("good/.skill-meta/versions/v000001.snapshot");
    std::fs::create_dir_all(&snap).unwrap();
    std::fs::write(snap.join("SKILL.md"), "---\nname: good\n---\n").unwrap();
    write_activation(
        dir.path(),
        "good",
        r#"{"schemaVersion": 1, "target": ".skill-meta/versions/v000001.snapshot"}"#,
    );

    // Null target
    create_skill_dir(dir.path(), "hidden-explicit");
    write_activation(
        dir.path(),
        "hidden-explicit",
        r#"{"schemaVersion": 1, "target": null}"#,
    );

    // Invalid JSON
    create_skill_dir(dir.path(), "bad");
    let meta = dir.path().join("bad/.skill-meta");
    std::fs::create_dir_all(&meta).unwrap();
    std::fs::write(meta.join("activation.json"), "BAD").unwrap();

    // Missing file
    create_skill_dir(dir.path(), "missing");

    let resolver = ActiveSkillResolver::new(dir.path());
    let names = vec![
        "good".to_string(),
        "hidden-explicit".to_string(),
        "bad".to_string(),
        "missing".to_string(),
    ];
    let results = bootstrap_activation(dir.path(), &names, &resolver);

    assert!(results[0].1.is_ok());
    assert!(results[1].1.is_ok());
    assert!(results[2].1.is_err());
    assert!(results[3].1.is_err());

    assert!(matches!(
        resolver.get("good"),
        Some(ActiveTarget::Snapshot { .. })
    ));
    assert!(matches!(
        resolver.get("hidden-explicit"),
        Some(ActiveTarget::Hidden { .. })
    ));
    assert!(matches!(
        resolver.get("bad"),
        Some(ActiveTarget::Hidden { .. })
    ));
    assert!(matches!(
        resolver.get("missing"),
        Some(ActiveTarget::Hidden { .. })
    ));
}

#[test]
fn snapshot_nonexistent_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    create_skill_dir(dir.path(), "alpha");
    write_activation(
        dir.path(),
        "alpha",
        r#"{"schemaVersion": 1, "target": ".skill-meta/versions/v000001.snapshot"}"#,
    );

    let err = load_activation(&dir.path().join("alpha")).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("does not exist"),
        "expected 'does not exist' in error: {msg}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// A2: xattr activation integration tests
// ─────────────────────────────────────────────────────────────────────────────

fn cstr(path: &Path) -> std::ffi::CString {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(path.as_os_str().as_bytes()).expect("path -> CString")
}

fn cname(name: &str) -> std::ffi::CString {
    std::ffi::CString::new(name).expect("xattr name -> CString")
}

fn lsetxattr(path: &Path, name: &str, value: &[u8], flags: i32) -> Result<(), i32> {
    let cp = cstr(path);
    let cn = cname(name);
    let rc = unsafe {
        libc::lsetxattr(
            cp.as_ptr(),
            cn.as_ptr(),
            value.as_ptr() as *const libc::c_void,
            value.len(),
            flags,
        )
    };
    if rc != 0 {
        Err(std::io::Error::last_os_error()
            .raw_os_error()
            .unwrap_or(libc::EIO))
    } else {
        Ok(())
    }
}

fn user_xattr_supported(dir: &Path) -> bool {
    match lsetxattr(dir, "user.skillfs.probe", b"1", 0) {
        Ok(()) => {
            let cp = cstr(dir);
            let cn = cname("user.skillfs.probe");
            unsafe { libc::lremovexattr(cp.as_ptr(), cn.as_ptr()) };
            true
        }
        Err(_) => false,
    }
}

fn set_activation_xattr(dir: &Path, value: &str) {
    lsetxattr(dir, ACTIVATION_XATTR, value.as_bytes(), 0).expect("lsetxattr for activation xattr");
}

fn workspace_target_dir() -> Option<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for ancestor in manifest_dir.ancestors() {
        if ancestor.join("Cargo.lock").exists() {
            return Some(ancestor.join("target").join("xattr-tests"));
        }
    }
    None
}

fn xattr_capable_tempdir() -> Option<tempfile::TempDir> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(env_path) = std::env::var("SKILLFS_XATTR_TEST_ROOT") {
        if !env_path.is_empty() {
            candidates.push(PathBuf::from(env_path));
        }
    }
    if let Some(target) = workspace_target_dir() {
        candidates.push(target);
    }
    if let Some(home) = std::env::var_os("HOME") {
        let mut path = PathBuf::from(home);
        path.push(".cache");
        path.push("skillfs-xattr-tests");
        candidates.push(path);
    }
    for cand in candidates {
        if std::fs::create_dir_all(&cand).is_err() {
            continue;
        }
        let td = match tempfile::Builder::new()
            .prefix("a2-integ-")
            .tempdir_in(&cand)
        {
            Ok(d) => d,
            Err(_) => continue,
        };
        if user_xattr_supported(td.path()) {
            return Some(td);
        }
    }
    None
}

/// A2: bootstrap_activation with xattr-only activation (no activation.json).
#[test]
fn bootstrap_xattr_only_activates_snapshot() {
    let td = match xattr_capable_tempdir() {
        Some(d) => d,
        None => {
            eprintln!("SKIP: no xattr-capable filesystem for A2 bootstrap xattr-only test");
            return;
        }
    };
    let root = td.path();

    let skill = root.join("alpha");
    std::fs::create_dir_all(skill.join(".skill-meta/versions/v000001.snapshot")).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: alpha\ndescription: test\n---\n",
    )
    .unwrap();
    std::fs::write(
        skill.join(".skill-meta/versions/v000001.snapshot/SKILL.md"),
        "---\nname: alpha\ndescription: snapshot\n---\n",
    )
    .unwrap();

    set_activation_xattr(
        &skill,
        r#"{"schemaVersion": 1, "target": ".skill-meta/versions/v000001.snapshot"}"#,
    );

    let resolver = ActiveSkillResolver::new(root);
    let results = bootstrap_activation(root, &["alpha".to_string()], &resolver);
    assert!(results[0].1.is_ok(), "xattr-only bootstrap must succeed");
    assert!(matches!(
        resolver.get("alpha"),
        Some(ActiveTarget::Snapshot { .. })
    ));
}

/// A2: bootstrap_activation with xattr/json mismatch hides skill.
#[test]
fn bootstrap_xattr_json_mismatch_hides_skill() {
    let td = match xattr_capable_tempdir() {
        Some(d) => d,
        None => {
            eprintln!("SKIP: no xattr-capable filesystem for A2 mismatch test");
            return;
        }
    };
    let root = td.path();

    let skill = root.join("alpha");
    std::fs::create_dir_all(skill.join(".skill-meta/versions/v000001.snapshot")).unwrap();
    std::fs::create_dir_all(skill.join(".skill-meta/versions/v000002.snapshot")).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: alpha\ndescription: test\n---\n",
    )
    .unwrap();

    set_activation_xattr(
        &skill,
        r#"{"schemaVersion": 1, "target": ".skill-meta/versions/v000001.snapshot"}"#,
    );
    write_activation(
        root,
        "alpha",
        r#"{"schemaVersion": 1, "target": ".skill-meta/versions/v000002.snapshot"}"#,
    );

    let resolver = ActiveSkillResolver::new(root);
    let results = bootstrap_activation(root, &["alpha".to_string()], &resolver);
    assert!(results[0].1.is_err(), "mismatch must be an error");
    assert!(
        matches!(resolver.get("alpha"), Some(ActiveTarget::Hidden { .. })),
        "mismatch must fail-safe to hidden"
    );
}

/// A2: bootstrap_activation with invalid xattr hides even if json is valid.
#[test]
fn bootstrap_invalid_xattr_hides_despite_valid_json() {
    let td = match xattr_capable_tempdir() {
        Some(d) => d,
        None => {
            eprintln!("SKIP: no xattr-capable filesystem for A2 invalid-xattr-hides test");
            return;
        }
    };
    let root = td.path();

    let skill = root.join("alpha");
    std::fs::create_dir_all(skill.join(".skill-meta/versions/v000001.snapshot")).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: alpha\ndescription: test\n---\n",
    )
    .unwrap();

    // Valid json file.
    write_activation(
        root,
        "alpha",
        r#"{"schemaVersion": 1, "target": ".skill-meta/versions/v000001.snapshot"}"#,
    );
    // Invalid xattr.
    set_activation_xattr(&skill, "NOT VALID JSON");

    let resolver = ActiveSkillResolver::new(root);
    let results = bootstrap_activation(root, &["alpha".to_string()], &resolver);
    assert!(results[0].1.is_err());
    assert!(matches!(
        resolver.get("alpha"),
        Some(ActiveTarget::Hidden { .. })
    ));
}

/// A2: FUSE mount with xattr-only activation serves snapshot content.
#[test]
fn fuse_mount_xattr_only_serves_snapshot() {
    if !fuse_available() {
        eprintln!("SKIP: FUSE not available");
        return;
    }
    let td = match xattr_capable_tempdir() {
        Some(d) => d,
        None => {
            eprintln!("SKIP: no xattr-capable filesystem for A2 FUSE test");
            return;
        }
    };

    let source = td.path();
    create_skill_dir(source, "demo");
    write_snapshot(
        source,
        "demo",
        "v000001.snapshot",
        "---\nname: demo\ndescription: xattr snapshot\n---\n",
    );
    // Only xattr, no activation.json.
    let skill_dir = source.join("demo");
    set_activation_xattr(
        &skill_dir,
        r#"{"schemaVersion": 1, "target": ".skill-meta/versions/v000001.snapshot"}"#,
    );

    let resolver = ActiveSkillResolver::new(source);
    bootstrap_activation(source, &["demo".to_string()], &resolver);

    let mountpoint = tempfile::tempdir().expect("mount tempdir");
    let mut store = skillfs_core::store::SkillStore::new();
    store.load_from_directory(source, &skillfs_core::ParseConfig::default());
    let shared: skillfs_core::SharedSkillStore = Arc::new(RwLock::new(store));

    let handle = skillfs_fuse::mount_background_configured(
        mountpoint.path(),
        source,
        shared,
        skillfs_fuse::MountOptions::default(),
        false,
        skillfs_fuse::MountConfig {
            active_resolver: Some(Arc::new(resolver)),
            ..skillfs_fuse::MountConfig::default()
        },
    )
    .expect("mount_background_configured");
    std::thread::sleep(Duration::from_millis(300));

    let skills_dir = mountpoint.path().join("skills");
    let listing = sorted_dir(&skills_dir);
    assert!(
        listing.contains(&"demo".to_string()),
        "xattr-activated skill must be visible, got {listing:?}"
    );

    let skill_md = mountpoint.path().join("skills/demo/SKILL.md");
    let content = std::fs::read_to_string(&skill_md).expect("read xattr-activated SKILL.md");
    assert!(
        content.contains("xattr snapshot"),
        "must serve snapshot content via xattr activation, got: {content}"
    );

    drop(handle);
    std::thread::sleep(Duration::from_millis(150));
    let _ = std::process::Command::new("fusermount3")
        .args(["-u", &mountpoint.path().to_string_lossy()])
        .output();
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests: canonical skill identity — directory basename, not frontmatter name
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn frontmatter_name_mismatch_uses_directory_basename_in_store_and_mount() {
    if !fuse_available() {
        eprintln!("SKIP: FUSE not available");
        return;
    }

    let mount = ActivationMount::new(
        |src| {
            let skill_dir = src.join("tianqi-weather");
            std::fs::create_dir_all(&skill_dir).unwrap();
            std::fs::write(
                skill_dir.join("SKILL.md"),
                "---\nname: 天气\ndescription: weather\n---\n",
            )
            .unwrap();
        },
        |_src_root| None,
    );

    let listing = sorted_dir(&mount.skills_dir());
    assert!(
        listing.contains(&"tianqi-weather".to_string()),
        "/skills/tianqi-weather must be visible, got {listing:?}"
    );
    assert!(
        !listing.contains(&"天气".to_string()),
        "/skills/天气 must NOT appear from frontmatter name, got {listing:?}"
    );

    assert!(mount.skill_md("tianqi-weather").exists());

    let err = std::fs::metadata(mount.skill_dir("天气")).unwrap_err();
    assert_eq!(
        err.raw_os_error(),
        Some(libc::ENOENT),
        "expected ENOENT for frontmatter-name path, got {err:?}"
    );
}

#[test]
fn frontmatter_name_mismatch_with_activation_uses_directory_basename() {
    if !fuse_available() {
        eprintln!("SKIP: FUSE not available");
        return;
    }

    let mount = ActivationMount::new(
        |src| {
            let skill_dir = src.join("tianqi-weather");
            std::fs::create_dir_all(&skill_dir).unwrap();
            std::fs::write(
                skill_dir.join("SKILL.md"),
                "---\nname: 天气\ndescription: weather\n---\n",
            )
            .unwrap();
            write_snapshot(
                src,
                "tianqi-weather",
                "v000001.snapshot",
                "---\nname: tianqi-weather\ndescription: snapshot\n---\n",
            );
            write_activation(
                src,
                "tianqi-weather",
                r#"{"schemaVersion": 1, "target": ".skill-meta/versions/v000001.snapshot"}"#,
            );
        },
        |src_root| {
            let resolver = ActiveSkillResolver::new(src_root);
            let names = vec!["tianqi-weather".to_string()];
            bootstrap_activation(src_root, &names, &resolver);
            Some(Arc::new(resolver))
        },
    );

    let listing = sorted_dir(&mount.skills_dir());
    assert!(
        listing.contains(&"tianqi-weather".to_string()),
        "/skills/tianqi-weather must be visible with activation, got {listing:?}"
    );
    assert!(
        !listing.contains(&"天气".to_string()),
        "/skills/天气 must NOT appear, got {listing:?}"
    );

    let err = std::fs::metadata(mount.skill_dir("天气")).unwrap_err();
    assert_eq!(err.raw_os_error(), Some(libc::ENOENT));
}
