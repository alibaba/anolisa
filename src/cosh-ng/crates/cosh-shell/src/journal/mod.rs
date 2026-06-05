use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::Path;

use crate::types::ShellEvent;

pub fn write_shell_events(path: impl AsRef<Path>, events: &[ShellEvent]) -> io::Result<()> {
    let file = File::create(path)?;
    let mut writer = BufWriter::new(file);
    for event in events {
        serde_json::to_writer(&mut writer, event).map_err(json_to_io)?;
        writer.write_all(b"\n")?;
    }
    writer.flush()
}

pub fn read_shell_events(path: impl AsRef<Path>) -> io::Result<Vec<ShellEvent>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut events = Vec::new();

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        events.push(serde_json::from_str(&line).map_err(json_to_io)?);
    }

    Ok(events)
}

fn json_to_io(err: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err)
}
