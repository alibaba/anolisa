use super::command_risk::{
    assess_pipeline, assess_simple_command, insert_structural_reason, is_high_risk_explanation,
    AssessmentConfidence, AssessmentPolicy, AutoAllowEvidence, CommandAssessment, CommandShape,
    ExecutionDecision, InteractionRequirement, OutputExposure, OutputStability, RiskImpact,
    SideEffectClass,
};
use super::command_risk_build::{
    dedupe_reasons, max_output_exposure, max_output_stability, min_confidence,
};
use super::command_risk_parser::ParsedCommand;

/// Returns the per-segment pipeline stages for all compound commands
/// (`&&`/`||`/`;`/newline separated, issue #1785): every segment is
/// assessed individually and the results aggregated so high-risk tails
/// keep their full stage assessment instead of being masked by the
/// first segment. A bare pipeline masked by an input redirection
/// (`cat < in | rm ...`, where `RedirectionRead` outranks `Pipeline` as
/// dominant shape) has no segment separators, so all of its stages
/// become a single pipeline segment. Returns `None` for non-compound
/// shapes and for single-stage `RedirectionRead` commands, which keep
/// their shape-specific paths.
pub(super) fn compound_segments(parsed: &ParsedCommand) -> Option<Vec<Vec<Vec<String>>>> {
    if !matches!(
        parsed.shape,
        CommandShape::AndOrList | CommandShape::Sequence | CommandShape::RedirectionRead
    ) {
        return None;
    }
    if !parsed.segments.is_empty() {
        return Some(parsed.segments.clone());
    }
    (parsed.stages.len() > 1).then(|| vec![parsed.stages.clone()])
}

/// Assesses a compound command (`&&` / `||` / `;` / newline separated)
/// by re-using the existing simple/pipeline assessment per segment and
/// aggregating the results: impact takes the maximum, confidence the
/// minimum, and reasons are union-deduplicated (issue #1785). Assessing
/// per recorded segment keeps command/argument boundaries, unlike the
/// earlier word-scan compensation (PR #1790 review) which both missed
/// rules that need full stage assessment (`kubectl delete`, `docker
/// run`, `awk system()`, `curl | sh`) and escalated benign arguments
/// (`echo rm>/dev/null && true`).
///
/// When `policy.compound_readonly_executor` is set, all segments carry
/// per-segment `AutoAllowEvidence`, none contains a shell builtin with
/// cross-segment state effects (`cd`, `export`, env assignments), and
/// none requires TTY interaction, the assessment is upgraded to
/// `AutoAllow` with `AutoAllowEvidence::CompoundReadonlyExecutor`.
/// Aggregated `Low` impact alone is insufficient; every segment must
/// individually qualify (issue #1882).
pub(super) fn assess_stripped_compound(
    command: &str,
    shape: CommandShape,
    segments: &[Vec<Vec<String>>],
    policy: AssessmentPolicy,
) -> CommandAssessment {
    let mut impact = RiskImpact::Low;
    let mut confidence = AssessmentConfidence::High;
    let mut interaction = InteractionRequirement::None;
    let mut output_stability = OutputStability::StableSnapshot;
    let mut output_exposure = OutputExposure::Normal;
    let mut side_effects: Vec<SideEffectClass> = Vec::new();
    let mut reasons: Vec<&'static str> = Vec::new();
    let mut all_segments_auto_allow = true;

    for segment in segments {
        let segment_text = segment
            .iter()
            .map(|stage| stage.join(" "))
            .collect::<Vec<_>>()
            .join(" | ");
        let parsed = ParsedCommand {
            shape: if segment.len() > 1 {
                CommandShape::Pipeline
            } else {
                CommandShape::Simple
            },
            stages: segment.clone(),
            null_redirections: 0,
            segments: Vec::new(),
        };
        let assessed = if segment.len() > 1 {
            assess_pipeline(&segment_text, parsed, policy)
        } else {
            assess_simple_command(&segment_text, parsed, policy)
        };
        impact = impact.max(assessed.impact);
        confidence = min_confidence(confidence, assessed.confidence);
        interaction = max_interaction(interaction, assessed.interaction);
        output_stability = max_output_stability(output_stability, assessed.output_stability);
        output_exposure = max_output_exposure(output_exposure, assessed.output_exposure);
        for side_effect in assessed.side_effects {
            if !side_effects.contains(&side_effect) {
                side_effects.push(side_effect);
            }
        }
        reasons.extend(assessed.reasons);
        // Track whether this segment individually carries auto-allow evidence
        // and does not contain state-mutating shell constructs.
        if assessed.auto_allow.is_none() || segment_has_state_mutating_builtin(segment) {
            all_segments_auto_allow = false;
        }
    }

    let mut reasons = dedupe_reasons(reasons);
    if impact == RiskImpact::High {
        // Keep a high-risk explanation as the primary reason so the
        // approval card renders the matching phrase (ARP SDD design §4).
        if let Some(position) = reasons
            .iter()
            .position(|reason| is_high_risk_explanation(reason))
        {
            let primary = reasons.remove(position);
            reasons.insert(0, primary);
        }
    }

    // When every segment individually qualifies for auto-allow and no
    // segment contains state-mutating builtins or TTY requirements, the
    // compound can be run through CompoundReadonlyExecutor without a shell.
    let compound_auto_allow = policy.compound_readonly_executor
        && all_segments_auto_allow
        && !segments.is_empty()
        && interaction == InteractionRequirement::None
        && impact == RiskImpact::Low;

    if compound_auto_allow {
        reasons.insert(0, "compound-readonly-executor");
        return CommandAssessment {
            source: policy.source,
            command: command.to_string(),
            shape,
            execution: ExecutionDecision::AutoAllow,
            impact,
            confidence: min_confidence(confidence, AssessmentConfidence::High),
            interaction,
            output_stability,
            output_exposure,
            side_effects,
            reasons,
            auto_allow: Some(AutoAllowEvidence::CompoundReadonlyExecutor),
        };
    }

    insert_structural_reason(
        &mut reasons,
        match shape {
            CommandShape::AndOrList => "and-or-list-not-auto-executable",
            CommandShape::Sequence => "sequence-not-auto-executable",
            CommandShape::RedirectionRead => "read-redirection-not-auto-executable",
            _ => "complex-shell-not-auto-executable",
        },
    );

    CommandAssessment {
        source: policy.source,
        command: command.to_string(),
        shape,
        execution: ExecutionDecision::AskUser,
        impact,
        confidence: min_confidence(confidence, AssessmentConfidence::Medium),
        interaction,
        output_stability,
        output_exposure,
        side_effects,
        reasons,
        auto_allow: None,
    }
}

/// Applies the conservative `Complex` classification: floor the impact
/// at Medium, force Low confidence, and fail closed to High when the
/// command splits into more than one segment (issue #1785 review) —
/// subshell/brace/background syntax cannot be reliably segmented, so
/// tail segments stay invisible to the first-stage assessment and the
/// risk must not be understated. The execution boundary (`AskUser`) is
/// untouched.
pub(super) fn finalize_complex(assessment: &mut CommandAssessment, parsed: &ParsedCommand) {
    assessment.execution = ExecutionDecision::AskUser;
    assessment.confidence = AssessmentConfidence::Low;
    if assessment.impact < RiskImpact::Medium {
        assessment.impact = RiskImpact::Medium;
    }
    if parsed.segments.len() > 1 {
        assessment.impact = RiskImpact::High;
        assessment.reasons.push("unsplittable-compound");
    }
    insert_structural_reason(&mut assessment.reasons, "complex-shell-not-auto-executable");
}

fn max_interaction(
    left: InteractionRequirement,
    right: InteractionRequirement,
) -> InteractionRequirement {
    use InteractionRequirement::*;
    let rank = |interaction| match interaction {
        None => 0,
        TtyRequired => 1,
        CredentialPromptLikely => 2,
    };
    if rank(right) > rank(left) {
        right
    } else {
        left
    }
}

/// Returns `true` if the segment's first stage contains a shell builtin
/// or construct that mutates session state consumed by later segments:
/// `cd` (working directory), `export`/`unset` (environment), or a bare
/// env-assignment token (shell variable). These constructs require a real
/// shell interpreter and must never be routed through CompoundReadonlyExecutor.
fn segment_has_state_mutating_builtin(segment: &[Vec<String>]) -> bool {
    let Some(first_stage) = segment.first() else {
        return false;
    };
    let Some(program) = first_stage.first() else {
        return false;
    };
    // Shell builtins that mutate session state visible to subsequent segments.
    if matches!(program.as_str(), "cd" | "export" | "unset" | "source" | ".") {
        return true;
    }
    // A bare env-assignment (e.g. `FOO=bar`) in command position is a shell
    // variable assignment, not an env-prefix for a program invocation. The
    // parser records it as the first token of a Simple stage; detect it by
    // the presence of `=` in the program name (env-prefix assignments are
    // only valid immediately before a command, and the parser puts them
    // before the program token in the same stage).
    if program.contains('=') {
        return true;
    }
    false
}
