//! Connector conformance helpers (port of daemon/internal/connectors/conformance.go):
//! capability profiles, account-binding summaries, group-room/handoff capability
//! declarations, and matrix-case evaluation into conformance results.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::supervisor::ConnectorsError;
use crate::{ConformanceResult, ConformanceResultStatus, RedactionStatus};

// Core conformance area (Go `ConformanceArea`). `TenantOwnership` is the
// default, matching Go's zero value.
crate::string_enum!(ConformanceArea {
    TenantOwnership => "tenant_ownership",
    PermissionGating => "permission_gating",
    Redaction => "redaction",
    ActiveTenantAccountBinding => "active_tenant_account_binding",
    InboundIdentity => "inbound_identity",
    DurableDedupe => "durable_dedupe",
    StableRoutingDecisions => "stable_routing_decisions",
    MinimumForegroundReply => "minimum_foreground_reply",
    RequiredDiagnostics => "required_diagnostics",
    DeliverySeparation => "delivery_separation",
});

// Surface support level (Go `SurfaceSupport`). `Unsupported` is the default
// variant (listed first so `#[default]` lands on it), matching how Go treats
// the empty-string zero value in `surfaceResult`.
crate::string_enum!(SurfaceSupport {
    Unsupported => "unsupported",
    Supported => "supported",
    Limited => "limited",
});

// --- matrix constants (conformance.go const block) ------------------------

pub const CONNECTOR_KIND_MATRIX: &str = "matrix";

pub const MATRIX_DURABLE_IDENTITY_RULE_ID: &str = "matrix_homeserver_conversation_event_id";
pub const MATRIX_DURABLE_IDENTITY_RULE: &str =
    "tenant_id + connector_id + homeserver_id + conversation_id + matrix_event_id";

pub const MATRIX_SURFACE_TENANT_PROVIDED_BOT_SETUP: &str = "tenant_provided_bot_setup";
pub const MATRIX_SURFACE_HOSTED_HOMESERVER: &str = "kuraagent_hosted_homeserver";
pub const MATRIX_SURFACE_ACCOUNT_PROVISIONING: &str = "matrix_account_provisioning";
pub const MATRIX_SURFACE_DIRECT_MESSAGE: &str = "direct_message";
pub const MATRIX_SURFACE_ALLOWED_ROOM_MENTION: &str = "allowed_room_mention";
pub const MATRIX_SURFACE_ALLOWED_ROOM_COMMAND: &str = "allowed_room_command";
pub const MATRIX_SURFACE_UNENCRYPTED_TEXT: &str = "unencrypted_text";
pub const MATRIX_SURFACE_ENCRYPTED_ROOMS: &str = "encrypted_rooms";
pub const MATRIX_SURFACE_UNDECRYPTABLE_EVENTS: &str = "undecryptable_events";
pub const MATRIX_SURFACE_FINAL_ONLY_FOREGROUND_REPLY: &str = "final_only_foreground_reply";
pub const MATRIX_SURFACE_CONNECTOR_BACKED_DELIVERY: &str = "connector_backed_delivery";

pub const GROUP_ROOM_SURFACE_MENTION_EVIDENCE: &str = "group_room_mention_evidence";
pub const GROUP_ROOM_SURFACE_ALLOWLIST_EVIDENCE: &str = "group_room_allowlist_evidence";
pub const GROUP_ROOM_SURFACE_UNSUPPORTED_SOURCE_EVIDENCE: &str =
    "group_room_unsupported_source_evidence";
pub const GROUP_ROOM_SURFACE_DUPLICATE_MESSAGE_EVIDENCE: &str =
    "group_room_duplicate_message_evidence";
pub const GROUP_ROOM_SURFACE_EDITED_MESSAGE_EVIDENCE: &str = "group_room_edited_message_evidence";
pub const GROUP_ROOM_SURFACE_DELETED_MESSAGE_EVIDENCE: &str = "group_room_deleted_message_evidence";

pub const HANDOFF_SURFACE_SOURCE_SUPPORT: &str = "handoff_source_support";
pub const HANDOFF_SURFACE_DESTINATION_SUPPORT: &str = "handoff_destination_support";
pub const HANDOFF_SURFACE_FIRST_RESPONSE_SOURCE_REFERENCES: &str =
    "handoff_first_response_source_references";

// --- wire structs ---------------------------------------------------------

/// Go `AccountBindingSummary`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountBindingSummary {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_id: String,
    pub connector_account_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider_account_label: String,
    pub permission_state: String,
    pub redaction_status: RedactionStatus,
}

/// Go `CapabilityProfile`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityProfile {
    pub profile_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub connector_id: String,
    pub connector_kind: String,
    pub core_invariant_results: HashMap<ConformanceArea, ConformanceResultStatus>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub provider_surface_results: HashMap<String, SurfaceSupport>,
    /// Go `json:"groupRoomCapabilities,omitempty"`: `omitempty` never omits a
    /// struct value, so the field is always serialized.
    #[serde(default)]
    pub group_room_capabilities: GroupRoomCapabilities,
    #[serde(default)]
    pub handoff_capabilities: HandoffCapabilities,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub equivalent_durable_identity_rule_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub equivalent_durable_identity_rule: String,
    pub declared_at: DateTime<Utc>,
}

/// Go `GroupRoomCapabilities`. Capability fields are `Option<SurfaceSupport>`:
/// `None` corresponds to Go's empty-string zero value (skipped by
/// `surface_results` and JSON `omitempty`), `Some(...)` to an explicit
/// declaration, so an explicit `unsupported` stays distinguishable.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupRoomCapabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mention_evidence: Option<SurfaceSupport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowlist_evidence: Option<SurfaceSupport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unsupported_source_evidence: Option<SurfaceSupport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duplicate_message_evidence: Option<SurfaceSupport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_message_evidence: Option<SurfaceSupport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_message_evidence: Option<SurfaceSupport>,
}

impl GroupRoomCapabilities {
    /// Go `surfaceResults`: maps every declared capability to its surface key.
    #[must_use]
    pub fn surface_results(&self) -> HashMap<String, SurfaceSupport> {
        let mut results = HashMap::new();
        if let Some(value) = self.mention_evidence {
            results.insert(GROUP_ROOM_SURFACE_MENTION_EVIDENCE.to_string(), value);
        }
        if let Some(value) = self.allowlist_evidence {
            results.insert(GROUP_ROOM_SURFACE_ALLOWLIST_EVIDENCE.to_string(), value);
        }
        if let Some(value) = self.unsupported_source_evidence {
            results.insert(
                GROUP_ROOM_SURFACE_UNSUPPORTED_SOURCE_EVIDENCE.to_string(),
                value,
            );
        }
        if let Some(value) = self.duplicate_message_evidence {
            results.insert(
                GROUP_ROOM_SURFACE_DUPLICATE_MESSAGE_EVIDENCE.to_string(),
                value,
            );
        }
        if let Some(value) = self.edited_message_evidence {
            results.insert(
                GROUP_ROOM_SURFACE_EDITED_MESSAGE_EVIDENCE.to_string(),
                value,
            );
        }
        if let Some(value) = self.deleted_message_evidence {
            results.insert(
                GROUP_ROOM_SURFACE_DELETED_MESSAGE_EVIDENCE.to_string(),
                value,
            );
        }
        results
    }
}

/// Go `HandoffCapabilities` (same `Option` semantics as `GroupRoomCapabilities`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffCapabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_support: Option<SurfaceSupport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination_support: Option<SurfaceSupport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_response_source_references: Option<SurfaceSupport>,
}

impl HandoffCapabilities {
    /// Go `surfaceResults`: maps every declared capability to its surface key.
    #[must_use]
    pub fn surface_results(&self) -> HashMap<String, SurfaceSupport> {
        let mut results = HashMap::new();
        if let Some(value) = self.source_support {
            results.insert(HANDOFF_SURFACE_SOURCE_SUPPORT.to_string(), value);
        }
        if let Some(value) = self.destination_support {
            results.insert(HANDOFF_SURFACE_DESTINATION_SUPPORT.to_string(), value);
        }
        if let Some(value) = self.first_response_source_references {
            results.insert(
                HANDOFF_SURFACE_FIRST_RESPONSE_SOURCE_REFERENCES.to_string(),
                value,
            );
        }
        results
    }
}

/// Input to [`run_matrix_case`] (Go `MatrixCase`; carries no JSON tags).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MatrixCase {
    pub scenario_id: String,
    pub connector_kind: String,
    pub connector_id: String,
    pub tenant_id: String,
    pub core_invariant_results: HashMap<ConformanceArea, ConformanceResultStatus>,
    pub provider_surface_results: HashMap<String, SurfaceSupport>,
    pub group_room_capabilities: GroupRoomCapabilities,
    pub handoff_capabilities: HandoffCapabilities,
    pub equivalent_durable_identity_rule_id: String,
    pub equivalent_durable_identity_rule: String,
    pub unsafe_incremental_update_degraded: bool,
    /// `RedactionStatus::default()` is `Redacted`, matching Go's empty-string
    /// default in `RunMatrixCase`.
    pub redaction_status: RedactionStatus,
    /// Zero (`DateTime::default()`) falls back to `Utc::now()`.
    pub now: DateTime<Utc>,
}

/// Go `CoreInvariantAreas`: the ten core areas in declaration order.
#[must_use]
pub fn core_invariant_areas() -> Vec<ConformanceArea> {
    vec![
        ConformanceArea::TenantOwnership,
        ConformanceArea::PermissionGating,
        ConformanceArea::Redaction,
        ConformanceArea::ActiveTenantAccountBinding,
        ConformanceArea::InboundIdentity,
        ConformanceArea::DurableDedupe,
        ConformanceArea::StableRoutingDecisions,
        ConformanceArea::MinimumForegroundReply,
        ConformanceArea::RequiredDiagnostics,
        ConformanceArea::DeliverySeparation,
    ]
}

/// Go `ValidateCapabilityProfile`.
pub fn validate_capability_profile(profile: &CapabilityProfile) -> Result<(), ConnectorsError> {
    if profile.connector_id.trim().is_empty() {
        return Err(ConnectorsError::ConnectorIdRequired);
    }
    if profile.connector_kind.trim().is_empty() {
        return Err(ConnectorsError::ConnectorKindRequired);
    }
    for area in core_invariant_areas() {
        if profile.core_invariant_results.get(&area).copied() != Some(ConformanceResultStatus::Pass)
        {
            return Err(ConnectorsError::CoreInvariantFailed);
        }
    }
    if !profile
        .equivalent_durable_identity_rule_id
        .trim()
        .is_empty()
        && profile.equivalent_durable_identity_rule.trim().is_empty()
    {
        return Err(ConnectorsError::EquivalentIdentityRequired);
    }
    Ok(())
}

/// Go `RunMatrixCase`: evaluates a matrix case into conformance results and a
/// capability profile.
pub fn run_matrix_case(
    input: MatrixCase,
) -> Result<(Vec<ConformanceResult>, CapabilityProfile), ConnectorsError> {
    if input.scenario_id.trim().is_empty() {
        return Err(ConnectorsError::ConformanceScenarioRequired);
    }
    if input.connector_kind.trim().is_empty() {
        return Err(ConnectorsError::ConformanceKindRequired);
    }
    if !input.equivalent_durable_identity_rule_id.trim().is_empty()
        && input.equivalent_durable_identity_rule.trim().is_empty()
    {
        return Err(ConnectorsError::EquivalentIdentityRequired);
    }

    // Go `now.IsZero()` -> the derived Default is the Unix epoch.
    let now = if input.now == DateTime::<Utc>::default() {
        Utc::now()
    } else {
        input.now
    };
    // Go's empty redaction status defaults to Redacted; the enum's Default is
    // already Redacted, so no transformation is needed.
    let redaction_status = input.redaction_status;

    // Missing core areas default to Fail (Go zero value).
    let mut core: HashMap<ConformanceArea, ConformanceResultStatus> =
        HashMap::with_capacity(core_invariant_areas().len());
    for area in core_invariant_areas() {
        let result = input
            .core_invariant_results
            .get(&area)
            .copied()
            .unwrap_or(ConformanceResultStatus::Fail);
        core.insert(area, result);
    }

    let mut surfaces: HashMap<String, SurfaceSupport> = input.provider_surface_results.clone();
    for (key, value) in input.group_room_capabilities.surface_results() {
        surfaces.insert(key, value);
    }
    for (key, value) in input.handoff_capabilities.surface_results() {
        surfaces.insert(key, value);
    }
    if input.unsafe_incremental_update_degraded {
        surfaces.insert(
            "incremental_visible_updates".to_string(),
            SurfaceSupport::Limited,
        );
    }

    let mut results = Vec::with_capacity(core.len() + surfaces.len());
    for area in core_invariant_areas() {
        let value = core[&area];
        results.push(ConformanceResult {
            conformance_result_id: format!("conf_{}_{}", input.scenario_id, area.as_str()),
            tenant_id: input.tenant_id.clone(),
            connector_kind: input.connector_kind.clone(),
            connector_id: input.connector_id.clone(),
            scenario_id: input.scenario_id.clone(),
            area: area.as_str().to_string(),
            result: value,
            reason_code: core_reason_code(value).to_string(),
            redaction_status,
            evidence_timestamp: now,
            retention_expires_at: now + chrono::Duration::days(90),
        });
    }
    let mut surface_keys: Vec<String> = surfaces.keys().cloned().collect();
    surface_keys.sort();
    for key in surface_keys {
        let value = surfaces[&key];
        results.push(ConformanceResult {
            conformance_result_id: format!("conf_{}_{}", input.scenario_id, key),
            tenant_id: input.tenant_id.clone(),
            connector_kind: input.connector_kind.clone(),
            connector_id: input.connector_id.clone(),
            scenario_id: input.scenario_id.clone(),
            area: key,
            result: surface_result(value),
            reason_code: String::new(),
            redaction_status,
            evidence_timestamp: now,
            retention_expires_at: now + chrono::Duration::days(90),
        });
    }

    let profile = CapabilityProfile {
        profile_id: format!("profile_{}", input.scenario_id),
        tenant_id: input.tenant_id.clone(),
        connector_id: input.connector_id.clone(),
        connector_kind: input.connector_kind.clone(),
        core_invariant_results: core,
        provider_surface_results: surfaces,
        group_room_capabilities: input.group_room_capabilities,
        handoff_capabilities: input.handoff_capabilities,
        equivalent_durable_identity_rule_id: input.equivalent_durable_identity_rule_id,
        equivalent_durable_identity_rule: input.equivalent_durable_identity_rule,
        declared_at: now,
    };
    Ok((results, profile))
}

/// Go `surfaceResult`: maps a surface support level to a conformance status.
#[must_use]
pub fn surface_result(surface: SurfaceSupport) -> ConformanceResultStatus {
    match surface {
        SurfaceSupport::Supported => ConformanceResultStatus::Supported,
        SurfaceSupport::Limited => ConformanceResultStatus::Limited,
        SurfaceSupport::Unsupported => ConformanceResultStatus::Unsupported,
    }
}

/// Go `coreReasonCode`: `core_invariant_failed` for failed core results, else empty.
#[must_use]
pub fn core_reason_code(result: ConformanceResultStatus) -> &'static str {
    if result == ConformanceResultStatus::Fail {
        "core_invariant_failed"
    } else {
        ""
    }
}
