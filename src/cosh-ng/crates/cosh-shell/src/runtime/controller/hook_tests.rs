use std::fs;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::{render_inline_guidance, shell_has_active_foreground_command};
use crate::runtime::state::InlineState;
use crate::runtime::state::RuntimeHookDisplay;
use cosh_shell::adapter::FakeAgentAdapter;
use cosh_shell::hook_types::FindingSeverity;
use cosh_shell::types::{ShellEvent, ShellEventKind};
use cosh_shell::AdapterInstance;

const TOP_MEMORY_PRESSURE_OUTPUT: &str = "\
top - 04:04:49 up 20:38,  0 user,  load average: 0.31, 0.40, 0.42
MiB Mem :  32768.0 total,   1400.0 free,  30200.0 used,   2188.0 buff/cache
MiB Swap:   8192.0 total,   2992.0 free,   5200.0 used.   1400.0 avail Mem

  PID USER      PR  NI    VIRT    RES    SHR S  %CPU  %MEM     TIME+ COMMAND
 1234 root      20   0 5120000   2.3g  100m S  12.0  45.2   1:23.45 java
";

const PS_HIGH_MEMORY_OUTPUT: &str = "\
USER       PID %CPU %MEM    VSZ   RSS TTY      STAT START   TIME COMMAND
root      1234  3.1 45.2 5120000 2376420 ?     Sl   10:00   1:23 java -jar app.jar
";

const TOP_INTERACTIVE_OUTPUT: &str = "\
\x1b[H\x1b[Jtop - 04:04:49 up 20:38,  0 user,  load average: 0.31, 0.40, 0.42
  PID USER      PR  NI    VIRT    RES    SHR S  %CPU  %MEM     TIME+ COMMAND
 1234 root      20   0 5120000   2.3g  100m S  12.0  45.2   1:23.45 java
";

fn state_with_builtin_hooks() -> InlineState {
    let mut state = InlineState::default();
    let mut hook_engine = cosh_shell::hook_engine::HookEngine::new();
    for hook in cosh_shell::builtin_hooks::default_builtin_hooks() {
        hook_engine.register(hook);
    }
    state.hooks.engine = hook_engine;
    state
}

fn write_hook_output(name: &str, content: &str) -> String {
    let path = std::env::temp_dir().join(format!(
        "cosh-shell-hook-{name}-{}-{}.txt",
        std::process::id(),
        content.len()
    ));
    fs::write(&path, content).expect("write hook output");
    path.to_string_lossy().to_string()
}

#[cfg(unix)]
fn unique_hook_dir(name: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "cosh-shell-runtime-hook-{name}-{}-{nanos}",
        std::process::id()
    ))
}

#[cfg(unix)]
fn write_executable_hook_at(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let _ = fs::remove_dir_all(dir);
    fs::create_dir_all(dir).expect("create hook dir");
    let path = dir.join("hook.sh");
    fs::write(&path, body).expect("write hook script");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod hook script");
    path
}

#[cfg(unix)]
fn write_executable_hook(name: &str, body: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = unique_hook_dir(name);
    let path = write_executable_hook_at(&dir, body);
    (dir, path)
}

fn command_events(command: &str, output_ref: &str, output_bytes: u64) -> Vec<ShellEvent> {
    let mut finished = ShellEvent::command_finished(
        ShellEventKind::CommandCompleted,
        "test-session",
        "cmd-1",
        0,
        200,
        output_ref,
    );
    finished.terminal_output_bytes = Some(output_bytes);
    vec![
        ShellEvent::command_started("test-session", "cmd-1", command, "/tmp", 100),
        finished,
    ]
}

#[test]
fn inline_natural_language_intercept_waits_for_open_command_to_finish() {
    let mut intercept = ShellEvent::user_input_intercepted("test-session", "你好");
    intercept.component = Some("natural_language".to_string());
    intercept.started_at_ms = Some(120);
    let mut events = vec![
        ShellEvent::command_started("test-session", "cmd-1", "sleep 30", "/tmp", 100),
        intercept,
    ];

    let mut state = InlineState::default();
    let mut output = Vec::new();
    let adapter = AdapterInstance::Fake(FakeAgentAdapter);
    render_inline_guidance(&events, &adapter, "bash", &mut state, &mut output)
        .expect("render inline guidance");

    assert!(output.is_empty());
    assert!(shell_has_active_foreground_command(&events));

    events.push(ShellEvent::command_finished(
        ShellEventKind::CommandCompleted,
        "test-session",
        "cmd-1",
        0,
        200,
        "terminal://test/cmd-1",
    ));
    render_inline_guidance(&events, &adapter, "bash", &mut state, &mut output)
        .expect("render inline guidance after command");
    std::thread::sleep(Duration::from_millis(20));
    render_inline_guidance(&events, &adapter, "bash", &mut state, &mut output)
        .expect("poll inline guidance after adapter event");

    let rendered = String::from_utf8(output).expect("utf8 output");
    assert!(rendered.contains("Thinking..."));
    assert!(rendered.contains("Received shell prompt request: 你好"));
    assert!(!shell_has_active_foreground_command(&events));
}

#[test]
fn smart_mode_top_memory_finding_renders_consultation_card() {
    let output_ref = write_hook_output("top-card", TOP_MEMORY_PRESSURE_OUTPUT);
    let events = command_events(
        "top -b -n1 | head -20",
        &output_ref,
        TOP_MEMORY_PRESSURE_OUTPUT.len() as u64,
    );
    let mut state = state_with_builtin_hooks();
    let adapter = AdapterInstance::Fake(FakeAgentAdapter);
    let mut output = Vec::new();

    render_inline_guidance(&events, &adapter, "bash", &mut state, &mut output)
        .expect("queue hook card");

    assert!(state.hooks.pending_consultation.is_none());
    assert_eq!(state.hooks.pending_consultation_queue.len(), 1);
    assert!(output.is_empty());

    std::thread::sleep(Duration::from_millis(300));
    render_inline_guidance(&events, &adapter, "bash", &mut state, &mut output)
        .expect("render queued hook card");

    let rendered = String::from_utf8(output).expect("utf8 output");
    assert!(rendered.contains("Hook: memory-pressure"), "{rendered}");
    assert!(
        rendered.contains("Confidence: high; reason: allowed"),
        "{rendered}"
    );
    assert!(rendered.contains("[Analyze] [Ignore]"), "{rendered}");
    assert!(
        rendered.contains("[Details] hook-cmd-1-memory-pressure"),
        "{rendered}"
    );
    assert!(state.hooks.pending_consultation.is_some());
    assert_eq!(state.hooks.pending_consultation_queue.len(), 0);
}

#[test]
fn smart_mode_success_finding_after_next_input_is_silent_without_card() {
    let output_ref = write_hook_output("top-continued-input", TOP_MEMORY_PRESSURE_OUTPUT);
    let mut events = command_events(
        "top -b -n1 | head -20",
        &output_ref,
        TOP_MEMORY_PRESSURE_OUTPUT.len() as u64,
    );
    events.push(ShellEvent::user_input_intercepted(
        "test-session",
        "what happened",
    ));
    let mut state = state_with_builtin_hooks();
    let adapter = AdapterInstance::Fake(FakeAgentAdapter);
    let mut output = Vec::new();

    render_inline_guidance(&events, &adapter, "bash", &mut state, &mut output)
        .expect("record silent hook finding");

    let rendered = String::from_utf8(output).expect("utf8 output");
    assert!(!rendered.contains("Hook finding"), "{rendered}");
    assert!(!rendered.contains("[Analyze] [Ignore]"), "{rendered}");
    assert!(state.hooks.pending_consultation.is_none());
    assert!(state.hooks.findings.iter().any(|hint| {
        hint.display == RuntimeHookDisplay::Silent && hint.display_reason == "user-continued-input"
    }));
}

#[test]
fn analyze_consultation_starts_agent_with_hook_finding() {
    let output_ref = write_hook_output("top-analyze", TOP_MEMORY_PRESSURE_OUTPUT);
    let mut events = command_events(
        "top -b -n1 | head -20",
        &output_ref,
        TOP_MEMORY_PRESSURE_OUTPUT.len() as u64,
    );
    let mut state = state_with_builtin_hooks();
    let adapter = AdapterInstance::Fake(FakeAgentAdapter);
    let mut output = Vec::new();

    render_inline_guidance(&events, &adapter, "bash", &mut state, &mut output)
        .expect("queue hook card");
    std::thread::sleep(Duration::from_millis(300));
    render_inline_guidance(&events, &adapter, "bash", &mut state, &mut output)
        .expect("render queued hook card");
    let card_id = state
        .hooks
        .pending_consultation
        .as_ref()
        .expect("pending consultation")
        .card_id
        .clone();
    let mut approve = ShellEvent::user_input_intercepted("test-session", card_id);
    approve.component = Some("card".to_string());
    approve.message = Some("approve".to_string());
    approve.started_at_ms = Some(220);
    events.push(approve);

    render_inline_guidance(&events, &adapter, "bash", &mut state, &mut output)
        .expect("handle hook analyze");
    std::thread::sleep(Duration::from_millis(20));
    render_inline_guidance(&events, &adapter, "bash", &mut state, &mut output)
        .expect("poll hook analyze");

    let rendered = String::from_utf8(output).expect("utf8 output");
    assert!(rendered.contains("Agent:"), "{rendered}");
    assert!(
        rendered.contains("Evidence excerpt received by fake adapter"),
        "{rendered}"
    );
    let normalized = rendered.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(!rendered.contains("Inspect output_ref"), "{rendered}");
    assert!(normalized.contains("MiB Mem"), "{rendered}");
    assert!(normalized.contains("java"), "{rendered}");
    let hook_hint = state
        .hooks
        .findings
        .iter()
        .find(|hint| hint.id == "hook-cmd-1-memory-pressure")
        .expect("memory-pressure hook finding");
    assert_eq!(
        hook_hint.related_hook_ids,
        vec!["high-memory-process".to_string()]
    );
    assert_eq!(hook_hint.topic, "memory");
    assert_eq!(hook_hint.entity_key, "system-memory");
    assert_eq!(hook_hint.effective_severity, FindingSeverity::Critical);
    assert_eq!(hook_hint.suppression_key, "memory:memory-pressure:top");
    assert_eq!(
        hook_hint.recommended_skill.as_deref(),
        Some("memory-analysis")
    );
    assert_eq!(hook_hint.output_ref.as_deref(), Some(output_ref.as_str()));
}

#[test]
fn ignore_consultation_does_not_start_agent() {
    let output_ref = write_hook_output("top-ignore", TOP_MEMORY_PRESSURE_OUTPUT);
    let mut events = command_events(
        "top -b -n1 | head -20",
        &output_ref,
        TOP_MEMORY_PRESSURE_OUTPUT.len() as u64,
    );
    let mut state = state_with_builtin_hooks();
    let adapter = AdapterInstance::Fake(FakeAgentAdapter);
    let mut output = Vec::new();

    render_inline_guidance(&events, &adapter, "bash", &mut state, &mut output)
        .expect("queue hook card");
    std::thread::sleep(Duration::from_millis(300));
    render_inline_guidance(&events, &adapter, "bash", &mut state, &mut output)
        .expect("render queued hook card");
    let card_id = state
        .hooks
        .pending_consultation
        .as_ref()
        .expect("pending consultation")
        .card_id
        .clone();
    let mut deny = ShellEvent::user_input_intercepted("test-session", card_id);
    deny.component = Some("card".to_string());
    deny.message = Some("deny".to_string());
    deny.started_at_ms = Some(220);
    events.push(deny);

    render_inline_guidance(&events, &adapter, "bash", &mut state, &mut output)
        .expect("handle hook ignore");

    assert!(state.agent_run.active.is_none());
    assert!(state.hooks.pending_consultation.is_none());
}

#[test]
fn smart_mode_ps_warning_renders_hint_without_card() {
    let output_ref = write_hook_output("ps-hint", PS_HIGH_MEMORY_OUTPUT);
    let events = command_events(
        "ps aux --sort=-%mem | head",
        &output_ref,
        PS_HIGH_MEMORY_OUTPUT.len() as u64,
    );
    let mut state = state_with_builtin_hooks();
    let adapter = AdapterInstance::Fake(FakeAgentAdapter);
    let mut output = Vec::new();

    render_inline_guidance(&events, &adapter, "bash", &mut state, &mut output)
        .expect("render hook hint");

    let rendered = String::from_utf8(output).expect("utf8 output");
    assert!(rendered.contains("Hook finding"), "{rendered}");
    assert!(
        rendered.contains("Use /hooks analyze|ignore|details hook-cmd-1-high-memory-process."),
        "{rendered}"
    );
    assert!(!rendered.contains("[Analyze] [Ignore]"), "{rendered}");
    assert!(state.hooks.pending_consultation.is_none());
}

#[cfg(unix)]
#[test]
fn smart_mode_external_warning_finding_uses_interruption_policy() {
    let (dir, hook_path) = write_executable_hook(
            "external-warning",
            "#!/bin/sh\nprintf '{\"hook_id\":\"external-warning\",\"severity\":\"warning\",\"title\":\"External warning\",\"description\":\"External warning description\",\"suggestion\":\"Inspect external warning\"}'\n",
        );
    let output_ref = write_hook_output("external-warning-output", "ok\n");
    let events = command_events("echo hi", &output_ref, 3);
    let mut state = InlineState::default();
    let mut hook_engine = cosh_shell::hook_engine::HookEngine::new();
    hook_engine.register_external(cosh_shell::hook_engine::ExternalHookConfig {
        path: hook_path,
        matcher: cosh_shell::hook_types::HookMatcher {
            id: "external-warning".to_string(),
            commands: vec!["echo".to_string()],
            command_patterns: Vec::new(),
            command_regex: None,
            exit_codes: Some(vec![0]),
            min_output_bytes: None,
            trigger: cosh_shell::hook_types::HookTrigger::OnSuccess,
        },
        timeout_ms: 3000,
        source: cosh_shell::hook_engine::ExternalHookSource::User,
        project_root: None,
        trusted: true,
    });
    state.hooks.engine = hook_engine;
    let adapter = AdapterInstance::Fake(FakeAgentAdapter);
    let mut output = Vec::new();

    render_inline_guidance(&events, &adapter, "bash", &mut state, &mut output)
        .expect("render external hook hint");

    let rendered = String::from_utf8(output).expect("utf8 output");
    assert!(rendered.contains("Hook finding"), "{rendered}");
    assert!(rendered.contains("external-warning"), "{rendered}");
    assert!(!rendered.contains("[Analyze] [Ignore]"), "{rendered}");
    assert_eq!(state.hooks.findings.len(), 1);
    let hint = &state.hooks.findings[0];
    assert_eq!(hint.topic, "external");
    assert_eq!(hint.display.label(), "hint");
    assert_eq!(hint.display_reason, "allowed");

    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn smart_mode_untrusted_project_hook_is_not_executed() {
    let dir = unique_hook_dir("project-untrusted-warning");
    let marker = dir.join("executed.marker");
    let body = format!(
        "#!/bin/sh\ntouch '{}'\nprintf '{{\"hook_id\":\"project-warning\",\"severity\":\"warning\",\"title\":\"Project warning\",\"description\":\"Project warning description\",\"suggestion\":\"Inspect project warning\"}}'\n",
        marker.display()
    );
    let hook_path = write_executable_hook_at(&dir, &body);
    let output_ref = write_hook_output("project-untrusted-output", "ok\n");
    let events = command_events("echo hi", &output_ref, 3);
    let mut state = InlineState::default();
    let mut hook_engine = cosh_shell::hook_engine::HookEngine::new();
    hook_engine.register_external(cosh_shell::hook_engine::ExternalHookConfig {
        path: hook_path,
        matcher: cosh_shell::hook_types::HookMatcher {
            id: "project-warning".to_string(),
            commands: vec!["echo".to_string()],
            command_patterns: Vec::new(),
            command_regex: None,
            exit_codes: Some(vec![0]),
            min_output_bytes: None,
            trigger: cosh_shell::hook_types::HookTrigger::OnSuccess,
        },
        timeout_ms: 3000,
        source: cosh_shell::hook_engine::ExternalHookSource::Project,
        project_root: Some(dir.clone()),
        trusted: false,
    });
    state.hooks.engine = hook_engine;
    let adapter = AdapterInstance::Fake(FakeAgentAdapter);
    let mut output = Vec::new();

    render_inline_guidance(&events, &adapter, "bash", &mut state, &mut output)
        .expect("render untrusted project hook");

    let rendered = String::from_utf8(output).expect("utf8 output");
    assert!(!rendered.contains("Hook finding"), "{rendered}");
    assert!(state.hooks.findings.is_empty());
    assert!(!marker.exists());

    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn smart_mode_trusted_project_warning_finding_uses_interruption_policy() {
    let dir = unique_hook_dir("project-trusted-warning");
    let marker = dir.join("executed.marker");
    let body = format!(
        "#!/bin/sh\ntouch '{}'\nprintf '{{\"hook_id\":\"project-warning\",\"severity\":\"warning\",\"title\":\"Project warning\",\"description\":\"Project warning description\",\"suggestion\":\"Inspect project warning\"}}'\n",
        marker.display()
    );
    let hook_path = write_executable_hook_at(&dir, &body);
    let output_ref = write_hook_output("project-trusted-output", "ok\n");
    let events = command_events("echo hi", &output_ref, 3);
    let mut state = InlineState::default();
    let mut hook_engine = cosh_shell::hook_engine::HookEngine::new();
    hook_engine.register_external(cosh_shell::hook_engine::ExternalHookConfig {
        path: hook_path,
        matcher: cosh_shell::hook_types::HookMatcher {
            id: "project-warning".to_string(),
            commands: vec!["echo".to_string()],
            command_patterns: Vec::new(),
            command_regex: None,
            exit_codes: Some(vec![0]),
            min_output_bytes: None,
            trigger: cosh_shell::hook_types::HookTrigger::OnSuccess,
        },
        timeout_ms: 3000,
        source: cosh_shell::hook_engine::ExternalHookSource::Project,
        project_root: Some(dir.clone()),
        trusted: true,
    });
    state.hooks.engine = hook_engine;
    let adapter = AdapterInstance::Fake(FakeAgentAdapter);
    let mut output = Vec::new();

    render_inline_guidance(&events, &adapter, "bash", &mut state, &mut output)
        .expect("render trusted project hook");

    let rendered = String::from_utf8(output).expect("utf8 output");
    assert!(rendered.contains("Hook finding"), "{rendered}");
    assert!(rendered.contains("project-warning"), "{rendered}");
    assert!(!rendered.contains("[Analyze] [Ignore]"), "{rendered}");
    assert_eq!(state.hooks.findings.len(), 1);
    let hint = &state.hooks.findings[0];
    assert_eq!(hint.topic, "external");
    assert_eq!(hint.display.label(), "hint");
    assert_eq!(hint.display_reason, "allowed");
    assert!(marker.exists());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn smart_mode_interactive_top_renders_sampling_hint_without_card() {
    let output_ref = write_hook_output("top-interactive-hint", TOP_INTERACTIVE_OUTPUT);
    let events = command_events("top", &output_ref, TOP_INTERACTIVE_OUTPUT.len() as u64);
    let mut state = state_with_builtin_hooks();
    let adapter = AdapterInstance::Fake(FakeAgentAdapter);
    let mut output = Vec::new();

    render_inline_guidance(&events, &adapter, "bash", &mut state, &mut output)
        .expect("render interactive top hint");

    let rendered = String::from_utf8(output).expect("utf8 output");
    assert!(rendered.contains("interactive-top-guidance"), "{rendered}");
    assert!(rendered.contains("top"), "{rendered}");
    assert!(rendered.contains("-b"), "{rendered}");
    assert!(rendered.contains("-n1"), "{rendered}");
    assert!(rendered.contains("%MEM"), "{rendered}");
    assert!(rendered.contains("head -30"), "{rendered}");
    assert!(!rendered.contains("[Analyze] [Ignore]"), "{rendered}");
    assert!(state.hooks.pending_consultation.is_none());
}

#[test]
fn disabled_hook_is_not_evaluated() {
    let output_ref = write_hook_output("ps-disabled", PS_HIGH_MEMORY_OUTPUT);
    let events = command_events(
        "ps aux --sort=-%mem | head",
        &output_ref,
        PS_HIGH_MEMORY_OUTPUT.len() as u64,
    );
    let mut state = state_with_builtin_hooks();
    state
        .hooks
        .disabled
        .insert("high-memory-process".to_string());
    let adapter = AdapterInstance::Fake(FakeAgentAdapter);
    let mut output = Vec::new();

    render_inline_guidance(&events, &adapter, "bash", &mut state, &mut output)
        .expect("render with disabled hook");

    assert!(state.hooks.findings.is_empty());
    let rendered = String::from_utf8(output).expect("utf8 output");
    assert!(!rendered.contains("Hook finding"), "{rendered}");
}
