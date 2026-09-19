//! Port of daemon/internal/connectors/matrix/routes.go: route-policy
//! normalization/readiness and per-event route decisions.

use chrono::{DateTime, Utc};
use kura_connectors::{DiagnosticReasonCode, RedactionStatus};

use crate::is_unset_time;
use crate::types::{
    ConversationType, InboundEvent, MessageKind, RoomSelectionState, RouteDecision, RouteOutcome,
    RoutePolicy, RoutePolicyState,
};

/// Go `NormalizeRoutePolicy`: fills every empty field with its ready default.
#[must_use]
pub fn normalize_route_policy(mut policy: RoutePolicy, now: DateTime<Utc>) -> RoutePolicy {
    let now = if is_unset_time(&now) { Utc::now() } else { now };
    if policy.room_invocation_gate.trim().is_empty() {
        policy.room_invocation_gate = "bot_mention_or_command_required".to_string();
    }
    if policy.encrypted_room_policy.trim().is_empty() {
        policy.encrypted_room_policy = "unsupported".to_string();
    }
    if policy.validation_state == RoutePolicyState::default() {
        policy.validation_state = RoutePolicyState::None;
    }
    if is_unset_time(&policy.validated_at) {
        policy.validated_at = now;
    }
    if policy.redaction_status == RedactionStatus::default() {
        policy.redaction_status = RedactionStatus::Redacted;
    }
    for room in &mut policy.selected_rooms {
        if room.conversation_type == ConversationType::default() {
            room.conversation_type = ConversationType::Room;
        }
        if room.room_selection_state == RoomSelectionState::default() {
            room.room_selection_state = RoomSelectionState::Selected;
        }
        if room.validation_state == RoutePolicyState::default() {
            room.validation_state = RoutePolicyState::Valid;
        }
        if room.redaction_status == RedactionStatus::default() {
            room.redaction_status = RedactionStatus::Redacted;
        }
    }
    policy
}

/// Go `HasReadyRoutePolicy`: the policy is valid and either allows a direct
/// user or contains a selected, valid room.
#[must_use]
pub fn has_ready_route_policy(policy: &RoutePolicy) -> bool {
    let policy = normalize_route_policy(policy.clone(), Utc::now());
    if policy.validation_state != RoutePolicyState::Valid {
        return false;
    }
    if !policy.allowed_direct_users.is_empty() {
        return true;
    }
    policy.selected_rooms.iter().any(|room| {
        room.conversation_type == ConversationType::Room
            && room.room_selection_state == RoomSelectionState::Selected
            && room.validation_state == RoutePolicyState::Valid
    })
}

/// Go `DecideRoute`: classifies one inbound event into a route outcome with
/// the mention/command room gate and direct-message allowlist.
#[must_use]
pub fn decide_route(
    event: &InboundEvent,
    policy: RoutePolicy,
    homeserver_id: &str,
    bot_user_id: &str,
) -> RouteDecision {
    let surface = event.conversation_type.as_str().to_string();
    if missing_matrix_identity(event) {
        return RouteDecision {
            outcome: RouteOutcome::Failed,
            reason_code: DiagnosticReasonCode::UnknownConnectorFailure
                .as_str()
                .to_string(),
            surface,
            ..RouteDecision::default()
        };
    }
    if event.message_kind != MessageKind::UnencryptedText {
        return RouteDecision {
            outcome: RouteOutcome::Unsupported,
            reason_code: DiagnosticReasonCode::UnsupportedCapability
                .as_str()
                .to_string(),
            surface,
            ..RouteDecision::default()
        };
    }
    if !homeserver_id.trim().is_empty() && event.homeserver_id.trim() != homeserver_id.trim() {
        return RouteDecision {
            outcome: RouteOutcome::Blocked,
            reason_code: DiagnosticReasonCode::BlockedRoute.as_str().to_string(),
            surface,
            ..RouteDecision::default()
        };
    }
    let policy = normalize_route_policy(policy, event.received_at);
    // Go mutates a copy of the event to record the mention and strip it.
    let mut event = event.clone();
    // The final arm mirrors Go's default: case (the empty-string zero value
    // which the port's Room default absorbs, so it is unreachable).
    #[allow(unreachable_patterns)]
    match event.conversation_type {
        ConversationType::DirectMessage => {
            if allowed_direct_sender(&event.sender_id, &policy) {
                return RouteDecision {
                    outcome: RouteOutcome::Accepted,
                    reason_code: "accepted".to_string(),
                    surface,
                    normalized_text: event.text.trim().to_string(),
                };
            }
            RouteDecision {
                outcome: RouteOutcome::Blocked,
                reason_code: DiagnosticReasonCode::BlockedRoute.as_str().to_string(),
                surface,
                ..RouteDecision::default()
            }
        }
        ConversationType::Room => {
            if !selected_room(&event.conversation_id, &policy) {
                return RouteDecision {
                    outcome: RouteOutcome::Blocked,
                    reason_code: DiagnosticReasonCode::BlockedRoute.as_str().to_string(),
                    surface,
                    ..RouteDecision::default()
                };
            }
            let mut text = event.text.trim().to_string();
            if !bot_user_id.trim().is_empty() {
                let mention = bot_user_id.trim().to_string();
                if text.contains(&mention) {
                    event.bot_mentioned = true;
                    text = text.replace(&mention, "").trim().to_string();
                }
            }
            if !event.bot_mentioned && !has_configured_command(&text, &policy.configured_commands) {
                return RouteDecision {
                    outcome: RouteOutcome::Ignored,
                    reason_code: "mention_required".to_string(),
                    surface,
                    ..RouteDecision::default()
                };
            }
            text = trim_configured_command(&text, &policy.configured_commands);
            RouteDecision {
                outcome: RouteOutcome::Accepted,
                reason_code: "accepted".to_string(),
                surface,
                normalized_text: text.trim().to_string(),
            }
        }
        _ => RouteDecision {
            outcome: RouteOutcome::Unsupported,
            reason_code: DiagnosticReasonCode::UnsupportedCapability
                .as_str()
                .to_string(),
            surface,
            ..RouteDecision::default()
        },
    }
}

/// Go `missingMatrixIdentity`.
fn missing_matrix_identity(event: &InboundEvent) -> bool {
    event.homeserver_id.trim().is_empty()
        || event.conversation_id.trim().is_empty()
        || event.matrix_event_id.trim().is_empty()
}

/// Go `allowedDirectSender`.
fn allowed_direct_sender(sender_id: &str, policy: &RoutePolicy) -> bool {
    policy
        .allowed_direct_users
        .iter()
        .any(|allowed| !allowed.trim().is_empty() && allowed.trim() == sender_id.trim())
}

/// Go `selectedRoom`.
fn selected_room(conversation_id: &str, policy: &RoutePolicy) -> bool {
    policy.selected_rooms.iter().any(|room| {
        room.conversation_id == conversation_id.trim()
            && room.room_selection_state == RoomSelectionState::Selected
            && room.validation_state == RoutePolicyState::Valid
    })
}

/// Go `hasConfiguredCommand`.
fn has_configured_command(text: &str, commands: &[String]) -> bool {
    commands.iter().any(|command| {
        let command = command.trim();
        !command.is_empty() && text.trim().starts_with(command)
    })
}

/// Go `trimConfiguredCommand`: strips the first matching command prefix.
fn trim_configured_command(text: &str, commands: &[String]) -> String {
    for command in commands {
        let command = command.trim();
        if !command.is_empty() {
            if let Some(rest) = text.trim().strip_prefix(command) {
                return rest.trim().to_string();
            }
        }
    }
    text.to_string()
}
