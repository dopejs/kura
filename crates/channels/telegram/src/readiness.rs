//! Telegram hosted-setup readiness evaluation (port of readiness.go).

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use kura_connectors::{DiagnosticReasonCode, LifecycleState, RedactionStatus};
use serde::{Deserialize, Serialize};

use crate::allowment::{
    AllowmentState, AllowmentValidation, allowment_state, has_valid_allowment, normalize_allowments,
};
use crate::wire_enum;

wire_enum!(TerminalState, default Ready; Ready => "ready", Degraded => "degraded", Unavailable => "unavailable", Cancelled => "cancelled", ActionRequired => "action-required");

wire_enum!(CredentialState, default Missing; Missing => "missing", Submitted => "submitted", Valid => "valid", Invalid => "invalid", Revoked => "revoked", RedactionSuppressed => "redaction_suppressed");

// The default `Unknown` mirrors Go's empty-string zero value, which
// `normalize_account_binding` maps to `PermissionUnknown`.
wire_enum!(PermissionState, default Unknown; Unknown => "unknown", Valid => "valid", Missing => "missing_permission", RateLimited => "rate_limited", ProviderUnavailable => "provider_unavailable", NetworkFailed => "network_failed");

wire_enum!(GroupBehavior, default Disabled; Disabled => "disabled", MentionOrCommandRequired => "mention_or_command_required");

/// A validated bot-account binding (Go `AccountBinding`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountBinding {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_id: String,
    pub connector_account_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider_account_label: String,
    pub permission_state: PermissionState,
    #[serde(default, skip_serializing_if = "crate::is_unset_time")]
    pub validated_at: DateTime<Utc>,
    pub redaction_status: RedactionStatus,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub safe_evidence: HashMap<String, String>,
}

/// Input to [`evaluate_hosted_setup`] (Go `HostedSetupInput`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HostedSetupInput {
    pub tenant_id: String,
    pub connector_id: String,
    pub display_name: String,
    pub credential: CredentialState,
    pub account_binding: AccountBinding,
    pub allowments: Vec<AllowmentValidation>,
    pub group_behavior: GroupBehavior,
    pub delivery_eligible: bool,
    pub started_at: DateTime<Utc>,
    pub validated_at: DateTime<Utc>,
    pub cancelled: bool,
}

/// Evaluated hosted-setup state (Go `HostedSetup`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostedSetup {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub connector_id: String,
    pub connector_kind: String,
    pub display_name: String,
    pub status: LifecycleState,
    pub terminal_state: TerminalState,
    pub hosted_ready: bool,
    pub credential_state: CredentialState,
    pub allowment_state: AllowmentState,
    pub group_behavior: GroupBehavior,
    pub delivery_eligible: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
    pub account_binding: AccountBinding,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowments: Vec<AllowmentValidation>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "crate::is_unset_time")]
    pub validated_at: DateTime<Utc>,
    pub redaction_status: RedactionStatus,
    #[serde(default, skip_serializing_if = "crate::is_unset_time")]
    pub retention_expires_at: DateTime<Utc>,
}

/// Go `EvaluateHostedSetup`: reduces the setup input to a terminal state.
/// Ready requires a valid credential, a valid permission state, and at least
/// one valid allowment.
#[must_use]
pub fn evaluate_hosted_setup(input: HostedSetupInput) -> HostedSetup {
    let now = if crate::is_unset_time(&input.validated_at) {
        Utc::now()
    } else {
        input.validated_at
    };
    let started = if crate::is_unset_time(&input.started_at) {
        now
    } else {
        input.started_at
    };
    let group_behavior = input.group_behavior;
    let allowments =
        normalize_allowments(&input.tenant_id, &input.connector_id, input.allowments, now);
    let binding = normalize_account_binding(
        &input.tenant_id,
        &input.connector_id,
        input.account_binding,
        now,
    );
    let mut setup = HostedSetup {
        tenant_id: input.tenant_id.trim().to_string(),
        connector_id: input.connector_id.trim().to_string(),
        connector_kind: "telegram".to_string(),
        display_name: input.display_name.trim().to_string(),
        status: LifecycleState::Healthy,
        terminal_state: TerminalState::Ready,
        hosted_ready: true,
        credential_state: input.credential,
        allowment_state: allowment_state(&allowments),
        group_behavior,
        delivery_eligible: input.delivery_eligible,
        reason_code: "healthy".to_string(),
        account_binding: binding.clone(),
        allowments,
        created_at: started,
        updated_at: now,
        validated_at: now,
        redaction_status: RedactionStatus::Redacted,
        retention_expires_at: now + chrono::Duration::days(90),
    };
    if input.cancelled {
        return setup.not_ready(
            LifecycleState::Disabled,
            TerminalState::Cancelled,
            "user_cancelled",
        );
    }
    if !crate::is_unset_time(&started)
        && now.signed_duration_since(started) > chrono::Duration::minutes(5)
        && setup.credential_state == CredentialState::Submitted
    {
        return setup.not_ready(
            LifecycleState::Degraded,
            TerminalState::ActionRequired,
            "telegram_setup_timeout",
        );
    }
    match setup.credential_state {
        CredentialState::Valid => {}
        CredentialState::Missing
        | CredentialState::Submitted
        | CredentialState::Invalid
        | CredentialState::Revoked => {
            return setup.not_ready(
                LifecycleState::Failed,
                TerminalState::ActionRequired,
                DiagnosticReasonCode::AuthMissing.as_str(),
            );
        }
        CredentialState::RedactionSuppressed => {
            setup.redaction_status = RedactionStatus::Suppressed;
            return setup.not_ready(
                LifecycleState::Failed,
                TerminalState::ActionRequired,
                DiagnosticReasonCode::UnknownConnectorFailure.as_str(),
            );
        }
    }
    match binding.permission_state {
        PermissionState::Valid => {}
        PermissionState::Missing => {
            return setup.not_ready(
                LifecycleState::PermissionBlocked,
                TerminalState::ActionRequired,
                DiagnosticReasonCode::PermissionMissing.as_str(),
            );
        }
        PermissionState::RateLimited => {
            return setup.not_ready(
                LifecycleState::RateLimited,
                TerminalState::Degraded,
                DiagnosticReasonCode::RateLimited.as_str(),
            );
        }
        PermissionState::ProviderUnavailable => {
            return setup.not_ready(
                LifecycleState::Degraded,
                TerminalState::Unavailable,
                DiagnosticReasonCode::ProviderUnavailable.as_str(),
            );
        }
        PermissionState::NetworkFailed => {
            return setup.not_ready(
                LifecycleState::Degraded,
                TerminalState::Unavailable,
                DiagnosticReasonCode::NetworkFailed.as_str(),
            );
        }
        PermissionState::Unknown => {
            return setup.not_ready(
                LifecycleState::Degraded,
                TerminalState::ActionRequired,
                DiagnosticReasonCode::UnknownConnectorFailure.as_str(),
            );
        }
    }
    if !has_valid_allowment(&setup.allowments) {
        return setup.not_ready(
            LifecycleState::Degraded,
            TerminalState::ActionRequired,
            "telegram_allowment_missing",
        );
    }
    setup.delivery_eligible = true;
    setup
}

impl HostedSetup {
    /// Go `(setup HostedSetup).notReady`.
    fn not_ready(
        mut self,
        status: LifecycleState,
        terminal: TerminalState,
        reason: &str,
    ) -> HostedSetup {
        self.status = status;
        self.terminal_state = terminal;
        self.hosted_ready = false;
        self.delivery_eligible = false;
        self.reason_code = reason.to_string();
        self
    }
}

/// Go `normalizeAccountBinding`.
pub(crate) fn normalize_account_binding(
    tenant_id: &str,
    connector_id: &str,
    mut binding: AccountBinding,
    now: DateTime<Utc>,
) -> AccountBinding {
    binding.tenant_id = first_non_empty(&[&binding.tenant_id, tenant_id]);
    binding.connector_id = first_non_empty(&[&binding.connector_id, connector_id]);
    if binding.permission_state == PermissionState::default() {
        binding.permission_state = PermissionState::Unknown;
    }
    if crate::is_unset_time(&binding.validated_at) {
        binding.validated_at = now;
    }
    if binding.redaction_status == RedactionStatus::default() {
        binding.redaction_status = RedactionStatus::Redacted;
    }
    binding
}

/// Go `firstNonEmpty`: first trimmed non-empty value, else empty string.
#[must_use]
pub(crate) fn first_non_empty(values: &[&str]) -> String {
    for value in values {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::allowment::ScopeType;
    use chrono::TimeZone;

    fn ts(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, s)
            .single()
            .expect("valid timestamp")
    }

    // Go TestEvaluateHostedSetupRequiresCredentialBindingAndAllowmentBeforeReady.
    #[test]
    fn evaluate_hosted_setup_requires_credential_binding_and_allowment_before_ready() {
        let now = ts(2026, 5, 8, 10, 0, 0);
        let setup = evaluate_hosted_setup(HostedSetupInput {
            tenant_id: "ten_telegram".to_string(),
            connector_id: "telegram-main".to_string(),
            display_name: "Telegram Main".to_string(),
            credential: CredentialState::Valid,
            account_binding: AccountBinding {
                connector_account_id: "bot_redacted".to_string(),
                provider_account_label: "agent_bot".to_string(),
                permission_state: PermissionState::Valid,
                ..AccountBinding::default()
            },
            allowments: vec![AllowmentValidation {
                allowment_id: "allow_dm".to_string(),
                scope_type: ScopeType::DirectChat,
                scope_id: "chat_redacted".to_string(),
                enabled: true,
                validation_state: crate::allowment::AllowmentValidationState::Valid,
                ..AllowmentValidation::default()
            }],
            validated_at: now,
            ..HostedSetupInput::default()
        });
        assert_eq!(setup.terminal_state, TerminalState::Ready);
        assert!(setup.hosted_ready);
        assert_eq!(setup.status, LifecycleState::Healthy);
        assert_eq!(
            setup.retention_expires_at.signed_duration_since(now),
            chrono::Duration::days(90)
        );
    }

    // Go TestEvaluateHostedSetupValidCredentialWithoutAllowmentIsActionRequired.
    #[test]
    fn evaluate_hosted_setup_valid_credential_without_allowment_is_action_required() {
        let setup = evaluate_hosted_setup(HostedSetupInput {
            tenant_id: "ten_telegram".to_string(),
            connector_id: "telegram-main".to_string(),
            display_name: "Telegram Main".to_string(),
            credential: CredentialState::Valid,
            account_binding: AccountBinding {
                connector_account_id: "bot_redacted".to_string(),
                provider_account_label: "agent_bot".to_string(),
                permission_state: PermissionState::Valid,
                ..AccountBinding::default()
            },
            validated_at: ts(2026, 5, 8, 10, 0, 0),
            ..HostedSetupInput::default()
        });
        assert_eq!(setup.terminal_state, TerminalState::ActionRequired);
        assert!(!setup.hosted_ready);
        assert_eq!(setup.reason_code, "telegram_allowment_missing");
    }

    // Go TestEvaluateHostedSetupTerminalStates.
    #[test]
    fn evaluate_hosted_setup_terminal_states() {
        let cases: Vec<(&str, HostedSetupInput, TerminalState, &str)> = vec![
            (
                "missing credential",
                HostedSetupInput {
                    credential: CredentialState::Missing,
                    ..HostedSetupInput::default()
                },
                TerminalState::ActionRequired,
                "auth_missing",
            ),
            (
                "revoked credential",
                HostedSetupInput {
                    credential: CredentialState::Revoked,
                    ..HostedSetupInput::default()
                },
                TerminalState::ActionRequired,
                "auth_missing",
            ),
            (
                "provider unavailable",
                HostedSetupInput {
                    credential: CredentialState::Valid,
                    account_binding: AccountBinding {
                        permission_state: PermissionState::ProviderUnavailable,
                        ..AccountBinding::default()
                    },
                    ..HostedSetupInput::default()
                },
                TerminalState::Unavailable,
                "provider_unavailable",
            ),
            (
                "cancelled",
                HostedSetupInput {
                    cancelled: true,
                    credential: CredentialState::Valid,
                    ..HostedSetupInput::default()
                },
                TerminalState::Cancelled,
                "user_cancelled",
            ),
        ];
        for (name, mut input, want_state, want_reason) in cases {
            input.tenant_id = "ten_telegram".to_string();
            input.connector_id = "telegram-main".to_string();
            input.display_name = "Telegram Main".to_string();
            input.validated_at = ts(2026, 5, 8, 10, 0, 0);
            let got = evaluate_hosted_setup(input);
            assert_eq!(got.terminal_state, want_state, "{name}: terminal state");
            assert_eq!(got.reason_code, want_reason, "{name}: reason");
            assert!(!got.hosted_ready, "{name}: must not be ready");
        }
    }

    // Go TestEvaluateHostedSetupTimeoutProducesActionableDiagnostic.
    #[test]
    fn evaluate_hosted_setup_timeout_produces_actionable_diagnostic() {
        let started = ts(2026, 5, 8, 10, 0, 0);
        let setup = evaluate_hosted_setup(HostedSetupInput {
            tenant_id: "ten_telegram".to_string(),
            connector_id: "telegram-main".to_string(),
            display_name: "Telegram Main".to_string(),
            credential: CredentialState::Submitted,
            started_at: started,
            validated_at: started + chrono::Duration::minutes(5) + chrono::Duration::seconds(1),
            ..HostedSetupInput::default()
        });
        assert_eq!(setup.terminal_state, TerminalState::ActionRequired);
        assert_eq!(setup.reason_code, "telegram_setup_timeout");
    }
}
