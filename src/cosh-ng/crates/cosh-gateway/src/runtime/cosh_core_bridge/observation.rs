impl CoshCoreBridge {
    fn read_next_event(
        &mut self,
        deadline: Instant,
    ) -> Result<RuntimeEventEnvelope, AgentRuntimePortError> {
        if let Some(event) = self.pending_events.pop_front() {
            return self.deliver(event);
        }
        if self.state == BridgeState::Terminal {
            return Err(AgentRuntimePortError::Terminal);
        }
        if self.state != BridgeState::PromptActive {
            return Err(AgentRuntimePortError::InvalidState {
                operation: "next_event",
                state: self.state.name(),
            });
        }

        loop {
            let turn_deadline = self.prompt_deadline.unwrap_or(deadline).min(deadline);
            if Instant::now() >= turn_deadline {
                self.fail_transport("core_prompt_deadline");
                return self
                    .pending_events
                    .pop_front()
                    .ok_or(AgentRuntimePortError::Terminal)
                    .and_then(|event| self.deliver(event));
            }
            let observation = match self.read_observation(turn_deadline, "next_event") {
                Ok(observation) => observation,
                Err(AgentRuntimePortError::Deadline { .. })
                    if deadline < self.prompt_deadline.unwrap_or(deadline) =>
                {
                    return Err(AgentRuntimePortError::Deadline {
                        operation: "next_event",
                    });
                }
                Err(_) => {
                    self.fail_transport("core_transport_failed");
                    return self
                        .pending_events
                        .pop_front()
                        .ok_or(AgentRuntimePortError::Terminal)
                        .and_then(|event| self.deliver(event));
                }
            };
            match self.map_observation(observation) {
                Ok(Some(event)) => return self.deliver(event),
                Ok(None) => {}
                Err(_) => {
                    self.fail_transport("core_protocol_failed");
                    return self
                        .pending_events
                        .pop_front()
                        .ok_or(AgentRuntimePortError::Terminal)
                        .and_then(|event| self.deliver(event));
                }
            }
        }
    }

    fn read_observation(
        &mut self,
        deadline: Instant,
        operation: &'static str,
    ) -> Result<CoshCoreObservation, AgentRuntimePortError> {
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(AgentRuntimePortError::Deadline { operation });
            }
            match self
                .supervisor
                .read_frame_timeout(remaining.min(READ_POLL_INTERVAL))
                .map_err(|_| AgentRuntimePortError::Transport)?
            {
                RuntimeFrameRead::Frame(frame) => {
                    return self
                        .codec
                        .decode_frame(frame.as_bytes())
                        .map_err(|_| AgentRuntimePortError::Protocol);
                }
                RuntimeFrameRead::Eof => {
                    return Ok(self
                        .codec
                        .finish_stdout()
                        .unwrap_or(CoshCoreObservation::ProtocolEndedWithoutResult));
                }
                RuntimeFrameRead::TimedOut => {
                    if self
                        .supervisor
                        .poll_terminal()
                        .map_err(|_| AgentRuntimePortError::Transport)?
                        .is_some()
                    {
                        return Ok(CoshCoreObservation::ProtocolEndedWithoutResult);
                    }
                }
            }
        }
    }

    fn map_observation(
        &mut self,
        observation: CoshCoreObservation,
    ) -> Result<Option<RuntimeEventEnvelope>, AgentRuntimePortError> {
        match observation {
            CoshCoreObservation::Stream(event) => self.map_stream(event),
            CoshCoreObservation::System(message) => {
                if let Some(provider_session_id) = message.provider_session_id {
                    self.require_provider_session(&provider_session_id)?;
                }
                Ok(None)
            }
            CoshCoreObservation::Assistant(message) => {
                self.require_provider_session(&message.provider_session_id)?;
                Ok(None)
            }
            CoshCoreObservation::ToolResults {
                provider_session_id,
                ..
            } => {
                self.require_provider_session(&provider_session_id)?;
                Ok(None)
            }
            CoshCoreObservation::Result(result) => {
                if self.current_message.is_some() || self.pending_input.is_some() {
                    return Err(AgentRuntimePortError::Protocol);
                }
                if let Some(provider_session_id) = result.provider_session_id.as_deref() {
                    self.require_provider_session(provider_session_id)?;
                }
                let turn_id = self
                    .active_turn
                    .take()
                    .ok_or(AgentRuntimePortError::Protocol)?;
                let event = if result.is_error {
                    AgentRuntimeEvent::Completed {
                        turn_id,
                        outcome: TurnOutcome::Failed {
                            error: safe_error(
                                "core_turn_failed",
                                ErrorCategory::RuntimeUnavailable,
                                false,
                                "The Agent runtime reported a failed turn",
                            ),
                        },
                    }
                } else {
                    AgentRuntimeEvent::Completed {
                        turn_id,
                        outcome: TurnOutcome::Completed,
                    }
                };
                self.settle(event);
                self.shutdown_process();
                Ok(self.pending_events.pop_front())
            }
            CoshCoreObservation::ProtocolEndedWithoutResult => {
                Err(AgentRuntimePortError::Transport)
            }
            CoshCoreObservation::ControlRequest(request) => self.map_control_request(request),
            CoshCoreObservation::ControlResponse(_)
            | CoshCoreObservation::RegistryResponse { .. }
            | CoshCoreObservation::Initialized(_) => Err(AgentRuntimePortError::Protocol),
        }
    }

    fn map_control_request(
        &mut self,
        envelope: super::CoshCoreControlRequestEnvelope,
    ) -> Result<Option<RuntimeEventEnvelope>, AgentRuntimePortError> {
        if self.config.execution_profile != CoshCoreExecutionProfile::GatewayBrokeredV1
            || self.state != BridgeState::PromptActive
            || self.pending_input.is_some()
        {
            return Err(AgentRuntimePortError::Protocol);
        }
        let private_request_id = envelope.request_id;
        if let CoshCoreControlRequest::AskUser {
            tool_use_id,
            question,
            options,
            allow_free_text,
            multi_select,
        } = &envelope.request
        {
            let private_tool_use_id = tool_use_id
                .as_ref()
                .ok_or(AgentRuntimePortError::Protocol)?;
            let stable_tool_id = self
                .tool_ids
                .get(private_tool_use_id)
                .cloned()
                .ok_or(AgentRuntimePortError::Protocol)?;
            let turn_id = self
                .active_turn
                .clone()
                .ok_or(AgentRuntimePortError::Protocol)?;
            let options = options
                .iter()
                .map(|option| {
                    Ok(RuntimeInputOption::new(
                        BoundedText::new(option.label.clone())
                            .map_err(|_| AgentRuntimePortError::Protocol)?,
                        option
                            .description
                            .clone()
                            .map(BoundedText::new)
                            .transpose()
                            .map_err(|_| AgentRuntimePortError::Protocol)?,
                    ))
                })
                .collect::<Result<Vec<_>, AgentRuntimePortError>>()?;
            let request = RuntimeInputRequest::new(
                InputRequestId::new(),
                self.config.identity.run_id.clone(),
                turn_id,
                Some(stable_tool_id),
                BoundedText::new(question.clone()).map_err(|_| AgentRuntimePortError::Protocol)?,
                options,
                *allow_free_text,
                *multi_select,
            )
            .map_err(|_| AgentRuntimePortError::Protocol)?;
            self.pending_input = Some(PendingInputRequest {
                private_request_id,
                request: request.clone(),
            });
            return Ok(Some(
                self.event(AgentRuntimeEvent::InputRequested { request }),
            ));
        }
        // The task-only Core inventory contains no hosted side-effect tool.
        // Reject every `can_use_tool` request before a CapabilityRequest,
        // Approval, Permit, or ExecutionTarget can be created.
        let _ = private_request_id;
        Err(AgentRuntimePortError::Unsupported {
            operation: "task-only core tool request",
        })
    }

}
