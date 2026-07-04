//! Built-in framework-driver registry.
//!
//! The set of supported frameworks is closed and compiled in: a framework
//! is supported only if an ANOLISA release ships a driver for it. There is
//! no runtime plugin loading.

use super::claude_code::ClaudeCodeDriver;
use super::codex::CodexDriver;
use super::cosh::CoshDriver;
use super::driver::FrameworkDriver;
use super::hermes::HermesDriver;
use super::openclaw::OpenClawDriver;

/// Immutable collection of the built-in framework drivers.
pub struct DriverRegistry {
    drivers: Vec<Box<dyn FrameworkDriver>>,
}

impl DriverRegistry {
    /// Build the registry of all built-in drivers.
    pub fn builtin() -> Self {
        Self {
            drivers: vec![
                Box::new(OpenClawDriver::new()),
                Box::new(HermesDriver::new()),
                Box::new(CoshDriver::new()),
                Box::new(CodexDriver::new()),
                Box::new(ClaudeCodeDriver::new()),
            ],
        }
    }

    /// Look up a driver by framework name.
    pub fn get(&self, framework: &str) -> Option<&dyn FrameworkDriver> {
        self.drivers
            .iter()
            .find(|d| d.name() == framework)
            .map(|d| d.as_ref())
    }

    /// True iff a driver exists for `framework`.
    pub fn contains(&self, framework: &str) -> bool {
        self.get(framework).is_some()
    }

    /// Names of all registered frameworks.
    pub fn names(&self) -> Vec<&'static str> {
        self.drivers.iter().map(|d| d.name()).collect()
    }
}

impl Default for DriverRegistry {
    fn default() -> Self {
        Self::builtin()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_registers_all_shipped_drivers() {
        let reg = DriverRegistry::builtin();
        assert!(reg.contains("openclaw"));
        assert!(reg.contains("hermes"));
        assert!(reg.contains("cosh"));
        assert!(reg.contains("codex"));
        assert!(reg.contains("claude-code"));
        assert_eq!(
            reg.names(),
            vec!["openclaw", "hermes", "cosh", "codex", "claude-code"]
        );
    }

    #[test]
    fn get_unknown_framework_is_none() {
        let reg = DriverRegistry::builtin();
        // `qoder` ships an adapter bundle but no built-in driver yet.
        assert!(reg.get("qoder").is_none());
        assert!(reg.get("qwencode").is_none());
    }
}
