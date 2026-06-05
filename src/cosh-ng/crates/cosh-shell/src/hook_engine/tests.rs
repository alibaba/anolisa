use super::loader::{parse_hook_header, parse_timeout};
use super::matcher::matches_command;
use super::*;
use std::fs;
use std::path::PathBuf;

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

fn make_block(command: &str) -> CommandBlock {
    CommandBlock {
        id: "b1".to_string(),
        session_id: "s1".to_string(),
        command: command.to_string(),
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
    }
}

#[cfg(unix)]
fn write_executable_hook(dir_name: &str, file_name: &str, body: &str) -> (PathBuf, PathBuf) {
    use std::os::unix::fs::PermissionsExt;

    let dir = std::env::temp_dir().join(dir_name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(file_name);
    fs::write(&path, body).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    (dir, path)
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
fn matches_on_fail_skips_non_actionable_exit_categories() {
    let matcher = make_matcher(vec!["grep"], vec![], HookTrigger::OnFail);

    assert!(!matches_command(
        &matcher,
        &make_input("grep needle file.txt", 1)
    ));
    assert!(!matches_command(&matcher, &make_input("grep needle", 130)));
    assert!(!matches_command(
        &matcher,
        &make_input("yes | grep needle | head -1", 141)
    ));

    let crash_matcher = make_matcher(vec!["worker"], vec![], HookTrigger::OnFail);
    assert!(matches_command(&crash_matcher, &make_input("worker", 137)));
}

#[test]
fn matches_command_name() {
    let matcher = make_matcher(vec!["git"], vec![], HookTrigger::OnComplete);
    let input = make_input("git status", 0);
    assert!(matches_command(&matcher, &input));
}

#[test]
fn matches_command_name_after_sudo_options() {
    let matcher = make_matcher(vec!["free"], vec![], HookTrigger::OnComplete);
    let input = make_input("sudo -n -E free -m", 0);
    assert!(matches_command(&matcher, &input));

    let env_input = make_input("LANG=C sudo -n free -m", 0);
    assert!(matches_command(&matcher, &env_input));

    let unknown_sudo_input = make_input("sudo --definitely-unknown free -m", 0);
    assert!(!matches_command(&matcher, &unknown_sudo_input));
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

#[test]
fn command_regex_is_literal_contains_for_now() {
    let matcher = HookMatcher {
        id: "test".to_string(),
        commands: vec![],
        command_patterns: vec![],
        command_regex: Some("cargo.*test".to_string()),
        min_output_bytes: None,
        exit_codes: None,
        trigger: HookTrigger::OnComplete,
    };

    assert!(matches_command(
        &matcher,
        &make_input("printf 'cargo.*test'", 0)
    ));
    assert!(!matches_command(
        &matcher,
        &make_input("cargo nextest run", 0)
    ));
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
fn load_project_hooks_missing_dir_is_noop() {
    let project = std::env::temp_dir().join("cosh_hook_test_project_missing_dir");
    let _ = fs::remove_dir_all(&project);
    fs::create_dir_all(&project).unwrap();

    let mut engine = HookEngine::new();
    engine.load_project_hooks_from_root(&project, false);

    assert!(engine.external_hooks().is_empty());
    assert!(engine.registered_hook_infos().is_empty());

    let _ = fs::remove_dir_all(&project);
}

#[cfg(unix)]
#[test]
fn untrusted_project_hook_is_discovered_but_not_executed() {
    let project = std::env::temp_dir().join("cosh_hook_test_project_untrusted");
    let hooks_dir = project.join(".cosh/hooks");
    let _ = fs::remove_dir_all(&project);
    fs::create_dir_all(&hooks_dir).unwrap();
    let marker = project.join("executed.marker");
    let hook = hooks_dir.join("project.sh");
    let body = format!(
        "#!/bin/sh\n# cosh-hook: project-hook\n# match-commands: echo\ntouch '{}'\nprintf '{{\"hook_id\":\"project-hook\",\"severity\":\"warning\",\"title\":\"t\",\"description\":\"d\",\"suggestion\":\"s\"}}'\n",
        marker.display()
    );
    fs::write(&hook, body).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let mut engine = HookEngine::new();
    engine.load_project_hooks_from_root(&project, false);

    assert_eq!(engine.external_hooks().len(), 1);
    assert_eq!(
        engine.external_hooks()[0].source,
        ExternalHookSource::Project
    );
    assert!(!engine.external_hooks()[0].trusted);
    assert!(engine.evaluate(&make_block("echo hi")).is_empty());
    assert!(!marker.exists());

    let infos = engine.registered_hook_infos();
    assert_eq!(infos[0].source, HookSourceInfo::ExternalProject);
    assert_eq!(infos[0].trusted, Some(false));

    let _ = fs::remove_dir_all(&project);
}

#[cfg(unix)]
#[test]
fn trusted_project_hook_executes_after_match() {
    let project = std::env::temp_dir().join("cosh_hook_test_project_trusted");
    let hooks_dir = project.join(".cosh/hooks");
    let _ = fs::remove_dir_all(&project);
    fs::create_dir_all(&hooks_dir).unwrap();
    let marker = project.join("executed.marker");
    let hook = hooks_dir.join("project.sh");
    let body = format!(
        "#!/bin/sh\n# cosh-hook: project-hook\n# match-commands: echo\ntouch '{}'\nprintf '{{\"hook_id\":\"project-hook\",\"severity\":\"warning\",\"title\":\"t\",\"description\":\"d\",\"suggestion\":\"s\"}}'\n",
        marker.display()
    );
    fs::write(&hook, body).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let mut engine = HookEngine::new();
    engine.load_project_hooks_from_root(&project, true);

    let findings = engine.evaluate(&make_block("echo hi"));
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].hook_id, "project-hook");
    assert!(marker.exists());
    let infos = engine.registered_hook_infos();
    assert_eq!(infos[0].source, HookSourceInfo::ExternalProject);
    assert_eq!(infos[0].trusted, Some(true));

    let _ = fs::remove_dir_all(&project);
}

#[cfg(unix)]
#[test]
fn trusted_project_hook_still_respects_disabled_filter() {
    let project = std::env::temp_dir().join("cosh_hook_test_project_disabled");
    let hooks_dir = project.join(".cosh/hooks");
    let _ = fs::remove_dir_all(&project);
    fs::create_dir_all(&hooks_dir).unwrap();
    let marker = project.join("executed.marker");
    let hook = hooks_dir.join("project.sh");
    let body = format!(
        "#!/bin/sh\n# cosh-hook: project-hook\n# match-commands: echo\ntouch '{}'\nprintf '{{\"hook_id\":\"project-hook\",\"severity\":\"warning\",\"title\":\"t\",\"description\":\"d\",\"suggestion\":\"s\"}}'\n",
        marker.display()
    );
    fs::write(&hook, body).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let mut engine = HookEngine::new();
    engine.load_project_hooks_from_root(&project, false);
    assert_eq!(engine.set_project_hooks_trusted(true), 1);

    let disabled = HashSet::from(["project-hook".to_string()]);
    assert!(engine
        .evaluate_with_disabled(&make_block("echo hi"), &disabled)
        .is_empty());
    assert!(!marker.exists());

    let _ = fs::remove_dir_all(&project);
}

#[cfg(unix)]
#[test]
fn external_hook_nonzero_exit_is_no_finding() {
    let (dir, path) = write_executable_hook(
        "cosh_hook_test_external_nonzero",
        "nonzero.sh",
        "#!/bin/sh\n# cosh-hook: nonzero-hook\n# match-commands: echo\nprintf '{\"hook_id\":\"nonzero-hook\",\"severity\":\"warning\",\"title\":\"t\",\"description\":\"d\",\"suggestion\":\"s\"}'\nexit 7\n",
    );
    let config = parse_hook_header(&path).unwrap();
    let mut engine = HookEngine::new();
    engine.register_external(config);

    assert!(engine.evaluate(&make_block("echo hi")).is_empty());

    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn external_hook_malformed_json_is_no_finding() {
    let (dir, path) = write_executable_hook(
        "cosh_hook_test_external_malformed",
        "malformed.sh",
        "#!/bin/sh\n# cosh-hook: malformed-hook\n# match-commands: echo\nprintf 'not-json'\n",
    );
    let config = parse_hook_header(&path).unwrap();
    let mut engine = HookEngine::new();
    engine.register_external(config);

    assert!(engine.evaluate(&make_block("echo hi")).is_empty());

    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn external_hook_empty_or_stderr_only_output_is_no_finding() {
    let (empty_dir, empty_path) = write_executable_hook(
        "cosh_hook_test_external_empty",
        "empty.sh",
        "#!/bin/sh\n# cosh-hook: empty-hook\n# match-commands: echo\n",
    );
    let (stderr_dir, stderr_path) = write_executable_hook(
        "cosh_hook_test_external_stderr",
        "stderr.sh",
        "#!/bin/sh\n# cosh-hook: stderr-hook\n# match-commands: echo\necho noisy >&2\n",
    );
    let mut engine = HookEngine::new();
    engine.register_external(parse_hook_header(&empty_path).unwrap());
    engine.register_external(parse_hook_header(&stderr_path).unwrap());

    assert!(engine.evaluate(&make_block("echo hi")).is_empty());

    let _ = fs::remove_dir_all(&empty_dir);
    let _ = fs::remove_dir_all(&stderr_dir);
}

#[cfg(unix)]
#[test]
fn external_hook_timeout_is_killed_and_no_finding() {
    let (dir, path) = write_executable_hook(
        "cosh_hook_test_external_timeout",
        "timeout.sh",
        "#!/bin/sh\n# cosh-hook: timeout-hook\n# match-commands: echo\n# timeout: 20ms\nsleep 2\nprintf '{\"hook_id\":\"timeout-hook\",\"severity\":\"warning\",\"title\":\"t\",\"description\":\"d\",\"suggestion\":\"s\"}'\n",
    );
    let config = parse_hook_header(&path).unwrap();
    let mut engine = HookEngine::new();
    engine.register_external(config);

    let started = std::time::Instant::now();
    assert!(engine.evaluate(&make_block("echo hi")).is_empty());
    assert!(started.elapsed() < std::time::Duration::from_secs(1));

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
