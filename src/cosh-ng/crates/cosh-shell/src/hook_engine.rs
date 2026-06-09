use crate::exit_classify::first_program_token;
use crate::hook_types::*;
use crate::types::CommandBlock;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use wait_timeout::ChildExt;

pub trait BuiltinHook: Send + Sync {
    fn id(&self) -> &str;
    fn matcher(&self) -> &HookMatcher;
    fn evaluate(&self, input: &HookInput) -> Option<HookFinding>;
}

#[derive(Debug, Clone)]
pub struct ExternalHookConfig {
    pub path: PathBuf,
    pub matcher: HookMatcher,
    pub timeout_ms: u64,
}

pub struct HookEngine {
    builtin_hooks: Vec<Box<dyn BuiltinHook>>,
    external_hooks: Vec<ExternalHookConfig>,
}

impl Default for HookEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl HookEngine {
    pub fn new() -> Self {
        Self {
            builtin_hooks: Vec::new(),
            external_hooks: Vec::new(),
        }
    }

    pub fn register(&mut self, hook: Box<dyn BuiltinHook>) {
        self.builtin_hooks.push(hook);
    }

    pub fn register_external(&mut self, config: ExternalHookConfig) {
        self.external_hooks.push(config);
    }

    pub fn load_hooks_from_dir(&mut self, dir: &Path) {
        let entries = match fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = path.metadata() {
                    if meta.permissions().mode() & 0o111 == 0 {
                        continue;
                    }
                }
            }
            if let Some(config) = parse_hook_header(&path) {
                self.external_hooks.push(config);
            }
        }
    }

    pub fn evaluate(&self, block: &CommandBlock) -> Vec<HookFinding> {
        let input = hook_input_from_block(block);
        let mut findings = Vec::new();
        for hook in &self.builtin_hooks {
            if matches_command(hook.matcher(), &input) {
                if let Some(finding) = hook.evaluate(&input) {
                    findings.push(finding);
                }
            }
        }
        for ext in &self.external_hooks {
            if matches_command(&ext.matcher, &input) {
                if let Some(finding) = run_external_hook(ext, &input) {
                    findings.push(finding);
                }
            }
        }
        findings.sort_by_key(|f| match f.severity {
            FindingSeverity::Critical => 0,
            FindingSeverity::Warning => 1,
            FindingSeverity::Info => 2,
        });
        findings
    }

    pub fn registered_hooks(&self) -> Vec<&str> {
        let mut ids: Vec<&str> = self.builtin_hooks.iter().map(|h| h.id()).collect();
        for ext in &self.external_hooks {
            ids.push(&ext.matcher.id);
        }
        ids
    }

    pub fn external_hooks(&self) -> &[ExternalHookConfig] {
        &self.external_hooks
    }
}

fn matches_command(matcher: &HookMatcher, input: &HookInput) -> bool {
    match matcher.trigger {
        HookTrigger::OnFail if input.exit_code == 0 => return false,
        HookTrigger::OnSuccess if input.exit_code != 0 => return false,
        _ => {}
    }
    if let Some(ref codes) = matcher.exit_codes {
        if !codes.contains(&input.exit_code) {
            return false;
        }
    }
    if let Some(min_bytes) = matcher.min_output_bytes {
        if input.output_bytes < min_bytes {
            return false;
        }
    }
    let program = first_program_token(&input.command);
    if matcher.commands.iter().any(|cmd| cmd == program) {
        return true;
    }
    if matcher
        .command_patterns
        .iter()
        .any(|p| input.command.trim_start().starts_with(p))
    {
        return true;
    }
    if let Some(ref pattern) = matcher.command_regex {
        if input.command.contains(pattern) {
            return true;
        }
    }
    matcher.commands.is_empty()
        && matcher.command_patterns.is_empty()
        && matcher.command_regex.is_none()
}

fn hook_input_from_block(block: &CommandBlock) -> HookInput {
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

fn parse_hook_header(path: &Path) -> Option<ExternalHookConfig> {
    let content = fs::read_to_string(path).ok()?;
    let mut hook_id: Option<String> = None;
    let mut match_commands: Vec<String> = Vec::new();
    let mut trigger = HookTrigger::OnComplete;
    let mut timeout_ms: u64 = 5000;

    for line in content.lines().take(10) {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("# cosh-hook:") {
            hook_id = Some(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("# match-commands:") {
            match_commands = val.split(',').map(|s| s.trim().to_string()).collect();
        } else if let Some(val) = line.strip_prefix("# trigger:") {
            trigger = match val.trim() {
                "on_fail" => HookTrigger::OnFail,
                "on_success" => HookTrigger::OnSuccess,
                _ => HookTrigger::OnComplete,
            };
        } else if let Some(val) = line.strip_prefix("# timeout:") {
            timeout_ms = parse_timeout(val.trim());
        }
    }

    let id = hook_id?;
    Some(ExternalHookConfig {
        path: path.to_path_buf(),
        matcher: HookMatcher {
            id,
            commands: match_commands,
            command_patterns: Vec::new(),
            command_regex: None,
            min_output_bytes: None,
            exit_codes: None,
            trigger,
        },
        timeout_ms,
    })
}

fn parse_timeout(s: &str) -> u64 {
    if let Some(ms) = s.strip_suffix("ms") {
        ms.trim().parse::<u64>().unwrap_or(5000)
    } else if let Some(secs) = s.strip_suffix('s') {
        secs.trim().parse::<u64>().unwrap_or(5) * 1000
    } else {
        s.parse::<u64>().unwrap_or(5000)
    }
}

fn run_external_hook(config: &ExternalHookConfig, input: &HookInput) -> Option<HookFinding> {
    let input_json = serde_json::to_string(input).ok()?;

    let mut child = Command::new(&config.path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| {
            eprintln!(
                "cosh-shell: external hook {:?} spawn failed: {e}",
                config.path
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
            eprintln!(
                "cosh-shell: external hook {:?} exited with error",
                config.path
            );
            return None;
        }
        Ok(None) => {
            let _ = child.kill();
            let _ = child.wait();
            eprintln!(
                "cosh-shell: external hook {:?} timed out after {}ms",
                config.path, config.timeout_ms
            );
            return None;
        }
        Err(e) => {
            eprintln!(
                "cosh-shell: external hook {:?} wait failed: {e}",
                config.path
            );
            return None;
        }
    }

    // Process has exited successfully; read stdout (8KB limit)
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
            eprintln!(
                "cosh-shell: external hook {:?} invalid JSON output: {e}",
                config.path
            );
        })
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_input(command: &str, exit_code: i32) -> HookInput {
        HookInput {
            command: command.to_string(),
            cwd: "/tmp".to_string(),
            exit_code,
            duration_ms: 100,
            output_ref: None,
            output_bytes: 0,
            output_preview: String::new(),
        }
    }

    fn make_matcher(commands: Vec<&str>, patterns: Vec<&str>, trigger: HookTrigger) -> HookMatcher {
        HookMatcher {
            id: "test".to_string(),
            commands: commands.into_iter().map(String::from).collect(),
            command_patterns: patterns.into_iter().map(String::from).collect(),
            command_regex: None,
            min_output_bytes: None,
            exit_codes: None,
            trigger,
        }
    }

    #[test]
    fn matches_on_fail_skips_success() {
        let matcher = make_matcher(vec!["cargo"], vec![], HookTrigger::OnFail);
        let input = make_input("cargo test", 0);
        assert!(!matches_command(&matcher, &input));
    }

    #[test]
    fn matches_on_fail_fires_on_nonzero() {
        let matcher = make_matcher(vec!["cargo"], vec![], HookTrigger::OnFail);
        let input = make_input("cargo test", 1);
        assert!(matches_command(&matcher, &input));
    }

    #[test]
    fn matches_command_name() {
        let matcher = make_matcher(vec!["git"], vec![], HookTrigger::OnComplete);
        let input = make_input("git status", 0);
        assert!(matches_command(&matcher, &input));
    }

    #[test]
    fn no_match_wrong_command_name() {
        let matcher = make_matcher(vec!["npm"], vec![], HookTrigger::OnComplete);
        let input = make_input("cargo build", 0);
        assert!(!matches_command(&matcher, &input));
    }

    #[test]
    fn matches_command_pattern_prefix() {
        let matcher = make_matcher(vec![], vec!["cargo test"], HookTrigger::OnComplete);
        let input = make_input("cargo test --workspace", 0);
        assert!(matches_command(&matcher, &input));
    }

    #[test]
    fn no_match_wrong_pattern() {
        let matcher = make_matcher(vec![], vec!["cargo test"], HookTrigger::OnComplete);
        let input = make_input("cargo build", 0);
        assert!(!matches_command(&matcher, &input));
    }

    struct FakeHook {
        matcher: HookMatcher,
        severity: FindingSeverity,
    }

    impl BuiltinHook for FakeHook {
        fn id(&self) -> &str {
            &self.matcher.id
        }
        fn matcher(&self) -> &HookMatcher {
            &self.matcher
        }
        fn evaluate(&self, _input: &HookInput) -> Option<HookFinding> {
            Some(HookFinding {
                hook_id: self.matcher.id.clone(),
                severity: self.severity,
                title: "test".to_string(),
                description: "desc".to_string(),
                suggestion: "fix it".to_string(),
                skill: None,
                cli_hint: None,
                context_refs: Vec::new(),
            })
        }
    }

    #[test]
    fn parse_timeout_seconds() {
        assert_eq!(parse_timeout("3s"), 3000);
        assert_eq!(parse_timeout("10s"), 10000);
    }

    #[test]
    fn parse_timeout_milliseconds() {
        assert_eq!(parse_timeout("500ms"), 500);
        assert_eq!(parse_timeout("2500ms"), 2500);
    }

    #[test]
    fn parse_timeout_raw_number() {
        assert_eq!(parse_timeout("4000"), 4000);
    }

    #[test]
    fn parse_timeout_invalid_falls_back() {
        assert_eq!(parse_timeout("bogus"), 5000);
        assert_eq!(parse_timeout(""), 5000);
    }

    #[test]
    fn parse_hook_header_full() {
        let dir = std::env::temp_dir().join("cosh_hook_test_full");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("my-hook.sh");
        fs::write(
            &path,
            "#!/bin/bash\n# cosh-hook: my-hook-id\n# match-commands: docker, kubectl\n# trigger: on_fail\n# timeout: 3s\necho hello\n",
        )
        .unwrap();

        let config = parse_hook_header(&path).unwrap();
        assert_eq!(config.matcher.id, "my-hook-id");
        assert_eq!(config.matcher.commands, vec!["docker", "kubectl"]);
        assert_eq!(config.matcher.trigger, HookTrigger::OnFail);
        assert_eq!(config.timeout_ms, 3000);
        assert_eq!(config.path, path);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_hook_header_defaults() {
        let dir = std::env::temp_dir().join("cosh_hook_test_defaults");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("simple.sh");
        fs::write(&path, "#!/bin/bash\n# cosh-hook: simple\n").unwrap();

        let config = parse_hook_header(&path).unwrap();
        assert_eq!(config.matcher.id, "simple");
        assert!(config.matcher.commands.is_empty());
        assert_eq!(config.matcher.trigger, HookTrigger::OnComplete);
        assert_eq!(config.timeout_ms, 5000);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_hook_header_missing_id_returns_none() {
        let dir = std::env::temp_dir().join("cosh_hook_test_no_id");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("no-id.sh");
        fs::write(&path, "#!/bin/bash\n# match-commands: git\n").unwrap();

        assert!(parse_hook_header(&path).is_none());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_hook_header_on_success_trigger() {
        let dir = std::env::temp_dir().join("cosh_hook_test_success");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("ok.sh");
        fs::write(
            &path,
            "#!/bin/bash\n# cosh-hook: ok-hook\n# trigger: on_success\n",
        )
        .unwrap();

        let config = parse_hook_header(&path).unwrap();
        assert_eq!(config.matcher.trigger, HookTrigger::OnSuccess);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_hook_header_on_complete_trigger() {
        let dir = std::env::temp_dir().join("cosh_hook_test_complete");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("complete.sh");
        fs::write(
            &path,
            "#!/bin/bash\n# cosh-hook: c-hook\n# trigger: on_complete\n",
        )
        .unwrap();

        let config = parse_hook_header(&path).unwrap();
        assert_eq!(config.matcher.trigger, HookTrigger::OnComplete);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_hook_header_timeout_ms_format() {
        let dir = std::env::temp_dir().join("cosh_hook_test_tms");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("tms.sh");
        fs::write(
            &path,
            "#!/bin/bash\n# cosh-hook: tms-hook\n# timeout: 1500ms\n",
        )
        .unwrap();

        let config = parse_hook_header(&path).unwrap();
        assert_eq!(config.timeout_ms, 1500);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_hooks_from_dir_skips_non_executable() {
        let dir = std::env::temp_dir().join("cosh_hook_test_noexec");
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::create_dir_all(&dir);

        // Non-executable file
        let path = dir.join("no-exec.sh");
        fs::write(&path, "#!/bin/bash\n# cosh-hook: no-exec\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        }

        // Executable file
        let path2 = dir.join("exec.sh");
        fs::write(&path2, "#!/bin/bash\n# cosh-hook: exec-hook\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path2, fs::Permissions::from_mode(0o755)).unwrap();
        }

        let mut engine = HookEngine::new();
        engine.load_hooks_from_dir(&dir);

        assert_eq!(engine.external_hooks().len(), 1);
        assert_eq!(engine.external_hooks()[0].matcher.id, "exec-hook");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn evaluate_returns_sorted_findings() {
        let mut engine = HookEngine::new();
        engine.register(Box::new(FakeHook {
            matcher: HookMatcher {
                id: "info-hook".to_string(),
                commands: vec![],
                command_patterns: vec![],
                command_regex: None,
                min_output_bytes: None,
                exit_codes: None,
                trigger: HookTrigger::OnComplete,
            },
            severity: FindingSeverity::Info,
        }));
        engine.register(Box::new(FakeHook {
            matcher: HookMatcher {
                id: "critical-hook".to_string(),
                commands: vec![],
                command_patterns: vec![],
                command_regex: None,
                min_output_bytes: None,
                exit_codes: None,
                trigger: HookTrigger::OnComplete,
            },
            severity: FindingSeverity::Critical,
        }));
        engine.register(Box::new(FakeHook {
            matcher: HookMatcher {
                id: "warning-hook".to_string(),
                commands: vec![],
                command_patterns: vec![],
                command_regex: None,
                min_output_bytes: None,
                exit_codes: None,
                trigger: HookTrigger::OnComplete,
            },
            severity: FindingSeverity::Warning,
        }));

        let block = CommandBlock {
            id: "b1".to_string(),
            session_id: "s1".to_string(),
            command: "ls".to_string(),
            cwd: "/tmp".to_string(),
            end_cwd: "/tmp".to_string(),
            started_at_ms: 0,
            ended_at_ms: 100,
            duration_ms: 100,
            exit_code: 0,
            status: crate::types::CommandStatus::Completed,
            output: crate::types::OutputRefs {
                terminal_output_ref: None,
                terminal_output_bytes: 0,
            },
        };

        let findings = engine.evaluate(&block);
        assert_eq!(findings.len(), 3);
        assert_eq!(findings[0].severity, FindingSeverity::Critical);
        assert_eq!(findings[1].severity, FindingSeverity::Warning);
        assert_eq!(findings[2].severity, FindingSeverity::Info);
    }
}
