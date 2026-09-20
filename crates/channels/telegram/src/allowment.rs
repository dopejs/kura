//! Telegram allowment model and route decisions (port of allowment.go).
//!

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use kura_connectors::{DiagnosticReasonCode, RedactionStatus};
use serde::{Deserialize, Serialize};

use crate::readiness::first_non_empty;
use crate::wire_enum;

// Who a [`AllowmentValidation`] grants access to (Go `ScopeType`).
wire_enum!(ScopeType, default User; User => "user", DirectChat => "direct_chat", Group => "group");

// Group invocation gate (Go `GroupGate`). Go's `MentionOrCommandSupported`
// alias is the same value as `MentionOrCommandRequired`, so it needs no
// separate variant. `None` corresponds to Go's empty-string zero value
// (treated as `MentionOrCommandRequired` for groups).
wire_enum!(GroupGate, default NotApplicable; NotApplicable => "not_applicable", MentionOrCommandRequired => "mention_or_command_required");

// Validation state of one allowment (Go `AllowmentValidationState`).
wire_enum!(AllowmentValidationState, default Invalid; Invalid => "invalid", Valid => "valid", Blocked => "blocked", Stale => "stale", MissingPermission => "missing_permission", NotFound => "not_found");

// Aggregate allowment state (Go `AllowmentState`).
wire_enum!(AllowmentState, default None; None => "none", Partial => "partial", Valid => "valid", Stale => "stale");

// Conversation shape for routing (Go `ConversationType`). The default
// `Direct` mirrors how Go treats the empty-string zero value.
wire_enum!(ConversationType, default Direct; Direct => "direct", Group => "group");

// Route decision outcome (Go `RouteOutcome`).
wire_enum!(RouteOutcome, default Accepted; Accepted => "accepted", Ignored => "ignored", Blocked => "blocked", Duplicate => "duplicate", Unsupported => "unsupported", Failed => "failed");

/// One allowlist entry: who may message the bot (Go `AllowmentValidation`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AllowmentValidation {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_id: String,
    pub allowment_id: String,
    #[serde(rename = "telegramScopeType")]
    pub scope_type: ScopeType,
    #[serde(rename = "telegramScopeId")]
    pub scope_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider_label: String,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_gate: Option<GroupGate>,
    pub validation_state: AllowmentValidationState,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
    #[serde(default)]
    pub validated_at: DateTime<Utc>,
    pub redaction_status: RedactionStatus,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub safe_evidence: HashMap<String, String>,
}

impl Default for AllowmentValidation {
    fn default() -> Self {
        AllowmentValidation {
            tenant_id: String::new(),
            connector_id: String::new(),
            allowment_id: String::new(),
            scope_type: ScopeType::User,
            scope_id: String::new(),
            provider_label: String::new(),
            enabled: false,
            group_gate: None,
            validation_state: AllowmentValidationState::Invalid,
            reason_code: String::new(),
            validated_at: DateTime::<Utc>::default(),
            redaction_status: RedactionStatus::Redacted,
            safe_evidence: HashMap::new(),
        }
    }
}

/// One inbound Telegram update (Go `InboundUpdate`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InboundUpdate {
    pub update_id: String,
    pub message_id: String,
    pub chat_id: String,
    pub sender_id: String,
    pub text: String,
    pub conversation_type: ConversationType,
    pub mentioned: bool,
    pub command: bool,
    pub unsupported_surface: String,
    pub received_at: DateTime<Utc>,
}

/// Routing decision for one inbound update (Go `RouteDecision`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RouteDecision {
    pub outcome: RouteOutcome,
    pub reason_code: String,
    pub surface: String,
}

/// Index of validated, enabled allowments by scope (Go `AllowmentIndex`).
#[derive(Debug, Clone, Default)]
pub struct AllowmentIndex {
    users: HashMap<String, AllowmentValidation>,
    direct_chats: HashMap<String, AllowmentValidation>,
    groups: HashMap<String, AllowmentValidation>,
}

/// Go `NewAllowmentIndex`: keeps only enabled, valid allowments with a
/// non-empty scope id, indexed by scope type.
#[must_use]
pub fn new_allowment_index(items: Vec<AllowmentValidation>) -> AllowmentIndex {
    let mut index = AllowmentIndex::default();
    for item in items {
        if !item.enabled || item.validation_state != AllowmentValidationState::Valid {
            continue;
        }
        let scope_id = item.scope_id.trim().to_string();
        if scope_id.is_empty() {
            continue;
        }
        match item.scope_type {
            ScopeType::User => {
                index.users.insert(scope_id, item);
            }
            ScopeType::DirectChat => {
                index.direct_chats.insert(scope_id, item);
            }
            ScopeType::Group => {
                index.groups.insert(scope_id, item);
            }
        }
    }
    index
}

/// Go `DecideRoute`: evaluates an inbound update against the allowment index
/// into an accepted/ignored/blocked/unsupported/failed decision.
#[must_use]
pub fn decide_route(update: &InboundUpdate, allowments: &AllowmentIndex) -> RouteDecision {
    let surface = update.conversation_type.as_str().to_string();
    if update.chat_id.trim().is_empty() {
        return RouteDecision {
            outcome: RouteOutcome::Failed,
            reason_code: "missing_durable_identity".to_string(),
            surface,
        };
    }
    if !update.unsupported_surface.trim().is_empty() {
        return RouteDecision {
            outcome: RouteOutcome::Unsupported,
            reason_code: DiagnosticReasonCode::UnsupportedCapability
                .as_str()
                .to_string(),
            surface: update.unsupported_surface.clone(),
        };
    }
    if update.text.trim().is_empty() {
        return RouteDecision {
            outcome: RouteOutcome::Unsupported,
            reason_code: DiagnosticReasonCode::UnsupportedCapability
                .as_str()
                .to_string(),
            surface,
        };
    }
    match update.conversation_type {
        ConversationType::Direct => {
            if allowments.direct_chats.contains_key(update.chat_id.trim()) {
                return RouteDecision {
                    outcome: RouteOutcome::Accepted,
                    reason_code: "accepted".to_string(),
                    surface: "direct_message".to_string(),
                };
            }
            if allowments.users.contains_key(update.sender_id.trim()) {
                return RouteDecision {
                    outcome: RouteOutcome::Accepted,
                    reason_code: "accepted".to_string(),
                    surface: "direct_message".to_string(),
                };
            }
            RouteDecision {
                outcome: RouteOutcome::Blocked,
                reason_code: DiagnosticReasonCode::BlockedRoute.as_str().to_string(),
                surface: "direct_message".to_string(),
            }
        }
        ConversationType::Group => {
            let Some(allowment) = allowments.groups.get(update.chat_id.trim()) else {
                return RouteDecision {
                    outcome: RouteOutcome::Blocked,
                    reason_code: DiagnosticReasonCode::BlockedRoute.as_str().to_string(),
                    surface: "group".to_string(),
                };
            };
            if allowment.group_gate.is_none()
                || allowment.group_gate == Some(GroupGate::MentionOrCommandRequired)
            {
                if !update.mentioned && !update.command {
                    return RouteDecision {
                        outcome: RouteOutcome::Ignored,
                        reason_code: "mention_required".to_string(),
                        surface: "group".to_string(),
                    };
                }
            }
            RouteDecision {
                outcome: RouteOutcome::Accepted,
                reason_code: "accepted".to_string(),
                surface: "group".to_string(),
            }
        }
    }
}

/// Go `normalizeAllowments`: fills tenant/connector, defaults the group gate,
/// validation state, reason code, validated-at timestamp, and redaction
/// status. Rust enums are non-empty by construction, so the Go empty-string
/// defaulting for `group_gate`/`validation_state`/`redaction_status` is
/// expressed through the enum defaults (`group_gate` defaults per scope,
/// mirroring Go).
pub(crate) fn normalize_allowments(
    tenant_id: &str,
    connector_id: &str,
    allowments: Vec<AllowmentValidation>,
    now: DateTime<Utc>,
) -> Vec<AllowmentValidation> {
    allowments
        .into_iter()
        .map(|mut item| {
            item.tenant_id = first_non_empty(&[&item.tenant_id, tenant_id]);
            item.connector_id = first_non_empty(&[&item.connector_id, connector_id]);
            if item.group_gate.is_none() {
                item.group_gate = Some(if item.scope_type == ScopeType::Group {
                    GroupGate::MentionOrCommandRequired
                } else {
                    GroupGate::NotApplicable
                });
            }
            if item.reason_code.is_empty() {
                item.reason_code = if item.validation_state == AllowmentValidationState::Valid {
                    "healthy".to_string()
                } else {
                    DiagnosticReasonCode::BlockedRoute.as_str().to_string()
                };
            }
            if crate::is_unset_time(&item.validated_at) {
                item.validated_at = now;
            }
            item
        })
        .collect()
}

/// Go `allowmentState`: aggregate state across the allowment list.
#[must_use]
pub(crate) fn allowment_state(allowments: &[AllowmentValidation]) -> AllowmentState {
    if allowments.is_empty() {
        return AllowmentState::None;
    }
    let mut valid = false;
    let mut stale = false;
    for item in allowments {
        if item.enabled && item.validation_state == AllowmentValidationState::Valid {
            valid = true;
        }
        if item.validation_state == AllowmentValidationState::Stale {
            stale = true;
        }
    }
    if valid {
        AllowmentState::Valid
    } else if stale {
        AllowmentState::Stale
    } else {
        AllowmentState::Partial
    }
}

/// Go `hasValidAllowment`.
#[must_use]
pub(crate) fn has_valid_allowment(allowments: &[AllowmentValidation]) -> bool {
    allowment_state(allowments) == AllowmentState::Valid
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allow(scope_type: ScopeType, scope_id: &str) -> AllowmentValidation {
        AllowmentValidation {
            scope_type,
            scope_id: scope_id.to_string(),
            enabled: true,
            validation_state: AllowmentValidationState::Valid,
            ..AllowmentValidation::default()
        }
    }

    // Go TestRouteDecisionEnforcesExplicitDirectAllowment.
    #[test]
    fn route_decision_enforces_explicit_direct_allowment() {
        let allowed = new_allowment_index(vec![allow(ScopeType::DirectChat, "chat_1")]);
        let accepted = decide_route(
            &InboundUpdate {
                conversation_type: ConversationType::Direct,
                chat_id: "chat_1".to_string(),
                sender_id: "user_1".to_string(),
                text: "hello".to_string(),
                ..InboundUpdate::default()
            },
            &allowed,
        );
        assert_eq!(
            accepted.outcome,
            RouteOutcome::Accepted,
            "allowed direct chat"
        );

        let blocked = decide_route(
            &InboundUpdate {
                conversation_type: ConversationType::Direct,
                chat_id: "chat_2".to_string(),
                sender_id: "user_2".to_string(),
                text: "hello".to_string(),
                ..InboundUpdate::default()
            },
            &allowed,
        );
        assert_eq!(
            blocked.outcome,
            RouteOutcome::Blocked,
            "unknown direct chat"
        );
        assert_eq!(blocked.reason_code, "blocked_route");
    }

    // Go TestRouteDecisionRequiresGroupAllowmentAndMentionOrCommand.
    #[test]
    fn route_decision_requires_group_allowment_and_mention_or_command() {
        let allowed = new_allowment_index(vec![AllowmentValidation {
            scope_type: ScopeType::Group,
            scope_id: "group_1".to_string(),
            enabled: true,
            group_gate: Some(GroupGate::MentionOrCommandRequired),
            validation_state: AllowmentValidationState::Valid,
            ..AllowmentValidation::default()
        }]);

        let ignored = decide_route(
            &InboundUpdate {
                conversation_type: ConversationType::Group,
                chat_id: "group_1".to_string(),
                text: "ordinary chatter".to_string(),
                ..InboundUpdate::default()
            },
            &allowed,
        );
        assert_eq!(
            ignored.outcome,
            RouteOutcome::Ignored,
            "group without mention"
        );
        assert_eq!(ignored.reason_code, "mention_required");

        let accepted = decide_route(
            &InboundUpdate {
                conversation_type: ConversationType::Group,
                chat_id: "group_1".to_string(),
                text: "/ask status".to_string(),
                command: true,
                ..InboundUpdate::default()
            },
            &allowed,
        );
        assert_eq!(accepted.outcome, RouteOutcome::Accepted, "group command");

        let blocked = decide_route(
            &InboundUpdate {
                conversation_type: ConversationType::Group,
                chat_id: "group_2".to_string(),
                text: "@bot status".to_string(),
                mentioned: true,
                ..InboundUpdate::default()
            },
            &allowed,
        );
        assert_eq!(blocked.outcome, RouteOutcome::Blocked, "unallowed group");
    }

    // Go TestRouteDecisionRejectsUnsupportedAndMissingIdentity.
    #[test]
    fn route_decision_rejects_unsupported_and_missing_identity() {
        let allowed = new_allowment_index(vec![allow(ScopeType::DirectChat, "chat_1")]);

        let unsupported = decide_route(
            &InboundUpdate {
                conversation_type: ConversationType::Direct,
                chat_id: "chat_1".to_string(),
                unsupported_surface: "voice".to_string(),
                text: "voice".to_string(),
                ..InboundUpdate::default()
            },
            &allowed,
        );
        assert_eq!(
            unsupported.outcome,
            RouteOutcome::Unsupported,
            "unsupported surface"
        );

        let failed = decide_route(
            &InboundUpdate {
                conversation_type: ConversationType::Direct,
                text: "missing chat".to_string(),
                ..InboundUpdate::default()
            },
            &allowed,
        );
        assert_eq!(failed.outcome, RouteOutcome::Failed, "missing identity");
        assert_eq!(failed.reason_code, "missing_durable_identity");
    }
}
