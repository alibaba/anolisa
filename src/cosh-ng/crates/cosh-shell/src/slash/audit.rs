//! Thin bounded `/audit` facade over `cosh-cli audit`.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use wait_timeout::ChildExt;

use crate::runtime::state::InlineState;

const AUDIT_CLI_TIMEOUT: Duration = Duration::from_secs(3);
const AUDIT_CLI_MAX_OUTPUT: usize = 256 * 1024;

pub(super) fn render_audit_command<W: Write>(
    arguments: &str,
    state: &InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let result =
        resolve_arguments(arguments, state).and_then(|args| run_audit_cli(&audit_program(), &args));
    match result {
        Ok(data) => {
            writeln!(output, "\r\nAudit")?;
            writeln!(output, "{}", safe_render_data(&data))?;
        }
        Err(error) => {
            let safe = crate::evidence::redact_sensitive_text(&error).0;
            writeln!(output, "\r\nAudit unavailable: {safe}")?;
            writeln!(
                output,
                "Audit export is a redacted incident bundle; diagnostics export is a separate Shell diagnostics bundle."
            )?;
        }
    }
    Ok(())
}

fn resolve_arguments(arguments: &str, state: &InlineState) -> Result<Vec<String>, String> {
    let parts = arguments.split_whitespace().collect::<Vec<_>>();
    match parts.as_slice() {
        [] | ["status"] => Ok(vec!["audit".to_string(), "status".to_string()]),
        ["trace", "current"] => {
            let session = state
                .shell_session_id
                .as_ref()
                .ok_or_else(|| "current Shell session is unavailable".to_string())?;
            Ok(vec![
                "audit".to_string(),
                "trace".to_string(),
                session.clone(),
            ])
        }
        ["export", "current", destination] => {
            let session = state
                .shell_session_id
                .as_ref()
                .ok_or_else(|| "current Shell session is unavailable".to_string())?;
            Ok(vec![
                "audit".to_string(),
                "export".to_string(),
                "--output".to_string(),
                (*destination).to_string(),
                "--identity".to_string(),
                session.clone(),
            ])
        }
        ["export", "current"] => Err("usage: /audit export current <dir>".to_string()),
        _ => Err(
            "usage: /audit status | /audit trace current | /audit export current <dir>".to_string(),
        ),
    }
}

fn audit_program() -> String {
    std::env::var("COSH_CLI_BIN").unwrap_or_else(|_| "cosh-cli".to_string())
}

fn run_audit_cli(program: &str, arguments: &[String]) -> Result<serde_json::Value, String> {
    run_audit_cli_with_timeout(program, arguments, AUDIT_CLI_TIMEOUT)
}

fn run_audit_cli_with_timeout(
    program: &str,
    arguments: &[String],
    timeout: Duration,
) -> Result<serde_json::Value, String> {
    let deadline = std::time::Instant::now() + timeout;
    let mut command = Command::new(program);
    command
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot start cosh-cli: {error}"))?;
    let process_group = child.id();
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| "cosh-cli stdout is unavailable".to_string())?;
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout
            .by_ref()
            .take((AUDIT_CLI_MAX_OUTPUT + 1) as u64)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = sender.send(result);
    });
    let status = match child
        .wait_timeout(timeout)
        .map_err(|error| format!("wait for cosh-cli failed: {error}"))?
    {
        Some(status) => status,
        None => {
            kill_audit_process_group(process_group);
            let _ = child.wait();
            return Err("cosh-cli audit timed out".to_string());
        }
    };
    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
    let bytes = receiver
        .recv_timeout(remaining)
        .map_err(|_| {
            kill_audit_process_group(process_group);
            "cosh-cli audit output did not close before timeout".to_string()
        })?
        .map_err(|error| format!("read cosh-cli output failed: {error}"))?;
    if bytes.len() > AUDIT_CLI_MAX_OUTPUT {
        return Err("cosh-cli audit output exceeds limit".to_string());
    }
    let envelope: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|_| "cosh-cli returned malformed JSON".to_string())?;
    if !status.success() || envelope.get("ok").and_then(|value| value.as_bool()) != Some(true) {
        return Err("cosh-cli audit query failed".to_string());
    }
    envelope
        .get("data")
        .cloned()
        .ok_or_else(|| "cosh-cli response has no data".to_string())
}

#[cfg(unix)]
fn kill_audit_process_group(process_group: u32) {
    use nix::sys::signal::{killpg, Signal};
    use nix::unistd::Pid;

    let _ = killpg(Pid::from_raw(process_group as i32), Signal::SIGKILL);
}

#[cfg(not(unix))]
fn kill_audit_process_group(_process_group: u32) {}

fn safe_render_data(data: &serde_json::Value) -> String {
    let rendered = serde_json::to_string_pretty(data).unwrap_or_else(|_| "{}".to_string());
    crate::evidence::redact_sensitive_text(&rendered).0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_trace_uses_stable_shell_session_id() {
        let state = InlineState {
            shell_session_id: Some("shell-session-1".to_string()),
            ..InlineState::default()
        };
        assert_eq!(
            resolve_arguments("trace current", &state).unwrap(),
            ["audit", "trace", "shell-session-1"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn bounded_cli_accepts_success_and_rejects_malformed_output() {
        let root = std::env::temp_dir().join(format!(
            "cosh-shell-audit-cli-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        let ok = root.join("ok.sh");
        let malformed = root.join("malformed.sh");
        let failed = root.join("failed.sh");
        let oversized = root.join("oversized.sh");
        let slow = root.join("slow.sh");
        let inherited_stdout = root.join("inherited-stdout.sh");
        std::fs::write(
            &ok,
            "#!/bin/sh\nprintf '%s' '{\"ok\":true,\"data\":{\"mode\":\"best_effort\"}}'\n",
        )
        .unwrap();
        std::fs::write(&malformed, "#!/bin/sh\nprintf '%s' 'not-json'\n").unwrap();
        std::fs::write(&failed, "#!/bin/sh\nprintf '%s' '{\"ok\":false}'\nexit 1\n").unwrap();
        std::fs::write(
            &oversized,
            "#!/bin/sh\ndd if=/dev/zero bs=262145 count=1 2>/dev/null\n",
        )
        .unwrap();
        std::fs::write(&slow, "#!/bin/sh\nsleep 1\n").unwrap();
        std::fs::write(
            &inherited_stdout,
            "#!/bin/sh\n(sleep 2) &\nprintf '%s' '{\"ok\":true,\"data\":{}}'\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&ok, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::set_permissions(&malformed, std::fs::Permissions::from_mode(0o700)).unwrap();
        for path in [&failed, &oversized, &slow, &inherited_stdout] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        assert_eq!(
            run_audit_cli(ok.to_str().unwrap(), &[]).unwrap()["mode"],
            "best_effort"
        );
        assert!(run_audit_cli(malformed.to_str().unwrap(), &[]).is_err());
        assert!(run_audit_cli(failed.to_str().unwrap(), &[]).is_err());
        assert!(run_audit_cli(oversized.to_str().unwrap(), &[]).is_err());
        assert!(
            run_audit_cli_with_timeout(slow.to_str().unwrap(), &[], Duration::from_millis(20))
                .is_err()
        );
        let started = std::time::Instant::now();
        assert!(run_audit_cli_with_timeout(
            inherited_stdout.to_str().unwrap(),
            &[],
            Duration::from_millis(50)
        )
        .is_err());
        assert!(started.elapsed() < Duration::from_millis(500));
        assert!(run_audit_cli("/definitely/missing/cosh-cli", &[]).is_err());
        let _ = std::fs::remove_dir_all(root);
    }
}
