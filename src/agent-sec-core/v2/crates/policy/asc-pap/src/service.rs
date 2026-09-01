use std::sync::Arc;

use asc_foundation_types::{ResourceId, Revision};
use asc_policy_engine::{PolicyTemplate, TemplateEnvelope, lower_template};
use asc_policy_types::identifiers::{PolicyId, Revision as PolicyRevision};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use crate::error::PapError;
use crate::model::{PreparedPolicy, PreparedScope};
use crate::repository::PapRepository;
use crate::scope::{ScopeSelector, ScopeTemplate};

/// PAP application service.
pub struct PapService<R> {
    repository: Arc<R>,
}

impl<R> Clone for PapService<R> {
    fn clone(&self) -> Self {
        Self {
            repository: Arc::clone(&self.repository),
        }
    }
}

impl<R: PapRepository> PapService<R> {
    /// Creates the service around one process-owned repository.
    pub fn new(repository: Arc<R>) -> Self {
        Self { repository }
    }

    /// Converges one Policy identity to the complete desired template.
    ///
    /// The caller does not allocate revisions. Identical latest content is a
    /// no-op; changed content receives the next never-reused immutable revision.
    /// Concurrent insert conflicts are retried inside PAP.
    ///
    /// # Errors
    /// Returns validation, lowering, conflict, or persistence errors.
    pub fn put_policy(
        &self,
        policy_id: Option<&ResourceId>,
        policy_name: &str,
        template: &PolicyTemplate,
    ) -> Result<PreparedPolicy, PapError> {
        validate_policy_name(policy_name)?;
        let template_digest = json_digest(template)?;
        let update_existing = policy_id.is_some();
        let mut selected_id = match policy_id {
            Some(id) => {
                validate_policy_id(id)?;
                id.clone()
            }
            None => generated_policy_id()?,
        };

        loop {
            let state = self.repository.get_policy_revision_state(&selected_id)?;
            if update_existing && state.is_none() {
                return Err(PapError::NotFound);
            }
            if !update_existing && state.is_some() {
                selected_id = generated_policy_id()?;
            } else {
                if let Some(current) = state.as_ref().and_then(|state| state.latest.as_ref())
                    && current.policy_name == policy_name
                    && current.template_digest == template_digest
                    && &current.template == template
                {
                    return Ok(current.clone());
                }

                let revision_value = state.as_ref().map_or(1, |state| {
                    state
                        .last_allocated_revision
                        .get()
                        .checked_add(1)
                        .unwrap_or(0)
                });
                let revision = Revision::new(revision_value).map_err(|_| PapError::Conflict)?;
                let engine_revision = PolicyRevision::new(revision.get())
                    .map_err(|message| PapError::InvalidIdentifier(message.to_owned()))?;
                let engine_policy_id =
                    PolicyId::new(selected_id.as_str()).map_err(PapError::InvalidIdentifier)?;
                let canonical_policy = lower_template(TemplateEnvelope {
                    policy_id: engine_policy_id,
                    revision: engine_revision,
                    template: template.clone(),
                })?;
                let candidate = PreparedPolicy {
                    policy_id: selected_id.clone(),
                    policy_name: policy_name.to_owned(),
                    revision,
                    template: template.clone(),
                    canonical_policy,
                    template_digest: template_digest.clone(),
                };

                match self.repository.put_policy(&candidate) {
                    Err(PapError::Conflict) if !update_existing => {
                        selected_id = generated_policy_id()?;
                    }
                    Err(PapError::Conflict) => {}
                    result => return result,
                }
            }
        }
    }

    /// Converges one Scope identity to a simple selector intent.
    ///
    /// # Errors
    /// Returns validation, conflict, or persistence errors.
    pub fn put_scope(
        &self,
        scope_id: Option<&ResourceId>,
        selector: &ScopeSelector,
    ) -> Result<PreparedScope, PapError> {
        selector.validate()?;
        let template = ScopeTemplate::execution_domain_default();
        template.validate()?;
        let template_digest = json_digest(&(selector, &template))?;
        let update_existing = scope_id.is_some();
        let mut selected_id = match scope_id {
            Some(id) => {
                validate_uuid(id, "scope id must be a UUID")?;
                id.clone()
            }
            None => generated_resource_id()?,
        };

        loop {
            let state = self.repository.get_scope_revision_state(&selected_id)?;
            if update_existing && state.is_none() {
                return Err(PapError::NotFound);
            }
            if !update_existing && state.is_some() {
                selected_id = generated_resource_id()?;
                continue;
            }
            if let Some(current) = state.as_ref().and_then(|state| state.latest.as_ref())
                && &current.selector == selector
                && current.template_digest == template_digest
                && current.template == template
            {
                return Ok(current.clone());
            }
            let revision_value = state.as_ref().map_or(1, |state| {
                state
                    .last_allocated_revision
                    .get()
                    .checked_add(1)
                    .unwrap_or(0)
            });
            let revision = Revision::new(revision_value).map_err(|_| PapError::Conflict)?;
            let candidate = PreparedScope {
                scope_id: selected_id.clone(),
                revision,
                selector: selector.clone(),
                template: template.clone(),
                template_digest: template_digest.clone(),
            };
            match self.repository.put_scope(&candidate) {
                Err(PapError::Conflict) if !update_existing => {
                    selected_id = generated_resource_id()?;
                }
                Err(PapError::Conflict) => {}
                result => return result,
            }
        }
    }

    /// Returns the repository for read and revision-lifecycle use cases.
    pub fn repository(&self) -> &R {
        &self.repository
    }
}

fn validate_policy_name(value: &str) -> Result<(), PapError> {
    if value.trim().is_empty() {
        return Err(PapError::InvalidPolicyName(
            "must contain a visible character".to_owned(),
        ));
    }
    if value.len() > 256 {
        return Err(PapError::InvalidPolicyName(
            "must not exceed 256 bytes".to_owned(),
        ));
    }
    if value.chars().any(char::is_control) {
        return Err(PapError::InvalidPolicyName(
            "must not contain control characters".to_owned(),
        ));
    }
    Ok(())
}

fn validate_policy_id(value: &ResourceId) -> Result<(), PapError> {
    validate_uuid(value, "policy id must be a UUID")
}

fn generated_policy_id() -> Result<ResourceId, PapError> {
    generated_resource_id()
}

fn validate_uuid(value: &ResourceId, message: &str) -> Result<(), PapError> {
    Uuid::parse_str(value.as_str())
        .map(|_| ())
        .map_err(|_| PapError::InvalidIdentifier(message.to_owned()))
}

fn generated_resource_id() -> Result<ResourceId, PapError> {
    ResourceId::new(Uuid::new_v4().to_string())
        .map_err(|error| PapError::InvalidIdentifier(error.to_string()))
}

fn json_digest<T: Serialize>(value: &T) -> Result<String, PapError> {
    let bytes = serde_json::to_vec(value).map_err(PapError::Serialization)?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}
