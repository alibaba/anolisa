use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use cosh_shell::adapter::FakeAgentAdapter;
use cosh_shell::governance::govern_agent_events;
use cosh_shell::interactive::run_line_interactive_bash;
use cosh_shell::ledger::build_command_blocks;
use cosh_shell::parser::{
    agent_request_after_confirmation, agent_request_confirmed_by_events, findings_from_blocks,
    interventions_from_findings,
};
use cosh_shell::raw_input::{RawInputCapture, RawObserverAction};
use cosh_shell::renderer::render_transcript;
use cosh_shell::shell_host::{
    run_raw_interactive_bash_with_output_control, run_raw_interactive_zsh_with_output_control,
    run_scripted_bash, ScriptedInput, ShellHostConfig,
};
use cosh_shell::{
    agent_render::{NoticePanelModel, RatatuiInlineRenderer},
    types::{Policy, ShellEvent, ShellEventKind},
    AdapterKind, AgentAdapter,
};

use crate::approval::handoff::trust_key_from_command;
use crate::hooks::{
    dirs_for_hook_loading, is_trusted_project_root, load_hook_feedback_preferences_into_state,
    project_hook_root_from_cwd,
};
use crate::question::runtime::pending_question_capture;
use crate::runtime::state::{AnalysisMode, ApprovalRequestStatus, CoshApprovalMode, InlineState};
use crate::{bootstrap_process_path_from_shell, build_adapter, RawShellKind};

use super::dispatcher::RuntimeDispatcher;
use super::events::ShellEventSnapshot;
use super::terminal::CrLfWriter;

pub(crate) fn run_demo() -> i32 {
    let events = demo_events();
    render_loop_from_events(&events)
}

pub(crate) fn run_host_demo() -> i32 {
    let work_dir =
        std::env::temp_dir().join(format!("cosh-shell-host-demo-{}", std::process::id()));
    let _work_dir_cleanup = TempSessionDir::new(work_dir.clone());
    let config = ShellHostConfig::new("host-demo-session", work_dir);
    let inputs = vec![
        ScriptedInput::user_line("/explain last error"),
        ScriptedInput::user_line("echo ok"),
        ScriptedInput::user_line("please analyze the last failure"),
        ScriptedInput::user_line("ls /path/that/does/not/exist"),
    ];

    let output = match run_scripted_bash(&config, &inputs) {
        Ok(output) => output,
        Err(err) => {
            eprintln!("host demo failed: {err}");
            return 1;
        }
    };

    render_loop_from_events(&output.events)
}

pub(crate) fn run_raw(adapter_name: &str, shell_kind: RawShellKind) -> i32 {
    let args = std::env::args().collect::<Vec<_>>();

    let Some(kind) = AdapterKind::parse(adapter_name) else {
        eprintln!("unknown adapter: {adapter_name}");
        return 2;
    };

    let work_dir =
        std::env::temp_dir().join(format!("cosh-shell-raw-session-{}", std::process::id()));
    let _work_dir_cleanup = TempSessionDir::new(work_dir.clone());
    let mut config = ShellHostConfig::new("raw-session", work_dir);

    let isolated = args.iter().any(|a| a == "--isolated")
        || std::env::var("COSH_SHELL_ISOLATED").as_deref() == Ok("1");
    if isolated {
        config.native_mode = false;
    }
    if config.native_mode {
        config.input_classifier = config.input_classifier.with_conservative(true);
    }

    let login = args.first().is_some_and(|a| a.starts_with('-'))
        || args.iter().any(|a| a == "--login" || a == "-l");
    config.login_shell = login;
    if config.native_mode {
        bootstrap_process_path_from_shell(&shell_kind, login);
    }

    let cosh_config = cosh_shell::load_config();
    config.input_classifier = config
        .input_classifier
        .with_ai_enabled(cosh_config.ai_enabled);

    let adapter = build_adapter(kind);
    let mut inline_state = InlineState::with_raw_session_dir(&config.work_dir);
    load_hook_feedback_preferences_into_state(&mut inline_state);
    inline_state.language = cosh_shell::parse_language_setting(&cosh_config.language)
        .map(cosh_shell::resolve_language_setting)
        .unwrap_or_default();
    match cosh_config.analysis_mode.as_str() {
        "auto" => inline_state.analysis_mode = AnalysisMode::Auto,
        "manual" => inline_state.analysis_mode = AnalysisMode::Manual,
        _ => {}
    }
    inline_state.debug = cosh_config.debug;
    inline_state.approval_mode = approval_mode_from_config(&cosh_config.approval_mode);
    for cmd in &cosh_config.trusted_commands {
        if let Some(key) = trust_key_from_command(cmd) {
            inline_state.control.trust_session_command(key);
        }
    }
    cosh_shell::tools::apply_readonly_config(&cosh_config);
    inline_state.hooks.engine = load_hook_engine(&cosh_config);

    let raw_result = match shell_kind {
        RawShellKind::Bash => {
            run_raw_interactive_bash_with_output_control(&config, |events, output| {
                render_raw_inline_events(events, output, &adapter, "bash", &mut inline_state)
            })
        }
        RawShellKind::Zsh => {
            run_raw_interactive_zsh_with_output_control(&config, |events, output| {
                render_raw_inline_events(events, output, &adapter, "zsh", &mut inline_state)
            })
        }
        RawShellKind::MissingShellValue => {
            eprintln!("missing value for --shell; supported shells: bash, zsh");
            return 2;
        }
        RawShellKind::Unsupported(shell) => {
            eprintln!("unsupported raw shell: {shell}; supported shells: bash, zsh");
            return 2;
        }
    };

    match raw_result {
        Ok(output) => output.exit_status.unwrap_or(0),
        Err(err) => {
            eprintln!("raw shell failed: {err}");
            1
        }
    }
}

pub(crate) fn run_interactive(adapter_name: &str) -> i32 {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    run_interactive_from_reader(
        "interactive-session",
        adapter_name,
        stdin.lock(),
        &mut stdout,
    )
}

pub(crate) fn run_interactive_demo(adapter_name: &str) -> i32 {
    let input = std::io::Cursor::new(
        "/explain last error\n\
         echo ok\n\
         please analyze the last failure\n\
         ls /path/that/does/not/exist\n",
    );
    let mut output = Vec::new();
    run_interactive_from_reader("interactive-demo-session", adapter_name, input, &mut output)
}

fn run_interactive_from_reader<R, W>(
    session_id: &str,
    adapter_name: &str,
    input: R,
    output: &mut W,
) -> i32
where
    R: std::io::BufRead,
    W: std::io::Write,
{
    let Some(kind) = AdapterKind::parse(adapter_name) else {
        eprintln!("unknown adapter: {adapter_name}");
        return 2;
    };

    let work_dir =
        std::env::temp_dir().join(format!("cosh-shell-{session_id}-{}", std::process::id()));
    let _work_dir_cleanup = TempSessionDir::new(work_dir.clone());
    let config = ShellHostConfig::new(session_id, work_dir);
    let shell_output = match run_line_interactive_bash(&config, input, output) {
        Ok(output) => output,
        Err(err) => {
            eprintln!("interactive demo failed: {err}");
            return 1;
        }
    };

    render_loop_from_events_with_adapter(&shell_output.shell.events, &build_adapter(kind))
}

pub(crate) fn run_adapter_demo(adapter_name: &str) -> i32 {
    let Some(kind) = AdapterKind::parse(adapter_name) else {
        eprintln!("unknown adapter: {adapter_name}");
        return 2;
    };
    let events = demo_events();
    render_loop_from_events_with_adapter(&events, &build_adapter(kind))
}

fn render_loop_from_events(events: &[ShellEvent]) -> i32 {
    render_loop_from_events_with_adapter(events, &FakeAgentAdapter)
}

struct TempSessionDir {
    path: PathBuf,
}

impl TempSessionDir {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for TempSessionDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn render_loop_from_events_with_adapter(events: &[ShellEvent], adapter: &impl AgentAdapter) -> i32 {
    let ledger = build_command_blocks(events);
    if !ledger.errors.is_empty() {
        eprintln!("ledger errors: {}", ledger.errors.join(", "));
        return 1;
    }

    let Some(block) = ledger.blocks.iter().find(|block| block.exit_code != 0) else {
        println!("No failed command found; no Agent intervention needed");
        return 0;
    };

    let findings = findings_from_blocks(&ledger.blocks);
    let interventions = interventions_from_findings(&findings);
    let user_confirmed = agent_request_confirmed_by_events(events);
    let governed_events = if user_confirmed {
        let Some(request) =
            agent_request_after_confirmation("demo-session", block, &findings, true)
        else {
            eprintln!("agent request was not confirmed");
            return 1;
        };
        let agent_events = match adapter.run(&request) {
            Ok(events) => events,
            Err(err) => {
                eprintln!("adapter failed: {}", err.message);
                return 1;
            }
        };
        govern_agent_events(&agent_events, &Policy::default()).events
    } else {
        Vec::new()
    };

    for line in render_transcript(block, &findings, &interventions, &governed_events) {
        println!("{line}");
    }

    if !user_confirmed {
        println!("Enter a slash command or natural-language request to ask for Agent analysis");
    }

    0
}

fn demo_events() -> Vec<ShellEvent> {
    vec![
        ShellEvent::user_input_intercepted("demo-session", "/explain last error"),
        ShellEvent::command_started("demo-session", "cmd-1", "missing-command", "/tmp", 100),
        ShellEvent::command_finished(
            ShellEventKind::CommandFailed,
            "demo-session",
            "cmd-1",
            127,
            140,
            "terminal://demo/cmd-1",
        ),
    ]
}

fn load_hook_engine(cosh_config: &cosh_shell::CoshConfig) -> cosh_shell::hook_engine::HookEngine {
    let mut hook_engine = cosh_shell::hook_engine::HookEngine::new();
    for hook in cosh_shell::builtin_hooks::default_builtin_hooks() {
        hook_engine.register(hook);
    }
    if let Some(hooks_dir) = dirs_for_hook_loading() {
        hook_engine.load_hooks_from_dir(&hooks_dir);
    }
    if let Ok(cwd) = std::env::current_dir() {
        if let Some(project_root) = project_hook_root_from_cwd(&cwd) {
            let trusted = is_trusted_project_root(
                &project_root,
                cosh_config.trusted_project_roots.as_slice(),
            );
            hook_engine.load_project_hooks_from_root(&project_root, trusted);
        }
    }
    hook_engine
}

fn render_raw_inline_events<W: Write>(
    events: &[ShellEvent],
    output: &mut W,
    adapter: &cosh_shell::AdapterInstance,
    shell_label: &str,
    inline_state: &mut InlineState,
) -> std::io::Result<RawObserverAction> {
    let mut terminal_output = CrLfWriter::new(output);
    let snapshot = ShellEventSnapshot::new(events);
    let actions = RuntimeDispatcher::dispatch_inline_batch(
        &snapshot,
        adapter,
        shell_label,
        inline_state,
        &mut terminal_output,
    )?;
    RuntimeDispatcher::apply_actions(actions, inline_state);
    if let Some(request) = inline_state
        .control
        .shell_handoff_mut()
        .emit_next_approved()
    {
        return Ok(RawObserverAction::EmitToPty(request));
    }
    if let Some(capture) = pending_card_capture(inline_state) {
        return Ok(RawObserverAction::CaptureInput(capture));
    }
    if inline_state.trigger_pty_prompt {
        inline_state.trigger_pty_prompt = false;
        return Ok(RawObserverAction::RestorePrompt);
    }
    let shell_busy = shell_has_active_foreground_command(snapshot.events());
    let host_shell_handoff_in_progress = inline_state.control.shell_handoff().has_pending();
    if let Some(action) =
        shell_handoff_timeout_recovery_action(inline_state, shell_busy, &mut terminal_output)?
    {
        return Ok(action);
    }
    if host_shell_handoff_in_progress && shell_busy {
        Ok(RawObserverAction::RawPassthrough)
    } else if inline_state
        .agent_run
        .active
        .as_ref()
        .is_some_and(|run| !run.completed)
    {
        Ok(RawObserverAction::DelayShellOutput)
    } else if shell_busy {
        Ok(RawObserverAction::RawPassthrough)
    } else {
        Ok(RawObserverAction::Continue)
    }
}

fn shell_handoff_timeout_recovery_action<W: Write>(
    state: &mut InlineState,
    shell_busy: bool,
    output: &mut W,
) -> std::io::Result<Option<RawObserverAction>> {
    if !shell_busy {
        return Ok(None);
    }
    let Some(timeout) = configured_shell_handoff_timeout() else {
        return Ok(None);
    };
    if !state
        .control
        .shell_handoff_mut()
        .mark_timeout_interrupt_if_elapsed(timeout)
    {
        return Ok(None);
    }
    let i18n = state.i18n();
    let timeout_secs = timeout.as_secs().to_string();
    RatatuiInlineRenderer::for_terminal()
        .with_language(state.language)
        .write_notice_panel(
            output,
            NoticePanelModel {
                title: i18n.t(cosh_shell::MessageId::ApprovalShellHandoffTimeoutTitle),
                body: vec![
                    i18n.format(
                        cosh_shell::MessageId::ApprovalShellHandoffTimeoutExceededBody,
                        &[("seconds", &timeout_secs)],
                    ),
                    i18n.t(cosh_shell::MessageId::ApprovalShellHandoffTimeoutInterruptBody)
                        .to_string(),
                ],
                footer: None,
            },
        )?;
    Ok(Some(RawObserverAction::InterruptForeground))
}

fn configured_shell_handoff_timeout() -> Option<Duration> {
    let secs = std::env::var("COSH_SHELL_HANDOFF_TIMEOUT_SECS")
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?;
    (secs > 0).then(|| Duration::from_secs(secs))
}

#[cfg(test)]
pub(crate) fn render_inline_guidance<W: Write>(
    events: &[ShellEvent],
    adapter: &cosh_shell::AdapterInstance,
    shell_label: &str,
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let snapshot = ShellEventSnapshot::new(events);
    let previous_cursor = state.control.event_cursor();
    state.control.set_event_cursor(Default::default());
    let actions =
        RuntimeDispatcher::dispatch_inline_batch(&snapshot, adapter, shell_label, state, output)?;
    RuntimeDispatcher::apply_actions(actions, state);
    state.control.set_event_cursor(previous_cursor);
    Ok(())
}

fn approval_mode_from_config(value: &str) -> CoshApprovalMode {
    match value {
        "recommend" | "suggest" => CoshApprovalMode::Recommend,
        "trust" => CoshApprovalMode::Trust,
        _ => CoshApprovalMode::Auto,
    }
}

pub(crate) fn pending_card_capture(state: &InlineState) -> Option<RawInputCapture> {
    if let Some(mode_panel) = state.control.pending_mode_panel() {
        return Some(RawInputCapture::Mode {
            id: mode_panel.id.clone(),
            option_count: 3,
            selected: mode_panel.selected_option,
        });
    }
    if let Some(config_panel) = state.control.pending_config_panel() {
        return Some(RawInputCapture::Config {
            id: config_panel.id.clone(),
            option_count: 2,
            selected: config_panel.selected_option,
        });
    }
    if let Some(config_language_panel) = state.control.pending_config_language_panel() {
        return Some(RawInputCapture::ConfigLanguage {
            id: config_language_panel.id.clone(),
            option_count: 3,
            selected: config_language_panel.selected_option,
        });
    }

    if state.agent_run.active.is_none() {
        if let Some(consultation) = state.hooks.pending_consultation.as_ref() {
            return Some(RawInputCapture::Consultation {
                id: consultation.card_id.clone(),
            });
        }
    }

    if let Some(capture) = pending_question_capture(state) {
        return Some(capture);
    }

    if let Some(capture) = crate::auth::runtime::pending_auth_capture(state) {
        return Some(capture);
    }

    if let Some(capture) = crate::runtime::evidence_requests::pending_evidence_capture(state) {
        return Some(capture);
    }

    state
        .approvals
        .requests
        .iter()
        .find(|request| request.status == ApprovalRequestStatus::Pending)
        .map(|request| RawInputCapture::Approval {
            id: request.id.clone(),
        })
}

pub(crate) fn shell_has_active_foreground_command(events: &[ShellEvent]) -> bool {
    let mut active = std::collections::HashSet::new();
    for event in events {
        let Some(command_id) = event.command_id.as_ref() else {
            continue;
        };

        match event.kind {
            ShellEventKind::CommandStarted => {
                active.insert(command_id.as_str());
            }
            ShellEventKind::CommandCompleted | ShellEventKind::CommandFailed => {
                active.remove(command_id.as_str());
            }
            _ => {}
        }
    }

    !active.is_empty()
}

#[cfg(test)]
mod hook_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_mode_config_keeps_legacy_suggest_as_recommend() {
        assert_eq!(
            approval_mode_from_config("recommend"),
            CoshApprovalMode::Recommend
        );
        assert_eq!(
            approval_mode_from_config("suggest"),
            CoshApprovalMode::Recommend
        );
        assert_eq!(approval_mode_from_config("trust"), CoshApprovalMode::Trust);
        assert_eq!(approval_mode_from_config("auto"), CoshApprovalMode::Auto);
        assert_eq!(approval_mode_from_config("unknown"), CoshApprovalMode::Auto);
    }

    #[test]
    fn temp_session_dir_guard_removes_session_directory_on_drop() {
        let dir = std::env::temp_dir().join(format!(
            "cosh-shell-temp-session-cleanup-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);

        {
            let _cleanup = TempSessionDir::new(dir.clone());
            fs::create_dir_all(dir.join("output-refs")).expect("create output refs");
            fs::write(dir.join("history"), "echo ok\n").expect("write history");
            fs::write(dir.join("output-refs/cmd-1.txt"), "ok\n").expect("write output ref");
        }

        assert!(!dir.exists(), "temp session dir should be removed on drop");
    }
}
