//! Compare-and-swap transitions used by containment reconciliation workers.

use rusqlite::params;

use super::super::{SecurityStore, SecurityStoreError, sqlite_time};
use super::{ACTION_COLUMNS, containment_action_from_row, containment_row};
use crate::security::{ContainmentAction, ContainmentLifecycle};

const RECONCILE_CLAIM_LEASE_NS: u64 = 1_000_000_000;

impl SecurityStore {
    /// Lists a bounded batch with reached expiry or explicit retry metadata.
    ///
    /// # Errors
    ///
    /// Returns a typed database, timestamp, stored-data, or lock error.
    pub fn due_containment_actions(
        &self,
        now_ns: u64,
        limit: usize,
    ) -> Result<Vec<ContainmentAction>, SecurityStoreError> {
        let conn = self.connection()?;
        let limit = limit.clamp(1, 1_000);
        let mut statement = conn.prepare(&format!(
            "SELECT {ACTION_COLUMNS}
             FROM containment_actions
             WHERE (lifecycle_state = 'pending' AND (
                    (duration_secs IS NOT NULL
                     AND expires_at_ns IS NOT NULL
                     AND expires_at_ns <= ?1)
                    OR (next_retry_at_ns IS NOT NULL AND next_retry_at_ns <= ?1)))
                OR (lifecycle_state = 'active'
                    AND duration_secs IS NOT NULL
                    AND expires_at_ns IS NOT NULL
                    AND expires_at_ns <= ?1)
                OR (lifecycle_state = 'expiring' AND (
                    (next_retry_at_ns IS NOT NULL AND next_retry_at_ns <= ?1)
                    OR (next_retry_at_ns IS NULL
                        AND duration_secs IS NOT NULL
                        AND expires_at_ns IS NOT NULL
                        AND expires_at_ns <= ?1)))
                OR (lifecycle_state NOT IN ('pending', 'active', 'expiring', 'expired', 'failed')
                    AND ((duration_secs IS NOT NULL
                          AND expires_at_ns IS NOT NULL
                          AND expires_at_ns <= ?1)
                         OR (next_retry_at_ns IS NOT NULL AND next_retry_at_ns <= ?1)))
             ORDER BY COALESCE(next_retry_at_ns, expires_at_ns, created_at_ns) ASC,
                      action_id ASC
             LIMIT ?2"
        ))?;
        let rows = statement.query_map(
            params![sqlite_time(now_ns)?, i64::try_from(limit).unwrap_or(1_000)],
            containment_row,
        )?;
        let mut actions = Vec::with_capacity(limit);
        for row in rows {
            actions.push(containment_action_from_row(row?)?);
        }
        Ok(actions)
    }

    /// Claims one due action without allowing stale coordinators to duplicate work.
    ///
    /// Active actions become durably expiring in the same compare-and-swap.
    ///
    /// # Errors
    ///
    /// Returns a typed database, timestamp, or lock error.
    pub(crate) fn claim_containment_reconciliation(
        &self,
        action: &ContainmentAction,
        now_ns: u64,
    ) -> Result<Option<ContainmentAction>, SecurityStoreError> {
        let mut claimed = action.clone();
        let claimed_at_ns = now_ns.max(action.updated_at_ns.saturating_add(1));
        claimed.updated_at_ns = claimed_at_ns;
        claimed.next_retry_at_ns = Some(claimed_at_ns.saturating_add(RECONCILE_CLAIM_LEASE_NS));
        if claimed.lifecycle_state == ContainmentLifecycle::Active {
            claimed.lifecycle_state = ContainmentLifecycle::Expiring;
        }
        let changed = self.connection()?.execute(
            "UPDATE containment_actions
             SET lifecycle_state = ?1, next_retry_at_ns = ?2, updated_at_ns = ?3
             WHERE action_id = ?4 AND lifecycle_state = ?5 AND updated_at_ns = ?6
               AND ((lifecycle_state = 'pending' AND (
                     (duration_secs IS NOT NULL
                      AND expires_at_ns IS NOT NULL
                      AND expires_at_ns <= ?7)
                     OR (next_retry_at_ns IS NOT NULL AND next_retry_at_ns <= ?7)))
                    OR (lifecycle_state = 'active'
                        AND duration_secs IS NOT NULL
                        AND expires_at_ns IS NOT NULL
                        AND expires_at_ns <= ?7)
                    OR (lifecycle_state = 'expiring' AND (
                        (next_retry_at_ns IS NOT NULL AND next_retry_at_ns <= ?7)
                        OR (next_retry_at_ns IS NULL
                            AND duration_secs IS NOT NULL
                            AND expires_at_ns IS NOT NULL
                            AND expires_at_ns <= ?7))))",
            params![
                lifecycle_value(claimed.lifecycle_state),
                sqlite_time(claimed.next_retry_at_ns.unwrap_or(claimed_at_ns))?,
                sqlite_time(claimed_at_ns)?,
                action.action_id.to_string(),
                lifecycle_value(action.lifecycle_state),
                sqlite_time(action.updated_at_ns)?,
                sqlite_time(now_ns)?,
            ],
        )?;
        Ok((changed == 1).then_some(claimed))
    }

    /// Finishes lifecycle mutation only for the worker holding the latest claim.
    ///
    /// # Errors
    ///
    /// Returns a typed database, timestamp, unsigned-value, or lock error.
    pub(crate) fn finish_containment_reconciliation(
        &self,
        action: &ContainmentAction,
        claimed_lifecycle: ContainmentLifecycle,
        claimed_at_ns: u64,
    ) -> Result<bool, SecurityStoreError> {
        let changed = self.connection()?.execute(
            "UPDATE containment_actions SET
                 lifecycle_state = ?1,
                 failure_stage = ?2,
                 failure_reason = ?3,
                 attempt_count = ?4,
                 next_retry_at_ns = ?5,
                 updated_at_ns = ?6
             WHERE action_id = ?7 AND lifecycle_state = ?8 AND updated_at_ns = ?9",
            params![
                lifecycle_value(action.lifecycle_state),
                action.failure_stage.map(super::failure_stage_value),
                action.failure_reason,
                i64::from(action.attempt_count),
                action.next_retry_at_ns.map(sqlite_time).transpose()?,
                sqlite_time(action.updated_at_ns)?,
                action.action_id.to_string(),
                lifecycle_value(claimed_lifecycle),
                sqlite_time(claimed_at_ns)?,
            ],
        )?;
        Ok(changed == 1)
    }
}

fn lifecycle_value(value: ContainmentLifecycle) -> &'static str {
    match value {
        ContainmentLifecycle::Pending => "pending",
        ContainmentLifecycle::Active => "active",
        ContainmentLifecycle::Expiring => "expiring",
        ContainmentLifecycle::Expired => "expired",
        ContainmentLifecycle::Failed => "failed",
    }
}
