//! Prefix-preserving template grouping for gap summaries.
//!
//! The prefix up to and including the first `:` or `=` stays verbatim; only
//! the remainder is normalized (digit runs → `0`, hex runs of 8+ chars →
//! `h`). Keeping the prefix verbatim means grouping can never merge lines
//! whose discriminating identity lives before the delimiter — the lesson
//! learned from over-aggressive masking in prior art. Lines without a
//! delimiter group by exact bytes only.

use std::collections::HashMap;

pub(crate) fn mask(line: &str) -> String {
    match line.find([':', '=']) {
        Some(pos) => {
            let (prefix, rest) = line.split_at(pos + 1);
            let mut out = String::with_capacity(line.len());
            out.push_str(prefix);
            normalize_into(rest, &mut out);
            out
        }
        None => line.to_string(),
    }
}

fn normalize_into(rest: &str, out: &mut String) {
    let chars: Vec<char> = rest.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_hexdigit() {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let mut j = i;
        while j < chars.len() && chars[j].is_ascii_hexdigit() {
            j += 1;
        }
        if j - i >= 8 {
            out.push('h');
        } else {
            // Inside a short run, digit sub-runs collapse to `0` and the
            // a-f letters (ordinary prose at this length) stay verbatim.
            let mut k = i;
            while k < j {
                if chars[k].is_ascii_digit() {
                    while k < j && chars[k].is_ascii_digit() {
                        k += 1;
                    }
                    out.push('0');
                } else {
                    out.push(chars[k]);
                    k += 1;
                }
            }
        }
        i = j;
    }
}

/// Top templates among a gap's noise lines: `(count, mask)` ordered by count
/// descending then first appearance, keeping only groups of `min_count`+.
pub(crate) fn top_templates(
    lines: &[&str],
    min_count: usize,
    top_k: usize,
) -> Vec<(usize, String)> {
    let mut counts: HashMap<String, (usize, usize)> = HashMap::new();
    for (idx, line) in lines.iter().enumerate() {
        let entry = counts.entry(mask(line)).or_insert((0, idx));
        entry.0 += 1;
    }
    let mut groups: Vec<(usize, usize, String)> = counts
        .into_iter()
        .map(|(mask, (count, first))| (count, first, mask))
        .collect();
    groups.retain(|(count, _, _)| *count >= min_count);
    groups.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    groups.truncate(top_k);
    groups
        .into_iter()
        .map(|(count, _, mask)| (count, mask))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    include!("../tests/template_tests.rs");
}
