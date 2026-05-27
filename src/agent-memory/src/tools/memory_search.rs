use crate::audit::AuditEntry;
use crate::error::{MemoryError, Result};
use crate::index::SearchHit;
use crate::service::MemoryService;

const TOOL: &str = "memory_search";

/// Tier B: structured BM25 search against the running index.
///
/// Returns up to `top_k` ranked snippets. Errors with `NotImplemented` if the
/// index worker isn't running (profile=expert or boot failure).
pub fn memory_search(svc: &MemoryService, query: &str, top_k: usize) -> Result<Vec<SearchHit>> {
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

    match index.search(query, top_k.max(1)) {
        Ok(hits) => {
            svc.audit_log(
                AuditEntry::new(TOOL)
                    .path(query.chars().take(120).collect::<String>())
                    .bytes(hits.len() as u64),
            );
            Ok(hits)
        }
        Err(e) => {
            svc.audit_log(AuditEntry::new(TOOL).error(e.to_string()));
            Err(e)
        }
    }
}
