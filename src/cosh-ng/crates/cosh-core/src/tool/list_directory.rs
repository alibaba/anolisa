//! Non-recursive directory listing.

use async_trait::async_trait;
use serde_json::Value;

use super::file_patterns::resolve_path;
use super::{Tool, ToolContext, ToolKind, ToolResult};

const MAX_ENTRIES: usize = 200;

pub(super) struct ListDirectoryTool;

#[async_trait]
impl Tool for ListDirectoryTool {
    fn name(&self) -> &str {
        "list_directory"
    }

    fn description(&self) -> &str {
        "List files and subdirectories directly inside a directory."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Directory to list (default: cwd)"
                }
            }
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::ReadOnly
    }

    async fn invoke(&self, params: Value, ctx: &ToolContext) -> Result<ToolResult, String> {
        let path = resolve_path(
            params.get("path").and_then(Value::as_str).unwrap_or("."),
            &ctx.cwd,
        );
        if !path.is_dir() {
            return Ok(ToolResult::error(format!(
                "Path is not a directory: {}",
                path.display()
            )));
        }

        let mut reader = tokio::fs::read_dir(&path)
            .await
            .map_err(|error| format!("Failed to list {}: {error}", path.display()))?;
        let mut entries = Vec::new();
        while let Some(entry) = reader
            .next_entry()
            .await
            .map_err(|error| format!("Failed to list {}: {error}", path.display()))?
        {
            let file_type = entry.file_type().await.map_err(|error| {
                format!("Failed to inspect {}: {error}", entry.path().display())
            })?;
            let suffix = if file_type.is_dir() { "/" } else { "" };
            entries.push(format!("{}{}", entry.file_name().to_string_lossy(), suffix));
        }
        entries.sort_unstable();

        let total = entries.len();
        entries.truncate(MAX_ENTRIES);
        let mut output = if entries.is_empty() {
            format!("Directory is empty: {}", path.display())
        } else {
            entries.join("\n")
        };
        if total > MAX_ENTRIES {
            output.push_str(&format!("\n\n... {} entries omitted", total - MAX_ENTRIES));
        }
        Ok(ToolResult::success(output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn lists_directories_with_suffix() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        let ctx = ToolContext {
            cwd: dir.path().to_path_buf(),
            session_id: "test".to_string(),
            project_root: dir.path().to_path_buf(),
        };

        let result = ListDirectoryTool
            .invoke(serde_json::json!({}), &ctx)
            .await
            .unwrap();

        assert_eq!(result.output, "Cargo.toml\nsrc/");
    }
}
