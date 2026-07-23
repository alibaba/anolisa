use super::{RawInputCapture, RawInputEvent, CTRL_C};
use crate::question::choices::{
    question_choice_count as shared_question_choice_count, toggle_question_option,
};
use crate::ui::{
    approval_action_at, hook_approval_action_at, ApprovalPanelAction, APPROVAL_PANEL_ACTIONS,
};

mod navigation;

#[derive(Debug, Default)]
pub(super) struct CardInputState {
    selected: usize,
    free_text: String,
    active_kind: Option<CardInputKind>,
    selected_options: Vec<usize>,
    pending_escape: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CardInputKind {
    Question {
        id: String,
        option_count: usize,
        allow_free_text: bool,
        multiple: bool,
        secret: bool,
    },
    Approval {
        id: String,
    },
    Mode {
        id: String,
        option_count: usize,
    },
    Config {
        id: String,
        option_count: usize,
    },
    ConfigLanguage {
        id: String,
        option_count: usize,
    },
    Session {
        id: String,
        option_count: usize,
        confirming_clear: bool,
    },
    Evidence {
        id: String,
    },
}

impl CardInputState {
    pub(super) fn apply_capture(&mut self, capture: &RawInputCapture) {
        let kind = match capture {
            RawInputCapture::Question {
                id,
                option_count,
                allow_free_text,
                multiple,
                secret,
            } => CardInputKind::Question {
                id: id.clone(),
                option_count: *option_count,
                allow_free_text: *allow_free_text,
                multiple: *multiple,
                secret: *secret,
            },
            RawInputCapture::Approval { id, .. } | RawInputCapture::Consultation { id } => {
                CardInputKind::Approval { id: id.clone() }
            }
            RawInputCapture::Mode {
                id, option_count, ..
            } => CardInputKind::Mode {
                id: id.clone(),
                option_count: *option_count,
            },
            RawInputCapture::Config {
                id, option_count, ..
            } => CardInputKind::Config {
                id: id.clone(),
                option_count: *option_count,
            },
            RawInputCapture::ConfigLanguage {
                id, option_count, ..
            } => CardInputKind::ConfigLanguage {
                id: id.clone(),
                option_count: *option_count,
            },
            RawInputCapture::Session {
                id,
                option_count,
                confirming_clear,
                ..
            } => CardInputKind::Session {
                id: id.clone(),
                option_count: *option_count,
                confirming_clear: *confirming_clear,
            },
            RawInputCapture::Evidence { id } => CardInputKind::Evidence { id: id.clone() },
        };
        if self.active_kind.as_ref() != Some(&kind) {
            let selected = match capture {
                RawInputCapture::Mode {
                    selected,
                    option_count,
                    ..
                }
                | RawInputCapture::Config {
                    selected,
                    option_count,
                    ..
                }
                | RawInputCapture::ConfigLanguage {
                    selected,
                    option_count,
                    ..
                }
                | RawInputCapture::Session {
                    selected,
                    option_count,
                    ..
                } => (*selected).min(option_count.saturating_sub(1)),
                _ => 0,
            };
            self.active_kind = Some(kind);
            self.selected = selected;
            self.free_text.clear();
            self.selected_options.clear();
            self.pending_escape.clear();
        }
    }

    pub(super) fn reset(&mut self) {
        self.active_kind = None;
        self.selected = 0;
        self.free_text.clear();
        self.selected_options.clear();
        self.pending_escape.clear();
    }

    pub(super) fn consume(
        &mut self,
        capture: &RawInputCapture,
        bytes: &[u8],
    ) -> Vec<RawInputEvent> {
        let mut events = Vec::new();
        let mut input = Vec::new();
        if self.pending_escape.is_empty() {
            input.extend_from_slice(bytes);
        } else {
            input.append(&mut self.pending_escape);
            input.extend_from_slice(bytes);
        }
        let mut idx = 0;
        while idx < input.len() {
            match input[idx] {
                CTRL_C => {
                    match capture {
                        RawInputCapture::Approval { id, .. }
                        | RawInputCapture::Consultation { id } => {
                            events.push(RawInputEvent::CardCancel(id.clone()))
                        }
                        RawInputCapture::Mode { id, .. } => {
                            events.push(RawInputEvent::ModeCancel(id.clone()))
                        }
                        RawInputCapture::Config { id, .. } => {
                            events.push(RawInputEvent::ConfigCancel(id.clone()))
                        }
                        RawInputCapture::ConfigLanguage { id, .. } => {
                            events.push(RawInputEvent::ConfigLanguageCancel(id.clone()))
                        }
                        RawInputCapture::Session { id, .. } => {
                            events.push(RawInputEvent::SessionCancel(id.clone()))
                        }
                        RawInputCapture::Question { id, .. } => {
                            events.push(RawInputEvent::QuestionCancel(id.clone()))
                        }
                        RawInputCapture::Evidence { id } => {
                            events.push(RawInputEvent::EvidenceCancel(id.clone()))
                        }
                    }
                    idx += 1;
                }
                b'\r' | b'\n' => {
                    let event = self.submit(capture);
                    let preserve_for_retry = matches!(
                        (&event, capture),
                        (
                            Some(RawInputEvent::CardAnswer(_)),
                            RawInputCapture::Question { secret: false, .. }
                        )
                    );
                    if let Some(event) = event {
                        events.push(event);
                    }
                    if !preserve_for_retry {
                        self.free_text.clear();
                    }
                    idx += 1;
                }
                0x7f | 0x08 => {
                    if self.free_text.pop().is_some() {
                        if let Some(event) = self.input_event(capture) {
                            events.push(event);
                        }
                    }
                    idx += 1;
                }
                0x09 => {
                    if let Some(event) = self.apply_tab(capture) {
                        events.push(event);
                    }
                    idx += 1;
                }
                0x1b if input.get(idx + 1) == Some(&b'[') => {
                    let Some(next_idx) =
                        self.consume_csi_sequence(capture, &input, idx, &mut events)
                    else {
                        self.pending_escape.extend_from_slice(&input[idx..]);
                        break;
                    };
                    idx = next_idx;
                }
                0x1b if input.get(idx + 1) == Some(&b'O') => {
                    if input.get(idx + 2).is_none() {
                        self.pending_escape.extend_from_slice(&input[idx..]);
                        break;
                    }
                    if let Some(event) = self.apply_arrow(capture, input[idx + 2]) {
                        events.push(event);
                    }
                    idx += 3;
                }
                0x1b if input.get(idx + 1) == Some(&0x1b) => {
                    match capture {
                        RawInputCapture::Approval { id, .. }
                        | RawInputCapture::Consultation { id } => {
                            events.push(RawInputEvent::CardCancel(id.clone()))
                        }
                        RawInputCapture::Mode { id, .. } => {
                            events.push(RawInputEvent::ModeCancel(id.clone()))
                        }
                        RawInputCapture::Config { id, .. } => {
                            events.push(RawInputEvent::ConfigCancel(id.clone()))
                        }
                        RawInputCapture::ConfigLanguage { id, .. } => {
                            events.push(RawInputEvent::ConfigLanguageCancel(id.clone()))
                        }
                        RawInputCapture::Session { id, .. } => {
                            events.push(RawInputEvent::SessionCancel(id.clone()))
                        }
                        RawInputCapture::Question { id, .. } => {
                            events.push(RawInputEvent::QuestionCancel(id.clone()))
                        }
                        RawInputCapture::Evidence { id } => {
                            events.push(RawInputEvent::EvidenceCancel(id.clone()))
                        }
                    }
                    idx += 2;
                }
                0x1b if input.get(idx + 1).is_none() => {
                    events.push(cancel_event(capture));
                    break;
                }
                0x1b => match capture {
                    RawInputCapture::Approval { id, .. } | RawInputCapture::Consultation { id } => {
                        events.push(RawInputEvent::CardCancel(id.clone()));
                        break;
                    }
                    RawInputCapture::Mode { id, .. } => {
                        events.push(RawInputEvent::ModeCancel(id.clone()));
                        break;
                    }
                    RawInputCapture::Config { id, .. } => {
                        events.push(RawInputEvent::ConfigCancel(id.clone()));
                        break;
                    }
                    RawInputCapture::ConfigLanguage { id, .. } => {
                        events.push(RawInputEvent::ConfigLanguageCancel(id.clone()));
                        break;
                    }
                    RawInputCapture::Session { id, .. } => {
                        events.push(RawInputEvent::SessionCancel(id.clone()));
                        break;
                    }
                    RawInputCapture::Question { id, .. } => {
                        events.push(RawInputEvent::QuestionCancel(id.clone()));
                        break;
                    }
                    RawInputCapture::Evidence { id } => {
                        events.push(RawInputEvent::EvidenceCancel(id.clone()));
                        break;
                    }
                },
                byte if !byte.is_ascii_control() => match capture {
                    RawInputCapture::Evidence { id } => {
                        if byte == b'i' || byte == b'I' {
                            events.push(RawInputEvent::EvidenceIgnore(id.clone()));
                        }
                        idx += 1;
                    }
                    RawInputCapture::Approval { id, .. } | RawInputCapture::Consultation { id } => {
                        if (byte == b'd' || byte == b'D') && self.free_text.is_empty() {
                            self.selected = 2;
                            events.push(RawInputEvent::CardDetails(id.clone()));
                        } else if byte.is_ascii() {
                            self.free_text.push(byte as char);
                        } else {
                            let start = idx;
                            while idx < input.len()
                                && !input[idx].is_ascii_control()
                                && input[idx] != 0x1b
                            {
                                idx += 1;
                            }
                            self.free_text
                                .push_str(&String::from_utf8_lossy(&input[start..idx]));
                            continue;
                        }
                        idx += 1;
                    }
                    RawInputCapture::Mode { .. }
                    | RawInputCapture::Config { .. }
                    | RawInputCapture::ConfigLanguage { .. } => {
                        idx += 1;
                    }
                    RawInputCapture::Session {
                        id,
                        option_count,
                        confirming_clear,
                        ..
                    } => {
                        match byte {
                            b'j' | b'J' if !*confirming_clear => {
                                let previous = self.selected;
                                self.selected =
                                    (self.selected + 1).min(option_count.saturating_sub(1));
                                if self.selected != previous {
                                    events.push(RawInputEvent::SessionFocus(
                                        id.clone(),
                                        self.selected,
                                    ));
                                }
                            }
                            b'k' | b'K' if !*confirming_clear => {
                                let previous = self.selected;
                                self.selected = self.selected.saturating_sub(1);
                                if self.selected != previous {
                                    events.push(RawInputEvent::SessionFocus(
                                        id.clone(),
                                        self.selected,
                                    ));
                                }
                            }
                            b' ' if !*confirming_clear && self.selected < *option_count => {
                                events
                                    .push(RawInputEvent::SessionToggle(id.clone(), self.selected));
                            }
                            b'd' | b'D' if !*confirming_clear => {
                                events.push(RawInputEvent::SessionDelete(id.clone()));
                            }
                            b'y' | b'Y' if *confirming_clear => {
                                events.push(RawInputEvent::SessionClearConfirm(id.clone()));
                            }
                            b'n' | b'N' if *confirming_clear => {
                                events.push(RawInputEvent::SessionCancel(id.clone()));
                            }
                            _ => {}
                        }
                        idx += 1;
                    }
                    RawInputCapture::Question {
                        id,
                        option_count,
                        multiple,
                        ..
                    } => {
                        if *multiple
                            && byte == b' '
                            && self.selected < *option_count
                            && self.free_text.is_empty()
                        {
                            toggle_question_option(&mut self.selected_options, self.selected);
                            events.push(RawInputEvent::CardToggle(id.clone(), self.selected));
                            idx += 1;
                            continue;
                        }
                        if byte.is_ascii() {
                            self.free_text.push(byte as char);
                            if let Some(event) = self.input_event(capture) {
                                events.push(event);
                            }
                            idx += 1;
                        } else {
                            let start = idx;
                            while idx < input.len()
                                && !input[idx].is_ascii_control()
                                && input[idx] != 0x1b
                            {
                                idx += 1;
                            }
                            self.free_text
                                .push_str(&String::from_utf8_lossy(&input[start..idx]));
                            if let Some(event) = self.input_event(capture) {
                                events.push(event);
                            }
                        }
                    }
                },
                _ => {
                    idx += 1;
                }
            }
        }
        events
    }

    fn submit(&self, capture: &RawInputCapture) -> Option<RawInputEvent> {
        match capture {
            RawInputCapture::Question {
                id,
                option_count,
                allow_free_text,
                multiple,
                secret,
            } => {
                let answer = self.free_text.trim();
                if is_removed_question_answer_slash(answer) {
                    return None;
                }
                if *multiple {
                    if !answer.is_empty() && *allow_free_text {
                        if self.selected_options.is_empty() {
                            return Some(card_answer_event(answer, *secret));
                        }
                        return Some(card_answer_event(
                            &format!(
                                "{}\n{}",
                                selected_options_answer(&self.selected_options),
                                answer
                            ),
                            *secret,
                        ));
                    }
                    if self.selected_options.is_empty() {
                        return Some(empty_question_submission(id, *secret));
                    }
                    return Some(card_answer_event(
                        &selected_options_answer(&self.selected_options),
                        *secret,
                    ));
                }
                if !answer.is_empty() && *allow_free_text {
                    return Some(card_answer_event(answer, *secret));
                }
                if self.selected < *option_count {
                    return Some(card_answer_event(&(self.selected + 1).to_string(), *secret));
                }
                if !answer.is_empty() {
                    return Some(card_answer_event(answer, *secret));
                }
                if *allow_free_text && self.selected == *option_count {
                    return Some(empty_question_submission(id, *secret));
                }
                if *allow_free_text && *option_count == 0 {
                    return Some(empty_question_submission(id, *secret));
                }
                None
            }
            RawInputCapture::Approval { id, .. } | RawInputCapture::Consultation { id } => {
                if !self.free_text.trim().is_empty() {
                    return None;
                }
                let action = if matches!(capture, RawInputCapture::Approval { is_hook: true, .. }) {
                    hook_approval_action_at(self.selected)
                } else {
                    approval_action_at(self.selected)
                };
                action.map(|a| approval_event_for_action(id, a))
            }
            RawInputCapture::Mode {
                id, option_count, ..
            } => {
                if *option_count == 0 || self.selected >= *option_count {
                    return None;
                }
                Some(RawInputEvent::ModeSet(id.clone(), self.selected))
            }
            RawInputCapture::Config {
                id, option_count, ..
            } => {
                if *option_count == 0 || self.selected >= *option_count {
                    return None;
                }
                if self.selected == 0 {
                    Some(RawInputEvent::ConfigSave(id.clone()))
                } else {
                    Some(RawInputEvent::ConfigCancel(id.clone()))
                }
            }
            RawInputCapture::ConfigLanguage {
                id, option_count, ..
            } => {
                if *option_count == 0 || self.selected >= *option_count {
                    return None;
                }
                Some(RawInputEvent::ConfigLanguageSet(id.clone(), self.selected))
            }
            RawInputCapture::Session {
                id,
                option_count,
                confirming_clear,
                ..
            } => {
                if *confirming_clear {
                    return Some(RawInputEvent::SessionClearConfirm(id.clone()));
                }
                if *option_count == 0 || self.selected >= *option_count {
                    return None;
                }
                Some(RawInputEvent::SessionResume(id.clone(), self.selected))
            }
            RawInputCapture::Evidence { id } => Some(RawInputEvent::EvidenceSend(id.clone())),
        }
    }

    fn input_event(&self, capture: &RawInputCapture) -> Option<RawInputEvent> {
        match capture {
            RawInputCapture::Question {
                id,
                allow_free_text,
                secret,
                ..
            } if *allow_free_text => {
                if is_removed_question_answer_slash_fragment(&self.free_text) {
                    return None;
                }
                if *secret {
                    Some(RawInputEvent::CardSecretInput(
                        id.clone(),
                        self.free_text.clone(),
                    ))
                } else {
                    Some(RawInputEvent::CardInput(id.clone(), self.free_text.clone()))
                }
            }
            _ => None,
        }
    }
}

fn cancel_event(capture: &RawInputCapture) -> RawInputEvent {
    match capture {
        RawInputCapture::Approval { id, .. } | RawInputCapture::Consultation { id } => {
            RawInputEvent::CardCancel(id.clone())
        }
        RawInputCapture::Mode { id, .. } => RawInputEvent::ModeCancel(id.clone()),
        RawInputCapture::Config { id, .. } => RawInputEvent::ConfigCancel(id.clone()),
        RawInputCapture::ConfigLanguage { id, .. } => {
            RawInputEvent::ConfigLanguageCancel(id.clone())
        }
        RawInputCapture::Session { id, .. } => RawInputEvent::SessionCancel(id.clone()),
        RawInputCapture::Question { id, .. } => RawInputEvent::QuestionCancel(id.clone()),
        RawInputCapture::Evidence { id } => RawInputEvent::EvidenceCancel(id.clone()),
    }
}

fn card_answer_event(answer: &str, secret: bool) -> RawInputEvent {
    if secret {
        RawInputEvent::CardSecretAnswer(answer.to_string())
    } else {
        RawInputEvent::CardAnswer(answer.to_string())
    }
}

fn empty_question_submission(id: &str, secret: bool) -> RawInputEvent {
    if secret {
        RawInputEvent::CardSecretAnswer(String::new())
    } else {
        RawInputEvent::QuestionSubmitAttempt(id.to_string())
    }
}

fn is_csi_final_byte(byte: u8) -> bool {
    (0x40..=0x7e).contains(&byte)
}

fn approval_action_max_index() -> usize {
    APPROVAL_PANEL_ACTIONS.len().saturating_sub(1)
}

fn selected_options_answer(selected_options: &[usize]) -> String {
    selected_options
        .iter()
        .map(|index| (index + 1).to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn is_removed_question_answer_slash(answer: &str) -> bool {
    answer.split_whitespace().next() == Some("/answer")
}

fn is_removed_question_answer_slash_fragment(answer: &str) -> bool {
    let answer = answer.trim_start();
    !answer.is_empty()
        && ("/answer".starts_with(answer) || answer.split_whitespace().next() == Some("/answer"))
}

fn approval_event_for_action(id: &str, action: ApprovalPanelAction) -> RawInputEvent {
    match action {
        ApprovalPanelAction::Approve => RawInputEvent::CardApprove(id.to_string()),
        ApprovalPanelAction::AlwaysTrust => RawInputEvent::CardAlwaysTrust(id.to_string()),
        ApprovalPanelAction::Deny => RawInputEvent::CardDeny(id.to_string()),
        ApprovalPanelAction::Details => RawInputEvent::CardDetails(id.to_string()),
    }
}

fn question_choice_count(capture: &RawInputCapture) -> usize {
    match capture {
        RawInputCapture::Question {
            option_count,
            allow_free_text,
            ..
        } => shared_question_choice_count(*option_count, *allow_free_text),
        RawInputCapture::Approval { .. }
        | RawInputCapture::Consultation { .. }
        | RawInputCapture::Evidence { .. }
        | RawInputCapture::Session { .. } => 0,
        RawInputCapture::Mode { .. }
        | RawInputCapture::Config { .. }
        | RawInputCapture::ConfigLanguage { .. } => 0,
    }
}

#[cfg(test)]
mod tests;
