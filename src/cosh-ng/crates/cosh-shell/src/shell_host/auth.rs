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

pub(super) fn marker_script_with_token(script: &str, token: &str) -> String {
    format!("COSH_MARKER_TOKEN='{token}'\n{script}")
}
