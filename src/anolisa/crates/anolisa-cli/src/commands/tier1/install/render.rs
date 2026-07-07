//! Rendering and error-mapping helpers for the `install` command.

use anolisa_core::ArtifactType;

use crate::color::Palette;
use crate::context::CliContext;
use crate::repo_config::RepoConfigError;
use crate::response::{CliError, render_json};

use super::types::*;

/// Render the dry-run plan (JSON envelope or the proposal §6.1 human text).
pub(crate) fn render_plan(ctx: &CliContext, preview: &InstallPreview) -> Result<(), CliError> {
    let resolved = &preview.resolution;
    let payload = InstallPlanPayload {
        component: resolved.component.clone(),
        package: resolved.package.clone(),
        version: resolved.entry.version.clone(),
        backend: resolved.backend.clone(),
        base_url: resolved.base_url.clone(),
        install_mode: ctx.install_mode.as_str().to_string(),
        artifact: ArtifactInfo {
            r#type: artifact_type_wire(&resolved.entry.artifact_type).to_string(),
            url: resolved.artifact_url.clone(),
            sha256: resolved.entry.sha256.clone(),
        },
        files: preview
            .files
            .iter()
            .map(|f| f.dest.display().to_string())
            .collect(),
        services: preview.services.iter().map(|s| s.unit.clone()).collect(),
        capabilities: preview
            .capabilities
            .iter()
            .map(|c| format!("{}: {}", c.path.display(), c.caps.join(",")))
            .collect(),
        dependencies: preview
            .dependencies
            .iter()
            .map(|r| {
                let row = DependencyPlanRow::from_resolution(r);
                if let Some(plan) = &preview.provision_plan {
                    row.with_provision_action(plan)
                } else {
                    row
                }
            })
            .collect(),
        dry_run: true,
        warnings: resolved.warnings.clone(),
    };

    if ctx.json {
        return render_json(super::COMMAND, &payload);
    }
    if ctx.quiet {
        return Ok(());
    }
    let color = Palette::new(ctx.no_color);
    println!(
        "{} {} v{} {}",
        color.command("install"),
        payload.component,
        payload.version,
        color.muted("(dry-run — nothing installed)"),
    );
    println!("{} {}", color.label("backend:"), payload.backend);
    println!(
        "{} {}",
        color.label("base_url:"),
        color.path(&payload.base_url)
    );
    println!("{} {}", color.label("package:"), payload.package);
    println!("{} {}", color.label("install_mode:"), payload.install_mode);
    println!(
        "{} {} ({})",
        color.label("artifact:"),
        color.path(&payload.artifact.url),
        payload.artifact.r#type
    );
    println!("{}", color.header("files:"));
    for f in &payload.files {
        println!("  - {}", color.path(f));
    }
    if !payload.services.is_empty() {
        println!(
            "{}",
            color.header("services (would enable/start when supported):")
        );
        for s in &payload.services {
            println!("  - {s}");
        }
    }
    if !payload.capabilities.is_empty() {
        println!("{}", color.header("capabilities (applied on install):"));
        for c in &payload.capabilities {
            println!("  - {c}");
        }
    }
    if !payload.dependencies.is_empty() {
        println!("{}", color.header("dependencies (preflight):"));
        for d in &payload.dependencies {
            let (kind, status) = (d.kind.as_str(), d.status.as_str());
            let action_tag = match d.action {
                Some(DependencyPlanAction::AutoInstall) => " [auto-install]",
                Some(DependencyPlanAction::Manual) => " [manual]",
                None => "",
            };
            match &d.note {
                Some(note) => {
                    println!("  - {} [{kind}]: {status}{action_tag} — {note}", d.name)
                }
                None => println!("  - {} [{kind}]: {status}{action_tag}", d.name),
            }
            if let Some(detail) = &d.detail {
                println!("      {detail}");
            }
        }
    }
    render_warnings(&payload.warnings, &color);
    Ok(())
}

pub(crate) fn render_result(payload: &InstallResultPayload, no_color: bool) {
    let color = Palette::new(no_color);
    println!(
        "{} {} v{} {}",
        color.command("install"),
        payload.component,
        payload.version,
        color.ok("succeeded"),
    );
    println!("{} {}", color.label("backend:"), payload.backend);
    println!("{} {}", color.label("package:"), payload.package);
    println!(
        "{} {}",
        color.label("operation_id:"),
        color.id(&payload.operation_id)
    );
    println!(
        "{} {}",
        color.label("files installed:"),
        payload.files_installed.len()
    );
    for p in &payload.files_installed {
        println!("  - {}", color.path(p));
    }
    if !payload.services.is_empty() {
        println!(
            "{}",
            color.header("services (enabled/started when supported):")
        );
        for s in &payload.services {
            println!("  - {s}");
        }
    }
    if !payload.provisioned_packages.is_empty() {
        println!(
            "{} {}",
            color.label("provisioned packages:"),
            payload.provisioned_packages.join(", ")
        );
    }
    render_warnings(&payload.warnings, &color);
}

pub(crate) fn render_warnings(warnings: &[String], color: &Palette) {
    if warnings.is_empty() {
        return;
    }
    println!("{}", color.warn("warnings:"));
    for w in warnings {
        println!("  - {w}");
    }
}

/// Route a [`RepoConfigError`] to the CLI error surface.
///
/// `caller_fixable` decides the bucket: selection/substitution/override
/// errors are actionable by the caller (pass a different `--backend`,
/// fix `[vars]`, fix the `--repo` URL) → INVALID_ARGUMENT (exit 2);
/// discovery/IO/parse failures mean the config asset itself is broken →
/// EXECUTION_FAILED (exit 1), mirroring the execution-policy split.
pub(crate) fn repo_config_err(err: RepoConfigError, caller_fixable: bool) -> CliError {
    if caller_fixable {
        CliError::InvalidArgument {
            command: super::COMMAND.to_string(),
            reason: err.to_string(),
        }
    } else {
        CliError::Runtime {
            command: super::COMMAND.to_string(),
            reason: format!("failed to load repo config: {err}"),
        }
    }
}

/// `{ext}` placeholder value for the conventional file name. Single-file
/// artifacts ship bare; OCI rows are references, not downloadable files,
/// and never resolve through URL derivation.
pub(crate) fn artifact_ext(t: &ArtifactType) -> &'static str {
    match t {
        ArtifactType::TarGz => ".tar.gz",
        ArtifactType::Zip => ".zip",
        ArtifactType::Rpm => ".rpm",
        ArtifactType::Deb => ".deb",
        ArtifactType::Binary | ArtifactType::File | ArtifactType::Oci => "",
    }
}

/// Wire-form artifact type string for the install runner.
pub(crate) fn artifact_type_wire(t: &ArtifactType) -> &'static str {
    match t {
        ArtifactType::TarGz => "tar_gz",
        ArtifactType::Binary => "binary",
        ArtifactType::Rpm => "rpm",
        ArtifactType::Deb => "deb",
        ArtifactType::Zip => "zip",
        ArtifactType::Oci => "oci",
        ArtifactType::File => "file",
    }
}
