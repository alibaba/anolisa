use std::fs::File;
use std::io;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use crate::input::{InputClassifier, InputDecision, InterceptReason};

use super::event_parser::{
    candidate_inline_hint, candidate_line_status, escape_candidate_allows_soft_newline,
    native_candidate_allows_soft_newline, native_candidate_should_return_to_shell,
    redact_extension_setting_value, starts_intercept_candidate, starts_native_cjk_candidate,
    starts_native_intercept_candidate, starts_native_partial_paste_opener,
    starts_native_paste_opener, CandidateLineBuffer, CandidateLineStatus, NativeLineState,
    BRACKETED_PASTE_END, BRACKETED_PASTE_START,
};
use super::generation::{LineSubmitCounter, UserPtyInputGeneration};
use super::mode::new_delay_input_mode;
use super::soft_newline::{
    contains_soft_newline_sequence, draft_text_from_bytes, render_newline_markers,
    render_soft_newline_markers,
};
use super::{write_all_pty, MainPromptGate, PromptGhostRoute, RawInputEvent, RawInputMode, CTRL_C};

pub(super) struct InputRelayContext<'a> {
    pub(super) master: &'a mut File,
    pub(super) input_classifier: &'a InputClassifier,
    pub(super) input_events: &'a Sender<RawInputEvent>,
    pub(super) input_mode: &'a Arc<Mutex<RawInputMode>>,
    pub(super) input_generation: &'a UserPtyInputGeneration,
    pub(super) line_submits: &'a mut LineSubmitCounter,
    pub(super) line_buffer: &'a mut CandidateLineBuffer,
    pub(super) native_line_state: &'a mut NativeLineState,
    pub(super) exit_tracker: &'a mut ExplicitExitTracker,
    pub(super) main_prompt_gate: &'a MainPromptGate,
    /// Routes exact slash-control submissions through bash so they enter
    /// native history (issue #1718); admission additionally requires the
    /// main prompt gate, so any submission (slash or shell) lowers the gate
    /// until the next prompt_ready marker and later bytes fall back to the
    /// Rust intercept path instead of leaking into a foreground process.
    pub(super) slash_route_enabled: bool,
}

/// Writes real user bytes to the PTY, bumping the shared input generation
/// first so replayed-prompt state armed for an older generation expires
/// before the resulting PTY output can be parsed. The event also reports how
/// many line submissions the write carried, so the output loop can match
/// them against shell prompt boundaries.
pub(super) fn write_user_bytes_to_pty(
    master: &mut File,
    input_generation: &UserPtyInputGeneration,
    line_submits: &mut LineSubmitCounter,
    input_events: &Sender<RawInputEvent>,
    main_prompt_gate: &MainPromptGate,
    bytes: &[u8],
) -> io::Result<()> {
    let line_submits = line_submits.count(bytes);
    if line_submits > 0 {
        // A submitted line leaves the primary prompt until the marker emits
        // the next prompt_ready (#1721 D16).
        main_prompt_gate.set_at_prompt(false);
    }
    let generation = input_generation.bump();
    let _ = input_events.send(RawInputEvent::PtyUserWrite {
        generation,
        line_submits,
    });
    write_all_pty(master, bytes)
}

pub(super) fn send_raw_input_events(bytes: &[u8], input_events: &Sender<RawInputEvent>) {
    if bytes.contains(&CTRL_C) {
        let _ = input_events.send(RawInputEvent::CtrlC);
    }
}

pub(super) fn send_shell_input_state(empty: bool, input_events: &Sender<RawInputEvent>) {
    let _ = input_events.send(RawInputEvent::ShellInputActivity { empty });
}

pub(super) fn relay_passthrough_input(
    bytes: &[u8],
    relay: &mut InputRelayContext<'_>,
) -> io::Result<bool> {
    relay_passthrough_input_with_activity(bytes, relay, true)
}

fn relay_passthrough_input_with_activity(
    bytes: &[u8],
    relay: &mut InputRelayContext<'_>,
    emit_activity: bool,
) -> io::Result<bool> {
    if relay.line_buffer.force_agent_intercept && relay.line_buffer.is_active() {
        relay.line_buffer.soft_newline_enabled = true;
        relay.line_buffer.push(bytes);
        if !relay.line_buffer.force_agent_intercept {
            let _ = relay.input_events.send(RawInputEvent::CandidateClearLine);
            let _ = relay.input_events.send(RawInputEvent::PromptGhostDismissed);
            if !relay.line_buffer.is_active() {
                send_shell_input_state(true, relay.input_events);
                return Ok(true);
            }
            redraw_candidate_line(relay.input_events, relay.line_buffer);
            return relay_candidate_line(relay, emit_activity);
        }
        if !relay.line_buffer.is_active() {
            relay.line_buffer.clear();
            let _ = relay.input_events.send(RawInputEvent::CandidateClearLine);
            let _ = relay.input_events.send(RawInputEvent::PromptGhostDismissed);
            send_shell_input_state(true, relay.input_events);
            return Ok(true);
        }
        redraw_candidate_line(relay.input_events, relay.line_buffer);
        return relay_candidate_line(relay, emit_activity);
    }
    if relay.input_classifier.is_conservative() {
        return relay_native_passthrough(bytes, relay, emit_activity);
    }
    if relay.line_buffer.is_active() || starts_intercept_candidate(bytes) {
        // Slash commands never compose: pasted newlines keep the pre-#1721
        // submit semantics (matrix #17 scope, same rule as native D10). The
        // held partial delimiter must join the classification input so a
        // split opener cannot mask a slash prefix (#1721).
        let combined: Vec<u8> = [
            relay.line_buffer.bytes.as_slice(),
            relay.line_buffer.pending_partial_bytes(),
            bytes,
        ]
        .concat();
        relay.line_buffer.soft_newline_enabled = escape_candidate_allows_soft_newline(&combined);
        relay.line_buffer.push(bytes);
        redraw_candidate_line(relay.input_events, relay.line_buffer);
        return relay_candidate_line(relay, emit_activity);
    }

    // A gated soft-newline shortcut upgrades the bash-owned line into the
    // draft card; fallback strips the sequence (#1932 F6). The tip only
    // fires when no upgrade happened.
    let handled = handle_prompt_line_soft_newline(bytes, relay)?;
    if matches!(handled, PromptLineSoftNewline::Upgraded) {
        return Ok(true);
    }
    observe_passthrough_soft_newline(bytes, relay.input_events);
    let bytes = match &handled {
        PromptLineSoftNewline::Stripped(stripped) => stripped.as_slice(),
        _ => bytes,
    };
    send_raw_input_events(bytes, relay.input_events);
    relay.native_line_state.observe_shell_bytes(bytes);
    if emit_activity && !bytes.is_empty() {
        send_shell_input_state(relay.native_line_state.is_empty(), relay.input_events);
    }
    relay.exit_tracker.observe_shell_bytes(bytes);
    write_user_bytes_to_pty(
        relay.master,
        relay.input_generation,
        relay.line_submits,
        relay.input_events,
        relay.main_prompt_gate,
        bytes,
    )?;
    Ok(false)
}

pub(super) fn relay_prompt_ghost_input(
    bytes: &[u8],
    ghost_text: &str,
    route: &PromptGhostRoute,
    relay: &mut InputRelayContext<'_>,
) -> io::Result<bool> {
    if bytes.starts_with(b"\x1b[Z") {
        if let PromptGhostRoute::AgentSelection {
            candidates, active, ..
        } = route
        {
            if candidates.len() > 1 {
                let next = (active + 1) % candidates.len();
                let candidate = &candidates[next];
                let next_route = PromptGhostRoute::AgentSelection {
                    candidates: candidates.clone(),
                    active: next,
                };
                if let Ok(mut mode) = relay.input_mode.lock() {
                    *mode = RawInputMode::PromptGhost {
                        text: candidate.text.clone(),
                        route: next_route.clone(),
                    };
                }
                let _ = relay.input_events.send(RawInputEvent::PromptGhostCycle {
                    text: candidate.text.clone(),
                });
                let remainder = &bytes[3..];
                if !remainder.is_empty() {
                    return relay_prompt_ghost_input(
                        remainder,
                        &candidate.text,
                        &next_route,
                        relay,
                    );
                }
                return Ok(true);
            }
        }
    }
    if matches!(bytes.first(), Some(b'\r' | b'\n')) {
        if let PromptGhostRoute::AgentSelection {
            candidates, active, ..
        } = route
        {
            if let Some(candidate) = candidates.get(*active) {
                let _ = relay.input_events.send(RawInputEvent::PromptGhostClear);
                let _ = relay.input_events.send(RawInputEvent::CandidateCommit(
                    candidate.text.as_bytes().to_vec(),
                ));
                let _ = relay
                    .input_events
                    .send(RawInputEvent::PromptGhostIntercept {
                        input: candidate.text.clone(),
                        suggestion_id: Some(candidate.suggestion_id.clone()),
                    });
                send_shell_input_state(true, relay.input_events);
                if let Ok(mut mode) = relay.input_mode.lock() {
                    *mode = new_delay_input_mode();
                }
                return Ok(true);
            }
        }
    }
    if bytes.starts_with(b"\t") && !relay.line_buffer.is_active() {
        let _ = relay.input_events.send(RawInputEvent::PromptGhostClear);
        let remainder = &bytes[1..];
        match route {
            PromptGhostRoute::NativeShell => {
                if let Ok(mut mode) = relay.input_mode.lock() {
                    *mode = RawInputMode::RawPassthrough;
                }
                relay
                    .native_line_state
                    .observe_shell_bytes(ghost_text.as_bytes());
                relay
                    .exit_tracker
                    .observe_shell_bytes(ghost_text.as_bytes());
                write_user_bytes_to_pty(
                    relay.master,
                    relay.input_generation,
                    relay.line_submits,
                    relay.input_events,
                    relay.main_prompt_gate,
                    ghost_text.as_bytes(),
                )?;
                if !remainder.is_empty() {
                    send_raw_input_events(remainder, relay.input_events);
                    relay.native_line_state.observe_shell_bytes(remainder);
                    relay.exit_tracker.observe_shell_bytes(remainder);
                    write_user_bytes_to_pty(
                        relay.master,
                        relay.input_generation,
                        relay.line_submits,
                        relay.input_events,
                        relay.main_prompt_gate,
                        remainder,
                    )?;
                }
            }
            PromptGhostRoute::AgentIntercept { suggestion_id } => {
                let _ = relay.input_events.send(RawInputEvent::PromptGhostAccepted {
                    suggestion_id: suggestion_id.clone(),
                });
                relay.line_buffer.soft_newline_enabled = true;
                relay.line_buffer.push(ghost_text.as_bytes());
                relay.line_buffer.force_agent_intercept = true;
                relay.line_buffer.forced_agent_suggestion_id = suggestion_id.clone();
                redraw_candidate_line(relay.input_events, relay.line_buffer);
                if let Ok(mut mode) = relay.input_mode.lock() {
                    *mode = RawInputMode::Passthrough;
                }
                if !remainder.is_empty() {
                    relay_passthrough_input(remainder, relay)?;
                }
            }
            PromptGhostRoute::AgentSelection {
                candidates, active, ..
            } => {
                let suggestion_id = candidates
                    .get(*active)
                    .map(|candidate| candidate.suggestion_id.clone());
                let _ = relay.input_events.send(RawInputEvent::PromptGhostAccepted {
                    suggestion_id: suggestion_id.clone(),
                });
                relay.line_buffer.soft_newline_enabled = true;
                relay.line_buffer.push(ghost_text.as_bytes());
                relay.line_buffer.force_agent_intercept = true;
                relay.line_buffer.forced_agent_suggestion_id = suggestion_id;
                redraw_candidate_line(relay.input_events, relay.line_buffer);
                if let Ok(mut mode) = relay.input_mode.lock() {
                    *mode = RawInputMode::Passthrough;
                }
                if !remainder.is_empty() {
                    relay_passthrough_input(remainder, relay)?;
                }
            }
        }
        return Ok(true);
    }
    dismiss_prompt_ghost_input(bytes, relay)
}

pub(super) fn dismiss_prompt_ghost_input(
    bytes: &[u8],
    relay: &mut InputRelayContext<'_>,
) -> io::Result<bool> {
    if let Ok(mut mode) = relay.input_mode.lock() {
        *mode = RawInputMode::Passthrough;
    }
    let _ = relay.input_events.send(RawInputEvent::PromptGhostClear);
    let _ = relay.input_events.send(RawInputEvent::PromptGhostDismissed);
    relay_passthrough_input(bytes, relay)
}

pub(super) fn send_held_input_events(bytes: &[u8], input_events: &Sender<RawInputEvent>) {
    send_raw_input_events(bytes, input_events);
    if held_input_requests_cancel(bytes) {
        let _ = input_events.send(RawInputEvent::CtrlC);
    }
}

pub(super) fn relay_delayed_input(
    bytes: &[u8],
    relay: &mut InputRelayContext<'_>,
) -> io::Result<()> {
    if bytes.contains(&CTRL_C) {
        let _ = relay.input_events.send(RawInputEvent::CtrlC);
        relay.line_buffer.clear();
        relay.native_line_state.clear();
        send_shell_input_state(true, relay.input_events);
        return Ok(());
    }
    if relay_passthrough_input_with_activity(bytes, relay, false)? {
        return Ok(());
    }
    Ok(())
}

fn relay_native_passthrough(
    bytes: &[u8],
    relay: &mut InputRelayContext<'_>,
    emit_activity: bool,
) -> io::Result<bool> {
    if relay.line_buffer.is_active()
        || starts_native_intercept_candidate(bytes, relay.native_line_state)
        || (relay.main_prompt_gate.is_at_prompt()
            && (starts_native_cjk_candidate(bytes, relay.native_line_state)
                || starts_native_paste_opener(bytes, relay.native_line_state)
                || starts_native_partial_paste_opener(bytes, relay.native_line_state)))
    {
        // Route flags must consider the whole draft so far: a bracketed
        // paste opener may arrive as its own chunk (or split mid-delimiter,
        // #1721) before the payload decides CJK vs slash (#1721 D13).
        let combined: Vec<u8> = [
            relay.line_buffer.bytes.as_slice(),
            relay.line_buffer.pending_partial_bytes(),
            bytes,
        ]
        .concat();
        relay.line_buffer.soft_newline_enabled = native_candidate_allows_soft_newline(&combined);
        relay.line_buffer.push(bytes);
        if native_candidate_should_return_to_shell(relay.input_classifier, relay.line_buffer) {
            return flush_candidate_line_to_shell(relay, emit_activity);
        }
        // Control bytes such as Tab must reach readline without first changing
        // the outer terminal cursor, whose display width differs from byte count.
        redraw_candidate_line(relay.input_events, relay.line_buffer);
        return relay_candidate_line(relay, emit_activity);
    }
    // Non-slash input: send directly to PTY. Shell marker's preexec/
    // command_not_found hooks handle NL/CJK intercept on the shell side.
    // Same soft-newline handling as the escape path (#1932 F6).
    let handled = handle_prompt_line_soft_newline(bytes, relay)?;
    if matches!(handled, PromptLineSoftNewline::Upgraded) {
        return Ok(true);
    }
    observe_passthrough_soft_newline(bytes, relay.input_events);
    let bytes = match &handled {
        PromptLineSoftNewline::Stripped(stripped) => stripped.as_slice(),
        _ => bytes,
    };
    send_raw_input_events(bytes, relay.input_events);
    relay.native_line_state.observe_shell_bytes(bytes);
    if emit_activity && !bytes.is_empty() {
        send_shell_input_state(relay.native_line_state.is_empty(), relay.input_events);
    }
    relay.exit_tracker.observe_shell_bytes(bytes);
    write_user_bytes_to_pty(
        relay.master,
        relay.input_generation,
        relay.line_submits,
        relay.input_events,
        relay.main_prompt_gate,
        bytes,
    )?;
    Ok(false)
}

fn relay_candidate_line(
    relay: &mut InputRelayContext<'_>,
    emit_activity: bool,
) -> io::Result<bool> {
    // A bracketed paste is still streaming: defer every routing decision
    // (submit, flush, card upgrade) until the closer arrives so embedded
    // newlines can never execute early (#1721).
    if relay.line_buffer.in_paste() {
        return Ok(true);
    }
    match candidate_line_status(
        &relay.line_buffer.bytes,
        relay.line_buffer.soft_newline_enabled,
    ) {
        CandidateLineStatus::Pending => Ok(true),
        CandidateLineStatus::Unsafe if relay.line_buffer.force_agent_intercept => {
            relay.line_buffer.clear();
            let _ = relay.input_events.send(RawInputEvent::CandidateClearLine);
            let _ = relay.input_events.send(RawInputEvent::PromptGhostDismissed);
            send_shell_input_state(true, relay.input_events);
            Ok(true)
        }
        CandidateLineStatus::Unsafe => flush_candidate_line_to_shell(relay, emit_activity),
        CandidateLineStatus::Complete { line, line_len } => {
            let force_agent_intercept = relay.line_buffer.force_agent_intercept;
            let suggestion_id = relay.line_buffer.forced_agent_suggestion_id.clone();
            let saw_paste = relay.line_buffer.saw_paste();
            let mut bytes = relay.line_buffer.take();
            let remainder = bytes.split_off(line_len);
            if force_agent_intercept {
                let _ = relay.input_events.send(RawInputEvent::CandidateCommit(
                    redact_extension_setting_value(line.as_bytes()),
                ));
                if let Ok(mut mode) = relay.input_mode.lock() {
                    *mode = new_delay_input_mode();
                }
                let _ = relay
                    .input_events
                    .send(RawInputEvent::PromptGhostIntercept {
                        input: line,
                        suggestion_id,
                    });
                send_shell_input_state(true, relay.input_events);
                if !remainder.is_empty() {
                    relay_passthrough_input_with_activity(&remainder, relay, emit_activity)?;
                }
                return Ok(true);
            }
            if line.contains('\n') {
                // Soft newlines only originate from prompt composition inside
                // the candidate buffer, so multi-line text always routes to
                // the agent before classification (#1721 D7). Whitespace-only
                // drafts are consumed instead of submitting an empty prompt.
                if line.trim().is_empty() {
                    let _ = relay.input_events.send(RawInputEvent::CandidateClearLine);
                    send_shell_input_state(true, relay.input_events);
                } else {
                    let reason = if line.trim_start().starts_with("??") {
                        InterceptReason::AgentMarker
                    } else {
                        InterceptReason::NaturalLanguage
                    };
                    let _ = relay.input_events.send(RawInputEvent::CandidateCommit(
                        render_newline_markers(&redact_extension_setting_value(line.as_bytes())),
                    ));
                    if let Ok(mut mode) = relay.input_mode.lock() {
                        *mode = new_delay_input_mode();
                    }
                    let _ = relay
                        .input_events
                        .send(RawInputEvent::UserIntercept(line, reason));
                    send_shell_input_state(true, relay.input_events);
                }
                if !remainder.is_empty() {
                    relay_passthrough_input_with_activity(&remainder, relay, emit_activity)?;
                }
                return Ok(true);
            }
            if line.trim() == "??" {
                // A lone `??` submit opens an empty prompt draft (#1932):
                // the terminal-agnostic entry into multi-line composition,
                // mirroring the soft-newline upgrade in redraw_candidate_line.
                let _ = relay.input_events.send(RawInputEvent::CandidateClearLine);
                let _ = relay.input_events.send(RawInputEvent::PromptDraftOpen {
                    text: String::new(),
                });
                send_shell_input_state(true, relay.input_events);
                if !remainder.is_empty() {
                    relay_passthrough_input_with_activity(&remainder, relay, emit_activity)?;
                }
                return Ok(true);
            }
            match relay.input_classifier.classify(&line) {
                InputDecision::Intercept { input, reason } => {
                    if reason == InterceptReason::Slash
                        && relay.slash_route_enabled
                        && relay.main_prompt_gate.is_at_prompt()
                        && relay.input_classifier.is_exact_slash_control_command(
                            line.split_whitespace().next().unwrap_or_default(),
                        )
                    {
                        // Submit the exact slash line through bash so
                        // readline records it in native history (issue
                        // #1718). The shell marker's DEBUG trap intercepts
                        // it at the prompt boundary (#1724) and emits the
                        // same UserInputIntercepted event as the Rust path
                        // below. write_user_bytes_to_pty lowers the prompt
                        // gate for this and every other line submission, so
                        // follow-up slash bytes can never route into a
                        // foreground process before the next prompt_ready.
                        return submit_line_bytes_to_shell(relay, bytes, remainder, emit_activity);
                    }
                    let _ = relay.input_events.send(RawInputEvent::CandidateCommit(
                        redact_extension_setting_value(line.as_bytes()),
                    ));
                    if let Ok(mut mode) = relay.input_mode.lock() {
                        *mode = new_delay_input_mode();
                    }
                    let _ = relay
                        .input_events
                        .send(RawInputEvent::UserIntercept(input, reason));
                    send_shell_input_state(true, relay.input_events);
                    if !remainder.is_empty() {
                        relay_passthrough_input_with_activity(&remainder, relay, emit_activity)?;
                    }
                    Ok(true)
                }
                InputDecision::SendToShell(_) => {
                    let mut bytes = bytes;
                    let mut remainder = remainder;
                    if saw_paste {
                        // Replay the whole pasted region as one bracketed
                        // paste: bash inserts the bytes (incl. embedded
                        // newlines) and waits for explicit Enter (#1721).
                        bytes.extend_from_slice(&remainder);
                        remainder = Vec::new();
                        let mut wrapped = Vec::with_capacity(bytes.len() + 12);
                        wrapped.extend_from_slice(b"\x1b[200~");
                        wrapped.extend_from_slice(&bytes);
                        wrapped.extend_from_slice(b"\x1b[201~");
                        bytes = wrapped;
                    }
                    submit_line_bytes_to_shell(relay, bytes, remainder, emit_activity)
                }
                InputDecision::Consume => {
                    let _ = relay.input_events.send(RawInputEvent::CandidateClearLine);
                    send_shell_input_state(true, relay.input_events);
                    if !remainder.is_empty() {
                        relay_passthrough_input_with_activity(&remainder, relay, emit_activity)?;
                    }
                    Ok(false)
                }
            }
        }
    }
}

fn flush_candidate_line_to_shell(
    relay: &mut InputRelayContext<'_>,
    emit_activity: bool,
) -> io::Result<bool> {
    let saw_paste = relay.line_buffer.saw_paste();
    let mut bytes = relay.line_buffer.take();
    if saw_paste {
        // The draft absorbed a bracketed paste: replay the wrapper so bash's
        // readline treats the bytes as pasted data instead of executing any
        // embedded newlines immediately (#1721).
        let mut wrapped = Vec::with_capacity(bytes.len() + 12);
        wrapped.extend_from_slice(b"\x1b[200~");
        wrapped.extend_from_slice(&bytes);
        wrapped.extend_from_slice(b"\x1b[201~");
        bytes = wrapped;
    }
    submit_line_bytes_to_shell(relay, bytes, Vec::new(), emit_activity)
}

/// Clears the cosh-echoed candidate line and writes the taken bytes to the
/// PTY, then relays any remainder. Shared by SendToShell submissions, unsafe
/// candidate flushes, and shell-routed slash submissions (issue #1718).
fn submit_line_bytes_to_shell(
    relay: &mut InputRelayContext<'_>,
    bytes: Vec<u8>,
    remainder: Vec<u8>,
    emit_activity: bool,
) -> io::Result<bool> {
    let _ = relay.input_events.send(RawInputEvent::CandidateClearLine);
    send_raw_input_events(&bytes, relay.input_events);
    relay.native_line_state.observe_shell_bytes(&bytes);
    if emit_activity && !bytes.is_empty() {
        send_shell_input_state(relay.native_line_state.is_empty(), relay.input_events);
    }
    relay.exit_tracker.observe_shell_bytes(&bytes);
    write_user_bytes_to_pty(
        relay.master,
        relay.input_generation,
        relay.line_submits,
        relay.input_events,
        relay.main_prompt_gate,
        &bytes,
    )?;
    if !remainder.is_empty() {
        relay_passthrough_input_with_activity(&remainder, relay, emit_activity)?;
    }
    Ok(false)
}

fn redraw_candidate_line(
    input_events: &Sender<RawInputEvent>,
    line_buffer: &mut CandidateLineBuffer,
) {
    let original = line_buffer.visible_line_bytes();
    send_shell_input_state(original.is_empty(), input_events);
    if line_buffer.soft_newline_enabled
        && !line_buffer.in_paste()
        && contains_soft_newline_sequence(original)
    {
        // First soft newline upgrades the draft into the prompt card
        // (#1721 D13): erase the inline echo, hand the buffered text to the
        // runtime, and let the capture own every following keystroke. The
        // leading `??` agent-marker is a routing gesture, not content
        // (#1932): strip it so the card opens with the prompt itself.
        let text = draft_text_from_bytes(original);
        let text = match text.strip_prefix("??") {
            Some(rest) => rest.trim_start_matches(' ').to_string(),
            None => text,
        };
        let _ = input_events.send(RawInputEvent::CandidateClearLine);
        let _ = input_events.send(RawInputEvent::PromptDraftOpen { text });
        line_buffer.clear();
        return;
    }
    let visible = redact_extension_setting_value(original);
    let hint = std::str::from_utf8(&visible)
        .ok()
        .and_then(candidate_inline_hint);
    let display = visible;
    line_buffer.relayed_len = display.len();
    let _ = input_events.send(RawInputEvent::CandidateRedraw {
        input: display,
        hint,
    });
}

/// Observe-only discoverability probe (#1721 T-c): when a soft-newline
/// shortcut is seen on a passthrough path (candidate buffer inactive), emit
/// a signal so the runtime can surface a one-time tip at the next
/// prompt-ready. The bytes themselves are always relayed unchanged.
fn observe_passthrough_soft_newline(bytes: &[u8], input_events: &Sender<RawInputEvent>) {
    if contains_soft_newline_sequence(bytes) {
        let _ = input_events.send(RawInputEvent::SoftNewlineShortcutObserved);
    }
    observe_passthrough_multiline_paste(bytes, input_events);
}

/// #1932 F5: a bracketed paste with embedded newlines relayed straight to
/// bash line-executes on paste-unaware readline setups. Observe-only:
/// best-effort single-chunk scan feeding the failure-insight hint; never
/// touches the relayed bytes.
fn observe_passthrough_multiline_paste(bytes: &[u8], input_events: &Sender<RawInputEvent>) {
    let Some(start) = bytes
        .windows(BRACKETED_PASTE_START.len())
        .position(|window| window == BRACKETED_PASTE_START)
    else {
        return;
    };
    let payload = &bytes[start + BRACKETED_PASTE_START.len()..];
    let payload_end = payload
        .windows(BRACKETED_PASTE_END.len())
        .position(|window| window == BRACKETED_PASTE_END)
        .unwrap_or(payload.len());
    if payload[..payload_end]
        .iter()
        .any(|byte| matches!(byte, b'\r' | b'\n'))
    {
        let _ = input_events.send(RawInputEvent::MultilinePasteObserved);
    }
}

fn held_input_requests_cancel(bytes: &[u8]) -> bool {
    String::from_utf8_lossy(bytes)
        .lines()
        .any(|line| line.split_whitespace().next() == Some("/cancel"))
}

mod exit_tracker;
mod soft_newline_upgrade;
pub(super) use exit_tracker::ExplicitExitTracker;
use soft_newline_upgrade::{handle_prompt_line_soft_newline, PromptLineSoftNewline};

#[cfg(test)]
#[path = "relay_tests.rs"]
mod tests;
