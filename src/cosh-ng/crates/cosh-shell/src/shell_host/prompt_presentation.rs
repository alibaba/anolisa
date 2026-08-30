//! Renders input-ownership state without changing the child shell's prompt.

use std::collections::VecDeque;
use std::io::{self, Write};

use crate::input::AssistanceControl;

use super::osc::OscParser;
use super::prompt_replay::{prompt_prefixed_replay_bytes, PromptReplayTracker};

const ASSISTED_SHELL_PREFIX: &[u8] = "◇ ".as_bytes();
const SHELL_ONLY_PREFIX: &[u8] = "◌ ".as_bytes();

/// Out-of-band prompt presentation for one interactive shell session.
///
/// Prompt boundaries come from the Enhanced hook's `prompt_ready` marker. The
/// prefix is written only to the outer terminal, so PS1/PROMPT, readline and
/// command text remain byte-for-byte owned by the child shell.
pub(super) struct PromptPresentation {
    enhanced: bool,
    assistance_control: Option<AssistanceControl>,
    pending_starts: VecDeque<usize>,
}

impl PromptPresentation {
    pub(super) fn new(enhanced: bool) -> Self {
        Self {
            enhanced,
            assistance_control: None,
            pending_starts: VecDeque::new(),
        }
    }

    pub(super) fn with_assistance_control(mut self, control: AssistanceControl) -> Self {
        self.assistance_control = Some(control);
        self
    }

    fn prefix(&self) -> Option<&'static [u8]> {
        if !self.enhanced {
            return None;
        }
        Some(
            if self
                .assistance_control
                .as_ref()
                .is_none_or(AssistanceControl::is_enabled)
            {
                ASSISTED_SHELL_PREFIX
            } else {
                SHELL_ONLY_PREFIX
            },
        )
    }

    pub(super) fn observe(&mut self, parser: &mut OscParser) {
        if self.enhanced {
            self.pending_starts
                .extend(parser.drain_prompt_presentation_display_starts());
        } else {
            parser.drain_prompt_presentation_display_starts();
        }
    }

    pub(super) fn write_range<W: Write>(
        &mut self,
        parser: &OscParser,
        start: usize,
        end: usize,
        output: &mut W,
    ) -> io::Result<()> {
        self.discard_before(start);
        let mut cursor = start;
        while let Some(boundary) = self.pending_starts.front().copied() {
            if boundary >= end {
                break;
            }
            parser.write_display_range(cursor, boundary, output)?;
            if let Some(prefix) = self.prefix() {
                output.write_all(prefix)?;
            }
            self.pending_starts.pop_front();
            cursor = boundary;
        }
        parser.write_display_range(cursor, end, output)
    }

    /// Writes bytes already transformed by prompt replay normalization while
    /// retaining the virtual boundary that belongs to their source range.
    pub(super) fn write_transformed_range<W: Write>(
        &mut self,
        start: usize,
        end: usize,
        bytes: &[u8],
        output: &mut W,
    ) -> io::Result<()> {
        self.discard_before(start);
        if bytes.len() == end.saturating_sub(start) {
            let mut cursor = start;
            while let Some(boundary) = self.pending_starts.front().copied() {
                if boundary >= end {
                    break;
                }
                output.write_all(&bytes[cursor - start..boundary - start])?;
                if let Some(prefix) = self.prefix() {
                    output.write_all(prefix)?;
                }
                self.pending_starts.pop_front();
                cursor = boundary;
            }
            return output.write_all(&bytes[cursor - start..]);
        }

        // Zsh may prepend a partial-line marker that replay normalization
        // removes. Its prompt_ready boundary is still the start of the range.
        if self.pending_starts.front().copied() == Some(start) && !bytes.is_empty() {
            if let Some(prefix) = self.prefix() {
                output.write_all(prefix)?;
            }
            self.pending_starts.pop_front();
        }
        self.discard_before(end);
        output.write_all(bytes)
    }

    pub(super) fn write_replayed_prompt<W: Write>(
        &self,
        output: &mut W,
        prompt: &[u8],
    ) -> io::Result<()> {
        if !prompt.is_empty() {
            if let Some(prefix) = self.prefix() {
                output.write_all(prefix)?;
            }
        }
        output.write_all(prompt)
    }

    pub(super) fn write_display_slice<W: Write>(
        &mut self,
        parser: &OscParser,
        output: &mut W,
        display_start: usize,
        display_end: usize,
        prompt_replay: &mut PromptReplayTracker,
    ) -> io::Result<()> {
        let prompt = parser.last_prompt_display();
        let prefix_len = display_end
            .saturating_sub(display_start)
            .min(prompt.len().max(prompt_replay.pending_prompt_len()).max(1));
        let prefix_end = display_start.saturating_add(prefix_len);
        let prefix = parser.read_display_range(display_start, prefix_end)?;
        let bytes = prompt_replay.strip(prefix.as_ref());
        let effective_start = prefix_end.saturating_sub(bytes.len());
        self.discard_before(effective_start);
        let normalized = prompt_prefixed_replay_bytes(bytes, prompt);
        self.write_transformed_range(effective_start, prefix_end, normalized.as_ref(), output)?;
        self.write_range(parser, prefix_end, display_end, output)
    }

    pub(super) fn discard_before(&mut self, position: usize) {
        while self
            .pending_starts
            .front()
            .is_some_and(|boundary| *boundary < position)
        {
            self.pending_starts.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_prefix_distinguishes_all_three_shell_states() {
        let mut output = Vec::new();
        PromptPresentation::new(true)
            .write_replayed_prompt(&mut output, b"alice$ ")
            .expect("assisted prompt");
        assert_eq!(output, "◇ alice$ ".as_bytes());

        output.clear();
        let state_file = std::env::temp_dir().join(format!(
            "cosh-shell-prompt-presentation-{}",
            std::process::id()
        ));
        let control = AssistanceControl::enabled(state_file);
        control.toggle().expect("disable assistance");
        PromptPresentation::new(true)
            .with_assistance_control(control)
            .write_replayed_prompt(&mut output, b"alice$ ")
            .expect("Shell-only prompt");
        assert_eq!(output, "◌ alice$ ".as_bytes());

        output.clear();
        PromptPresentation::new(false)
            .write_replayed_prompt(&mut output, b"alice$ ")
            .expect("native prompt");
        assert_eq!(output, b"alice$ ");
    }
}
