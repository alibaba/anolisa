use std::collections::VecDeque;
use std::time::{Duration, Instant};

use cosh_shell::types::ShellHandoffRequest;

#[derive(Debug, Default)]
pub(crate) struct ShellHandoffState {
    approved: VecDeque<PendingApprovedShellHandoff>,
    pending: VecDeque<PendingApprovedShellHandoff>,
}

impl ShellHandoffState {
    pub(crate) fn enqueue_approved_request(&mut self, request: ShellHandoffRequest) {
        self.approved.push_back(PendingApprovedShellHandoff {
            request,
            emitted_at: None,
            timeout_interrupt_sent: false,
        });
    }

    pub(crate) fn emit_next_approved(&mut self) -> Option<ShellHandoffRequest> {
        let mut handoff = self.approved.pop_front()?;
        handoff.emitted_at = Some(Instant::now());
        handoff.timeout_interrupt_sent = false;
        let request = handoff.request.clone();
        self.pending.push_back(handoff);
        Some(request)
    }

    pub(crate) fn pending_front(&self) -> Option<&PendingApprovedShellHandoff> {
        self.pending.front()
    }

    pub(crate) fn pop_pending(&mut self) -> Option<PendingApprovedShellHandoff> {
        self.pending.pop_front()
    }

    pub(crate) fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub(crate) fn has_active_handoff(&self) -> bool {
        !self.approved.is_empty() || !self.pending.is_empty()
    }

    pub(crate) fn mark_timeout_interrupt_if_elapsed(&mut self, timeout: Duration) -> bool {
        let Some(handoff) = self.pending.front_mut() else {
            return false;
        };
        let Some(emitted_at) = handoff.emitted_at else {
            return false;
        };
        if handoff.timeout_interrupt_sent || emitted_at.elapsed() < timeout {
            return false;
        }

        handoff.timeout_interrupt_sent = true;
        true
    }

    #[cfg(test)]
    pub(crate) fn approved_is_empty(&self) -> bool {
        self.approved.is_empty()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PendingApprovedShellHandoff {
    request: ShellHandoffRequest,
    emitted_at: Option<Instant>,
    timeout_interrupt_sent: bool,
}

impl PendingApprovedShellHandoff {
    pub(crate) fn request(&self) -> &ShellHandoffRequest {
        &self.request
    }

    pub(crate) fn timeout_interrupt_sent(&self) -> bool {
        self.timeout_interrupt_sent
    }
}
