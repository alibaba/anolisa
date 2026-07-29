use std::fs::File;
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::sync::{Arc, Mutex};

use nix::libc;

use crate::raw_input::{
    signal_foreground_process_group, write_all_pty, RawInputMode, RawObserverAction,
};
use crate::types::ShellHandoffRequest;

use super::super::osc::OscParser;
use super::super::prompt_replay::{prompt_replay_bytes, PromptReplayTracker};
use super::terminal_recovery::{PendingTerminalRecovery, TerminalRecoveryOwner};
use super::{mark_pending_prompt_replayed, write_pending_display, write_prompt_ghost};

fn write_handoff_request(path: &Path, command: &str) -> io::Result<()> {
    std::fs::write(path, command.as_bytes())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn resolve_pty_emit<W: Write>(
    master: &mut File,
    child_pid: u32,
    terminal_fd: i32,
    parser: &mut OscParser,
    output: &mut W,
    input_mode: &Arc<Mutex<RawInputMode>>,
    action: RawObserverAction,
    display_start: &mut usize,
    prompt_replay: &mut PromptReplayTracker,
    pending_terminal_restore: &mut PendingTerminalRecovery,
    recovery_request_file: &Path,
    handoff_request_file: &Path,
) -> io::Result<RawObserverAction> {
    match action {
        RawObserverAction::EmitToPty(request) => {
            emit_to_pty(
                master,
                terminal_fd,
                parser,
                output,
                request,
                display_start,
                prompt_replay,
                pending_terminal_restore,
                handoff_request_file,
                false,
            )?;
            Ok(RawObserverAction::RawPassthrough)
        }
        RawObserverAction::EmitToPtyWithPromptRestore(request) => {
            emit_to_pty(
                master,
                terminal_fd,
                parser,
                output,
                request,
                display_start,
                prompt_replay,
                pending_terminal_restore,
                handoff_request_file,
                true,
            )?;
            Ok(RawObserverAction::RawPassthrough)
        }
        RawObserverAction::InterruptForeground => {
            output.flush()?;
            pending_terminal_restore
                .mark_owner(TerminalRecoveryOwner::CoshTimeoutInterrupt, terminal_fd);
            signal_foreground_process_group(
                master.as_raw_fd(),
                terminal_fd,
                child_pid,
                libc::SIGINT,
            )?;
            pending_terminal_restore.restore_modes(terminal_fd)?;
            pending_terminal_restore.request_shell_recovery(recovery_request_file)?;
            parser.push_control_event("timeout_interrupt");
            Ok(RawObserverAction::Continue)
        }
        RawObserverAction::RestorePrompt {
            ghost_text,
            ghost_route,
        } => {
            output.flush()?;
            let raw_prompt = parser.last_prompt_display();
            let prompt = prompt_replay_bytes(raw_prompt);
            if prompt.is_empty() {
                return Ok(RawObserverAction::RestorePrompt {
                    ghost_text,
                    ghost_route,
                });
            }
            if parser.display.len() > *display_start {
                write_pending_display(parser, output, display_start, prompt_replay)?;
            } else {
                output.write_all(prompt)?;
                mark_pending_prompt_replayed(parser, raw_prompt, display_start);
                prompt_replay.arm_for_replay(raw_prompt);
            }
            if let Some(text) = &ghost_text {
                let selection = matches!(
                    ghost_route,
                    crate::raw_input::PromptGhostRoute::AgentSelection { .. }
                );
                if let Ok(mut mode) = input_mode.lock() {
                    *mode = RawInputMode::PromptGhost {
                        text: text.clone(),
                        route: ghost_route,
                    };
                }
                write_prompt_ghost(output, text, selection)?;
            }
            output.flush()?;
            Ok(RawObserverAction::Continue)
        }
        other => Ok(other),
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_to_pty<W: Write>(
    master: &mut File,
    terminal_fd: i32,
    parser: &mut OscParser,
    output: &mut W,
    request: ShellHandoffRequest,
    display_start: &mut usize,
    prompt_replay: &mut PromptReplayTracker,
    pending_terminal_restore: &mut PendingTerminalRecovery,
    handoff_request_file: &Path,
    restore_prompt: bool,
) -> io::Result<()> {
    output.flush()?;
    if restore_prompt {
        restore_prompt_display_before_handoff(parser, output, display_start, prompt_replay)?;
    }
    let bytes = request.pty_bytes().map_err(|message| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("blocked shell handoff: {message}"),
        )
    })?;
    pending_terminal_restore.record_intervention_start(terminal_fd);
    parser.register_pending_handoff_origin(&request);
    write_handoff_request(handoff_request_file, &request.command)?;
    if let Err(err) = write_all_pty(master, &bytes) {
        let _ = std::fs::remove_file(handoff_request_file);
        return Err(err);
    }
    Ok(())
}

pub(super) fn restore_prompt_display_before_handoff<W: Write>(
    parser: &OscParser,
    output: &mut W,
    display_start: &mut usize,
    prompt_replay: &mut PromptReplayTracker,
) -> io::Result<()> {
    if parser.display.len() > *display_start {
        write_pending_display(parser, output, display_start, prompt_replay)?;
        output.flush()?;
        return Ok(());
    }

    let raw_prompt = parser.last_prompt_display();
    let prompt = prompt_replay_bytes(raw_prompt);
    if prompt.is_empty() {
        return Ok(());
    }
    output.write_all(prompt)?;
    output.flush()?;
    mark_pending_prompt_replayed(parser, raw_prompt, display_start);
    prompt_replay.arm_for_replay(raw_prompt);
    Ok(())
}
