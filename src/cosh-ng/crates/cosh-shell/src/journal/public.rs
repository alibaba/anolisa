#[allow(dead_code)]
#[path = "mod.rs"]
mod implementation;

pub use implementation::read_shell_events;

pub(crate) use implementation::write_shell_events;
