//! Tool abstractions for LLM function calling (OpenAI tools API).
//!
//! A `Tool` exposes an executable capability (shell, cosh-cli subsystems, ...)
//! to the LLM. The `ToolRegistry` converts registered tools into the OpenAI
//! `tools` request payload and dispatches `tool_calls` responses back.
//!
//! cosh-tui is the reference agent consumer of cosh-cli: most tools are thin
//! wrappers around `cosh pkg/svc/checkpoint/...` subcommands, which already
//! emit structured `CoshResponse` JSON. A generic `run_shell_command` tool
//! exists as a fallback for anything cosh-cli does not (yet) wrap.

pub mod cosh;
pub mod shell;

use serde_json::Value;

/// Three-state safety classification for a specific tool invocation. Drives
/// approval decisions in `App::process_pending_tools` (audit-design.md §9.3).
///
/// Modes interplay:
/// - `Safe`           — Auto/Yolo run automatically; Ask still confirms.
/// - `NeedsApproval`  — Auto/Ask confirm; Yolo runs (the user explicitly said "less prompts").
/// - `Forbidden`      — every mode (including Yolo) must refuse to auto-run.
///   Yolo represents "stop bothering me" — NOT "I accept any consequence".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafetyClass {
    Safe,
    NeedsApproval,
    Forbidden,
}

/// Trait implemented by every tool the LLM can invoke.
pub trait Tool: Send + Sync {
    /// Canonical tool name (maps to OpenAI function.name).
    fn name(&self) -> &str;

    /// Human + LLM readable description (shown to the model).
    fn description(&self) -> &str;

    /// JSON-schema describing the accepted arguments.
    fn parameters(&self) -> Value;

    /// Synchronously execute the tool with the given JSON args.
    fn execute(&self, args: &Value) -> Result<String, String>;

    /// Whether this invocation is safe (no user approval required under Auto).
    /// Default: unsafe, always ask.
    fn is_safe(&self, _args: &Value) -> bool {
        false
    }

    /// Three-state safety classification used by the approval flow. Default
    /// derives from `is_safe` for backwards compatibility:
    /// - `is_safe == true`  → `Safe`
    /// - `is_safe == false` → `NeedsApproval`
    ///
    /// Tools that can express "absolutely never auto-run" (e.g. shell
    /// commands the audit policy classifies as `Outcome::Deny`) should
    /// override this to return `Forbidden`. The default never returns
    /// `Forbidden`, so existing `cosh_*` tools keep their current Auto/Yolo
    /// behaviour.
    fn safety_class(&self, args: &Value) -> SafetyClass {
        if self.is_safe(args) {
            SafetyClass::Safe
        } else {
            SafetyClass::NeedsApproval
        }
    }

    /// One-line preview used in UI/approval dialog.
    fn preview(&self, args: &Value) -> String {
        format!("{}({})", self.name(), args)
    }
}

/// Holds all tools available to the agentic loop.
pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    /// Build the default registry: cosh-cli wrappers + shell fallback.
    pub fn new() -> Self {
        let mut reg = Self { tools: Vec::new() };
        // cosh-cli subsystem wrappers (structured JSON I/O).
        cosh::register_all(&mut reg);
        // Generic shell fallback for anything cosh doesn't wrap.
        reg.register(Box::new(shell::RunShellCommand));
        reg
    }

    #[cfg(test)]
    pub fn empty() -> Self {
        Self { tools: Vec::new() }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(tool);
    }

    pub fn find(&self, name: &str) -> Option<&dyn Tool> {
        self.tools
            .iter()
            .find(|t| t.name() == name)
            .map(|t| t.as_ref())
    }

    /// Serialize the registry into the OpenAI `tools` request field.
    pub fn to_openai_tools(&self) -> Vec<crate::llm::ToolSpec> {
        self.tools
            .iter()
            .map(|t| crate::llm::ToolSpec {
                tool_type: "function".to_string(),
                function: crate::llm::FunctionSpec {
                    name: t.name().to_string(),
                    description: t.description().to_string(),
                    parameters: t.parameters(),
                },
            })
            .collect()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    #[cfg(test)]
    pub fn list(&self) -> Vec<(&str, &str)> {
        self.tools
            .iter()
            .map(|t| (t.name(), t.description()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn registry_default_includes_shell_and_cosh_tools() {
        let reg = ToolRegistry::new();
        assert!(reg.find("run_shell_command").is_some());
        assert!(reg.find("cosh_svc_status").is_some());
        assert!(reg.find("cosh_svc_list").is_some());
        assert!(reg.find("cosh_pkg_search").is_some());
        assert!(reg.find("cosh_checkpoint_status").is_some());
        assert!(reg.len() > 5);
    }

    #[test]
    fn registry_empty_is_empty() {
        let reg = ToolRegistry::empty();
        assert!(reg.is_empty());
        assert!(reg.find("anything").is_none());
    }

    #[test]
    fn registry_to_openai_tools_shape() {
        let reg = ToolRegistry::new();
        let specs = reg.to_openai_tools();
        assert_eq!(specs.len(), reg.len());
        for spec in &specs {
            assert_eq!(spec.tool_type, "function");
            assert!(!spec.function.name.is_empty());
            assert!(!spec.function.description.is_empty());
            assert!(spec.function.parameters.is_object());
        }
    }

    #[test]
    fn registry_find_missing_returns_none() {
        let reg = ToolRegistry::new();
        assert!(reg.find("nonexistent_tool_xyz").is_none());
    }

    #[test]
    fn custom_tool_can_be_registered() {
        struct Dummy;
        impl Tool for Dummy {
            fn name(&self) -> &str {
                "dummy"
            }
            fn description(&self) -> &str {
                "test"
            }
            fn parameters(&self) -> Value {
                json!({"type":"object"})
            }
            fn execute(&self, _args: &Value) -> Result<String, String> {
                Ok("ok".into())
            }
            fn is_safe(&self, _args: &Value) -> bool {
                true
            }
        }
        let mut reg = ToolRegistry::empty();
        reg.register(Box::new(Dummy));
        let tool = reg.find("dummy").unwrap();
        assert_eq!(tool.name(), "dummy");
        assert!(tool.is_safe(&json!({})));
        assert_eq!(tool.execute(&json!({})).unwrap(), "ok");
    }

    #[test]
    fn registry_list_has_descriptions() {
        let reg = ToolRegistry::new();
        let list = reg.list();
        assert_eq!(list.len(), reg.len());
        for (name, desc) in list {
            assert!(!name.is_empty());
            assert!(!desc.is_empty());
        }
    }
}
