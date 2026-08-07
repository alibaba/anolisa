// SPDX-License-Identifier: Apache-2.0
//! Firecracker process ownership and HTTP API over Unix domain sockets.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use blaze_core::backend::BackendKind;
#[cfg(test)]
use blaze_core::backend::SpawnRequest;
use blaze_core::policy::{FirecrackerConfig, VmConfig, parse_memory_value, to_mib_ceil};
use blaze_core::{BlazeError, Result};
use tokio::net::UnixStream;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use uuid::Uuid;

use super::netns::{NetworkManager, NetworkSlot};
#[cfg(target_os = "linux")]
use super::terminate_recorded_process;
use super::{
    BackendInstance, BackendSpawnRequest, BackendSpawner, DynBackendInstance, OwnedRunDir,
    SpawnFailure, SpawnResult, configure_pid_handoff, prepare_pid_handoff, record_backend_stopped,
    remove_file_if_exists, spawn_result, stopped_marker, terminate_child,
};

const NETWORK_BOOT_IP: &str = "ip=169.254.0.2::169.254.0.1:255.255.255.252::eth0:off";

/// Firecracker backend factory.
pub struct FirecrackerSpawner {
    images_dir: PathBuf,
    socket_timeout: Duration,
    network: Arc<NetworkManager>,
    network_required: bool,
    version: Mutex<Option<String>>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum NetworkProcessState {
    PreSpawn,
    #[default]
    Launching,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct NetworkRecord {
    slot: usize,
    owner: Uuid,
    #[serde(default)]
    process_state: NetworkProcessState,
}

impl FirecrackerSpawner {
    /// Create a spawner without requiring host networking during startup
    /// probing. Individual network-enabled requests still run the full probe.
    pub fn new(images_dir: PathBuf) -> Self {
        Self {
            images_dir,
            socket_timeout: Duration::from_secs(5),
            network: Arc::new(NetworkManager::default()),
            network_required: false,
            version: Mutex::new(None),
        }
    }

    /// Create a spawner whose startup probe includes network prerequisites
    /// when at least one loaded policy enables Firecracker networking.
    pub fn with_network_requirement(images_dir: PathBuf, network_required: bool) -> Self {
        Self {
            network_required,
            ..Self::new(images_dir)
        }
    }

    async fn network_probe_ready(&self) -> Result<bool> {
        if !self.network_required {
            return Ok(true);
        }
        self.network.probe().await
    }

    async fn start(
        &self,
        request: BackendSpawnRequest,
    ) -> std::result::Result<DynBackendInstance, SpawnFailure> {
        validate_regular_file(&request.binary_path, "firecracker binary")?;
        validate_regular_file(&request.storage.rootfs_path, "rootfs")?;
        validate_regular_file(&self.images_dir.join("vmlinux"), "vmlinux")?;
        let api_socket = request.run_dir.path().join("api.sock");
        let guest_socket = request.run_dir.path().join("vsock.uds");
        let pid_file = request.run_dir.path().join("firecracker.pid");
        let stopped_marker = stopped_marker(request.run_dir.path());
        let network_file = request.run_dir.path().join("network.json");
        let network_temp_file = network_metadata_temp(&network_file);
        remove_if_exists(&api_socket).await?;
        remove_if_exists(&guest_socket).await?;
        remove_file_if_exists(&stopped_marker).await?;
        remove_if_exists(&network_file).await?;
        remove_if_exists(&network_temp_file).await?;
        let fc_config = request
            .backend
            .firecracker
            .as_ref()
            .cloned()
            .unwrap_or_default();
        let network = if fc_config.enable_network {
            if !self.network.probe().await? {
                return Err(BlazeError::BackendError {
                    msg: "Firecracker networking is unavailable; it requires Linux root and executable ip, sysctl, and iptables commands".to_string(),
                }
                .into());
            }
            match self
                .network
                .create(request.instance_id, |slot| {
                    write_network_metadata(&network_file, slot)
                })
                .await
            {
                Ok(network) => Some(network),
                Err(error) => {
                    let (source, residual) = error.into_parts();
                    if let Some(network) = residual {
                        let owner: DynBackendInstance = Arc::new(FirecrackerInstance::new(
                            request.instance_id,
                            None,
                            runtime_files(
                                api_socket,
                                guest_socket,
                                pid_file,
                                stopped_marker,
                                network_file,
                            ),
                            Some(network),
                            self.network.clone(),
                        ));
                        return Err(SpawnFailure::compensate_started(source, owner).await);
                    }
                    if let Err(cleanup) = remove_if_exists(&network_file).await {
                        let owner: DynBackendInstance = Arc::new(FirecrackerInstance::new(
                            request.instance_id,
                            None,
                            runtime_files(
                                api_socket,
                                guest_socket,
                                pid_file,
                                stopped_marker,
                                network_file,
                            ),
                            None,
                            self.network.clone(),
                        ));
                        return Err(SpawnFailure::compensate_started(
                            BlazeError::BackendError {
                                msg: format!(
                                    "{source}; network metadata cleanup failed: {cleanup}"
                                ),
                            },
                            owner,
                        )
                        .await);
                    }
                    if let Err(cleanup) = remove_if_exists(&network_temp_file).await {
                        let owner: DynBackendInstance = Arc::new(FirecrackerInstance::new(
                            request.instance_id,
                            None,
                            runtime_files(
                                api_socket,
                                guest_socket,
                                pid_file,
                                stopped_marker,
                                network_file,
                            ),
                            None,
                            self.network.clone(),
                        ));
                        return Err(SpawnFailure::compensate_started(
                            BlazeError::BackendError {
                                msg: format!(
                                    "{source}; temporary network metadata cleanup failed: {cleanup}"
                                ),
                            },
                            owner,
                        )
                        .await);
                    }
                    return Err(source.into());
                }
            }
        } else {
            None
        };

        let mut command = build_launch_command(
            &request.binary_path,
            network.as_ref(),
            &api_socket,
            request.instance_id,
        );
        request.run_dir.inherit_into(&mut command);
        let config_path = match write_vm_config(
            &self.images_dir,
            &request,
            &fc_config,
            &guest_socket,
            network.as_ref(),
        ) {
            Ok(path) => path,
            Err(error) => {
                return Err(self
                    .compensate_before_spawn(
                        request.instance_id,
                        runtime_files(
                            api_socket,
                            guest_socket,
                            pid_file,
                            stopped_marker,
                            network_file,
                        ),
                        network,
                        error,
                    )
                    .await);
            }
        };
        command.arg("--config-file").arg(config_path);
        if let Err(error) =
            configure_logs(&mut command, request.run_dir.path(), fc_config.serial_log)
        {
            return Err(self
                .compensate_before_spawn(
                    request.instance_id,
                    runtime_files(
                        api_socket,
                        guest_socket,
                        pid_file,
                        stopped_marker,
                        network_file,
                    ),
                    network,
                    error,
                )
                .await);
        }
        command.env("BLAZE_INSTANCE_ID", request.instance_id.to_string());
        if let Some(slot) = network.as_ref()
            && let Err(error) =
                write_network_record(&network_file, slot, NetworkProcessState::Launching)
        {
            return Err(self
                .compensate_before_spawn(
                    request.instance_id,
                    runtime_files(
                        api_socket,
                        guest_socket,
                        pid_file,
                        stopped_marker,
                        network_file,
                    ),
                    network,
                    error,
                )
                .await);
        }
        let pid_handoff = match configure_pid_handoff(&mut command, &pid_file) {
            Ok(pid_handoff) => pid_handoff,
            Err(error) => {
                return Err(self
                    .compensate_before_spawn(
                        request.instance_id,
                        runtime_files(
                            api_socket,
                            guest_socket,
                            pid_file,
                            stopped_marker,
                            network_file,
                        ),
                        network,
                        error,
                    )
                    .await);
            }
        };
        let child = command.spawn();
        drop(pid_handoff);
        let mut child = match child {
            Ok(child) => child,
            Err(source) => {
                return Err(self
                    .compensate_before_spawn(
                        request.instance_id,
                        runtime_files(
                            api_socket,
                            guest_socket,
                            pid_file,
                            stopped_marker,
                            network_file,
                        ),
                        network,
                        source.into(),
                    )
                    .await);
            }
        };
        if let Err(error) = wait_for_socket(&api_socket, &mut child, self.socket_timeout).await {
            let owner: DynBackendInstance = Arc::new(FirecrackerInstance::new(
                request.instance_id,
                Some(child),
                runtime_files(
                    api_socket,
                    guest_socket,
                    pid_file,
                    stopped_marker,
                    network_file,
                ),
                network,
                self.network.clone(),
            ));
            return Err(SpawnFailure::compensate_started(error, owner).await);
        }

        let instance = FirecrackerInstance::new(
            request.instance_id,
            Some(child),
            configured_runtime_files(
                runtime_files(
                    api_socket,
                    guest_socket,
                    pid_file,
                    stopped_marker,
                    network_file,
                ),
                fc_config.enable_vsock,
            ),
            network,
            self.network.clone(),
        );
        Ok(Arc::new(instance))
    }

    async fn compensate_before_spawn(
        &self,
        instance_id: Uuid,
        files: FirecrackerRuntimeFiles,
        network: Option<NetworkSlot>,
        source: BlazeError,
    ) -> SpawnFailure {
        if network.is_none() {
            return SpawnFailure::clean(source);
        }
        let owner: DynBackendInstance = Arc::new(FirecrackerInstance::new(
            instance_id,
            None,
            files,
            network,
            self.network.clone(),
        ));
        SpawnFailure::compensate_started(source, owner).await
    }
}

#[async_trait]
impl BackendSpawner for FirecrackerSpawner {
    async fn prepare_spawn(&self, run_dir: &OwnedRunDir) -> Result<()> {
        prepare_pid_handoff(&run_dir.path().join("firecracker.pid"))
    }

    async fn spawn(
        &self,
        request: BackendSpawnRequest,
    ) -> std::result::Result<DynBackendInstance, SpawnFailure> {
        self.start(request).await
    }

    async fn probe(&self, binary_path: &Path) -> Result<bool> {
        if !binary_path.is_file() || !executable_in_path("unshare") {
            return Ok(false);
        }
        if !self.network_probe_ready().await? {
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

    async fn cleanup_orphan(&self, instance_id: Uuid, run_dir: &OwnedRunDir) -> Result<()> {
        cleanup_orphan_run_dir_with(instance_id, run_dir.path(), &self.network).await
    }
}

struct FirecrackerInstance {
    instance_id: Uuid,
    child: Mutex<Option<Child>>,
    exit_result: Mutex<Option<SpawnResult>>,
    files: FirecrackerRuntimeFiles,
    network: Mutex<Option<NetworkSlot>>,
    network_manager: Arc<NetworkManager>,
    cleanup_complete: AtomicBool,
    killed: AtomicBool,
}

struct FirecrackerRuntimeFiles {
    api_socket: PathBuf,
    guest_socket: PathBuf,
    pid_file: PathBuf,
    stopped_marker: PathBuf,
    network_file: PathBuf,
}

fn runtime_files(
    api_socket: PathBuf,
    guest_socket: PathBuf,
    pid_file: PathBuf,
    stopped_marker: PathBuf,
    network_file: PathBuf,
) -> FirecrackerRuntimeFiles {
    FirecrackerRuntimeFiles {
        api_socket,
        guest_socket,
        pid_file,
        stopped_marker,
        network_file,
    }
}

fn configured_runtime_files(
    mut files: FirecrackerRuntimeFiles,
    enable_vsock: bool,
) -> FirecrackerRuntimeFiles {
    if !enable_vsock {
        files.guest_socket = PathBuf::new();
    }
    files
}

impl FirecrackerInstance {
    fn new(
        instance_id: Uuid,
        child: Option<Child>,
        files: FirecrackerRuntimeFiles,
        network: Option<NetworkSlot>,
        network_manager: Arc<NetworkManager>,
    ) -> Self {
        Self {
            instance_id,
            child: Mutex::new(child),
            exit_result: Mutex::new(None),
            files,
            network: Mutex::new(network),
            network_manager,
            cleanup_complete: AtomicBool::new(false),
            killed: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl BackendInstance for FirecrackerInstance {
    fn backend(&self) -> BackendKind {
        BackendKind::Firecracker
    }

    fn guest_socket_path(&self) -> &Path {
        &self.files.guest_socket
    }

    async fn try_wait(&self) -> Result<Option<SpawnResult>> {
        let result = {
            let mut guard = self.child.lock().await;
            let Some(child) = guard.as_mut() else {
                let result = self.exit_result.lock().await.unwrap_or(SpawnResult {
                    instance_id: self.instance_id,
                    exit_code: None,
                    signal: None,
                });
                drop(guard);
                self.cleanup().await?;
                return Ok(Some(result));
            };
            let Some(status) = child.try_wait()? else {
                return Ok(None);
            };
            record_backend_stopped(&self.files.stopped_marker).await?;
            let result = spawn_result(self.instance_id, status);
            *self.exit_result.lock().await = Some(result);
            *guard = None;
            result
        };
        self.cleanup().await?;
        Ok(Some(result))
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
        record_backend_stopped(&self.files.stopped_marker).await?;
        *guard = None;
        drop(guard);
        self.cleanup().await?;
        self.killed.store(true, Ordering::Release);
        Ok(())
    }
}

impl FirecrackerInstance {
    async fn cleanup(&self) -> Result<()> {
        if self.cleanup_complete.load(Ordering::Acquire) {
            return Ok(());
        }
        remove_if_exists(&self.files.api_socket).await?;
        remove_if_exists(&self.files.guest_socket).await?;
        remove_if_exists(&self.files.pid_file).await?;
        let mut network = self.network.lock().await;
        if let Some(slot) = network.as_ref().cloned() {
            self.network_manager.destroy(&slot).await?;
            *network = None;
        }
        remove_if_exists(&self.files.network_file).await?;
        remove_if_exists(&network_metadata_temp(&self.files.network_file)).await?;
        self.cleanup_complete.store(true, Ordering::Release);
        Ok(())
    }
}

fn write_vm_config(
    images_dir: &Path,
    request: &BackendSpawnRequest,
    config: &FirecrackerConfig,
    guest_socket: &Path,
    network: Option<&NetworkSlot>,
) -> Result<PathBuf> {
    let vcpus = config
        .vcpus
        .or(request.vm.as_ref().map(|vm| vm.vcpus))
        .unwrap_or(1);
    let memory_mib = resolve_memory(config, request.vm.as_ref())?;
    let mut boot_args = config.boot_args.clone();
    if network.is_some() {
        let network_arguments = boot_args
            .split_whitespace()
            .filter(|argument| argument.starts_with("ip="))
            .collect::<Vec<_>>();
        match network_arguments.as_slice() {
            [] => {
                boot_args.push(' ');
                boot_args.push_str(NETWORK_BOOT_IP);
            }
            [argument] if *argument == NETWORK_BOOT_IP => {}
            arguments => {
                return Err(BlazeError::BackendError {
                    msg: format!(
                        "Firecracker networking requires exactly {NETWORK_BOOT_IP:?}, found {}",
                        arguments
                            .iter()
                            .map(|argument| format!("{argument:?}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                });
            }
        }
    }
    let mut value = serde_json::json!({
        "boot-source": {
            "kernel_image_path": path_string(&images_dir.join("vmlinux"), "vmlinux")?,
            "boot_args": boot_args
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
    if let Some(network) = network {
        value["network-interfaces"] = serde_json::json!([{
            "iface_id": "eth0",
            "guest_mac": "02:FC:00:00:00:02",
            "host_dev_name": network.tap_name()
        }]);
    }
    let path = request.run_dir.path().join("vmconfig.json");
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

fn build_launch_command(
    binary: &Path,
    network: Option<&NetworkSlot>,
    api_socket: &Path,
    instance_id: Uuid,
) -> Command {
    #[cfg(target_os = "linux")]
    let mut command = if let Some(network) = network {
        let mut command = Command::new("ip");
        command
            .arg("netns")
            .arg("exec")
            .arg(network.netns())
            .arg("unshare")
            .arg("--mount")
            .arg("--propagation")
            .arg("private")
            .arg("--")
            .arg(binary);
        command
    } else {
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
    let mut command = {
        let _ = network;
        Command::new(binary)
    };
    command.arg("--api-sock").arg(api_socket);
    command.arg("--id").arg(format!("fc-{instance_id}"));
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
    if path.as_os_str().is_empty() {
        return Ok(());
    }
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn write_network_metadata(path: &Path, network: &NetworkSlot) -> Result<()> {
    write_network_record(path, network, NetworkProcessState::PreSpawn)
}

fn write_network_record(
    path: &Path,
    network: &NetworkSlot,
    process_state: NetworkProcessState,
) -> Result<()> {
    let parent = path.parent().ok_or_else(|| BlazeError::BackendError {
        msg: format!("network metadata has no parent: {}", path.display()),
    })?;
    let temporary = network_metadata_temp(path);
    (|| -> Result<()> {
        let bytes = serde_json::to_vec_pretty(&NetworkRecord {
            slot: network.slot(),
            owner: network.owner(),
            process_state,
        })
        .map_err(|error| BlazeError::BackendError {
            msg: format!("serialize network metadata: {error}"),
        })?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    })()
}

fn read_network_metadata(path: &Path) -> Result<(NetworkSlot, NetworkProcessState)> {
    let record: NetworkRecord = serde_json::from_slice(&std::fs::read(path)?).map_err(|error| {
        BlazeError::BackendError {
            msg: format!("parse network metadata {}: {error}", path.display()),
        }
    })?;
    Ok((
        NetworkSlot::from_record(record.slot, record.owner)?,
        record.process_state,
    ))
}

fn network_metadata_temp(path: &Path) -> PathBuf {
    path.with_extension("json.tmp")
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

async fn cleanup_orphan_run_dir_with(
    instance_id: Uuid,
    run_dir: &Path,
    network_manager: &NetworkManager,
) -> Result<()> {
    let stopped_marker = stopped_marker(run_dir);
    let pid_file = run_dir.join("firecracker.pid");
    let network_file = run_dir.join("network.json");
    let network_temp_file = network_metadata_temp(&network_file);
    let record_path = if network_file.is_file() {
        Some(network_file.as_path())
    } else if network_temp_file.is_file() {
        Some(network_temp_file.as_path())
    } else {
        None
    };
    let network_record = match record_path {
        Some(path) => match read_network_metadata(path) {
            Ok((network, state)) => {
                if network.owner() != instance_id {
                    return Err(BlazeError::BackendError {
                        msg: format!(
                            "network record owner {} does not match instance {instance_id}",
                            network.owner()
                        ),
                    });
                }
                Some((network, Some(state)))
            }
            Err(error) if path == network_temp_file.as_path() && !network_file.exists() => {
                match network_manager.find_by_owner(instance_id).await? {
                    // The namespace name proves ownership, but it cannot prove
                    // whether the backend crossed the spawn boundary.
                    Some(network) => Some((network, None)),
                    None => return Err(error),
                }
            }
            Err(error) => return Err(error),
        },
        None => network_manager
            .find_by_owner(instance_id)
            .await?
            .map(|network| (network, None)),
    };
    let process_may_exist = pid_file.exists()
        || network_record
            .as_ref()
            .is_none_or(|(_, state)| *state != Some(NetworkProcessState::PreSpawn));
    if !stopped_marker.is_file() {
        #[cfg(target_os = "linux")]
        {
            if process_may_exist {
                terminate_recorded_process(instance_id, &pid_file, "firecracker").await?;
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = instance_id;
            if process_may_exist {
                return Err(BlazeError::BackendError {
                    msg: format!(
                        "cannot validate Firecracker orphan {} outside Linux",
                        pid_file.display()
                    ),
                });
            }
        }
        record_backend_stopped(&stopped_marker).await?;
    }

    if let Some((network, _)) = network_record {
        network_manager.destroy(&network).await?;
        remove_if_exists(&network_file).await?;
    }
    remove_if_exists(&network_temp_file).await?;
    remove_if_exists(&run_dir.join("api.sock")).await?;
    remove_if_exists(&run_dir.join("vsock.uds")).await?;
    remove_if_exists(&pid_file).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use blaze_core::storage::StorageSlot;

    use crate::spawner::netns::{IpCommandRunner, IpOutput, NetworkManager, test_network_slot};

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
    fn launch_command_uses_the_sandbox_uuid_as_the_backend_id() {
        let instance_id = Uuid::new_v4();
        let command = build_launch_command(
            Path::new("/usr/bin/firecracker"),
            None,
            Path::new("/proc/self/fd/17/api.sock"),
            instance_id,
        );
        let arguments = command.as_std().get_args().collect::<Vec<_>>();
        let id_index = arguments
            .iter()
            .position(|argument| *argument == "--id")
            .expect("--id argument");
        let expected = format!("fc-{instance_id}");

        assert_eq!(arguments.get(id_index + 1), Some(&expected.as_ref()));
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
            None,
        )
        .expect("write config");
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).expect("read config"))
                .expect("parse config");
        assert!(value.get("network-interfaces").is_none());
    }

    #[test]
    fn vm_config_and_reported_guest_transport_agree() {
        let temp = tempfile::tempdir().expect("temp");
        let request = spawn_request(temp.path());
        let socket = temp.path().join("vsock.uds");
        let disabled = FirecrackerConfig::default();
        let disabled_path = write_vm_config(
            &temp.path().join("images"),
            &request,
            &disabled,
            &socket,
            None,
        )
        .expect("disabled config");
        let disabled_value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(disabled_path).expect("read disabled config"))
                .expect("parse disabled config");
        assert!(disabled_value.get("vsock").is_none());
        let files = configured_runtime_files(
            runtime_files(
                temp.path().join("api.sock"),
                socket.clone(),
                temp.path().join("firecracker.pid"),
                stopped_marker(temp.path()),
                temp.path().join("network.json"),
            ),
            disabled.enable_vsock,
        );
        assert!(files.guest_socket.as_os_str().is_empty());

        let enabled = FirecrackerConfig {
            enable_vsock: true,
            ..FirecrackerConfig::default()
        };
        let enabled_path = write_vm_config(
            &temp.path().join("images"),
            &request,
            &enabled,
            &socket,
            None,
        )
        .expect("enabled config");
        let enabled_value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(enabled_path).expect("read enabled config"))
                .expect("parse enabled config");
        assert_eq!(
            enabled_value["vsock"]["uds_path"],
            path_string(&socket, "socket").expect("socket path")
        );
        let files = configured_runtime_files(
            runtime_files(
                temp.path().join("api.sock"),
                socket.clone(),
                temp.path().join("firecracker.pid"),
                stopped_marker(temp.path()),
                temp.path().join("network.json"),
            ),
            enabled.enable_vsock,
        );
        assert_eq!(files.guest_socket, socket);
    }

    #[test]
    fn vm_config_wires_an_allocated_network_slot() {
        let temp = tempfile::tempdir().expect("temp");
        let request = spawn_request(temp.path());
        let network = test_network_slot(0);

        let path = write_vm_config(
            &temp.path().join("images"),
            &request,
            &FirecrackerConfig::default(),
            &temp.path().join("guest.sock"),
            Some(&network),
        )
        .expect("write config");
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).expect("read config"))
                .expect("parse config");
        assert_eq!(value["network-interfaces"][0]["iface_id"], "eth0");
        assert_eq!(value["network-interfaces"][0]["host_dev_name"], "tap0");
        assert!(
            value["boot-source"]["boot_args"]
                .as_str()
                .expect("boot args")
                .contains("::eth0:off")
        );
    }

    #[test]
    fn vm_config_accepts_the_matching_network_boot_argument() {
        let temp = tempfile::tempdir().expect("temp");
        let request = spawn_request(temp.path());
        let network = test_network_slot(0);
        let config = FirecrackerConfig {
            boot_args: format!("console=ttyS0 {NETWORK_BOOT_IP}"),
            ..FirecrackerConfig::default()
        };

        write_vm_config(
            &temp.path().join("images"),
            &request,
            &config,
            &temp.path().join("guest.sock"),
            Some(&network),
        )
        .expect("matching network boot argument");
    }

    #[test]
    fn vm_config_rejects_an_incompatible_network_boot_argument() {
        let temp = tempfile::tempdir().expect("temp");
        let request = spawn_request(temp.path());
        let network = test_network_slot(0);
        let config = FirecrackerConfig {
            boot_args: "console=ttyS0 ip=dhcp".to_string(),
            ..FirecrackerConfig::default()
        };

        let error = write_vm_config(
            &temp.path().join("images"),
            &request,
            &config,
            &temp.path().join("guest.sock"),
            Some(&network),
        )
        .expect_err("incompatible network boot argument");

        assert!(error.to_string().contains("requires"));
        assert!(error.to_string().contains("ip=dhcp"));
    }

    #[test]
    fn vm_config_rejects_conflicting_network_boot_arguments() {
        let temp = tempfile::tempdir().expect("temp");
        let request = spawn_request(temp.path());
        let network = test_network_slot(0);
        let config = FirecrackerConfig {
            boot_args: format!("console=ttyS0 {NETWORK_BOOT_IP} ip=dhcp"),
            ..FirecrackerConfig::default()
        };

        let error = write_vm_config(
            &temp.path().join("images"),
            &request,
            &config,
            &temp.path().join("guest.sock"),
            Some(&network),
        )
        .expect_err("conflicting network boot arguments");

        assert!(error.to_string().contains("exactly"));
        assert!(error.to_string().contains("ip=dhcp"));
    }

    #[test]
    fn network_metadata_is_published_atomically() {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("network.json");
        let slot = test_network_slot(7);

        write_network_metadata(&path, &slot).expect("write metadata");

        let (stored, state) = read_network_metadata(&path).expect("parse metadata");
        assert_eq!(stored, slot);
        assert_eq!(state, NetworkProcessState::PreSpawn);
        assert!(!network_metadata_temp(&path).exists());
    }

    #[test]
    fn network_metadata_records_launch_intent_before_spawn() {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("network.json");
        let slot = test_network_slot(7);
        write_network_metadata(&path, &slot).expect("write pre-spawn metadata");

        write_network_record(&path, &slot, NetworkProcessState::Launching)
            .expect("record launch intent");

        let (stored, state) = read_network_metadata(&path).expect("parse metadata");
        assert_eq!(stored, slot);
        assert_eq!(state, NetworkProcessState::Launching);
        assert!(!network_metadata_temp(&path).exists());
    }

    #[test]
    fn network_metadata_rejects_out_of_range_slots() {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("network.json");
        std::fs::write(
            &path,
            br#"{"slot":16383,"owner":"00000000-0000-0000-0000-000000000001"}"#,
        )
        .expect("metadata");

        let error = read_network_metadata(&path).expect_err("invalid slot");

        assert!(error.to_string().contains("outside"));
    }

    #[tokio::test]
    async fn network_cleanup_failure_retains_a_retryable_backend_owner() {
        let temp = tempfile::tempdir().expect("temp");
        let network_file = temp.path().join("network.json");
        let slot = test_network_slot(0);
        write_network_metadata(&network_file, &slot).expect("network metadata");
        let namespace = format!("{}\n", slot.netns());
        let runner = Arc::new(TestIpRunner::with_responses([
            ip_success(namespace.as_bytes()),
            ip_failure("delete peer failed"),
            ip_success(namespace.as_bytes()),
            ip_success(b""),
            ip_success(b""),
        ]));
        let network_manager = Arc::new(NetworkManager::with_runner(runner.clone()));
        let owner: DynBackendInstance = Arc::new(FirecrackerInstance::new(
            slot.owner(),
            None,
            runtime_files(
                temp.path().join("api.sock"),
                temp.path().join("guest.sock"),
                temp.path().join("firecracker.pid"),
                stopped_marker(temp.path()),
                network_file.clone(),
            ),
            Some(slot.clone()),
            network_manager,
        ));

        owner.kill().await.expect_err("first cleanup must fail");
        assert!(network_file.exists());
        owner.kill().await.expect("retry cleanup");
        assert!(!network_file.exists());
        assert!(
            runner
                .calls()
                .iter()
                .any(|args| args == &["netns", "del", slot.netns()])
        );
        assert!(
            !runner
                .calls()
                .iter()
                .any(|args| args == &["link", "del", "blz-veth-0"])
        );
    }

    #[tokio::test]
    async fn try_wait_retries_cleanup_after_observing_process_exit() {
        let temp = tempfile::tempdir().expect("temp");
        let network_file = temp.path().join("network.json");
        let slot = test_network_slot(0);
        write_network_metadata(&network_file, &slot).expect("network metadata");
        let namespace = format!("{}\n", slot.netns());
        let runner = Arc::new(TestIpRunner::with_responses([
            ip_success(namespace.as_bytes()),
            ip_failure("delete peer failed"),
            ip_success(namespace.as_bytes()),
            ip_success(b""),
            ip_success(b""),
        ]));
        let child = Command::new("sh")
            .arg("-c")
            .arg("exit 7")
            .spawn()
            .expect("spawn child");
        let instance = FirecrackerInstance::new(
            slot.owner(),
            Some(child),
            runtime_files(
                temp.path().join("api.sock"),
                temp.path().join("guest.sock"),
                temp.path().join("firecracker.pid"),
                stopped_marker(temp.path()),
                network_file.clone(),
            ),
            Some(slot.clone()),
            Arc::new(NetworkManager::with_runner(runner.clone())),
        );

        let first_error = loop {
            match instance.try_wait().await {
                Ok(None) => tokio::time::sleep(Duration::from_millis(5)).await,
                Ok(Some(result)) => {
                    panic!("cleanup failure must not report completion: {result:?}")
                }
                Err(error) => break error,
            }
        };
        assert!(first_error.to_string().contains("delete peer failed"));
        assert!(network_file.exists());

        let result = instance
            .try_wait()
            .await
            .expect("retry cleanup")
            .expect("completed process");
        assert_eq!(result.exit_code, Some(7));
        assert!(!network_file.exists());
        assert!(
            runner
                .calls()
                .iter()
                .any(|args| args == &["netns", "del", slot.netns()])
        );
    }

    #[tokio::test]
    async fn stopped_orphan_still_releases_recorded_network() {
        let temp = tempfile::tempdir().expect("temp");
        record_backend_stopped(&stopped_marker(temp.path()))
            .await
            .expect("stopped marker");
        let network_file = temp.path().join("network.json");
        let network = test_network_slot(0);
        write_network_metadata(&network_file, &network).expect("network metadata");
        let namespace = format!("{}\n", network.netns());
        let runner = Arc::new(TestIpRunner::with_responses([
            ip_success(namespace.as_bytes()),
            ip_success(b""),
            ip_success(b""),
        ]));
        let network_manager = NetworkManager::with_runner(runner.clone());

        cleanup_orphan_run_dir_with(network.owner(), temp.path(), &network_manager)
            .await
            .expect("orphan cleanup");

        assert!(!network_file.exists());
        let calls = runner.calls();
        assert!(calls.iter().any(|args| {
            args == &[
                "netns",
                "exec",
                network.netns(),
                "ip",
                "link",
                "del",
                "blz-vpeer-0",
            ]
        }));
        assert!(
            calls
                .iter()
                .any(|args| args == &["netns", "del", network.netns()])
        );
    }

    #[tokio::test]
    async fn orphan_cleanup_recovers_a_complete_temporary_network_record() {
        let temp = tempfile::tempdir().expect("temp");
        record_backend_stopped(&stopped_marker(temp.path()))
            .await
            .expect("stopped marker");
        let network_file = temp.path().join("network.json");
        let network_temp_file = network_metadata_temp(&network_file);
        let network = test_network_slot(0);
        let bytes = serde_json::to_vec(&NetworkRecord {
            slot: network.slot(),
            owner: network.owner(),
            process_state: NetworkProcessState::PreSpawn,
        })
        .expect("serialize metadata");
        std::fs::write(&network_temp_file, bytes).expect("temporary metadata");
        let namespace = format!("{}\n", network.netns());
        let runner = Arc::new(TestIpRunner::with_responses([
            ip_success(namespace.as_bytes()),
            ip_success(b""),
            ip_success(b""),
        ]));
        let network_manager = NetworkManager::with_runner(runner.clone());

        cleanup_orphan_run_dir_with(network.owner(), temp.path(), &network_manager)
            .await
            .expect("orphan cleanup");

        assert!(!network_file.exists());
        assert!(!network_temp_file.exists());
        assert!(
            runner
                .calls()
                .iter()
                .any(|args| args == &["netns", "del", network.netns()])
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn orphan_cleanup_retains_a_truncated_network_record_without_pid_proof() {
        let temp = tempfile::tempdir().expect("temp");
        let network_file = temp.path().join("network.json");
        let network_temp_file = network_metadata_temp(&network_file);
        std::fs::write(&network_temp_file, b"{").expect("truncated metadata");
        let network = test_network_slot(0);
        let namespace = format!("{}\n", network.netns());
        let runner = Arc::new(TestIpRunner::with_responses([ip_success(
            namespace.as_bytes(),
        )]));
        let network_manager = NetworkManager::with_runner(runner.clone());

        let error = cleanup_orphan_run_dir_with(network.owner(), temp.path(), &network_manager)
            .await
            .expect_err("unknown launch state must fail closed");

        assert!(error.to_string().contains("missing PID handoff"));
        assert!(network_temp_file.exists());
        assert!(!stopped_marker(temp.path()).exists());
        assert_eq!(
            runner.calls(),
            vec![vec!["netns".to_string(), "list".to_string()]]
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn orphan_cleanup_retains_an_unrecorded_namespace_without_pid_proof() {
        let temp = tempfile::tempdir().expect("temp");
        let network = test_network_slot(0);
        let namespace = format!("{}\n", network.netns());
        let runner = Arc::new(TestIpRunner::with_responses([ip_success(
            namespace.as_bytes(),
        )]));
        let network_manager = NetworkManager::with_runner(runner.clone());

        let error = cleanup_orphan_run_dir_with(network.owner(), temp.path(), &network_manager)
            .await
            .expect_err("unknown launch state must fail closed");

        assert!(error.to_string().contains("missing PID handoff"));
        assert!(!stopped_marker(temp.path()).exists());
        assert_eq!(
            runner.calls(),
            vec![vec!["netns".to_string(), "list".to_string()]]
        );
    }

    #[tokio::test]
    async fn stopped_orphan_releases_an_unrecorded_owner_namespace() {
        let temp = tempfile::tempdir().expect("temp");
        record_backend_stopped(&stopped_marker(temp.path()))
            .await
            .expect("stopped marker");
        let network = test_network_slot(0);
        let namespace = format!("{}\n", network.netns());
        let runner = Arc::new(TestIpRunner::with_responses([
            ip_success(namespace.as_bytes()),
            ip_success(namespace.as_bytes()),
            ip_success(b""),
            ip_success(b""),
        ]));
        let network_manager = NetworkManager::with_runner(runner.clone());

        cleanup_orphan_run_dir_with(network.owner(), temp.path(), &network_manager)
            .await
            .expect("stopped process permits network recovery");

        assert!(
            runner
                .calls()
                .iter()
                .any(|args| args == &["netns", "del", network.netns()])
        );
    }

    #[tokio::test]
    async fn network_record_owner_mismatch_issues_no_host_commands() {
        let temp = tempfile::tempdir().expect("temp");
        let network_file = temp.path().join("network.json");
        let network = test_network_slot(0);
        write_network_metadata(&network_file, &network).expect("network metadata");
        let runner = Arc::new(TestIpRunner::default());
        let network_manager = NetworkManager::with_runner(runner.clone());

        let error = cleanup_orphan_run_dir_with(Uuid::from_u128(2), temp.path(), &network_manager)
            .await
            .expect_err("mismatched owner must fail");

        assert!(error.to_string().contains("does not match instance"));
        assert!(network_file.exists());
        assert!(runner.calls().is_empty());
    }

    #[tokio::test]
    async fn stale_network_record_does_not_delete_a_reused_slot() {
        let temp = tempfile::tempdir().expect("temp");
        record_backend_stopped(&stopped_marker(temp.path()))
            .await
            .expect("stopped marker");
        let network_file = temp.path().join("network.json");
        let old_network = test_network_slot(0);
        write_network_metadata(&network_file, &old_network).expect("network metadata");
        let new_network =
            NetworkSlot::from_record(0, Uuid::from_u128(2)).expect("new network owner");
        let namespace = format!("{}\n", new_network.netns());
        let runner = Arc::new(TestIpRunner::with_responses([ip_success(
            namespace.as_bytes(),
        )]));
        let network_manager = NetworkManager::with_runner(runner.clone());

        cleanup_orphan_run_dir_with(old_network.owner(), temp.path(), &network_manager)
            .await
            .expect("retire stale record");

        assert!(!network_file.exists());
        let calls = runner.calls();
        assert_eq!(calls, vec![vec!["netns".to_string(), "list".to_string()]]);
    }

    #[tokio::test]
    async fn pre_spawn_orphan_releases_network_without_pid_metadata() {
        let temp = tempfile::tempdir().expect("temp");
        let network_file = temp.path().join("network.json");
        let network = test_network_slot(0);
        write_network_metadata(&network_file, &network).expect("network metadata");
        let namespace = format!("{}\n", network.netns());
        let runner = Arc::new(TestIpRunner::with_responses([
            ip_success(namespace.as_bytes()),
            ip_success(b""),
            ip_success(b""),
        ]));
        let network_manager = NetworkManager::with_runner(runner.clone());

        cleanup_orphan_run_dir_with(network.owner(), temp.path(), &network_manager)
            .await
            .expect("pre-spawn cleanup");

        assert!(!network_file.exists());
        assert!(stopped_marker(temp.path()).exists());
        assert!(
            runner
                .calls()
                .iter()
                .any(|args| args == &["netns", "del", network.netns()])
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn unconfirmed_process_ownership_retains_network_metadata() {
        let temp = tempfile::tempdir().expect("temp");
        let network_file = temp.path().join("network.json");
        let network = test_network_slot(0);
        write_network_record(&network_file, &network, NetworkProcessState::Launching)
            .expect("network metadata");
        let runner = Arc::new(TestIpRunner::default());
        let network_manager = NetworkManager::with_runner(runner.clone());

        let error = cleanup_orphan_run_dir_with(network.owner(), temp.path(), &network_manager)
            .await
            .expect_err("missing process metadata must block cleanup");

        assert!(error.to_string().contains("missing PID handoff"));
        assert!(network_file.exists());
        assert!(runner.calls().is_empty());
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
    async fn backend_probe_skips_network_checks_when_no_policy_enables_them() {
        let temp = tempfile::tempdir().expect("temp");
        let called = Arc::new(AtomicBool::new(false));
        let network = Arc::new(NetworkManager::with_runner(Arc::new(
            UnavailableNetworkRunner {
                called: called.clone(),
            },
        )));
        let spawner = FirecrackerSpawner {
            images_dir: temp.path().join("images"),
            socket_timeout: Duration::from_secs(1),
            network,
            network_required: false,
            version: Mutex::new(None),
        };

        assert!(spawner.network_probe_ready().await.expect("probe"));
        assert!(!called.load(Ordering::Acquire));
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
            Some(child),
            runtime_files(
                temp.path().join("api.sock"),
                temp.path().join("guest.sock"),
                pid_file.clone(),
                stopped_marker(temp.path()),
                temp.path().join("network.json"),
            ),
            None,
            Arc::new(NetworkManager::default()),
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

    fn spawn_request(root: &Path) -> BackendSpawnRequest {
        let instance_id = Uuid::new_v4();
        let run_dir = root.join("run");
        let slot_dir = root.join("slot");
        BackendSpawnRequest::new(
            SpawnRequest {
                instance_id,
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
            },
            OwnedRunDir::for_test(instance_id, run_dir),
        )
        .expect("matching backend request")
    }

    #[derive(Default)]
    struct TestIpRunner {
        responses: std::sync::Mutex<VecDeque<IpOutput>>,
        calls: std::sync::Mutex<Vec<Vec<String>>>,
    }

    struct UnavailableNetworkRunner {
        called: Arc<AtomicBool>,
    }

    #[async_trait]
    impl IpCommandRunner for UnavailableNetworkRunner {
        async fn output(&self, _args: &[String], _timeout: Duration) -> Result<IpOutput> {
            self.called.store(true, Ordering::Release);
            Ok(ip_failure("network commands unavailable"))
        }

        #[cfg(target_os = "linux")]
        fn executable_in_path(&self, _name: &str) -> bool {
            false
        }

        #[cfg(target_os = "linux")]
        fn has_network_admin(&self) -> bool {
            false
        }
    }

    impl TestIpRunner {
        fn with_responses<const N: usize>(responses: [IpOutput; N]) -> Self {
            Self {
                responses: std::sync::Mutex::new(responses.into()),
                calls: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<Vec<String>> {
            self.calls.lock().expect("calls lock").clone()
        }
    }

    #[async_trait]
    impl IpCommandRunner for TestIpRunner {
        async fn output(&self, args: &[String], _timeout: Duration) -> Result<IpOutput> {
            self.calls.lock().expect("calls lock").push(args.to_vec());
            Ok(self
                .responses
                .lock()
                .expect("responses lock")
                .pop_front()
                .unwrap_or_else(|| ip_success(b"")))
        }
    }

    fn ip_success(stdout: &[u8]) -> IpOutput {
        IpOutput {
            success: true,
            status: "exit status: 0".to_string(),
            stdout: stdout.to_vec(),
            stderr: Vec::new(),
        }
    }

    fn ip_failure(stderr: &str) -> IpOutput {
        IpOutput {
            success: false,
            status: "exit status: 1".to_string(),
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }
}
