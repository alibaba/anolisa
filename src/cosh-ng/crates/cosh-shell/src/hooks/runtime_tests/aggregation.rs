use super::*;

#[test]
fn aggregation_combines_memory_pressure_and_process() {
    let findings = vec![
        finding("high-memory-process", FindingSeverity::Info),
        finding("memory-pressure", FindingSeverity::Warning),
    ];
    let aggregated = aggregate_hook_findings(findings);

    assert_eq!(aggregated.len(), 1);
    assert_eq!(aggregated[0].primary.hook_id, "memory-pressure");
    assert_eq!(aggregated[0].related[0].hook_id, "high-memory-process");
    assert_eq!(
        display_for_aggregate(&block(0), &aggregated[0], AnalysisMode::Smart),
        RuntimeHookDisplay::Consultation
    );
}

#[test]
fn aggregation_combines_same_topic_and_recommended_skill() {
    let findings = vec![
        external_finding(
            "docker-health",
            FindingSeverity::Warning,
            Some("container-analysis"),
        ),
        external_finding(
            "docker-cgroup",
            FindingSeverity::Critical,
            Some("container-analysis"),
        ),
    ];
    let aggregated = aggregate_hook_findings(findings);

    assert_eq!(aggregated.len(), 1);
    assert_eq!(aggregated[0].primary.hook_id, "docker-cgroup");
    assert_eq!(aggregated[0].related[0].hook_id, "docker-health");
    assert_eq!(
        aggregated[0].recommended_skill.as_deref(),
        Some("container-analysis")
    );
    assert_eq!(aggregated[0].topic, "external");
}

#[test]
fn aggregation_keeps_external_findings_without_skill_separate() {
    let findings = vec![
        external_finding("docker-health", FindingSeverity::Warning, None),
        external_finding("docker-cgroup", FindingSeverity::Warning, None),
    ];
    let aggregated = aggregate_hook_findings(findings);

    assert_eq!(aggregated.len(), 2);
}
