//! Runtime projection construction ported from daemon/internal/profiles/projection.go.

use chrono::{DateTime, Utc};

use crate::redaction::safe_profile_summary;
use crate::types::{
    ActiveSelection, AgentProfile, LegacyMappingEvidence, OverlayReferenceInput,
    OverlayValidationState, RedactionStatus, RuntimeProjection, RuntimeResourceKind,
};

/// Pre-Roadmap-58 placeholder recorded when no explicit binding influenced a run.
pub const DEFERRED_BINDING_CLASSIFICATION_MARKER: &str = "roadmap_58_deferred_binding_unapplied";

/// Recorded when an explicit binding influenced a run (FR-026).
pub const APPLIED_BINDING_CLASSIFICATION_MARKER: &str = "roadmap_58_applied_binding";

/// Go: `RuntimeProjectionInput`. `occurred_at: None` maps to Go's zero
/// `time.Time`, which `BuildRuntimeProjection` replaces with `time.Now()`.
#[derive(Debug, Clone, Default)]
pub struct RuntimeProjectionInput {
    pub resource_kind: RuntimeResourceKind,
    pub resource_id: String,
    pub thread_id: String,
    pub session_id: String,
    pub run_id: String,
    pub workflow_id: String,
    pub handoff_id: String,
    pub occurred_at: Option<DateTime<Utc>>,
    /// Overrides the binding classification recorded on the projection. Empty
    /// preserves the pre-Roadmap-58 deferred marker; Roadmap 58 sets
    /// [`APPLIED_BINDING_CLASSIFICATION_MARKER`] when an explicit binding
    /// influenced the run (FR-026).
    pub binding_classification: String,
}

/// Go: `BuildRuntimeProjection` — builds the safe runtime evidence record for
/// a profile selection. The projection ID is assigned by the caller/store.
pub fn build_runtime_projection(
    profile: &AgentProfile,
    selection: &ActiveSelection,
    input: RuntimeProjectionInput,
) -> RuntimeProjection {
    let occurred_at = input.occurred_at.unwrap_or_else(Utc::now);
    let classification = if input.binding_classification.is_empty() {
        DEFERRED_BINDING_CLASSIFICATION_MARKER.to_string()
    } else {
        input.binding_classification
    };
    RuntimeProjection {
        tenant_id: profile.tenant_id.clone(),
        profile_id: profile.profile_id.clone(),
        profile_version_id: selection.profile_version_id.clone(),
        selection_id: selection.selection_id.clone(),
        resource_kind: input.resource_kind,
        resource_id: input.resource_id,
        thread_id: input.thread_id,
        session_segment_id: input.session_id,
        run_id: input.run_id,
        workflow_id: input.workflow_id,
        handoff_link_id: input.handoff_id,
        selection_scope: selection.selection_scope.clone(),
        selection_reason: selection.selection_reason.clone(),
        safe_display_name: profile.display_name.clone(),
        safe_summary: safe_profile_summary(profile),
        configuration_scope: "explicit_profile_configuration".to_string(),
        deferred_binding_classification: classification,
        occurred_at,
        redaction_status: RedactionStatus::REDACTED,
        ..RuntimeProjection::default()
    }
}

/// Go: `DefaultLegacyMappingEvidence` — seeded partial evidence explaining
/// that legacy provider/prompt configuration stays external to the profile.
pub fn default_legacy_mapping_evidence() -> Vec<LegacyMappingEvidence> {
    vec![
        LegacyMappingEvidence {
            source_kind: "provider_defaults".to_string(),
            mapping_state: OverlayValidationState::PARTIAL,
            reason_code: "legacy_provider_default_partial".to_string(),
            safe_summary:
                "Provider defaults remain external unless explicitly copied into the profile."
                    .to_string(),
            redaction_status: RedactionStatus::REDACTED,
        },
        LegacyMappingEvidence {
            source_kind: "prompt_config_reference".to_string(),
            mapping_state: OverlayValidationState::PARTIAL,
            reason_code: "legacy_prompt_config_partial".to_string(),
            safe_summary:
                "Legacy prompt/config references are visible as partial evidence and are not hidden profile truth."
                    .to_string(),
            redaction_status: RedactionStatus::REDACTED,
        },
    ]
}

/// Go: `DefaultLegacyOverlayReferenceInputs` — seeded overlay inputs pointing
/// at the tenant-default legacy configuration.
pub fn default_legacy_overlay_reference_inputs() -> Vec<OverlayReferenceInput> {
    vec![
        OverlayReferenceInput {
            reference_kind: "legacy_provider_defaults".to_string(),
            reference_uri: "legacy://provider-defaults/tenant-default".to_string(),
            scope: "profile".to_string(),
        },
        OverlayReferenceInput {
            reference_kind: "legacy_prompt_config".to_string(),
            reference_uri: "legacy://prompt-config/tenant-default".to_string(),
            scope: "profile".to_string(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Persona, SELECTION_SCOPE_TENANT_DEFAULT, SelectionReason};

    #[test]
    fn build_runtime_projection_uses_safe_profile_metadata() {
        let profile = AgentProfile {
            tenant_id: "ten_1".to_string(),
            profile_id: "prof_1".to_string(),
            display_name: "Ops Agent".to_string(),
            persona: Persona {
                safe_summary: "operator profile".to_string(),
                ..Persona::default()
            },
            ..AgentProfile::default()
        };
        let selection = ActiveSelection {
            selection_id: "sel_1".to_string(),
            profile_version_id: "profv_1".to_string(),
            selection_scope: SELECTION_SCOPE_TENANT_DEFAULT.to_string(),
            selection_reason: SelectionReason::USER_ACTIVATED,
            ..ActiveSelection::default()
        };
        let projection = build_runtime_projection(
            &profile,
            &selection,
            RuntimeProjectionInput {
                resource_kind: RuntimeResourceKind::RUN,
                resource_id: "run_1".to_string(),
                run_id: "run_1".to_string(),
                ..RuntimeProjectionInput::default()
            },
        );
        assert_eq!(projection.safe_display_name, "Ops Agent");
        assert_eq!(projection.safe_summary, "operator profile");
        assert_eq!(projection.redaction_status, RedactionStatus::REDACTED);
        assert_eq!(
            projection.deferred_binding_classification,
            DEFERRED_BINDING_CLASSIFICATION_MARKER
        );
        assert_eq!(
            projection.configuration_scope,
            "explicit_profile_configuration"
        );
    }

    #[test]
    fn build_runtime_projection_defaults_occurred_at_and_honors_classification_override() {
        let profile = AgentProfile::default();
        let selection = ActiveSelection::default();
        let projection =
            build_runtime_projection(&profile, &selection, RuntimeProjectionInput::default());
        // None maps to Go's zero time: replaced with now, not left zero.
        let elapsed = Utc::now() - projection.occurred_at;
        assert!(
            elapsed.num_seconds() < 60,
            "occurred_at should default to now"
        );

        let fixed = DateTime::parse_from_rfc3339("2026-05-12T10:00:00Z")
            .expect("fixture time")
            .to_utc();
        let applied = build_runtime_projection(
            &profile,
            &selection,
            RuntimeProjectionInput {
                occurred_at: Some(fixed),
                binding_classification: APPLIED_BINDING_CLASSIFICATION_MARKER.to_string(),
                ..RuntimeProjectionInput::default()
            },
        );
        assert_eq!(applied.occurred_at, fixed);
        assert_eq!(
            applied.deferred_binding_classification,
            APPLIED_BINDING_CLASSIFICATION_MARKER
        );
    }

    #[test]
    fn default_legacy_seeds_validate_as_mutation_inputs() {
        let input = crate::types::MutationInput {
            display_name: "Support".to_string(),
            legacy_mapping_evidence: default_legacy_mapping_evidence(),
            overlay_references: default_legacy_overlay_reference_inputs(),
            ..crate::types::MutationInput::default()
        };
        crate::validate_mutation(&input).expect("seeded legacy defaults must pass validation");
    }
}
