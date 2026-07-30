use crate::input::InterceptReason;

mod capture_bridge;
mod card_capture;
mod draft_editor;
mod event_parser;

pub(crate) use draft_editor::PromptDraftEditor;
mod generation;
mod mode;
mod pty;
mod relay;
mod relay_action;
mod soft_newline;
mod spawn;

pub(crate) use event_parser::redact_extension_setting_value;
pub(crate) use generation::UserPtyInputGeneration;
pub(crate) use mode::{update_input_mode, update_locked_input_mode, RawInputMode};
pub use mode::{PromptGhostCandidate, PromptGhostRoute, RawInputCapture, RawObserverAction};
pub(crate) use pty::{
    set_pty_winsize, signal_foreground_process_group, signal_process_group, write_all_pty,
};
pub use relay_action::RawRelayAction;
pub(crate) use spawn::{spawn_raw_action_relay, spawn_raw_input_relay};

pub(super) const CTRL_C: u8 = 0x03;
pub(super) const CTRL_U: u8 = 0x15;
pub(super) const ESC: u8 = 0x1b;

/// Shared "bash is sitting at its primary prompt" gate (#1721 D16).
///
/// Set by the output side when the shell marker emits `prompt_ready` (PS1
/// only); cleared whenever user bytes carrying a line submit reach the PTY
/// or a command starts. CJK line-start candidates may only open while the
/// gate is up, so PS2 continuations, heredocs, and running commands keep
/// pre-#1721 byte passthrough (fail-closed: a lost signal disables capture,
/// never the other way around).
///
/// Ordering: `Relaxed` is sufficient because the gate is a standalone
/// boolean latch — readers only branch on the flag and never rely on it to
/// order access to other shared state; a stale read degrades to the
/// fail-closed passthrough behavior.
#[derive(Clone, Debug, Default)]
pub(crate) struct MainPromptGate(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl MainPromptGate {
    pub(crate) fn set_at_prompt(&self, at_prompt: bool) {
        self.0
            .store(at_prompt, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn is_at_prompt(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RawInputEvent {
    ShellInputActivity {
        empty: bool,
    },
    /// User bytes the relay wrote to the PTY: the write generation plus how
    /// many line submissions (accept-line CR/LF) the write carried. Anchors
    /// prompt replay state so real user input expires stale replays.
    PtyUserWrite {
        generation: u64,
        line_submits: usize,
    },
    CtrlC,
    Esc,
    CandidateRedraw {
        input: Vec<u8>,
        hint: Option<String>,
    },
    CandidateCommit(Vec<u8>),
    PromptGhostClear,
    PromptGhostAccepted {
        suggestion_id: Option<String>,
    },
    PromptGhostCycle {
        text: String,
    },
    PromptGhostDismissed,
    PromptGhostIntercept {
        input: String,
        suggestion_id: Option<String>,
    },
    CandidateClearLine,
    UserIntercept(String, InterceptReason),
    /// A whitelisted soft-newline shortcut was observed on a passthrough
    /// path (candidate buffer inactive). Observe-only: the bytes were still
    /// relayed to the shell unchanged; downstream may surface a one-time
    /// discoverability tip at the next prompt-ready (#1721).
    SoftNewlineShortcutObserved,
    /// A multi-line bracketed paste was relayed straight to bash (#1932):
    /// feeds the failure-insight multi-line entry hint, observe-only.
    MultilinePasteObserved,
    /// #1721 D13: the first soft newline in a candidate upgrades the draft
    /// into the multi-line prompt card; carries the buffered text.
    PromptDraftOpen {
        text: String,
    },
    /// Draft card state snapshot after an editing keystroke (D14).
    PromptDraftChanged {
        id: String,
        text: String,
        viewport: draft_editor::DraftViewport,
        line_count: usize,
    },
    /// Enter inside the draft card: submit the multi-line prompt.
    PromptDraftSubmit {
        id: String,
        text: String,
    },
    /// Esc/Ctrl+C inside the draft card: cancel composition (D15).
    PromptDraftCancel {
        id: String,
    },
    /// The soft-newline upgrade submitted a synthetic empty line so bash
    /// repaints PS1 (#1932); its visually blank accept echo is dropped
    /// at the next prompt boundary instead of surfacing as a blank line.
    SyntheticPromptRepaint,
    CaptureSubmitted {
        kind: &'static str,
        target_id: String,
        generation: u64,
    },
    CaptureDrained {
        generation: u64,
    },
    CaptureExpired {
        generation: u64,
    },
    CaptureOverflow {
        generation: u64,
    },
    CardFocus(String, usize),
    CardToggle(String, usize),
    CardInput(String, String),
    CardSecretInput(String, String),
    CardApprove(String),
    CardApproveTurn(String),
    CardAlwaysTrust(String),
    CardDeny(String),
    CardDetails(String),
    CardCancel(String),
    CardAnswer(String),
    QuestionSubmitAttempt(String),
    CardSecretAnswer(String),
    QuestionCancel(String),
    /// Ctrl+C on a question capture: abandon the whole prompt.
    ///
    /// Distinct from [`RawInputEvent::QuestionCancel`] (ESC) only so a multi-step prompt can let
    /// ESC step back while Ctrl+C keeps its usual meaning. Panels with a single step treat both
    /// identically.
    QuestionAbort(String),
    EvidenceSend(String),
    EvidenceIgnore(String),
    EvidenceCancel(String),
    ModeFocus(String, usize),
    ModeSet(String, usize),
    ModeCancel(String),
    ConfigFocus(String, usize),
    ConfigSave(String),
    ConfigCancel(String),
    ConfigLanguageFocus(String, usize),
    ConfigLanguageSet(String, usize),
    ConfigLanguageCancel(String),
    SessionFocus(String, usize),
    SessionToggle(String, usize),
    SessionResume(String, usize),
    SessionDelete(String),
    SessionClearConfirm(String),
    SessionCancel(String),
}

#[cfg(test)]
mod tests {
    use super::event_parser::{
        candidate_inline_hint, candidate_line_status, native_candidate_should_return_to_shell,
        redact_extension_setting_value, starts_native_intercept_candidate, CandidateLineBuffer,
        CandidateLineStatus, NativeLineState,
    };
    use super::relay::ExplicitExitTracker;
    use super::soft_newline::render_soft_newline_markers;
    use crate::input::InputClassifier;

    fn soft_buffer() -> CandidateLineBuffer {
        let mut line = CandidateLineBuffer::default();
        line.soft_newline_enabled = true;
        line
    }

    fn status_of(line: &CandidateLineBuffer) -> CandidateLineStatus {
        candidate_line_status(&line.bytes, line.soft_newline_enabled)
    }

    // Split paste delimiters (#1721): PTY reads may split the paste delimiter itself; a half
    // closer must never become draft content nor leave in_paste stuck.
    #[test]
    fn split_paste_delimiters_resolve_across_pushes() {
        let mut line = soft_buffer();
        line.push(b"\x1b[200~");
        line.push("\u{5206}\u{6790}".as_bytes());
        line.push(b"\x1b[20");
        assert!(line.is_active(), "held partial keeps the candidate open");
        line.push(b"1~");
        assert!(!line.in_paste(), "joined closer must end the paste");
        let text = String::from_utf8_lossy(line.visible_line_bytes()).into_owned();
        assert!(
            !text.contains("20") && !text.contains("1~"),
            "no delimiter fragments in the draft: {text}"
        );

        // Split opener while the buffer is already active.
        let mut line = soft_buffer();
        line.push("\u{95ee}".as_bytes());
        line.push(b"\x1b[2");
        line.push(b"00~x\x1b[201~");
        assert!(!line.in_paste());
        assert_eq!(line.visible_line_bytes(), "\u{95ee}x".as_bytes());
    }

    // Cross-chunk pasted CRLF (#1721): a pasted CRLF split across reads folds into one newline.
    #[test]
    fn split_pasted_crlf_folds_into_one_newline() {
        let mut line = soft_buffer();
        line.push(b"\x1b[200~");
        line.push("\u{7b2c}\u{4e00}\u{884c}\r".as_bytes());
        line.push("\n\u{7b2c}\u{4e8c}\u{884c}".as_bytes());
        line.push(b"\x1b[201~");
        line.push(b"\r");
        let CandidateLineStatus::Complete { line: text, .. } = status_of(&line) else {
            panic!("draft must submit");
        };
        assert_eq!(text, "\u{7b2c}\u{4e00}\u{884c}\n\u{7b2c}\u{4e8c}\u{884c}");
    }

    // Matrix #3-#6: every whitelisted shortcut becomes one soft newline and
    // never completes or flushes the line; no suffix bytes leak (I3).
    #[test]
    fn soft_newline_shortcuts_insert_without_submitting() {
        for sequence in [
            b"\x1b\r".as_slice(),
            b"\x1b\n".as_slice(),
            b"\x1b[13;2u".as_slice(),
            b"\x1b[13;3u".as_slice(),
            b"\x1b[27;2;13~".as_slice(),
            b"\x1b[27;3;13~".as_slice(),
        ] {
            let mut line = soft_buffer();
            line.push("请帮我分析系统负载".as_bytes());
            line.push(sequence);
            line.push("给出优化建议".as_bytes());
            assert_eq!(
                status_of(&line),
                CandidateLineStatus::Pending,
                "sequence {sequence:?} must stay pending",
            );
            let display =
                String::from_utf8_lossy(&render_soft_newline_markers(line.visible_line_bytes()))
                    .into_owned();
            assert!(
                !display.contains(";2u") && !display.contains("13;"),
                "no literal leak in display for {sequence:?}: {display}"
            );

            // Matrix #1: Enter submits the whole multi-line draft.
            line.push(b"\r");
            let CandidateLineStatus::Complete {
                line: text,
                line_len,
            } = status_of(&line)
            else {
                panic!("enter must complete for {sequence:?}");
            };
            assert_eq!(text, "请帮我分析系统负载\n给出优化建议");
            assert_eq!(line_len, line.bytes.len());
        }
    }

    // Matrix #2: bare Ctrl+J keeps its submit semantics (D6).
    #[test]
    fn bare_ctrl_j_still_submits() {
        let mut line = soft_buffer();
        line.push("请帮我分析".as_bytes());
        line.push(b"\n");
        let CandidateLineStatus::Complete { line: text, .. } = status_of(&line) else {
            panic!("bare LF must complete");
        };
        assert_eq!(text, "请帮我分析");
    }

    // Matrix #7: bracketed-paste newlines become soft newlines; CRLF folds.
    #[test]
    fn pasted_newlines_stay_soft() {
        let mut line = soft_buffer();
        line.push(b"\x1b[200~");
        line.push("分析负载\r\n给出建议\r不要 sudo\n结束".as_bytes());
        line.push(b"\x1b[201~");
        assert_eq!(status_of(&line), CandidateLineStatus::Pending);
        line.push(b"\r");
        let CandidateLineStatus::Complete { line: text, .. } = status_of(&line) else {
            panic!("paste draft must complete on enter");
        };
        assert_eq!(text, "分析负载\n给出建议\n不要 sudo\n结束");
    }

    // Matrix #8/#9: non-whitelisted escapes and control bytes stay unsafe
    // even in a multi-line draft (I2).
    #[test]
    fn foreign_controls_stay_unsafe_with_soft_newlines() {
        let mut line = soft_buffer();
        line.push("分析".as_bytes());
        line.push(b"\x1b\r");
        line.push(b"\x1b[A");
        assert_eq!(status_of(&line), CandidateLineStatus::Unsafe);

        let mut line = soft_buffer();
        line.push("分析".as_bytes());
        line.push(b"\x1b\r");
        line.push(&[0x16]);
        assert_eq!(status_of(&line), CandidateLineStatus::Unsafe);
    }

    // Matrix #10: the 4096-byte cap still applies to multi-line drafts.
    #[test]
    fn oversized_multiline_draft_is_unsafe() {
        let mut line = soft_buffer();
        line.push(&[b'a'; 4000]);
        line.push(b"\x1b\r");
        line.push(&[b'b'; 200]);
        assert_eq!(status_of(&line), CandidateLineStatus::Unsafe);
    }

    // Matrix #11: a CSI-u shortcut split across chunks normalizes once whole.
    #[test]
    fn split_shortcut_normalizes_after_completion() {
        let mut line = soft_buffer();
        line.push("分析".as_bytes());
        line.push(b"\x1b[13;2");
        assert_eq!(status_of(&line), CandidateLineStatus::Pending);
        line.push(b"u");
        line.push("结束".as_bytes());
        line.push(b"\r");
        let CandidateLineStatus::Complete { line: text, .. } = status_of(&line) else {
            panic!("split shortcut must complete on enter");
        };
        assert_eq!(text, "分析\n结束");
    }

    // Matrix #12: backspace removes a soft newline as one visible character.
    #[test]
    fn backspace_deletes_soft_newline_whole() {
        let mut line = soft_buffer();
        line.push("分析".as_bytes());
        line.push(b"\x1b[13;2u");
        line.push(&[0x7f]);
        line.push(b"\r");
        let CandidateLineStatus::Complete { line: text, .. } = status_of(&line) else {
            panic!("draft must complete after backspace");
        };
        assert_eq!(text, "分析");
    }

    // Matrix #13: Ctrl+U clears the whole multi-line draft.
    #[test]
    fn ctrl_u_clears_multiline_draft() {
        let mut line = soft_buffer();
        line.push("分析".as_bytes());
        line.push(b"\x1b\r");
        line.push("建议".as_bytes());
        line.push(&[super::CTRL_U]);
        assert!(!line.is_active());
    }

    // Matrix #14/I1+I2: the native path keeps pre-#1721 semantics — shortcut
    // bytes in a native candidate stay Unsafe and flush to the shell.
    #[test]
    fn native_buffer_keeps_shortcut_bytes_unsafe() {
        let mut line = CandidateLineBuffer::default();
        line.push(b"/mo");
        line.push(b"\x1b[13;2u");
        assert_eq!(status_of(&line), CandidateLineStatus::Unsafe);
        assert!(
            line.bytes
                .windows(b"\x1b[13;2u".len())
                .any(|window| window == b"\x1b[13;2u"),
            "native buffer must keep the raw bytes untouched"
        );
    }

    // Matrix #19 precondition: a draft of only soft newlines normalizes to
    // whitespace-only text (relay consumes it without submitting).
    #[test]
    fn soft_newline_only_draft_normalizes_to_whitespace() {
        let mut line = soft_buffer();
        line.push(b"\x1b\r");
        line.push(b"\x1b[13;2u");
        line.push(b"\r");
        let CandidateLineStatus::Complete { line: text, .. } = status_of(&line) else {
            panic!("whitespace draft must complete");
        };
        assert!(text.trim().is_empty());
        assert_eq!(text, "\n\n");
    }

    #[test]
    fn bare_slash_has_no_inline_hint() {
        assert_eq!(candidate_inline_hint("/"), None);
        assert_eq!(candidate_inline_hint("  /"), None);
        assert_eq!(
            candidate_inline_hint("/mo"),
            Some("/mode approval [recommend|auto|trust]".to_string())
        );
        assert_eq!(candidate_inline_hint("/approval"), None);
        assert_eq!(
            candidate_inline_hint("/sk"),
            Some("/skills [list|detail] [name]".to_string())
        );
    }

    #[test]
    fn extension_setting_values_are_redacted_from_candidate_echo() {
        let command = b"/extensions settings set fixture token secret-value --scope user";
        let redacted = redact_extension_setting_value(command);
        let shown = String::from_utf8(redacted).expect("redacted command remains UTF-8");
        assert_eq!(
            shown,
            "/extensions settings set fixture token ************ ******* ****"
        );
        assert!(!shown.contains("secret-value"));
    }

    #[test]
    fn extension_setting_value_is_redacted_from_first_typed_byte() {
        let prefix = b"/extensions settings set fixture token ";
        assert_eq!(redact_extension_setting_value(prefix), prefix);
        assert_eq!(
            redact_extension_setting_value(b"/extensions settings set fixture token s"),
            b"/extensions settings set fixture token *"
        );
    }

    #[test]
    fn other_slash_values_are_not_redacted() {
        let command = b"/extensions settings get fixture token";
        assert_eq!(redact_extension_setting_value(command), command);
    }

    #[test]
    fn native_slash_candidate_only_starts_at_line_start() {
        let mut state = NativeLineState::default();

        assert!(starts_native_intercept_candidate(b"/", &state));
        assert!(starts_native_intercept_candidate(b"?? hello", &state));

        state.observe_shell_bytes(b"vim .");
        assert!(!starts_native_intercept_candidate(b"/", &state));
        assert!(!starts_native_intercept_candidate(b"?? hello", &state));

        state.observe_shell_bytes(b"\n");
        assert!(starts_native_intercept_candidate(b"/mode", &state));
    }

    #[test]
    fn native_slash_candidate_returns_paths_and_tab_to_shell() {
        let classifier = InputClassifier::conservative();
        let mut line = CandidateLineBuffer::default();

        line.push(b"/m");
        assert!(!native_candidate_should_return_to_shell(&classifier, &line));

        line.push(b"ode agent");
        assert!(!native_candidate_should_return_to_shell(&classifier, &line));

        line.clear();
        line.push(b"/Users");
        assert!(native_candidate_should_return_to_shell(&classifier, &line));

        line.clear();
        line.push(b"/tmp/");
        assert!(native_candidate_should_return_to_shell(&classifier, &line));

        line.clear();
        line.push(b"/\t");
        assert!(native_candidate_should_return_to_shell(&classifier, &line));
    }

    #[test]
    fn candidate_line_ctrl_u_clears_pending_input() {
        let mut line = CandidateLineBuffer::default();

        line.push(b"Analyze memory pressure");
        line.push(&[super::CTRL_U]);

        assert!(!line.is_active());
        assert!(line.visible_line_bytes().is_empty());
    }

    #[test]
    fn explicit_exit_tracker_detects_split_exit_zero() {
        let mut tracker = ExplicitExitTracker::default();

        tracker.observe_shell_bytes(b"ex");
        assert!(!tracker.saw_explicit_exit());
        tracker.observe_shell_bytes(b"it 0\n");

        assert!(tracker.saw_explicit_exit());
    }

    #[test]
    fn explicit_exit_tracker_ignores_non_exit_lines() {
        let mut tracker = ExplicitExitTracker::default();

        tracker.observe_shell_bytes(b"echo exit\n");
        tracker.observe_shell_bytes(b"printf logout\n");

        assert!(!tracker.saw_explicit_exit());
    }
}
