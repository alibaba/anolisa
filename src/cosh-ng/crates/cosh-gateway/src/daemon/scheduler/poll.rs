impl<F: RuntimeFactory> TaskScheduler<F> {
    fn poll_active(&mut self, now_ms: u64) -> Result<SchedulerTick, GatewayDaemonError> {
        if self
            .active
            .as_ref()
            .ok_or_else(no_active_run)?
            .terminal
            .is_some()
        {
            return self.finish_terminal(now_ms);
        }
        let cancellation_requested = {
            let active = self.active.as_ref().ok_or_else(no_active_run)?;
            self.coordinator
                .store
                .run_cancellation_requested(&active.scheduled.task_id, &active.scheduled.run_id)?
        };
        if cancellation_requested {
            let cancellation_at_ms = refreshed_now_ms(now_ms)?;
            self.require_active_lease_time(cancellation_at_ms)?;
            self.cancel_pending_input(cancellation_at_ms)?;
            let cancel_result = self
                .active
                .as_mut()
                .ok_or_else(no_active_run)?
                .handle
                .shutdown(CancelReason::UserRequested);
            let cancelled_at_ms = refreshed_now_ms(now_ms)?;
            self.require_active_lease_time(cancelled_at_ms)?;
            return match cancel_result {
                Ok(()) => self.finish_cancelled(cancelled_at_ms),
                Err(error) => {
                    self.active.as_mut().ok_or_else(no_active_run)?.abort_error = Some(error);
                    Err(GatewayDaemonError::Protocol(
                        "Runtime cancellation was not acknowledged".to_owned(),
                    ))
                }
            };
        }
        if let Some(pending) = self
            .active
            .as_ref()
            .ok_or_else(no_active_run)?
            .pending_input
            .as_ref()
        {
            if now_ms < pending.expires_at_ms {
                return Ok(SchedulerTick::Idle);
            }
            return self.expire_active_input(now_ms);
        }
        if let Some(pending) = self
            .active
            .as_ref()
            .ok_or_else(no_active_run)?
            .pending_brokered
            .as_ref()
        {
            if now_ms < pending.approval.expires_at_ms {
                return Ok(SchedulerTick::Idle);
            }
            let approval = self
                .coordinator
                .store
                .load_approval_record(&pending.approval.approval_id)?;
            return self.expire_active_brokered_approval(&approval, now_ms);
        }
        if let Some(pending) = self
            .active
            .as_ref()
            .ok_or_else(no_active_run)?
            .pending_permission
            .as_ref()
        {
            if now_ms < pending.approval.expires_at_ms {
                return Ok(SchedulerTick::Idle);
            }
            let (permission, approval_id) = {
                let active = self.active.as_ref().ok_or_else(no_active_run)?;
                let pending = active
                    .pending_permission
                    .as_ref()
                    .ok_or_else(no_active_run)?;
                (
                    pending.permission.clone(),
                    pending.approval.approval_id.clone(),
                )
            };
            return self.expire_active_provider_approval(&approval_id, &permission, now_ms);
        }
        let abort_error = self
            .active
            .as_ref()
            .ok_or_else(no_active_run)?
            .abort_error
            .clone();
        if let Some(abort_error) = abort_error {
            let acknowledged = self
                .active
                .as_mut()
                .ok_or_else(no_active_run)?
                .handle
                .shutdown(CancelReason::RuntimeShutdown)
                .is_ok();
            if acknowledged {
                let stopped_at_ms = refreshed_now_ms(now_ms)?;
                self.require_active_lease_time(stopped_at_ms)?;
                return self.finish_failed(abort_error, stopped_at_ms);
            }
            return Err(GatewayDaemonError::Protocol(
                "Runtime cancellation after durable start failure is not acknowledged".to_owned(),
            ));
        }
        let poll = self
            .active
            .as_mut()
            .ok_or_else(no_active_run)?
            .handle
            .poll();
        let polled_at_ms = refreshed_now_ms(now_ms)?;
        self.require_active_lease_time(polled_at_ms)?;
        match poll {
            RuntimePoll::Pending => Ok(SchedulerTick::Idle),
            RuntimePoll::Observed { sequence } => {
                let active = self.active.as_ref().ok_or_else(no_active_run)?;
                self.coordinator.record_runtime_binding_sequence(
                    &active.lease,
                    &active.binding,
                    sequence,
                    polled_at_ms,
                )?;
                Ok(SchedulerTick::Idle)
            }
            RuntimePoll::Update { sequence, update } => {
                let active = self.active.as_mut().ok_or_else(no_active_run)?;
                let next_sequence = active.next_event_sequence.checked_add(1).ok_or_else(|| {
                    GatewayDaemonError::Protocol(
                        "Runtime event sequence exceeds the supported range".to_owned(),
                    )
                })?;
                self.coordinator.record_runtime_binding_sequence(
                    &active.lease,
                    &active.binding,
                    sequence,
                    polled_at_ms,
                )?;
                let view = self.coordinator.record_runtime_update(
                    &active.lease,
                    active.next_event_sequence,
                    update,
                    polled_at_ms,
                )?;
                active.next_event_sequence = next_sequence;
                Ok(SchedulerTick::Progressed(view))
            }
            RuntimePoll::PermissionRequested {
                permission,
                request,
                summary,
            } => {
                let active = self.active.as_ref().ok_or_else(no_active_run)?;
                if permission.binding_id != active.binding.binding_id
                    || permission.runtime_generation != active.binding.runtime_generation
                    || permission.run_id != active.scheduled.run_id
                    || request.actor.actor_id != active.scheduled.actor.actor_id
                    || request.task_id != active.scheduled.task_id
                    || request.run_id != active.scheduled.run_id
                    || request.target != active.scheduled.target
                {
                    return self.finish_failed(
                        runtime_lost_error(
                            "runtime_permission_binding_invalid",
                            "Runtime permission request did not match the active Run",
                        )?,
                        polled_at_ms,
                    );
                }
                self.coordinator.record_runtime_binding_sequence(
                    &active.lease,
                    &active.binding,
                    permission.event_sequence,
                    polled_at_ms,
                )?;
                let approval = ApprovalRequest {
                    approval_id: ApprovalId::new(),
                    request_id: request.request_id.clone(),
                    task_id: request.task_id.clone(),
                    run_id: request.run_id.clone(),
                    summary: summary.summary,
                    expires_at_ms: request.expires_at_ms,
                };
                let view = self.coordinator.record_provider_approval(
                    &self.active.as_ref().ok_or_else(no_active_run)?.lease,
                    &permission,
                    &request,
                    &approval,
                    polled_at_ms,
                )?;
                self.active
                    .as_mut()
                    .ok_or_else(no_active_run)?
                    .pending_permission = Some(PendingPermission {
                    permission,
                    approval,
                });
                Ok(SchedulerTick::Progressed(view))
            }
            RuntimePoll::BrokeredExecutionRequested {
                brokered,
                request,
                operation,
                summary,
            } => {
                self.admit_brokered_execution(brokered, *request, operation, summary, polled_at_ms)
            }
            RuntimePoll::InputRequested { sequence, request } => {
                let active = self.active.as_ref().ok_or_else(no_active_run)?;
                if request.run_id() != &active.scheduled.run_id
                    || active.pending_input.is_some()
                    || active.pending_permission.is_some()
                    || active.pending_brokered.is_some()
                {
                    return self.finish_failed(
                        runtime_lost_error(
                            "runtime_input_binding_invalid",
                            "Runtime input request did not match the active Run",
                        )?,
                        polled_at_ms,
                    );
                }
                let expires_at_ms = polled_at_ms
                    .checked_add(DEFAULT_RUNTIME_INPUT_TIMEOUT_MS)
                    .ok_or_else(|| {
                        GatewayDaemonError::Protocol(
                            "Runtime input deadline exceeds the supported range".to_owned(),
                        )
                    })?;
                let command = LedgerCommand {
                    actor_id: active.scheduled.actor.actor_id.clone(),
                    idempotency_key: IdempotencyKey::new(format!(
                        "scheduler-input-request-{}",
                        request.request_id().as_str()
                    ))
                    .map_err(|error| GatewayDaemonError::Protocol(error.to_string()))?,
                    command_digest: digest_json(&(
                        "record_runtime_input_request",
                        &request,
                        expires_at_ms,
                        &active.binding,
                        sequence,
                        active.lease.generation,
                    ))?,
                    committed_at_ms: polled_at_ms,
                };
                let record = match self.coordinator.store.record_runtime_input_request(
                    &command,
                    &request,
                    expires_at_ms,
                    &active.binding.binding_id,
                    &active.binding.runtime_instance_id,
                    active.binding.runtime_generation,
                    sequence,
                    &active.lease,
                )? {
                    LedgerOutcome::Applied(record) | LedgerOutcome::Replayed(record) => record,
                };
                let view = TaskView::from(
                    &self
                        .coordinator
                        .store
                        .load_task(&active.scheduled.task_id)?,
                );
                #[cfg(test)]
                if std::mem::take(&mut self.fail_next_input_request_install) {
                    return Err(GatewayDaemonError::Protocol(
                        "injected failure before pending input installation".to_owned(),
                    ));
                }
                self.active
                    .as_mut()
                    .ok_or_else(no_active_run)?
                    .pending_input = Some(record);
                Ok(SchedulerTick::Progressed(view))
            }
            RuntimePoll::Succeeded => self.finish_succeeded(polled_at_ms),
            RuntimePoll::Failed(error) => self.finish_failed(error, polled_at_ms),
            RuntimePoll::Cancelled => Err(GatewayDaemonError::Protocol(
                "Runtime reported cancellation without a durable request".to_owned(),
            )),
        }
    }

    fn cancel_pending_input(&mut self, now_ms: u64) -> Result<(), GatewayDaemonError> {
        let pending = if let Some(pending) = self
            .active
            .as_ref()
            .ok_or_else(no_active_run)?
            .pending_input
            .clone()
        {
            pending
        } else {
            let (actor_id, task_id, run_id) = {
                let active = self.active.as_ref().ok_or_else(no_active_run)?;
                (
                    active.scheduled.actor.actor_id.clone(),
                    active.scheduled.task_id.clone(),
                    active.scheduled.run_id.clone(),
                )
            };
            let task = self.coordinator.store.load_task(&task_id)?;
            let Some(request_id) = task.pending_input_request_id().cloned() else {
                return Ok(());
            };
            let pending = self
                .coordinator
                .store
                .load_runtime_input_request(&request_id)?;
            if pending.actor_id != actor_id
                || pending.task_id != task_id
                || pending.run_id != run_id
                || pending.state != RuntimeInputRequestState::Pending
            {
                return Err(GatewayDaemonError::Protocol(
                    "durable pending input does not match the active Run".to_owned(),
                ));
            }
            pending
        };
        let command = runtime_input_command(
            &pending.actor_id,
            "cancel",
            pending.request.request_id(),
            pending.revision,
            now_ms,
        )?;
        let cancelled = match self.coordinator.store.cancel_runtime_input_request(
            &command,
            pending.request.request_id(),
            pending.revision,
        )? {
            LedgerOutcome::Applied(record) | LedgerOutcome::Replayed(record) => record,
        };
        if cancelled.state != RuntimeInputRequestState::Cancelled {
            return Err(GatewayDaemonError::Protocol(
                "Runtime input cancellation did not reach its durable terminal state".to_owned(),
            ));
        }
        self.active
            .as_mut()
            .ok_or_else(no_active_run)?
            .pending_input = None;
        Ok(())
    }

    fn expire_active_input(&mut self, now_ms: u64) -> Result<SchedulerTick, GatewayDaemonError> {
        let pending = self
            .active
            .as_ref()
            .ok_or_else(no_active_run)?
            .pending_input
            .clone()
            .ok_or_else(no_active_run)?;
        let command = runtime_input_command(
            &pending.actor_id,
            "expire",
            pending.request.request_id(),
            pending.revision,
            now_ms,
        )?;
        let expired = match self.coordinator.store.expire_runtime_input_request(
            &command,
            pending.request.request_id(),
            pending.revision,
        )? {
            LedgerOutcome::Applied(record) | LedgerOutcome::Replayed(record) => record,
        };
        if expired.state != RuntimeInputRequestState::Expired {
            return Err(GatewayDaemonError::Protocol(
                "Runtime input expiry did not reach its durable terminal state".to_owned(),
            ));
        }
        self.active
            .as_mut()
            .ok_or_else(no_active_run)?
            .pending_input = None;
        self.finish_suspended_after_input(now_ms)
    }

    fn finish_suspended_after_input(
        &mut self,
        now_ms: u64,
    ) -> Result<SchedulerTick, GatewayDaemonError> {
        self.require_active_lease_time(now_ms)?;
        let stopped = self
            .active
            .as_mut()
            .ok_or_else(no_active_run)?
            .handle
            .shutdown(CancelReason::RuntimeShutdown)
            .is_ok();
        if !stopped {
            return Err(GatewayDaemonError::Protocol(
                "Runtime shutdown after input convergence was not acknowledged".to_owned(),
            ));
        }
        let stopped_at_ms = refreshed_now_ms(now_ms)?;
        self.require_active_lease_time(stopped_at_ms)?;
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
            .close_runtime_binding(&actor_id, &lease, &binding, stopped_at_ms)?;
        let task = self.coordinator.store.load_task(&task_id)?;
        if task.state() != TaskState::Suspended {
            return Err(GatewayDaemonError::Protocol(
                "Runtime input convergence did not suspend its Task".to_owned(),
            ));
        }
        self.coordinator.release_lease(&lease, stopped_at_ms)?;
        self.active.take();
        Ok(SchedulerTick::Settled(TaskView::from(&task)))
    }

    fn expire_active_provider_approval(
        &mut self,
        approval_id: &ApprovalId,
        permission: &RuntimePermissionRef,
        now_ms: u64,
    ) -> Result<SchedulerTick, GatewayDaemonError> {
        let (actor_id, lease) = {
            let active = self.active.as_ref().ok_or_else(no_active_run)?;
            (
                active.scheduled.actor.actor_id.clone(),
                active.lease.clone(),
            )
        };
        let command = LedgerCommand {
            actor_id,
            idempotency_key: IdempotencyKey::new(format!(
                "scheduler-expire-provider-{}",
                approval_id.as_str()
            ))
            .map_err(|error| GatewayDaemonError::Protocol(error.to_string()))?,
            command_digest: digest_json(&(
                "expire_provider_approval",
                approval_id,
                permission,
                lease.generation,
            ))?,
            committed_at_ms: now_ms,
        };
        self.coordinator.store.expire_provider_approval(
            &command,
            approval_id,
            permission,
            &lease,
        )?;
        let expiry_error = runtime_lost_error(
            "provider_permission_expired",
            "The provider-native approval expired before it was resolved",
        )?;
        let shutdown_acknowledged = self
            .active
            .as_mut()
            .ok_or_else(no_active_run)?
            .handle
            .shutdown(CancelReason::RuntimeShutdown)
            .is_ok();
        if !shutdown_acknowledged {
            self.active.as_mut().ok_or_else(no_active_run)?.abort_error = Some(expiry_error);
            return Err(GatewayDaemonError::Protocol(
                "Runtime cancellation after approval expiry was not acknowledged".to_owned(),
            ));
        }
        let expired_at_ms = refreshed_now_ms(now_ms)?;
        self.require_active_lease_time(expired_at_ms)?;
        self.finish_failed(expiry_error, expired_at_ms)
    }

}
