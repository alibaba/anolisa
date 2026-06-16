// SPDX-License-Identifier: Apache-2.0
//! Backend Spawner abstraction (v0.1 decision).
//!
//! The data-plane lifecycle of a sandbox process is hidden behind the
//! [`BackendSpawner`] trait so that:
//!
//! - [`LinuxSandboxSpawner`] drives a real `linux-sandbox` binary on
//!   Linux production hosts (via [`tokio::process::Command`]).
//! - [`MockSpawner`] simulates the same lifecycle on macOS dev machines
//!   or whenever the backend binary is missing — keeping the daemon
//!   functional for API/integration tests without a real backend.
//!
//! Selection happens at daemon boot in `daemon::build_spawner`: when
//! `[backends].linux-sandbox` points at an existing path we
//! pick [`LinuxSandboxSpawner`]; otherwise we warn and fall back to
//! `MockSpawner`. The `wait()` method and the `backend` field on
//! [`SpawnHandle`] are part of the v0.1 contract but only consumed
//! by the v0.2 supervisor — they are intentionally allowed to be
//! dead-code today.

#![allow(dead_code)]

use std::path::Path;
use std::sync::Arc;

use anvil_core::AnvilError;
use anvil_core::backend::BackendKind;
use anvil_core::lifecycle::SandboxInstance;
use async_trait::async_trait;
use uuid::Uuid;

/// Handle returned by [`BackendSpawner::spawn`]. Tracked by the daemon
/// (`ServerState::spawn_handles`) so that subsequent `wait` / `kill`
/// calls can find the right process.
#[derive(Debug, Clone)]
pub struct SpawnHandle {
    /// PID of the spawned process. `None` when the implementation does
    /// not use real OS processes (e.g. [`MockSpawner`]).
    pub pid: Option<u32>,
    pub backend: BackendKind,
    pub instance_id: Uuid,
}

/// Result reported by [`BackendSpawner::wait`] when the sandbox process
/// exits. At least one of `exit_code` / `signal` should be set in real
/// implementations; v0.1 placeholders return `exit_code = Some(0)`.
#[derive(Debug, Clone)]
pub struct SpawnResult {
    pub instance_id: Uuid,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
}

/// Backend Spawner trait — uniform process-management surface for every
/// backend. v0.1 implementations: [`LinuxSandboxSpawner`] and
/// [`MockSpawner`].
#[async_trait]
pub trait BackendSpawner: Send + Sync {
    /// Start a sandbox process for `instance`. The caller is expected
    /// to have already moved the instance into `Creating`.
    async fn spawn(
        &self,
        instance: &SandboxInstance,
        binary_path: &Path,
        work_dir: &Path,
    ) -> Result<SpawnHandle, AnvilError>;

    /// Wait for the process to exit. v0.1 stub returns immediately;
    /// the real supervisor lands together with the v0.2 lifecycle work.
    async fn wait(&self, handle: &SpawnHandle) -> Result<SpawnResult, AnvilError>;

    /// Send SIGTERM to the process. No-op when the handle has no PID.
    async fn kill(&self, handle: &SpawnHandle) -> Result<(), AnvilError>;

    /// Probe whether the backend binary at `binary_path` is usable.
    async fn probe(&self, binary_path: &Path) -> Result<bool, AnvilError>;
}

/// Convenience alias used by `ServerState`.
pub type DynSpawner = Arc<dyn BackendSpawner>;

// ---------------------------------------------------------------------------
// LinuxSandboxSpawner
// ---------------------------------------------------------------------------

/// Production spawner: runs `linux-sandbox run --bundle <work_dir>` via
/// [`tokio::process::Command`]. v0.1 only fires-and-forgets the child;
/// `wait()` is a placeholder until the supervisor lands in v0.2.
pub struct LinuxSandboxSpawner;

#[async_trait]
impl BackendSpawner for LinuxSandboxSpawner {
    async fn spawn(
        &self,
        instance: &SandboxInstance,
        binary_path: &Path,
        work_dir: &Path,
    ) -> Result<SpawnHandle, AnvilError> {
        let child = tokio::process::Command::new(binary_path)
            .arg("run")
            .arg("--bundle")
            .arg(work_dir)
            .env("ANVIL_INSTANCE_ID", instance.id.to_string())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|source| AnvilError::IoError { source })?;

        let pid = child.id();
        // v0.1: we drop the `Child` here on purpose. Tracking child
        // handles for `wait()` requires a daemon-side process map, which
        // lands together with the runtime supervisor in v0.2.
        drop(child);

        tracing::info!(
            instance_id = %instance.id,
            backend = %instance.backend,
            ?pid,
            binary = %binary_path.display(),
            bundle = %work_dir.display(),
            "spawned linux-sandbox process",
        );
        Ok(SpawnHandle {
            pid,
            backend: BackendKind::LinuxSandbox,
            instance_id: instance.id,
        })
    }

    async fn wait(&self, handle: &SpawnHandle) -> Result<SpawnResult, AnvilError> {
        // v0.1 placeholder: the supervisor that holds the `Child` and
        // drives `wait()` lands in v0.2 (TODO).
        tracing::debug!(
            instance_id = %handle.instance_id,
            "linux-sandbox wait (v0.1 placeholder, returns 0)",
        );
        Ok(SpawnResult {
            instance_id: handle.instance_id,
            exit_code: Some(0),
            signal: None,
        })
    }

    async fn kill(&self, handle: &SpawnHandle) -> Result<(), AnvilError> {
        let Some(pid) = handle.pid else {
            return Ok(());
        };
        // v0.1: shell out to `kill -TERM` so we don't pull in libc as a
        // direct dep just for one syscall. SIGKILL escalation lands
        // with the v0.2 supervisor.
        let status = tokio::process::Command::new("kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .status()
            .await
            .map_err(|source| AnvilError::IoError { source })?;
        tracing::info!(
            instance_id = %handle.instance_id,
            pid,
            success = status.success(),
            "sent SIGTERM to linux-sandbox",
        );
        Ok(())
    }

    async fn probe(&self, binary_path: &Path) -> Result<bool, AnvilError> {
        Ok(binary_path.exists())
    }
}

// ---------------------------------------------------------------------------
// MockSpawner
// ---------------------------------------------------------------------------

/// Test / dev spawner. Records intent in tracing logs but never
/// launches a process; used on macOS development hosts and in unit
/// tests that should not depend on a real backend binary.
pub struct MockSpawner;

#[async_trait]
impl BackendSpawner for MockSpawner {
    async fn spawn(
        &self,
        instance: &SandboxInstance,
        _binary_path: &Path,
        _work_dir: &Path,
    ) -> Result<SpawnHandle, AnvilError> {
        tracing::info!(
            instance_id = %instance.id,
            backend = %instance.backend,
            "[mock] simulated spawn",
        );
        Ok(SpawnHandle {
            pid: None,
            backend: instance.backend,
            instance_id: instance.id,
        })
    }

    async fn wait(&self, handle: &SpawnHandle) -> Result<SpawnResult, AnvilError> {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        Ok(SpawnResult {
            instance_id: handle.instance_id,
            exit_code: Some(0),
            signal: None,
        })
    }

    async fn kill(&self, handle: &SpawnHandle) -> Result<(), AnvilError> {
        tracing::info!(instance_id = %handle.instance_id, "[mock] simulated kill");
        Ok(())
    }

    async fn probe(&self, _binary_path: &Path) -> Result<bool, AnvilError> {
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use anvil_core::backend::BackendKind;
    use anvil_core::lifecycle::{SandboxInstance, StartPath};
    use anvil_core::policy::WorkloadClass;

    use super::*;

    fn fixture_instance(backend: BackendKind) -> SandboxInstance {
        SandboxInstance::new(
            backend,
            WorkloadClass::AgentRl,
            "sha256:deadbeef".into(),
            StartPath::Cold,
            "test-policy".into(),
        )
    }

    #[tokio::test]
    async fn mock_spawner_lifecycle() {
        let spawner = MockSpawner;
        let instance = fixture_instance(BackendKind::LinuxSandbox);
        let handle = spawner
            .spawn(&instance, &PathBuf::from("/fake"), &PathBuf::from("/tmp"))
            .await
            .expect("mock spawn never fails");
        assert!(handle.pid.is_none(), "mock must not produce real PIDs");
        assert_eq!(handle.instance_id, instance.id);
        assert_eq!(handle.backend, BackendKind::LinuxSandbox);

        let result = spawner.wait(&handle).await.expect("mock wait");
        assert_eq!(result.exit_code, Some(0));
        assert!(result.signal.is_none());

        spawner.kill(&handle).await.expect("mock kill");

        let probe = spawner
            .probe(&PathBuf::from("/whatever"))
            .await
            .expect("mock probe always Ok");
        assert!(probe, "mock always reports binary as available");
    }

    #[tokio::test]
    async fn linux_sandbox_probe_missing_binary() {
        let spawner = LinuxSandboxSpawner;
        let exists = spawner
            .probe(&PathBuf::from("/nonexistent/anolisa-linux-sandbox"))
            .await
            .expect("probe never errors on missing path");
        assert!(!exists);
    }

    #[tokio::test]
    async fn linux_sandbox_kill_with_no_pid_is_noop() {
        let spawner = LinuxSandboxSpawner;
        let handle = SpawnHandle {
            pid: None,
            backend: BackendKind::LinuxSandbox,
            instance_id: Uuid::new_v4(),
        };
        // Should be a clean no-op rather than an io error.
        spawner.kill(&handle).await.expect("kill no-op");
    }
}
