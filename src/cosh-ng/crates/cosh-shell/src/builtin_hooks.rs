use crate::exit_classify::{classify_exit, first_program_token, ExitCodeCategory};
use crate::hook_engine::BuiltinHook;
use crate::hook_types::*;

pub struct FailedCommandHook {
    matcher: HookMatcher,
}

impl FailedCommandHook {
    pub fn new() -> Self {
        Self {
            matcher: HookMatcher {
                id: "failed-command".into(),
                commands: vec![],
                command_patterns: vec![],
                exit_codes: None,
                trigger: HookTrigger::OnFail,
            },
        }
    }
}

impl BuiltinHook for FailedCommandHook {
    fn id(&self) -> &str {
        "failed-command"
    }
    fn matcher(&self) -> &HookMatcher {
        &self.matcher
    }
    fn evaluate(&self, input: &HookInput) -> Option<HookFinding> {
        let category = classify_exit(input.exit_code, &input.command);
        match category {
            ExitCodeCategory::Success
            | ExitCodeCategory::UserInterrupt
            | ExitCodeCategory::PipelineNormal
            | ExitCodeCategory::CommandSpecificNormal => None,
            _ => Some(HookFinding {
                hook_id: "failed-command".into(),
                severity: FindingSeverity::Warning,
                title: format!(
                    "`{}` exited with code {}",
                    input.command.trim(),
                    input.exit_code
                ),
                description: format!("Command failed in {}", input.cwd),
                suggestion: "Use /explain to analyze the failure".into(),
                skill: None,
                cli_hint: None,
            }),
        }
    }
}

pub struct TestFailureHook {
    matcher: HookMatcher,
}

impl TestFailureHook {
    const COMMAND_PATTERNS: &[&str] = &[
        "cargo test",
        "npm test",
        "pnpm test",
        "yarn test",
        "pytest",
        "make test",
    ];

    pub fn new() -> Self {
        Self {
            matcher: HookMatcher {
                id: "test-failure".into(),
                commands: vec![],
                command_patterns: Self::COMMAND_PATTERNS
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                exit_codes: None,
                trigger: HookTrigger::OnFail,
            },
        }
    }

    fn skill_for_command(command: &str) -> &'static str {
        let prog = first_program_token(command);
        match prog {
            "cargo" => "rust-project",
            "npm" | "pnpm" | "yarn" => "node-project",
            _ => "test-analysis",
        }
    }
}

impl BuiltinHook for TestFailureHook {
    fn id(&self) -> &str {
        "test-failure"
    }
    fn matcher(&self) -> &HookMatcher {
        &self.matcher
    }
    fn evaluate(&self, input: &HookInput) -> Option<HookFinding> {
        let skill = Self::skill_for_command(&input.command);
        Some(HookFinding {
            hook_id: "test-failure".into(),
            severity: FindingSeverity::Warning,
            title: format!(
                "Test command `{}` failed (exit {})",
                input.command.trim(),
                input.exit_code
            ),
            description: format!("Test failure in {}", input.cwd),
            suggestion: format!("Use /{skill} to diagnose test failures"),
            skill: Some(skill.to_string()),
            cli_hint: None,
        })
    }
}

pub fn default_builtin_hooks() -> Vec<Box<dyn BuiltinHook>> {
    vec![
        Box::new(FailedCommandHook::new()),
        Box::new(TestFailureHook::new()),
    ]
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

    #[test]
    fn failed_command_skips_sigint() {
        let hook = FailedCommandHook::new();
        let input = make_input("sleep 100", 130);
        assert!(hook.evaluate(&input).is_none());
    }

    #[test]
    fn failed_command_skips_grep_exit_one() {
        let hook = FailedCommandHook::new();
        let input = make_input("grep pattern file.txt", 1);
        assert!(hook.evaluate(&input).is_none());
    }

    #[test]
    fn failed_command_produces_finding_for_exit_two() {
        let hook = FailedCommandHook::new();
        let input = make_input("ls --bad-flag", 2);
        let finding = hook.evaluate(&input).expect("should produce a finding");
        assert_eq!(finding.hook_id, "failed-command");
        assert_eq!(finding.severity, FindingSeverity::Warning);
        assert!(finding.title.contains("exited with code 2"));
    }

    #[test]
    fn test_failure_rust_project_skill() {
        let hook = TestFailureHook::new();
        let input = make_input("cargo test --workspace", 101);
        let finding = hook.evaluate(&input).expect("should produce a finding");
        assert_eq!(finding.hook_id, "test-failure");
        assert_eq!(finding.skill.as_deref(), Some("rust-project"));
    }

    #[test]
    fn test_failure_node_project_skill() {
        let hook = TestFailureHook::new();
        let input = make_input("npm test", 1);
        let finding = hook.evaluate(&input).expect("should produce a finding");
        assert_eq!(finding.skill.as_deref(), Some("node-project"));
    }

    #[test]
    fn test_failure_pnpm_node_project_skill() {
        let hook = TestFailureHook::new();
        let input = make_input("pnpm test", 1);
        let finding = hook.evaluate(&input).expect("should produce a finding");
        assert_eq!(finding.skill.as_deref(), Some("node-project"));
    }

    #[test]
    fn test_failure_pytest_analysis_skill() {
        let hook = TestFailureHook::new();
        let input = make_input("pytest -v", 1);
        let finding = hook.evaluate(&input).expect("should produce a finding");
        assert_eq!(finding.skill.as_deref(), Some("test-analysis"));
    }

    #[test]
    fn default_builtin_hooks_returns_two() {
        let hooks = default_builtin_hooks();
        assert_eq!(hooks.len(), 2);
        assert_eq!(hooks[0].id(), "failed-command");
        assert_eq!(hooks[1].id(), "test-failure");
    }

    #[test]
    fn failed_command_skips_sigterm() {
        let hook = FailedCommandHook::new();
        let input = make_input("tail -f log", 143);
        assert!(hook.evaluate(&input).is_none());
    }

    #[test]
    fn failed_command_skips_sigpipe() {
        let hook = FailedCommandHook::new();
        let input = make_input("yes | head -1", 141);
        assert!(hook.evaluate(&input).is_none());
    }
}
