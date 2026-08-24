//! Staged pipeline execution and end-to-end arbitration (roadmap §4.3).
//!
//! [`run`] takes a request through detection, routing, and the escalation
//! ladder: every applicable lossless transformation, then at most one
//! content-specific retrievable-lossy compressor, then at most one bounded
//! truncation — escalating only while the configured size policy is unmet.
//! A specialized lossy decision is final: no second lossy compressor and no
//! generic lossy fallback runs after it. Only the bounded truncation stage,
//! the ladder's last resort (§4.3 step 4), may still follow — and a stash
//! write whose marker a later stage cuts out of the output is rolled back
//! rather than committed.
//!
//! Arbitration is end-to-end: the original and the final candidate are
//! compared once. A candidate that does not remove normalized tokens,
//! violates required reversibility, or exceeds the overall timeout budget is
//! rejected as a whole — its newly created Stash keys are rolled back and
//! the original content is emitted unchanged. Compression stays an optional
//! optimization throughout: a failing compressor never fails the request;
//! the first failure is kept as a bounded diagnostic (roadmap principle 6).

use std::time::{Duration, Instant};

use tokenless_ccr::{StashError, StashStore, StashWrite, marker_for};
use tokenless_protocol::{
    CompressionRequest, CompressionResponse, DIAGNOSTIC_MAX_BYTES, Disposition, PROTOCOL_VERSION,
    Reversibility, TOKENIZER_ID,
};
use tokenless_stats::estimate_tokens;

use crate::content::{ContentType, detect};
use crate::registry::{CompressorSpec, Stage};

/// One executable compression step. [`crate::CompressorSpec`] is the routing
/// metadata; this trait is the behavior behind it. Existing compressors
/// migrate onto it starting with the response cleanup (roadmap §5.3); until
/// then only test doubles implement it.
pub trait Compressor {
    /// The routing spec this compressor registers under.
    fn spec(&self) -> &CompressorSpec;

    /// Produces a transformed candidate for `content`.
    ///
    /// `stash` is present when the host can actually retrieve stashed
    /// payloads. A retrievable-lossy compressor records every write in
    /// [`CompressOutcome::stash_writes`], which is what lets the pipeline
    /// roll them back when the end-to-end candidate is rejected.
    ///
    /// # Errors
    ///
    /// An error never fails the request: the pipeline records the first
    /// failure as a bounded diagnostic and continues with the unchanged
    /// input.
    fn compress(
        &self,
        content: &str,
        stash: Option<&dyn StashStore>,
    ) -> Result<CompressOutcome, CompressError>;
}

/// A successful compressor result.
#[derive(Debug)]
pub struct CompressOutcome {
    /// The transformed content.
    pub output: String,
    /// Recovery state of `output` relative to the input.
    pub reversibility: Reversibility,
    /// Stash writes performed while producing `output`, in order. The
    /// pipeline deletes exactly these on rollback; their keys become
    /// [`CompressionResponse::stash_keys`] when the candidate is emitted.
    pub stash_writes: Vec<StashWrite>,
}

/// Errors a compressor can surface.
#[derive(Debug, thiserror::Error)]
pub enum CompressError {
    /// The stash backend failed while storing removed content.
    #[error(transparent)]
    Stash(#[from] StashError),
    /// The compressor could not produce a candidate from this input.
    #[error("{0}")]
    Failed(String),
}

/// Arbitration policy for one [`run`] call.
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    /// Overall budget across detection and all stages. When exceeded, the
    /// pipeline stops at the next stage boundary, rolls back, and returns
    /// [`Disposition::Timeout`].
    pub timeout: Duration,
    /// The size policy: escalate beyond the lossless stage only while the
    /// candidate exceeds this many normalized tokens. `None` never
    /// escalates — only lossless transformations run.
    pub max_tokens: Option<u64>,
    /// Reject a candidate whose removed content would be unrecoverable
    /// ([`Disposition::ReversibilityUnavailable`]).
    pub require_reversibility: bool,
    /// Measure the candidate but emit the original
    /// ([`Disposition::DryRun`]); its stash writes are rolled back.
    pub dry_run: bool,
}

/// Runs one request through detection, routing, the escalation ladder, and
/// end-to-end arbitration.
///
/// The response is always emittable: on every non-applied disposition
/// `output` is the original request content (fail-open contract of protocol
/// v1). `compressors` is the executable registry — callers pass the
/// compressor set matching their build; filtering by content type, seam,
/// and declared capabilities happens here.
#[must_use]
pub fn run(
    request: &CompressionRequest,
    compressors: &[&dyn Compressor],
    stash: Option<&dyn StashStore>,
    config: &PipelineConfig,
) -> CompressionResponse {
    let deadline = Instant::now() + config.timeout;
    let before_tokens = tokens(&request.content);
    let content_type = detect(&request.content);

    let mut lossless: Vec<&dyn Compressor> = Vec::new();
    let mut lossy: Vec<&dyn Compressor> = Vec::new();
    let mut truncation: Vec<&dyn Compressor> = Vec::new();
    for &compressor in compressors {
        let spec = compressor.spec();
        if !spec.matches(content_type, request.seam, request.capabilities) {
            continue;
        }
        match spec.stage {
            Stage::Lossless => lossless.push(compressor),
            Stage::RetrievableLossy => lossy.push(compressor),
            Stage::Truncation => truncation.push(compressor),
        }
    }

    let mut arb = Arbitration {
        request,
        stash,
        content_type,
        before_tokens,
        current: request.content.clone(),
        chain: Vec::new(),
        ledger: StashLedger::default(),
        reversibility: Reversibility::Lossless,
        ran_any: false,
        first_error: None,
    };

    // §4.3 steps 1–2: all applicable lossless transformations produce x1,
    // which is dropped outright if it did not remove normalized tokens.
    for &compressor in &lossless {
        if Instant::now() > deadline {
            return arb.rejected(Disposition::Timeout);
        }
        arb.step(compressor);
    }
    if !arb.chain.is_empty() && tokens(&arb.current) >= before_tokens {
        arb.revert();
    }

    // §4.3 steps 3–4: escalate only while the size policy is unmet, one
    // compressor per stage — the first registered candidate decides.
    for stage in [&lossy, &truncation] {
        let Some(&compressor) = stage.first() else {
            continue;
        };
        let policy_met = config
            .max_tokens
            .is_none_or(|max| tokens(&arb.current) <= max);
        if policy_met {
            break;
        }
        if Instant::now() > deadline {
            return arb.rejected(Disposition::Timeout);
        }
        arb.step(compressor);
    }

    if Instant::now() > deadline {
        return arb.rejected(Disposition::Timeout);
    }
    arb.judge(config)
}

/// Normalized token count under the protocol's `heuristic-v1` counter.
fn tokens(text: &str) -> u64 {
    estimate_tokens(text) as u64
}

/// The in-flight candidate and everything needed to emit or undo it.
struct Arbitration<'a> {
    request: &'a CompressionRequest,
    stash: Option<&'a dyn StashStore>,
    content_type: ContentType,
    before_tokens: u64,
    current: String,
    chain: Vec<String>,
    ledger: StashLedger,
    reversibility: Reversibility,
    ran_any: bool,
    first_error: Option<String>,
}

impl Arbitration<'_> {
    /// Runs one compressor over the current candidate and adopts its
    /// outcome. A failure leaves the candidate unchanged and is kept as the
    /// diagnostic if it is the first.
    fn step(&mut self, compressor: &dyn Compressor) {
        match compressor.compress(&self.current, self.stash) {
            Ok(outcome) => {
                self.ran_any = true;
                self.current = outcome.output;
                self.chain.push(compressor.spec().id.to_owned());
                for write in outcome.stash_writes {
                    self.ledger.record(write);
                }
                self.reversibility = worst(self.reversibility, outcome.reversibility);
            }
            Err(error) => {
                if self.first_error.is_none() {
                    self.first_error = Some(truncate_diagnostic(&format!(
                        "{}: {error}",
                        compressor.spec().id
                    )));
                }
            }
        }
    }

    /// Drops everything adopted so far: rows this run created are deleted
    /// (their markers never reach the model) and the candidate returns to
    /// the original content.
    fn revert(&mut self) {
        self.ledger.rollback(self.stash);
        self.current.clone_from(&self.request.content);
        self.chain.clear();
        self.reversibility = Reversibility::Lossless;
    }

    /// A non-applied verdict: rolled back, original content, unchanged
    /// counts.
    fn rejected(&mut self, disposition: Disposition) -> CompressionResponse {
        self.revert();
        let mut response = CompressionResponse::passthrough(self.request, self.before_tokens);
        response.disposition = disposition;
        response.content_type = Some(self.content_type.wire_str().to_owned());
        if disposition == Disposition::Error {
            response.diagnostic = self.first_error.take();
        }
        response
    }

    /// §4.3 steps 5–6: the single end-to-end comparison of `tokens(x0)`
    /// against the final candidate, and the explicit rejection dispositions.
    fn judge(&mut self, config: &PipelineConfig) -> CompressionResponse {
        if self.chain.is_empty() {
            let disposition = if self.ran_any {
                Disposition::NoSavings
            } else if self.first_error.is_some() {
                Disposition::Error
            } else {
                Disposition::Passthrough
            };
            return self.rejected(disposition);
        }
        if config.require_reversibility && self.reversibility == Reversibility::Unrecoverable {
            return self.rejected(Disposition::ReversibilityUnavailable);
        }
        let after_tokens = tokens(&self.current);
        if after_tokens >= self.before_tokens {
            return self.rejected(Disposition::NoSavings);
        }
        if config.dry_run {
            let mut response = self.rejected(Disposition::DryRun);
            response.after_tokens = after_tokens;
            return response;
        }
        let output = std::mem::take(&mut self.current);
        let stash_keys = self.ledger.commit(&output, self.stash);
        CompressionResponse {
            protocol_version: PROTOCOL_VERSION,
            output,
            disposition: Disposition::Applied,
            content_type: Some(self.content_type.wire_str().to_owned()),
            compressor_chain: std::mem::take(&mut self.chain),
            reversibility: self.reversibility,
            before_tokens: self.before_tokens,
            after_tokens,
            stash_keys,
            tokenizer_id: TOKENIZER_ID.to_owned(),
            diagnostic: None,
        }
    }
}

/// Stash bookkeeping for one arbitration: which keys the candidate's output
/// references, and which rows this run brought into existence and therefore
/// owns for rollback purposes.
#[derive(Default)]
struct StashLedger {
    /// Keys written while producing the candidate, in emission order,
    /// deduplicated.
    keys: Vec<String>,
    /// `(key, generation)` ownership chains for rows this run *created*.
    /// A pre-existing row (`created == false` on first sight) is never
    /// tracked: this run only refreshed its expiry, and deleting it would
    /// strand markers emitted by earlier requests.
    owned: Vec<(String, u64)>,
}

impl StashLedger {
    /// Records one stash write, maintaining the ownership chain per the
    /// [`StashStore`] contract: a re-stash of an owned key re-adopts the new
    /// generation only while `previous_generation` matches the one recorded
    /// here; a mismatch means a foreign writer refreshed in between, and the
    /// key is dropped from the rollback list so that refresh stays live.
    fn record(&mut self, write: StashWrite) {
        if !self.keys.contains(&write.key) {
            self.keys.push(write.key.clone());
        }
        if write.created {
            self.owned.push((write.key, write.generation));
            return;
        }
        let Some(index) = self.owned.iter().position(|(key, _)| *key == write.key) else {
            return;
        };
        if write.previous_generation == Some(self.owned[index].1) {
            self.owned[index].1 = write.generation;
        } else {
            self.owned.swap_remove(index);
        }
    }

    /// Deletes every row this run created. Best-effort: a row that survives
    /// a backend error is unreachable and evicted by expiry, and the
    /// generation guard keeps concurrently refreshed rows alive.
    fn rollback(&mut self, stash: Option<&dyn StashStore>) {
        self.keys.clear();
        for (key, generation) in self.owned.drain(..) {
            if let Some(stash) = stash {
                let _ = stash.delete(&key, generation);
            }
        }
    }

    /// Commits the ledger against the final output: only keys whose marker
    /// actually appears in it are emitted (the protocol's `stash_keys` are
    /// the keys *present in* the applied result), while a write whose marker
    /// a later stage cut out is rolled back instead of leaking as an
    /// unreachable row.
    fn commit(&mut self, output: &str, stash: Option<&dyn StashStore>) -> Vec<String> {
        let (kept, orphaned): (Vec<String>, Vec<String>) = std::mem::take(&mut self.keys)
            .into_iter()
            .partition(|key| output.contains(&marker_for(key)));
        for key in &orphaned {
            if let Some(index) = self.owned.iter().position(|(owned, _)| owned == key) {
                let (key, generation) = self.owned.swap_remove(index);
                if let Some(stash) = stash {
                    let _ = stash.delete(&key, generation);
                }
            }
        }
        self.owned.clear();
        kept
    }
}

/// Reversibility of a chain is its weakest link.
fn worst(a: Reversibility, b: Reversibility) -> Reversibility {
    use Reversibility::{Lossless, Retrievable, Unrecoverable};
    match (a, b) {
        (Unrecoverable, _) | (_, Unrecoverable) => Unrecoverable,
        (Retrievable, _) | (_, Retrievable) => Retrievable,
        (Lossless, Lossless) => Lossless,
    }
}

/// Marker appended to a truncated diagnostic so readers can tell a clipped
/// message from a complete one.
const TRUNCATION_SUFFIX: &str = " [truncated]";

/// Truncates to [`DIAGNOSTIC_MAX_BYTES`] on a char boundary, marking the
/// cut. The marker fits inside the limit — the bound is a protocol
/// contract, so it is never exceeded on top.
fn truncate_diagnostic(message: &str) -> String {
    if message.len() <= DIAGNOSTIC_MAX_BYTES {
        return message.to_owned();
    }
    let mut end = DIAGNOSTIC_MAX_BYTES - TRUNCATION_SUFFIX.len();
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{TRUNCATION_SUFFIX}", &message[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    include!("tests/pipeline_tests.rs");
}
