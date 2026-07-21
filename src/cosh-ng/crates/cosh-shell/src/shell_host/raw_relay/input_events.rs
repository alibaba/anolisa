//! Raw input event delivery into the OSC parser and terminal display.

use std::io::{self, Write};
use std::sync::mpsc::Receiver;

use crate::raw_input::RawInputEvent;

use super::{clear_prompt_ghost_line, OscParser};

pub(super) fn drain_raw_input_events<W: Write>(
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
                    write!(output, "\r\x1b[2K{prompt}")?;
                    output.write_all(&input)?;
                }
                writeln!(output)?;
                output.flush()?;
            }
            RawInputEvent::PromptGhostClear => {
                clear_prompt_ghost_line(parser, output, prompt, native_candidate_echoed_len)?;
            }
            RawInputEvent::PromptGhostDismissed => parser.push_prompt_ghost_event("dismissed"),
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
                } else {
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
            RawInputEvent::CardSecretInput(id, text) => {
                parser.push_secret_card_event("input", &format!("{id}:{text}"))
            }
            RawInputEvent::CardApprove(id) => parser.push_card_event("approve", &id),
            RawInputEvent::CardAlwaysTrust(id) => parser.push_card_event("always_trust", &id),
            RawInputEvent::CardDeny(id) => parser.push_card_event("deny", &id),
            RawInputEvent::CardDetails(id) => parser.push_card_event("details", &id),
            RawInputEvent::CardCancel(id) => parser.push_card_event("cancel", &id),
            RawInputEvent::CardAnswer(answer) => parser.push_card_event("answer", &answer),
            RawInputEvent::CardSecretAnswer(answer) => {
                parser.push_secret_card_event("answer", &answer)
            }
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
            RawInputEvent::SessionFocus(id, selected) => {
                parser.push_card_event("session_focus", &format!("{id}:{selected}"))
            }
            RawInputEvent::SessionToggle(id, selected) => {
                parser.push_card_event("session_toggle", &format!("{id}:{selected}"))
            }
            RawInputEvent::SessionResume(id, selected) => {
                parser.push_card_event("session_resume", &format!("{id}:{selected}"))
            }
            RawInputEvent::SessionDelete(id) => parser.push_card_event("session_delete", &id),
            RawInputEvent::SessionClearConfirm(id) => {
                parser.push_card_event("session_clear_confirm", &id)
            }
            RawInputEvent::SessionCancel(id) => parser.push_card_event("session_cancel", &id),
        }
    }
    Ok(())
}
