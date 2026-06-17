use serde::Deserialize;

use crate::config::HookDefinition;

/// Extension configuration parsed from `cosh-extension.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct ExtensionConfig {
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    /// Skill directories relative to the extension root.
    /// Accepts a single string or an array of strings.
    #[serde(default)]
    pub skills: SkillsDirs,
    /// Hook definitions grouped by event name.
    #[serde(default)]
    pub hooks: ExtensionHooks,
}

fn default_version() -> String {
    "0.0.0".to_string()
}

// ─── SkillsDirs: string | string[] with default "skills" ─────────────────

/// Wrapper for flexible JSON deserialization: accepts `"skills"` or `["skills", "more"]`.
#[derive(Debug, Clone)]
pub struct SkillsDirs(pub Vec<String>);

impl Default for SkillsDirs {
    fn default() -> Self {
        Self(vec!["skills".to_string()])
    }
}

impl<'de> Deserialize<'de> for SkillsDirs {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de;

        struct SkillsDirsVisitor;

        impl<'de> de::Visitor<'de> for SkillsDirsVisitor {
            type Value = SkillsDirs;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a string or array of strings")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(SkillsDirs(vec![value.to_string()]))
            }

            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut dirs = Vec::new();
                while let Some(s) = seq.next_element::<String>()? {
                    dirs.push(s);
                }
                Ok(SkillsDirs(dirs))
            }
        }

        deserializer.deserialize_any(SkillsDirsVisitor)
    }
}

// ─── ExtensionHooks ──────────────────────────────────────────────────────

/// Hook definitions declared by an extension, grouped by event type.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ExtensionHooks {
    #[serde(default, rename = "PreToolUse")]
    pub pre_tool_use: Vec<HookDefinition>,
    #[serde(default, rename = "PostToolUse")]
    pub post_tool_use: Vec<HookDefinition>,
    #[serde(default, rename = "UserPromptSubmit")]
    pub user_prompt_submit: Vec<HookDefinition>,
    #[serde(default, rename = "SessionStart")]
    pub session_start: Vec<HookDefinition>,
    #[serde(default, rename = "Stop")]
    pub stop: Vec<HookDefinition>,
}

impl ExtensionHooks {
    /// Returns true if no hooks are defined for any event.
    pub fn is_empty(&self) -> bool {
        self.pre_tool_use.is_empty()
            && self.post_tool_use.is_empty()
            && self.user_prompt_submit.is_empty()
            && self.session_start.is_empty()
            && self.stop.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_config() {
        let json = r#"{"name": "test-ext", "version": "1.0.0"}"#;
        let config: ExtensionConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.name, "test-ext");
        assert_eq!(config.version, "1.0.0");
        assert_eq!(config.skills.0, vec!["skills"]);
        assert!(config.hooks.is_empty());
    }

    #[test]
    fn test_parse_skills_as_string() {
        let json = r#"{"name": "ext", "skills": "my-skills"}"#;
        let config: ExtensionConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.skills.0, vec!["my-skills"]);
    }

    #[test]
    fn test_parse_skills_as_array() {
        let json = r#"{"name": "ext", "skills": ["s1", "s2"]}"#;
        let config: ExtensionConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.skills.0, vec!["s1", "s2"]);
    }

    #[test]
    fn test_parse_hooks() {
        let json = r#"{
            "name": "ext",
            "hooks": {
                "PreToolUse": [{"command": "echo pre", "name": "pre-hook"}],
                "SessionStart": [{"command": "echo start"}]
            }
        }"#;
        let config: ExtensionConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.hooks.pre_tool_use.len(), 1);
        assert_eq!(config.hooks.pre_tool_use[0].command, "echo pre");
        assert_eq!(config.hooks.session_start.len(), 1);
        assert!(config.hooks.post_tool_use.is_empty());
    }
}
