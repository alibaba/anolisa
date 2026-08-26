//! Versioned compression protocol shared by every tokenless frontend.
//!
//! This crate defines protocol v1 (evolution roadmap §4.1): the compatibility
//! boundary between agent-specific adapters and the shared compression
//! pipeline. It is deliberately not an OpenAI or Anthropic request shape —
//! [`CompressionRequest`] carries only the model-visible content plus the
//! attribution and capability facts the pipeline needs, and
//! [`CompressionResponse`] carries the final content plus the decision the
//! adapter needs to build its host-specific envelope.
//!
//! The roadmap section numbers cited throughout this crate (§4.1, §4.5,
//! §5.1, §5.6, …) refer to the tokenless evolution roadmap, which has not
//! landed in this repository yet. Until it does, the JSON examples and
//! contract tests in this crate are the authoritative wire contract.
//!
//! # Compatibility rules
//!
//! - Readers ignore unknown fields within a supported major protocol version,
//!   so optional fields may be added without a version bump.
//! - An incompatible shape requires a new `protocol_version`, never a
//!   parallel adapter-specific payload.
//! - [`CompressionRequest::from_json`] / [`CompressionResponse::from_json`]
//!   check the version before the full parse, so a future version is reported
//!   as [`ProtocolError::UnsupportedVersion`] rather than a shape error.
//!
//! # Fail-open contract
//!
//! `CompressionResponse::output` always holds exactly what the adapter must
//! emit. On every non-[`Disposition::Applied`] disposition it is the original
//! model-visible content, so adapters never need fallback logic of their own
//! (roadmap principle 6).
//!
//! # Token counter identity
//!
//! All token counts in protocol v1 use the counter recorded in
//! [`TOKENIZER_ID`]. The choice is the measured §5.1 decision (see the note
//! in `docs/roadmap/evolution-roadmap.md`): the character-class heuristic
//! `heuristic-v1`, not a provider tokenizer. Counts are normalized tokens for
//! arbitration and attribution, not billing estimates.

use serde::{Deserialize, Serialize};

/// The protocol version this crate implements.
pub const PROTOCOL_VERSION: u32 = 1;

/// Identity of the normalized token counter used for every count in
/// protocol v1: the character-class heuristic implemented by
/// `tokenless-stats` (CJK ≈ 1 token per char, other ≈ 1 token per 4 chars).
///
/// Any change to the estimator's character classes or ratios requires a new
/// ID; rows and responses produced under different IDs must never be merged
/// into one series without an explicit per-counter breakdown.
pub const TOKENIZER_ID: &str = "heuristic-v1";

/// Upper bound, in bytes, for [`CompressionResponse::diagnostic`]. Writers
/// truncate to this limit on a char boundary before emitting, so a failing
/// pipeline can never bloat the response payload it is supposed to shrink
/// (roadmap principle 6: diagnostics stay bounded).
pub const DIAGNOSTIC_MAX_BYTES: usize = 4096;

/// Error returned when a protocol payload cannot be accepted or produced.
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    /// The payload declares a protocol version this build does not support.
    #[error("unsupported protocol_version {found} (supported: {PROTOCOL_VERSION})")]
    UnsupportedVersion {
        /// The version the payload declared.
        found: u32,
    },
    /// The payload is not valid JSON for the declared version's shape.
    #[error("malformed protocol payload: {0}")]
    Malformed(#[from] serde_json::Error),
    /// A value could not be serialized to the wire format. Unreachable for
    /// the derived v1 shapes; kept so `to_json` stays honest if a future
    /// field gains a fallible serializer.
    #[error("protocol serialization failed: {0}")]
    Serialize(#[source] serde_json::Error),
}

/// Where in the agent loop the content was intercepted (roadmap §4.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Seam {
    /// Content headed into a model request (e.g. schema publication).
    BeforeModel,
    /// Tool input before execution (e.g. command rewrite).
    PreTool,
    /// Tool output after execution — the primary compression seam.
    PostTool,
    /// A proxy frontend observing model traffic.
    Proxy,
}

impl Seam {
    /// The `snake_case` wire name, identical to this enum's serde encoding.
    /// The stable vocabulary for language bindings, logs, and statistics.
    #[must_use]
    pub fn wire_str(self) -> &'static str {
        match self {
            Self::BeforeModel => "before_model",
            Self::PreTool => "pre_tool",
            Self::PostTool => "post_tool",
            Self::Proxy => "proxy",
        }
    }
}

/// What the requesting adapter's host can actually do with the result.
///
/// The pipeline intersects compressor candidates with these capabilities
/// (roadmap principle 2): a response compressor must not run when the host
/// cannot replace the original model-visible output. Every capability
/// defaults to `false`, so an adapter that declares nothing gets passthrough
/// rather than an unemittable candidate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// The host can replace the model-visible tool output with
    /// [`CompressionResponse::output`].
    #[serde(default)]
    pub replace_output: bool,
    /// The host exposes a retrieval tool (`tokenless_retrieve` or an
    /// equivalent), so retrievable-lossy markers are actually recoverable.
    #[serde(default)]
    pub publish_retrieve_tool: bool,
    /// The host's replacement slot accepts arbitrary text. When `false`,
    /// an applied post-tool output must remain valid JSON with a stable
    /// top-level schema (a structured slot): non-JSON encodings such as
    /// TOON never win, and empty top-level fields dropped by cleanup are
    /// restored before final acceptance.
    #[serde(default)]
    pub replace_with_text: bool,
}

/// A compression request: the model-visible content plus attribution.
///
/// Adapters own their private host contracts (roadmap §4.5); only the
/// model-visible value is copied here. UI or business objects that must
/// remain unmodified never enter the protocol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompressionRequest {
    /// Must equal [`PROTOCOL_VERSION`]. Enforced on every deserialization
    /// path, including direct `serde_json::from_str`.
    #[serde(deserialize_with = "version_must_match")]
    pub protocol_version: u32,
    /// The model-visible content to consider for compression.
    pub content: String,
    /// Stable identifier of the requesting agent frontend
    /// (e.g. `claude-code`).
    pub agent_id: String,
    /// Session attribution, when the host provides one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Tool-use attribution, when the host provides one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    /// Name of the tool that produced the content, when one exists.
    /// Absent for non-tool seams such as schema publication.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Where in the agent loop this content was intercepted.
    pub seam: Seam,
    /// What the host can do with the result. Missing fields are `false`.
    #[serde(default)]
    pub capabilities: Capabilities,
}

impl CompressionRequest {
    /// Creates a v1 request with the required fields; optional attribution
    /// and capabilities are set directly on the public fields.
    pub fn new(content: impl Into<String>, agent_id: impl Into<String>, seam: Seam) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            content: content.into(),
            agent_id: agent_id.into(),
            session_id: None,
            tool_use_id: None,
            tool_name: None,
            seam,
            capabilities: Capabilities::default(),
        }
    }

    /// Parses a request, rejecting unsupported versions before shape errors.
    ///
    /// # Errors
    ///
    /// [`ProtocolError::UnsupportedVersion`] when `protocol_version` differs
    /// from [`PROTOCOL_VERSION`]; [`ProtocolError::Malformed`] when the JSON
    /// does not match the v1 shape.
    pub fn from_json(json: &str) -> Result<Self, ProtocolError> {
        check_version(json)?;
        Ok(serde_json::from_str(json)?)
    }

    /// Serializes to the wire format.
    ///
    /// # Errors
    ///
    /// [`ProtocolError::Serialize`] — unreachable for the current derived
    /// shape, surfaced instead of a panic per library error policy.
    pub fn to_json(&self) -> Result<String, ProtocolError> {
        serde_json::to_string(self).map_err(ProtocolError::Serialize)
    }
}

/// The pipeline's verdict on one request.
///
/// Only [`Disposition::Applied`] means [`CompressionResponse::output`]
/// differs from the request content; every other disposition returns the
/// original so the adapter can emit unconditionally. These names are the
/// shared vocabulary the M1 exit gate requires CLI and Runtime to agree on
/// (roadmap §5.6).
///
/// The Runtime shares this enum directly (its pre-protocol
/// `CompressionDisposition` retired when response compression moved behind
/// the registry), so user-visible strings come from [`Disposition::wire_str`]
/// and cannot fork from the wire values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    /// A compressed candidate was accepted; `output` replaces the original.
    Applied,
    /// Dry-run mode: a candidate was produced and measured, but `output` is
    /// the original content. `before_tokens`/`after_tokens` carry the
    /// predicted delta; dry-run results are never mixed into applied
    /// savings. Mirrors the Runtime's existing dry-run disposition.
    DryRun,
    /// The pipeline chose not to touch the content (skip rule, missing
    /// capability, or unrecognized content routed to passthrough).
    Passthrough,
    /// A candidate was produced but rejected because it did not remove
    /// normalized tokens; no active savings are recorded.
    NoSavings,
    /// Required-reversible mode rejected a candidate whose removed content
    /// would not be retrievable.
    ReversibilityUnavailable,
    /// The pipeline exceeded its overall timeout budget; the original is
    /// preserved.
    Timeout,
    /// An optional compression step failed; the original is preserved and
    /// a bounded diagnostic is recorded (roadmap principle 6).
    Error,
}

impl Disposition {
    /// The `snake_case` wire name, identical to this enum's serde encoding.
    /// The stable vocabulary for language bindings, logs, and statistics.
    #[must_use]
    pub fn wire_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::DryRun => "dry_run",
            Self::Passthrough => "passthrough",
            Self::NoSavings => "no_savings",
            Self::ReversibilityUnavailable => "reversibility_unavailable",
            Self::Timeout => "timeout",
            Self::Error => "error",
        }
    }
}

/// Recovery state of an applied transformation (roadmap principle 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reversibility {
    /// Nothing task-relevant was removed; no recovery needed.
    Lossless,
    /// Removed content is stored in the Stash and referenced by emitted
    /// markers; retrieval restores it byte-exactly.
    Retrievable,
    /// Content was removed without a recovery path. Rejected outright in
    /// required-reversible mode.
    Unrecoverable,
}

/// A compression response: the content to emit plus the decision facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompressionResponse {
    /// Must equal [`PROTOCOL_VERSION`]. Enforced on every deserialization
    /// path, including direct `serde_json::from_str`.
    #[serde(deserialize_with = "version_must_match")]
    pub protocol_version: u32,
    /// Exactly what the adapter must emit — compressed on
    /// [`Disposition::Applied`], the original otherwise.
    pub output: String,
    /// The pipeline's verdict.
    pub disposition: Disposition,
    /// Detected content taxonomy value (e.g. `build_log`), once a detector
    /// classified the content. Wire values are stable strings; the Rust
    /// taxonomy type arrives with the detector and registry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// Stable IDs of the compressors that shaped `output`, in order.
    /// Empty on non-applied dispositions.
    #[serde(default)]
    pub compressor_chain: Vec<String>,
    /// Recovery state of `output`. [`Reversibility::Lossless`] whenever the
    /// original was returned unchanged.
    pub reversibility: Reversibility,
    /// Normalized tokens of the request content, counted by `tokenizer_id`.
    pub before_tokens: u64,
    /// Normalized tokens of `output`, counted by `tokenizer_id`.
    pub after_tokens: u64,
    /// Stash keys committed by this response. Only keys present in an
    /// applied, emitted result appear here; rolled-back candidates never
    /// leak keys (roadmap §4.6).
    #[serde(default)]
    pub stash_keys: Vec<String>,
    /// Identity of the counter behind both token counts. Writers set
    /// [`TOKENIZER_ID`]. A payload missing the field reads as
    /// [`TOKENIZER_ID`] too: the heuristic estimator is the only counter
    /// that ever shipped before the field existed, so the default is the
    /// factual legacy identity rather than an ambiguous empty string.
    #[serde(default = "default_tokenizer_id")]
    pub tokenizer_id: String,
    /// Bounded diagnostic accompanying [`Disposition::Error`]: at most
    /// [`DIAGNOSTIC_MAX_BYTES`] bytes. The pipeline, as the only writer,
    /// truncates before setting it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
}

impl CompressionResponse {
    /// The canonical passthrough response: the original content, unchanged
    /// counts, and no artifacts. Every frontend must produce this same shape
    /// so dispositions stay comparable across CLI and Runtime (§5.6).
    #[must_use]
    pub fn passthrough(request: &CompressionRequest, before_tokens: u64) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            output: request.content.clone(),
            disposition: Disposition::Passthrough,
            content_type: None,
            compressor_chain: Vec::new(),
            reversibility: Reversibility::Lossless,
            before_tokens,
            after_tokens: before_tokens,
            stash_keys: Vec::new(),
            tokenizer_id: TOKENIZER_ID.to_owned(),
            diagnostic: None,
        }
    }

    /// True when `output` replaced the original content.
    #[must_use]
    pub fn is_applied(&self) -> bool {
        self.disposition == Disposition::Applied
    }

    /// Parses a response, rejecting unsupported versions before shape errors.
    ///
    /// # Errors
    ///
    /// [`ProtocolError::UnsupportedVersion`] when `protocol_version` differs
    /// from [`PROTOCOL_VERSION`]; [`ProtocolError::Malformed`] when the JSON
    /// does not match the v1 shape.
    pub fn from_json(json: &str) -> Result<Self, ProtocolError> {
        check_version(json)?;
        Ok(serde_json::from_str(json)?)
    }

    /// Serializes to the wire format.
    ///
    /// # Errors
    ///
    /// [`ProtocolError::Serialize`] — unreachable for the current derived
    /// shape, surfaced instead of a panic per library error policy.
    pub fn to_json(&self) -> Result<String, ProtocolError> {
        serde_json::to_string(self).map_err(ProtocolError::Serialize)
    }
}

/// Extracts and checks `protocol_version` without depending on the rest of
/// the shape, so a future version's payload reports as unsupported rather
/// than malformed.
fn check_version(json: &str) -> Result<(), ProtocolError> {
    #[derive(Deserialize)]
    struct VersionOnly {
        protocol_version: u32,
    }
    let v: VersionOnly = serde_json::from_str(json)?;
    if v.protocol_version != PROTOCOL_VERSION {
        return Err(ProtocolError::UnsupportedVersion {
            found: v.protocol_version,
        });
    }
    Ok(())
}

/// Field-level guard used by the derived `Deserialize` impls, so the version
/// gate cannot be bypassed by deserializing the structs directly instead of
/// going through `from_json`.
fn version_must_match<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = u32::deserialize(deserializer)?;
    if v != PROTOCOL_VERSION {
        return Err(serde::de::Error::custom(format!(
            "unsupported protocol_version {v} (supported: {PROTOCOL_VERSION})"
        )));
    }
    Ok(v)
}

fn default_tokenizer_id() -> String {
    TOKENIZER_ID.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    include!("tests/protocol_tests.rs");
}
