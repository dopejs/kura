//! Agent profile lifecycle, version-created, and runtime-projection events
//! (port of `agent_profiles.go`).

use serde::{Deserialize, Serialize};

use crate::util::{now_utc, payload};
use crate::{Event, Resource};
use kura_profiles::{ChangeKind, RedactionStatus, RuntimeProjection};

/// Go: `AgentProfileLifecycleInput`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProfileLifecycleInput {
    pub tenant_id: String,
    pub profile_id: String,
    pub profile_version_id: String,
    pub actor_principal_id: String,
    pub event_name: String,
    pub outcome: String,
    pub reason_code: String,
    pub permission_gate: String,
    pub safe_summary: String,
    pub audit_event_id: String,
    pub redaction_status: RedactionStatus,
}

/// Go: `AgentProfileLifecycleEvent` — the default redaction status is
/// `redacted` so lifecycle events never leak raw persona material.
#[must_use]
pub fn agent_profile_lifecycle_event(input: AgentProfileLifecycleInput) -> Event {
    let status = if input.redaction_status.is_empty() {
        RedactionStatus::REDACTED
    } else {
        input.redaction_status.clone()
    };
    Event {
        category: "agent_profile".to_string(),
        name: input.event_name.clone(),
        tenant_id: input.tenant_id.clone(),
        occurred_at: now_utc(),
        resource: Resource {
            kind: "agent_profile".to_string(),
            id: input.profile_id.clone(),
        },
        payload: payload![
            "profileId" => input.profile_id,
            "profileVersionId" => input.profile_version_id,
            "actorPrincipalId" => input.actor_principal_id,
            "outcome" => input.outcome,
            "reasonCode" => input.reason_code,
            "permissionGate" => input.permission_gate,
            "safeSummary" => input.safe_summary,
            "auditEventId" => input.audit_event_id,
            "redactionStatus" => status.as_str(),
        ],
        ..Event::default()
    }
}

/// Go: `AgentProfileVersionInput`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProfileVersionInput {
    pub tenant_id: String,
    pub profile_id: String,
    pub profile_version_id: String,
    pub change_kind: ChangeKind,
    pub version_number: i64,
    pub reason_code: String,
    pub redaction_status: RedactionStatus,
}

/// Go: `AgentProfileVersionCreatedEvent`.
#[must_use]
pub fn agent_profile_version_created_event(input: AgentProfileVersionInput) -> Event {
    let status = if input.redaction_status.is_empty() {
        RedactionStatus::REDACTED
    } else {
        input.redaction_status.clone()
    };
    Event {
        category: "agent_profile".to_string(),
        name: "agent_profile.version_created".to_string(),
        tenant_id: input.tenant_id.clone(),
        occurred_at: now_utc(),
        resource: Resource {
            kind: "agent_profile_version".to_string(),
            id: input.profile_version_id.clone(),
        },
        payload: payload![
            "profileId" => input.profile_id,
            "profileVersionId" => input.profile_version_id,
            "changeKind" => input.change_kind.as_str(),
            "versionNumber" => input.version_number,
            "reasonCode" => input.reason_code,
            "redactionStatus" => status.as_str(),
        ],
        ..Event::default()
    }
}

/// Go: `AgentProfileRuntimeProjectedEvent` — metadata-only evidence that a
/// profile projection was applied to a runtime resource.
#[must_use]
pub fn agent_profile_runtime_projected_event(projection: RuntimeProjection) -> Event {
    Event {
        category: "agent_profile".to_string(),
        name: "agent_profile.runtime_projected".to_string(),
        tenant_id: projection.tenant_id.clone(),
        occurred_at: projection.occurred_at,
        resource: Resource {
            kind: projection.resource_kind.as_str().to_string(),
            id: projection.resource_id.clone(),
        },
        payload: payload![
            "runtimeProfileProjectionId" => projection.runtime_profile_projection_id,
            "profileId" => projection.profile_id,
            "profileVersionId" => projection.profile_version_id,
            "selectionId" => projection.selection_id,
            "selectionScope" => projection.selection_scope,
            "selectionReason" => projection.selection_reason.as_str(),
            "safeDisplayName" => projection.safe_display_name,
            "safeSummary" => projection.safe_summary,
            "redactionStatus" => projection.redaction_status.as_str(),
        ],
        ..Event::default()
    }
}
