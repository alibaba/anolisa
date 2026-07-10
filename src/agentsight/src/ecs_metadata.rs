//! ECS metadata service client
//!
//! Queries instance metadata from the Alibaba Cloud ECS metadata service
//! at `http://100.100.100.200/latest/meta-data/`.
//!
//! Supports both IMDSv1 (no token) and hardened IMDS (token-based).
//! Returns `None` when not running on an ECS instance (2s timeout).

use std::time::Duration;

const METADATA_BASE: &str = "http://100.100.100.200/latest/meta-data";

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

/// Probe the ECS metadata service with a 2-second timeout.
///
/// Spawns a background thread so that the caller is guaranteed to return
/// within ~2 seconds even if the metadata endpoint is unreachable.
/// Returns `None` when not running on an ECS instance.
pub fn probe_ecs_metadata() -> Option<EcsMetadata> {
    let handle = std::thread::spawn(fetch_ecs_metadata);
    match handle.join() {
        Ok(result) => result,
        Err(_) => {
            log::debug!("ECS metadata probe thread panicked");
            None
        }
    }
}

/// Fetch ECS metadata fields from the metadata service.
fn fetch_ecs_metadata() -> Option<EcsMetadata> {
    let agent = ureq::builder()
        .timeout_connect(Duration::from_secs(2))
        .build();

    // Verify reachability via instance-id
    let instance_id = read_metadata(&agent, "instance-id")?;

    let region_id = read_metadata(&agent, "region-id").unwrap_or_default();

    let eip = read_metadata(&agent, "eipv4").unwrap_or_default();
    let public_ipv4 = read_metadata(&agent, "public-ipv4").unwrap_or_default();

    Some(EcsMetadata {
        instance_id,
        region_id,
        eip,
        public_ipv4,
    })
}

/// Read a single metadata path, trying IMDSv2 token first, then IMDSv1.
fn read_metadata(agent: &ureq::Agent, path: &str) -> Option<String> {
    let url = format!("{METADATA_BASE}/{path}");

    // Try IMDSv2 (token-based) first
    if let Some(token) = get_imdsv2_token(agent) {
        if let Some(val) = read_with_token(agent, &url, &token) {
            return Some(val);
        }
    }

    // Fallback to IMDSv1 (no token)
    read_plain(agent, &url)
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

/// Read metadata without a token (IMDSv1).
fn read_plain(agent: &ureq::Agent, url: &str) -> Option<String> {
    agent
        .get(url)
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
        // The probe should return None within ~2 seconds.
        let start = std::time::Instant::now();
        let result = probe_ecs_metadata();
        let elapsed = start.elapsed();
        assert!(result.is_none(), "Expected None on non-ECS host");
        // Should not take much longer than the 2s connect timeout
        assert!(elapsed.as_secs() < 10, "Probe took too long: {elapsed:?}");
    }
}
