use super::hooks::render_hooks_command;
use crate::hooks::state::{RuntimeHookDisplay, RuntimeHookFinding};
use crate::runtime::prelude::*;

struct EnvLock {
    path: std::path::PathBuf,
}

impl Drop for EnvLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn zh_state() -> InlineState {
    InlineState {
        language: Language::ZhCn,
        ..InlineState::default()
    }
}

fn env_lock() -> EnvLock {
    let path =
        std::env::temp_dir().join(format!("cosh-shell-test-env-{}.lock", std::process::id()));
    loop {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => return EnvLock { path },
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Err(err) => panic!("create env test lock failed: {err}"),
        }
    }
}

fn register_project_hook(state: &mut InlineState) {
    state.hooks.engine.register_external(ExternalHookConfig {
        path: std::path::PathBuf::from("/tmp/project/.cosh/hooks/project.sh"),
        matcher: HookMatcher {
            id: "project-hook".to_string(),
            commands: vec!["echo".to_string()],
            command_patterns: Vec::new(),
            command_regex: None,
            min_output_bytes: None,
            exit_codes: None,
            trigger: HookTrigger::OnComplete,
        },
        timeout_ms: 1000,
        source: ExternalHookSource::Project,
        project_root: Some(std::path::PathBuf::from("/tmp/project")),
        trusted: false,
    });
}

fn hook_hint() -> RuntimeHookFinding {
    RuntimeHookFinding {
        id: "hook-cmd-1-memory-pressure".to_string(),
        command_block_id: "cmd-1".to_string(),
        command: "free -m".to_string(),
        output_ref: Some("/tmp/out".to_string()),
        ended_at_ms: 200,
        prompt_hint: "hook_finding=memory-pressure".to_string(),
        finding_markdown: None,
        hook_finding: None,
        recommended_skill: Some("memory-analysis".to_string()),
        display: RuntimeHookDisplay::Hint,
        display_reason: "allowed".to_string(),
        related_hook_ids: Vec::new(),
        topic: "memory".to_string(),
        entity_key: "system-memory".to_string(),
        effective_severity: FindingSeverity::Critical,
        confidence: "high".to_string(),
        suppression_key: "memory:memory-pressure:free".to_string(),
    }
}

fn render_hooks_test_command(
    sub: Option<&str>,
    arg: Option<&str>,
    extra: Option<&str>,
    state: &mut InlineState,
) -> String {
    let adapter = AdapterInstance::Fake(FakeAgentAdapter);
    let mut output = Vec::new();
    render_hooks_command(sub, arg, extra, &[], &adapter, state, &mut output)
        .expect("render hooks command");
    String::from_utf8(output).expect("utf8 output")
}

#[test]
fn hooks_empty_list_uses_zh_catalog_text() {
    let mut state = zh_state();

    let output = render_hooks_test_command(None, None, None, &mut state);

    assert!(output.contains("Hook 状态"), "{output}");
    assert!(output.contains("未注册 Hook。"), "{output}");
    assert!(output.contains("已注册 0 个 Hook。"), "{output}");
    assert!(!output.contains("Hook status"), "{output}");
    assert!(!output.contains("No hooks registered"), "{output}");
}

#[test]
fn hooks_session_actions_use_zh_catalog_text() {
    let mut state = zh_state();

    let muted = render_hooks_test_command(Some("mute"), Some("memory"), None, &mut state);
    assert!(muted.contains("Hook 目标已静音"), "{muted}");
    assert!(
        muted.contains("本会话已静音 Hook 目标 'memory'。"),
        "{muted}"
    );
    assert!(!muted.contains("Hook target muted"), "{muted}");

    let unmuted = render_hooks_test_command(Some("unmute"), Some("memory"), None, &mut state);
    assert!(unmuted.contains("Hook 目标已取消静音"), "{unmuted}");
    assert!(
        unmuted.contains("已取消静音 Hook 目标 'memory'。"),
        "{unmuted}"
    );
    assert!(!unmuted.contains("Hook target unmuted"), "{unmuted}");

    let enabled = render_hooks_test_command(Some("enable"), Some("linux-memory"), None, &mut state);
    assert!(enabled.contains("Hook 已启用"), "{enabled}");
    assert!(
        enabled.contains("Hook 'linux-memory' 已启用。"),
        "{enabled}"
    );
    assert!(!enabled.contains("Hook enabled"), "{enabled}");

    let disabled =
        render_hooks_test_command(Some("disable"), Some("linux-memory"), None, &mut state);
    assert!(disabled.contains("Hook 已禁用"), "{disabled}");
    assert!(
        disabled.contains("Hook 'linux-memory' 已禁用。"),
        "{disabled}"
    );
    assert!(!disabled.contains("Hook disabled"), "{disabled}");
}

#[test]
fn hooks_history_and_events_empty_state_use_zh_catalog_text() {
    let mut state = zh_state();

    let history = render_hooks_test_command(Some("history"), None, None, &mut state);
    assert!(history.contains("Hook 历史"), "{history}");
    assert!(history.contains("本会话未记录 Hook finding。"), "{history}");
    assert!(!history.contains("No hook findings recorded"), "{history}");

    let events = render_hooks_test_command(Some("events"), None, None, &mut state);
    assert!(events.contains("Hook 显示事件"), "{events}");
    assert!(events.contains("本会话未记录 Hook 显示事件。"), "{events}");
    assert!(
        !events.contains("No hook display events recorded"),
        "{events}"
    );
}

#[test]
fn hooks_usage_uses_zh_catalog_text() {
    let mut state = zh_state();

    let output = render_hooks_test_command(Some("bogus"), None, None, &mut state);

    assert!(output.contains("用法"), "{output}");
    assert!(
        output.contains("/hooks                - 显示 Hook 状态"),
        "{output}"
    );
    assert!(!output.contains("/hooks clear-project-trust"), "{output}");
    assert!(!output.contains("/hooks feedback"), "{output}");
    assert!(!output.contains("/hooks analyze"), "{output}");
    assert!(!output.contains("show hook status"), "{output}");
    assert!(
        !output.contains("clear project hook trust store"),
        "{output}"
    );
}

#[test]
fn hooks_project_trust_empty_state_uses_zh_catalog_text() {
    let mut state = zh_state();

    let output = render_hooks_test_command(Some("trust-project"), None, None, &mut state);

    assert!(output.contains("项目 Hook 已信任"), "{output}");
    assert!(output.contains("本会话未注册项目 Hook。"), "{output}");
    assert!(output.contains("信任状态未变更。"), "{output}");
    assert!(!output.contains("Project hooks trusted"), "{output}");
    assert!(
        !output.contains("No project hooks are registered"),
        "{output}"
    );
}

#[test]
fn hooks_feedback_errors_use_zh_catalog_text() {
    let mut state = zh_state();

    let usage = render_hooks_test_command(Some("feedback"), Some("bad"), Some("id-1"), &mut state);
    assert!(usage.contains("用法"), "{usage}");
    assert!(
        usage.contains("/hooks feedback noisy|useful <finding_id>"),
        "{usage}"
    );

    let missing =
        render_hooks_test_command(Some("feedback"), Some("noisy"), Some("missing"), &mut state);
    assert!(missing.contains("Hook 反馈"), "{missing}");
    assert!(
        missing.contains("本会话未找到 finding 'missing'。"),
        "{missing}"
    );
    assert!(
        missing.contains("使用 /hooks history 复制最近的 finding id。"),
        "{missing}"
    );
    assert!(
        !missing.contains("Finding 'missing' was not found"),
        "{missing}"
    );
}

#[test]
fn hooks_project_trust_uses_zh_catalog_text() {
    let _env_lock = env_lock();
    let store = std::env::temp_dir().join(format!(
        "cosh-shell-slash-hooks-trust-{}.txt",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&store);
    std::env::set_var("COSH_SHELL_PROJECT_TRUST_STORE", &store);
    let mut state = zh_state();
    register_project_hook(&mut state);

    let trusted = render_hooks_test_command(Some("trust-project"), None, None, &mut state);
    assert!(trusted.contains("项目 Hook 已信任"), "{trusted}");
    assert!(
        trusted.contains("已将 1 个项目 Hook 标记为 trusted。"),
        "{trusted}"
    );
    assert!(
        trusted.contains("信任已持久化；已禁用 Hook 保持禁用。"),
        "{trusted}"
    );
    assert!(!trusted.contains("Project hooks trusted"), "{trusted}");

    let cleared = render_hooks_test_command(Some("clear-project-trust"), None, None, &mut state);
    assert!(cleared.contains("项目 Hook 信任已清除"), "{cleared}");
    assert!(
        cleared.contains("已将 1 个项目 Hook 标记为 untrusted。"),
        "{cleared}"
    );
    assert!(cleared.contains("项目 Hook 信任存储已清除"), "{cleared}");
    assert!(!cleared.contains("Project hook trust cleared"), "{cleared}");

    std::env::remove_var("COSH_SHELL_PROJECT_TRUST_STORE");
    let _ = std::fs::remove_file(&store);
}

#[test]
fn hooks_feedback_uses_zh_catalog_text() {
    let _env_lock = env_lock();
    let store = std::env::temp_dir().join(format!(
        "cosh-shell-slash-hooks-feedback-{}.txt",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&store);
    std::env::set_var("COSH_SHELL_HOOK_FEEDBACK_STORE", &store);
    let mut state = zh_state();

    let usage = render_hooks_test_command(
        Some("feedback"),
        Some("bad"),
        Some("finding-id"),
        &mut state,
    );
    assert!(usage.contains("用法"), "{usage}");
    assert!(
        usage.contains("/hooks feedback noisy|useful <finding_id>"),
        "{usage}"
    );

    let missing = render_hooks_test_command(
        Some("feedback"),
        Some("noisy"),
        Some("missing-id"),
        &mut state,
    );
    assert!(missing.contains("Hook 反馈"), "{missing}");
    assert!(
        missing.contains("本会话未找到 finding 'missing-id'。"),
        "{missing}"
    );
    assert!(
        !missing.contains("Finding 'missing-id' was not found"),
        "{missing}"
    );

    state.hooks.findings.push(hook_hint());
    let recorded = render_hooks_test_command(
        Some("feedback"),
        Some("noisy"),
        Some("hook-cmd-1-memory-pressure"),
        &mut state,
    );
    assert!(recorded.contains("Hook 反馈已记录"), "{recorded}");
    assert!(
        recorded.contains("已为 finding 'hook-cmd-1-memory-pressure' 记录反馈 'noisy'。"),
        "{recorded}"
    );
    assert!(
        recorded.contains("反馈已持久化，仅影响展示策略。"),
        "{recorded}"
    );
    assert!(!recorded.contains("Hook feedback recorded"), "{recorded}");

    let cleared = render_hooks_test_command(Some("clear-feedback"), None, None, &mut state);
    assert!(cleared.contains("Hook 反馈已清除"), "{cleared}");
    assert!(
        cleared.contains("已从本会话清除 1 条反馈偏好。"),
        "{cleared}"
    );
    assert!(cleared.contains("Hook 反馈偏好已清除。"), "{cleared}");
    assert!(!cleared.contains("Hook feedback cleared"), "{cleared}");

    std::env::remove_var("COSH_SHELL_HOOK_FEEDBACK_STORE");
    let _ = std::fs::remove_file(&store);
}
