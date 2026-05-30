use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub mod cache;
pub mod gate;
pub mod probes;

/// Top-level environment facts collected by all probes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvFacts {
    pub platform: Platform,
    pub nesting: Option<NestingInfo>,
    pub gpu: Option<GpuInfo>,
    pub tee: Option<TeeInfo>,
    pub kernel: KernelInfo,
    pub distro: DistroInfo,
    pub arch: Arch,
    pub capabilities: HashSet<Capability>,
    pub filesystem: FsInfo,
    /// Detected agent frameworks on this machine (empty = none detected).
    /// Drives `anolisa adapter scan` output and `--with-adapter=auto`.
    pub frameworks: Vec<DetectedFramework>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Platform {
    Physical,
    Vm(Hypervisor),
    Container(ContainerRuntime),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Hypervisor {
    Kvm,
    Xen,
    HyperV,
    VMware,
    Other(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ContainerRuntime {
    Runc,
    Gvisor,
    Kata,
    Firecracker,
    Other(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NestingInfo {
    pub outer: Platform,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuInfo {
    pub vendor: String,
    pub model: String,
    pub driver_version: Option<String>,
    pub compute_capable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TeeType {
    Tdx,
    Sev,
    Sgx,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeeInfo {
    pub tee_type: TeeType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KernelInfo {
    pub version: String,
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    pub btf_available: bool,
    pub cgroups_v2: bool,
    pub namespaces: bool,
    pub landlock_abi: Option<u32>,
    pub kvm_available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistroInfo {
    pub id: String,
    pub version: String,
    pub name: String,
    pub pkg_base: PkgBase,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PkgBase {
    Rpm,
    Deb,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Arch {
    X86_64,
    Aarch64,
    Riscv64,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum Capability {
    CapBpf,
    CapSysAdmin,
    CapNetAdmin,
    CapSysPtrace,
    Other(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsInfo {
    pub btrfs_available: bool,
    pub overlayfs_available: bool,
}

/// One agent framework detected on the host. The probe library defines the
/// detection rules; this struct just records the outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedFramework {
    /// Framework identifier: `cosh`, `openclaw`, `hermes`, `mcp`, ...
    pub name: String,
    /// Adapter classification — informs default install behavior.
    pub kind: FrameworkKind,
    /// Optional version string if the probe was able to determine one.
    pub version: Option<String>,
    /// Optional install path if the probe found a binary or config.
    pub location: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FrameworkKind {
    /// Bundled with ANOLISA (always present on a machine that has anolisa).
    FirstParty,
    /// External agent framework installed independently.
    ThirdParty,
    /// Open protocol the system speaks (e.g. MCP).
    Protocol,
}

impl EnvFacts {
    /// Create a minimal placeholder for testing / offline use.
    pub fn placeholder() -> Self {
        Self {
            platform: Platform::Physical,
            nesting: None,
            gpu: None,
            tee: None,
            kernel: KernelInfo {
                version: "unknown".into(),
                major: 0,
                minor: 0,
                patch: 0,
                btf_available: false,
                cgroups_v2: false,
                namespaces: false,
                landlock_abi: None,
                kvm_available: false,
            },
            distro: DistroInfo {
                id: "unknown".into(),
                version: "0".into(),
                name: "Unknown".into(),
                pkg_base: PkgBase::Unknown,
            },
            arch: Arch::X86_64,
            capabilities: HashSet::new(),
            filesystem: FsInfo {
                btrfs_available: false,
                overlayfs_available: false,
            },
            frameworks: vec![DetectedFramework {
                name: "cosh".into(),
                kind: FrameworkKind::FirstParty,
                version: None,
                location: None,
            }],
            timestamp: chrono::Utc::now(),
        }
    }
}
