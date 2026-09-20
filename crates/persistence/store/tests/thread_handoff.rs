//! Behavioral tests for the thread-handoff DAOs (rs/store/src/thread_handoff.rs),
//! ported from daemon/internal/store/thread_handoff_test.go: save/get/list
//! round-trips, source-reference persistence, tenant isolation, and the
//! consume path that flips Referenced decisions to Consumed.

use chrono::{DateTime, Duration, TimeZone, Utc};
use kura_store::SQLiteStore;
use kura_threads::{
    ConversationShape, HandoffLink, HandoffSourceReference, HandoffSourceReferenceDecision,
    HandoffSourceReferenceEligibility, HandoffSourceReferenceStatus, HandoffStatus,
    RedactionStatus, SourceKind, Thread,
};

fn temp_dir(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("kura_store_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.to_string_lossy().to_string()
}

fn thread(thread_id: &str, tenant_id: &str, now: DateTime<Utc>) -> Thread {
    Thread {
        thread_id: thread_id.to_string(),
        tenant_id: tenant_id.to_string(),
        lifecycle_state: kura_threads::LifecycleState::Active,
        current_session_segment_id: format!("seg_{thread_id}"),
        source_kind: SourceKind::Channel,
        source_summary: "handoff test thread".to_string(),
        last_activity_at: now,
        created_at: now,
        updated_at: now,
        retention_expires_at: Some(now + Duration::days(90)),
        redaction_status: RedactionStatus::Redacted,
    }
}

fn link(tenant_id: &str, source: &str, destination: &str) -> HandoffLink {
    HandoffLink {
        handoff_link_id: String::new(),
        tenant_id: tenant_id.to_string(),
        source_thread_id: source.to_string(),
        source_session_segment_id: format!("seg_{source}"),
        destination_thread_id: destination.to_string(),
        destination_session_segment_id: format!("seg_{destination}"),
        source_conversation_shape: ConversationShape::Room,
        destination_conversation_shape: ConversationShape::DirectMessage,
        source_kind: Some(SourceKind::Channel),
        destination_kind: Some(SourceKind::Chat),
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

fn source_ref(
    handoff_link_id: &str,
    turn_id: &str,
    decision: HandoffSourceReferenceDecision,
    eligibility: HandoffSourceReferenceEligibility,
) -> HandoffSourceReference {
    HandoffSourceReference {
        handoff_source_reference_id: String::new(),
        handoff_link_id: handoff_link_id.to_string(),
        tenant_id: "ten_1".to_string(),
        source_thread_id: "thr_source".to_string(),
        source_session_segment_id: "seg_thr_source".to_string(),
        destination_thread_id: "thr_dest".to_string(),
        destination_session_segment_id: "seg_thr_dest".to_string(),
        continuity_turn_id: turn_id.to_string(),
        artifact_excerpt_ref: String::new(),
        eligibility_status: eligibility,
        decision,
        safe_summary: "safe summary".to_string(),
        redaction_status: RedactionStatus::Redacted,
        created_at: None,
        consumed_at: None,
        retention_expires_at: None,
    }
}

#[test]
fn handoff_link_save_get_list_round_trip() {
    let dir = temp_dir("handoff_link");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap();
    store
        .upsert_thread(&thread("thr_source", "ten_1", now))
        .unwrap();
    store
        .upsert_thread(&thread("thr_dest", "ten_1", now))
        .unwrap();

    let saved = store
        .save_handoff_link(link("ten_1", "thr_source", "thr_dest"))
        .unwrap();
    assert!(!saved.handoff_link_id.is_empty());
    assert_eq!(saved.permission_gate, "connectors.manage");
    assert_eq!(saved.status, HandoffStatus::Succeeded);
    assert!(saved.created_at.is_some());
    assert!(
        saved.retention_expires_at.is_some(),
        "default retention applied"
    );

    let got = store
        .get_handoff_link("ten_1", &saved.handoff_link_id)
        .unwrap()
        .expect("present");
    assert_eq!(got.handoff_link_id, saved.handoff_link_id);
    assert_eq!(got.source_thread_id, "thr_source");
    assert_eq!(got.destination_thread_id, "thr_dest");
    assert_eq!(got.source_conversation_shape, ConversationShape::Room);
    assert_eq!(
        got.destination_conversation_shape,
        ConversationShape::DirectMessage
    );
    assert_eq!(
        got.source_reference_status,
        HandoffSourceReferenceStatus::Available
    );

    // Listed from either side of the link, tenant-scoped.
    assert_eq!(
        store
            .list_handoff_links("ten_1", "thr_source", 20)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .list_handoff_links("ten_1", "thr_dest", 20)
            .unwrap()
            .len(),
        1
    );
    assert!(
        store
            .list_handoff_links("ten_2", "thr_source", 20)
            .unwrap()
            .is_empty()
    );

    // Missing link → None.
    assert!(
        store
            .get_handoff_link("ten_1", "handoff_missing")
            .unwrap()
            .is_none()
    );

    // Upsert via SaveHandoffLink with a fixed id flips status fields.
    let mut updated = got.clone();
    updated.status = HandoffStatus::FailedClosed;
    updated.reason_code = "destination_unavailable".to_string();
    let resaved = store.save_handoff_link(updated).unwrap();
    assert_eq!(resaved.status, HandoffStatus::FailedClosed);
    let after = store
        .get_handoff_link("ten_1", &saved.handoff_link_id)
        .unwrap()
        .unwrap();
    assert_eq!(after.reason_code, "destination_unavailable");
}

#[test]
fn handoff_source_references_save_and_consume() {
    let dir = temp_dir("handoff_refs");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap();
    store
        .upsert_thread(&thread("thr_source", "ten_1", now))
        .unwrap();
    store
        .upsert_thread(&thread("thr_dest", "ten_1", now))
        .unwrap();
    let saved = store
        .save_handoff_link(link("ten_1", "thr_source", "thr_dest"))
        .unwrap();

    let mut refs = vec![
        source_ref(
            &saved.handoff_link_id,
            "turn_1",
            HandoffSourceReferenceDecision::Referenced,
            HandoffSourceReferenceEligibility::Eligible,
        ),
        source_ref(
            &saved.handoff_link_id,
            "turn_2",
            HandoffSourceReferenceDecision::Excluded,
            HandoffSourceReferenceEligibility::RetentionExpired,
        ),
    ];
    store.save_handoff_source_references(&mut refs).unwrap();
    assert!(!refs[0].handoff_source_reference_id.is_empty());
    assert!(refs[0].created_at.is_some());
    assert!(refs[0].retention_expires_at.is_some());

    let listed = store
        .list_handoff_source_references_for_link("ten_1", &saved.handoff_link_id)
        .unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].continuity_turn_id, "turn_1");
    assert_eq!(listed[1].decision, HandoffSourceReferenceDecision::Excluded);

    // Consume: link becomes consumed; Referenced refs flip to Consumed.
    store
        .mark_handoff_source_references_consumed(
            "ten_1",
            &saved.handoff_link_id,
            "resp_1",
            Some(now),
        )
        .unwrap();
    let consumed_link = store
        .get_handoff_link("ten_1", &saved.handoff_link_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        consumed_link.source_reference_status,
        HandoffSourceReferenceStatus::Consumed
    );
    assert_eq!(consumed_link.first_destination_response_id, "resp_1");
    let consumed_refs = store
        .list_handoff_source_references_for_link("ten_1", &saved.handoff_link_id)
        .unwrap();
    assert_eq!(
        consumed_refs[0].decision,
        HandoffSourceReferenceDecision::Consumed
    );
    assert_eq!(
        consumed_refs[1].decision,
        HandoffSourceReferenceDecision::Excluded
    );
    assert!(consumed_refs[0].consumed_at.is_some());
}
