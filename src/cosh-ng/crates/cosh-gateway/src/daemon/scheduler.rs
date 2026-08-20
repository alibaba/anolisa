//! Durable single-Run scheduler built on Outbox and Run-lease fencing.

mod brokered;
#[cfg(test)]
mod input_tests;

pub use brokered::{
    BrokeredApprovalContext, BrokeredApprovalPlan, BrokeredExecutionDriver, BrokeredResolution,
    BrokeredResolutionContext, BrokeredResolutionSource,
};
use brokered::{PendingBrokered, RejectingBrokeredExecutionDriver};

use std::path::Path;
use std::time::Duration;

use cosh_gateway_contracts::common::{
    ActorRef, BoundedName, BoundedOpaque, BoundedText, Digest, IdempotencyKey, RuntimeBindingRef,
    RuntimeSelector, TargetRef, WorkspaceRef,
};
use cosh_gateway_contracts::error::{ContractError, ErrorCategory};
use cosh_gateway_contracts::ids::{ActorId, InputRequestId, InstallationId, RunId, TaskId};
use cosh_gateway_contracts::profile::{GatewayCapabilityProfile, GatewayCapabilityProfileIdentity};
use cosh_gateway_contracts::task::{
    CancelReason, CancellationStage, RuntimeUpdate, TaskEvent, TaskState,
};
use cosh_gateway_contracts::{
    capability::{
        ApprovalDecision, ApprovalRequest, BrokeredOperation, CapabilityRequest, DenialCode,
        RuntimeExecutionFence,
    },
    ids::{ApprovalId, ExecutionId},
    runtime::{
        BrokeredExecutionDelivery, BrokeredExecutionRef, BrokeredRequestAcknowledgement,
        RuntimeInputRequest, RuntimeInputResponse, RuntimePermissionDecision, RuntimePermissionRef,
        ToolSummary,
    },
};
use serde::{Deserialize, Serialize};

use crate::capability::DurableApprovalCoordinator;
use crate::storage::{
    ApprovalRecord, ApprovalState, BrokeredRequestRecord, BrokeredRuntimeDispatchKind,
    BrokeredRuntimeDispatchRecord, BrokeredRuntimeDispatchState, LeaseClaim, LeaseCommand,
    LedgerCommand, LedgerOutcome, OutboxClaim, ProviderPermissionDispatchDecision,
    ProviderPermissionDispatchState, RuntimeInputDispatchRecord, RuntimeInputDispatchState,
    RuntimeInputRequestRecord, RuntimeInputRequestState, SqliteTaskStore, StoreError, TaskCommit,
};

use super::{digest_json, GatewayDaemonError, TaskCoordinator, TaskView};

// Scheduler phases remain one private state-machine namespace so the file
// split does not turn transition helpers into a wider internal API.
include!("scheduler/model.rs");
include!("scheduler/lifecycle.rs");
include!("scheduler/tick.rs");
include!("scheduler/approval.rs");
include!("scheduler/input.rs");
include!("scheduler/poll.rs");
include!("scheduler/settlement.rs");
include!("scheduler/coordinator_recovery.rs");
include!("scheduler/coordinator_events.rs");
include!("scheduler/support.rs");

#[cfg(test)]
mod tests;
