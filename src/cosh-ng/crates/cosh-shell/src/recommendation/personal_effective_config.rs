use crate::adapter::CoshCoreAdapter;

pub(crate) fn current_ai_configured(adapter: &CoshCoreAdapter) -> Result<bool, String> {
    let value = adapter.registry_query("auth", "state", serde_json::Value::Null)?;
    auth_state_is_configured(&value)
}

fn auth_state_is_configured(value: &serde_json::Value) -> Result<bool, String> {
    let active = value
        .get("active_provider")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "missing active provider".to_string())?;
    let provider = value
        .get("saved_providers")
        .and_then(serde_json::Value::as_array)
        .and_then(|providers| {
            providers.iter().find(|provider| {
                provider
                    .get("provider_id")
                    .and_then(serde_json::Value::as_str)
                    == Some(active)
            })
        })
        .ok_or_else(|| "active provider is unavailable".to_string())?;
    let bool_field = |name: &str| {
        provider
            .get(name)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    };
    if provider
        .get("provider_type")
        .and_then(serde_json::Value::as_str)
        == Some("aliyun")
    {
        return Ok(provider
            .get("auth_source")
            .and_then(serde_json::Value::as_str)
            == Some("ecs_ram_role")
            || (bool_field("has_access_key_id") && bool_field("has_access_key_secret")));
    }
    Ok(bool_field("has_api_key"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_active_openai_and_aliyun_credentials() {
        let openai = serde_json::json!({
            "active_provider": "prod",
            "saved_providers": [{
                "provider_id": "prod",
                "provider_type": "openai_compat",
                "has_api_key": true
            }]
        });
        let aliyun = serde_json::json!({
            "active_provider": "ecs",
            "saved_providers": [{
                "provider_id": "ecs",
                "provider_type": "aliyun",
                "auth_source": "ecs_ram_role"
            }]
        });

        assert_eq!(auth_state_is_configured(&openai), Ok(true));
        assert_eq!(auth_state_is_configured(&aliyun), Ok(true));
    }

    #[test]
    fn rejects_missing_or_incomplete_active_credentials() {
        let incomplete = serde_json::json!({
            "active_provider": "prod",
            "saved_providers": [{
                "provider_id": "prod",
                "provider_type": "openai_compat",
                "has_api_key": false
            }]
        });

        assert_eq!(auth_state_is_configured(&incomplete), Ok(false));
        assert!(auth_state_is_configured(&serde_json::json!({})).is_err());
    }
}
