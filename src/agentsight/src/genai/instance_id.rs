//! Instance ID resolution utility
//!
//! Provides a shared function to resolve the current machine's instance ID,
//! used by both SLS PutLogs uploader and Logtail file exporter.

/// 获取实例ID：优先请求阿里云 ECS metadata（超时1秒），失败则回退到 hostname
pub fn get_instance_id() -> String {
    // 尝试从 ECS metadata service 获取 instance-id
    match ureq::get("http://100.100.100.200/latest/meta-data/instance-id")
        .timeout(std::time::Duration::from_secs(1))
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
            log::debug!("ECS metadata not available, falling back to hostname: {}", e);
        }
    }
    // 回退: /etc/hostname -> $HOSTNAME -> "unknown"
    std::fs::read_to_string("/etc/hostname")
        .map(|s| s.trim().to_string())
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}
