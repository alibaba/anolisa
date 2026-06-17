use super::*;

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
        origin: Default::default(),
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

#[test]
fn evaluate_with_disabled_skips_matching_hook() {
    let mut engine = HookEngine::new();
    engine.register(Box::new(FakeHook {
        matcher: HookMatcher {
            id: "disabled-hook".to_string(),
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
        origin: Default::default(),
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
    let disabled = HashSet::from(["disabled-hook".to_string()]);

    assert!(engine.evaluate_with_disabled(&block, &disabled).is_empty());
    assert_eq!(engine.evaluate(&block).len(), 1);
}
