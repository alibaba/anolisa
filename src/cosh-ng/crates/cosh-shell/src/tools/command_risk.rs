use super::broker::can_run_approved_bash_tool;
use super::command_risk_build::{
    assessment, dedupe_reasons, high_risk_program, high_risk_program_assessment, high_shell_syntax,
    max_output_exposure, max_output_stability, min_confidence,
};
use super::command_risk_parser::{is_env_assignment, parse_command, ParsedCommand};
use super::guarded_diagnostic::validate_guarded_diagnostic;
use super::is_sensitive_target;
use super::readonly_pipeline::validate_readonly_pipeline;

pub use super::command_risk_model::{
    AssessmentConfidence, AssessmentPolicy, AssessmentSource, AssessmentSummary, AutoAllowEvidence,
    AutoExecutionPolicy, AutoExecutionRoute, CommandAssessment, CommandShape, ExecutionDecision,
    InteractionRequirement, OutputExposure, OutputStability, ReadonlyEvidence, RiskImpact,
    RiskReason, SideEffectClass,
};

pub fn assess_shell_command(command: &str, policy: AssessmentPolicy) -> CommandAssessment {
    let command = command.trim();
    let parsed = parse_command(command);
    if command.is_empty() {
        return assessment(
            policy.source,
            command,
            parsed.shape,
            ExecutionDecision::AskUser,
            RiskImpact::Medium,
            AssessmentConfidence::Low,
            InteractionRequirement::None,
            OutputStability::StableSnapshot,
            OutputExposure::Normal,
            vec![SideEffectClass::Unknown],
            vec!["empty-command"],
            None,
        );
    }
    if command.contains('\0') {
        return assessment(
            policy.source,
            command,
            parsed.shape,
            ExecutionDecision::Block,
            RiskImpact::High,
            AssessmentConfidence::High,
            InteractionRequirement::None,
            OutputStability::StableSnapshot,
            OutputExposure::Normal,
            vec![SideEffectClass::Unknown],
            vec!["unsafe-binding"],
            None,
        );
    }
    if parsed.shape == CommandShape::Unparseable {
        return assessment(
            policy.source,
            command,
            parsed.shape,
            ExecutionDecision::AskUser,
            RiskImpact::High,
            AssessmentConfidence::Low,
            InteractionRequirement::None,
            OutputStability::StableSnapshot,
            OutputExposure::Normal,
            vec![SideEffectClass::Unknown],
            vec!["parse-failed"],
            None,
        );
    }
    if parsed.shape == CommandShape::CommandSubstitution {
        return high_shell_syntax(policy.source, command, parsed.shape, "command-substitution");
    }
    if parsed.shape == CommandShape::RedirectionWrite {
        return high_shell_syntax(policy.source, command, parsed.shape, "redirection-write");
    }

    match parsed.shape {
        CommandShape::Simple | CommandShape::EnvSimple => {
            assess_simple_command(command, parsed, policy)
        }
        CommandShape::Pipeline => assess_pipeline(command, parsed, policy),
        CommandShape::AndOrList | CommandShape::Sequence | CommandShape::RedirectionRead => {
            let mut simple = assess_first_stage(command, &parsed, policy);
            simple.shape = parsed.shape;
            simple.execution = ExecutionDecision::AskUser;
            simple.confidence = min_confidence(simple.confidence, AssessmentConfidence::Medium);
            simple.reasons.push(match parsed.shape {
                CommandShape::AndOrList => "and-or-list-not-auto-executable",
                CommandShape::Sequence => "sequence-not-auto-executable",
                CommandShape::RedirectionRead => "read-redirection-not-auto-executable",
                _ => "complex-shell-not-auto-executable",
            });
            simple
        }
        CommandShape::Complex => {
            let mut simple = assess_first_stage(command, &parsed, policy);
            simple.shape = parsed.shape;
            simple.execution = ExecutionDecision::AskUser;
            simple.confidence = AssessmentConfidence::Low;
            if simple.impact < RiskImpact::Medium {
                simple.impact = RiskImpact::Medium;
            }
            simple.reasons.push("complex-shell-not-auto-executable");
            simple
        }
        CommandShape::Empty
        | CommandShape::Unparseable
        | CommandShape::CommandSubstitution
        | CommandShape::RedirectionWrite => unreachable!("handled above"),
    }
}

pub fn blocked_shell_binding_assessment(
    source: AssessmentSource,
    command: &str,
    reason: &'static str,
) -> CommandAssessment {
    assessment(
        source,
        command.trim(),
        CommandShape::Unparseable,
        ExecutionDecision::Block,
        RiskImpact::High,
        AssessmentConfidence::High,
        InteractionRequirement::None,
        OutputStability::StableSnapshot,
        OutputExposure::Normal,
        vec![SideEffectClass::Unknown],
        vec![reason],
        None,
    )
}

fn assess_simple_command(
    command: &str,
    parsed: ParsedCommand,
    policy: AssessmentPolicy,
) -> CommandAssessment {
    let tokens = parsed.stages.first().cloned().unwrap_or_default();
    let program_index = tokens
        .iter()
        .position(|token| !is_env_assignment(token))
        .unwrap_or(0);
    let command_tokens = &tokens[program_index..];
    let Some(program) = command_tokens
        .first()
        .map(|token| basename(token).to_string())
    else {
        return assessment(
            policy.source,
            command,
            parsed.shape,
            ExecutionDecision::AskUser,
            RiskImpact::Medium,
            AssessmentConfidence::Low,
            InteractionRequirement::None,
            OutputStability::StableSnapshot,
            OutputExposure::Normal,
            vec![SideEffectClass::Unknown],
            vec!["empty-command"],
            None,
        );
    };
    let sensitive = command_tokens
        .iter()
        .any(|token| is_sensitive_target(token));
    if sensitive {
        return assessment(
            policy.source,
            command,
            parsed.shape,
            ExecutionDecision::AskUser,
            RiskImpact::High,
            AssessmentConfidence::High,
            InteractionRequirement::None,
            OutputStability::StableSnapshot,
            OutputExposure::MayContainSecrets,
            vec![SideEffectClass::SensitiveDataRead],
            vec!["sensitive-path"],
            None,
        );
    }

    if let Some(high) = high_risk_program_assessment(policy.source, command, parsed.shape, &program)
    {
        return high;
    }

    let mut stage = stage_assessment(&program, command_tokens);
    if let Some(readonly) = direct_readonly_evidence(command) {
        stage.impact = RiskImpact::Low;
        stage.confidence = AssessmentConfidence::High;
        stage.reasons.insert(0, readonly.reason_code());
        return finalize_simple(
            policy,
            command,
            parsed.shape,
            stage,
            Some(readonly.auto_allow()),
        );
    }

    if is_safe_diagnostic_family(&program) {
        let guarded_evidence =
            policy.guarded_diagnostic_executor && validate_guarded_diagnostic(command).is_ok();
        stage.impact = if policy.auto_mode && guarded_evidence {
            RiskImpact::Low
        } else {
            RiskImpact::Medium
        };
        stage.confidence = AssessmentConfidence::High;
        stage.reasons.insert(0, "safe-diagnostic-family");
        return finalize_simple(
            policy,
            command,
            parsed.shape,
            stage,
            (policy.auto_mode && guarded_evidence).then_some(AutoAllowEvidence::GuardedDiagnostic),
        );
    }

    finalize_simple(policy, command, parsed.shape, stage, None)
}

fn direct_readonly_evidence(command: &str) -> Option<ReadonlyEvidence> {
    can_run_approved_bash_tool(command)
        .is_ok()
        .then_some(ReadonlyEvidence::DirectReadonlyBroker)
}

fn assess_pipeline(
    command: &str,
    parsed: ParsedCommand,
    policy: AssessmentPolicy,
) -> CommandAssessment {
    let mut impact = RiskImpact::Low;
    let mut confidence = AssessmentConfidence::High;
    let mut output_stability = OutputStability::StableSnapshot;
    let mut output_exposure = OutputExposure::Normal;
    let mut side_effects = Vec::new();
    let mut reasons = Vec::new();
    let mut any_unknown = false;
    let mut all_diagnostic = true;
    let mut has_network_producer = false;
    let mut has_shell_consumer = false;

    for stage_tokens in &parsed.stages {
        let program = stage_tokens
            .iter()
            .position(|token| !is_env_assignment(token))
            .and_then(|idx| stage_tokens.get(idx))
            .map(|token| basename(token).to_string());
        let Some(program) = program else {
            any_unknown = true;
            all_diagnostic = false;
            continue;
        };
        if stage_tokens.iter().any(|token| is_sensitive_target(token)) {
            impact = RiskImpact::High;
            output_exposure = OutputExposure::MayContainSecrets;
            side_effects.push(SideEffectClass::SensitiveDataRead);
            reasons.push("sensitive-path");
            all_diagnostic = false;
            continue;
        }
        if let Some(high) = high_risk_program(&program) {
            impact = RiskImpact::High;
            side_effects.push(high.0);
            reasons.push(high.1);
            all_diagnostic = false;
            continue;
        }
        if matches!(program.as_str(), "curl" | "wget") {
            has_network_producer = true;
        }
        if matches!(program.as_str(), "sh" | "bash" | "zsh" | "fish") {
            has_shell_consumer = true;
        }
        let stage = stage_assessment(&program, stage_tokens);
        impact = impact.max(stage.impact);
        confidence = min_confidence(confidence, stage.confidence);
        output_stability = max_output_stability(output_stability, stage.output_stability);
        output_exposure = max_output_exposure(output_exposure, stage.output_exposure);
        side_effects.extend(stage.side_effects);
        if !is_diagnostic_pipeline_stage(&program) {
            all_diagnostic = false;
        }
        if stage.reasons.contains(&"unknown-command") {
            any_unknown = true;
        }
    }

    let readonly_pipeline_evidence =
        policy.readonly_pipeline_executor && validate_readonly_pipeline(command).is_ok();

    if has_network_producer && has_shell_consumer {
        impact = RiskImpact::High;
        confidence = AssessmentConfidence::High;
        side_effects.push(SideEffectClass::RemoteCodeExecution);
        reasons.insert(0, "remote-code-execution");
    } else if readonly_pipeline_evidence {
        impact = RiskImpact::Low;
        confidence = AssessmentConfidence::High;
        reasons.insert(0, "readonly-pipeline-executor");
    } else if impact == RiskImpact::High {
        if reasons.is_empty() {
            reasons.push("pipeline-high-impact-stage");
        }
    } else if all_diagnostic || looks_like_diagnostic_pipeline(command) {
        impact = RiskImpact::Medium;
        confidence = min_confidence(confidence, AssessmentConfidence::Medium);
        reasons.insert(0, "diagnostic-pipeline-heuristic");
    } else {
        impact = RiskImpact::Medium;
        confidence = min_confidence(confidence, AssessmentConfidence::Medium);
        reasons.insert(0, "pipeline-not-auto-executable");
    }
    if any_unknown {
        confidence = min_confidence(confidence, AssessmentConfidence::Medium);
        reasons.push("unknown-stage");
    }
    reasons.push("pipeline-not-auto-executable");
    if side_effects.is_empty() {
        side_effects.push(SideEffectClass::None);
    }

    let auto_allow = if policy.auto_mode && readonly_pipeline_evidence && impact == RiskImpact::Low
    {
        Some(AutoAllowEvidence::ReadonlyPipelineExecutor)
    } else {
        None
    };
    let execution = if auto_allow.is_some() {
        ExecutionDecision::AutoAllow
    } else {
        ExecutionDecision::AskUser
    };

    assessment(
        policy.source,
        command,
        CommandShape::Pipeline,
        execution,
        impact,
        confidence,
        InteractionRequirement::None,
        output_stability,
        output_exposure,
        side_effects,
        dedupe_reasons(reasons),
        auto_allow,
    )
}

fn assess_first_stage(
    command: &str,
    parsed: &ParsedCommand,
    policy: AssessmentPolicy,
) -> CommandAssessment {
    let simple = ParsedCommand {
        shape: if parsed.shape == CommandShape::EnvSimple {
            CommandShape::EnvSimple
        } else {
            CommandShape::Simple
        },
        stages: parsed.stages.first().cloned().into_iter().collect(),
    };
    assess_simple_command(command, simple, policy)
}

fn finalize_simple(
    policy: AssessmentPolicy,
    command: &str,
    shape: CommandShape,
    stage: StageAssessment,
    evidence: Option<AutoAllowEvidence>,
) -> CommandAssessment {
    let auto_allow = evidence.filter(|_| policy.auto_mode);
    let execution = if auto_allow.is_some() {
        ExecutionDecision::AutoAllow
    } else if stage.interaction == InteractionRequirement::TtyRequired {
        ExecutionDecision::ForegroundHandoffRequired
    } else {
        ExecutionDecision::AskUser
    };
    assessment(
        policy.source,
        command,
        shape,
        execution,
        stage.impact,
        stage.confidence,
        stage.interaction,
        stage.output_stability,
        stage.output_exposure,
        stage.side_effects,
        dedupe_reasons(stage.reasons),
        auto_allow,
    )
}

#[derive(Debug, Clone)]
struct StageAssessment {
    impact: RiskImpact,
    confidence: AssessmentConfidence,
    interaction: InteractionRequirement,
    output_stability: OutputStability,
    output_exposure: OutputExposure,
    side_effects: Vec<SideEffectClass>,
    reasons: Vec<&'static str>,
}

fn stage_assessment(program: &str, tokens: &[String]) -> StageAssessment {
    if matches!(
        program,
        "less" | "more" | "man" | "htop" | "ssh" | "scp" | "sftp"
    ) || matches!(program, "python" | "python3" | "node" | "irb" | "ruby")
        && !has_eval_arg(tokens)
        || matches!(program, "docker" | "podman" | "kubectl") && has_tty_arg(tokens)
    {
        return StageAssessment {
            impact: RiskImpact::Medium,
            confidence: AssessmentConfidence::High,
            interaction: InteractionRequirement::TtyRequired,
            output_stability: OutputStability::UnstableInteractive,
            output_exposure: OutputExposure::Normal,
            side_effects: vec![SideEffectClass::None],
            reasons: vec!["requires-tty"],
        };
    }
    if program == "top" {
        return StageAssessment {
            impact: RiskImpact::Medium,
            confidence: AssessmentConfidence::High,
            interaction: if top_is_batch_snapshot(tokens) {
                InteractionRequirement::None
            } else {
                InteractionRequirement::TtyRequired
            },
            output_stability: if top_is_batch_snapshot(tokens) {
                OutputStability::StableSnapshot
            } else {
                OutputStability::Streaming
            },
            output_exposure: OutputExposure::MayContainCommandLine,
            side_effects: vec![SideEffectClass::None],
            reasons: vec!["streaming-diagnostic"],
        };
    }
    if program == "awk" {
        let high = tokens.iter().any(|token| {
            token.contains("system(") || token.contains("getline") || token.contains('>')
        });
        return StageAssessment {
            impact: if high {
                RiskImpact::High
            } else {
                RiskImpact::Medium
            },
            confidence: AssessmentConfidence::Medium,
            interaction: InteractionRequirement::None,
            output_stability: OutputStability::PotentiallyLarge,
            output_exposure: OutputExposure::Normal,
            side_effects: if high {
                vec![SideEffectClass::RemoteCodeExecution]
            } else {
                vec![SideEffectClass::None]
            },
            reasons: vec![if high {
                "awk-shell-execution"
            } else {
                "awk-not-auto-allowlisted"
            }],
        };
    }
    if matches!(program, "curl" | "wget") {
        return StageAssessment {
            impact: RiskImpact::Medium,
            confidence: AssessmentConfidence::Medium,
            interaction: InteractionRequirement::None,
            output_stability: OutputStability::PotentiallyLarge,
            output_exposure: OutputExposure::Normal,
            side_effects: vec![SideEffectClass::NetworkRead],
            reasons: vec!["network-read"],
        };
    }
    if matches!(program, "cargo" | "npm" | "make") {
        return StageAssessment {
            impact: RiskImpact::Medium,
            confidence: AssessmentConfidence::Medium,
            interaction: InteractionRequirement::None,
            output_stability: OutputStability::PotentiallyLarge,
            output_exposure: OutputExposure::Normal,
            side_effects: vec![SideEffectClass::Unknown],
            reasons: vec!["build-or-test-command"],
        };
    }
    if matches!(program, "df" | "ps") {
        return StageAssessment {
            impact: RiskImpact::Medium,
            confidence: AssessmentConfidence::High,
            interaction: InteractionRequirement::None,
            output_stability: OutputStability::StableSnapshot,
            output_exposure: if program == "ps" {
                OutputExposure::MayContainCommandLine
            } else {
                OutputExposure::Normal
            },
            side_effects: vec![SideEffectClass::None],
            reasons: vec!["safe-diagnostic-family"],
        };
    }
    if matches!(program, "grep" | "rg" | "find" | "head" | "tail" | "cat")
        && tokens.iter().any(|token| is_secret_search_token(token))
    {
        return StageAssessment {
            impact: RiskImpact::High,
            confidence: AssessmentConfidence::High,
            interaction: InteractionRequirement::None,
            output_stability: OutputStability::StableSnapshot,
            output_exposure: OutputExposure::MayContainSecrets,
            side_effects: vec![SideEffectClass::SensitiveDataRead],
            reasons: vec!["sensitive-search"],
        };
    }
    if matches!(
        program,
        "grep" | "rg" | "head" | "tail" | "sort" | "uniq" | "cut" | "wc"
    ) {
        return StageAssessment {
            impact: RiskImpact::Low,
            confidence: AssessmentConfidence::Medium,
            interaction: InteractionRequirement::None,
            output_stability: if program == "tail" {
                OutputStability::PotentiallyLarge
            } else {
                OutputStability::StableSnapshot
            },
            output_exposure: OutputExposure::Normal,
            side_effects: vec![SideEffectClass::None],
            reasons: vec!["readonly-pipeline-stage"],
        };
    }
    if matches!(program, "docker" | "podman" | "kubectl") {
        return assess_container_or_cluster(program, tokens);
    }

    StageAssessment {
        impact: RiskImpact::Medium,
        confidence: AssessmentConfidence::Low,
        interaction: InteractionRequirement::None,
        output_stability: OutputStability::StableSnapshot,
        output_exposure: OutputExposure::Normal,
        side_effects: vec![SideEffectClass::Unknown],
        reasons: vec!["unknown-command"],
    }
}

fn assess_container_or_cluster(program: &str, tokens: &[String]) -> StageAssessment {
    let read_subcommands = if matches!(program, "kubectl") {
        &["get", "describe", "logs"][..]
    } else {
        &["ps", "images", "inspect", "logs"][..]
    };
    let write_subcommands = if matches!(program, "kubectl") {
        &["apply", "delete", "exec", "scale", "patch"][..]
    } else {
        &["run", "rm", "stop", "exec", "kill"][..]
    };
    let subcommand = tokens.get(1).map(String::as_str).unwrap_or("");
    if write_subcommands.contains(&subcommand) {
        return StageAssessment {
            impact: RiskImpact::High,
            confidence: AssessmentConfidence::High,
            interaction: if has_tty_arg(tokens) {
                InteractionRequirement::TtyRequired
            } else {
                InteractionRequirement::None
            },
            output_stability: OutputStability::PotentiallyLarge,
            output_exposure: OutputExposure::Normal,
            side_effects: vec![SideEffectClass::ServiceControl],
            reasons: vec!["service-or-container-control"],
        };
    }
    StageAssessment {
        impact: RiskImpact::Medium,
        confidence: if read_subcommands.contains(&subcommand) {
            AssessmentConfidence::High
        } else {
            AssessmentConfidence::Medium
        },
        interaction: InteractionRequirement::None,
        output_stability: OutputStability::PotentiallyLarge,
        output_exposure: OutputExposure::Normal,
        side_effects: vec![SideEffectClass::NetworkRead],
        reasons: vec!["cluster-or-container-read"],
    }
}

fn basename(program: &str) -> &str {
    program
        .rsplit_once('/')
        .map(|(_, name)| name)
        .unwrap_or(program)
}

fn has_eval_arg(tokens: &[String]) -> bool {
    tokens
        .iter()
        .skip(1)
        .any(|arg| matches!(arg.as_str(), "-c" | "-e" | "--eval" | "--command"))
}

fn has_tty_arg(tokens: &[String]) -> bool {
    tokens.iter().any(|arg| {
        matches!(
            arg.as_str(),
            "-it" | "-ti" | "-i" | "-t" | "--interactive" | "--tty"
        ) || arg.starts_with("--interactive=")
            || arg.starts_with("--tty=")
    })
}

fn top_is_batch_snapshot(tokens: &[String]) -> bool {
    tokens.iter().any(|arg| arg == "-b" || arg == "-l")
}

fn is_safe_diagnostic_family(program: &str) -> bool {
    matches!(program, "df" | "ps" | "top")
}

fn is_diagnostic_pipeline_stage(program: &str) -> bool {
    matches!(
        program,
        "df" | "ps" | "top" | "grep" | "rg" | "head" | "tail" | "sort" | "uniq" | "cut" | "wc"
    )
}

fn looks_like_diagnostic_pipeline(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    (lower.contains("ps ") || lower.starts_with("ps") || lower.contains("df "))
        && (lower.contains("| head") || lower.contains("| grep") || lower.contains("| sort"))
}

fn is_secret_search_token(token: &str) -> bool {
    matches!(
        token.to_ascii_lowercase().as_str(),
        "token" | "secret" | "password" | "credential" | "apikey" | "api_key"
    )
}
