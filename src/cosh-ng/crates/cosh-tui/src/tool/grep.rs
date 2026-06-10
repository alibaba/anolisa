use async_trait::async_trait;
use serde_json::Value;
use tokio::process::Command;

use super::{Tool, ToolContext, ToolKind, ToolResult};

pub struct GrepTool;

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search for a pattern in files. Uses ripgrep (rg) if available, otherwise falls back to grep."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The regex pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "Directory or file to search in (default: cwd)"
                },
                "include": {
                    "type": "string",
                    "description": "File glob pattern to include (e.g., '*.rs')"
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
            .and_then(|v| v.as_str())
            .ok_or("missing 'pattern' parameter")?;

        let search_path = params
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or(".");

        let include = params.get("include").and_then(|v| v.as_str());

        let has_rg = which::which("rg").is_ok();

        let output = if has_rg {
            let mut cmd = Command::new("rg");
            cmd.arg("--line-number")
                .arg("--no-heading")
                .arg("--color=never")
                .arg("--max-count=100");

            if let Some(glob) = include {
                cmd.arg("--glob").arg(glob);
            }

            cmd.arg(pattern).arg(search_path).current_dir(&ctx.cwd);

            cmd.output().await
        } else {
            let mut cmd = Command::new("grep");
            cmd.arg("-rn").arg("--color=never");

            if let Some(glob) = include {
                cmd.arg("--include").arg(glob);
            }

            cmd.arg(pattern).arg(search_path).current_dir(&ctx.cwd);

            cmd.output().await
        };

        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let stderr = String::from_utf8_lossy(&out.stderr);

                if stdout.is_empty() && out.status.code() == Some(1) {
                    return Ok(ToolResult::success("No matches found."));
                }

                let mut result = stdout.to_string();
                if !stderr.is_empty() {
                    result.push_str("\n[stderr]\n");
                    result.push_str(&stderr);
                }

                Ok(ToolResult::success(result))
            }
            Err(e) => Err(format!("Failed to run grep: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn test_ctx_in(dir: &std::path::Path) -> ToolContext {
        ToolContext {
            cwd: dir.to_path_buf(),
            session_id: "test".to_string(),
        }
    }

    #[tokio::test]
    async fn grep_finds_pattern() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "hello world\ngoodbye world\n").unwrap();

        let tool = GrepTool;
        let result = tool
            .invoke(
                serde_json::json!({"pattern": "hello", "path": "."}),
                &test_ctx_in(dir.path()),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.output.contains("hello"));
    }

    #[tokio::test]
    async fn grep_no_match() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "hello world\n").unwrap();

        let tool = GrepTool;
        let result = tool
            .invoke(
                serde_json::json!({"pattern": "nonexistent_xyz", "path": "."}),
                &test_ctx_in(dir.path()),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.output.contains("No matches"));
    }

    #[tokio::test]
    async fn grep_with_include() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.rs"), "fn main() {}\n").unwrap();
        std::fs::write(dir.path().join("test.txt"), "fn main() {}\n").unwrap();

        let tool = GrepTool;
        let result = tool
            .invoke(
                serde_json::json!({"pattern": "fn main", "path": ".", "include": "*.rs"}),
                &test_ctx_in(dir.path()),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.output.contains("test.rs"));
    }
}
