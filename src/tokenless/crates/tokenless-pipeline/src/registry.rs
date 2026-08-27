//! Compile-time compressor registry and candidate filtering (roadmap §4.2).
//!
//! Registration is static (roadmap principle 7): the registry is a `const`
//! slice assembled at compile time; dynamic plugins and configuration-driven
//! loading are out of scope. [`candidates`] applies the routing rule from
//! principle 2 — a compressor is only a candidate when it supports the
//! detected content type, runs at the request's seam, and every capability
//! it requires is declared by the adapter.

use crate::content::ContentType;
use tokenless_protocol::{Capabilities, Seam};

/// Escalation stage a compressor belongs to (roadmap principle 3). The
/// ordering matches the escalation ladder: lossless runs before
/// retrievable-lossy runs before truncation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    /// Removes nothing task-relevant; always safe to apply.
    Lossless,
    /// Removes content that is stashed and retrievable by marker.
    RetrievableLossy,
    /// Bounded truncation; the last resort.
    Truncation,
}

/// Bounded cost class used for timeout-aware selection: the pipeline skips
/// classes whose per-class budget does not fit in the remaining overall
/// timeout budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CostClass {
    /// Single-pass scanning, no allocation proportional to input.
    Cheap,
    /// Parsing or restructuring proportional to input size.
    Moderate,
    /// Multi-pass analysis; only selected when the budget is generous.
    Expensive,
}

/// One registered compressor's routing metadata.
#[derive(Debug, Clone, Copy)]
pub struct CompressorSpec {
    /// Stable ID reported in `compressor_chain` and persisted in stats.
    pub id: &'static str,
    /// Content types this compressor understands.
    pub content_types: &'static [ContentType],
    /// Seams this compressor may run at.
    pub seams: &'static [Seam],
    /// Capabilities the adapter must declare for this compressor to be a
    /// candidate. A required `replace_output` keeps response-shaping
    /// compressors away from hosts that cannot replace model-visible
    /// output; a required `publish_retrieve_tool` keeps retrievable-lossy
    /// compressors away from hosts where markers would be dead ends.
    pub required_capabilities: Capabilities,
    /// Escalation stage this compressor runs in.
    pub stage: Stage,
    /// Cost class for timeout-aware selection.
    pub cost_class: CostClass,
}

impl CompressorSpec {
    /// The routing rule from principle 2: a spec matches when it supports
    /// the detected content type, runs at the request's seam, and every
    /// capability it requires is declared by the adapter.
    #[must_use]
    pub fn matches(
        &self,
        content_type: ContentType,
        seam: Seam,
        capabilities: Capabilities,
    ) -> bool {
        self.content_types.contains(&content_type)
            && self.seams.contains(&seam)
            && (!self.required_capabilities.replace_output || capabilities.replace_output)
            && (!self.required_capabilities.publish_retrieve_tool
                || capabilities.publish_retrieve_tool)
            && (!self.required_capabilities.replace_with_text || capabilities.replace_with_text)
    }
}

/// Routing spec of the existing JSON response cleanup (roadmap §5.3): the
/// structural cleanup and truncation pass that predates the pipeline. Its
/// [`Compressor`](crate::Compressor) implementation lives with the Runtime,
/// which owns the stash and per-call configuration; its `spec()` returns a
/// reference to this spec so the registry entry and the implementation
/// cannot drift.
///
/// Registered as retrievable-lossy: truncated content is stashed and
/// marker-referenced when a stash store is attached. Only `replace_output`
/// is required — the compressor degrades to a lossless-only claim or an
/// unrecoverable one depending on what it actually removed, and required
/// reversibility is enforced by arbitration, not by routing.
pub const RESPONSE_CLEANUP: CompressorSpec = CompressorSpec {
    id: "response-cleanup",
    content_types: &[ContentType::JsonRecords],
    seams: &[Seam::PostTool],
    required_capabilities: Capabilities {
        replace_output: true,
        publish_retrieve_tool: false,
        replace_with_text: false,
    },
    stage: Stage::RetrievableLossy,
    cost_class: CostClass::Moderate,
};

/// Routing spec of the lossless terminal cleanup (roadmap §6.1 level 1):
/// ANSI colour and style codes, which carry no information. Redraws, cursor
/// control, and glyph animation are left for the retrievable-lossy stage,
/// which can stash what it decides to drop. The engine lives in
/// `tokenless-compressors`; the adapter lives with the Runtime.
/// Requires `replace_with_text` because its output is plain text — it must
/// stay away from structured slots (JSON envelopes keep their own path).
pub const TERMINAL_CLEANUP: CompressorSpec = CompressorSpec {
    id: "terminal-cleanup",
    content_types: &[ContentType::BuildLog, ContentType::PlainText],
    seams: &[Seam::PostTool],
    required_capabilities: Capabilities {
        replace_output: true,
        publish_retrieve_tool: false,
        replace_with_text: true,
    },
    stage: Stage::Lossless,
    cost_class: CostClass::Cheap,
};

/// Routing spec of the build/log compressor (roadmap §6.1 level 2). Also
/// matches `PlainText`: shell text that is not a build log still gets the
/// engine's conservative generic mode (bounded head/tail keep + stash),
/// which replaces the blind mid-string truncation such text received on the
/// legacy JSON-envelope path. Shell text detected as another specific type
/// (diff, source code, stack trace, …) matches no spec yet and passes
/// through — each gap closes when that type's own compressor lands.
/// `publish_retrieve_tool` is required: without a reachable retrieve tool
/// its per-gap markers would be dead ends.
pub const BUILD_LOG: CompressorSpec = CompressorSpec {
    id: "build-log",
    content_types: &[ContentType::BuildLog, ContentType::PlainText],
    seams: &[Seam::PostTool],
    required_capabilities: Capabilities {
        replace_output: true,
        publish_retrieve_tool: true,
        replace_with_text: true,
    },
    stage: Stage::RetrievableLossy,
    cost_class: CostClass::Moderate,
};

/// The production registry. Entries are appended with the compressor that
/// implements them, never speculatively.
pub const REGISTRY: &[CompressorSpec] = &[RESPONSE_CLEANUP, TERMINAL_CLEANUP, BUILD_LOG];

/// Filters `registry` down to the candidates for one request, preserving
/// registration order (the pipeline groups by [`Stage`] on top of it).
pub fn candidates(
    registry: &[CompressorSpec],
    content_type: ContentType,
    seam: Seam,
    capabilities: Capabilities,
) -> impl Iterator<Item = &CompressorSpec> {
    registry
        .iter()
        .filter(move |spec| spec.matches(content_type, seam, capabilities))
}

#[cfg(test)]
mod tests {
    use super::*;
    include!("tests/registry_tests.rs");
}
