//! Emergency context preflight: the fail-closed guard executed before every
//! provider request while the emergency threshold is crossed.

use std::io::Write;

use crate::config::CoreConfig;
use crate::protocol::OutputMessage;
use crate::provider::ContentGenerator;
use crate::provider::Message;

use super::budget::{ContextBudget, ModelCapability};
use super::engine::compact_in_memory;
use super::runtime::CompactionRuntime;

/// Stable prefix identifying typed context-limit turn failures.
pub(crate) const CONTEXT_LIMIT_ERROR_PREFIX: &str = "context_limit:";

/// Emergency context preflight executed before every provider request.
///
/// When the next request would cross the emergency threshold, compacts
/// synchronously at this complete exchange boundary and commits the result
/// into `runtime`. Returns a typed `context_limit:` error instead of
/// submitting an oversized request when no safe split reclaims enough
/// context.
///
/// # Errors
///
/// Returns a `context_limit:`-prefixed message when the effective context
/// still exceeds the emergency threshold after the in-memory compaction
/// attempt.
pub(crate) async fn run_context_preflight<W: Write>(
    runtime: &mut CompactionRuntime,
    messages: &[Message],
    provider: &dyn ContentGenerator,
    model: &str,
    config: &CoreConfig,
    prefix_tokens: u64,
    writer: &mut W,
) -> Result<(), String> {
    let policy = &config.session.compaction;
    if !policy.enabled {
        return Ok(());
    }
    let capability = ModelCapability::resolve(policy, config.agent.session_token_limit, model);
    let budget = ContextBudget::compute(capability, prefix_tokens, policy);
    if !budget.over_emergency(runtime.effective_history_tokens(messages, prefix_tokens)) {
        return Ok(());
    }
    emit_status(writer, "compaction_emergency_started");
    let candidate = compact_in_memory(
        messages,
        runtime.state(),
        provider,
        model,
        config,
        budget.target_tokens,
    )
    .await;
    if let Some(candidate) = candidate {
        runtime.commit_state(candidate);
    }
    let history_tokens = runtime.effective_history_tokens(messages, prefix_tokens);
    if budget.over_emergency(history_tokens) {
        emit_status(writer, "compaction_emergency_failed");
        return Err(format!(
            "{CONTEXT_LIMIT_ERROR_PREFIX} effective context (~{history_tokens} tokens) \
             exceeds the emergency threshold ({} of {} usable history tokens) and no safe \
             split reclaimed enough context; compact manually or start a new session",
            budget.emergency_tokens, budget.usable_history
        ));
    }
    emit_status(writer, "compaction_emergency_completed");
    Ok(())
}

/// Writes one system-status line to the protocol stream, mirroring
/// `CoshCore::emit`'s best-effort serialization.
fn emit_status<W: Write>(writer: &mut W, status: &str) {
    if let Ok(json) = serde_json::to_string(&OutputMessage::system_status(status)) {
        let _ = writeln!(writer, "{json}");
        let _ = writer.flush();
    }
}
