//! Distribution detection via /etc/os-release.

use super::{DetectError, EnvDetector};
use crate::{DistroInfo, EnvFacts, PkgBase};
use std::fs;

pub struct DistroProbe;

impl EnvDetector for DistroProbe {
    fn name(&self) -> &str {
        "distro"
    }

    fn priority(&self) -> u8 {
        15
    }

    fn detect(&self, facts: &mut EnvFacts) -> Result<(), DetectError> {
        let content = fs::read_to_string("/etc/os-release").unwrap_or_default();
        let mut id = String::new();
        let mut version = String::new();
        let mut name = String::new();

        for line in content.lines() {
            if let Some(val) = line.strip_prefix("ID=") {
                id = val.trim_matches('"').to_string();
            } else if let Some(val) = line.strip_prefix("VERSION_ID=") {
                version = val.trim_matches('"').to_string();
            } else if let Some(val) = line.strip_prefix("NAME=") {
                name = val.trim_matches('"').to_string();
            }
        }

        let pkg_base = match id.as_str() {
            "alinux" | "anolis" | "fedora" | "rhel" | "centos" => PkgBase::Rpm,
            "ubuntu" | "debian" | "linuxmint" | "pop" => PkgBase::Deb,
            _ => PkgBase::Unknown,
        };

        facts.distro = DistroInfo {
            id,
            version,
            name,
            pkg_base,
        };

        Ok(())
    }
}
