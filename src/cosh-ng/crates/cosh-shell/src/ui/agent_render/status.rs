use std::io::{self, Write};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct AgentStatusAnimation {
    enabled: bool,
    visible: bool,
    frame: usize,
    last_render_at: Option<Instant>,
    last_label: Option<String>,
    /// Set `true` after `clear()` so the next `render()` emits a leading
    /// newline, guaranteeing the spinner starts on a fresh line below any
    /// panel that was written between the clear and the repaint.
    needs_leading_newline: bool,
}

impl AgentStatusAnimation {
    pub(super) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            visible: false,
            frame: 0,
            last_render_at: None,
            last_label: None,
            needs_leading_newline: false,
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
        // When the spinner is being repainted after a clear (i.e. a panel was
        // just drawn), emit a leading newline so the spinner always starts on
        // a fresh line below the panel's bottom border rather than potentially
        // sharing the border row.
        if self.needs_leading_newline {
            writeln!(output)?;
            self.needs_leading_newline = false;
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
            write!(output, "\r\x1b[2K\r")?;
            output.flush()?;
            self.visible = false;
            self.last_label = None;
            self.needs_leading_newline = true;
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
    fn render_after_clear_emits_leading_newline() {
        let mut animation = AgentStatusAnimation::new(true);
        let mut output = Vec::new();

        animation.render(&mut output, "Thinking").expect("render");
        animation.clear(&mut output).expect("clear");
        let before = output.len();
        animation
            .render(&mut output, "Thinking: running shell command")
            .expect("render after clear");

        let rendered = String::from_utf8(output[before..].to_vec()).unwrap();
        assert!(
            rendered.starts_with('\n'),
            "expected leading newline to protect card border, got: {rendered:?}"
        );
    }

    #[test]
    fn subsequent_renders_do_not_add_extra_newlines() {
        let mut animation = AgentStatusAnimation::new(true);
        let mut output = Vec::new();

        animation.render(&mut output, "Thinking").expect("first");
        animation.clear(&mut output).expect("clear");
        animation
            .render(&mut output, "Thinking")
            .expect("second (after clear)");
        let after_second = output.len();
        animation
            .render(&mut output, "Thinking: step 2")
            .expect("third (no extra clear)");

        let third_render = String::from_utf8(output[after_second..].to_vec()).unwrap();
        assert!(
            !third_render.starts_with('\n'),
            "subsequent render should not add leading newline: {third_render:?}"
        );
    }
}
