//! Authored Policy commands and bounded template-file decoding.

use std::fs::File;
use std::io::Read as _;
use std::path::PathBuf;

use asc_daemon_protocol::method::{
    POLICY_TEMPLATES_CREATE, POLICY_TEMPLATES_DELETE, POLICY_TEMPLATES_GET, POLICY_TEMPLATES_LIST,
    POLICY_TEMPLATES_UPDATE,
};
use asc_daemon_protocol::{CreatePolicyParams, DaemonRequest, RevisionParams, UpdatePolicyParams};
use asc_foundation_types::{ResourceId, Revision};
use asc_policy_types::authoring::PolicyTemplate;
use clap::{Args, Subcommand};

use super::common::{Page, encode, resource_id, revision};
use crate::InputError;

#[derive(Debug, Subcommand)]
pub(crate) enum PolicyCommand {
    /// Create a Policy with a server-generated ID.
    Create(PolicyInput),
    /// Read the exact current revision.
    Get(PolicyRevision),
    /// List one page of current Policies.
    List(Page),
    /// Replace an existing Policy's name and complete template (not upsert).
    Update {
        #[arg(long, value_parser = resource_id)]
        policy_id: ResourceId,
        #[command(flatten)]
        input: PolicyInput,
    },
    /// Delete the exact current revision.
    Delete(PolicyRevision),
}

#[derive(Debug, Args)]
pub(crate) struct PolicyInput {
    /// Complete Policy name.
    #[arg(long)]
    name: String,
    /// JSON `PolicyTemplate` file, resolved in the CLI's working directory.
    #[arg(long)]
    file: PathBuf,
}

#[derive(Debug, Args)]
pub(crate) struct PolicyRevision {
    #[arg(long, value_parser = resource_id)]
    policy_id: ResourceId,
    #[arg(long, value_parser = revision)]
    revision: Revision,
}

impl PolicyCommand {
    pub(super) fn request(&self) -> Result<DaemonRequest, InputError> {
        match self {
            Self::Create(input) => encode(
                POLICY_TEMPLATES_CREATE,
                &CreatePolicyParams {
                    policy_name: input.name.clone(),
                    template: input.template()?,
                },
            ),
            Self::Update { policy_id, input } => encode(
                POLICY_TEMPLATES_UPDATE,
                &UpdatePolicyParams {
                    policy_id: policy_id.clone(),
                    policy_name: input.name.clone(),
                    template: input.template()?,
                },
            ),
            Self::Get(input) | Self::Delete(input) => encode(
                if matches!(self, Self::Get(_)) {
                    POLICY_TEMPLATES_GET
                } else {
                    POLICY_TEMPLATES_DELETE
                },
                &RevisionParams {
                    id: input.policy_id.clone(),
                    revision: input.revision,
                },
            ),
            Self::List(page) => encode(POLICY_TEMPLATES_LIST, &page.params()),
        }
    }
}

impl PolicyInput {
    fn template(&self) -> Result<PolicyTemplate, InputError> {
        let mut bytes = Vec::new();
        File::open(&self.file)?
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > asc_daemon_client::MAX_FRAME_BYTES {
            return Err(InputError::TooLarge);
        }
        // Decode the typed template before conversion to Value, so duplicate
        // fields cannot be silently collapsed by an untyped JSON map.
        Ok(serde_json::from_slice(&bytes)?)
    }
}
