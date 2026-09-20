//! Port of daemon/internal/connectors/matrix/setup.go: hosted-setup
//! evaluation into a terminal setup state.

use chrono::{Duration, Utc};
use kura_connectors::{DiagnosticReasonCode, LifecycleState, RedactionStatus};

use crate::is_unset_time;
use crate::readiness::{
    homeserver_state, normalize_homeserver_binding, validate_homeserver_binding,
};
use crate::routes::{has_ready_route_policy, normalize_route_policy};
use crate::types::{
    BotCredentialState, HostedSetup, HostedSetupInput, RoutePolicyState, TerminalState,
};

/// Go `EvaluateHostedSetup`: reduces a hosted-setup input into an actionable
/// bounded terminal state, gating on redaction suppression, cancellation,
/// unsupported hosted-homeserver/account-provisioning requests, bot credential
/// validity, provider/network availability, binding readiness, conformance,
/// and route-policy readiness — in that order.
#[must_use]
pub fn evaluate_hosted_setup(input: HostedSetupInput) -> HostedSetup {
    let mut now = input.validated_at;
    if is_unset_time(&now) {
        now = Utc::now();
    }
    let mut timeout = input.setup_timeout;
    if timeout <= Duration::zero() {
        timeout = Duration::minutes(5);
    }
    let binding = normalize_homeserver_binding(
        &input.tenant_id,
        &input.connector_id,
        input.homeserver_binding,
    );
    let policy = normalize_route_policy(input.route_policy, now);
    let mut setup = HostedSetup {
        tenant_id: input.tenant_id.trim().to_string(),
        connector_id: input.connector_id.trim().to_string(),
        connector_kind: crate::types::CONNECTOR_KIND.to_string(),
        display_name: input.display_name.trim().to_string(),
        status: LifecycleState::Degraded,
        terminal_state: TerminalState::ActionRequired,
        delivery_eligible: false,
        reason_code: String::new(),
        bot_credential_state: input.bot_credential_state,
        homeserver_state: homeserver_state(&binding),
        route_policy_state: RoutePolicyState::None,
        homeserver_binding_id: binding.homeserver_binding_id.clone(),
        homeserver_binding: binding,
        route_policy: policy,
        created_at: now,
        updated_at: now,
        validated_at: now,
        redaction_status: RedactionStatus::Redacted,
        retention_expires_at: now + Duration::days(90),
        setup_completed_within: timeout,
    };
    if setup.bot_credential_state == BotCredentialState::default() {
        setup.bot_credential_state = BotCredentialState::Unknown;
    }
    if input.redaction_suppressed
        || setup.bot_credential_state == BotCredentialState::RedactionSuppressed
    {
        setup.status = LifecycleState::Failed;
        setup.reason_code = DiagnosticReasonCode::UnknownConnectorFailure
            .as_str()
            .to_string();
        setup.redaction_status = RedactionStatus::Suppressed;
        return setup;
    }
    if input.cancelled {
        setup.status = LifecycleState::Disabled;
        setup.terminal_state = TerminalState::Cancelled;
        setup.reason_code = "user_cancelled".to_string();
        return setup;
    }
    if input.requested_hosted_homeserver || input.requested_account_provision {
        setup.status = LifecycleState::UnsupportedCapability;
        setup.reason_code = DiagnosticReasonCode::UnsupportedCapability
            .as_str()
            .to_string();
        return setup;
    }
    if setup.bot_credential_state != BotCredentialState::Valid {
        setup.reason_code = reason_for_bot_credential(setup.bot_credential_state);
        return setup;
    }
    if !input.provider_available {
        setup.status = LifecycleState::Failed;
        setup.terminal_state = TerminalState::Unavailable;
        setup.reason_code = DiagnosticReasonCode::ProviderUnavailable
            .as_str()
            .to_string();
        return setup;
    }
    if !input.network_available {
        setup.status = LifecycleState::Failed;
        setup.terminal_state = TerminalState::Unavailable;
        setup.reason_code = DiagnosticReasonCode::NetworkFailed.as_str().to_string();
        return setup;
    }
    if validate_homeserver_binding(&setup.homeserver_binding).is_err() {
        setup.reason_code = DiagnosticReasonCode::PermissionMissing.as_str().to_string();
        return setup;
    }
    if !input.conformance_passed {
        setup.reason_code = "conformance_not_ready".to_string();
        return setup;
    }
    if has_ready_route_policy(&setup.route_policy) {
        setup.status = LifecycleState::Healthy;
        setup.terminal_state = TerminalState::Ready;
        setup.route_policy_state = RoutePolicyState::Valid;
        setup.delivery_eligible = true;
        setup.reason_code = "healthy".to_string();
        return setup;
    }
    setup.route_policy_state = RoutePolicyState::None;
    setup.reason_code = DiagnosticReasonCode::BlockedRoute.as_str().to_string();
    setup
}

/// Go `reasonForBotCredential`.
#[must_use]
pub fn reason_for_bot_credential(state: BotCredentialState) -> String {
    match state {
        BotCredentialState::PermissionMissing => {
            DiagnosticReasonCode::PermissionMissing.as_str().to_string()
        }
        BotCredentialState::Invalid
        | BotCredentialState::Revoked
        | BotCredentialState::NotStarted
        | BotCredentialState::Submitted
        | BotCredentialState::Unknown => DiagnosticReasonCode::AuthMissing.as_str().to_string(),
        _ => DiagnosticReasonCode::UnknownConnectorFailure
            .as_str()
            .to_string(),
    }
}
