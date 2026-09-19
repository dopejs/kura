//! Port of the daemon/internal/connectors management layer: the channel
//! management wire records (route policies, routing decisions, reply/delivery
//! outcomes, support-evidence bundles) and their pure predicates, plus the
//! connector management page/projection/detail builders and repair helpers.
//! Sources: management.go, management_support.go, management_routes.go,
//! management_repair.go. Wire values match the Go json tags exactly (camelCase
//! fields, explicit enum literals — note the hyphenated "action-required",
//! "credential-rotation", "route-revalidate", "diagnostic-rerun", "re-enable"
//! literals that are not plain snake_case).
//!
//! The record types previously lived store-local in
//! kura-store/src/channel_management.rs; Go keeps them in the connectors
//! package and kura-store now re-exports them from this crate.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    Connector, ConnectorDiagnosticState, DiagnosticReasonCode, FreshnessState, LifecycleState,
    RedactionStatus, RemediationOwner, RetrySafety, Status, freshness_at,
};

// --- management.go enums ---------------------------------------------------

crate::string_enum!(ManagementState {
    Ready => "ready",
    Disabled => "disabled",
    Degraded => "degraded",
    Unavailable => "unavailable",
    ActionRequired => "action-required",
});

crate::string_enum!(CapabilitySupport {
    Supported => "supported",
    Limited => "limited",
    Unsupported => "unsupported",
});

crate::string_enum!(DiagnosticFreshness {
    Fresh => "fresh",
    Stale => "stale",
});

crate::string_enum!(ManagementActionKind {
    Repair => "repair",
    Reconnect => "reconnect",
    CredentialRotation => "credential-rotation",
    RouteRevalidate => "route-revalidate",
    DiagnosticRerun => "diagnostic-rerun",
    Disable => "disable",
    ReEnable => "re-enable",
});

crate::string_enum!(ManagementTerminalState {
    Ready => "ready",
    Degraded => "degraded",
    Unavailable => "unavailable",
    Disabled => "disabled",
    Cancelled => "cancelled",
    ActionRequired => "action-required",
});

crate::string_enum!(RouteDecisionOutcome {
    Accepted => "accepted",
    Ignored => "ignored",
    Blocked => "blocked",
    Duplicate => "duplicate",
    Unsupported => "unsupported",
    Failed => "failed",
    Disabled => "disabled",
});

// --- channel-management record types (moved from kura-store) ---------------

/// Go `connectors.RoutePolicy`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutePolicy {
    pub route_policy_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub eligible_senders: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub eligible_conversations: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub eligible_rooms: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub eligible_channels: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub invocation_gates: Vec<String>,
    pub background_delivery_eligible: bool,
    pub validation_state: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
    pub validated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub audit_event_id: String,
    pub redaction_status: RedactionStatus,
}

/// Go `connectors.RoutingDecision`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutingDecision {
    pub routing_decision_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_kind: String,
    pub outcome: RouteDecisionOutcome,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
    pub occurred_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub safe_evidence: HashMap<String, String>,
    pub redaction_status: RedactionStatus,
    pub retention_expires_at: DateTime<Utc>,
}

/// Go `connectors.ForegroundReplyOutcome`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForegroundReplyOutcome {
    pub reply_outcome_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub routing_decision_id: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
    pub occurred_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub safe_evidence: HashMap<String, String>,
    pub redaction_status: RedactionStatus,
    pub retention_expires_at: DateTime<Utc>,
}

/// Go `connectors.BackgroundDeliveryOutcome`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundDeliveryOutcome {
    pub delivery_outcome_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub delivery_target_id: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
    pub occurred_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub safe_evidence: HashMap<String, String>,
    pub redaction_status: RedactionStatus,
    pub retention_expires_at: DateTime<Utc>,
}

/// Go `connectors.SupportEvidenceBundle`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportEvidenceBundle {
    pub support_evidence_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub generated_by_principal_id: String,
    pub generated_at: DateTime<Utc>,
    pub current_state: ManagementState,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub state_transitions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostic_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repair_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routing_decision_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reply_outcome_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub delivery_outcome_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub audit_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redactions: Vec<String>,
    pub retention_expires_at: DateTime<Utc>,
    pub redaction_status: RedactionStatus,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub safe_evidence: HashMap<String, String>,
}

// --- management.go page/detail wire records --------------------------------

/// Go `connectors.ChannelManagementPage`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelManagementPage {
    pub limit: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub next_cursor: String,
    pub order: String,
}

/// Go `connectors.ChannelConnectorListResponse`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelConnectorListResponse {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub page: ChannelManagementPage,
    #[serde(default)]
    pub items: Vec<ChannelConnectorProjection>,
}

/// Go `connectors.ChannelConnectorProjection`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelConnectorProjection {
    pub connector_id: String,
    pub connector_kind: String,
    pub display_name: String,
    pub enablement_state: ManagementState,
    pub setup_state: String,
    pub health_status: String,
    pub diagnostic_freshness: DiagnosticFreshness,
    pub delivery_eligible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_action: Option<ChannelManagementNextAction>,
    #[serde(default)]
    pub capabilities: HashMap<String, CapabilitySupport>,
    pub redaction_status: RedactionStatus,
    pub updated_at: DateTime<Utc>,
}

/// Go `connectors.ChannelConnectorDetail` — the projection is embedded in Go
/// and flattened onto the JSON object; `#[serde(flatten)]` reproduces that.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelConnectorDetail {
    #[serde(flatten)]
    pub projection: ChannelConnectorProjection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic_summary: Option<ConnectorDiagnosticState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_policy: Option<RoutePolicy>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_route_decisions: Vec<RoutingDecision>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub foreground_reply_outcomes: Vec<ForegroundReplyOutcome>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub background_delivery: Vec<BackgroundDeliveryOutcome>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repair_actions: Vec<RepairAction>,
    pub support_evidence_available: bool,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub retention: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub support_evidence: Option<SupportEvidenceBundle>,
}

/// Go `connectors.ChannelManagementNextAction`. Go's `omitempty` on the
/// enum-typed reason fields maps to `Option`: absent when not set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelManagementNextAction {
    pub action_kind: ManagementActionKind,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<DiagnosticReasonCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remediation_owner: Option<RemediationOwner>,
}

/// Go `connectors.EnablementState` (state/reason are plain strings in Go).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnablementState {
    pub tenant_id: String,
    pub connector_id: String,
    pub state: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub changed_by_principal_id: String,
    pub changed_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validated_at: Option<DateTime<Utc>>,
    pub audit_event_id: String,
}

/// Go `connectors.EnablementMutationResult`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnablementMutationResult {
    pub connector_id: String,
    pub enablement_state: ManagementState,
    pub delivery_eligible: bool,
    pub audit_event_id: String,
    pub changed_at: DateTime<Utc>,
}

/// Go `connectors.RepairAction`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairAction {
    pub repair_action_id: String,
    pub tenant_id: String,
    pub connector_id: String,
    pub connector_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub actor_principal_id: String,
    pub action_kind: ManagementActionKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_diagnostic_state_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub setup_session_id: String,
    pub status: ManagementTerminalState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_safety: Option<RetrySafety>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remediation_owner: Option<RemediationOwner>,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    pub audit_event_id: String,
    pub redaction_status: RedactionStatus,
}

/// Go `connectors.ConnectorAuditRecord`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorAuditRecord {
    pub audit_event_id: String,
    pub tenant_id: String,
    pub connector_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub principal_id: String,
    pub action: String,
    pub permission_gate: String,
    pub outcome: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
    pub created_at: DateTime<Utc>,
    pub redaction_status: RedactionStatus,
}

/// Go `connectors.ProjectionInput` — carries no JSON tags.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProjectionInput {
    pub tenant_id: String,
    pub connectors: Vec<Connector>,
    pub diagnostics: HashMap<String, Vec<ConnectorDiagnosticState>>,
    /// Zero (`DateTime::default()`, the Unix epoch) falls back to `Utc::now()`,
    /// matching Go's `now.IsZero()` handling.
    pub now: DateTime<Utc>,
    pub limit: i64,
    pub cursor: String,
    pub state_filter: String,
    pub kind_filter: String,
}

// --- management_routes.go predicates ---------------------------------------

/// Go `DefaultRoutePolicy`: a future-eligible, redacted, valid policy.
#[must_use]
pub fn default_route_policy(
    tenant_id: &str,
    connector_id: &str,
    now: DateTime<Utc>,
) -> RoutePolicy {
    let now = if now == DateTime::<Utc>::UNIX_EPOCH {
        Utc::now()
    } else {
        now
    };
    RoutePolicy {
        tenant_id: tenant_id.to_string(),
        connector_id: connector_id.to_string(),
        background_delivery_eligible: true,
        validation_state: "valid".to_string(),
        validated_at: now,
        redaction_status: RedactionStatus::Redacted,
        ..RoutePolicy::default()
    }
}

/// Go `NormalizeRoutePolicy`: fills the validated-at timestamp, the
/// validation state, and the redaction status when unset.
#[must_use]
pub fn normalize_route_policy(mut policy: RoutePolicy, now: DateTime<Utc>) -> RoutePolicy {
    let now = if now == DateTime::<Utc>::UNIX_EPOCH {
        Utc::now()
    } else {
        now
    };
    if policy.validated_at == DateTime::<Utc>::UNIX_EPOCH {
        policy.validated_at = now;
    }
    if policy.validation_state.is_empty() {
        policy.validation_state = "valid".to_string();
    }
    // Go zeroes an empty RedactionStatus to Redacted; the Rust enum has no
    // empty wire value and its Default is already Redacted, so this
    // normalization is satisfied implicitly.
    policy
}

/// Go `connectors.RoutePolicyIsValid` — a policy is valid only when its
/// validation state is exactly `valid` (trimmed).
#[must_use]
pub fn route_policy_is_valid(policy: &RoutePolicy) -> bool {
    policy.validation_state.trim() == "valid"
}

/// Go `connectors.RoutePolicyAllowsConversation` — valid policies with any
/// eligible conversation/room/channel list allow the conversation when it
/// matches one of the listed identities.
#[must_use]
pub fn route_policy_allows_conversation(policy: &RoutePolicy, conversation_id: &str) -> bool {
    if !route_policy_is_valid(policy) {
        return false;
    }
    let conversation_id = conversation_id.trim();
    if conversation_id.is_empty() {
        return false;
    }
    if policy.eligible_conversations.is_empty()
        && policy.eligible_rooms.is_empty()
        && policy.eligible_channels.is_empty()
    {
        return false;
    }
    contains_route_policy_value(&policy.eligible_conversations, conversation_id)
        || contains_route_policy_value(&policy.eligible_rooms, conversation_id)
        || contains_route_policy_value(&policy.eligible_channels, conversation_id)
}

/// Go `connectors.RoutePolicyAllowsSender` — valid policies without an
/// eligible-sender list allow every sender; otherwise the sender must match.
#[must_use]
pub fn route_policy_allows_sender(policy: &RoutePolicy, sender_id: &str) -> bool {
    if !route_policy_is_valid(policy) {
        return false;
    }
    if policy.eligible_senders.is_empty() {
        return true;
    }
    contains_route_policy_value(&policy.eligible_senders, sender_id.trim())
}

/// Go `connectors.containsRoutePolicyValue`.
#[must_use]
pub fn contains_route_policy_value(values: &[String], target: &str) -> bool {
    let target = target.trim();
    if target.is_empty() {
        return false;
    }
    values.iter().any(|value| value.trim() == target)
}

// --- management.go page/projection builders --------------------------------

/// Go `BuildConnectorPage`: filters by tenant/kind/state, orders by
/// attention → disabled → ready → name → id, and paginates with a numeric
/// cursor. A non-positive limit defaults to 20.
#[must_use]
pub fn build_connector_page(input: &ProjectionInput) -> ChannelConnectorListResponse {
    let now = if input.now == DateTime::<Utc>::UNIX_EPOCH {
        Utc::now()
    } else {
        input.now
    };
    let limit = if input.limit <= 0 { 20 } else { input.limit };

    let mut items: Vec<ChannelConnectorProjection> = Vec::new();
    for connector in &input.connectors {
        if !input.tenant_id.is_empty()
            && !connector.tenant_id.is_empty()
            && connector.tenant_id != input.tenant_id
        {
            continue;
        }
        if !input.kind_filter.is_empty() && connector.kind != input.kind_filter {
            continue;
        }
        let diagnostics = input
            .diagnostics
            .get(&connector.connector_id)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let projection =
            build_connector_projection(connector.clone(), latest_diagnostic(diagnostics), now);
        if !input.state_filter.is_empty()
            && projection.enablement_state.as_str() != input.state_filter
        {
            continue;
        }
        items.push(projection);
    }
    sort_connector_projections(&mut items);

    let offset = (parse_cursor_offset(&input.cursor) as usize).min(items.len());
    let mut end = offset + limit as usize;
    let next_cursor = if end < items.len() {
        end.to_string()
    } else {
        end = items.len();
        String::new()
    };
    ChannelConnectorListResponse {
        tenant_id: input.tenant_id.clone(),
        page: ChannelManagementPage {
            limit,
            next_cursor,
            order: "attention_disabled_ready_name_id".to_string(),
        },
        items: items[offset..end].to_vec(),
    }
}

/// Go `BuildConnectorProjection`: derives the enablement state, setup and
/// health strings, diagnostic freshness, delivery eligibility, capability
/// profile, and the next-action hint (diagnostic-driven unless ready; a
/// disabled connector without diagnostics gets the re-enable action).
#[must_use]
pub fn build_connector_projection(
    connector: Connector,
    diagnostic: Option<&ConnectorDiagnosticState>,
    now: DateTime<Utc>,
) -> ChannelConnectorProjection {
    let state = management_state_for_connector(&connector, diagnostic);
    let freshness = match diagnostic {
        Some(d) if freshness_at(d.evidence_timestamp, now) == FreshnessState::Fresh => {
            DiagnosticFreshness::Fresh
        }
        _ => DiagnosticFreshness::Stale,
    };
    let delivery_eligible = state == ManagementState::Ready;
    let mut projection = ChannelConnectorProjection {
        connector_id: connector.connector_id.clone(),
        connector_kind: connector.kind.clone(),
        display_name: connector.display_name.clone(),
        enablement_state: state,
        setup_state: setup_state_for_management(state),
        health_status: health_status_for_management(&connector, diagnostic),
        diagnostic_freshness: freshness,
        delivery_eligible,
        capabilities: capability_profile_for_kind(&connector.kind),
        redaction_status: RedactionStatus::Redacted,
        updated_at: connector.updated_at,
        ..ChannelConnectorProjection::default()
    };
    if let Some(d) = diagnostic {
        if state != ManagementState::Ready {
            projection.next_action = Some(ChannelManagementNextAction {
                action_kind: next_action_for_diagnostic(d),
                label: next_action_label_for_diagnostic(d),
                reason_code: Some(d.reason_code),
                remediation_owner: Some(d.remediation_owner),
            });
        }
    } else if state == ManagementState::Disabled {
        projection.next_action = Some(ChannelManagementNextAction {
            action_kind: ManagementActionKind::ReEnable,
            label: "Re-enable connector".to_string(),
            reason_code: None,
            remediation_owner: None,
        });
    }
    projection
}

/// Go `ManagementStateForConnector`: disabled wins immediately; otherwise the
/// diagnostic status overrides, and the connector health status is the fallback.
#[must_use]
pub fn management_state_for_connector(
    connector: &Connector,
    diagnostic: Option<&ConnectorDiagnosticState>,
) -> ManagementState {
    if connector.status == Status::Disabled {
        return ManagementState::Disabled;
    }
    if let Some(d) = diagnostic {
        match d.status {
            LifecycleState::PermissionBlocked | LifecycleState::Failed => {
                return ManagementState::ActionRequired;
            }
            LifecycleState::RateLimited => return ManagementState::Unavailable,
            LifecycleState::Degraded | LifecycleState::UnsupportedCapability => {
                return ManagementState::Degraded;
            }
            _ => {}
        }
    }
    match connector.status {
        Status::Healthy | Status::Registered => ManagementState::Ready,
        Status::Degraded | Status::BackingOff => ManagementState::Degraded,
        Status::Failed => ManagementState::ActionRequired,
        Status::Disabled => ManagementState::Disabled,
    }
}

/// Go `CapabilityProfileForKind`: the full management capability set for the
/// built-in kinds (discord/telegram/slack/matrix, case-insensitive); unknown
/// kinds lose reconnect, credential-rotation, and route-edit support.
#[must_use]
pub fn capability_profile_for_kind(kind: &str) -> HashMap<String, CapabilitySupport> {
    let mut capabilities: HashMap<String, CapabilitySupport> = HashMap::from([
        ("disable".to_string(), CapabilitySupport::Supported),
        ("re-enable".to_string(), CapabilitySupport::Supported),
        ("repair".to_string(), CapabilitySupport::Supported),
        ("reconnect".to_string(), CapabilitySupport::Supported),
        (
            "credential-rotation".to_string(),
            CapabilitySupport::Limited,
        ),
        ("route-edit".to_string(), CapabilitySupport::Supported),
        (
            "foreground-reply-status".to_string(),
            CapabilitySupport::Supported,
        ),
        (
            "background-delivery-status".to_string(),
            CapabilitySupport::Supported,
        ),
        ("support-evidence".to_string(), CapabilitySupport::Supported),
    ]);
    match kind.to_lowercase().as_str() {
        "discord" | "telegram" | "slack" | "matrix" => capabilities,
        _ => {
            capabilities.insert("reconnect".to_string(), CapabilitySupport::Unsupported);
            capabilities.insert(
                "credential-rotation".to_string(),
                CapabilitySupport::Unsupported,
            );
            capabilities.insert("route-edit".to_string(), CapabilitySupport::Unsupported);
            capabilities
        }
    }
}

/// Go `SortConnectorProjections`: stable sort by management-state rank, then
/// display name, then connector id.
pub fn sort_connector_projections(items: &mut [ChannelConnectorProjection]) {
    items.sort_by(|left, right| {
        let rank = management_state_rank(left.enablement_state)
            .cmp(&management_state_rank(right.enablement_state));
        if rank != std::cmp::Ordering::Equal {
            return rank;
        }
        if left.display_name != right.display_name {
            return left.display_name.cmp(&right.display_name);
        }
        left.connector_id.cmp(&right.connector_id)
    });
}

/// Go `latestDiagnostic`: the newest evidence, first element wins ties.
#[must_use]
pub fn latest_diagnostic(items: &[ConnectorDiagnosticState]) -> Option<&ConnectorDiagnosticState> {
    let mut latest = items.first()?;
    for item in &items[1..] {
        if item.evidence_timestamp > latest.evidence_timestamp {
            latest = item;
        }
    }
    Some(latest)
}

/// Go `managementStateRank`: attention states (action-required, unavailable,
/// degraded) rank first, then disabled, then ready.
#[must_use]
pub fn management_state_rank(state: ManagementState) -> i64 {
    match state {
        ManagementState::ActionRequired
        | ManagementState::Unavailable
        | ManagementState::Degraded => 0,
        ManagementState::Disabled => 1,
        ManagementState::Ready => 2,
    }
}

/// Go `setupStateForManagement`: "ready"/"disabled" map to themselves,
/// everything else uses the state literal.
#[must_use]
pub fn setup_state_for_management(state: ManagementState) -> String {
    match state {
        ManagementState::Ready => "ready".to_string(),
        ManagementState::Disabled => "disabled".to_string(),
        _ => state.as_str().to_string(),
    }
}

/// Go `healthStatusForManagement`: the diagnostic lifecycle status when a
/// diagnostic exists, otherwise the connector status.
#[must_use]
pub fn health_status_for_management(
    connector: &Connector,
    diagnostic: Option<&ConnectorDiagnosticState>,
) -> String {
    match diagnostic {
        Some(d) => d.status.as_str().to_string(),
        None => connector.status.as_str().to_string(),
    }
}

/// Go `nextActionForDiagnostic`: auth/permission problems → reconnect,
/// blocked routes → route revalidation, unsupported capability → disable,
/// anything else → repair.
#[must_use]
pub fn next_action_for_diagnostic(diagnostic: &ConnectorDiagnosticState) -> ManagementActionKind {
    match diagnostic.reason_code {
        DiagnosticReasonCode::AuthMissing | DiagnosticReasonCode::PermissionMissing => {
            ManagementActionKind::Reconnect
        }
        DiagnosticReasonCode::BlockedRoute => ManagementActionKind::RouteRevalidate,
        DiagnosticReasonCode::UnsupportedCapability => ManagementActionKind::Disable,
        _ => ManagementActionKind::Repair,
    }
}

/// Go `nextActionLabelForDiagnostic`: the human-readable label for the
/// next action.
#[must_use]
pub fn next_action_label_for_diagnostic(diagnostic: &ConnectorDiagnosticState) -> String {
    match next_action_for_diagnostic(diagnostic) {
        ManagementActionKind::Reconnect => "Reconnect authorization".to_string(),
        ManagementActionKind::RouteRevalidate => "Review route policy".to_string(),
        ManagementActionKind::Disable => "Disable unsupported connector".to_string(),
        _ => "Repair connector".to_string(),
    }
}

/// Go `parseCursorOffset`: trims, rejects non-numeric and negative cursors.
#[must_use]
pub fn parse_cursor_offset(cursor: &str) -> i64 {
    let cursor = cursor.trim();
    if cursor.is_empty() {
        return 0;
    }
    match cursor.parse::<i64>() {
        Ok(offset) if offset >= 0 => offset,
        _ => 0,
    }
}

// --- management_support.go -------------------------------------------------

/// Go `BuildSupportEvidenceBundle`: a redacted support-evidence bundle for
/// one connector, carrying the current enablement state from its projection,
/// a fixed redaction list, 90-day retention, and safe connector identity.
#[must_use]
pub fn build_support_evidence_bundle(
    input: &ProjectionInput,
    connector: &Connector,
    principal_id: &str,
    now: DateTime<Utc>,
) -> SupportEvidenceBundle {
    let now = if now == DateTime::<Utc>::UNIX_EPOCH {
        Utc::now()
    } else {
        now
    };
    let diagnostics = input
        .diagnostics
        .get(&connector.connector_id)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let projection =
        build_connector_projection(connector.clone(), latest_diagnostic(diagnostics), now);
    SupportEvidenceBundle {
        tenant_id: input.tenant_id.clone(),
        connector_id: connector.connector_id.clone(),
        generated_by_principal_id: principal_id.to_string(),
        generated_at: now,
        current_state: projection.enablement_state,
        redactions: vec![
            "message_body".to_string(),
            "raw_provider_payload".to_string(),
            "credentials".to_string(),
            "authorization_grants".to_string(),
        ],
        retention_expires_at: now + Duration::days(90),
        redaction_status: RedactionStatus::Redacted,
        safe_evidence: HashMap::from([
            ("connectorKind".to_string(), connector.kind.clone()),
            ("displayName".to_string(), connector.display_name.clone()),
        ]),
        ..SupportEvidenceBundle::default()
    }
}

// --- management_repair.go --------------------------------------------------

/// Go `TerminalStateForRepairAction`: a disabled connector always terminates
/// disabled; disable actions terminate disabled; rerun/revalidation actions
/// terminate degraded; everything else stays action-required.
#[must_use]
pub fn terminal_state_for_repair_action(
    action_kind: ManagementActionKind,
    disabled: bool,
) -> ManagementTerminalState {
    if disabled {
        return ManagementTerminalState::Disabled;
    }
    match action_kind {
        ManagementActionKind::Disable => ManagementTerminalState::Disabled,
        ManagementActionKind::DiagnosticRerun | ManagementActionKind::RouteRevalidate => {
            ManagementTerminalState::Degraded
        }
        _ => ManagementTerminalState::ActionRequired,
    }
}

/// Go `RetrySafetyForRepairAction`: reconnect and credential rotation are
/// blocked; everything else is retryable.
#[must_use]
pub fn retry_safety_for_repair_action(action_kind: ManagementActionKind) -> RetrySafety {
    match action_kind {
        ManagementActionKind::Reconnect | ManagementActionKind::CredentialRotation => {
            RetrySafety::Blocked
        }
        _ => RetrySafety::Retryable,
    }
}
