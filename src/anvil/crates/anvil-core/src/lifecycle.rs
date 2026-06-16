// SPDX-License-Identifier: Apache-2.0
//! Sandbox lifecycle state machine + JSON persistence.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::backend::BackendKind;
use crate::error::{AnvilError, Result};
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
            SandboxState::Reset => "reset",
            SandboxState::Warm => "warm",
            SandboxState::Destroyed => "destroyed",
        }
    }
}

impl std::fmt::Display for SandboxState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Whether a request entered `creating` from cold boot or via a warm
/// pool reuse — used as the primary latency / capacity SLO dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StartPath {
    Cold,
    Warm,
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
}

impl SandboxInstance {
    /// Create a new instance in [`SandboxState::Pending`] with `start_path`
    /// pre-classified by the caller (cold for fresh boots, warm for
    /// pool reuses).
    pub fn new(
        backend: BackendKind,
        workload_class: WorkloadClass,
        image_digest: String,
        start_path: StartPath,
        policy_name: String,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            state: SandboxState::Pending,
            backend,
            workload_class,
            image_digest,
            start_path,
            created_at: now,
            updated_at: now,
            policy_name,
        }
    }

    /// Apply a state transition. Returns
    /// [`AnvilError::InvalidStateTransition`] when the move is not part
    /// of the lifecycle state graph.
    pub fn transition(&mut self, target: SandboxState) -> Result<()> {
        if !is_valid_transition(self.state, target) {
            return Err(AnvilError::InvalidStateTransition {
                from: self.state.to_string(),
                to: target.to_string(),
            });
        }
        let prev = self.state;
        self.state = target;
        self.updated_at = Utc::now();
        // entering `creating` re-classifies the start path: warm-pool
        // reuse goes warm → creating, fresh boots go pending → creating.
        if target == SandboxState::Creating {
            self.start_path = if prev == SandboxState::Warm {
                StartPath::Warm
            } else {
                StartPath::Cold
            };
        }
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
        fs::write(&tmp_path, &json)?;
        fs::rename(&tmp_path, &final_path)?;
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
    use SandboxState::{Checkpointed, Creating, Destroyed, Paused, Pending, Reset, Running, Warm};
    if to == Destroyed {
                // `* → destroyed` is always valid (terminal sink).
        return from != Destroyed;
    }
    match (from, to) {
        (Pending, Creating) => true,
        (Creating, Running) => true,
        (Running, Paused) => true,
        (Running, Reset) => true,
        (Paused, Checkpointed) => true,
        (Paused, Running) => true, // resume
        (Reset, Warm) => true,
        (Warm, Creating) => true, // pool reuse / warm path
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
            StartPath::Cold,
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
    fn happy_path_warm_reuse() {
        let mut inst = fresh();
        inst.transition(SandboxState::Creating).expect("ok");
        inst.transition(SandboxState::Running).expect("ok");
        inst.transition(SandboxState::Reset).expect("ok");
        inst.transition(SandboxState::Warm).expect("ok");
        // warm → creating must flip start_path to Warm.
        inst.transition(SandboxState::Creating).expect("ok");
        assert_eq!(inst.start_path, StartPath::Warm);
    }

    #[test]
    fn destroy_is_always_legal_except_from_destroyed() {
        let mut inst = fresh();
        inst.transition(SandboxState::Destroyed).expect("ok");
        let again = inst.transition(SandboxState::Destroyed);
        assert!(matches!(
            again,
            Err(AnvilError::InvalidStateTransition { .. })
        ));
    }

    #[test]
    fn illegal_pending_to_running() {
        let mut inst = fresh();
        let err = inst.transition(SandboxState::Running).expect_err("illegal");
        assert!(matches!(err, AnvilError::InvalidStateTransition { .. }));
    }

    #[test]
    fn illegal_running_to_warm() {
        let mut inst = fresh();
        inst.transition(SandboxState::Creating).expect("ok");
        inst.transition(SandboxState::Running).expect("ok");
        let err = inst.transition(SandboxState::Warm).expect_err("illegal");
        assert!(matches!(err, AnvilError::InvalidStateTransition { .. }));
    }

    #[test]
    fn illegal_warm_to_running() {
        let mut inst = fresh();
        inst.transition(SandboxState::Creating).expect("ok");
        inst.transition(SandboxState::Running).expect("ok");
        inst.transition(SandboxState::Reset).expect("ok");
        inst.transition(SandboxState::Warm).expect("ok");
        let err = inst.transition(SandboxState::Running).expect_err("illegal");
        assert!(matches!(err, AnvilError::InvalidStateTransition { .. }));
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
}
