use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

use wait_timeout::ChildExt;

use crate::hooks::model::HookInput;
use crate::types::CommandBlock;
use crate::types::HookFinding;

use super::ExternalHookConfig;

pub(super) fn hook_input_from_block(block: &CommandBlock) -> HookInput {
    let output_preview = block
        .output
        .terminal_output_ref
        .as_deref()
        .and_then(|path| read_preview(path, 50))
        .unwrap_or_default();
    HookInput {
        command: block.command.clone(),
        cwd: block.cwd.clone(),
        exit_code: block.exit_code,
        duration_ms: block.duration_ms,
        output_ref: block.output.terminal_output_ref.clone(),
        output_bytes: block.output.terminal_output_bytes,
        output_preview,
    }
}

fn read_preview(path: &str, max_lines: usize) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    let preview: String = content
        .lines()
        .take(max_lines)
        .collect::<Vec<_>>()
        .join("\n");
    if preview.is_empty() {
        None
    } else {
        Some(preview)
    }
}

pub(super) fn run_external_hook(
    config: &ExternalHookConfig,
    input: &HookInput,
) -> Option<HookFinding> {
    let input_json = serde_json::to_string(input).ok()?;

    let mut child = Command::new(&config.path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| {
            tracing::error!(
                target: "cosh_hook",
                path = %config.path.display(),
                "external hook spawn failed: {e}"
            );
        })
        .ok()?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input_json.as_bytes());
        // drop stdin so the child sees EOF
    }

    let clamped_ms = config.timeout_ms.min(10_000);
    let timeout = Duration::from_millis(clamped_ms);
    match child.wait_timeout(timeout) {
        Ok(Some(status)) if status.success() => {}
        Ok(Some(_)) => {
            tracing::warn!(
                target: "cosh_hook",
                path = %config.path.display(),
                "external hook exited with error"
            );
            return None;
        }
        Ok(None) => {
            let _ = child.kill();
            let _ = child.wait();
            tracing::warn!(
                target: "cosh_hook",
                path = %config.path.display(),
                timeout_ms = config.timeout_ms,
                "external hook timed out"
            );
            return None;
        }
        Err(e) => {
            tracing::warn!(
                target: "cosh_hook",
                path = %config.path.display(),
                "external hook wait failed: {e}"
            );
            return None;
        }
    }

    const MAX_HOOK_OUTPUT: usize = 8192;
    let mut stdout_buf = vec![0u8; MAX_HOOK_OUTPUT];
    let mut total_read = 0;
    if let Some(mut stdout) = child.stdout.take() {
        use std::io::Read;
        loop {
            let remaining = MAX_HOOK_OUTPUT - total_read;
            if remaining == 0 {
                break;
            }
            match stdout.read(&mut stdout_buf[total_read..]) {
                Ok(0) => break,
                Ok(n) => total_read += n,
                Err(_) => break,
            }
        }
    }
    stdout_buf.truncate(total_read);
    let stdout = String::from_utf8_lossy(&stdout_buf);
    if stdout.trim().is_empty() {
        return None;
    }
    serde_json::from_str::<HookFinding>(stdout.trim())
        .map_err(|e| {
            tracing::warn!(
                target: "cosh_hook",
                path = %config.path.display(),
                "external hook invalid JSON output: {e}"
            );
        })
        .ok()
}
