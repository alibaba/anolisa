use super::*;

#[test]
fn warning_process_without_pressure_is_hint_not_card() {
    let findings = vec![finding("high-memory-process", FindingSeverity::Warning)];
    let aggregated = aggregate_hook_findings(findings);

    assert_eq!(
        display_for_aggregate(&block(0), &aggregated[0], AnalysisMode::Smart),
        RuntimeHookDisplay::Hint
    );
}

#[test]
fn recent_memory_pressure_promotes_followup_process_warning_to_card() {
    let mut state = InlineState::default();
    let pressure_block = block_with_command_at("free -m", 1_000);
    let pressure =
        aggregate_hook_findings(vec![finding("memory-pressure", FindingSeverity::Warning)])
            .remove(0);
    record_aggregated_hook_finding(&pressure_block, pressure, &mut state);

    let mut process_block = block_with_command_at("ps aux --sort=-%mem | head", 2_000);
    process_block.id = "cmd-2".to_string();
    let process =
        aggregate_hook_findings(vec![process_finding("java (PID 1234) uses 31.2% MEM")]).remove(0);
    record_aggregated_hook_finding(&process_block, process, &mut state);

    let hint = state.hooks.findings.last().unwrap();
    assert_eq!(hint.display, RuntimeHookDisplay::Consultation);
    assert_eq!(hint.confidence, "high");
    assert_eq!(hint.related_hook_ids, vec!["memory-pressure".to_string()]);
}

#[test]
fn pressure_upgrades_twenty_percent_process_to_warning() {
    let mut state = InlineState::default();
    let pressure_block = block_with_command_at("free -m", 1_000);
    let pressure =
        aggregate_hook_findings(vec![finding("memory-pressure", FindingSeverity::Warning)])
            .remove(0);
    record_aggregated_hook_finding(&pressure_block, pressure, &mut state);

    let mut process_block = block_with_command_at("ps aux --sort=-%mem | head", 2_000);
    process_block.id = "cmd-2".to_string();
    let process = aggregate_hook_findings(vec![process_finding_with_severity(
        "java (PID 1234) uses 25.0% MEM",
        FindingSeverity::Info,
    )])
    .remove(0);
    record_aggregated_hook_finding(&process_block, process, &mut state);

    let hint = state.hooks.findings.last().unwrap();
    assert_eq!(hint.effective_severity, FindingSeverity::Warning);
    assert_eq!(hint.display, RuntimeHookDisplay::Consultation);
}

#[test]
fn critical_pressure_upgrades_thirty_five_percent_process_to_critical() {
    let mut state = InlineState::default();
    let pressure_block = block_with_command_at("free -m", 1_000);
    let pressure =
        aggregate_hook_findings(vec![finding("memory-pressure", FindingSeverity::Critical)])
            .remove(0);
    record_aggregated_hook_finding(&pressure_block, pressure, &mut state);

    let mut process_block = block_with_command_at("ps aux --sort=-%mem | head", 2_000);
    process_block.id = "cmd-2".to_string();
    let process = aggregate_hook_findings(vec![process_finding_with_severity(
        "java (PID 1234) uses 36.0% MEM",
        FindingSeverity::Warning,
    )])
    .remove(0);
    record_aggregated_hook_finding(&process_block, process, &mut state);

    let hint = state.hooks.findings.last().unwrap();
    assert_eq!(hint.effective_severity, FindingSeverity::Critical);
    assert_eq!(hint.display, RuntimeHookDisplay::Consultation);
}

#[test]
fn pressure_does_not_upgrade_process_when_percent_is_unparseable() {
    let mut state = InlineState::default();
    let pressure_block = block_with_command_at("free -m", 1_000);
    let pressure =
        aggregate_hook_findings(vec![finding("memory-pressure", FindingSeverity::Warning)])
            .remove(0);
    record_aggregated_hook_finding(&pressure_block, pressure, &mut state);

    let mut process_block = block_with_command_at("ps aux --sort=-%mem | head", 2_000);
    process_block.id = "cmd-2".to_string();
    let process = aggregate_hook_findings(vec![process_finding_with_severity(
        "java process memory is high",
        FindingSeverity::Info,
    )])
    .remove(0);
    record_aggregated_hook_finding(&process_block, process, &mut state);

    let hint = state.hooks.findings.last().unwrap();
    assert_eq!(hint.effective_severity, FindingSeverity::Info);
    assert_eq!(hint.display, RuntimeHookDisplay::Silent);
}

#[test]
fn stale_memory_pressure_does_not_promote_process_warning() {
    let mut state = InlineState::default();
    let pressure_block = block_with_command_at("free -m", 1_000);
    let pressure =
        aggregate_hook_findings(vec![finding("memory-pressure", FindingSeverity::Warning)])
            .remove(0);
    record_aggregated_hook_finding(&pressure_block, pressure, &mut state);

    let mut process_block = block_with_command_at(
        "ps aux --sort=-%mem | head",
        1_001 + INTERRUPTION_BUDGET_WINDOW_MS,
    );
    process_block.id = "cmd-2".to_string();
    let process =
        aggregate_hook_findings(vec![process_finding("java (PID 1234) uses 31.2% MEM")]).remove(0);
    record_aggregated_hook_finding(&process_block, process, &mut state);

    let hint = state.hooks.findings.last().unwrap();
    assert_eq!(hint.display, RuntimeHookDisplay::Hint);
    assert_eq!(hint.confidence, "medium");
    assert!(hint.related_hook_ids.is_empty());
}

#[test]
fn interactive_top_guidance_info_is_hint_not_card() {
    let findings = vec![finding("interactive-top-guidance", FindingSeverity::Info)];
    let aggregated = aggregate_hook_findings(findings);

    assert_eq!(
        display_for_aggregate(
            &block_with_command("top"),
            &aggregated[0],
            AnalysisMode::Smart
        ),
        RuntimeHookDisplay::Hint
    );
}

#[test]
fn swap_only_memory_pressure_info_is_silent() {
    let findings = vec![finding("memory-pressure", FindingSeverity::Info)];
    let aggregated = aggregate_hook_findings(findings);

    assert_eq!(
        display_for_aggregate(
            &block_with_command("free -m"),
            &aggregated[0],
            AnalysisMode::Smart
        ),
        RuntimeHookDisplay::Silent
    );
}

#[test]
fn high_memory_process_entity_key_uses_stable_pid() {
    let first = aggregate_hook_findings(vec![process_finding("java (PID 1234) uses 31.2% MEM")]);
    let second = aggregate_hook_findings(vec![process_finding("java (PID 1234) uses 45.8% MEM")]);
    let block = block_with_command("ps aux --sort=-%mem | head");

    assert_eq!(entity_key(&block, &first[0]), "process:pid:1234");
    assert_eq!(
        entity_key(&block, &first[0]),
        entity_key(&block, &second[0])
    );
}

#[test]
fn high_memory_process_entity_key_falls_back_to_title() {
    let aggregated = aggregate_hook_findings(vec![process_finding("java uses 31.2% MEM")]);
    let block = block_with_command("ps aux --sort=-%mem | head");

    assert_eq!(
        entity_key(&block, &aggregated[0]),
        "process:title:java uses 31.2% MEM"
    );
}

#[test]
fn high_memory_process_suppression_key_uses_stable_pid() {
    let first = aggregate_hook_findings(vec![process_finding("java (PID 1234) uses 31.2% MEM")]);
    let second = aggregate_hook_findings(vec![process_finding("java (PID 1234) uses 45.8% MEM")]);
    let block = block_with_command("ps aux --sort=-%mem | head");

    assert_eq!(
        suppression_key(&block, &first[0]),
        "memory:high-memory-process:process:pid:1234:ps"
    );
    assert_eq!(
        suppression_key(&block, &first[0]),
        suppression_key(&block, &second[0])
    );
}

#[test]
fn high_memory_process_suppression_key_keeps_different_pids_separate() {
    let first = aggregate_hook_findings(vec![process_finding("java (PID 1234) uses 31.2% MEM")]);
    let second =
        aggregate_hook_findings(vec![process_finding("postgres (PID 5678) uses 32.1% MEM")]);
    let block = block_with_command("ps aux --sort=-%mem | head");

    assert_ne!(
        suppression_key(&block, &first[0]),
        suppression_key(&block, &second[0])
    );
}
