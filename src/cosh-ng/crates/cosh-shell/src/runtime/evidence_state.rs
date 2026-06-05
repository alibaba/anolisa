use std::collections::HashSet;

use crate::evidence::output_policy::{shell_evidence_view, EvidenceFacts, EvidenceView};
use cosh_shell::types::{CommandBlock, ShellHandoffRequest};

#[derive(Debug, Default)]
pub(crate) struct EvidenceState {
    shell_command_completed: Vec<RuntimeShellCommandCompleted>,
    continued_shell_handoff_approvals: HashSet<String>,
}

impl EvidenceState {
    pub(crate) fn record_shell_command_completed(
        &mut self,
        evidence: RuntimeShellCommandCompleted,
    ) {
        self.shell_command_completed.push(evidence);
    }

    pub(crate) fn claim_pending_shell_handoff_continuations(
        &mut self,
    ) -> Vec<RuntimeShellCommandCompleted> {
        let mut requests = Vec::new();
        for evidence in self.shell_command_completed.iter_mut() {
            if evidence.continuation_state != ShellEvidenceContinuationState::PendingRecovery {
                continue;
            }
            let Some(approval_id) = evidence.approval_id.as_ref() else {
                continue;
            };
            if !self
                .continued_shell_handoff_approvals
                .insert(approval_id.clone())
            {
                continue;
            }
            evidence.continuation_state = ShellEvidenceContinuationState::RecoveryQueued;
            requests.push(evidence.clone());
        }
        requests
    }

    pub(crate) fn claim_stalled_provider_shell_handoff_continuations(
        &mut self,
    ) -> Vec<RuntimeShellCommandCompleted> {
        let mut requests = Vec::new();
        for evidence in self.shell_command_completed.iter_mut() {
            if !matches!(
                evidence.continuation_state,
                ShellEvidenceContinuationState::DeliveredToProvider
                    | ShellEvidenceContinuationState::ProviderProgressObserved
            ) {
                continue;
            }
            let Some(approval_id) = evidence.approval_id.as_ref() else {
                continue;
            };
            if !self
                .continued_shell_handoff_approvals
                .insert(approval_id.clone())
            {
                continue;
            }
            evidence.continuation_state = ShellEvidenceContinuationState::RecoveryQueued;
            requests.push(evidence.clone());
        }
        requests
    }

    pub(crate) fn mark_provider_progress_observed(&mut self, closed: bool) {
        for evidence in &mut self.shell_command_completed {
            match evidence.continuation_state {
                ShellEvidenceContinuationState::DeliveredToProvider
                | ShellEvidenceContinuationState::ProviderProgressObserved => {
                    evidence.continuation_state = if closed {
                        ShellEvidenceContinuationState::Closed
                    } else {
                        ShellEvidenceContinuationState::ProviderProgressObserved
                    };
                }
                ShellEvidenceContinuationState::PendingRecovery
                | ShellEvidenceContinuationState::RecoveryQueued
                | ShellEvidenceContinuationState::Closed => {}
            }
        }
    }

    pub(crate) fn latest_recovery(&self) -> Option<&RuntimeShellCommandCompleted> {
        self.shell_command_completed
            .iter()
            .rev()
            .find(|evidence| evidence.recovery_reason.is_some())
    }

    pub(crate) fn has_open_provider_shell_evidence(&self) -> bool {
        self.shell_command_completed.iter().any(|evidence| {
            matches!(
                evidence.continuation_state,
                ShellEvidenceContinuationState::DeliveredToProvider
                    | ShellEvidenceContinuationState::ProviderProgressObserved
            )
        })
    }

    pub(crate) fn provider_visible_view(evidence: &RuntimeShellCommandCompleted) -> EvidenceView {
        shell_evidence_view(EvidenceFacts {
            shell_session_id: &evidence.shell_session_id,
            command_id: &evidence.command_block_id,
            command: &evidence.command,
            cwd: &evidence.cwd,
            end_cwd: &evidence.end_cwd,
            status: evidence.status,
            exit_code: evidence.exit_code,
            duration_ms: evidence.duration_ms,
            output_ref: evidence.terminal_output_ref.as_deref(),
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ShellEvidenceDelivery {
    pub(crate) delivered: bool,
    pub(crate) status: &'static str,
    pub(crate) recovery_reason: Option<&'static str>,
}

impl ShellEvidenceDelivery {
    pub(crate) fn not_attempted() -> Self {
        Self {
            delivered: false,
            status: "not_attempted",
            recovery_reason: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShellEvidenceContinuationState {
    PendingRecovery,
    DeliveredToProvider,
    ProviderProgressObserved,
    RecoveryQueued,
    Closed,
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeShellCommandCompleted {
    pub(crate) approval_id: Option<String>,
    pub(crate) shell_session_id: String,
    pub(crate) command_block_id: String,
    pub(crate) command: String,
    pub(crate) cwd: String,
    pub(crate) end_cwd: String,
    pub(crate) status: &'static str,
    pub(crate) exit_code: i32,
    pub(crate) duration_ms: u64,
    pub(crate) terminal_output_ref: Option<String>,
    pub(crate) redaction_status: &'static str,
    pub(crate) provider_result_delivered: bool,
    pub(crate) provider_result_delivery_status: &'static str,
    pub(crate) recovery_reason: Option<&'static str>,
    pub(crate) continuation_state: ShellEvidenceContinuationState,
}

impl RuntimeShellCommandCompleted {
    pub(crate) fn from_shell_handoff(
        handoff: &ShellHandoffRequest,
        block: &CommandBlock,
        status: &'static str,
    ) -> Self {
        let delivery = ShellEvidenceDelivery::not_attempted();
        Self {
            approval_id: Some(handoff.approval_id.clone()),
            shell_session_id: block.session_id.clone(),
            command_block_id: block.id.clone(),
            command: block.command.clone(),
            cwd: block.cwd.clone(),
            end_cwd: block.end_cwd.clone(),
            status,
            exit_code: block.exit_code,
            duration_ms: block.duration_ms,
            terminal_output_ref: block.output.terminal_output_ref.clone(),
            redaction_status: "ref_only",
            provider_result_delivered: delivery.delivered,
            provider_result_delivery_status: delivery.status,
            recovery_reason: delivery.recovery_reason,
            continuation_state: ShellEvidenceContinuationState::PendingRecovery,
        }
    }

    pub(crate) fn apply_provider_result_delivery(&mut self, delivery: ShellEvidenceDelivery) {
        self.provider_result_delivered = delivery.delivered;
        self.provider_result_delivery_status = delivery.status;
        self.recovery_reason = delivery.recovery_reason;
        self.continuation_state = if delivery.delivered {
            ShellEvidenceContinuationState::DeliveredToProvider
        } else {
            ShellEvidenceContinuationState::PendingRecovery
        };
    }

    pub(crate) fn selected_execution_path(&self) -> &'static str {
        if self.provider_result_delivered {
            "control_protocol_host_executed_shell_result"
        } else {
            "foreground_shell_handoff_recovery"
        }
    }

    pub(crate) fn path_selection_reason(&self) -> &'static str {
        if self.provider_result_delivered {
            "provider advertised host-executed shell result support"
        } else if let Some(reason) = self.recovery_reason {
            reason
        } else {
            "provider result was not delivered; shell evidence continuation required"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EvidenceState, RuntimeShellCommandCompleted, ShellEvidenceContinuationState,
        ShellEvidenceDelivery,
    };

    #[test]
    fn evidence_state_claims_pending_continuations_once() {
        let mut state = EvidenceState::default();
        state.record_shell_command_completed(shell_evidence(
            Some("req-1"),
            false,
            "provider_run_not_active",
            Some("provider run was not active"),
        ));
        state.record_shell_command_completed(shell_evidence(
            Some("req-2"),
            true,
            "delivered",
            None,
        ));

        let first = state.claim_pending_shell_handoff_continuations();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].approval_id.as_deref(), Some("req-1"));
        assert_eq!(
            first[0].continuation_state,
            ShellEvidenceContinuationState::RecoveryQueued
        );

        let second = state.claim_pending_shell_handoff_continuations();
        assert!(second.is_empty());
    }

    #[test]
    fn evidence_state_tracks_latest_recovery() {
        let mut state = EvidenceState::default();
        state.record_shell_command_completed(shell_evidence(
            Some("req-1"),
            false,
            "unsupported",
            Some("provider unsupported"),
        ));
        state.record_shell_command_completed(shell_evidence(
            Some("req-2"),
            false,
            "provider_run_not_active",
            Some("provider missing"),
        ));

        let latest = state.latest_recovery().expect("latest recovery");
        assert_eq!(latest.approval_id.as_deref(), Some("req-2"));
        assert_eq!(
            latest.provider_result_delivery_status,
            "provider_run_not_active"
        );
    }

    #[test]
    fn evidence_state_applies_provider_delivery_metadata() {
        let mut evidence = shell_evidence(Some("req-1"), false, "not_attempted", None);

        evidence.apply_provider_result_delivery(ShellEvidenceDelivery {
            delivered: true,
            status: "delivered",
            recovery_reason: None,
        });

        assert!(evidence.provider_result_delivered);
        assert_eq!(evidence.provider_result_delivery_status, "delivered");
        assert_eq!(
            evidence.selected_execution_path(),
            "control_protocol_host_executed_shell_result"
        );
        assert_eq!(
            evidence.path_selection_reason(),
            "provider advertised host-executed shell result support"
        );
        assert_eq!(
            evidence.continuation_state,
            ShellEvidenceContinuationState::DeliveredToProvider
        );
    }

    #[test]
    fn provider_progress_observed_closes_delivered_evidence_for_recovery_claims() {
        let mut state = EvidenceState::default();
        state.record_shell_command_completed(shell_evidence(
            Some("req-1"),
            true,
            "delivered",
            None,
        ));

        state.mark_provider_progress_observed(false);
        assert!(state.claim_pending_shell_handoff_continuations().is_empty());
        assert_eq!(
            state.shell_command_completed[0].continuation_state,
            ShellEvidenceContinuationState::ProviderProgressObserved
        );

        state.mark_provider_progress_observed(true);
        assert_eq!(
            state.shell_command_completed[0].continuation_state,
            ShellEvidenceContinuationState::Closed
        );
    }

    fn shell_evidence(
        approval_id: Option<&str>,
        provider_result_delivered: bool,
        provider_result_delivery_status: &'static str,
        recovery_reason: Option<&'static str>,
    ) -> RuntimeShellCommandCompleted {
        RuntimeShellCommandCompleted {
            approval_id: approval_id.map(ToString::to_string),
            shell_session_id: "raw-test".to_string(),
            command_block_id: "cmd-1".to_string(),
            command: "df -h".to_string(),
            cwd: "/tmp".to_string(),
            end_cwd: "/tmp".to_string(),
            status: "completed",
            exit_code: 0,
            duration_ms: 10,
            terminal_output_ref: None,
            redaction_status: "ref_only",
            provider_result_delivered,
            provider_result_delivery_status,
            recovery_reason,
            continuation_state: if provider_result_delivered {
                ShellEvidenceContinuationState::DeliveredToProvider
            } else {
                ShellEvidenceContinuationState::PendingRecovery
            },
        }
    }
}
