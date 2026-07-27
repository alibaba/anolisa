//! `skill-discover` virtual skill content generation.
//!
//! These methods produce the synthesized `SKILL.md` body served at
//! `/skills/skill-discover/SKILL.md`. The body lists secondary views
//! (when `skillfs-views.toml` is present) or falls back to a flat
//! listing of every skill in the store.

use super::SkillFs;
use crate::path::find_common_path_prefix;

impl SkillFs {
    /// Generate SKILL.md content for the virtual `skill-discover` skill.
    ///
    /// When views are configured, the body lists every secondary view as a
    /// section with a table of `name | description | source_path` rows.
    /// The `source_path` is the real physical path to each skill's SKILL.md,
    /// enabling the AI to open it directly via `read_file`.
    ///
    /// When no views config is present, falls back to a simple listing of all
    /// skills in the store.
    pub(super) fn get_skill_discover_content(&self) -> String {
        let store = self.store.read();

        // ── Case 1: views config present ─────────────────────────────────
        if let Some(cfg) = &self.views_config {
            let secondary_views = cfg.secondary_views();
            if secondary_views.is_empty() {
                return self.simple_discover_md(&store);
            }

            // Collect all skill names in secondary views (for frontmatter description).
            let hidden_names: Vec<&str> = secondary_views
                .iter()
                .flat_map(|v| v.skills.iter().map(|s| s.as_str()))
                .filter(|name| store.get(name).is_some())
                .collect();

            // Collect all source paths to find a common prefix.
            let all_paths: Vec<std::path::PathBuf> = hidden_names
                .iter()
                .filter_map(|name| store.get(name).map(|e| e.source_path.clone()))
                .collect();
            let common_prefix = find_common_path_prefix(&all_paths);

            let frontmatter = format!(
                "---\nname: skill-discover\ndescription: 'Hidden skills: {}'\nversion: 0.1.0\ntags: [meta, discovery]\nenabled: true\n---\n",
                hidden_names.join(", ")
            );

            let mut body = String::from("\n# Secondary Skill Views\n\n");

            // Show base path hint once so individual paths stay short.
            if let Some(ref prefix) = common_prefix {
                body.push_str(&format!(
                    "Base path: `{}`\n\nPaths below are relative to the base path. \
Use `read_file` on any `source_path` to read the skill and learn how to use it.\n\n",
                    prefix.display()
                ));
            } else {
                body.push_str("Use `read_file` on any `source_path` to read the skill and learn how to use it.\n\n");
            }

            for view in &secondary_views {
                body.push_str(&format!("## {}\n", view.name));
                if !view.description.is_empty() {
                    body.push_str(&format!("{}\n\n", view.description));
                } else {
                    body.push('\n');
                }
                body.push_str("| name | description | source_path |\n");
                body.push_str("|------|-------------|-------------|\n");

                for skill_name in &view.skills {
                    if let Some(entry) = store.get(skill_name.as_str()) {
                        let desc = entry
                            .metadata
                            .description
                            .lines()
                            .next()
                            .unwrap_or("")
                            .trim()
                            .replace('|', r"\|");
                        let display_path = match &common_prefix {
                            Some(prefix) => entry
                                .source_path
                                .strip_prefix(prefix)
                                .map(|p| p.display().to_string())
                                .unwrap_or_else(|_| entry.source_path.display().to_string()),
                            None => entry.source_path.display().to_string(),
                        };
                        body.push_str(&format!(
                            "| {} | {} | {} |\n",
                            skill_name, desc, display_path
                        ));
                    }
                }
                body.push('\n');
            }

            return format!("{}{}", frontmatter, body);
        }

        // ── Case 2: no views config — simple listing ──────────────────────
        self.simple_discover_md(&store)
    }

    /// Fallback skill-discover content when no views config is present.
    fn simple_discover_md(&self, store: &skillfs_core::store::SkillStore) -> String {
        let mut body = String::from(
            "| name | description |
|------|-------------|
",
        );
        let mut names: Vec<&str> = store.list();
        names.sort_unstable();
        for name in names {
            if let Some(entry) = store.get(name) {
                let desc = entry
                    .metadata
                    .description
                    .lines()
                    .next()
                    .unwrap_or("")
                    .trim()
                    .replace('|', r"\|");
                body.push_str(&format!("| {} | {} |\n", name, desc));
            }
        }
        format!(
            "---
name: skill-discover
description: Lists all available skills.
version: 0.1.0
tags: [meta, discovery]
enabled: true
---

# Available Skills

{}
",
            body
        )
    }
}
