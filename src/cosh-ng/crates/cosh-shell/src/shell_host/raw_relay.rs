use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::process::Child;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use nix::libc;
use nix::pty::Winsize;

use crate::raw_input::{
    update_input_mode, update_locked_input_mode, RawInputEvent, RawInputMode, RawObserverAction,
    UserPtyInputGeneration,
};
use crate::types::{ShellEvent, ShellEventKind};

use super::osc::{DisplayCutKind, OscParser};
use super::prompt_replay::{
    prompt_prefixed_replay_bytes, prompt_replay_bytes, PromptReplayTracker,
};

mod input_events;
mod pty_emit;
mod terminal_recovery;
mod terminal_size;

use input_events::drain_raw_input_events;
use pty_emit::resolve_pty_emit;
#[cfg(test)]
use pty_emit::restore_prompt_display_before_handoff;
use terminal_recovery::{restore_terminal_after_interrupted_command, PendingTerminalRecovery};
use terminal_size::sync_outer_terminal_winsize;

pub(super) struct RawActionWatchdog {
    driver_done: Arc<Mutex<Option<Instant>>>,
    grace: Duration,
}

impl RawActionWatchdog {
    pub(super) fn new(driver_done: Arc<Mutex<Option<Instant>>>, grace: Duration) -> Self {
        Self { driver_done, grace }
    }

    fn expired(&self) -> bool {
        self.driver_done
            .lock()
            .ok()
            .and_then(|done| *done)
            .is_some_and(|done| done.elapsed() > self.grace)
    }
}

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
    input_generation: &UserPtyInputGeneration,
    last_winsize: &mut Winsize,
    prompt: &str,
    recovery_request_file: &Path,
    handoff_request_file: &Path,
    watchdog: Option<&RawActionWatchdog>,
) -> io::Result<()>
where
    F: FnMut(&[ShellEvent], &mut W) -> io::Result<RawObserverAction>,
{
    let mut buffer = [0_u8; 8192];
    let mut display_start = parser.display.len();
    let mut native_candidate_echoed_len = 0;
    let mut prompt_replay = PromptReplayTracker::new(input_generation.clone());
    let mut last_pty_output: Option<Instant> = None;
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
            input_events,
            parser,
            output,
            prompt,
            &mut native_candidate_echoed_len,
            &mut prompt_replay,
        )?;
        let mut observer_action = merge_pending_prompt_restore(
            observe_with_input_mode_lock(event_observer, &parser.events, output, input_mode)?,
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
            &mut prompt_replay,
            &mut pending_terminal_restore,
            recovery_request_file,
            handoff_request_file,
        )?;
        remember_pending_prompt_restore(&observer_action, &mut pending_prompt_restore);
        update_input_mode(
            input_mode,
            &observer_action,
            latest_capture_submission_generation(&parser.events),
        );
        let mut hold_shell_output = observer_action.hold_shell_output();
        if !hold_shell_output && parser.display.len() > display_start {
            write_pending_display_preserving_prompt_ghost(
                parser,
                output,
                &mut display_start,
                &mut prompt_replay,
                input_mode,
            )?;
            output.flush()?;
        }
        loop {
            match master.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    last_pty_output = Some(Instant::now());
                    parser.feed(&buffer[..n])?;
                    for (cut, cut_kind) in parser.drain_intervention_display_cuts() {
                        // Only a real prompt boundary (precmd) confirms the
                        // shell finished responding to the relay writes seen
                        // so far; an intercepted line's remaining response is
                        // just the prompt repaint replay dedup strips.
                        match cut_kind {
                            DisplayCutKind::PromptBoundary => {
                                prompt_replay.observe_prompt_boundary()
                            }
                            DisplayCutKind::Intercept => prompt_replay.observe_intercept_cut(),
                        }
                        let cut = cut.min(parser.display.len());
                        if !hold_shell_output && cut > display_start {
                            write_display_slice(
                                parser,
                                output,
                                display_start,
                                cut,
                                &mut prompt_replay,
                            )?;
                            output.flush()?;
                            display_start = cut;
                        }
                        observer_action = merge_pending_prompt_restore(
                            observe_with_input_mode_lock(
                                event_observer,
                                &parser.events,
                                output,
                                input_mode,
                            )?,
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
                            &mut prompt_replay,
                            &mut pending_terminal_restore,
                            recovery_request_file,
                            handoff_request_file,
                        )?;
                        remember_pending_prompt_restore(
                            &observer_action,
                            &mut pending_prompt_restore,
                        );
                        update_input_mode(
                            input_mode,
                            &observer_action,
                            latest_capture_submission_generation(&parser.events),
                        );
                        hold_shell_output = observer_action.hold_shell_output();
                        if !hold_shell_output && parser.display.len() > display_start {
                            write_pending_display_preserving_prompt_ghost(
                                parser,
                                output,
                                &mut display_start,
                                &mut prompt_replay,
                                input_mode,
                            )?;
                            output.flush()?;
                        }
                    }
                    observer_action = merge_pending_prompt_restore(
                        observe_with_input_mode_lock(
                            event_observer,
                            &parser.events,
                            output,
                            input_mode,
                        )?,
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
                        &mut prompt_replay,
                        &mut pending_terminal_restore,
                        recovery_request_file,
                        handoff_request_file,
                    )?;
                    remember_pending_prompt_restore(&observer_action, &mut pending_prompt_restore);
                    update_input_mode(
                        input_mode,
                        &observer_action,
                        latest_capture_submission_generation(&parser.events),
                    );
                    hold_shell_output = observer_action.hold_shell_output();
                    if !hold_shell_output && parser.display.len() > display_start {
                        write_pending_display_preserving_prompt_ghost(
                            parser,
                            output,
                            &mut display_start,
                            &mut prompt_replay,
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
                        &mut prompt_replay,
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
                &mut prompt_replay,
            )?;
            return Ok(());
        }
        if let Some(watchdog) = watchdog {
            if watchdog.expired() {
                child.kill()?;
                child.wait()?;
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "raw action relay watchdog: shell did not exit after the trailing exit was relayed",
                ));
            }
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
            input_events,
            parser,
            output,
            prompt,
            &mut native_candidate_echoed_len,
            &mut prompt_replay,
        )?;
        observer_action = merge_pending_prompt_restore(
            observe_with_input_mode_lock(event_observer, &parser.events, output, input_mode)?,
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
            &mut prompt_replay,
            &mut pending_terminal_restore,
            recovery_request_file,
            handoff_request_file,
        )?;
        remember_pending_prompt_restore(&observer_action, &mut pending_prompt_restore);
        update_input_mode(
            input_mode,
            &observer_action,
            latest_capture_submission_generation(&parser.events),
        );
        hold_shell_output = observer_action.hold_shell_output();
        if !hold_shell_output && parser.display.len() > display_start {
            write_pending_display_preserving_prompt_ghost(
                parser,
                output,
                &mut display_start,
                &mut prompt_replay,
                input_mode,
            )?;
            output.flush()?;
        }
        // The PTY is drained (WouldBlock) at this point: write off
        // submissions a foreground program consumed once the shell has
        // painted a prompt after the last boundary and idles at it. A bare
        // precmd is not enough — the user's PROMPT_COMMAND and PS1 paint run
        // after the marker, so silence alone must never clear the ledger.
        prompt_replay.reconcile_idle_at_prompt(
            parser.has_active_foreground_command(),
            parser.has_prompt_painted_since_ready(),
            last_pty_output,
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn latest_capture_submission_generation(events: &[ShellEvent]) -> Option<u64> {
    for event in events.iter().rev() {
        let Some(capture) = event.capture.as_ref() else {
            continue;
        };
        match capture.lifecycle {
            crate::types::ShellCaptureLifecycle::Submitted => return Some(capture.generation),
            crate::types::ShellCaptureLifecycle::Drained
            | crate::types::ShellCaptureLifecycle::Expired
            | crate::types::ShellCaptureLifecycle::Overflow => return None,
        }
    }
    None
}
fn observe_with_input_mode_lock<W: Write, F>(
    event_observer: &mut F,
    events: &[ShellEvent],
    output: &mut W,
    input_mode: &Arc<Mutex<RawInputMode>>,
) -> io::Result<RawObserverAction>
where
    F: FnMut(&[ShellEvent], &mut W) -> io::Result<RawObserverAction>,
{
    let Ok(mut mode) = input_mode.lock() else {
        return event_observer(events, output);
    };
    let action = event_observer(events, output)?;
    let acknowledged = latest_capture_submission_generation(events);
    update_locked_input_mode(&mut mode, &action, acknowledged);
    Ok(action)
}

fn release_held_shell_output<W: Write, F>(
    event_observer: &mut F,
    events: &[ShellEvent],
    parser: &OscParser,
    output: &mut W,
    display_start: &mut usize,
    prompt_replay: &mut PromptReplayTracker,
) -> io::Result<()>
where
    F: FnMut(&[ShellEvent], &mut W) -> io::Result<RawObserverAction>,
{
    drain_observer_until_released(event_observer, events, output)?;
    if parser.display.len() > *display_start {
        write_pending_display(parser, output, display_start, prompt_replay)?;
        output.flush()?;
    }
    Ok(())
}

fn write_pending_display<W: Write>(
    parser: &OscParser,
    output: &mut W,
    display_start: &mut usize,
    prompt_replay: &mut PromptReplayTracker,
) -> io::Result<()> {
    let display_end = parser.display.len();
    write_display_slice(parser, output, *display_start, display_end, prompt_replay)?;
    *display_start = display_end;
    Ok(())
}

fn write_pending_display_preserving_prompt_ghost<W: Write>(
    parser: &OscParser,
    output: &mut W,
    display_start: &mut usize,
    prompt_replay: &mut PromptReplayTracker,
    input_mode: &Arc<Mutex<RawInputMode>>,
) -> io::Result<()> {
    write_pending_display(parser, output, display_start, prompt_replay)?;
    let ghost = input_mode.lock().ok().and_then(|mode| match &*mode {
        RawInputMode::PromptGhost { text, route } => Some((
            text.clone(),
            matches!(
                route,
                crate::raw_input::PromptGhostRoute::AgentSelection { .. }
            ),
        )),
        _ => None,
    });
    if let Some((text, selection)) = ghost {
        write_prompt_ghost(output, &text, selection)?;
    }
    Ok(())
}

fn write_prompt_ghost<W: Write>(output: &mut W, text: &str, selection: bool) -> io::Result<()> {
    let marker = if selection { " ›" } else { "" };
    write!(output, "\x1b[s\x1b[2m{marker} {text}\x1b[0m\x1b[u")
}

fn write_display_slice<W: Write>(
    parser: &OscParser,
    output: &mut W,
    display_start: usize,
    display_end: usize,
    prompt_replay: &mut PromptReplayTracker,
) -> io::Result<()> {
    let bytes = prompt_replay.strip(&parser.display[display_start..display_end]);
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

fn mark_pending_prompt_replayed(parser: &OscParser, prompt: &[u8], display_start: &mut usize) {
    if prompt.is_empty() || *display_start > parser.display.len() {
        return;
    }
    if parser.display[*display_start..].starts_with(prompt) {
        *display_start += prompt.len();
    }
}

#[cfg(test)]
#[path = "raw_relay_tests.rs"]
mod tests;
