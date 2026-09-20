use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;

use crate::error::ThreadsError;
use crate::redaction::RedactionStatus;
use crate::source::SourceKind;
use crate::utc_now_or;

/// Thread lifecycle states. Go: `LifecycleState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleState {
    Active,
    Reset,
    Archived,
    Reopened,
}

/// Lifecycle mutation kinds. Go: `LifecycleActionKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleActionKind {
    Reset,
    Archive,
    Reopen,
}

/// Core thread record. Go: `Thread` (no JSON tags upstream; camelCase here
/// per rs/MIGRATION.md).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    pub thread_id: String,
    pub tenant_id: String,
    pub lifecycle_state: LifecycleState,
    pub current_session_segment_id: String,
    pub source_kind: SourceKind,
    pub source_summary: String,
    pub last_activity_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_expires_at: Option<DateTime<Utc>>,
    pub redaction_status: RedactionStatus,
}

/// A session segment within a thread; reset creates a new generation.
/// Go: `SessionSegment`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSegment {
    pub session_segment_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    pub generation: i32,
    pub state: String,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    pub last_active_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reset_from_session_segment_id: String,
    pub partial_evidence: bool,
}

/// Audit record of an applied lifecycle mutation. Go: `LifecycleAction`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleAction {
    pub lifecycle_action_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub action_kind: LifecycleActionKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub actor_principal_id: String,
    pub prior_state: LifecycleState,
    pub resulting_state: LifecycleState,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prior_session_segment_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub resulting_session_segment_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
    pub requested_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
    pub status: String,
    pub audit_event_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_expires_at: Option<DateTime<Utc>>,
    pub redaction_status: RedactionStatus,
}

/// Go: `LifecycleAuditRecord`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleAuditRecord {
    pub audit_event_id: String,
    pub tenant_id: String,
    pub thread_id: String,
    pub principal_id: String,
    pub action: String,
    pub permission_gate: String,
    pub outcome: String,
    pub reason_code: String,
    pub created_at: DateTime<Utc>,
    pub redaction_status: RedactionStatus,
}

/// Go: `LegacySessionEvidence`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacySessionEvidence {
    pub legacy_evidence_id: String,
    pub tenant_id: String,
    pub thread_id: String,
    pub session_id: String,
    pub routing_key: String,
    pub channel: String,
    pub account_id: String,
    pub peer_id: String,
    pub thread_id_from_session: String,
    pub projection_status: String,
    #[serde(default)]
    pub missing_fields: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub redaction_status: RedactionStatus,
}

/// Inputs every lifecycle mutation requires. Go: `LifecycleMutationInput`.
///
/// `now: None` maps to Go's zero `time.Time` (callers substitute the current
/// time); tests always pass an explicit instant.
#[derive(Debug, Clone)]
pub struct LifecycleMutationInput {
    pub actor_principal_id: String,
    pub reason_code: String,
    pub audit_event_id: String,
    pub now: Option<DateTime<Utc>>,
    pub new_segment_id: String,
}

/// Go: `ResetThread`. Allowed from active/reopened/reset; creates the next
/// session segment and records the reset lifecycle action.
pub fn reset_thread(
    thread: &Thread,
    input: &LifecycleMutationInput,
) -> Result<(Thread, LifecycleAction, SessionSegment), ThreadsError> {
    require_audit(input)?;
    if !matches!(
        thread.lifecycle_state,
        LifecycleState::Active | LifecycleState::Reopened | LifecycleState::Reset
    ) {
        return Err(ThreadsError::LifecycleTransitionNotAllowed);
    }
    let now = utc_now_or(input.now);
    let segment_id = if input.new_segment_id.is_empty() {
        format!("{}_reset", thread.current_session_segment_id)
    } else {
        input.new_segment_id.clone()
    };
    let mut updated = thread.clone();
    updated.lifecycle_state = LifecycleState::Reset;
    updated.current_session_segment_id = segment_id.clone();
    updated.updated_at = now;
    updated.last_activity_at = now;
    let mut action = lifecycle_action(
        thread,
        input,
        LifecycleActionKind::Reset,
        LifecycleState::Reset,
        now,
    );
    action.resulting_session_segment_id = segment_id.clone();
    let segment = SessionSegment {
        session_segment_id: segment_id,
        thread_id: thread.thread_id.clone(),
        tenant_id: thread.tenant_id.clone(),
        session_id: String::new(),
        generation: 1,
        state: "active".to_string(),
        started_at: now,
        ended_at: None,
        last_active_at: now,
        reset_from_session_segment_id: thread.current_session_segment_id.clone(),
        partial_evidence: false,
    };
    Ok((updated, action, segment))
}

/// Go: `ArchiveThread`. Archiving an already-archived thread is rejected.
pub fn archive_thread(
    thread: &Thread,
    input: &LifecycleMutationInput,
) -> Result<(Thread, LifecycleAction), ThreadsError> {
    require_audit(input)?;
    if thread.lifecycle_state == LifecycleState::Archived {
        return Err(ThreadsError::LifecycleTransitionNotAllowed);
    }
    let now = utc_now_or(input.now);
    let mut updated = thread.clone();
    updated.lifecycle_state = LifecycleState::Archived;
    updated.updated_at = now;
    updated.last_activity_at = now;
    let action = lifecycle_action(
        thread,
        input,
        LifecycleActionKind::Archive,
        LifecycleState::Archived,
        now,
    );
    Ok((updated, action))
}

/// Go: `ReopenThread`. Only archived threads can be reopened.
pub fn reopen_thread(
    thread: &Thread,
    input: &LifecycleMutationInput,
) -> Result<(Thread, LifecycleAction), ThreadsError> {
    require_audit(input)?;
    if thread.lifecycle_state != LifecycleState::Archived {
        return Err(ThreadsError::LifecycleTransitionNotAllowed);
    }
    let now = utc_now_or(input.now);
    let mut updated = thread.clone();
    updated.lifecycle_state = LifecycleState::Reopened;
    updated.updated_at = now;
    updated.last_activity_at = now;
    let action = lifecycle_action(
        thread,
        input,
        LifecycleActionKind::Reopen,
        LifecycleState::Reopened,
        now,
    );
    Ok((updated, action))
}

fn lifecycle_action(
    thread: &Thread,
    input: &LifecycleMutationInput,
    kind: LifecycleActionKind,
    resulting: LifecycleState,
    now: DateTime<Utc>,
) -> LifecycleAction {
    LifecycleAction {
        lifecycle_action_id: String::new(),
        thread_id: thread.thread_id.clone(),
        tenant_id: thread.tenant_id.clone(),
        action_kind: kind,
        actor_principal_id: input.actor_principal_id.clone(),
        prior_state: thread.lifecycle_state,
        resulting_state: resulting,
        prior_session_segment_id: thread.current_session_segment_id.clone(),
        resulting_session_segment_id: String::new(),
        reason_code: input.reason_code.clone(),
        requested_at: now,
        completed_at: now,
        status: "succeeded".to_string(),
        audit_event_id: input.audit_event_id.clone(),
        retention_expires_at: None,
        redaction_status: RedactionStatus::Redacted,
    }
}

fn require_audit(input: &LifecycleMutationInput) -> Result<(), ThreadsError> {
    if input.audit_event_id.is_empty() {
        return Err(ThreadsError::AuditEvidenceRequired);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::TimeZone;

    fn active_thread() -> Thread {
        Thread {
            thread_id: "thr_1".to_string(),
            tenant_id: "ten_1".to_string(),
            lifecycle_state: LifecycleState::Active,
            current_session_segment_id: "seg_old".to_string(),
            source_kind: SourceKind::Chat,
            source_summary: String::new(),
            last_activity_at: Utc::now(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            retention_expires_at: None,
            redaction_status: RedactionStatus::Redacted,
        }
    }

    // Port of TestLifecycleResetPreservesThreadAndCreatesNewSegment.
    #[test]
    fn reset_preserves_thread_and_creates_new_segment() {
        let now = Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap();
        let thread = active_thread();
        let (updated, action, segment) = reset_thread(
            &thread,
            &LifecycleMutationInput {
                actor_principal_id: "prn_1".to_string(),
                reason_code: "user_requested_reset".to_string(),
                audit_event_id: "audit_1".to_string(),
                now: Some(now),
                new_segment_id: "seg_new".to_string(),
            },
        )
        .expect("reset_thread");
        assert_eq!(updated.thread_id, thread.thread_id);
        assert_eq!(updated.lifecycle_state, LifecycleState::Reset);
        assert_eq!(updated.current_session_segment_id, "seg_new");
        assert_eq!(action.prior_state, LifecycleState::Active);
        assert_eq!(action.resulting_state, LifecycleState::Reset);
        assert_eq!(action.audit_event_id, "audit_1");
        assert_eq!(segment.thread_id, thread.thread_id);
        assert_eq!(segment.session_segment_id, "seg_new");
        assert_eq!(segment.reset_from_session_segment_id, "seg_old");
        assert_eq!(segment.started_at, now);
    }

    // Port of TestLifecycleArchiveAndReopenRules.
    #[test]
    fn archive_and_reopen_rules() {
        let now = Utc.with_ymd_and_hms(2026, 5, 11, 11, 0, 0).unwrap();
        let thread = Thread {
            current_session_segment_id: "seg_1".to_string(),
            ..active_thread()
        };
        let (archived, action) = archive_thread(
            &thread,
            &LifecycleMutationInput {
                actor_principal_id: "prn_1".to_string(),
                reason_code: String::new(),
                audit_event_id: "audit_archive".to_string(),
                now: Some(now),
                new_segment_id: String::new(),
            },
        )
        .expect("archive_thread");
        assert_eq!(archived.lifecycle_state, LifecycleState::Archived);
        assert_eq!(action.resulting_state, LifecycleState::Archived);

        let (reopened, reopen_action) = reopen_thread(
            &archived,
            &LifecycleMutationInput {
                actor_principal_id: "prn_1".to_string(),
                reason_code: String::new(),
                audit_event_id: "audit_reopen".to_string(),
                now: Some(now + chrono::Duration::minutes(1)),
                new_segment_id: String::new(),
            },
        )
        .expect("reopen_thread");
        assert_eq!(reopened.lifecycle_state, LifecycleState::Reopened);
        assert_eq!(reopen_action.prior_state, LifecycleState::Archived);

        // Mutations without audit evidence fail closed.
        let result = archive_thread(
            &reopened,
            &LifecycleMutationInput {
                actor_principal_id: "prn_1".to_string(),
                reason_code: String::new(),
                audit_event_id: String::new(),
                now: None,
                new_segment_id: String::new(),
            },
        );
        assert_eq!(result.unwrap_err(), ThreadsError::AuditEvidenceRequired);
    }
}
