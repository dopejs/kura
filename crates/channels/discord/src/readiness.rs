//! Hosted-setup readiness evaluation (port of readiness.go): the readiness and
//! credential state strings plus `evaluate_hosted_setup` deriving the hosted
//! setup projection from config-derived evidence.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::destinations::{
    DestinationValidation, DestinationValidationState, has_explicit_hosted_destination,
    selected_destinations_valid,
};
use crate::redaction_status_redacted;
use kura_connectors::{DiagnosticReasonCode, LifecycleState, RedactionStatus};

/// Go `ReadinessState`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessState {
    #[default]
    HostedReady,
    DegradedNeedsRepair,
    Failed,
    Disabled,
}

impl ReadinessState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ReadinessState::HostedReady => "hosted_ready",
            ReadinessState::DegradedNeedsRepair => "degraded_needs_repair",
            ReadinessState::Failed => "failed",
            ReadinessState::Disabled => "disabled",
        }
    }
}

impl std::fmt::Display for ReadinessState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Go `CredentialState`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialState {
    #[default]
    Missing,
    Submitted,
    Valid,
    Invalid,
    Revoked,
    RedactionSuppressed,
}

impl CredentialState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            CredentialState::Missing => "missing",
            CredentialState::Submitted => "submitted",
            CredentialState::Valid => "valid",
            CredentialState::Invalid => "invalid",
            CredentialState::Revoked => "revoked",
            CredentialState::RedactionSuppressed => "redaction_suppressed",
        }
    }
}

impl std::fmt::Display for CredentialState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Go `HostedSetupInput` (no JSON tags).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HostedSetupInput {
    pub tenant_id: String,
    pub connector_id: String,
    pub display_name: String,
    pub credential: CredentialState,
    pub respond_in_dm: bool,
    pub require_mention: bool,
    pub delivery_mode: String,
    pub destinations: Vec<DestinationValidation>,
    pub validated_at: DateTime<Utc>,
}

/// Go `HostedSetup`. Wire shape matches the Go json tags.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostedSetup {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub connector_id: String,
    pub connector_kind: String,
    pub display_name: String,
    pub status: LifecycleState,
    pub readiness_state: ReadinessState,
    pub hosted_ready: bool,
    pub credential_state: CredentialState,
    pub respond_in_dm: bool,
    pub require_mention: bool,
    pub delivery_mode: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub destinations: Vec<DestinationValidation>,
    #[serde(default, skip_serializing_if = "is_unset_time")]
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "is_unset_time")]
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "is_unset_time")]
    pub validated_at: DateTime<Utc>,
    pub redaction_status: RedactionStatus,
    #[serde(default, skip_serializing_if = "is_unset_time")]
    pub retention_expires_at: DateTime<Utc>,
}

fn is_unset_time(dt: &DateTime<Utc>) -> bool {
    dt.timestamp() == 0 && dt.timestamp_subsec_nanos() == 0
}

/// Go `EvaluateHostedSetup`.
#[must_use]
pub fn evaluate_hosted_setup(input: HostedSetupInput) -> HostedSetup {
    let mut now = input.validated_at;
    if now == DateTime::<Utc>::default() {
        now = Utc::now();
    }
    let mode = input.delivery_mode.trim();
    let mode = if mode.is_empty() {
        "gateway".to_string()
    } else {
        mode.to_string()
    };
    let mut setup = HostedSetup {
        tenant_id: input.tenant_id.trim().to_string(),
        connector_id: input.connector_id.trim().to_string(),
        connector_kind: "discord".to_string(),
        display_name: input.display_name.trim().to_string(),
        status: LifecycleState::Healthy,
        readiness_state: ReadinessState::HostedReady,
        hosted_ready: true,
        credential_state: input.credential,
        respond_in_dm: input.respond_in_dm,
        require_mention: input.require_mention,
        delivery_mode: mode,
        reason_code: String::new(),
        destinations: normalize_destination_evidence(
            &input.tenant_id,
            &input.connector_id,
            &input.destinations,
            now,
        ),
        created_at: now,
        updated_at: now,
        validated_at: now,
        redaction_status: redaction_status_redacted(),
        retention_expires_at: now + Duration::days(90),
    };
    match input.credential {
        CredentialState::Valid => {
            if !has_explicit_hosted_destination(&setup.destinations) {
                setup.status = LifecycleState::Degraded;
                setup.readiness_state = ReadinessState::DegradedNeedsRepair;
                setup.hosted_ready = false;
                setup.reason_code = "missing_explicit_destination".to_string();
            } else if !selected_destinations_valid(&setup.destinations) {
                setup.status = LifecycleState::Degraded;
                setup.readiness_state = ReadinessState::DegradedNeedsRepair;
                setup.hosted_ready = false;
                setup.reason_code = "destination_validation_failed".to_string();
            } else {
                setup.reason_code = "healthy".to_string();
            }
        }
        CredentialState::Missing => {
            setup.status = LifecycleState::Failed;
            setup.readiness_state = ReadinessState::Failed;
            setup.hosted_ready = false;
            setup.credential_state = CredentialState::Missing;
            setup.reason_code = DiagnosticReasonCode::AuthMissing.as_str().to_string();
        }
        CredentialState::Invalid | CredentialState::Revoked => {
            setup.status = LifecycleState::Failed;
            setup.readiness_state = ReadinessState::Failed;
            setup.hosted_ready = false;
            setup.reason_code = DiagnosticReasonCode::AuthMissing.as_str().to_string();
        }
        CredentialState::RedactionSuppressed => {
            setup.status = LifecycleState::Failed;
            setup.readiness_state = ReadinessState::Failed;
            setup.hosted_ready = false;
            setup.redaction_status = RedactionStatus::Suppressed;
            setup.reason_code = DiagnosticReasonCode::UnknownConnectorFailure
                .as_str()
                .to_string();
        }
        CredentialState::Submitted => {
            // Go default branch: a submitted-but-not-valid credential is not
            // yet usable and reports the unknown-connector failure reason.
            setup.status = LifecycleState::Failed;
            setup.readiness_state = ReadinessState::Failed;
            setup.hosted_ready = false;
            setup.reason_code = DiagnosticReasonCode::UnknownConnectorFailure
                .as_str()
                .to_string();
        }
    }
    setup
}

/// Go `normalizeDestinationEvidence`: fills defaults for validated-at,
/// redaction status, tenant/connector binding, and the validation state.
#[must_use]
pub fn normalize_destination_evidence(
    tenant_id: &str,
    connector_id: &str,
    destinations: &[DestinationValidation],
    now: DateTime<Utc>,
) -> Vec<DestinationValidation> {
    let mut items = Vec::with_capacity(destinations.len());
    for mut destination in destinations.iter().cloned() {
        if destination.validated_at == DateTime::<Utc>::default() {
            destination.validated_at = now;
        }
        // Go normalizes an empty RedactionStatus to Redacted; the enum's
        // default is already Redacted, so nothing to do.
        if destination.tenant_id.trim().is_empty() {
            destination.tenant_id = tenant_id.trim().to_string();
        }
        if destination.connector_id.trim().is_empty() {
            destination.connector_id = connector_id.trim().to_string();
        }
        // Go maps the empty-string state to Invalid; the enum's default is
        // Invalid, matching that normalization.
        if destination.reason_code.trim().is_empty() {
            destination.reason_code =
                if destination.validation_state == DestinationValidationState::Valid {
                    "healthy".to_string()
                } else {
                    DiagnosticReasonCode::BlockedRoute.as_str().to_string()
                };
        }
        items.push(destination);
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::destinations::DestinationType;
    use kura_connectors::DiagnosticReasonCode;

    fn ts() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-05-07T10:00:00Z")
            .expect("parse")
            .with_timezone(&Utc)
    }

    // Go TestEvaluateHostedSetupRequiresValidCredentialAndExplicitDestinations
    #[test]
    fn evaluate_hosted_setup_requires_valid_credential_and_explicit_destinations() {
        let setup = evaluate_hosted_setup(HostedSetupInput {
            tenant_id: "ten_discord".to_string(),
            connector_id: "discord-main".to_string(),
            display_name: "Discord Main".to_string(),
            credential: CredentialState::Valid,
            respond_in_dm: true,
            require_mention: true,
            destinations: vec![
                DestinationValidation {
                    destination_id: "guild_redacted".to_string(),
                    destination_type: DestinationType::Guild,
                    selected: true,
                    validation_state: DestinationValidationState::Valid,
                    ..DestinationValidation::default()
                },
                DestinationValidation {
                    destination_id: "channel_redacted".to_string(),
                    destination_type: DestinationType::Channel,
                    selected: true,
                    validation_state: DestinationValidationState::Valid,
                    ..DestinationValidation::default()
                },
            ],
            validated_at: ts(),
            ..HostedSetupInput::default()
        });
        assert_eq!(setup.readiness_state, ReadinessState::HostedReady);
        assert_eq!(setup.status, LifecycleState::Healthy);
        assert!(setup.hosted_ready);
    }

    // Go TestEvaluateHostedSetupSavesDegradedForMissingOrPartiallyInvalidDestinations
    #[test]
    fn evaluate_hosted_setup_saves_degraded_for_missing_or_partially_invalid_destinations() {
        let missing = evaluate_hosted_setup(HostedSetupInput {
            tenant_id: "ten_discord".to_string(),
            connector_id: "discord-main".to_string(),
            display_name: "Discord Main".to_string(),
            credential: CredentialState::Valid,
            validated_at: ts(),
            ..HostedSetupInput::default()
        });
        assert_eq!(missing.readiness_state, ReadinessState::DegradedNeedsRepair);
        assert!(!missing.hosted_ready);
        assert_eq!(missing.reason_code, "missing_explicit_destination");

        let partial = evaluate_hosted_setup(HostedSetupInput {
            tenant_id: "ten_discord".to_string(),
            connector_id: "discord-main".to_string(),
            display_name: "Discord Main".to_string(),
            credential: CredentialState::Valid,
            destinations: vec![
                DestinationValidation {
                    destination_id: "guild_redacted".to_string(),
                    destination_type: DestinationType::Guild,
                    selected: true,
                    validation_state: DestinationValidationState::Valid,
                    ..DestinationValidation::default()
                },
                DestinationValidation {
                    destination_id: "channel_redacted".to_string(),
                    destination_type: DestinationType::Channel,
                    selected: true,
                    validation_state: DestinationValidationState::MissingPermission,
                    reason_code: DiagnosticReasonCode::PermissionMissing.as_str().to_string(),
                    ..DestinationValidation::default()
                },
            ],
            validated_at: ts(),
            ..HostedSetupInput::default()
        });
        assert_eq!(partial.readiness_state, ReadinessState::DegradedNeedsRepair);
        assert!(!partial.hosted_ready);
        assert_eq!(partial.reason_code, "destination_validation_failed");
    }

    #[test]
    fn evaluate_hosted_setup_fails_without_credential() {
        let setup = evaluate_hosted_setup(HostedSetupInput {
            tenant_id: "ten_discord".to_string(),
            connector_id: "discord-main".to_string(),
            display_name: "Discord Main".to_string(),
            credential: CredentialState::Missing,
            validated_at: ts(),
            ..HostedSetupInput::default()
        });
        assert_eq!(setup.status, LifecycleState::Failed);
        assert_eq!(setup.readiness_state, ReadinessState::Failed);
        assert!(!setup.hosted_ready);
        assert_eq!(
            setup.reason_code,
            DiagnosticReasonCode::AuthMissing.as_str()
        );
        assert_eq!(setup.credential_state, CredentialState::Missing);
    }

    #[test]
    fn evaluate_hosted_setup_suppresses_redaction_for_redaction_suppressed_credential() {
        let setup = evaluate_hosted_setup(HostedSetupInput {
            credential: CredentialState::RedactionSuppressed,
            validated_at: ts(),
            ..HostedSetupInput::default()
        });
        assert_eq!(setup.status, LifecycleState::Failed);
        assert!(!setup.hosted_ready);
        assert_eq!(setup.redaction_status, RedactionStatus::Suppressed);
        assert_eq!(
            setup.reason_code,
            DiagnosticReasonCode::UnknownConnectorFailure.as_str()
        );
    }

    #[test]
    fn normalize_destination_evidence_fills_defaults() {
        let now = ts();
        let items = normalize_destination_evidence(
            "ten_discord",
            "discord-main",
            &[DestinationValidation {
                destination_id: "channel_1".to_string(),
                destination_type: DestinationType::Channel,
                selected: true,
                ..DestinationValidation::default()
            }],
            now,
        );
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].tenant_id, "ten_discord");
        assert_eq!(items[0].connector_id, "discord-main");
        assert_eq!(items[0].validated_at, now);
        assert_eq!(
            items[0].validation_state,
            DestinationValidationState::Invalid
        );
        assert_eq!(
            items[0].reason_code,
            DiagnosticReasonCode::BlockedRoute.as_str()
        );
        assert_eq!(items[0].redaction_status, RedactionStatus::Redacted);
    }
}
