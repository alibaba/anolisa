use std::io::{self, Write};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct AgentStatusAnimation {
    enabled: bool,
    visible: bool,
    frame: usize,
    last_render_at: Option<Instant>,
    last_label: Option<String>,
}

impl AgentStatusAnimation {
    pub(super) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            visible: false,
            frame: 0,
            last_render_at: None,
            last_label: None,
        }
    }

    pub fn render<W: Write>(&mut self, output: &mut W, label: &str) -> io::Result<()> {
        if !self.enabled {
            return Ok(());
        }

        let now = Instant::now();
        let label_changed = self.last_label.as_deref() != Some(label);
        if !label_changed
            && self
                .last_render_at
                .is_some_and(|last| now.duration_since(last) < Duration::from_millis(220))
        {
            return Ok(());
        }

        const FRAMES: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];
        // When the spinner first appears (or reappears after clear), drop it
        // onto a dedicated line so the \r\x1b[2K sequence never overwrites a
        // card's bottom border that was just rendered above the cursor.
        if !self.visible {
            writeln!(output)?;
        }
        write!(output, "\r\x1b[2K")?;
        write!(output, "{} {}", FRAMES[self.frame % FRAMES.len()], label)?;
        output.flush()?;

        self.frame += 1;
        self.last_render_at = Some(now);
        self.last_label = Some(label.to_string());
        self.visible = true;
        Ok(())
    }

    pub fn clear<W: Write>(&mut self, output: &mut W) -> io::Result<()> {
        if self.enabled && self.visible {
            // Erase the spinner line, then move up to reclaim the blank line
            // the initial render() inserted so no extra whitespace accumulates
            // between inline cards across spinner cycles.
            write!(output, "\r\x1b[2K\x1b[1A\r\x1b[2K")?;
            output.flush()?;
            self.visible = false;
            self.last_label = None;
        }
        Ok(())
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_same_label_is_throttled_without_clearing() {
        let mut animation = AgentStatusAnimation::new(true);
        let mut output = Vec::new();

        animation
            .render(&mut output, "Thinking")
            .expect("first render");
        let first = output.len();
        animation
            .render(&mut output, "Thinking")
            .expect("second render");

        assert_eq!(output.len(), first);
    }

    #[test]
    fn changed_label_renders_immediately() {
        let mut animation = AgentStatusAnimation::new(true);
        let mut output = Vec::new();

        animation
            .render(&mut output, "Thinking")
            .expect("first render");
        let first = output.len();
        animation
            .render(&mut output, "Thinking: reading file")
            .expect("changed render");

        assert!(output.len() > first);
    }

    #[test]
    fn first_render_emits_newline_to_protect_card_border() {
        let mut animation = AgentStatusAnimation::new(true);
        let mut output = Vec::new();

        animation.render(&mut output, "Thinking").expect("first render");
        let text = String::from_utf8(output).unwrap();

        // The first render must emit a newline before the spinner so it
        // occupies a dedicated line below any previously rendered card.
        assert!(
            text.starts_with('\n'),
            "first render should start with \\n to avoid overwriting card borders: {text:?}"
        );
        assert!(
            text.contains("Thinking"),
            "spinner label should be visible: {text:?}"
        );
    }

    #[test]
    fn subsequent_renders_do_not_emit_extra_newline() {
        let mut animation = AgentStatusAnimation::new(true);
        let mut output = Vec::new();

        animation.render(&mut output, "Thinking").expect("first render");
        animation.last_render_at = Some(Instant::now() - Duration::from_millis(300));
        let first_len = output.len();

        animation
            .render(&mut output, "Thinking")
            .expect("second render");
        let second_chunk = String::from_utf8(output[first_len..].to_vec()).unwrap();

        assert!(
            !second_chunk.starts_with('\n'),
            "subsequent render should not emit \\n: {second_chunk:?}"
        );
    }

    #[test]
    fn clear_reclaims_blank_line() {
        let mut animation = AgentStatusAnimation::new(true);
        let mut output = Vec::new();

        animation.render(&mut output, "Thinking").expect("render");
        animation.clear(&mut output).expect("clear");
        let text = String::from_utf8(output).unwrap();

        assert!(
            text.contains("\x1b[1A"),
            "clear should move cursor up to reclaim blank line: {text:?}"
        );
    }

    #[test]
    fn render_after_clear_emits_newline_again() {
        let mut animation = AgentStatusAnimation::new(true);
        let mut output = Vec::new();

        animation.render(&mut output, "Thinking").expect("first render");
        animation.clear(&mut output).expect("clear");
        let pre_rerender = output.len();

        animation
            .render(&mut output, "Still thinking")
            .expect("re-render");
        let rerender_chunk = String::from_utf8(output[pre_rerender..].to_vec()).unwrap();

        assert!(
            rerender_chunk.starts_with('\n'),
            "render after clear should emit \\n again: {rerender_chunk:?}"
        );
    }
}
