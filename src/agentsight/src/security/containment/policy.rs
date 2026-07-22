//! Strict binding, target, and acknowledgement checks for containment trust decisions.

use std::collections::HashSet;

use agentsight_enforcement_protocol::{
    ApplyCredentialPolicy, Binding, BindingState, CredentialExfiltrationPolicy, PolicyMode,
    SecurityEventKind,
};
use uuid::Uuid;

use super::{ContainmentCandidate, ContainmentError};
use crate::enforcement::read_process_start_time;
use crate::security::RiskCaseDetail;

const AGENT_SOURCE: &str = "source AGENT = exec \"**\"";
const CREDENTIAL_SOURCE: &str = "source CREDENTIAL = file \"";
const RULE: &str = "rule agentsight-credential-exfiltration:";
const SINK: &str = " connect endpoint \"*\" if CREDENTIAL";
const TRUSTED: &str = " unless target \"";
const REASON: &str = "  because \"credential-derived data reached an untrusted network target\"";
const CREDENTIAL_TAINT_TTL_SECS: u64 = 900;

pub(super) struct ResolvedPolicy {
    pub(super) detail: RiskCaseDetail,
    pub(super) binding: Binding,
    pub(super) source_path: String,
    pub(super) trusted_endpoints: Vec<String>,
}

pub(super) fn resolve_policy(
    detail: RiskCaseDetail,
    bindings: Vec<Binding>,
) -> Option<ResolvedPolicy> {
    let evidence = detail.evidence.first()?;
    let file_action = match &evidence.kind {
        SecurityEventKind::FileAction(action) => action,
        _ => return None,
    };
    let mut matching = bindings
        .into_iter()
        .filter(|binding| binding.request.binding_id == evidence.identity.binding_id);
    let binding = matching.next().filter(|_| matching.next().is_none())?;
    let request = &binding.request;
    if binding.state != BindingState::Enforced
        || request.agent_id != detail.case.agent_id
        || request.agent_id != evidence.identity.agent_id
        || request.session_id != detail.case.session_id
        || request.session_id != evidence.identity.session_id
        || request.root_pid != evidence.identity.pid
        || request.process_start_time != evidence.identity.process_start_time
        || request.policy_id != detail.case.policy_id
        || request.policy_id != file_action.policy_id
        || request.policy_revision != detail.case.policy_revision.to_string()
        || file_action.policy_revision != detail.case.policy_revision
    {
        return None;
    }
    let compiled = parse_compiled_policy(&request.policy_dsl, CompiledMode::Audit)?;
    Some(ResolvedPolicy {
        detail,
        binding,
        source_path: compiled.source_path,
        trusted_endpoints: compiled.trusted_endpoints,
    })
}

pub(super) fn enforce_request(
    context: &ResolvedPolicy,
    binding_id: Uuid,
    root_pid: i32,
    process_start_time: u64,
) -> Option<ApplyCredentialPolicy> {
    let policy = CredentialExfiltrationPolicy {
        policy_id: context.binding.request.policy_id.clone(),
        revision: context.binding.request.policy_revision.parse().ok()?,
        source_patterns: vec![context.source_path.clone()],
        trusted_endpoints: context.trusted_endpoints.clone(),
        taint_label: "CREDENTIAL".into(),
        taint_ttl_secs: CREDENTIAL_TAINT_TTL_SECS,
        mode: PolicyMode::Enforce,
    };
    policy.validate().ok()?;
    Some(ApplyCredentialPolicy {
        binding_id,
        agent_id: context.binding.request.agent_id.clone(),
        session_id: context.binding.request.session_id.clone(),
        root_pid,
        process_start_time,
        policy,
    })
}

pub(super) fn live_candidates(
    agent_id: &str,
    candidates: Vec<ContainmentCandidate>,
) -> Result<Vec<ContainmentCandidate>, ContainmentError> {
    let mut seen = HashSet::new();
    let mut live = Vec::new();
    for candidate in candidates {
        if candidate.agent_id != agent_id {
            continue;
        }
        if !seen.insert(candidate.root_pid) {
            return Err(ContainmentError::AmbiguousCandidate(candidate.root_pid));
        }
        if read_process_start_time(candidate.root_pid)
            .is_ok_and(|actual| actual == candidate.process_start_time)
        {
            live.push(candidate);
        }
    }
    Ok(live)
}

pub(super) fn select_candidate(
    agent_id: &str,
    root_pid: i32,
    candidates: &[ContainmentCandidate],
) -> Result<ContainmentCandidate, ContainmentError> {
    live_candidates(agent_id, candidates.to_vec())?
        .into_iter()
        .find(|candidate| candidate.root_pid == root_pid)
        .ok_or(ContainmentError::RootProcessStale(root_pid))
}

pub(super) fn acknowledgement_matches(binding: &Binding, expected: &ApplyCredentialPolicy) -> bool {
    let request = &binding.request;
    let expected_source = match expected.policy.source_patterns.as_slice() {
        [source] => source,
        _ => return false,
    };
    if expected.policy.taint_label != "CREDENTIAL"
        || expected.policy.mode != PolicyMode::Enforce
        || expected.policy.trusted_endpoints.len() > 1
        || binding.state != BindingState::Enforced
        || request.binding_id != expected.binding_id
        || request.agent_id != expected.agent_id
        || request.session_id != expected.session_id
        || request.root_pid != expected.root_pid
        || request.process_start_time != expected.process_start_time
        || request.policy_id != expected.policy.policy_id
        || request.policy_revision != expected.policy.revision.to_string()
    {
        return false;
    }
    parse_compiled_policy(&request.policy_dsl, CompiledMode::Enforce).is_some_and(|compiled| {
        compiled.source_path == *expected_source
            && compiled.trusted_endpoints == expected.policy.trusted_endpoints
    })
}

#[derive(Clone, Copy)]
enum CompiledMode {
    Audit,
    Enforce,
}

struct CompiledPolicy {
    source_path: String,
    trusted_endpoints: Vec<String>,
}

fn parse_compiled_policy(dsl: &str, mode: CompiledMode) -> Option<CompiledPolicy> {
    let body = dsl.strip_suffix('\n')?;
    let lines: Vec<_> = body.split('\n').collect();
    if lines.len() != 5 || lines[0] != AGENT_SOURCE || lines[2] != RULE || lines[4] != REASON {
        return None;
    }
    let source_path = lines[1]
        .strip_prefix(CREDENTIAL_SOURCE)?
        .strip_suffix('"')?;
    if !valid_source_path(source_path) {
        return None;
    }
    let action = match mode {
        CompiledMode::Audit => "  notify",
        CompiledMode::Enforce => "  block",
    };
    let suffix = lines[3].strip_prefix(action)?.strip_prefix(SINK)?;
    let trusted_endpoints = if suffix.is_empty() {
        Vec::new()
    } else {
        let endpoint = suffix.strip_prefix(TRUSTED)?.strip_suffix('"')?;
        if !valid_literal(endpoint) {
            return None;
        }
        vec![endpoint.to_string()]
    };
    Some(CompiledPolicy {
        source_path: source_path.to_string(),
        trusted_endpoints,
    })
}

fn valid_source_path(value: &str) -> bool {
    if !valid_literal(value) || !value.starts_with('/') || value.contains('\u{fffd}') {
        return false;
    }
    let mut segments = value.split('/');
    segments.next() == Some("")
        && segments.clone().next().is_some()
        && segments.all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

fn valid_literal(value: &str) -> bool {
    !value.is_empty()
        && value.len() < 127
        && !value
            .chars()
            .any(|character| character == '"' || character == '\\' || character.is_control())
}
