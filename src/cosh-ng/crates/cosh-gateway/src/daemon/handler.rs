//! Transport-neutral Task request admission and dispatch.

use cosh_gateway_contracts::common::{ActorRef, RuntimeSelector, TargetRef, WorkspaceRef};
use cosh_gateway_contracts::ids::{ActorId, TaskId};

use super::{
    AppendTaskInput, CancelTask, GatewayDaemonError, GatewayRequest, GatewayResult,
    ResolveApproval, RetryTask, SubmitTask, TaskEventPage, TaskView, GATEWAY_API_VERSION,
};

/// Mutating Task operations available to the transport handler.
pub(super) trait TaskCommandPort {
    fn submit(
        &mut self,
        actor: &ActorRef,
        workspace: &WorkspaceRef,
        request: SubmitTask,
    ) -> Result<TaskView, GatewayDaemonError>;

    fn cancel(
        &mut self,
        actor_id: &ActorId,
        request: CancelTask,
    ) -> Result<TaskView, GatewayDaemonError>;

    fn retry(
        &mut self,
        actor: &ActorRef,
        target: &TargetRef,
        workspace: &WorkspaceRef,
        runtime: &RuntimeSelector,
        request: RetryTask,
    ) -> Result<TaskView, GatewayDaemonError>;

    fn resolve_approval(
        &mut self,
        actor_id: &ActorId,
        request: ResolveApproval,
    ) -> Result<TaskView, GatewayDaemonError>;

    fn append_input(
        &mut self,
        actor_id: &ActorId,
        request: AppendTaskInput,
    ) -> Result<TaskView, GatewayDaemonError>;
}

/// Read-only Task projections available to the transport handler.
pub(super) trait TaskProjectionPort {
    fn get(&self, actor_id: &ActorId, task_id: &TaskId) -> Result<TaskView, GatewayDaemonError>;

    fn events(
        &self,
        actor_id: &ActorId,
        task_id: &TaskId,
        after_revision: Option<u64>,
        limit: u16,
    ) -> Result<TaskEventPage, GatewayDaemonError>;
}

/// Trusted admission values selected before request dispatch.
pub(super) struct TaskAdmission<'a> {
    pub(super) target: &'a TargetRef,
    pub(super) workspace: &'a WorkspaceRef,
    pub(super) runtime: &'a RuntimeSelector,
}

/// Dispatches one authenticated request through Task command and projection ports.
pub(super) fn dispatch<P>(
    actor: &ActorRef,
    request: GatewayRequest,
    admission: TaskAdmission<'_>,
    ports: &mut P,
) -> Result<GatewayResult, GatewayDaemonError>
where
    P: TaskCommandPort + TaskProjectionPort,
{
    if request.api_version() != GATEWAY_API_VERSION {
        return Err(GatewayDaemonError::Protocol(
            "unsupported Gateway API version".to_owned(),
        ));
    }
    match request {
        GatewayRequest::Ping { .. } => Ok(GatewayResult::Pong),
        GatewayRequest::Submit { request, .. } => {
            validate_submission_admission(&request, admission.target, admission.runtime)?;
            ports
                .submit(actor, admission.workspace, request)
                .map(GatewayResult::Task)
        }
        GatewayRequest::Get { task_id, .. } => ports
            .get(&actor.actor_id, &task_id)
            .map(GatewayResult::Task),
        GatewayRequest::Events {
            task_id,
            after_revision,
            limit,
            ..
        } => ports
            .events(&actor.actor_id, &task_id, after_revision, limit)
            .map(GatewayResult::Events),
        GatewayRequest::Cancel { request, .. } => ports
            .cancel(&actor.actor_id, request)
            .map(GatewayResult::Cancelled),
        GatewayRequest::Retry { request, .. } => ports
            .retry(
                actor,
                admission.target,
                admission.workspace,
                admission.runtime,
                request,
            )
            .map(GatewayResult::Retried),
        GatewayRequest::ResolveApproval { request, .. } => ports
            .resolve_approval(&actor.actor_id, request)
            .map(GatewayResult::ApprovalResolved),
        GatewayRequest::AppendInput { request, .. } => ports
            .append_input(&actor.actor_id, request)
            .map(GatewayResult::InputAppended),
    }
}

pub(super) fn validate_submission_admission(
    request: &SubmitTask,
    admitted_target: &TargetRef,
    admitted_runtime: &RuntimeSelector,
) -> Result<(), GatewayDaemonError> {
    if request.target != *admitted_target || request.runtime != *admitted_runtime {
        return Err(GatewayDaemonError::Protocol(
            "Task target or Runtime is not admitted by this daemon".to_owned(),
        ));
    }
    Ok(())
}
