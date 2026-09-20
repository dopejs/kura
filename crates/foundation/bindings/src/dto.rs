//! Request/response DTOs (port of dto.go) shared by the store and API layers.
//! Responses surface only safe, redacted fields (FR-028); scope references are
//! presented via `safe_label`.

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use crate::redaction::{safe_label, safe_reason};
use crate::types::{
    BindingRule, BindingRuntimeScope, BindingStatus, CapabilityVisibilityPolicy, Classification,
    EffectiveVisibility, RedactionStatus, RepairStatus, RuntimeBindingEvidence, ScopeKind,
    ValidationStatus, Visibility, VisibilityScopeKind, Workspace, WorkspaceStatus,
};

/// The body for POST /v1/workspaces.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateWorkspaceRequest {
    pub display_name: String,
}

/// The body for PATCH /v1/workspaces/{id}.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateWorkspaceRequest {
    pub status: WorkspaceStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
}

/// The body for POST /v1/bindings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateBindingRequest {
    pub scope_kind: ScopeKind,
    pub scope_ref: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub selected_profile_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub selected_workspace_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
}

/// The body for PATCH /v1/bindings/{id}.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateBindingRequest {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub selected_profile_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub selected_workspace_id: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub disable: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// The body for PUT /v1/capability-visibility.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetVisibilityRequest {
    pub scope_kind: VisibilityScopeKind,
    pub scope_ref: String,
    pub capability_id: String,
    pub visibility: Visibility,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
}

/// The safe JSON view of a workspace.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceResource {
    pub workspace_id: String,
    pub tenant_id: String,
    pub display_name: String,
    pub status: WorkspaceStatus,
    pub is_default: bool,
    pub repair_status: RepairStatus,
    pub redaction_status: RedactionStatus,
    pub created_at: String,
    pub updated_at: String,
}

/// The safe JSON view of a binding rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BindingResource {
    pub binding_id: String,
    pub tenant_id: String,
    pub scope_kind: ScopeKind,
    pub scope_label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub selected_profile_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub selected_profile_version_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub selected_workspace_id: String,
    pub status: BindingStatus,
    pub repair_status: RepairStatus,
    pub validation_status: ValidationStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub resulting_selection_summary: String,
    pub last_material_change_at: String,
    pub redaction_status: RedactionStatus,
}

/// The safe JSON view of a visibility policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityVisibilityResource {
    pub policy_id: String,
    pub tenant_id: String,
    pub scope_kind: VisibilityScopeKind,
    pub scope_ref: String,
    pub capability_id: String,
    pub visibility: Visibility,
    pub validation_status: ValidationStatus,
}

/// The safe JSON view of runtime binding evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BindingRuntimeEvidenceResource {
    pub projection_id: String,
    pub resource_kind: String,
    pub resource_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub selected_profile_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub selected_profile_version_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub selected_workspace_id: String,
    pub binding_scope: BindingRuntimeScope,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub binding_id: String,
    pub classification: Classification,
    pub selection_reason: String,
    #[serde(rename = "capabilityVisibilitySummary")]
    pub capability_visibility: Vec<CapabilityDecisionResource>,
    pub occurred_at: String,
    pub redaction_status: RedactionStatus,
}

/// The safe JSON view of one capability decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityDecisionResource {
    pub capability_id: String,
    pub effective: EffectiveVisibility,
    pub default_enabled: bool,
    pub offered: bool,
    pub executable: bool,
    pub reason: String,
    pub scope: String,
}

/// Formats like Go's RFC3339Nano layout: trailing fractional zeros trimmed, no
/// fraction when zero, "Z" for UTC.
fn rfc3339_nano(ts: &DateTime<Utc>) -> String {
    let full = ts.to_rfc3339_opts(SecondsFormat::Nanos, true);
    // full is "…T15:04:05.fffffffffZ"; trim trailing zeros from the fraction and drop
    // the dot entirely when the fraction is zero (Go's ".999999999" behavior).
    match full.rfind('.') {
        Some(dot) => {
            let (head, frac_z) = full.split_at(dot);
            let frac = frac_z[1..frac_z.len() - 1].trim_end_matches('0');
            if frac.is_empty() {
                format!("{head}Z")
            } else {
                format!("{head}.{frac}Z")
            }
        }
        None => full,
    }
}

/// Maps a Workspace to its safe JSON view.
pub fn to_workspace_resource(ws: &Workspace) -> WorkspaceResource {
    WorkspaceResource {
        workspace_id: ws.workspace_id.clone(),
        tenant_id: ws.tenant_id.clone(),
        display_name: safe_label(&ws.display_name),
        status: ws.status.clone(),
        is_default: ws.is_default,
        repair_status: ws.repair_status.clone(),
        redaction_status: ws.redaction_status.clone(),
        created_at: rfc3339_nano(&ws.created_at),
        updated_at: rfc3339_nano(&ws.updated_at),
    }
}

/// Maps a BindingRule to its safe JSON view.
pub fn to_binding_resource(b: &BindingRule) -> BindingResource {
    BindingResource {
        binding_id: b.binding_id.clone(),
        tenant_id: b.tenant_id.clone(),
        scope_kind: b.scope_kind.clone(),
        scope_label: safe_label(&b.scope_ref),
        selected_profile_id: b.selected_profile_id.clone(),
        selected_profile_version_id: b.selected_profile_version_id.clone(),
        selected_workspace_id: b.selected_workspace_id.clone(),
        status: b.status.clone(),
        repair_status: b.repair_status.clone(),
        validation_status: b.validation_status.clone(),
        resulting_selection_summary: safe_label(&b.resulting_selection_summary),
        last_material_change_at: rfc3339_nano(&b.updated_at),
        redaction_status: b.redaction_status.clone(),
    }
}

/// Maps a policy to its safe JSON view.
pub fn to_capability_visibility_resource(
    p: &CapabilityVisibilityPolicy,
) -> CapabilityVisibilityResource {
    CapabilityVisibilityResource {
        policy_id: p.policy_id.clone(),
        tenant_id: p.tenant_id.clone(),
        scope_kind: p.scope_kind.clone(),
        scope_ref: safe_label(&p.scope_ref),
        capability_id: safe_label(&p.capability_id),
        visibility: p.visibility.clone(),
        validation_status: p.validation_status.clone(),
    }
}

/// Maps runtime evidence to its safe JSON view.
pub fn to_runtime_evidence_resource(e: &RuntimeBindingEvidence) -> BindingRuntimeEvidenceResource {
    let decisions = e
        .capability_visibility
        .iter()
        .map(|d| CapabilityDecisionResource {
            capability_id: safe_label(&d.capability_id),
            effective: d.effective.clone(),
            default_enabled: d.default_enabled,
            offered: d.offered,
            executable: d.executable,
            reason: safe_reason(&d.reason),
            scope: d.scope.clone(),
        })
        .collect();
    BindingRuntimeEvidenceResource {
        projection_id: e.projection_id.clone(),
        resource_kind: e.resource_kind.clone(),
        resource_id: e.resource_id.clone(),
        selected_profile_id: e.selected_profile_id.clone(),
        selected_profile_version_id: e.selected_profile_version_id.clone(),
        selected_workspace_id: e.selected_workspace_id.clone(),
        binding_scope: e.binding_scope.clone(),
        binding_id: e.binding_id.clone(),
        classification: e.classification.clone(),
        selection_reason: safe_reason(&e.selection_reason),
        capability_visibility: decisions,
        occurred_at: rfc3339_nano(&e.occurred_at),
        redaction_status: e.redaction_status.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CapabilityDecision, EffectiveVisibility};

    // The Go package has no dto_test.go; these pin the wire-format details the Go
    // code guarantees: RFC3339Nano timestamps, omitempty fields, and SafeLabel
    // redaction in mapped resources (FR-028).
    #[test]
    fn rfc3339_nano_matches_go_layout() {
        assert_eq!(
            rfc3339_nano(&DateTime::<Utc>::UNIX_EPOCH),
            "1970-01-01T00:00:00Z"
        );
        let nanos = DateTime::from_timestamp_nanos(1_234_567_890);
        assert_eq!(rfc3339_nano(&nanos), "1970-01-01T00:00:01.23456789Z");
    }

    #[test]
    fn workspace_resource_redacts_display_name() {
        let ws = Workspace {
            workspace_id: "ws_1".into(),
            tenant_id: "ten_1".into(),
            display_name: "secret=hunter2".into(),
            status: WorkspaceStatus::ACTIVE,
            is_default: true,
            repair_status: RepairStatus::HEALTHY,
            redaction_status: RedactionStatus::REDACTED,
            ..Default::default()
        };
        let resource = to_workspace_resource(&ws);
        assert_eq!(resource.display_name, "(redacted)");
        assert_eq!(resource.created_at, "1970-01-01T00:00:00Z");
    }

    #[test]
    fn binding_resource_omits_empty_selection_ids() {
        let rule = BindingRule {
            binding_id: "bnd_1".into(),
            tenant_id: "ten_1".into(),
            scope_kind: ScopeKind::CHANNEL,
            scope_ref: "discord:chan_123".into(),
            status: BindingStatus::ACTIVE,
            ..Default::default()
        };
        let json = serde_json::to_value(to_binding_resource(&rule)).unwrap();
        assert_eq!(json["scopeLabel"], "discord:chan_123");
        assert!(
            json.get("selectedProfileId").is_none(),
            "omitempty must drop empty ids: {json}"
        );
        // SafeLabel never yields an empty string, so omitempty never drops the summary.
        assert_eq!(json["resultingSelectionSummary"], "(unnamed)");
    }

    #[test]
    fn runtime_evidence_resource_serializes_summary_field() {
        let evidence = RuntimeBindingEvidence {
            projection_id: "brp_1".into(),
            resource_kind: "thread".into(),
            resource_id: "thr_1".into(),
            binding_scope: BindingRuntimeScope::CHANNEL,
            classification: Classification::APPLIED,
            selection_reason: "explicit_binding_selection".into(),
            capability_visibility: vec![CapabilityDecision {
                capability_id: "cap.x".into(),
                effective: EffectiveVisibility::VISIBLE,
                offered: true,
                executable: true,
                reason: "visible_by_policy".into(),
                scope: "profile".into(),
                ..Default::default()
            }],
            redaction_status: RedactionStatus::REDACTED,
            ..Default::default()
        };
        let json = serde_json::to_value(to_runtime_evidence_resource(&evidence)).unwrap();
        assert!(json.get("capabilityVisibilitySummary").is_some());
        assert_eq!(json["classification"], "applied_binding");
        assert_eq!(json["bindingScope"], "channel");
    }
}
