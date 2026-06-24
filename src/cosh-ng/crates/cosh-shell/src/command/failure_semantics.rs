use crate::types::{CommandBlock, CommandOrigin, ShellEvent, ShellEventKind};

use super::{classify_exit, first_program_token, ExitCodeCategory};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FailureSemantics {
    pub(crate) class: FailureClass,
    pub(crate) confidence: FailureConfidence,
    pub(crate) reasons: Vec<FailureReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailureClass {
    Success,
    ExpectedNoResult,
    UsageOrHelp,
    InteractiveCancel,
    UserInterrupt,
    PipelineNormal,
    CommandNotFound,
    PermissionDenied,
    AbnormalSignal,
    BuildOrTestFailure,
    GenericRuntimeFailure,
    ProviderOrInternalArtifact,
    UnknownFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailureConfidence {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FailureReason {
    ExitCode(i32),
    Program(String),
    SignalLikeExit,
    CommandSpecificExpectedNonZero,
    UsageLine,
    HelpSuggestion,
    OptionParseError,
    MissingArgument,
    BuildFailureOutput,
    TestFailureOutput,
    CommandNotFound,
    PermissionDenied,
    InteractiveCancelOutput,
    UserInterruptEvent,
    OutputUnavailable,
}

pub(crate) fn classify_failure(
    block: &CommandBlock,
    events: &[ShellEvent],
    output_excerpt: Option<&str>,
) -> FailureSemantics {
    let mut reasons = vec![
        FailureReason::ExitCode(block.exit_code),
        FailureReason::Program(first_program_token(&block.command).to_string()),
    ];

    if block.exit_code == 0 {
        return semantics(FailureClass::Success, FailureConfidence::High, reasons);
    }

    if command_has_user_interrupt_event(events, block) {
        reasons.push(FailureReason::UserInterruptEvent);
        return semantics(
            FailureClass::UserInterrupt,
            FailureConfidence::High,
            reasons,
        );
    }

    if matches!(
        block.origin,
        CommandOrigin::ShellInternal | CommandOrigin::ProviderTool
    ) {
        return semantics(
            FailureClass::ProviderOrInternalArtifact,
            FailureConfidence::Medium,
            reasons,
        );
    }

    let category = classify_exit(block.exit_code, &block.command);
    match category {
        ExitCodeCategory::Success => {
            return semantics(FailureClass::Success, FailureConfidence::High, reasons);
        }
        ExitCodeCategory::UserInterrupt => {
            reasons.push(FailureReason::SignalLikeExit);
            return semantics(
                FailureClass::UserInterrupt,
                FailureConfidence::High,
                reasons,
            );
        }
        ExitCodeCategory::PipelineNormal => {
            return semantics(
                FailureClass::PipelineNormal,
                FailureConfidence::High,
                reasons,
            );
        }
        ExitCodeCategory::CommandSpecificNormal => {
            reasons.push(FailureReason::CommandSpecificExpectedNonZero);
            return semantics(
                FailureClass::ExpectedNoResult,
                FailureConfidence::High,
                reasons,
            );
        }
        ExitCodeCategory::CommandNotFound => {
            reasons.push(FailureReason::CommandNotFound);
            return semantics(
                FailureClass::CommandNotFound,
                FailureConfidence::High,
                reasons,
            );
        }
        ExitCodeCategory::PermissionDenied => {
            reasons.push(FailureReason::PermissionDenied);
            return semantics(
                FailureClass::PermissionDenied,
                FailureConfidence::High,
                reasons,
            );
        }
        ExitCodeCategory::AbnormalSignal => {
            reasons.push(FailureReason::SignalLikeExit);
            return semantics(
                FailureClass::AbnormalSignal,
                FailureConfidence::High,
                reasons,
            );
        }
        ExitCodeCategory::GenericError => {}
    }

    let Some(output) = output_excerpt else {
        reasons.push(FailureReason::OutputUnavailable);
        return semantics(
            FailureClass::UnknownFailure,
            FailureConfidence::Low,
            reasons,
        );
    };
    let normalized = normalize_output(output);
    if interactive_cancel_output(&normalized) {
        reasons.push(FailureReason::InteractiveCancelOutput);
        return semantics(
            FailureClass::InteractiveCancel,
            FailureConfidence::High,
            reasons,
        );
    }

    let real_failure = real_failure_signals(&normalized);
    let usage = usage_help_signals(&normalized, block.exit_code);
    if real_failure.high_confidence && !usage.explicit_option_parse_error() {
        reasons.extend(real_failure.reasons);
        return semantics(
            FailureClass::BuildOrTestFailure,
            FailureConfidence::High,
            reasons,
        );
    }

    if usage.high_confidence() {
        reasons.extend(usage.reasons);
        return semantics(FailureClass::UsageOrHelp, FailureConfidence::High, reasons);
    }
    if usage.medium_confidence() {
        reasons.extend(usage.reasons);
        return semantics(
            FailureClass::UsageOrHelp,
            FailureConfidence::Medium,
            reasons,
        );
    }

    if real_failure.high_confidence {
        reasons.extend(real_failure.reasons);
        return semantics(
            FailureClass::BuildOrTestFailure,
            FailureConfidence::High,
            reasons,
        );
    }

    semantics(
        FailureClass::GenericRuntimeFailure,
        FailureConfidence::Medium,
        reasons,
    )
}

fn semantics(
    class: FailureClass,
    confidence: FailureConfidence,
    reasons: Vec<FailureReason>,
) -> FailureSemantics {
    FailureSemantics {
        class,
        confidence,
        reasons,
    }
}

fn command_has_user_interrupt_event(events: &[ShellEvent], block: &CommandBlock) -> bool {
    events.iter().any(|event| {
        event.kind == ShellEventKind::UserInputIntercepted
            && event.component.as_deref() == Some("control")
            && event.input.as_deref() == Some("ctrl_c")
            && event.started_at_ms.is_some_and(|timestamp| {
                timestamp >= block.started_at_ms && timestamp <= block.ended_at_ms
            })
    })
}

fn normalize_output(output: &str) -> String {
    strip_ansi(output).to_ascii_lowercase()
}

fn strip_ansi(output: &str) -> String {
    let mut cleaned = String::with_capacity(output.len());
    let mut chars = output.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        cleaned.push(ch);
    }
    cleaned
}

#[derive(Default)]
struct UsageHelpSignals {
    usage_line: bool,
    help_suggestion: bool,
    option_parse_error: bool,
    missing_argument: bool,
    option_list_ratio_high: bool,
    reasons: Vec<FailureReason>,
}

impl UsageHelpSignals {
    fn explicit_option_parse_error(&self) -> bool {
        self.option_parse_error || self.missing_argument
    }

    fn high_confidence(&self) -> bool {
        (self.usage_line
            && (self.option_parse_error || self.missing_argument || self.help_suggestion))
            || (self.option_parse_error && self.help_suggestion)
            || self.option_list_ratio_high
    }

    fn medium_confidence(&self) -> bool {
        self.usage_line || self.option_parse_error || self.missing_argument
    }
}

fn usage_help_signals(output: &str, exit_code: i32) -> UsageHelpSignals {
    let mut signals = UsageHelpSignals::default();
    let lines = output.lines().filter(|line| !line.trim().is_empty());
    let mut total_lines = 0usize;
    let mut option_lines = 0usize;

    for line in lines {
        total_lines += 1;
        let trimmed = line.trim_start();
        if trimmed.starts_with("usage:") {
            signals.usage_line = true;
            push_reason_once(&mut signals.reasons, FailureReason::UsageLine);
        }
        if line.contains("try ") && line.contains("--help")
            || line.contains("for more information") && line.contains("--help")
            || line.contains("run ") && line.contains("--help")
        {
            signals.help_suggestion = true;
            push_reason_once(&mut signals.reasons, FailureReason::HelpSuggestion);
        }
        if [
            "unexpected argument",
            "unrecognized option",
            "unknown option",
            "no such option",
            "unrecognized arguments:",
            "unknown flag:",
            "unknown shorthand flag:",
            "flag provided but not defined",
            "invalid choice:",
        ]
        .iter()
        .any(|needle| line.contains(needle))
        {
            signals.option_parse_error = true;
            push_reason_once(&mut signals.reasons, FailureReason::OptionParseError);
        }
        if [
            "missing required",
            "missing argument",
            "required arguments were not provided",
        ]
        .iter()
        .any(|needle| line.contains(needle))
        {
            signals.missing_argument = true;
            push_reason_once(&mut signals.reasons, FailureReason::MissingArgument);
        }
        if is_option_list_line(trimmed) {
            option_lines += 1;
        }
    }

    if matches!(exit_code, 1 | 2 | 64)
        && total_lines >= 5
        && option_lines >= 3
        && option_lines * 10 >= total_lines * 3
    {
        signals.option_list_ratio_high = true;
        push_reason_once(&mut signals.reasons, FailureReason::UsageLine);
    }

    signals
}

fn is_option_list_line(line: &str) -> bool {
    line.starts_with("--")
        || line
            .strip_prefix('-')
            .and_then(|rest| rest.chars().next())
            .is_some_and(|ch| ch.is_ascii_alphanumeric())
}

#[derive(Default)]
struct RealFailureSignals {
    high_confidence: bool,
    reasons: Vec<FailureReason>,
}

fn real_failure_signals(output: &str) -> RealFailureSignals {
    let mut signals = RealFailureSignals::default();
    for line in output.lines() {
        if line.contains("test result: failed")
            || line.trim() == "failures:"
            || line.contains("failed tests")
            || line.contains("pytest") && line.contains("failed")
        {
            signals.high_confidence = true;
            push_reason_once(&mut signals.reasons, FailureReason::TestFailureOutput);
        }
        if line.contains("compilation failed")
            || line.contains("error[e")
            || line.contains("npm err!")
            || line.contains("make: ***")
            || line.contains("traceback")
            || line.contains("panic")
            || line.contains("segmentation fault")
            || line.contains("core dumped")
        {
            signals.high_confidence = true;
            push_reason_once(&mut signals.reasons, FailureReason::BuildFailureOutput);
        }
    }
    signals
}

fn interactive_cancel_output(output: &str) -> bool {
    [
        "sudo: a password is required",
        "a password is required",
        "a terminal is required",
        "operation cancelled",
        "operation canceled",
        "keyboardinterrupt",
        "interrupted",
        "access key id",
        "access key secret",
        "default region id",
    ]
    .iter()
    .any(|needle| output.contains(needle))
}

fn push_reason_once(reasons: &mut Vec<FailureReason>, reason: FailureReason) {
    if !reasons.contains(&reason) {
        reasons.push(reason);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CommandStatus, OutputRefs};

    fn block(exit_code: i32, command: &str) -> CommandBlock {
        CommandBlock {
            id: "cmd-1".to_string(),
            session_id: "session".to_string(),
            command: command.to_string(),
            origin: CommandOrigin::UserInteractive,
            cwd: "/tmp".to_string(),
            end_cwd: "/tmp".to_string(),
            started_at_ms: 100,
            ended_at_ms: 200,
            duration_ms: 100,
            exit_code,
            status: if exit_code == 0 {
                CommandStatus::Completed
            } else {
                CommandStatus::Failed
            },
            output: OutputRefs {
                terminal_output_ref: None,
                terminal_output_bytes: 0,
            },
        }
    }

    fn class(exit_code: i32, command: &str, output: Option<&str>) -> FailureClass {
        classify_failure(&block(exit_code, command), &[], output).class
    }

    #[test]
    fn usage_help_exit_two_is_not_generic_failure() {
        assert_eq!(
            class(
                2,
                "demo --bad",
                Some("error: unexpected argument '--bad'\nUsage: demo [OPTIONS]\n")
            ),
            FailureClass::UsageOrHelp
        );
    }

    #[test]
    fn exit_two_without_output_is_unknown() {
        assert_eq!(class(2, "demo --bad", None), FailureClass::UnknownFailure);
    }

    #[test]
    fn real_test_failure_with_usage_footer_stays_test_failure() {
        assert_eq!(
            class(
                2,
                "fake-test",
                Some("test result: FAILED. 1 failed\nUsage: fake-test [OPTIONS]\n")
            ),
            FailureClass::BuildOrTestFailure
        );
    }

    #[test]
    fn explicit_parse_error_wins_over_generic_failure_words() {
        assert_eq!(
            class(
                2,
                "cargo test --bad-flag",
                Some("error: unexpected argument '--bad-flag'\nUsage: cargo test [OPTIONS]\n")
            ),
            FailureClass::UsageOrHelp
        );
    }

    #[test]
    fn expected_nonzero_commands_are_classified_as_no_result() {
        assert_eq!(
            class(1, "grep missing file.txt", Some("")),
            FailureClass::ExpectedNoResult
        );
        assert_eq!(
            class(1, "diff a b", Some("1c1\n< a\n---\n> b\n")),
            FailureClass::ExpectedNoResult
        );
    }

    #[test]
    fn reserved_shell_failures_remain_actionable() {
        assert_eq!(class(127, "nope", Some("")), FailureClass::CommandNotFound);
        assert_eq!(
            class(126, "./script", Some("")),
            FailureClass::PermissionDenied
        );
    }
}
