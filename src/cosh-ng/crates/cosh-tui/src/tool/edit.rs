use std::path::Path;

use async_trait::async_trait;
use serde_json::Value;

use super::{Tool, ToolContext, ToolKind, ToolResult};

pub struct EditTool;

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }

    fn description(&self) -> &str {
        "Edit a file by replacing an exact string occurrence with a new string. The old_string must match exactly (including whitespace and indentation)."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to edit"
                },
                "old_string": {
                    "type": "string",
                    "description": "The exact string to find and replace"
                },
                "new_string": {
                    "type": "string",
                    "description": "The string to replace it with"
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "If true, replace all occurrences (default: false)"
                }
            },
            "required": ["path", "old_string", "new_string"]
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
        let old_string = params
            .get("old_string")
            .and_then(|v| v.as_str())
            .ok_or("missing 'old_string' parameter")?;
        let new_string = params
            .get("new_string")
            .and_then(|v| v.as_str())
            .ok_or("missing 'new_string' parameter")?;
        let replace_all = params
            .get("replace_all")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let path = resolve_path(path_str, &ctx.cwd);

        if !path.exists() {
            return Ok(ToolResult::error(format!(
                "File not found: {}",
                path.display()
            )));
        }

        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;

        let count = content.matches(old_string).count();
        if count == 0 {
            return Ok(ToolResult::error(format!(
                "old_string not found in {}",
                path.display()
            )));
        }
        if count > 1 && !replace_all {
            return Ok(ToolResult::error(format!(
                "old_string found {count} times in {}. Use replace_all=true to replace all, or provide more context to make the match unique.",
                path.display()
            )));
        }

        let new_content = if replace_all {
            content.replace(old_string, new_string)
        } else {
            content.replacen(old_string, new_string, 1)
        };

        tokio::fs::write(&path, &new_content)
            .await
            .map_err(|e| format!("Failed to write {}: {e}", path.display()))?;

        Ok(ToolResult::success(format!(
            "Replaced {count} occurrence(s) in {}",
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
    use std::path::PathBuf;
    use tempfile::NamedTempFile;

    fn test_ctx() -> ToolContext {
        ToolContext {
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/tmp")),
            session_id: "test".to_string(),
        }
    }

    #[tokio::test]
    async fn edit_single_replacement() {
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "hello world").unwrap();

        let tool = EditTool;
        let result = tool
            .invoke(
                serde_json::json!({
                    "path": tmp.path().to_str().unwrap(),
                    "old_string": "hello",
                    "new_string": "goodbye"
                }),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(!result.is_error);

        let content = std::fs::read_to_string(tmp.path()).unwrap();
        assert_eq!(content, "goodbye world");
    }

    #[tokio::test]
    async fn edit_not_found() {
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "hello world").unwrap();

        let tool = EditTool;
        let result = tool
            .invoke(
                serde_json::json!({
                    "path": tmp.path().to_str().unwrap(),
                    "old_string": "xyz",
                    "new_string": "abc"
                }),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.output.contains("not found"));
    }

    #[tokio::test]
    async fn edit_ambiguous_without_replace_all() {
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "aaa bbb aaa").unwrap();

        let tool = EditTool;
        let result = tool
            .invoke(
                serde_json::json!({
                    "path": tmp.path().to_str().unwrap(),
                    "old_string": "aaa",
                    "new_string": "ccc"
                }),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.output.contains("2 times"));
    }

    #[tokio::test]
    async fn edit_replace_all() {
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "aaa bbb aaa").unwrap();

        let tool = EditTool;
        let result = tool
            .invoke(
                serde_json::json!({
                    "path": tmp.path().to_str().unwrap(),
                    "old_string": "aaa",
                    "new_string": "ccc",
                    "replace_all": true
                }),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(!result.is_error);

        let content = std::fs::read_to_string(tmp.path()).unwrap();
        assert_eq!(content, "ccc bbb ccc");
    }
}
