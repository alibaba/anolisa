use crate::input::InputClassifier;

use super::super::event_parser::NativeLineState;
use super::super::generation::LineSubmitCounter;

const BASH_SLASH_SUBMISSION_GUARD: &[u8] = b"\x1b[99~";
const BASH_PRIVATE_SLASH_SUBMISSION_GUARD: &[u8] = b"\x1b[100~";

pub(super) fn guarded_bash_submission(
    required: bool,
    history_private: bool,
    input_classifier: &InputClassifier,
    line_submits: &LineSubmitCounter,
    bytes: &[u8],
) -> Option<Vec<u8>> {
    let submissions = line_submits.submission_positions(bytes);
    let mut guards = Vec::new();
    let mut line_start = 0;
    let mut readline_owned = false;
    let mut batch_readline_safe = required;
    for (index, submit) in submissions.into_iter().enumerate() {
        let line = &bytes[line_start..submit];
        let exact_slash = exact_slash_submission(input_classifier, line);
        let blank = line.iter().all(u8::is_ascii_whitespace);
        let ctrl_o = bytes[submit] == 0x0f;
        let needs_guard = if !batch_readline_safe {
            false
        } else if index == 0 {
            required
        } else if readline_owned {
            // Ctrl-O loads another history entry before the next submitted
            // bytes, so the relay cannot prove the resulting Readline line.
            true
        } else {
            exact_slash
        };
        if needs_guard {
            let guard = if index == 0 && history_private {
                BASH_PRIVATE_SLASH_SUBMISSION_GUARD
            } else {
                BASH_SLASH_SUBMISSION_GUARD
            };
            guards.push((submit, guard));
        }
        if index == 0 {
            batch_readline_safe &= exact_slash || ctrl_o;
        } else if (readline_owned && !ctrl_o) || (!exact_slash && !blank && !ctrl_o) {
            // An ordinary command ends the part of the batch whose future
            // consumer is provably Readline. Later bytes stay byte-exact for
            // whichever shell/foreground reader owns the PTY next.
            batch_readline_safe = false;
        }
        readline_owned = batch_readline_safe && ctrl_o;
        line_start = submit + 1;
    }
    if guards.is_empty() {
        return None;
    }

    // Keep each guard adjacent to its submission boundary while preserving
    // #2918's single-write batch ownership. Do not assume the PTY delivers
    // this buffer atomically: write_all_pty may short-write. In vi-insert, ESC
    // is itself a complete binding, so a delivery gap longer than the user's
    // keyseq-timeout can reinterpret the remaining bytes as vi commands and
    // silently mutate the input line.
    let extra = guards.iter().map(|(_, guard)| guard.len()).sum::<usize>();
    let mut guarded = Vec::with_capacity(bytes.len() + extra);
    let mut cursor = 0;
    for (submit, guard) in guards {
        guarded.extend_from_slice(&bytes[cursor..submit]);
        guarded.extend_from_slice(guard);
        cursor = submit;
    }
    guarded.extend_from_slice(&bytes[cursor..]);
    Some(guarded)
}

fn exact_slash_submission(input_classifier: &InputClassifier, bytes: &[u8]) -> bool {
    let Ok(line) = std::str::from_utf8(bytes) else {
        return false;
    };
    let first_word = line.split_whitespace().next().unwrap_or_default();
    input_classifier.is_exact_slash_control_command(first_word)
}

pub(super) fn bash_submission_needs_guard(
    enabled: bool,
    at_prompt: bool,
    input_classifier: &InputClassifier,
    state: &NativeLineState,
    line_submits: &LineSubmitCounter,
    bytes: &[u8],
) -> bool {
    if !enabled || !at_prompt {
        return false;
    }
    let Some(submit) = line_submits.first_submission(bytes) else {
        return false;
    };

    // Readline owns history recall, completion, and cursor edits, so only its
    // submission widget can inspect a buffer once the relay mirror is dirty.
    // Keep clean ordinary lines and empty Enters off the widget path to avoid
    // a visible Readline redisplay before accept-line. Dirty ordinary lines
    // may still redisplay once because invoking bind-x is observable; that is
    // the cost of distinguishing them from recalled slash controls without a
    // global pre-execution hook.
    let mut submitted = state.clone();
    submitted.observe_shell_bytes(&bytes[..submit]);
    let Some(line) = submitted.clean_visible_line() else {
        return true;
    };
    let Ok(line) = std::str::from_utf8(line) else {
        return true;
    };
    let first_word = line.split_whitespace().next().unwrap_or_default();
    input_classifier.is_exact_slash_control_command(first_word)
}
