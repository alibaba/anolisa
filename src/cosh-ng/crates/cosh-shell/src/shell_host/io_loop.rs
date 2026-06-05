use std::fs::File;
use std::io::{self, Read, Write};
use std::process::Child;
use std::thread;
use std::time::{Duration, Instant};

use super::osc::OscParser;

pub(super) fn read_until(
    master: &mut File,
    child: &mut Child,
    parser: &mut OscParser,
    timeout: Duration,
    condition: impl Fn(&OscParser) -> bool,
) -> io::Result<bool> {
    let deadline = Instant::now() + timeout;
    let mut buffer = [0_u8; 8192];

    while Instant::now() < deadline {
        loop {
            match master.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    parser.feed(&buffer[..n])?;
                    if condition(parser) {
                        return Ok(true);
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) if child.try_wait()?.is_some() => return Ok(condition(parser)),
                Err(err) => return Err(err),
            }
        }

        if child.try_wait()?.is_some() {
            return Ok(condition(parser));
        }
        thread::sleep(Duration::from_millis(10));
    }

    Ok(condition(parser))
}

pub(super) fn read_until_streaming<W: Write>(
    master: &mut File,
    child: &mut Child,
    parser: &mut OscParser,
    output: &mut W,
    timeout: Duration,
    condition: impl Fn(&OscParser) -> bool,
) -> io::Result<bool> {
    let deadline = Instant::now() + timeout;
    let mut buffer = [0_u8; 8192];
    let mut display_start = parser.display.len();

    while Instant::now() < deadline {
        loop {
            match master.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    parser.feed(&buffer[..n])?;
                    if parser.display.len() > display_start {
                        output.write_all(&parser.display[display_start..])?;
                        output.flush()?;
                        display_start = parser.display.len();
                    }
                    if condition(parser) {
                        return Ok(true);
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) if child.try_wait()?.is_some() => return Ok(condition(parser)),
                Err(err) => return Err(err),
            }
        }

        if child.try_wait()?.is_some() {
            return Ok(condition(parser));
        }
        thread::sleep(Duration::from_millis(10));
    }

    Ok(condition(parser))
}

pub(super) fn wait_child(child: &mut Child) -> io::Result<Option<i32>> {
    match child.try_wait()? {
        Some(status) => Ok(status.code()),
        None => Ok(child.wait()?.code()),
    }
}
