//! Application orchestration for `update all`.

use std::collections::HashMap;

use anolisa_core::execution::ExecutionIntent;
use anolisa_core::planner::Step;
use anolisa_core::state::ObjectKind;
use anolisa_core::state_store::StateStore;
use anolisa_platform::privilege;

use crate::commands::common;
use crate::context::CliContext;
use crate::response::CliError;

use super::super::application as update_application;
use super::super::{
    AdapterAction, PlannedUpdateRoute, UpdateOutcome, plan_component_update, update_backends,
};
use super::{MergedUpdate, execute_merged_updates, merged_update_package};

const BATCH_COMMAND: &str = "update all";

/// Batch command input plus whether the caller requested preview or apply.
pub(super) struct BatchRequest {
    pub(super) intent: ExecutionIntent,
}

/// Typed disposition of one batch member.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BatchMemberStatus {
    /// Effects completed for this member.
    Updated,
    /// The member has an executable plan, but no effects ran.
    Planned,
    /// Existing state already covers the latest version.
    AlreadyCurrent,
    /// Planning or execution failed for this member.
    Failed,
}

impl BatchMemberStatus {
    /// Returns the stable label used by the existing batch wire format.
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Updated => "updated",
            Self::Planned => "planned",
            Self::AlreadyCurrent => "already-current",
            Self::Failed => "failed",
        }
    }
}

/// Typed result for one recorded component in state order.
#[derive(Debug)]
pub(super) struct BatchMemberOutcome {
    pub(super) component: String,
    pub(super) status: BatchMemberStatus,
    pub(super) reason: Option<String>,
    /// Only merged preview members expose their existing per-member plan.
    pub(super) plan: Option<Vec<Step>>,
    pub(super) adapter_actions: Vec<AdapterAction>,
}

/// Complete batch result consumed by the command renderer.
#[derive(Debug)]
pub(super) struct BatchApplicationOutcome {
    pub(super) intent: ExecutionIntent,
    pub(super) merged_transaction: Option<Vec<String>>,
    pub(super) items: Vec<BatchMemberOutcome>,
}

/// Ordered presentation facts consumed by the command renderer.
#[derive(Debug)]
pub(super) enum BatchOutputEvent {
    /// Announces members sharing one native transaction.
    MergedGroup { members: String },
    /// Shows one merged member's plan in the legacy human format.
    PreviewPlan {
        component: String,
        from_version: Option<String>,
        steps: Vec<Step>,
    },
    /// Announces a member using the single-component application path.
    Member { component: String },
    /// Surfaces a non-fatal application warning.
    Warning(String),
}

/// Effects returned by the merged transaction boundary.
pub(super) struct BatchEffectOutcome {
    pub(super) items: Vec<BatchMemberOutcome>,
}

/// Per-member fallback result returned to merged transaction orchestration.
pub(super) struct MemberApplicationOutcome {
    pub(super) outcome: UpdateOutcome,
    pub(super) warnings: Vec<String>,
    pub(super) adapter_actions: Vec<AdapterAction>,
}

impl BatchApplicationOutcome {
    /// Returns whether any member failed.
    pub(super) fn has_failures(&self) -> bool {
        self.items
            .iter()
            .any(|item| item.status == BatchMemberStatus::Failed)
    }

    /// Returns whether this batch was prepared without applying effects.
    pub(super) fn is_preview(&self) -> bool {
        matches!(self.intent, ExecutionIntent::Plan)
    }
}

/// Run the batch application against production host dependencies.
pub(super) fn run(
    request: BatchRequest,
    ctx: &CliContext,
    output: &mut dyn FnMut(BatchOutputEvent),
) -> Result<BatchApplicationOutcome, CliError> {
    let layout = common::resolve_layout(ctx);
    let state_path = layout.state_dir.join("installed.toml");
    let store = StateStore::load_for_layout(&state_path, privilege::effective_uid(), &layout)
        .map_err(|err| CliError::Runtime {
            command: BATCH_COMMAND.to_string(),
            reason: format!("failed to load installed state: {err}"),
        })?;
    let names: Vec<String> = store
        .installations
        .iter()
        .filter(|installation| installation.kind == ObjectKind::Component)
        .map(|installation| installation.name.clone())
        .collect();
    drop(store);

    if names.is_empty() {
        return Ok(BatchApplicationOutcome {
            intent: request.intent,
            merged_transaction: None,
            items: Vec::new(),
        });
    }

    // Per-member applications return typed outcomes; batch owns the only
    // final renderer, so their human and JSON output stays suppressed.
    let mut suppressed_ctx = ctx.clone();
    suppressed_ctx.json = false;
    suppressed_ctx.quiet = true;
    suppressed_ctx.dry_run = matches!(request.intent, ExecutionIntent::Plan);

    // Preserve the existing read-only peek. Delegated U5 refreshes merge;
    // every other route re-plans through the single-component application.
    let mut merged: Vec<MergedUpdate> = Vec::new();
    let mut per_item: Vec<String> = Vec::new();
    for name in &names {
        let candidate = update_backends(name, &suppressed_ctx)
            .and_then(|(query, txn)| plan_component_update(name, &suppressed_ctx, &query, &txn))
            .ok()
            .and_then(|planned| {
                merged_update_package(&planned).map(|package| MergedUpdate {
                    name: name.clone(),
                    package,
                    planned,
                })
            });
        match candidate {
            Some(item) => merged.push(item),
            None => per_item.push(name.clone()),
        }
    }

    if merged.len() < 2 {
        per_item.clone_from(&names);
        merged.clear();
    }
    let merged_transaction =
        (!merged.is_empty()).then(|| merged.iter().map(|item| item.package.clone()).collect());

    let preview = matches!(request.intent, ExecutionIntent::Plan);
    let mut results: HashMap<String, BatchMemberOutcome> = HashMap::with_capacity(names.len());

    if !merged.is_empty() {
        let members = merged
            .iter()
            .map(|item| item.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        output(BatchOutputEvent::MergedGroup { members });
        if preview {
            for item in &merged {
                let PlannedUpdateRoute::Delegated { steps } = &item.planned.route else {
                    unreachable!("merged updates are classified as delegated")
                };
                let steps = steps.clone();
                output(BatchOutputEvent::PreviewPlan {
                    component: item.name.clone(),
                    from_version: item.planned.native_from.clone(),
                    steps: steps.clone(),
                });
                results.insert(
                    item.name.clone(),
                    BatchMemberOutcome {
                        component: item.name.clone(),
                        status: BatchMemberStatus::Planned,
                        reason: None,
                        plan: Some(steps),
                        adapter_actions: Vec::new(),
                    },
                );
            }
        } else {
            for item in execute_merged_updates(merged, ctx, output).items {
                results.insert(item.component.clone(), item);
            }
        }
    }

    for name in &per_item {
        output(BatchOutputEvent::Member {
            component: name.clone(),
        });
        let member = update_application::run(
            update_application::ApplicationRequest {
                component: name,
                intent: request.intent,
            },
            &suppressed_ctx,
        )
        .and_then(member_application_outcome);
        let item = match member {
            Ok(member) => {
                for warning in member.warnings {
                    output(BatchOutputEvent::Warning(warning));
                }
                BatchMemberOutcome {
                    component: name.clone(),
                    status: batch_status(member.outcome, request.intent),
                    reason: None,
                    plan: None,
                    adapter_actions: member.adapter_actions,
                }
            }
            Err(error) => failed_item(name, error.reason().to_string()),
        };
        results.insert(name.clone(), item);
    }

    let items = names
        .into_iter()
        .map(|name| {
            results
                .remove(&name)
                .expect("every recorded component receives one batch outcome")
        })
        .collect();

    Ok(BatchApplicationOutcome {
        intent: request.intent,
        merged_transaction,
        items,
    })
}

pub(super) fn member_application_outcome(
    outcome: update_application::ApplicationOutcome,
) -> Result<MemberApplicationOutcome, CliError> {
    let batch_outcome = outcome.batch_outcome()?;
    Ok(MemberApplicationOutcome {
        outcome: batch_outcome,
        warnings: outcome.warnings().to_vec(),
        adapter_actions: outcome.adapter_actions().to_vec(),
    })
}

pub(super) fn failed_item(name: &str, reason: String) -> BatchMemberOutcome {
    BatchMemberOutcome {
        component: name.to_string(),
        status: BatchMemberStatus::Failed,
        reason: Some(reason),
        plan: None,
        adapter_actions: Vec::new(),
    }
}

pub(super) fn batch_status(outcome: UpdateOutcome, intent: ExecutionIntent) -> BatchMemberStatus {
    match (outcome, intent) {
        (UpdateOutcome::Updated, ExecutionIntent::Apply) => BatchMemberStatus::Updated,
        (UpdateOutcome::Updated, ExecutionIntent::Plan) => BatchMemberStatus::Planned,
        (UpdateOutcome::AlreadyCurrent, _) => BatchMemberStatus::AlreadyCurrent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_status_is_driven_by_execution_intent() {
        assert_eq!(
            batch_status(UpdateOutcome::Updated, ExecutionIntent::Apply),
            BatchMemberStatus::Updated
        );
        assert_eq!(
            batch_status(UpdateOutcome::Updated, ExecutionIntent::Plan),
            BatchMemberStatus::Planned
        );
        for intent in [ExecutionIntent::Plan, ExecutionIntent::Apply] {
            assert_eq!(
                batch_status(UpdateOutcome::AlreadyCurrent, intent),
                BatchMemberStatus::AlreadyCurrent
            );
        }
    }

    #[test]
    fn aggregate_failure_is_typed() {
        let outcome = BatchApplicationOutcome {
            intent: ExecutionIntent::Apply,
            merged_transaction: None,
            items: vec![failed_item("cosh", "failed".to_string())],
        };

        assert!(outcome.has_failures());
        assert!(!outcome.is_preview());
    }
}
