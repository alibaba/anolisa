pub mod loader;
pub mod manager;
pub mod types;

pub use manager::SkillManager;
#[allow(unused_imports)]
pub use types::{SkillConfig, SkillLevel};

/// Configuration directory name used by copilot-shell / cosh.
pub const COPILOT_CONFIG_DIR: &str = ".copilot-shell";
/// Sub-directory containing skill definitions.
pub const SKILLS_DIR: &str = "skills";
/// Canonical skill manifest file name inside a skill directory.
pub const SKILL_MANIFEST: &str = "SKILL.md";
/// System-wide skills installed by the os-skills RPM package.
pub const SYSTEM_SKILLS_DIR: &str = "/usr/share/anolisa/skills";
