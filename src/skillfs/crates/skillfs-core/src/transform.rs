//! Ordered read-time transform pipeline for `SKILL.md` content.
//!
//! A [`TransformPipeline`] applies an optional, fixed-order sequence of
//! transform stages to the bytes served for a `SKILL.md` read:
//!
//! ```text
//! directive -> os_adapter
//! ```
//!
//! Both stages are optional and independent:
//!
//! - An **empty** pipeline (no stages) returns the input bytes unchanged.
//! - The **directive** stage runs the existing conditional compiler and, when
//!   present, is always applied first. It is enabled by default so existing
//!   mounts keep byte-for-byte compatible output.
//! - The optional **os_adapter** stage runs second.
//!
//! The pipeline holds each stage in a dedicated slot, so a stage can never be
//! added twice and the order is structurally fixed — there is no way to build
//! an invalid duplicate or out-of-order pipeline. The set of stages is decided
//! once at mount startup; the per-read hot path does no YAML parsing, OS
//! detection, subprocess execution, network access, or LLM calls.

use crate::compiler;
use crate::env::EnvironmentProfile;
use crate::os_adapter::OsAdapterStage;

/// A single read-time transform over `SKILL.md` bytes.
///
/// Stages are pure: given the same input they always produce the same output,
/// and they never touch the filesystem, spawn processes, or perform I/O.
pub trait TransformStage: Send + Sync {
    /// Stable, content-free stage identifier used in diagnostics.
    fn name(&self) -> &'static str;

    /// Transform `input` and return the stage's output.
    fn apply(&self, input: &str) -> String;
}

/// The first pipeline stage: the existing conditional-directive compiler.
///
/// Wraps [`compiler::compile`] with a fixed [`EnvironmentProfile`] captured at
/// mount startup so its output is identical to the pre-pipeline behavior.
pub struct DirectiveStage {
    env: EnvironmentProfile,
}

impl DirectiveStage {
    /// Build the directive stage bound to `env`.
    pub fn new(env: EnvironmentProfile) -> Self {
        Self { env }
    }
}

impl TransformStage for DirectiveStage {
    fn name(&self) -> &'static str {
        "directive"
    }

    fn apply(&self, input: &str) -> String {
        compiler::compile(input, &self.env)
    }
}

/// A fixed-order, optional set of transform stages applied to `SKILL.md` reads.
///
/// Each stage lives in its own slot, which makes the `directive -> os_adapter`
/// order structural and forbids duplicate stages by construction. Build with
/// [`TransformPipeline::directive_only`] for the default behavior, or with
/// [`TransformPipeline::empty`] and the setters for adapter-only / raw
/// configurations.
#[derive(Default)]
pub struct TransformPipeline {
    directive: Option<DirectiveStage>,
    os_adapter: Option<OsAdapterStage>,
}

impl TransformPipeline {
    /// Build a pipeline with no stages. [`Self::run`] returns its input
    /// unchanged until a stage is set.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Build a pipeline containing only the directive/compiler stage.
    ///
    /// With no adapter set, [`Self::run`] is byte-for-byte equivalent to calling
    /// [`compiler::compile`] directly.
    pub fn directive_only(env: EnvironmentProfile) -> Self {
        Self {
            directive: Some(DirectiveStage::new(env)),
            os_adapter: None,
        }
    }

    /// Set or clear the directive stage.
    ///
    /// Pass `Some(stage)` to enable it, `None` to disable it. Taking a prebuilt
    /// stage keeps [`EnvironmentProfile`] detection at the call site, so an
    /// adapter-only or empty pipeline never probes the environment. Idempotent —
    /// a stage can never be present more than once.
    pub fn set_directive(&mut self, stage: Option<DirectiveStage>) {
        self.directive = stage;
    }

    /// Set (or replace) the OS adapter stage. Idempotent — the adapter can never
    /// be present more than once, preserving the fixed `directive -> os_adapter`
    /// order.
    pub fn set_os_adapter(&mut self, stage: OsAdapterStage) {
        self.os_adapter = Some(stage);
    }

    /// Run every enabled stage in order and return the final Agent-visible
    /// bytes. Returns the input unchanged when no stage is enabled.
    pub fn run(&self, input: &str) -> String {
        let mut current: Option<String> = None;
        if let Some(stage) = &self.directive {
            current = Some(stage.apply(current.as_deref().unwrap_or(input)));
        }
        if let Some(stage) = &self.os_adapter {
            current = Some(stage.apply(current.as_deref().unwrap_or(input)));
        }
        current.unwrap_or_else(|| input.to_string())
    }

    /// Ordered names of the enabled stages, for content-free initialization
    /// diagnostics. Empty when no stage is enabled.
    pub fn stage_names(&self) -> Vec<&'static str> {
        let mut names = Vec::new();
        if let Some(stage) = &self.directive {
            names.push(stage.name());
        }
        if let Some(stage) = &self.os_adapter {
            names.push(stage.name());
        }
        names
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{EnvironmentProfile, OsKind};
    use crate::os_adapter::{OsAdapterStage, OsTarget};
    use std::collections::{HashMap, HashSet};
    use std::path::Path;

    fn plain_env() -> EnvironmentProfile {
        EnvironmentProfile {
            os: OsKind::Linux,
            available_commands: HashSet::new(),
            env_vars: HashMap::new(),
        }
    }

    /// A minimal always-on adapter that rewrites `apt-get install -y ` when
    /// converting toward Alinux.
    fn apt_adapter() -> OsAdapterStage {
        let yaml = "- ubuntu: \"apt-get install -y \"\n  alinux: \"dnf install -y \"\n  direction: bidirectional\n  auto_apply: always\n";
        OsAdapterStage::from_bytes(yaml.as_bytes(), OsTarget::Alinux, Path::new("t.yaml")).unwrap()
    }

    #[test]
    fn empty_pipeline_returns_input_unchanged() {
        let pipeline = TransformPipeline::empty();
        let content = "A\n<!-- @if os == darwin -->\nB\n<!-- @endif -->\nC\n";
        assert_eq!(pipeline.run(content), content);
        assert!(pipeline.stage_names().is_empty());
    }

    #[test]
    fn directive_only_matches_compiler_output() {
        let env = plain_env();
        let content = "A\n<!-- @if os == darwin -->\nB\n<!-- @endif -->\nC\n";
        let pipeline = TransformPipeline::directive_only(env.clone());
        assert_eq!(pipeline.run(content), compiler::compile(content, &env));
        assert_eq!(pipeline.stage_names(), vec!["directive"]);
    }

    #[test]
    fn set_directive_toggles_stage() {
        let env = plain_env();
        let mut pipeline = TransformPipeline::empty();
        pipeline.set_directive(Some(DirectiveStage::new(env.clone())));
        assert_eq!(pipeline.stage_names(), vec!["directive"]);
        pipeline.set_directive(None);
        assert!(pipeline.stage_names().is_empty());
    }

    #[test]
    fn directive_cannot_be_duplicated() {
        let env = plain_env();
        let mut pipeline = TransformPipeline::directive_only(env.clone());
        // Re-setting replaces rather than appends: still a single stage.
        pipeline.set_directive(Some(DirectiveStage::new(env)));
        assert_eq!(pipeline.stage_names(), vec!["directive"]);
    }

    #[test]
    fn combined_pipeline_applies_directive_then_os_adapter() {
        let mut pipeline = TransformPipeline::directive_only(plain_env());
        pipeline.set_os_adapter(apt_adapter());
        assert_eq!(pipeline.stage_names(), vec!["directive", "os_adapter"]);
        let input = "<!-- @if os == linux -->\nrun apt-get install -y foo\n<!-- @endif -->\n";
        let out = pipeline.run(input);
        // Directive stripped the markers; adapter rewrote the command.
        assert!(out.contains("run dnf install -y foo"), "got: {out}");
        assert!(!out.contains("@if"));
        assert!(!out.contains("apt-get"));
    }

    #[test]
    fn adapter_only_pipeline_skips_directive() {
        let mut pipeline = TransformPipeline::empty();
        pipeline.set_os_adapter(apt_adapter());
        assert_eq!(pipeline.stage_names(), vec!["os_adapter"]);
        let input = "<!-- @if os == linux -->\nrun apt-get install -y foo\n<!-- @endif -->\n";
        let out = pipeline.run(input);
        // Directive disabled: markers survive; adapter still rewrote the command.
        assert!(out.contains("<!-- @if os == linux -->"), "got: {out}");
        assert!(out.contains("run dnf install -y foo"));
    }

    #[test]
    fn os_adapter_cannot_be_duplicated() {
        let mut pipeline = TransformPipeline::empty();
        pipeline.set_os_adapter(apt_adapter());
        pipeline.set_os_adapter(apt_adapter());
        assert_eq!(pipeline.stage_names(), vec!["os_adapter"]);
    }
}
