use async_trait::async_trait;
use serde_json::Value;
use tokio::io::AsyncReadExt;

use super::{Tool, ToolContext, ToolKind, ToolResult};

const MAX_FILE_BYTES: usize = 10 * 1024 * 1024;

pub struct ReadFileTool {
    terminal_output_guidance: &'static str,
}

impl ReadFileTool {
    pub fn new() -> Self {
        Self {
            terminal_output_guidance:
                "terminal-output:// refs are not files. Use fenced cosh-request output fallback in cosh-shell.",
        }
    }

    pub fn with_shell_evidence_tool_guidance() -> Self {
        Self {
            terminal_output_guidance:
                "terminal-output:// refs are not files. Use cosh_shell_evidence action=read_output with output_id.",
        }
    }
}

impl Default for ReadFileTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read the contents of a file. Returns the file content with line numbers."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to read (absolute or relative to cwd)"
                },
                "offset": {
                    "type": "integer",
                    "description": "Line number to start reading from (0-based, default: 0)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to read (default: 2000)"
                }
            },
            "required": ["path"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::ReadOnly
    }

    async fn invoke(&self, params: Value, ctx: &ToolContext) -> Result<ToolResult, String> {
        let path_str = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or("missing 'path' parameter")?;

        if path_str.starts_with("terminal-output://") {
            return Ok(ToolResult::error(self.terminal_output_guidance));
        }

        let path = resolve_path(path_str, &ctx.cwd);

        if !path.exists() {
            return Ok(ToolResult::error(format!(
                "File not found: {}",
                path.display()
            )));
        }
        if !path.is_file() {
            return Ok(ToolResult::error(format!("Not a file: {}", path.display())));
        }

        let file = tokio::fs::File::open(&path)
            .await
            .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
        let metadata = file
            .metadata()
            .await
            .map_err(|e| format!("Failed to inspect {}: {e}", path.display()))?;
        if metadata.len() > MAX_FILE_BYTES as u64 {
            return Ok(ToolResult::error(format!(
                "File exceeds the {} MiB read limit: {}",
                MAX_FILE_BYTES / (1024 * 1024),
                path.display()
            )));
        }

        // The metadata check avoids reading known-large files, while the
        // bounded reader also protects against files that grow after stat.
        let mut reader = file.take((MAX_FILE_BYTES + 1) as u64);
        let mut bytes = Vec::new();
        reader
            .read_to_end(&mut bytes)
            .await
            .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
        if bytes.len() > MAX_FILE_BYTES {
            return Ok(ToolResult::error(format!(
                "File exceeds the {} MiB read limit: {}",
                MAX_FILE_BYTES / (1024 * 1024),
                path.display()
            )));
        }
        let content = String::from_utf8(bytes).map_err(|error| {
            format!(
                "Failed to read {}: file is not valid UTF-8: {}",
                path.display(),
                error.utf8_error()
            )
        })?;

        let offset = params.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(2000) as usize;

        let requested_end = offset.saturating_add(limit);
        let mut output = String::new();
        let mut total = 0;
        for (index, line) in content.lines().enumerate() {
            total = index + 1;
            if index >= offset && index < requested_end {
                output.push_str(&format!("{}\t{line}\n", index + 1));
            }
        }

        let end = requested_end.min(total);
        if end < total {
            output.push_str(&format!(
                "\n... ({} more lines, {} total)\n",
                total - end,
                total
            ));
        }

        Ok(ToolResult::success(output))
    }
}

// resolve_path is provided by the parent module (super::resolve_path)
// and supports ~ expansion.
use super::resolve_path;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::NamedTempFile;

    fn test_ctx() -> ToolContext {
        ToolContext {
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/tmp")),
            session_id: "test".to_string(),
            project_root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/tmp")),
        }
    }

    #[tokio::test]
    async fn read_existing_file() {
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "line1\nline2\nline3\n").unwrap();

        let tool = ReadFileTool::new();
        let result = tool
            .invoke(
                serde_json::json!({"path": tmp.path().to_str().unwrap()}),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.output.contains("1\tline1"));
        assert!(result.output.contains("2\tline2"));
        assert!(result.output.contains("3\tline3"));
    }

    #[tokio::test]
    async fn read_with_offset_and_limit() {
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "a\nb\nc\nd\ne\n").unwrap();

        let tool = ReadFileTool::new();
        let result = tool
            .invoke(
                serde_json::json!({"path": tmp.path().to_str().unwrap(), "offset": 1, "limit": 2}),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.output.contains("2\tb"));
        assert!(result.output.contains("3\tc"));
        assert!(!result.output.contains("1\ta"));
    }

    #[tokio::test]
    async fn rejects_file_larger_than_limit() {
        let tmp = NamedTempFile::new().unwrap();
        tmp.as_file().set_len((MAX_FILE_BYTES + 1) as u64).unwrap();

        let tool = ReadFileTool::new();
        let result = tool
            .invoke(
                serde_json::json!({"path": tmp.path().to_str().unwrap()}),
                &test_ctx(),
            )
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(result.output.contains("10 MiB read limit"));
    }

    #[tokio::test]
    async fn read_nonexistent_file() {
        let tool = ReadFileTool::new();
        let result = tool
            .invoke(
                serde_json::json!({"path": "/tmp/definitely_not_a_real_file_xyz"}),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.output.contains("not found"));
    }

    #[tokio::test]
    async fn read_terminal_output_ref_fails_closed() {
        let tool = ReadFileTool::with_shell_evidence_tool_guidance();
        let result = tool
            .invoke(
                serde_json::json!({"path": "terminal-output://raw-session/cmd-1"}),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result
            .output
            .contains("terminal-output:// refs are not files"));
        assert!(result
            .output
            .contains("cosh_shell_evidence action=read_output"));
        assert!(!result.output.contains("fenced cosh-request"));
    }

    #[tokio::test]
    async fn read_terminal_output_ref_defaults_to_fenced_fallback_guidance() {
        let tool = ReadFileTool::new();
        let result = tool
            .invoke(
                serde_json::json!({"path": "terminal-output://raw-session/cmd-1"}),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.output.contains("fenced cosh-request"));
        assert!(!result.output.contains("cosh_shell_evidence"));
    }
}
