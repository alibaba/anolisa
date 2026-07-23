//! Durable Shell-hosted boundaries for Core Tool execution.

use super::{ShellApprovalAuditInput, ShellAuditRecorder};
use crate::types::audit::{
    AuditEventOutcome, AuditEventV1, AuditIdentity, AuditMode, AuditOutcomeStatus, AuditSubject,
    AuditToolData,
};

impl ShellAuditRecorder {
    /// Durably authorizes a Core Tool whose side effect is hosted by Shell.
    pub(crate) fn authorize_host_execution(
        &mut self,
        request: ShellApprovalAuditInput<'_>,
    ) -> Result<(), String> {
        if self.owned_approvals.contains(request.id) {
            // Shell-owned approvals were made durable by record_approval_resolved.
            // They have no provider Tool identity and must not acquire a second
            // semantic execution boundary before the native command lifecycle.
            return Ok(());
        }
        let tool_use_id = request
            .tool_use_id
            .ok_or_else(|| "host execution has no Tool identity".to_string())?;
        let tool = AuditEventV1::shell(
            "tool.execution.started",
            AuditIdentity {
                shell_session_id: Some(request.session_id.to_string()),
                run_id: Some(request.run_id.to_string()),
                request_id: request.request_id.map(str::to_string),
                tool_use_id: Some(tool_use_id.to_string()),
                ..AuditIdentity::default()
            },
            AuditEventOutcome {
                status: AuditOutcomeStatus::Started,
                code: None,
                retryable: false,
            },
            AuditSubject {
                kind: "tool".to_string(),
                name: Some(request.subject.to_string()),
            },
            &AuditToolData {
                tool_kind: request.subject.to_string(),
                execution_path: Some("shell_foreground_handoff".to_string()),
                ..AuditToolData::default()
            },
        )?;
        self.append_governed(tool)
    }

    fn append_governed(&mut self, event: AuditEventV1) -> Result<(), String> {
        if self.append(event, true) || self.mode == AuditMode::BestEffort {
            Ok(())
        } else {
            Err("AUDIT_REQUIRED_UNAVAILABLE: host execution was blocked before start".to_string())
        }
    }
}
