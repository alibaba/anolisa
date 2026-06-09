use std::borrow::Cow;
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

#[allow(clippy::too_many_arguments)]
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
    let mut native_candidate_echoed_len = 0;
    let mut replayed_prompt_prefix: Option<Vec<u8>> = None;
    loop {
        sync_outer_terminal_winsize(master.as_raw_fd(), child.id(), last_winsize)?;
        drain_raw_input_events(
            input_events,
            parser,
            output,
            prompt,
            &mut native_candidate_echoed_len,
        )?;
        let mut observer_action = event_observer(&parser.events, output)?;
        observer_action = resolve_pty_emit(
            master,
            parser,
            output,
            observer_action,
            &mut display_start,
            &mut replayed_prompt_prefix,
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
                            parser,
                            output,
                            observer_action,
                            &mut display_start,
                            &mut replayed_prompt_prefix,
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
                    if !hold_shell_output && parser.display.len() > display_start {
                        write_pending_display(
                            parser,
                            output,
                            &mut display_start,
                            &mut replayed_prompt_prefix,
                        )?;
                        output.flush()?;
                    }
                    observer_action = event_observer(&parser.events, output)?;
                    observer_action = resolve_pty_emit(
                        master,
                        parser,
                        output,
                        observer_action,
                        &mut display_start,
                        &mut replayed_prompt_prefix,
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
        drain_raw_input_events(
            input_events,
            parser,
            output,
            prompt,
            &mut native_candidate_echoed_len,
        )?;
        observer_action = event_observer(&parser.events, output)?;
        observer_action = resolve_pty_emit(
            master,
            parser,
            output,
            observer_action,
            &mut display_start,
            &mut replayed_prompt_prefix,
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

fn strip_replayed_prompt_prefix<'a>(
    bytes: &'a [u8],
    replayed_prompt_prefix: &mut Option<Vec<u8>>,
) -> &'a [u8] {
    let Some(raw_prompt) = replayed_prompt_prefix.as_deref() else {
        return bytes;
    };

    let replay_prompt = prompt_replay_bytes(raw_prompt);
    let replay_start = leading_replay_separator_len(bytes);
    let replay_bytes = &bytes[replay_start..];
    if !bytes.is_empty() && replay_bytes.is_empty() {
        return replay_bytes;
    }
    let stripped = if replay_bytes.starts_with(raw_prompt) {
        Some(&replay_bytes[raw_prompt.len()..])
    } else if replay_prompt.len() != raw_prompt.len() && replay_bytes.starts_with(replay_prompt) {
        Some(&replay_bytes[replay_prompt.len()..])
    } else {
        None
    };

    if let Some(rest) = stripped {
        if !rest.is_empty() && leading_replay_separator_len(rest) == rest.len() {
            return &rest[rest.len()..];
        }
    }

    if !bytes.is_empty() || stripped.is_some() {
        *replayed_prompt_prefix = None;
    }
    stripped.unwrap_or(bytes)
}

fn leading_replay_separator_len(bytes: &[u8]) -> usize {
    let mut idx = 0;
    while idx < bytes.len() {
        if matches!(bytes[idx], b'\r' | b'\n') {
            idx += 1;
            continue;
        }
        if bytes[idx..].starts_with(b"\x1b[?2004h") || bytes[idx..].starts_with(b"\x1b[?2004l") {
            idx += b"\x1b[?2004h".len();
            continue;
        }
        break;
    }
    idx
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
    native_candidate_echoed_len: &mut usize,
) -> io::Result<()> {
    let native_mode = prompt.is_empty();
    while let Ok(event) = input_events.try_recv() {
        match event {
            RawInputEvent::CtrlC => parser.push_control_event("ctrl_c"),
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

fn resolve_pty_emit<W: Write>(
    master: &mut File,
    parser: &OscParser,
    output: &mut W,
    action: RawObserverAction,
    display_start: &mut usize,
    replayed_prompt_prefix: &mut Option<Vec<u8>>,
) -> io::Result<RawObserverAction> {
    match action {
        RawObserverAction::EmitToPty(bytes) => {
            output.flush()?;
            master.write_all(&bytes)?;
            master.flush()?;
            Ok(RawObserverAction::Continue)
        }
        RawObserverAction::RestorePrompt => {
            output.flush()?;
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

fn prompt_replay_bytes(prompt: &[u8]) -> &[u8] {
    strip_zsh_partial_line_marker(prompt).unwrap_or(prompt)
}

fn prompt_prefixed_replay_bytes<'a>(bytes: &'a [u8], prompt: &'a [u8]) -> Cow<'a, [u8]> {
    if prompt.is_empty() || !bytes.starts_with(prompt) {
        return Cow::Borrowed(bytes);
    }

    let replay = prompt_replay_bytes(prompt);
    if replay.len() == prompt.len() {
        return Cow::Borrowed(bytes);
    }

    let mut replayed = Vec::with_capacity(replay.len() + bytes.len().saturating_sub(prompt.len()));
    replayed.extend_from_slice(replay);
    replayed.extend_from_slice(&bytes[prompt.len()..]);
    Cow::Owned(replayed)
}

fn strip_zsh_partial_line_marker(prompt: &[u8]) -> Option<&[u8]> {
    let marker_end = prompt.iter().position(|byte| *byte == b'\n')?;
    if marker_end > 512 {
        return None;
    }
    if !visible_line_is_zsh_partial_marker(&prompt[..marker_end]) {
        return None;
    }
    let after_newline = marker_end + 1;
    if prompt[after_newline..].starts_with(b"\x1b[A") {
        Some(&prompt[marker_end..])
    } else {
        Some(&prompt[after_newline..])
    }
}

fn visible_line_is_zsh_partial_marker(line: &[u8]) -> bool {
    let mut visible = Vec::new();
    let mut idx = 0;
    while idx < line.len() {
        match line[idx] {
            b'\x1b' if line.get(idx + 1) == Some(&b'[') => {
                idx += 2;
                while idx < line.len() {
                    let byte = line[idx];
                    idx += 1;
                    if byte.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            b'\r' => idx += 1,
            b'\x08' => {
                visible.pop();
                idx += 1;
            }
            byte => {
                visible.push(byte);
                idx += 1;
            }
        }
    }

    visible
        .iter()
        .all(|byte| byte.is_ascii_whitespace() || *byte == b'%')
        && visible.iter().any(|byte| *byte == b'%')
        && visible.iter().filter(|byte| **byte == b'%').count() == 1
}

#[cfg(test)]
mod tests {
    use super::{prompt_prefixed_replay_bytes, prompt_replay_bytes, strip_replayed_prompt_prefix};

    #[test]
    fn prompt_replay_strips_zsh_partial_line_marker() {
        let prompt = b"\x1b[0m\x1b[1m\x1b[7m%\x1b[27m\x1b[0m      \r\x1b[K\r\r\n\x1b[Aprompt> ";

        assert_eq!(prompt_replay_bytes(prompt), b"\n\x1b[Aprompt> ");
    }

    #[test]
    fn prompt_replay_strips_plain_zsh_percent_marker_line() {
        let prompt = b"%\r\nprompt> ";

        assert_eq!(prompt_replay_bytes(prompt), b"prompt> ");
    }

    #[test]
    fn prompt_replay_strips_styled_plain_percent_marker_line() {
        let prompt = b"\x1b[1m%\x1b[0m   \r\x1b[K\nprompt> ";

        assert_eq!(prompt_replay_bytes(prompt), b"prompt> ");
    }

    #[test]
    fn prompt_replay_keeps_literal_percent_prompt() {
        let prompt = b"usage 50% prompt> ";

        assert_eq!(prompt_replay_bytes(prompt), prompt);
    }

    #[test]
    fn prompt_replay_keeps_multiline_prompt_with_non_marker_percent() {
        let prompt = b"usage 50%\nprompt> ";

        assert_eq!(prompt_replay_bytes(prompt), prompt);
    }

    #[test]
    fn prompt_prefixed_replay_strips_marker_when_releasing_held_prompt() {
        let prompt = b"\x1b[1m%\x1b[0m   \r\x1b[K\nprompt> ";
        let display = b"\x1b[1m%\x1b[0m   \r\x1b[K\nprompt> echo after\r\n";

        assert_eq!(
            prompt_prefixed_replay_bytes(display, prompt).as_ref(),
            b"prompt> echo after\r\n"
        );
    }

    #[test]
    fn prompt_prefixed_replay_keeps_non_prompt_output() {
        let prompt = b"prompt> ";
        let display = b"%\r\nregular command output\r\n";

        assert_eq!(
            prompt_prefixed_replay_bytes(display, prompt).as_ref(),
            display
        );
    }

    #[test]
    fn replayed_prompt_prefix_is_suppressed_from_next_pty_echo() {
        let mut replayed = Some(b"prompt> ".to_vec());

        assert_eq!(
            strip_replayed_prompt_prefix(b"prompt> echo after\r\n", &mut replayed),
            b"echo after\r\n"
        );
        assert!(replayed.is_none());
    }

    #[test]
    fn replayed_prompt_prefix_suppression_tolerates_leading_newline() {
        let mut replayed = Some(b"prompt> ".to_vec());

        assert_eq!(
            strip_replayed_prompt_prefix(b"\r\nprompt> echo after\r\n", &mut replayed),
            b"echo after\r\n"
        );
        assert!(replayed.is_none());
    }

    #[test]
    fn replayed_prompt_prefix_suppression_tolerates_bracketed_paste_toggle() {
        let mut replayed = Some(b"prompt> \x1b[?2004h".to_vec());

        assert_eq!(
            strip_replayed_prompt_prefix(
                b"\x1b[?2004l\r\nprompt> \x1b[?2004hecho after\r\n",
                &mut replayed
            ),
            b"echo after\r\n"
        );
        assert!(replayed.is_none());
    }

    #[test]
    fn replayed_prompt_prefix_suppression_keeps_pending_after_control_only_slice() {
        let mut replayed = Some(b"prompt> \x1b[?2004h".to_vec());

        assert_eq!(
            strip_replayed_prompt_prefix(b"\x1b[?2004l\r\n", &mut replayed),
            b""
        );
        assert!(replayed.is_some());
        assert_eq!(
            strip_replayed_prompt_prefix(b"prompt> \x1b[?2004hecho after\r\n", &mut replayed),
            b"echo after\r\n"
        );
        assert!(replayed.is_none());
    }

    #[test]
    fn replayed_prompt_prefix_suppression_keeps_pending_after_prompt_control_only_slice() {
        let mut replayed = Some(b"prompt> \x1b[?2004h".to_vec());

        assert_eq!(
            strip_replayed_prompt_prefix(b"prompt> \x1b[?2004h\x1b[?2004l\r\n", &mut replayed),
            b""
        );
        assert!(replayed.is_some());
        assert_eq!(
            strip_replayed_prompt_prefix(b"prompt> \x1b[?2004hecho after\r\n", &mut replayed),
            b"echo after\r\n"
        );
        assert!(replayed.is_none());
    }
}
