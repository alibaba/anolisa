//! Bounded suppression for Bash Readline's internal slash-guard redisplay.
//!
//! An authenticated marker arms one submission window. Only a complete guard
//! redraw and its optional verbose echo are suppressed; other bytes fail open.

use std::{borrow::Cow, io};

use super::OscParser;

const GUARD_COMMAND: &[u8] = b"case $- in *x*) builtin set +x; builtin true __cosh_slash_guard__; builtin set -x ;; *) : ;; esac";
const GUARD_REDRAW_TAIL: &[u8] = b"__cosh_slash_guard__; builtin set -x ;; *) : ;; esac";
const MAX_PENDING_BYTES: usize = 64 * 1024;
const MAX_PENDING_LINES: usize = 16;

enum GuardEcho {
    ReadlineRedraw { starts_at: usize },
    Verbose { starts_at: usize },
}

#[derive(Debug)]
pub(super) struct PendingSlashGuardEcho {
    line: Vec<u8>,
    lines: usize,
    bytes_seen: usize,
    redraw_suppressed: bool,
}

impl PendingSlashGuardEcho {
    pub(super) fn new() -> Self {
        Self {
            line: Vec::new(),
            lines: 0,
            bytes_seen: 0,
            redraw_suppressed: false,
        }
    }

    pub(super) fn filter<'a>(slot: &mut Option<Self>, data: &'a [u8]) -> Cow<'a, [u8]> {
        if slot.is_none() {
            return Cow::Borrowed(data);
        }
        let mut output = Vec::with_capacity(data.len());
        for byte in data.iter().copied() {
            let Some(pending) = slot.as_mut() else {
                output.push(byte);
                continue;
            };
            pending.line.push(byte);
            pending.bytes_seen += 1;
            if pending.bytes_seen >= MAX_PENDING_BYTES {
                output.extend_from_slice(&pending.line);
                *slot = None;
                continue;
            }
            if byte == b'\n' {
                match pending.classify_guard_echo() {
                    Some(GuardEcho::ReadlineRedraw { starts_at }) => {
                        output.extend_from_slice(&pending.line[..starts_at]);
                        output.extend_from_slice(pending.line_ending());
                        pending.line.clear();
                        pending.redraw_suppressed = true;
                        continue;
                    }
                    Some(GuardEcho::Verbose { starts_at }) => {
                        output.extend_from_slice(&pending.line[..starts_at]);
                        output.extend_from_slice(pending.line_ending());
                        *slot = None;
                        continue;
                    }
                    None => {}
                }
                // A complete non-matching line may be asynchronous user or
                // job output. Release it immediately while the bounded arm
                // waits for the guard redraw or its verbose duplicate.
                output.extend_from_slice(&pending.line);
                pending.line.clear();
                pending.lines += 1;
                if pending.lines >= MAX_PENDING_LINES {
                    *slot = None;
                }
            }
        }
        Cow::Owned(output)
    }

    pub(super) fn flush(slot: &mut Option<Self>) -> Vec<u8> {
        slot.take().map_or_else(Vec::new, |pending| pending.line)
    }

    fn classify_guard_echo(&self) -> Option<GuardEcho> {
        let body = self.line_body()?;
        if self.redraw_suppressed {
            return body
                .strip_suffix(GUARD_COMMAND)
                .map(|prefix| GuardEcho::Verbose {
                    starts_at: prefix.len(),
                });
        }

        // A carriage return is ordinary terminal output, not an ownership
        // boundary. Delete only a complete static replacement or a verified
        // '<' horizontal-scroll suffix; every preceding byte remains user/job
        // output.
        let mut matches = body
            .windows(GUARD_REDRAW_TAIL.len())
            .enumerate()
            .filter(|(_, window)| *window == GUARD_REDRAW_TAIL);
        let tail_start = matches.next()?.0;
        if matches.next().is_some()
            || !body[tail_start + GUARD_REDRAW_TAIL.len()..]
                .iter()
                .all(|byte| matches!(byte, b' ' | b'\x08'))
        {
            return None;
        }
        let guard_end = tail_start + GUARD_REDRAW_TAIL.len();
        let starts_at = body[..guard_end]
            .windows(GUARD_COMMAND.len())
            .position(|window| window == GUARD_COMMAND)
            .or_else(|| {
                body[..tail_start]
                    .iter()
                    .rposition(|byte| *byte == b'<')
                    .filter(|start| GUARD_COMMAND.ends_with(&body[*start + 1..guard_end]))
            })?;
        Some(GuardEcho::ReadlineRedraw { starts_at })
    }

    fn line_body(&self) -> Option<&[u8]> {
        let without_lf = self.line.strip_suffix(b"\n")?;
        Some(without_lf.strip_suffix(b"\r").unwrap_or(without_lf))
    }

    fn line_ending(&self) -> &'static [u8] {
        if self.line.ends_with(b"\r\n") {
            b"\r\n"
        } else {
            b"\n"
        }
    }
}

impl OscParser {
    pub(super) fn flush_pending_slash_guard_echo(&mut self) -> io::Result<()> {
        let pending = PendingSlashGuardEcho::flush(&mut self.pending_slash_guard_echo);
        if pending.is_empty() {
            return Ok(());
        }
        self.append_passthrough(&pending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suppresses_fragmented_guard_redisplay() {
        let mut pending = Some(PendingSlashGuardEcho::new());
        assert!(PendingSlashGuardEcho::filter(&mut pending, b"<builtin ").is_empty());
        assert_eq!(
            PendingSlashGuardEcho::filter(
                &mut pending,
                b"true __cosh_slash_guard__; builtin set -x ;; *) : ;; esac\r\n"
            )
            .as_ref(),
            b"\r\n"
        );
        assert!(pending.is_some());
    }

    #[test]
    fn suppresses_multiline_prompt_redisplay() {
        let mut pending = Some(PendingSlashGuardEcho::new());
        assert_eq!(
            PendingSlashGuardEcho::filter(&mut pending, b"first prompt line\r\n").as_ref(),
            b"first prompt line\r\n"
        );
        assert_eq!(
            PendingSlashGuardEcho::filter(
                &mut pending,
                b"<builtin true __cosh_slash_guard__; builtin set -x ;; *) : ;; esac\r\n"
            )
            .as_ref(),
            b"\r\n"
        );
    }

    #[test]
    fn suppresses_readline_cleanup_after_shorter_guard() {
        let mut pending = Some(PendingSlashGuardEcho::new());
        assert_eq!(
            PendingSlashGuardEcho::filter(
                &mut pending,
                b"<builtin true __cosh_slash_guard__; builtin set -x ;; *) : ;; esac      \x08\x08\x08\r\n"
            )
            .as_ref(),
            b"\r\n"
        );
        assert!(pending.is_some());
    }

    #[test]
    fn preserves_partial_output_before_guard_redisplay() {
        let mut pending = Some(PendingSlashGuardEcho::new());
        assert_eq!(
            PendingSlashGuardEcho::filter(
                &mut pending,
                b"BACKGROUND_PARTIAL<builtin true __cosh_slash_guard__; builtin set -x ;; *) : ;; esac\r\n"
            )
            .as_ref(),
            b"BACKGROUND_PARTIAL\r\n"
        );
    }

    #[test]
    fn preserves_carriage_return_partial_output_before_guard_redisplay() {
        let mut pending = Some(PendingSlashGuardEcho::new());
        assert_eq!(
            PendingSlashGuardEcho::filter(
                &mut pending,
                b"BEFORE\rBACKGROUND_PARTIAL<builtin true __cosh_slash_guard__; builtin set -x ;; *) : ;; esac\r\n"
            )
            .as_ref(),
            b"BEFORE\rBACKGROUND_PARTIAL\r\n"
        );
    }

    #[test]
    fn suppresses_verbose_echo_after_guard_redisplay() {
        let mut pending = Some(PendingSlashGuardEcho::new());
        assert_eq!(
            PendingSlashGuardEcho::filter(
                &mut pending,
                b"<builtin true __cosh_slash_guard__; builtin set -x ;; *) : ;; esac\r\ncase $- in *x*) builtin set +x; builtin true __cosh_slash_guard__; builtin set -x ;; *) : ;; esac\r\n"
            )
            .as_ref(),
            b"\r\n\r\n"
        );
        assert!(pending.is_none());
    }

    #[test]
    fn verbose_echo_preserves_unrelated_partial_prefix() {
        let mut pending = Some(PendingSlashGuardEcho::new());
        assert_eq!(
            PendingSlashGuardEcho::filter(
                &mut pending,
                b"<builtin true __cosh_slash_guard__; builtin set -x ;; *) : ;; esac\r\n"
            )
            .as_ref(),
            b"\r\n"
        );
        assert_eq!(
            PendingSlashGuardEcho::filter(
                &mut pending,
                b"BACKGROUND_PARTIALcase $- in *x*) builtin set +x; builtin true __cosh_slash_guard__; builtin set -x ;; *) : ;; esac\r\n"
            )
            .as_ref(),
            b"BACKGROUND_PARTIAL\r\n"
        );
    }

    #[test]
    fn unproven_sentinel_line_fails_open() {
        let mut pending = Some(PendingSlashGuardEcho::new());
        let line = b"BACKGROUND_PARTIAL builtin true __cosh_slash_guard__ unrelated\r\n";
        assert_eq!(
            PendingSlashGuardEcho::filter(&mut pending, line).as_ref(),
            line
        );
        assert!(pending.is_some());
    }

    #[test]
    fn complete_guard_match_preserves_preceding_angle_bracket() {
        let mut pending = Some(PendingSlashGuardEcho::new());
        assert_eq!(
            PendingSlashGuardEcho::filter(
                &mut pending,
                b"BACKGROUND_PARTIAL<case $- in *x*) builtin set +x; builtin true __cosh_slash_guard__; builtin set -x ;; *) : ;; esac\r\n"
            )
            .as_ref(),
            b"BACKGROUND_PARTIAL<\r\n"
        );
    }

    #[test]
    fn unarmed_filter_borrows_the_input() {
        let mut pending = None;
        assert!(matches!(
            PendingSlashGuardEcho::filter(&mut pending, b"ordinary output"),
            Cow::Borrowed(b"ordinary output")
        ));
    }

    #[test]
    fn mismatch_and_cap_fail_open() {
        let mut mismatch = Some(PendingSlashGuardEcho::new());
        let bytes = vec![b'x'; MAX_PENDING_BYTES];
        assert_eq!(PendingSlashGuardEcho::filter(&mut mismatch, &bytes), bytes);
        assert!(mismatch.is_none());

        let mut marker = Some(PendingSlashGuardEcho::new());
        assert!(PendingSlashGuardEcho::filter(&mut marker, b"partial").is_empty());
        assert_eq!(PendingSlashGuardEcho::flush(&mut marker), b"partial");
        assert!(marker.is_none());
    }

    #[test]
    fn line_cap_preserves_every_non_matching_line() {
        let mut pending = Some(PendingSlashGuardEcho::new());
        for _ in 0..MAX_PENDING_LINES {
            assert_eq!(
                PendingSlashGuardEcho::filter(&mut pending, b"background\r\n").as_ref(),
                b"background\r\n"
            );
        }
        assert!(pending.is_none());
    }
}
