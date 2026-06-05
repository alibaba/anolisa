use std::io::{self, Write};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct AgentStatusAnimation {
    enabled: bool,
    visible: bool,
    frame: usize,
    last_render_at: Option<Instant>,
}

impl AgentStatusAnimation {
    pub(super) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            visible: false,
            frame: 0,
            last_render_at: None,
        }
    }

    pub fn render<W: Write>(&mut self, output: &mut W, label: &str) -> io::Result<()> {
        if !self.enabled {
            return Ok(());
        }

        let now = Instant::now();
        if self
            .last_render_at
            .is_some_and(|last| now.duration_since(last) < Duration::from_millis(220))
        {
            return Ok(());
        }

        const FRAMES: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];
        write!(output, "\r\x1b[2K")?;
        write!(output, "{} {}", FRAMES[self.frame % FRAMES.len()], label)?;
        output.flush()?;

        self.frame += 1;
        self.last_render_at = Some(now);
        self.visible = true;
        Ok(())
    }

    pub fn clear<W: Write>(&mut self, output: &mut W) -> io::Result<()> {
        if self.enabled && self.visible {
            write!(output, "\r\x1b[2K\r")?;
            output.flush()?;
            self.visible = false;
        }
        Ok(())
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
}
