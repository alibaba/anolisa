/// Create or idempotently store an immutable Policy revision.
pub const POLICY_TEMPLATES_PUT: &str = "policy.templates.put";
/// Read a Policy revision.
pub const POLICY_TEMPLATES_GET: &str = "policy.templates.get";
/// List Policy revisions.
pub const POLICY_TEMPLATES_LIST: &str = "policy.templates.list";
/// Delete one exact Policy revision.
pub const POLICY_TEMPLATES_DELETE: &str = "policy.templates.delete";
/// Create or idempotently store an immutable Scope revision.
pub const POLICY_SCOPES_PUT: &str = "policy.scopes.put";
/// Read a Scope revision.
pub const POLICY_SCOPES_GET: &str = "policy.scopes.get";
/// List Scope revisions.
pub const POLICY_SCOPES_LIST: &str = "policy.scopes.list";
/// Delete a Scope revision.
pub const POLICY_SCOPES_DELETE: &str = "policy.scopes.delete";
/// Create or update a Binding desired revision.
pub const POLICY_BINDINGS_PUT: &str = "policy.bindings.put";
/// Read one Binding.
pub const POLICY_BINDINGS_GET: &str = "policy.bindings.get";
/// List Bindings.
pub const POLICY_BINDINGS_LIST: &str = "policy.bindings.list";
/// Request Binding removal.
pub const POLICY_BINDINGS_DELETE: &str = "policy.bindings.delete";

pub fn is_policy(method: &str) -> bool {
    matches!(
        method,
        POLICY_TEMPLATES_PUT
            | POLICY_TEMPLATES_GET
            | POLICY_TEMPLATES_LIST
            | POLICY_TEMPLATES_DELETE
            | POLICY_SCOPES_PUT
            | POLICY_SCOPES_GET
            | POLICY_SCOPES_LIST
            | POLICY_SCOPES_DELETE
            | POLICY_BINDINGS_PUT
            | POLICY_BINDINGS_GET
            | POLICY_BINDINGS_LIST
            | POLICY_BINDINGS_DELETE
    )
}
