#[allow(dead_code, unused_imports)]
#[path = "mod.rs"]
mod implementation;

pub use implementation::{PromptGhostRoute, RawInputCapture, RawObserverAction, RawRelayAction};

pub(crate) use implementation::{
    set_pty_winsize, signal_foreground_process_group, signal_process_group, spawn_raw_action_relay,
    spawn_raw_input_relay, update_input_mode, write_all_pty, RawInputEvent, RawInputMode,
};
