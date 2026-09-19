//! Workspace/capability binding lifecycle, visibility-change, and runtime
//! projection events (port of `workspace_capability_bindings.go`).

use serde::{Deserialize, Serialize};

use crate::util::payload;
use crate::{Event, Resource};
use kura_bindings::{
    RedactionStatus as BindingRedactionStatus, RuntimeBindingEvidence, Visibility,
    VisibilityScopeKind, safe_label, safe_reason,
};

/// Go: `BindingLifecycleInput` — a binding/workspace lifecycle or denial
/// event. Payload carries only safe, redacted fields (FR-028).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BindingLifecycleInput {
    pub tenant_id: String,
    pub binding_id: String,
    pub workspace_id: String,
    pub actor_principal_id: String,
    pub event_name: String,
    pub outcome: String,
    pub reason_code: String,
    pub permission_gate: String,
    pub safe_summary: String,
    pub previous_selection_summary: String,
    pub resulting_selection_summary: String,
    pub audit_event_id: String,
}

/// Go: `BindingLifecycleEvent` — the resource id falls back to the workspace
/// id when no binding id is present (FR-010).
#[must_use]
pub fn binding_lifecycle_event(input: BindingLifecycleInput) -> Event {
    let resource_id = if input.binding_id.is_empty() {
        input.workspace_id.clone()
    } else {
        input.binding_id.clone()
    };
    Event {
        category: "binding".to_string(),
        name: input.event_name.clone(),
        tenant_id: input.tenant_id.clone(),
        occurred_at: crate::util::now_utc(),
        resource: Resource {
            kind: "workspace_capability_binding".to_string(),
            id: resource_id,
        },
        payload: payload![
            "bindingId" => input.binding_id,
            "workspaceId" => input.workspace_id,
            "actorPrincipalId" => input.actor_principal_id,
            "outcome" => input.outcome,
            "reasonCode" => input.reason_code,
            "permissionGate" => input.permission_gate,
            "safeSummary" => safe_label(&input.safe_summary),
            "previousSelectionSummary" => safe_label(&input.previous_selection_summary),
            "resultingSelectionSummary" => safe_label(&input.resulting_selection_summary),
            "auditEventId" => input.audit_event_id,
            "redactionStatus" => BindingRedactionStatus::REDACTED.as_str(),
        ],
        ..Event::default()
    }
}

/// Go: `CapabilityVisibilityChangedInput`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityVisibilityChangedInput {
    pub tenant_id: String,
    pub actor_principal_id: String,
    pub scope_kind: VisibilityScopeKind,
    pub scope_ref: String,
    pub capability_id: String,
    pub visibility: Visibility,
    pub audit_event_id: String,
}

/// Go: `CapabilityVisibilityChangedEvent` — capability and scope references
/// are safe-labeled before projection.
#[must_use]
pub fn capability_visibility_changed_event(input: CapabilityVisibilityChangedInput) -> Event {
    Event {
        category: "binding".to_string(),
        name: "capability_visibility.changed".to_string(),
        tenant_id: input.tenant_id.clone(),
        occurred_at: crate::util::now_utc(),
        resource: Resource {
            kind: "capability_visibility_policy".to_string(),
            id: safe_label(&input.capability_id),
        },
        payload: payload![
            "actorPrincipalId" => input.actor_principal_id,
            "scopeKind" => input.scope_kind.as_str(),
            "scopeRef" => safe_label(&input.scope_ref),
            "capabilityId" => safe_label(&input.capability_id),
            "visibility" => input.visibility.as_str(),
            "auditEventId" => input.audit_event_id,
            "redactionStatus" => BindingRedactionStatus::REDACTED.as_str(),
        ],
        ..Event::default()
    }
}

/// Go: `BindingRuntimeProjectedEvent` — runtime binding evidence (FR-013).
#[must_use]
pub fn binding_runtime_projected_event(evidence: RuntimeBindingEvidence) -> Event {
    Event {
        category: "binding".to_string(),
        name: "binding.runtime_projected".to_string(),
        tenant_id: evidence.tenant_id.clone(),
        occurred_at: evidence.occurred_at,
        resource: Resource {
            kind: evidence.resource_kind.clone(),
            id: evidence.resource_id.clone(),
        },
        payload: payload![
            "projectionId" => evidence.projection_id,
            "selectedProfileId" => evidence.selected_profile_id,
            "selectedWorkspaceId" => evidence.selected_workspace_id,
            "bindingScope" => evidence.binding_scope.as_str(),
            "bindingId" => evidence.binding_id,
            "classification" => evidence.classification.as_str(),
            "selectionReason" => safe_reason(&evidence.selection_reason),
            "redactionStatus" => evidence.redaction_status.as_str(),
        ],
        ..Event::default()
    }
}
