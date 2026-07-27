//! Platform detection: Physical vs VM vs Container.

use super::{DetectError, EnvDetector};
use crate::{ContainerRuntime, EnvFacts, Hypervisor, Platform};
use std::fs;
use std::path::Path;

pub struct PlatformProbe;

impl EnvDetector for PlatformProbe {
    fn name(&self) -> &str {
        "platform"
    }

    fn priority(&self) -> u8 {
        10
    }

    fn detect(&self, facts: &mut EnvFacts) -> Result<(), DetectError> {
        // Container detection (highest priority — we might be inside a container inside a VM)
        if is_container() {
            facts.platform = Platform::Container(detect_container_runtime());
            return Ok(());
        }

        // VM detection
        if let Some(hypervisor) = detect_hypervisor() {
            facts.platform = Platform::Vm(hypervisor);
            return Ok(());
        }

        // Default to physical
        facts.platform = Platform::Physical;
        Ok(())
    }
}

fn is_container() -> bool {
    // Check common container indicators
    Path::new("/.dockerenv").exists()
        || Path::new("/run/.containerenv").exists()
        || has_container_cgroup()
}

fn has_container_cgroup() -> bool {
    fs::read_to_string("/proc/1/cgroup")
        .map(|content| {
            content.contains("/docker/")
                || content.contains("/kubepods/")
                || content.contains("/lxc/")
        })
        .unwrap_or(false)
}

fn detect_container_runtime() -> ContainerRuntime {
    // Check /run/.containerenv for podman
    // Check kernel cmdline for kata/firecracker
    if let Ok(cmdline) = fs::read_to_string("/proc/cmdline") {
        if cmdline.contains("kata") {
            return ContainerRuntime::Kata;
        }
        if cmdline.contains("firecracker") {
            return ContainerRuntime::Firecracker;
        }
    }

    // Check for gVisor (runsc)
    if let Ok(content) = fs::read_to_string("/proc/version") {
        if content.contains("gVisor") {
            return ContainerRuntime::Gvisor;
        }
    }

    ContainerRuntime::Runc
}

fn detect_hypervisor() -> Option<Hypervisor> {
    // Try systemd-detect-virt output (if available)
    if let Ok(output) = std::process::Command::new("systemd-detect-virt").output() {
        if output.status.success() {
            let virt = String::from_utf8_lossy(&output.stdout).trim().to_string();
            return match virt.as_str() {
                "kvm" => Some(Hypervisor::Kvm),
                "xen" => Some(Hypervisor::Xen),
                "microsoft" => Some(Hypervisor::HyperV),
                "vmware" => Some(Hypervisor::VMware),
                "none" => None,
                other => Some(Hypervisor::Other(other.to_string())),
            };
        }
    }

    // Fallback: check DMI
    if let Ok(product) = fs::read_to_string("/sys/class/dmi/id/product_name") {
        let product = product.trim().to_lowercase();
        if product.contains("kvm") || product.contains("qemu") {
            return Some(Hypervisor::Kvm);
        }
        if product.contains("vmware") {
            return Some(Hypervisor::VMware);
        }
        if product.contains("virtualbox") {
            return Some(Hypervisor::Other("virtualbox".into()));
        }
    }

    None
}
