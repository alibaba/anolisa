pub struct OutputTruncator {
    pub max_chars: usize,
    pub max_lines: usize,
}

impl Default for OutputTruncator {
    fn default() -> Self {
        Self {
            max_chars: 25000,
            max_lines: 1000,
        }
    }
}

impl OutputTruncator {
    pub fn truncate(&self, output: &str) -> (String, bool) {
        let line_count = output.lines().count();
        let char_count = output.len();

        if char_count <= self.max_chars && line_count <= self.max_lines {
            return (output.to_string(), false);
        }

        let truncated = if line_count > self.max_lines {
            let lines: Vec<&str> = output.lines().collect();
            let kept = &lines[..self.max_lines];
            kept.join("\n")
        } else {
            output[..self.max_chars].to_string()
        };

        let result = format!(
            "{truncated}\n\n[output truncated: {char_count} chars / {line_count} lines → {} chars]",
            truncated.len()
        );
        (result, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_truncation_when_within_limits() {
        let t = OutputTruncator::default();
        let (result, truncated) = t.truncate("hello world");
        assert_eq!(result, "hello world");
        assert!(!truncated);
    }

    #[test]
    fn truncates_by_line_count() {
        let t = OutputTruncator {
            max_chars: 100_000,
            max_lines: 3,
        };
        let input = "line1\nline2\nline3\nline4\nline5\n";
        let (result, truncated) = t.truncate(input);
        assert!(truncated);
        assert!(result.starts_with("line1\nline2\nline3"));
        assert!(result.contains("[output truncated:"));
    }

    #[test]
    fn truncates_by_char_count() {
        let t = OutputTruncator {
            max_chars: 10,
            max_lines: 100_000,
        };
        let input = "a]".repeat(20);
        let (result, truncated) = t.truncate(&input);
        assert!(truncated);
        assert!(result.contains("[output truncated:"));
    }

    #[test]
    fn empty_input() {
        let t = OutputTruncator::default();
        let (result, truncated) = t.truncate("");
        assert_eq!(result, "");
        assert!(!truncated);
    }
}
