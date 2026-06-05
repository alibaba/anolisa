use crate::exit_classify::first_program_token;
use crate::hook_types::*;
use crate::types::CommandBlock;
use std::fs;

pub trait BuiltinHook: Send + Sync {
    fn id(&self) -> &str;
    fn matcher(&self) -> &HookMatcher;
    fn evaluate(&self, input: &HookInput) -> Option<HookFinding>;
}

pub struct HookEngine {
    builtin_hooks: Vec<Box<dyn BuiltinHook>>,
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
        }
    }

    pub fn register(&mut self, hook: Box<dyn BuiltinHook>) {
        self.builtin_hooks.push(hook);
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
        findings.sort_by_key(|f| match f.severity {
            FindingSeverity::Critical => 0,
            FindingSeverity::Warning => 1,
            FindingSeverity::Info => 2,
        });
        findings
    }

    pub fn registered_hooks(&self) -> Vec<&str> {
        self.builtin_hooks.iter().map(|h| h.id()).collect()
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
    matcher.commands.is_empty() && matcher.command_patterns.is_empty()
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
    let preview: String = content.lines().take(max_lines).collect::<Vec<_>>().join("\n");
    if preview.is_empty() {
        None
    } else {
        Some(preview)
    }
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

    fn make_matcher(
        commands: Vec<&str>,
        patterns: Vec<&str>,
        trigger: HookTrigger,
    ) -> HookMatcher {
        HookMatcher {
            id: "test".to_string(),
            commands: commands.into_iter().map(String::from).collect(),
            command_patterns: patterns.into_iter().map(String::from).collect(),
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
            })
        }
    }

    #[test]
    fn evaluate_returns_sorted_findings() {
        let mut engine = HookEngine::new();
        engine.register(Box::new(FakeHook {
            matcher: HookMatcher {
                id: "info-hook".to_string(),
                commands: vec![],
                command_patterns: vec![],
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
