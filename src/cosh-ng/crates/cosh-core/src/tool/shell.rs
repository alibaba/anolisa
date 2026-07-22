use async_trait::async_trait;
use serde_json::Value;
use tokio::process::Command;

use crate::process::{output_with_timeout, OutputError};

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

        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(command).current_dir(&ctx.cwd);

        // Deadline-bounded execution with process-group cleanup: a bare
        // tokio::time::timeout would leak the child's process tree.
        let result =
            output_with_timeout(cmd, None, std::time::Duration::from_millis(timeout_ms)).await;

        match result {
            Ok(output) => {
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
            Err(OutputError::Timeout) => Ok(ToolResult::error(format!(
                "Command timed out after {timeout_ms}ms"
            ))),
            Err(e) => Err(format!("Failed to execute command: {e}")),
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
            project_root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/tmp")),
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
            .invoke(serde_json::json!({"command": "echo err >&2"}), &test_ctx())
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

    #[cfg(unix)]
    #[tokio::test]
    async fn shell_timeout_kills_process_group() {
        use crate::process::test_support::*;

        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("marker");
        let pid_file = dir.path().join("pids");
        let command = leak_script(&marker, &pid_file);

        let tool = ShellTool;
        let started = std::time::Instant::now();
        let result = tool
            .invoke(
                serde_json::json!({"command": command, "timeout_ms": 300}),
                &test_ctx(),
            )
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.output.contains("timed out"));

        let pids = read_pids(&pid_file);
        let _cleanup = PidCleanup(pids.clone());
        for pid in &pids {
            assert_process_gone(*pid);
        }
        wait_past_marker_deadline(started);
        assert!(!marker.exists(), "grandchild survived the tool timeout");
    }

    #[tokio::test]
    async fn shell_missing_command() {
        let tool = ShellTool;
        let result = tool.invoke(serde_json::json!({}), &test_ctx()).await;
        assert!(result.is_err());
    }
}
