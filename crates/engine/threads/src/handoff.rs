use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;

use crate::continuity::ContinuityTurn;
use crate::error::ThreadsError;
use crate::group_room::ConversationShape;
use crate::redaction::RedactionStatus;
use crate::redaction::safe_group_room_evidence_summary;
use crate::source::SourceKind;
use crate::utc_now_or;

/// Go: `HandoffStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffStatus {
    Succeeded,
    Denied,
    FailedClosed,
    Unsupported,
    Expired,
}

/// Go: `HandoffSourceReferenceStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffSourceReferenceStatus {
    Available,
    Consumed,
    Blocked,
    Expired,
    None,
}

/// Go: `HandoffSourceReferenceEligibility`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffSourceReferenceEligibility {
    Eligible,
    PermissionDenied,
    RedactionFailed,
    RetentionExpired,
    ResetBoundary,
    IncompleteEvidence,
    Unsupported,
}

/// Go: `HandoffSourceReferenceDecision`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffSourceReferenceDecision {
    Referenced,
    Excluded,
    Consumed,
}

/// Go: `HandoffLink`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffLink {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub handoff_link_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub source_thread_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_session_segment_id: String,
    pub destination_thread_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub destination_session_segment_id: String,
    pub source_conversation_shape: ConversationShape,
    pub destination_conversation_shape: ConversationShape,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_kind: Option<SourceKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination_kind: Option<SourceKind>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_connector_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub destination_connector_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_conversation_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub destination_conversation_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub actor_principal_id: String,
    pub permission_gate: String,
    pub status: HandoffStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub first_destination_response_id: String,
    pub source_reference_status: HandoffSourceReferenceStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_profile_projection: Option<kura_profiles::RuntimeProjection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumed_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_expires_at: Option<DateTime<Utc>>,
    pub redaction_status: RedactionStatus,
}

/// Go: `HandoffSourceReference`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffSourceReference {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub handoff_source_reference_id: String,
    pub handoff_link_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub source_thread_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_session_segment_id: String,
    pub destination_thread_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub destination_session_segment_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub continuity_turn_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub artifact_excerpt_ref: String,
    pub eligibility_status: HandoffSourceReferenceEligibility,
    pub decision: HandoffSourceReferenceDecision,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub safe_summary: String,
    pub redaction_status: RedactionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumed_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_expires_at: Option<DateTime<Utc>>,
}

/// Go: `HandoffValidationInput`.
#[derive(Debug, Clone)]
pub struct HandoffValidationInput {
    pub link: HandoffLink,
    pub has_mutation_permission: bool,
    pub source_eligible: bool,
    pub destination_eligible: bool,
    pub source_permission_allowed: bool,
    pub destination_permission_allowed: bool,
}

/// Go: `ValidateHandoff` — same-thread check first, then permission, then
/// eligibility; each failure mode is a distinct error.
pub fn validate_handoff(input: &HandoffValidationInput) -> Result<(), ThreadsError> {
    if input.link.source_thread_id.is_empty()
        || input.link.destination_thread_id.is_empty()
        || input.link.source_thread_id == input.link.destination_thread_id
    {
        return Err(ThreadsError::HandoffSameThread);
    }
    if !input.has_mutation_permission
        || !input.source_permission_allowed
        || !input.destination_permission_allowed
    {
        return Err(ThreadsError::HandoffPermissionDenied);
    }
    if !input.source_eligible || !input.destination_eligible {
        return Err(ThreadsError::HandoffNotEligible);
    }
    Ok(())
}

/// Go: `BuildHandoffSourceReferences` — every source turn is classified; the
/// last matching exclusion rule wins (cross-thread, reset boundary, unsafe
/// redaction, retention expiry).
///
/// A turn without `retention_expires_at` (Go zero time) skips the retention
/// check here, matching Go's `!IsZero()` guard.
pub fn build_handoff_source_references(
    link: &HandoffLink,
    turns: &[ContinuityTurn],
    now: Option<DateTime<Utc>>,
) -> Vec<HandoffSourceReference> {
    let now = utc_now_or(now);
    let mut refs = Vec::with_capacity(turns.len());
    for turn in turns {
        let mut eligibility = HandoffSourceReferenceEligibility::Eligible;
        let mut decision = HandoffSourceReferenceDecision::Referenced;
        if !turn.thread_id.is_empty() && turn.thread_id != link.source_thread_id {
            eligibility = HandoffSourceReferenceEligibility::IncompleteEvidence;
            decision = HandoffSourceReferenceDecision::Excluded;
        }
        if turn.session_segment_id != link.source_session_segment_id {
            eligibility = HandoffSourceReferenceEligibility::ResetBoundary;
            decision = HandoffSourceReferenceDecision::Excluded;
        }
        if matches!(
            turn.content_redaction_status,
            RedactionStatus::RedactionFailed | RedactionStatus::Suppressed
        ) {
            eligibility = HandoffSourceReferenceEligibility::RedactionFailed;
            decision = HandoffSourceReferenceDecision::Excluded;
        }
        if let Some(expires) = turn.retention_expires_at {
            // Go: !IsZero() && !After(now) — expires_at <= now is expired.
            if expires <= now {
                eligibility = HandoffSourceReferenceEligibility::RetentionExpired;
                decision = HandoffSourceReferenceDecision::Excluded;
            }
        }
        refs.push(HandoffSourceReference {
            handoff_source_reference_id: String::new(),
            handoff_link_id: link.handoff_link_id.clone(),
            tenant_id: link.tenant_id.clone(),
            source_thread_id: link.source_thread_id.clone(),
            source_session_segment_id: link.source_session_segment_id.clone(),
            destination_thread_id: link.destination_thread_id.clone(),
            destination_session_segment_id: link.destination_session_segment_id.clone(),
            continuity_turn_id: turn.continuity_turn_id.clone(),
            artifact_excerpt_ref: String::new(),
            eligibility_status: eligibility,
            decision,
            safe_summary: safe_group_room_evidence_summary(&turn.safe_content).text,
            redaction_status: RedactionStatus::Redacted,
            created_at: Some(now),
            consumed_at: None,
            retention_expires_at: turn.retention_expires_at,
        });
    }
    refs
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::Duration;
    use chrono::TimeZone;

    use crate::continuity::ContinuityRole;

    fn base_link() -> HandoffLink {
        HandoffLink {
            handoff_link_id: "handoff_1".to_string(),
            tenant_id: "ten_1".to_string(),
            source_thread_id: "thr_source".to_string(),
            source_session_segment_id: "seg_current".to_string(),
            destination_thread_id: "thr_dest".to_string(),
            destination_session_segment_id: "seg_dest".to_string(),
            source_conversation_shape: ConversationShape::Room,
            destination_conversation_shape: ConversationShape::Room,
            source_kind: None,
            destination_kind: None,
            source_connector_id: String::new(),
            destination_connector_id: String::new(),
            source_conversation_id: String::new(),
            destination_conversation_id: String::new(),
            actor_principal_id: String::new(),
            permission_gate: "connectors.manage".to_string(),
            status: HandoffStatus::Succeeded,
            reason_code: String::new(),
            first_destination_response_id: String::new(),
            source_reference_status: HandoffSourceReferenceStatus::Available,
            active_profile_projection: None,
            created_at: None,
            consumed_at: None,
            retention_expires_at: None,
            redaction_status: RedactionStatus::Redacted,
        }
    }

    fn validation_input(link: HandoffLink) -> HandoffValidationInput {
        HandoffValidationInput {
            link,
            has_mutation_permission: true,
            source_eligible: true,
            destination_eligible: true,
            source_permission_allowed: true,
            destination_permission_allowed: true,
        }
    }

    fn turn(
        id: &str,
        thread_id: &str,
        segment: &str,
        status: RedactionStatus,
        expires: Option<DateTime<Utc>>,
    ) -> ContinuityTurn {
        ContinuityTurn {
            continuity_turn_id: id.to_string(),
            tenant_id: String::new(),
            thread_id: thread_id.to_string(),
            session_segment_id: segment.to_string(),
            acceptance_sequence: 0,
            role: ContinuityRole::User,
            source_kind: SourceKind::Chat,
            source_linkage_id: String::new(),
            source_message_id: String::new(),
            source_timestamp: None,
            dispatch_id: String::new(),
            response_to_turn_id: String::new(),
            safe_content: id.to_string(),
            content_redaction_status: status,
            artifact_excerpt_refs: Vec::new(),
            recorded_at: Utc::now(),
            retention_expires_at: expires,
            source_event_key: String::new(),
        }
    }

    // Port of TestValidateHandoffRequiresSeparateDestinationThreadAndPermission.
    #[test]
    fn validate_handoff_requires_separate_destination_thread_and_permission() {
        let link = base_link();
        validate_handoff(&validation_input(link.clone())).expect("valid handoff");

        let mut same_thread = link.clone();
        same_thread.destination_thread_id = "thr_source".to_string();
        assert_eq!(
            validate_handoff(&validation_input(same_thread)).unwrap_err(),
            ThreadsError::HandoffSameThread
        );

        let mut no_permission = validation_input(link);
        no_permission.has_mutation_permission = false;
        assert_eq!(
            validate_handoff(&no_permission).unwrap_err(),
            ThreadsError::HandoffPermissionDenied
        );
    }

    // Port of TestBuildHandoffSourceReferencesExcludesResetBoundaryAndUnsafeTurns.
    #[test]
    fn source_references_exclude_reset_boundary_and_unsafe_turns() {
        let link = base_link();
        let now = Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap();
        let refs = build_handoff_source_references(
            &link,
            &[
                turn(
                    "turn_1",
                    "",
                    "seg_current",
                    RedactionStatus::Redacted,
                    Some(now + Duration::hours(1)),
                ),
                turn(
                    "turn_old",
                    "",
                    "seg_old",
                    RedactionStatus::Redacted,
                    Some(now + Duration::hours(1)),
                ),
                turn(
                    "turn_unsafe",
                    "",
                    "seg_current",
                    RedactionStatus::Suppressed,
                    Some(now + Duration::hours(1)),
                ),
                turn(
                    "turn_expired",
                    "",
                    "seg_current",
                    RedactionStatus::Redacted,
                    Some(now - Duration::hours(1)),
                ),
            ],
            Some(now),
        );
        assert_eq!(refs.len(), 4);
        assert_eq!(refs[0].decision, HandoffSourceReferenceDecision::Referenced);
        assert_eq!(
            refs[0].eligibility_status,
            HandoffSourceReferenceEligibility::Eligible
        );
        assert_eq!(refs[1].decision, HandoffSourceReferenceDecision::Excluded);
        assert_eq!(
            refs[1].eligibility_status,
            HandoffSourceReferenceEligibility::ResetBoundary
        );
        assert_eq!(refs[2].decision, HandoffSourceReferenceDecision::Excluded);
        assert_eq!(
            refs[2].eligibility_status,
            HandoffSourceReferenceEligibility::RedactionFailed
        );
        assert_eq!(refs[3].decision, HandoffSourceReferenceDecision::Excluded);
        assert_eq!(
            refs[3].eligibility_status,
            HandoffSourceReferenceEligibility::RetentionExpired
        );
    }

    // Port of TestBuildHandoffSourceReferencesExcludesCrossRoomTurns.
    #[test]
    fn source_references_exclude_cross_room_turns() {
        let mut link = base_link();
        link.destination_session_segment_id = String::new();
        let now = Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap();
        let refs = build_handoff_source_references(
            &link,
            &[turn(
                "turn_cross_room",
                "thr_other_room",
                "seg_current",
                RedactionStatus::Redacted,
                None,
            )],
            Some(now),
        );
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].decision, HandoffSourceReferenceDecision::Excluded);
        assert_eq!(
            refs[0].eligibility_status,
            HandoffSourceReferenceEligibility::IncompleteEvidence
        );
    }
}
