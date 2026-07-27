use std::io::{self, Read};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::{current_raw_input_mode, InputRead, RawInputMode};

pub(super) fn read_input_chunks<R>(
    mut input: R,
    sender: SyncSender<InputRead>,
    input_mode: Arc<Mutex<RawInputMode>>,
) where
    R: Read,
{
    let mut buffer = [0_u8; 8192];
    let mut idle_backoff = Duration::from_millis(2);
    loop {
        let observed_mode = current_raw_input_mode(&input_mode);
        let read_result = input.read(&mut buffer);
        // Compare ownership boundaries instead of full modes: display-only
        // updates inside the same owner (e.g. prompt ghost candidate cycling)
        // must not mark bytes as crossing an ownership cutover.
        let ownership_changed_during_read = current_raw_input_mode(&input_mode).input_ownership()
            != observed_mode.input_ownership();
        let input = match read_result {
            Ok(0) => InputRead::Eof,
            Ok(count) => {
                idle_backoff = Duration::from_millis(2);
                InputRead::Bytes {
                    bytes: buffer[..count].to_vec(),
                    received_at: Instant::now(),
                    observed_mode,
                    ownership_changed_during_read,
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(idle_backoff);
                idle_backoff = (idle_backoff * 2).min(Duration::from_millis(32));
                continue;
            }
            Err(error) => InputRead::Error(error),
        };
        let done = !matches!(input, InputRead::Bytes { .. });
        if sender.send(input).is_err() || done {
            return;
        }
    }
}
