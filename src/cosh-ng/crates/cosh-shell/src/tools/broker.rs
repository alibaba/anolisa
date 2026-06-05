use std::sync::{OnceLock, RwLock};

use super::readonly_rules::{self, RuntimeReadonlyConfig};
use crate::CoshConfig;

#[cfg(test)]
use std::process::{Command, Stdio};
#[cfg(test)]
use std::time::{Duration, Instant};

#[cfg(test)]
const AUTO_TOOL_TIMEOUT: Duration = Duration::from_secs(3);
#[cfg(test)]
const OUTPUT_LIMIT_BYTES: usize = 8 * 1024;
static READONLY_CONFIG: OnceLock<RwLock<RuntimeReadonlyConfig>> = OnceLock::new();

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
enum ToolExecutionStatus {
    Executed,
    Blocked,
    TimedOut,
    Failed,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct ToolExecutionResult {
    status: ToolExecutionStatus,
    command: String,
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
    reason: String,
}

#[cfg(test)]
fn run_approved_bash_tool(command: &str) -> ToolExecutionResult {
    let command = command.trim();
    let tokens = match readonly_tokens(command) {
        Ok(tokens) => tokens,
        Err(reason) => return blocked_result(command, reason),
    };

    run_tokenized_tool(
        command,
        &tokens,
        "approved read-only tool",
        AUTO_TOOL_TIMEOUT,
    )
}

#[cfg(test)]
fn run_user_approved_bash_tool(command: &str) -> ToolExecutionResult {
    let command = command.trim();
    if let Err(reason) = user_approved_shell_command(command) {
        return blocked_result(command, reason);
    }

    run_shell_tool(
        command,
        "user-approved Bash tool",
        user_approved_tool_timeout(),
    )
}

pub fn apply_readonly_config(config: &CoshConfig) {
    set_readonly_config(config.readonly_config().clone());
}

pub(crate) fn set_readonly_config(config: RuntimeReadonlyConfig) {
    let lock = READONLY_CONFIG.get_or_init(|| RwLock::new(RuntimeReadonlyConfig::default()));
    if let Ok(mut guard) = lock.write() {
        *guard = config;
    }
}

#[cfg(test)]
fn run_shell_tool(command: &str, label: &str, timeout: Option<Duration>) -> ToolExecutionResult {
    let child = match Command::new("bash")
        .args(["-lc", command])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(err) => {
            return ToolExecutionResult {
                status: ToolExecutionStatus::Failed,
                command: command.to_string(),
                exit_code: None,
                stdout: String::new(),
                stderr: String::new(),
                reason: format!("failed to start {label}: {err}"),
            };
        }
    };

    wait_for_tool(command, child, label, timeout, "executed through bash -lc")
}

#[cfg(test)]
fn run_tokenized_tool(
    command: &str,
    tokens: &[String],
    label: &str,
    timeout: Duration,
) -> ToolExecutionResult {
    let child = match Command::new(&tokens[0])
        .args(&tokens[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(err) => {
            return ToolExecutionResult {
                status: ToolExecutionStatus::Failed,
                command: command.to_string(),
                exit_code: None,
                stdout: String::new(),
                stderr: String::new(),
                reason: format!("failed to start {label}: {err}"),
            };
        }
    };

    wait_for_tool(
        command,
        child,
        label,
        Some(timeout),
        "executed directly without a shell",
    )
}

#[cfg(test)]
fn wait_for_tool(
    command: &str,
    mut child: std::process::Child,
    label: &str,
    timeout: Option<Duration>,
    success_detail: &str,
) -> ToolExecutionResult {
    let deadline = timeout.map(|timeout| Instant::now() + timeout);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if deadline.is_some_and(|deadline| Instant::now() >= deadline) => {
                let _ = child.kill();
                let output = child.wait_with_output();
                let (stdout, stderr) = output
                    .map(|output| {
                        (
                            decode_limited(&output.stdout),
                            decode_limited(&output.stderr),
                        )
                    })
                    .unwrap_or_else(|err| (String::new(), err.to_string()));
                return ToolExecutionResult {
                    status: ToolExecutionStatus::TimedOut,
                    command: command.to_string(),
                    exit_code: None,
                    stdout,
                    stderr,
                    reason: format!("{label} timed out"),
                };
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(err) => {
                return ToolExecutionResult {
                    status: ToolExecutionStatus::Failed,
                    command: command.to_string(),
                    exit_code: None,
                    stdout: String::new(),
                    stderr: String::new(),
                    reason: format!("failed while waiting for {label}: {err}"),
                };
            }
        }
    }

    match child.wait_with_output() {
        Ok(output) => ToolExecutionResult {
            status: ToolExecutionStatus::Executed,
            command: command.to_string(),
            exit_code: output.status.code(),
            stdout: decode_limited(&output.stdout),
            stderr: decode_limited(&output.stderr),
            reason: format!("{label} {success_detail}"),
        },
        Err(err) => ToolExecutionResult {
            status: ToolExecutionStatus::Failed,
            command: command.to_string(),
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            reason: format!("failed to collect {label} output: {err}"),
        },
    }
}

pub fn can_run_approved_bash_tool(command: &str) -> Result<(), String> {
    readonly_tokens(command).map(|_| ())
}

#[cfg(test)]
fn user_approved_shell_command(command: &str) -> Result<(), String> {
    if command.is_empty() {
        return Err("empty tool command".to_string());
    }
    if command.contains('\0') {
        return Err("blocked NUL byte in shell command".to_string());
    }

    Ok(())
}

#[cfg(test)]
fn user_approved_tool_timeout() -> Option<Duration> {
    parse_user_approved_tool_timeout(
        std::env::var("COSH_SHELL_USER_TOOL_TIMEOUT_SECS")
            .ok()
            .as_deref(),
    )
}

#[cfg(test)]
fn parse_user_approved_tool_timeout(value: Option<&str>) -> Option<Duration> {
    value
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .map(Duration::from_secs)
}

fn readonly_tokens(command: &str) -> Result<Vec<String>, String> {
    let tokens = direct_exec_tokens(command)?;

    if configured_readonly_command(&tokens) {
        Ok(tokens)
    } else {
        Err("command is not in the read-only tool allowlist".to_string())
    }
}

fn configured_readonly_command(tokens: &[String]) -> bool {
    if let Some(lock) = READONLY_CONFIG.get() {
        return match lock.read() {
            Ok(config) => readonly_rules::is_readonly_command_with_config(tokens, &config),
            Err(_) => false,
        };
    }

    readonly_rules::is_readonly_command(tokens)
}

fn direct_exec_tokens(command: &str) -> Result<Vec<String>, String> {
    if command.is_empty() {
        return Err("empty tool command".to_string());
    }
    if command.contains('\0') {
        return Err("blocked NUL byte in tool command".to_string());
    }
    if command.chars().any(is_shell_meta) {
        return Err("blocked shell metacharacter; tool broker does not use a shell".to_string());
    }

    let tokens = command
        .split_ascii_whitespace()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return Err("empty tool command".to_string());
    }

    Ok(tokens)
}

fn is_shell_meta(ch: char) -> bool {
    matches!(
        ch,
        ';' | '|'
            | '&'
            | '>'
            | '<'
            | '$'
            | '`'
            | '('
            | ')'
            | '{'
            | '}'
            | '\''
            | '"'
            | '\\'
            | '\n'
            | '\r'
    )
}

#[cfg(test)]
fn blocked_result(command: &str, reason: String) -> ToolExecutionResult {
    ToolExecutionResult {
        status: ToolExecutionStatus::Blocked,
        command: command.to_string(),
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        reason,
    }
}

#[cfg(test)]
fn decode_limited(bytes: &[u8]) -> String {
    let mut text = String::from_utf8_lossy(bytes).to_string();
    if text.len() <= OUTPUT_LIMIT_BYTES {
        return text;
    }
    text.truncate(OUTPUT_LIMIT_BYTES);
    while !text.is_char_boundary(text.len()) {
        text.pop();
    }
    text.push_str("\n[truncated]");
    text
}

#[cfg(test)]
mod tests {
    use super::readonly_rules::{
        PathMode, RuntimeGenericSpec, RuntimeReadonlyConfig, RuntimeReadonlySpec, RuntimeValidator,
    };

    use super::{
        parse_user_approved_tool_timeout, readonly_tokens, run_approved_bash_tool,
        run_user_approved_bash_tool, set_readonly_config, ToolExecutionStatus,
    };

    #[test]
    fn readonly_broker_allows_simple_git_status() {
        let result = run_approved_bash_tool("git status --short");

        assert_eq!(result.status, ToolExecutionStatus::Executed);
        assert_eq!(result.exit_code, Some(0));
    }

    #[test]
    fn readonly_broker_blocks_shell_metas_and_mutation() {
        let piped = run_approved_bash_tool("ps aux | head");
        let mutation = run_approved_bash_tool("touch /tmp/cosh-shell-broker-should-not-run");

        assert_eq!(piped.status, ToolExecutionStatus::Blocked);
        assert!(piped.reason.contains("metacharacter"));
        assert_eq!(mutation.status, ToolExecutionStatus::Blocked);
        assert!(mutation.reason.contains("allowlist"));
    }

    #[test]
    fn readonly_broker_uses_configured_rules_after_hard_gate() {
        set_readonly_config(RuntimeReadonlyConfig {
            overrides: vec![RuntimeReadonlySpec {
                command: "cosh_readonly_echo".to_string(),
                validator: RuntimeValidator::Generic(RuntimeGenericSpec {
                    short_flags: String::new(),
                    long_flags: Vec::new(),
                    value_flags: Vec::new(),
                    deny_flags: Vec::new(),
                    path_mode: PathMode::Unchecked,
                    bare_number_max: 0,
                }),
            }],
            ..RuntimeReadonlyConfig::default()
        });

        assert!(readonly_tokens("cosh_readonly_echo hello").is_ok());

        let chained = run_approved_bash_tool("cosh_readonly_echo hello && touch /tmp/nope");
        assert_eq!(chained.status, ToolExecutionStatus::Blocked);
        assert!(chained.reason.contains("metacharacter"));
    }

    #[test]
    fn user_approved_broker_runs_non_allowlisted_command_through_shell() {
        let result = run_user_approved_bash_tool("true");

        assert_eq!(result.status, ToolExecutionStatus::Executed);
        assert_eq!(result.exit_code, Some(0));
        assert!(result.reason.contains("bash -lc"));
    }

    #[test]
    fn user_approved_broker_allows_shell_syntax_after_confirmation() {
        for command in [
            "printf 'alpha\\nbeta\\n' | grep beta",
            "echo ok >/dev/null",
            "git status&&pwd",
        ] {
            let result = run_user_approved_bash_tool(command);
            assert_eq!(result.status, ToolExecutionStatus::Executed, "{command}");
        }
    }

    #[test]
    fn user_approved_broker_waits_longer_than_auto_tool_timeout() {
        let result = run_user_approved_bash_tool("sleep 4; printf done");

        assert_eq!(result.status, ToolExecutionStatus::Executed);
        assert_eq!(result.stdout, "done");
    }

    #[test]
    fn user_approved_broker_has_no_default_timeout() {
        assert_eq!(parse_user_approved_tool_timeout(None), None);
        assert_eq!(
            parse_user_approved_tool_timeout(Some("2")),
            Some(std::time::Duration::from_secs(2))
        );
        assert_eq!(parse_user_approved_tool_timeout(Some("0")), None);
        assert_eq!(parse_user_approved_tool_timeout(Some("invalid")), None);
    }

    #[test]
    fn user_approved_broker_rejects_empty_or_nul_command() {
        for command in ["", "printf ok\0printf bad"] {
            let result = run_user_approved_bash_tool(command);
            assert_eq!(result.status, ToolExecutionStatus::Blocked, "{command:?}");
        }
    }

    #[test]
    fn readonly_broker_tokenizes_tabs_but_rejects_newlines_and_unspaced_metas() {
        let tabbed = run_approved_bash_tool("git\tstatus\t--short");
        let newline = run_approved_bash_tool("git status\npwd");
        let chained = run_approved_bash_tool("git status&&pwd");
        let redirected = run_approved_bash_tool("git status>/tmp/cosh-shell-broker-should-not-run");

        assert_eq!(tabbed.status, ToolExecutionStatus::Executed);
        assert_eq!(newline.status, ToolExecutionStatus::Blocked);
        assert!(newline.reason.contains("metacharacter"));
        assert_eq!(chained.status, ToolExecutionStatus::Blocked);
        assert!(chained.reason.contains("metacharacter"));
        assert_eq!(redirected.status, ToolExecutionStatus::Blocked);
        assert!(redirected.reason.contains("metacharacter"));
    }

    #[test]
    fn readonly_broker_allows_bounded_cpu_diagnostics() {
        for command in [
            "top -l 1 -n 15 -s 0",
            "top -l 1 -o cpu -n 20",
            "top -b -n 1 -o %CPU",
            "top -n 1 -b -o %CPU",
            "ps -Ao pid,pcpu,pmem,comm -r",
            "sysctl -n hw.ncpu",
            "sysctl -n machdep.cpu.brand_string",
        ] {
            assert!(readonly_tokens(command).is_ok(), "{command}");
        }
    }

    #[test]
    fn readonly_broker_allows_disk_usage_diagnostics() {
        for command in ["df", "df -h", "df -hi", "df -h ."] {
            assert!(readonly_tokens(command).is_ok(), "{command}");
        }
    }

    #[test]
    fn readonly_broker_rejects_unbounded_or_chained_cpu_diagnostics() {
        for command in [
            "top",
            "top -l 2 -n 15",
            "top -l 1 -n 1000",
            "top -l 1 -n 15 | head -30",
            "sysctl -a",
            "sysctl -w hw.ncpu=1",
            "sysctl -n hw.ncpu$(echo x)",
            "sysctl -n machdep.cpu.brand_string && echo ok",
        ] {
            let result = run_approved_bash_tool(command);
            assert_eq!(result.status, ToolExecutionStatus::Blocked, "{command}");
        }
    }

    #[test]
    fn readonly_broker_rejects_risky_per_command_arguments() {
        for command in [
            "git -c core.pager=cat status",
            "git diff --ext-diff",
            "git show --textconv HEAD:README.md",
            "git diff --output=/tmp/cosh-shell-git-diff.txt",
            "ps -o command=",
            "find . -exec echo {} ;",
            "find . -delete",
            "find /proc -name cpuinfo",
            "find . -maxdepth 100 -name Cargo.toml",
            "cat /dev/zero",
            "cat /proc/cpuinfo",
            "cat \"中文.md\"",
            "head -n 100000 README.md",
            "tail -f README.md",
            "grep -R cosh .",
            "grep cosh /proc/cpuinfo",
            "rg --pre cat cosh .",
            "rg --pre=cat cosh .",
            "rg -n cosh /dev",
            "ls /dev/zero",
            "df --output=source",
            "df /dev/zero",
        ] {
            let result = run_approved_bash_tool(command);
            assert_eq!(result.status, ToolExecutionStatus::Blocked, "{command}");
        }
    }

    #[test]
    fn readonly_broker_allows_safe_per_command_arguments() {
        for command in [
            "git status --short",
            "git diff --stat",
            "git diff --name-only --",
            "git log --oneline -n 5",
            "git show --stat HEAD",
            "ps -Ao pid,pcpu,pmem,comm -r",
            "ls -la .",
            "cat README.md",
            "cat 中文.md",
            "head -n 20 README.md",
            "head -20 README.md",
            "tail -n 20 README.md",
            "grep -n cosh README.md",
            "grep -e cosh README.md",
            "rg -n cosh crates/cosh-shell",
            "rg --files crates/cosh-shell",
            "find . -maxdepth 2 -type f -name Cargo.toml -print",
            "df -h .",
            "uname -a",
            "id -u",
        ] {
            assert!(readonly_tokens(command).is_ok(), "{command}");
        }
    }
}
