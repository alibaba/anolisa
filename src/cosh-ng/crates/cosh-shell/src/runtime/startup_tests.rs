use std::collections::BTreeMap;

use super::{
    extract_bootstrap_path, merge_path_lists, plan_startup_for_render, raw_passthrough_args,
    record_visible_personal_impressions, render_pending_recommendation_notice,
    startup_prompt_selection_supported, visible_personal_candidates, write_startup_suggestion_card,
};
use crate::config::Language;
use crate::diagnostics::health::{HealthMessageId, HealthScanReport, HealthTryItem, HealthTryKind};
use crate::recommendation::personal_feedback::FrozenPromptBinding;
use crate::recommendation::personal_model::{
    ActivityPayload, CandidateEvidenceSummary, CandidateSource, ContextAffinity, FeedbackAction,
    ScopeKind, DISCLOSURE_VERSION,
};
use crate::recommendation::personal_planner::{PlannerCandidate, PlannerContext};
use crate::recommendation::personal_runtime::PersonalRuntime;
use crate::runtime::state::{AnalysisMode, InlineState, PendingInputGhostBinding};
use crate::ui::RatatuiInlineRenderer;
use crate::I18n;

#[test]
fn recommendation_notice_is_nonblocking_persisted_and_shown_once() {
    let root = std::env::temp_dir().join(format!(
        "cosh-startup-notice-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let writer = PersonalRuntime::open(true, &root, 1)
        .unwrap()
        .spawn_writer()
        .unwrap();
    let mut state = InlineState {
        personalization: crate::recommendation::personal_state::PersonalizationState {
            writer: Some(writer),
            ..Default::default()
        },
        analysis_mode: AnalysisMode::Smart,
        ..InlineState::default()
    };
    let mut first = Vec::new();

    render_pending_recommendation_notice(&mut state, &mut first).unwrap();

    let text = String::from_utf8(first).unwrap();
    assert!(text.starts_with("\r\u{1b}[2K"), "{text:?}");
    assert!(text.contains("Prompt recommendations are on"));
    assert!(text.contains("current AI"));
    for hidden in ["gate4", "endpoint", "provider_id", "fingerprint"] {
        assert!(!text.contains(hidden));
    }
    assert!(state.personalization.notice_shown);
    assert!(state.trigger_pty_prompt);
    assert_eq!(
        state
            .personalization
            .writer
            .as_ref()
            .unwrap()
            .poll_snapshot()
            .unwrap()
            .preferences
            .notice_version_seen,
        crate::recommendation::personal_model::DISCLOSURE_VERSION
    );

    state.personalization.notice_shown = false;
    let mut second = Vec::new();
    render_pending_recommendation_notice(&mut state, &mut second).unwrap();
    assert!(second.is_empty());

    let mut writer = state.personalization.writer.take().unwrap();
    writer
        .shutdown(1, std::time::Duration::from_secs(1))
        .unwrap();
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn recommendation_notice_waits_until_smart_or_auto_ai_mode() {
    let root = std::env::temp_dir().join(format!(
        "cosh-startup-notice-mode-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let writer = PersonalRuntime::open(true, &root, 1)
        .unwrap()
        .spawn_writer()
        .unwrap();
    let mut state = InlineState {
        personalization: crate::recommendation::personal_state::PersonalizationState {
            writer: Some(writer),
            ..Default::default()
        },
        analysis_mode: AnalysisMode::Manual,
        ..InlineState::default()
    };
    let mut output = Vec::new();

    render_pending_recommendation_notice(&mut state, &mut output).unwrap();
    assert!(output.is_empty());
    state.analysis_mode = AnalysisMode::Smart;
    state.personalization.ai_disabled = true;
    render_pending_recommendation_notice(&mut state, &mut output).unwrap();
    assert!(output.is_empty());

    let mut writer = state.personalization.writer.take().unwrap();
    writer
        .shutdown(1, std::time::Duration::from_secs(1))
        .unwrap();
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn repeated_candidate_impressions_use_distinct_activity_identities() {
    let root = std::env::temp_dir().join(format!(
        "cosh-startup-impression-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut runtime = PersonalRuntime::open(true, &root, 1).unwrap();
    runtime.mark_notice_seen(DISCLOSURE_VERSION, 1).unwrap();
    let writer = runtime.spawn_writer().unwrap();
    let mut state = InlineState {
        personalization: crate::recommendation::personal_state::PersonalizationState {
            writer: Some(writer),
            ..Default::default()
        },
        ..InlineState::default()
    };
    for lifecycle in ["intent-1", "intent-2"] {
        state.pending_prompt_suggestion_bindings.insert(
            "candidate-1".to_string(),
            PendingInputGhostBinding::Personal(FrozenPromptBinding {
                candidate_id: "candidate-1".to_string(),
                task_ref: "task-1".to_string(),
                original_prompt: "continue payment investigation".to_string(),
                source: CandidateSource::RecentTask,
                suppression_key: "suppress-1".to_string(),
                profile_generation: 1,
                intent_lifecycle_id: lifecycle.to_string(),
            }),
        );
        record_visible_personal_impressions(&mut state, ".");
        state.pending_prompt_suggestion_bindings.clear();
    }

    let mut writer = state.personalization.writer.take().unwrap();
    writer
        .shutdown(1, std::time::Duration::from_secs(1))
        .unwrap();
    let snapshot = writer.poll_snapshot().unwrap();
    let impressions = snapshot
        .journal
        .records
        .iter()
        .filter(|record| {
            matches!(
                record.payload,
                ActivityPayload::RecommendationFeedback {
                    action: FeedbackAction::Impression,
                    ..
                }
            )
        })
        .count();
    assert_eq!(impressions, 2);
    let _ = std::fs::remove_dir_all(root);
}

fn health_try(id: &str, score: i32) -> HealthTryItem {
    let prompt_id = match id {
        "health-a" => HealthMessageId::HealthTryAnalyzeMemoryPressure,
        "health-b" => HealthMessageId::HealthTryInspectDiskUsage,
        _ => HealthMessageId::HealthTryCheckSwapPressure,
    };
    HealthTryItem {
        id: id.to_string(),
        label_id: prompt_id,
        label_args: BTreeMap::new(),
        prompt_id: Some(prompt_id),
        prompt_args: BTreeMap::new(),
        kind: HealthTryKind::AskAgent,
        command: None,
        reason_id: HealthMessageId::HealthTryReasonMemoryLow,
        reason_args: BTreeMap::new(),
        score,
        finding_id: format!("finding-{id}"),
    }
}

fn personal(id: &str) -> PlannerCandidate {
    PlannerCandidate {
        candidate_id: id.to_string(),
        source: CandidateSource::RecentTask,
        task_ref: format!("task-{id}"),
        prompt_text: format!("continue {id}"),
        context_affinity: ContextAffinity {
            scope_kind: ScopeKind::Repo,
            repo_id: Some("repo-a".to_string()),
            host_id: Some("host-a".to_string()),
        },
        last_seen_hour_bucket: 10_000,
        evidence: CandidateEvidenceSummary {
            snapshot_ids: vec![format!("snapshot-{id}")],
            agent_request_count: 1,
            compatible_shell_count: 0,
            submitted_feedback_count: 0,
            intent_occurrence_count: 1,
            active_day_buckets: vec![1],
            continuation_evidence: false,
        },
        entities: Vec::new(),
        suppression_key: format!("suppress-{id}"),
        last_action_failed: false,
        consecutive_explicit_dismissals: 0,
        suppressed: false,
    }
}

fn planner_context() -> PlannerContext {
    PlannerContext {
        now_hour_bucket: 10_000,
        repo_id: Some("repo-a".to_string()),
        host_id: Some("host-a".to_string()),
    }
}

#[test]
fn startup_render_plan_suppresses_personal_when_health_did_not_resolve() {
    let rendered = plan_startup_for_render(
        I18n::new(Language::EnUs),
        None,
        &planner_context(),
        &[personal("recent")],
    );

    assert!(rendered.visible_candidates.is_empty());
    assert!(rendered.ghost_candidate.is_none());
}

#[test]
fn startup_render_plan_is_health_first_and_caps_actual_visible_at_three() {
    let mut report = HealthScanReport::new("health", 0);
    report.try_items = vec![health_try("health-a", 100), health_try("health-b", 90)];

    let rendered = plan_startup_for_render(
        I18n::new(Language::EnUs),
        Some(&report),
        &planner_context(),
        &[
            personal("recent-a"),
            personal("recent-b"),
            personal("z-omitted"),
        ],
    );

    assert_eq!(
        rendered
            .visible_candidates
            .iter()
            .map(|candidate| candidate.candidate_id.as_str())
            .collect::<Vec<_>>(),
        vec!["health-a", "health-b", "recent-a"]
    );
    assert_eq!(
        rendered
            .ghost_candidate
            .as_ref()
            .map(|candidate| candidate.candidate_id.as_str()),
        Some("health-a")
    );
    assert_eq!(
        visible_personal_candidates(&rendered)
            .iter()
            .map(|candidate| candidate.candidate_id.as_str())
            .collect::<Vec<_>>(),
        vec!["recent-a"]
    );
}

#[test]
fn suggestion_card_labels_health_before_personal_and_explains_all_keys() {
    let mut report = HealthScanReport::new("health", 0);
    report.try_items = vec![health_try("health-a", 100)];
    let rendered = plan_startup_for_render(
        I18n::new(Language::EnUs),
        Some(&report),
        &planner_context(),
        &[personal("recent-a"), personal("recent-b")],
    );
    let state = InlineState {
        language: Language::ZhCn,
        ..InlineState::default()
    };
    let mut output = Vec::new();

    write_startup_suggestion_card(
        &state,
        &RatatuiInlineRenderer::with_width(120).with_language(Language::ZhCn),
        &rendered.visible_candidates,
        &mut output,
    )
    .unwrap();

    let text = String::from_utf8(output).unwrap();
    assert!(text.find("[异常排查]").unwrap() < text.find("[个性化]").unwrap());
    assert!(text.contains("Shift+Tab 切换"));
    assert!(text.contains("Tab 填入"));
    assert!(text.contains("Enter 直接提问"));
    assert_eq!(rendered.visible_candidates.len(), 3);
}

#[test]
fn single_suggestion_hides_cycle_instruction() {
    let state = InlineState::default();
    let mut output = Vec::new();
    write_startup_suggestion_card(
        &state,
        &RatatuiInlineRenderer::with_width(120),
        &[personal("recent-a")],
        &mut output,
    )
    .unwrap();

    let text = String::from_utf8(output).unwrap();
    assert!(text.contains("Tab insert · Enter ask"));
    assert!(!text.contains("Shift+Tab cycle"));
}

#[test]
fn prompt_selection_requires_capable_terminal_but_not_color() {
    assert!(startup_prompt_selection_supported(
        false,
        Some("xterm-256color")
    ));
    assert!(startup_prompt_selection_supported(false, None));
    assert!(!startup_prompt_selection_supported(false, Some("dumb")));
    assert!(!startup_prompt_selection_supported(
        true,
        Some("xterm-256color")
    ));
}

#[test]
fn startup_render_plan_keeps_three_health_prompts_at_narrow_width() {
    let mut report = HealthScanReport::new("health", 0);
    report.try_items = vec![
        health_try("health-a", 100),
        health_try("health-b", 90),
        health_try("health-c", 80),
    ];

    let rendered = plan_startup_for_render(
        I18n::new(Language::EnUs),
        Some(&report),
        &planner_context(),
        &[
            personal("recent-a"),
            personal("recent-b"),
            personal("z-omitted"),
        ],
    );

    assert_eq!(rendered.visible_candidates.len(), 3);
    assert_eq!(
        rendered
            .visible_candidates
            .iter()
            .map(|candidate| candidate.candidate_id.as_str())
            .collect::<Vec<_>>(),
        vec!["health-a", "health-b", "health-c"]
    );
    assert!(visible_personal_candidates(&rendered).is_empty());
}

#[test]
fn startup_renderer_planner_fixture_delta_stays_within_p95_p99_budget() {
    let mut report = HealthScanReport::new("health", 0);
    report.try_items = vec![health_try("health-a", 100)];
    let personal = vec![personal("recent-a"), personal("recent-b")];
    let context = planner_context();

    for _ in 0..32 {
        std::hint::black_box(plan_startup_for_render(
            I18n::new(Language::EnUs),
            Some(&report),
            &context,
            &personal,
        ));
    }

    let mut deltas = Vec::with_capacity(512);
    for _ in 0..512 {
        let baseline_started = std::time::Instant::now();
        std::hint::black_box(plan_startup_for_render(
            I18n::new(Language::EnUs),
            Some(&report),
            &context,
            &[],
        ));
        let baseline = baseline_started.elapsed();

        let personalized_started = std::time::Instant::now();
        std::hint::black_box(plan_startup_for_render(
            I18n::new(Language::EnUs),
            Some(&report),
            &context,
            &personal,
        ));
        deltas.push(personalized_started.elapsed().saturating_sub(baseline));
    }
    deltas.sort_unstable();
    let p95 = deltas[(deltas.len() * 95).div_ceil(100) - 1];
    let p99 = deltas[(deltas.len() * 99).div_ceil(100) - 1];

    eprintln!("startup renderer+planner delta p95={p95:?} p99={p99:?}");
    assert!(p95 <= std::time::Duration::from_millis(20), "p95={p95:?}");
    assert!(p99 <= std::time::Duration::from_millis(50), "p99={p99:?}");
}

#[test]
fn bootstrap_path_extracts_last_marked_value() {
    let text = "plugin noise\n__COSH_PATH_BEGIN__/a:/b__COSH_PATH_END__\n";
    assert_eq!(extract_bootstrap_path(text), Some("/a:/b".to_string()));
    assert_eq!(extract_bootstrap_path("plugin noise"), None);
}

#[test]
fn bootstrap_path_merge_keeps_existing_and_common_dirs() {
    assert_eq!(
        merge_path_lists(&[
            "/opt/homebrew/bin:/usr/bin:/bin",
            "/usr/local/bin:/bin",
            "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        ]),
        "/opt/homebrew/bin:/usr/bin:/bin:/usr/local/bin:/usr/sbin:/sbin"
    );
}

#[test]
fn raw_passthrough_args_strips_raw_adapter_for_dash_c() {
    assert_eq!(
        raw_passthrough_args(&[
            "cosh-shell".to_string(),
            "raw".to_string(),
            "cosh-core".to_string(),
            "-c".to_string(),
            "echo ok".to_string()
        ]),
        Some(vec![
            "cosh-shell".to_string(),
            "-c".to_string(),
            "echo ok".to_string()
        ])
    );
}

#[test]
fn raw_passthrough_args_preserves_shell_option() {
    assert_eq!(
        raw_passthrough_args(&[
            "cosh-shell".to_string(),
            "raw".to_string(),
            "--shell".to_string(),
            "bash".to_string(),
            "cosh-core".to_string(),
            "-c".to_string(),
            "echo ok".to_string()
        ]),
        Some(vec![
            "cosh-shell".to_string(),
            "--shell".to_string(),
            "bash".to_string(),
            "-c".to_string(),
            "echo ok".to_string()
        ])
    );
}
