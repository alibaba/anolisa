use crate::audit::AuditEntry;
use crate::error::{MemoryError, Result};
use crate::index::SearchHit;
use crate::service::MemoryService;

const TOOL: &str = "memory_search";

/// Tier B: search the memory store.
///
/// `mode` controls the search algorithm:
/// - `"bm25"` (default): FTS5 keyword search.
/// - `"vector"`: dense embedding cosine similarity (requires embedding config).
/// - `"hybrid"`: reciprocal rank fusion of BM25 + vector results.
///
/// Returns up to `top_k` ranked snippets. Errors with `NotImplemented` if the
/// index worker isn't running, or if `mode=vector|hybrid` is requested without
/// an embedding provider.
pub fn memory_search(
    svc: &MemoryService,
    query: &str,
    top_k: usize,
    mode: Option<&str>,
) -> Result<Vec<SearchHit>> {
    let mode = mode.unwrap_or("bm25");
    let index = match svc.index.as_ref() {
        Some(i) => i,
        None => {
            let err = MemoryError::NotImplemented(
                "index disabled; enable [memory.index].enabled or use mem_grep instead",
            );
            svc.audit_log(AuditEntry::new(TOOL).error(err.to_string()));
            return Err(err);
        }
    };

    match mode {
        "bm25" => {
            let hits = index.search(query, top_k.max(1))?;
            svc.audit_log(
                AuditEntry::new(TOOL)
                    .path(format!("bm25:{:.120}", query))
                    .bytes(hits.len() as u64),
            );
            Ok(hits)
        }
        "vector" | "hybrid" => {
            let emb = match svc.embedding.as_ref() {
                Some(e) => e,
                None => {
                    // Graceful fallback: when no embedding provider is
                    // configured, vector/hybrid silently degrades to BM25
                    // so that callers (auto-recall, corpus supplement) can
                    // safely request hybrid without checking config first.
                    tracing::debug!(
                        "{mode} requested but no embedding provider — falling back to bm25"
                    );
                    let hits = index.search(query, top_k.max(1))?;
                    svc.audit_log(
                        AuditEntry::new(TOOL)
                            .path(format!("bm25(fallback from {mode}):{:.120}", query))
                            .bytes(hits.len() as u64),
                    );
                    return Ok(hits);
                }
            };

            // FIXME: this blocks the tokio worker. In production the
            // embedding call should be spawned on a dedicated blocking
            // thread or use a channel-based async bridge. For now, the
            // MCP server is single-client stdio so the block_on cost is
            // bounded by the embedding provider's HTTP timeout.
            //
            // Use try_current() so tests (which run outside a tokio
            // runtime) get a clean error instead of a panic.
            let rt = tokio::runtime::Handle::try_current().map_err(|_| {
                MemoryError::NotImplemented(
                    "embedding requires a tokio runtime; tests should use #[tokio::test]",
                )
            })?;
            let embedding = rt.block_on(emb.embed(query)).map_err(|e| {
                svc.audit_log(
                    AuditEntry::new(TOOL)
                        .path(format!("embed:{:.120}", query))
                        .error(e.to_string()),
                );
                MemoryError::Other(format!("embedding failed: {e}"))
            })?;

            let hits = if mode == "vector" {
                index.search_vec(&embedding.vector, top_k.max(1))?
            } else {
                index.search_hybrid(query, &embedding.vector, top_k.max(1))?
            };

            svc.audit_log(
                AuditEntry::new(TOOL)
                    .path(format!("{mode}:{:.120}", query))
                    .bytes(hits.len() as u64),
            );
            Ok(hits)
        }
        unknown => Err(MemoryError::InvalidArgument(format!(
            "unknown search mode '{unknown}'; expected bm25, vector, or hybrid"
        ))),
    }
}
