//! Agent-framework probe.
//!
//! Detection rules live here (not in component manifests) so that adding a new
//! framework only requires upgrading anolisa, not editing every component
//! manifest. Component manifests merely declare *which* frameworks they ship
//! adapters for; this probe decides *which* of those frameworks are actually
//! present on the host.
//!
//! Currently a stub: returns just `cosh` (first-party, always present) plus
//! best-effort signal-based detection scaffolding for `openclaw` / `hermes` /
//! `mcp` to be filled in by real probes later.

use super::{DetectError, EnvDetector};
use crate::{DetectedFramework, EnvFacts, FrameworkKind};

pub struct FrameworksProbe;

impl EnvDetector for FrameworksProbe {
    fn name(&self) -> &str {
        "frameworks"
    }

    fn priority(&self) -> u8 {
        90 // run after platform/kernel/distro
    }

    fn detect(&self, facts: &mut EnvFacts) -> Result<(), DetectError> {
        let mut found = Vec::new();

        // cosh is first-party and considered always present once anolisa is installed.
        found.push(DetectedFramework {
            name: "cosh".into(),
            kind: FrameworkKind::FirstParty,
            version: None,
            location: None,
        });

        // TODO(owner: env-detection, when: adapter auto-selection ships):
        // replace scaffolding with real signal-based detection rules.
        // openclaw: PATH lookup `openclaw` or `~/.openclaw/`
        // hermes:   PATH lookup `hermes`  or `/opt/hermes/`
        // mcp:      detect claude-desktop / cursor / claude-code config files

        facts.frameworks = found;
        Ok(())
    }
}
