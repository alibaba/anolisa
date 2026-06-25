//! ECS environment detection and STS credential retrieval.
//!
//! Uses `ureq` (synchronous HTTP) to query the ECS metadata service at
//! `http://100.100.100.200`. All functions are blocking and safe to call
//! from the cosh-shell synchronous event loop.

use std::time::Duration;

/// ECS metadata service endpoint (link-local address available only on ECS instances).
const ECS_METADATA_ENDPOINT: &str = "http://100.100.100.200";

/// Fixed RAM Role name used by SysOM.
pub(crate) const ECS_RAM_ROLE_NAME: &str = "AliyunECSInstanceForSysomRole";

/// Console URL template for ECS authorization.
const CONSOLE_URL_TEMPLATE: &str =
    "https://alinux.console.aliyun.com/{regionId}/guide/cosh?instance={instanceId}";

/// Polling interval for RAM Role authorization check.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Maximum number of poll attempts (2s * 100 = 200s timeout).
const MAX_POLL_COUNT: u32 = 100;

/// STS credentials obtained from ECS RAM Role.
#[derive(Debug, Clone)]
pub(crate) struct StsCredentials {
    pub access_key_id: String,
    pub access_key_secret: String,
    pub security_token: String,
}

/// Build a ureq agent with short timeouts for metadata service queries.
fn metadata_agent(connect_timeout_ms: u64, read_timeout_ms: u64) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_millis(connect_timeout_ms))
        .timeout_read(Duration::from_millis(read_timeout_ms))
        .build()
}

/// Detect if running on an ECS instance by querying the metadata service.
/// Returns the instance ID if on ECS, None otherwise.
/// Uses a very short timeout (500ms connect, 1s read) to fail fast on non-ECS.
pub(crate) fn detect_ecs_instance() -> Option<String> {
    let agent = metadata_agent(500, 1000);
    let resp = agent
        .get(&format!(
            "{}/latest/meta-data/instance-id",
            ECS_METADATA_ENDPOINT
        ))
        .call()
        .ok()?;
    let id = resp.into_string().ok()?.trim().to_string();
    if id.is_empty() {
        None
    } else {
        Some(id)
    }
}

/// Get the ECS region ID by querying zone-id and stripping the AZ suffix.
/// e.g. "cn-hangzhou-j" → "cn-hangzhou"
pub(crate) fn get_ecs_region_id() -> Option<String> {
    let agent = metadata_agent(1000, 2000);
    let resp = agent
        .get(&format!(
            "{}/latest/meta-data/zone-id",
            ECS_METADATA_ENDPOINT
        ))
        .call()
        .ok()?;
    let zone_id = resp.into_string().ok()?.trim().to_string();
    if zone_id.is_empty() {
        return None;
    }
    // Strip trailing AZ letter: "cn-hangzhou-j" → "cn-hangzhou"
    let region_id = if let Some(pos) = zone_id.rfind('-') {
        let suffix = &zone_id[pos + 1..];
        if suffix.len() == 1 && suffix.chars().all(|c| c.is_ascii_lowercase()) {
            zone_id[..pos].to_string()
        } else {
            zone_id
        }
    } else {
        zone_id
    };

    if region_id.is_empty() {
        None
    } else {
        Some(region_id)
    }
}

/// Generate the Aliyun console URL for ECS authorization.
pub(crate) fn generate_console_url(instance_id: &str, region_id: Option<&str>) -> String {
    let region = region_id.unwrap_or("cn-hangzhou");
    CONSOLE_URL_TEMPLATE
        .replace("{regionId}", region)
        .replace("{instanceId}", instance_id)
}

/// Check if the ECS instance has been granted the RAM Role.
pub(crate) fn check_ram_role_authorized() -> bool {
    let agent = metadata_agent(2000, 3000);
    let resp = agent
        .get(&format!(
            "{}/latest/meta-data/ram/security-credentials/{}",
            ECS_METADATA_ENDPOINT, ECS_RAM_ROLE_NAME
        ))
        .call();
    match resp {
        Ok(r) => {
            let body = r.into_string().unwrap_or_default();
            body.contains("AccessKeyId")
        }
        Err(_) => false,
    }
}

/// Get STS credentials from the ECS RAM Role metadata endpoint.
pub(crate) fn get_sts_credentials() -> Option<StsCredentials> {
    let agent = metadata_agent(2000, 5000);
    let resp = agent
        .get(&format!(
            "{}/latest/meta-data/ram/security-credentials/{}",
            ECS_METADATA_ENDPOINT, ECS_RAM_ROLE_NAME
        ))
        .call()
        .ok()?;
    let json: serde_json::Value = resp.into_json().ok()?;
    let ak = json["AccessKeyId"].as_str()?;
    let sk = json["AccessKeySecret"].as_str()?;
    let token = json["SecurityToken"].as_str()?;
    Some(StsCredentials {
        access_key_id: ak.to_string(),
        access_key_secret: sk.to_string(),
        security_token: token.to_string(),
    })
}

/// Poll for ECS RAM Role authorization. Blocks the calling thread.
/// Returns true if authorized within the timeout, false otherwise.
pub(crate) fn poll_for_authorization() -> bool {
    for _ in 0..MAX_POLL_COUNT {
        if check_ram_role_authorized() {
            return true;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    false
}

/// Result of an ECS detection + authorization background task.
#[derive(Debug)]
pub(crate) enum EcsTaskResult {
    /// Not on ECS — fall back to manual AK/SK input.
    NotOnEcs,
    /// On ECS, authorization succeeded — credentials ready.
    Authorized(StsCredentials),
    /// On ECS, but authorization timed out or failed.
    AuthorizationFailed(String),
}

/// ECS info discovered during detection (passed to polling phase).
#[derive(Debug, Clone)]
pub(crate) struct EcsInfo {
    pub instance_id: String,
    pub console_url: String,
}

/// Run ECS detection (blocking). Returns instance info if on ECS.
pub(crate) fn detect_ecs_environment() -> Option<EcsInfo> {
    let instance_id = detect_ecs_instance()?;
    let region_id = get_ecs_region_id();
    let console_url = generate_console_url(&instance_id, region_id.as_deref());
    Some(EcsInfo {
        instance_id,
        console_url,
    })
}

/// Run the ECS authorization polling flow (blocking): poll → get credentials.
/// Called from a background thread AFTER ECS has already been detected.
pub(crate) fn poll_and_get_credentials() -> EcsTaskResult {
    if poll_for_authorization() {
        match get_sts_credentials() {
            Some(creds) => EcsTaskResult::Authorized(creds),
            None => EcsTaskResult::AuthorizationFailed("Failed to get STS credentials".into()),
        }
    } else {
        EcsTaskResult::AuthorizationFailed(
            "Timeout waiting for RAM Role authorization".into(),
        )
    }
}

/// Run full ECS authorization flow (blocking): detect → poll → get credentials.
pub(crate) fn run_ecs_authorization() -> EcsTaskResult {
    match detect_ecs_environment() {
        None => EcsTaskResult::NotOnEcs,
        Some(_info) => {
            if poll_for_authorization() {
                match get_sts_credentials() {
                    Some(creds) => EcsTaskResult::Authorized(creds),
                    None => {
                        EcsTaskResult::AuthorizationFailed("Failed to get STS credentials".into())
                    }
                }
            } else {
                EcsTaskResult::AuthorizationFailed(
                    "Timeout waiting for RAM Role authorization".into(),
                )
            }
        }
    }
}
