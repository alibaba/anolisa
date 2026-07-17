use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::process::Child;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use nix::libc;
use nix::pty::Winsize;

use crate::raw_input::{
    set_pty_winsize, signal_foreground_process_group, signal_process_group, update_input_mode,
    write_all_pty, RawInputEvent, RawInputMode, RawObserverAction,
};
use crate::types::{ShellEvent, ShellEventKind, ShellHandoffRequest};

use super::model::current_terminal_winsize;
use super::osc::OscParser;
use super::prompt_replay::{
    prompt_prefixed_replay_bytes, prompt_replay_bytes, strip_replayed_prompt_prefix,
};

#[allow(clippy::too_many_arguments)]
pub(super) fn read_raw_until_exit<W: Write, F>(
    master: &mut File,
    terminal: &File,
    child: &mut Child,
    parser: &mut OscParser,
    output: &mut W,
    event_observer: &mut F,
    input_events: &Receiver<RawInputEvent>,
    input_mode: &Arc<Mutex<RawInputMode>>,
    last_winsize: &mut Winsize,
    prompt: &str,
    recovery_request_file: &Path,
    handoff_request_file: &Path,
) -> io::Result<()>
where
    F: FnMut(&[ShellEvent], &mut W) -> io::Result<RawObserverAction>,
{
    let mut buffer = [0_u8; 8192];
    let mut display_start = parser.display.len();
    let mut native_candidate_echoed_len = 0;
    let mut replayed_prompt_prefix: Option<Vec<u8>> = None;
    let mut pending_terminal_restore = PendingTerminalRecovery::default();
    let mut pending_prompt_restore = None;
    loop {
        sync_outer_terminal_winsize(master.as_raw_fd(), child.id(), last_winsize)?;
        if restore_terminal_after_interrupted_command(
            terminal.as_raw_fd(),
            parser,
            &mut pending_terminal_restore,
        )? {
            thread::sleep(Duration::from_millis(10));
            continue;
        }
        drain_raw_input_events(
            master,
            input_events,
            parser,
            output,
            prompt,
            &mut native_candidate_echoed_len,
        )?;
        let mut observer_action = merge_pending_prompt_restore(
            event_observer(&parser.events, output)?,
            &mut pending_prompt_restore,
        );
        observer_action = resolve_pty_emit(
            master,
            child.id(),
            terminal.as_raw_fd(),
            parser,
            output,
            input_mode,
            observer_action,
            &mut display_start,
            &mut replayed_prompt_prefix,
            &mut pending_terminal_restore,
            recovery_request_file,
            handoff_request_file,
        )?;
        remember_pending_prompt_restore(&observer_action, &mut pending_prompt_restore);
        update_input_mode(input_mode, &observer_action);
        let mut hold_shell_output = observer_action.hold_shell_output();
        if !hold_shell_output && parser.display.len() > display_start {
            write_pending_display_preserving_prompt_ghost(
                parser,
                output,
                &mut display_start,
                &mut replayed_prompt_prefix,
                input_mode,
            )?;
            output.flush()?;
        }
        loop {
            match master.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    parser.feed(&buffer[..n])?;
                    for cut in parser.drain_intervention_display_cuts() {
                        let cut = cut.min(parser.display.len());
                        if !hold_shell_output && cut > display_start {
                            write_display_slice(
                                parser,
                                output,
                                display_start,
                                cut,
                                &mut replayed_prompt_prefix,
                            )?;
                            output.flush()?;
                            display_start = cut;
                        }
                        observer_action = merge_pending_prompt_restore(
                            event_observer(&parser.events, output)?,
                            &mut pending_prompt_restore,
                        );
                        observer_action = resolve_pty_emit(
                            master,
                            child.id(),
                            terminal.as_raw_fd(),
                            parser,
                            output,
                            input_mode,
                            observer_action,
                            &mut display_start,
                            &mut replayed_prompt_prefix,
                            &mut pending_terminal_restore,
                            recovery_request_file,
                            handoff_request_file,
                        )?;
                        remember_pending_prompt_restore(
                            &observer_action,
                            &mut pending_prompt_restore,
                        );
                        update_input_mode(input_mode, &observer_action);
                        hold_shell_output = observer_action.hold_shell_output();
                        if !hold_shell_output && parser.display.len() > display_start {
                            write_pending_display_preserving_prompt_ghost(
                                parser,
                                output,
                                &mut display_start,
                                &mut replayed_prompt_prefix,
                                input_mode,
                            )?;
                            output.flush()?;
                        }
                    }
                    observer_action = merge_pending_prompt_restore(
                        event_observer(&parser.events, output)?,
                        &mut pending_prompt_restore,
                    );
                    observer_action = resolve_pty_emit(
                        master,
                        child.id(),
                        terminal.as_raw_fd(),
                        parser,
                        output,
                        input_mode,
                        observer_action,
                        &mut display_start,
                        &mut replayed_prompt_prefix,
                        &mut pending_terminal_restore,
                        recovery_request_file,
                        handoff_request_file,
                    )?;
                    remember_pending_prompt_restore(&observer_action, &mut pending_prompt_restore);
                    update_input_mode(input_mode, &observer_action);
                    hold_shell_output = observer_action.hold_shell_output();
                    if !hold_shell_output && parser.display.len() > display_start {
                        write_pending_display_preserving_prompt_ghost(
                            parser,
                            output,
                            &mut display_start,
                            &mut replayed_prompt_prefix,
                            input_mode,
                        )?;
                        output.flush()?;
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) if child.try_wait()?.is_some() => {
                    release_held_shell_output(
                        event_observer,
                        &parser.events,
                        parser,
                        output,
                        &mut display_start,
                        &mut replayed_prompt_prefix,
                    )?;
                    return Ok(());
                }
                Err(err) => return Err(err),
            }
        }

        if child.try_wait()?.is_some() {
            release_held_shell_output(
                event_observer,
                &parser.events,
                parser,
                output,
                &mut display_start,
                &mut replayed_prompt_prefix,
            )?;
            return Ok(());
        }
        sync_outer_terminal_winsize(master.as_raw_fd(), child.id(), last_winsize)?;
        if restore_terminal_after_interrupted_command(
            terminal.as_raw_fd(),
            parser,
            &mut pending_terminal_restore,
        )? {
            thread::sleep(Duration::from_millis(10));
            continue;
        }
        drain_raw_input_events(
            master,
            input_events,
            parser,
            output,
            prompt,
            &mut native_candidate_echoed_len,
        )?;
        observer_action = merge_pending_prompt_restore(
            event_observer(&parser.events, output)?,
            &mut pending_prompt_restore,
        );
        observer_action = resolve_pty_emit(
            master,
            child.id(),
            terminal.as_raw_fd(),
            parser,
            output,
            input_mode,
            observer_action,
            &mut display_start,
            &mut replayed_prompt_prefix,
            &mut pending_terminal_restore,
            recovery_request_file,
            handoff_request_file,
        )?;
        remember_pending_prompt_restore(&observer_action, &mut pending_prompt_restore);
        update_input_mode(input_mode, &observer_action);
        hold_shell_output = observer_action.hold_shell_output();
        if !hold_shell_output && parser.display.len() > display_start {
            write_pending_display_preserving_prompt_ghost(
                parser,
                output,
                &mut display_start,
                &mut replayed_prompt_prefix,
                input_mode,
            )?;
            output.flush()?;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn release_held_shell_output<W: Write, F>(
    event_observer: &mut F,
    events: &[ShellEvent],
    parser: &OscParser,
    output: &mut W,
    display_start: &mut usize,
    replayed_prompt_prefix: &mut Option<Vec<u8>>,
) -> io::Result<()>
where
    F: FnMut(&[ShellEvent], &mut W) -> io::Result<RawObserverAction>,
{
    drain_observer_until_released(event_observer, events, output)?;
    if parser.display.len() > *display_start {
        write_pending_display(parser, output, display_start, replayed_prompt_prefix)?;
        output.flush()?;
    }
    Ok(())
}

fn write_pending_display<W: Write>(
    parser: &OscParser,
    output: &mut W,
    display_start: &mut usize,
    replayed_prompt_prefix: &mut Option<Vec<u8>>,
) -> io::Result<()> {
    let display_end = parser.display.len();
    write_display_slice(
        parser,
        output,
        *display_start,
        display_end,
        replayed_prompt_prefix,
    )?;
    *display_start = display_end;
    Ok(())
}

fn write_pending_display_preserving_prompt_ghost<W: Write>(
    parser: &OscParser,
    output: &mut W,
    display_start: &mut usize,
    replayed_prompt_prefix: &mut Option<Vec<u8>>,
    input_mode: &Arc<Mutex<RawInputMode>>,
) -> io::Result<()> {
    write_pending_display(parser, output, display_start, replayed_prompt_prefix)?;
    let ghost_text = input_mode.lock().ok().and_then(|mode| match &*mode {
        RawInputMode::PromptGhost { text, .. } => Some(text.clone()),
        _ => None,
    });
    if let Some(text) = ghost_text {
        write_prompt_ghost(output, &text)?;
    }
    Ok(())
}

fn write_prompt_ghost<W: Write>(output: &mut W, text: &str) -> io::Result<()> {
    write!(output, "\x1b[s\x1b[2m {text}\x1b[0m\x1b[u")
}

fn write_display_slice<W: Write>(
    parser: &OscParser,
    output: &mut W,
    display_start: usize,
    display_end: usize,
    replayed_prompt_prefix: &mut Option<Vec<u8>>,
) -> io::Result<()> {
    let bytes = strip_replayed_prompt_prefix(
        &parser.display[display_start..display_end],
        replayed_prompt_prefix,
    );
    let prompt = parser.last_prompt_display();
    output.write_all(&prompt_prefixed_replay_bytes(bytes, prompt))
}

fn drain_observer_until_released<W: Write, F>(
    event_observer: &mut F,
    events: &[ShellEvent],
    output: &mut W,
) -> io::Result<()>
where
    F: FnMut(&[ShellEvent], &mut W) -> io::Result<RawObserverAction>,
{
    for _ in 0..1_000 {
        if !event_observer(events, output)?.hold_shell_output() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

fn drain_raw_input_events<W: Write>(
    _master: &mut File,
    input_events: &Receiver<RawInputEvent>,
    parser: &mut OscParser,
    output: &mut W,
    prompt: &str,
    native_candidate_echoed_len: &mut usize,
) -> io::Result<()> {
    let native_mode = prompt.is_empty();
    while let Ok(event) = input_events.try_recv() {
        match event {
            RawInputEvent::ShellInputActivity => parser.push_shell_input_activity_event(),
            RawInputEvent::CtrlC => {
                parser.push_control_event("ctrl_c");
            }
            RawInputEvent::CandidateRedraw { input, hint } => {
                if native_mode {
                    if input.len() >= *native_candidate_echoed_len {
                        output.write_all(&input[*native_candidate_echoed_len..])?;
                    } else {
                        let erased = *native_candidate_echoed_len - input.len();
                        for _ in 0..erased {
                            write!(output, "\x08 \x08")?;
                        }
                    }
                    *native_candidate_echoed_len = input.len();
                } else {
                    write!(output, "\r\x1b[2K{prompt}")?;
                    output.write_all(&input)?;
                    if let Some(hint) = hint {
                        write!(output, "\x1b[s\x1b[2m {hint}\x1b[0m\x1b[u")?;
                    }
                }
                output.flush()?;
            }
            RawInputEvent::CandidateCommit(input) => {
                if native_mode {
                    if input.len() > *native_candidate_echoed_len {
                        output.write_all(&input[*native_candidate_echoed_len..])?;
                    }
                    *native_candidate_echoed_len = 0;
                } else {
                    write!(output, "\r\x1b[2K")?;
                    write!(output, "{prompt}")?;
                    output.write_all(&input)?;
                }
                writeln!(output)?;
                output.flush()?;
            }
            RawInputEvent::PromptGhostClear => {
                clear_prompt_ghost_line(parser, output, prompt, native_candidate_echoed_len)?;
            }
            RawInputEvent::PromptGhostDismissed => {
                parser.push_prompt_ghost_event("dismissed");
            }
            RawInputEvent::PromptGhostIntercept {
                input,
                suggestion_id,
            } => {
                let session_id = parser.session_id.clone();
                let component = suggestion_id
                    .map(|id| format!("prompt_ghost:{id}"))
                    .unwrap_or_else(|| "prompt_ghost".to_string());
                parser.push_intercept_event(&session_id, input, None, &component);
            }
            RawInputEvent::CandidateClearLine => {
                if native_mode {
                    for _ in 0..*native_candidate_echoed_len {
                        write!(output, "\x08 \x08")?;
                    }
                    *native_candidate_echoed_len = 0;
                } else if !native_mode {
                    write!(output, "\r\x1b[2K{prompt}")?;
                }
                output.flush()?;
            }
            RawInputEvent::UserIntercept(input, reason) => {
                let session_id = parser.session_id.clone();
                parser.push_intercept_event(&session_id, input, None, reason.as_str())
            }
            RawInputEvent::CardFocus(id, selected) => {
                parser.push_card_event("focus", &format!("{id}:{selected}"))
            }
            RawInputEvent::CardToggle(id, selected) => {
                parser.push_card_event("toggle", &format!("{id}:{selected}"))
            }
            RawInputEvent::CardInput(id, text) => {
                parser.push_card_event("input", &format!("{id}:{text}"))
            }
            RawInputEvent::CardApprove(id) => parser.push_card_event("approve", &id),
            RawInputEvent::CardAlwaysTrust(id) => parser.push_card_event("always_trust", &id),
            RawInputEvent::CardDeny(id) => parser.push_card_event("deny", &id),
            RawInputEvent::CardDetails(id) => parser.push_card_event("details", &id),
            RawInputEvent::CardCancel(id) => parser.push_card_event("cancel", &id),
            RawInputEvent::CardAnswer(answer) => parser.push_card_event("answer", &answer),
            RawInputEvent::QuestionCancel(id) => parser.push_card_event("question_cancel", &id),
            RawInputEvent::EvidenceSend(id) => parser.push_card_event("evidence_send", &id),
            RawInputEvent::EvidenceIgnore(id) => parser.push_card_event("evidence_ignore", &id),
            RawInputEvent::EvidenceCancel(id) => parser.push_card_event("evidence_cancel", &id),
            RawInputEvent::ModeFocus(id, selected) => {
                parser.push_card_event("mode_focus", &format!("{id}:{selected}"))
            }
            RawInputEvent::ModeSet(id, selected) => {
                parser.push_card_event("mode_set", &format!("{id}:{selected}"))
            }
            RawInputEvent::ModeCancel(id) => parser.push_card_event("mode_cancel", &id),
            RawInputEvent::ConfigFocus(id, selected) => {
                parser.push_card_event("config_focus", &format!("{id}:{selected}"))
            }
            RawInputEvent::ConfigSave(id) => parser.push_card_event("config_save", &id),
            RawInputEvent::ConfigCancel(id) => parser.push_card_event("config_cancel", &id),
            RawInputEvent::ConfigLanguageFocus(id, selected) => {
                parser.push_card_event("config_language_focus", &format!("{id}:{selected}"))
            }
            RawInputEvent::ConfigLanguageSet(id, selected) => {
                parser.push_card_event("config_language_set", &format!("{id}:{selected}"))
            }
            RawInputEvent::ConfigLanguageCancel(id) => {
                parser.push_card_event("config_language_cancel", &id)
            }
        }
    }
    Ok(())
}

fn clear_prompt_ghost_line<W: Write>(
    parser: &OscParser,
    output: &mut W,
    fallback_prompt: &str,
    native_candidate_echoed_len: &mut usize,
) -> io::Result<()> {
    write!(output, "\r\x1b[2K")?;
    let replay = prompt_replay_bytes(parser.last_prompt_display());
    if replay.is_empty() {
        output.write_all(fallback_prompt.as_bytes())?;
    } else {
        output.write_all(replay)?;
    }
    *native_candidate_echoed_len = 0;
    output.flush()
}

fn shell_has_active_foreground_command(events: &[ShellEvent]) -> bool {
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

fn shell_has_completed_foreground_command(events: &[ShellEvent]) -> bool {
    events.iter().any(|event| {
        matches!(
            event.kind,
            ShellEventKind::CommandCompleted | ShellEventKind::CommandFailed
        )
    })
}

fn merge_pending_prompt_restore(
    observed: RawObserverAction,
    pending: &mut Option<RawObserverAction>,
) -> RawObserverAction {
    match observed {
        action @ RawObserverAction::RestorePrompt { .. } => {
            pending.take();
            action
        }
        action @ RawObserverAction::Continue => pending.take().unwrap_or(action),
        action => {
            pending.take();
            action
        }
    }
}

fn remember_pending_prompt_restore(
    action: &RawObserverAction,
    pending: &mut Option<RawObserverAction>,
) {
    if matches!(action, RawObserverAction::RestorePrompt { .. }) {
        *pending = Some(action.clone());
    }
}

fn sync_outer_terminal_winsize(
    master_fd: i32,
    child_pid: u32,
    last_winsize: &mut Winsize,
) -> io::Result<()> {
    let Some(current) = current_terminal_winsize() else {
        return Ok(());
    };
    if same_winsize(&current, last_winsize) {
        return Ok(());
    }

    set_pty_winsize(master_fd, current)?;
    signal_process_group(child_pid, libc::SIGWINCH)?;
    *last_winsize = current;
    Ok(())
}

fn write_handoff_request(path: &Path, command: &str) -> io::Result<()> {
    std::fs::write(path, command.as_bytes())
}

#[allow(clippy::too_many_arguments)]
fn resolve_pty_emit<W: Write>(
    master: &mut File,
    child_pid: u32,
    terminal_fd: i32,
    parser: &mut OscParser,
    output: &mut W,
    input_mode: &Arc<Mutex<RawInputMode>>,
    action: RawObserverAction,
    display_start: &mut usize,
    replayed_prompt_prefix: &mut Option<Vec<u8>>,
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
                replayed_prompt_prefix,
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
                replayed_prompt_prefix,
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
                write_pending_display(parser, output, display_start, replayed_prompt_prefix)?;
            } else {
                output.write_all(prompt)?;
                mark_pending_prompt_replayed(parser, raw_prompt, display_start);
                *replayed_prompt_prefix = Some(raw_prompt.to_vec());
            }
            if let Some(text) = &ghost_text {
                if let Ok(mut mode) = input_mode.lock() {
                    *mode = RawInputMode::PromptGhost {
                        text: text.clone(),
                        route: ghost_route,
                    };
                }
                write_prompt_ghost(output, text)?;
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
    replayed_prompt_prefix: &mut Option<Vec<u8>>,
    pending_terminal_restore: &mut PendingTerminalRecovery,
    handoff_request_file: &Path,
    restore_prompt: bool,
) -> io::Result<()> {
    output.flush()?;
    if restore_prompt {
        restore_prompt_display_before_handoff(
            parser,
            output,
            display_start,
            replayed_prompt_prefix,
        )?;
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

fn restore_prompt_display_before_handoff<W: Write>(
    parser: &OscParser,
    output: &mut W,
    display_start: &mut usize,
    replayed_prompt_prefix: &mut Option<Vec<u8>>,
) -> io::Result<()> {
    if parser.display.len() > *display_start {
        write_pending_display(parser, output, display_start, replayed_prompt_prefix)?;
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
    *replayed_prompt_prefix = Some(raw_prompt.to_vec());
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalRecoveryOwner {
    CoshTimeoutInterrupt,
}

#[derive(Default)]
struct PendingTerminalRecovery {
    owner: Option<TerminalRecoveryOwner>,
    snapshot: Option<libc::termios>,
}

impl PendingTerminalRecovery {
    fn record_intervention_start(&mut self, terminal_fd: i32) {
        self.snapshot = read_recoverable_terminal_snapshot(terminal_fd)
            .ok()
            .flatten();
        self.owner = None;
    }

    fn mark_owner(&mut self, owner: TerminalRecoveryOwner, terminal_fd: i32) {
        if self.snapshot.is_none() {
            self.snapshot = read_recoverable_terminal_snapshot(terminal_fd)
                .ok()
                .flatten();
        }
        self.owner = Some(owner);
    }

    fn clear(&mut self) {
        self.owner = None;
        self.snapshot = None;
    }

    fn restore_modes(&self, terminal_fd: i32) -> io::Result<()> {
        if self.owner.is_none() {
            return Ok(());
        }
        if let Some(snapshot) = self.snapshot {
            restore_pty_terminal_modes_from_snapshot(terminal_fd, snapshot)
        } else {
            restore_pty_terminal_modes_to_minimal_sane(terminal_fd)
        }
    }

    fn request_shell_recovery(&self, path: &Path) -> io::Result<()> {
        if self.owner.is_none() {
            return Ok(());
        }
        std::fs::write(path, b"1")
    }
}

fn restore_terminal_after_interrupted_command(
    terminal_fd: i32,
    parser: &OscParser,
    pending_terminal_restore: &mut PendingTerminalRecovery,
) -> io::Result<bool> {
    if pending_terminal_restore.owner.is_none()
        || shell_has_active_foreground_command(&parser.events)
        || !shell_has_completed_foreground_command(&parser.events)
    {
        return Ok(false);
    }
    pending_terminal_restore.restore_modes(terminal_fd)?;
    pending_terminal_restore.clear();
    Ok(false)
}

fn read_pty_terminal_modes(terminal_fd: i32) -> io::Result<libc::termios> {
    let mut termios = unsafe { std::mem::zeroed::<libc::termios>() };
    if unsafe { libc::tcgetattr(terminal_fd, &mut termios) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(termios)
}

fn read_recoverable_terminal_snapshot(terminal_fd: i32) -> io::Result<Option<libc::termios>> {
    let termios = read_pty_terminal_modes(terminal_fd)?;
    if terminal_modes_look_like_external_command_state(&termios) {
        Ok(Some(termios))
    } else {
        Ok(None)
    }
}

fn terminal_modes_look_like_external_command_state(termios: &libc::termios) -> bool {
    let required = libc::ECHO | libc::ICANON | libc::ISIG;
    termios.c_lflag & required == required
}

fn restore_pty_terminal_modes_from_snapshot(
    terminal_fd: i32,
    snapshot: libc::termios,
) -> io::Result<()> {
    if unsafe { libc::tcsetattr(terminal_fd, libc::TCSANOW, &snapshot) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn restore_pty_terminal_modes_to_minimal_sane(terminal_fd: i32) -> io::Result<()> {
    let mut termios = read_pty_terminal_modes(terminal_fd)?;
    termios.c_lflag |= libc::ICANON | libc::ISIG | libc::IEXTEN | libc::ECHO;
    termios.c_iflag |= libc::ICRNL | libc::IXON;
    termios.c_oflag |= libc::OPOST;
    if unsafe { libc::tcsetattr(terminal_fd, libc::TCSANOW, &termios) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn mark_pending_prompt_replayed(parser: &OscParser, prompt: &[u8], display_start: &mut usize) {
    if prompt.is_empty() || *display_start > parser.display.len() {
        return;
    }
    if parser.display[*display_start..].starts_with(prompt) {
        *display_start += prompt.len();
    }
}

fn same_winsize(left: &Winsize, right: &Winsize) -> bool {
    left.ws_row == right.ws_row
        && left.ws_col == right.ws_col
        && left.ws_xpixel == right.ws_xpixel
        && left.ws_ypixel == right.ws_ypixel
}

#[cfg(test)]
#[path = "raw_relay_tests.rs"]
mod tests;
