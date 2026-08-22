//! Side-effect-fenced checkpoint operations for protocol V2.

use std::path::{Component, Path};
use std::sync::Arc;

use ws_ckpt_common::{
    validate_checkpoint_id_v2, validate_workspace_id_v2, ErrorCode, GuardedCheckpointEvidenceV2,
    GuardedCheckpointOutcomeV2, GuardedCheckpointRejectionCodeV2, Response, SnapshotMeta,
    WorkspaceGenerationTokenV2, GUARDED_CHECKPOINT_EVIDENCE_LIMIT_V2,
    GUARDED_CHECKPOINT_PROTOCOL_VERSION_V2, LIVE_CHILD,
};

use crate::state::DaemonState;

pub(crate) async fn workspace_identity(
    state: &Arc<DaemonState>,
    registration_path: &str,
) -> Response {
    let path = Path::new(registration_path);
    if let Err(message) = validate_registration_path(registration_path) {
        return rejected(
            GuardedCheckpointRejectionCodeV2::InvalidRegistrationPath,
            message,
        );
    }

    // This is deliberately an exact map lookup. Identity discovery must not
    // canonicalize, initialize, adopt, or repair caller-supplied paths.
    let Some(candidate) = state.wsid_for_exact_registration_path(path) else {
        return workspace_not_found(registration_path);
    };
    if let Err(message) = validate_workspace_id_v2(&candidate) {
        return rejected(
            GuardedCheckpointRejectionCodeV2::InvalidWorkspaceId,
            message,
        );
    }
    let _wsid_guard = state.lock_wsid(&candidate).await;
    let Some(workspace) = state.get_by_wsid(&candidate) else {
        return workspace_not_found(registration_path);
    };
    if !state.exact_registration_is_current(path, &candidate, &workspace) {
        return workspace_not_found(registration_path);
    }
    if !registration_resolves_to_live(state, path, &candidate).await {
        return workspace_not_found(registration_path);
    }

    let registered_path = {
        let workspace = workspace.read().await;
        if workspace.path.to_str() != Some(registration_path) {
            return workspace_not_found(registration_path);
        }
        workspace.path.to_string_lossy().into_owned()
    };
    let generation = match state.backend.live_generation(&candidate).await {
        Ok(generation) => generation,
        Err(error) => {
            return rejected(
                GuardedCheckpointRejectionCodeV2::DaemonNotReady,
                format!("failed to read live workspace generation: {error:#}"),
            )
        }
    };

    Response::WorkspaceIdentityV2Ok {
        protocol_version: GUARDED_CHECKPOINT_PROTOCOL_VERSION_V2,
        ws_id: candidate,
        registered_path,
        generation,
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn checkpoint(
    state: &Arc<DaemonState>,
    caller_uid: Option<u32>,
    ws_id: &str,
    expected_generation: WorkspaceGenerationTokenV2,
    checkpoint_id: &str,
    operation_digest: [u8; 32],
    message: Option<String>,
    metadata: Option<String>,
    pin: bool,
) -> Response {
    let caller_uid = match caller_uid {
        Some(uid) => uid,
        None => {
            return rejected(
                GuardedCheckpointRejectionCodeV2::PeerCredentialsUnavailable,
                "guarded checkpoint requires kernel peer credentials",
            )
        }
    };
    if let Err(message) = validate_workspace_id_v2(ws_id) {
        return rejected(
            GuardedCheckpointRejectionCodeV2::InvalidWorkspaceId,
            message,
        );
    }
    if let Err(message) = validate_checkpoint_id_v2(checkpoint_id) {
        return rejected(
            GuardedCheckpointRejectionCodeV2::InvalidCheckpointId,
            message,
        );
    }
    let parsed_metadata = match metadata {
        Some(value) => match serde_json::from_str(&value) {
            Ok(value) => Some(value),
            Err(error) => {
                return rejected(
                    GuardedCheckpointRejectionCodeV2::InvalidMetadata,
                    format!("metadata is not valid JSON: {error}"),
                )
            }
        },
        None => None,
    };

    let _wsid_guard = state.lock_wsid(ws_id).await;
    let Some(workspace) = state.get_by_wsid(ws_id) else {
        return workspace_not_found(ws_id);
    };
    let mut workspace = workspace.write().await;
    if workspace.ws_id != ws_id {
        return workspace_not_found(ws_id);
    }
    if !registration_resolves_to_live(state, &workspace.path, ws_id).await {
        return rejected(
            GuardedCheckpointRejectionCodeV2::InvalidRegistrationPath,
            "registered workspace path no longer resolves to the live subvolume",
        );
    }

    let generation = match state.backend.live_generation(ws_id).await {
        Ok(generation) => generation,
        Err(error) => {
            return rejected(
                GuardedCheckpointRejectionCodeV2::DaemonNotReady,
                format!("failed to read live workspace generation: {error:#}"),
            )
        }
    };
    if generation != expected_generation {
        return rejected(
            GuardedCheckpointRejectionCodeV2::GenerationMismatch,
            "workspace generation no longer matches the guarded request",
        );
    }

    if let Some(evidence) = workspace.index.governed_evidence.get(checkpoint_id) {
        if evidence_matches(
            evidence,
            ws_id,
            expected_generation,
            checkpoint_id,
            operation_digest,
            caller_uid,
        ) && evidence_is_visible(&workspace.index, evidence)
        {
            return Response::GuardedCheckpointV2Ok {
                evidence: evidence.clone(),
            };
        }
        return rejected(
            GuardedCheckpointRejectionCodeV2::OperationConflict,
            "checkpoint id is already bound to a different guarded operation",
        );
    }
    if workspace.index.snapshots.contains_key(checkpoint_id) {
        return rejected(
            GuardedCheckpointRejectionCodeV2::OperationConflict,
            "checkpoint id already exists without matching guarded evidence",
        );
    }

    let mut next_index = match index_with_evidence_slot(&workspace.index) {
        Some(index) => index,
        None => {
            return rejected(
                GuardedCheckpointRejectionCodeV2::EvidenceCapacityReached,
                "guarded evidence capacity is occupied by visible snapshots",
            )
        }
    };

    if !state.check_workspace_quiescent(ws_id).await {
        return rejected(
            GuardedCheckpointRejectionCodeV2::WriteLockConflict,
            "workspace has active write operations; retry after it becomes quiescent",
        );
    }

    // The registered path is user-replaceable. Inspect the same internal live
    // subvolume that `create_snapshot(ws_id, ..)` will operate on.
    let live_path = state.backend.data_root().join(ws_id);
    let is_empty = match tokio::fs::read_dir(&live_path).await {
        Ok(mut entries) => match entries.next_entry().await {
            Ok(entry) => entry.is_none(),
            Err(error) => {
                return rejected(
                    GuardedCheckpointRejectionCodeV2::DaemonNotReady,
                    format!("failed to inspect workspace contents: {error}"),
                )
            }
        },
        Err(error) => {
            return rejected(
                GuardedCheckpointRejectionCodeV2::DaemonNotReady,
                format!("failed to inspect workspace contents: {error}"),
            )
        }
    };

    let Some(registered_path) = workspace.path.to_str().map(str::to_owned) else {
        return rejected(
            GuardedCheckpointRejectionCodeV2::InvalidRegistrationPath,
            "registered workspace path is not valid UTF-8",
        );
    };
    if is_empty {
        let reason = "Empty workspace, no snapshot created.".to_string();
        let evidence = evidence(
            ws_id,
            registered_path,
            expected_generation,
            checkpoint_id,
            operation_digest,
            caller_uid,
            GuardedCheckpointOutcomeV2::Skipped {
                reason: reason.clone(),
            },
        );
        next_index
            .governed_evidence
            .insert(checkpoint_id.to_string(), evidence.clone());
        if let Err(error) =
            crate::index_store::save_durable(&state.index_dir(ws_id), &next_index).await
        {
            return rejected(
                GuardedCheckpointRejectionCodeV2::DaemonNotReady,
                format!("failed to durably save skipped checkpoint evidence: {error:#}"),
            );
        }
        workspace.index = next_index;
        return Response::GuardedCheckpointV2Ok { evidence };
    }

    if let Err(error) = state.backend.create_snapshot(ws_id, checkpoint_id).await {
        return backend_effect_error(format!(
            "guarded checkpoint backend operation failed; reconcile before retrying: {error:#}"
        ));
    }

    let evidence = evidence(
        ws_id,
        registered_path,
        expected_generation,
        checkpoint_id,
        operation_digest,
        caller_uid,
        GuardedCheckpointOutcomeV2::Created {
            snapshot_id: checkpoint_id.to_string(),
        },
    );
    if let Some(old_head) = next_index.head.clone() {
        if let Some(head) = next_index.snapshots.get_mut(&old_head) {
            head.child_ids.retain(|child| child != LIVE_CHILD);
            head.child_ids.push(checkpoint_id.to_string());
        }
    }
    next_index.snapshots.insert(
        checkpoint_id.to_string(),
        SnapshotMeta {
            message,
            metadata: parsed_metadata,
            pinned: pin,
            created_at: chrono::Utc::now(),
            missing: false,
            parent_id: next_index.head.clone(),
            child_ids: vec![LIVE_CHILD.to_string()],
        },
    );
    next_index.head = Some(checkpoint_id.to_string());
    next_index
        .governed_evidence
        .insert(checkpoint_id.to_string(), evidence.clone());

    if let Err(error) = crate::index_store::save_durable(&state.index_dir(ws_id), &next_index).await
    {
        return backend_effect_error(format!(
            "snapshot was created but durable evidence save failed; reconcile before retrying: {error:#}"
        ));
    }
    workspace.index = next_index;
    Response::GuardedCheckpointV2Ok { evidence }
}

pub(crate) async fn checkpoint_evidence(
    state: &Arc<DaemonState>,
    caller_uid: Option<u32>,
    ws_id: &str,
    expected_generation: WorkspaceGenerationTokenV2,
    checkpoint_id: &str,
    operation_digest: [u8; 32],
) -> Response {
    let caller_uid = match caller_uid {
        Some(uid) => uid,
        None => {
            return rejected(
                GuardedCheckpointRejectionCodeV2::PeerCredentialsUnavailable,
                "checkpoint evidence requires kernel peer credentials",
            )
        }
    };
    if let Err(message) = validate_workspace_id_v2(ws_id) {
        return rejected(
            GuardedCheckpointRejectionCodeV2::InvalidWorkspaceId,
            message,
        );
    }
    if let Err(message) = validate_checkpoint_id_v2(checkpoint_id) {
        return rejected(
            GuardedCheckpointRejectionCodeV2::InvalidCheckpointId,
            message,
        );
    }

    let _wsid_guard = state.lock_wsid(ws_id).await;
    let Some(workspace) = state.get_by_wsid(ws_id) else {
        return workspace_not_found(ws_id);
    };
    let workspace = workspace.read().await;
    let Some(evidence) = workspace.index.governed_evidence.get(checkpoint_id) else {
        return Response::CheckpointEvidenceV2Ok { evidence: None };
    };
    if evidence.caller_uid != caller_uid {
        return rejected(
            GuardedCheckpointRejectionCodeV2::CallerMismatch,
            "stored checkpoint evidence belongs to a different caller",
        );
    }
    if !evidence_matches(
        evidence,
        ws_id,
        expected_generation,
        checkpoint_id,
        operation_digest,
        caller_uid,
    ) {
        return rejected(
            GuardedCheckpointRejectionCodeV2::OperationConflict,
            "stored checkpoint evidence does not match the requested operation",
        );
    }

    Response::CheckpointEvidenceV2Ok {
        evidence: evidence_is_visible(&workspace.index, evidence).then(|| evidence.clone()),
    }
}

fn validate_registration_path(value: &str) -> Result<(), String> {
    let path = Path::new(value);
    if value.is_empty() || !path.is_absolute() || value.as_bytes().contains(&0) {
        return Err("registration path must be a non-empty absolute path".to_string());
    }
    if path
        .components()
        .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err("registration path must not contain '.' or '..' components".to_string());
    }
    Ok(())
}

async fn registration_resolves_to_live(
    state: &DaemonState,
    registration_path: &Path,
    ws_id: &str,
) -> bool {
    let live_path = state.backend.data_root().join(ws_id);
    match tokio::try_join!(
        tokio::fs::canonicalize(registration_path),
        tokio::fs::canonicalize(live_path)
    ) {
        Ok((registration_target, live_target)) => registration_target == live_target,
        Err(_) => false,
    }
}

fn index_with_evidence_slot(
    index: &ws_ckpt_common::SnapshotIndex,
) -> Option<ws_ckpt_common::SnapshotIndex> {
    let mut next = index.clone();
    while next.governed_evidence.len() >= GUARDED_CHECKPOINT_EVIDENCE_LIMIT_V2 {
        let evict = next
            .governed_evidence
            .iter()
            .find(|(_, evidence)| {
                matches!(
                    &evidence.outcome,
                    GuardedCheckpointOutcomeV2::Skipped { .. }
                ) || created_evidence_is_marked_missing(&next, evidence)
            })
            .map(|(checkpoint_id, _)| checkpoint_id.clone())?;
        next.governed_evidence.remove(&evict);
    }
    Some(next)
}

fn created_evidence_is_marked_missing(
    index: &ws_ckpt_common::SnapshotIndex,
    evidence: &GuardedCheckpointEvidenceV2,
) -> bool {
    match &evidence.outcome {
        GuardedCheckpointOutcomeV2::Created { snapshot_id }
            if snapshot_id == &evidence.checkpoint_id =>
        {
            index
                .snapshots
                .get(snapshot_id)
                .is_some_and(|snapshot| snapshot.missing)
        }
        _ => false,
    }
}

fn evidence(
    ws_id: &str,
    registered_path: String,
    generation: WorkspaceGenerationTokenV2,
    checkpoint_id: &str,
    operation_digest: [u8; 32],
    caller_uid: u32,
    outcome: GuardedCheckpointOutcomeV2,
) -> GuardedCheckpointEvidenceV2 {
    GuardedCheckpointEvidenceV2 {
        ws_id: ws_id.to_string(),
        registered_path,
        generation,
        checkpoint_id: checkpoint_id.to_string(),
        operation_digest,
        caller_uid,
        outcome,
    }
}

fn evidence_matches(
    evidence: &GuardedCheckpointEvidenceV2,
    ws_id: &str,
    generation: WorkspaceGenerationTokenV2,
    checkpoint_id: &str,
    operation_digest: [u8; 32],
    caller_uid: u32,
) -> bool {
    evidence.ws_id == ws_id
        && evidence.generation == generation
        && evidence.checkpoint_id == checkpoint_id
        && evidence.operation_digest == operation_digest
        && evidence.caller_uid == caller_uid
}

fn evidence_is_visible(
    index: &ws_ckpt_common::SnapshotIndex,
    evidence: &GuardedCheckpointEvidenceV2,
) -> bool {
    match &evidence.outcome {
        GuardedCheckpointOutcomeV2::Created { snapshot_id }
            if snapshot_id == &evidence.checkpoint_id =>
        {
            index
                .snapshots
                .get(&evidence.checkpoint_id)
                .is_some_and(|snapshot| !snapshot.missing)
        }
        GuardedCheckpointOutcomeV2::Created { .. } => false,
        GuardedCheckpointOutcomeV2::Skipped { .. } => true,
    }
}

fn workspace_not_found(workspace: &str) -> Response {
    rejected(
        GuardedCheckpointRejectionCodeV2::WorkspaceNotFound,
        format!("workspace is not registered: {workspace}"),
    )
}

fn rejected(code: GuardedCheckpointRejectionCodeV2, message: impl Into<String>) -> Response {
    Response::GuardedCheckpointV2Rejected {
        code,
        message: message.into(),
    }
}

fn backend_effect_error(message: String) -> Response {
    Response::Error {
        code: ErrorCode::InternalError,
        message,
    }
}

#[cfg(test)]
mod tests;
