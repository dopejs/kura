//! Thread group/room conversation-shape, participation, scoped-reset, and
//! handoff-link events (port of `thread_group_room.go`).

use crate::util::{is_go_zero_time, now_utc, payload};
use crate::wire;
use crate::{Event, Resource, Scope};
use kura_threads::{
    ConversationShape, ConversationShapeEvidence, HandoffLink, LifecycleAction,
    ParticipationDecision, ResetEvent,
};

pub const THREAD_CONVERSATION_SHAPE_RECORDED_NAME: &str = "thread.conversation_shape_recorded";
pub const THREAD_PARTICIPATION_DECISION_RECORDED_NAME: &str =
    "thread.participation_decision_recorded";
pub const THREAD_RESET_SCOPED_NAME: &str = "thread.reset_scoped";
pub const THREAD_HANDOFF_LINKED_NAME: &str = "thread.handoff_linked";

/// Go: `ThreadConversationShapeEvent`.
#[must_use]
pub fn thread_conversation_shape_event(evidence: ConversationShapeEvidence) -> Event {
    let occurred_at = evidence.recorded_at.unwrap_or_else(now_utc);
    Event {
        tenant_id: evidence.tenant_id.clone(),
        category: "thread".to_string(),
        name: THREAD_CONVERSATION_SHAPE_RECORDED_NAME.to_string(),
        occurred_at,
        scope: Scope {
            session_id: evidence.session_segment_id.clone(),
            connector_id: evidence.connector_id.clone(),
            ..Scope::default()
        },
        resource: Resource {
            kind: "thread_conversation_shape".to_string(),
            id: evidence.conversation_shape_id.clone(),
        },
        payload: payload![
            "tenantId" => evidence.tenant_id,
            "threadId" => evidence.thread_id,
            "sessionSegmentId" => evidence.session_segment_id,
            "conversationShapeId" => evidence.conversation_shape_id,
            "shape" => wire::conversation_shape(&evidence.shape),
            "shapeEvidenceStatus" => wire::shape_evidence_status(&evidence.shape_evidence_status),
            "redactionStatus" => wire::redaction_status(&evidence.redaction_status),
        ],
        ..Event::default()
    }
}

/// Go: `ThreadParticipationDecisionEvent`.
#[must_use]
pub fn thread_participation_decision_event(decision: ParticipationDecision) -> Event {
    let occurred_at = decision.occurred_at.unwrap_or_else(now_utc);
    Event {
        tenant_id: decision.tenant_id.clone(),
        category: "thread".to_string(),
        name: THREAD_PARTICIPATION_DECISION_RECORDED_NAME.to_string(),
        occurred_at,
        scope: Scope {
            session_id: decision.session_segment_id.clone(),
            connector_id: decision.connector_id.clone(),
            ..Scope::default()
        },
        resource: Resource {
            kind: "thread_participation_decision".to_string(),
            id: decision.participation_decision_id.clone(),
        },
        payload: payload![
            "tenantId" => decision.tenant_id,
            "threadId" => decision.thread_id,
            "sessionSegmentId" => decision.session_segment_id,
            "participationDecisionId" => decision.participation_decision_id,
            "conversationShape" => wire::conversation_shape(&decision.conversation_shape),
            "decision" => wire::participation_decision_value(&decision.decision),
            "reasonCode" => decision.reason_code,
            "createdAssistantWork" => decision.created_assistant_work,
            "redactionStatus" => wire::redaction_status(&decision.redaction_status),
        ],
        ..Event::default()
    }
}

/// Go: `ThreadScopedResetEvent` — lifecycle-action flavor of the scoped reset.
#[must_use]
pub fn thread_scoped_reset_event(reset: LifecycleAction, shape: ConversationShape) -> Event {
    let occurred_at = if is_go_zero_time(reset.completed_at) {
        now_utc()
    } else {
        reset.completed_at
    };
    Event {
        tenant_id: reset.tenant_id.clone(),
        category: "thread".to_string(),
        name: THREAD_RESET_SCOPED_NAME.to_string(),
        occurred_at,
        scope: Scope {
            session_id: reset.resulting_session_segment_id.clone(),
            ..Scope::default()
        },
        resource: Resource {
            kind: "thread_reset_scoped".to_string(),
            id: reset.lifecycle_action_id.clone(),
        },
        payload: payload![
            "tenantId" => reset.tenant_id,
            "threadId" => reset.thread_id,
            "sessionSegmentId" => reset.resulting_session_segment_id,
            "conversationShape" => wire::conversation_shape(&shape),
            "lifecycleActionId" => reset.lifecycle_action_id,
            "status" => reset.status,
            "permissionGate" => "connectors.manage",
            "redactionStatus" => wire::redaction_status(&reset.redaction_status),
        ],
        ..Event::default()
    }
}

/// Go: `ThreadScopedResetEvidenceEvent` — reset-event flavor of the scoped reset.
#[must_use]
pub fn thread_scoped_reset_evidence_event(reset: ResetEvent) -> Event {
    let occurred_at = reset.completed_at.unwrap_or_else(now_utc);
    Event {
        tenant_id: reset.tenant_id.clone(),
        category: "thread".to_string(),
        name: THREAD_RESET_SCOPED_NAME.to_string(),
        occurred_at,
        scope: Scope {
            session_id: reset.resulting_session_segment_id.clone(),
            ..Scope::default()
        },
        resource: Resource {
            kind: "thread_reset_scoped".to_string(),
            id: reset.reset_event_id.clone(),
        },
        payload: payload![
            "tenantId" => reset.tenant_id,
            "threadId" => reset.thread_id,
            "sessionSegmentId" => reset.resulting_session_segment_id,
            "conversationShape" => wire::conversation_shape(&reset.conversation_shape),
            "resetEventId" => reset.reset_event_id,
            "status" => wire::reset_event_status(&reset.status),
            "reasonCode" => reset.reason_code,
            "permissionGate" => reset.permission_gate,
            "redactionStatus" => wire::redaction_status(&reset.redaction_status),
        ],
        ..Event::default()
    }
}

/// Go: `ThreadHandoffLinkedEvent`.
#[must_use]
pub fn thread_handoff_linked_event(link: HandoffLink) -> Event {
    let occurred_at = link.created_at.unwrap_or_else(now_utc);
    Event {
        tenant_id: link.tenant_id.clone(),
        category: "thread".to_string(),
        name: THREAD_HANDOFF_LINKED_NAME.to_string(),
        occurred_at,
        scope: Scope {
            session_id: link.destination_session_segment_id.clone(),
            connector_id: link.destination_connector_id.clone(),
            ..Scope::default()
        },
        resource: Resource {
            kind: "thread_handoff_link".to_string(),
            id: link.handoff_link_id.clone(),
        },
        payload: payload![
            "tenantId" => link.tenant_id,
            "handoffLinkId" => link.handoff_link_id,
            "sourceThreadId" => link.source_thread_id,
            "destinationThreadId" => link.destination_thread_id,
            "sourceConversationShape" => wire::conversation_shape(&link.source_conversation_shape),
            "destinationConversationShape" => wire::conversation_shape(&link.destination_conversation_shape),
            "status" => wire::handoff_status(&link.status),
            "reasonCode" => link.reason_code,
            "permissionGate" => link.permission_gate,
            "sourceReferenceStatus" => wire::handoff_source_reference_status(&link.source_reference_status),
            "redactionStatus" => wire::redaction_status(&link.redaction_status),
        ],
        ..Event::default()
    }
}
