use cosh_shell::hook_types::HookFinding;
use cosh_shell::types::CommandBlock;

use super::runtime::{
    computed_entity_key, computed_finding_confidence, computed_suppression_key,
    finding_topic_from_findings, is_memory_hook, memory_hook_preference,
    recommended_skill_from_findings, severity_rank, AggregatedHookFinding,
};

pub(super) fn aggregate_hook_findings(findings: Vec<HookFinding>) -> Vec<AggregatedHookFinding> {
    let mut memory_findings = Vec::new();
    let mut aggregated = Vec::new();

    for finding in findings {
        if is_memory_hook(&finding.hook_id) {
            memory_findings.push(finding);
        } else if let Some(existing) = aggregated
            .iter_mut()
            .find(|aggregate| should_merge_into_aggregate(aggregate, &finding))
        {
            merge_finding_into_aggregate(existing, finding);
        } else {
            aggregated.push(new_aggregated_hook_finding(finding, Vec::new()));
        }
    }

    if !memory_findings.is_empty() {
        memory_findings.sort_by(|left, right| {
            severity_rank(right.severity)
                .cmp(&severity_rank(left.severity))
                .then_with(|| {
                    memory_hook_preference(&right.hook_id)
                        .cmp(&memory_hook_preference(&left.hook_id))
                })
        });
        let primary = memory_findings.remove(0);
        aggregated.push(new_aggregated_hook_finding(primary, memory_findings));
    }

    aggregated.sort_by(|left, right| {
        severity_rank(right.primary.severity).cmp(&severity_rank(left.primary.severity))
    });
    aggregated
}

fn should_merge_into_aggregate(aggregate: &AggregatedHookFinding, finding: &HookFinding) -> bool {
    let Some(skill) = finding.skill.as_deref() else {
        return false;
    };
    aggregate.recommended_skill.as_deref() == Some(skill)
        && aggregate.topic == finding_topic_from_findings(finding, &[])
}

fn merge_finding_into_aggregate(aggregate: &mut AggregatedHookFinding, finding: HookFinding) {
    if severity_rank(finding.severity) > severity_rank(aggregate.primary.severity) {
        let previous_primary = std::mem::replace(&mut aggregate.primary, finding);
        aggregate.related.insert(0, previous_primary);
    } else {
        aggregate.related.push(finding);
    }
    aggregate.recommended_skill =
        recommended_skill_from_findings(&aggregate.primary, &aggregate.related);
    aggregate.topic =
        finding_topic_from_findings(&aggregate.primary, &aggregate.related).to_string();
    aggregate.effective_severity = aggregate.primary.severity;
}

fn new_aggregated_hook_finding(
    primary: HookFinding,
    related: Vec<HookFinding>,
) -> AggregatedHookFinding {
    let recommended_skill = recommended_skill_from_findings(&primary, &related);
    let topic = finding_topic_from_findings(&primary, &related).to_string();
    let effective_severity = primary.severity;
    AggregatedHookFinding {
        primary,
        related,
        recommended_skill,
        topic,
        entity_key: String::new(),
        effective_severity,
        confidence: String::new(),
        suppression_key: String::new(),
    }
}

pub(super) fn refresh_aggregate_metadata(
    block: &CommandBlock,
    aggregate: &mut AggregatedHookFinding,
) {
    aggregate.recommended_skill =
        recommended_skill_from_findings(&aggregate.primary, &aggregate.related);
    aggregate.topic =
        finding_topic_from_findings(&aggregate.primary, &aggregate.related).to_string();
    aggregate.entity_key = computed_entity_key(block, aggregate);
    aggregate.effective_severity = aggregate.primary.severity;
    aggregate.confidence = computed_finding_confidence(block, aggregate).to_string();
    aggregate.suppression_key = computed_suppression_key(block, aggregate);
}
