//! Thread continuity turn/preview events (port of `thread_continuity.go`).

use crate::util::{is_go_zero_time, now_utc, payload};
use crate::wire;
use crate::{Event, Resource, Scope};
use kura_threads::{ContinuityPreview, ContinuityTurn};

pub const THREAD_CONTINUITY_TURN_RECORDED_NAME: &str = "thread.continuity_turn_recorded";
pub const THREAD_CONTINUITY_PREVIEW_RECORDED_NAME: &str = "thread.continuity_preview_recorded";

/// Go: `ThreadContinuityTurnRecordedEvent` — metadata-only; the safe content
/// and artifact excerpts are deliberately NOT projected.
#[must_use]
pub fn thread_continuity_turn_recorded_event(turn: ContinuityTurn, outcome: &str) -> Event {
    let outcome = if outcome.is_empty() {
        "recorded"
    } else {
        outcome
    };
    let occurred_at = if is_go_zero_time(turn.recorded_at) {
        now_utc()
    } else {
        turn.recorded_at
    };
    Event {
        tenant_id: turn.tenant_id.clone(),
        category: "thread".to_string(),
        name: THREAD_CONTINUITY_TURN_RECORDED_NAME.to_string(),
        occurred_at,
        scope: Scope {
            session_id: turn.session_segment_id.clone(),
            ..Scope::default()
        },
        resource: Resource {
            kind: "thread_continuity_turn".to_string(),
            id: turn.continuity_turn_id.clone(),
        },
        payload: payload![
            "tenantId" => turn.tenant_id,
            "threadId" => turn.thread_id,
            "sessionSegmentId" => turn.session_segment_id,
            "continuityTurnId" => turn.continuity_turn_id,
            "dispatchId" => turn.dispatch_id,
            "action" => "turn_recorded",
            "outcome" => outcome,
            "reasonCode" => "included_recent",
            "redactionStatus" => wire::redaction_status(&turn.content_redaction_status),
            "acceptanceSequence" => turn.acceptance_sequence,
        ],
        ..Event::default()
    }
}

/// Go: `ThreadContinuityPreviewRecordedEvent` — metadata-only; the assembled
/// item detail is deliberately NOT projected.
#[must_use]
pub fn thread_continuity_preview_recorded_event(preview: ContinuityPreview) -> Event {
    let occurred_at = if is_go_zero_time(preview.assembly_completed_at) {
        now_utc()
    } else {
        preview.assembly_completed_at
    };
    Event {
        tenant_id: preview.tenant_id.clone(),
        category: "thread".to_string(),
        name: THREAD_CONTINUITY_PREVIEW_RECORDED_NAME.to_string(),
        occurred_at,
        scope: Scope {
            session_id: preview.session_segment_id.clone(),
            ..Scope::default()
        },
        resource: Resource {
            kind: "thread_continuity_preview".to_string(),
            id: preview.continuity_preview_id.clone(),
        },
        payload: payload![
            "tenantId" => preview.tenant_id,
            "threadId" => preview.thread_id,
            "sessionSegmentId" => preview.session_segment_id,
            "continuityPreviewId" => preview.continuity_preview_id,
            "dispatchId" => preview.dispatch_id,
            "action" => "preview_recorded",
            "outcome" => wire::continuity_status(&preview.status),
            "reasonCode" => preview.failure_class,
            "redactionStatus" => wire::redaction_status(&preview.redaction_status),
        ],
        ..Event::default()
    }
}
