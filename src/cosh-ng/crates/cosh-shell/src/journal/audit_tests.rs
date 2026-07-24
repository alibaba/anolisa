use super::*;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

fn private_root() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "cosh-shell-audit-test-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir(&root).unwrap();
    #[cfg(unix)]
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    root.canonicalize().unwrap()
}

#[test]
fn distinct_writers_lock_distinct_segments_and_close() {
    let root = private_root();
    let mut first = AuditSegmentWriter::create(&root).unwrap();
    let mut second = AuditSegmentWriter::create(&root).unwrap();
    let event = || {
        AuditEventV1::shell(
            "session.started",
            AuditIdentity {
                shell_session_id: Some("session".to_string()),
                ..AuditIdentity::default()
            },
            AuditEventOutcome {
                status: AuditOutcomeStatus::Started,
                code: None,
                retryable: false,
            },
            AuditSubject {
                kind: "session".to_string(),
                name: None,
            },
            &serde_json::json!({}),
        )
        .unwrap()
    };
    first.append(&mut event(), true).unwrap();
    second.append(&mut event(), true).unwrap();
    assert_ne!(first.active_path(), second.active_path());
    first.close().unwrap();
    second.close().unwrap();
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn command_projection_contains_no_raw_command_cwd_or_path() {
    let root = private_root();
    let mut recorder = ShellAuditRecorder {
        writer: Some(AuditSegmentWriter::create(&root).unwrap()),
        writer_root: Some(root.clone()),
        mode: AuditMode::BestEffort,
        shell_session_id: "session-1".to_string(),
        seen_events: 0,
        hash_salt: "salt".to_string(),
        degraded: false,
        warning_emitted: false,
        owned_approvals: std::collections::HashSet::new(),
        command_refs: std::collections::HashMap::new(),
    };
    let secret = "super-secret-command-value";
    let mut started = ShellEvent::command_started(
        "session-1",
        "cmd-1",
        format!("curl --token {secret}"),
        "/private/secret/work",
        1,
    );
    started.terminal_output_ref = Some("/private/secret/output".to_string());
    recorder.observe_shell_events(&[started]);
    drop(recorder);
    let content = walk_segment_text(&root);
    assert!(!content.contains(secret), "{content}");
    assert!(!content.contains("/private/secret"), "{content}");
    assert!(content.contains("terminal-output://") || content.contains("session.started"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn required_owned_approval_resolution_fails_before_execution_boundary() {
    let mut recorder = ShellAuditRecorder {
        writer: None,
        writer_root: None,
        mode: AuditMode::Required,
        shell_session_id: "session-1".to_string(),
        seen_events: 0,
        hash_salt: "salt".to_string(),
        degraded: true,
        warning_emitted: false,
        owned_approvals: std::collections::HashSet::new(),
        command_refs: std::collections::HashMap::new(),
    };
    let requested = recorder.record_approval_requested(ShellApprovalAuditInput {
        id: "approval-1",
        audit_ref: None,
        session_id: "session-1",
        run_id: "run-1",
        request_id: None,
        tool_use_id: None,
        subject: "shell command",
        risk: "medium",
        assessment: None,
        preview: "$ echo ok",
        status: "pending",
    });
    assert!(requested.is_none());
    let result = recorder.record_approval_resolved(ShellApprovalAuditInput {
        id: "approval-1",
        audit_ref: None,
        session_id: "session-1",
        run_id: "run-1",
        request_id: None,
        tool_use_id: None,
        subject: "shell command",
        risk: "medium",
        assessment: None,
        preview: "$ echo ok",
        status: "approved",
    });
    assert!(result
        .unwrap_err()
        .starts_with("AUDIT_REQUIRED_UNAVAILABLE"));
}

#[test]
fn required_core_host_execution_fails_before_handoff_boundary() {
    let mut recorder = ShellAuditRecorder {
        writer: None,
        writer_root: None,
        mode: AuditMode::Required,
        shell_session_id: "session-1".to_string(),
        seen_events: 0,
        hash_salt: "salt".to_string(),
        degraded: true,
        warning_emitted: false,
        owned_approvals: std::collections::HashSet::new(),
        command_refs: std::collections::HashMap::new(),
    };

    let result = recorder.authorize_host_execution(ShellApprovalAuditInput {
        id: "approval-1",
        audit_ref: None,
        session_id: "session-1",
        run_id: "run-1",
        request_id: Some("request-1"),
        tool_use_id: Some("tool-1"),
        subject: "run_shell_command",
        risk: "medium",
        assessment: None,
        preview: "$ echo ok",
        status: "approved",
    });

    assert!(result
        .unwrap_err()
        .starts_with("AUDIT_REQUIRED_UNAVAILABLE"));
}

#[test]
fn core_host_execution_does_not_duplicate_the_approval_resolution() {
    let root = private_root();
    let mut recorder = ShellAuditRecorder {
        writer: Some(AuditSegmentWriter::create(&root).unwrap()),
        writer_root: Some(root.clone()),
        mode: AuditMode::BestEffort,
        shell_session_id: "session-1".to_string(),
        seen_events: 0,
        hash_salt: "salt".to_string(),
        degraded: false,
        warning_emitted: false,
        owned_approvals: std::collections::HashSet::new(),
        command_refs: std::collections::HashMap::new(),
    };

    recorder
        .authorize_host_execution(ShellApprovalAuditInput {
            id: "approval-1",
            audit_ref: Some("core-approval-event"),
            session_id: "session-1",
            run_id: "run-1",
            request_id: Some("request-1"),
            tool_use_id: Some("tool-1"),
            subject: "run_shell_command",
            risk: "medium",
            assessment: None,
            preview: "$ echo ok",
            status: "approved",
        })
        .unwrap();

    drop(recorder);
    let content = walk_segment_text(&root);
    assert!(content.contains("\"event_type\":\"tool.execution.started\""));
    assert!(!content.contains("\"event_type\":\"approval.resolved\""));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn shell_owned_approval_does_not_require_a_provider_tool_identity() {
    let mut recorder = ShellAuditRecorder {
        writer: None,
        writer_root: None,
        mode: AuditMode::Required,
        shell_session_id: "session-1".to_string(),
        seen_events: 0,
        hash_salt: "salt".to_string(),
        degraded: false,
        warning_emitted: false,
        owned_approvals: std::collections::HashSet::from(["approval-1".to_string()]),
        command_refs: std::collections::HashMap::new(),
    };

    let result = recorder.authorize_host_execution(ShellApprovalAuditInput {
        id: "approval-1",
        audit_ref: None,
        session_id: "session-1",
        run_id: "run-1",
        request_id: None,
        tool_use_id: None,
        subject: "Bash",
        risk: "medium",
        assessment: None,
        preview: "$ echo ok",
        status: "approved",
    });

    assert!(result.is_ok());
}

#[test]
fn successful_write_closes_shell_degraded_episode() {
    let root = private_root();
    let mut recorder = ShellAuditRecorder {
        writer: Some(AuditSegmentWriter::create(&root).unwrap()),
        writer_root: Some(root.clone()),
        mode: AuditMode::BestEffort,
        shell_session_id: "session-1".to_string(),
        seen_events: 0,
        hash_salt: "salt".to_string(),
        degraded: true,
        warning_emitted: true,
        owned_approvals: std::collections::HashSet::new(),
        command_refs: std::collections::HashMap::new(),
    };
    assert!(recorder
        .record_evidence_accessed("command_output", Some("small"), None, true)
        .is_some());
    assert!(!recorder.degraded);
    drop(recorder);
    let content = walk_segment_text(&root);
    assert!(content.contains("\"event_type\":\"audit.degraded\""));
    assert!(content.contains("\"event_type\":\"audit.recovered\""));
    let _ = std::fs::remove_dir_all(root);
}

fn walk_segment_text(root: &Path) -> String {
    let mut text = String::new();
    let segments = root.join("v1/segments");
    for date in std::fs::read_dir(segments).unwrap() {
        for file in std::fs::read_dir(date.unwrap().path()).unwrap() {
            text.push_str(&std::fs::read_to_string(file.unwrap().path()).unwrap());
        }
    }
    text
}
