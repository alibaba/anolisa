use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const TOOL_TIMEOUT: Duration = Duration::from_secs(3);
const OUTPUT_LIMIT_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolExecutionStatus {
    Executed,
    Blocked,
    TimedOut,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolExecutionResult {
    pub status: ToolExecutionStatus,
    pub command: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub reason: String,
}

impl ToolExecutionStatus {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Executed => "executed",
            Self::Blocked => "blocked",
            Self::TimedOut => "timed_out",
            Self::Failed => "failed",
        }
    }
}

pub fn run_approved_bash_tool(command: &str) -> ToolExecutionResult {
    let command = command.trim();
    let tokens = match readonly_tokens(command) {
        Ok(tokens) => tokens,
        Err(reason) => return blocked_result(command, reason),
    };

    let mut child = match Command::new(&tokens[0])
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
                reason: format!("failed to start approved read-only tool: {err}"),
            };
        }
    };

    let deadline = Instant::now() + TOOL_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() >= deadline => {
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
                    reason: "approved read-only tool timed out".to_string(),
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
                    reason: format!("failed while waiting for approved read-only tool: {err}"),
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
            reason: "approved read-only tool executed by broker".to_string(),
        },
        Err(err) => ToolExecutionResult {
            status: ToolExecutionStatus::Failed,
            command: command.to_string(),
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            reason: format!("failed to collect approved read-only tool output: {err}"),
        },
    }
}

pub fn can_run_approved_bash_tool(command: &str) -> Result<(), String> {
    readonly_tokens(command).map(|_| ())
}

fn readonly_tokens(command: &str) -> Result<Vec<String>, String> {
    if command.is_empty() {
        return Err("empty tool command".to_string());
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

    if is_readonly_command(&tokens) {
        Ok(tokens)
    } else {
        Err("command is not in the read-only tool allowlist".to_string())
    }
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

fn is_readonly_command(tokens: &[String]) -> bool {
    match tokens.first().map(String::as_str) {
        Some("pwd" | "whoami" | "hostname" | "date" | "uptime") => tokens.len() == 1,
        Some("id") => is_readonly_id(tokens),
        Some("uname") => is_readonly_uname(tokens),
        Some("ls") => is_readonly_ls(tokens),
        Some("cat") => is_readonly_cat(tokens),
        Some("head") => is_readonly_head_tail(tokens, false),
        Some("tail") => is_readonly_head_tail(tokens, true),
        Some("grep") => is_readonly_grep(tokens),
        Some("rg") => is_readonly_rg(tokens),
        Some("find") => is_readonly_find(tokens),
        Some("vm_stat") => tokens.len() == 1,
        Some("sysctl") => is_readonly_sysctl(tokens),
        Some("top") => is_bounded_top_snapshot(tokens),
        Some("ps") => is_readonly_ps(tokens),
        Some("git") => is_readonly_git(tokens),
        _ => false,
    }
}

fn is_readonly_id(tokens: &[String]) -> bool {
    tokens
        .iter()
        .skip(1)
        .all(|token| matches!(token.as_str(), "-u" | "-g" | "-G" | "-n" | "-r"))
}

fn is_readonly_uname(tokens: &[String]) -> bool {
    tokens.iter().skip(1).all(|token| {
        token.starts_with('-')
            && token
                .chars()
                .skip(1)
                .all(|ch| matches!(ch, 'a' | 'm' | 'n' | 'p' | 'r' | 's' | 'v'))
    })
}

fn is_readonly_ls(tokens: &[String]) -> bool {
    tokens.iter().skip(1).all(|token| {
        if token.starts_with('-') {
            token.chars().skip(1).all(|ch| {
                matches!(
                    ch,
                    '1' | 'A'
                        | 'F'
                        | 'G'
                        | 'H'
                        | 'L'
                        | 'R'
                        | 'S'
                        | 'a'
                        | 'd'
                        | 'h'
                        | 'l'
                        | 'r'
                        | 't'
                )
            })
        } else {
            !is_blocked_special_path(token)
        }
    })
}

fn is_readonly_cat(tokens: &[String]) -> bool {
    let mut saw_path = false;
    let mut paths_only = false;

    for token in tokens.iter().skip(1) {
        if paths_only {
            if !is_safe_readonly_path(token) {
                return false;
            }
            saw_path = true;
            continue;
        }

        match token.as_str() {
            "--" => paths_only = true,
            "-n" | "-b" | "-s" => {}
            _ if token.starts_with('-') => return false,
            _ => {
                if !is_safe_readonly_path(token) {
                    return false;
                }
                saw_path = true;
            }
        }
    }

    saw_path
}

fn is_readonly_head_tail(tokens: &[String], is_tail: bool) -> bool {
    let mut idx = 1;
    let mut saw_path = false;
    let mut paths_only = false;

    while idx < tokens.len() {
        let token = tokens[idx].as_str();
        if paths_only {
            if !is_safe_readonly_path(token) {
                return false;
            }
            saw_path = true;
            idx += 1;
            continue;
        }

        match token {
            "--" => {
                paths_only = true;
                idx += 1;
            }
            "-q" | "-v" => idx += 1,
            "-n" | "-c" => {
                let Some(value) = tokens.get(idx + 1) else {
                    return false;
                };
                if !is_bounded_positive_count(value, 10_000) {
                    return false;
                }
                idx += 2;
            }
            "-f" if is_tail => return false,
            _ if token.starts_with("-n") || token.starts_with("-c") => {
                if !is_bounded_positive_count(&token[2..], 10_000) {
                    return false;
                }
                idx += 1;
            }
            _ if token.starts_with('-') && token[1..].chars().all(|ch| ch.is_ascii_digit()) => {
                if !is_bounded_positive_count(&token[1..], 10_000) {
                    return false;
                }
                idx += 1;
            }
            _ if token.starts_with('-') => return false,
            _ => {
                if !is_safe_readonly_path(token) {
                    return false;
                }
                saw_path = true;
                idx += 1;
            }
        }
    }

    saw_path
}

fn is_readonly_grep(tokens: &[String]) -> bool {
    let mut idx = 1;
    let mut pattern_seen = false;
    let mut saw_path = false;
    let mut operands_only = false;

    while idx < tokens.len() {
        let token = tokens[idx].as_str();
        if operands_only {
            if !pattern_seen {
                pattern_seen = true;
            } else if !is_safe_readonly_path(token) {
                return false;
            } else {
                saw_path = true;
            }
            idx += 1;
            continue;
        }

        match token {
            "--" => {
                operands_only = true;
                idx += 1;
            }
            "-e" => {
                if tokens.get(idx + 1).is_none() {
                    return false;
                }
                pattern_seen = true;
                idx += 2;
            }
            "-A" | "-B" | "-C" | "-m" => {
                let Some(value) = tokens.get(idx + 1) else {
                    return false;
                };
                if !is_bounded_positive_count(value, 10_000) {
                    return false;
                }
                idx += 2;
            }
            _ if is_safe_grep_short_flags(token) => idx += 1,
            _ if token.starts_with("-A")
                || token.starts_with("-B")
                || token.starts_with("-C")
                || token.starts_with("-m") =>
            {
                if !is_bounded_positive_count(&token[2..], 10_000) {
                    return false;
                }
                idx += 1;
            }
            _ if token.starts_with('-') => return false,
            _ if !pattern_seen => {
                pattern_seen = true;
                idx += 1;
            }
            _ => {
                if !is_safe_readonly_path(token) {
                    return false;
                }
                saw_path = true;
                idx += 1;
            }
        }
    }

    pattern_seen && saw_path
}

fn is_safe_grep_short_flags(token: &str) -> bool {
    token.starts_with('-')
        && token.len() > 1
        && token[1..].chars().all(|ch| {
            matches!(
                ch,
                'n' | 'i' | 'H' | 'h' | 'E' | 'F' | 'w' | 'x' | 's' | 'l' | 'L' | 'c'
            )
        })
}

fn is_readonly_rg(tokens: &[String]) -> bool {
    let mut idx = 1;
    let mut pattern_seen = false;
    let mut saw_files_mode = false;
    let mut operands_only = false;

    while idx < tokens.len() {
        let token = tokens[idx].as_str();
        if operands_only {
            if !pattern_seen && !saw_files_mode {
                pattern_seen = true;
            } else if !is_safe_readonly_path(token) {
                return false;
            }
            idx += 1;
            continue;
        }

        match token {
            "--" => {
                operands_only = true;
                idx += 1;
            }
            "--files" => {
                saw_files_mode = true;
                idx += 1;
            }
            "--line-number" | "--ignore-case" | "--smart-case" | "--case-sensitive"
            | "--fixed-strings" | "--word-regexp" | "--count" | "--no-heading"
            | "--with-filename" | "--hidden" => idx += 1,
            "-n" | "-i" | "-S" | "-s" | "-w" | "-x" | "-l" | "-c" => idx += 1,
            "-g" | "--glob" | "-t" | "--type" | "-T" | "--type-not" => {
                if tokens.get(idx + 1).is_none() {
                    return false;
                }
                idx += 2;
            }
            "-A" | "-B" | "-C" | "-m" | "--max-count" => {
                let Some(value) = tokens.get(idx + 1) else {
                    return false;
                };
                if !is_bounded_positive_count(value, 10_000) {
                    return false;
                }
                idx += 2;
            }
            _ if token.starts_with("-A")
                || token.starts_with("-B")
                || token.starts_with("-C")
                || token.starts_with("-m") =>
            {
                if !is_bounded_positive_count(&token[2..], 10_000) {
                    return false;
                }
                idx += 1;
            }
            _ if token.starts_with("--pre") => return false,
            _ if token.starts_with('-') => return false,
            _ if !pattern_seen && !saw_files_mode => {
                pattern_seen = true;
                idx += 1;
            }
            _ => {
                if !is_safe_readonly_path(token) {
                    return false;
                }
                idx += 1;
            }
        }
    }

    pattern_seen || saw_files_mode
}

fn is_readonly_find(tokens: &[String]) -> bool {
    if tokens.len() == 1 {
        return false;
    }

    let mut idx = 1;
    while idx < tokens.len() {
        let token = tokens[idx].as_str();
        match token {
            "-print" | "-ls" => idx += 1,
            "-maxdepth" | "-mindepth" => {
                let Some(value) = tokens.get(idx + 1) else {
                    return false;
                };
                if !is_bounded_positive_count(value, 20) {
                    return false;
                }
                idx += 2;
            }
            "-name" | "-iname" | "-path" => {
                if tokens.get(idx + 1).is_none() {
                    return false;
                }
                idx += 2;
            }
            "-type" => {
                let Some(value) = tokens.get(idx + 1) else {
                    return false;
                };
                if !value
                    .chars()
                    .all(|ch| matches!(ch, 'f' | 'd' | 'l' | 's' | 'p'))
                {
                    return false;
                }
                idx += 2;
            }
            _ if token.starts_with('-') => return false,
            _ => {
                if !is_safe_readonly_path(token) {
                    return false;
                }
                idx += 1;
            }
        }
    }

    true
}

fn is_readonly_ps(tokens: &[String]) -> bool {
    let mut idx = 1;
    while idx < tokens.len() {
        let token = tokens[idx].as_str();
        match token {
            "aux" | "-A" | "-a" | "-e" | "-f" | "-r" | "-u" | "-x" => idx += 1,
            "-ef" | "-aux" => idx += 1,
            "-o" => {
                let Some(fields) = tokens.get(idx + 1) else {
                    return false;
                };
                if !is_safe_ps_fields(fields) {
                    return false;
                }
                idx += 2;
            }
            "-Ao" => {
                let Some(fields) = tokens.get(idx + 1) else {
                    return false;
                };
                if !is_safe_ps_fields(fields) {
                    return false;
                }
                idx += 2;
            }
            _ if token.starts_with("-Ao") => {
                let fields = token.trim_start_matches("-Ao");
                if fields.is_empty() || !is_safe_ps_fields(fields) {
                    return false;
                }
                idx += 1;
            }
            _ => return false,
        }
    }

    true
}

fn is_safe_ps_fields(fields: &str) -> bool {
    fields.split(',').all(|field| {
        matches!(
            field,
            "pid"
                | "ppid"
                | "pcpu"
                | "pmem"
                | "rss"
                | "vsz"
                | "stat"
                | "state"
                | "time"
                | "etime"
                | "user"
                | "uid"
                | "comm"
                | "command"
        )
    })
}

fn is_readonly_git(tokens: &[String]) -> bool {
    if tokens.len() < 2 || tokens.iter().skip(1).any(is_risky_git_arg) {
        return false;
    }

    match tokens.get(1).map(String::as_str) {
        Some("status") => tokens.iter().skip(2).all(|arg| {
            matches!(
                arg.as_str(),
                "--short"
                    | "--branch"
                    | "--porcelain"
                    | "--porcelain=v1"
                    | "--porcelain=v2"
                    | "--ignored"
                    | "--untracked-files"
                    | "--untracked-files=no"
                    | "--untracked-files=normal"
                    | "--untracked-files=all"
            )
        }),
        Some("diff") => tokens.iter().skip(2).all(is_safe_git_diff_arg),
        Some("log") => tokens.iter().skip(2).all(is_safe_git_log_arg),
        Some("show") => tokens.iter().skip(2).all(is_safe_git_show_arg),
        Some("branch" | "remote" | "rev-parse") => tokens.iter().skip(2).all(is_plain_git_arg),
        _ => false,
    }
}

fn is_risky_git_arg(arg: &String) -> bool {
    let arg = arg.as_str();
    arg == "-c"
        || arg.starts_with("-c")
        || arg == "--ext-diff"
        || arg == "--textconv"
        || arg.starts_with("--output")
        || arg.starts_with("--exec-path")
}

fn is_safe_git_diff_arg(arg: &String) -> bool {
    matches!(
        arg.as_str(),
        "--stat"
            | "--shortstat"
            | "--numstat"
            | "--summary"
            | "--name-only"
            | "--name-status"
            | "--cached"
            | "--staged"
            | "--check"
            | "--no-ext-diff"
            | "--"
    ) || is_plain_git_arg(arg)
}

fn is_safe_git_log_arg(arg: &String) -> bool {
    matches!(
        arg.as_str(),
        "--oneline"
            | "--decorate"
            | "--stat"
            | "--shortstat"
            | "--name-only"
            | "--name-status"
            | "--no-ext-diff"
            | "--"
    ) || arg.starts_with('-') && arg[1..].chars().all(|ch| matches!(ch, 'n' | '1'..='9'))
        || is_plain_git_arg(arg)
}

fn is_safe_git_show_arg(arg: &String) -> bool {
    matches!(
        arg.as_str(),
        "--stat" | "--shortstat" | "--name-only" | "--name-status" | "--no-ext-diff" | "--"
    ) || is_plain_git_arg(arg)
}

fn is_plain_git_arg(arg: &String) -> bool {
    !arg.starts_with('-') && !is_blocked_special_path(arg)
}

fn is_safe_readonly_path(path: &str) -> bool {
    !path.is_empty() && path != "-" && !path.starts_with('-') && !is_blocked_special_path(path)
}

fn is_bounded_positive_count(value: &str, max: u32) -> bool {
    let Ok(count) = value.parse::<u32>() else {
        return false;
    };

    count > 0 && count <= max
}

fn is_blocked_special_path(path: &str) -> bool {
    path == "/dev"
        || path.starts_with("/dev/")
        || path == "/proc"
        || path.starts_with("/proc/")
        || path == "/sys"
        || path.starts_with("/sys/")
}

fn is_readonly_sysctl(tokens: &[String]) -> bool {
    if tokens.len() < 3 || tokens.get(1).map(String::as_str) != Some("-n") {
        return false;
    }

    tokens[2..].iter().all(|key| {
        matches!(
            key.as_str(),
            "hw.ncpu"
                | "hw.logicalcpu"
                | "hw.physicalcpu"
                | "hw.memsize"
                | "hw.model"
                | "machdep.cpu.brand_string"
                | "machdep.cpu.core_count"
                | "machdep.cpu.thread_count"
                | "kern.osproductversion"
                | "kern.version"
        )
    })
}

fn is_bounded_top_snapshot(tokens: &[String]) -> bool {
    if tokens.len() < 2 {
        return false;
    }

    let mut idx = 1;
    let mut macos_single_sample = false;
    let mut linux_batch = false;
    let mut top_count = None;

    while idx < tokens.len() {
        match tokens[idx].as_str() {
            "-b" => {
                linux_batch = true;
                idx += 1;
            }
            "-l" => {
                let Some(value) = tokens.get(idx + 1) else {
                    return false;
                };
                if value != "1" {
                    return false;
                }
                macos_single_sample = true;
                idx += 2;
            }
            "-n" => {
                let Some(value) = tokens.get(idx + 1) else {
                    return false;
                };
                let Ok(count) = value.parse::<u16>() else {
                    return false;
                };
                if count == 0 || count > 100 {
                    return false;
                }
                top_count = Some(count);
                idx += 2;
            }
            "-s" | "-d" => {
                let Some(value) = tokens.get(idx + 1) else {
                    return false;
                };
                let Ok(seconds) = value.parse::<u8>() else {
                    return false;
                };
                if seconds > 5 {
                    return false;
                }
                idx += 2;
            }
            "-o" | "-stats" => {
                let Some(value) = tokens.get(idx + 1) else {
                    return false;
                };
                if !is_safe_top_field(value) {
                    return false;
                }
                idx += 2;
            }
            _ => return false,
        }
    }

    macos_single_sample || (linux_batch && top_count == Some(1))
}

fn is_safe_top_field(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ',' | '%'))
}

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
    use super::{readonly_tokens, run_approved_bash_tool, ToolExecutionStatus};

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
            "uname -a",
            "id -u",
        ] {
            assert!(readonly_tokens(command).is_ok(), "{command}");
        }
    }
}
