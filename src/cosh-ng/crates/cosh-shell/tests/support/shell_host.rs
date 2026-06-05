use std::io::Read;
use std::path::Path;
use std::time::Duration;

use cosh_shell::journal::read_shell_events;
use cosh_shell::ledger::{build_command_blocks, LedgerOutput};
use cosh_shell::shell_host::ShellHostOutput;
use cosh_shell::types::CommandBlock;

pub(crate) fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

pub(crate) fn ledger_from_output(output: &ShellHostOutput) -> LedgerOutput {
    let replayed_events = read_shell_events(&output.journal_path).expect("journal events");
    let ledger = build_command_blocks(&replayed_events);
    assert!(ledger.errors.is_empty(), "{:?}", ledger.errors);
    ledger
}

pub(crate) fn assert_no_osc_marker(output: &[u8]) {
    assert!(!output
        .windows(b"\x1b]1337;COSH;".len())
        .any(|window| window == b"\x1b]1337;COSH;"));
}

pub(crate) fn assert_clean_shell_output_ref(block: &CommandBlock, expected: &str) {
    let output_ref = block
        .output
        .terminal_output_ref
        .as_deref()
        .expect("terminal output ref");
    let text = std::fs::read_to_string(output_ref).expect("output ref text");
    assert!(text.contains(expected), "{text:?}");
    assert!(!text.contains("\x1b[?2004"), "{text:?}");
    assert!(!text.contains('\u{0008}'), "{text:?}");
    assert!(!text.contains("\x1b[0m"), "{text:?}");
    assert!(!text.contains("\x1b[27m"), "{text:?}");
    assert!(!text.contains("\x1b[24m"), "{text:?}");
    assert!(!text.contains("\x1b[J"), "{text:?}");
    assert!(!text.contains("\x1b[K"), "{text:?}");
}

pub(crate) fn shell_arg(path: &Path) -> String {
    let value = path.display().to_string();
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(unix)]
pub(crate) fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = std::fs::metadata(path)
        .expect("tool metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).expect("tool permissions");
}

pub(crate) struct DelayedInput {
    chunks: Vec<(Vec<u8>, Duration)>,
    index: usize,
}

impl DelayedInput {
    pub(crate) fn new(chunks: Vec<(Vec<u8>, Duration)>) -> Self {
        Self { chunks, index: 0 }
    }
}

impl Read for DelayedInput {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let Some((chunk, delay)) = self.chunks.get(self.index) else {
            return Ok(0);
        };

        std::thread::sleep(*delay);
        let len = chunk.len().min(buf.len());
        buf[..len].copy_from_slice(&chunk[..len]);
        self.index += 1;
        Ok(len)
    }
}
