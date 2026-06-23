use std::collections::HashMap;
use std::path::Path;

use tracing::{info, warn};

use crate::parser;
use crate::{CategoryMeta, ParseConfig, SkillEntry};

// ---------------------------------------------------------------------------
// LoadError
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct LoadError {
    pub path: std::path::PathBuf,
    pub error: String,
}

// ---------------------------------------------------------------------------
// SkillStore
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct SkillStore {
    skills: HashMap<String, SkillEntry>,
    /// Category name → category metadata (from `_category.yaml`)
    pub categories: HashMap<String, CategoryMeta>,
    /// Skill name → category name (empty string = uncategorized)
    skill_categories: HashMap<String, String>,
}

impl SkillStore {
    /// Create a new empty store.
    pub fn new() -> Self {
        Self {
            skills: HashMap::new(),
            categories: HashMap::new(),
            skill_categories: HashMap::new(),
        }
    }

    /// Load all skills from a source directory (initial scan).
    ///
    /// Supports both flat and categorized layouts:
    /// - **Flat**: `{source}/{skill_name}/SKILL.md`
    /// - **Categorized**: `{source}/{category}/{skill_name}/SKILL.md`
    ///
    /// A subdirectory is treated as a **category** when it contains no
    /// `SKILL.md` of its own but has sub-subdirectories that contain
    /// `SKILL.md` files.
    pub fn load_from_directory(&mut self, source: &Path, config: &ParseConfig) -> Vec<LoadError> {
        let mut errors = Vec::new();
        let mut loaded_count = 0usize;

        let entries = match std::fs::read_dir(source) {
            Ok(e) => e,
            Err(e) => {
                errors.push(LoadError {
                    path: source.to_path_buf(),
                    error: format!("cannot read directory: {e}"),
                });
                return errors;
            }
        };

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    warn!("failed to read dir entry: {e}");
                    continue;
                }
            };

            let path = entry.path();

            // Skip non-directories
            if !path.is_dir() {
                continue;
            }

            // Skip hidden directories
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') {
                    continue;
                }
            }

            // Check max_skills limit (rough guard)
            if loaded_count >= config.max_skills {
                errors.push(LoadError {
                    path: path.clone(),
                    error: format!("max skills limit reached ({})", config.max_skills),
                });
                continue;
            }

            if is_category_dir(&path) {
                // ---- Categorized layout ----
                let cat_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();

                // Try to load _category.yaml
                let cat_meta = load_category_meta(&path, &cat_name);
                self.categories.insert(cat_name.clone(), cat_meta);

                // Load skills inside this category directory
                let cat_errors =
                    self.load_skills_from_category(&path, &cat_name, config, &mut loaded_count);
                errors.extend(cat_errors);
            } else {
                // ---- Flat layout ----
                let skill_md = path.join("SKILL.md");
                if !skill_md.exists() {
                    continue;
                }

                match parser::parse_skill_file_with_limit(&skill_md, config.max_skill_size) {
                    Ok(mut entry) => {
                        let dir_name = path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("unknown")
                            .to_string();
                        entry.metadata.name = dir_name.clone();
                        info!(name = %dir_name, "loaded skill");
                        self.upsert(entry);
                        self.skill_categories.insert(dir_name, String::new());
                        loaded_count += 1;
                    }
                    Err(e) => {
                        errors.push(LoadError {
                            path: skill_md,
                            error: e.to_string(),
                        });
                    }
                }
            }
        }

        info!(count = loaded_count, "finished loading skills");
        errors
    }

    /// Load skills from a single category directory.
    fn load_skills_from_category(
        &mut self,
        cat_path: &Path,
        cat_name: &str,
        config: &ParseConfig,
        loaded_count: &mut usize,
    ) -> Vec<LoadError> {
        let mut errors = Vec::new();

        let entries = match std::fs::read_dir(cat_path) {
            Ok(e) => e,
            Err(e) => {
                errors.push(LoadError {
                    path: cat_path.to_path_buf(),
                    error: format!("cannot read category directory: {e}"),
                });
                return errors;
            }
        };

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    warn!("failed to read dir entry in category {cat_name}: {e}");
                    continue;
                }
            };

            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') {
                    continue;
                }
            }

            if *loaded_count >= config.max_skills {
                errors.push(LoadError {
                    path: path.clone(),
                    error: format!("max skills limit reached ({})", config.max_skills),
                });
                continue;
            }

            let skill_md = path.join("SKILL.md");
            if !skill_md.exists() {
                continue;
            }

            match parser::parse_skill_file_with_limit(&skill_md, config.max_skill_size) {
                Ok(mut entry) => {
                    let dir_name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown")
                        .to_string();
                    entry.metadata.name = dir_name.clone();
                    info!(name = %dir_name, category = %cat_name, "loaded skill");
                    self.upsert(entry);
                    self.skill_categories.insert(dir_name, cat_name.to_string());
                    *loaded_count += 1;
                }
                Err(e) => {
                    errors.push(LoadError {
                        path: skill_md,
                        error: e.to_string(),
                    });
                }
            }
        }

        errors
    }

    /// Insert or update a skill entry.
    pub fn upsert(&mut self, entry: SkillEntry) {
        self.skills.insert(entry.metadata.name.clone(), entry);
    }

    /// Remove a skill by name.
    pub fn remove(&mut self, name: &str) -> Option<SkillEntry> {
        self.skill_categories.remove(name);
        self.skills.remove(name)
    }

    /// Get a skill by name.
    pub fn get(&self, name: &str) -> Option<&SkillEntry> {
        self.skills.get(name)
    }

    /// Iterate over all skills.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &SkillEntry)> {
        self.skills.iter()
    }

    /// List all skill names (sorted alphabetically).
    pub fn list(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.skills.keys().map(|s| s.as_str()).collect();
        names.sort_unstable();
        names
    }

    /// Get the number of skills.
    pub fn len(&self) -> usize {
        self.skills.len()
    }

    /// Check if store is empty.
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    /// Split store skills into (primary, secondary) based on a primary list.
    ///
    /// - `primary_list = None` -> (all_skills, empty), no filtering.
    /// - `primary_list = Some(list)` -> skills in list become primary (filtered
    ///   to those present in store); all others become secondary.
    pub fn split_primary(&self, primary_list: Option<&[String]>) -> (Vec<String>, Vec<String>) {
        match primary_list {
            None => {
                let all = self.list().iter().map(|s| s.to_string()).collect();
                (all, Vec::new())
            }
            Some(list) => {
                let primary: Vec<String> = list
                    .iter()
                    .filter(|name| self.skills.contains_key(name.as_str()))
                    .cloned()
                    .collect();
                let primary_set: std::collections::HashSet<&str> =
                    primary.iter().map(|s| s.as_str()).collect();
                let secondary: Vec<String> = self
                    .list()
                    .iter()
                    .filter(|name| !primary_set.contains(*name))
                    .map(|s| s.to_string())
                    .collect();
                (primary, secondary)
            }
        }
    }
}

impl Default for SkillStore {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Returns `true` when `dir` looks like a category container:
/// it has no `SKILL.md` of its own but contains at least one sub-directory
/// that does have a `SKILL.md`.
fn is_category_dir(dir: &Path) -> bool {
    if dir.join("SKILL.md").exists() {
        return false;
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() && p.join("SKILL.md").exists() {
                return true;
            }
        }
    }
    false
}

/// Load `_category.yaml` from `dir` if present; fall back to a default meta
/// with `name = cat_name`.
fn load_category_meta(dir: &Path, cat_name: &str) -> CategoryMeta {
    let yaml_path = dir.join("_category.yaml");
    if yaml_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&yaml_path) {
            if let Ok(meta) = serde_yaml::from_str::<CategoryMeta>(&content) {
                return meta;
            }
        }
    }
    CategoryMeta {
        name: cat_name.to_string(),
        description: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ParseStatus, SkillMetadata};
    use std::time::SystemTime;

    // Helper to create a test skill entry
    fn create_test_entry(name: &str, description: &str, tags: Vec<&str>) -> SkillEntry {
        SkillEntry {
            metadata: SkillMetadata {
                name: name.to_string(),
                description: description.to_string(),
                version: "1.0.0".to_string(),
                tags: tags.into_iter().map(|s| s.to_string()).collect(),
                enabled: true,
                requires: None,
            },
            parameters: Vec::new(),
            returns: Vec::new(),
            body: String::new(),
            parse_status: ParseStatus::Ok,
            source_path: std::path::PathBuf::new(),
            last_modified: SystemTime::UNIX_EPOCH,
        }
    }

    // -----------------------------------------------------------------------
    // Basic CRUD Tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_new_store_is_empty() {
        let store = SkillStore::new();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn test_upsert_new() {
        let mut store = SkillStore::new();
        let entry = create_test_entry("test-skill", "A test skill", vec!["test"]);

        store.upsert(entry);

        assert_eq!(store.len(), 1);
        assert!(!store.is_empty());
    }

    #[test]
    fn test_upsert_existing() {
        let mut store = SkillStore::new();
        let entry1 = create_test_entry("test-skill", "Original description", vec!["test"]);
        store.upsert(entry1);

        let entry2 =
            create_test_entry("test-skill", "Updated description", vec!["test", "updated"]);
        store.upsert(entry2);

        assert_eq!(store.len(), 1);
        let retrieved = store.get("test-skill").unwrap();
        assert_eq!(retrieved.metadata.description, "Updated description");
    }

    #[test]
    fn test_remove_existing() {
        let mut store = SkillStore::new();
        let entry = create_test_entry("test-skill", "A test skill", vec![]);
        store.upsert(entry);

        let removed = store.remove("test-skill");

        assert!(removed.is_some());
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn test_remove_nonexistent() {
        let mut store = SkillStore::new();

        let removed = store.remove("nonexistent");

        assert!(removed.is_none());
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn test_get_existing() {
        let mut store = SkillStore::new();
        let entry = create_test_entry("test-skill", "A test skill", vec![]);
        store.upsert(entry);

        let retrieved = store.get("test-skill");

        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().metadata.name, "test-skill");
    }

    #[test]
    fn test_get_nonexistent() {
        let store = SkillStore::new();

        let retrieved = store.get("nonexistent");

        assert!(retrieved.is_none());
    }

    #[test]
    fn test_list_names() {
        let mut store = SkillStore::new();
        store.upsert(create_test_entry("zebra", "Zebra skill", vec![]));
        store.upsert(create_test_entry("alpha", "Alpha skill", vec![]));
        store.upsert(create_test_entry("beta", "Beta skill", vec![]));

        let names = store.list();

        assert_eq!(names.len(), 3);
        assert_eq!(names[0], "alpha");
        assert_eq!(names[1], "beta");
        assert_eq!(names[2], "zebra");
    }

    // -----------------------------------------------------------------------
    // Manifest Tests
    // -----------------------------------------------------------------------

    // -----------------------------------------------------------------------
    // Load from Directory Tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_load_from_directory_empty() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let mut store = SkillStore::new();
        let config = ParseConfig {
            strict: false,
            max_skill_size: 1_048_576,
            max_skills: 1000,
        };

        let errors = store.load_from_directory(temp_dir.path(), &config);

        assert!(errors.is_empty());
        assert!(store.is_empty());
    }

    #[test]
    fn test_load_from_directory_ignores_hidden() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let hidden_dir = temp_dir.path().join(".hidden");
        std::fs::create_dir(&hidden_dir).unwrap();
        std::fs::write(hidden_dir.join("SKILL.md"), "---\nname: hidden\n---\n").unwrap();

        let mut store = SkillStore::new();
        let config = ParseConfig {
            strict: false,
            max_skill_size: 1_048_576,
            max_skills: 1000,
        };

        let errors = store.load_from_directory(temp_dir.path(), &config);

        assert!(errors.is_empty());
        assert!(store.is_empty()); // hidden dir should be ignored
    }

    #[test]
    fn test_load_from_directory_ignores_files() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        std::fs::write(temp_dir.path().join("not-a-dir.txt"), "not a skill").unwrap();

        let mut store = SkillStore::new();
        let config = ParseConfig {
            strict: false,
            max_skill_size: 1_048_576,
            max_skills: 1000,
        };

        let errors = store.load_from_directory(temp_dir.path(), &config);

        assert!(errors.is_empty());
        assert!(store.is_empty());
    }

    #[test]
    fn test_load_from_directory_no_skill_md() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let skill_dir = temp_dir.path().join("empty-skill");
        std::fs::create_dir(&skill_dir).unwrap();
        // No SKILL.md file

        let mut store = SkillStore::new();
        let config = ParseConfig {
            strict: false,
            max_skill_size: 1_048_576,
            max_skills: 1000,
        };

        let errors = store.load_from_directory(temp_dir.path(), &config);

        assert!(errors.is_empty());
        assert!(store.is_empty());
    }
}
