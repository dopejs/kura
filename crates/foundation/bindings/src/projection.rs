//! Runtime binding evidence construction (port of projection.go): materializes an
//! EffectiveBindingSelection into durable, redacted evidence (FR-013, FR-026, B22).

use chrono::{DateTime, Utc};

use crate::redaction::safe_reason;
use crate::types::{
    CapabilityDecision, Classification, EffectiveBindingSelection, RedactionStatus,
    ResolutionOutcome, RuntimeBindingEvidence,
};
use crate::visibility::safe_scope_label;

/// Carries the per-run facts needed to build durable runtime binding evidence in the
/// same work-start pass as the profile runtime projection.
#[derive(Debug, Clone)]
pub struct RuntimeBindingEvidenceInput {
    pub projection_id: String,
    pub tenant_id: String,
    pub resource_kind: String,
    pub resource_id: String,
    pub occurred_at: DateTime<Utc>,
    /// Marks runs that predate explicit binding support so evidence is labeled
    /// legacy_default rather than presented as user-configured (FR-026).
    pub legacy_default: bool,
}

impl Default for RuntimeBindingEvidenceInput {
    fn default() -> Self {
        Self {
            projection_id: String::new(),
            tenant_id: String::new(),
            resource_kind: String::new(),
            resource_id: String::new(),
            occurred_at: DateTime::<Utc>::UNIX_EPOCH,
            legacy_default: false,
        }
    }
}

/// Materializes an EffectiveBindingSelection into durable, redacted runtime evidence.
/// It flips the planted profile-projection deferral marker by recording
/// `Classification::APPLIED` when an explicit binding influenced the run (FR-013,
/// FR-026, B22).
pub fn build_runtime_binding_evidence(
    sel: &EffectiveBindingSelection,
    input: &RuntimeBindingEvidenceInput,
) -> RuntimeBindingEvidence {
    let classification = if input.legacy_default {
        Classification::LEGACY
    } else if sel.outcome == ResolutionOutcome::RESOLVED {
        Classification::APPLIED
    } else {
        Classification::DEFAULT
    };

    let reason = if sel.outcome == ResolutionOutcome::RESOLVED {
        "explicit_binding_selection".to_string()
    } else if sel.outcome == ResolutionOutcome::REPAIR_REQUIRED {
        safe_reason(&sel.repair_reason)
    } else {
        "tenant_default_selection".to_string()
    };

    let decisions = sel
        .capability_visibility
        .iter()
        .map(|d| CapabilityDecision {
            capability_id: d.capability_id.trim().to_string(),
            effective: d.effective.clone(),
            default_enabled: d.default_enabled,
            offered: d.offered,
            executable: d.executable,
            reason: safe_reason(&d.reason),
            scope: safe_scope_label(&d.scope),
        })
        .collect();

    RuntimeBindingEvidence {
        projection_id: input.projection_id.trim().to_string(),
        tenant_id: input.tenant_id.trim().to_string(),
        resource_kind: input.resource_kind.trim().to_string(),
        resource_id: input.resource_id.trim().to_string(),
        selected_profile_id: sel.selected_profile_id.trim().to_string(),
        selected_profile_version_id: sel.selected_profile_version_id.trim().to_string(),
        selected_workspace_id: sel.selected_workspace_id.trim().to_string(),
        binding_scope: sel.binding_scope.clone(),
        binding_id: sel.binding_id.trim().to_string(),
        classification,
        selection_reason: reason,
        capability_visibility: decisions,
        occurred_at: input.occurred_at,
        redaction_status: RedactionStatus::REDACTED,
    }
}
