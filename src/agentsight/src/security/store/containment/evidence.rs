//! Correlates containment binding evidence back to its originating risk case.

use rusqlite::{OptionalExtension, TransactionBehavior, params};
use uuid::Uuid;

use super::super::{SecurityStore, SecurityStoreError, parse_uuid, sqlite_time};

impl SecurityStore {
    /// Returns the original risk case for a durable containment binding.
    ///
    /// # Errors
    ///
    /// Returns a typed database, stored-data, or lock error.
    pub fn case_id_for_containment_binding(
        &self,
        binding_id: Uuid,
    ) -> Result<Option<Uuid>, SecurityStoreError> {
        self.connection()?
            .query_row(
                "SELECT case_id FROM containment_actions WHERE binding_id = ?1",
                [binding_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|case_id| parse_uuid(&case_id))
            .transpose()
    }

    /// Appends unique containment evidence and upgrades confirmed denial risk.
    ///
    /// Allowed decisions may raise the risk score but cannot mark the case
    /// blocked or critical. Existing blocked state is never downgraded.
    ///
    /// # Errors
    ///
    /// Returns a typed case, database, timestamp, stored-data, or lock error.
    pub fn append_containment_evidence(
        &self,
        case_id: Uuid,
        evidence_ids: &[Uuid],
        risk_score: u8,
        blocked: bool,
        occurred_at_ns: u64,
    ) -> Result<(), SecurityStoreError> {
        let occurred_at_ns = sqlite_time(occurred_at_ns)?;
        let mut conn = self.connection()?;
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
            "UPDATE risk_cases SET
                 severity = CASE WHEN ?1 THEN 'critical' ELSE severity END,
                 risk_score = MAX(risk_score, ?2),
                 blocked = CASE WHEN ?1 THEN 1 ELSE blocked END,
                 updated_at_ns = MAX(updated_at_ns, ?3)
             WHERE case_id = ?4",
            params![
                blocked,
                i64::from(risk_score),
                occurred_at_ns,
                case_id.to_string()
            ],
        )?;
        if changed != 1 {
            return Err(SecurityStoreError::MissingCase(case_id));
        }
        let mut next_position: i64 = transaction.query_row(
            "SELECT COALESCE(MAX(position) + 1, 0)
             FROM risk_evidence_links WHERE case_id = ?1",
            [case_id.to_string()],
            |row| row.get(0),
        )?;
        for event_id in evidence_ids {
            let inserted = transaction.execute(
                "INSERT OR IGNORE INTO risk_evidence_links (case_id, event_id, position)
                 VALUES (?1, ?2, ?3)",
                params![case_id.to_string(), event_id.to_string(), next_position],
            )?;
            if inserted == 1 {
                next_position += 1;
            }
        }
        transaction.commit()?;
        Ok(())
    }
}
