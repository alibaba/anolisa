impl AgentRuntimePort for CoshCoreBridge {
    fn binding_id(&self) -> &RuntimeBindingId {
        &self.config.identity.binding_id
    }

    fn dispatch(
        &mut self,
        command: AgentRuntimeCommand,
        deadline: Instant,
    ) -> Result<(), AgentRuntimePortError> {
        match command {
            AgentRuntimeCommand::OpenSession {
                task_id,
                run_id,
                workspace,
            } => self.open_session(task_id, run_id, workspace, deadline),
            AgentRuntimeCommand::Prompt {
                run_id,
                turn_id,
                input,
            } => self.prompt(run_id, turn_id, input, deadline),
            AgentRuntimeCommand::Cancel {
                run_id, turn_id, ..
            } => self.cancel(run_id, turn_id, deadline),
            AgentRuntimeCommand::Close { binding } => self.close(binding, deadline),
            AgentRuntimeCommand::ResumeSession { .. } => Err(AgentRuntimePortError::Unsupported {
                operation: "resume_session",
            }),
            AgentRuntimeCommand::ResolvePermission { .. } => {
                Err(AgentRuntimePortError::Unsupported {
                    operation: "resolve_permission",
                })
            }
            AgentRuntimeCommand::AcknowledgeBrokeredRequest { acknowledgement } => {
                let _ = acknowledgement;
                Err(AgentRuntimePortError::Unsupported {
                    operation: "brokered acknowledgement",
                })
            }
            AgentRuntimeCommand::DeliverBrokeredResult { delivery } => {
                let _ = delivery;
                Err(AgentRuntimePortError::Unsupported {
                    operation: "brokered result delivery",
                })
            }
            AgentRuntimeCommand::ResolveInput {
                request_id,
                run_id,
                turn_id,
                response,
            } => self.resolve_input(request_id, run_id, turn_id, response),
        }
    }

    fn next_event(
        &mut self,
        deadline: Instant,
    ) -> Result<RuntimeEventEnvelope, AgentRuntimePortError> {
        self.read_next_event(deadline)
    }
}

impl Drop for CoshCoreBridge {
    fn drop(&mut self) {
        self.shutdown_process();
    }
}
