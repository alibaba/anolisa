# Session Compaction

[中文版](../../../../zh/user-entrypoint/cosh-ng/shell/session-compaction.md)

Session compaction reduces the conversation history sent to the model without
deleting the original transcript. It is useful for long operational sessions
whose command output and Agent exchanges accumulate substantial context.

## Compact a Session Manually

Run this command at the shell prompt.

```text
/session compact
```

Manual compaction starts immediately in the background and does not depend on
the automatic token threshold or `preserve_recent_runs`. The native shell
remains usable while Agent requests pause until compaction finishes.

A single completed Agent run is enough. The compactor prefers an earlier safe
boundary when that already meets the target, but it may summarize through the
latest completed run when necessary. It never includes an incomplete user
turn or unfinished tool exchange in the summarized prefix.

Use these commands while the job is active.

```text
/session compact status
/session compact cancel
```

Cancellation leaves both the complete transcript and the current projection
unchanged.

## Automatic and Emergency Compaction

Automatic compaction is evaluated after an Agent run reaches an idle boundary.
By default it starts after model-visible history exceeds 70% of the usable
model window and aims for at most 30%. It retains the two most recent complete
Agent runs verbatim, so the first automatic compaction normally needs at least
three complete runs.

Crossing the threshold alone is not sufficient. If no new safe prefix exists,
Core waits for another complete run instead of starting a background job that
would fail with `nothing_to_compact`.

At 90% of the usable window, Core runs synchronous protection before the next
provider request. It starts with the configured recent-run protection. If the
safe boundary cannot reclaim enough space, Core retries with one completed run
protected and then with the same boundary used by manual compaction. The active
run remains verbatim throughout. A failed compaction attempt or an exhausted
set of safe boundaries returns a typed context-limit error before Core sends an
oversized request.

## Data and Safety Guarantees

- The persisted transcript remains complete and append-only.
- A versioned summary projection changes only the context sent to the model.
- Compaction never splits a tool call from its results.
- Provider work runs without holding the session-store lock.
- Generation, digest, and revision checks reject stale commits.
- A summary is committed only when it reduces the effective context.
- Provider failure, cancellation, or invalid output leaves the prior
  projection unchanged.

Compaction may still report an actionable failure when no complete prefix
exists, one run exceeds the summarizer input budget, authentication or the
provider fails, the session changes concurrently, or the generated summary
does not reduce context.

## Configuration

```toml
[session.compaction]
enabled = true
auto = true
trigger_ratio = 0.70
emergency_ratio = 0.90
target_ratio = 0.30
preserve_recent_runs = 2

# Optional model-specific overrides:
# auto_compact_token_limit = 89600
# model_context_window = 128000
# model_max_output_tokens = 8192
```

`preserve_recent_runs` controls normal automatic compaction. Emergency
protection starts with the same value, then follows the fallback sequence
described above. An explicit `/session compact` starts at the final policy and
may summarize the latest completed run. None of these paths summarizes the
active run.

`model_max_output_tokens` is both the reply space reserved from the model window
and the `max_tokens` cap sent to the provider. When it is unset, a known model
uses `min(model output capability, 16384)` and an unknown model uses `4096`.
Core limits either value to half of the resolved context window. Lowering the
setting leaves more room for history and also shortens the longest reply. See
[Configuration](../configuration.md) for the complete setting reference.
