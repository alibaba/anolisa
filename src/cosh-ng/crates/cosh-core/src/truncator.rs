pub struct OutputTruncator {
    pub max_bytes: usize,
    pub max_lines: usize,
}

impl Default for OutputTruncator {
    fn default() -> Self {
        Self {
            max_bytes: 25000,
            max_lines: 1000,
        }
    }
}

pub(crate) fn truncate_to_byte_limit(value: &str, max_bytes: usize) -> &str {
    let mut boundary = max_bytes.min(value.len());
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    &value[..boundary]
}

impl OutputTruncator {
    pub fn truncate(&self, output: &str) -> (String, bool) {
        let line_count = output.lines().count();
        let byte_count = output.len();

        if byte_count <= self.max_bytes && line_count <= self.max_lines {
            return (output.to_string(), false);
        }

        let truncated = if line_count > self.max_lines {
            let lines: Vec<&str> = output.lines().collect();
            let kept = &lines[..self.max_lines];
            kept.join("\n")
        } else {
            truncate_to_byte_limit(output, self.max_bytes).to_string()
        };

        let result = format!(
            "{truncated}\n\n[output truncated: {byte_count} bytes / {line_count} lines → {} bytes]",
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
            max_bytes: 100_000,
            max_lines: 3,
        };
        let input = "line1\nline2\nline3\nline4\nline5\n";
        let (result, truncated) = t.truncate(input);
        assert!(truncated);
        assert_eq!(
            result,
            "line1\nline2\nline3\n\n[output truncated: 30 bytes / 5 lines → 17 bytes]"
        );
    }

    #[test]
    fn truncates_by_byte_count() {
        let t = OutputTruncator {
            max_bytes: 10,
            max_lines: 100_000,
        };
        let input = "a]".repeat(20);
        let (result, truncated) = t.truncate(&input);
        assert!(truncated);
        assert_eq!(
            result,
            "a]a]a]a]a]\n\n[output truncated: 40 bytes / 1 lines → 10 bytes]"
        );
    }

    fn assert_truncation(input: &str, max_bytes: usize, expected: &str) {
        let t = OutputTruncator {
            max_bytes,
            max_lines: 100_000,
        };
        let (result, truncated) = t.truncate(input);

        assert!(truncated);
        assert_eq!(
            result,
            format!(
                "{expected}\n\n[output truncated: {} bytes / 1 lines → {} bytes]",
                input.len(),
                expected.len()
            )
        );
    }

    #[test]
    fn truncates_multibyte_output_at_exact_utf8_boundaries() {
        assert_truncation("中文a", 6, "中文");
        assert_truncation("😀a", 4, "😀");
    }

    #[test]
    fn rounds_multibyte_output_limits_down_to_utf8_boundaries() {
        assert_truncation("中文", 5, "中");
        assert_truncation("a😀", 3, "a");
    }

    #[test]
    fn empty_input() {
        let t = OutputTruncator::default();
        let (result, truncated) = t.truncate("");
        assert_eq!(result, "");
        assert!(!truncated);
    }
}
