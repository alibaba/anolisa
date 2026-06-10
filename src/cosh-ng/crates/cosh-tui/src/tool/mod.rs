pub mod edit;
pub mod grep;
pub mod read_file;
pub mod shell;
pub mod skill;
pub mod todo;
pub mod write_file;

use std::collections::HashMap;
use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::Value;

use crate::provider::ToolDeclaration;

#[derive(Debug, Clone, PartialEq)]
pub enum ToolKind {
    ReadOnly,
    FileEdit,
    ShellExec,
    Other,
}

pub struct ToolContext {
    pub cwd: PathBuf,
    pub session_id: String,
}

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub output: String,
    pub is_error: bool,
}

impl ToolResult {
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            is_error: false,
        }
    }

    pub fn error(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            is_error: true,
        }
    }
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> Value;
    fn kind(&self) -> ToolKind;
    async fn invoke(&self, params: Value, ctx: &ToolContext) -> Result<ToolResult, String>;
}

pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.tools.keys().cloned().collect();
        names.sort();
        names
    }

    pub fn with_defaults() -> Self {
        let mut registry = Self::new();
        registry.register(Box::new(shell::ShellTool));
        registry.register(Box::new(read_file::ReadFileTool));
        registry.register(Box::new(write_file::WriteFileTool));
        registry.register(Box::new(edit::EditTool));
        registry.register(Box::new(grep::GrepTool));
        registry.register(Box::new(todo::TodoTool::new()));
        registry.register(Box::new(skill::SkillTool::new()));
        registry
    }

    pub fn declarations(&self) -> Vec<ToolDeclaration> {
        let mut decls: Vec<_> = self
            .tools
            .values()
            .map(|t| ToolDeclaration {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters: t.parameters_schema(),
            })
            .collect();
        decls.push(ToolDeclaration {
            name: "ask_user_question".to_string(),
            description: "Ask the user a question. Use this when you need clarification or want the user to choose between options.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "question": {
                        "type": "string",
                        "description": "The question to ask the user"
                    },
                    "options": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "label": { "type": "string" },
                                "description": { "type": "string" }
                            },
                            "required": ["label"]
                        },
                        "description": "Options for the user to choose from"
                    },
                    "allow_free_text": {
                        "type": "boolean",
                        "description": "Whether to allow free-text input (default: true)"
                    },
                    "multi_select": {
                        "type": "boolean",
                        "description": "Whether to allow selecting multiple options (default: false)"
                    }
                },
                "required": ["question"]
            }),
        });
        decls.sort_by(|a, b| a.name.cmp(&b.name));
        decls
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyTool;

    #[async_trait]
    impl Tool for DummyTool {
        fn name(&self) -> &str {
            "dummy"
        }
        fn description(&self) -> &str {
            "A dummy tool for testing"
        }
        fn parameters_schema(&self) -> Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "input": { "type": "string" }
                },
                "required": ["input"]
            })
        }
        fn kind(&self) -> ToolKind {
            ToolKind::ReadOnly
        }
        async fn invoke(&self, params: Value, _ctx: &ToolContext) -> Result<ToolResult, String> {
            let input = params
                .get("input")
                .and_then(|v| v.as_str())
                .unwrap_or("none");
            Ok(ToolResult::success(format!("echo: {input}")))
        }
    }

    #[test]
    fn registry_register_and_get() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(DummyTool));
        assert!(registry.get("dummy").is_some());
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn registry_names() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(DummyTool));
        let names = registry.names();
        assert_eq!(names, vec!["dummy"]);
    }

    #[test]
    fn registry_declarations() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(DummyTool));
        let decls = registry.declarations();
        assert_eq!(decls.len(), 2);
        assert!(decls.iter().any(|d| d.name == "dummy"));
        assert!(decls.iter().any(|d| d.name == "ask_user_question"));
    }

    #[tokio::test]
    async fn tool_invoke() {
        let tool = DummyTool;
        let ctx = ToolContext {
            cwd: PathBuf::from("/tmp"),
            session_id: "test".to_string(),
        };
        let result = tool
            .invoke(serde_json::json!({"input": "hello"}), &ctx)
            .await
            .unwrap();
        assert_eq!(result.output, "echo: hello");
        assert!(!result.is_error);
    }

    #[test]
    fn tool_result_constructors() {
        let ok = ToolResult::success("done");
        assert!(!ok.is_error);
        assert_eq!(ok.output, "done");

        let err = ToolResult::error("failed");
        assert!(err.is_error);
        assert_eq!(err.output, "failed");
    }
}
