use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::{broadcast, RwLock};

use super::loader;
use super::types::{SkillConfig, SkillLevel};
use super::{COPILOT_CONFIG_DIR, SKILLS_DIR, SYSTEM_SKILLS_DIR};

/// Central manager for skill discovery, caching, hot-reload and priority
/// merging. Mirrors the role of copilot-shell's `SkillManager`.
pub struct SkillManager {
    cache: RwLock<HashMap<SkillLevel, Vec<SkillConfig>>>,
    project_root: PathBuf,
    custom_paths: Vec<PathBuf>,
    extension_paths: Vec<PathBuf>,
    change_tx: broadcast::Sender<()>,
    #[allow(dead_code)]
    watcher_handle: RwLock<Option<notify::RecommendedWatcher>>,
    /// Test-only overrides for user / system directories.
    user_dir_override: Option<PathBuf>,
    system_dir_override: Option<PathBuf>,
}

impl SkillManager {
    /// Create a new SkillManager.
    ///
    /// * `project_root` – the current project/workspace root (used for
    ///   project-level skills at `<project>/.copilot-shell/skills/`).
    /// * `custom_paths` – already-expanded custom skill directory paths from
    ///   config (`skills.custom_paths`).
    /// * `extension_paths` – skill directories contributed by loaded extensions.
    pub fn new(project_root: PathBuf, custom_paths: Vec<PathBuf>, extension_paths: Vec<PathBuf>) -> Arc<Self> {
        let (change_tx, _) = broadcast::channel(16);
        Arc::new(Self {
            cache: RwLock::new(HashMap::new()),
            project_root,
            custom_paths,
            extension_paths,
            change_tx,
            watcher_handle: RwLock::new(None),
            user_dir_override: None,
            system_dir_override: None,
        })
    }

    /// Test constructor that overrides user and system directories to avoid
    /// scanning the real home / system paths.
    #[cfg(test)]
    pub fn new_isolated(
        project_root: PathBuf,
        custom_paths: Vec<PathBuf>,
        user_dir: Option<PathBuf>,
        system_dir: Option<PathBuf>,
    ) -> Arc<Self> {
        let (change_tx, _) = broadcast::channel(16);
        Arc::new(Self {
            cache: RwLock::new(HashMap::new()),
            project_root,
            custom_paths,
            extension_paths: Vec::new(),
            change_tx,
            watcher_handle: RwLock::new(None),
            user_dir_override: user_dir,
            system_dir_override: system_dir,
        })
    }

    /// Rescan all skill directories and update the internal cache.
    pub async fn refresh(&self) {
        let mut new_cache: HashMap<SkillLevel, Vec<SkillConfig>> = HashMap::new();

        for &level in SkillLevel::all() {
            if level == SkillLevel::Custom || level == SkillLevel::Extension {
                continue; // handled separately below
            }
            if let Some(dir) = self.base_dir_of(level) {
                if dir.exists() {
                    let skills = loader::load_skills_from_dir(&dir, level);
                    if !skills.is_empty() {
                        new_cache.insert(level, skills);
                    }
                }
            }
        }

        // Custom level: multiple paths
        if !self.custom_paths.is_empty() {
            let mut custom_skills: Vec<SkillConfig> = Vec::new();
            for custom_path in &self.custom_paths {
                if custom_path.exists() {
                    let skills =
                        loader::load_skills_from_dir(custom_path, SkillLevel::Custom);
                    custom_skills.extend(skills);
                }
            }
            if !custom_skills.is_empty() {
                new_cache.insert(SkillLevel::Custom, custom_skills);
            }
        }

        // Extension level: multiple paths from loaded extensions
        if !self.extension_paths.is_empty() {
            let mut ext_skills: Vec<SkillConfig> = Vec::new();
            for ext_path in &self.extension_paths {
                if ext_path.exists() {
                    let skills =
                        loader::load_skills_from_dir(ext_path, SkillLevel::Extension);
                    ext_skills.extend(skills);
                }
            }
            if !ext_skills.is_empty() {
                new_cache.insert(SkillLevel::Extension, ext_skills);
            }
        }

        *self.cache.write().await = new_cache;
        let _ = self.change_tx.send(());
    }

    /// Return a deduplicated, priority-merged list of all available skills.
    /// Higher-priority levels (Project > Custom > User > Extension > System)
    /// shadow lower-priority skills with the same name.
    pub async fn list(&self) -> Vec<SkillConfig> {
        let cache = self.cache.read().await;
        let mut merged: HashMap<String, SkillConfig> = HashMap::new();

        // Insert in reverse priority order so that higher-priority entries
        // overwrite lower ones.
        for &level in SkillLevel::all().iter().rev() {
            if let Some(skills) = cache.get(&level) {
                for skill in skills {
                    merged.insert(skill.name.clone(), skill.clone());
                }
            }
        }

        let mut result: Vec<_> = merged.into_values().collect();
        result.sort_by(|a, b| a.name.cmp(&b.name));
        result
    }

    /// Look up a single skill by name, returning the highest-priority match.
    pub async fn load(&self, name: &str) -> Option<SkillConfig> {
        let cache = self.cache.read().await;
        for &level in SkillLevel::all() {
            if let Some(skills) = cache.get(&level) {
                if let Some(skill) = skills.iter().find(|s| s.name == name) {
                    return Some(skill.clone());
                }
            }
        }
        None
    }

    /// Subscribe to change notifications (e.g. after hot-reload).
    #[allow(dead_code)]
    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        self.change_tx.subscribe()
    }

    /// Start file-system watchers for all relevant skill directories.
    /// Changes are debounced (150 ms) then trigger `refresh()`.
    pub async fn start_watching(self: &Arc<Self>) {
        use notify::{RecursiveMode, Watcher};

        let (fs_tx, mut fs_rx) = tokio::sync::mpsc::channel::<()>(16);

        let watcher_result =
            notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
                if res.is_ok() {
                    let _ = fs_tx.try_send(());
                }
            });

        let mut watcher = match watcher_result {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!(target: "skill_manager", "Failed to create file watcher: {e}");
                return;
            }
        };

        for dir in self.watch_dirs() {
            if dir.exists() {
                if let Err(e) = watcher.watch(&dir, RecursiveMode::Recursive) {
                    tracing::warn!(
                        target: "skill_manager",
                        "Failed to watch {}: {e}",
                        dir.display()
                    );
                }
            }
        }

        *self.watcher_handle.write().await = Some(watcher);

        // Spawn debounce task
        let manager = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                // Wait for the first change event
                if fs_rx.recv().await.is_none() {
                    break;
                }
                // Debounce: wait 150 ms, drain any further events
                tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;
                while fs_rx.try_recv().is_ok() {}

                manager.refresh().await;
            }
        });
    }

    // ── private helpers ──────────────────────────────────────────────

    fn base_dir_of(&self, level: SkillLevel) -> Option<PathBuf> {
        match level {
            SkillLevel::Project => {
                // Skip if project_root is the same as home (avoids double-scan)
                let home = dirs::home_dir().and_then(|h| h.canonicalize().ok());
                let project = self.project_root.canonicalize().ok();
                if home.is_some() && home == project {
                    None
                } else {
                    Some(
                        self.project_root
                            .join(COPILOT_CONFIG_DIR)
                            .join(SKILLS_DIR),
                    )
                }
            }
            SkillLevel::Custom => None, // custom paths are iterated separately
            SkillLevel::Extension => None, // extension paths are iterated separately
            SkillLevel::User => {
                if let Some(ref p) = self.user_dir_override {
                    return Some(p.clone());
                }
                dirs::home_dir().map(|h| h.join(COPILOT_CONFIG_DIR).join(SKILLS_DIR))
            }
            SkillLevel::System => {
                if let Some(ref p) = self.system_dir_override {
                    return Some(p.clone());
                }
                Some(PathBuf::from(SYSTEM_SKILLS_DIR))
            }
        }
    }

    fn watch_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        for &level in SkillLevel::all() {
            if let Some(d) = self.base_dir_of(level) {
                dirs.push(d);
            }
        }
        dirs.extend(self.custom_paths.iter().cloned());
        dirs.extend(self.extension_paths.iter().cloned());
        dirs
    }
}

/// Expand `~`, `${VAR}`, and `$VAR` in a path string.
pub fn expand_path(raw: &str) -> Option<PathBuf> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }

    let expanded = if raw == "~" {
        dirs::home_dir()?
    } else if let Some(rest) = raw.strip_prefix("~/") {
        dirs::home_dir()?.join(rest)
    } else {
        PathBuf::from(raw)
    };

    // Expand ${VAR} and $VAR in each component
    let s = expanded.to_string_lossy().to_string();
    Some(PathBuf::from(expand_env_vars(&s)))
}

fn expand_env_vars(s: &str) -> String {
    let mut result = s.to_string();

    // ${VAR}
    let mut search_from = 0;
    while let Some(pos) = result[search_from..].find("${") {
        let start = search_from + pos;
        if let Some(end) = result[start..].find('}') {
            let var_name = &result[start + 2..start + end];
            match std::env::var(var_name) {
                Ok(val) => {
                    result = format!("{}{}{}", &result[..start], val, &result[start + end + 1..]);
                    search_from = start + val.len();
                }
                Err(_) => {
                    search_from = start + end + 1;
                }
            }
        } else {
            break;
        }
    }

    // $VAR (only when not already handled as ${VAR})
    let mut out = String::new();
    let mut chars = result.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '$' && chars.peek().map_or(false, |c| c.is_ascii_alphabetic() || *c == '_') {
            let mut var = String::new();
            while chars
                .peek()
                .map_or(false, |c| c.is_ascii_alphanumeric() || *c == '_')
            {
                var.push(chars.next().unwrap());
            }
            out.push_str(&std::env::var(&var).unwrap_or_else(|_| format!("${}", var)));
        } else {
            out.push(c);
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn make_skill_file(dir: &Path, skill_name: &str, description: &str) {
        let skills_dir = dir.join(COPILOT_CONFIG_DIR).join(SKILLS_DIR);
        let skill_dir = skills_dir.join(skill_name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join(super::super::SKILL_MANIFEST),
            format!(
                "---\nname: {skill_name}\ndescription: {description}\n---\n\nBody of {skill_name}."
            ),
        )
        .unwrap();
    }

    /// Create an isolated manager that only scans project + custom dirs.
    fn isolated_manager(
        project_root: &Path,
        custom_paths: Vec<PathBuf>,
    ) -> Arc<SkillManager> {
        let empty = tempfile::tempdir().unwrap();
        SkillManager::new_isolated(
            project_root.to_path_buf(),
            custom_paths,
            Some(empty.path().join("nonexistent-user")),
            Some(empty.path().join("nonexistent-system")),
        )
    }

    fn make_flat_skill_file(dir: &Path, skill_name: &str) {
        let skills_dir = dir.join(COPILOT_CONFIG_DIR).join(SKILLS_DIR);
        std::fs::create_dir_all(&skills_dir).unwrap();
        std::fs::write(
            skills_dir.join(format!("{skill_name}.md")),
            format!(
                "---\nname: {skill_name}\ndescription: flat desc\n---\n\nFlat body."
            ),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn priority_override() {
        let project_dir = tempfile::tempdir().unwrap();
        let user_dir = tempfile::tempdir().unwrap();

        // Create a user-level skill
        let user_skills = user_dir
            .path()
            .join(COPILOT_CONFIG_DIR)
            .join(SKILLS_DIR);
        std::fs::create_dir_all(user_skills.join("shared")).unwrap();
        std::fs::write(
            user_skills.join("shared").join("SKILL.md"),
            "---\nname: shared\ndescription: user version\n---\n\nUser body.",
        )
        .unwrap();

        // Create a project-level skill with the same name
        make_skill_file(project_dir.path(), "shared", "project version");

        let mgr = SkillManager::new_isolated(
            project_dir.path().to_path_buf(),
            vec![],
            Some(user_skills.clone()),
            Some(PathBuf::from("/nonexistent-sys")),
        );
        mgr.refresh().await;

        let all = mgr.list().await;
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "shared");
        assert_eq!(all[0].level, SkillLevel::Project);
        assert_eq!(all[0].description, "project version");
    }

    #[tokio::test]
    async fn custom_paths_loading() {
        let custom_dir = tempfile::tempdir().unwrap();
        let skill_dir = custom_dir.path().join("my-custom-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: my-custom-skill\ndescription: custom\n---\n\nCustom body.",
        )
        .unwrap();

        let project_dir = tempfile::tempdir().unwrap();
        let mgr = isolated_manager(
            project_dir.path(),
            vec![custom_dir.path().to_path_buf()],
        );
        mgr.refresh().await;

        let all = mgr.list().await;
        assert!(all.iter().any(|s| s.name == "my-custom-skill"));
    }

    #[tokio::test]
    async fn system_dir_loading() {
        let sys_dir = tempfile::tempdir().unwrap();
        let skill_dir = sys_dir.path().join("sys-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: sys-skill\ndescription: system\n---\n\nSystem body.",
        )
        .unwrap();

        let project_dir = tempfile::tempdir().unwrap();
        let mgr = SkillManager::new_isolated(
            project_dir.path().to_path_buf(),
            vec![],
            Some(PathBuf::from("/nonexistent-user")),
            Some(sys_dir.path().to_path_buf()),
        );
        mgr.refresh().await;

        let skill = mgr.load("sys-skill").await.unwrap();
        assert_eq!(skill.level, SkillLevel::System);
    }

    #[tokio::test]
    async fn watcher_triggers_refresh() {
        let project_dir = tempfile::tempdir().unwrap();
        let custom_dir = tempfile::tempdir().unwrap();

        let mgr = isolated_manager(
            project_dir.path(),
            vec![custom_dir.path().to_path_buf()],
        );
        mgr.refresh().await;
        assert!(mgr.list().await.is_empty());

        mgr.start_watching().await;

        // Create a new skill file in the custom dir
        let new_skill_dir = custom_dir.path().join("new-skill");
        std::fs::create_dir_all(&new_skill_dir).unwrap();
        std::fs::write(
            new_skill_dir.join("SKILL.md"),
            "---\nname: new-skill\ndescription: dynamic\n---\n\nDynamic body.",
        )
        .unwrap();

        // Wait for the watcher to pick up the new skill (poll up to 5s).
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(5);
        loop {
            let all = mgr.list().await;
            if all.iter().any(|s| s.name == "new-skill") {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                let names: Vec<&String> = all.iter().map(|s| &s.name).collect();
                panic!(
                    "watcher did not pick up new-skill within 5s, found: {:?}",
                    names
                );
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
    }

    #[test]
    fn expand_path_tilde() {
        let p = expand_path("~/foo/bar").unwrap();
        let home = dirs::home_dir().unwrap();
        assert_eq!(p, home.join("foo/bar"));
    }

    #[test]
    fn expand_path_envvar() {
        std::env::set_var("COSH_TEST_EXPAND", "/custom");
        let p = expand_path("${COSH_TEST_EXPAND}/skills").unwrap();
        assert_eq!(p, PathBuf::from("/custom/skills"));
        std::env::remove_var("COSH_TEST_EXPAND");
    }

    #[test]
    fn expand_path_bare_dollar_var() {
        std::env::set_var("COSH_TEST_BARE", "/bare");
        let p = expand_path("$COSH_TEST_BARE/dir").unwrap();
        assert_eq!(p, PathBuf::from("/bare/dir"));
        std::env::remove_var("COSH_TEST_BARE");
    }

    #[test]
    fn expand_path_empty_returns_none() {
        assert!(expand_path("").is_none());
        assert!(expand_path("  ").is_none());
    }
}
