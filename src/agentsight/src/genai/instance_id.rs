//! Instance ID resolution utility
//!
//! Provides a shared function to resolve the current machine's instance ID,
//! used by both SLS PutLogs uploader and Logtail file exporter.
//!
//! Both `get_instance_id()` and `get_owner_account_id()` are cached via
//! `OnceLock` — the metadata HTTP call is only made once per process lifetime.

use std::sync::OnceLock;
use std::time::Duration;

/// ECS metadata 请求超时（连接 + 读取均为 1 秒）
const METADATA_TIMEOUT: Duration = Duration::from_secs(1);

/// 全局缓存：owner-account-id
static OWNER_ACCOUNT_ID: OnceLock<String> = OnceLock::new();
/// 全局缓存：instance-id
static INSTANCE_ID: OnceLock<String> = OnceLock::new();

/// 构建带有显式 connect timeout 的 ureq agent，避免非 ECS 环境 TCP SYN 重试卡死
fn metadata_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(METADATA_TIMEOUT)
        .timeout(METADATA_TIMEOUT)
        .build()
}

/// 获取 owner account ID（带缓存）：首次调用请求阿里云 ECS metadata（超时1秒），
/// 失败返回空字符串。后续调用直接返回缓存值。
pub fn get_owner_account_id() -> &'static str {
    OWNER_ACCOUNT_ID.get_or_init(|| fetch_owner_account_id())
}

/// 实际请求 owner-account-id
fn fetch_owner_account_id() -> String {
    let agent = metadata_agent();
    match agent
        .get("http://100.100.100.200/latest/meta-data/owner-account-id")
        .call()
    {
        Ok(resp) => {
            if let Ok(body) = resp.into_string() {
                let uid = body.trim().to_string();
                if !uid.is_empty() {
                    log::info!("Got ECS owner-account-id: {}", uid);
                    return uid;
                }
            }
        }
        Err(e) => {
            log::warn!("ECS owner-account-id not available: {}", e);
        }
    }
    String::new()
}

/// 获取实例ID（带缓存）：首次调用请求阿里云 ECS metadata（超时1秒），
/// 失败则回退到 hostname。后续调用直接返回缓存值。
pub fn get_instance_id() -> &'static str {
    INSTANCE_ID.get_or_init(|| fetch_instance_id())
}

/// 实际请求 instance-id
fn fetch_instance_id() -> String {
    // 尝试从 ECS metadata service 获取 instance-id
    let agent = metadata_agent();
    match agent
        .get("http://100.100.100.200/latest/meta-data/instance-id")
        .call()
    {
        Ok(resp) => {
            if let Ok(body) = resp.into_string() {
                let id = body.trim().to_string();
                if !id.is_empty() {
                    log::debug!("Got ECS instance-id: {}", id);
                    return id;
                }
            }
        }
        Err(e) => {
            log::debug!(
                "ECS metadata not available, falling back to hostname: {}",
                e
            );
        }
    }
    // 回退: /etc/hostname -> $HOSTNAME -> "unknown"
    std::fs::read_to_string("/etc/hostname")
        .map(|s| s.trim().to_string())
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}
