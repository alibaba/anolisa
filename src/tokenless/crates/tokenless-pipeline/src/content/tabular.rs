use super::non_empty_lines;

/// Markdown tables, or CSV/TSV with a consistent delimiter count across the
/// first non-empty lines.
pub(super) fn is_tabular(scan: &str) -> bool {
    let lines: Vec<&str> = non_empty_lines(scan).take(10).collect();
    if lines.len() < 3 {
        return false;
    }
    let markdown = lines[0].starts_with('|')
        && lines[0].ends_with('|')
        && lines[1].chars().all(|c| matches!(c, '|' | '-' | ':' | ' '));
    if markdown {
        return true;
    }
    let consistent = |delim: char, min: usize| {
        let first = lines[0].matches(delim).count();
        first >= min && lines.iter().all(|l| l.matches(delim).count() == first)
    };
    consistent('\t', 1) || consistent(',', 2)
}
