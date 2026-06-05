use std::collections::HashSet;
use std::io::Write;

use nix::libc;

mod activity_runtime;
mod agent_run_runtime;
mod approval_runtime;
mod approved_tool_runtime;
mod command_hook_runtime;
mod details_runtime;
mod failed_command_runtime;
mod intercept_runtime;
mod mode_runtime;
mod question_runtime;
mod recommendation_runtime;
mod runtime_state;
mod slash_runtime;
mod startup_runtime;
mod terminal_output;

use activity_runtime::{
    next_activity_id, record_activity_rows, render_activity_rows, write_tool_output_ref,
    ActivityKind, RuntimeActivityRow,
};
use agent_run_runtime::{
    flush_held_agent_events, poll_active_agent_run, start_agent_run,
    start_agent_run_before_held_text, stop_active_agent_run_without_rendering,
};
use approval_runtime::{
    approval_request_from_governed_event, record_approval_requests, record_auto_approved_request,
    render_approval_actions, render_approval_requests, render_approval_resolution,
};
use approved_tool_runtime::{
    render_approved_tool_result, request_is_executable_bash_tool, request_is_readonly_builtin_tool,
};
use command_hook_runtime::{
    command_hook_hints_for_block, command_hook_hints_for_blocks, record_command_result_hooks,
    render_command_hook_findings,
};
use details_runtime::render_runtime_details;
use failed_command_runtime::{
    block_end_event_index, latest_pending_failed_block_before_event, render_post_failure_actions,
    should_analyze_failed_block, start_agent_for_block,
};
use intercept_runtime::render_intercept_agent_guidance;
use question_runtime::{
    agent_request_from_pending_question_answer, has_pending_question, pending_question_capture,
    record_user_questions, render_question_answer_actions, render_question_answer_notice,
    render_question_focus_actions, render_question_input_actions, render_question_toggle_actions,
    render_user_questions,
};
use recommendation_runtime::{
    record_selectable_recommendations, render_selectable_recommendations, render_selection_actions,
};
use runtime_state::{
    AnalysisMode, ApprovalMode, ApprovalRequestKind, ApprovalRequestStatus, InlineState,
    RuntimeApprovalJournalEntry, RuntimeApprovalRequest, RuntimeCommandHookHint,
};
use slash_runtime::render_slash_actions;
use startup_runtime::render_startup_banner;
use terminal_output::CrLfWriter;

use cosh_shell::{
    adapter_for_kind,
    agent_render::{
        ApprovalDetailsPanelModel, ApprovalJournalEntryModel, ApprovalJournalPanelModel,
        ApprovalPanelAction, ApprovalPanelModel, ApprovalReceiptPanelModel,
    },
    agent_request_after_confirmation, agent_request_confirmed_by_events,
    agent_request_from_intercepted_input, approval_command_from_event, build_command_blocks,
    can_run_approved_bash_tool, event_cancels_failed_command_analysis,
    event_confirms_failed_command_analysis, event_requests_agent_cancel, findings_from_blocks,
    govern_agent_events, interventions_from_findings, render_transcript, run_line_interactive_bash,
    run_raw_interactive_bash_with_output_control, run_raw_interactive_zsh_with_output_control,
    run_scripted_bash, AdapterInstance, AdapterKind, AgentAdapter, AgentEvent, AgentMode,
    AgentRequest, ApprovalCommandKind, CommandBlock, CommandStatus, FakeAgentAdapter, Finding,
    GovernedEvent, OutputRefs, Policy, RatatuiInlineRenderer, RawInputCapture, RawObserverAction,
    ScriptedInput, ShellEvent, ShellEventKind, ShellHostConfig, ToolExecutionResult,
    ToolExecutionStatus,
};

static mut ORIGINAL_TERMIOS: Option<libc::termios> = None;

fn install_terminal_recovery() {
    let fd = libc::STDIN_FILENO;
    if unsafe { libc::isatty(fd) } != 1 {
        return;
    }
    let mut original = unsafe { std::mem::zeroed::<libc::termios>() };
    if unsafe { libc::tcgetattr(fd, &mut original) } < 0 {
        return;
    }
    unsafe { ORIGINAL_TERMIOS = Some(original) };

    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        prev_hook(info);
    }));

    unsafe {
        libc::signal(libc::SIGTERM, restore_and_exit as *const () as libc::sighandler_t);
        libc::signal(libc::SIGHUP, restore_and_exit as *const () as libc::sighandler_t);
        libc::signal(libc::SIGQUIT, restore_and_exit as *const () as libc::sighandler_t);
    }
}

fn restore_terminal() {
    unsafe {
        if let Some(ref original) = ORIGINAL_TERMIOS {
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, original);
        }
    }
}

extern "C" fn restore_and_exit(sig: libc::c_int) {
    restore_terminal();
    unsafe {
        libc::signal(sig, libc::SIG_DFL);
        libc::raise(sig);
    }
}

fn main() {
    let args = std::env::args().collect::<Vec<_>>();

    if args.iter().any(|a| a == "--version") {
        println!("cosh-shell {}", env!("CARGO_PKG_VERSION"));
        std::process::exit(0);
    }
    if args.iter().any(|a| a == "--help") {
        print_usage_help();
        std::process::exit(0);
    }

    install_terminal_recovery();

    let has_subcommand = matches!(
        args.get(1).map(String::as_str),
        Some("demo" | "host-demo" | "raw" | "interactive" | "interactive-demo" | "adapter-demo")
    );
    if !has_subcommand {
        if let Some(status) = passthrough_non_interactive(&args) {
            std::process::exit(status);
        }
    }

    let status = match args.get(1).map(String::as_str) {
        Some("demo") => run_demo(),
        Some("host-demo") => run_host_demo(),
        Some("raw") => run_raw(
            adapter_name_from_args(&args[2..]).unwrap_or("fake"),
            args.iter().any(|arg| arg == "--run"),
            raw_shell_from_args_or_env(&args[2..]),
        ),
        Some("interactive") => run_interactive(
            args.get(2).map(String::as_str).unwrap_or("fake"),
            args.iter().any(|arg| arg == "--run"),
        ),
        Some("interactive-demo") => run_interactive_demo(
            args.get(2).map(String::as_str).unwrap_or("fake"),
            args.iter().any(|arg| arg == "--run"),
        ),
        Some("adapter-demo") => run_adapter_demo(
            args.get(2).map(String::as_str).unwrap_or("fake"),
            args.iter().any(|arg| arg == "--run"),
        ),
        _ => {
            eprintln!(
                "usage: cosh-shell <demo|host-demo|raw|interactive|interactive-demo|adapter-demo [fake|claude|qwen] [--run] [--shell bash|zsh]>"
            );
            2
        }
    };
    std::process::exit(status);
}

fn run_demo() -> i32 {
    let events = demo_events();
    render_loop_from_events(&events)
}

fn run_host_demo() -> i32 {
    let work_dir =
        std::env::temp_dir().join(format!("cosh-shell-host-demo-{}", std::process::id()));
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum RawShellKind {
    Bash,
    Zsh,
    MissingShellValue,
    Unsupported(String),
}

fn adapter_name_from_args(args: &[String]) -> Option<&str> {
    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
            "--run" => idx += 1,
            "--shell" => idx += 2,
            arg if arg.starts_with("--shell=") => idx += 1,
            arg if arg.starts_with("--") => idx += 1,
            arg => return Some(arg),
        }
    }

    None
}

fn raw_shell_from_args_or_env(args: &[String]) -> RawShellKind {
    if let Some(shell) = raw_shell_from_args(args) {
        return shell;
    }

    std::env::var("COSH_SHELL_RAW_SHELL")
        .ok()
        .as_deref()
        .map(parse_raw_shell)
        .unwrap_or(RawShellKind::Bash)
}

fn raw_shell_from_args(args: &[String]) -> Option<RawShellKind> {
    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
            "--shell" => {
                return Some(match args.get(idx + 1) {
                    Some(value) if !value.starts_with("--") => parse_raw_shell(value),
                    _ => RawShellKind::MissingShellValue,
                });
            }
            arg if arg.starts_with("--shell=") => {
                return Some(parse_raw_shell(arg.trim_start_matches("--shell=")));
            }
            _ => idx += 1,
        }
    }

    None
}

fn parse_raw_shell(value: &str) -> RawShellKind {
    let name = value.rsplit('/').next().unwrap_or(value);
    match name {
        "bash" => RawShellKind::Bash,
        "zsh" => RawShellKind::Zsh,
        other => RawShellKind::Unsupported(other.to_string()),
    }
}

fn run_raw(adapter_name: &str, run_model: bool, shell_kind: RawShellKind) -> i32 {
    let Some(kind) = AdapterKind::parse(adapter_name) else {
        eprintln!("unknown adapter: {adapter_name}");
        return 2;
    };

    let work_dir =
        std::env::temp_dir().join(format!("cosh-shell-raw-session-{}", std::process::id()));
    let config = ShellHostConfig::new("raw-session", work_dir);
    let adapter = build_adapter(kind, run_model);
    let mut inline_state = InlineState::with_raw_session_dir(&config.work_dir);
    let mut hook_engine = cosh_shell::HookEngine::new();
    for hook in cosh_shell::default_builtin_hooks() {
        hook_engine.register(hook);
    }
    inline_state.hook_engine = hook_engine;
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

fn render_raw_inline_events<W: Write>(
    events: &[ShellEvent],
    output: &mut W,
    adapter: &AdapterInstance,
    shell_label: &str,
    inline_state: &mut InlineState,
) -> std::io::Result<RawObserverAction> {
    let mut terminal_output = CrLfWriter::new(output);
    render_inline_guidance(
        events,
        adapter,
        shell_label,
        inline_state,
        &mut terminal_output,
    )?;
    if let Some(capture) = pending_card_capture(inline_state) {
        Ok(RawObserverAction::CaptureInput(capture))
    } else if inline_state.active_run.is_some() {
        Ok(RawObserverAction::DelayShellOutput)
    } else if shell_has_active_foreground_command(events) {
        Ok(RawObserverAction::RawPassthrough)
    } else {
        Ok(RawObserverAction::Continue)
    }
}

fn pending_card_capture(state: &InlineState) -> Option<RawInputCapture> {
    if let Some(mode_panel) = state.pending_mode_panel.as_ref() {
        return Some(RawInputCapture::Mode {
            id: mode_panel.id.clone(),
            option_count: 2,
        });
    }

    if let Some(capture) = pending_question_capture(state) {
        return Some(capture);
    }

    state
        .approval_requests
        .iter()
        .find(|request| request.status == ApprovalRequestStatus::Pending)
        .map(|request| RawInputCapture::Approval {
            id: request.id.clone(),
        })
}

fn run_interactive(adapter_name: &str, run_model: bool) -> i32 {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    run_interactive_from_reader(
        "interactive-session",
        adapter_name,
        run_model,
        stdin.lock(),
        &mut stdout,
    )
}

fn run_interactive_demo(adapter_name: &str, run_model: bool) -> i32 {
    let input = std::io::Cursor::new(
        "/explain last error\n\
         echo ok\n\
         please analyze the last failure\n\
         ls /path/that/does/not/exist\n",
    );
    let mut output = Vec::new();
    run_interactive_from_reader(
        "interactive-demo-session",
        adapter_name,
        run_model,
        input,
        &mut output,
    )
}

fn run_interactive_from_reader<R, W>(
    session_id: &str,
    adapter_name: &str,
    run_model: bool,
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
    let config = ShellHostConfig::new(session_id, work_dir);
    let shell_output = match run_line_interactive_bash(&config, input, output) {
        Ok(output) => output,
        Err(err) => {
            eprintln!("interactive demo failed: {err}");
            return 1;
        }
    };

    render_loop_from_events_with_adapter(
        &shell_output.shell.events,
        &build_adapter(kind, run_model),
    )
}

fn run_adapter_demo(adapter_name: &str, run_model: bool) -> i32 {
    let Some(kind) = AdapterKind::parse(adapter_name) else {
        eprintln!("unknown adapter: {adapter_name}");
        return 2;
    };
    let events = demo_events();
    render_loop_from_events_with_adapter(&events, &build_adapter(kind, run_model))
}

fn build_adapter(kind: AdapterKind, run_model: bool) -> cosh_shell::AdapterInstance {
    let adapter = adapter_for_kind(kind);
    if !run_model {
        return adapter;
    }

    match adapter {
        cosh_shell::AdapterInstance::ClaudeCode(adapter) => {
            cosh_shell::AdapterInstance::ClaudeCode(adapter.with_model_call(true))
        }
        other => other,
    }
}

fn render_loop_from_events(events: &[ShellEvent]) -> i32 {
    render_loop_from_events_with_adapter(events, &FakeAgentAdapter)
}

fn render_inline_guidance<W: Write>(
    events: &[ShellEvent],
    adapter: &AdapterInstance,
    shell_label: &str,
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    state.shell_exited = events
        .iter()
        .any(|event| event.kind == ShellEventKind::ShellExited);
    let shell_busy = shell_has_active_foreground_command(events);
    render_agent_cancel_actions(events, &[], state, output, !shell_busy)?;
    if shell_busy {
        return Ok(());
    }

    render_startup_banner(events, adapter, shell_label, state, output)?;
    render_question_focus_actions(events, state, output)?;
    render_question_toggle_actions(events, state, output)?;
    render_question_input_actions(events, state, output)?;
    render_question_answer_actions(events, adapter, state, output)?;
    render_slash_actions(events, state, output)?;
    let ledger = build_command_blocks(events);
    let findings = findings_from_blocks(&ledger.blocks);
    for block in &ledger.blocks {
        if state.handled_command_hooks.contains(&block.id) {
            continue;
        }
        state.handled_command_hooks.insert(block.id.clone());
        let hook_findings = state.hook_engine.evaluate(block);
        for finding in hook_findings {
            state.command_hook_hints.push(RuntimeCommandHookHint {
                id: format!("hook-{}", block.id),
                command_block_id: block.id.clone(),
                ended_at_ms: block.ended_at_ms,
                prompt_hint: finding.title.clone(),
                finding_markdown: Some(finding.description.clone()),
            });
        }
    }
    record_command_result_hooks(&ledger.blocks, state);
    render_command_hook_findings(&ledger.blocks, state, output)?;
    render_intercept_agent_guidance(events, &ledger.blocks, adapter, state, output)?;

    let analysis_mode = state.analysis_mode;
    for block in ledger
        .blocks
        .iter()
        .filter(|block| should_analyze_failed_block(block, analysis_mode))
    {
        start_agent_for_block(
            block,
            &findings,
            adapter,
            state,
            output,
            block_end_event_index(events, block),
        )?;
        output.flush()?;
    }

    render_agent_cancel_actions(events, &ledger.blocks, state, output, !shell_busy)?;
    render_post_failure_actions(events, &ledger.blocks, &findings, adapter, state, output)?;

    render_selection_actions(events, state, output)?;
    render_approval_actions(events, adapter, state, output)?;
    flush_held_agent_events(state, output)?;
    if !shell_busy {
        poll_active_agent_run(state, output, adapter)?;
    }
    flush_held_agent_events(state, output)?;
    render_owned_shell_prompt(state, output)?;

    Ok(())
}

fn render_owned_shell_prompt<W: Write>(
    state: &mut InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    if !state.needs_prompt_after_agent_run
        || state.active_run.is_some()
        || state.shell_exited
        || pending_card_capture(state).is_some()
    {
        return Ok(());
    }

    // TODO(native-mode): In wrapped-shell mode the child shell also emits its
    // own PS1 prompt, so this "cosh-osc$ " line produces a visible double-prompt.
    // Once native-mode lands (cosh *is* the shell) this owned prompt becomes the
    // only one and the duplication disappears.  Until then, suppress or reconcile
    // with the child shell's prompt output.
    write!(output, "cosh-osc$ ")?;
    output.flush()?;
    state.needs_prompt_after_agent_run = false;
    Ok(())
}

fn render_agent_cancel_actions<W: Write>(
    events: &[ShellEvent],
    blocks: &[CommandBlock],
    state: &mut InlineState,
    output: &mut W,
    allow_render: bool,
) -> std::io::Result<()> {
    for (idx, event) in events.iter().enumerate() {
        if !event_requests_agent_cancel(event) {
            continue;
        }

        let key = format!("agent-cancel-{idx}");
        if !state.handled_cancel_requests.insert(key) {
            continue;
        }

        if let Some(active_run) = state.active_run.as_ref() {
            active_run.handle.cancel();
            if allow_render {
                RatatuiInlineRenderer::for_terminal().write_notice(
                    output,
                    "Agent cancellation requested",
                    vec!["Stopping active Agent run...".to_string()],
                    Some("Shell remains active."),
                )?;
                output.flush()?;
            }
            continue;
        }

        if is_control_cancel_event(event) {
            continue;
        }

        let cancellation_reason =
            if let Some(block) = latest_pending_failed_block_before_event(blocks, state, event) {
                state.canceled_blocks.insert(block.id.clone());
                state.handled_cancellations.insert(format!("cancel-{idx}"));
                format!("cancelled pending analysis for `{}`", block.command)
            } else {
                "no active Agent run is currently waiting for cancellation".to_string()
            };

        let governed = govern_agent_events(
            &[AgentEvent::AgentCancelled {
                run_id: format!("cancel-{idx}"),
                reason: cancellation_reason,
            }],
            &Policy::default(),
        )
        .events;
        let body = governed
            .first()
            .map(|event| {
                let mut lines = event
                    .display_text
                    .lines()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>();
                if lines.first().is_some_and(|line| line == "Agent cancelled") {
                    lines.remove(0);
                }
                lines
            })
            .unwrap_or_else(|| vec!["Agent cancellation requested".to_string()]);
        RatatuiInlineRenderer::for_terminal().write_notice(
            output,
            "Agent cancelled",
            body,
            Some("Shell remains active."),
        )?;
        output.flush()?;
    }

    Ok(())
}

fn shell_has_active_foreground_command(events: &[ShellEvent]) -> bool {
    let mut active = HashSet::new();
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

fn is_control_cancel_event(event: &ShellEvent) -> bool {
    event.kind == ShellEventKind::UserInputIntercepted
        && event.component.as_deref() == Some("control")
        && event.input.as_deref() == Some("ctrl_c")
}

fn stable_event_key(prefix: &str, idx: usize, event: &ShellEvent) -> String {
    match event.started_at_ms {
        Some(started_at_ms) => format!(
            "{prefix}:{}:{}:{}",
            started_at_ms,
            event.component.as_deref().unwrap_or_default(),
            event.input.as_deref().unwrap_or_default()
        ),
        None => format!("{prefix}:{idx}"),
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

fn passthrough_non_interactive(args: &[String]) -> Option<i32> {
    if args.iter().any(|a| a == "-c") {
        let shell = detect_passthrough_shell(args);
        let pass_args: Vec<&str> = args[1..].iter().map(String::as_str).collect();
        let status = std::process::Command::new(&shell)
            .args(&pass_args)
            .status()
            .map(|s| s.code().unwrap_or(1))
            .unwrap_or_else(|err| {
                eprintln!("cosh-shell: exec {shell} failed: {err}");
                126
            });
        return Some(status);
    }

    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        let shell = detect_passthrough_shell(args);
        let status = std::process::Command::new(&shell)
            .stdin(std::process::Stdio::inherit())
            .status()
            .map(|s| s.code().unwrap_or(1))
            .unwrap_or_else(|err| {
                eprintln!("cosh-shell: exec {shell} failed: {err}");
                126
            });
        return Some(status);
    }

    None
}

fn detect_passthrough_shell(args: &[String]) -> String {
    for (i, arg) in args.iter().enumerate() {
        if arg == "--shell" {
            if let Some(val) = args.get(i + 1) {
                return val.clone();
            }
        }
        if let Some(val) = arg.strip_prefix("--shell=") {
            return val.to_string();
        }
    }
    std::env::var("COSH_SHELL_DEFAULT_SHELL").unwrap_or_else(|_| "bash".to_string())
}

fn print_usage_help() {
    eprintln!(
        "Usage: cosh-shell [OPTIONS]\n\
         \n\
         AI-augmented interactive shell wrapper.\n\
         \n\
         Modes:\n\
           raw [adapter] [--run]   Interactive mode with AI (default: fake adapter)\n\
           demo                    Demo with synthetic events\n\
         \n\
         Options:\n\
           -c <command>            Execute command and exit (passthrough to bash/zsh)\n\
           --shell <shell>         Use specified shell (bash, zsh) [default: bash]\n\
           --version               Print version\n\
           --help                  Print help"
    );
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        adapter_name_from_args, parse_raw_shell, raw_shell_from_args, render_inline_guidance,
        shell_has_active_foreground_command, AdapterInstance, FakeAgentAdapter, InlineState,
        RawShellKind, ShellEvent, ShellEventKind,
    };

    #[test]
    fn raw_shell_selection_uses_explicit_arg_only() {
        assert_eq!(parse_raw_shell("/bin/zsh"), RawShellKind::Zsh);
        assert_eq!(parse_raw_shell("bash"), RawShellKind::Bash);
        assert_eq!(
            parse_raw_shell("/usr/bin/fish"),
            RawShellKind::Unsupported("fish".to_string())
        );
        assert_eq!(
            raw_shell_from_args(&["fake".to_string(), "--shell".to_string(), "zsh".to_string()]),
            Some(RawShellKind::Zsh)
        );
        assert_eq!(
            raw_shell_from_args(&[
                "fake".to_string(),
                "--shell=bash".to_string(),
                "--run".to_string()
            ]),
            Some(RawShellKind::Bash)
        );
        assert_eq!(
            raw_shell_from_args(&["fake".to_string(), "--run".to_string()]),
            None
        );
        assert_eq!(
            raw_shell_from_args(&["fake".to_string(), "--shell".to_string()]),
            Some(RawShellKind::MissingShellValue)
        );
        assert_eq!(
            raw_shell_from_args(&[
                "fake".to_string(),
                "--shell".to_string(),
                "--run".to_string()
            ]),
            Some(RawShellKind::MissingShellValue)
        );
        assert_eq!(
            adapter_name_from_args(&["--shell".to_string(), "zsh".to_string(), "qwen".to_string()]),
            Some("qwen")
        );
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
}
