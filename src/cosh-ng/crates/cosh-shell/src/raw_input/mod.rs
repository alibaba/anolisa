use crate::input::InterceptReason;

mod capture_bridge;
mod card_capture;
mod event_parser;
mod generation;
mod mode;
mod pty;
mod relay;
mod relay_action;
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
        candidate_inline_hint, native_candidate_should_return_to_shell,
        redact_extension_setting_value, starts_native_intercept_candidate, CandidateLineBuffer,
        NativeLineState,
    };
    use super::relay::ExplicitExitTracker;
    use crate::input::InputClassifier;

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
