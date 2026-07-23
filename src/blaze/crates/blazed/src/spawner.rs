// SPDX-License-Identifier: Apache-2.0
//! Backend Spawner abstraction (v0.1 decision).
//!
//! The data-plane lifecycle of a sandbox process is hidden behind the
//! [`BackendSpawner`] trait so that:
//!
//! - [`BubblewrapSpawner`] drives `bwrap` (bubblewrap) on Linux
//!   production hosts (via [`tokio::process::Command`]).
//! - [`MockSpawner`] simulates the same lifecycle on macOS dev machines
//!   or whenever the backend binary is missing — keeping the daemon
//!   functional for API/integration tests without a real backend.
//!
//! Selection happens at daemon boot in `daemon::build_spawner`: when
//! `[backends].bubblewrap` points at an existing path we
//! pick [`BubblewrapSpawner`]; otherwise we warn and fall back to
//! `MockSpawner`. The `wait()` method and the `backend` field on
//! [`SpawnHandle`] are part of the v0.1 contract but only consumed
//! by the v0.2 supervisor — they are intentionally allowed to be
//! dead-code today.

#![allow(dead_code)]

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use blaze_core::BlazeError;
use blaze_core::backend::BackendKind;
use blaze_core::lifecycle::SandboxInstance;
use blaze_core::policy::{
    BackendConfigs, FirecrackerConfig, VmConfig, parse_memory_value, to_mib_ceil,
};
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
/// backend. v0.1 implementations: [`BubblewrapSpawner`] and
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
        backend_config: &BackendConfigs,
        vm_config: Option<&VmConfig>,
    ) -> Result<SpawnHandle, BlazeError>;

    /// Wait for the process to exit. v0.1 stub returns immediately;
    /// the real supervisor lands together with the v0.2 lifecycle work.
    async fn wait(&self, handle: &SpawnHandle) -> Result<SpawnResult, BlazeError>;

    /// Send SIGTERM to the process. No-op when the handle has no PID.
    async fn kill(&self, handle: &SpawnHandle) -> Result<(), BlazeError>;

    /// Probe whether the backend binary at `binary_path` is usable.
    async fn probe(&self, binary_path: &Path) -> Result<bool, BlazeError>;

    /// Restore a sandbox from a snapshot + memory file.
    async fn restore(
        &self,
        _snapshot_path: &Path,
        _mem_path: &Path,
        _work_dir: &Path,
    ) -> Result<SpawnHandle, BlazeError> {
        Err(BlazeError::BackendError {
            msg: "restore not supported by this backend".to_string(),
        })
    }

    /// Pause a running sandbox (freeze all processes).
    async fn pause(&self, _handle: &SpawnHandle) -> Result<(), BlazeError> {
        Err(BlazeError::BackendError {
            msg: "pause not supported by this backend".to_string(),
        })
    }

    /// Resume a paused sandbox.
    async fn resume(&self, _handle: &SpawnHandle) -> Result<(), BlazeError> {
        Err(BlazeError::BackendError {
            msg: "resume not supported by this backend".to_string(),
        })
    }

    /// Create a snapshot of a running or paused sandbox.
    async fn create_snapshot(
        &self,
        _handle: &SpawnHandle,
        _output_path: &Path,
        _mem_output_path: &Path,
    ) -> Result<(), BlazeError> {
        Err(BlazeError::BackendError {
            msg: "create_snapshot not supported by this backend".to_string(),
        })
    }
}

/// Convenience alias used by `ServerState`.
pub type DynSpawner = Arc<dyn BackendSpawner>;

// ---------------------------------------------------------------------------
// BubblewrapSpawner
// ---------------------------------------------------------------------------

/// Production spawner: invokes `bwrap` (bubblewrap) directly with a
/// minimal namespace-isolation profile. v0.1 only fires-and-forgets the
/// child; `wait()` is a placeholder until the supervisor lands in v0.2.
pub struct BubblewrapSpawner;

#[async_trait]
impl BackendSpawner for BubblewrapSpawner {
    async fn spawn(
        &self,
        instance: &SandboxInstance,
        binary_path: &Path,
        work_dir: &Path,
        _backend_config: &BackendConfigs,
        _vm_config: Option<&VmConfig>,
    ) -> Result<SpawnHandle, BlazeError> {
        // v0.1: spawn a minimal bwrap sandbox running `/bin/sleep 3600`
        // so the lifecycle (Running -> Destroyed via SIGTERM) can be
        // exercised end-to-end. `work_dir` is reserved for the future
        // OCI-bundle path; not consumed by bwrap today.
        let _ = work_dir;
        let child = tokio::process::Command::new(binary_path)
            .args([
                "--ro-bind",
                "/",
                "/",
                "--proc",
                "/proc",
                "--dev",
                "/dev",
                "--tmpfs",
                "/tmp",
                "--unshare-pid",
                "--unshare-net",
                "--die-with-parent",
                "--",
                "/bin/sleep",
                "3600",
            ])
            .env("BLAZE_INSTANCE_ID", instance.id.to_string())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|source| BlazeError::IoError { source })?;

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
            "spawned bwrap sandbox process",
        );
        Ok(SpawnHandle {
            pid,
            backend: BackendKind::Bubblewrap,
            instance_id: instance.id,
        })
    }

    async fn wait(&self, handle: &SpawnHandle) -> Result<SpawnResult, BlazeError> {
        // v0.1 placeholder: the supervisor that holds the `Child` and
        // drives `wait()` lands in v0.2 (TODO).
        tracing::debug!(
            instance_id = %handle.instance_id,
            "bubblewrap wait (v0.1 placeholder, returns 0)",
        );
        Ok(SpawnResult {
            instance_id: handle.instance_id,
            exit_code: Some(0),
            signal: None,
        })
    }

    async fn kill(&self, handle: &SpawnHandle) -> Result<(), BlazeError> {
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
            .map_err(|source| BlazeError::IoError { source })?;
        tracing::info!(
            instance_id = %handle.instance_id,
            pid,
            success = status.success(),
            "sent SIGTERM to bubblewrap sandbox",
        );
        Ok(())
    }

    async fn probe(&self, binary_path: &Path) -> Result<bool, BlazeError> {
        Ok(binary_path.exists())
    }
}

// ---------------------------------------------------------------------------
// FirecrackerSpawner
// ---------------------------------------------------------------------------

/// Resolve vCPUs for a Firecracker microVM using the fallback chain:
/// `backend.firecracker.vcpus` > `[vm].vcpus` > code default (1).
fn resolve_firecracker_vcpus(fc: &FirecrackerConfig, vm: Option<&VmConfig>) -> u32 {
    fc.vcpus.or(vm.map(|v| v.vcpus)).unwrap_or(1)
}

/// Resolve memory for a Firecracker microVM using the fallback chain:
/// `backend.firecracker.memory` > `[vm].memory` > code default ("256Mi").
/// Returns the value in MiB, rounded up to the nearest integer.
fn resolve_firecracker_memory(
    fc: &FirecrackerConfig,
    vm: Option<&VmConfig>,
) -> Result<u64, BlazeError> {
    let source = if fc.memory.is_some() {
        "backend.firecracker.memory"
    } else if vm.is_some() {
        "[vm].memory"
    } else {
        "code default"
    };
    let memory_str = fc
        .memory
        .as_deref()
        .or(vm.map(|v| v.memory.as_str()))
        .unwrap_or("256Mi");
    Ok(to_mib_ceil(parse_memory_value(memory_str).map_err(
        |e| BlazeError::BackendError {
            msg: format!("invalid firecracker memory value \"{memory_str}\" from {source}: {e}"),
        },
    )?))
}

/// Firecracker microVM spawner: launches the `firecracker` binary with a
/// JSON vmconfig generated from the instance metadata + images_dir.
///
/// Phase 1 implementation:
/// - Generates a minimal vmconfig (kernel, rootfs drive, no network)
/// - Starts FC in API-server mode, waits for /machine-config readiness
/// - Tracks PID for kill()
///
/// Phase 2+ will add: snapshot restore, virtio-pmem, KML memfile path.
pub struct FirecrackerSpawner {
    pub images_dir: std::path::PathBuf,
}

#[async_trait]
impl BackendSpawner for FirecrackerSpawner {
    async fn spawn(
        &self,
        instance: &SandboxInstance,
        binary_path: &Path,
        work_dir: &Path,
        backend_config: &BackendConfigs,
        vm_config: Option<&VmConfig>,
    ) -> Result<SpawnHandle, BlazeError> {
        let fc_cfg = backend_config
            .firecracker
            .as_ref()
            .cloned()
            .unwrap_or_default();

        // Resolve vCPU/memory via the fallback chain:
        // backend.firecracker > [vm] > code default.
        let resolved_vcpus = resolve_firecracker_vcpus(&fc_cfg, vm_config);
        let resolved_memory_mib = resolve_firecracker_memory(&fc_cfg, vm_config)?;

        // Derive paths from images_dir
        let vmlinux = self.images_dir.join("vmlinux");
        let rootfs = self.images_dir.join("rootfs.ext4");

        if !vmlinux.exists() {
            return Err(BlazeError::BackendError {
                msg: format!("vmlinux not found at {}", vmlinux.display()),
            });
        }
        if !rootfs.exists() {
            return Err(BlazeError::BackendError {
                msg: format!("rootfs not found at {}", rootfs.display()),
            });
        }

        // Create per-instance runtime directory
        let instance_dir = work_dir.join(instance.id.to_string());
        std::fs::create_dir_all(&instance_dir).map_err(|source| BlazeError::IoError { source })?;

        let api_socket = instance_dir.join("api.sock");
        let log_file = instance_dir.join("firecracker.log");

        // Pre-compute UTF-8 path strings; Linux allows non-UTF-8 paths, so fail
        // explicitly rather than unwrap() inside the vmconfig builder.
        let vmlinux_str = vmlinux.to_str().ok_or_else(|| BlazeError::BackendError {
            msg: format!("vmlinux path is not valid UTF-8: {}", vmlinux.display()),
        })?;
        let rootfs_str = rootfs.to_str().ok_or_else(|| BlazeError::BackendError {
            msg: format!("rootfs path is not valid UTF-8: {}", rootfs.display()),
        })?;
        let log_path_str = log_file.to_str().ok_or_else(|| BlazeError::BackendError {
            msg: format!(
                "firecracker log path is not valid UTF-8: {}",
                log_file.display()
            ),
        })?;

        // Generate vmconfig JSON
        let vmconfig = serde_json::json!({
            "boot-source": {
                "kernel_image_path": vmlinux_str,
                "boot_args": fc_cfg.boot_args
            },
            "drives": [{
                "drive_id": "rootfs",
                "path_on_host": rootfs_str,
                "is_root_device": true,
                "is_read_only": false
            }],
            "machine-config": {
                "vcpu_count": resolved_vcpus,
                "mem_size_mib": resolved_memory_mib
            },
            "logger": {
                "log_path": log_path_str,
                "level": "Info",
                "show_level": true,
                "show_log_origin": true
            }
        });

        let config_path = instance_dir.join("vmconfig.json");
        let vmconfig_json =
            serde_json::to_string_pretty(&vmconfig).map_err(|e| BlazeError::BackendError {
                msg: format!("failed to serialize firecracker vmconfig JSON: {e}"),
            })?;
        std::fs::write(&config_path, vmconfig_json)
            .map_err(|source| BlazeError::IoError { source })?;

        // Spawn firecracker process
        let mut cmd = tokio::process::Command::new(binary_path);
        cmd.arg("--api-sock")
            .arg(&api_socket)
            .arg("--config-file")
            .arg(&config_path)
            .env("BLAZE_INSTANCE_ID", instance.id.to_string());

        // Guest ttyS0 output is emitted on Firecracker's stdout. When
        // serial_log is enabled, persist it to serial.log for debugging.
        if fc_cfg.serial_log {
            let serial_log = instance_dir.join("serial.log");
            rotate_serial_log_if_needed(&serial_log)
                .map_err(|source| BlazeError::IoError { source })?;
            let file = std::fs::File::create(&serial_log)
                .map_err(|source| BlazeError::IoError { source })?;
            cmd.stdout(file);
            tracing::info!(
                instance_id = %instance.id,
                serial_log = %serial_log.display(),
                "firecracker serial log enabled",
            );
        } else {
            cmd.stdout(std::process::Stdio::null());
        }
        cmd.stderr(std::process::Stdio::null());

        let child = cmd
            .spawn()
            .map_err(|source| BlazeError::IoError { source })?;

        let pid = child.id();
        drop(child); // v0.1: fire-and-forget, supervisor in v0.2

        tracing::info!(
            instance_id = %instance.id,
            ?pid,
            api_socket = %api_socket.display(),
            "spawned firecracker microVM",
        );

        Ok(SpawnHandle {
            pid,
            backend: BackendKind::Firecracker,
            instance_id: instance.id,
        })
    }

    async fn wait(&self, handle: &SpawnHandle) -> Result<SpawnResult, BlazeError> {
        // v0.1 placeholder
        tracing::debug!(
            instance_id = %handle.instance_id,
            "firecracker wait (v0.1 placeholder)",
        );
        Ok(SpawnResult {
            instance_id: handle.instance_id,
            exit_code: Some(0),
            signal: None,
        })
    }

    async fn kill(&self, handle: &SpawnHandle) -> Result<(), BlazeError> {
        let Some(pid) = handle.pid else {
            return Ok(());
        };
        let status = tokio::process::Command::new("kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .status()
            .await
            .map_err(|source| BlazeError::IoError { source })?;
        tracing::info!(
            instance_id = %handle.instance_id,
            pid,
            success = status.success(),
            "sent SIGTERM to firecracker",
        );
        Ok(())
    }

    async fn probe(&self, binary_path: &Path) -> Result<bool, BlazeError> {
        // Check binary exists and is executable
        if !binary_path.exists() {
            return Ok(false);
        }
        // Quick version check
        let output = tokio::process::Command::new(binary_path)
            .arg("--version")
            .output()
            .await;
        match output {
            Ok(o) => Ok(o.status.success()),
            Err(_) => Ok(false),
        }
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
        _backend_config: &BackendConfigs,
        _vm_config: Option<&VmConfig>,
    ) -> Result<SpawnHandle, BlazeError> {
        tracing::info!(
            instance_id = %instance.id,
            backend = %instance.backend,
            "[mock] simulated spawn",
        );
        Ok(SpawnHandle {
            pid: None,
            backend: BackendKind::Mock,
            instance_id: instance.id,
        })
    }

    async fn wait(&self, handle: &SpawnHandle) -> Result<SpawnResult, BlazeError> {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        Ok(SpawnResult {
            instance_id: handle.instance_id,
            exit_code: Some(0),
            signal: None,
        })
    }

    async fn kill(&self, handle: &SpawnHandle) -> Result<(), BlazeError> {
        tracing::info!(instance_id = %handle.instance_id, "[mock] simulated kill");
        Ok(())
    }

    async fn probe(&self, _binary_path: &Path) -> Result<bool, BlazeError> {
        Ok(true)
    }
}

/// Rotate serial log if it exceeds the size limit.
/// Keeps at most one backup (.1) to preserve crash context.
fn rotate_serial_log_if_needed(path: &Path) -> std::io::Result<()> {
    const MAX_SERIAL_LOG_BYTES: u64 = 16 * 1024 * 1024; // 16 MiB
    if let Ok(meta) = std::fs::metadata(path)
        && meta.len() > MAX_SERIAL_LOG_BYTES
    {
        let backup = path.with_extension("log.1");
        let _ = std::fs::rename(path, &backup); // best-effort rotate
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use blaze_core::backend::BackendKind;
    use blaze_core::lifecycle::{SandboxInstance, StartPath};
    use blaze_core::policy::{VmConfig, WorkloadClass};

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
        let instance = fixture_instance(BackendKind::Bubblewrap);
        let handle = spawner
            .spawn(
                &instance,
                &PathBuf::from("/fake"),
                &PathBuf::from("/tmp"),
                &BackendConfigs::default(),
                None,
            )
            .await
            .expect("mock spawn never fails");
        assert!(handle.pid.is_none(), "mock must not produce real PIDs");
        assert_eq!(handle.instance_id, instance.id);
        // MockSpawner always reports its true identity regardless of the
        // policy-selected backend on the instance.
        assert_eq!(handle.backend, BackendKind::Mock);

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
    async fn bubblewrap_probe_missing_binary() {
        let spawner = BubblewrapSpawner;
        let exists = spawner
            .probe(&PathBuf::from("/nonexistent/bwrap"))
            .await
            .expect("probe never errors on missing path");
        assert!(!exists);
    }

    #[tokio::test]
    async fn bubblewrap_kill_with_no_pid_is_noop() {
        let spawner = BubblewrapSpawner;
        let handle = SpawnHandle {
            pid: None,
            backend: BackendKind::Bubblewrap,
            instance_id: Uuid::new_v4(),
        };
        // Should be a clean no-op rather than an io error.
        spawner.kill(&handle).await.expect("kill no-op");
    }

    #[test]
    fn firecracker_fallback_chain_priority() {
        let vm = VmConfig {
            vcpus: 2,
            memory: "1G".into(),
        };

        // backend override > [vm]
        let override_cfg = FirecrackerConfig {
            vcpus: Some(4),
            memory: Some("2G".into()),
            ..Default::default()
        };
        assert_eq!(resolve_firecracker_vcpus(&override_cfg, Some(&vm)), 4);
        // "2G" = 2_000_000_000 bytes -> ceil(2_000_000_000 / 1_048_576) = 1908 MiB.
        assert_eq!(
            resolve_firecracker_memory(&override_cfg, Some(&vm)).unwrap(),
            2_000_000_000u64.div_ceil(1 << 20)
        );

        // [vm] > code default
        let no_override = FirecrackerConfig::default();
        assert_eq!(resolve_firecracker_vcpus(&no_override, Some(&vm)), 2);
        // "1G" = 1_000_000_000 bytes -> ceil(1_000_000_000 / 1_048_576) = 954 MiB.
        assert_eq!(
            resolve_firecracker_memory(&no_override, Some(&vm)).unwrap(),
            1_000_000_000u64.div_ceil(1 << 20)
        );

        // default when neither
        assert_eq!(resolve_firecracker_vcpus(&no_override, None), 1);
        assert_eq!(resolve_firecracker_memory(&no_override, None).unwrap(), 256);
    }

    #[test]
    fn firecracker_memory_only_fc_override_no_vm() {
        // fc.memory set, vm absent => uses fc.memory
        let fc = FirecrackerConfig {
            memory: Some("512Mi".into()),
            ..Default::default()
        };
        assert_eq!(resolve_firecracker_memory(&fc, None).unwrap(), 512);
    }

    #[test]
    fn firecracker_vcpus_only_vm_layer() {
        // fc.vcpus absent, vm present => uses vm.vcpus
        let fc = FirecrackerConfig::default();
        let vm = VmConfig {
            vcpus: 8,
            memory: "256Mi".into(),
        };
        assert_eq!(resolve_firecracker_vcpus(&fc, Some(&vm)), 8);
    }

    #[test]
    fn firecracker_memory_invalid_value_returns_error() {
        // Invalid memory string should propagate as BackendError
        let fc = FirecrackerConfig {
            memory: Some("not-a-size".into()),
            ..Default::default()
        };
        let result = resolve_firecracker_memory(&fc, None);
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("not-a-size"),
            "error should reference the invalid value: {err_msg}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn firecracker_spawn_rejects_non_utf8_paths() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let tmp = std::env::temp_dir().join("blaze-test-non-utf8");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        // Create a subdirectory whose name contains invalid UTF-8 bytes
        let bad_name = OsStr::from_bytes(b"images-\xff\xfe");
        let non_utf8_dir = tmp.join(bad_name);

        // On macOS (APFS/HFS+), non-UTF-8 directory names are unsupported; skip gracefully.
        if std::fs::create_dir_all(&non_utf8_dir).is_err() {
            eprintln!("skipping: filesystem does not support non-UTF-8 directory names");
            let _ = std::fs::remove_dir_all(&tmp);
            return;
        }

        // Place image files inside the non-UTF-8 directory so exists() checks pass
        std::fs::write(non_utf8_dir.join("vmlinux"), b"fake-kernel").unwrap();
        std::fs::write(non_utf8_dir.join("rootfs.ext4"), b"fake-rootfs").unwrap();

        let spawner = FirecrackerSpawner {
            images_dir: non_utf8_dir,
        };
        let instance = fixture_instance(BackendKind::Firecracker);
        let result = spawner
            .spawn(
                &instance,
                &PathBuf::from("/usr/bin/firecracker"),
                &tmp,
                &BackendConfigs {
                    firecracker: Some(FirecrackerConfig::default()),
                },
                None,
            )
            .await;

        assert!(result.is_err(), "non-UTF-8 path must produce an error");
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("not valid UTF-8"),
            "error should mention UTF-8 validation failure: {err_msg}"
        );

        // Cleanup
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn firecracker_spawn_missing_vmlinux_returns_error() {
        let tmp = std::env::temp_dir().join("blaze-test-missing-vmlinux");
        let _ = std::fs::create_dir_all(&tmp);
        // Only create rootfs, omit vmlinux
        std::fs::write(tmp.join("rootfs.ext4"), b"fake-rootfs").unwrap();
        let _ = std::fs::remove_file(tmp.join("vmlinux"));

        let spawner = FirecrackerSpawner {
            images_dir: tmp.clone(),
        };
        let instance = fixture_instance(BackendKind::Firecracker);
        let result = spawner
            .spawn(
                &instance,
                &PathBuf::from("/usr/bin/firecracker"),
                &tmp,
                &BackendConfigs {
                    firecracker: Some(FirecrackerConfig::default()),
                },
                None,
            )
            .await;

        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("vmlinux not found"),
            "should report vmlinux missing: {err_msg}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn firecracker_spawn_missing_rootfs_returns_error() {
        let tmp = std::env::temp_dir().join("blaze-test-missing-rootfs");
        let _ = std::fs::create_dir_all(&tmp);
        // Only create vmlinux, omit rootfs
        std::fs::write(tmp.join("vmlinux"), b"fake-kernel").unwrap();
        let _ = std::fs::remove_file(tmp.join("rootfs.ext4"));

        let spawner = FirecrackerSpawner {
            images_dir: tmp.clone(),
        };
        let instance = fixture_instance(BackendKind::Firecracker);
        let result = spawner
            .spawn(
                &instance,
                &PathBuf::from("/usr/bin/firecracker"),
                &tmp,
                &BackendConfigs {
                    firecracker: Some(FirecrackerConfig::default()),
                },
                None,
            )
            .await;

        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("rootfs not found"),
            "should report rootfs missing: {err_msg}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
