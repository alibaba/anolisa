use tokenless_ccr::InMemoryStore;

struct FailingStore;

impl StashStore for FailingStore {
    fn stash(&self, _payload: &str) -> Result<StashWrite, tokenless_ccr::StashError> {
        Err(tokenless_ccr::StashError::Backend("store down".into()))
    }
    fn retrieve(&self, _hash: &str) -> Result<Option<String>, tokenless_ccr::StashError> {
        Ok(None)
    }
    fn len(&self) -> usize {
        0
    }
    fn evict_expired(&self) -> Result<usize, tokenless_ccr::StashError> {
        Ok(0)
    }
    fn delete(&self, _hash: &str, _generation: u64) -> Result<bool, tokenless_ccr::StashError> {
        Ok(false)
    }
}

/// Replace the whole marker line carrying `key` with `payload` — the
/// reassembly rule for a gap without template summary lines.
fn replace_marker_line(output: &str, key: &str, payload: &str) -> String {
    let marker = marker_for(key);
    let start = output.find(&marker).expect("marker present");
    let line_start = output[..start].rfind('\n').map(|p| p + 1).unwrap_or(0);
    let line_end = output[start..].find('\n').map(|p| start + p + 1).unwrap_or(output.len());
    format!("{}{}{}", &output[..line_start], payload, &output[line_end..])
}

fn cargo_like_log() -> String {
    let mut lines = Vec::new();
    for i in 0..5 {
        lines.push(format!("$ cargo build --release step {i}"));
    }
    for i in 0..60 {
        lines.push(format!("   Compiling pkg{i:03} v0.1.{i}"));
    }
    lines.push("error[E0308]: mismatched types in src/main.rs".to_string());
    for i in 60..120 {
        lines.push(format!("   Compiling pkg{i:03} v0.1.{i}"));
    }
    for i in 0..10 {
        lines.push(format!("summary tail line {i} of the build output"));
    }
    lines.join("\n") + "\n"
}

#[test]
fn build_log_mode_stashes_gaps_and_keeps_signal() {
    let text = cargo_like_log();
    let store = InMemoryStore::new();
    let outcome = compress_log(&text, BuildLogMode::BuildLog, Some(&store));

    assert_eq!(outcome.omitted_blocks, 2);
    assert_eq!(outcome.stash_writes.len(), 2);
    assert_eq!(outcome.stash_errors, 0);
    assert!(outcome.retrievable);
    assert!(outcome.output.contains("error[E0308]: mismatched types in src/main.rs"));
    assert!(outcome.output.starts_with("$ cargo build --release step 0"));
    assert!(outcome.output.contains("summary tail line 9"));
    assert!(outcome.output.chars().count() < text.chars().count());

    // Byte-exact reassembly: each marker line swaps back to its payload.
    let mut reassembled = outcome.output.clone();
    for write in &outcome.stash_writes {
        let payload = store.retrieve(&write.key).unwrap().expect("stashed payload");
        reassembled = replace_marker_line(&reassembled, &write.key, &payload);
    }
    assert_eq!(reassembled, text);
}

#[test]
fn dry_run_without_store_renders_identical_markers() {
    let text = cargo_like_log();
    let store = InMemoryStore::new();
    let active = compress_log(&text, BuildLogMode::BuildLog, Some(&store));
    let dry = compress_log(&text, BuildLogMode::BuildLog, None);

    // Keys are content-derived, so measurement output equals active output.
    assert_eq!(dry.output, active.output);
    assert!(dry.stash_writes.is_empty());
    assert_eq!(dry.omitted_blocks, 2);
    assert!(!dry.retrievable);
}

#[test]
fn short_input_is_unchanged() {
    let text = "error: boom\nsecond line\nthird line\n".repeat(3);
    let outcome = compress_log(&text, BuildLogMode::BuildLog, None);
    assert_eq!(outcome.output, text);
    assert_eq!(outcome.omitted_blocks, 0);
    assert!(outcome.retrievable);
}

/// A run whose only failure signal is a crash line buried in routine
/// progress: the line names no error, so an error-keyword classifier stashes
/// it and the log reads as a clean run.
#[test]
fn keywordless_crash_survives_a_bulky_log() {
    for crash in [
        "Segmentation fault (core dumped)",
        "Killed",
        "./stage2.sh: line 8: linker: command not found",
    ] {
        let mut lines: Vec<String> =
            (0..35).map(|i| format!("   Compiling widget{i:02} v0.{i}.2")).collect();
        lines.push(crash.to_string());
        lines.extend((0..35).map(|i| format!("   Compiling gadget{i:02} v0.{i}.9")));
        let text = lines.join("\n") + "\n";

        let outcome = compress_log(&text, BuildLogMode::BuildLog, Some(&InMemoryStore::new()));
        assert!(outcome.output.chars().count() < text.chars().count(), "crash: {crash}");
        assert!(outcome.output.contains(crash), "crash line was stashed: {crash}");
    }
}

#[test]
fn all_signal_input_is_unchanged() {
    let text: String =
        (0..31).map(|i| format!("error: distinct failure number {i}\n")).collect();
    let outcome = compress_log(&text, BuildLogMode::BuildLog, None);
    assert_eq!(outcome.output, text);
    assert_eq!(outcome.omitted_blocks, 0);
}

fn dup_run_log() -> String {
    let mut lines = Vec::new();
    for i in 0..12 {
        lines.push(format!("step {i} preparing sources and copying items over"));
    }
    for _ in 0..5 {
        lines.push("error: flaky connection reset by peer".to_string());
    }
    for i in 0..20 {
        lines.push(format!("cleanup item {i} removed from the workspace temp dir"));
    }
    lines.join("\n") + "\n"
}

#[test]
fn duplicate_signal_run_collapses_to_count() {
    let text = dup_run_log();
    let store = InMemoryStore::new();
    let outcome = compress_log(&text, BuildLogMode::BuildLog, Some(&store));

    assert_eq!(outcome.output.matches("error: flaky connection reset by peer").count(), 1);
    let note = "[tokenless: previous line repeated 4 more times]\n";
    assert!(outcome.output.contains(note));

    // Reassembly: expand the annotation by count, then swap markers back.
    let out = &outcome.output;
    let pos = out.find(note).unwrap();
    let prev_start = out[..pos - 1].rfind('\n').map(|p| p + 1).unwrap_or(0);
    let prev_line = &out[prev_start..pos];
    let mut reassembled =
        format!("{}{}{}", &out[..pos], prev_line.repeat(4), &out[pos + note.len()..]);
    for write in &outcome.stash_writes {
        let payload = store.retrieve(&write.key).unwrap().unwrap();
        reassembled = replace_marker_line(&reassembled, &write.key, &payload);
    }
    assert_eq!(reassembled, text);
}

#[test]
fn short_and_near_duplicate_signal_runs_stay_verbatim() {
    let mut lines: Vec<String> =
        (0..15).map(|i| format!("step {i} preparing sources for the build")).collect();
    lines.push("error: boom happened once".to_string());
    lines.push("error: boom happened once".to_string());
    for i in 0..8 {
        lines.push(format!("error: distinct boom number {i}"));
    }
    for i in 0..15 {
        lines.push(format!("cleanup item {i} removed from workspace"));
    }
    let text = lines.join("\n") + "\n";
    let outcome = compress_log(&text, BuildLogMode::BuildLog, None);

    assert!(!outcome.output.contains("[tokenless: previous line repeated"));
    assert_eq!(outcome.output.matches("error: boom happened once").count(), 2);
    assert_eq!(outcome.output.matches("error: distinct boom number").count(), 8);
}

#[test]
fn trace_region_spanning_a_gap_stays_verbatim() {
    let mut lines: Vec<String> =
        (0..40).map(|i| format!("   Compiling pkg{i:03} v0.2.{i}")).collect();
    let trace = [
        "Traceback (most recent call last):",
        "  File \"/app/a.py\", line 9, in outer",
        "    middle()",
        "  File \"/app/b.py\", line 7, in middle",
        "    inner()",
        "  File \"/app/c.py\", line 5, in inner",
        "    deepest()",
        "  File \"/app/d.py\", line 3, in deepest",
        "    boom()",
        "KeyError: 'boom'",
    ];
    lines.extend(trace.iter().map(|s| s.to_string()));
    lines.extend((0..40).map(|i| format!("   Compiling pkg{:03} v0.3.{i}", i + 40)));
    let text = lines.join("\n") + "\n";
    let outcome = compress_log(&text, BuildLogMode::BuildLog, None);

    // The middle frames sit outside every signal context window — only the
    // atomic trace region keeps them out of the omission gaps.
    for frame in trace {
        assert!(outcome.output.contains(frame), "missing trace line: {frame}");
    }
    assert!(outcome.omitted_blocks >= 2);
}

#[test]
fn failed_stash_keeps_gaps_verbatim() {
    let text = cargo_like_log();
    let outcome = compress_log(&text, BuildLogMode::BuildLog, Some(&FailingStore));

    assert_eq!(outcome.output, text);
    assert_eq!(outcome.stash_errors, 2);
    assert_eq!(outcome.omitted_blocks, 0);
    assert!(outcome.stash_writes.is_empty());
}

#[test]
fn generic_line_mode_keeps_head_and_tail() {
    let text: String =
        (0..120).map(|i| format!("record {i} holding some ordinary content\n")).collect();
    let store = InMemoryStore::new();
    let outcome = compress_log(&text, BuildLogMode::GenericText, Some(&store));

    assert_eq!(outcome.omitted_blocks, 1);
    assert_eq!(outcome.stash_writes.len(), 1);
    assert!(outcome.output.starts_with("record 0 "));
    assert!(outcome.output.contains("record 119 "));
    assert!(outcome.output.contains("… (omitted 40 lines, run: tokenless retrieve"));

    let write = &outcome.stash_writes[0];
    let payload = store.retrieve(&write.key).unwrap().unwrap();
    assert_eq!(replace_marker_line(&outcome.output, &write.key, &payload), text);
}

#[test]
fn generic_mode_below_thresholds_is_unchanged() {
    let text: String =
        (0..50).map(|i| format!("record {i} holding some ordinary content\n")).collect();
    let outcome = compress_log(&text, BuildLogMode::GenericText, None);
    assert_eq!(outcome.output, text);
    assert_eq!(outcome.omitted_blocks, 0);
}

#[test]
fn generic_char_mode_handles_a_giant_single_line() {
    let text = "x".repeat(70_000);
    let store = InMemoryStore::new();
    let outcome = compress_log(&text, BuildLogMode::GenericText, Some(&store));

    assert_eq!(outcome.omitted_blocks, 1);
    let write = &outcome.stash_writes[0];
    let payload = store.retrieve(&write.key).unwrap().unwrap();
    assert_eq!(payload.chars().count(), 70_000 - 2 * 16_384);
    let block = format!(
        "\n… (omitted {} chars, run: tokenless retrieve '{}')\n",
        70_000 - 2 * 16_384,
        marker_for(&write.key)
    );
    assert!(outcome.output.contains(&block));
    assert_eq!(outcome.output.replace(&block, &payload), text);
}

#[test]
fn build_log_mode_needs_line_volume_regardless_of_chars() {
    let text = format!("{}\n{}\n", "a".repeat(40_000), "b".repeat(40_000));
    let outcome = compress_log(&text, BuildLogMode::BuildLog, None);
    assert_eq!(outcome.output, text);
}
