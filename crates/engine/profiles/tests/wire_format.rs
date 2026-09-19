//! Wire-compatibility tests: fixtures copied byte-for-byte from
//! daemon/internal/contracts/agent_profile_contracts_test.go. Deserializing
//! and re-serializing must reproduce the exact JSON the Go daemon emits.

use kura_profiles::{
    AgentProfile, MutationResult, ProfilesError, RuntimeProjection, validate_mutation,
    validation_reason_code,
};
use kura_profiles::{MutationInput, OverlayValidationState};

const AGENT_PROFILE_RESOURCE: &str = r#"{"profileId":"prof_1","tenantId":"ten_57","displayName":"Support Agent","displayIdentity":{"name":"Support","safeSummary":"Support"},"persona":{"tone":"direct","safeSummary":"Direct support persona"},"defaultProviderPreference":{"providerId":"openai_compatible","model":"gpt-test","validationState":"valid"},"safetyDefaults":{"approvalPosture":"ask","validationState":"valid"},"status":"active","activeVersionId":"profv_1","tenantDefault":true,"overlayReferenceCount":1,"createdAt":"2026-05-12T10:00:00Z","updatedAt":"2026-05-12T10:00:00Z","redactionStatus":"redacted"}"#;

const RUNTIME_PROJECTION: &str = r#"{"runtimeProfileProjectionId":"rpp_1","tenantId":"ten_57","profileId":"prof_1","profileVersionId":"profv_1","selectionId":"sel_1","resourceKind":"thread","resourceId":"thr_1","threadId":"thr_1","selectionScope":"tenant_default","selectionReason":"user_activated","safeDisplayName":"Support Agent","safeSummary":"Direct support persona","configurationScope":"explicit_profile_configuration","deferredBindingClassification":"roadmap_58_deferred_binding_unapplied","occurredAt":"2026-05-12T10:00:00Z","redactionStatus":"redacted"}"#;

#[test]
fn agent_profile_resource_round_trips_byte_exact() {
    let profile: AgentProfile = serde_json::from_str(AGENT_PROFILE_RESOURCE).expect("decode");
    assert_eq!(profile.profile_id, "prof_1");
    assert_eq!(
        profile.default_provider_preference.validation_state,
        OverlayValidationState::VALID
    );
    let encoded = serde_json::to_string(&profile).expect("encode");
    assert_eq!(encoded, AGENT_PROFILE_RESOURCE);
}

#[test]
fn runtime_projection_round_trips_byte_exact() {
    let projection: RuntimeProjection = serde_json::from_str(RUNTIME_PROJECTION).expect("decode");
    assert_eq!(projection.runtime_profile_projection_id, "rpp_1");
    assert!(projection.retention_expires_at.is_none());
    let encoded = serde_json::to_string(&projection).expect("encode");
    assert_eq!(encoded, RUNTIME_PROJECTION);
}

#[test]
fn unknown_validation_state_survives_json_and_is_rejected_by_policy() {
    // Go decodes OverlayValidationState as a plain string, so unknown values
    // reach ValidateMutation and fail with a stable reason code — serde must
    // not reject them first.
    let input: MutationInput = serde_json::from_str(
        r#"{"displayName":"Support","defaultProviderPreference":{"validationState":"garbage"}}"#,
    )
    .expect("unknown validation state must decode");
    let err = validate_mutation(&input).expect_err("policy must reject the unknown state");
    assert!(matches!(err, ProfilesError::InvalidProfile(_)));
    assert_eq!(
        validation_reason_code(&err),
        "provider_validation_state_unknown"
    );
}

#[test]
fn mutation_result_always_serializes_selection() {
    // Go's `json:"selection,omitempty"` on a struct field never omits; the
    // zero selection is serialized. Keep that contract.
    let result = MutationResult::default();
    let value = serde_json::to_value(&result).expect("encode");
    assert!(
        value.get("selection").is_some(),
        "selection must always be present, got {value}"
    );
}

#[test]
fn omitempty_fields_are_dropped_when_zero() {
    let profile = AgentProfile::default();
    let value = serde_json::to_value(&profile).expect("encode");
    let object = value.as_object().expect("profile is an object");
    for key in [
        "legacyMappingEvidence",
        "activeVersionId",
        "archivedAt",
        "disabledAt",
        "createdByPrincipalId",
        "updatedByPrincipalId",
    ] {
        assert!(!object.contains_key(key), "{key} must be omitted when zero");
    }
    for key in [
        "profileId",
        "tenantId",
        "displayName",
        "displayIdentity",
        "persona",
        "defaultProviderPreference",
        "safetyDefaults",
        "status",
        "tenantDefault",
        "overlayReferenceCount",
        "createdAt",
        "updatedAt",
        "redactionStatus",
    ] {
        assert!(object.contains_key(key), "{key} must always be present");
    }
}
