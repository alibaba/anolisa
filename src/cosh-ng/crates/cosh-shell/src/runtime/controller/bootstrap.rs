use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::approval::handoff::trust_key_from_command;
use crate::diagnostics::health::{spawn_startup_health_scan, startup_health_scan_enabled_for_env};
use crate::hooks::{
    dirs_for_hook_loading, is_trusted_project_root, load_hook_feedback_preferences,
    project_hook_root_from_cwd,
};
use crate::runtime::cli_args::{LaunchOptions, RawShellKind, ResumeLaunch};
use crate::runtime::prelude::*;
use crate::runtime::startup::bootstrap_process_path_from_shell;
use crate::runtime::state::{AnalysisMode, InlineState};

use super::{approval_mode_from_config, render_raw_inline_events};

fn build_adapter(kind: AdapterKind) -> AdapterInstance {
    match adapter_for_kind(kind) {
        AdapterInstance::ClaudeCode(adapter) => {
            AdapterInstance::ClaudeCode(adapter.with_model_call(true))
        }
        AdapterInstance::QwenCli(adapter) => {
            AdapterInstance::QwenCli(adapter.with_model_call(true))
        }
        AdapterInstance::CoshCore(adapter) => {
            AdapterInstance::CoshCore(adapter.with_model_call(true))
        }
        other => other,
    }
}

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
            let err = crate::evidence::redact_sensitive_text(&err.to_string()).0;
            eprintln!("host demo failed: {err}");
            return 1;
        }
    };

    render_loop_from_events(&output.events)
}

pub(crate) fn run_raw(
    adapter_name: &str,
    shell_kind: RawShellKind,
    launch_options: LaunchOptions,
) -> i32 {
    let args = std::env::args().collect::<Vec<_>>();

    let Some(kind) = AdapterKind::parse(adapter_name) else {
        let adapter_name = crate::evidence::redact_sensitive_text(adapter_name).0;
        eprintln!("unknown adapter: {adapter_name}");
        return 2;
    };

    let work_dir =
        std::env::temp_dir().join(format!("cosh-shell-raw-session-{}", std::process::id()));
    let _work_dir_cleanup = TempSessionDir::new(work_dir.clone());
    let session_id = format!(
        "raw-session-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    );
    let mut config = ShellHostConfig::new(session_id, work_dir);

    let isolated = args.iter().any(|a| a == "--isolated")
        || std::env::var("COSH_SHELL_ISOLATED").as_deref() == Ok("1");
    if isolated {
        config.native_mode = false;
        if let Ok(prompt) = std::env::var("COSH_POC_PS1") {
            if !prompt.is_empty() {
                config.prompt = prompt;
            }
        }
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

    let cosh_config = load_config();
    config.input_classifier = config
        .input_classifier
        .with_ai_enabled(cosh_config.ai_enabled);

    let adapter = build_adapter(kind);
    let mut inline_state = InlineState::with_raw_session_dir(&config.work_dir);
    inline_state.shell_session_id = Some(config.session_id.clone());
    if let Some(resume) = launch_options.resume {
        inline_state
            .control
            .session_mut()
            .set_pending_launch(match resume {
                ResumeLaunch::Picker => crate::slash::session::SessionLaunchRequest::Picker,
                ResumeLaunch::Session(id) => {
                    crate::slash::session::SessionLaunchRequest::Resume(id)
                }
            });
    }
    let snapshot_publisher = inline_state.shell_rewrite.start_worker();
    config.set_shell_environment_observer(move |snapshot| {
        snapshot_publisher.publish(snapshot);
    });
    if startup_health_scan_enabled_for_env(&cosh_config.health) {
        inline_state.startup_health.pending =
            Some(spawn_startup_health_scan(cosh_config.health.clone()));
    }
    let hook_feedback = load_hook_feedback_preferences();
    inline_state.hooks.feedback = hook_feedback.feedback;
    inline_state.hooks.noisy_groups = hook_feedback.noisy_groups;
    inline_state.language = parse_language_setting(&cosh_config.language)
        .map(resolve_language_setting)
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
    apply_readonly_config(&cosh_config);
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
            let shell = crate::evidence::redact_sensitive_text(&shell).0;
            eprintln!("unsupported raw shell: {shell}; supported shells: bash, zsh");
            return 2;
        }
    };

    config.clear_shell_environment_observer();
    inline_state.shell_rewrite.shutdown();

    match raw_result {
        Ok(output) => output.exit_status.unwrap_or(0),
        Err(err) => {
            let err = crate::evidence::redact_sensitive_text(&err.to_string()).0;
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
        let adapter_name = crate::evidence::redact_sensitive_text(adapter_name).0;
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
            let err = crate::evidence::redact_sensitive_text(&err.to_string()).0;
            eprintln!("interactive demo failed: {err}");
            return 1;
        }
    };

    render_loop_from_events_with_adapter(&shell_output.shell.events, &build_adapter(kind))
}

pub(crate) fn run_adapter_demo(adapter_name: &str) -> i32 {
    let Some(kind) = AdapterKind::parse(adapter_name) else {
        let adapter_name = crate::evidence::redact_sensitive_text(adapter_name).0;
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
        let errors = crate::evidence::redact_sensitive_text(&ledger.errors.join(", ")).0;
        eprintln!("ledger errors: {errors}");
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
                let message = crate::evidence::redact_sensitive_text(&err.message).0;
                eprintln!("adapter failed: {message}");
                return 1;
            }
        };
        govern_agent_events(&agent_events, &Policy::default()).events
    } else {
        Vec::new()
    };

    for line in render_transcript(block, &findings, &interventions, &governed_events) {
        let line = crate::evidence::redact_sensitive_text(&line).0;
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

fn load_hook_engine(cosh_config: &CoshConfig) -> HookEngine {
    let mut hook_engine = HookEngine::new();
    for hook in default_builtin_hooks() {
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

#[cfg(test)]
mod tests {
    use super::*;

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
