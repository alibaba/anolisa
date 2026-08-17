//! Brokered approval, governed execution, and non-replayable Runtime dispatch.

use super::*;

/// Trusted input for admitting one Runtime-originated brokered operation.
pub struct BrokeredApprovalContext<'a> {
    /// Active authenticated Run that owns the callback.
    pub scheduled: &'a ScheduledRun,
    /// Exact Runtime callback identity.
    pub brokered: &'a BrokeredExecutionRef,
    /// Gateway-normalized capability request.
    pub request: &'a CapabilityRequest,
    /// Closed typed operation proposed for governed execution.
    pub operation: &'a BrokeredOperation,
    /// Redacted presentation supplied to policy.
    pub summary: &'a ToolSummary,
    /// Runtime and lease generation fence captured at admission.
    pub runtime_fence: &'a RuntimeExecutionFence,
    /// Admission timestamp.
    pub now_ms: u64,
}

/// Policy-produced approval plan with a trusted immutable target identity.
pub struct BrokeredApprovalPlan {
    /// Pending approval to persist before acknowledging the Runtime.
    pub approval: ApprovalRequest,
    /// Digest produced by a trusted target resolver, never by the scheduler.
    pub target_identity_digest: Digest,
}

/// Trusted input for resolving and optionally executing one brokered request.
pub struct BrokeredResolutionContext<'a> {
    /// Exact durable approval being resolved.
    pub approval: &'a ApprovalRecord,
    /// Complete durable request and target binding.
    pub request: &'a BrokeredRequestRecord,
    /// Exact live Runtime callback identity.
    pub brokered: &'a BrokeredExecutionRef,
    /// Current exact Run-lease claim; renewals may advance its revision only.
    pub lease: &'a LeaseClaim,
    /// Caller-stable key from the authenticated approval command.
    pub idempotency_key: &'a IdempotencyKey,
    /// Explicit actor decision.
    pub decision: ApprovalDecision,
    /// Resolution timestamp.
    pub now_ms: u64,
}

/// Durable authority backing one terminal brokered Runtime result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrokeredResolutionSource {
    /// A durable explicit approval denial proves that no target ran.
    ApprovalDenied {
        /// Explicitly denied approval authorizing the result.
        approval_id: ApprovalId,
    },
    /// A durable governed execution proves the typed target outcome.
    Execution {
        /// Governed execution authorizing the result.
        execution_id: ExecutionId,
    },
}

/// Result returned after the driver has durably resolved policy and execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrokeredResolution {
    /// Durable ledger fact that authorizes result dispatch.
    pub source: BrokeredResolutionSource,
    /// Complete typed payload delivered to the Runtime without a Permit.
    pub delivery: BrokeredExecutionDelivery,
}

/// Injected trusted boundary for brokered policy, target resolution, and execution.
pub trait BrokeredExecutionDriver: Send {
    /// Resolves immutable target identity and produces an approval plan.
    ///
    /// # Errors
    ///
    /// Returns a bounded failure when policy cannot safely admit the request.
    fn plan_approval(
        &mut self,
        context: BrokeredApprovalContext<'_>,
    ) -> Result<BrokeredApprovalPlan, ContractError>;

    /// Resolves policy and performs any approved target execution durably.
    ///
    /// The implementation owns policy re-evaluation, single-use Permit
    /// issuance, security audit, and typed target invocation. The scheduler
    /// owns only the subsequent non-replayable Runtime dispatch.
    ///
    /// # Errors
    ///
    /// Returns a bounded failure when the request cannot converge safely.
    fn resolve(
        &mut self,
        store: &mut SqliteTaskStore,
        context: BrokeredResolutionContext<'_>,
    ) -> Result<BrokeredResolution, ContractError>;
}

pub(super) struct RejectingBrokeredExecutionDriver;

impl BrokeredExecutionDriver for RejectingBrokeredExecutionDriver {
    fn plan_approval(
        &mut self,
        _context: BrokeredApprovalContext<'_>,
    ) -> Result<BrokeredApprovalPlan, ContractError> {
        Err(runtime_handle_unsupported("brokered execution policy"))
    }

    fn resolve(
        &mut self,
        _store: &mut SqliteTaskStore,
        _context: BrokeredResolutionContext<'_>,
    ) -> Result<BrokeredResolution, ContractError> {
        Err(runtime_handle_unsupported("brokered execution policy"))
    }
}

#[derive(Debug, Clone)]
pub(super) struct PendingBrokered {
    pub(super) brokered: BrokeredExecutionRef,
    pub(super) approval: ApprovalRequest,
    pub(super) resolution: Option<BrokeredResolution>,
}

impl<F: RuntimeFactory> TaskScheduler<F> {
    pub(super) fn resolve_brokered_approval(
        &mut self,
        actor_id: &ActorId,
        idempotency_key: IdempotencyKey,
        approval: ApprovalRecord,
        decision: ApprovalDecision,
        now_ms: u64,
    ) -> Result<SchedulerTick, GatewayDaemonError> {
        let resumable_approved = approval.state == ApprovalState::Approved
            && decision == ApprovalDecision::Approve
            && self.active.as_ref().is_some_and(|active| {
                active.scheduled.actor.actor_id == *actor_id
                    && active.pending_brokered.as_ref().is_some_and(|pending| {
                        pending.approval.approval_id == approval.approval_id
                            && pending.resolution.is_none()
                    })
            })
            && matches!(
                self.coordinator
                    .store
                    .load_brokered_runtime_dispatch_record(
                        &approval.request_id,
                        BrokeredRuntimeDispatchKind::Result,
                    ),
                Err(StoreError::LedgerNotFound { .. })
            );
        if approval.state != ApprovalState::Pending && !resumable_approved {
            return self.replay_resolved_brokered_approval(actor_id, &approval, decision, now_ms);
        }
        if self.active.is_none() {
            return Err(GatewayDaemonError::Protocol(
                "brokered approval requires its live Runtime handle".to_owned(),
            ));
        }
        self.ensure_active_operation_budget(now_ms)?;
        let (lease, pending, task_id, run_id) = {
            let active = self.active.as_ref().ok_or_else(no_active_run)?;
            if &active.scheduled.actor.actor_id != actor_id {
                return Err(GatewayDaemonError::Unauthorized);
            }
            (
                active.lease.clone(),
                active.pending_brokered.clone(),
                active.scheduled.task_id.clone(),
                active.scheduled.run_id.clone(),
            )
        };
        let pending = pending.ok_or_else(|| {
            GatewayDaemonError::Protocol(
                "brokered approval does not match a live Runtime callback".to_owned(),
            )
        })?;
        if pending.approval.approval_id != approval.approval_id
            || pending.approval.request_id != approval.request_id
            || approval.task_id != task_id
            || approval.run_id != run_id
        {
            return Err(GatewayDaemonError::Unauthorized);
        }
        if approval.state == ApprovalState::Pending && now_ms >= approval.expires_at_ms {
            self.expire_active_brokered_approval(&approval, now_ms)?;
            return Err(GatewayDaemonError::Protocol(
                "brokered approval is no longer resolvable".to_owned(),
            ));
        }
        let request = self
            .coordinator
            .store
            .load_brokered_request(&approval.request_id)?;
        if request.approval_id.as_ref() != Some(&approval.approval_id)
            || Some(&request.runtime_fence) != approval.runtime_fence.as_ref()
            || Some(&request.target_identity_digest) != approval.target_identity_digest.as_ref()
            || request.request.actor.actor_id != *actor_id
            || request.request.task_id != task_id
            || request.request.run_id != run_id
            || request.request.request_id != pending.brokered.request_id
            || request.operation != pending.brokered.operation
        {
            return Err(GatewayDaemonError::Unauthorized);
        }
        let resolution = match self.brokered_driver.resolve(
            &mut self.coordinator.store,
            BrokeredResolutionContext {
                approval: &approval,
                request: &request,
                brokered: &pending.brokered,
                lease: &lease,
                idempotency_key: &idempotency_key,
                decision,
                now_ms,
            },
        ) {
            Ok(resolution) => resolution,
            Err(error) => return self.shutdown_after_brokered_failure(error, now_ms),
        };
        validate_resolution(&approval, decision, &resolution)?;
        self.active
            .as_mut()
            .ok_or_else(no_active_run)?
            .pending_brokered
            .as_mut()
            .ok_or_else(no_active_run)?
            .resolution = Some(resolution.clone());
        let payload_digest = digest_json(&resolution.delivery)?;
        let prepare_command = brokered_dispatch_command(
            actor_id,
            "prepare",
            BrokeredRuntimeDispatchKind::Result,
            &pending.brokered,
            0,
            now_ms,
        )?;
        let prepared_outcome = match &resolution.source {
            BrokeredResolutionSource::ApprovalDenied { approval_id } => self
                .coordinator
                .store
                .prepare_brokered_denied_result_dispatch(
                    &prepare_command,
                    approval_id,
                    &pending.brokered,
                    &resolution.delivery,
                    &lease,
                )?,
            BrokeredResolutionSource::Execution { execution_id } => self
                .coordinator
                .store
                .prepare_brokered_execution_result_dispatch(
                    &prepare_command,
                    execution_id,
                    &pending.brokered,
                    &resolution.delivery,
                    &lease,
                )?,
        };
        let prepared = match prepared_outcome {
            LedgerOutcome::Applied(record) => record,
            LedgerOutcome::Replayed(record) => {
                return self.reject_replayed_brokered_dispatch(record, now_ms)
            }
        };
        let uncertain = matches!(
            &resolution.delivery.outcome,
            cosh_gateway_contracts::runtime::BrokeredExecutionOutcome::Uncertain { .. }
        );
        if let Some(tick) = self.dispatch_brokered_result(
            actor_id,
            &lease,
            &pending.brokered,
            resolution.delivery,
            &payload_digest,
            prepared,
            now_ms,
        )? {
            return Ok(tick);
        }
        self.active
            .as_mut()
            .ok_or_else(no_active_run)?
            .pending_brokered = None;
        if uncertain {
            return self.finish_suspended_after_brokered_result(now_ms);
        }
        let task = self.coordinator.store.load_task(&task_id)?;
        Ok(SchedulerTick::Progressed(TaskView::from(&task)))
    }

    pub(super) fn admit_brokered_execution(
        &mut self,
        brokered: BrokeredExecutionRef,
        request: CapabilityRequest,
        operation: BrokeredOperation,
        summary: ToolSummary,
        now_ms: u64,
    ) -> Result<SchedulerTick, GatewayDaemonError> {
        let (scheduled, binding, lease) = {
            let active = self.active.as_ref().ok_or_else(no_active_run)?;
            if active.pending_permission.is_some() || active.pending_brokered.is_some() {
                return self.finish_failed(
                    runtime_lost_error(
                        "runtime_brokered_callback_order_invalid",
                        "Runtime emitted another callback while one was pending",
                    )?,
                    now_ms,
                );
            }
            (
                active.scheduled.clone(),
                active.binding.clone(),
                active.lease.clone(),
            )
        };
        if brokered.binding_id != binding.binding_id
            || brokered.runtime_generation != binding.runtime_generation
            || brokered.run_id != scheduled.run_id
            || brokered.request_id != request.request_id
            || brokered.operation != operation
            || request.actor.actor_id != scheduled.actor.actor_id
            || request.task_id != scheduled.task_id
            || request.run_id != scheduled.run_id
            || request.target != scheduled.target
            || request.expires_at_ms <= now_ms
        {
            return self.finish_failed(
                runtime_lost_error(
                    "runtime_brokered_binding_invalid",
                    "Brokered Runtime request did not match the active Run",
                )?,
                now_ms,
            );
        }
        self.coordinator.record_runtime_binding_sequence(
            &lease,
            &binding,
            brokered.event_sequence,
            now_ms,
        )?;
        let runtime_fence = RuntimeExecutionFence {
            binding_id: binding.binding_id.clone(),
            runtime_generation: binding.runtime_generation,
            lease_generation: lease.generation,
            lease_revision: lease.revision,
        };
        let plan = match self.brokered_driver.plan_approval(BrokeredApprovalContext {
            scheduled: &scheduled,
            brokered: &brokered,
            request: &request,
            operation: &operation,
            summary: &summary,
            runtime_fence: &runtime_fence,
            now_ms,
        }) {
            Ok(plan) => plan,
            Err(error) => return self.shutdown_after_brokered_failure(error, now_ms),
        };
        if plan.approval.expires_at_ms > request.expires_at_ms
            || plan.approval.expires_at_ms <= now_ms
        {
            return self.finish_failed(
                runtime_lost_error(
                    "brokered_approval_plan_invalid",
                    "Brokered approval plan did not preserve the admitted request",
                )?,
                now_ms,
            );
        }
        let command = brokered_dispatch_command(
            &scheduled.actor.actor_id,
            "admit",
            BrokeredRuntimeDispatchKind::Acknowledgement,
            &brokered,
            0,
            now_ms,
        )?;
        let approval_record = DurableApprovalCoordinator::new(&mut self.coordinator.store)
            .record_pending(
                &command,
                &request,
                &plan.approval,
                crate::capability::BrokeredApprovalBinding {
                    operation: &operation,
                    target_identity_digest: &plan.target_identity_digest,
                    runtime_fence: &runtime_fence,
                },
            )
            .map_err(|error| GatewayDaemonError::Protocol(error.to_string()))?;
        if approval_record.state != ApprovalState::Pending {
            return self.finish_failed(
                runtime_lost_error(
                    "brokered_approval_not_pending",
                    "Brokered approval was not durably pending before acknowledgement",
                )?,
                now_ms,
            );
        }
        let acknowledgement = BrokeredRequestAcknowledgement {
            request_id: request.request_id,
            approval_id: plan.approval.approval_id.clone(),
        };
        let payload_digest = digest_json(&acknowledgement)?;
        let prepare_command = brokered_dispatch_command(
            &scheduled.actor.actor_id,
            "prepare",
            BrokeredRuntimeDispatchKind::Acknowledgement,
            &brokered,
            0,
            now_ms,
        )?;
        let prepared = match self
            .coordinator
            .store
            .prepare_brokered_acknowledgement_dispatch(
                &prepare_command,
                &plan.approval.approval_id,
                &brokered,
                &payload_digest,
                &lease,
            )? {
            LedgerOutcome::Applied(record) => record,
            LedgerOutcome::Replayed(record) => {
                return self.reject_replayed_brokered_dispatch(record, now_ms)
            }
        };
        if let Some(tick) = self.dispatch_brokered_acknowledgement(
            &scheduled.actor.actor_id,
            &lease,
            &brokered,
            acknowledgement,
            &payload_digest,
            prepared,
            now_ms,
        )? {
            return Ok(tick);
        }
        let task = self.coordinator.store.load_task(&scheduled.task_id)?;
        self.active
            .as_mut()
            .ok_or_else(no_active_run)?
            .pending_brokered = Some(PendingBrokered {
            brokered,
            approval: plan.approval,
            resolution: None,
        });
        Ok(SchedulerTick::Progressed(TaskView::from(&task)))
    }

    fn replay_resolved_brokered_approval(
        &mut self,
        actor_id: &ActorId,
        approval: &ApprovalRecord,
        decision: ApprovalDecision,
        now_ms: u64,
    ) -> Result<SchedulerTick, GatewayDaemonError> {
        if !matches!(
            (approval.state, decision),
            (ApprovalState::Denied, ApprovalDecision::Deny)
                | (ApprovalState::Approved, ApprovalDecision::Approve)
        ) {
            return Err(GatewayDaemonError::Protocol(
                "brokered approval was already resolved with another decision".to_owned(),
            ));
        }
        let request = self
            .coordinator
            .store
            .load_brokered_request(&approval.request_id)?;
        if request.approval_id.as_ref() != Some(&approval.approval_id)
            || request.request.actor.actor_id != *actor_id
            || request.request.task_id != approval.task_id
            || request.request.run_id != approval.run_id
            || Some(&request.runtime_fence) != approval.runtime_fence.as_ref()
            || Some(&request.target_identity_digest) != approval.target_identity_digest.as_ref()
        {
            return Err(GatewayDaemonError::Unauthorized);
        }
        let dispatch = self
            .coordinator
            .store
            .load_brokered_runtime_dispatch_record(
                &approval.request_id,
                BrokeredRuntimeDispatchKind::Result,
            )
            .map_err(|error| match error {
                StoreError::LedgerNotFound { .. } => GatewayDaemonError::Protocol(
                    "brokered result was resolved without a durable dispatch".to_owned(),
                ),
                other => GatewayDaemonError::Store(other),
            })?;
        validate_replayed_dispatch(
            actor_id,
            approval,
            &request,
            decision,
            &dispatch,
            &self.coordinator.store,
        )?;
        match dispatch.state {
            BrokeredRuntimeDispatchState::Delivered => {
                let task = self.coordinator.store.load_task(&approval.task_id)?;
                Ok(SchedulerTick::Progressed(TaskView::from(&task)))
            }
            BrokeredRuntimeDispatchState::Started => {
                self.coordinator
                    .store
                    .mark_brokered_dispatches_unknown_for_run(&approval.run_id, now_ms)?;
                Err(indeterminate_replay_error())
            }
            BrokeredRuntimeDispatchState::Unknown => Err(indeterminate_replay_error()),
            BrokeredRuntimeDispatchState::Prepared => {
                self.resume_prepared_brokered_result(actor_id, approval, decision, dispatch, now_ms)
            }
        }
    }

    fn resume_prepared_brokered_result(
        &mut self,
        actor_id: &ActorId,
        approval: &ApprovalRecord,
        decision: ApprovalDecision,
        dispatch: BrokeredRuntimeDispatchRecord,
        now_ms: u64,
    ) -> Result<SchedulerTick, GatewayDaemonError> {
        let (lease, pending, resolution) = {
            let active = self.active.as_ref().ok_or_else(|| {
                GatewayDaemonError::Protocol(
                    "prepared brokered result lost its live Runtime payload".to_owned(),
                )
            })?;
            let pending = active.pending_brokered.as_ref().ok_or_else(|| {
                GatewayDaemonError::Protocol(
                    "prepared brokered result lost its live Runtime callback".to_owned(),
                )
            })?;
            if active.scheduled.actor.actor_id != *actor_id
                || active.scheduled.task_id != approval.task_id
                || active.scheduled.run_id != approval.run_id
                || pending.approval.approval_id != approval.approval_id
                || pending.brokered != dispatch.brokered
            {
                return Err(GatewayDaemonError::Unauthorized);
            }
            let resolution = pending.resolution.clone().ok_or_else(|| {
                GatewayDaemonError::Protocol(
                    "prepared brokered result lost its exact payload".to_owned(),
                )
            })?;
            (active.lease.clone(), pending.clone(), resolution)
        };
        validate_resolution(approval, decision, &resolution)?;
        let payload_digest = digest_json(&resolution.delivery)?;
        if payload_digest != dispatch.payload_digest {
            return Err(GatewayDaemonError::Unauthorized);
        }
        let uncertain = matches!(
            &resolution.delivery.outcome,
            cosh_gateway_contracts::runtime::BrokeredExecutionOutcome::Uncertain { .. }
        );
        if let Some(tick) = self.dispatch_brokered_result(
            actor_id,
            &lease,
            &pending.brokered,
            resolution.delivery,
            &payload_digest,
            dispatch,
            now_ms,
        )? {
            return Ok(tick);
        }
        self.active
            .as_mut()
            .ok_or_else(no_active_run)?
            .pending_brokered = None;
        if uncertain {
            return self.finish_suspended_after_brokered_result(now_ms);
        }
        let task = self.coordinator.store.load_task(&approval.task_id)?;
        Ok(SchedulerTick::Progressed(TaskView::from(&task)))
    }

    fn dispatch_brokered_acknowledgement(
        &mut self,
        actor_id: &ActorId,
        lease: &LeaseClaim,
        brokered: &BrokeredExecutionRef,
        acknowledgement: BrokeredRequestAcknowledgement,
        payload_digest: &Digest,
        prepared: BrokeredRuntimeDispatchRecord,
        now_ms: u64,
    ) -> Result<Option<SchedulerTick>, GatewayDaemonError> {
        if prepared.state != BrokeredRuntimeDispatchState::Prepared {
            return self
                .reject_replayed_brokered_dispatch(prepared, now_ms)
                .map(Some);
        }
        let start_command = brokered_dispatch_command(
            actor_id,
            "start",
            BrokeredRuntimeDispatchKind::Acknowledgement,
            brokered,
            prepared.revision,
            now_ms,
        )?;
        let started = match self.coordinator.store.start_brokered_runtime_dispatch(
            &start_command,
            BrokeredRuntimeDispatchKind::Acknowledgement,
            brokered,
            payload_digest,
            prepared.revision,
            lease,
        )? {
            LedgerOutcome::Applied(record) => record,
            LedgerOutcome::Replayed(record) => {
                return self
                    .reject_replayed_brokered_dispatch(record, now_ms)
                    .map(Some)
            }
        };
        let write = self
            .active
            .as_mut()
            .ok_or_else(no_active_run)?
            .handle
            .acknowledge_brokered_request(brokered, acknowledgement);
        let dispatched_at_ms = refreshed_now_ms(now_ms)?;
        self.require_active_lease_time(dispatched_at_ms)?;
        if let Err(error) = write {
            return self
                .fail_unknown_brokered_dispatch(actor_id, lease, &started, error, dispatched_at_ms)
                .map(Some);
        }
        let complete_command = brokered_dispatch_command(
            actor_id,
            "complete",
            BrokeredRuntimeDispatchKind::Acknowledgement,
            brokered,
            started.revision,
            dispatched_at_ms,
        )?;
        match self.coordinator.store.complete_brokered_runtime_dispatch(
            &complete_command,
            BrokeredRuntimeDispatchKind::Acknowledgement,
            brokered,
            payload_digest,
            started.revision,
            lease,
        ) {
            Ok(LedgerOutcome::Applied(record))
                if record.state == BrokeredRuntimeDispatchState::Delivered =>
            {
                Ok(None)
            }
            Ok(LedgerOutcome::Applied(record)) | Ok(LedgerOutcome::Replayed(record)) => self
                .reject_replayed_brokered_dispatch(record, dispatched_at_ms)
                .map(Some),
            Err(_) => self
                .fail_unknown_brokered_dispatch(
                    actor_id,
                    lease,
                    &started,
                    runtime_lost_error(
                        "brokered_acknowledgement_receipt_unknown",
                        "Runtime accepted an acknowledgement whose receipt could not be persisted",
                    )?,
                    dispatched_at_ms,
                )
                .map(Some),
        }
    }

    fn dispatch_brokered_result(
        &mut self,
        actor_id: &ActorId,
        lease: &LeaseClaim,
        brokered: &BrokeredExecutionRef,
        delivery: BrokeredExecutionDelivery,
        payload_digest: &Digest,
        prepared: BrokeredRuntimeDispatchRecord,
        now_ms: u64,
    ) -> Result<Option<SchedulerTick>, GatewayDaemonError> {
        if prepared.state != BrokeredRuntimeDispatchState::Prepared {
            return self
                .reject_replayed_brokered_dispatch(prepared, now_ms)
                .map(Some);
        }
        let start_command = brokered_dispatch_command(
            actor_id,
            "start",
            BrokeredRuntimeDispatchKind::Result,
            brokered,
            prepared.revision,
            now_ms,
        )?;
        let started = match self.coordinator.store.start_brokered_runtime_dispatch(
            &start_command,
            BrokeredRuntimeDispatchKind::Result,
            brokered,
            payload_digest,
            prepared.revision,
            lease,
        )? {
            LedgerOutcome::Applied(record) => record,
            LedgerOutcome::Replayed(record) => {
                return self
                    .reject_replayed_brokered_dispatch(record, now_ms)
                    .map(Some)
            }
        };
        let write = self
            .active
            .as_mut()
            .ok_or_else(no_active_run)?
            .handle
            .deliver_brokered_result(brokered, delivery);
        let dispatched_at_ms = refreshed_now_ms(now_ms)?;
        self.require_active_lease_time(dispatched_at_ms)?;
        if let Err(error) = write {
            return self
                .fail_unknown_brokered_dispatch(actor_id, lease, &started, error, dispatched_at_ms)
                .map(Some);
        }
        #[cfg(test)]
        if std::mem::take(&mut self.fail_next_brokered_result_completion) {
            return self
                .fail_unknown_brokered_dispatch(
                    actor_id,
                    lease,
                    &started,
                    runtime_lost_error(
                        "brokered_result_receipt_unknown",
                        "Runtime accepted a brokered result whose receipt could not be persisted",
                    )?,
                    dispatched_at_ms,
                )
                .map(Some);
        }
        let complete_command = brokered_dispatch_command(
            actor_id,
            "complete",
            BrokeredRuntimeDispatchKind::Result,
            brokered,
            started.revision,
            dispatched_at_ms,
        )?;
        match self.coordinator.store.complete_brokered_runtime_dispatch(
            &complete_command,
            BrokeredRuntimeDispatchKind::Result,
            brokered,
            payload_digest,
            started.revision,
            lease,
        ) {
            Ok(LedgerOutcome::Applied(record))
                if record.state == BrokeredRuntimeDispatchState::Delivered =>
            {
                Ok(None)
            }
            Ok(LedgerOutcome::Applied(record)) | Ok(LedgerOutcome::Replayed(record)) => self
                .reject_replayed_brokered_dispatch(record, dispatched_at_ms)
                .map(Some),
            Err(_) => self
                .fail_unknown_brokered_dispatch(
                    actor_id,
                    lease,
                    &started,
                    runtime_lost_error(
                        "brokered_result_receipt_unknown",
                        "Runtime accepted a brokered result whose receipt could not be persisted",
                    )?,
                    dispatched_at_ms,
                )
                .map(Some),
        }
    }

    pub(super) fn expire_active_brokered_approval(
        &mut self,
        approval: &ApprovalRecord,
        now_ms: u64,
    ) -> Result<SchedulerTick, GatewayDaemonError> {
        let command = LedgerCommand {
            actor_id: approval.actor_id.clone(),
            idempotency_key: IdempotencyKey::new(format!(
                "scheduler-expire-brokered-{}",
                approval.approval_id.as_str()
            ))
            .map_err(|error| GatewayDaemonError::Protocol(error.to_string()))?,
            command_digest: digest_json(&(
                "expire_brokered_approval",
                &approval.approval_id,
                approval.revision,
            ))?,
            committed_at_ms: now_ms,
        };
        let resolved = self.coordinator.store.resolve_approval(
            &command,
            &approval.approval_id,
            approval.revision,
            crate::storage::ApprovalResolution::Cancel,
        )?;
        let expired = match resolved {
            LedgerOutcome::Applied(record) | LedgerOutcome::Replayed(record) => record,
        };
        if expired.state != ApprovalState::Expired {
            return Err(GatewayDaemonError::Protocol(
                "brokered approval expiry did not persist the expired state".to_owned(),
            ));
        }
        let error = runtime_lost_error(
            "brokered_approval_expired",
            "The brokered approval expired before it was resolved",
        )?;
        self.shutdown_after_brokered_failure(error, now_ms)
    }

    fn finish_suspended_after_brokered_result(
        &mut self,
        now_ms: u64,
    ) -> Result<SchedulerTick, GatewayDaemonError> {
        let stopped = self
            .active
            .as_mut()
            .ok_or_else(no_active_run)?
            .handle
            .shutdown(CancelReason::RuntimeShutdown)
            .is_ok();
        if !stopped {
            return Err(GatewayDaemonError::Protocol(
                "Runtime shutdown after an uncertain execution was not acknowledged".to_owned(),
            ));
        }
        let (actor_id, task_id, lease, binding) = {
            let active = self.active.as_ref().ok_or_else(no_active_run)?;
            (
                active.scheduled.actor.actor_id.clone(),
                active.scheduled.task_id.clone(),
                active.lease.clone(),
                active.binding.clone(),
            )
        };
        self.coordinator
            .close_runtime_binding(&actor_id, &lease, &binding, now_ms)?;
        let task = self.coordinator.store.load_task(&task_id)?;
        if task.state() != TaskState::Suspended {
            return Err(GatewayDaemonError::Protocol(
                "uncertain brokered execution did not suspend its Task".to_owned(),
            ));
        }
        self.coordinator.release_lease(&lease, now_ms)?;
        self.active.take();
        Ok(SchedulerTick::Settled(TaskView::from(&task)))
    }

    fn reject_replayed_brokered_dispatch(
        &mut self,
        record: BrokeredRuntimeDispatchRecord,
        now_ms: u64,
    ) -> Result<SchedulerTick, GatewayDaemonError> {
        let error = runtime_lost_error(
            "brokered_runtime_dispatch_replayed",
            "Brokered Runtime callback dispatch cannot be safely replayed",
        )?;
        if record.state == BrokeredRuntimeDispatchState::Started {
            let (actor_id, lease) = {
                let active = self.active.as_ref().ok_or_else(no_active_run)?;
                (
                    active.scheduled.actor.actor_id.clone(),
                    active.lease.clone(),
                )
            };
            return self.fail_unknown_brokered_dispatch(&actor_id, &lease, &record, error, now_ms);
        }
        self.shutdown_after_brokered_failure(error, now_ms)
    }

    fn fail_unknown_brokered_dispatch(
        &mut self,
        actor_id: &ActorId,
        lease: &LeaseClaim,
        record: &BrokeredRuntimeDispatchRecord,
        error: ContractError,
        now_ms: u64,
    ) -> Result<SchedulerTick, GatewayDaemonError> {
        let command = brokered_dispatch_command(
            actor_id,
            "unknown",
            record.kind,
            &record.brokered,
            record.revision,
            now_ms,
        )?;
        match self
            .coordinator
            .store
            .mark_brokered_runtime_dispatch_unknown(
                &command,
                record.kind,
                &record.brokered,
                &record.payload_digest,
                record.revision,
                lease,
            )? {
            LedgerOutcome::Applied(marked)
                if marked.state == BrokeredRuntimeDispatchState::Unknown => {}
            LedgerOutcome::Applied(_) | LedgerOutcome::Replayed(_) => {
                return Err(GatewayDaemonError::Protocol(
                    "brokered Runtime dispatch could not be marked indeterminate".to_owned(),
                ))
            }
        }
        let task_id = self
            .active
            .as_ref()
            .ok_or_else(no_active_run)?
            .scheduled
            .task_id
            .clone();
        if self.coordinator.store.load_task(&task_id)?.state() == TaskState::Suspended {
            return self.finish_suspended_after_brokered_result(now_ms);
        }
        self.shutdown_after_brokered_failure(error, now_ms)
    }

    fn shutdown_after_brokered_failure(
        &mut self,
        error: ContractError,
        now_ms: u64,
    ) -> Result<SchedulerTick, GatewayDaemonError> {
        let acknowledged = self
            .active
            .as_mut()
            .ok_or_else(no_active_run)?
            .handle
            .shutdown(CancelReason::RuntimeShutdown)
            .is_ok();
        if !acknowledged {
            self.active.as_mut().ok_or_else(no_active_run)?.abort_error = Some(error);
            return Err(GatewayDaemonError::Protocol(
                "Runtime shutdown after a brokered dispatch failure was not acknowledged"
                    .to_owned(),
            ));
        }
        self.finish_failed(error, refreshed_now_ms(now_ms)?)
    }
}

fn brokered_dispatch_command(
    actor_id: &ActorId,
    operation: &str,
    kind: BrokeredRuntimeDispatchKind,
    brokered: &BrokeredExecutionRef,
    expected_revision: u64,
    committed_at_ms: u64,
) -> Result<LedgerCommand, GatewayDaemonError> {
    let kind_label = match kind {
        BrokeredRuntimeDispatchKind::Acknowledgement => "ack",
        BrokeredRuntimeDispatchKind::Result => "result",
    };
    Ok(LedgerCommand {
        actor_id: actor_id.clone(),
        idempotency_key: IdempotencyKey::new(format!(
            "scheduler-brokered-{operation}-{kind_label}-{}-{expected_revision}",
            brokered.request_id.as_str()
        ))
        .map_err(|error| GatewayDaemonError::Protocol(error.to_string()))?,
        command_digest: digest_json(&(operation, kind, brokered, expected_revision))?,
        committed_at_ms,
    })
}

fn validate_resolution(
    approval: &ApprovalRecord,
    decision: ApprovalDecision,
    resolution: &BrokeredResolution,
) -> Result<(), GatewayDaemonError> {
    use cosh_gateway_contracts::runtime::BrokeredExecutionOutcome;

    if resolution.delivery.request_id != approval.request_id {
        return Err(GatewayDaemonError::Protocol(
            "brokered result request identity does not match its approval".to_owned(),
        ));
    }
    let valid = match (&resolution.source, &resolution.delivery.outcome, decision) {
        (
            BrokeredResolutionSource::ApprovalDenied { approval_id },
            BrokeredExecutionOutcome::Denied { .. },
            ApprovalDecision::Deny,
        ) => approval_id == &approval.approval_id,
        (
            BrokeredResolutionSource::Execution { execution_id },
            BrokeredExecutionOutcome::Succeeded {
                execution_id: outcome_id,
                ..
            }
            | BrokeredExecutionOutcome::Failed {
                execution_id: outcome_id,
                ..
            }
            | BrokeredExecutionOutcome::Uncertain {
                execution_id: outcome_id,
                ..
            },
            ApprovalDecision::Approve,
        ) => execution_id == outcome_id,
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(GatewayDaemonError::Protocol(
            "brokered driver result does not match the actor decision or durable source".to_owned(),
        ))
    }
}

fn validate_replayed_dispatch(
    actor_id: &ActorId,
    approval: &ApprovalRecord,
    request: &BrokeredRequestRecord,
    decision: ApprovalDecision,
    dispatch: &BrokeredRuntimeDispatchRecord,
    store: &SqliteTaskStore,
) -> Result<(), GatewayDaemonError> {
    let brokered = &dispatch.brokered;
    if dispatch.actor_id != *actor_id
        || dispatch.task_id != approval.task_id
        || dispatch.kind != BrokeredRuntimeDispatchKind::Result
        || brokered.request_id != request.request.request_id
        || brokered.run_id != request.request.run_id
        || brokered.operation != request.operation
        || brokered.binding_id != request.runtime_fence.binding_id
        || brokered.runtime_generation != request.runtime_fence.runtime_generation
    {
        return Err(GatewayDaemonError::Unauthorized);
    }
    match (&dispatch.source, decision) {
        (
            crate::storage::BrokeredRuntimeDispatchSource::ApprovalDenied { approval_id },
            ApprovalDecision::Deny,
        ) if approval.state == ApprovalState::Denied && approval_id == &approval.approval_id => {
            Ok(())
        }
        (
            crate::storage::BrokeredRuntimeDispatchSource::Execution { execution_id },
            ApprovalDecision::Approve,
        ) if approval.state == ApprovalState::Approved => {
            let execution = store.load_execution_record(execution_id)?;
            if execution.actor_id != *actor_id
                || execution.task_id != approval.task_id
                || execution.run_id != approval.run_id
                || execution.target_identity_digest.as_ref()
                    != approval.target_identity_digest.as_ref()
                || execution.runtime_fence.as_ref() != approval.runtime_fence.as_ref()
                || !matches!(
                    execution.state,
                    crate::storage::ExecutionState::Succeeded
                        | crate::storage::ExecutionState::Failed
                        | crate::storage::ExecutionState::Uncertain
                )
            {
                return Err(GatewayDaemonError::Unauthorized);
            }
            Ok(())
        }
        _ => Err(GatewayDaemonError::Unauthorized),
    }
}

fn indeterminate_replay_error() -> GatewayDaemonError {
    GatewayDaemonError::Protocol(
        "brokered result delivery is indeterminate and cannot be replayed".to_owned(),
    )
}

#[cfg(test)]
mod tests;
