//! Thread lifecycle / source-linkage / runtime-projection / retention /
//! redaction / restart-recovery events (port of `thread_lifecycle.go`).

use chrono::{DateTime, SecondsFormat, Utc};

use crate::util::{is_go_zero_time, now_utc, payload};
use crate::wire;
use crate::{Event, Resource, Scope};
use kura_threads::{
    LifecycleAction, LifecycleActionKind, RedactionStatus, RuntimeProjection, SourceLinkage,
};

pub const THREAD_LIFECYCLE_RESET_NAME: &str = "thread.lifecycle_reset";
pub const THREAD_LIFECYCLE_ARCHIVED_NAME: &str = "thread.lifecycle_archived";
pub const THREAD_LIFECYCLE_REOPENED_NAME: &str = "thread.lifecycle_reopened";
pub const THREAD_SOURCE_LINKED_NAME: &str = "thread.source_linked";
pub const THREAD_RUNTIME_PROJECTION_NAME: &str = "thread.runtime_projection_recorded";
pub const THREAD_RETENTION_APPLIED_NAME: &str = "thread.retention_applied";
pub const THREAD_REDACTION_FAILED_NAME: &str = "thread.redaction_failed";
pub const THREAD_AUDIT_FAILED_CLOSED_NAME: &str = "thread.audit_failed_closed";
pub const THREAD_RESTART_RECOVERED_NAME: &str = "thread.restart_recovered";

/// Go: `ThreadLifecycleEvent` — the event name is derived from the action kind.
#[must_use]
pub fn thread_lifecycle_event(action: LifecycleAction) -> Event {
    let occurred_at = if is_go_zero_time(action.completed_at) {
        now_utc()
    } else {
        action.completed_at
    };
    Event {
        tenant_id: action.tenant_id.clone(),
        category: "thread".to_string(),
        name: lifecycle_event_name(action.action_kind).to_string(),
        occurred_at,
        scope: Scope {
            session_id: action.resulting_session_segment_id.clone(),
            ..Scope::default()
        },
        resource: Resource {
            kind: "thread".to_string(),
            id: action.thread_id.clone(),
        },
        payload: payload![
            "tenantId" => action.tenant_id,
            "threadId" => action.thread_id,
            "sessionSegmentId" => action.resulting_session_segment_id,
            "action" => wire::lifecycle_action_kind(&action.action_kind),
            "outcome" => action.status,
            "auditEventId" => action.audit_event_id,
            "reasonCode" => action.reason_code,
            "redactionStatus" => wire::redaction_status(&action.redaction_status),
        ],
        ..Event::default()
    }
}

/// Go: `ThreadRestartRecoveryEvent` — daemon-restart recovery summary.
#[must_use]
pub fn thread_restart_recovery_event(
    tenant_id: &str,
    checked_threads: i64,
    projected_legacy_sessions: i64,
    partial_thread_states: i64,
) -> Event {
    Event {
        tenant_id: tenant_id.to_string(),
        category: "thread".to_string(),
        name: THREAD_RESTART_RECOVERED_NAME.to_string(),
        occurred_at: now_utc(),
        resource: Resource {
            kind: "thread_lifecycle_recovery".to_string(),
            id: tenant_id.to_string(),
        },
        payload: payload![
            "tenantId" => tenant_id,
            "checkedThreads" => checked_threads,
            "projectedLegacySessions" => projected_legacy_sessions,
            "partialThreadStates" => partial_thread_states,
            "outcome" => "recovered",
            "ambiguousStatePolicy" => "metadata_only_partial_evidence",
            "redactionStatus" => wire::redaction_status(&RedactionStatus::Redacted),
            "semanticMemoryInteraction" => "none",
        ],
        ..Event::default()
    }
}

/// Go: `ThreadSourceLinkedEvent`.
#[must_use]
pub fn thread_source_linked_event(link: SourceLinkage) -> Event {
    let occurred_at = link.linked_at.unwrap_or_else(now_utc);
    Event {
        tenant_id: link.tenant_id.clone(),
        category: "thread".to_string(),
        name: THREAD_SOURCE_LINKED_NAME.to_string(),
        occurred_at,
        resource: Resource {
            kind: "thread_source_linkage".to_string(),
            id: link.source_linkage_id.clone(),
        },
        payload: payload![
            "tenantId" => link.tenant_id,
            "threadId" => link.thread_id,
            "sourceLinkageId" => link.source_linkage_id,
            "routingOutcome" => wire::routing_outcome(&link.routing_outcome),
            "redactionStatus" => wire::redaction_status(&link.redaction_status),
        ],
        ..Event::default()
    }
}

/// Go: `ThreadRuntimeProjectionEvent` — metadata-only trace of a runtime
/// resource attached to a thread/segment.
#[must_use]
pub fn thread_runtime_projection_event(projection: RuntimeProjection) -> Event {
    let occurred_at = if is_go_zero_time(projection.occurred_at) {
        now_utc()
    } else {
        projection.occurred_at
    };
    Event {
        tenant_id: projection.tenant_id.clone(),
        category: "thread".to_string(),
        name: THREAD_RUNTIME_PROJECTION_NAME.to_string(),
        occurred_at,
        scope: Scope {
            session_id: projection.session_segment_id.clone(),
            ..Scope::default()
        },
        resource: Resource {
            kind: "thread_runtime_projection".to_string(),
            id: projection.runtime_projection_id.clone(),
        },
        payload: payload![
            "tenantId" => projection.tenant_id,
            "threadId" => projection.thread_id,
            "sessionSegmentId" => projection.session_segment_id,
            "runtimeProjectionId" => projection.runtime_projection_id,
            "resourceKind" => wire::runtime_resource_kind(&projection.resource_kind),
            "resourceId" => projection.resource_id,
            "status" => projection.status,
            "redactionStatus" => wire::redaction_status(&projection.redaction_status),
        ],
        ..Event::default()
    }
}

/// Go: `ThreadRetentionAppliedEvent` — the expiry is rendered with
/// RFC3339Nano semantics (trailing fractional zeros trimmed), matching
/// `time.Time.Format(time.RFC3339Nano)`. The Rust `RedactionStatus` enum
/// has no zero value, so the Go `"" -> redacted` default is not needed.
#[must_use]
pub fn thread_retention_applied_event(
    tenant_id: &str,
    thread_id: &str,
    expires_at: DateTime<Utc>,
    status: RedactionStatus,
) -> Event {
    Event {
        tenant_id: tenant_id.to_string(),
        category: "thread".to_string(),
        name: THREAD_RETENTION_APPLIED_NAME.to_string(),
        occurred_at: now_utc(),
        resource: Resource {
            kind: "thread".to_string(),
            id: thread_id.to_string(),
        },
        payload: payload![
            "tenantId" => tenant_id,
            "threadId" => thread_id,
            "retentionExpiresAt" => expires_at.to_rfc3339_opts(SecondsFormat::AutoSi, true),
            "redactionStatus" => wire::redaction_status(&status),
        ],
        ..Event::default()
    }
}

/// Go: `ThreadRedactionFailedEvent`.
#[must_use]
pub fn thread_redaction_failed_event(tenant_id: &str, thread_id: &str, reason_code: &str) -> Event {
    thread_failure_event(
        THREAD_REDACTION_FAILED_NAME,
        tenant_id,
        thread_id,
        reason_code,
        "redaction_failed",
        RedactionStatus::RedactionFailed,
    )
}

/// Go: `ThreadAuditFailedClosedEvent`.
#[must_use]
pub fn thread_audit_failed_closed_event(
    tenant_id: &str,
    thread_id: &str,
    reason_code: &str,
) -> Event {
    thread_failure_event(
        THREAD_AUDIT_FAILED_CLOSED_NAME,
        tenant_id,
        thread_id,
        reason_code,
        "failed_closed",
        RedactionStatus::Redacted,
    )
}

/// Go: `lifecycleEventName`.
fn lifecycle_event_name(kind: LifecycleActionKind) -> &'static str {
    match kind {
        LifecycleActionKind::Archive => THREAD_LIFECYCLE_ARCHIVED_NAME,
        LifecycleActionKind::Reopen => THREAD_LIFECYCLE_REOPENED_NAME,
        LifecycleActionKind::Reset => THREAD_LIFECYCLE_RESET_NAME,
    }
}

/// Go: `threadFailureEvent`.
fn thread_failure_event(
    name: &str,
    tenant_id: &str,
    thread_id: &str,
    reason_code: &str,
    outcome: &str,
    status: RedactionStatus,
) -> Event {
    Event {
        tenant_id: tenant_id.to_string(),
        category: "thread".to_string(),
        name: name.to_string(),
        occurred_at: now_utc(),
        resource: Resource {
            kind: "thread".to_string(),
            id: thread_id.to_string(),
        },
        payload: payload![
            "tenantId" => tenant_id,
            "threadId" => thread_id,
            "outcome" => outcome,
            "reasonCode" => reason_code,
            "redactionStatus" => wire::redaction_status(&status),
        ],
        ..Event::default()
    }
}
