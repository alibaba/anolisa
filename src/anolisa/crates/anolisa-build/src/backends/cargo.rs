//! Cargo/Rust build backend.

use super::super::{Artifact, ArtifactType, BuildBackend, BuildError, BuildProfile, BuildSpec};

pub struct CargoBuilder;

impl BuildBackend for CargoBuilder {
    fn name(&self) -> &str {
        "cargo"
    }

    fn build(&self, spec: &BuildSpec) -> Result<Vec<Artifact>, BuildError> {
        let profile_flag = match spec.profile {
            BuildProfile::Release => "--release",
            BuildProfile::Debug => "",
        };

        let profile_dir = match spec.profile {
            BuildProfile::Release => "release",
            BuildProfile::Debug => "debug",
        };

        // TODO(owner: build-backend, when: source builds graduate): invoke
        // cargo and surface command status instead of predicting paths only.
        let _cmd = format!(
            "cargo build {profile_flag} --manifest-path {}/Cargo.toml",
            spec.source_dir.display()
        );

        // Produce expected artifacts
        let artifacts: Vec<Artifact> = spec
            .targets
            .iter()
            .map(|target| Artifact {
                path: spec
                    .source_dir
                    .join("target")
                    .join(profile_dir)
                    .join(target),
                artifact_type: ArtifactType::Binary,
            })
            .collect();

        Ok(artifacts)
    }

    fn clean(&self, spec: &BuildSpec) -> Result<(), BuildError> {
        let target_dir = spec.source_dir.join("target");
        if target_dir.exists() {
            std::fs::remove_dir_all(&target_dir)?;
        }
        Ok(())
    }
}
