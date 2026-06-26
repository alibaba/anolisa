use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use super::config::{flatten_hook_groups, ExtensionConfig, ExtensionHooks};
use super::variables::{hydrate_config, VariableContext};
use super::{
    Extension, InstallMetadata, EXTENSION_CONFIG_FILENAME, INSTALL_METADATA_FILENAME,
};

/// Central manager for discovering, loading, and querying extensions.
pub struct ExtensionManager {
    extensions: Vec<Extension>,
    workspace_dir: PathBuf,
    /// Test-only override for user extensions directory.
    user_dir_override: Option<PathBuf>,
    /// Test-only override for system extensions directory.
    system_dir_override: Option<PathBuf>,
}

impl ExtensionManager {
    /// Create a new ExtensionManager.
    ///
    /// * `workspace_dir` – current project / workspace root directory (used for
    ///   `${workspacePath}` variable substitution).
    pub fn new(workspace_dir: PathBuf) -> Self {
        Self {
            extensions: Vec::new(),
            workspace_dir,
            user_dir_override: None,
            system_dir_override: None,
        }
    }

    /// Test constructor that overrides user and system directories.
    #[cfg(test)]
    pub fn new_isolated(
        workspace_dir: PathBuf,
        user_dir: Option<PathBuf>,
        system_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            extensions: Vec::new(),
            workspace_dir,
            user_dir_override: user_dir,
            system_dir_override: system_dir,
        }
    }

    /// Scan system and user extension directories and load all valid extensions.
    /// User-level extensions override system-level extensions with the same name.
    pub fn refresh(&mut self) {
        let mut extensions_map: HashMap<String, Extension> = HashMap::new();

        // 1. Scan system-level directory (lower priority)
        let system_dir = self
            .system_dir_override
            .clone()
            .unwrap_or_else(super::system_extensions_dir);
        self.scan_directory(&system_dir, &mut extensions_map);

        // 2. Scan user-level directory (higher priority, overrides system)
        let user_dir = self
            .user_dir_override
            .clone()
            .or_else(super::user_extensions_dir);
        if let Some(user_dir) = user_dir {
            self.scan_directory(&user_dir, &mut extensions_map);
        }

        // Collect into sorted vec
        let mut exts: Vec<Extension> = extensions_map.into_values().collect();
        exts.sort_by(|a, b| a.name.cmp(&b.name));

        // Apply disable state from ~/.copilot-shell/states/extensions.json
        let disabled = crate::state::load_disabled(crate::state::EXTENSIONS_STATE);
        for ext in &mut exts {
            if disabled.contains(&ext.name) {
                ext.is_active = false;
            }
        }

        self.extensions = exts;
    }

    /// Return all active extensions' skill directory absolute paths.
    /// Each extension may declare multiple skill directories.
    pub fn skill_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        for ext in &self.extensions {
            if !ext.is_active {
                continue;
            }
            for skill_dir in &ext.config.skills.0 {
                let path = if std::path::Path::new(skill_dir).is_absolute() {
                    PathBuf::from(skill_dir)
                } else {
                    ext.path.join(skill_dir)
                };
                dirs.push(path);
            }
        }
        dirs
    }

    /// Collect all active extensions' hook definitions into a merged ExtensionHooks.
    pub fn hook_definitions(&self) -> ExtensionHooks {
        let mut merged = ExtensionHooks::default();
        for ext in &self.extensions {
            if !ext.is_active {
                continue;
            }
            merged.merge(&ext.config.hooks);
        }
        merged
    }

    /// Return the list of loaded extensions.
    pub fn list(&self) -> &[Extension] {
        &self.extensions
    }

    /// Get all hook names defined by a specific extension.
    /// Used for cleanup when enabling an extension (remove its hooks from hooks.json disabled).
    pub fn extension_hook_names(&self, ext_name: &str) -> HashSet<String> {
        let mut names = HashSet::new();
        let Some(ext) = self.extensions.iter().find(|e| e.name == ext_name) else {
            return names;
        };
        let all_groups = [
            &ext.config.hooks.pre_tool_use,
            &ext.config.hooks.post_tool_use,
            &ext.config.hooks.post_tool_use_failure,
            &ext.config.hooks.user_prompt_submit,
            &ext.config.hooks.session_start,
            &ext.config.hooks.stop,
            &ext.config.hooks.before_model,
            &ext.config.hooks.after_model,
        ];
        for groups in all_groups {
            for def in flatten_hook_groups(groups) {
                if let Some(name) = def.name {
                    names.insert(name);
                }
            }
        }
        names
    }

    // ─── Private helpers ─────────────────────────────────────────────

    fn scan_directory(
        &self,
        dir: &PathBuf,
        map: &mut HashMap<String, Extension>,
    ) {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => return, // Directory doesn't exist or not readable
        };

        for entry in entries.flatten() {
            let entry_path = entry.path();
            if !entry_path.is_dir() {
                continue;
            }

            // Resolve symlinks for the actual path
            let resolved_path = entry_path
                .canonicalize()
                .unwrap_or_else(|_| entry_path.clone());

            let config_file = resolved_path.join(EXTENSION_CONFIG_FILENAME);
            if !config_file.exists() {
                continue;
            }

            match self.load_extension(&resolved_path) {
                Some(ext) => {
                    map.insert(ext.name.clone(), ext);
                }
                None => {
                    tracing::warn!(
                        target: "extension",
                        "Failed to load extension from: {}",
                        resolved_path.display()
                    );
                }
            }
        }
    }

    fn load_extension(&self, ext_dir: &PathBuf) -> Option<Extension> {
        let config_path = ext_dir.join(EXTENSION_CONFIG_FILENAME);
        let config_content = match std::fs::read_to_string(&config_path) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    target: "extension",
                    "Failed to read {}: {e}",
                    config_path.display()
                );
                return None;
            }
        };
        let mut config: ExtensionConfig = serde_json::from_str(&config_content)
            .map_err(|e| {
                tracing::warn!(
                    target: "extension",
                    "Failed to parse {}: {e}",
                    config_path.display()
                );
            })
            .ok()?;

        // Apply variable substitution
        let ctx = VariableContext {
            extension_path: ext_dir,
            workspace_path: &self.workspace_dir,
        };
        hydrate_config(&mut config, &ctx);

        // Load optional install metadata
        let metadata_path = ext_dir.join(INSTALL_METADATA_FILENAME);
        let install_metadata = if metadata_path.exists() {
            match std::fs::read_to_string(&metadata_path) {
                Ok(s) => match serde_json::from_str::<InstallMetadata>(&s) {
                    Ok(m) => Some(m),
                    Err(e) => {
                        tracing::warn!(
                            target: "extension",
                            "Failed to parse {}: {e}",
                            metadata_path.display()
                        );
                        None
                    }
                },
                Err(e) => {
                    tracing::warn!(
                        target: "extension",
                        "Failed to read {}: {e}",
                        metadata_path.display()
                    );
                    None
                }
            }
        } else {
            None
        };

        Some(Extension {
            name: config.name.clone(),
            version: config.version.clone(),
            path: ext_dir.clone(),
            is_active: true,
            config,
            install_metadata,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn create_extension_dir(base: &std::path::Path, name: &str, config_json: &str) -> PathBuf {
        let ext_dir = base.join(name);
        fs::create_dir_all(&ext_dir).unwrap();
        fs::write(ext_dir.join(EXTENSION_CONFIG_FILENAME), config_json).unwrap();
        ext_dir
    }

    #[test]
    fn test_load_extension_from_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let user_dir = tmp.path().join("user-ext");
        fs::create_dir_all(&user_dir).unwrap();

        create_extension_dir(
            &user_dir,
            "my-ext",
            r#"{"name": "my-ext", "version": "1.0.0"}"#,
        );

        let mut mgr = ExtensionManager::new_isolated(
            PathBuf::from("/workspace"),
            Some(user_dir),
            None,
        );
        mgr.refresh();

        assert_eq!(mgr.list().len(), 1);
        assert_eq!(mgr.list()[0].name, "my-ext");
        assert_eq!(mgr.list()[0].version, "1.0.0");
        assert!(mgr.list()[0].is_active);
    }

    #[test]
    fn test_skill_dirs_resolution() {
        let tmp = tempfile::tempdir().unwrap();
        let user_dir = tmp.path().join("user-ext");
        fs::create_dir_all(&user_dir).unwrap();

        create_extension_dir(
            &user_dir,
            "ext-a",
            r#"{"name": "ext-a", "skills": ["${extensionPath}/my-skills", "extra"]}"#,
        );

        let mut mgr = ExtensionManager::new_isolated(
            PathBuf::from("/workspace"),
            Some(user_dir.clone()),
            None,
        );
        mgr.refresh();

        let dirs = mgr.skill_dirs();
        assert_eq!(dirs.len(), 2);
        // First should be absolute (variable substituted)
        let ext_path = user_dir.join("ext-a").canonicalize().unwrap();
        assert_eq!(dirs[0], ext_path.join("my-skills"));
        // Second is relative, joined with extension path
        assert_eq!(dirs[1], ext_path.join("extra"));
    }

    #[test]
    fn test_hook_definitions_collection() {
        let tmp = tempfile::tempdir().unwrap();
        let user_dir = tmp.path().join("user-ext");
        fs::create_dir_all(&user_dir).unwrap();

        create_extension_dir(
            &user_dir,
            "hook-ext",
            r#"{
                "name": "hook-ext",
                "hooks": {
                    "PreToolUse": [
                        {"hooks": [{"type": "command", "command": "echo pre", "name": "h1"}]}
                    ],
                    "Stop": [
                        {"hooks": [{"type": "command", "command": "echo stop"}]}
                    ]
                }
            }"#,
        );

        let mut mgr = ExtensionManager::new_isolated(
            PathBuf::from("/ws"),
            Some(user_dir),
            None,
        );
        mgr.refresh();

        let hooks = mgr.hook_definitions();
        assert_eq!(hooks.pre_tool_use.len(), 1);
        assert_eq!(hooks.pre_tool_use[0].hooks[0].command, "echo pre");
        assert_eq!(hooks.stop.len(), 1);
        assert!(hooks.post_tool_use.is_empty());
    }

    #[test]
    fn test_user_overrides_system() {
        let tmp = tempfile::tempdir().unwrap();
        let sys_dir = tmp.path().join("system");
        let user_dir = tmp.path().join("user");
        fs::create_dir_all(&sys_dir).unwrap();
        fs::create_dir_all(&user_dir).unwrap();

        create_extension_dir(
            &sys_dir,
            "shared-ext",
            r#"{"name": "shared-ext", "version": "1.0.0"}"#,
        );
        create_extension_dir(
            &user_dir,
            "shared-ext",
            r#"{"name": "shared-ext", "version": "2.0.0"}"#,
        );

        let mut mgr = ExtensionManager::new_isolated(
            PathBuf::from("/ws"),
            Some(user_dir),
            Some(sys_dir),
        );
        mgr.refresh();

        assert_eq!(mgr.list().len(), 1);
        assert_eq!(mgr.list()[0].version, "2.0.0"); // user wins
    }

    #[test]
    fn test_variable_substitution() {
        let tmp = tempfile::tempdir().unwrap();
        let user_dir = tmp.path().join("user-ext");
        fs::create_dir_all(&user_dir).unwrap();

        create_extension_dir(
            &user_dir,
            "var-ext",
            r#"{
                "name": "var-ext",
                "hooks": {
                    "PreToolUse": [
                        {"hooks": [{"type": "command", "command": "${extensionPath}/run.sh --ws=${workspacePath}"}]}
                    ]
                }
            }"#,
        );

        let mut mgr = ExtensionManager::new_isolated(
            PathBuf::from("/my/workspace"),
            Some(user_dir.clone()),
            None,
        );
        mgr.refresh();

        let hooks = mgr.hook_definitions();
        let ext_path = user_dir.join("var-ext").canonicalize().unwrap();
        let expected = format!("{}/run.sh --ws=/my/workspace", ext_path.display());
        assert_eq!(hooks.pre_tool_use[0].hooks[0].command, expected);
    }

    #[test]
    fn test_missing_config_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let user_dir = tmp.path().join("user-ext");
        fs::create_dir_all(&user_dir).unwrap();

        // Create dir WITHOUT cosh-extension.json
        fs::create_dir_all(user_dir.join("no-config")).unwrap();
        // Create one WITH config
        create_extension_dir(
            &user_dir,
            "valid-ext",
            r#"{"name": "valid-ext"}"#,
        );

        let mut mgr = ExtensionManager::new_isolated(
            PathBuf::from("/ws"),
            Some(user_dir),
            None,
        );
        mgr.refresh();

        assert_eq!(mgr.list().len(), 1);
        assert_eq!(mgr.list()[0].name, "valid-ext");
    }

    #[test]
    fn test_install_metadata_loaded() {
        let tmp = tempfile::tempdir().unwrap();
        let user_dir = tmp.path().join("user-ext");
        fs::create_dir_all(&user_dir).unwrap();

        let ext_dir = create_extension_dir(
            &user_dir,
            "meta-ext",
            r#"{"name": "meta-ext", "version": "1.0.0"}"#,
        );
        fs::write(
            ext_dir.join(INSTALL_METADATA_FILENAME),
            r#"{"source": "/tmp/source", "type": "local", "installed_at": "2025-06-17T00:00:00Z"}"#,
        )
        .unwrap();

        let mut mgr = ExtensionManager::new_isolated(
            PathBuf::from("/ws"),
            Some(user_dir),
            None,
        );
        mgr.refresh();

        let ext = &mgr.list()[0];
        assert!(ext.install_metadata.is_some());
        let meta = ext.install_metadata.as_ref().unwrap();
        assert_eq!(meta.source, "/tmp/source");
        assert_eq!(meta.install_type, "local");
        assert_eq!(meta.installed_at, "2025-06-17T00:00:00Z");
    }
}
