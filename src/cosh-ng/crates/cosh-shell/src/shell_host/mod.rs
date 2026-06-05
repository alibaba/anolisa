mod adapter;
mod auth;
mod bootstrap;
mod io_loop;
mod lifecycle;
mod marker;
mod model;
mod osc;
mod prompt_replay;
mod raw_relay;
mod raw_runner;
mod scripted;

pub use model::{ScriptedInput, ShellHostConfig, ShellHostOutput};
pub use raw_runner::{
    run_raw_interactive_bash, run_raw_interactive_bash_with_observer,
    run_raw_interactive_bash_with_output_control, run_raw_interactive_zsh_with_output_control,
    run_raw_relay_bash, run_raw_relay_bash_with_actions, run_raw_relay_bash_with_actions_observer,
    run_raw_relay_bash_with_actions_output_control, run_raw_relay_bash_with_observer,
    run_raw_relay_bash_with_output_control, run_raw_relay_zsh_with_actions,
    run_raw_relay_zsh_with_output_control,
};
pub use scripted::{run_scripted_bash, run_scripted_zsh, run_streaming_line_bash};
