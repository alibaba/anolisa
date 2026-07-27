//! Shared generation counter for real user bytes written to the shell PTY.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Monotonic counter shared between the input relay and the PTY output loop.
///
/// The relay bumps it *before* writing user bytes to the PTY master, so any
/// PTY output produced in response to those bytes is only readable after the
/// bump. The output loop compares snapshots against this counter to expire
/// stale prompt-replay state without depending on channel drain timing.
#[derive(Debug, Clone, Default)]
pub(crate) struct UserPtyInputGeneration(Arc<AtomicU64>);

impl UserPtyInputGeneration {
    /// Records one batch of user bytes about to reach the PTY; returns the
    /// new generation.
    pub(crate) fn bump(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub(crate) fn current(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";
const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";
// Default `operate-and-get-next` binding: accepts the current line exactly
// like Enter, so it must be treated as a submission candidate.
const CTRL_O: u8 = 0x0f;

/// Counts line-submission candidates in relayed user bytes.
///
/// This is a heuristic over-approximation of readline's logical accept-line:
/// CR, LF, and Ctrl-O (bash's default `operate-and-get-next`) count; custom
/// `.inputrc` bindings to `accept-line` cannot be enumerated from the byte
/// stream and stay uncovered. Over-counts (e.g. Ctrl-O typed inside a
/// foreground program) are reconciled by the prompt-replay tracker once the
/// shell idles at a prompt.
///
/// Newlines inside a bracketed paste are inserted literally by readline and
/// never produce a prompt boundary, so they are excluded. Paste delimiters
/// split across writes are buffered until they can be classified.
#[derive(Debug, Default)]
pub(super) struct LineSubmitCounter {
    in_bracketed_paste: bool,
    pending_delimiter: Vec<u8>,
}

impl LineSubmitCounter {
    pub(super) fn count(&mut self, bytes: &[u8]) -> usize {
        let mut scan = std::mem::take(&mut self.pending_delimiter);
        scan.extend_from_slice(bytes);
        let mut submits = 0;
        let mut idx = 0;
        while idx < scan.len() {
            let rest = &scan[idx..];
            if rest.starts_with(BRACKETED_PASTE_START) {
                self.in_bracketed_paste = true;
                idx += BRACKETED_PASTE_START.len();
                continue;
            }
            if rest.starts_with(BRACKETED_PASTE_END) {
                self.in_bracketed_paste = false;
                idx += BRACKETED_PASTE_END.len();
                continue;
            }
            if is_paste_delimiter_prefix(rest) {
                // Keep the partial delimiter for the next write; ESC-sequence
                // prefixes never contain CR/LF, so nothing is under-counted.
                self.pending_delimiter = rest.to_vec();
                return submits;
            }
            if matches!(scan[idx], b'\r' | b'\n' | CTRL_O) && !self.in_bracketed_paste {
                submits += 1;
            }
            idx += 1;
        }
        submits
    }
}

fn is_paste_delimiter_prefix(bytes: &[u8]) -> bool {
    bytes.len() < BRACKETED_PASTE_START.len()
        && (BRACKETED_PASTE_START.starts_with(bytes) || BRACKETED_PASTE_END.starts_with(bytes))
}

#[cfg(test)]
mod tests {
    use super::{LineSubmitCounter, UserPtyInputGeneration};

    #[test]
    fn bump_is_visible_through_clones() {
        let generation = UserPtyInputGeneration::default();
        let shared = generation.clone();

        assert_eq!(generation.current(), 0);
        assert_eq!(shared.bump(), 1);
        assert_eq!(generation.current(), 1);
    }

    #[test]
    fn line_submit_counter_counts_cr_and_lf() {
        let mut counter = LineSubmitCounter::default();

        assert_eq!(counter.count(b"ls -la\r"), 1);
        assert_eq!(counter.count(b"\r\r"), 2);
        assert_eq!(counter.count(b"abc"), 0);
    }

    #[test]
    fn line_submit_counter_counts_ctrl_o_as_submission() {
        let mut counter = LineSubmitCounter::default();

        assert_eq!(counter.count(b"ls -la\x0f"), 1);
        assert_eq!(counter.count(b"\x1b[200~\x0f\x1b[201~"), 0);
    }

    #[test]
    fn line_submit_counter_ignores_newlines_inside_bracketed_paste() {
        let mut counter = LineSubmitCounter::default();

        assert_eq!(counter.count(b"\x1b[200~line1\nline2\x1b[201~\r"), 1);
    }

    #[test]
    fn line_submit_counter_tracks_paste_state_across_writes() {
        let mut counter = LineSubmitCounter::default();

        assert_eq!(counter.count(b"\x1b[200~one\n"), 0);
        assert_eq!(counter.count(b"two\n\x1b[201~"), 0);
        assert_eq!(counter.count(b"\r"), 1);
    }

    #[test]
    fn line_submit_counter_buffers_split_paste_delimiter() {
        let mut counter = LineSubmitCounter::default();

        assert_eq!(counter.count(b"\x1b[20"), 0);
        assert_eq!(counter.count(b"0~in\n"), 0);
        assert_eq!(counter.count(b"\x1b[201~\r"), 1);
    }
}
