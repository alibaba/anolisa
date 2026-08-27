//! Build/log compression: classify lines, protect stack traces, collapse
//! duplicate signal runs, keep signal ∪ context ∪ head/tail, and stash each
//! omitted gap behind its own retrievable marker.
//!
//! Hard rules, enforced structurally:
//! - Signal lines and trace regions are never template-grouped and never
//!   fall into an omission gap. The single sanctioned exception is a run of
//!   byte-identical Signal lines, collapsed to its first occurrence plus a
//!   repeat-count annotation that reconstructs the run exactly.
//! - Every omitted byte is owned by exactly one gap's stash entry; template
//!   summaries are derived metadata with no separate stash.
//! - All thresholds are fixed constants: no adaptive budgets, so identical
//!   input always yields identical output (golden-testable).

mod classify;
mod template;
mod trace;

use tokenless_ccr::{StashStore, StashWrite, compute_key, marker_for};

use crate::terminal_cleanup::clean_terminal;
use classify::{LineClass, LineInfo, LogLevel, analyze};
use template::top_templates;
use trace::trace_regions;

/// BuildLog engages only on real multi-line logs (§6.1: short unknown
/// output passes through unchanged).
const MIN_ENGAGE_LINES: usize = 30;
/// Always-kept head window (command echo, first context).
const HEAD_KEEP_LINES: usize = 5;
/// Always-kept tail window (final summaries, exit status).
const TAIL_KEEP_LINES: usize = 10;
/// Context lines kept around every Signal line.
const CONTEXT_LINES: usize = 2;
/// A non-kept run shorter than this stays verbatim — a marker line would
/// cost more than it saves.
const MIN_GAP_LINES: usize = 5;
/// Net character saving below this returns the input unchanged.
const MIN_SAVED_CHARS: usize = 200;
/// Minimum occurrences for a template summary line under a gap marker.
const TEMPLATE_MIN_COUNT: usize = 5;
/// Maximum template summary lines per gap.
const TEMPLATE_TOP_K: usize = 3;
/// Minimum consecutive byte-identical Signal lines to collapse.
const DUP_SIGNAL_MIN_RUN: usize = 3;
/// GenericText line-mode engagement threshold.
const GENERIC_MIN_LINES: usize = 100;
/// GenericText line-mode keep windows.
const GENERIC_HEAD_LINES: usize = 40;
const GENERIC_TAIL_LINES: usize = 40;
/// GenericText char-mode engagement threshold — mirrors the shell string
/// truncation ceiling this compressor replaces, keeping the giant
/// single-line safety net.
const GENERIC_CHAR_ENGAGE: usize = 65_536;
/// GenericText char-mode keep windows.
const GENERIC_HEAD_CHARS: usize = 16_384;
const GENERIC_TAIL_CHARS: usize = 16_384;

/// Which ruleset drives the omission plan. The caller routes by detected
/// content type: real build logs get the classifier, other long plain text
/// gets the conservative head/tail treatment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildLogMode {
    BuildLog,
    GenericText,
}

/// Engine result. `stash_writes` reports every store write in order so the
/// caller can honor the pipeline's ledger contract; a failed write keeps its
/// gap verbatim (fail-open per gap), so every emitted marker is backed.
#[derive(Debug)]
pub struct BuildLogOutcome {
    pub output: String,
    pub stash_writes: Vec<StashWrite>,
    /// Gaps replaced by markers in `output`.
    pub omitted_blocks: usize,
    /// Failed stash attempts; their gaps stayed verbatim.
    pub stash_errors: usize,
    /// Every omitted block is backed by a successful write. `false` only
    /// when markers were rendered without a store (dry-run measurement —
    /// keys are content-derived, so the markers are still deterministic).
    pub retrievable: bool,
}

pub fn compress_log(
    text: &str,
    mode: BuildLogMode,
    stash: Option<&dyn StashStore>,
) -> BuildLogOutcome {
    match mode {
        BuildLogMode::BuildLog => build_log_mode(text, stash),
        BuildLogMode::GenericText => generic_mode(text, stash),
    }
}

fn unchanged(text: &str) -> BuildLogOutcome {
    BuildLogOutcome {
        output: text.to_string(),
        stash_writes: Vec::new(),
        omitted_blocks: 0,
        stash_errors: 0,
        retrievable: true,
    }
}

enum Segment<'a> {
    /// Emitted exactly as received.
    Verbatim(&'a str),
    /// Annotation after the first line of a collapsed duplicate-Signal run.
    DupNote(usize),
    /// An omitted block backed by one stash entry.
    Gap(GapPlan),
}

struct GapPlan {
    payload: String,
    omitted_lines: usize,
    /// info, debug, trace, notice, other.
    histogram: [usize; 5],
    templates: Vec<(usize, String)>,
}

fn build_log_mode(text: &str, stash: Option<&dyn StashStore>) -> BuildLogOutcome {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let n = lines.len();
    if n < MIN_ENGAGE_LINES {
        return unchanged(text);
    }
    // Classification and trace detection run on the colour-stripped view, the
    // same view the lossless stage emits; kept lines are still emitted
    // verbatim as received.
    let stripped: Vec<String> = lines
        .iter()
        .map(|line| clean_terminal(line.trim_end_matches('\n')))
        .collect();
    let info: Vec<LineInfo> = stripped.iter().map(|s| analyze(s)).collect();

    let mut keep = vec![false; n];
    for flag in keep.iter_mut().take(HEAD_KEEP_LINES) {
        *flag = true;
    }
    for flag in keep.iter_mut().skip(n.saturating_sub(TAIL_KEEP_LINES)) {
        *flag = true;
    }
    for (i, line_info) in info.iter().enumerate() {
        if line_info.class == LineClass::Signal {
            let lo = i.saturating_sub(CONTEXT_LINES);
            let hi = (i + CONTEXT_LINES + 1).min(n);
            for flag in &mut keep[lo..hi] {
                *flag = true;
            }
        }
    }
    let mut in_trace = vec![false; n];
    for region in trace_regions(&stripped) {
        for flag in &mut in_trace[region.clone()] {
            *flag = true;
        }
        for flag in &mut keep[region] {
            *flag = true;
        }
    }

    // Collapse runs of byte-identical Signal lines. Raw bytes (newline
    // included) so expansion by count reconstructs exactly; trace regions
    // are atomic and never collapsed.
    let mut elided = vec![false; n];
    let mut dup_note: Vec<Option<usize>> = vec![None; n];
    let mut i = 0;
    while i < n {
        if info[i].class == LineClass::Signal && !in_trace[i] {
            let mut j = i + 1;
            while j < n && !in_trace[j] && lines[j] == lines[i] {
                j += 1;
            }
            if j - i >= DUP_SIGNAL_MIN_RUN {
                for flag in &mut elided[i + 1..j] {
                    *flag = true;
                }
                dup_note[i] = Some(j - i - 1);
            }
            i = j;
        } else {
            i += 1;
        }
    }

    let mut segments: Vec<Segment> = Vec::new();
    let mut gap: Vec<usize> = Vec::new();
    for idx in 0..n {
        if keep[idx] {
            flush_gap(&mut segments, &mut gap, &lines, &stripped, &info);
            if !elided[idx] {
                segments.push(Segment::Verbatim(lines[idx]));
                if let Some(repeats) = dup_note[idx] {
                    segments.push(Segment::DupNote(repeats));
                }
            }
        } else {
            gap.push(idx);
        }
    }
    flush_gap(&mut segments, &mut gap, &lines, &stripped, &info);
    assemble(text, &segments, stash)
}

/// Turn pending omitted-line indices into a gap segment, or keep them
/// verbatim when the run is too short to earn a marker.
fn flush_gap<'a>(
    segments: &mut Vec<Segment<'a>>,
    gap: &mut Vec<usize>,
    lines: &[&'a str],
    stripped: &[String],
    info: &[LineInfo],
) {
    if gap.is_empty() {
        return;
    }
    if gap.len() < MIN_GAP_LINES {
        for &idx in gap.iter() {
            segments.push(Segment::Verbatim(lines[idx]));
        }
        gap.clear();
        return;
    }
    let payload: String = gap.iter().map(|&idx| lines[idx]).collect();
    let mut histogram = [0usize; 5];
    for &idx in gap.iter() {
        histogram[level_bucket(info[idx].level)] += 1;
    }
    // Template summaries cover every omitted line (§6.1 groups repeated
    // templated lines, not a special class); Signal lines are never here.
    let omitted: Vec<&str> = gap.iter().map(|&idx| stripped[idx].as_str()).collect();
    let templates = top_templates(&omitted, TEMPLATE_MIN_COUNT, TEMPLATE_TOP_K);
    segments.push(Segment::Gap(GapPlan {
        payload,
        omitted_lines: gap.len(),
        histogram,
        templates,
    }));
    gap.clear();
}

fn level_bucket(level: LogLevel) -> usize {
    match level {
        LogLevel::Info => 0,
        LogLevel::Debug => 1,
        LogLevel::Trace => 2,
        LogLevel::Notice => 3,
        LogLevel::Other => 4,
    }
}

/// Two-phase emission. Phase A renders a candidate with content-derived
/// keys — key length is fixed, so the measurement is exact — and bails to
/// the unchanged input when the saving is too small, before any store
/// write. Phase B performs the writes and renders markers from the store's
/// returned keys, so a marker can never disagree with the row backing it.
fn assemble(
    text: &str,
    segments: &[Segment<'_>],
    stash: Option<&dyn StashStore>,
) -> BuildLogOutcome {
    let mut candidate = String::with_capacity(text.len());
    for segment in segments {
        match segment {
            Segment::Verbatim(s) => candidate.push_str(s),
            Segment::DupNote(repeats) => candidate.push_str(&render_dup_note(*repeats)),
            Segment::Gap(plan) => {
                candidate.push_str(&render_gap(plan, &compute_key(plan.payload.as_bytes())));
            }
        }
    }
    if text
        .chars()
        .count()
        .saturating_sub(candidate.chars().count())
        < MIN_SAVED_CHARS
    {
        return unchanged(text);
    }

    let Some(store) = stash else {
        let omitted_blocks = segments
            .iter()
            .filter(|s| matches!(s, Segment::Gap(_)))
            .count();
        return BuildLogOutcome {
            output: candidate,
            stash_writes: Vec::new(),
            omitted_blocks,
            stash_errors: 0,
            retrievable: omitted_blocks == 0,
        };
    };
    let mut output = String::with_capacity(candidate.len());
    let mut stash_writes = Vec::new();
    let mut omitted_blocks = 0;
    let mut stash_errors = 0;
    for segment in segments {
        match segment {
            Segment::Verbatim(s) => output.push_str(s),
            Segment::DupNote(repeats) => output.push_str(&render_dup_note(*repeats)),
            Segment::Gap(plan) => match store.stash(&plan.payload) {
                Ok(write) => {
                    output.push_str(&render_gap(plan, &write.key));
                    stash_writes.push(write);
                    omitted_blocks += 1;
                }
                Err(_) => {
                    output.push_str(&plan.payload);
                    stash_errors += 1;
                }
            },
        }
    }
    BuildLogOutcome {
        output,
        stash_writes,
        omitted_blocks,
        stash_errors,
        retrievable: true,
    }
}

/// `repeats` is the number of *additional* copies, so a line rendered once
/// followed by this note stands for `repeats + 1` occurrences. "more" keeps
/// that reading unambiguous.
fn render_dup_note(repeats: usize) -> String {
    format!("[tokenless: previous line repeated {repeats} more times]\n")
}

fn render_gap(plan: &GapPlan, key: &str) -> String {
    const BUCKETS: [&str; 5] = ["info", "debug", "trace", "notice", "other"];
    let parts: Vec<String> = plan
        .histogram
        .iter()
        .zip(BUCKETS)
        .filter(|(count, _)| **count > 0)
        .map(|(count, name)| format!("{count} {name}"))
        .collect();
    let mut out = format!(
        "… (omitted {} lines: {}; run: tokenless retrieve '{}')\n",
        plan.omitted_lines,
        parts.join(", "),
        marker_for(key),
    );
    for (count, mask) in &plan.templates {
        out.push_str(&format!("  {count}× {mask}\n"));
    }
    out
}

fn generic_mode(text: &str, stash: Option<&dyn StashStore>) -> BuildLogOutcome {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    if lines.len() >= GENERIC_MIN_LINES {
        let tail_start = lines.len() - GENERIC_TAIL_LINES;
        let omitted = tail_start - GENERIC_HEAD_LINES;
        return single_gap(
            text,
            lines[..GENERIC_HEAD_LINES].concat(),
            lines[GENERIC_HEAD_LINES..tail_start].concat(),
            lines[tail_start..].concat(),
            |key| {
                format!(
                    "… (omitted {omitted} lines, run: tokenless retrieve '{}')\n",
                    marker_for(key)
                )
            },
            stash,
        );
    }
    let total_chars = text.chars().count();
    if total_chars >= GENERIC_CHAR_ENGAGE {
        let head_end = byte_of_char(text, GENERIC_HEAD_CHARS);
        let tail_start = byte_of_char(text, total_chars - GENERIC_TAIL_CHARS);
        let omitted = total_chars - GENERIC_HEAD_CHARS - GENERIC_TAIL_CHARS;
        return single_gap(
            text,
            text[..head_end].to_string(),
            text[head_end..tail_start].to_string(),
            text[tail_start..].to_string(),
            |key| {
                format!(
                    "\n… (omitted {omitted} chars, run: tokenless retrieve '{}')\n",
                    marker_for(key)
                )
            },
            stash,
        );
    }
    unchanged(text)
}

/// One head/gap/tail split with a single stash entry. A failed write falls
/// open to the unchanged input — there is nothing else to salvage here.
fn single_gap(
    text: &str,
    head: String,
    payload: String,
    tail: String,
    marker: impl Fn(&str) -> String,
    stash: Option<&dyn StashStore>,
) -> BuildLogOutcome {
    let derived = compute_key(payload.as_bytes());
    let candidate_chars =
        head.chars().count() + marker(&derived).chars().count() + tail.chars().count();
    if text.chars().count().saturating_sub(candidate_chars) < MIN_SAVED_CHARS {
        return unchanged(text);
    }
    match stash {
        None => BuildLogOutcome {
            output: format!("{head}{}{tail}", marker(&derived)),
            stash_writes: Vec::new(),
            omitted_blocks: 1,
            stash_errors: 0,
            retrievable: false,
        },
        Some(store) => match store.stash(&payload) {
            Ok(write) => {
                let output = format!("{head}{}{tail}", marker(&write.key));
                BuildLogOutcome {
                    output,
                    stash_writes: vec![write],
                    omitted_blocks: 1,
                    stash_errors: 0,
                    retrievable: true,
                }
            }
            Err(_) => {
                let mut outcome = unchanged(text);
                outcome.stash_errors = 1;
                outcome
            }
        },
    }
}

fn byte_of_char(text: &str, char_idx: usize) -> usize {
    text.char_indices()
        .nth(char_idx)
        .map(|(byte, _)| byte)
        .unwrap_or(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    include!("tests/build_log_tests.rs");
}
