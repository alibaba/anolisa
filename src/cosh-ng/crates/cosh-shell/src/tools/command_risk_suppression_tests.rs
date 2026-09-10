//! Issue #1752 Layer 2 acceptance matrix: which stripped output
//! suppressions restore the auto-allow decision.
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

/// The suppression is classified, not treated as a write, and the reason
/// stays visible for audit whichever boundary the channel lands on.
fn assert_suppressed(assessment: &CommandAssessment, command: &str) {
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

#[test]
fn stderr_only_suppression_restores_auto_allow() {
    // Issue #1752 decision: suppressing stderr does not change the
    // command's observable output, so the transcript and the audit trail
    // stay complete and the shape verdict survives. Covers the accepted
    // stderr forms — `2>`, `2>>`, the spaced and whole-word quoted
    // variants, an auxiliary fd, and a Tab-separated form (AGENTS.md
    // requires the unspaced/Tab variants for auto-approve gates).
    for command in [
        "find /tmp -maxdepth 3 -name cosh 2>/dev/null",
        "ls 2>/dev/null",
        "ps aux 2>/dev/null",
        "cat x 2>>/dev/null",
        "du -sh /var 2> /dev/null",
        "ps aux 2>\"/dev/null\"",
        "ps aux 2>'/dev/null'",
        "du -sh /var 2> '/dev/null'",
        "ls\t2>/dev/null",
        "ls 10>/dev/null",
    ] {
        let assessment = auto(command);
        assert_suppressed(&assessment, command);
        assert_eq!(
            assessment.execution,
            ExecutionDecision::AutoAllow,
            "{command}: {:?}",
            assessment.reasons
        );
        // An auto-approved run must carry its evidence: routing keys off
        // `auto_allow`, never off the verdict alone.
        assert!(assessment.auto_allow.is_some(), "{command}");
    }
}

#[test]
fn stdout_suppression_keeps_ask_user() {
    // Issue #1752 decision: a discarded stdout leaves nothing reviewable
    // in the transcript, so the default-fd and fd-1 forms keep the
    // pre-existing AskUser boundary and clear the evidence.
    for command in [
        "ls >/dev/null",
        "ls>/dev/null",
        "ls 1>/dev/null",
        "ls > /dev/null",
        "cat x >/dev/null",
        "ls >\"/dev/null\"",
        "ls 1>'/dev/null'",
        "ls\t>/dev/null",
    ] {
        let assessment = auto(command);
        assert_suppressed(&assessment, command);
        assert_eq!(
            assessment.execution,
            ExecutionDecision::AskUser,
            "{command}: {:?}",
            assessment.reasons
        );
        assert!(assessment.auto_allow.is_none(), "{command}");
    }
}

#[test]
fn mixed_suppression_follows_the_stdout_rule() {
    // Issue #1752 acceptance: `2>/dev/null >/dev/null` is routed by the
    // stdout rule, whatever the order of the two suppressions.
    for command in ["ls 2>/dev/null >/dev/null", "ls >/dev/null 2>/dev/null"] {
        let assessment = auto(command);
        assert_suppressed(&assessment, command);
        assert_eq!(
            assessment.execution,
            ExecutionDecision::AskUser,
            "{command}: {:?}",
            assessment.reasons
        );
        assert!(assessment.auto_allow.is_none(), "{command}");
    }
}

#[test]
fn close_and_duplicate_forms_keep_their_existing_boundaries() {
    // V-F5 (issue #2054): `[N]>&-` closes are never auto-allowed, and one
    // close form anywhere in the command holds the AskUser boundary even
    // next to a stderr-only sink.
    for command in ["ls 2>&-", "ls >&-", "ls 1>&-", "ls 2>&- 2>/dev/null"] {
        let assessment = auto(command);
        assert_suppressed(&assessment, command);
        assert_eq!(
            assessment.execution,
            ExecutionDecision::AskUser,
            "{command}: {:?}",
            assessment.reasons
        );
        assert!(assessment.auto_allow.is_none(), "{command}");
    }

    // `2>&1` is descriptor routing, not suppression: with no suppression
    // recorded the policy never runs, and the unstripped `>&` text keeps
    // the command off the readonly broker path.
    let dup_only = auto("ls 2>&1");
    assert_eq!(dup_only.execution, ExecutionDecision::AskUser);
    assert!(dup_only.auto_allow.is_none());

    // A duplication followed by a stderr-only sink is still stderr-only.
    let dup_then_sink = auto("ls 2>&1 2>/dev/null");
    assert_suppressed(&dup_then_sink, "ls 2>&1 2>/dev/null");
    assert_eq!(dup_then_sink.execution, ExecutionDecision::AutoAllow);
    assert!(dup_then_sink.auto_allow.is_some());
}

#[test]
fn suppression_never_masks_the_remaining_command_risk() {
    // Issue #1752 acceptance: risk stays decided by the real shape, and
    // the `&>` / `2>&1`-onto-file forms keep the byte-exact fail-closed
    // RedirectionWrite path.
    let delete = auto("rm -rf /tmp/x 2>/dev/null");
    assert_eq!(delete.impact, RiskImpact::High);
    assert!(delete.reasons.contains(&"filesystem-delete"));
    assert!(delete.reasons.contains(&"output-suppressed"));
    assert_eq!(delete.execution, ExecutionDecision::AskUser);
    assert!(delete.auto_allow.is_none());

    for command in ["ls &>/dev/null", "ls &>/dev/null 2>/dev/null"] {
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
fn quoted_argument_suppression_still_asks_at_the_readonly_broker() {
    // The issue's original reproduction carries a quoted glob
    // (`-name '*cosh*'`). Layer 2 classifies its `2>/dev/null` as
    // stderr-only and stops downgrading the verdict, but the readonly
    // broker rejects any quote in the text it is asked to auto-approve
    // (`broker.rs`: `is_shell_meta`), so the command still asks. That
    // gate is deliberate — the broker execs without a shell, so a quoted
    // word would reach argv verbatim — and relaxing it is a separate
    // decision from this one. Pinned here so the two layers cannot be
    // confused when the suppression policy changes again.
    let quoted_glob = auto("find /tmp -maxdepth 3 -name '*cosh*' 2>/dev/null");
    assert_suppressed(
        &quoted_glob,
        "find /tmp -maxdepth 3 -name '*cosh*' 2>/dev/null",
    );
    assert_eq!(quoted_glob.execution, ExecutionDecision::AskUser);
    assert!(quoted_glob.auto_allow.is_none());

    // The same search without quotes is auto-approved, which isolates the
    // broker's quote rule as the remaining gate.
    let bare_glob = auto("find /tmp -maxdepth 3 -name cosh 2>/dev/null");
    assert_eq!(bare_glob.execution, ExecutionDecision::AutoAllow);
    assert!(bare_glob.auto_allow.is_some());
}
