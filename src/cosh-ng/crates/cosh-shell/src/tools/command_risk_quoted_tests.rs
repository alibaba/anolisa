//! Quoted SAFE_OUTPUT_SINK redirection coverage for issue #1752.
//!
//! Declared only from `lib.rs` (the `wrap_tests` pattern) so the cases
//! stay out of the lib/bin test overlap ratchet
//! (`scripts/check-test-inventory.sh`): the module is compiled for the
//! `--lib` target only, while `main.rs` does not declare it.

use crate::tools::command_risk::{
    assess_shell_command, AssessmentPolicy, AssessmentSource, AutoAllowEvidence, CommandAssessment,
    ExecutionDecision, RiskImpact,
};

fn auto(command: &str) -> CommandAssessment {
    assess_shell_command(
        command,
        AssessmentPolicy::auto_with_guarded_diagnostics(AssessmentSource::ProviderShellTool),
    )
}

fn ask(command: &str) -> CommandAssessment {
    assess_shell_command(
        command,
        AssessmentPolicy::ask(AssessmentSource::ProviderShellTool),
    )
}

#[test]
fn quoted_safe_output_sink_redirection_is_null_suppression() {
    // Issue #1752: an agent-emitted stderr suppression whose target word
    // is wholly quoted (`2>"/dev/null"`, `2>'/dev/null'`) undergoes quote
    // removal to the same literal sink path as the unquoted form — the
    // SAFE_OUTPUT_SINK entries contain no `$`, backtick or backslash, so
    // the quotes cannot introduce expansion — and must join the issue
    // #1667 null-suppression channel instead of the fail-closed
    // RedirectionWrite path.
    for command in [
        "ps aux 2>\"/dev/null\"",
        "ps aux 2>'/dev/null'",
        "cat x 2>>\"/dev/null\"",
        "cat x 2>>'/dev/null'",
        "du -sh /var 2> '/dev/null'",
        "ls >\"/dev/null\"",
        "ls 1>'/dev/null'",
        "find /tmp -maxdepth 3 -name '*cosh*' 2>\"/dev/null\"",
    ] {
        let assessment = ask(command);
        assert_ne!(assessment.impact, RiskImpact::High, "{command}");
        assert!(
            !assessment.reasons.contains(&"redirection-write"),
            "{command}: {:?}",
            assessment.reasons
        );
        assert!(
            assessment.reasons.contains(&"output-suppressed"),
            "{command}: {:?}",
            assessment.reasons
        );
    }

    // V-TOK, asserted at the assessment layer: `parse_command` and
    // `ParsedCommand` are `pub(super)`, unreachable from a lib-root
    // module, so the no-leak guarantee (the fd word and the quoted
    // target never enter argv; the sink is counted as a null
    // redirection) is pinned through its observable effects instead —
    // `output-suppressed` present proves the null-redirection count,
    // and any argv leak would fail the readonly re-check on the
    // stripped argv and drop the Layer 2 auto-allow asserted below.
    // Issue #1752 Layer 2: a wholly-quoted stderr-only sink joins the
    // unquoted stderr channel — the auto-allow boundary re-opens with
    // broker evidence rebuilt on the stripped argv.
    let auto_policy = auto("ps aux 2>\"/dev/null\"");
    assert_eq!(auto_policy.execution, ExecutionDecision::AutoAllow);
    assert_eq!(
        auto_policy.auto_allow,
        Some(AutoAllowEvidence::DirectReadonlyBroker)
    );
}

#[test]
fn quoted_non_sink_redirection_targets_stay_fail_closed() {
    // Issue #1752 narrows the issue #1667 V-M8 fail-closed rule only for
    // whole-word quoted SAFE_OUTPUT_SINK targets. Every other quoted
    // target keeps the RedirectionWrite classification: regular files,
    // expansion, and suffix concatenation (`'/dev/null'x` builds the
    // different word `/dev/nullx` in every shell).
    for command in [
        "cat log 2>\"/tmp/evil\"",
        "cat log 2>'/tmp/evil'",
        "cat log 2>\"$F\"",
        "cat log 2>$FILE",
        "cat log 2>'/dev/null'x",
        "cat log 2>'/dev/nul*'",
        "ls 2>' /dev/null'",
        "ls &>'/dev/null'",
    ] {
        let assessment = ask(command);
        assert_eq!(assessment.impact, RiskImpact::High, "{command}");
        assert!(
            assessment.reasons.contains(&"redirection-write"),
            "{command}: {:?}",
            assessment.reasons
        );
    }
}

#[test]
fn null_suppression_routes_the_auto_allow_boundary_by_channel() {
    // Issue #1752 Layer 2 decision table: suppression routes by the
    // channel it suppresses instead of a blanket boundary. Forms whose
    // null redirections only touch stderr (fd 2) keep the stdout
    // evidence chain observable — the transcript still carries what
    // the command printed — so the auto-allow boundary re-opens for
    // them, with broker evidence rebuilt on the parser's stripped argv
    // (never a silent auto-allow without evidence).
    for command in [
        "ps aux 2>/dev/null",
        "ps aux 2>>/dev/null",
        "ps aux 2> /dev/null",
        "ps aux 2>\"/dev/null\"",
        "ps aux 2>'/dev/null'",
        "find /tmp -maxdepth 3 -name '*cosh*' 2>/dev/null",
        "du -sh /var 2> /dev/null",
    ] {
        let auto_policy = auto(command);
        assert_eq!(
            auto_policy.execution,
            ExecutionDecision::AutoAllow,
            "{command}"
        );
        assert_eq!(
            auto_policy.auto_allow,
            Some(AutoAllowEvidence::DirectReadonlyBroker),
            "{command}"
        );
        assert!(
            auto_policy.reasons.contains(&"output-suppressed"),
            "{command}: {:?}",
            auto_policy.reasons
        );
    }

    // Forms that suppress stdout — the bare default, fd 1, or a mixed
    // stderr+stdout suppression — leave the transcript with nothing to
    // review, so the execution boundary stays AskUser.
    for command in [
        "ls >/dev/null",
        "ls>/dev/null",
        "ls 1>/dev/null",
        "ls 2>/dev/null >/dev/null",
    ] {
        let auto_policy = auto(command);
        assert_eq!(
            auto_policy.execution,
            ExecutionDecision::AskUser,
            "{command}"
        );
        assert!(auto_policy.auto_allow.is_none(), "{command}");
        assert!(
            auto_policy.reasons.contains(&"output-suppressed"),
            "{command}: {:?}",
            auto_policy.reasons
        );
    }

    // `&>/dev/null` merges stderr into stdout before discarding both —
    // the whole observable stream is gone — and keeps its fail-closed
    // RedirectionWrite behavior (decision table: stay as-is).
    let both_streams = ask("ls &>/dev/null");
    assert_eq!(both_streams.impact, RiskImpact::High);
    assert!(both_streams.reasons.contains(&"redirection-write"));

    // fd duplication (`2>&1`) is not null suppression and keeps its
    // existing assessment (decision table: stay as-is): not High, not
    // annotated `output-suppressed`, and still gated to AskUser under
    // an auto policy because the text broker gate rejects `>`.
    let dup = ask("ls 2>&1");
    assert_ne!(dup.impact, RiskImpact::High);
    assert!(!dup.reasons.contains(&"output-suppressed"));
    let dup_auto = auto("ls 2>&1");
    assert_eq!(dup_auto.execution, ExecutionDecision::AskUser);
    assert!(dup_auto.auto_allow.is_none());

    // Remaining command risk is never masked by the stderr channel:
    // a high-risk program under stderr suppression stays High.
    let delete = ask("rm -rf x 2>/dev/null");
    assert_eq!(delete.impact, RiskImpact::High);
    assert!(delete.reasons.contains(&"filesystem-delete"));
    assert!(delete.reasons.contains(&"output-suppressed"));
}
