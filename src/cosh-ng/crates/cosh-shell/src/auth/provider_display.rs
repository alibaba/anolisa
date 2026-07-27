use crate::runtime::prelude::AuthProviderInfo;

pub(crate) fn auth_required_providers_for_display(
    providers: &[AuthProviderInfo],
) -> Vec<AuthProviderInfo> {
    let mut providers = providers.to_vec();
    for provider in &mut providers {
        if provider.id == "aliyun" && !provider.label.contains("免费可用") {
            provider.label = format!("{} (免费可用)", provider.label);
        }
    }
    providers.sort_by_key(|provider| if provider.id == "aliyun" { 0 } else { 1 });
    providers
}
