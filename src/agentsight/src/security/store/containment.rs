//! SQLite reads and writes for durable containment lifecycle state.

use rusqlite::{OptionalExtension, Row, params};
use uuid::Uuid;

use super::{SecurityStore, SecurityStoreError, parse_uuid, sqlite_time, unsigned};
use crate::security::{ContainmentAction, ContainmentFailureStage, ContainmentLifecycle};

const ACTION_COLUMNS: &str = "action_id, case_id, binding_id, agent_id, root_pid,
    process_start_time, source_path, duration_secs, expires_at_ns, lifecycle_state,
    blocked_at_ns, requested_by, failure_stage, failure_reason, attempt_count,
    next_retry_at_ns, created_at_ns, updated_at_ns";

impl SecurityStore {
    /// Inserts one containment action, returning false for a duplicate action ID.
    ///
    /// # Errors
    ///
    /// Returns a typed database, unsigned-value, or lock error.
    pub fn insert_containment_action(
        &self,
        action: &ContainmentAction,
    ) -> Result<bool, SecurityStoreError> {
        let changed = self.connection()?.execute(
            "INSERT INTO containment_actions (
                action_id, case_id, binding_id, agent_id, root_pid, process_start_time,
                source_path, duration_secs, expires_at_ns, lifecycle_state, blocked_at_ns,
                requested_by, failure_stage, failure_reason, attempt_count, next_retry_at_ns,
                created_at_ns, updated_at_ns
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                ?16, ?17, ?18
             )
             ON CONFLICT(action_id) DO NOTHING",
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
        )?;
        Ok(changed == 1)
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

    /// Lists actionable temporary rows with a reached expiry or explicit retry.
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
        let mut statement = conn.prepare(&format!(
            "SELECT {ACTION_COLUMNS}
             FROM containment_actions
             WHERE duration_secs IS NOT NULL
               AND (
                    (expires_at_ns IS NOT NULL AND expires_at_ns <= ?1)
                 OR (next_retry_at_ns IS NOT NULL AND next_retry_at_ns <= ?1)
               )
             ORDER BY COALESCE(next_retry_at_ns, expires_at_ns, created_at_ns) ASC,
                      action_id ASC"
        ))?;
        let rows = statement.query_map([sqlite_time(now_ns)?], containment_row)?;
        let limit = limit.clamp(1, 1_000);
        let mut actions = Vec::with_capacity(limit);
        for row in rows {
            let action = containment_action_from_row(row?)?;
            if matches!(
                action.lifecycle_state,
                ContainmentLifecycle::Pending
                    | ContainmentLifecycle::Active
                    | ContainmentLifecycle::Expiring
            ) && actions.len() < limit
            {
                actions.push(action);
            }
        }
        Ok(actions)
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

    /// Records the first confirmed kernel denial for a containment binding.
    ///
    /// Later calls for the same binding are successful no-ops and cannot
    /// overwrite the original timestamp.
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
             SET blocked_at_ns = ?1,
                 updated_at_ns = MAX(updated_at_ns, ?1)
             WHERE binding_id = ?2 AND blocked_at_ns IS NULL",
            params![blocked_at_ns, binding_id.to_string()],
        )?;
        Ok(changed == 1)
    }
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
