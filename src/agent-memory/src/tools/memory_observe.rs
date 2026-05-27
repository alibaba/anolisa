use chrono::Utc;

use crate::audit::AuditEntry;
use crate::error::Result;
use crate::service::MemoryService;

const TOOL: &str = "memory_observe";

/// Tier B: record an observation. The OS picks a stable filename under
/// `notes/observed/<ulid>.md` and writes a minimal frontmatter + body.
/// Returns the relative path so the model can later read or move it.
pub fn memory_observe(svc: &MemoryService, content: &str, hint: Option<&str>) -> Result<String> {
    let ulid = ulid::Ulid::new();
    let path = format!("notes/observed/{ulid}.md");

    let mut body = String::new();
    body.push_str("---\n");
    if let Some(h) = hint {
        // hint is sanitized for TOML frontmatter safety (newlines break syntax);
        // content goes into the markdown body (not frontmatter) so no TOML sanitization needed.
        let safe = h.replace('\n', " ");
        body.push_str(&format!("hint: {safe}\n"));
    }
    body.push_str(&format!("created_at: {}\n", Utc::now().to_rfc3339()));
    body.push_str("---\n\n");
    body.push_str(content);
    if !content.ends_with('\n') {
        body.push('\n');
    }

    // svc.write emits its own mem_write audit entry, and on failure that
    // entry already carries the path + error. We only add a memory_observe
    // entry on success so the high-level intent is visible in the session
    // log; failure paths are NOT double-audited (the underlying mem_write
    // entry is the single source of truth for the error).
    let n = svc.write(&path, &body, false)?;
    svc.audit_log(AuditEntry::new(TOOL).path(path.clone()).bytes(n));
    Ok(path)
}
