use async_trait::async_trait;
use serde_json::Value;
use tokio::process::Command;

use super::{Tool, ToolContext, ToolKind, ToolResult};

pub struct ShellTool;

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Execute a shell command and return its output. Use this to run commands, scripts, and system utilities."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Optional timeout in milliseconds (default: 30000)"
                }
            },
            "required": ["command"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::ShellExec
    }

    async fn invoke(&self, params: Value, ctx: &ToolContext) -> Result<ToolResult, String> {
        let command = params
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or("missing 'command' parameter")?;

        let timeout_ms = params
            .get("timeout_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(30_000);

        let result = tokio::time::timeout(
            std::time::Duration::from_millis(timeout_ms),
            Command::new("sh")
                .arg("-c")
                .arg(command)
                .current_dir(&ctx.cwd)
                .output(),
        )
        .await;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let exit_code = output.status.code().unwrap_or(-1);

                let mut result_text = String::new();
                if !stdout.is_empty() {
                    result_text.push_str(&stdout);
                }
                if !stderr.is_empty() {
                    if !result_text.is_empty() {
                        result_text.push('\n');
                    }
                    result_text.push_str("[stderr]\n");
                    result_text.push_str(&stderr);
                }
                if result_text.is_empty() {
                    result_text = format!("(exit code: {exit_code})");
                }

                Ok(ToolResult {
                    output: result_text,
                    is_error: !output.status.success(),
                })
            }
            Ok(Err(e)) => Err(format!("Failed to execute command: {e}")),
            Err(_) => Ok(ToolResult::error(format!(
                "Command timed out after {timeout_ms}ms"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_ctx() -> ToolContext {
        ToolContext {
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/tmp")),
            session_id: "test".to_string(),
        }
    }

    #[tokio::test]
    async fn shell_echo() {
        let tool = ShellTool;
        let result = tool
            .invoke(serde_json::json!({"command": "echo hello"}), &test_ctx())
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.output.contains("hello"));
    }

    #[tokio::test]
    async fn shell_exit_code() {
        let tool = ShellTool;
        let result = tool
            .invoke(serde_json::json!({"command": "false"}), &test_ctx())
            .await
            .unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn shell_stderr() {
        let tool = ShellTool;
        let result = tool
            .invoke(
                serde_json::json!({"command": "echo err >&2"}),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(result.output.contains("err"));
        assert!(result.output.contains("[stderr]"));
    }

    #[tokio::test]
    async fn shell_timeout() {
        let tool = ShellTool;
        let result = tool
            .invoke(
                serde_json::json!({"command": "sleep 60", "timeout_ms": 200}),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.output.contains("timed out"));
    }

    #[tokio::test]
    async fn shell_missing_command() {
        let tool = ShellTool;
        let result = tool.invoke(serde_json::json!({}), &test_ctx()).await;
        assert!(result.is_err());
    }
}
