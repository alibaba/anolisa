//! Final PostTool candidate arbitration.

use tokenless_compressors::Recoverability;
use tokenless_protocol::{Disposition, estimate_tokens};

/// Inputs to the one final PostTool decision.
pub(super) struct ArbitrationInput<'a> {
    pub(super) original: &'a str,
    pub(super) candidate: &'a str,
    pub(super) has_operations: bool,
    pub(super) recoverability: Recoverability,
    pub(super) require_reversibility: bool,
    pub(super) dry_run: bool,
    pub(super) timed_out: bool,
}

/// Whether the candidate reaches the model or is measured/rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Verdict {
    Apply,
    DryRun,
    Reject(Disposition),
}

pub(super) fn decide(input: &ArbitrationInput<'_>) -> Verdict {
    if input.timed_out {
        return Verdict::Reject(Disposition::Timeout);
    }
    if !input.has_operations {
        return Verdict::Reject(Disposition::NoSavings);
    }
    if input.require_reversibility && input.recoverability == Recoverability::Unrecoverable {
        return Verdict::Reject(Disposition::RecoverabilityUnavailable);
    }
    let saves_chars = input.candidate.chars().count() < input.original.chars().count();
    let saves_tokens = estimate_tokens(input.candidate) < estimate_tokens(input.original);
    if !(saves_chars && saves_tokens) {
        return Verdict::Reject(Disposition::NoSavings);
    }
    if input.dry_run {
        Verdict::DryRun
    } else {
        Verdict::Apply
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input<'a>(original: &'a str, candidate: &'a str) -> ArbitrationInput<'a> {
        ArbitrationInput {
            original,
            candidate,
            has_operations: true,
            recoverability: Recoverability::Lossless,
            require_reversibility: false,
            dry_run: false,
            timed_out: false,
        }
    }

    #[test]
    fn candidate_must_save_both_characters_and_tokens() {
        assert_eq!(decide(&input("abcdefgh", "abc")), Verdict::Apply);
        assert_eq!(
            decide(&input("abcd", "你")),
            Verdict::Reject(Disposition::NoSavings)
        );
    }

    #[test]
    fn dry_run_and_reversibility_have_explicit_verdicts() {
        let mut case = input("abcdefgh", "abc");
        case.dry_run = true;
        assert_eq!(decide(&case), Verdict::DryRun);

        case.dry_run = false;
        case.require_reversibility = true;
        case.recoverability = Recoverability::Unrecoverable;
        assert_eq!(
            decide(&case),
            Verdict::Reject(Disposition::RecoverabilityUnavailable)
        );

        case.require_reversibility = false;
        case.timed_out = true;
        assert_eq!(decide(&case), Verdict::Reject(Disposition::Timeout));
    }
}
