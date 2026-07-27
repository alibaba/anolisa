//! Glob-based file discovery.

use async_trait::async_trait;
use serde_json::Value;

use super::file_patterns::expand_file_patterns;
use super::{Tool, ToolContext, ToolKind, ToolResult};

const MAX_RESULTS: usize = 100;

pub(super) struct GlobTool;

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> &str {
        "Find files by glob pattern, such as '**/*.rs'. Returns at most 100 sorted paths."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern to match"
                },
                "path": {
                    "type": "string",
                    "description": "Directory to search from (default: cwd)"
                }
            },
            "required": ["pattern"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::ReadOnly
    }

    async fn invoke(&self, params: Value, ctx: &ToolContext) -> Result<ToolResult, String> {
        let pattern = params
            .get("pattern")
            .and_then(Value::as_str)
            .ok_or("missing 'pattern' parameter")?
            .to_string();
        let base = params
            .get("path")
            .and_then(Value::as_str)
            .map(|path| super::file_patterns::resolve_path(path, &ctx.cwd))
            .unwrap_or_else(|| ctx.cwd.clone());

        if !base.is_dir() {
            return Ok(ToolResult::error(format!(
                "Search path is not a directory: {}",
                base.display()
            )));
        }

        let expand_pattern = pattern.clone();
        let expand_base = base.clone();
        let matches = tokio::task::spawn_blocking(move || {
            expand_file_patterns(&[expand_pattern], &expand_base, MAX_RESULTS)
        })
        .await
        .map_err(|error| format!("File discovery task failed: {error}"))??;
        if matches.paths.is_empty() {
            return Ok(ToolResult::success(format!(
                "No files found matching pattern \"{pattern}\" in {}",
                base.display()
            )));
        }

        let mut output = matches
            .paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        if matches.truncated {
            output.push_str("\n\n... results truncated at 100 files");
        }
        Ok(ToolResult::success(output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn finds_matching_files_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("nested")).unwrap();
        std::fs::write(dir.path().join("one.rs"), "one").unwrap();
        std::fs::write(dir.path().join("nested/two.rs"), "two").unwrap();
        std::fs::write(dir.path().join("three.txt"), "three").unwrap();
        let ctx = ToolContext {
            cwd: dir.path().to_path_buf(),
            session_id: "test".to_string(),
            project_root: PathBuf::from(dir.path()),
        };

        let result = GlobTool
            .invoke(serde_json::json!({"pattern": "**/*.rs"}), &ctx)
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.output.contains("one.rs"));
        assert!(result.output.contains("two.rs"));
        assert!(!result.output.contains("three.txt"));
    }
}
