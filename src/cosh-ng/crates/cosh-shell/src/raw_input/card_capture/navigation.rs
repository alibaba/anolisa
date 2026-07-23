//! Keyboard navigation shared by interactive card captures.

use crate::ui::hook_approval_action_max_index;

use super::{
    approval_action_max_index, is_csi_final_byte, question_choice_count, CardInputState,
    RawInputCapture, RawInputEvent,
};

impl CardInputState {
    pub(super) fn consume_csi_sequence(
        &mut self,
        capture: &RawInputCapture,
        input: &[u8],
        idx: usize,
        events: &mut Vec<RawInputEvent>,
    ) -> Option<usize> {
        let final_idx = (idx + 2..input.len()).find(|pos| is_csi_final_byte(input[*pos]))?;
        let params = &input[idx + 2..final_idx];
        let final_byte = input[final_idx];

        match (params, final_byte) {
            (b"", b'A' | b'B' | b'C' | b'D') => {
                if let Some(event) = self.apply_arrow(capture, final_byte) {
                    events.push(event);
                }
            }
            (b"", b'Z') => {
                if let Some(event) = self.apply_shift_tab(capture) {
                    events.push(event);
                }
            }
            (_, b'~') => {
                // Bracketed paste and keypad sequences such as Delete end with
                // '~'. The sequence itself is terminal control data; any pasted
                // payload arrives as normal bytes between 200~ and 201~.
            }
            _ => {}
        }

        Some(final_idx + 1)
    }

    pub(super) fn apply_arrow(
        &mut self,
        capture: &RawInputCapture,
        code: u8,
    ) -> Option<RawInputEvent> {
        match capture {
            RawInputCapture::Question { id, .. } => {
                let choice_count = question_choice_count(capture);
                if choice_count == 0 {
                    return None;
                }
                let previous = self.selected;
                match code {
                    b'A' | b'D' => {
                        self.selected = self.selected.saturating_sub(1);
                    }
                    b'B' | b'C' => {
                        self.selected = (self.selected + 1).min(choice_count.saturating_sub(1));
                    }
                    _ => {}
                }
                if self.selected != previous {
                    Some(RawInputEvent::CardFocus(id.clone(), self.selected))
                } else {
                    None
                }
            }
            RawInputCapture::Approval { id, .. } | RawInputCapture::Consultation { id } => {
                let is_hook = matches!(capture, RawInputCapture::Approval { is_hook: true, .. });
                let max_idx = if is_hook {
                    hook_approval_action_max_index()
                } else {
                    approval_action_max_index()
                };
                let previous = self.selected;
                match code {
                    b'D' => self.selected = self.selected.saturating_sub(1),
                    b'C' => self.selected = (self.selected + 1).min(max_idx),
                    _ => {}
                }
                if self.selected != previous {
                    Some(RawInputEvent::CardFocus(id.clone(), self.selected))
                } else {
                    None
                }
            }
            RawInputCapture::Mode {
                id, option_count, ..
            }
            | RawInputCapture::Config {
                id, option_count, ..
            }
            | RawInputCapture::ConfigLanguage {
                id, option_count, ..
            }
            | RawInputCapture::Session {
                id, option_count, ..
            } => {
                if *option_count == 0 {
                    return None;
                }
                let previous = self.selected;
                match code {
                    b'A' | b'D' => self.selected = self.selected.saturating_sub(1),
                    b'B' | b'C' => {
                        self.selected = (self.selected + 1).min(option_count.saturating_sub(1));
                    }
                    _ => {}
                }
                if self.selected != previous {
                    match capture {
                        RawInputCapture::Mode { .. } => {
                            Some(RawInputEvent::ModeFocus(id.clone(), self.selected))
                        }
                        RawInputCapture::Config { .. } => {
                            Some(RawInputEvent::ConfigFocus(id.clone(), self.selected))
                        }
                        RawInputCapture::ConfigLanguage { .. } => Some(
                            RawInputEvent::ConfigLanguageFocus(id.clone(), self.selected),
                        ),
                        RawInputCapture::Session { .. } => {
                            Some(RawInputEvent::SessionFocus(id.clone(), self.selected))
                        }
                        _ => None,
                    }
                } else {
                    None
                }
            }
            RawInputCapture::Evidence { .. } => None,
        }
    }

    pub(super) fn apply_tab(&mut self, capture: &RawInputCapture) -> Option<RawInputEvent> {
        match capture {
            RawInputCapture::Question { id, .. } => {
                let choice_count = question_choice_count(capture);
                let previous = self.selected;
                if choice_count > 0 {
                    self.selected = (self.selected + 1).min(choice_count.saturating_sub(1));
                }
                if self.selected != previous {
                    Some(RawInputEvent::CardFocus(id.clone(), self.selected))
                } else {
                    None
                }
            }
            RawInputCapture::Approval { id, .. } | RawInputCapture::Consultation { id } => {
                let max_idx = if matches!(capture, RawInputCapture::Approval { is_hook: true, .. })
                {
                    hook_approval_action_max_index()
                } else {
                    approval_action_max_index()
                };
                self.selected = (self.selected + 1).min(max_idx);
                Some(RawInputEvent::CardFocus(id.clone(), self.selected))
            }
            RawInputCapture::Mode {
                id, option_count, ..
            }
            | RawInputCapture::Config {
                id, option_count, ..
            }
            | RawInputCapture::ConfigLanguage {
                id, option_count, ..
            }
            | RawInputCapture::Session {
                id, option_count, ..
            } => {
                if *option_count == 0 {
                    return None;
                }
                self.selected = (self.selected + 1).min(option_count.saturating_sub(1));
                match capture {
                    RawInputCapture::Mode { .. } => {
                        Some(RawInputEvent::ModeFocus(id.clone(), self.selected))
                    }
                    RawInputCapture::Config { .. } => {
                        Some(RawInputEvent::ConfigFocus(id.clone(), self.selected))
                    }
                    RawInputCapture::ConfigLanguage { .. } => Some(
                        RawInputEvent::ConfigLanguageFocus(id.clone(), self.selected),
                    ),
                    RawInputCapture::Session { .. } => {
                        Some(RawInputEvent::SessionFocus(id.clone(), self.selected))
                    }
                    _ => None,
                }
            }
            RawInputCapture::Evidence { .. } => None,
        }
    }

    fn apply_shift_tab(&mut self, capture: &RawInputCapture) -> Option<RawInputEvent> {
        match capture {
            RawInputCapture::Question { id, .. } => {
                let previous = self.selected;
                self.selected = self.selected.saturating_sub(1);
                if self.selected != previous {
                    Some(RawInputEvent::CardFocus(id.clone(), self.selected))
                } else {
                    None
                }
            }
            RawInputCapture::Approval { id, .. } | RawInputCapture::Consultation { id } => {
                self.selected = self.selected.saturating_sub(1);
                Some(RawInputEvent::CardFocus(id.clone(), self.selected))
            }
            RawInputCapture::Mode { id, .. } => {
                self.selected = self.selected.saturating_sub(1);
                Some(RawInputEvent::ModeFocus(id.clone(), self.selected))
            }
            RawInputCapture::Config { id, .. } => {
                self.selected = self.selected.saturating_sub(1);
                Some(RawInputEvent::ConfigFocus(id.clone(), self.selected))
            }
            RawInputCapture::ConfigLanguage { id, .. } => {
                self.selected = self.selected.saturating_sub(1);
                Some(RawInputEvent::ConfigLanguageFocus(
                    id.clone(),
                    self.selected,
                ))
            }
            RawInputCapture::Session { id, .. } => {
                self.selected = self.selected.saturating_sub(1);
                Some(RawInputEvent::SessionFocus(id.clone(), self.selected))
            }
            RawInputCapture::Evidence { .. } => None,
        }
    }
}
