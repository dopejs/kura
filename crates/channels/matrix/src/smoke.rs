//! Port of daemon/internal/connectors/matrix/smoke.go: structured smoke
//! evidence with a safe-live execution path over a validated credential +
//! route policy + send.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use kura_connectors::RedactionStatus;
use kura_imtypes::{OutboundReply, SentReply};
use serde::{Deserialize, Serialize};

use crate::is_unset_time;
use crate::routes::has_ready_route_policy;
use crate::string_enum;
use crate::types::{HomeserverBinding, RoutePolicy};

// Go `SmokeStatus`.
string_enum!(SmokeStatus {
    Passed => "passed",
    Failed => "failed",
    Skipped => "skipped",
});

// Go `SmokeAuthorizationMode`.
string_enum!(SmokeAuthorizationMode {
    SafeLive => "safe_live",
    FakeMatrix => "fake_matrix",
    Unavailable => "unavailable",
});

/// Go `SmokeEvidence`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmokeEvidence {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub smoke_evidence_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub homeserver_binding_id: String,
    pub status: SmokeStatus,
    pub authorization_mode: SmokeAuthorizationMode,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub owner: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub remaining_risk: String,
    pub validated_at: DateTime<Utc>,
    pub retention_expires_at: DateTime<Utc>,
    pub redaction_status: RedactionStatus,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub safe_evidence: HashMap<String, String>,
}

/// The transport seam a safe-live smoke needs (Go's inline interface in
/// `SafeLiveSmokeInput`).
pub trait SmokeTransport: Send + Sync {
    /// Go `ValidateHomeserverBinding`.
    fn validate_homeserver_binding(
        &self,
        binding: HomeserverBinding,
    ) -> (HomeserverBinding, Result<(), String>);
    /// Go `ValidateRoutePolicy`.
    fn validate_route_policy(&self, policy: RoutePolicy) -> (RoutePolicy, Result<(), String>);
    /// Go `SendReply`.
    fn send_reply(&self, reply: OutboundReply) -> Result<SentReply, String>;
}

/// Go `SafeLiveSmokeInput`.
pub struct SafeLiveSmokeInput<T: SmokeTransport> {
    pub tenant_id: String,
    pub connector_id: String,
    pub owner: String,
    pub now: DateTime<Utc>,
    pub transport: T,
    pub binding: HomeserverBinding,
    pub route_policy: RoutePolicy,
    pub smoke_room_id: String,
}

/// Go `StructuredSkipSmokeEvidence`.
#[must_use]
pub fn structured_skip_smoke_evidence(
    tenant_id: &str,
    connector_id: &str,
    owner: &str,
    reason: &str,
    now: DateTime<Utc>,
) -> SmokeEvidence {
    let now = if is_unset_time(&now) { Utc::now() } else { now };
    SmokeEvidence {
        smoke_evidence_id: format!("matrix_smoke_{}", connector_id),
        tenant_id: tenant_id.to_string(),
        connector_id: connector_id.to_string(),
        homeserver_binding_id: format!("matrix_homeserver_{}", connector_id),
        status: SmokeStatus::Skipped,
        authorization_mode: SmokeAuthorizationMode::Unavailable,
        owner: owner.to_string(),
        reason: reason.to_string(),
        remaining_risk:
            "No live Matrix hosted smoke was run; release review must consume this structured skip."
                .to_string(),
        validated_at: now,
        retention_expires_at: now + Duration::days(90),
        redaction_status: RedactionStatus::Redacted,
        safe_evidence: HashMap::from([("policy".to_string(), "structured_skip".to_string())]),
    }
}

/// Go `ExecuteSafeLiveSmoke`: validates the homeserver binding, the route
/// policy, readiness, and finally sends the smoke message to the smoke room.
pub fn execute_safe_live_smoke<T: SmokeTransport>(
    input: SafeLiveSmokeInput<T>,
) -> Result<SmokeEvidence, String> {
    let now = if is_unset_time(&input.now) {
        Utc::now()
    } else {
        input.now
    };
    let (binding, result) = input.transport.validate_homeserver_binding(input.binding);
    result?;
    let mut policy = input.route_policy;
    if policy.homeserver_binding_id.trim().is_empty() {
        policy.homeserver_binding_id = binding.homeserver_binding_id.clone();
    }
    let (policy, result) = input.transport.validate_route_policy(policy);
    result?;
    if !has_ready_route_policy(&policy) {
        return Err("matrix route policy is not ready for safe-live smoke".to_string());
    }
    let mut room_id = input.smoke_room_id.trim().to_string();
    if room_id.is_empty() && !policy.selected_rooms.is_empty() {
        room_id = policy.selected_rooms[0].conversation_id.clone();
    }
    if room_id.is_empty() {
        return Err("matrix safe-live smoke room is required".to_string());
    }
    let sent = input.transport.send_reply(OutboundReply {
        connector_id: input.connector_id.clone(),
        channel_id: room_id,
        content: "Kura Matrix smoke validation".to_string(),
        reply_to_external_message_id: String::new(),
    })?;
    let binding_id = if binding.homeserver_binding_id.trim().is_empty() {
        format!("matrix_homeserver_{}", input.connector_id.trim())
    } else {
        binding.homeserver_binding_id.clone()
    };
    Ok(SmokeEvidence {
        smoke_evidence_id: format!("matrix_smoke_{}", input.connector_id),
        tenant_id: input.tenant_id,
        connector_id: input.connector_id,
        homeserver_binding_id: binding_id,
        status: SmokeStatus::Passed,
        authorization_mode: SmokeAuthorizationMode::SafeLive,
        owner: input.owner,
        reason: "healthy".to_string(),
        remaining_risk: String::new(),
        validated_at: now,
        retention_expires_at: now + Duration::days(90),
        redaction_status: RedactionStatus::Redacted,
        safe_evidence: HashMap::from([
            ("policy".to_string(), "safe_live_matrix_smoke".to_string()),
            ("eventId".to_string(), sent.external_message_id),
        ]),
    })
}
