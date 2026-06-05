use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::process::Child;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use nix::libc;
use nix::pty::Winsize;

use crate::raw_input::{
    set_pty_winsize, signal_process_group, update_input_mode, RawInputEvent, RawInputMode,
    RawObserverAction,
};
use crate::types::ShellEvent;
use crate::types::ShellEventKind;

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
) -> io::Result<()>
where
    F: FnMut(&[ShellEvent], &mut W) -> io::Result<RawObserverAction>,
{
    let mut buffer = [0_u8; 8192];
    let mut display_start = parser.display.len();
    let mut native_candidate_echoed_len = 0;
    let mut replayed_prompt_prefix: Option<Vec<u8>> = None;
    let mut foreground_interrupts = ForegroundInterruptEscalator::default();
    let mut pending_terminal_restore = false;
    loop {
        sync_outer_terminal_winsize(master.as_raw_fd(), child.id(), last_winsize)?;
        if restore_terminal_after_interrupted_command(
            master,
            terminal.as_raw_fd(),
            parser,
            output,
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
            &mut foreground_interrupts,
            &mut pending_terminal_restore,
        )?;
        let mut observer_action = event_observer(&parser.events, output)?;
        observer_action = resolve_pty_emit(
            master,
            child.id(),
            parser,
            output,
            observer_action,
            &mut display_start,
            &mut replayed_prompt_prefix,
            &mut pending_terminal_restore,
        )?;
        update_input_mode(input_mode, &observer_action);
        let mut hold_shell_output = observer_action.hold_shell_output();
        if !hold_shell_output && parser.display.len() > display_start {
            write_pending_display(
                parser,
                output,
                &mut display_start,
                &mut replayed_prompt_prefix,
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
                        observer_action = event_observer(&parser.events, output)?;
                        observer_action = resolve_pty_emit(
                            master,
                            child.id(),
                            parser,
                            output,
                            observer_action,
                            &mut display_start,
                            &mut replayed_prompt_prefix,
                            &mut pending_terminal_restore,
                        )?;
                        update_input_mode(input_mode, &observer_action);
                        hold_shell_output = observer_action.hold_shell_output();
                        if !hold_shell_output && parser.display.len() > display_start {
                            write_pending_display(
                                parser,
                                output,
                                &mut display_start,
                                &mut replayed_prompt_prefix,
                            )?;
                            output.flush()?;
                        }
                    }
                    observer_action = event_observer(&parser.events, output)?;
                    observer_action = resolve_pty_emit(
                        master,
                        child.id(),
                        parser,
                        output,
                        observer_action,
                        &mut display_start,
                        &mut replayed_prompt_prefix,
                        &mut pending_terminal_restore,
                    )?;
                    update_input_mode(input_mode, &observer_action);
                    hold_shell_output = observer_action.hold_shell_output();
                    if !hold_shell_output && parser.display.len() > display_start {
                        write_pending_display(
                            parser,
                            output,
                            &mut display_start,
                            &mut replayed_prompt_prefix,
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
            master,
            terminal.as_raw_fd(),
            parser,
            output,
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
            &mut foreground_interrupts,
            &mut pending_terminal_restore,
        )?;
        observer_action = event_observer(&parser.events, output)?;
        observer_action = resolve_pty_emit(
            master,
            child.id(),
            parser,
            output,
            observer_action,
            &mut display_start,
            &mut replayed_prompt_prefix,
            &mut pending_terminal_restore,
        )?;
        update_input_mode(input_mode, &observer_action);
        hold_shell_output = observer_action.hold_shell_output();
        if !hold_shell_output && parser.display.len() > display_start {
            write_pending_display(
                parser,
                output,
                &mut display_start,
                &mut replayed_prompt_prefix,
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
    master: &mut File,
    input_events: &Receiver<RawInputEvent>,
    parser: &mut OscParser,
    output: &mut W,
    prompt: &str,
    native_candidate_echoed_len: &mut usize,
    foreground_interrupts: &mut ForegroundInterruptEscalator,
    pending_terminal_restore: &mut bool,
) -> io::Result<()> {
    let native_mode = prompt.is_empty();
    while let Ok(event) = input_events.try_recv() {
        match event {
            RawInputEvent::CtrlC => {
                parser.push_control_event("ctrl_c");
                restore_outer_terminal_presentation(output)?;
                if shell_has_active_foreground_command(&parser.events) {
                    *pending_terminal_restore = true;
                }
                if foreground_interrupts.observe_ctrl_c(&parser.events) {
                    *pending_terminal_restore = true;
                    master.write_all(&[0x1c])?;
                    master.flush()?;
                    parser.push_control_event("ctrl_backslash_escalation");
                    restore_outer_terminal_presentation(output)?;
                }
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
                        write!(output, "\x1b[s\x1b[2m  {hint}\x1b[0m\x1b[u")?;
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

fn restore_outer_terminal_presentation<W: Write>(output: &mut W) -> io::Result<()> {
    output.write_all(b"\x1b[0m\x1b[?25h\x1b[?1049l\x1b[?2004l\x1b[?7h")?;
    output.flush()
}

#[derive(Debug)]
struct ForegroundInterruptEscalator {
    last_ctrl_c: Option<Instant>,
    window: Duration,
}

impl Default for ForegroundInterruptEscalator {
    fn default() -> Self {
        Self {
            last_ctrl_c: None,
            window: Duration::from_secs(2),
        }
    }
}

impl ForegroundInterruptEscalator {
    fn observe_ctrl_c(&mut self, events: &[ShellEvent]) -> bool {
        if !shell_has_active_foreground_command(events) {
            self.last_ctrl_c = None;
            return false;
        }

        let now = Instant::now();
        let escalate = self
            .last_ctrl_c
            .is_some_and(|last| now.duration_since(last) <= self.window);
        self.last_ctrl_c = Some(now);
        escalate
    }
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

fn resolve_pty_emit<W: Write>(
    master: &mut File,
    child_pid: u32,
    parser: &mut OscParser,
    output: &mut W,
    action: RawObserverAction,
    display_start: &mut usize,
    replayed_prompt_prefix: &mut Option<Vec<u8>>,
    pending_terminal_restore: &mut bool,
) -> io::Result<RawObserverAction> {
    match action {
        RawObserverAction::EmitToPty(request) => {
            output.flush()?;
            let bytes = request.pty_bytes().map_err(|message| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("blocked shell handoff: {message}"),
                )
            })?;
            master.write_all(&bytes)?;
            master.flush()?;
            Ok(RawObserverAction::Continue)
        }
        RawObserverAction::InterruptForeground => {
            output.flush()?;
            signal_process_group(child_pid, libc::SIGINT)?;
            *pending_terminal_restore = true;
            parser.push_control_event("timeout_interrupt");
            Ok(RawObserverAction::Continue)
        }
        RawObserverAction::RestorePrompt => {
            output.flush()?;
            if parser.display.len() > *display_start {
                return Ok(RawObserverAction::Continue);
            }
            let raw_prompt = parser.last_prompt_display();
            let prompt = prompt_replay_bytes(raw_prompt);
            if prompt.is_empty() {
                master.write_all(b"\n")?;
                master.flush()?;
            } else {
                output.write_all(prompt)?;
                output.flush()?;
                mark_pending_prompt_replayed(parser, raw_prompt, display_start);
                *replayed_prompt_prefix = Some(raw_prompt.to_vec());
            }
            Ok(RawObserverAction::Continue)
        }
        other => Ok(other),
    }
}

fn restore_terminal_after_interrupted_command<W: Write>(
    master: &mut File,
    terminal_fd: i32,
    parser: &OscParser,
    output: &mut W,
    pending_terminal_restore: &mut bool,
) -> io::Result<bool> {
    let prompt = parser.last_prompt_display();
    if !*pending_terminal_restore
        || shell_has_active_foreground_command(&parser.events)
        || prompt.is_empty()
        || !parser.display.ends_with(prompt)
    {
        return Ok(false);
    }
    restore_pty_terminal_modes_for_hidden_command(terminal_fd)?;
    restore_outer_terminal_presentation(output)?;
    master
        .write_all(b"COSH_INTERNAL_RESTORE=1 stty echo icanon isig iexten opost 2>/dev/null\n")?;
    master.flush()?;
    *pending_terminal_restore = false;
    Ok(true)
}

fn restore_pty_terminal_modes_for_hidden_command(master_fd: i32) -> io::Result<()> {
    let mut termios = unsafe { std::mem::zeroed::<libc::termios>() };
    if unsafe { libc::tcgetattr(master_fd, &mut termios) } < 0 {
        return Err(io::Error::last_os_error());
    }

    termios.c_lflag |= libc::ICANON | libc::ISIG | libc::IEXTEN;
    termios.c_lflag &= !(libc::ECHO | libc::ECHONL);
    termios.c_iflag |= libc::ICRNL | libc::IXON;
    termios.c_oflag |= libc::OPOST;

    if unsafe { libc::tcsetattr(master_fd, libc::TCSAFLUSH, &termios) } < 0 {
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
