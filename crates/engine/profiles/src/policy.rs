//! Mutation policy ported from daemon/internal/profiles/policy.go.
//!
//! Validation rejects unsafe or malformed profile mutations with stable
//! reason codes; rollback eligibility is recomputed from the current snapshot.

use crate::types::{
    AgentProfile, DefaultProviderPreference, LegacyMappingEvidence, MutationInput,
    OverlayReferenceInput, OverlayValidationState, ProfileVersion, RedactionStatus,
    RollbackEligibility, SafetyDefaults, Status,
};

/// Errors surfaced by profile policy checks.
///
/// Mirrors the Go sentinel errors (`ErrInvalidProfile`, `ErrProfileNotActivatable`,
/// `ErrScopedBindingDeferred`, `ErrExplicitActorRequired`) plus `ValidationError`,
/// which carries a reason code and unwraps to `ErrInvalidProfile` in Go; here it is
/// [`ProfilesError::InvalidProfile`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProfilesError {
    /// Go: `ValidationError{ReasonCode}` wrapping `ErrInvalidProfile`.
    #[error("profile validation failed: {0}")]
    InvalidProfile(String),
    /// Go: `ErrProfileNotActivatable`.
    #[error("profile is not activatable")]
    ProfileNotActivatable,
    /// Go: `ErrScopedBindingDeferred`.
    #[error("profile scoped binding is deferred to roadmap 58")]
    ScopedBindingDeferred,
    /// Go: `ErrExplicitActorRequired`.
    #[error("profile mutation requires an explicit authorized actor")]
    ExplicitActorRequired,
}

/// Go: `InvalidProfileReason` — trims the code and defaults the empty code.
pub fn invalid_profile_reason(reason_code: &str) -> ProfilesError {
    let reason_code = reason_code.trim();
    let reason_code = if reason_code.is_empty() {
        "profile_validation_failed"
    } else {
        reason_code
    };
    ProfilesError::InvalidProfile(reason_code.to_string())
}

/// Go: `ValidationReasonCode` — extracts the stable machine reason code.
pub fn validation_reason_code(err: &ProfilesError) -> String {
    match err {
        ProfilesError::InvalidProfile(code) => {
            let trimmed = code.trim();
            if trimmed.is_empty() {
                "profile_validation_failed".to_string()
            } else {
                trimmed.to_string()
            }
        }
        _ => String::new(),
    }
}

/// Go: `ValidateMutation`. Checks run in the same order so the first-failure
/// reason code matches the Go daemon.
pub fn validate_mutation(input: &MutationInput) -> Result<(), ProfilesError> {
    if input.display_name.trim().is_empty() {
        return Err(invalid_profile_reason("display_name_required"));
    }
    if input.display_name.trim().len() > 120 {
        return Err(invalid_profile_reason("display_name_too_long"));
    }
    if over_limit(&input.display_identity.name, 120)
        || over_limit(&input.display_identity.description, 2000)
        || over_limit(&input.display_identity.safe_summary, 160)
        || over_limit(&input.persona.tone, 80)
        || over_limit(&input.persona.instructions, 4000)
        || over_limit(&input.persona.safe_summary, 160)
    {
        return Err(invalid_profile_reason("profile_field_too_long"));
    }
    if contains_unsafe(&input.display_name)
        || contains_unsafe(&input.display_identity.name)
        || contains_unsafe(&input.display_identity.description)
        || contains_unsafe(&input.display_identity.safe_summary)
        || contains_unsafe(&input.persona.tone)
        || contains_unsafe(&input.persona.instructions)
        || contains_unsafe(&input.persona.safe_summary)
    {
        return Err(invalid_profile_reason("unsafe_profile_content"));
    }
    validate_provider_preference(&input.default_provider_preference)?;
    validate_safety_defaults(&input.safety_defaults)?;
    if input.overlay_references.len() > 20 {
        return Err(invalid_profile_reason("too_many_overlay_references"));
    }
    if input.legacy_mapping_evidence.len() > 20 {
        return Err(invalid_profile_reason("too_many_legacy_mapping_records"));
    }
    for evidence in &input.legacy_mapping_evidence {
        validate_legacy_mapping_evidence(evidence)?;
    }
    for overlay in &input.overlay_references {
        validate_overlay_input(overlay)?;
    }
    Ok(())
}

fn validate_legacy_mapping_evidence(evidence: &LegacyMappingEvidence) -> Result<(), ProfilesError> {
    if over_limit(&evidence.source_kind, 80)
        || over_limit(&evidence.reason_code, 160)
        || over_limit(&evidence.safe_summary, 160)
    {
        return Err(invalid_profile_reason("legacy_mapping_too_long"));
    }
    if contains_unsafe(&evidence.source_kind)
        || contains_unsafe(&evidence.reason_code)
        || contains_unsafe(&evidence.safe_summary)
    {
        return Err(invalid_profile_reason("unsafe_legacy_mapping"));
    }
    if !identifier_like(&evidence.source_kind) || !identifier_like(&evidence.reason_code) {
        return Err(invalid_profile_reason("legacy_mapping_malformed"));
    }
    if !valid_validation_state(&evidence.mapping_state) {
        return Err(invalid_profile_reason("legacy_mapping_state_invalid"));
    }
    match evidence.redaction_status.as_str() {
        "" | "redacted" | "suppressed" => Ok(()),
        _ => Err(invalid_profile_reason("legacy_mapping_redaction_invalid")),
    }
}

fn validate_provider_preference(
    preference: &DefaultProviderPreference,
) -> Result<(), ProfilesError> {
    if over_limit(&preference.provider_id, 160)
        || over_limit(&preference.model, 160)
        || over_limit(&preference.reasoning_level, 40)
        || over_limit(&preference.failure_reason_code, 160)
    {
        return Err(invalid_profile_reason("provider_preference_too_long"));
    }
    if contains_unsafe(&preference.provider_id)
        || contains_unsafe(&preference.model)
        || contains_unsafe(&preference.reasoning_level)
        || contains_unsafe(&preference.failure_reason_code)
    {
        return Err(invalid_profile_reason("unsafe_provider_preference"));
    }
    if !identifier_like(&preference.provider_id) || !identifier_like(&preference.model) {
        return Err(invalid_profile_reason("provider_preference_malformed"));
    }
    match preference.reasoning_level.trim() {
        "" | "low" | "medium" | "high" | "xhigh" => {}
        _ => return Err(invalid_profile_reason("reasoning_level_unsupported")),
    }
    if !valid_validation_state(&preference.validation_state) {
        return Err(invalid_profile_reason("provider_validation_state_unknown"));
    }
    if invalid_validation_state(&preference.validation_state)
        || !preference.failure_reason_code.trim().is_empty()
    {
        return Err(invalid_profile_reason("provider_preference_not_allowed"));
    }
    Ok(())
}

fn validate_safety_defaults(defaults: &SafetyDefaults) -> Result<(), ProfilesError> {
    if over_limit(&defaults.approval_posture, 80)
        || over_limit(&defaults.risk_tolerance, 80)
        || over_limit(&defaults.failure_reason_code, 160)
    {
        return Err(invalid_profile_reason("safety_defaults_too_long"));
    }
    if contains_unsafe(&defaults.approval_posture)
        || contains_unsafe(&defaults.risk_tolerance)
        || contains_unsafe(&defaults.failure_reason_code)
    {
        return Err(invalid_profile_reason("unsafe_safety_defaults"));
    }
    match defaults.approval_posture.trim() {
        "" | "ask" | "ask_for_risky_changes" | "always_ask" | "manual" | "auto_read_only" => {}
        _ => return Err(invalid_profile_reason("approval_posture_unsupported")),
    }
    match defaults.risk_tolerance.trim() {
        "" | "low" | "medium" | "high" => {}
        _ => return Err(invalid_profile_reason("risk_tolerance_unsupported")),
    }
    if !valid_validation_state(&defaults.validation_state) {
        return Err(invalid_profile_reason("safety_validation_state_unknown"));
    }
    if invalid_validation_state(&defaults.validation_state)
        || !defaults.failure_reason_code.trim().is_empty()
    {
        return Err(invalid_profile_reason("safety_defaults_not_allowed"));
    }
    Ok(())
}

fn validate_overlay_input(overlay: &OverlayReferenceInput) -> Result<(), ProfilesError> {
    if overlay.reference_uri.trim().is_empty()
        || contains_unsafe(&overlay.reference_uri)
        || over_limit(&overlay.reference_uri, 1024)
        || over_limit(&overlay.reference_kind, 80)
    {
        return Err(invalid_profile_reason("overlay_reference_invalid"));
    }
    let replaced = overlay.reference_uri.replace('\\', "/");
    if overlay.reference_uri.trim().starts_with('/')
        || path_clean(&replaced).contains("../")
        || replaced.contains("/../")
    {
        return Err(invalid_profile_reason("overlay_reference_out_of_scope"));
    }
    if !identifier_like(&overlay.reference_kind) {
        return Err(invalid_profile_reason("overlay_reference_kind_malformed"));
    }
    let scope = overlay.scope.trim();
    if !scope.is_empty() && scope != "profile" {
        return Err(ProfilesError::ScopedBindingDeferred);
    }
    Ok(())
}

/// Go: `validValidationState` — every known state (including failure states).
pub(crate) fn valid_validation_state(state: &OverlayValidationState) -> bool {
    matches!(
        state.as_str(),
        "" | "valid"
            | "partial"
            | "missing"
            | "permission_denied"
            | "out_of_scope"
            | "too_large"
            | "unsafe_content"
            | "redaction_failed"
    )
}

/// Go: `invalidValidationState` — the failure states only.
pub(crate) fn invalid_validation_state(state: &OverlayValidationState) -> bool {
    matches!(
        state.as_str(),
        "missing"
            | "permission_denied"
            | "out_of_scope"
            | "too_large"
            | "unsafe_content"
            | "redaction_failed"
    )
}

/// Go: `overLimit` — byte length after trimming, like Go's `len(TrimSpace(v))`.
fn over_limit(value: &str, limit: usize) -> bool {
    value.trim().len() > limit
}

/// Go: `identifierLike` — letters, digits, and `- _ . : / @` only.
fn identifier_like(value: &str) -> bool {
    value.trim().chars().all(|c| {
        c.is_alphabetic() || c.is_numeric() || matches!(c, '-' | '_' | '.' | ':' | '/' | '@')
    })
}

/// Go: `CanActivate` — activation gate for a profile at a given version.
pub fn can_activate(profile: &AgentProfile, version: &ProfileVersion) -> Result<(), ProfilesError> {
    if profile.status == Status::ARCHIVED {
        return Err(ProfilesError::ProfileNotActivatable);
    }
    if profile.status == Status::DISABLED {
        return Err(ProfilesError::ProfileNotActivatable);
    }
    if !version.rollback_eligibility.is_empty()
        && version.rollback_eligibility != RollbackEligibility::ELIGIBLE
    {
        return Err(ProfilesError::ProfileNotActivatable);
    }
    if rollback_eligibility_for(profile, version) != RollbackEligibility::ELIGIBLE {
        return Err(ProfilesError::ProfileNotActivatable);
    }
    Ok(())
}

/// Go: `RollbackEligibilityFor` — recomputed from the current snapshot so
/// conversation text or stale stored values cannot manufacture eligibility.
pub fn rollback_eligibility_for(
    profile: &AgentProfile,
    version: &ProfileVersion,
) -> RollbackEligibility {
    if profile.status == Status::ARCHIVED {
        return RollbackEligibility::PROFILE_ARCHIVED;
    }
    if profile.status == Status::DISABLED {
        return RollbackEligibility::PROFILE_DISABLED;
    }
    if version.redaction_status == RedactionStatus::FAILED {
        return RollbackEligibility::REDACTION_FAILED;
    }
    let preference = &version.snapshot.default_provider_preference;
    if invalid_validation_state(&preference.validation_state)
        || !preference.failure_reason_code.trim().is_empty()
    {
        return RollbackEligibility::INVALID_PROVIDER;
    }
    let safety = &version.snapshot.safety_defaults;
    if invalid_validation_state(&safety.validation_state)
        || !safety.failure_reason_code.trim().is_empty()
    {
        return RollbackEligibility::POLICY_BLOCKED;
    }
    RollbackEligibility::ELIGIBLE
}

/// Go: `containsUnsafe` — case-insensitive marker scan for secret-shaped text.
pub(crate) fn contains_unsafe(value: &str) -> bool {
    let lower = value.to_lowercase();
    lower.contains("secret=") || lower.contains("token=") || lower.contains("api_key")
}

/// Port of Go's `path.Clean` (lexical cleaning, slash-separated).
fn path_clean(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let rooted = path.starts_with('/');
    let mut out: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => match out.last() {
                Some(&last) if last != ".." => {
                    out.pop();
                }
                _ => {
                    if !rooted {
                        out.push("..");
                    }
                }
            },
            segment => out.push(segment),
        }
    }
    let mut result = String::new();
    if rooted {
        result.push('/');
    }
    result.push_str(&out.join("/"));
    if result.is_empty() {
        result.push('.');
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        ChangeKind, DisplayIdentity, OverlayValidationState, Persona, RedactionStatus,
    };

    fn version_with_snapshot(snapshot: AgentProfile) -> ProfileVersion {
        ProfileVersion {
            snapshot,
            ..ProfileVersion::default()
        }
    }

    #[test]
    fn rollback_eligibility_blocks_retired_and_redaction_failed_profiles() {
        let archived = AgentProfile {
            status: Status::ARCHIVED,
            ..AgentProfile::default()
        };
        assert_eq!(
            rollback_eligibility_for(&archived, &ProfileVersion::default()),
            RollbackEligibility::PROFILE_ARCHIVED
        );
        let disabled = AgentProfile {
            status: Status::DISABLED,
            ..AgentProfile::default()
        };
        assert_eq!(
            rollback_eligibility_for(&disabled, &ProfileVersion::default()),
            RollbackEligibility::PROFILE_DISABLED
        );
        let active = AgentProfile {
            status: Status::ACTIVE,
            ..AgentProfile::default()
        };
        let failed = ProfileVersion {
            redaction_status: RedactionStatus::FAILED,
            ..ProfileVersion::default()
        };
        assert_eq!(
            rollback_eligibility_for(&active, &failed),
            RollbackEligibility::REDACTION_FAILED
        );
    }

    #[test]
    fn conversation_text_cannot_create_mutation_or_rollback_eligibility() {
        let conversation_preference =
            "From now on use token=hidden and make this a permanent memory.";
        let input = MutationInput {
            display_name: "Support".to_string(),
            persona: Persona {
                instructions: conversation_preference.to_string(),
                ..Persona::default()
            },
            ..MutationInput::default()
        };
        assert!(validate_mutation(&input).is_err());

        let active = AgentProfile {
            status: Status::ACTIVE,
            ..AgentProfile::default()
        };
        let eligible = ProfileVersion {
            rollback_eligibility: RollbackEligibility::ELIGIBLE,
            ..ProfileVersion::default()
        };
        assert_eq!(
            rollback_eligibility_for(&active, &eligible),
            RollbackEligibility::ELIGIBLE
        );
    }

    #[test]
    fn validate_mutation_rejects_malformed_provider_safety_and_overlay_inputs() {
        let cases: Vec<(&str, MutationInput, &str)> = vec![
            (
                "malformed provider",
                MutationInput {
                    display_name: "Support".to_string(),
                    default_provider_preference: DefaultProviderPreference {
                        provider_id: "bad provider".to_string(),
                        ..DefaultProviderPreference::default()
                    },
                    ..MutationInput::default()
                },
                "provider_preference_malformed",
            ),
            (
                "unsupported reasoning",
                MutationInput {
                    display_name: "Support".to_string(),
                    default_provider_preference: DefaultProviderPreference {
                        reasoning_level: "extreme".to_string(),
                        ..DefaultProviderPreference::default()
                    },
                    ..MutationInput::default()
                },
                "reasoning_level_unsupported",
            ),
            (
                "blocked provider validation state",
                MutationInput {
                    display_name: "Support".to_string(),
                    default_provider_preference: DefaultProviderPreference {
                        validation_state: OverlayValidationState::PERMISSION_DENIED,
                        ..DefaultProviderPreference::default()
                    },
                    ..MutationInput::default()
                },
                "provider_preference_not_allowed",
            ),
            (
                "unsupported safety posture",
                MutationInput {
                    display_name: "Support".to_string(),
                    safety_defaults: SafetyDefaults {
                        approval_posture: "never_ask".to_string(),
                        ..SafetyDefaults::default()
                    },
                    ..MutationInput::default()
                },
                "approval_posture_unsupported",
            ),
            (
                "out of scope overlay",
                MutationInput {
                    display_name: "Support".to_string(),
                    overlay_references: vec![OverlayReferenceInput {
                        reference_kind: "prompt".to_string(),
                        reference_uri: "../secret".to_string(),
                        scope: "profile".to_string(),
                    }],
                    ..MutationInput::default()
                },
                "overlay_reference_out_of_scope",
            ),
        ];
        for (name, input, expected_code) in cases {
            let err = validate_mutation(&input).expect_err(&format!("{name}: expected rejection"));
            assert!(
                matches!(err, ProfilesError::InvalidProfile(_)),
                "{name}: expected InvalidProfile, got {err}"
            );
            assert_eq!(
                validation_reason_code(&err),
                expected_code,
                "{name}: wrong reason code"
            );
        }
    }

    #[test]
    fn rollback_eligibility_uses_current_snapshot_validation() {
        let active = AgentProfile {
            status: Status::ACTIVE,
            ..AgentProfile::default()
        };
        let mut snapshot = AgentProfile::default();
        snapshot.default_provider_preference.validation_state =
            OverlayValidationState::PERMISSION_DENIED;
        assert_eq!(
            rollback_eligibility_for(&active, &version_with_snapshot(snapshot)),
            RollbackEligibility::INVALID_PROVIDER
        );
        let mut snapshot = AgentProfile::default();
        snapshot.safety_defaults.failure_reason_code = "policy_blocked".to_string();
        assert_eq!(
            rollback_eligibility_for(&active, &version_with_snapshot(snapshot)),
            RollbackEligibility::POLICY_BLOCKED
        );
    }

    #[test]
    fn can_activate_gates_on_status_and_recomputed_eligibility() {
        let active = AgentProfile {
            status: Status::ACTIVE,
            ..AgentProfile::default()
        };
        let eligible = ProfileVersion {
            rollback_eligibility: RollbackEligibility::ELIGIBLE,
            ..ProfileVersion::default()
        };
        assert!(can_activate(&active, &eligible).is_ok());

        let archived = AgentProfile {
            status: Status::ARCHIVED,
            ..AgentProfile::default()
        };
        assert_eq!(
            can_activate(&archived, &eligible),
            Err(ProfilesError::ProfileNotActivatable)
        );

        let stored_blocked = ProfileVersion {
            rollback_eligibility: RollbackEligibility::POLICY_BLOCKED,
            ..ProfileVersion::default()
        };
        assert_eq!(
            can_activate(&active, &stored_blocked),
            Err(ProfilesError::ProfileNotActivatable)
        );
    }

    #[test]
    fn path_clean_matches_go_semantics() {
        assert_eq!(path_clean(""), ".");
        assert_eq!(path_clean("../secret"), "../secret");
        assert_eq!(path_clean("a/../b"), "b");
        assert_eq!(path_clean("/a/../../b"), "/b");
        assert_eq!(path_clean("a//./b/"), "a/b");
    }

    #[test]
    fn legacy_mapping_and_reason_code_helpers_match_go() {
        let err = invalid_profile_reason("  ");
        assert_eq!(validation_reason_code(&err), "profile_validation_failed");
        assert_eq!(
            err.to_string(),
            "profile validation failed: profile_validation_failed"
        );
        assert_eq!(
            validation_reason_code(&ProfilesError::ScopedBindingDeferred),
            ""
        );

        let mut input = MutationInput {
            display_name: "Support".to_string(),
            ..MutationInput::default()
        };
        input.legacy_mapping_evidence = vec![LegacyMappingEvidence {
            source_kind: "provider_defaults".to_string(),
            mapping_state: OverlayValidationState::PARTIAL,
            reason_code: "legacy_provider_default_partial".to_string(),
            redaction_status: RedactionStatus::REDACTED,
            ..LegacyMappingEvidence::default()
        }];
        assert!(validate_mutation(&input).is_ok());

        let mut bad_state = input.clone();
        bad_state.legacy_mapping_evidence[0].mapping_state =
            OverlayValidationState::from("garbage");
        let err = validate_mutation(&bad_state).expect_err("unknown mapping state must fail");
        assert_eq!(validation_reason_code(&err), "legacy_mapping_state_invalid");

        let mut bad_redaction = input;
        bad_redaction.legacy_mapping_evidence[0].redaction_status = RedactionStatus::FAILED;
        let err = validate_mutation(&bad_redaction).expect_err("failed redaction must be rejected");
        assert_eq!(
            validation_reason_code(&err),
            "legacy_mapping_redaction_invalid"
        );
    }

    #[test]
    fn change_kind_and_identity_fields_accept_safe_values() {
        let input = MutationInput {
            display_name: "Support".to_string(),
            display_identity: DisplayIdentity {
                name: "Support".to_string(),
                ..DisplayIdentity::default()
            },
            ..MutationInput::default()
        };
        assert!(validate_mutation(&input).is_ok());
        assert_eq!(ChangeKind::VALIDATED.as_str(), "validated");
    }
}
