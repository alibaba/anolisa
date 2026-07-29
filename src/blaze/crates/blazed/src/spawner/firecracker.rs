// SPDX-License-Identifier: Apache-2.0
//! Firecracker process ownership and HTTP API over Unix domain sockets.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use blaze_core::backend::{BackendKind, SpawnRequest};
use blaze_core::policy::{FirecrackerConfig, VmConfig, parse_memory_value, to_mib_ceil};
use blaze_core::{BlazeError, Result};
use tokio::net::UnixStream;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use uuid::Uuid;

#[cfg(target_os = "linux")]
use super::terminate_recorded_process;
use super::{
    BackendInstance, BackendSpawner, DynBackendInstance, SpawnFailure, SpawnResult,
    record_backend_stopped, remove_file_if_exists, spawn_result, stopped_marker, terminate_child,
};

/// Firecracker backend factory.
pub struct FirecrackerSpawner {
    images_dir: PathBuf,
    socket_timeout: Duration,
    version: Mutex<Option<String>>,
}

impl FirecrackerSpawner {
    /// Create a spawner resolving the guest kernel from `images_dir`.
    pub fn new(images_dir: PathBuf) -> Self {
        Self {
            images_dir,
            socket_timeout: Duration::from_secs(5),
            version: Mutex::new(None),
        }
    }

    async fn start(
        &self,
        request: SpawnRequest,
    ) -> std::result::Result<DynBackendInstance, SpawnFailure> {
        validate_regular_file(&request.binary_path, "firecracker binary")?;
        validate_regular_file(&request.storage.rootfs_path, "rootfs")?;
        validate_regular_file(&self.images_dir.join("vmlinux"), "vmlinux")?;
        tokio::fs::create_dir_all(&request.run_dir).await?;
        let api_socket = request.run_dir.join("api.sock");
        let guest_socket = request.run_dir.join("vsock.uds");
        let pid_file = request.run_dir.join("firecracker.pid");
        let stopped_marker = stopped_marker(&request.run_dir);
        remove_if_exists(&api_socket).await?;
        remove_if_exists(&guest_socket).await?;
        remove_if_exists(&pid_file).await?;
        remove_file_if_exists(&stopped_marker).await?;
        let fc_config = request
            .backend
            .firecracker
            .as_ref()
            .cloned()
            .unwrap_or_default();
        let mut command = build_launch_command(&request.binary_path, &api_socket);
        let config_path = write_vm_config(&self.images_dir, &request, &fc_config, &guest_socket)?;
        command.arg("--config-file").arg(config_path);
        configure_logs(&mut command, &request.run_dir, fc_config.serial_log)?;
        command.env("BLAZE_INSTANCE_ID", request.instance_id.to_string());
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(source) => return Err(source.into()),
        };
        if let Some(pid) = child.id()
            && let Err(error) = tokio::fs::write(&pid_file, format!("{pid}\n")).await
        {
            let owner: DynBackendInstance = Arc::new(FirecrackerInstance::new(
                request.instance_id,
                child,
                api_socket,
                guest_socket,
                pid_file,
                stopped_marker,
            ));
            return Err(SpawnFailure::compensate_started(error.into(), owner).await);
        }
        if let Err(error) = wait_for_socket(&api_socket, &mut child, self.socket_timeout).await {
            let owner: DynBackendInstance = Arc::new(FirecrackerInstance::new(
                request.instance_id,
                child,
                api_socket,
                guest_socket,
                pid_file,
                stopped_marker,
            ));
            return Err(SpawnFailure::compensate_started(error, owner).await);
        }

        let instance = FirecrackerInstance::new(
            request.instance_id,
            child,
            api_socket,
            guest_socket,
            pid_file,
            stopped_marker,
        );
        Ok(Arc::new(instance))
    }
}

#[async_trait]
impl BackendSpawner for FirecrackerSpawner {
    async fn spawn(
        &self,
        request: SpawnRequest,
    ) -> std::result::Result<DynBackendInstance, SpawnFailure> {
        self.start(request).await
    }

    async fn probe(&self, binary_path: &Path) -> Result<bool> {
        if !binary_path.is_file() || !executable_in_path("unshare") {
            return Ok(false);
        }
        match read_backend_version(binary_path).await {
            Ok(version) => {
                *self.version.lock().await = Some(version);
                Ok(true)
            }
            Err(error) => {
                tracing::debug!(%error, binary = %binary_path.display(), "firecracker version probe failed");
                Ok(false)
            }
        }
    }

    async fn cleanup_orphan(&self, instance_id: Uuid, run_dir: &Path) -> Result<()> {
        cleanup_orphan_run_dir(instance_id, run_dir).await
    }
}

struct FirecrackerInstance {
    instance_id: Uuid,
    child: Mutex<Option<Child>>,
    api_socket: PathBuf,
    guest_socket: PathBuf,
    pid_file: PathBuf,
    stopped_marker: PathBuf,
    killed: AtomicBool,
}

impl FirecrackerInstance {
    fn new(
        instance_id: Uuid,
        child: Child,
        api_socket: PathBuf,
        guest_socket: PathBuf,
        pid_file: PathBuf,
        stopped_marker: PathBuf,
    ) -> Self {
        Self {
            instance_id,
            child: Mutex::new(Some(child)),
            api_socket,
            guest_socket,
            pid_file,
            stopped_marker,
            killed: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl BackendInstance for FirecrackerInstance {
    fn backend(&self) -> BackendKind {
        BackendKind::Firecracker
    }

    async fn try_wait(&self) -> Result<Option<SpawnResult>> {
        let status = {
            let mut guard = self.child.lock().await;
            let Some(child) = guard.as_mut() else {
                return Ok(Some(SpawnResult {
                    instance_id: self.instance_id,
                    exit_code: None,
                    signal: None,
                }));
            };
            let Some(status) = child.try_wait()? else {
                return Ok(None);
            };
            record_backend_stopped(&self.stopped_marker).await?;
            *guard = None;
            status
        };
        self.cleanup().await?;
        Ok(Some(spawn_result(self.instance_id, status)))
    }

    async fn kill(&self) -> Result<()> {
        if self.killed.load(Ordering::Acquire) {
            return Ok(());
        }
        let mut guard = self.child.lock().await;
        if self.killed.load(Ordering::Acquire) {
            return Ok(());
        }
        if let Some(child) = guard.as_mut() {
            terminate_child(child, "firecracker").await?;
        }
        record_backend_stopped(&self.stopped_marker).await?;
        *guard = None;
        drop(guard);
        self.cleanup().await?;
        self.killed.store(true, Ordering::Release);
        Ok(())
    }
}

impl FirecrackerInstance {
    async fn cleanup(&self) -> Result<()> {
        remove_if_exists(&self.api_socket).await?;
        remove_if_exists(&self.guest_socket).await?;
        remove_if_exists(&self.pid_file).await?;
        Ok(())
    }
}

fn write_vm_config(
    images_dir: &Path,
    request: &SpawnRequest,
    config: &FirecrackerConfig,
    guest_socket: &Path,
) -> Result<PathBuf> {
    let vcpus = config
        .vcpus
        .or(request.vm.as_ref().map(|vm| vm.vcpus))
        .unwrap_or(1);
    let memory_mib = resolve_memory(config, request.vm.as_ref())?;
    let mut value = serde_json::json!({
        "boot-source": {
            "kernel_image_path": path_string(&images_dir.join("vmlinux"), "vmlinux")?,
            "boot_args": config.boot_args
        },
        "drives": [{
            "drive_id": "rootfs",
            "path_on_host": path_string(&request.storage.rootfs_path, "rootfs")?,
            "is_root_device": true,
            "is_read_only": false
        }],
        "machine-config": {
            "vcpu_count": vcpus,
            "mem_size_mib": memory_mib
        }
    });
    if config.enable_vsock {
        value["vsock"] = serde_json::json!({
            "guest_cid": 3,
            "uds_path": path_string(guest_socket, "guest socket")?
        });
    }
    let path = request.run_dir.join("vmconfig.json");
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&value).map_err(|error| BlazeError::BackendError {
            msg: format!("serialize Firecracker VM config: {error}"),
        })?,
    )?;
    Ok(path)
}

fn resolve_memory(config: &FirecrackerConfig, vm: Option<&VmConfig>) -> Result<u64> {
    let value = config
        .memory
        .as_deref()
        .or_else(|| vm.map(|vm| vm.memory.as_str()))
        .unwrap_or("256Mi");
    parse_memory_value(value)
        .map(to_mib_ceil)
        .map_err(|error| BlazeError::BackendError {
            msg: format!("invalid Firecracker memory {value:?}: {error}"),
        })
}

fn build_launch_command(binary: &Path, api_socket: &Path) -> Command {
    #[cfg(target_os = "linux")]
    let mut command = {
        let mut command = Command::new("unshare");
        command
            .arg("--mount")
            .arg("--propagation")
            .arg("private")
            .arg("--")
            .arg(binary);
        command
    };
    #[cfg(not(target_os = "linux"))]
    let mut command = Command::new(binary);
    command.arg("--api-sock").arg(api_socket);
    command.arg("--id").arg(format!(
        "fc-{}",
        api_socket
            .parent()
            .and_then(Path::file_name)
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or("blaze")
    ));
    command
}

fn configure_logs(command: &mut Command, run_dir: &Path, serial_log: bool) -> Result<()> {
    if serial_log {
        let serial_log = run_dir.join("serial.log");
        rotate_serial_log_if_needed(&serial_log)?;
        let stdout = std::fs::File::create(serial_log)?;
        command.stdout(stdout);
    } else {
        command.stdout(Stdio::null());
    }
    let stderr = std::fs::File::create(run_dir.join("stderr.log"))?;
    command.stderr(stderr);
    command.stdin(Stdio::null());
    Ok(())
}

async fn read_backend_version(binary_path: &Path) -> Result<String> {
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        Command::new(binary_path).arg("--version").output(),
    )
    .await
    .map_err(|_| BlazeError::BackendError {
        msg: format!("firecracker probe timed out: {}", binary_path.display()),
    })??;
    if !output.status.success() {
        return Err(BlazeError::BackendError {
            msg: format!(
                "firecracker version probe failed with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    parse_backend_version(&output.stdout)
}

fn parse_backend_version(stdout: &[u8]) -> Result<String> {
    let stdout = std::str::from_utf8(stdout).map_err(|error| BlazeError::BackendError {
        msg: format!("firecracker version probe returned non-UTF-8 output: {error}"),
    })?;
    let mut versions = stdout
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("Firecracker v"));
    let version = versions.next().ok_or_else(|| BlazeError::BackendError {
        msg: "firecracker version probe did not return a Firecracker version line".to_string(),
    })?;
    if versions.next().is_some() {
        return Err(BlazeError::BackendError {
            msg: "firecracker version probe returned multiple Firecracker version lines"
                .to_string(),
        });
    }
    let release = version
        .strip_prefix("Firecracker v")
        .expect("version prefix checked");
    if release.is_empty() || release.chars().any(char::is_whitespace) {
        return Err(BlazeError::BackendError {
            msg: format!("firecracker version probe returned an invalid version line: {version:?}"),
        });
    }
    Ok(version.to_string())
}

async fn wait_for_socket(socket: &Path, child: &mut Child, timeout: Duration) -> Result<()> {
    let started = Instant::now();
    loop {
        if socket.exists() && UnixStream::connect(socket).await.is_ok() {
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            return Err(BlazeError::BackendError {
                msg: format!(
                    "Firecracker exited before API socket {} became ready: {status}",
                    socket.display()
                ),
            });
        }
        if started.elapsed() >= timeout {
            return Err(BlazeError::BackendError {
                msg: format!(
                    "Firecracker API socket {} was not ready within {timeout:?}",
                    socket.display()
                ),
            });
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

async fn remove_if_exists(path: &Path) -> Result<()> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn validate_regular_file(path: &Path, label: &str) -> Result<()> {
    if !path.is_file() {
        return Err(BlazeError::BackendError {
            msg: format!("{label} not found at {}", path.display()),
        });
    }
    Ok(())
}

fn executable_in_path(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|directory| is_executable_file(&directory.join(name)))
}

fn is_executable_file(candidate: &Path) -> bool {
    if !candidate.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(candidate)
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn rotate_serial_log_if_needed(path: &Path) -> Result<()> {
    const MAX_SERIAL_LOG_BYTES: u64 = 16 * 1024 * 1024;
    let Ok(metadata) = std::fs::metadata(path) else {
        return Ok(());
    };
    if metadata.len() <= MAX_SERIAL_LOG_BYTES {
        return Ok(());
    }
    let backup = path.with_extension("log.1");
    match std::fs::remove_file(&backup) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    std::fs::rename(path, backup)?;
    Ok(())
}

fn path_string<'a>(path: &'a Path, label: &str) -> Result<&'a str> {
    path.to_str().ok_or_else(|| BlazeError::BackendError {
        msg: format!("{label} path is not valid UTF-8: {}", path.display()),
    })
}

pub(super) async fn cleanup_orphan_run_dir(instance_id: Uuid, run_dir: &Path) -> Result<()> {
    let stopped_marker = stopped_marker(run_dir);
    if stopped_marker.is_file() {
        return Ok(());
    }
    let pid_file = run_dir.join("firecracker.pid");
    #[cfg(target_os = "linux")]
    {
        terminate_recorded_process(instance_id, &pid_file, "firecracker").await?;
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = instance_id;
        if pid_file.exists() {
            return Err(BlazeError::BackendError {
                msg: format!(
                    "cannot validate Firecracker orphan {} outside Linux",
                    pid_file.display()
                ),
            });
        }
    }

    record_backend_stopped(&stopped_marker).await?;
    remove_if_exists(&run_dir.join("api.sock")).await?;
    remove_if_exists(&run_dir.join("vsock.uds")).await?;
    remove_if_exists(&pid_file).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use blaze_core::storage::StorageSlot;

    use super::*;

    #[test]
    fn version_parser_discards_non_version_log_lines() {
        let stdout = b"Firecracker v1.16.0\n\n\
            2026-07-24T21:55:14Z [anonymous-instance:main] \
            Firecracker exiting successfully. exit_code=0\n";
        assert_eq!(
            parse_backend_version(stdout).expect("version"),
            "Firecracker v1.16.0"
        );
    }

    #[test]
    fn version_parser_rejects_missing_or_ambiguous_version() {
        assert!(parse_backend_version(b"Firecracker exiting successfully\n").is_err());
        assert!(parse_backend_version(b"Firecracker v1.15.0\nFirecracker v1.16.0\n").is_err());
    }

    #[test]
    fn vm_config_omits_network_until_the_network_capability_is_enabled() {
        let temp = tempfile::tempdir().expect("temp");
        let request = spawn_request(temp.path());

        let path = write_vm_config(
            &temp.path().join("images"),
            &request,
            &FirecrackerConfig::default(),
            &temp.path().join("guest.sock"),
        )
        .expect("write config");
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).expect("read config"))
                .expect("parse config");
        assert!(value.get("network-interfaces").is_none());
    }

    #[test]
    fn serial_log_rotates_before_reuse() {
        let temp = tempfile::tempdir().expect("temp");
        let log = temp.path().join("serial.log");
        let file = std::fs::File::create(&log).expect("create log");
        file.set_len(16 * 1024 * 1024 + 1).expect("grow log");

        rotate_serial_log_if_needed(&log).expect("rotate");

        assert!(!log.exists());
        assert_eq!(
            std::fs::metadata(temp.path().join("serial.log.1"))
                .expect("rotated log")
                .len(),
            16 * 1024 * 1024 + 1
        );
    }

    #[cfg(unix)]
    #[test]
    fn executable_check_requires_an_executable_file() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("temp");
        let tool = temp.path().join("tool");
        std::fs::write(&tool, b"#!/bin/sh\n").expect("write tool");
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o644))
            .expect("non-executable permissions");
        assert!(!is_executable_file(&tool));
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755))
            .expect("executable permissions");
        assert!(is_executable_file(&tool));
    }

    #[tokio::test]
    async fn start_failure_terminates_child_and_removes_process_metadata() {
        let temp = tempfile::tempdir().expect("temp");
        let pid_file = temp.path().join("firecracker.pid");
        let termination_marker = temp.path().join("terminated");
        let child = Command::new("sh")
            .arg("-c")
            .arg("trap 'printf term > \"$MARKER\"; exit 0' TERM; while :; do sleep 1; done")
            .env("MARKER", &termination_marker)
            .spawn()
            .expect("spawn child");
        std::fs::write(&pid_file, format!("{}\n", child.id().expect("child pid")))
            .expect("pid metadata");
        tokio::time::sleep(Duration::from_millis(50)).await;
        let owner: DynBackendInstance = Arc::new(FirecrackerInstance::new(
            Uuid::new_v4(),
            child,
            temp.path().join("api.sock"),
            temp.path().join("guest.sock"),
            pid_file.clone(),
            stopped_marker(temp.path()),
        ));
        let failure = SpawnFailure::compensate_started(
            BlazeError::BackendError {
                msg: "injected start failure".to_string(),
            },
            owner,
        )
        .await;
        let (source, owner) = failure.into_parts();

        assert!(source.to_string().contains("injected start failure"));
        assert!(
            owner.is_none(),
            "successful compensation must drop ownership"
        );
        assert_eq!(
            std::fs::read_to_string(termination_marker).expect("termination marker"),
            "term"
        );
        assert!(!pid_file.exists());
    }

    fn spawn_request(root: &Path) -> SpawnRequest {
        let instance_id = Uuid::new_v4();
        let run_dir = root.join("run");
        std::fs::create_dir_all(&run_dir).expect("run dir");
        let slot_dir = root.join("slot");
        SpawnRequest {
            instance_id,
            run_dir,
            binary_path: root.join("firecracker"),
            storage: StorageSlot {
                id: instance_id.to_string(),
                rootfs_path: slot_dir.join("rootfs.ext4"),
                mem_path: slot_dir.join("mem.bin"),
                mem_diff_path: slot_dir.join("mem.diff"),
                rootfs_diff_path: slot_dir.join("rootfs.diff"),
                instance_dir: slot_dir,
            },
            backend: blaze_core::policy::BackendConfigs::default(),
            vm: None,
        }
    }
}
