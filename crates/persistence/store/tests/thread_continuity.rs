//! Behavioral tests for the thread-continuity DAOs (rs/store/src/thread_continuity.rs),
//! ported from daemon/internal/store/thread_continuity_test.go: acceptance
//! sequence allocation, source-event-key dedup, session-scoped listing,
//! preview persistence, and tenant isolation.

use chrono::{DateTime, Duration, TimeZone, Utc};
use kura_store::SQLiteStore;
use kura_store::thread_continuity::ContinuityLookupQuery;
use kura_threads::{
    ContinuityItemKind, ContinuityPreview, ContinuityPreviewItem, ContinuityRole, ContinuityStatus,
    ContinuityTurn, RedactionStatus, SessionSegment, SourceKind, Thread,
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
        source_kind: SourceKind::Chat,
        source_summary: "continuity test".to_string(),
        last_activity_at: now,
        created_at: now,
        updated_at: now,
        retention_expires_at: Some(now + Duration::days(90)),
        redaction_status: RedactionStatus::Redacted,
    }
}

fn segment(thread: &Thread, now: DateTime<Utc>) -> SessionSegment {
    SessionSegment {
        session_segment_id: thread.current_session_segment_id.clone(),
        thread_id: thread.thread_id.clone(),
        tenant_id: thread.tenant_id.clone(),
        session_id: "sess_1".to_string(),
        generation: 1,
        state: "active".to_string(),
        started_at: now,
        ended_at: None,
        last_active_at: now,
        reset_from_session_segment_id: String::new(),
        partial_evidence: false,
    }
}

fn turn(thread: &Thread, content: &str) -> ContinuityTurn {
    ContinuityTurn {
        continuity_turn_id: String::new(),
        tenant_id: thread.tenant_id.clone(),
        thread_id: thread.thread_id.clone(),
        session_segment_id: thread.current_session_segment_id.clone(),
        acceptance_sequence: 0,
        role: ContinuityRole::User,
        source_kind: SourceKind::Chat,
        source_linkage_id: String::new(),
        source_message_id: String::new(),
        source_timestamp: None,
        dispatch_id: String::new(),
        response_to_turn_id: String::new(),
        safe_content: content.to_string(),
        content_redaction_status: RedactionStatus::Redacted,
        artifact_excerpt_refs: Vec::new(),
        recorded_at: Utc::now(),
        retention_expires_at: None,
        source_event_key: String::new(),
    }
}

#[test]
fn continuity_turn_sequence_allocation_and_dedup() {
    let dir = temp_dir("continuity_turns");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap();
    let thread = thread("thr_1", "ten_1", now);
    store.upsert_thread(&thread).unwrap();
    store
        .upsert_thread_session_segment(&segment(&thread, now))
        .unwrap();

    // Sequences allocate transactionally: 1, then 2.
    let first = store
        .save_continuity_turn(&turn(&thread, "first message"))
        .unwrap();
    assert_eq!(first.acceptance_sequence, 1);
    assert!(!first.continuity_turn_id.is_empty());
    assert!(first.retention_expires_at.is_some(), "retention defaulted");

    let second = store
        .save_continuity_turn(&turn(&thread, "second message"))
        .unwrap();
    assert_eq!(second.acceptance_sequence, 2);

    // Same source_event_key resolves to the existing turn (dedup).
    let mut dup = turn(&thread, "duplicate");
    dup.source_event_key = "evt_1".to_string();
    let saved = store.save_continuity_turn(&dup).unwrap();
    let mut again = turn(&thread, "duplicate retry");
    again.source_event_key = "evt_1".to_string();
    let resolved = store.save_continuity_turn(&again).unwrap();
    assert_eq!(
        resolved.continuity_turn_id, saved.continuity_turn_id,
        "dedup by source event key"
    );

    // Listing is scoped to the current session segment and ordered newest first.
    let query = ContinuityLookupQuery {
        tenant_id: "ten_1".to_string(),
        thread_id: "thr_1".to_string(),
        session_segment_id: thread.current_session_segment_id.clone(),
        limit: 0,
        now: Some(now + Duration::days(1)),
    };
    let listed = store.list_continuity_turns(&query).unwrap();
    assert_eq!(listed.len(), 3);
    assert_eq!(listed[0].acceptance_sequence, 3, "newest first");
    assert_eq!(listed[1].acceptance_sequence, 2);
    assert_eq!(listed[2].acceptance_sequence, 1);

    // Cross-tenant + cross-segment lookups are empty.
    let other_tenant = ContinuityLookupQuery {
        tenant_id: "ten_2".to_string(),
        ..query.clone()
    };
    assert!(
        store
            .list_continuity_turns(&other_tenant)
            .unwrap()
            .is_empty()
    );
    let other_segment = ContinuityLookupQuery {
        session_segment_id: "seg_other".to_string(),
        ..query.clone()
    };
    assert!(
        store
            .list_continuity_turns(&other_segment)
            .unwrap()
            .is_empty()
    );

    // Outside-session-segment listing finds reset-boundary turns.
    let mut out = turn(&thread, "old segment");
    out.session_segment_id = "seg_old".to_string();
    let out_saved = store.save_continuity_turn(&out).unwrap();
    let outside = store
        .list_continuity_turns_outside_session_segment(&query)
        .unwrap();
    assert_eq!(outside.len(), 1);
    assert_eq!(outside[0].continuity_turn_id, out_saved.continuity_turn_id);
}

#[test]
fn continuity_preview_save_and_detail() {
    let dir = temp_dir("continuity_preview");
    let store = SQLiteStore::new(&dir).unwrap();
    // Recent timestamps so the retention window (`now .. now+90d`) covers the
    // detail/summary lookups, which compare against the real clock.
    let now = Utc::now() - Duration::minutes(1);
    let thread = thread("thr_2", "ten_1", now);
    store.upsert_thread(&thread).unwrap();

    let preview = ContinuityPreview {
        continuity_preview_id: String::new(),
        tenant_id: "ten_1".to_string(),
        thread_id: "thr_2".to_string(),
        session_segment_id: thread.current_session_segment_id.clone(),
        dispatch_id: String::new(),
        request_turn_id: String::new(),
        response_turn_id: String::new(),
        window_policy_id: String::new(),
        max_prior_turns: 0,
        active_window_days: 0,
        included_count: 1,
        excluded_count: 0,
        continuity_applied: true,
        status: ContinuityStatus::Applied,
        failure_class: String::new(),
        assembly_started_at: now,
        assembly_completed_at: now + Duration::seconds(1),
        assembly_duration_ms: 0,
        retention_expires_at: DateTime::<Utc>::UNIX_EPOCH,
        redaction_status: RedactionStatus::Redacted,
    };
    let mut items = vec![
        ContinuityPreviewItem {
            preview_item_id: String::new(),
            continuity_preview_id: String::new(),
            tenant_id: "ten_1".to_string(),
            thread_id: "thr_2".to_string(),
            item_kind: ContinuityItemKind::Turn,
            continuity_turn_id: "turn_1".to_string(),
            role: None,
            artifact_ref: String::new(),
            artifact_excerpt_id: String::new(),
            handoff_source_reference_id: String::new(),
            decision: kura_threads::ContinuityDecision::Included,
            reason_code: kura_threads::ContinuityReason::IncludedRecent,
            acceptance_sequence: 1,
            source_timestamp: None,
            safe_summary: "included turn".to_string(),
            redaction_status: RedactionStatus::Redacted,
            item_order: 0,
        },
        ContinuityPreviewItem {
            preview_item_id: String::new(),
            continuity_preview_id: String::new(),
            tenant_id: "ten_1".to_string(),
            thread_id: "thr_2".to_string(),
            item_kind: ContinuityItemKind::ArtifactExcerpt,
            continuity_turn_id: String::new(),
            role: None,
            artifact_ref: "art_1".to_string(),
            artifact_excerpt_id: "excerpt_1".to_string(),
            handoff_source_reference_id: String::new(),
            decision: kura_threads::ContinuityDecision::Excluded,
            reason_code: kura_threads::ContinuityReason::TooOld,
            acceptance_sequence: 0,
            source_timestamp: None,
            safe_summary: "stale artifact".to_string(),
            redaction_status: RedactionStatus::Redacted,
            item_order: 0,
        },
    ];
    let saved = store.save_continuity_preview(preview, &mut items).unwrap();
    assert!(!saved.continuity_preview_id.is_empty());
    assert_eq!(
        saved.window_policy_id,
        kura_threads::DEFAULT_CONTINUITY_WINDOW_POLICY_ID,
        "policy defaulted"
    );
    assert_eq!(
        saved.max_prior_turns,
        kura_threads::DEFAULT_CONTINUITY_MAX_PRIOR_TURNS
    );
    assert_eq!(
        saved.assembly_duration_ms, 1000,
        "duration computed from started/completed"
    );
    assert!(saved.retention_expires_at > now, "retention defaulted");
    assert_eq!(items[0].item_order, 0);
    assert_eq!(items[1].item_order, 1, "order defaulted by index");

    let detail = store
        .get_continuity_preview_detail("ten_1", "thr_2", &saved.continuity_preview_id)
        .unwrap()
        .expect("detail present");
    assert_eq!(
        detail.preview.continuity_preview_id,
        saved.continuity_preview_id
    );
    assert_eq!(detail.preview.status, ContinuityStatus::Applied);
    assert_eq!(detail.items.len(), 2);
    assert_eq!(detail.items[0].item_kind, ContinuityItemKind::Turn);
    assert_eq!(
        detail.items[1].reason_code,
        kura_threads::ContinuityReason::TooOld
    );

    // Missing / cross-tenant / other-thread detail → None.
    assert!(
        store
            .get_continuity_preview_detail("ten_1", "thr_2", "contprev_missing")
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .get_continuity_preview_detail("ten_2", "thr_2", &saved.continuity_preview_id)
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .get_continuity_preview_detail("ten_1", "thr_other", &saved.continuity_preview_id)
            .unwrap()
            .is_none()
    );

    // Summaries list newest first.
    let summaries = store
        .list_continuity_preview_summaries("ten_1", "thr_2", 10)
        .unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(
        summaries[0].continuity_preview_id,
        saved.continuity_preview_id
    );
}
