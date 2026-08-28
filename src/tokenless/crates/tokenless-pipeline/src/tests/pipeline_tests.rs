// Arbitration contract tests (§4.3): the escalation ladder, the single
// end-to-end comparison, every explicit rejection disposition, and the
// rollback of stash writes whose markers never reach the model. Compressors
// here are test doubles — real ones migrate behind the interface starting
// with the response cleanup.

use std::sync::atomic::{AtomicUsize, Ordering};

use tokenless_ccr::InMemoryStore;
use tokenless_protocol::{Capabilities, ContentOrigin, Seam};

use crate::registry::CostClass;

const REPLACE_ONLY: Capabilities = Capabilities {
    replace_output: true,
    publish_retrieve_tool: false,
    replace_with_text: false,
};
const FULL: Capabilities = Capabilities {
    replace_output: true,
    publish_retrieve_tool: true,
    replace_with_text: false,
};

const fn test_spec(id: &'static str, stage: Stage, capabilities: Capabilities) -> CompressorSpec {
    CompressorSpec {
        id,
        content_types: &[ContentType::PlainText],
        seams: &[Seam::PostTool],
        required_capabilities: capabilities,
        stage,
        cost_class: CostClass::Cheap,
    }
}

fn request(content: &str, capabilities: Capabilities) -> CompressionRequest {
    let mut request = CompressionRequest::new(content, "test-agent", Seam::PostTool);
    request.capabilities = capabilities;
    request
}

fn config() -> PipelineConfig {
    PipelineConfig {
        timeout: Duration::from_secs(5),
        max_tokens: None,
        require_reversibility: false,
        dry_run: false,
    }
}

const SPACEY: &str = "alpha   beta   gamma   delta   epsilon   zeta";
const MULTILINE: &str = "keep this head line\n\
                        the rest of this content is a long tail\n\
                        that the stasher moves into the stash\n\
                        line after line of plain prose\n\
                        with enough weight to make stashing save tokens";
const SPACEY_MULTILINE: &str = "keep   this   head\n\
                               and   stash   this   long   tail   of   prose\n\
                               spread   over   several   lines   of   filler";

/// Lossless: collapses runs of spaces.
struct SpaceSquisher;

static SQUISHER_SPEC: CompressorSpec = test_spec("space-squisher", Stage::Lossless, REPLACE_ONLY);

impl Compressor for SpaceSquisher {
    fn spec(&self) -> &CompressorSpec {
        &SQUISHER_SPEC
    }

    fn compress(
        &self,
        content: &str,
        _stash: Option<&dyn StashStore>,
    ) -> Result<CompressOutcome, CompressError> {
        let mut output = String::with_capacity(content.len());
        let mut previous_space = false;
        for ch in content.chars() {
            if ch == ' ' && previous_space {
                continue;
            }
            previous_space = ch == ' ';
            output.push(ch);
        }
        Ok(CompressOutcome {
            output,
            reversibility: Reversibility::Lossless,
            stash_writes: Vec::new(),
        })
    }
}

/// Lossless that grows its input: must be rejected by arbitration. Counts
/// its rollback notifications so the stage gate's contract is observable.
#[derive(Default)]
struct Bloater {
    discarded: AtomicUsize,
}

static BLOATER_SPEC: CompressorSpec = test_spec("bloater", Stage::Lossless, REPLACE_ONLY);

impl Compressor for Bloater {
    fn spec(&self) -> &CompressorSpec {
        &BLOATER_SPEC
    }

    fn compress(
        &self,
        content: &str,
        _stash: Option<&dyn StashStore>,
    ) -> Result<CompressOutcome, CompressError> {
        Ok(CompressOutcome {
            output: format!("{content}{content}"),
            reversibility: Reversibility::Lossless,
            stash_writes: Vec::new(),
        })
    }

    fn discarded(&self) {
        self.discarded.fetch_add(1, Ordering::Relaxed);
    }
}

/// Retrievable-lossy: keeps the first line, stashes the rest behind a marker.
struct TailStasher;

static TAIL_STASHER_SPEC: CompressorSpec =
    test_spec("tail-stasher", Stage::RetrievableLossy, FULL);

impl Compressor for TailStasher {
    fn spec(&self) -> &CompressorSpec {
        &TAIL_STASHER_SPEC
    }

    fn compress(
        &self,
        content: &str,
        stash: Option<&dyn StashStore>,
    ) -> Result<CompressOutcome, CompressError> {
        let stash = stash.ok_or_else(|| CompressError::Failed("stash unavailable".to_owned()))?;
        let (head, tail) = content
            .split_once('\n')
            .ok_or_else(|| CompressError::Failed("nothing to stash".to_owned()))?;
        let write = stash.stash(tail)?;
        let output = format!("{head}\n{}", marker_for(&write.key));
        Ok(CompressOutcome {
            output,
            reversibility: Reversibility::Retrievable,
            stash_writes: vec![write],
        })
    }
}

/// Lossy double that stashes the tail twice: the second write refreshes the
/// first, exercising the unbroken ownership chain on rollback.
struct DoubleStasher;

static DOUBLE_STASHER_SPEC: CompressorSpec =
    test_spec("double-stasher", Stage::RetrievableLossy, FULL);

impl Compressor for DoubleStasher {
    fn spec(&self) -> &CompressorSpec {
        &DOUBLE_STASHER_SPEC
    }

    fn compress(
        &self,
        content: &str,
        stash: Option<&dyn StashStore>,
    ) -> Result<CompressOutcome, CompressError> {
        let stash = stash.ok_or_else(|| CompressError::Failed("stash unavailable".to_owned()))?;
        let (head, tail) = content
            .split_once('\n')
            .ok_or_else(|| CompressError::Failed("nothing to stash".to_owned()))?;
        let first = stash.stash(tail)?;
        let second = stash.stash(tail)?;
        let output = format!("{head}\n{}", marker_for(&second.key));
        Ok(CompressOutcome {
            output,
            reversibility: Reversibility::Retrievable,
            stash_writes: vec![first, second],
        })
    }
}

/// Lossy double with a foreign refresh interleaved between its two reported
/// writes, breaking the ownership chain.
struct ForeignRefreshStasher;

static FOREIGN_REFRESH_SPEC: CompressorSpec =
    test_spec("foreign-refresh", Stage::RetrievableLossy, FULL);

impl Compressor for ForeignRefreshStasher {
    fn spec(&self) -> &CompressorSpec {
        &FOREIGN_REFRESH_SPEC
    }

    fn compress(
        &self,
        content: &str,
        stash: Option<&dyn StashStore>,
    ) -> Result<CompressOutcome, CompressError> {
        let stash = stash.ok_or_else(|| CompressError::Failed("stash unavailable".to_owned()))?;
        let (head, tail) = content
            .split_once('\n')
            .ok_or_else(|| CompressError::Failed("nothing to stash".to_owned()))?;
        let first = stash.stash(tail)?;
        // Another writer refreshes the row between this run's two writes.
        stash.stash(tail)?;
        let third = stash.stash(tail)?;
        let output = format!("{head}\n{}", marker_for(&third.key));
        Ok(CompressOutcome {
            output,
            reversibility: Reversibility::Retrievable,
            stash_writes: vec![first, third],
        })
    }
}

/// Lossy without a recovery path: drops the second half of the content.
struct HalfDropper;

static HALF_DROPPER_SPEC: CompressorSpec =
    test_spec("half-dropper", Stage::RetrievableLossy, REPLACE_ONLY);

impl Compressor for HalfDropper {
    fn spec(&self) -> &CompressorSpec {
        &HALF_DROPPER_SPEC
    }

    fn compress(
        &self,
        content: &str,
        _stash: Option<&dyn StashStore>,
    ) -> Result<CompressOutcome, CompressError> {
        let mut end = content.len() / 2;
        while !content.is_char_boundary(end) {
            end -= 1;
        }
        Ok(CompressOutcome {
            output: content[..end].to_owned(),
            reversibility: Reversibility::Unrecoverable,
            stash_writes: Vec::new(),
        })
    }
}

/// Lossy double that must never be reached behind another lossy candidate.
struct CountingLossy;

static COUNTING_LOSSY_SPEC: CompressorSpec =
    test_spec("counting-lossy", Stage::RetrievableLossy, FULL);
static SECOND_LOSSY_CALLS: AtomicUsize = AtomicUsize::new(0);

impl Compressor for CountingLossy {
    fn spec(&self) -> &CompressorSpec {
        &COUNTING_LOSSY_SPEC
    }

    fn compress(
        &self,
        content: &str,
        _stash: Option<&dyn StashStore>,
    ) -> Result<CompressOutcome, CompressError> {
        SECOND_LOSSY_CALLS.fetch_add(1, Ordering::SeqCst);
        Ok(CompressOutcome {
            output: content.to_owned(),
            reversibility: Reversibility::Unrecoverable,
            stash_writes: Vec::new(),
        })
    }
}

/// Truncation: keeps the first 8 bytes.
struct HeadTrunc;

static HEAD_TRUNC_SPEC: CompressorSpec = test_spec("head-trunc", Stage::Truncation, REPLACE_ONLY);

impl Compressor for HeadTrunc {
    fn spec(&self) -> &CompressorSpec {
        &HEAD_TRUNC_SPEC
    }

    fn compress(
        &self,
        content: &str,
        _stash: Option<&dyn StashStore>,
    ) -> Result<CompressOutcome, CompressError> {
        let mut end = content.len().min(8);
        while !content.is_char_boundary(end) {
            end -= 1;
        }
        Ok(CompressOutcome {
            output: content[..end].to_owned(),
            reversibility: Reversibility::Unrecoverable,
            stash_writes: Vec::new(),
        })
    }
}

/// Optional step that always fails, with an oversized message.
struct Failing;

static FAILING_SPEC: CompressorSpec = test_spec("failing", Stage::Lossless, REPLACE_ONLY);

impl Compressor for Failing {
    fn spec(&self) -> &CompressorSpec {
        &FAILING_SPEC
    }

    fn compress(
        &self,
        _content: &str,
        _stash: Option<&dyn StashStore>,
    ) -> Result<CompressOutcome, CompressError> {
        Err(CompressError::Failed("b".repeat(2 * DIAGNOSTIC_MAX_BYTES)))
    }
}

/// Lossless that would save tokens but overruns any small budget.
struct Slow;

static SLOW_SPEC: CompressorSpec = test_spec("slow-squisher", Stage::Lossless, REPLACE_ONLY);

impl Compressor for Slow {
    fn spec(&self) -> &CompressorSpec {
        &SLOW_SPEC
    }

    fn compress(
        &self,
        content: &str,
        _stash: Option<&dyn StashStore>,
    ) -> Result<CompressOutcome, CompressError> {
        std::thread::sleep(Duration::from_millis(30));
        Ok(CompressOutcome {
            output: content.replace(' ', ""),
            reversibility: Reversibility::Lossless,
            stash_writes: Vec::new(),
        })
    }
}

#[test]
fn lossless_savings_are_applied() {
    let request = request(SPACEY, REPLACE_ONLY);
    let response = run(&request, &[&SpaceSquisher], None, &config());
    assert_eq!(response.disposition, Disposition::Applied);
    assert_eq!(response.output, "alpha beta gamma delta epsilon zeta");
    assert_eq!(response.compressor_chain, ["space-squisher"]);
    assert!(response.after_tokens < response.before_tokens);
    assert_eq!(response.reversibility, Reversibility::Lossless);
    assert_eq!(response.content_type.as_deref(), Some("plain_text"));
    assert_eq!(response.tokenizer_id, TOKENIZER_ID);
    assert!(response.stash_keys.is_empty());
}

#[test]
fn no_candidates_is_passthrough() {
    let request = request(SPACEY, REPLACE_ONLY);
    let response = run(&request, &[], None, &config());
    assert_eq!(response.disposition, Disposition::Passthrough);
    assert_eq!(response.output, SPACEY);
    assert_eq!(response.after_tokens, response.before_tokens);
    assert_eq!(response.content_type.as_deref(), Some("plain_text"));
}

#[test]
fn undeclared_capabilities_filter_out_candidates() {
    let request = request(SPACEY, Capabilities::default());
    let response = run(&request, &[&SpaceSquisher], None, &config());
    assert_eq!(response.disposition, Disposition::Passthrough);
    assert_eq!(response.output, SPACEY);
}

#[test]
fn file_content_protects_types_off_the_release_list() {
    // `SPACEY_MULTILINE` is prose — `PlainText`, which the list protects.
    // The gate runs before routing, so not even a lossless compressor sees
    // it: protected means full passthrough, and a rewritten copy of a file
    // breaks the model's next exact-match edit just as a lossy one would.
    let mut request = request(SPACEY_MULTILINE, FULL);
    request.content_origin = ContentOrigin::FileContent;
    let store = InMemoryStore::new();
    let mut config = config();
    config.max_tokens = Some(10);
    let response = run(
        &request,
        &[&SpaceSquisher, &TailStasher],
        Some(&store),
        &config,
    );
    assert_eq!(response.disposition, Disposition::Passthrough);
    assert_eq!(response.output, SPACEY_MULTILINE);
    assert_eq!(response.after_tokens, response.before_tokens);
    assert!(response.compressor_chain.is_empty());
    assert_eq!(store.len(), 0);
    // The detected type is still reported: a protected passthrough has to be
    // distinguishable from a no-savings one when tuning the list (§4.7).
    assert_eq!(response.content_type.as_deref(), Some("plain_text"));

    // Same content, same compressors, command output: the gate is about
    // where the content came from, not what it is.
    request.content_origin = ContentOrigin::CommandOutput;
    let response = run(
        &request,
        &[&SpaceSquisher, &TailStasher],
        Some(&store),
        &config,
    );
    assert_eq!(response.disposition, Disposition::Applied);
}

#[test]
fn an_undeclared_origin_never_reaches_the_release_gate() {
    // The migration default: protected content still compresses, because no
    // caller has claimed it came from a file.
    let request = request(SPACEY_MULTILINE, FULL);
    assert_eq!(request.content_origin, ContentOrigin::Unspecified);
    let mut config = config();
    config.max_tokens = Some(10);
    let response = run(&request, &[&SpaceSquisher], None, &config);
    assert_eq!(response.disposition, Disposition::Applied);
}

#[test]
fn growth_is_rejected_as_no_savings() {
    let request = request(SPACEY, REPLACE_ONLY);
    let bloater = Bloater::default();
    let response = run(&request, &[&bloater], None, &config());
    assert_eq!(response.disposition, Disposition::NoSavings);
    assert_eq!(response.output, SPACEY);
    assert_eq!(response.after_tokens, response.before_tokens);
    assert!(response.compressor_chain.is_empty());
    // The stage gate dropped the candidate, so the compressor is told: a
    // caller that reports what shaped the result must not name it. The
    // final rejections carry no such notification — `text_dry_run_measures_
    // without_a_store_and_keeps_the_chain` in the Runtime pins that side.
    assert_eq!(bloater.discarded.load(Ordering::Relaxed), 1);
}

#[test]
fn size_policy_gates_escalation() {
    let store = InMemoryStore::new();
    let request = request(MULTILINE, FULL);
    // No size policy: the lossy stage never runs, nothing is stashed.
    let response = run(&request, &[&TailStasher], Some(&store), &config());
    assert_eq!(response.disposition, Disposition::Passthrough);
    assert_eq!(store.len(), 0);
}

#[test]
fn lossy_stage_stashes_and_applies() {
    let store = InMemoryStore::new();
    let request = request(MULTILINE, FULL);
    let mut config = config();
    config.max_tokens = Some(10);
    let response = run(&request, &[&TailStasher], Some(&store), &config);
    assert_eq!(response.disposition, Disposition::Applied);
    assert_eq!(response.reversibility, Reversibility::Retrievable);
    assert_eq!(response.compressor_chain, ["tail-stasher"]);
    assert_eq!(response.stash_keys.len(), 1);
    assert!(response.output.starts_with("keep this head line\n<<tokenless:"));
    let tail = &MULTILINE[MULTILINE.find('\n').unwrap() + 1..];
    let stashed = store.retrieve(&response.stash_keys[0]).unwrap();
    assert_eq!(stashed.as_deref(), Some(tail));
}

#[test]
fn rejected_candidates_roll_back_stash_writes() {
    let store = InMemoryStore::new();
    // The marker is longer than the stashed tail: a produced-but-worse
    // candidate must disappear completely, stash entry included.
    let request = request("ab\ncd", FULL);
    let mut config = config();
    config.max_tokens = Some(1);
    let response = run(&request, &[&TailStasher], Some(&store), &config);
    assert_eq!(response.disposition, Disposition::NoSavings);
    assert_eq!(response.output, "ab\ncd");
    assert!(response.stash_keys.is_empty());
    assert_eq!(store.len(), 0);
}

#[test]
fn rollback_preserves_rows_that_predate_the_run() {
    let store = InMemoryStore::new();
    let preexisting = store.stash("cd").expect("seed row");
    let request = request("ab\ncd", FULL);
    let mut config = config();
    config.max_tokens = Some(1);
    let response = run(&request, &[&TailStasher], Some(&store), &config);
    assert_eq!(response.disposition, Disposition::NoSavings);
    // The run only refreshed a row it did not create: it must survive, or
    // markers emitted by earlier requests would go dead.
    assert_eq!(store.len(), 1);
    let stashed = store.retrieve(&preexisting.key).expect("retrieve");
    assert_eq!(stashed.as_deref(), Some("cd"));
}

#[test]
fn an_unbroken_refresh_chain_is_rolled_back_wholly() {
    let store = InMemoryStore::new();
    let request = request("ab\ncd", FULL);
    let mut config = config();
    config.max_tokens = Some(1);
    let response = run(&request, &[&DoubleStasher], Some(&store), &config);
    assert_eq!(response.disposition, Disposition::NoSavings);
    // Both writes belong to this run's chain; rollback re-adopted the
    // second generation, so the row created here is gone.
    assert_eq!(store.len(), 0);
}

#[test]
fn a_foreign_refresh_keeps_the_row_alive() {
    let store = InMemoryStore::new();
    let request = request("ab\ncd", FULL);
    let mut config = config();
    config.max_tokens = Some(1);
    let response = run(&request, &[&ForeignRefreshStasher], Some(&store), &config);
    assert_eq!(response.disposition, Disposition::NoSavings);
    // The interleaved foreign refresh broke the ownership chain: the key
    // leaves the rollback list and the row stays live.
    assert_eq!(store.len(), 1);
}

#[test]
fn truncation_that_cuts_a_marker_rolls_back_its_stash_write() {
    let store = InMemoryStore::new();
    let request = request(MULTILINE, FULL);
    let mut config = config();
    config.max_tokens = Some(2);
    let compressors: [&dyn Compressor; 2] = [&TailStasher, &HeadTrunc];
    let response = run(&request, &compressors, Some(&store), &config);
    assert_eq!(response.disposition, Disposition::Applied);
    assert_eq!(response.compressor_chain, ["tail-stasher", "head-trunc"]);
    assert_eq!(response.reversibility, Reversibility::Unrecoverable);
    assert_eq!(response.output, "keep thi");
    // The truncated output no longer references the stash write: the key is
    // not committed and the row is rolled back instead of leaking.
    assert!(response.stash_keys.is_empty());
    assert_eq!(store.len(), 0);
}

#[test]
fn required_reversibility_rejects_unrecoverable_candidates() {
    let request = request(MULTILINE, REPLACE_ONLY);
    let mut config = config();
    config.max_tokens = Some(10);
    config.require_reversibility = true;
    let response = run(&request, &[&HalfDropper], None, &config);
    assert_eq!(response.disposition, Disposition::ReversibilityUnavailable);
    assert_eq!(response.output, MULTILINE);
    assert!(response.compressor_chain.is_empty());

    config.require_reversibility = false;
    let response = run(&request, &[&HalfDropper], None, &config);
    assert_eq!(response.disposition, Disposition::Applied);
    assert_eq!(response.reversibility, Reversibility::Unrecoverable);
}

#[test]
fn only_the_first_lossy_candidate_runs() {
    let store = InMemoryStore::new();
    let request = request(MULTILINE, FULL);
    let mut config = config();
    config.max_tokens = Some(10);
    let compressors: [&dyn Compressor; 2] = [&TailStasher, &CountingLossy];
    let response = run(&request, &compressors, Some(&store), &config);
    assert_eq!(response.disposition, Disposition::Applied);
    assert_eq!(response.compressor_chain, ["tail-stasher"]);
    assert_eq!(SECOND_LOSSY_CALLS.load(Ordering::SeqCst), 0);
}

#[test]
fn truncation_is_the_last_resort() {
    let request = request(MULTILINE, REPLACE_ONLY);
    let mut config = config();
    config.max_tokens = Some(4);
    let response = run(&request, &[&HeadTrunc], None, &config);
    assert_eq!(response.disposition, Disposition::Applied);
    assert_eq!(response.compressor_chain, ["head-trunc"]);
    assert_eq!(response.reversibility, Reversibility::Unrecoverable);
    assert_eq!(response.output, &MULTILINE[..8]);
}

#[test]
fn the_ladder_composes_lossless_then_lossy() {
    let store = InMemoryStore::new();
    let request = request(SPACEY_MULTILINE, FULL);
    let mut config = config();
    config.max_tokens = Some(10);
    let compressors: [&dyn Compressor; 2] = [&SpaceSquisher, &TailStasher];
    let response = run(&request, &compressors, Some(&store), &config);
    assert_eq!(response.disposition, Disposition::Applied);
    assert_eq!(response.compressor_chain, ["space-squisher", "tail-stasher"]);
    assert_eq!(response.reversibility, Reversibility::Retrievable);
    assert!(response.output.starts_with("keep this head\n<<tokenless:"));
}

#[test]
fn dry_run_measures_without_emitting() {
    let store = InMemoryStore::new();
    let request = request(MULTILINE, FULL);
    let mut config = config();
    config.max_tokens = Some(10);
    config.dry_run = true;
    let response = run(&request, &[&TailStasher], Some(&store), &config);
    assert_eq!(response.disposition, Disposition::DryRun);
    assert_eq!(response.output, MULTILINE);
    assert!(response.after_tokens < response.before_tokens);
    assert!(response.compressor_chain.is_empty());
    assert!(response.stash_keys.is_empty());
    assert_eq!(store.len(), 0);
}

#[test]
fn failures_are_fail_open_with_a_bounded_diagnostic() {
    let request = request(SPACEY, REPLACE_ONLY);
    let response = run(&request, &[&Failing], None, &config());
    assert_eq!(response.disposition, Disposition::Error);
    assert_eq!(response.output, SPACEY);
    let diagnostic = response.diagnostic.expect("Error carries a diagnostic");
    assert!(diagnostic.len() <= DIAGNOSTIC_MAX_BYTES);
    assert!(diagnostic.starts_with("failing: "));
    assert!(diagnostic.ends_with(TRUNCATION_SUFFIX));

    // A failing optional step does not poison a successful sibling.
    let response = run(&request, &[&Failing, &SpaceSquisher], None, &config());
    assert_eq!(response.disposition, Disposition::Applied);
    assert!(response.diagnostic.is_none());
}

#[test]
fn timeout_rolls_back_and_preserves_the_original() {
    let request = request(SPACEY, REPLACE_ONLY);
    let mut config = config();
    config.timeout = Duration::from_millis(5);
    let response = run(&request, &[&Slow], None, &config);
    assert_eq!(response.disposition, Disposition::Timeout);
    assert_eq!(response.output, SPACEY);
    assert!(response.compressor_chain.is_empty());
}

/// Retrievable-lossy that returns its input untouched under a scary claim:
/// it must contribute neither a chain entry nor a reversibility downgrade.
struct IdentityPass;

static IDENTITY_SPEC: CompressorSpec = test_spec("identity-pass", Stage::RetrievableLossy, FULL);

impl Compressor for IdentityPass {
    fn spec(&self) -> &CompressorSpec {
        &IDENTITY_SPEC
    }

    fn compress(
        &self,
        content: &str,
        _stash: Option<&dyn StashStore>,
    ) -> Result<CompressOutcome, CompressError> {
        Ok(CompressOutcome {
            output: content.to_owned(),
            reversibility: Reversibility::Unrecoverable,
            stash_writes: Vec::new(),
        })
    }
}

#[test]
fn no_op_compressor_joins_neither_chain_nor_reversibility() {
    let request = request(SPACEY, FULL);
    let mut config = config();
    config.max_tokens = Some(0); // force escalation into the lossy stage

    let response = run(&request, &[&SpaceSquisher, &IdentityPass], None, &config);
    assert_eq!(response.disposition, Disposition::Applied);
    assert_eq!(response.compressor_chain, ["space-squisher"]);
    assert_eq!(response.reversibility, Reversibility::Lossless);

    // A run where only no-ops executed resolves to NoSavings, not Applied.
    let response = run(&request, &[&IdentityPass], None, &config);
    assert_eq!(response.disposition, Disposition::NoSavings);
    assert_eq!(response.output, SPACEY);
}
