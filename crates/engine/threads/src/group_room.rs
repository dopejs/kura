use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;

use crate::error::ThreadsError;
use crate::lifecycle::LifecycleAction;
use crate::redaction::RedactionStatus;
use crate::redaction::safe_group_room_evidence_summary;
use crate::source::SourceKind;
use crate::utc_now_or;

/// Go: `ConversationShape`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationShape {
    DirectMessage,
    Group,
    Room,
    Web,
    Unknown,
    Unsupported,
}

/// Go: `ShapeEvidenceStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShapeEvidenceStatus {
    Proven,
    Partial,
    Unsupported,
    Failed,
}

/// Go: `MentionStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MentionStatus {
    Qualified,
    Missing,
    Ambiguous,
    Unsupported,
    Failed,
}

/// Go: `AllowlistStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AllowlistStatus {
    Eligible,
    NotAllowlisted,
    Unsupported,
    Failed,
}

/// Go: `ParticipationDecisionValue`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParticipationDecisionValue {
    Accepted,
    Ignored,
    Blocked,
    Denied,
    Duplicate,
    Unsupported,
    Failed,
}

/// Go: `ResetEventStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResetEventStatus {
    Succeeded,
    Denied,
    FailedClosed,
    Unsupported,
}

// Group/room reason codes (Go string constants).
pub const GROUP_ROOM_REASON_ACCEPTED_QUALIFYING_MENTION: &str = "accepted_qualifying_mention";
pub const GROUP_ROOM_REASON_MISSING_QUALIFYING_MENTION: &str = "missing_qualifying_mention";
pub const GROUP_ROOM_REASON_NOT_ALLOWLISTED: &str = "not_allowlisted";
pub const GROUP_ROOM_REASON_PERMISSION_DENIED: &str = "permission_denied";
pub const GROUP_ROOM_REASON_DUPLICATE_SOURCE_EVENT: &str = "duplicate_source_event";
pub const GROUP_ROOM_REASON_UNSUPPORTED_CONVERSATION_SHAPE: &str = "unsupported_conversation_shape";
pub const GROUP_ROOM_REASON_REDACTION_FAILED: &str = "redaction_failed";
pub const GROUP_ROOM_REASON_INCOMPLETE_SOURCE_IDENTITY: &str = "incomplete_source_identity";
pub const GROUP_ROOM_REASON_CONNECTOR_DISABLED: &str = "connector_disabled";
pub const GROUP_ROOM_REASON_CONNECTOR_FAILED: &str = "connector_failed";
pub const GROUP_ROOM_REASON_SCOPED_RESET_SUCCEEDED: &str = "scoped_reset_succeeded";

/// Go: `ConversationShapeEvidence`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationShapeEvidence {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub conversation_shape_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_segment_id: String,
    pub shape: ConversationShape,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_kind: Option<SourceKind>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_account_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_conversation_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_conversation_summary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub participant_summary: String,
    pub shape_evidence_status: ShapeEvidenceStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_expires_at: Option<DateTime<Utc>>,
    pub redaction_status: RedactionStatus,
}

/// Go: `ParticipationPolicy`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParticipationPolicy {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub participation_policy_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_account_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_conversation_id: String,
    pub shape: ConversationShape,
    pub allowlist_eligible: bool,
    pub qualifying_mention_required: bool,
    pub policy_status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub configured_by_principal_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configured_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_expires_at: Option<DateTime<Utc>>,
    pub redaction_status: RedactionStatus,
}

/// Go: `ParticipationDecision`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParticipationDecision {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub participation_decision_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_segment_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_account_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_conversation_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_message_id: String,
    pub conversation_shape: ConversationShape,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub policy_id: String,
    pub mention_status: MentionStatus,
    pub allowlist_status: AllowlistStatus,
    pub decision: ParticipationDecisionValue,
    pub reason_code: String,
    pub created_assistant_work: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurred_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_expires_at: Option<DateTime<Utc>>,
    pub redaction_status: RedactionStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub safe_summary: String,
}

/// Go: `ResetEvent`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetEvent {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reset_event_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thread_id: String,
    pub conversation_shape: ConversationShape,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_conversation_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub actor_principal_id: String,
    pub permission_gate: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prior_session_segment_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub resulting_session_segment_id: String,
    pub status: ResetEventStatus,
    pub reason_code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub audit_event_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_expires_at: Option<DateTime<Utc>>,
    pub redaction_status: RedactionStatus,
}

/// Go: `ParticipationEvaluationInput`. `occurred_at: None` maps to Go's zero
/// time (defaults to now).
#[derive(Debug, Clone)]
pub struct ParticipationEvaluationInput {
    pub shape: ConversationShape,
    pub allowlist_eligible: bool,
    pub qualifying_mention: bool,
    pub permission_allowed: bool,
    pub duplicate: bool,
    pub unsupported: bool,
    pub redaction_allowed: bool,
    pub occurred_at: Option<DateTime<Utc>>,
    pub safe_summary: String,
}

/// Go: `ConversationShapeResolutionInput`. `claimed_shape: None` maps to Go's
/// empty claimed shape.
#[derive(Debug, Clone)]
pub struct ConversationShapeResolutionInput {
    pub tenant_id: String,
    pub thread_id: String,
    pub session_segment_id: String,
    pub source_kind: SourceKind,
    pub connector_id: String,
    pub connector_kind: String,
    pub source_account_id: String,
    pub source_conversation_id: String,
    pub source_conversation_summary: String,
    pub claimed_shape: Option<ConversationShape>,
    pub now: Option<DateTime<Utc>>,
}

/// Go: `NormalizeConversationShape` — trims and validates against the known
/// shape set.
pub fn normalize_conversation_shape(shape: &str) -> Result<ConversationShape, ThreadsError> {
    match shape.trim() {
        "direct_message" => Ok(ConversationShape::DirectMessage),
        "group" => Ok(ConversationShape::Group),
        "room" => Ok(ConversationShape::Room),
        "web" => Ok(ConversationShape::Web),
        "unknown" => Ok(ConversationShape::Unknown),
        "unsupported" => Ok(ConversationShape::Unsupported),
        _ => Err(ThreadsError::InvalidConversationShape),
    }
}

/// Go: `ResolveConversationShape` — unclaimed shapes fall back to source-kind
/// evidence; unknown/unsupported claims downgrade the evidence status.
pub fn resolve_conversation_shape(
    input: &ConversationShapeResolutionInput,
) -> ConversationShapeEvidence {
    let now = utc_now_or(input.now);
    let mut status = ShapeEvidenceStatus::Proven;
    let shape = match input.claimed_shape {
        None => match input.source_kind {
            SourceKind::Shell => ConversationShape::Web,
            SourceKind::Legacy => {
                status = ShapeEvidenceStatus::Partial;
                ConversationShape::Unknown
            }
            _ => {
                status = ShapeEvidenceStatus::Unsupported;
                ConversationShape::Unsupported
            }
        },
        Some(ConversationShape::Unknown) => {
            status = ShapeEvidenceStatus::Partial;
            ConversationShape::Unknown
        }
        Some(ConversationShape::Unsupported) => {
            status = ShapeEvidenceStatus::Unsupported;
            ConversationShape::Unsupported
        }
        Some(shape) => shape,
    };
    ConversationShapeEvidence {
        conversation_shape_id: String::new(),
        tenant_id: input.tenant_id.clone(),
        thread_id: input.thread_id.clone(),
        session_segment_id: input.session_segment_id.clone(),
        shape,
        source_kind: Some(input.source_kind),
        connector_id: input.connector_id.clone(),
        connector_kind: input.connector_kind.clone(),
        source_account_id: input.source_account_id.clone(),
        source_conversation_id: input.source_conversation_id.trim().to_string(),
        source_conversation_summary: safe_group_room_evidence_summary(
            &input.source_conversation_summary,
        )
        .text,
        participant_summary: String::new(),
        shape_evidence_status: status,
        recorded_at: Some(now),
        updated_at: Some(now),
        retention_expires_at: None,
        redaction_status: RedactionStatus::Redacted,
    }
}

/// Go: `DefaultParticipationPolicy`.
pub fn default_participation_policy(shape: ConversationShape) -> ParticipationPolicy {
    ParticipationPolicy {
        participation_policy_id: String::new(),
        tenant_id: String::new(),
        connector_id: String::new(),
        connector_kind: String::new(),
        source_account_id: String::new(),
        source_conversation_id: String::new(),
        shape,
        allowlist_eligible: false,
        qualifying_mention_required: true,
        policy_status: "enabled".to_string(),
        configured_by_principal_id: String::new(),
        configured_at: None,
        updated_at: None,
        retention_expires_at: None,
        redaction_status: RedactionStatus::Redacted,
    }
}

/// Go: `EvaluateParticipation` — fail-closed decision ladder: unsupported
/// shape, redaction failure, duplicate, permission, allowlist, then mention.
pub fn evaluate_participation(input: &ParticipationEvaluationInput) -> ParticipationDecision {
    let mut decision = ParticipationDecision {
        participation_decision_id: String::new(),
        tenant_id: String::new(),
        thread_id: String::new(),
        session_segment_id: String::new(),
        connector_id: String::new(),
        connector_kind: String::new(),
        source_account_id: String::new(),
        source_conversation_id: String::new(),
        source_message_id: String::new(),
        conversation_shape: input.shape,
        policy_id: String::new(),
        mention_status: MentionStatus::Qualified,
        allowlist_status: AllowlistStatus::Eligible,
        decision: ParticipationDecisionValue::Accepted,
        reason_code: GROUP_ROOM_REASON_ACCEPTED_QUALIFYING_MENTION.to_string(),
        created_assistant_work: matches!(
            input.shape,
            ConversationShape::Group | ConversationShape::Room
        ),
        occurred_at: Some(utc_now_or(input.occurred_at)),
        retention_expires_at: None,
        redaction_status: RedactionStatus::Redacted,
        safe_summary: safe_group_room_evidence_summary(&input.safe_summary).text,
    };
    if input.unsupported
        || !matches!(
            input.shape,
            ConversationShape::Group | ConversationShape::Room
        )
    {
        decision.mention_status = MentionStatus::Unsupported;
        decision.allowlist_status = AllowlistStatus::Unsupported;
        decision.decision = ParticipationDecisionValue::Unsupported;
        decision.reason_code = GROUP_ROOM_REASON_UNSUPPORTED_CONVERSATION_SHAPE.to_string();
        decision.created_assistant_work = false;
    } else if !input.redaction_allowed {
        decision.decision = ParticipationDecisionValue::Failed;
        decision.reason_code = GROUP_ROOM_REASON_REDACTION_FAILED.to_string();
        decision.redaction_status = RedactionStatus::Suppressed;
        decision.safe_summary = "suppressed".to_string();
        decision.created_assistant_work = false;
    } else if input.duplicate {
        decision.decision = ParticipationDecisionValue::Duplicate;
        decision.reason_code = GROUP_ROOM_REASON_DUPLICATE_SOURCE_EVENT.to_string();
        decision.created_assistant_work = false;
    } else if !input.permission_allowed {
        decision.decision = ParticipationDecisionValue::Denied;
        decision.reason_code = GROUP_ROOM_REASON_PERMISSION_DENIED.to_string();
        decision.created_assistant_work = false;
    } else if !input.allowlist_eligible {
        decision.allowlist_status = AllowlistStatus::NotAllowlisted;
        decision.decision = ParticipationDecisionValue::Blocked;
        decision.reason_code = GROUP_ROOM_REASON_NOT_ALLOWLISTED.to_string();
        decision.created_assistant_work = false;
    } else if !input.qualifying_mention {
        decision.mention_status = MentionStatus::Missing;
        decision.decision = ParticipationDecisionValue::Ignored;
        decision.reason_code = GROUP_ROOM_REASON_MISSING_QUALIFYING_MENTION.to_string();
        decision.created_assistant_work = false;
    }
    decision
}

/// Go: `BuildScopedResetEvent` — records the reset against the conversation
/// shape; unknown/unsupported shapes fail closed as unsupported.
pub fn build_scoped_reset_event(
    action: &LifecycleAction,
    shape: &ConversationShapeEvidence,
) -> ResetEvent {
    let mut status = ResetEventStatus::Succeeded;
    let mut reason = action.reason_code.clone();
    if reason.trim().is_empty() {
        reason = GROUP_ROOM_REASON_SCOPED_RESET_SUCCEEDED.to_string();
    }
    let conversation_shape = shape.shape;
    if matches!(
        conversation_shape,
        ConversationShape::Unknown | ConversationShape::Unsupported
    ) || shape.shape_evidence_status == ShapeEvidenceStatus::Unsupported
    {
        status = ResetEventStatus::Unsupported;
        reason = GROUP_ROOM_REASON_UNSUPPORTED_CONVERSATION_SHAPE.to_string();
    }
    ResetEvent {
        reset_event_id: String::new(),
        tenant_id: action.tenant_id.clone(),
        thread_id: action.thread_id.clone(),
        conversation_shape,
        source_conversation_id: shape.source_conversation_id.trim().to_string(),
        actor_principal_id: action.actor_principal_id.clone(),
        permission_gate: "connectors.manage".to_string(),
        prior_session_segment_id: action.prior_session_segment_id.clone(),
        resulting_session_segment_id: action.resulting_session_segment_id.clone(),
        status,
        reason_code: reason,
        requested_at: Some(action.requested_at),
        completed_at: Some(action.completed_at),
        audit_event_id: action.audit_event_id.clone(),
        retention_expires_at: None,
        redaction_status: RedactionStatus::Redacted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::TimeZone;

    use crate::lifecycle::LifecycleActionKind;
    use crate::lifecycle::LifecycleState;

    fn evaluation_input(shape: ConversationShape) -> ParticipationEvaluationInput {
        ParticipationEvaluationInput {
            shape,
            allowlist_eligible: false,
            qualifying_mention: false,
            permission_allowed: false,
            duplicate: false,
            unsupported: false,
            redaction_allowed: false,
            occurred_at: None,
            safe_summary: String::new(),
        }
    }

    // Port of TestEvaluateParticipationRequiresAllowlistAndMention.
    #[test]
    fn evaluate_participation_requires_allowlist_and_mention() {
        let accepted = evaluate_participation(&ParticipationEvaluationInput {
            allowlist_eligible: true,
            qualifying_mention: true,
            permission_allowed: true,
            redaction_allowed: true,
            safe_summary: "safe room mention".to_string(),
            ..evaluation_input(ConversationShape::Room)
        });
        assert_eq!(accepted.decision, ParticipationDecisionValue::Accepted);
        assert!(accepted.created_assistant_work);

        let missing_mention = evaluate_participation(&ParticipationEvaluationInput {
            allowlist_eligible: true,
            permission_allowed: true,
            redaction_allowed: true,
            ..evaluation_input(ConversationShape::Room)
        });
        assert_eq!(
            missing_mention.decision,
            ParticipationDecisionValue::Ignored
        );
        assert_eq!(
            missing_mention.reason_code,
            GROUP_ROOM_REASON_MISSING_QUALIFYING_MENTION
        );
        assert!(!missing_mention.created_assistant_work);

        let not_allowlisted = evaluate_participation(&ParticipationEvaluationInput {
            qualifying_mention: true,
            permission_allowed: true,
            redaction_allowed: true,
            ..evaluation_input(ConversationShape::Group)
        });
        assert_eq!(
            not_allowlisted.decision,
            ParticipationDecisionValue::Blocked
        );
        assert_eq!(
            not_allowlisted.reason_code,
            GROUP_ROOM_REASON_NOT_ALLOWLISTED
        );
        assert!(!not_allowlisted.created_assistant_work);
    }

    // Port of TestResolveConversationShapePreservesStableRoomIdentity.
    #[test]
    fn resolve_conversation_shape_preserves_stable_room_identity() {
        let resolution_input =
            |thread_id: &str, segment: &str, conversation: &str| ConversationShapeResolutionInput {
                tenant_id: "ten_1".to_string(),
                thread_id: thread_id.to_string(),
                session_segment_id: segment.to_string(),
                source_kind: SourceKind::Channel,
                connector_id: "slack-main".to_string(),
                connector_kind: "slack".to_string(),
                source_account_id: "workspace_redacted".to_string(),
                source_conversation_id: conversation.to_string(),
                source_conversation_summary: "Slack / #support".to_string(),
                claimed_shape: Some(ConversationShape::Room),
                now: None,
            };
        let first =
            resolve_conversation_shape(&resolution_input("thr_room_1", "seg_1", "channel_a"));
        let second =
            resolve_conversation_shape(&resolution_input("thr_room_2", "seg_2", "channel_b"));
        assert_eq!(first.shape, ConversationShape::Room);
        assert_eq!(first.shape_evidence_status, ShapeEvidenceStatus::Proven);
        assert_ne!(first.source_conversation_id, second.source_conversation_id);

        let unsupported = resolve_conversation_shape(&ConversationShapeResolutionInput {
            tenant_id: String::new(),
            thread_id: String::new(),
            session_segment_id: String::new(),
            source_kind: SourceKind::Channel,
            connector_id: String::new(),
            connector_kind: String::new(),
            source_account_id: String::new(),
            source_conversation_id: String::new(),
            source_conversation_summary: String::new(),
            claimed_shape: None,
            now: None,
        });
        assert_eq!(unsupported.shape, ConversationShape::Unsupported);
        assert_eq!(
            unsupported.shape_evidence_status,
            ShapeEvidenceStatus::Unsupported
        );
    }

    // Port of TestUnknownShapeDoesNotCreateParticipation.
    #[test]
    fn unknown_shape_does_not_create_participation() {
        let decision = evaluate_participation(&ParticipationEvaluationInput {
            allowlist_eligible: true,
            qualifying_mention: true,
            permission_allowed: true,
            redaction_allowed: true,
            ..evaluation_input(ConversationShape::Unknown)
        });
        assert_eq!(decision.decision, ParticipationDecisionValue::Unsupported);
        assert!(!decision.created_assistant_work);
    }

    fn reset_action(now: DateTime<Utc>) -> LifecycleAction {
        LifecycleAction {
            lifecycle_action_id: String::new(),
            thread_id: "thr_reset".to_string(),
            tenant_id: "ten_1".to_string(),
            action_kind: LifecycleActionKind::Reset,
            actor_principal_id: "prn_1".to_string(),
            prior_state: LifecycleState::Active,
            resulting_state: LifecycleState::Reset,
            prior_session_segment_id: "seg_old".to_string(),
            resulting_session_segment_id: "seg_new".to_string(),
            reason_code: "operator_reset".to_string(),
            requested_at: now,
            completed_at: now,
            status: "succeeded".to_string(),
            audit_event_id: "audit_reset".to_string(),
            retention_expires_at: None,
            redaction_status: RedactionStatus::Redacted,
        }
    }

    fn shape_evidence(
        shape: ConversationShape,
        status: ShapeEvidenceStatus,
    ) -> ConversationShapeEvidence {
        ConversationShapeEvidence {
            conversation_shape_id: String::new(),
            tenant_id: String::new(),
            thread_id: String::new(),
            session_segment_id: String::new(),
            shape,
            source_kind: None,
            connector_id: String::new(),
            connector_kind: String::new(),
            source_account_id: String::new(),
            source_conversation_id: format!(
                "source_{}",
                serde_json::to_string(&shape).unwrap().trim_matches('"')
            ),
            source_conversation_summary: "safe summary".to_string(),
            participant_summary: String::new(),
            shape_evidence_status: status,
            recorded_at: None,
            updated_at: None,
            retention_expires_at: None,
            redaction_status: RedactionStatus::Redacted,
        }
    }

    // Port of TestBuildScopedResetEventCapturesConversationShapeAndSourceScope.
    #[test]
    fn scoped_reset_event_captures_conversation_shape_and_source_scope() {
        let now = Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap();
        let action = reset_action(now);
        for shape in [
            ConversationShape::DirectMessage,
            ConversationShape::Group,
            ConversationShape::Room,
            ConversationShape::Web,
        ] {
            let event = build_scoped_reset_event(
                &action,
                &shape_evidence(shape, ShapeEvidenceStatus::Proven),
            );
            assert_eq!(event.conversation_shape, shape);
            assert!(!event.source_conversation_id.is_empty());
            assert_eq!(event.status, ResetEventStatus::Succeeded);
            assert_eq!(event.permission_gate, "connectors.manage");
            assert_eq!(event.prior_session_segment_id, "seg_old");
            assert_eq!(event.resulting_session_segment_id, "seg_new");
        }
    }

    // Port of TestBuildScopedResetEventFailsClosedForUnsupportedSourceShape.
    #[test]
    fn scoped_reset_event_fails_closed_for_unsupported_source_shape() {
        let now = Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap();
        let mut action = reset_action(now);
        action.actor_principal_id = String::new();
        action.reason_code = String::new();
        let event = build_scoped_reset_event(
            &action,
            &shape_evidence(
                ConversationShape::Unsupported,
                ShapeEvidenceStatus::Unsupported,
            ),
        );
        assert_eq!(event.status, ResetEventStatus::Unsupported);
        assert_eq!(
            event.reason_code,
            GROUP_ROOM_REASON_UNSUPPORTED_CONVERSATION_SHAPE
        );
    }
}
