//! Port of daemon/internal/connectors/matrix/types.go: the Matrix connector
//! domain types and string enums.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use kura_connectors::{LifecycleState, RedactionStatus};
use serde::{Deserialize, Serialize};

use crate::string_enum;

/// Go `ConnectorKind` = baseconnectors.ConnectorKindMatrix.
pub const CONNECTOR_KIND: &str = kura_connectors::CONNECTOR_KIND_MATRIX;

/// Go `Config`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub enabled: bool,
    pub connector_id: String,
    pub display_name: String,
    pub homeserver_url: String,
    pub homeserver_id: String,
    pub bot_user_id: String,
    pub selected_room_ids: Vec<String>,
    pub allowed_direct_user_ids: Vec<String>,
    pub configured_commands: Vec<String>,
}

string_enum!(TerminalState {
    Ready => "ready",
    Degraded => "degraded",
    Unavailable => "unavailable",
    Cancelled => "cancelled",
    ActionRequired => "action-required",
});

string_enum!(BotCredentialState {
    Unknown => "unknown",
    NotStarted => "not_started",
    Submitted => "submitted",
    Valid => "valid",
    Invalid => "invalid",
    Revoked => "revoked",
    PermissionMissing => "permission_missing",
    RedactionSuppressed => "redaction_suppressed",
});

string_enum!(HomeserverState {
    Unknown => "unknown",
    Reachable => "reachable",
    Unreachable => "unreachable",
    Unsupported => "unsupported",
    RateLimited => "rate_limited",
    FederationFailed => "federation_failed",
    NetworkFailed => "network_failed",
});

// Default is `Missing`, matching Go's normalization of the empty string in
// [crate::normalize_homeserver_binding].
string_enum!(AuthorizationState {
    Missing => "missing",
    Valid => "valid",
    Revoked => "revoked",
    PermissionMissing => "permission_missing",
    OwnershipMismatch => "ownership_mismatch",
    ProviderUnavailable => "provider_unavailable",
    NetworkFailed => "network_failed",
    Unknown => "unknown",
});

string_enum!(HomeserverCapabilityState {
    Unknown => "unknown",
    Valid => "valid",
    Unsupported => "unsupported",
    Stale => "stale",
    RateLimited => "rate_limited",
});

// Default is `None`, matching Go's normalization of the empty string in
// [crate::normalize_route_policy].
string_enum!(RoutePolicyState {
    None => "none",
    Partial => "partial",
    Valid => "valid",
    Stale => "stale",
    Blocked => "blocked",
    MissingPermission => "missing_permission",
});

// Default is `Room`, matching Go's normalization of the empty string in
// [crate::normalize_route_policy] and the transport's conversation typing.
string_enum!(ConversationType {
    Room => "room",
    DirectMessage => "direct_message",
});

// Default is `Selected`, matching Go's normalization of the empty string.
string_enum!(RoomSelectionState {
    Selected => "selected",
    NotSelected => "not_selected",
    Stale => "stale",
    Left => "left",
    Banned => "banned",
    MissingMembership => "missing_membership",
    EncryptedUnsupported => "encrypted_unsupported",
    NotApplicable => "not_applicable",
});

// Default is `UnencryptedText`, matching Go's normalization of the empty
// string in [crate::normalize_inbound_event].
string_enum!(MessageKind {
    UnencryptedText => "unencrypted_text",
    EncryptedUnsupported => "encrypted_unsupported",
    UndecryptableUnsupported => "undecryptable_unsupported",
    MediaUnsupported => "media_unsupported",
    CallUnsupported => "call_unsupported",
    VoiceUnsupported => "voice_unsupported",
    ReactionUnsupported => "reaction_unsupported",
    BridgeMetadataUnsupported => "bridge_metadata_unsupported",
    Unsupported => "unsupported",
    Unknown => "unknown",
});

string_enum!(RouteOutcome {
    Failed => "failed",
    Accepted => "accepted",
    Ignored => "ignored",
    Blocked => "blocked",
    Duplicate => "duplicate",
    Unsupported => "unsupported",
});

/// Go `HomeserverBinding`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HomeserverBinding {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub homeserver_binding_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub homeserver_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub homeserver_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bot_user_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bot_device_id: String,
    pub authorization_state: AuthorizationState,
    pub homeserver_capability_state: HomeserverCapabilityState,
    pub validated_at: DateTime<Utc>,
    pub redaction_status: RedactionStatus,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub safe_evidence: HashMap<String, String>,
}

/// Go `ConversationRoute`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationRoute {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub conversation_id: String,
    pub conversation_type: ConversationType,
    pub room_selection_state: RoomSelectionState,
    pub validation_state: RoutePolicyState,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
    pub redaction_status: RedactionStatus,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub safe_evidence: HashMap<String, String>,
}

/// Go `RoutePolicy`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutePolicy {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub homeserver_binding_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selected_rooms: Vec<ConversationRoute>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_direct_users: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub room_invocation_gate: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub configured_commands: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub encrypted_room_policy: String,
    pub validation_state: RoutePolicyState,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
    pub validated_at: DateTime<Utc>,
    pub redaction_status: RedactionStatus,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub safe_evidence: HashMap<String, String>,
}

/// Go `HostedSetupInput`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostedSetupInput {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub display_name: String,
    pub bot_credential_state: BotCredentialState,
    pub homeserver_binding: HomeserverBinding,
    pub route_policy: RoutePolicy,
    pub provider_available: bool,
    pub network_available: bool,
    pub conformance_passed: bool,
    pub cancelled: bool,
    pub redaction_suppressed: bool,
    pub requested_hosted_homeserver: bool,
    pub requested_account_provision: bool,
    pub started_at: DateTime<Utc>,
    pub setup_timeout: Duration,
    pub validated_at: DateTime<Utc>,
}

/// Go `HostedSetup`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostedSetup {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub display_name: String,
    pub status: LifecycleState,
    pub terminal_state: TerminalState,
    pub bot_credential_state: BotCredentialState,
    pub homeserver_state: HomeserverState,
    pub route_policy_state: RoutePolicyState,
    pub delivery_eligible: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub homeserver_binding_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
    pub homeserver_binding: HomeserverBinding,
    pub route_policy: RoutePolicy,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub validated_at: DateTime<Utc>,
    pub redaction_status: RedactionStatus,
    pub retention_expires_at: DateTime<Utc>,
    pub setup_completed_within: Duration,
}

/// Go `InboundEvent`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboundEvent {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub homeserver_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub conversation_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub matrix_event_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sync_batch_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub transaction_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sender_id: String,
    pub conversation_type: ConversationType,
    pub message_kind: MessageKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    pub bot_mentioned: bool,
    pub received_at: DateTime<Utc>,
}

/// Go `RouteDecision`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteDecision {
    pub outcome: RouteOutcome,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub surface: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub normalized_text: String,
}

/// Go `RedactionResult`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactionResult {
    pub status: RedactionStatus,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub safe_evidence: HashMap<String, String>,
}

/// Go `ReplyOutcome`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplyOutcome {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub inbound_event_identity: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub assistant_execution_outcome: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub matrix_reply_outcome: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reply_progression_level: String,
    pub reply_context: ConversationType,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub failure_reason_code: String,
    pub redaction_status: RedactionStatus,
}
