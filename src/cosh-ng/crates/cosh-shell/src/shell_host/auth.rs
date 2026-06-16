use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static MARKER_TOKEN_COUNTER: AtomicU64 = AtomicU64::new(1);

pub(super) fn generate_marker_token() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let counter = MARKER_TOKEN_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{:x}{:x}{:x}", std::process::id(), nanos, counter)
}

pub(super) fn marker_script_with_token(
    script: &str,
    token: &str,
    recovery_request_file: &str,
    handoff_request_file: &str,
) -> String {
    format!(
        "COSH_MARKER_TOKEN='{}'\nCOSH_RECOVERY_REQUEST_FILE='{}'\nCOSH_HANDOFF_REQUEST_FILE='{}'\n{}",
        shell_single_quote_value(token),
        shell_single_quote_value(recovery_request_file),
        shell_single_quote_value(handoff_request_file),
        script
    )
}

fn shell_single_quote_value(value: &str) -> String {
    value.replace('\'', "'\\''")
}
