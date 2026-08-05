//! Transactional retention for complete security case graphs.

use rusqlite::{TransactionBehavior, params};

use super::{AuditError, AuditStore, sqlite_time};

const PURGEABLE_CASES: &str = "SELECT cases.case_id
     FROM risk_cases AS cases
     WHERE cases.updated_at_ns < ?1
       AND cases.status IN ('false_positive', 'accepted_risk', 'resolved')
       AND NOT EXISTS (
           SELECT 1
           FROM containment_actions AS actions
           WHERE actions.case_id = cases.case_id
             AND (
                 actions.lifecycle_state IN ('pending', 'active', 'expiring')
                 OR actions.updated_at_ns >= ?1
             )
       )";

impl AuditStore {
    /// Deletes expired events and complete inactive case graphs before `cutoff_ns`.
    ///
    /// Evidence shared with a retained case and every graph with live containment
    /// remain intact. The returned count includes every deleted graph row.
    ///
    /// # Errors
    ///
    /// Returns a typed database, timestamp, or lock error.
    pub fn purge_before(&self, cutoff_ns: u64) -> Result<u64, AuditError> {
        let cutoff_ns = sqlite_time(cutoff_ns)?;
        let mut conn = self.connection()?;
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut deleted = 0_u64;

        deleted += transaction.execute(
            &format!("DELETE FROM containment_actions WHERE case_id IN ({PURGEABLE_CASES})"),
            params![cutoff_ns],
        )? as u64;
        deleted += transaction.execute(
            &format!("DELETE FROM risk_evidence_links WHERE case_id IN ({PURGEABLE_CASES})"),
            params![cutoff_ns],
        )? as u64;
        deleted += transaction.execute(
            &format!("DELETE FROM risk_cases WHERE case_id IN ({PURGEABLE_CASES})"),
            params![cutoff_ns],
        )? as u64;
        deleted += transaction.execute(
            "DELETE FROM security_events
             WHERE occurred_at_ns < ?1
               AND NOT EXISTS (
                   SELECT 1 FROM risk_evidence_links AS links
                   WHERE links.event_id = security_events.event_id
               )",
            params![cutoff_ns],
        )? as u64;

        transaction.commit()?;
        Ok(deleted)
    }
}
