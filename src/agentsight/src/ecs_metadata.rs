//! ECS metadata service client
//!
//! Shared primitives for querying the Alibaba Cloud ECS metadata service
//! at `http://100.100.100.200/latest/meta-data/`.
//!
//! Provides [`metadata_agent()`] and [`read_plain()`] used by both
//! `genai::instance_id` and the dashboard ECS probe.
//!
//! Supports both IMDSv1 (no token) and hardened IMDS (token-based).

use std::sync::mpsc;
use std::time::Duration;

/// Base URL for the ECS metadata service.
pub(crate) const METADATA_BASE: &str = "http://100.100.100.200/latest/meta-data";

/// Hard deadline for the entire metadata probe (all HTTP requests combined).
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Per-request timeout for each individual HTTP call (connect + read).
const REQUEST_TIMEOUT: Duration = Duration::from_millis(800);

/// Build a ureq agent with explicit connect + overall request timeouts.
///
/// Shared by [`read_plain`] and the dashboard ECS probe.
pub(crate) fn metadata_agent(timeout: Duration) -> ureq::Agent {
    ureq::builder()
        .timeout_connect(timeout)
        .timeout(timeout)
        .build()
}

/// Read a single metadata path without IMDSv2 authentication (IMDSv1).
///
/// `path` is relative to [`METADATA_BASE`], e.g. `"instance-id"`.
/// Returns `None` when the endpoint is unreachable or returns empty.
pub(crate) fn read_plain(agent: &ureq::Agent, path: &str) -> Option<String> {
    let url = format!("{METADATA_BASE}/{path}");
    agent
        .get(&url)
        .call()
        .ok()
        .and_then(|r| r.into_string().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// ECS instance metadata collected from the metadata service.
#[derive(Debug, Clone)]
pub struct EcsMetadata {
    /// ECS instance ID, e.g. `i-bp1xxx`
    pub instance_id: String,
    /// Region ID, e.g. `cn-hangzhou`
    pub region_id: String,
    /// Elastic IP address (empty if no EIP bound)
    pub eip: String,
    /// Public IPv4 address (empty if no public IP)
    pub public_ipv4: String,
}

impl EcsMetadata {
    /// Returns the public IP address, preferring EIP over public-ipv4.
    pub fn public_ip(&self) -> Option<&str> {
        if !self.eip.is_empty() {
            Some(&self.eip)
        } else if !self.public_ipv4.is_empty() {
            Some(&self.public_ipv4)
        } else {
            None
        }
    }

    /// Returns the ECS console URL for the instance detail page.
    pub fn instance_url(&self) -> String {
        format!(
            "https://ecs.console.aliyun.com/server/{}/detail?regionId={}",
            self.instance_id, self.region_id
        )
    }
}

/// Probe the ECS metadata service with a hard 2-second deadline.
///
/// Spawns a background thread that performs all HTTP requests, then uses
/// `mpsc::recv_timeout` to enforce the deadline.  If the thread does not
/// finish within [`PROBE_TIMEOUT`], the caller returns `None` immediately
/// (the background thread is abandoned and will be cleaned up on process exit).
///
/// Returns `None` when not running on an ECS instance.
pub fn probe_ecs_metadata() -> Option<EcsMetadata> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(fetch_ecs_metadata());
    });
    match rx.recv_timeout(PROBE_TIMEOUT) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            log::debug!("ECS metadata probe timed out after {PROBE_TIMEOUT:?}");
            None
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            log::debug!("ECS metadata probe thread panicked");
            None
        }
    }
}

/// Fetch ECS metadata fields from the metadata service.
fn fetch_ecs_metadata() -> Option<EcsMetadata> {
    let agent = metadata_agent(REQUEST_TIMEOUT);

    // Obtain IMDSv2 token once (None on non-ECS or when IMDSv2 is unavailable)
    let token = get_imdsv2_token(&agent);

    // Verify reachability via instance-id
    let instance_id = read_metadata(&agent, "instance-id", token.as_deref())?;

    let region_id = read_metadata(&agent, "region-id", token.as_deref()).unwrap_or_default();

    let eip = read_metadata(&agent, "eipv4", token.as_deref()).unwrap_or_default();
    let public_ipv4 = read_metadata(&agent, "public-ipv4", token.as_deref()).unwrap_or_default();

    Some(EcsMetadata {
        instance_id,
        region_id,
        eip,
        public_ipv4,
    })
}

/// Read a single metadata path, using the pre-fetched token if available.
fn read_metadata(agent: &ureq::Agent, path: &str, token: Option<&str>) -> Option<String> {
    // Try IMDSv2 (token-based) first if we have a token
    if let Some(t) = token {
        let url = format!("{METADATA_BASE}/{path}");
        if let Some(val) = read_with_token(agent, &url, t) {
            return Some(val);
        }
    }

    // Fallback to IMDSv1 (no token)
    read_plain(agent, path)
}

/// Attempt to read metadata with an IMDSv2 session token.
fn read_with_token(agent: &ureq::Agent, url: &str, token: &str) -> Option<String> {
    agent
        .get(url)
        .set("X-Forwarded-For", "China")
        .set("X-Alibaba-Cloud-Ecs-Metadata-Token", token)
        .call()
        .ok()
        .and_then(|r| r.into_string().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

const IMDS_TOKEN_URL: &str = "http://100.100.100.200/latest/api/token";

/// Obtain an IMDSv2 session token via PUT request.
fn get_imdsv2_token(agent: &ureq::Agent) -> Option<String> {
    agent
        .put(IMDS_TOKEN_URL)
        .set("X-Alibaba-Cloud-Ecs-Metadata-Token-Ttl-Seconds", "300")
        .call()
        .ok()
        .and_then(|r| r.into_string().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_metadata() -> EcsMetadata {
        EcsMetadata {
            instance_id: "i-bp1abcdef".to_string(),
            region_id: "cn-hangzhou".to_string(),
            eip: "47.98.1.2".to_string(),
            public_ipv4: String::new(),
        }
    }

    #[test]
    fn public_ip_prefers_eip_over_public_ipv4() {
        let meta = EcsMetadata {
            eip: "1.2.3.4".to_string(),
            public_ipv4: "5.6.7.8".to_string(),
            ..sample_metadata()
        };
        assert_eq!(meta.public_ip(), Some("1.2.3.4"));
    }

    #[test]
    fn public_ip_falls_back_to_public_ipv4() {
        let meta = EcsMetadata {
            eip: String::new(),
            public_ipv4: "5.6.7.8".to_string(),
            ..sample_metadata()
        };
        assert_eq!(meta.public_ip(), Some("5.6.7.8"));
    }

    #[test]
    fn public_ip_returns_none_when_both_empty() {
        let meta = EcsMetadata {
            eip: String::new(),
            public_ipv4: String::new(),
            ..sample_metadata()
        };
        assert_eq!(meta.public_ip(), None);
    }

    #[test]
    fn instance_url_contains_region_and_id() {
        let meta = sample_metadata();
        let url = meta.instance_url();
        assert!(url.contains("cn-hangzhou"));
        assert!(url.contains("i-bp1abcdef"));
        assert!(url.starts_with("https://ecs.console.aliyun.com/"));
    }

    #[test]
    fn probe_returns_none_when_not_on_ecs() {
        // On non-ECS machines the metadata endpoint is unreachable.
        // The probe should return None within the PROBE_TIMEOUT (2s).
        let start = std::time::Instant::now();
        let result = probe_ecs_metadata();
        let elapsed = start.elapsed();
        assert!(result.is_none(), "Expected None on non-ECS host");
        // recv_timeout guarantees we don't exceed PROBE_TIMEOUT by much
        assert!(
            elapsed < PROBE_TIMEOUT + Duration::from_secs(1),
            "Probe took too long: {elapsed:?}"
        );
    }
}
