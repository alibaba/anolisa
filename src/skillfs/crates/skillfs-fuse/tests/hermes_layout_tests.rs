//! Integration tests for the Hermes skill layout mode.

mod common;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use common::{MountFixture, create_skill_dir, list_dir_names};

fn seed_hermes_workspace(dir: &Path) {
    std::fs::create_dir_all(dir.join(".hub")).unwrap();
    std::fs::write(dir.join(".hub/config.json"), r#"{"version": 1}"#).unwrap();
    std::fs::write(dir.join(".bundled_manifest"), "manifest-content").unwrap();
    std::fs::write(dir.join(".no-bundled-skills"), "").unwrap();

    let apple_notes = dir.join("apple/apple-notes");
    std::fs::create_dir_all(&apple_notes).unwrap();
    std::fs::write(
        apple_notes.join("SKILL.md"),
        "---\nname: apple-notes\ndescription: notes\n---\nApple Notes skill body.\n",
    )
    .unwrap();

    let apple_music = dir.join("apple/apple-music");
    std::fs::create_dir_all(&apple_music).unwrap();
    std::fs::write(
        apple_music.join("SKILL.md"),
        "---\nname: apple-music\ndescription: music\n---\n",
    )
    .unwrap();
}

// -----------------------------------------------------------------------
// 1. Flat mode behavior unchanged
// -----------------------------------------------------------------------

#[test]
fn flat_mode_in_place_unchanged() {
    skip_if_no_fuse!();

    let fix = MountFixture::in_place(|dir| {
        create_skill_dir(dir, "my-skill");
    });

    let md_path = fix.mountpoint().join("my-skill/SKILL.md");
    let content = std::fs::read_to_string(&md_path).expect("read SKILL.md");
    assert!(
        content.contains("my-skill"),
        "flat in-place SKILL.md should be readable"
    );
}

// -----------------------------------------------------------------------
// 2. Hermes mode management path passthrough
// -----------------------------------------------------------------------

#[test]
fn hermes_management_path_stat() {
    skip_if_no_fuse!();

    let fix = MountFixture::in_place_hermes(|dir| {
        seed_hermes_workspace(dir);
    });

    let hub = fix.mountpoint().join(".hub");
    let meta = std::fs::metadata(&hub).expect("stat .hub");
    assert!(meta.is_dir(), ".hub must be a directory");
}

#[test]
fn hermes_management_path_readdir() {
    skip_if_no_fuse!();

    let fix = MountFixture::in_place_hermes(|dir| {
        seed_hermes_workspace(dir);
    });

    let hub = fix.mountpoint().join(".hub");
    let entries = list_dir_names(&hub);
    assert!(
        entries.contains(&"config.json".to_string()),
        ".hub readdir should contain config.json, got: {:?}",
        entries
    );
}

#[test]
fn hermes_management_path_mkdir_eexist() {
    skip_if_no_fuse!();

    let fix = MountFixture::in_place_hermes(|dir| {
        seed_hermes_workspace(dir);
    });

    let hub = fix.mountpoint().join(".hub");
    assert!(hub.exists(), "stat .hub should succeed first");
    let err = std::fs::create_dir(&hub).expect_err("mkdir .hub should fail");
    assert_eq!(
        err.raw_os_error(),
        Some(libc::EEXIST),
        "mkdir existing .hub must return EEXIST, got: {}",
        err
    );
}

// -----------------------------------------------------------------------
// 3. Hermes mode manifest passthrough
// -----------------------------------------------------------------------

#[test]
fn hermes_manifest_stat_and_read() {
    skip_if_no_fuse!();

    let fix = MountFixture::in_place_hermes(|dir| {
        seed_hermes_workspace(dir);
    });

    let manifest = fix.mountpoint().join(".bundled_manifest");
    let meta = std::fs::metadata(&manifest).expect("stat .bundled_manifest");
    assert!(meta.is_file(), ".bundled_manifest must be a regular file");

    let content = std::fs::read_to_string(&manifest).expect("read .bundled_manifest");
    assert_eq!(content, "manifest-content");
}

// -----------------------------------------------------------------------
// 4. Hermes mode category dir is container
// -----------------------------------------------------------------------

#[test]
fn hermes_category_dir_readdir() {
    skip_if_no_fuse!();

    let fix = MountFixture::in_place_hermes(|dir| {
        seed_hermes_workspace(dir);
    });

    let apple = fix.mountpoint().join("apple");
    let entries = list_dir_names(&apple);
    assert!(
        entries.contains(&"apple-notes".to_string()),
        "apple/ readdir should contain apple-notes, got: {:?}",
        entries
    );
    assert!(
        entries.contains(&"apple-music".to_string()),
        "apple/ readdir should contain apple-music, got: {:?}",
        entries
    );
}

// -----------------------------------------------------------------------
// 5. Hermes mode nested skill leaf readable
// -----------------------------------------------------------------------

#[test]
fn hermes_nested_skill_md_readable() {
    skip_if_no_fuse!();

    let fix = MountFixture::in_place_hermes(|dir| {
        seed_hermes_workspace(dir);
    });

    let md = fix.mountpoint().join("apple/apple-notes/SKILL.md");
    let content = std::fs::read_to_string(&md).expect("read nested SKILL.md");
    assert!(
        content.contains("Apple Notes skill body"),
        "nested SKILL.md should be readable with correct content"
    );
}

// -----------------------------------------------------------------------
// 6. Management path changes do not trigger notify
//    (path classification unit test — management paths produce HermesMeta
//     which mutate callbacks skip for observe_mutation)
// -----------------------------------------------------------------------

#[test]
fn hermes_path_classification_management() {
    use skillfs_fuse::path::{
        PathType, SkillLayout, is_hermes_management_path, parse_path_with_layout,
    };

    assert!(is_hermes_management_path(".hub"));
    assert!(is_hermes_management_path(".bundled_manifest"));
    assert!(is_hermes_management_path(".no-bundled-skills"));
    assert!(!is_hermes_management_path("apple"));

    let pt = parse_path_with_layout(Path::new("/.hub"), true, SkillLayout::Hermes);
    assert!(
        matches!(pt, PathType::HermesMeta { ref name } if name == ".hub"),
        "expected HermesMeta, got: {:?}",
        pt
    );

    let pt = parse_path_with_layout(Path::new("/.hub/config.json"), true, SkillLayout::Hermes);
    assert!(
        matches!(pt, PathType::HermesMetaChild { ref name, .. } if name == ".hub"),
        "expected HermesMetaChild, got: {:?}",
        pt
    );

    let pt = parse_path_with_layout(Path::new("/apple"), true, SkillLayout::Hermes);
    assert!(
        matches!(pt, PathType::CategoryDir { ref category } if category == "apple"),
        "expected CategoryDir, got: {:?}",
        pt
    );
}

// -----------------------------------------------------------------------
// 7. Nested skill source-relative path preserved
// -----------------------------------------------------------------------

#[test]
fn hermes_nested_skill_path_preserved() {
    use skillfs_fuse::path::{PathType, SkillLayout, parse_path_with_layout};

    let pt = parse_path_with_layout(Path::new("/apple/apple-notes"), true, SkillLayout::Hermes);
    match pt {
        PathType::NestedSkillDir {
            category,
            skill_name,
        } => {
            assert_eq!(category, "apple");
            assert_eq!(skill_name, "apple-notes");
        }
        other => panic!("expected NestedSkillDir, got: {:?}", other),
    }

    let pt = parse_path_with_layout(
        Path::new("/apple/apple-notes/SKILL.md"),
        true,
        SkillLayout::Hermes,
    );
    match pt {
        PathType::NestedSkillMd {
            category,
            skill_name,
        } => {
            assert_eq!(category, "apple");
            assert_eq!(skill_name, "apple-notes");
        }
        other => panic!("expected NestedSkillMd, got: {:?}", other),
    }

    let pt = parse_path_with_layout(
        Path::new("/apple/apple-notes/scripts/run.sh"),
        true,
        SkillLayout::Hermes,
    );
    match pt {
        PathType::NestedPassthrough {
            category,
            skill_name,
            relative_path,
        } => {
            assert_eq!(category, "apple");
            assert_eq!(skill_name, "apple-notes");
            assert_eq!(relative_path, std::path::PathBuf::from("scripts/run.sh"));
        }
        other => panic!("expected NestedPassthrough, got: {:?}", other),
    }
}

// -----------------------------------------------------------------------
// 8. Management path writes do not trigger notify
// -----------------------------------------------------------------------

#[test]
fn hermes_management_path_write_no_notify() {
    skip_if_no_fuse!();

    use parking_lot::RwLock;
    use skillfs_core::{ParseConfig, SharedSkillStore, store::SkillStore};
    use skillfs_fuse::security::{InMemoryNotifyClient, NotifyController};
    use skillfs_fuse::{MountConfig, MountOptions, SkillLayout, mount_background_configured};

    let source = tempfile::tempdir().unwrap();
    seed_hermes_workspace(source.path());

    let mut store = SkillStore::new();
    store.load_from_directory(source.path(), &ParseConfig::default());
    let shared: SharedSkillStore = Arc::new(RwLock::new(store));

    let mountpoint = tempfile::tempdir().unwrap();

    let notify_client = Arc::new(InMemoryNotifyClient::new());
    let notify_ctrl = NotifyController::new(
        notify_client.clone(),
        source.path().to_path_buf(),
        Duration::from_millis(50),
        5000,
    );

    let config = MountConfig {
        notify_controller: Some(notify_ctrl.clone()),
        skill_layout: Some(SkillLayout::Hermes),
        ..MountConfig::default()
    };

    let _handle = mount_background_configured(
        mountpoint.path(),
        source.path(),
        shared,
        MountOptions::default(),
        true,
        config,
    )
    .unwrap();

    std::thread::sleep(Duration::from_millis(300));

    let mp = mountpoint.path();

    // Write to a management path — should NOT trigger notify.
    std::fs::write(mp.join(".hub/new-file.json"), r#"{"test": true}"#).unwrap();
    std::fs::write(mp.join(".bundled_manifest"), "updated-manifest").unwrap();

    // Wait and check no notify was produced.
    std::thread::sleep(Duration::from_millis(300));
    notify_ctrl.flush_for_testing();
    assert!(
        notify_client.is_empty(),
        "management path writes must not trigger notify, got {} events",
        notify_client.len()
    );
}

// -----------------------------------------------------------------------
// 10. Hermes activation current — nested skill is readable
// -----------------------------------------------------------------------

#[test]
fn hermes_activation_current() {
    skip_if_no_fuse!();

    use parking_lot::RwLock;
    use skillfs_core::{ParseConfig, SharedSkillStore, store::SkillStore};
    use skillfs_fuse::security::{ActiveSkillResolver, ActiveTarget};
    use skillfs_fuse::{MountConfig, MountOptions, SkillLayout, mount_background_configured};

    let source = tempfile::tempdir().unwrap();
    seed_hermes_workspace(source.path());

    let mut store = SkillStore::new();
    store.load_from_directory(source.path(), &ParseConfig::default());
    let shared: SharedSkillStore = Arc::new(RwLock::new(store));

    let resolver = ActiveSkillResolver::new(source.path());
    resolver.set(
        "apple/apple-notes",
        ActiveTarget::Current {
            source_dir: source.path().join("apple/apple-notes"),
        },
    );

    let mountpoint = tempfile::tempdir().unwrap();
    let config = MountConfig {
        active_resolver: Some(Arc::new(resolver)),
        skill_layout: Some(SkillLayout::Hermes),
        ..MountConfig::default()
    };

    let _handle = mount_background_configured(
        mountpoint.path(),
        source.path(),
        shared,
        MountOptions::default(),
        true,
        config,
    )
    .unwrap();

    std::thread::sleep(Duration::from_millis(300));

    let md = mountpoint.path().join("apple/apple-notes/SKILL.md");
    let content = std::fs::read_to_string(&md).expect("read nested SKILL.md");
    assert!(
        content.contains("Apple Notes skill body"),
        "current activation must serve live source: {content}"
    );
}

// -----------------------------------------------------------------------
// 11. Hermes activation fallback — reads from snapshot
// -----------------------------------------------------------------------

#[test]
fn hermes_activation_fallback() {
    skip_if_no_fuse!();

    use parking_lot::RwLock;
    use skillfs_core::{ParseConfig, SharedSkillStore, store::SkillStore};
    use skillfs_fuse::security::{ActiveSkillResolver, ActiveTarget};
    use skillfs_fuse::{MountConfig, MountOptions, SkillLayout, mount_background_configured};

    let source = tempfile::tempdir().unwrap();
    seed_hermes_workspace(source.path());

    let snap_dir = source
        .path()
        .join("apple/apple-notes/.skill-meta/versions/v000001.snapshot");
    std::fs::create_dir_all(&snap_dir).unwrap();
    std::fs::write(
        snap_dir.join("SKILL.md"),
        "---\nname: apple-notes\ndescription: snapshot\n---\nSnapshot body.\n",
    )
    .unwrap();

    let mut store = SkillStore::new();
    store.load_from_directory(source.path(), &ParseConfig::default());
    let shared: SharedSkillStore = Arc::new(RwLock::new(store));

    let resolver = ActiveSkillResolver::new(source.path());
    resolver.set(
        "apple/apple-notes",
        ActiveTarget::Snapshot {
            snapshot_dir: snap_dir.clone(),
            version: "v000001.snapshot".to_string(),
        },
    );

    let mountpoint = tempfile::tempdir().unwrap();
    let config = MountConfig {
        active_resolver: Some(Arc::new(resolver)),
        skill_layout: Some(SkillLayout::Hermes),
        ..MountConfig::default()
    };

    let _handle = mount_background_configured(
        mountpoint.path(),
        source.path(),
        shared,
        MountOptions::default(),
        true,
        config,
    )
    .unwrap();

    std::thread::sleep(Duration::from_millis(300));

    let md = mountpoint.path().join("apple/apple-notes/SKILL.md");
    let content = std::fs::read_to_string(&md).expect("read nested SKILL.md");
    assert!(
        content.contains("Snapshot body"),
        "fallback activation must serve snapshot: {content}"
    );
}

// -----------------------------------------------------------------------
// 12. Hermes activation hidden — ENOENT on leaf, category stays visible
// -----------------------------------------------------------------------

#[test]
fn hermes_activation_hidden() {
    skip_if_no_fuse!();

    use parking_lot::RwLock;
    use skillfs_core::{ParseConfig, SharedSkillStore, store::SkillStore};
    use skillfs_fuse::security::{ActiveSkillResolver, ActiveTarget};
    use skillfs_fuse::{MountConfig, MountOptions, SkillLayout, mount_background_configured};

    let source = tempfile::tempdir().unwrap();
    seed_hermes_workspace(source.path());

    let mut store = SkillStore::new();
    store.load_from_directory(source.path(), &ParseConfig::default());
    let shared: SharedSkillStore = Arc::new(RwLock::new(store));

    let resolver = ActiveSkillResolver::new(source.path());
    resolver.set(
        "apple/apple-notes",
        ActiveTarget::Hidden {
            reason: "test hidden".to_string(),
        },
    );
    resolver.set(
        "apple/apple-music",
        ActiveTarget::Current {
            source_dir: source.path().join("apple/apple-music"),
        },
    );

    let mountpoint = tempfile::tempdir().unwrap();
    let config = MountConfig {
        active_resolver: Some(Arc::new(resolver)),
        skill_layout: Some(SkillLayout::Hermes),
        ..MountConfig::default()
    };

    let _handle = mount_background_configured(
        mountpoint.path(),
        source.path(),
        shared,
        MountOptions::default(),
        true,
        config,
    )
    .unwrap();

    std::thread::sleep(Duration::from_millis(300));

    let mp = mountpoint.path();

    // Category dir itself must still be accessible.
    let apple = mp.join("apple");
    assert!(
        apple.is_dir(),
        "category dir must remain visible even with hidden children"
    );

    // Hidden leaf must return ENOENT.
    let notes = mp.join("apple/apple-notes");
    let err = std::fs::metadata(&notes).expect_err("hidden skill must return ENOENT");
    assert_eq!(
        err.raw_os_error(),
        Some(libc::ENOENT),
        "hidden nested skill lookup must return ENOENT, got: {err}"
    );

    // Category listing must omit hidden children.
    let entries = list_dir_names(&apple);
    assert!(
        !entries.contains(&"apple-notes".to_string()),
        "hidden skill must be omitted from category listing, got: {:?}",
        entries
    );

    // Visible child must still appear.
    assert!(
        entries.contains(&"apple-music".to_string()),
        "visible skill must appear in category listing, got: {:?}",
        entries
    );
}

// -----------------------------------------------------------------------
// 13. Hermes nested SKILL.md write triggers notify
// -----------------------------------------------------------------------

#[test]
fn hermes_nested_write_triggers_notify() {
    skip_if_no_fuse!();

    use parking_lot::RwLock;
    use skillfs_core::{ParseConfig, SharedSkillStore, store::SkillStore};
    use skillfs_fuse::security::{InMemoryNotifyClient, NotifyController};
    use skillfs_fuse::{MountConfig, MountOptions, SkillLayout, mount_background_configured};

    let source = tempfile::tempdir().unwrap();
    seed_hermes_workspace(source.path());

    let mut store = SkillStore::new();
    store.load_from_directory(source.path(), &ParseConfig::default());
    let shared: SharedSkillStore = Arc::new(RwLock::new(store));

    let mountpoint = tempfile::tempdir().unwrap();

    let notify_client = Arc::new(InMemoryNotifyClient::new());
    let notify_ctrl = NotifyController::new(
        notify_client.clone(),
        source.path().to_path_buf(),
        Duration::from_millis(50),
        5000,
    );

    let config = MountConfig {
        notify_controller: Some(notify_ctrl.clone()),
        skill_layout: Some(SkillLayout::Hermes),
        ..MountConfig::default()
    };

    let _handle = mount_background_configured(
        mountpoint.path(),
        source.path(),
        shared,
        MountOptions::default(),
        true,
        config,
    )
    .unwrap();

    std::thread::sleep(Duration::from_millis(300));

    let mp = mountpoint.path();

    // Write to a nested SKILL.md.
    std::fs::write(
        mp.join("apple/apple-notes/SKILL.md"),
        "---\nname: apple-notes\ndescription: updated\n---\nUpdated.\n",
    )
    .unwrap();

    std::thread::sleep(Duration::from_millis(300));
    notify_ctrl.flush_for_testing();

    let events = notify_client.events();
    assert!(
        !events.is_empty(),
        "nested SKILL.md write must trigger notify"
    );

    let event = &events[0];
    assert_eq!(
        event.skill_name, "apple/apple-notes",
        "skillName must be category/skill"
    );
    assert!(
        event.skill_dir.ends_with("/apple/apple-notes"),
        "skillDir must end with /apple/apple-notes, got: {}",
        event.skill_dir
    );
    assert!(
        event.paths.contains(&"SKILL.md".to_string()),
        "paths must contain SKILL.md, got: {:?}",
        event.paths
    );
}

// -----------------------------------------------------------------------
// 9. Non-skill subdirectory under category is accessible
// -----------------------------------------------------------------------

#[test]
fn hermes_non_skill_subdir_accessible() {
    skip_if_no_fuse!();

    let fix = MountFixture::in_place_hermes(|dir| {
        seed_hermes_workspace(dir);
        let docs = dir.join("apple/docs");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(docs.join("readme.txt"), "documentation").unwrap();
    });

    let docs = fix.mountpoint().join("apple/docs");
    let meta = std::fs::metadata(&docs).expect("stat apple/docs");
    assert!(meta.is_dir(), "non-skill subdir must be a directory");

    let readme = fix.mountpoint().join("apple/docs/readme.txt");
    let content = std::fs::read_to_string(&readme).expect("read readme.txt");
    assert_eq!(content, "documentation");
}
