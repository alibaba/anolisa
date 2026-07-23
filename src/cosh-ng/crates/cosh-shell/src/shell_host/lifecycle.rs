use std::io;

use crate::journal::{redacted_shell_events, write_shell_events};
use crate::types::{ShellEvent, ShellEventKind};

use super::model::{ShellHostConfig, ShellHostOutput};
use super::osc::{now_ms, OscParser};

pub(super) fn push_shell_started_event(parser: &mut OscParser, config: &ShellHostConfig) {
    parser.events.push(ShellEvent {
        kind: ShellEventKind::ShellStarted,
        session_id: config.session_id.clone(),
        command_id: None,
        command: None,
        cwd: std::env::current_dir()
            .ok()
            .map(|path| path.display().to_string()),
        end_cwd: None,
        exit_code: None,
        started_at_ms: Some(now_ms()),
        ended_at_ms: None,
        duration_ms: None,
        terminal_output_ref: None,
        terminal_output_bytes: None,
        input: None,
        component: None,
        message: None,
        command_origin: None,
        shell_environment_generation: None,
        audit_identity: None,
    });
}

pub(super) fn push_shell_exited_event(
    parser: &mut OscParser,
    config: &ShellHostConfig,
    exit_status: Option<i32>,
) -> io::Result<()> {
    parser.finish_current_on_exit(exit_status.unwrap_or(0))?;
    parser.events.push(ShellEvent {
        kind: ShellEventKind::ShellExited,
        session_id: config.session_id.clone(),
        command_id: None,
        command: None,
        cwd: None,
        end_cwd: None,
        exit_code: exit_status,
        started_at_ms: None,
        ended_at_ms: Some(now_ms()),
        duration_ms: None,
        terminal_output_ref: None,
        terminal_output_bytes: None,
        input: None,
        component: None,
        message: None,
        command_origin: None,
        shell_environment_generation: None,
        audit_identity: None,
    });
    Ok(())
}

pub(super) fn finish_shell_host_output(
    config: &ShellHostConfig,
    mut parser: OscParser,
    exit_status: Option<i32>,
) -> io::Result<ShellHostOutput> {
    push_shell_exited_event(&mut parser, config, exit_status)?;
    build_shell_host_output(config, parser, exit_status)
}

pub(super) fn build_shell_host_output(
    config: &ShellHostConfig,
    parser: OscParser,
    exit_status: Option<i32>,
) -> io::Result<ShellHostOutput> {
    let journal_path = config.work_dir.join("events.jsonl");
    let events = redacted_shell_events(&parser.events);
    write_shell_events(&journal_path, &events)?;

    Ok(ShellHostOutput {
        events,
        terminal_output: parser.clean,
        work_dir: config.work_dir.clone(),
        journal_path,
        exit_status,
    })
}
