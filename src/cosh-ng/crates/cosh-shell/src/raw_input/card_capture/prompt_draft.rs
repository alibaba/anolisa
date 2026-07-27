//! PromptDraft key handling for the card capture (#1721 D13/D14).
//!
//! Split out of `card_capture.rs` (layout discipline): every method here is
//! the body of a `RawInputCapture::PromptDraft` match arm in `consume_split`
//! / `submit` / `input_event`, operating on the shared [`CardInputState`].

use super::{CardInputKind, CardInputState, RawInputCapture, RawInputEvent};

impl CardInputState {
    /// A trailing bare ESC may be the first half of a split legacy
    /// Alt+Enter: hold it briefly; the relay injects a second ESC on
    /// timeout which resolves to an explicit cancel (#1721).
    pub(super) fn draft_hold_bare_escape(&mut self, capture: &RawInputCapture) -> bool {
        if !matches!(capture, RawInputCapture::PromptDraft { .. }) {
            return false;
        }
        self.pending_input.push(0x1b);
        true
    }

    /// Tabs inside a bracketed paste are data (code, TSV), not completion
    /// keys: insert them into the draft (#1721). Returns whether the
    /// byte was consumed here.
    pub(super) fn draft_pasted_tab(
        &mut self,
        capture: &RawInputCapture,
        events: &mut Vec<RawInputEvent>,
    ) -> bool {
        if !matches!(capture, RawInputCapture::PromptDraft { .. }) || !self.draft_paste {
            return false;
        }
        self.draft.insert_text("\t");
        if let Some(event) = self.input_event(capture) {
            events.push(event);
        }
        true
    }

    /// A bare ESC is being held for the draft card, waiting for a possible
    /// split CR/LF continuation (#1721). The relay cancels on timeout.
    pub(in super::super) fn draft_escape_pending(&self) -> bool {
        self.pending_input == [0x1b]
            && matches!(self.active_kind, Some(CardInputKind::PromptDraft { .. }))
    }

    /// Pasted CR/LF extends the draft; only a real Enter keystroke submits
    /// (matrix #4/#6). Returns the next input index.
    pub(super) fn draft_pasted_newline(
        &mut self,
        capture: &RawInputCapture,
        input: &[u8],
        mut idx: usize,
        events: &mut Vec<RawInputEvent>,
    ) -> usize {
        self.draft.insert_newline();
        if input[idx] == b'\r' && input.get(idx + 1) == Some(&b'\n') {
            idx += 1;
        }
        if let Some(event) = self.input_event(capture) {
            events.push(event);
        }
        idx + 1
    }

    /// Ctrl+A / Ctrl+E / Ctrl+U inside the draft card (D14); Ctrl+U keeps
    /// bash muscle memory: kill to line start.
    pub(super) fn draft_control_key(
        &mut self,
        capture: &RawInputCapture,
        byte: u8,
        events: &mut Vec<RawInputEvent>,
    ) {
        match byte {
            0x01 => self.draft.move_line_start(),
            0x05 => self.draft.move_line_end(),
            _ => self.draft.kill_to_line_start(),
        }
        if let Some(event) = self.input_event(capture) {
            events.push(event);
        }
    }

    /// Legacy Alt+Enter arrives as ESC + CR/LF: treat it as a soft newline
    /// inside the draft (whitelist parity); any other ESC combo cancels.
    /// Returns `(next index, cancelled)`.
    pub(super) fn draft_escape_sequence(
        &mut self,
        capture: &RawInputCapture,
        id: &str,
        input: &[u8],
        idx: usize,
        events: &mut Vec<RawInputEvent>,
    ) -> (usize, bool) {
        if matches!(input.get(idx + 1), Some(b'\r') | Some(b'\n')) {
            self.draft.insert_newline();
            if let Some(event) = self.input_event(capture) {
                events.push(event);
            }
            (idx + 2, false)
        } else {
            events.push(RawInputEvent::PromptDraftCancel { id: id.to_string() });
            (idx + 1, true)
        }
    }

    /// Printable/UTF-8 run inserted at the cursor. Incomplete UTF-8 tails
    /// stay pending so a split CJK character survives chunked reads.
    /// Returns the next input index.
    pub(super) fn draft_insert_visible(
        &mut self,
        capture: &RawInputCapture,
        input: &[u8],
        start: usize,
        events: &mut Vec<RawInputEvent>,
    ) -> usize {
        let mut idx = start;
        while idx < input.len() && !input[idx].is_ascii_control() && input[idx] != 0x1b {
            idx += 1;
        }
        let bytes = &input[start..idx];
        match std::str::from_utf8(bytes) {
            Ok(text) => self.draft.insert_text(text),
            Err(error) if error.error_len().is_none() => {
                let valid_len = error.valid_up_to();
                if valid_len > 0 {
                    self.draft.insert_text(
                        std::str::from_utf8(&bytes[..valid_len]).expect("validated UTF-8 prefix"),
                    );
                }
                self.pending_input.extend_from_slice(&bytes[valid_len..]);
            }
            Err(_) => {
                self.draft.insert_text(&String::from_utf8_lossy(bytes));
            }
        }
        if let Some(event) = self.input_event(capture) {
            events.push(event);
        }
        idx
    }

    /// Enter submits the whole draft; blank drafts never submit (matrix #9)
    /// so the user can keep composing or cancel with Esc.
    pub(super) fn draft_submit_event(&self, id: &str) -> Option<RawInputEvent> {
        if self.draft.is_blank() {
            return None;
        }
        Some(RawInputEvent::PromptDraftSubmit {
            id: id.to_string(),
            text: self.draft.text(),
        })
    }

    /// Full snapshot after an editing keystroke (D14).
    pub(super) fn draft_changed_event(&self, id: &str) -> RawInputEvent {
        RawInputEvent::PromptDraftChanged {
            id: id.to_string(),
            text: self.draft.text(),
            viewport: self.draft.viewport(),
            line_count: self.draft.line_count(),
        }
    }
}
