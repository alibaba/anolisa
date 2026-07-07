//! `--all` batch install support for the `install` command.

use serde::Serialize;

use crate::color::Palette;
use crate::commands::common;
use crate::commands::common::RepoPersistPolicy;
use crate::context::CliContext;
use crate::response::{CliError, render_json, render_json_with_status};

use super::InstallArgs;
use super::types::InstallOutcome;

// `handle_one` lives in dispatch.rs; re-exported from the parent module.
use super::handle_one;
// ── --all support ───────────────────────────────────────────────────

/// Wire shape for a batch entry.  `status` is one of:
/// `installed` | `planned` (dry-run) | `adopted` | `adopt-planned` (dry-run) |
/// `failed` | `skipped`.
#[derive(Serialize)]
pub(crate) struct AllSummaryItem {
    component: String,
    status: &'static str,
    reason: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct AllSummaryPayload {
    total: usize,
    installed: usize,
    planned: usize,
    /// Existing system RPMs recorded as rpm-observed (§7.5).
    adopted: usize,
    /// Dry-run adopt previews.
    adopt_planned: usize,
    failed: usize,
    skipped: usize,
    dry_run: bool,
    items: Vec<AllSummaryItem>,
}

pub(crate) fn handle_all(args: InstallArgs, ctx: &CliContext) -> Result<(), CliError> {
    let names = resolve_all_components(ctx, args.backend.as_deref())?;
    if names.is_empty() {
        if !ctx.quiet && !ctx.json {
            let color = Palette::new(ctx.no_color);
            println!(
                "{}",
                color.muted("no available components in component index; nothing to install")
            );
        }
        if ctx.json {
            return render_json(
                "install --all",
                AllSummaryPayload {
                    total: 0,
                    installed: 0,
                    planned: 0,
                    adopted: 0,
                    adopt_planned: 0,
                    failed: 0,
                    skipped: 0,
                    dry_run: ctx.dry_run,
                    items: Vec::new(),
                },
            );
        }
        return Ok(());
    }

    // Suppress per-component rendering: handle_all owns the final output.
    // Each handle_one call runs in quiet mode so it doesn't print individual
    // JSON envelopes or human-mode messages — only the batch summary at the
    // end goes to stdout.
    let suppressed_ctx = CliContext {
        json: false,
        quiet: true,
        ..ctx.clone()
    };

    let mut items: Vec<AllSummaryItem> = Vec::with_capacity(names.len());
    let mut first_error: Option<CliError> = None;
    let mut last_processed = 0usize;

    for (idx, name) in names.iter().enumerate() {
        last_processed = idx;
        if !ctx.quiet && !ctx.json {
            let color = Palette::new(ctx.no_color);
            println!("{} {name}", color.label("==>"));
        }
        let per_args = InstallArgs {
            component: Some(name.clone()),
            all: false,
            fail_fast: false,
            version: None,
            backend: args.backend.clone(),
            repo: args.repo.clone(),
            package: None,
        };
        match handle_one(name.clone(), per_args, &suppressed_ctx) {
            // Map (outcome, dry-run) to a batch status string so the summary
            // distinguishes a fresh install from an RPM adopt (§7.5). Dry-run
            // successes are "planned"/"adopt-planned": nothing was written.
            Ok(outcome) => items.push(AllSummaryItem {
                component: name.clone(),
                status: batch_status(outcome, ctx.dry_run),
                reason: None,
            }),
            Err(err) => {
                let reason = err.reason().to_string();
                items.push(AllSummaryItem {
                    component: name.clone(),
                    status: "failed",
                    reason: Some(reason),
                });
                if first_error.is_none() {
                    first_error = Some(err);
                }
                if args.fail_fast {
                    break;
                }
            }
        }
    }

    // --fail-fast may have left components unprocessed.  Mark them as
    // skipped so `total` always equals the full target set.
    for name in &names[last_processed + 1..] {
        items.push(AllSummaryItem {
            component: name.clone(),
            status: "skipped",
            reason: Some("--fail-fast: not attempted".to_string()),
        });
    }

    let installed = items.iter().filter(|i| i.status == "installed").count();
    let planned = items.iter().filter(|i| i.status == "planned").count();
    let adopted = items.iter().filter(|i| i.status == "adopted").count();
    let adopt_planned = items.iter().filter(|i| i.status == "adopt-planned").count();
    let failed = items.iter().filter(|i| i.status == "failed").count();
    let skipped = items.iter().filter(|i| i.status == "skipped").count();

    if ctx.json {
        // The batch summary is the single, complete JSON response.  We
        // return BatchPartial (not Ok) so that main's render_error still
        // sets a non-zero exit code — but render_error recognises
        // BatchPartial and skips the second JSON render.
        render_json_with_status(
            "install --all",
            failed == 0,
            AllSummaryPayload {
                total: names.len(),
                installed,
                planned,
                adopted,
                adopt_planned,
                failed,
                skipped,
                dry_run: ctx.dry_run,
                items,
            },
        )?;
        return match first_error {
            Some(_) => Err(CliError::BatchPartial {
                command: "install --all".to_string(),
            }),
            None => Ok(()),
        };
    }

    if !ctx.quiet {
        let color = Palette::new(ctx.no_color);
        println!();
        let failed_names: Vec<&str> = items
            .iter()
            .filter(|i| i.status == "failed")
            .map(|i| i.component.as_str())
            .collect();
        let ok_word = if ctx.dry_run { "planned" } else { "installed" };
        let ok_count = if ctx.dry_run { planned } else { installed };
        // Adopts are a distinct outcome from installs; show them as their own
        // segment (and only when non-zero) so the count isn't lost (§7.5).
        let adopt_word = if ctx.dry_run {
            "adopt-planned"
        } else {
            "adopted"
        };
        let adopt_count = if ctx.dry_run { adopt_planned } else { adopted };
        let adopt_segment = if adopt_count > 0 {
            format!("  {adopt_word}={adopt_count}")
        } else {
            String::new()
        };
        if failed_names.is_empty() {
            println!(
                "{} total={}  {ok_word}={}{adopt_segment}  skipped={}",
                color.label("summary:"),
                names.len(),
                ok_count,
                skipped,
            );
        } else {
            println!(
                "{} total={}  {ok_word}={}{adopt_segment}  failed={} ({})  skipped={}",
                color.label("summary:"),
                names.len(),
                ok_count,
                failed,
                failed_names.join(", "),
                skipped,
            );
            for item in items.iter().filter(|i| i.status == "failed") {
                if let Some(reason) = &item.reason {
                    eprintln!("{} {}: {reason}", color.err("failed:"), item.component);
                }
            }
        }
        // List adopted components explicitly so `--all` shows which were
        // taken over rather than freshly installed.
        for item in items
            .iter()
            .filter(|i| i.status == "adopted" || i.status == "adopt-planned")
        {
            println!(
                "{} {}",
                color.label("adopted rpm-observed:"),
                item.component
            );
        }
    }

    // Human mode: preserve non-zero exit code on failure.
    match first_error {
        Some(_) => Err(CliError::BatchPartial {
            command: "install --all".to_string(),
        }),
        None => Ok(()),
    }
}

/// Batch status string for a successful `handle_one`, combining the outcome
/// with dry-run. Kept aligned with the `filter`-by-string counting in
/// [`handle_all`] (§7.5): a new string here must be matched there too.
pub(crate) fn batch_status(outcome: InstallOutcome, dry_run: bool) -> &'static str {
    match (outcome, dry_run) {
        (InstallOutcome::Installed, false) => "installed",
        (InstallOutcome::Installed, true) => "planned",
        (InstallOutcome::Adopted, false) => "adopted",
        (InstallOutcome::Adopted, true) => "adopt-planned",
    }
}

/// Load the component index and return names of components that support
/// the given backend. When `backend` is `None`, the repo's default
/// backend is used.
pub(crate) fn resolve_all_components(
    ctx: &CliContext,
    backend: Option<&str>,
) -> Result<Vec<String>, CliError> {
    let layout = common::resolve_layout(ctx);
    let env = anolisa_env::EnvService::detect();
    let repo_config =
        common::load_repo_config(ctx, &layout, "install --all", RepoPersistPolicy::Require)?;
    let index =
        crate::resolution::load_component_index(&layout, &env, &repo_config).map_err(|err| {
            CliError::Runtime {
                command: "install --all".to_string(),
                reason: format!("failed to load component index: {err}"),
            }
        })?;
    let (selected_backend, _) =
        repo_config
            .select_backend(backend)
            .map_err(|err| CliError::InvalidArgument {
                command: "install --all".to_string(),
                reason: format!("{err}"),
            })?;
    let selected_backend = selected_backend.to_string();
    let names: Vec<String> = index
        .components
        .iter()
        .filter(|entry| entry.backends.iter().any(|b| b.kind == selected_backend))
        .map(|entry| entry.name.clone())
        .collect();
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_status_maps_outcome_and_dry_run() {
        assert_eq!(batch_status(InstallOutcome::Installed, false), "installed");
        assert_eq!(batch_status(InstallOutcome::Installed, true), "planned");
        assert_eq!(batch_status(InstallOutcome::Adopted, false), "adopted");
        assert_eq!(batch_status(InstallOutcome::Adopted, true), "adopt-planned");
    }
}
