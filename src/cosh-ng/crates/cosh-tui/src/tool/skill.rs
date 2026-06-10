use std::collections::HashMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::Value;

use super::{Tool, ToolContext, ToolKind, ToolResult};

#[derive(Debug, Clone)]
pub struct SkillMeta {
    pub name: String,
    pub description: String,
    pub allowed_tools: Vec<String>,
    pub prompt: String,
}

pub struct SkillTool {
    cache: std::sync::Mutex<Option<HashMap<String, SkillMeta>>>,
}

impl SkillTool {
    pub fn new() -> Self {
        Self {
            cache: std::sync::Mutex::new(None),
        }
    }

    fn load_skills(cwd: &Path) -> HashMap<String, SkillMeta> {
        let mut skills = HashMap::new();

        let search_dirs = [
            dirs::home_dir()
                .map(|h| h.join(".copilot-shell/skills")),
            Some(cwd.join(".copilot-shell/skills")),
        ];

        for dir in search_dirs.into_iter().flatten() {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().map_or(false, |ext| ext == "md") {
                        if let Some(skill) = Self::parse_skill_file(&path) {
                            skills.insert(skill.name.clone(), skill);
                        }
                    }
                }
            }
        }

        skills
    }

    fn parse_skill_file(path: &PathBuf) -> Option<SkillMeta> {
        let content = std::fs::read_to_string(path).ok()?;
        let content = content.trim();

        if !content.starts_with("---") {
            return None;
        }

        let end = content[3..].find("---")?;
        let frontmatter = &content[3..3 + end];
        let prompt = content[3 + end + 3..].trim().to_string();

        let mut name = None;
        let mut description = String::new();
        let mut allowed_tools = Vec::new();

        for line in frontmatter.lines() {
            let line = line.trim();
            if let Some(val) = line.strip_prefix("name:") {
                name = Some(val.trim().to_string());
            } else if let Some(val) = line.strip_prefix("description:") {
                description = val.trim().to_string();
            } else if let Some(val) = line.strip_prefix("allowedTools:") {
                let val = val.trim();
                if val.starts_with('[') && val.ends_with(']') {
                    allowed_tools = val[1..val.len() - 1]
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                }
            }
        }

        let name = name.or_else(|| {
            path.file_stem().map(|s| s.to_string_lossy().to_string())
        })?;

        Some(SkillMeta {
            name,
            description,
            allowed_tools,
            prompt,
        })
    }

    fn get_skills(&self, cwd: &Path) -> HashMap<String, SkillMeta> {
        let mut cache = self.cache.lock().unwrap();
        if cache.is_none() {
            *cache = Some(Self::load_skills(cwd));
        }
        cache.clone().unwrap()
    }
}

#[async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &str {
        "skill"
    }

    fn description(&self) -> &str {
        "Load and invoke a skill (SKILL.md) from the skills directory. Use action 'list' to see available skills, or provide a skill name to get its prompt."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["invoke", "list"],
                    "description": "Action to perform: 'invoke' a skill or 'list' available skills"
                },
                "name": {
                    "type": "string",
                    "description": "Name of the skill to invoke (required for 'invoke' action)"
                },
                "args": {
                    "type": "string",
                    "description": "Arguments to pass to the skill"
                }
            },
            "required": ["action"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Other
    }

    async fn invoke(&self, params: Value, ctx: &ToolContext) -> Result<ToolResult, String> {
        let action = params
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("invoke");

        let skills = self.get_skills(&ctx.cwd);

        match action {
            "list" => {
                if skills.is_empty() {
                    return Ok(ToolResult::success("No skills found."));
                }
                let list: Vec<String> = skills
                    .values()
                    .map(|s| format!("- {}: {}", s.name, s.description))
                    .collect();
                Ok(ToolResult::success(list.join("\n")))
            }
            "invoke" | _ => {
                let name = params
                    .get("name")
                    .and_then(|v| v.as_str())
                    .ok_or("missing 'name' parameter for invoke action")?;

                match skills.get(name) {
                    Some(skill) => Ok(ToolResult::success(format!(
                        "[Skill '{}' loaded]\n\n{}",
                        skill.name, skill.prompt
                    ))),
                    None => Ok(ToolResult::error(format!(
                        "Skill '{}' not found. Use action 'list' to see available skills.",
                        name
                    ))),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parse_skill_file() {
        let dir = tempfile::tempdir().unwrap();
        let skill_path = dir.path().join("test-skill.md");
        let mut f = std::fs::File::create(&skill_path).unwrap();
        writeln!(
            f,
            "---\nname: test-skill\ndescription: A test skill\nallowedTools: [shell, read_file]\n---\n\nYou are a test skill."
        )
        .unwrap();

        let skill = SkillTool::parse_skill_file(&skill_path).unwrap();
        assert_eq!(skill.name, "test-skill");
        assert_eq!(skill.description, "A test skill");
        assert_eq!(skill.allowed_tools, vec!["shell", "read_file"]);
        assert!(skill.prompt.contains("You are a test skill"));
    }

    #[test]
    fn parse_skill_file_fallback_name() {
        let dir = tempfile::tempdir().unwrap();
        let skill_path = dir.path().join("my-skill.md");
        let mut f = std::fs::File::create(&skill_path).unwrap();
        writeln!(f, "---\ndescription: No name field\n---\n\nPrompt text.").unwrap();

        let skill = SkillTool::parse_skill_file(&skill_path).unwrap();
        assert_eq!(skill.name, "my-skill");
    }

    #[tokio::test]
    async fn skill_list_empty() {
        let tool = SkillTool::new();
        let ctx = ToolContext {
            cwd: PathBuf::from("/nonexistent"),
            session_id: "test".to_string(),
        };
        let result = tool
            .invoke(serde_json::json!({"action": "list"}), &ctx)
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.output.contains("No skills found"));
    }

    #[tokio::test]
    async fn skill_invoke_not_found() {
        let tool = SkillTool::new();
        let ctx = ToolContext {
            cwd: PathBuf::from("/nonexistent"),
            session_id: "test".to_string(),
        };
        let result = tool
            .invoke(
                serde_json::json!({"action": "invoke", "name": "missing"}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.output.contains("not found"));
    }

    #[tokio::test]
    async fn skill_invoke_found() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join(".copilot-shell/skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        let skill_path = skills_dir.join("demo.md");
        let mut f = std::fs::File::create(&skill_path).unwrap();
        writeln!(
            f,
            "---\nname: demo\ndescription: Demo skill\n---\n\nYou are demo."
        )
        .unwrap();

        let tool = SkillTool::new();
        let ctx = ToolContext {
            cwd: dir.path().to_path_buf(),
            session_id: "test".to_string(),
        };
        let result = tool
            .invoke(serde_json::json!({"action": "invoke", "name": "demo"}), &ctx)
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.output.contains("You are demo"));
    }
}
