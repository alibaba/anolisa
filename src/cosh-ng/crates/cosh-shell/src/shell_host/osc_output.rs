use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::types::COMMAND_OUTPUT_REF_MAX_BYTES;

#[cfg(test)]
pub(super) fn write_output_ref(dir: &Path, command_id: &str, output: &[u8]) -> io::Result<PathBuf> {
    Ok(
        write_output_ref_with_session_cap(dir, command_id, output, 0, usize::MAX)?
            .path
            .expect("unbounded session cap should capture output ref"),
    )
}

#[derive(Debug)]
pub(super) struct OutputRefCapture {
    pub(super) path: Option<PathBuf>,
    pub(super) captured_bytes: usize,
    pub(super) status: OutputRefCaptureStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OutputRefCaptureStatus {
    Captured,
    SessionCapReached,
}

pub(super) fn write_output_ref_with_session_cap(
    dir: &Path,
    command_id: &str,
    output: &[u8],
    session_captured_bytes: usize,
    session_cap_bytes: usize,
) -> io::Result<OutputRefCapture> {
    fs::create_dir_all(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    let captured = capped_output_ref_bytes(output, COMMAND_OUTPUT_REF_MAX_BYTES);
    if session_captured_bytes.saturating_add(captured.len()) > session_cap_bytes {
        return Ok(OutputRefCapture {
            path: None,
            captured_bytes: 0,
            status: OutputRefCaptureStatus::SessionCapReached,
        });
    }

    let path = dir.join(format!("{command_id}.txt"));
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&path)?;
    file.write_all(&captured)?;
    file.sync_all()?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    Ok(OutputRefCapture {
        path: Some(path),
        captured_bytes: captured.len(),
        status: OutputRefCaptureStatus::Captured,
    })
}

pub(super) fn capped_output_ref_bytes(output: &[u8], max_bytes: usize) -> Vec<u8> {
    if output.len() <= max_bytes {
        return output.to_vec();
    }

    let marker = format!(
        "\n[captured output truncated: original_bytes={}, max_capture_bytes={}]\n",
        output.len(),
        max_bytes
    )
    .into_bytes();
    if max_bytes <= marker.len() {
        return marker[..max_bytes].to_vec();
    }

    let available = max_bytes - marker.len();
    let head_len = utf8_floor_boundary(output, available / 2);
    let tail_len = available.saturating_sub(head_len);
    let tail_start = utf8_ceil_boundary(output, output.len().saturating_sub(tail_len));

    let mut captured = Vec::with_capacity(max_bytes);
    captured.extend_from_slice(&output[..head_len]);
    captured.extend_from_slice(&marker);
    captured.extend_from_slice(&output[tail_start..]);
    captured
}

fn utf8_floor_boundary(bytes: &[u8], mut index: usize) -> usize {
    index = index.min(bytes.len());
    while index > 0 && index < bytes.len() && is_utf8_continuation(bytes[index]) {
        index -= 1;
    }
    index
}

fn utf8_ceil_boundary(bytes: &[u8], mut index: usize) -> usize {
    index = index.min(bytes.len());
    while index < bytes.len() && is_utf8_continuation(bytes[index]) {
        index += 1;
    }
    index
}

fn is_utf8_continuation(byte: u8) -> bool {
    byte & 0b1100_0000 == 0b1000_0000
}
