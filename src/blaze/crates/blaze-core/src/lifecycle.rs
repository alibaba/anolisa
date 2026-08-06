// SPDX-License-Identifier: Apache-2.0
//! Sandbox lifecycle state machine + JSON persistence.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::backend::BackendKind;
use crate::error::{BlazeError, Result};
use crate::policy::WorkloadClass;

/// All known states. Transitions are enforced by [`SandboxInstance::transition`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxState {
    Pending,
    Creating,
    Running,
    Paused,
    Checkpointed,
    RecoveryRequired,
    Reset,
    Warm,
    Destroyed,
}

impl SandboxState {
    pub const fn as_str(&self) -> &'static str {
        match self {
            SandboxState::Pending => "pending",
            SandboxState::Creating => "creating",
            SandboxState::Running => "running",
            SandboxState::Paused => "paused",
            SandboxState::Checkpointed => "checkpointed",
            SandboxState::RecoveryRequired => "recovery-required",
            SandboxState::Reset => "reset",
            SandboxState::Warm => "warm",
            SandboxState::Destroyed => "destroyed",
        }
    }
}

/// Persisted multi-step operation used for crash diagnosis and recovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OperationKind {
    /// Sandbox creation is acquiring resources or starting a backend.
    Create,
    /// Runtime resources are being destroyed.
    Destroy,
}

/// Durable journal entry for one active lifecycle operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationJournal {
    /// Operation being performed.
    pub kind: OperationKind,
    /// UTC time at which the operation became externally visible.
    pub started_at: DateTime<Utc>,
}

impl std::fmt::Display for SandboxState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Persisted record of whether sandbox startup used a reusable instance.
///
/// New instances always use [`StartPath::Cold`]. [`StartPath::Warm`] remains
/// readable so startup reconciliation can clean records written by older releases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StartPath {
    /// Sandbox creation started without a reusable instance.
    Cold,
    /// Legacy reusable-instance start retained only for persisted-state compatibility.
    Warm,
}

/// Durable knowledge about whether a backend may still own a live process.
///
/// `Unknown` is the safe default for state written by older daemon versions.
/// Recovery must confirm termination for both `Unknown` and `Starting`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BackendOwnership {
    #[default]
    Unknown,
    NotStarted,
    Starting,
    Running,
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxInstance {
    pub id: Uuid,
    pub state: SandboxState,
    pub backend: BackendKind,
    pub workload_class: WorkloadClass,
    pub image_digest: String,
    pub start_path: StartPath,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub policy_name: String,
    /// Last durably known backend ownership state.
    #[serde(default)]
    pub backend_ownership: BackendOwnership,
    /// Active multi-step operation, if any.
    #[serde(default)]
    pub operation: Option<OperationJournal>,
}

impl SandboxInstance {
    /// Create a new instance in [`SandboxState::Pending`].
    pub fn new(
        backend: BackendKind,
        workload_class: WorkloadClass,
        image_digest: String,
        policy_name: String,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            state: SandboxState::Pending,
            backend,
            workload_class,
            image_digest,
            start_path: StartPath::Cold,
            created_at: now,
            updated_at: now,
            policy_name,
            backend_ownership: BackendOwnership::NotStarted,
            operation: None,
        }
    }

    /// Persist an operation before starting its first data-plane mutation.
    pub fn begin_operation(&mut self, kind: OperationKind) {
        self.operation = Some(OperationJournal {
            kind,
            started_at: Utc::now(),
        });
        self.updated_at = Utc::now();
    }

    /// Clear the marker before atomically persisting the final state.
    pub fn finish_operation(&mut self) {
        self.operation = None;
        self.updated_at = Utc::now();
    }

    /// Apply a state transition. Returns
    /// [`BlazeError::InvalidStateTransition`] when the move is not part
    /// of the lifecycle state graph.
    pub fn transition(&mut self, target: SandboxState) -> Result<()> {
        if !is_valid_transition(self.state, target) {
            return Err(BlazeError::InvalidStateTransition {
                from: self.state.to_string(),
                to: target.to_string(),
            });
        }
        let prev = self.state;
        self.state = target;
        self.updated_at = Utc::now();
        tracing::info!(
            instance = %self.id,
            from = %prev,
            to = %target,
            backend = %self.backend,
            class = %self.workload_class,
            "sandbox state transition"
        );
        Ok(())
    }

    /// Persist this instance to `{state_dir}/{id}/state.json`. Atomic
    /// rename via `state.json.tmp` to avoid torn reads on daemon restart.
    pub fn persist(&self, state_dir: &Path) -> Result<()> {
        let dir = state_dir.join(self.id.to_string());
        fs::create_dir_all(&dir)?;
        let final_path = dir.join("state.json");
        let tmp_path = dir.join("state.json.tmp");
        let json = serde_json::to_vec_pretty(self)?;
        {
            let mut file = File::create(&tmp_path)?;
            file.write_all(&json)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
        }
        fs::rename(&tmp_path, &final_path)?;
        File::open(&dir)?.sync_all()?;
        Ok(())
    }

    /// Reload an instance previously persisted via [`Self::persist`].
    pub fn load(state_dir: &Path, id: Uuid) -> Result<Self> {
        let path: PathBuf = state_dir.join(id.to_string()).join("state.json");
        let raw = fs::read(&path)?;
        let instance: SandboxInstance = serde_json::from_slice(&raw)?;
        Ok(instance)
    }
}

fn is_valid_transition(from: SandboxState, to: SandboxState) -> bool {
    use SandboxState::{
        Checkpointed, Creating, Destroyed, Paused, Pending, RecoveryRequired, Running,
    };
    if to == Destroyed {
        // `* → destroyed` is always valid (terminal sink).
        return from != Destroyed;
    }
    if to == RecoveryRequired {
        return !matches!(from, Destroyed | RecoveryRequired);
    }
    match (from, to) {
        (Pending, Creating) => true,
        (Creating, Running) => true,
        (Running, Paused) => true,
        (Paused, Checkpointed) => true,
        (Paused, Running) => true, // resume
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> SandboxInstance {
        SandboxInstance::new(
            BackendKind::KataFc,
            WorkloadClass::AgentRl,
            "sha256:deadbeef".into(),
            "agent-rl-default".into(),
        )
    }

    #[test]
    fn happy_path_cold() {
        let mut inst = fresh();
        for target in [
            SandboxState::Creating,
            SandboxState::Running,
            SandboxState::Paused,
            SandboxState::Checkpointed,
            SandboxState::Destroyed,
        ] {
            inst.transition(target).expect("legal transition");
            assert_eq!(inst.state, target);
        }
    }

    #[test]
    fn destroy_is_always_legal_except_from_destroyed() {
        let mut inst = fresh();
        inst.transition(SandboxState::Destroyed).expect("ok");
        let again = inst.transition(SandboxState::Destroyed);
        assert!(matches!(
            again,
            Err(BlazeError::InvalidStateTransition { .. })
        ));
    }

    #[test]
    fn recovery_required_can_finish_but_cannot_be_reentered() {
        let mut inst = fresh();
        inst.transition(SandboxState::Creating).expect("creating");
        inst.transition(SandboxState::Running).expect("running");
        inst.transition(SandboxState::RecoveryRequired)
            .expect("recovery required");

        let repeated = inst.transition(SandboxState::RecoveryRequired);
        assert!(matches!(
            repeated,
            Err(BlazeError::InvalidStateTransition { .. })
        ));

        inst.transition(SandboxState::Destroyed)
            .expect("destroyed from recovery");
        let terminal = inst.transition(SandboxState::RecoveryRequired);
        assert!(matches!(
            terminal,
            Err(BlazeError::InvalidStateTransition { .. })
        ));
    }

    #[test]
    fn illegal_pending_to_running() {
        let mut inst = fresh();
        let err = inst.transition(SandboxState::Running).expect_err("illegal");
        assert!(matches!(err, BlazeError::InvalidStateTransition { .. }));
    }

    #[test]
    fn reset_and_warm_are_not_runtime_transition_targets() {
        let mut inst = fresh();
        inst.transition(SandboxState::Creating).expect("ok");
        inst.transition(SandboxState::Running).expect("ok");
        for target in [SandboxState::Reset, SandboxState::Warm] {
            let error = inst.transition(target).expect_err("legacy-only state");
            assert!(matches!(error, BlazeError::InvalidStateTransition { .. }));
        }
    }

    #[test]
    fn persist_then_load_round_trip() {
        let tmp = tempfile::tempdir().expect("tmp");
        let mut inst = fresh();
        inst.transition(SandboxState::Creating).expect("ok");
        inst.persist(tmp.path()).expect("persist");

        let loaded = SandboxInstance::load(tmp.path(), inst.id).expect("load");
        assert_eq!(loaded.id, inst.id);
        assert_eq!(loaded.state, SandboxState::Creating);
        assert_eq!(loaded.policy_name, inst.policy_name);
    }

    #[test]
    fn legacy_state_without_optional_fields_deserializes() {
        let inst = fresh();
        let value = serde_json::json!({
            "id": inst.id,
            "state": "running",
            "backend": "mock",
            "workload_class": "agent-rl",
            "image_digest": "sha256:old",
            "start_path": "cold",
            "created_at": inst.created_at,
            "updated_at": inst.updated_at,
            "policy_name": "legacy"
        });
        let loaded: SandboxInstance = serde_json::from_value(value).expect("legacy state");
        assert!(loaded.operation.is_none());
        assert_eq!(loaded.backend_ownership, BackendOwnership::Unknown);
    }

    #[test]
    fn legacy_reset_and_warm_states_deserialize() {
        let inst = fresh();
        for state in ["reset", "warm"] {
            let value = serde_json::json!({
                "id": inst.id,
                "state": state,
                "backend": "mock",
                "workload_class": "agent-rl",
                "image_digest": "sha256:old",
                "start_path": "warm",
                "created_at": inst.created_at,
                "updated_at": inst.updated_at,
                "policy_name": "legacy"
            });
            let loaded: SandboxInstance = serde_json::from_value(value).expect("legacy state");
            assert_eq!(loaded.state.as_str(), state);
            assert_eq!(loaded.start_path, StartPath::Warm);
        }
    }

    #[test]
    fn create_journal_round_trips() {
        let tmp = tempfile::tempdir().expect("tmp");
        let mut instance = fresh();
        instance.begin_operation(OperationKind::Create);
        instance.persist(tmp.path()).expect("persist");

        let mut loaded = SandboxInstance::load(tmp.path(), instance.id).expect("load");
        assert_eq!(
            loaded.operation.as_ref().map(|operation| operation.kind),
            Some(OperationKind::Create)
        );
        loaded.finish_operation();
        assert!(loaded.operation.is_none());
    }
}
