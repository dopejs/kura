use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;

use crate::continuity::ContinuityPreview;
use crate::group_room::ConversationShapeEvidence;
use crate::group_room::ParticipationDecision;
use crate::group_room::ResetEvent;
use crate::handoff::HandoffLink;
use crate::lifecycle::LifecycleAction;
use crate::lifecycle::LifecycleActionKind;
use crate::lifecycle::LifecycleState;
use crate::lifecycle::SessionSegment;
use crate::lifecycle::Thread;
use crate::redaction::RedactionStatus;
use crate::redaction::safe_summary;
use crate::source::SourceKind;
use crate::source::SourceLinkage;

/// Runtime resource kinds surfaced on the operator trace. Go: threads
/// `RuntimeResourceKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeResourceKind {
    Session,
    Run,
    Workflow,
    Approval,
    ForegroundReply,
    BackgroundDelivery,
    ConnectorMessage,
}

/// Go: threads `RuntimeProjection` — metadata-only trace of a runtime
/// resource attached to a thread/segment.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeProjection {
    pub runtime_projection_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_segment_id: String,
    pub resource_kind: RuntimeResourceKind,
    pub resource_id: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
    pub occurred_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub route: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub safe_summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_expires_at: Option<DateTime<Utc>>,
    pub redaction_status: RedactionStatus,
}

/// Go: `ThreadListResponse`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadListResponse {
    pub tenant_id: String,
    pub page: ThreadPage,
    pub items: Vec<ThreadResource>,
}

/// Go: `ThreadDetailResponse`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadDetailResponse {
    pub thread: ThreadResource,
    #[serde(default)]
    pub session_segments: Vec<SessionSegment>,
    #[serde(default)]
    pub source_linkages: Vec<SourceLinkage>,
    #[serde(default)]
    pub runtime_projections: Vec<RuntimeProjection>,
    #[serde(default)]
    pub lifecycle_actions: Vec<LifecycleAction>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub continuity_previews: Vec<ContinuityPreview>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_shape: Option<ConversationShapeEvidence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub participation_decisions: Vec<ParticipationDecision>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reset_events: Vec<ResetEvent>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub handoff_links: Vec<HandoffLink>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_profile_projection: Option<kura_profiles::RuntimeProjection>,
}

/// Go: `ThreadPage`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadPage {
    pub limit: i32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub next_cursor: String,
    pub order: String,
}

/// Metadata-only API view of a thread. Go: `ThreadResource`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadResource {
    pub thread_id: String,
    pub tenant_id: String,
    pub lifecycle_state: LifecycleState,
    pub source_kind: SourceKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_summary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub current_session_segment_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub current_session_id: String,
    pub last_activity_at: DateTime<Utc>,
    pub available_actions: Vec<LifecycleActionKind>,
    pub redaction_status: RedactionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_expires_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

/// Go: `BuildThreadResource`.
pub fn build_thread_resource(thread: &Thread, current_session_id: &str) -> ThreadResource {
    ThreadResource {
        thread_id: thread.thread_id.clone(),
        tenant_id: thread.tenant_id.clone(),
        lifecycle_state: thread.lifecycle_state,
        source_kind: thread.source_kind,
        source_summary: thread.source_summary.clone(),
        current_session_segment_id: thread.current_session_segment_id.clone(),
        current_session_id: current_session_id.to_string(),
        last_activity_at: thread.last_activity_at,
        available_actions: available_actions(thread.lifecycle_state),
        redaction_status: thread.redaction_status,
        retention_expires_at: thread.retention_expires_at,
        updated_at: thread.updated_at,
    }
}

/// Go: `RuntimeProjectionInput`. `redaction_status: None` maps to Go's empty
/// status and defaults to redacted.
#[derive(Debug, Clone)]
pub struct RuntimeProjectionInput {
    pub projection_id: String,
    pub thread_id: String,
    pub tenant_id: String,
    pub session_segment_id: String,
    pub resource_kind: RuntimeResourceKind,
    pub resource_id: String,
    pub status: String,
    pub reason_code: String,
    pub occurred_at: DateTime<Utc>,
    pub route: String,
    pub safe_summary: String,
    pub retention_expires_at: Option<DateTime<Utc>>,
    pub redaction_status: Option<RedactionStatus>,
}

/// Go: `BuildRuntimeProjection` — always stores a redaction-safe summary.
pub fn build_runtime_projection(input: &RuntimeProjectionInput) -> RuntimeProjection {
    RuntimeProjection {
        runtime_projection_id: input.projection_id.clone(),
        thread_id: input.thread_id.clone(),
        tenant_id: input.tenant_id.clone(),
        session_segment_id: input.session_segment_id.clone(),
        resource_kind: input.resource_kind,
        resource_id: input.resource_id.clone(),
        status: input.status.clone(),
        reason_code: input.reason_code.clone(),
        occurred_at: input.occurred_at,
        route: input.route.clone(),
        safe_summary: safe_summary(&input.safe_summary, true).text,
        retention_expires_at: input.retention_expires_at,
        redaction_status: input.redaction_status.unwrap_or(RedactionStatus::Redacted),
    }
}

/// Go: `AvailableActions` — archived threads may only be reopened.
pub fn available_actions(state: LifecycleState) -> Vec<LifecycleActionKind> {
    match state {
        LifecycleState::Archived => vec![LifecycleActionKind::Reopen],
        _ => vec![LifecycleActionKind::Reset, LifecycleActionKind::Archive],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::Duration;
    use chrono::TimeZone;

    // Port of TestBuildThreadResourceIsMetadataOnly.
    #[test]
    fn build_thread_resource_is_metadata_only() {
        let now = Utc.with_ymd_and_hms(2026, 5, 11, 12, 0, 0).unwrap();
        let resource = build_thread_resource(
            &Thread {
                thread_id: "thr_1".to_string(),
                tenant_id: "ten_1".to_string(),
                lifecycle_state: LifecycleState::Active,
                current_session_segment_id: "seg_1".to_string(),
                source_kind: SourceKind::Channel,
                source_summary: "Slack Main / #support".to_string(),
                last_activity_at: now,
                created_at: now,
                updated_at: now,
                retention_expires_at: Some(now + Duration::days(90)),
                redaction_status: RedactionStatus::Redacted,
            },
            "sess_1",
        );
        assert_eq!(resource.thread_id, "thr_1");
        assert_eq!(resource.current_session_id, "sess_1");
        assert_eq!(
            resource.available_actions,
            vec![LifecycleActionKind::Reset, LifecycleActionKind::Archive]
        );
    }

    // Port of TestBuildRuntimeProjectionSupportsOperatorTraceResourceKinds.
    #[test]
    fn build_runtime_projection_supports_operator_trace_resource_kinds() {
        let now = Utc.with_ymd_and_hms(2026, 5, 11, 12, 0, 0).unwrap();
        let kinds = [
            RuntimeResourceKind::Session,
            RuntimeResourceKind::Run,
            RuntimeResourceKind::Workflow,
            RuntimeResourceKind::Approval,
            RuntimeResourceKind::ForegroundReply,
            RuntimeResourceKind::BackgroundDelivery,
            RuntimeResourceKind::ConnectorMessage,
        ];
        for kind in kinds {
            let kind_str = serde_json::to_string(&kind)
                .unwrap()
                .trim_matches('"')
                .to_string();
            let projection = build_runtime_projection(&RuntimeProjectionInput {
                projection_id: format!("rtp_{kind_str}"),
                thread_id: "thr_1".to_string(),
                tenant_id: "ten_1".to_string(),
                session_segment_id: "seg_1".to_string(),
                resource_kind: kind,
                resource_id: format!("res_{kind_str}"),
                status: "completed".to_string(),
                reason_code: "accepted".to_string(),
                occurred_at: now,
                route: format!("/trace/{kind_str}"),
                safe_summary: format!("metadata summary for {kind_str}"),
                retention_expires_at: None,
                redaction_status: None,
            });
            assert_eq!(projection.resource_kind, kind);
            assert!(!projection.runtime_projection_id.is_empty());
            assert!(!projection.resource_id.is_empty());
            assert!(!projection.safe_summary.is_empty());
            assert_ne!(projection.safe_summary, "suppressed");
            assert_eq!(projection.redaction_status, RedactionStatus::Redacted);
        }
    }

    // Port of TestRuntimeProjectionRejectsMemoryBehaviorFields.
    #[test]
    fn runtime_projection_rejects_memory_behavior_fields() {
        let projection = build_runtime_projection(&RuntimeProjectionInput {
            projection_id: "rtp_no_memory".to_string(),
            thread_id: "thr_1".to_string(),
            tenant_id: "ten_1".to_string(),
            session_segment_id: String::new(),
            resource_kind: RuntimeResourceKind::Run,
            resource_id: "run_1".to_string(),
            status: "completed".to_string(),
            reason_code: String::new(),
            occurred_at: Utc.with_ymd_and_hms(2026, 5, 11, 12, 0, 0).unwrap(),
            route: String::new(),
            safe_summary: "metadata only".to_string(),
            retention_expires_at: None,
            redaction_status: None,
        });
        let raw = serde_json::to_string(&projection).expect("marshal projection");
        for forbidden in [
            "semanticSummary",
            "recalledMemory",
            "contextPacking",
            "autonomousPruning",
        ] {
            assert!(
                !raw.contains(forbidden),
                "projection leaked {forbidden} in {raw}"
            );
        }
    }
}
