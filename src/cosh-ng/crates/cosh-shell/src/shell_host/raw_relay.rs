use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::process::Child;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use nix::libc;
use nix::pty::Winsize;

use crate::raw_input::{
    set_pty_winsize, signal_process_group, update_input_mode, RawInputEvent, RawInputMode,
    RawObserverAction,
};
use crate::types::ShellEvent;

use super::model::current_terminal_winsize;
use super::osc::OscParser;

pub(super) fn read_raw_until_exit<W: Write, F>(
    master: &mut File,
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
    loop {
        sync_outer_terminal_winsize(master.as_raw_fd(), child.id(), last_winsize)?;
        drain_raw_input_events(input_events, parser, output, prompt)?;
        let mut observer_action = event_observer(&parser.events, output)?;
        update_input_mode(input_mode, &observer_action);
        let mut hold_shell_output = observer_action.hold_shell_output();
        if !hold_shell_output && parser.display.len() > display_start {
            output.write_all(&parser.display[display_start..])?;
            output.flush()?;
            display_start = parser.display.len();
        }
        loop {
            match master.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    parser.feed(&buffer[..n])?;
                    for cut in parser.drain_intervention_display_cuts() {
                        let cut = cut.min(parser.display.len());
                        if !hold_shell_output && cut > display_start {
                            output.write_all(&parser.display[display_start..cut])?;
                            output.flush()?;
                            display_start = cut;
                        }
                        observer_action = event_observer(&parser.events, output)?;
                        update_input_mode(input_mode, &observer_action);
                        hold_shell_output = observer_action.hold_shell_output();
                        if !hold_shell_output && parser.display.len() > display_start {
                            output.write_all(&parser.display[display_start..])?;
                            output.flush()?;
                            display_start = parser.display.len();
                        }
                    }
                    if !hold_shell_output && parser.display.len() > display_start {
                        output.write_all(&parser.display[display_start..])?;
                        output.flush()?;
                        display_start = parser.display.len();
                    }
                    observer_action = event_observer(&parser.events, output)?;
                    update_input_mode(input_mode, &observer_action);
                    hold_shell_output = observer_action.hold_shell_output();
                    if !hold_shell_output && parser.display.len() > display_start {
                        output.write_all(&parser.display[display_start..])?;
                        output.flush()?;
                        display_start = parser.display.len();
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
            )?;
            return Ok(());
        }
        sync_outer_terminal_winsize(master.as_raw_fd(), child.id(), last_winsize)?;
        drain_raw_input_events(input_events, parser, output, prompt)?;
        observer_action = event_observer(&parser.events, output)?;
        update_input_mode(input_mode, &observer_action);
        hold_shell_output = observer_action.hold_shell_output();
        if !hold_shell_output && parser.display.len() > display_start {
            output.write_all(&parser.display[display_start..])?;
            output.flush()?;
            display_start = parser.display.len();
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
) -> io::Result<()>
where
    F: FnMut(&[ShellEvent], &mut W) -> io::Result<RawObserverAction>,
{
    drain_observer_until_released(event_observer, events, output)?;
    if parser.display.len() > *display_start {
        output.write_all(&parser.display[*display_start..])?;
        output.flush()?;
        *display_start = parser.display.len();
    }
    Ok(())
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
    input_events: &Receiver<RawInputEvent>,
    parser: &mut OscParser,
    output: &mut W,
    prompt: &str,
) -> io::Result<()> {
    let native_mode = prompt.is_empty();
    while let Ok(event) = input_events.try_recv() {
        match event {
            RawInputEvent::CtrlC => parser.push_control_event("ctrl_c"),
            RawInputEvent::CandidateRedraw { input, hint } => {
                if !native_mode {
                    write!(output, "\r\x1b[2K{prompt}")?;
                }
                output.write_all(&input)?;
                if let Some(hint) = hint {
                    write!(output, "\x1b[s\x1b[2m  {hint}\x1b[0m\x1b[u")?;
                }
                output.flush()?;
            }
            RawInputEvent::CandidateCommit(input) => {
                if !native_mode {
                    write!(output, "\r\x1b[2K{prompt}")?;
                    output.write_all(&input)?;
                }
                writeln!(output)?;
                output.flush()?;
            }
            RawInputEvent::CandidateClearLine => {
                write!(output, "\r\x1b[2K")?;
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
            RawInputEvent::CardDeny(id) => parser.push_card_event("deny", &id),
            RawInputEvent::CardDetails(id) => parser.push_card_event("details", &id),
            RawInputEvent::CardCancel(id) => parser.push_card_event("cancel", &id),
            RawInputEvent::CardAnswer(answer) => parser.push_card_event("answer", &answer),
            RawInputEvent::ModeFocus(id, selected) => {
                parser.push_card_event("mode_focus", &format!("{id}:{selected}"))
            }
            RawInputEvent::ModeSet(id, selected) => {
                parser.push_card_event("mode_set", &format!("{id}:{selected}"))
            }
            RawInputEvent::ModeCancel(id) => parser.push_card_event("mode_cancel", &id),
        }
    }
    Ok(())
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

fn same_winsize(left: &Winsize, right: &Winsize) -> bool {
    left.ws_row == right.ws_row
        && left.ws_col == right.ws_col
        && left.ws_xpixel == right.ws_xpixel
        && left.ws_ypixel == right.ws_ypixel
}
