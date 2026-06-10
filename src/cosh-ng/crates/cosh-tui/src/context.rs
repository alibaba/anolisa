use std::path::Path;

pub struct ContextBuilder;

impl ContextBuilder {
    pub fn build_system_prompt(
        cwd: &Path,
        tool_names: &[String],
        approval_mode: &str,
        output_language: Option<&str>,
    ) -> String {
        let mut parts = Vec::new();

        parts.push(format!(
            "# Environment\n- OS: {}\n- Shell: {}\n- CWD: {}",
            std::env::consts::OS,
            std::env::var("SHELL").unwrap_or_else(|_| "unknown".to_string()),
            cwd.display(),
        ));

        if let Some(ctx) = Self::load_project_context(cwd) {
            parts.push(format!("# Project Context\n{ctx}"));
        }

        parts.push(format!(
            "# Approval Mode\nCurrent mode: `{approval_mode}`"
        ));

        if !tool_names.is_empty() {
            parts.push(format!(
                "# Available Tools\n{}",
                tool_names.join(", ")
            ));
        }

        if let Some(lang) = output_language {
            parts.push(format!(
                "# Output Language\nRespond in {lang}."
            ));
        }

        parts.join("\n\n")
    }

    fn load_project_context(cwd: &Path) -> Option<String> {
        let path = cwd.join(".copilot-shell/CONTEXT.md");
        std::fs::read_to_string(&path).ok().filter(|s| !s.trim().is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn basic_system_prompt() {
        let cwd = PathBuf::from("/tmp/test-project");
        let tools = vec!["shell".to_string(), "read_file".to_string()];
        let prompt = ContextBuilder::build_system_prompt(&cwd, &tools, "balanced", None);

        assert!(prompt.contains("/tmp/test-project"));
        assert!(prompt.contains("shell, read_file"));
        assert!(prompt.contains("balanced"));
    }

    #[test]
    fn prompt_with_language() {
        let cwd = PathBuf::from("/tmp");
        let prompt =
            ContextBuilder::build_system_prompt(&cwd, &[], "trust", Some("Chinese"));

        assert!(prompt.contains("Respond in Chinese"));
    }

    #[test]
    fn prompt_without_project_context() {
        let cwd = PathBuf::from("/nonexistent/path");
        let prompt = ContextBuilder::build_system_prompt(&cwd, &[], "auto", None);

        assert!(!prompt.contains("Project Context"));
    }
}
