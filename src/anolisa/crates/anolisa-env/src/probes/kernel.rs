//! Kernel information detection.

use super::{DetectError, EnvDetector};
use crate::{EnvFacts, KernelInfo};
use std::fs;
use std::path::Path;

pub struct KernelProbe;

impl EnvDetector for KernelProbe {
    fn name(&self) -> &str {
        "kernel"
    }

    fn priority(&self) -> u8 {
        20
    }

    fn detect(&self, facts: &mut EnvFacts) -> Result<(), DetectError> {
        let uname = nix_uname();
        let (major, minor, patch) = parse_version(&uname);

        facts.kernel = KernelInfo {
            version: uname.clone(),
            major,
            minor,
            patch,
            btf_available: Path::new("/sys/kernel/btf/vmlinux").exists(),
            cgroups_v2: is_cgroups_v2(),
            namespaces: Path::new("/proc/self/ns/mnt").exists(),
            landlock_abi: detect_landlock_abi(),
            kvm_available: Path::new("/dev/kvm").exists(),
        };

        Ok(())
    }
}

fn nix_uname() -> String {
    fs::read_to_string("/proc/version")
        .ok()
        .and_then(|v| v.split_whitespace().nth(2).map(|s| s.to_string()))
        .unwrap_or_else(|| "unknown".into())
}

fn parse_version(version: &str) -> (u32, u32, u32) {
    let parts: Vec<&str> = version.split(|c: char| !c.is_ascii_digit()).collect();
    let major = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minor = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let patch = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    (major, minor, patch)
}

fn is_cgroups_v2() -> bool {
    Path::new("/sys/fs/cgroup/cgroup.controllers").exists()
}

fn detect_landlock_abi() -> Option<u32> {
    // Landlock ABI version can be detected via the landlock syscall
    // For skeleton, just check if the sysfs entry exists
    if Path::new("/sys/kernel/security/landlock").exists() {
        Some(1) // Placeholder; real implementation would probe ABI version
    } else {
        None
    }
}
