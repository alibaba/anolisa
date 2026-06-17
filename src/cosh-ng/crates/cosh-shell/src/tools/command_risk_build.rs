use super::command_risk::{
    AssessmentConfidence, AssessmentSource, AutoAllowEvidence, CommandAssessment, CommandShape,
    ExecutionDecision, InteractionRequirement, OutputExposure, OutputStability, RiskImpact,
    SideEffectClass,
};

pub(super) fn high_risk_program_assessment(
    source: AssessmentSource,
    command: &str,
    shape: CommandShape,
    program: &str,
) -> Option<CommandAssessment> {
    let (side_effect, reason, interaction) = high_risk_program(program)?;
    Some(assessment(
        source,
        command,
        shape,
        ExecutionDecision::AskUser,
        RiskImpact::High,
        AssessmentConfidence::High,
        interaction,
        OutputStability::StableSnapshot,
        if side_effect == SideEffectClass::CredentialAccess {
            OutputExposure::MayContainSecrets
        } else {
            OutputExposure::Normal
        },
        vec![side_effect],
        vec![reason],
        None,
    ))
}

pub(super) fn high_risk_program(
    program: &str,
) -> Option<(SideEffectClass, &'static str, InteractionRequirement)> {
    match program {
        "sudo" | "su" => Some((
            SideEffectClass::PrivilegeEscalation,
            "privilege-escalation",
            InteractionRequirement::CredentialPromptLikely,
        )),
        "passwd" => Some((
            SideEffectClass::CredentialAccess,
            "credential-access",
            InteractionRequirement::CredentialPromptLikely,
        )),
        "vim" | "vi" | "nvim" | "nano" | "emacs" => Some((
            SideEffectClass::FilesystemWrite,
            "interactive-editor",
            InteractionRequirement::TtyRequired,
        )),
        "rm" | "rmdir" => Some((
            SideEffectClass::FilesystemDelete,
            "filesystem-delete",
            InteractionRequirement::None,
        )),
        "mv" | "dd" => Some((
            SideEffectClass::FilesystemWrite,
            "filesystem-write",
            InteractionRequirement::None,
        )),
        "chmod" | "chown" => Some((
            SideEffectClass::PermissionChange,
            "permission-change",
            InteractionRequirement::None,
        )),
        "kill" | "pkill" | "killall" => Some((
            SideEffectClass::ProcessControl,
            "process-control",
            InteractionRequirement::None,
        )),
        "brew" | "apt" | "apt-get" | "dnf" | "yum" => Some((
            SideEffectClass::PackageInstall,
            "package-manager-mutation",
            InteractionRequirement::None,
        )),
        "systemctl" | "launchctl" | "service" => Some((
            SideEffectClass::ServiceControl,
            "service-control",
            InteractionRequirement::None,
        )),
        _ => None,
    }
}

pub(super) fn high_shell_syntax(
    source: AssessmentSource,
    command: &str,
    shape: CommandShape,
    reason: &'static str,
) -> CommandAssessment {
    assessment(
        source,
        command,
        shape,
        ExecutionDecision::AskUser,
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

pub(super) fn assessment(
    source: AssessmentSource,
    command: &str,
    shape: CommandShape,
    execution: ExecutionDecision,
    impact: RiskImpact,
    confidence: AssessmentConfidence,
    interaction: InteractionRequirement,
    output_stability: OutputStability,
    output_exposure: OutputExposure,
    side_effects: Vec<SideEffectClass>,
    reasons: Vec<&'static str>,
    auto_allow: Option<AutoAllowEvidence>,
) -> CommandAssessment {
    CommandAssessment {
        source,
        command: command.to_string(),
        shape,
        execution,
        impact,
        confidence,
        interaction,
        output_stability,
        output_exposure,
        side_effects,
        reasons,
        auto_allow,
    }
}

pub(super) fn min_confidence(
    left: AssessmentConfidence,
    right: AssessmentConfidence,
) -> AssessmentConfidence {
    use AssessmentConfidence::*;
    match (left, right) {
        (Low, _) | (_, Low) => Low,
        (Medium, _) | (_, Medium) => Medium,
        (High, High) => High,
    }
}

pub(super) fn max_output_stability(
    left: OutputStability,
    right: OutputStability,
) -> OutputStability {
    use OutputStability::*;
    let rank = |stability| match stability {
        StableSnapshot => 0,
        PotentiallyLarge => 1,
        Streaming => 2,
        UnstableInteractive => 3,
    };
    if rank(right) > rank(left) {
        right
    } else {
        left
    }
}

pub(super) fn max_output_exposure(left: OutputExposure, right: OutputExposure) -> OutputExposure {
    use OutputExposure::*;
    let rank = |exposure| match exposure {
        Normal => 0,
        MayContainCommandLine => 1,
        MayContainEnvironment => 2,
        MayContainSecrets => 3,
    };
    if rank(right) > rank(left) {
        right
    } else {
        left
    }
}

pub(super) fn dedupe_reasons(reasons: Vec<&'static str>) -> Vec<&'static str> {
    let mut out = Vec::new();
    for reason in reasons {
        if !out.contains(&reason) {
            out.push(reason);
        }
    }
    out
}
