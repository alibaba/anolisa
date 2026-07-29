#[allow(dead_code, unused_imports)]
#[path = "mod.rs"]
mod implementation;

pub use implementation::{RawInputCapture, RawObserverAction, RawRelayAction};

#[allow(unused_imports)]
pub(crate) use implementation::{
    foreground_process_group_for_fds, process_group_exists, redact_extension_setting_value,
    set_pty_winsize, signal_foreground_process_group, signal_process_group,
    signal_process_group_id, spawn_raw_action_relay, spawn_raw_input_relay, update_input_mode,
    update_locked_input_mode, write_all_pty, MainPromptGate, PromptDraftEditor, PromptGhostRoute,
    RawInputEvent, RawInputMode, UserPtyInputGeneration,
};
