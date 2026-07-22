//! SQLite reads and writes for durable containment lifecycle state.

mod evidence;
mod reconcile;

pub(crate) use reconcile::DueContainmentAction;

use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};
use uuid::Uuid;

use super::{SecurityStore, SecurityStoreError, parse_status, parse_uuid, sqlite_time, unsigned};
use crate::security::{
    ContainmentAction, ContainmentFailureStage, ContainmentLifecycle, RiskCaseStatus,
};

const ACTION_COLUMNS: &str = "action_id, case_id, binding_id, agent_id, root_pid,
    process_start_time, source_path, duration_secs, expires_at_ns, lifecycle_state,
    blocked_at_ns, requested_by, failure_stage, failure_reason, attempt_count,
    next_retry_at_ns, created_at_ns, updated_at_ns";

/// Result of atomically claiming the one live containment slot for a case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContainmentClaimResult {
    /// The supplied pending action now owns the case-level claim.
    Claimed,
    /// Another pending, active, or expiring action already owns the claim.
    Existing(Box<ContainmentAction>),
    /// Human review made the case ineligible before the claim was persisted.
    CaseIneligible(RiskCaseStatus),
}

/// Result of atomically activating an action and confirming its case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainmentActivationResult {
    /// The action is active and an open case is confirmed.
    Activated,
    /// Human review made the case ineligible while enforcement was applying.
    CaseIneligible(RiskCaseStatus),
    /// Another worker replaced the supplied lifecycle claim.
    LostClaim,
}

impl SecurityStore {
    /// Inserts one containment action, returning false when any constraint conflict is ignored.
    ///
    /// # Errors
    ///
    /// Returns a typed database, unsigned-value, or lock error.
    pub fn insert_containment_action(
        &self,
        action: &ContainmentAction,
    ) -> Result<bool, SecurityStoreError> {
        let conn = self.connection()?;
        let changed = insert_action(&conn, action)?;
        Ok(changed == 1)
    }

    /// Atomically claims the one pending/active/expiring action slot for a case.
    ///
    /// # Errors
    ///
    /// Returns a typed database, unsigned-value, case, stored-data, or lock error.
    pub fn claim_containment_action(
        &self,
        action: &ContainmentAction,
    ) -> Result<ContainmentClaimResult, SecurityStoreError> {
        if action.lifecycle_state != ContainmentLifecycle::Pending {
            return Err(SecurityStoreError::InvalidData(
                "a containment claim must start pending".into(),
            ));
        }
        let mut conn = self.connection()?;
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let status = transaction
            .query_row(
                "SELECT status FROM risk_cases WHERE case_id = ?1",
                [action.case_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or(SecurityStoreError::MissingCase(action.case_id))?;
        let status = parse_status(&status)?;
        if !matches!(status, RiskCaseStatus::Open | RiskCaseStatus::Confirmed) {
            transaction.commit()?;
            return Ok(ContainmentClaimResult::CaseIneligible(status));
        }
        let changed = insert_action(&transaction, action)?;
        let result = if changed == 1 {
            ContainmentClaimResult::Claimed
        } else {
            let existing = live_action(&transaction, action.case_id)?.ok_or_else(|| {
                SecurityStoreError::InvalidData(format!(
                    "containment claim {} conflicted without a live case action",
                    action.action_id
                ))
            })?;
            ContainmentClaimResult::Existing(Box::new(existing))
        };
        transaction.commit()?;
        Ok(result)
    }

    /// Atomically activates a pending action and confirms an open case.
    ///
    /// # Errors
    ///
    /// Returns a typed database, timestamp, case, stored-data, or lock error.
    pub fn activate_containment_action(
        &self,
        action_id: Uuid,
        claimed_at_ns: u64,
        updated_at_ns: u64,
    ) -> Result<ContainmentActivationResult, SecurityStoreError> {
        let updated_at_ns = updated_at_ns.max(claimed_at_ns.saturating_add(1));
        let claimed_at_ns = sqlite_time(claimed_at_ns)?;
        let updated_at_ns = sqlite_time(updated_at_ns)?;
        let mut conn = self.connection()?;
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let action = transaction
            .query_row(
                "SELECT case_id, lifecycle_state, updated_at_ns
                 FROM containment_actions WHERE action_id = ?1",
                [action_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| {
                SecurityStoreError::InvalidData(format!(
                    "containment action {action_id} does not exist"
                ))
            })?;
        let case_id = parse_uuid(&action.0)?;
        if parse_lifecycle(&action.1)? != ContainmentLifecycle::Pending || action.2 != claimed_at_ns
        {
            transaction.commit()?;
            return Ok(ContainmentActivationResult::LostClaim);
        }
        let status = transaction
            .query_row(
                "SELECT status FROM risk_cases WHERE case_id = ?1",
                [action.0.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or(SecurityStoreError::MissingCase(case_id))?;
        let status = parse_status(&status)?;
        if !matches!(status, RiskCaseStatus::Open | RiskCaseStatus::Confirmed) {
            transaction.commit()?;
            return Ok(ContainmentActivationResult::CaseIneligible(status));
        }
        let activated = transaction.execute(
            "UPDATE containment_actions
             SET lifecycle_state = 'active', failure_stage = NULL, failure_reason = NULL,
                 next_retry_at_ns = NULL, updated_at_ns = ?1
             WHERE action_id = ?2 AND lifecycle_state = 'pending' AND updated_at_ns = ?3",
            params![updated_at_ns, action_id.to_string(), claimed_at_ns],
        )?;
        if activated != 1 {
            transaction.commit()?;
            return Ok(ContainmentActivationResult::LostClaim);
        }
        if status == RiskCaseStatus::Open {
            let confirmed = transaction.execute(
                "UPDATE risk_cases SET status = 'confirmed', updated_at_ns = ?1
                 WHERE case_id = ?2 AND status = 'open'",
                params![updated_at_ns, action.0],
            )?;
            if confirmed != 1 {
                return Err(SecurityStoreError::InvalidData(format!(
                    "risk case {case_id} changed before confirmation"
                )));
            }
        }
        transaction.commit()?;
        Ok(ContainmentActivationResult::Activated)
    }

    /// Loads one containment action by its stable action ID.
    ///
    /// # Errors
    ///
    /// Returns a typed database, stored-data, or lock error.
    pub fn containment_action(
        &self,
        action_id: Uuid,
    ) -> Result<Option<ContainmentAction>, SecurityStoreError> {
        let conn = self.connection()?;
        let mut statement = conn.prepare(&format!(
            "SELECT {ACTION_COLUMNS} FROM containment_actions WHERE action_id = ?1"
        ))?;
        statement
            .query_row([action_id.to_string()], containment_row)
            .optional()?
            .map(containment_action_from_row)
            .transpose()
    }

    /// Loads the newest containment action for one case.
    ///
    /// # Errors
    ///
    /// Returns a typed database, stored-data, or lock error.
    pub fn latest_containment_action(
        &self,
        case_id: Uuid,
    ) -> Result<Option<ContainmentAction>, SecurityStoreError> {
        let conn = self.connection()?;
        let mut statement = conn.prepare(&format!(
            "SELECT {ACTION_COLUMNS}
             FROM containment_actions
             WHERE case_id = ?1
             ORDER BY created_at_ns DESC, action_id ASC
             LIMIT 1"
        ))?;
        statement
            .query_row([case_id.to_string()], containment_row)
            .optional()?
            .map(containment_action_from_row)
            .transpose()
    }

    /// Persists the current mutable lifecycle fields for an action.
    ///
    /// An existing first-block timestamp is never cleared or replaced.
    ///
    /// # Errors
    ///
    /// Returns a typed database, unsigned-value, or lock error.
    pub fn update_containment_action(
        &self,
        action: &ContainmentAction,
    ) -> Result<bool, SecurityStoreError> {
        let changed = self.connection()?.execute(
            "UPDATE containment_actions SET
                lifecycle_state = ?1,
                blocked_at_ns = COALESCE(blocked_at_ns, ?2),
                failure_stage = ?3,
                failure_reason = ?4,
                attempt_count = ?5,
                next_retry_at_ns = ?6,
                expires_at_ns = ?7,
                duration_secs = ?8,
                updated_at_ns = ?9
             WHERE action_id = ?10",
            params![
                lifecycle_value(action.lifecycle_state),
                action.blocked_at_ns.map(sqlite_time).transpose()?,
                action.failure_stage.map(failure_stage_value),
                action.failure_reason,
                i64::from(action.attempt_count),
                action.next_retry_at_ns.map(sqlite_time).transpose()?,
                action.expires_at_ns.map(sqlite_time).transpose()?,
                action.duration_secs.map(sqlite_time).transpose()?,
                sqlite_time(action.updated_at_ns)?,
                action.action_id.to_string(),
            ],
        )?;
        Ok(changed == 1)
    }

    /// Records the earliest confirmed kernel denial for a containment binding.
    ///
    /// Duplicate or reordered calls retain the smallest occurrence timestamp.
    ///
    /// # Errors
    ///
    /// Returns a typed database, timestamp, or lock error.
    pub fn mark_containment_blocked(
        &self,
        binding_id: Uuid,
        blocked_at_ns: u64,
    ) -> Result<bool, SecurityStoreError> {
        let blocked_at_ns = sqlite_time(blocked_at_ns)?;
        let changed = self.connection()?.execute(
            "UPDATE containment_actions
             SET blocked_at_ns = CASE
                     WHEN blocked_at_ns IS NULL OR blocked_at_ns > ?1 THEN ?1
                     ELSE blocked_at_ns
                 END,
                 updated_at_ns = MAX(updated_at_ns, ?1)
             WHERE binding_id = ?2",
            params![blocked_at_ns, binding_id.to_string()],
        )?;
        Ok(changed == 1)
    }
}

fn insert_action(
    conn: &Connection,
    action: &ContainmentAction,
) -> Result<usize, SecurityStoreError> {
    conn.execute(
        "INSERT INTO containment_actions (
            action_id, case_id, binding_id, agent_id, root_pid, process_start_time,
            source_path, duration_secs, expires_at_ns, lifecycle_state, blocked_at_ns,
            requested_by, failure_stage, failure_reason, attempt_count, next_retry_at_ns,
            created_at_ns, updated_at_ns
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
            ?16, ?17, ?18
         ) ON CONFLICT DO NOTHING",
        params![
            action.action_id.to_string(),
            action.case_id.to_string(),
            action.binding_id.to_string(),
            action.agent_id,
            i64::from(action.root_pid),
            sqlite_time(action.process_start_time)?,
            action.source_path,
            action.duration_secs.map(sqlite_time).transpose()?,
            action.expires_at_ns.map(sqlite_time).transpose()?,
            lifecycle_value(action.lifecycle_state),
            action.blocked_at_ns.map(sqlite_time).transpose()?,
            action.requested_by,
            action.failure_stage.map(failure_stage_value),
            action.failure_reason,
            i64::from(action.attempt_count),
            action.next_retry_at_ns.map(sqlite_time).transpose()?,
            sqlite_time(action.created_at_ns)?,
            sqlite_time(action.updated_at_ns)?,
        ],
    )
    .map_err(Into::into)
}

fn live_action(
    conn: &Connection,
    case_id: Uuid,
) -> Result<Option<ContainmentAction>, SecurityStoreError> {
    let mut statement = conn.prepare(&format!(
        "SELECT {ACTION_COLUMNS}
         FROM containment_actions
         WHERE case_id = ?1 AND lifecycle_state IN ('pending', 'active', 'expiring')
         LIMIT 1"
    ))?;
    statement
        .query_row([case_id.to_string()], containment_row)
        .optional()?
        .map(containment_action_from_row)
        .transpose()
}

type ContainmentRow = (
    String,
    String,
    String,
    String,
    i32,
    i64,
    String,
    Option<i64>,
    Option<i64>,
    String,
    Option<i64>,
    String,
    Option<String>,
    Option<String>,
    i64,
    Option<i64>,
    i64,
    i64,
);

fn containment_row(row: &Row<'_>) -> rusqlite::Result<ContainmentRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
        row.get(12)?,
        row.get(13)?,
        row.get(14)?,
        row.get(15)?,
        row.get(16)?,
        row.get(17)?,
    ))
}

fn containment_action_from_row(
    row: ContainmentRow,
) -> Result<ContainmentAction, SecurityStoreError> {
    Ok(ContainmentAction {
        action_id: parse_uuid(&row.0)?,
        case_id: parse_uuid(&row.1)?,
        binding_id: parse_uuid(&row.2)?,
        agent_id: row.3,
        root_pid: row.4,
        process_start_time: unsigned(row.5, "process_start_time")?,
        source_path: row.6,
        duration_secs: row
            .7
            .map(|value| unsigned(value, "duration_secs"))
            .transpose()?,
        expires_at_ns: row
            .8
            .map(|value| unsigned(value, "expires_at_ns"))
            .transpose()?,
        lifecycle_state: parse_lifecycle(&row.9)?,
        blocked_at_ns: row
            .10
            .map(|value| unsigned(value, "blocked_at_ns"))
            .transpose()?,
        requested_by: row.11,
        failure_stage: row.12.as_deref().map(parse_failure_stage).transpose()?,
        failure_reason: row.13,
        attempt_count: u32::try_from(row.14)
            .map_err(|_| SecurityStoreError::InvalidData("attempt_count is out of range".into()))?,
        next_retry_at_ns: row
            .15
            .map(|value| unsigned(value, "next_retry_at_ns"))
            .transpose()?,
        created_at_ns: unsigned(row.16, "created_at_ns")?,
        updated_at_ns: unsigned(row.17, "updated_at_ns")?,
    })
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

fn parse_lifecycle(value: &str) -> Result<ContainmentLifecycle, SecurityStoreError> {
    match value {
        "pending" => Ok(ContainmentLifecycle::Pending),
        "active" => Ok(ContainmentLifecycle::Active),
        "expiring" => Ok(ContainmentLifecycle::Expiring),
        "expired" => Ok(ContainmentLifecycle::Expired),
        "failed" => Ok(ContainmentLifecycle::Failed),
        _ => Err(SecurityStoreError::InvalidData(format!(
            "unknown containment lifecycle '{value}'"
        ))),
    }
}

fn failure_stage_value(value: ContainmentFailureStage) -> &'static str {
    match value {
        ContainmentFailureStage::Attach => "attach",
        ContainmentFailureStage::Detach => "detach",
        ContainmentFailureStage::Reconcile => "reconcile",
    }
}

fn parse_failure_stage(value: &str) -> Result<ContainmentFailureStage, SecurityStoreError> {
    match value {
        "attach" => Ok(ContainmentFailureStage::Attach),
        "detach" => Ok(ContainmentFailureStage::Detach),
        "reconcile" => Ok(ContainmentFailureStage::Reconcile),
        _ => Err(SecurityStoreError::InvalidData(format!(
            "unknown containment failure stage '{value}'"
        ))),
    }
}
