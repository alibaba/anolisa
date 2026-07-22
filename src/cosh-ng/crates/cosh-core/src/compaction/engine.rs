//! Core-owned compaction lifecycle shared by manual and automatic triggers.
//!
//! Commit order: load snapshot → capture generation and source digest →
//! choose a safe prefix → generate the summary without holding the session
//! lock → validate the candidate → reload and revalidate generation and
//! digest → atomically commit through the session store.

use std::io::Write;

use serde::Serialize;

use crate::cli::CliArgs;
use crate::config::CoreConfig;
use crate::context::ContextBuilder;
use crate::provider::ContentGenerator;
use crate::session::{PersistedSession, ProviderSessionId, SessionError, SessionStore};

use super::boundary::{group_agent_runs, select_compacted_through, BoundaryError};
use super::budget::{
    estimate_messages_tokens, estimate_text_tokens, measure_history, ContextBudget, ModelCapability,
};
use super::projection::{
    effective_messages, source_digest, CompactionState, TokenMeasurement, COMPACTION_PROMPT_VERSION,
};
use super::summarize::{
    generate_summary, render_summary_input, summary_input_token_budget, SummaryError,
};

/// Estimated tokens reserved for tool declarations in CLI-mode budgets.
const CLI_TOOL_DECLARATION_RESERVE: u64 = 2_048;

/// Expected pre-compaction context revision for an automatic attempt.
///
/// An automatic recommendation is emitted at an idle boundary and acted on
/// later by a separate process; by then the session may have advanced.
/// [`compact_session`] validates this against the freshly loaded session
/// *before* any provider work, so a stale recommendation never spends a model
/// call or rewrites a context that already moved on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpectedRevision {
    /// Session store generation captured when the recommendation was emitted.
    pub generation: u64,
    /// Projection revision captured when the recommendation was emitted
    /// (`0` when no projection existed yet).
    pub projection_revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
/// Why a compaction attempt was started.
pub enum CompactionTrigger {
    /// Explicit user request (`/session compact` or `--compact`).
    Manual,
    /// Idle-boundary automatic trigger after crossing the soft threshold.
    Auto,
    /// In-run emergency protection before an oversized provider request.
    Emergency,
}

impl CompactionTrigger {
    /// Returns the stable protocol label.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Auto => "auto",
            Self::Emergency => "emergency",
        }
    }
}

#[derive(Debug, Clone)]
/// Failure classes that leave the previous projection untouched.
pub enum CompactionError {
    /// Compaction is disabled by configuration.
    Disabled,
    /// The transcript has no compactable complete Agent run.
    NothingToCompact,
    /// The transcript violates the tool protocol; compaction fails closed.
    Boundary(String),
    /// The summarizer provider request failed.
    Provider(String),
    /// The provider returned no usable summary text.
    EmptySummary,
    /// The provider exceeded the persisted summary bound.
    OversizedSummary,
    /// The summary stream was cancelled before completion.
    Cancelled,
    /// The candidate did not shrink the effective context.
    NotReducing,
    /// No safe cut fits inside the bounded summarizer input.
    OversizedInput,
    /// The session generation moved between snapshot and commit.
    Conflict,
    /// The summarized prefix changed between snapshot and commit.
    DigestMismatch,
    /// The session store rejected the operation.
    Session(SessionError),
}

impl CompactionError {
    /// Returns the stable machine-readable code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::NothingToCompact => "nothing_to_compact",
            Self::Boundary(_) => "unsafe_boundary",
            Self::Provider(_) => "provider_error",
            Self::EmptySummary => "empty_summary",
            Self::OversizedSummary => "oversized_summary",
            Self::Cancelled => "cancelled",
            Self::NotReducing => "not_reducing",
            Self::OversizedInput => "oversized_input",
            Self::Conflict => "conflict",
            Self::DigestMismatch => "digest_mismatch",
            Self::Session(error) => error.code(),
        }
    }
}

impl std::fmt::Display for CompactionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disabled => write!(formatter, "session compaction is disabled"),
            Self::NothingToCompact => {
                write!(formatter, "no complete Agent run is old enough to compact")
            }
            Self::Boundary(detail) => {
                write!(formatter, "unsafe transcript boundary: {detail}")
            }
            Self::Provider(detail) => write!(formatter, "summary generation failed: {detail}"),
            Self::EmptySummary => write!(formatter, "provider returned an empty summary"),
            Self::OversizedSummary => {
                write!(formatter, "provider summary exceeded the size bound")
            }
            Self::Cancelled => write!(formatter, "compaction was cancelled"),
            Self::NotReducing => {
                write!(formatter, "candidate summary did not reduce the context")
            }
            Self::OversizedInput => write!(
                formatter,
                "no safe split fits the bounded summarizer input; history cannot be \
                 summarized without loss"
            ),
            Self::Conflict => write!(
                formatter,
                "session changed concurrently; candidate was discarded"
            ),
            Self::DigestMismatch => write!(
                formatter,
                "summarized transcript prefix changed; candidate was discarded"
            ),
            Self::Session(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for CompactionError {}

impl From<BoundaryError> for CompactionError {
    fn from(error: BoundaryError) -> Self {
        Self::Boundary(error.to_string())
    }
}

impl From<SummaryError> for CompactionError {
    fn from(error: SummaryError) -> Self {
        match error {
            SummaryError::Provider(detail) => Self::Provider(detail),
            SummaryError::Empty => Self::EmptySummary,
            SummaryError::Oversized => Self::OversizedSummary,
            SummaryError::Cancelled => Self::Cancelled,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
/// Committed compaction outcome reported over CLI and shell protocols.
pub struct CompactionReport {
    /// Session that owns the projection.
    pub session_id: String,
    /// Committed projection revision.
    pub revision: u64,
    /// Transcript index covered by the summary.
    pub compacted_through: usize,
    /// Complete transcript length; unchanged by compaction.
    pub transcript_messages: usize,
    /// Effective context measurement before compaction.
    pub tokens_before: TokenMeasurement,
    /// Estimated effective context after compaction.
    pub tokens_after: TokenMeasurement,
    /// Persisted summary size in bytes.
    pub summary_bytes: usize,
    /// Trigger that started this attempt.
    pub trigger: CompactionTrigger,
    /// Budget snapshot used for the decision.
    pub budget: ContextBudget,
}

/// Runs one full compaction attempt against a persisted session.
///
/// The session lock is only held inside the final `persist`; the provider
/// request runs lock-free. Any error leaves the stored projection unchanged.
///
/// # Errors
///
/// Returns a [`CompactionError`] describing why the candidate was not
/// committed.
pub async fn compact_session(
    store: &SessionStore,
    session_id: &ProviderSessionId,
    provider: &dyn ContentGenerator,
    config: &CoreConfig,
    prefix_tokens: u64,
    provider_reported_history: Option<u64>,
    trigger: CompactionTrigger,
    expected: Option<ExpectedRevision>,
) -> Result<(CompactionReport, PersistedSession), CompactionError> {
    let policy = &config.session.compaction;
    if !policy.enabled {
        return Err(CompactionError::Disabled);
    }

    // 1. Snapshot: capture generation and digest before any provider work.
    let session = store.load(session_id).map_err(CompactionError::Session)?;
    let base_generation = session.generation;

    // 1a. Fail closed on a stale automatic recommendation *before* spending a
    //     provider call: if the session generation or the projection revision
    //     moved since the recommendation was emitted, this attempt targets an
    //     out-of-date context and must be discarded with no model work.
    if let Some(expected) = expected {
        if base_generation != expected.generation {
            return Err(CompactionError::Conflict);
        }
        let current_revision = session
            .compaction
            .as_ref()
            .map(|state| state.revision)
            .unwrap_or(0);
        if current_revision != expected.projection_revision {
            return Err(CompactionError::Conflict);
        }
    }

    let model = if session.model.is_empty() {
        config.resolve_provider().model
    } else {
        session.model.clone()
    };

    let capability = ModelCapability::resolve(policy, config.agent.session_token_limit, &model);
    let budget = ContextBudget::compute(capability, prefix_tokens, policy);

    let effective_before = effective_messages(&session.messages, session.compaction.as_ref());
    let estimated_before = estimate_messages_tokens(&effective_before);
    let tokens_before = measure_history(provider_reported_history, estimated_before);

    // 2. Choose a safe prefix over the complete transcript.
    let cut = select_compacted_through(
        &session.messages,
        policy.preserve_recent_runs,
        budget.target_tokens,
    )?
    .ok_or(CompactionError::NothingToCompact)?;
    let previous_cut = session
        .compaction
        .as_ref()
        .map(|state| state.compacted_through)
        .unwrap_or(0);
    if cut <= previous_cut {
        return Err(CompactionError::NothingToCompact);
    }

    // 3. Bound the summarizer input by the summarizer model's own context
    //    window (plus the byte-level memory ceiling). The committed cut may
    //    only cover messages the input actually rendered; otherwise omitted
    //    history would silently vanish from the effective context.
    let (cut, input) = bounded_cut_and_input(
        &session.messages,
        session.compaction.as_ref(),
        previous_cut,
        cut,
        summary_input_token_budget(&capability),
    )?;
    let digest = source_digest(&session.messages[..cut]);

    // 4. Summarize without holding the session lock.
    let summary = generate_summary(provider, &model, &input).await?;

    // 5. Validate the candidate before touching persistence.
    let candidate = CompactionState {
        revision: session
            .compaction
            .as_ref()
            .map(|state| state.revision)
            .unwrap_or(0)
            + 1,
        compacted_through: cut,
        summary,
        model: model.clone(),
        prompt_version: COMPACTION_PROMPT_VERSION,
        source_digest: digest.clone(),
        tokens_before: Some(tokens_before),
        tokens_after: None,
        created_at_ms: now_ms(),
    };
    let effective_after = effective_messages(&session.messages, Some(&candidate));
    let estimated_after = estimate_messages_tokens(&effective_after);
    if estimated_after >= estimated_before {
        return Err(CompactionError::NotReducing);
    }
    let tokens_after = measure_history(None, estimated_after);
    let mut candidate = candidate;
    candidate.tokens_after = Some(tokens_after);

    // 6. Reacquire, revalidate, and commit atomically.
    let mut current = store.load(session_id).map_err(CompactionError::Session)?;
    if current.generation != base_generation {
        return Err(CompactionError::Conflict);
    }
    if current.messages.len() < cut || source_digest(&current.messages[..cut]) != digest {
        return Err(CompactionError::DigestMismatch);
    }
    current.compaction = Some(candidate.clone());
    match store.persist(&mut current) {
        Ok(()) => {}
        Err(SessionError::Conflict { .. }) => return Err(CompactionError::Conflict),
        Err(error) => return Err(CompactionError::Session(error)),
    }

    let report = CompactionReport {
        session_id: session_id.to_string(),
        revision: candidate.revision,
        compacted_through: cut,
        transcript_messages: current.messages.len(),
        tokens_before,
        tokens_after,
        summary_bytes: candidate.summary.len(),
        trigger,
        budget,
    };
    Ok((report, current))
}

/// In-memory emergency compaction for the active in-run context.
///
/// Used between complete model/tool exchanges inside one Agent run, where the
/// running process is the session's only writer; the resulting projection is
/// committed together with the run's transcript at the next persist.
///
/// Returns `None` when no safe cut exists or the candidate fails validation;
/// the caller keeps its previous projection in either case.
pub(crate) async fn compact_in_memory(
    messages: &[crate::provider::Message],
    previous: Option<&CompactionState>,
    provider: &dyn ContentGenerator,
    model: &str,
    config: &CoreConfig,
    target_tokens: u64,
) -> Option<CompactionState> {
    let policy = &config.session.compaction;
    if !policy.enabled {
        return None;
    }
    let previous_cut = previous.map(|state| state.compacted_through).unwrap_or(0);
    let cut = select_compacted_through(messages, policy.preserve_recent_runs, target_tokens)
        .ok()
        .flatten()
        .filter(|cut| *cut > previous_cut)?;
    // Emergency compaction shares the exact model-aware input budget used by
    // the manual/automatic engine path, so the three triggers cannot drift.
    let capability = ModelCapability::resolve(policy, config.agent.session_token_limit, model);
    let (cut, input) = bounded_cut_and_input(
        messages,
        previous,
        previous_cut,
        cut,
        summary_input_token_budget(&capability),
    )
    .ok()?;
    let digest = source_digest(&messages[..cut]);
    let summary = generate_summary(provider, model, &input).await.ok()?;

    let estimated_before = estimate_messages_tokens(&effective_messages(messages, previous));
    let candidate = CompactionState {
        revision: previous.map(|state| state.revision).unwrap_or(0) + 1,
        compacted_through: cut,
        summary,
        model: model.to_string(),
        prompt_version: COMPACTION_PROMPT_VERSION,
        source_digest: digest,
        tokens_before: Some(measure_history(None, estimated_before)),
        tokens_after: None,
        created_at_ms: now_ms(),
    };
    let estimated_after = estimate_messages_tokens(&effective_messages(messages, Some(&candidate)));
    if estimated_after >= estimated_before {
        return None;
    }
    let mut candidate = candidate;
    candidate.tokens_after = Some(measure_history(None, estimated_after));
    Some(candidate)
}

/// Clamps a selected cut so the projection never covers messages that the
/// bounded summarizer input could not represent.
///
/// `max_input_tokens` is the model-aware budget from
/// [`summary_input_token_budget`]; the byte ceiling inside
/// [`render_summary_input`] stays as a memory-safety second layer. Returns
/// the final cut together with the input that renders exactly the committed
/// message range.
///
/// # Errors
///
/// Returns [`CompactionError::OversizedInput`] when not even one complete
/// Agent run fits inside the summarizer input bound, and propagates
/// boundary validation failures.
fn bounded_cut_and_input(
    messages: &[crate::provider::Message],
    previous: Option<&CompactionState>,
    previous_cut: usize,
    cut: usize,
    max_input_tokens: u64,
) -> Result<(usize, String), CompactionError> {
    let rendered = render_summary_input(previous, &messages[previous_cut..cut], max_input_tokens);
    if rendered.rendered_messages >= cut - previous_cut {
        return Ok((cut, rendered.input));
    }
    // Shrink to the largest run boundary whose whole span was rendered.
    let limit = previous_cut + rendered.rendered_messages;
    let adjusted = group_agent_runs(messages)?
        .iter()
        .map(|span| span.start)
        .filter(|start| *start > previous_cut && *start <= limit)
        .max()
        .ok_or(CompactionError::OversizedInput)?;
    let rerendered = render_summary_input(
        previous,
        &messages[previous_cut..adjusted],
        max_input_tokens,
    );
    if rerendered.rendered_messages < adjusted - previous_cut {
        // A strictly shorter slice must render fully; anything else means
        // one run alone exceeds the input bound.
        return Err(CompactionError::OversizedInput);
    }
    Ok((adjusted, rerendered.input))
}

/// Estimates the runtime prefix (`P`) for provider-free CLI budgets.
pub(crate) fn cli_prefix_tokens(config: &CoreConfig, workspace: &std::path::Path) -> u64 {
    let system_prompt = ContextBuilder::build_system_prompt(
        workspace,
        &[],
        &[],
        &config.agent.approval_mode,
        config.ai.output_language.as_deref(),
    );
    estimate_text_tokens(&system_prompt) + CLI_TOOL_DECLARATION_RESERVE
}

#[derive(Debug, Serialize)]
struct CompactCliEnvelope<'a> {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<&'a CompactionReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<CompactCliError>,
}

#[derive(Debug, Serialize)]
struct CompactCliError {
    code: String,
    message: String,
    recoverable: bool,
}

/// Serves `cosh-core --headless --workspace <ws> --resume <id> --compact`.
///
/// Emits exactly one JSON envelope on stdout and returns the exit code.
pub(crate) async fn run_compact_cli(args: &CliArgs, config: CoreConfig) -> i32 {
    let result = compact_cli_inner(args, &config).await;
    let stdout = std::io::stdout();
    let mut writer = std::io::BufWriter::new(stdout.lock());
    let (envelope, code) = match &result {
        Ok(report) => (
            CompactCliEnvelope {
                ok: true,
                data: Some(report),
                error: None,
            },
            0,
        ),
        Err(error) => (
            CompactCliEnvelope {
                ok: false,
                data: None,
                error: Some(CompactCliError {
                    code: error.code().to_string(),
                    message: error.to_string(),
                    recoverable: true,
                }),
            },
            1,
        ),
    };
    if let Ok(encoded) = serde_json::to_string(&envelope) {
        let _ = writeln!(writer, "{encoded}");
        let _ = writer.flush();
    }
    code
}

async fn compact_cli_inner(
    args: &CliArgs,
    config: &CoreConfig,
) -> Result<CompactionReport, CompactionError> {
    let Some(resume) = args.resume.as_deref() else {
        return Err(CompactionError::Session(SessionError::InvalidRequest {
            message: "--compact requires --resume <session-id>".to_string(),
        }));
    };
    let session_id = ProviderSessionId::parse(resume).map_err(CompactionError::Session)?;

    // Validate the manual/auto argument combination *before* any auth check,
    // provider creation, or engine call, so a malformed invocation fails
    // closed with no side effects. The only legal shapes are:
    //   - manual: neither --expect-generation nor --expect-revision;
    //   - auto (`--auto-compact`): both present, binding the run to an exact
    //     generation and projection revision.
    // Anything else (auto missing a bound, only one bound, or a manual run
    // carrying an expected bound) is a typed invalid_request. Leaving expected
    // unbound would let an automatic run compact without revision binding.
    let (trigger, expected) = match (
        args.auto_compact,
        args.expect_generation,
        args.expect_revision,
    ) {
        (false, None, None) => (CompactionTrigger::Manual, None),
        (true, Some(generation), Some(projection_revision)) => (
            CompactionTrigger::Auto,
            Some(ExpectedRevision {
                generation,
                projection_revision,
            }),
        ),
        _ => {
            return Err(CompactionError::Session(SessionError::InvalidRequest {
                message: "automatic compaction requires both --expect-generation and \
                              --expect-revision; manual compaction must supply neither"
                    .to_string(),
            }));
        }
    };

    let workspace = args
        .workspace
        .as_deref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
        });
    let store = SessionStore::for_workspace(&config.session.persist_dir, &workspace)
        .map_err(CompactionError::Session)?;
    if crate::needs_auth(config) {
        return Err(CompactionError::Provider(
            "provider credentials are not configured; run /auth first".to_string(),
        ));
    }
    let provider = crate::create_provider(config);
    let prefix_tokens = cli_prefix_tokens(config, &workspace);
    let (report, _updated) = compact_session(
        &store,
        &session_id,
        provider.as_ref(),
        config,
        prefix_tokens,
        None,
        trigger,
        expected,
    )
    .await?;
    Ok(report)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::mock::MockProvider;
    use crate::provider::Message;

    fn store(temp: &tempfile::TempDir) -> SessionStore {
        SessionStore::for_workspace(temp.path().join("sessions").to_str().unwrap(), temp.path())
            .expect("session store")
    }

    /// Builds a transcript of `runs` complete Agent runs with bulky content.
    fn bulky_messages(runs: usize) -> Vec<Message> {
        let filler = "log line with useful diagnostics 诊断输出 ".repeat(120);
        let mut messages = Vec::new();
        for run in 0..runs {
            messages.push(Message::user(&format!("prompt {run}: {filler}")));
            messages.push(Message::assistant(&format!("answer {run}: {filler}")));
        }
        messages
    }

    fn persisted(store: &SessionStore, runs: usize) -> PersistedSession {
        let mut session = PersistedSession::new(
            ProviderSessionId::new(),
            store.workspace_scope().to_string(),
            "mock-model".to_string(),
            bulky_messages(runs),
        );
        store.persist(&mut session).expect("persist fixture");
        session
    }

    fn summary_provider() -> MockProvider {
        MockProvider::text_only("## Objective and constraints\n- keep diagnosing memory usage")
    }

    async fn compact(
        store: &SessionStore,
        session: &PersistedSession,
        provider: &dyn ContentGenerator,
        config: &CoreConfig,
    ) -> Result<(CompactionReport, PersistedSession), CompactionError> {
        compact_session(
            store,
            &session.session_id,
            provider,
            config,
            1_000,
            None,
            CompactionTrigger::Manual,
            None,
        )
        .await
    }

    #[tokio::test]
    async fn compact_persist_reload_preserves_identity_and_transcript() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let session = persisted(&store, 4);
        let before = store.inspect(&session.session_id).expect("summary before");

        let provider = summary_provider();
        let config = CoreConfig::default();
        let (report, updated) = compact(&store, &session, &provider, &config)
            .await
            .expect("compaction committed");

        assert_eq!(report.session_id, session.session_id.to_string());
        assert!(report.compacted_through > 0);
        assert!(report.tokens_after.value < report.tokens_before.value);
        assert_eq!(updated.messages.len(), session.messages.len());

        // Simulated process restart: reload from disk.
        let reloaded = store.load(&session.session_id).expect("reload");
        let projection = reloaded.compaction.as_ref().expect("projection persisted");
        assert_eq!(projection.revision, 1);
        assert_eq!(projection.compacted_through, report.compacted_through);
        assert_eq!(reloaded.session_id, session.session_id);
        assert_eq!(reloaded.created_at_ms, session.created_at_ms);
        assert_eq!(reloaded.messages.len(), session.messages.len());

        // Picker metadata stays stable: first prompt and message count.
        let after = store.inspect(&session.session_id).expect("summary after");
        assert_eq!(after.first_prompt, before.first_prompt);
        assert_eq!(after.message_count, before.message_count);

        // The effective context replaces the prefix with one snapshot.
        let effective =
            crate::compaction::effective_messages(&reloaded.messages, reloaded.compaction.as_ref());
        assert!(effective.len() < reloaded.messages.len());
        assert!(effective[0].content.as_text().contains("compacted-history"));
    }

    #[tokio::test]
    async fn sessions_without_projection_load_compatibly() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let session = persisted(&store, 2);
        let loaded = store.load(&session.session_id).expect("load");
        assert!(loaded.compaction.is_none());
        assert_eq!(loaded.messages.len(), 4);
    }

    #[tokio::test]
    async fn provider_error_leaves_previous_projection_untouched() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let session = persisted(&store, 4);
        let config = CoreConfig::default();

        // Commit a first projection.
        let provider = summary_provider();
        compact(&store, &session, &provider, &config)
            .await
            .expect("first compaction");
        let committed = store.load(&session.session_id).unwrap();

        // Grow the transcript so another cut becomes available, then fail.
        let mut grown = committed.clone();
        grown.messages.extend(bulky_messages(3));
        store.persist(&mut grown).expect("grow transcript");
        let failing = MockProvider::partial_error();
        let error = compact(&store, &grown, &failing, &config)
            .await
            .expect_err("provider failure");
        assert_eq!(error.code(), "provider_error");

        let after = store.load(&session.session_id).unwrap();
        assert_eq!(after.compaction, committed.compaction);
    }

    #[tokio::test]
    async fn empty_summary_is_never_committed() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let session = persisted(&store, 4);
        let provider = MockProvider::text_only("   \n ");
        let error = compact(&store, &session, &provider, &CoreConfig::default())
            .await
            .expect_err("empty summary");
        assert_eq!(error.code(), "empty_summary");
        assert!(store
            .load(&session.session_id)
            .unwrap()
            .compaction
            .is_none());
    }

    #[tokio::test]
    async fn inflating_summary_is_rejected_as_not_reducing() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        // Short transcript: three tiny runs, preserve one.
        let mut session = PersistedSession::new(
            ProviderSessionId::new(),
            store.workspace_scope().to_string(),
            "mock-model".to_string(),
            vec![
                Message::user("a"),
                Message::assistant("b"),
                Message::user("c"),
                Message::assistant("d"),
                Message::user("e"),
                Message::assistant("f"),
            ],
        );
        store.persist(&mut session).unwrap();
        let mut config = CoreConfig::default();
        config.session.compaction.preserve_recent_runs = 1;
        let provider = MockProvider::text_only(&"inflated summary ".repeat(1_000));
        let error = compact(&store, &session, &provider, &config)
            .await
            .expect_err("token inflation");
        assert_eq!(error.code(), "not_reducing");
        assert!(store
            .load(&session.session_id)
            .unwrap()
            .compaction
            .is_none());
    }

    #[tokio::test]
    async fn cancelled_stream_is_never_committed() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let session = persisted(&store, 4);
        // Stream ends without MessageEnd, as after a cancel().
        let provider = MockProvider::new(vec![vec![crate::provider::GenerateEvent::TextDelta(
            "partial".to_string(),
        )]]);
        let error = compact(&store, &session, &provider, &CoreConfig::default())
            .await
            .expect_err("cancelled stream");
        assert_eq!(error.code(), "cancelled");
        assert!(store
            .load(&session.session_id)
            .unwrap()
            .compaction
            .is_none());
    }

    /// Provider that concurrently advances the session while summarizing.
    struct ConflictingProvider {
        persist_dir: String,
        workspace: std::path::PathBuf,
        session_id: ProviderSessionId,
        inner: MockProvider,
    }

    #[async_trait::async_trait]
    impl ContentGenerator for ConflictingProvider {
        async fn generate(
            &self,
            messages: &[Message],
            tools: &[crate::provider::ToolDeclaration],
            config: &crate::provider::GenerateConfig,
        ) -> Result<crate::provider::GenerateStream, String> {
            let store = SessionStore::for_workspace(&self.persist_dir, &self.workspace)
                .expect("conflict store");
            let mut session = store.load(&self.session_id).expect("conflict load");
            session.messages.push(Message::user("concurrent turn"));
            session
                .messages
                .push(Message::assistant("concurrent answer"));
            store.persist(&mut session).expect("conflict persist");
            self.inner.generate(messages, tools, config).await
        }

        fn cancel(&self) {}
    }

    #[tokio::test]
    async fn generation_conflict_discards_candidate() {
        let temp = tempfile::tempdir().unwrap();
        let persist_dir = temp.path().join("sessions").to_str().unwrap().to_string();
        let store = store(&temp);
        let session = persisted(&store, 4);
        let provider = ConflictingProvider {
            persist_dir,
            workspace: temp.path().to_path_buf(),
            session_id: session.session_id.clone(),
            inner: summary_provider(),
        };
        let error = compact_session(
            &store,
            &session.session_id,
            &provider,
            &CoreConfig::default(),
            1_000,
            None,
            CompactionTrigger::Auto,
            None,
        )
        .await
        .expect_err("generation conflict");
        assert_eq!(error.code(), "conflict");
        let after = store.load(&session.session_id).unwrap();
        assert!(after.compaction.is_none());
        assert_eq!(after.messages.len(), session.messages.len() + 2);
    }

    /// Panics if the summarizer is ever invoked; proves stale attempts fail
    /// closed before any provider work.
    struct PanicProvider;

    #[async_trait::async_trait]
    impl ContentGenerator for PanicProvider {
        async fn generate(
            &self,
            _messages: &[Message],
            _tools: &[crate::provider::ToolDeclaration],
            _config: &crate::provider::GenerateConfig,
        ) -> Result<crate::provider::GenerateStream, String> {
            panic!("provider must not be called for a stale automatic compaction");
        }

        fn cancel(&self) {}
    }

    async fn compact_expecting(
        store: &SessionStore,
        session: &PersistedSession,
        provider: &dyn ContentGenerator,
        expected: ExpectedRevision,
    ) -> Result<(CompactionReport, PersistedSession), CompactionError> {
        compact_session(
            store,
            &session.session_id,
            provider,
            &CoreConfig::default(),
            1_000,
            None,
            CompactionTrigger::Auto,
            Some(expected),
        )
        .await
    }

    #[tokio::test]
    async fn stale_generation_fails_closed_without_calling_provider() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let session = persisted(&store, 4);
        let error = compact_expecting(
            &store,
            &session,
            &PanicProvider,
            ExpectedRevision {
                generation: session.generation + 5,
                projection_revision: 0,
            },
        )
        .await
        .expect_err("stale generation");
        assert_eq!(error.code(), "conflict");
        assert!(store
            .load(&session.session_id)
            .unwrap()
            .compaction
            .is_none());
    }

    #[tokio::test]
    async fn stale_projection_revision_fails_closed_without_calling_provider() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let session = persisted(&store, 4);
        // The session currently has no projection (revision 0); expecting a
        // later revision means the recommendation is out of date.
        let error = compact_expecting(
            &store,
            &session,
            &PanicProvider,
            ExpectedRevision {
                generation: session.generation,
                projection_revision: 3,
            },
        )
        .await
        .expect_err("stale revision");
        assert_eq!(error.code(), "conflict");
        assert!(store
            .load(&session.session_id)
            .unwrap()
            .compaction
            .is_none());
    }

    #[tokio::test]
    async fn matching_expected_revision_compacts_normally() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let session = persisted(&store, 4);
        let provider = summary_provider();
        let (report, _) = compact_expecting(
            &store,
            &session,
            &provider,
            ExpectedRevision {
                generation: session.generation,
                projection_revision: 0,
            },
        )
        .await
        .expect("matching revision compacts");
        assert_eq!(report.revision, 1);
        assert!(store
            .load(&session.session_id)
            .unwrap()
            .compaction
            .is_some());
    }

    #[tokio::test]
    async fn compact_in_memory_reduces_and_respects_disabled_policy() {
        let messages = bulky_messages(5);
        let provider = summary_provider();
        let config = CoreConfig::default();
        let candidate = compact_in_memory(&messages, None, &provider, "mock-model", &config, 0)
            .await
            .expect("in-memory candidate");
        assert!(candidate.compacted_through > 0);
        assert_eq!(candidate.revision, 1);

        let mut disabled = CoreConfig::default();
        disabled.session.compaction.enabled = false;
        let provider = summary_provider();
        assert!(
            compact_in_memory(&messages, None, &provider, "mock-model", &disabled, 0)
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn single_oversized_run_returns_oversized_input_and_keeps_transcript() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        // Run 0 opens with a user message larger than the whole summarizer
        // input budget, so not even one complete run can be rendered without
        // loss. Run 1 is preserved.
        let giant = "x".repeat(200 * 1024);
        let mut session = PersistedSession::new(
            ProviderSessionId::new(),
            store.workspace_scope().to_string(),
            "mock-model".to_string(),
            vec![
                Message::user(&giant),
                Message::assistant("ok"),
                Message::user("second"),
                Message::assistant("done"),
            ],
        );
        store.persist(&mut session).unwrap();
        let mut config = CoreConfig::default();
        config.session.compaction.preserve_recent_runs = 1;
        let provider = summary_provider();
        let error = compact(&store, &session, &provider, &config)
            .await
            .expect_err("oversized single run");
        assert_eq!(error.code(), "oversized_input");
        // The provider is never allowed to commit a lossy projection.
        let after = store.load(&session.session_id).unwrap();
        assert!(after.compaction.is_none());
        assert_eq!(after.messages.len(), session.messages.len());
    }

    #[tokio::test]
    async fn run_exceeding_small_model_window_fails_closed_without_byte_cap_help() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        // ~48 KiB per run: far below the 192 KiB byte ceiling, but well over
        // the token budget of a 4K-window model — only the model-aware bound
        // can reject this request.
        let session = persisted(&store, 4);
        let mut config = CoreConfig::default();
        config.session.compaction.preserve_recent_runs = 1;
        config.session.compaction.model_context_window = Some(4_000);
        let provider = summary_provider();
        let error = compact(&store, &session, &provider, &config)
            .await
            .expect_err("run larger than the model window");
        assert_eq!(error.code(), "oversized_input");
        let after = store.load(&session.session_id).unwrap();
        assert!(after.compaction.is_none());
        assert_eq!(after.messages.len(), session.messages.len());
    }

    #[test]
    fn cut_shrinks_to_largest_run_boundary_inside_the_token_budget() {
        // Three bulky runs (six messages, ~1.5K tokens per message); a budget
        // that fits only four messages must shrink the cut to the run
        // boundary at message 4 — never mid-run, never mid-message.
        let messages = bulky_messages(3);
        let (cut, input) = bounded_cut_and_input(&messages, None, 0, 6, 6_500).expect("shrunk cut");
        assert_eq!(cut, 4);
        // The rendered input covers exactly the committed range.
        assert!(input.contains("prompt 1:"));
        assert!(!input.contains("prompt 2:"));
    }

    #[test]
    fn single_run_over_token_budget_is_oversized_input() {
        let messages = bulky_messages(2);
        let error =
            bounded_cut_and_input(&messages, None, 0, 2, 1_000).expect_err("one run too large");
        assert!(matches!(error, CompactionError::OversizedInput));
    }

    /// Records the estimated token size of every summary request it serves.
    struct CapturingProvider {
        inner: MockProvider,
        captured: std::sync::Arc<std::sync::Mutex<Vec<u64>>>,
    }

    #[async_trait::async_trait]
    impl ContentGenerator for CapturingProvider {
        async fn generate(
            &self,
            messages: &[Message],
            tools: &[crate::provider::ToolDeclaration],
            config: &crate::provider::GenerateConfig,
        ) -> Result<crate::provider::GenerateStream, String> {
            self.captured
                .lock()
                .expect("captured requests lock")
                .push(estimate_messages_tokens(messages));
            self.inner.generate(messages, tools, config).await
        }

        fn cancel(&self) {}
    }

    #[tokio::test]
    async fn summary_request_never_exceeds_a_small_model_window() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        // A large recovered session on a 12K-window model: the request that
        // carries the summary input must itself fit the window with the
        // output reserve subtracted.
        let session = persisted(&store, 8);
        let mut config = CoreConfig::default();
        config.session.compaction.preserve_recent_runs = 1;
        config.session.compaction.model_context_window = Some(12_000);
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let provider = CapturingProvider {
            inner: summary_provider(),
            captured: std::sync::Arc::clone(&captured),
        };
        let (report, _) = compact(&store, &session, &provider, &config)
            .await
            .expect("bounded compaction");
        assert!(report.compacted_through > 0);
        let requests = captured.lock().expect("captured requests lock");
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0] + 2_048 <= 12_000,
            "summary request of ~{} tokens exceeds the 12K window",
            requests[0]
        );
    }

    #[tokio::test]
    async fn nothing_to_compact_for_short_sessions() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let session = persisted(&store, 2);
        let provider = summary_provider();
        let error = compact(&store, &session, &provider, &CoreConfig::default())
            .await
            .expect_err("too few runs");
        assert_eq!(error.code(), "nothing_to_compact");
    }
}
