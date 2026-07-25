//! State restoration after an auth configuration submission fails.

use super::runtime::{AuthPhase, RuntimeAuthState};

pub(super) fn restore_after_failed_submission(auth: &mut RuntimeAuthState) {
    auth.phase = AuthPhase::FillingField;
    auth.current_field = 0;
    auth.collected_values.clear();
    auth.field_input.clear();
    auth.field_error = None;
    if let Some(provider_id) = auth.editing_provider_name.clone() {
        let fields = &auth.providers[auth.selected_provider].fields;
        // Slash auth prepends provider_id before edit mode, so retries preserve that identity.
        debug_assert_eq!(
            fields.first().map(|field| field.name.as_str()),
            Some("provider_id")
        );
        auth.collected_values
            .insert("provider_id".to_string(), provider_id);
        auth.current_field = 1.min(fields.len());
    }
}
