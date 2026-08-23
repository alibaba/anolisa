use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::thread;

use cosh_gateway::capability::{ExecutionTarget, ExecutionTargetOutcome};
use cosh_gateway_contracts::capability::RuntimeExecutionFence;
use cosh_gateway_contracts::common::{BoundedName, BoundedOpaque, Digest, TargetRef};
use cosh_gateway_contracts::ids::RuntimeBindingId;
use cosh_gateway_contracts::runtime::{
    BrokeredOperationResult, WorkspaceCheckpointCreateV1Outcome,
};
use cosh_types::checkpoint::{
    GuardedCheckpointEvidenceV2, GuardedCheckpointOutcomeV2, GuardedCheckpointRejectionCodeV2,
    WorkspaceGenerationTokenV2, WsCkptRequest, WsCkptResponse,
};

use super::*;

enum DaemonReply {
    Response(WsCkptResponse),
}

fn spawn_daemon(
    replies: Vec<DaemonReply>,
) -> (
    tempfile::TempDir,
    String,
    thread::JoinHandle<Vec<WsCkptRequest>>,
) {
    let directory = tempfile::tempdir().unwrap();
    let socket_path = directory.path().join("ws-ckpt.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    let handle = thread::spawn(move || {
        let mut requests = Vec::with_capacity(replies.len());
        for reply in replies {
            let (mut stream, _) = listener.accept().unwrap();
            let mut length = [0_u8; 4];
            stream.read_exact(&mut length).unwrap();
            let mut payload = vec![0_u8; u32::from_le_bytes(length) as usize];
            stream.read_exact(&mut payload).unwrap();
            requests.push(bincode::deserialize(&payload).unwrap());

            let DaemonReply::Response(response) = reply;
            let payload = bincode::serialize(&response).unwrap();
            stream
                .write_all(&(payload.len() as u32).to_le_bytes())
                .unwrap();
            stream.write_all(&payload).unwrap();
        }
        requests
    });
    (
        directory,
        socket_path.to_string_lossy().into_owned(),
        handle,
    )
}

fn digest(byte: u8) -> Digest {
    Digest::parse(format!("{byte:02x}").repeat(32)).unwrap()
}

fn operation() -> CheckpointOperation {
    CheckpointOperation {
        target: TargetRef {
            kind: BoundedName::new("local").unwrap(),
            authority: BoundedName::new("cosh").unwrap(),
            identifier: BoundedOpaque::new("primary").unwrap(),
        },
        target_identity_digest: digest(8),
        runtime_fence: RuntimeExecutionFence {
            binding_id: RuntimeBindingId::new(),
            runtime_generation: 11,
            lease_generation: 12,
            lease_revision: 13,
        },
        operation_digest: digest(9),
        input_digest: digest(10),
        checkpoint_id: CheckpointId::new(),
        binding: CheckpointBinding {
            version: BINDING_VERSION,
            ws_id: "ws-abc123".to_owned(),
            registered_path: "/workspace".to_owned(),
            generation: [7; 32],
            owner_uid: nix::unistd::Uid::effective().as_raw(),
        },
    }
}

fn evidence(operation: &CheckpointOperation) -> GuardedCheckpointEvidenceV2 {
    GuardedCheckpointEvidenceV2 {
        ws_id: operation.binding.ws_id.clone(),
        registered_path: operation.binding.registered_path.clone(),
        generation: WorkspaceGenerationTokenV2::from_bytes(operation.binding.generation),
        checkpoint_id: operation.checkpoint_id.as_str().to_owned(),
        operation_digest: digest_bytes(&operation.operation_digest).unwrap(),
        caller_uid: operation.binding.owner_uid,
        outcome: GuardedCheckpointOutcomeV2::Created {
            snapshot_id: operation.checkpoint_id.as_str().to_owned(),
        },
    }
}

fn execute(
    operation: &CheckpointOperation,
    replies: Vec<DaemonReply>,
) -> (ExecutionTargetOutcome, Vec<WsCkptRequest>) {
    let (_directory, socket_path, daemon) = spawn_daemon(replies);
    let client = CkptClient::with_timeout(&socket_path, 1_000)
        .require_trusted_peer(operation.binding.owner_uid);
    let mut target = CheckpointTarget { client: &client };
    let outcome = target.execute(operation);
    (outcome, daemon.join().unwrap())
}

fn assert_guarded_request(request: &WsCkptRequest, operation: &CheckpointOperation) {
    let WsCkptRequest::GuardedCheckpointV2 {
        ws_id,
        expected_generation,
        checkpoint_id,
        operation_digest,
        message,
        metadata,
        pin,
    } = request
    else {
        panic!("expected one guarded checkpoint request")
    };
    assert_eq!(ws_id, &operation.binding.ws_id);
    assert_eq!(
        expected_generation,
        &WorkspaceGenerationTokenV2::from_bytes(operation.binding.generation)
    );
    assert_eq!(checkpoint_id, operation.checkpoint_id.as_str());
    assert_eq!(
        operation_digest,
        &digest_bytes(&operation.operation_digest).unwrap()
    );
    assert_eq!(message.as_deref(), Some("COSH governed Task checkpoint"));
    assert_eq!(metadata, &None);
    assert!(!pin);
}

fn assert_evidence_request(request: &WsCkptRequest, operation: &CheckpointOperation) {
    let WsCkptRequest::CheckpointEvidenceV2 {
        ws_id,
        expected_generation,
        checkpoint_id,
        operation_digest,
    } = request
    else {
        panic!("expected one read-only evidence request")
    };
    assert_eq!(ws_id, &operation.binding.ws_id);
    assert_eq!(
        expected_generation,
        &WorkspaceGenerationTokenV2::from_bytes(operation.binding.generation)
    );
    assert_eq!(checkpoint_id, operation.checkpoint_id.as_str());
    assert_eq!(
        operation_digest,
        &digest_bytes(&operation.operation_digest).unwrap()
    );
}

#[test]
fn created_checkpoint_commits_the_exact_typed_receipt() {
    let operation = operation();
    let evidence = evidence(&operation);
    let expected_receipt = digest_parts(&[
        RECEIPT_DOMAIN,
        operation.target_identity_digest.as_str().as_bytes(),
        operation.operation_digest.as_str().as_bytes(),
        operation.checkpoint_id.as_str().as_bytes(),
        &evidence.generation.into_bytes(),
        &serde_json::to_vec(&evidence).unwrap(),
    ])
    .unwrap();
    let (outcome, requests) = execute(
        &operation,
        vec![DaemonReply::Response(
            WsCkptResponse::GuardedCheckpointV2Ok {
                evidence: evidence.clone(),
            },
        )],
    );

    let ExecutionTargetOutcome::Conclusive {
        succeeded,
        receipt_digest,
        typed_result,
        ..
    } = outcome
    else {
        panic!("created checkpoint must be conclusive")
    };
    assert!(succeeded);
    assert_eq!(receipt_digest, expected_receipt);
    assert!(matches!(
        typed_result,
        Some(BrokeredOperationResult::WorkspaceCheckpointCreateV1(result))
            if result.checkpoint_id == operation.checkpoint_id
                && matches!(
                    result.outcome,
                    WorkspaceCheckpointCreateV1Outcome::Created { ref snapshot_id }
                        if snapshot_id.as_str() == operation.checkpoint_id.as_str()
                )
    ));
    assert_eq!(requests.len(), 1);
    assert_guarded_request(&requests[0], &operation);
}

#[test]
fn possibly_applied_reconciles_with_evidence_without_replay() {
    let operation = operation();
    let evidence = evidence(&operation);
    let mut mismatched_evidence = evidence.clone();
    mismatched_evidence.operation_digest = [6; 32];
    let (outcome, requests) = execute(
        &operation,
        vec![
            DaemonReply::Response(WsCkptResponse::GuardedCheckpointV2Ok {
                evidence: mismatched_evidence,
            }),
            DaemonReply::Response(WsCkptResponse::CheckpointEvidenceV2Ok {
                evidence: Some(evidence),
            }),
        ],
    );

    assert!(matches!(
        outcome,
        ExecutionTargetOutcome::Conclusive {
            succeeded: true,
            typed_result: Some(BrokeredOperationResult::WorkspaceCheckpointCreateV1(_)),
            ..
        }
    ));
    assert_eq!(requests.len(), 2);
    assert_guarded_request(&requests[0], &operation);
    assert_evidence_request(&requests[1], &operation);
}

#[test]
fn missing_reconcile_evidence_is_unknown_and_never_replayed() {
    let operation = operation();
    let mut mismatched_evidence = evidence(&operation);
    mismatched_evidence.operation_digest = [6; 32];
    let (outcome, requests) = execute(
        &operation,
        vec![
            DaemonReply::Response(WsCkptResponse::GuardedCheckpointV2Ok {
                evidence: mismatched_evidence,
            }),
            DaemonReply::Response(WsCkptResponse::CheckpointEvidenceV2Ok { evidence: None }),
        ],
    );

    assert!(matches!(outcome, ExecutionTargetOutcome::Unknown { .. }));
    assert_eq!(requests.len(), 2);
    assert_guarded_request(&requests[0], &operation);
    assert_evidence_request(&requests[1], &operation);
}

#[test]
fn explicit_v2_rejection_is_a_conclusive_failure() {
    let operation = operation();
    let (outcome, requests) = execute(
        &operation,
        vec![DaemonReply::Response(
            WsCkptResponse::GuardedCheckpointV2Rejected {
                code: GuardedCheckpointRejectionCodeV2::GenerationMismatch,
                message: "daemon-private generation detail".to_owned(),
            },
        )],
    );

    let ExecutionTargetOutcome::Conclusive {
        succeeded,
        safe_detail,
        typed_result,
        ..
    } = outcome
    else {
        panic!("an explicit V2 pre-effect rejection must be conclusive")
    };
    assert!(!succeeded);
    assert!(typed_result.is_none());
    let safe_detail = safe_detail.unwrap();
    assert!(!safe_detail.as_str().contains("daemon-private"));
    assert_eq!(requests.len(), 1);
    assert_guarded_request(&requests[0], &operation);
}
