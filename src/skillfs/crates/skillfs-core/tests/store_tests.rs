//! Integration tests for the store module using fixture directories.

mod common;
use std::path::{Path, PathBuf};

use common::load_fixture;
use skillfs_core::{ParseConfig, store::SkillStore};

fn add_skill(source_dir: &Path, name: &str, content: &str) -> PathBuf {
    let skill_dir = source_dir.join(name);
    std::fs::create_dir_all(&skill_dir).unwrap();
    let skill_md = skill_dir.join("SKILL.md");
    std::fs::write(&skill_md, content).unwrap();
    skill_md
}

fn add_skill_from_fixture(source_dir: &Path, name: &str, fixture_name: &str) -> PathBuf {
    let content = load_fixture(fixture_name);
    add_skill(source_dir, name, &content)
}

fn load_store(source_dir: &Path) -> (SkillStore, Vec<skillfs_core::store::LoadError>) {
    let mut store = SkillStore::new();
    let errors = store.load_from_directory(source_dir, &ParseConfig::default());
    (store, errors)
}

#[test]
fn test_load_from_directory_with_valid_skills() {
    let source_dir = tempfile::tempdir().unwrap();

    // Add multiple valid skills (skill name comes from file content, not directory name)
    add_skill_from_fixture(source_dir.path(), "web-search-dir", "valid_full.md");
    add_skill_from_fixture(source_dir.path(), "hello-world-dir", "valid_minimal.md");
    add_skill_from_fixture(source_dir.path(), "code-review-dir", "valid_no_params.md");

    let (store, _errors) = load_store(source_dir.path());

    assert_eq!(store.len(), 3);
    // Names come from SKILL.md frontmatter, not directory names
    assert!(store.get("web-search").is_some());
    assert!(store.get("hello-world").is_some());
    assert!(store.get("code-review").is_some());
}

#[test]
fn test_load_from_directory_with_mixed_quality() {
    let source_dir = tempfile::tempdir().unwrap();

    // Add skills with various parse statuses
    // Note: skill names come from file content, not directory names
    add_skill_from_fixture(source_dir.path(), "valid-dir", "valid_full.md");
    add_skill_from_fixture(source_dir.path(), "degraded-dir", "missing_frontmatter.md");
    add_skill_from_fixture(source_dir.path(), "errored-dir", "invalid_yaml.md");

    let (store, _errors) = load_store(source_dir.path());

    // All should be loaded (even degraded/errored)
    assert_eq!(store.len(), 3);

    // Check parse statuses by iterating (names come from file content)
    let skills: Vec<_> = store.list();
    assert_eq!(skills.len(), 3);
}

#[test]
fn test_load_from_directory_ignores_hidden() {
    let source_dir = tempfile::tempdir().unwrap();

    // Add a normal skill (name comes from file: hello-world)
    add_skill_from_fixture(source_dir.path(), "visible-dir", "valid_minimal.md");

    // Create a hidden directory with a skill
    let hidden_dir = source_dir.path().join(".hidden");
    std::fs::create_dir(&hidden_dir).unwrap();
    std::fs::write(hidden_dir.join("SKILL.md"), "---\nname: hidden\n---\n").unwrap();

    let (store, _errors) = load_store(source_dir.path());

    assert_eq!(store.len(), 1);
    // Name comes from SKILL.md frontmatter
    assert!(store.get("hello-world").is_some());
    assert!(store.get("hidden").is_none());
}

#[test]
fn test_load_from_directory_ignores_files() {
    let source_dir = tempfile::tempdir().unwrap();

    add_skill_from_fixture(source_dir.path(), "valid", "valid_minimal.md");

    // Create a file (not directory) in source
    std::fs::write(source_dir.path().join("not-a-dir.txt"), "not a skill").unwrap();

    let (store, errors) = load_store(source_dir.path());

    assert!(errors.is_empty());
    assert_eq!(store.len(), 1);
}

#[test]
fn test_load_from_directory_skips_no_skill_md() {
    let source_dir = tempfile::tempdir().unwrap();

    add_skill_from_fixture(source_dir.path(), "valid", "valid_minimal.md");

    // Create a directory without SKILL.md
    let empty_dir = source_dir.path().join("empty-dir");
    std::fs::create_dir(&empty_dir).unwrap();
    std::fs::write(empty_dir.join("README.md"), "not a skill file").unwrap();

    let (store, errors) = load_store(source_dir.path());

    assert!(errors.is_empty());
    assert_eq!(store.len(), 1);
}

#[test]
fn test_load_from_directory_empty() {
    let source_dir = tempfile::tempdir().unwrap();

    let (store, errors) = load_store(source_dir.path());

    assert!(errors.is_empty());
    assert!(store.is_empty());
}

#[test]
fn test_reload_updates_existing() {
    let source_dir = tempfile::tempdir().unwrap();

    // First load
    add_skill(
        source_dir.path(),
        "test-skill",
        "---\nname: test-skill\ndescription: Original\n---\n",
    );
    let (store, _errors) = load_store(source_dir.path());
    assert_eq!(
        store.get("test-skill").unwrap().metadata.description,
        "Original"
    );

    // Update the skill file
    let skill_path = source_dir.path().join("test-skill").join("SKILL.md");
    std::fs::write(
        &skill_path,
        "---\nname: test-skill\ndescription: Updated\n---\n",
    )
    .unwrap();

    // Reload
    let (store, _errors) = load_store(source_dir.path());

    assert_eq!(
        store.get("test-skill").unwrap().metadata.description,
        "Updated"
    );
}
