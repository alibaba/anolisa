//! Query and formatting utilities for tokenless stats.

use std::collections::HashMap;

use crate::record::StatsRecord;
use crate::recorder::StatsSummary;

/// Format a summary report with overall stats and breakdown by operation type.
pub fn format_summary(records: &[StatsRecord], title: Option<&str>) -> String {
    let total = StatsSummary::from_records(records);

    let mut output = String::new();

    if let Some(t) = title {
        output.push_str(t);
        output.push('\n');
        output.push_str(&"=".repeat(60));
        output.push('\n');
    }

    output.push_str(&format!("Total Records: {}\n\n", total.total_records));

    output.push_str("Character Savings:\n");
    output.push_str(&format!("  Before: {} chars\n", total.total_before_chars));
    output.push_str(&format!("  After:  {} chars\n", total.total_after_chars));
    output.push_str(&format!(
        "  Saved:  {} chars ({:.1}%)\n\n",
        total.chars_saved(),
        total.chars_percent()
    ));

    output.push_str("Token Savings:\n");
    output.push_str(&format!("  Before: {} tokens\n", total.total_before_tokens));
    output.push_str(&format!("  After:  {} tokens\n", total.total_after_tokens));
    output.push_str(&format!(
        "  Saved:  {} tokens ({:.1}%)\n\n",
        total.tokens_saved(),
        total.tokens_percent()
    ));

    // Breakdown by operation type
    let mut by_op: HashMap<&str, StatsSummary> = HashMap::new();
    for r in records {
        let op = r.operation.as_str();
        let entry = by_op.entry(op).or_default();
        entry.total_records += 1;
        entry.total_before_chars += r.before_chars;
        entry.total_after_chars += r.after_chars;
        entry.total_before_tokens += r.before_tokens;
        entry.total_after_tokens += r.after_tokens;
    }

    output.push_str("Breakdown by Operation:\n");
    output.push_str(&"-".repeat(40));
    output.push('\n');

    let mut ops: Vec<_> = by_op.iter().collect();
    ops.sort_by(|a, b| b.1.total_records.cmp(&a.1.total_records));

    for (op, s) in ops {
        output.push_str(&format!("  {}: {} records\n", op, s.total_records));
        output.push_str(&format!(
            "    Chars: {} -> {} (-{:.1}%)\n",
            s.total_before_chars,
            s.total_after_chars,
            s.chars_percent()
        ));
        output.push_str(&format!(
            "    Tokens: {} -> {} (-{:.1}%)\n",
            s.total_before_tokens,
            s.total_after_tokens,
            s.tokens_percent()
        ));
    }

    output
}

/// Format a list of records for display
pub fn format_list(records: &[StatsRecord], limit: usize) -> String {
    if records.is_empty() {
        return "No records found.".to_string();
    }

    let display = if records.len() > limit {
        &records[..limit]
    } else {
        records
    };

    let mut output = String::new();
    output.push_str(&format!("Showing {} record(s):\n", display.len()));
    output.push_str(&"=".repeat(80));
    output.push('\n');

    for record in display {
        output.push_str(&record.format_summary_line());
        output.push('\n');
    }

    if records.len() > limit {
        output.push_str(&format!(
            "\n... and {} more (use --limit to show all)",
            records.len() - limit
        ));
    }

    output
}

/// Format a single record showing before/after text content.
/// If before and after are identical, shows original text with "(no compression)" note.
pub fn format_show(record: &StatsRecord) -> String {
    let before = record.before_text.as_deref().unwrap_or("");
    let after = record.after_text.as_deref().unwrap_or("");

    if before.is_empty() && after.is_empty() {
        return "  (no text content stored)\n".to_string();
    }

    let mut output = String::new();

    if before == after || after.is_empty() {
        // No compression happened or no after text
        output.push_str("=== Original (no compression) ===\n");
        output.push_str(before);
        if !before.is_empty() && !before.ends_with('\n') {
            output.push('\n');
        }
    } else {
        output.push_str("=== Before ===\n");
        output.push_str(before);
        if !before.is_empty() && !before.ends_with('\n') {
            output.push('\n');
        }
        output.push_str("\n=== After ===\n");
        output.push_str(after);
        if !after.is_empty() && !after.ends_with('\n') {
            output.push('\n');
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{OperationType, StatsRecord};
    use chrono::Local;

    fn test_record() -> StatsRecord {
        let mut r = StatsRecord::new(
            OperationType::CompressSchema,
            "copilot-shell".to_string(),
            1000,
            400,
            500,
            200,
        );
        r.id = 1;
        r.timestamp = Local::now();
        r.before_text = Some("original text".to_string());
        r.after_text = Some("compressed".to_string());
        r
    }

    #[test]
    fn test_format_summary() {
        let records = vec![test_record()];
        let output = format_summary(&records, Some("Test Summary"));

        assert!(output.contains("Test Summary"));
        assert!(output.contains("Total Records: 1"));
        assert!(output.contains("Character Savings"));
        assert!(output.contains("Token Savings"));
    }

    #[test]
    fn test_format_list() {
        let records = vec![test_record()];
        let output = format_list(&records, 20);

        assert!(output.contains("Showing 1 record"));
        assert!(output.contains("[ID:1]"));
    }

    #[test]
    fn test_format_show_with_compression() {
        let record = test_record();
        let output = format_show(&record);

        assert!(output.contains("=== Before ==="));
        assert!(output.contains("original text"));
        assert!(output.contains("=== After ==="));
        assert!(output.contains("compressed"));
    }

    #[test]
    fn test_format_show_no_compression() {
        let mut r = StatsRecord::new(
            OperationType::CompressSchema,
            "test".to_string(),
            100,
            25,
            100,
            25,
        );
        r.id = 2;
        r.timestamp = Local::now();
        r.before_text = Some("same text".to_string());
        r.after_text = Some("same text".to_string());

        let output = format_show(&r);
        assert!(output.contains("no compression"));
        assert!(output.contains("same text"));
        assert!(!output.contains("=== After ==="));
    }

    #[test]
    fn test_format_show_no_text_stored() {
        let mut r = StatsRecord::new(
            OperationType::CompressSchema,
            "test".to_string(),
            100,
            25,
            80,
            20,
        );
        r.id = 3;
        r.timestamp = Local::now();

        let output = format_show(&r);
        assert!(output.contains("no text content stored"));
    }

    #[test]
    fn test_format_diff_no_diff_available() {
        let mut r = StatsRecord::new(
            OperationType::CompressSchema,
            "test".to_string(),
            100,
            25,
            80,
            20,
        );
        r.id = 1;
        r.timestamp = Local::now();

        let output = format_show(&r);
        assert!(output.contains("no text content stored"));
    }
}
