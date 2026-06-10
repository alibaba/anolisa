use std::path::Path;

use async_trait::async_trait;
use serde_json::Value;

use super::{Tool, ToolContext, ToolKind, ToolResult};

pub struct WriteFileTool;

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Write content to a file, creating it if it doesn't exist or overwriting if it does."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to write (absolute or relative to cwd)"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file"
                }
            },
            "required": ["path", "content"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::FileEdit
    }

    async fn invoke(&self, params: Value, ctx: &ToolContext) -> Result<ToolResult, String> {
        let path_str = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or("missing 'path' parameter")?;
        let content = params
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or("missing 'content' parameter")?;

        let path = resolve_path(path_str, &ctx.cwd);

        if let Some(parent) = path.parent() {
            if !parent.exists() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| format!("Failed to create directory {}: {e}", parent.display()))?;
            }
        }

        tokio::fs::write(&path, content)
            .await
            .map_err(|e| format!("Failed to write {}: {e}", path.display()))?;

        let lines = content.lines().count();
        let bytes = content.len();
        Ok(ToolResult::success(format!(
            "Wrote {bytes} bytes ({lines} lines) to {}",
            path.display()
        )))
    }
}

fn resolve_path(path_str: &str, cwd: &Path) -> std::path::PathBuf {
    let p = std::path::Path::new(path_str);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn test_ctx_in(dir: &Path) -> ToolContext {
        ToolContext {
            cwd: dir.to_path_buf(),
            session_id: "test".to_string(),
        }
    }

    #[tokio::test]
    async fn write_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let tool = WriteFileTool;
        let path = dir.path().join("test.txt");

        let result = tool
            .invoke(
                serde_json::json!({"path": path.to_str().unwrap(), "content": "hello world"}),
                &test_ctx_in(dir.path()),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.output.contains("11 bytes"));

        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "hello world");
    }

    #[tokio::test]
    async fn write_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let tool = WriteFileTool;
        let path = dir.path().join("sub/dir/test.txt");

        let result = tool
            .invoke(
                serde_json::json!({"path": path.to_str().unwrap(), "content": "nested"}),
                &test_ctx_in(dir.path()),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(path.exists());
    }

    #[tokio::test]
    async fn write_relative_path() {
        let dir = tempfile::tempdir().unwrap();
        let tool = WriteFileTool;

        let result = tool
            .invoke(
                serde_json::json!({"path": "relative.txt", "content": "rel"}),
                &test_ctx_in(dir.path()),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(dir.path().join("relative.txt").exists());
    }
}
