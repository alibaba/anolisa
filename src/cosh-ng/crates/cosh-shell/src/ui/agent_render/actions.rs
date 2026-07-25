#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalPanelAction {
    Approve,
    ApproveTurn,
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

/// Actions when turn-scope batch consent is offered (issue #1773): the
/// multi-request turn adds "Allow all this turn" next to the single-shot
/// approval while keeping AlwaysTrust visible.
pub const TURN_APPROVAL_PANEL_ACTIONS: [ApprovalActionDescriptor; 5] = [
    ApprovalActionDescriptor {
        action: ApprovalPanelAction::Approve,
    },
    ApprovalActionDescriptor {
        action: ApprovalPanelAction::ApproveTurn,
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

/// Which action list an approval card offers. Resolved once per request
/// (single source of truth) and shared by rendering, raw-input capture,
/// and focus parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalActionSet {
    Hook,
    Standard,
    TurnConsent,
}

impl ApprovalActionSet {
    pub fn descriptors(self) -> &'static [ApprovalActionDescriptor] {
        match self {
            Self::Hook => &HOOK_APPROVAL_PANEL_ACTIONS,
            Self::Standard => &APPROVAL_PANEL_ACTIONS,
            Self::TurnConsent => &TURN_APPROVAL_PANEL_ACTIONS,
        }
    }
    pub fn action_at(self, index: usize) -> Option<ApprovalPanelAction> {
        self.descriptors()
            .get(index)
            .map(|descriptor| descriptor.action)
    }
    pub fn action_index(self, action: ApprovalPanelAction) -> Option<usize> {
        self.descriptors()
            .iter()
            .position(|descriptor| descriptor.action == action)
    }
    pub fn max_index(self) -> usize {
        self.descriptors().len().saturating_sub(1)
    }
}

/// Greedy row packing for approval action labels: each item renders as
/// `  [ label ] ` (7 columns of fixed overhead) with a 2-column separator
/// between items. Items that no longer fit wrap to the next row; a single
/// over-wide item still gets its own row (fail-safe, never truncated).
/// Callers pre-wrap — `Paragraph::wrap` is never used for action rows.
pub fn pack_action_rows(label_widths: &[usize], content_width: usize) -> Vec<Vec<usize>> {
    const ITEM_OVERHEAD: usize = 7; // "  [ " + " ] "
    const SEPARATOR: usize = 2;
    let mut rows: Vec<Vec<usize>> = Vec::new();
    let mut row: Vec<usize> = Vec::new();
    let mut row_width = 0usize;
    for (index, label_width) in label_widths.iter().enumerate() {
        let item_width = label_width + ITEM_OVERHEAD;
        let needed = if row.is_empty() {
            item_width
        } else {
            row_width + SEPARATOR + item_width
        };
        if !row.is_empty() && needed > content_width {
            rows.push(std::mem::take(&mut row));
            row_width = 0;
        }
        row_width = if row.is_empty() {
            item_width
        } else {
            row_width + SEPARATOR + item_width
        };
        row.push(index);
    }
    if !row.is_empty() {
        rows.push(row);
    }
    rows
}

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
