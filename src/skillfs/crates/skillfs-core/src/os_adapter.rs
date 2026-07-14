//! Opt-in OS-adapter transform stage for `SKILL.md`.
//!
//! Rewrites distribution-specific literal strings (package managers, package
//! names, service unit names, filesystem paths) between Ubuntu/Debian and
//! Alinux/Anolis style using an externally supplied, versioned rule artifact.
//! SkillFS never maintains its own mapping table; it consumes a read-only rule
//! file whose format is a top-level YAML sequence of rules.
//!
//! # Rule schema
//!
//! Each list entry is a mapping with these fields:
//!
//! ```yaml
//! - ubuntu: "apt-get install -y "     # literal source on the Ubuntu/Debian side
//!   alinux: "dnf install -y "         # literal source on the Alinux/Anolis side
//!   direction: bidirectional          # bidirectional | ubuntu_to_alinux_only | alinux_to_ubuntu_only
//!   auto_apply: always                # always | never (explicit eligibility)
//!   confidence: high                  # OPTIONAL, ignored by SkillFS
//!   notes: "..."                      # OPTIONAL, ignored by SkillFS
//! ```
//!
//! Eligibility is governed **only** by the explicit `auto_apply` field: a rule
//! is applied automatically iff `auto_apply = always`. `confidence` and `notes`
//! are accepted as human annotations but carry no behavior — SkillFS does not
//! inherit any confidence-driven semantics.
//!
//! # Determinism and validation
//!
//! The rule artifact is parsed, validated, and compiled into an ordered literal
//! substitution table exactly once, when the mount starts. Validation rejects
//! unknown fields, invalid `direction`/`auto_apply` values, empty patterns, and
//! duplicate or ambiguous source patterns for the resolved target. Rules apply
//! in file order (callers must place more specific patterns first). The per-read
//! hot path only performs in-memory string substitution.
//!
//! # Module layout
//!
//! - `detect` — target-OS selection and `/etc/os-release` detection.
//! - `error` — the owned [`OsAdapterError`] type.
//! - `rules` — schema, validation, and substitution compilation.
//! - `stage` — the [`OsAdapterStage`] transform stage.

mod detect;
mod error;
mod rules;
mod stage;

pub use detect::{OsTarget, TargetSelector, parse_os_release};
pub use error::OsAdapterError;
pub use stage::OsAdapterStage;
