//! Quoted SAFE_OUTPUT_SINK redirection coverage for issue #1752.
//!
//! Declared only from `lib.rs` (the `wrap_tests` pattern) so the cases
//! stay out of the lib/bin test overlap ratchet
//! (`scripts/check-test-inventory.sh`): the module is compiled for the
//! `--lib` target only, while `main.rs` does not declare it.

use crate::tools::command_risk::{
    assess_shell_command, AssessmentPolicy, AssessmentSource, CommandAssessment, ExecutionDecision,
    RiskImpact,
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
    // and any argv leak would leave a plain auto-allowable command and
    // flip the V-M10 boundary assertion below.
    let auto_policy = auto("ps aux 2>\"/dev/null\"");
    assert_eq!(auto_policy.execution, ExecutionDecision::AskUser);
    assert!(auto_policy.auto_allow.is_none());
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
