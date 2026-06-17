#[allow(dead_code)]
pub(crate) mod exit_classify;

#[allow(unused_imports)]
pub(crate) use exit_classify::{
    classify_executed_command_outcome, classify_exit, classify_shell_handoff_command_outcome,
    first_program_token, CommandOutcome, ExitCodeCategory,
};
