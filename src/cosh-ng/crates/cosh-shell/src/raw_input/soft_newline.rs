//! Soft-newline translation for the raw-input candidate line buffer (#1721).
//! Mechanically split out of `event_parser.rs` to keep that file under the
//! layout gate threshold; the whitelist constant below stays the single
//! source of truth (design §5.3).

use super::event_parser::CandidateLineBuffer;

/// Whitelisted soft-newline key encodings (#1721): Alt+Enter as legacy
/// meta-sends-escape ESC+CR / ESC+LF and Shift+Enter as the two CSI-u
/// encodings. Single source of truth (design §5.3); every other escape
/// form keeps the pre-fix fail-closed handling.
pub(super) const SOFT_NEWLINE_SEQUENCES: [&[u8]; 4] =
    [b"\x1b\r", b"\x1b\n", b"\x1b[27;2u", b"\x1b[13;2u"];

impl CandidateLineBuffer {
    /// Rewrites a just-completed whitelisted soft-newline sequence at the
    /// buffer tail into a literal `\n`, recording its offset. Translation is
    /// fail-closed (I1): a buffer that is already doomed to the pre-fix
    /// `Unsafe` flush (escape or control bytes, hard newlines, invalid
    /// UTF-8, oversize) is left byte-for-byte untouched so the flush stays
    /// identical to the pre-fix behavior.
    pub(super) fn translate_soft_newline_suffix(&mut self) {
        if self.bytes.len() > 4096 {
            return;
        }
        let Some(sequence) = SOFT_NEWLINE_SEQUENCES
            .iter()
            .find(|sequence| self.bytes.ends_with(sequence))
        else {
            return;
        };
        let start = self.bytes.len() - sequence.len();
        if !self.is_clean_soft_newline_prefix(start) {
            return;
        }
        self.bytes.truncate(start);
        self.soft_newlines.push(self.bytes.len());
        self.bytes.push(b'\n');
    }

    /// Clean means the prefix could still become a Complete natural-language
    /// line: valid UTF-8 and no escape or control bytes besides `\t` and
    /// previously inserted soft newlines. Hard newlines block translation so
    /// the remainder past a submit terminator stays raw (re-pushed later).
    fn is_clean_soft_newline_prefix(&self, end: usize) -> bool {
        let prefix = &self.bytes[..end];
        if std::str::from_utf8(prefix).is_err() {
            return false;
        }
        prefix.iter().enumerate().all(|(index, byte)| {
            *byte == b'\t'
                || *byte >= 0x20
                || (*byte == b'\n' && self.soft_newlines.contains(&index))
        })
    }
}

// Matrix tests for design.md §5.1 (M1-M18) of the soft-newline fix (#1721).
// Positive rows assert the whitelist translation; counter-proof rows pin the
// pre-fix behavior byte-for-byte (invariant I1 fail-closed, I3 payload).

#[cfg(test)]
mod soft_newline_tests {
    use super::super::event_parser::{
        candidate_line_status, CandidateLineBuffer, CandidateLineStatus,
    };

    fn buffer_after_push(inputs: &[&[u8]]) -> CandidateLineBuffer {
        let mut buffer = CandidateLineBuffer::default();
        for input in inputs {
            buffer.push(input);
        }
        buffer
    }

    fn status_of(buffer: &CandidateLineBuffer) -> CandidateLineStatus {
        candidate_line_status(&buffer.bytes, &buffer.soft_newlines)
    }

    #[test]
    fn every_whitelisted_sequence_translates() {
        // Single source of truth (design §5.3): tests exercise the same
        // whitelist constant the implementation matches against.
        for sequence in super::SOFT_NEWLINE_SEQUENCES {
            let buffer = buffer_after_push(&[b"hi", sequence]);
            assert_eq!(
                status_of(&buffer),
                CandidateLineStatus::Pending,
                "{sequence:?}"
            );
            assert_eq!(buffer.bytes, b"hi\n", "{sequence:?}");
            assert_eq!(buffer.soft_newlines, [2], "{sequence:?}");
        }
    }

    #[test]
    fn bare_cr_still_submits() {
        // T-01 / M1: bare Enter keeps the submit semantics.
        let buffer = buffer_after_push(&[b"hello\r"]);
        assert_eq!(
            status_of(&buffer),
            CandidateLineStatus::Complete {
                line: "hello".to_string(),
                line_len: 6,
            }
        );
    }

    #[test]
    fn alt_enter_lf_variant_inserts_soft_newline() {
        // T-04 / M4: legacy meta-sends-escape Alt+Enter as ESC + LF.
        let buffer = buffer_after_push(&[b"hello\x1b\n"]);
        assert_eq!(status_of(&buffer), CandidateLineStatus::Pending);
        assert_eq!(buffer.bytes, b"hello\n");
    }

    #[test]
    fn excluded_csi_variants_stay_unsafe() {
        // T-07 / M7 (I1): variants outside the whitelist keep the pre-fix
        // Unsafe verdict and the raw bytes, so the flush stays byte-exact.
        for tail in [
            b"\x1b[13;5u".as_slice(),
            b"\x1b[13;3u",
            b"\x1b[27;2;13~",
            b"\x1b[13;2v",
        ] {
            let mut expected = b"hello".to_vec();
            expected.extend_from_slice(tail);
            let buffer = buffer_after_push(&[b"hello", tail]);
            assert_eq!(status_of(&buffer), CandidateLineStatus::Unsafe, "{tail:?}");
            assert_eq!(buffer.bytes, expected, "{tail:?}");
        }
    }

    #[test]
    fn ss3_sequence_stays_unsafe() {
        // T-08 / M8: complete SS3 stays Unsafe; the bare prefix stays Pending.
        let buffer = buffer_after_push(&[b"hello\x1bOM"]);
        assert_eq!(status_of(&buffer), CandidateLineStatus::Unsafe);
        let buffer = buffer_after_push(&[b"hello\x1bO"]);
        assert_eq!(status_of(&buffer), CandidateLineStatus::Pending);
    }

    #[test]
    fn incomplete_escape_prefixes_stay_pending() {
        // T-09 / M9: partial escape sequences keep waiting, bytes untouched.
        for tail in [b"\x1b".as_slice(), b"\x1b[", b"\x1b[27;2", b"\x1b[13;2"] {
            let mut expected = b"hello".to_vec();
            expected.extend_from_slice(tail);
            let buffer = buffer_after_push(&[b"hello", tail]);
            assert_eq!(status_of(&buffer), CandidateLineStatus::Pending, "{tail:?}");
            assert_eq!(buffer.bytes, expected, "{tail:?}");
        }
    }

    #[test]
    fn split_push_whitelist_sequence_becomes_soft_newline() {
        // T-09b / M9: whitelist sequences arriving across push calls are
        // still recognized once the final byte lands.
        for chunks in [
            [b"hello\x1b".as_slice(), b"[13;2u".as_slice()].as_slice(),
            &[b"hello\x1b", b"\r"],
            &[b"hello", b"\x1b[27;2", b"u"],
        ] {
            let buffer = buffer_after_push(chunks);
            assert_eq!(
                status_of(&buffer),
                CandidateLineStatus::Pending,
                "{chunks:?}"
            );
            assert_eq!(buffer.bytes, b"hello\n", "{chunks:?}");
        }
    }

    #[test]
    fn ctrl_v_stays_unsafe() {
        // T-10 / M10: Ctrl+V still hits the fail-closed control-byte rule.
        let buffer = buffer_after_push(&[b"hello\x16"]);
        assert_eq!(status_of(&buffer), CandidateLineStatus::Unsafe);
    }

    #[test]
    fn control_bytes_stay_unsafe() {
        // T-11 / M11 (I1): every control byte outside \n \r \t (and the
        // escape-prefix handling covered above) keeps the Unsafe verdict.
        for byte in 0x00u8..0x20 {
            if matches!(byte, b'\n' | b'\r' | b'\t' | 0x1b) {
                continue;
            }
            assert_eq!(
                candidate_line_status(&[b'h', byte], &[]),
                CandidateLineStatus::Unsafe,
                "{byte:#04x}"
            );
        }
    }

    #[test]
    fn bracketed_paste_newline_still_submits() {
        // T-12 / M12: pasted hard newlines keep the pre-fix submit semantics
        // (paste contract stays with the forerunner SDD; pinned here).
        let buffer = buffer_after_push(&[b"\x1b[200~a\nb\x1b[201~"]);
        assert_eq!(buffer.bytes, b"a\nb");
        assert_eq!(
            status_of(&buffer),
            CandidateLineStatus::Complete {
                line: "a".to_string(),
                line_len: 2,
            }
        );
    }

    #[test]
    fn stacked_soft_newlines_stay_pending() {
        // T-13 / M13: repeated soft newlines accumulate literal \n bytes.
        let buffer = buffer_after_push(&[b"a\x1b\rb\x1b\rc"]);
        assert_eq!(status_of(&buffer), CandidateLineStatus::Pending);
        assert_eq!(buffer.bytes, b"a\nb\nc");
    }

    #[test]
    fn soft_newline_then_enter_submits_multiline_payload() {
        // T-14 / M14 (I3): bare Enter after a soft newline submits the whole
        // buffer with the soft newline preserved as a literal \n.
        let buffer = buffer_after_push(&[b"a\x1b\rb\r"]);
        assert_eq!(
            status_of(&buffer),
            CandidateLineStatus::Complete {
                line: "a\nb".to_string(),
                line_len: 4,
            }
        );
    }

    #[test]
    fn trailing_soft_newline_survives_submission() {
        // I3: submitting right after a soft newline must not trim the
        // inserted \n away with the submit terminator.
        let buffer = buffer_after_push(&[b"a\x1b\r\r"]);
        assert_eq!(
            status_of(&buffer),
            CandidateLineStatus::Complete {
                line: "a\n".to_string(),
                line_len: 3,
            }
        );
    }

    #[test]
    fn utf8_text_around_soft_newline_is_preserved() {
        // T-15 / M15: multi-byte characters on both sides stay intact.
        let buffer = buffer_after_push(&["你好".as_bytes(), b"\x1b\r", "世界".as_bytes()]);
        assert_eq!(status_of(&buffer), CandidateLineStatus::Pending);
        assert_eq!(buffer.bytes, "你好\n世界".as_bytes());
    }

    #[test]
    fn backspace_removes_soft_newline_as_one_unit() {
        // T-15 / M15: backspace deletes the trailing multi-byte character,
        // then undoes the soft newline as a whole, never splitting either.
        let buffer = buffer_after_push(&[b"a\x1b\r", "界".as_bytes(), b"\x7f"]);
        assert_eq!(buffer.bytes, b"a\n");
        assert_eq!(status_of(&buffer), CandidateLineStatus::Pending);
        let buffer = buffer_after_push(&[b"a\x1b\r", "界".as_bytes(), b"\x7f\x7f"]);
        assert_eq!(buffer.bytes, b"a");
        let buffer = buffer_after_push(&[b"a\x1b\r", "界".as_bytes(), b"\x7f\x7f\x7f"]);
        assert_eq!(buffer.bytes, b"");
    }

    #[test]
    fn visible_line_shows_current_line_after_soft_newline() {
        // R3 minimal redraw contract: only the current (last) line is shown.
        let buffer = buffer_after_push(&[b"a\x1b\rbc"]);
        assert_eq!(buffer.visible_line_bytes(), b"bc");
        let buffer = buffer_after_push(&[b"abc"]);
        assert_eq!(buffer.visible_line_bytes(), b"abc");
    }

    #[test]
    fn oversize_buffer_stays_unsafe_with_raw_sequence() {
        // T-17 / M17: oversize buffers stay Unsafe and keep the raw
        // sequence so the flush stays byte-exact.
        let head = vec![b'a'; 4096];
        let buffer = buffer_after_push(&[&head, b"\x1b\r"]);
        assert_eq!(status_of(&buffer), CandidateLineStatus::Unsafe);
        assert!(buffer.bytes.ends_with(b"\x1b\r"));
    }

    #[test]
    fn whitelist_after_pending_escape_stays_unsafe() {
        // I1 fail-closed: a buffer already carrying escape bytes must not
        // translate; the doomed-Unsafe flush stays byte-identical.
        let buffer = buffer_after_push(&[b"a\x1b", b"\x1b\r"]);
        assert_eq!(status_of(&buffer), CandidateLineStatus::Unsafe);
        assert_eq!(buffer.bytes, b"a\x1b\x1b\r");
    }

    #[test]
    fn whitelist_after_hard_newline_keeps_raw_tail() {
        // I1/I3: after a hard newline the line is already submittable; the
        // trailing sequence stays raw inside the remainder (re-pushed later).
        let buffer = buffer_after_push(&[b"\x1b[200~a\nb\x1b[201~", b"\x1b\r"]);
        assert_eq!(buffer.bytes, b"a\nb\x1b\r");
        assert_eq!(
            status_of(&buffer),
            CandidateLineStatus::Complete {
                line: "a".to_string(),
                line_len: 2,
            }
        );
    }

    #[test]
    fn whitelist_after_invalid_utf8_stays_unsafe() {
        // I1 fail-closed: invalid UTF-8 buffers do not translate, keeping
        // the pre-fix Unsafe flush byte-identical.
        let buffer = buffer_after_push(&[&[0xff], b"\x1b\r"]);
        assert_eq!(status_of(&buffer), CandidateLineStatus::Unsafe);
        assert_eq!(buffer.bytes, [0xff, 0x1b, 0x0d]);
    }
}
