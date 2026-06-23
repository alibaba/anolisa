//! Query and formatting utilities for tokenless stats.

use std::collections::{BTreeMap, HashMap};

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
    ops.sort_by_key(|b| std::cmp::Reverse(b.1.total_records));

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

/// Format summary as machine-readable JSON.
///
/// Output structure:
/// ```json
/// {
///   "total": { "records": N, "before_tokens": N, "after_tokens": N,
///              "chars_saved_percent": 83.0, "tokens_saved_percent": 83.0, ... },
///   "by_operation": { "compress-response": { "records": N, ... }, ... }
/// }
/// ```
pub fn format_summary_json(records: &[StatsRecord]) -> String {
    let total = StatsSummary::from_records(records);

    let mut by_op: BTreeMap<&str, StatsSummary> = BTreeMap::new();
    for r in records {
        let entry = by_op.entry(r.operation.as_str()).or_default();
        entry.total_records += 1;
        entry.total_before_chars += r.before_chars;
        entry.total_after_chars += r.after_chars;
        entry.total_before_tokens += r.before_tokens;
        entry.total_after_tokens += r.after_tokens;
    }

    let by_op_json: serde_json::Map<String, serde_json::Value> = by_op
        .iter()
        .map(|(op, s)| {
            (
                op.to_string(),
                serde_json::json!({
                    "records": s.total_records,
                    "before_chars": s.total_before_chars,
                    "after_chars": s.total_after_chars,
                    "chars_saved": s.chars_saved(),
                    "before_tokens": s.total_before_tokens,
                    "after_tokens": s.total_after_tokens,
                    "tokens_saved": s.tokens_saved(),
                    "chars_saved_percent": s.chars_percent(),
                    "tokens_saved_percent": s.tokens_percent(),
                }),
            )
        })
        .collect();

    let mut total_json = serde_json::to_value(&total).unwrap_or_default();
    if let Some(obj) = total_json.as_object_mut() {
        obj.insert(
            "chars_saved".to_string(),
            serde_json::json!(total.chars_saved()),
        );
        obj.insert(
            "tokens_saved".to_string(),
            serde_json::json!(total.tokens_saved()),
        );
        obj.insert(
            "chars_saved_percent".to_string(),
            serde_json::json!(total.chars_percent()),
        );
        obj.insert(
            "tokens_saved_percent".to_string(),
            serde_json::json!(total.tokens_percent()),
        );
    }

    let output = serde_json::json!({
        "schema_version": "1.0",
        "total": total_json,
        "by_operation": by_op_json,
    });

    serde_json::to_string_pretty(&output).unwrap_or_default()
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

/// Sum tokens by operation type. `use_before` selects `before_tokens` (the
/// raw/baseline context) when true, else `after_tokens` (the compressed
/// context actually seen under tokenless).
fn tokens_by_op(records: &[StatsRecord], use_before: bool) -> BTreeMap<&'static str, usize> {
    let mut map: BTreeMap<&'static str, usize> = BTreeMap::new();
    for r in records {
        let t = if use_before {
            r.before_tokens
        } else {
            r.after_tokens
        };
        *map.entry(r.operation.as_str()).or_default() += t;
    }
    map
}

/// Format a side-by-side comparison between a baseline (compression-off /
/// dry-run) run and a tokenless (compression-on / active) run.
///
/// Baseline context uses each record's `before_tokens` (the raw text that
/// reached the LLM); the tokenless side uses `after_tokens` (the compressed
/// text). Savings = baseline − tokenless.
pub fn format_compare(baseline: &[StatsRecord], tokenless: &[StatsRecord]) -> String {
    let base_by_op = tokens_by_op(baseline, true);
    let tls_by_op = tokens_by_op(tokenless, false);

    let base_total: usize = base_by_op.values().sum();
    let tls_total: usize = tls_by_op.values().sum();

    let mut ops: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    ops.extend(base_by_op.keys().copied());
    ops.extend(tls_by_op.keys().copied());

    let saved_total = base_total.saturating_sub(tls_total);
    let saved_pct = if base_total > 0 {
        (saved_total as f64 / base_total as f64) * 100.0
    } else {
        0.0
    };

    let mut output = String::new();
    output.push_str("Tokenless Comparison Report\n");
    output.push_str(&"=".repeat(60));
    output.push('\n');
    output.push_str(&format!(
        "{:<22}{:>12}{:>14}{:>10}{:>12}\n",
        "operation", "baseline", "tokenless", "saved", "saved%"
    ));
    output.push_str(&"-".repeat(68));
    output.push('\n');

    for op in ops {
        let b = *base_by_op.get(op).unwrap_or(&0);
        let t = *tls_by_op.get(op).unwrap_or(&0);
        let saved = b.saturating_sub(t);
        let pct = if b > 0 {
            (saved as f64 / b as f64) * 100.0
        } else {
            0.0
        };
        output.push_str(&format!(
            "{:<22}{:>12}{:>14}{:>10}{:>11.1}%\n",
            op, b, t, saved, pct
        ));
    }

    output.push_str(&"-".repeat(68));
    output.push('\n');
    output.push_str(&format!(
        "{:<22}{:>12}{:>14}{:>10}{:>11.1}%\n",
        "TOTAL", base_total, tls_total, saved_total, saved_pct
    ));

    output
}

/// Format the comparison as machine-readable JSON.
pub fn format_compare_json(baseline: &[StatsRecord], tokenless: &[StatsRecord]) -> String {
    let base_by_op = tokens_by_op(baseline, true);
    let tls_by_op = tokens_by_op(tokenless, false);

    let base_total: usize = base_by_op.values().sum();
    let tls_total: usize = tls_by_op.values().sum();
    let saved_total = base_total.saturating_sub(tls_total);
    let saved_pct = if base_total > 0 {
        (saved_total as f64 / base_total as f64) * 100.0
    } else {
        0.0
    };

    let by_op_json = |map: &BTreeMap<&str, usize>| -> serde_json::Map<String, serde_json::Value> {
        map.iter()
            .map(|(op, &v)| (op.to_string(), serde_json::json!(v)))
            .collect()
    };

    let output = serde_json::json!({
        "schema_version": "1.0",
        "baseline_tokens": base_total,
        "tokenless_tokens": tls_total,
        "saved_tokens": saved_total,
        "saved_percent": saved_pct,
        "baseline_by_operation": by_op_json(&base_by_op),
        "tokenless_by_operation": by_op_json(&tls_by_op),
    });

    serde_json::to_string_pretty(&output).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{CompressionMode, OperationType, StatsRecord};
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
    fn test_format_summary_json_valid() {
        let records = vec![test_record()];
        let output = format_summary_json(&records);
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();

        // schema_version
        assert_eq!(parsed.get("schema_version").unwrap(), "1.0");

        let total = parsed.get("total").unwrap();
        // StatsRecord::new(op, agent, before_chars=1000, before_tokens=400,
        //                  after_chars=500, after_tokens=200)
        assert_eq!(total.get("records").unwrap(), 1);
        assert_eq!(total.get("before_chars").unwrap(), 1000);
        assert_eq!(total.get("after_chars").unwrap(), 500);
        assert_eq!(total.get("before_tokens").unwrap(), 400);
        assert_eq!(total.get("after_tokens").unwrap(), 200);
        // absolute saved values (shiloong review feedback)
        assert_eq!(total.get("chars_saved").unwrap(), 500);
        assert_eq!(total.get("tokens_saved").unwrap(), 200);
        assert!(total.get("chars_saved_percent").unwrap().as_f64().unwrap() > 0.0);
        assert!(total.get("tokens_saved_percent").unwrap().as_f64().unwrap() > 0.0);

        let ops = parsed.get("by_operation").unwrap().as_object().unwrap();
        assert!(ops.contains_key("compress-schema"));
        let op = ops.get("compress-schema").unwrap();
        assert_eq!(op.get("records").unwrap(), 1);
        assert_eq!(op.get("chars_saved").unwrap(), 500);
        assert_eq!(op.get("tokens_saved").unwrap(), 200);
    }

    #[test]
    fn test_format_summary_json_empty() {
        let records: Vec<StatsRecord> = vec![];
        let output = format_summary_json(&records);
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();

        let total = parsed.get("total").unwrap();
        assert_eq!(total.get("records").unwrap(), 0);
        assert_eq!(
            total.get("chars_saved_percent").unwrap().as_f64().unwrap(),
            0.0
        );
        assert_eq!(
            total.get("tokens_saved_percent").unwrap().as_f64().unwrap(),
            0.0
        );

        let ops = parsed.get("by_operation").unwrap().as_object().unwrap();
        assert!(ops.is_empty());
    }

    #[test]
    fn test_format_summary_json_field_consistency() {
        let records = vec![test_record()];
        let output = format_summary_json(&records);
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();

        let total_keys: std::collections::BTreeSet<String> = parsed
            .get("total")
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        let ops = parsed.get("by_operation").unwrap().as_object().unwrap();
        for (_op, val) in ops {
            let op_keys: std::collections::BTreeSet<String> =
                val.as_object().unwrap().keys().cloned().collect();
            assert_eq!(total_keys, op_keys, "field names must be identical");
        }
    }

    #[test]
    fn test_format_summary_json_ordered_operations() {
        let mut r1 = test_record();
        r1.operation = OperationType::CompressResponse;

        let mut r2 = test_record();
        r2.operation = OperationType::RewriteCommand;

        let mut r3 = test_record();
        r3.operation = OperationType::CompressSchema;

        let records = vec![r2, r1, r3]; // intentionally unordered
        let output = format_summary_json(&records);
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();

        let ops = parsed.get("by_operation").unwrap().as_object().unwrap();
        let keys: Vec<&String> = ops.keys().collect();
        // BTreeMap sorts lexicographically
        assert_eq!(
            keys,
            vec!["compress-response", "compress-schema", "rewrite-command"]
        );
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

    fn compare_record(
        op: OperationType,
        mode: CompressionMode,
        before: usize,
        after: usize,
    ) -> StatsRecord {
        let mut r = StatsRecord::new(op, "cli".to_string(), before * 4, before, after * 4, after)
            .with_mode(mode);
        r.timestamp = Local::now();
        r
    }

    #[test]
    fn test_format_compare_totals_and_delta() {
        // Baseline (dry-run): context reached = before_tokens.
        // Tokenless (active): context reached = after_tokens.
        let baseline = vec![
            compare_record(
                OperationType::CompressSchema,
                CompressionMode::DryRun,
                400,
                200,
            ),
            compare_record(
                OperationType::CompressResponse,
                CompressionMode::DryRun,
                1000,
                300,
            ),
        ];
        let tokenless = vec![
            compare_record(
                OperationType::CompressSchema,
                CompressionMode::Active,
                400,
                200,
            ),
            compare_record(
                OperationType::CompressResponse,
                CompressionMode::Active,
                1000,
                300,
            ),
        ];

        let out = format_compare(&baseline, &tokenless);
        // baseline total = 400+1000 = 1400; tokenless total = 200+300 = 500
        assert!(out.contains("TOTAL"));
        assert!(out.contains(&format!("{:>12}", 1400)));
        assert!(out.contains(&format!("{:>14}", 500)));
        assert!(out.contains("compress-schema"));
        assert!(out.contains("compress-response"));
    }

    #[test]
    fn test_format_compare_json_fields() {
        let baseline = vec![compare_record(
            OperationType::CompressSchema,
            CompressionMode::DryRun,
            400,
            200,
        )];
        let tokenless = vec![compare_record(
            OperationType::CompressSchema,
            CompressionMode::Active,
            400,
            200,
        )];
        let parsed: serde_json::Value =
            serde_json::from_str(&format_compare_json(&baseline, &tokenless)).unwrap();
        assert_eq!(parsed.get("schema_version").unwrap(), "1.0");
        assert_eq!(parsed.get("baseline_tokens").unwrap(), 400);
        assert_eq!(parsed.get("tokenless_tokens").unwrap(), 200);
        assert_eq!(parsed.get("saved_tokens").unwrap(), 200);
        assert!(parsed.get("saved_percent").unwrap().as_f64().unwrap() > 0.0);
    }

    #[test]
    fn test_format_compare_empty() {
        let out = format_compare(&[], &[]);
        assert!(out.contains("TOTAL"));
        // no baseline tokens → saved 0%, no panic
        let parsed: serde_json::Value =
            serde_json::from_str(&format_compare_json(&[], &[])).unwrap();
        assert_eq!(parsed.get("saved_percent").unwrap().as_f64().unwrap(), 0.0);
    }
}
