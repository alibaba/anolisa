#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalPanelAction {
    Approve,
    AlwaysTrust,
    Deny,
    Details,
}

#[derive(Debug, Clone, Copy)]
pub struct ApprovalActionDescriptor {
    pub action: ApprovalPanelAction,
}

pub const APPROVAL_PANEL_ACTIONS: [ApprovalActionDescriptor; 4] = [
    ApprovalActionDescriptor {
        action: ApprovalPanelAction::Approve,
    },
    ApprovalActionDescriptor {
        action: ApprovalPanelAction::AlwaysTrust,
    },
    ApprovalActionDescriptor {
        action: ApprovalPanelAction::Deny,
    },
    ApprovalActionDescriptor {
        action: ApprovalPanelAction::Details,
    },
];

/// Actions for hook approval panels (excludes AlwaysTrust).
pub const HOOK_APPROVAL_PANEL_ACTIONS: [ApprovalActionDescriptor; 3] = [
    ApprovalActionDescriptor {
        action: ApprovalPanelAction::Approve,
    },
    ApprovalActionDescriptor {
        action: ApprovalPanelAction::Deny,
    },
    ApprovalActionDescriptor {
        action: ApprovalPanelAction::Details,
    },
];

pub fn approval_action_at(index: usize) -> Option<ApprovalPanelAction> {
    APPROVAL_PANEL_ACTIONS
        .get(index)
        .map(|descriptor| descriptor.action)
}

/// Look up action by index in the hook-specific action list (no AlwaysTrust).
pub fn hook_approval_action_at(index: usize) -> Option<ApprovalPanelAction> {
    HOOK_APPROVAL_PANEL_ACTIONS
        .get(index)
        .map(|descriptor| descriptor.action)
}

pub fn approval_action_index(action: ApprovalPanelAction) -> Option<usize> {
    APPROVAL_PANEL_ACTIONS
        .iter()
        .position(|descriptor| descriptor.action == action)
}

/// Max selectable index for hook approval panels.
pub fn hook_approval_action_max_index() -> usize {
    HOOK_APPROVAL_PANEL_ACTIONS.len().saturating_sub(1)
}
