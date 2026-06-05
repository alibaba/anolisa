#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalPanelAction {
    Approve,
    Deny,
    Details,
}

#[derive(Debug, Clone, Copy)]
pub struct ApprovalActionDescriptor {
    pub action: ApprovalPanelAction,
    pub label: &'static str,
}

pub const APPROVAL_PANEL_ACTIONS: [ApprovalActionDescriptor; 3] = [
    ApprovalActionDescriptor {
        action: ApprovalPanelAction::Approve,
        label: "Allow once",
    },
    ApprovalActionDescriptor {
        action: ApprovalPanelAction::Deny,
        label: "Deny",
    },
    ApprovalActionDescriptor {
        action: ApprovalPanelAction::Details,
        label: "Details",
    },
];

pub fn approval_action_at(index: usize) -> Option<ApprovalPanelAction> {
    APPROVAL_PANEL_ACTIONS
        .get(index)
        .map(|descriptor| descriptor.action)
}

pub fn approval_action_index(action: ApprovalPanelAction) -> Option<usize> {
    APPROVAL_PANEL_ACTIONS
        .iter()
        .position(|descriptor| descriptor.action == action)
}
