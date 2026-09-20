//! Port of daemon/internal/connectors/matrix/readiness.go: homeserver binding
//! normalization, validation, and state derivation.

use chrono::Utc;
use kura_connectors::RedactionStatus;

use crate::is_unset_time;
use crate::types::{
    AuthorizationState, HomeserverBinding, HomeserverCapabilityState, HomeserverState,
};

/// Go `ErrHomeserverBindingInvalid`.
pub const ERR_HOMESERVER_BINDING_INVALID: &str = "matrix homeserver binding is invalid";

/// Go `NormalizeHomeserverBinding`: tenant-safely fills the binding identity
/// and state defaults.
#[must_use]
pub fn normalize_homeserver_binding(
    tenant_id: &str,
    connector_id: &str,
    mut binding: HomeserverBinding,
) -> HomeserverBinding {
    binding.tenant_id = coalesce_string(&[binding.tenant_id.clone(), tenant_id.to_string()]);
    binding.connector_id =
        coalesce_string(&[binding.connector_id.clone(), connector_id.to_string()]);
    if binding.homeserver_binding_id.is_empty() && !binding.connector_id.is_empty() {
        binding.homeserver_binding_id = format!("matrix_homeserver_{}", binding.connector_id);
    }
    if binding.authorization_state == AuthorizationState::default() {
        binding.authorization_state = AuthorizationState::Missing;
    }
    if binding.homeserver_capability_state == HomeserverCapabilityState::default() {
        binding.homeserver_capability_state = HomeserverCapabilityState::Unknown;
    }
    if is_unset_time(&binding.validated_at) {
        binding.validated_at = Utc::now();
    }
    if binding.redaction_status == RedactionStatus::default() {
        binding.redaction_status = RedactionStatus::Redacted;
    }
    binding
}

/// Go `coalesceString`: the first trimmed non-empty value.
#[must_use]
pub fn coalesce_string(values: &[String]) -> String {
    for value in values {
        if !value.trim().is_empty() {
            return value.trim().to_string();
        }
    }
    String::new()
}

/// Go `ValidateHomeserverBinding`: the binding is ready only when every
/// identity field is present and both authorization and capability states are
/// valid.
pub fn validate_homeserver_binding(binding: &HomeserverBinding) -> Result<(), String> {
    if binding.tenant_id.trim().is_empty()
        || binding.connector_id.trim().is_empty()
        || binding.homeserver_url.trim().is_empty()
        || binding.bot_user_id.trim().is_empty()
        || binding.authorization_state != AuthorizationState::Valid
        || binding.homeserver_capability_state != HomeserverCapabilityState::Valid
    {
        return Err(ERR_HOMESERVER_BINDING_INVALID.to_string());
    }
    Ok(())
}

/// Go `homeserverState`: the derived homeserver state for a binding.
#[must_use]
pub fn homeserver_state(binding: &HomeserverBinding) -> HomeserverState {
    match binding.homeserver_capability_state {
        HomeserverCapabilityState::Unsupported => HomeserverState::Unsupported,
        HomeserverCapabilityState::RateLimited => HomeserverState::RateLimited,
        _ => match binding.authorization_state {
            AuthorizationState::ProviderUnavailable => HomeserverState::Unreachable,
            AuthorizationState::NetworkFailed => HomeserverState::NetworkFailed,
            _ => {
                if binding.homeserver_capability_state == HomeserverCapabilityState::Valid {
                    HomeserverState::Reachable
                } else {
                    HomeserverState::Unknown
                }
            }
        },
    }
}
