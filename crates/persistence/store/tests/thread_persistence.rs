//! Round-trip integration tests for the thread-persistence DAOs ported from
//! `daemon/internal/store/thread_lifecycle.go` and `thread_group_room.go` into
//! `thread_persistence.rs`. Ports of TestThreadLifecycleMigrationAndTenantSafePersistence,
//! TestThreadLifecycleTenantRetentionPolicyOverride,
//! TestThreadLifecycleListOrderingDetailAndLegacyProjection (ordering slice),
//! TestThreadLifecycleRetentionFiltersExpiredEvidenceAndHonorsTenantOverride, and
//! TestGroupRoomMigrationAndEvidencePersistence.

use chrono::{DateTime, Duration, TimeZone, Utc};
use kura_store::{SQLiteStore, ThreadListQuery};
use kura_threads::{
    AllowlistStatus, ConversationShape, ConversationShapeEvidence,
    GROUP_ROOM_REASON_ACCEPTED_QUALIFYING_MENTION, GROUP_ROOM_REASON_DUPLICATE_SOURCE_EVENT,
    GROUP_ROOM_REASON_SCOPED_RESET_SUCCEEDED, LifecycleState, MentionStatus, ParticipationDecision,
    ParticipationDecisionValue, RedactionStatus, ResetEvent, ResetEventStatus, RoutingOutcome,
    RuntimeProjectionInput, RuntimeResourceKind, SessionSegment, ShapeEvidenceStatus,
    SourceContinuationKey, SourceKind, SourceLinkage, Thread, build_runtime_projection,
};

fn temp_dir(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("kura_store_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.to_string_lossy().to_string()
}

fn thread_fixture(thread_id: &str, tenant_id: &str, now: DateTime<Utc>) -> Thread {
    Thread {
        thread_id: thread_id.to_string(),
        tenant_id: tenant_id.to_string(),
        lifecycle_state: LifecycleState::Active,
        current_session_segment_id: format!("seg_{thread_id}"),
        source_kind: SourceKind::Channel,
        source_summary: "Slack Main / #support".to_string(),
        last_activity_at: now,
        created_at: now,
        updated_at: now,
        retention_expires_at: Some(now + Duration::days(90)),
        redaction_status: RedactionStatus::Redacted,
    }
}

fn segment_fixture(thread: &Thread, session_id: &str, now: DateTime<Utc>) -> SessionSegment {
    SessionSegment {
        session_segment_id: thread.current_session_segment_id.clone(),
        thread_id: thread.thread_id.clone(),
        tenant_id: thread.tenant_id.clone(),
        session_id: session_id.to_string(),
        generation: 1,
        state: "active".to_string(),
        started_at: now,
        ended_at: None,
        last_active_at: thread.last_activity_at,
        reset_from_session_segment_id: String::new(),
        partial_evidence: false,
    }
}

fn linkage_fixture(
    id: &str,
    thread: &Thread,
    source: &SourceContinuationKey,
    outcome: RoutingOutcome,
    current: bool,
    now: DateTime<Utc>,
    retention: Option<DateTime<Utc>>,
) -> SourceLinkage {
    SourceLinkage {
        source_linkage_id: id.to_string(),
        thread_id: thread.thread_id.clone(),
        tenant_id: thread.tenant_id.clone(),
        source_kind: SourceKind::Channel,
        connector_id: source.connector_id.clone(),
        connector_kind: "slack".to_string(),
        source_account_id: source.source_account_id.clone(),
        source_conversation_id: source.source_conversation_id.clone(),
        source_message_id: String::new(),
        routing_outcome: outcome,
        current,
        linked_at: Some(now),
        retention_expires_at: retention,
        redaction_status: RedactionStatus::Redacted,
    }
}

#[test]
fn thread_round_trips_and_tenant_filtering() {
    let dir = temp_dir("thread_rt");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap();

    let mut thread = thread_fixture("thr_1", "ten_1", now);
    store.upsert_thread(&thread).unwrap();
    // Upsert again through the ON CONFLICT path with a changed field.
    thread.lifecycle_state = LifecycleState::Archived;
    thread.updated_at = now + Duration::minutes(1);
    store.upsert_thread(&thread).unwrap();

    let got = store
        .get_thread_for_tenant("ten_1", "thr_1")
        .unwrap()
        .expect("found for tenant");
    assert_eq!(got.thread_id, "thr_1");
    assert_eq!(got.tenant_id, "ten_1");
    assert_eq!(got.lifecycle_state, LifecycleState::Archived);
    assert_eq!(got.source_kind, SourceKind::Channel);
    assert_eq!(got.source_summary, "Slack Main / #support");
    assert_eq!(got.current_session_segment_id, "seg_thr_1");
    assert_eq!(got.redaction_status, RedactionStatus::Redacted);
    assert_eq!(got.retention_expires_at, Some(now + Duration::days(90)));

    // Cross-tenant lookup must not find the thread.
    assert!(
        store
            .get_thread_for_tenant("ten_2", "thr_1")
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .get_thread_for_tenant("ten_1", "thr_missing")
            .unwrap()
            .is_none()
    );
}

#[test]
fn retention_policy_override_affects_expiry() {
    let dir = temp_dir("thread_retention_policy");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap();

    let default_expiry = store.thread_retention_expiry("ten_default", now).unwrap();
    assert_eq!(default_expiry, now + Duration::days(90));

    let longer = now + Duration::days(180);
    store
        .set_thread_retention_policy("ten_long", longer)
        .unwrap();
    assert_eq!(
        store.thread_retention_expiry("ten_long", now).unwrap(),
        longer
    );
    // A policy shorter than the default horizon is ignored.
    store
        .set_thread_retention_policy("ten_short", now + Duration::days(30))
        .unwrap();
    assert_eq!(
        store.thread_retention_expiry("ten_short", now).unwrap(),
        now + Duration::days(90)
    );
}

#[test]
fn thread_list_ordering_puts_archived_last() {
    let dir = temp_dir("thread_list");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap();

    let fixtures = [
        Thread {
            thread_id: "thr_archived_newer".to_string(),
            lifecycle_state: LifecycleState::Archived,
            source_kind: SourceKind::Workflow,
            source_summary: "workflow".to_string(),
            last_activity_at: now + Duration::minutes(3),
            updated_at: now + Duration::minutes(3),
            ..thread_fixture("thr_archived_newer", "ten_1", now)
        },
        Thread {
            thread_id: "thr_reopened".to_string(),
            lifecycle_state: LifecycleState::Reopened,
            source_kind: SourceKind::Channel,
            source_summary: "Slack / #ops".to_string(),
            last_activity_at: now + Duration::minutes(2),
            updated_at: now + Duration::minutes(2),
            ..thread_fixture("thr_reopened", "ten_1", now)
        },
        Thread {
            thread_id: "thr_reset".to_string(),
            lifecycle_state: LifecycleState::Reset,
            source_kind: SourceKind::Chat,
            source_summary: "chat".to_string(),
            last_activity_at: now + Duration::minutes(1),
            updated_at: now + Duration::minutes(1),
            ..thread_fixture("thr_reset", "ten_1", now)
        },
    ];
    for thread in &fixtures {
        store.upsert_thread(thread).unwrap();
        store
            .upsert_thread_session_segment(&segment_fixture(
                thread,
                &format!("sess_{}", thread.thread_id),
                now,
            ))
            .unwrap();
    }

    let list = store
        .list_threads_for_tenant(&ThreadListQuery {
            tenant_id: "ten_1".to_string(),
            limit: 10,
            ..ThreadListQuery::default()
        })
        .unwrap();
    let order: Vec<String> = list
        .items
        .iter()
        .map(|item| item.thread_id.clone())
        .collect();
    assert_eq!(order, ["thr_reopened", "thr_reset", "thr_archived_newer"]);
    assert_eq!(list.page.limit, 10);
    assert_eq!(list.page.order, "active_recent_archived_id");
    assert!(list.page.next_cursor.is_empty());

    // State filter narrows the result set.
    let archived_only = store
        .list_threads_for_tenant(&ThreadListQuery {
            tenant_id: "ten_1".to_string(),
            limit: 10,
            state_filter: "archived".to_string(),
            ..ThreadListQuery::default()
        })
        .unwrap();
    assert_eq!(archived_only.items.len(), 1);
    assert_eq!(archived_only.items[0].thread_id, "thr_archived_newer");

    // Segments are persisted per thread in generation order.
    let segments = store
        .list_thread_session_segments("ten_1", "thr_reopened")
        .unwrap();
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].session_id, "sess_thr_reopened");
}

#[test]
fn source_linkage_current_flag_switches_current_thread() {
    let dir = temp_dir("thread_source_current");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap();
    // Saved linkage values are normalized (the router saves the canonical key);
    // the lookup key below is deliberately unnormalized to exercise query-side
    // normalization (trim + lowercase) in get_current_thread_for_source.
    let source = SourceContinuationKey {
        tenant_id: "ten_1".to_string(),
        connector_id: "slack-main".to_string(),
        source_account_id: "workspace_a".to_string(),
        source_conversation_id: "channel_a".to_string(),
    };

    let thread_a = thread_fixture("thr_a", "ten_1", now);
    let thread_b = thread_fixture("thr_b", "ten_1", now);
    store.upsert_thread(&thread_a).unwrap();
    store.upsert_thread(&thread_b).unwrap();

    let retention = Some(now + Duration::days(90));
    store
        .save_thread_source_linkage(&linkage_fixture(
            "src_a",
            &thread_a,
            &source,
            RoutingOutcome::Accepted,
            true,
            now,
            retention,
        ))
        .unwrap();
    store
        .save_thread_source_linkage(&linkage_fixture(
            "src_b",
            &thread_b,
            &source,
            RoutingOutcome::Accepted,
            true,
            now,
            retention,
        ))
        .unwrap();

    // The latest current linkage wins; the key parts are trimmed + lowercased.
    let query_key = SourceContinuationKey {
        tenant_id: " ten_1 ".to_string(),
        connector_id: "Slack-Main".to_string(),
        source_account_id: "Workspace_A".to_string(),
        source_conversation_id: "Channel_A".to_string(),
    };
    let current = store
        .get_current_thread_for_source(&query_key)
        .unwrap()
        .expect("current thread found");
    assert_eq!(current.thread_id, "thr_b");
    assert_eq!(current.tenant_id, "ten_1");

    // A key with any missing part is rejected (normalization fails closed).
    let incomplete = SourceContinuationKey {
        tenant_id: "ten_1".to_string(),
        connector_id: String::new(),
        source_account_id: String::new(),
        source_conversation_id: String::new(),
    };
    assert!(store.get_current_thread_for_source(&incomplete).is_err());

    // No linkage for a different source resolves to None.
    let other = SourceContinuationKey {
        tenant_id: "ten_1".to_string(),
        connector_id: "matrix-main".to_string(),
        source_account_id: "hs_1".to_string(),
        source_conversation_id: "room_1".to_string(),
    };
    assert!(
        store
            .get_current_thread_for_source(&other)
            .unwrap()
            .is_none()
    );
}

#[test]
fn retention_filters_expired_evidence() {
    let dir = temp_dir("thread_retention_filter");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();
    let expired_at = Utc.with_ymd_and_hms(2000, 1, 1, 0, 0, 0).unwrap();
    let future = now + Duration::days(180);

    let thread = thread_fixture("thr_retention", "ten_retention", now);
    store.upsert_thread(&thread).unwrap();
    store
        .upsert_thread_session_segment(&segment_fixture(&thread, "sess_retention", now))
        .unwrap();

    let source = SourceContinuationKey {
        tenant_id: "ten_retention".to_string(),
        connector_id: "slack-main".to_string(),
        source_account_id: "acct_redacted".to_string(),
        source_conversation_id: "conv_redacted".to_string(),
    };
    for linkage in [
        linkage_fixture(
            "src_current",
            &thread,
            &source,
            RoutingOutcome::Accepted,
            true,
            now,
            Some(future),
        ),
        linkage_fixture(
            "src_expired",
            &thread,
            &source,
            RoutingOutcome::Accepted,
            false,
            expired_at,
            Some(expired_at),
        ),
        linkage_fixture(
            "src_retained",
            &thread,
            &source,
            RoutingOutcome::Duplicate,
            false,
            now,
            Some(future),
        ),
    ] {
        store.save_thread_source_linkage(&linkage).unwrap();
    }

    let projection = |id: &str, occurred: DateTime<Utc>, retention: Option<DateTime<Utc>>| {
        build_runtime_projection(&RuntimeProjectionInput {
            projection_id: id.to_string(),
            thread_id: "thr_retention".to_string(),
            tenant_id: "ten_retention".to_string(),
            session_segment_id: "seg_thr_retention".to_string(),
            resource_kind: RuntimeResourceKind::Run,
            resource_id: format!("run_{id}"),
            status: "completed".to_string(),
            reason_code: "accepted".to_string(),
            occurred_at: occurred,
            route: String::new(),
            safe_summary: format!("summary for {id}"),
            retention_expires_at: retention,
            redaction_status: None,
        })
    };
    store
        .save_thread_runtime_projection(&projection("rtp_expired", expired_at, Some(expired_at)))
        .unwrap();
    store
        .save_thread_runtime_projection(&projection("rtp_retained", now, Some(future)))
        .unwrap();

    let linkages = store
        .list_thread_source_linkages("ten_retention", "thr_retention", now)
        .unwrap();
    let ids: Vec<String> = linkages
        .iter()
        .map(|l| l.source_linkage_id.clone())
        .collect();
    assert!(ids.contains(&"src_current".to_string()));
    assert!(ids.contains(&"src_retained".to_string()));
    assert!(!ids.contains(&"src_expired".to_string()));

    let projections = store
        .list_thread_runtime_projections("ten_retention", "thr_retention", now)
        .unwrap();
    assert_eq!(projections.len(), 1);
    assert_eq!(projections[0].runtime_projection_id, "rtp_retained");
}

#[test]
fn conversation_shape_and_participation_round_trip() {
    let dir = temp_dir("thread_group_room");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap();

    let thread = thread_fixture("thr_room", "ten_1", now);
    store.upsert_thread(&thread).unwrap();

    let shape = ConversationShapeEvidence {
        conversation_shape_id: "shape_1".to_string(),
        tenant_id: "ten_1".to_string(),
        thread_id: "thr_room".to_string(),
        session_segment_id: "seg_thr_room".to_string(),
        shape: ConversationShape::Room,
        source_kind: Some(SourceKind::Channel),
        connector_id: "slack-main".to_string(),
        connector_kind: "slack".to_string(),
        source_account_id: "workspace_redacted".to_string(),
        source_conversation_id: "channel_redacted".to_string(),
        source_conversation_summary: String::new(),
        participant_summary: String::new(),
        shape_evidence_status: ShapeEvidenceStatus::Proven,
        recorded_at: Some(now),
        updated_at: Some(now),
        retention_expires_at: Some(now + Duration::days(90)),
        redaction_status: RedactionStatus::Redacted,
    };
    store.save_conversation_shape_evidence(&shape).unwrap();
    // Upsert again through the ON CONFLICT path with a changed summary.
    let mut updated_shape = shape.clone();
    updated_shape.source_conversation_summary = "Slack / #support".to_string();
    updated_shape.updated_at = Some(now + Duration::minutes(1));
    store
        .save_conversation_shape_evidence(&updated_shape)
        .unwrap();

    let got_shape = store
        .get_conversation_shape_for_thread("ten_1", "thr_room")
        .unwrap()
        .expect("shape found");
    assert_eq!(got_shape.shape, ConversationShape::Room);
    assert_eq!(got_shape.shape_evidence_status, ShapeEvidenceStatus::Proven);
    assert_eq!(got_shape.source_conversation_summary, "Slack / #support");

    let mut decision = ParticipationDecision {
        participation_decision_id: "part_1".to_string(),
        tenant_id: "ten_1".to_string(),
        thread_id: "thr_room".to_string(),
        session_segment_id: "seg_thr_room".to_string(),
        connector_id: "slack-main".to_string(),
        connector_kind: "slack".to_string(),
        source_account_id: "workspace_redacted".to_string(),
        source_conversation_id: "channel_redacted".to_string(),
        source_message_id: "msg_1".to_string(),
        conversation_shape: ConversationShape::Room,
        policy_id: String::new(),
        mention_status: MentionStatus::Qualified,
        allowlist_status: AllowlistStatus::Eligible,
        decision: ParticipationDecisionValue::Accepted,
        reason_code: GROUP_ROOM_REASON_ACCEPTED_QUALIFYING_MENTION.to_string(),
        created_assistant_work: true,
        occurred_at: Some(now),
        retention_expires_at: Some(now + Duration::days(90)),
        redaction_status: RedactionStatus::Redacted,
        safe_summary: String::new(),
    };
    store.save_participation_decision(&decision).unwrap();
    // Upsert again through the ON CONFLICT path with a changed status.
    decision.decision = ParticipationDecisionValue::Duplicate;
    decision.reason_code = GROUP_ROOM_REASON_DUPLICATE_SOURCE_EVENT.to_string();
    store.save_participation_decision(&decision).unwrap();

    // The unique source-message identity dedups to the original row.
    let by_message = store
        .get_participation_decision_by_source_message(
            "ten_1",
            "slack-main",
            "workspace_redacted",
            "channel_redacted",
            "msg_1",
        )
        .unwrap()
        .expect("decision found by source message");
    assert_eq!(by_message.participation_decision_id, "part_1");
    assert_eq!(by_message.decision, ParticipationDecisionValue::Accepted);
    assert!(by_message.created_assistant_work);

    let decisions = store
        .list_participation_decisions_for_thread("ten_1", "thr_room", 10)
        .unwrap();
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].decision, ParticipationDecisionValue::Accepted);

    // A different source message creates a second row.
    store
        .save_participation_decision(&ParticipationDecision {
            participation_decision_id: "part_2".to_string(),
            source_message_id: "msg_2".to_string(),
            decision: ParticipationDecisionValue::Ignored,
            ..decision.clone()
        })
        .unwrap();
    assert_eq!(
        store
            .list_participation_decisions_for_thread("ten_1", "thr_room", 10)
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn reset_event_round_trips() {
    let dir = temp_dir("thread_reset_event");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap();

    let thread = thread_fixture("thr_reset_evt", "ten_1", now);
    store.upsert_thread(&thread).unwrap();

    let event = ResetEvent {
        reset_event_id: "reset_1".to_string(),
        tenant_id: "ten_1".to_string(),
        thread_id: "thr_reset_evt".to_string(),
        conversation_shape: ConversationShape::Room,
        source_conversation_id: "channel_redacted".to_string(),
        actor_principal_id: "prn_1".to_string(),
        permission_gate: "connectors.manage".to_string(),
        prior_session_segment_id: "seg_thr_reset_evt".to_string(),
        resulting_session_segment_id: "seg_reset".to_string(),
        status: ResetEventStatus::Succeeded,
        reason_code: GROUP_ROOM_REASON_SCOPED_RESET_SUCCEEDED.to_string(),
        requested_at: Some(now),
        completed_at: Some(now),
        audit_event_id: "audit_reset".to_string(),
        retention_expires_at: Some(now + Duration::days(90)),
        redaction_status: RedactionStatus::Redacted,
    };
    store.save_reset_event(&event).unwrap();
    // Upsert again through the ON CONFLICT path with a changed reason.
    let mut updated = event.clone();
    updated.reason_code = "operator_reset".to_string();
    updated.completed_at = Some(now + Duration::minutes(1));
    store.save_reset_event(&updated).unwrap();

    let events = store
        .list_reset_events_for_thread("ten_1", "thr_reset_evt", 10)
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].reset_event_id, "reset_1");
    assert_eq!(events[0].conversation_shape, ConversationShape::Room);
    assert_eq!(events[0].source_conversation_id, "channel_redacted");
    assert_eq!(events[0].reason_code, "operator_reset");
}
