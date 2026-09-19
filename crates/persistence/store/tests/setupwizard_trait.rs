//! Trait-surface tests for `kura_setupwizard::Store` implemented by
//! `SetupWizardStoreHandle` (the Send + Sync newtype over the SQLite store).
//! Exercises the exact async trait methods the setup-wizard service calls:
//! session save/get/list and attempt append/list.

use std::sync::Arc;

use chrono::Utc;
use kura_setupwizard::{
    RedactionStatus, RemediationOwner, SafeUseMode, SetupAttempt, SetupOperation, SetupSession,
    SetupState, SetupStyle, Store, TargetKind,
};
use kura_store::{SQLiteStore, SetupWizardStoreHandle};

fn temp_dir(name: &str) -> String {
    let dir =
        std::env::temp_dir().join(format!("kura_store_sw_trait_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.to_string_lossy().to_string()
}

fn make_session(session_id: &str) -> SetupSession {
    let now = Utc::now();
    SetupSession {
        setup_session_id: session_id.to_string(),
        tenant_id: "ten_1".to_string(),
        actor_principal_id: "trait_user".to_string(),
        target_id: "provider.openai_compatible".to_string(),
        target_kind: TargetKind::Provider,
        setup_style: SetupStyle::SubmittedSecret,
        state: SetupState::InProgress,
        reason_code: String::new(),
        retryable: true,
        remediation_owner: RemediationOwner::ProductUser,
        safe_use_mode: SafeUseMode::Blocked,
        allowed_capabilities: Vec::new(),
        current_attempt_id: String::new(),
        diagnostic_result_id: String::new(),
        diagnostic_run_id: String::new(),
        diagnostic_stage: String::new(),
        diagnostic_source_kind: String::new(),
        diagnostic_source_id: String::new(),
        diagnostic_allowed_use: Vec::new(),
        redaction_status: RedactionStatus::Redacted,
        resource_refs: Vec::new(),
        redacted_evidence: std::collections::HashMap::new(),
        oauth_state_ref: String::new(),
        created_at: now,
        updated_at: now,
        last_transition_at: now,
        last_transition_audit_id: String::new(),
        operator_remediation: String::new(),
        user_remediation: String::new(),
        unsupported_reason_code: String::new(),
    }
}

fn make_attempt(attempt_id: &str, session_id: &str) -> SetupAttempt {
    SetupAttempt {
        attempt_id: attempt_id.to_string(),
        setup_session_id: session_id.to_string(),
        tenant_id: "ten_1".to_string(),
        actor_principal_id: "trait_user".to_string(),
        operation: SetupOperation::Start,
        from_state: SetupState::NotStarted,
        to_state: SetupState::InProgress,
        reason_code: String::new(),
        redacted_evidence: std::collections::HashMap::new(),
        resource_refs: Vec::new(),
        redaction_status: RedactionStatus::Redacted,
        diagnostic_result_id: String::new(),
        created_at: Utc::now(),
    }
}

#[tokio::test]
async fn setupwizard_store_trait_session_round_trip() {
    let dir = temp_dir("session");
    let store = SQLiteStore::new(&dir).unwrap();
    let handle = Arc::new(SetupWizardStoreHandle::new(store));

    let mut session = make_session("setup_ten_1_target_test");
    handle.save_setup_session(session.clone()).await.unwrap();
    // Re-save through the upsert path with a state change.
    session.state = SetupState::Ready;
    session.reason_code = "healthy".to_string();
    handle.save_setup_session(session.clone()).await.unwrap();

    let got = handle
        .get_setup_session("ten_1", "setup_ten_1_target_test")
        .await
        .unwrap()
        .expect("session");
    assert_eq!(got.state, SetupState::Ready);
    assert_eq!(got.reason_code, "healthy");
    assert_eq!(
        handle
            .get_setup_session("ten_other", "setup_ten_1_target_test")
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        handle.get_setup_session("ten_1", "missing").await.unwrap(),
        None
    );

    let listed = handle.list_setup_sessions("ten_1").await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].setup_session_id, "setup_ten_1_target_test");
    assert!(
        handle
            .list_setup_sessions("ten_other")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn setupwizard_store_trait_attempt_round_trip() {
    let dir = temp_dir("attempt");
    let store = SQLiteStore::new(&dir).unwrap();
    let handle = Arc::new(SetupWizardStoreHandle::new(store));

    let session_id = "setup_ten_1_target_test";
    handle
        .save_setup_session(make_session(session_id))
        .await
        .unwrap();
    handle
        .append_setup_attempt(make_attempt("attempt_1", session_id))
        .await
        .unwrap();
    handle
        .append_setup_attempt(make_attempt("attempt_2", session_id))
        .await
        .unwrap();

    let attempts = handle
        .list_setup_attempts("ten_1", session_id)
        .await
        .unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].operation, SetupOperation::Start);
    assert_eq!(attempts[0].to_state, SetupState::InProgress);
    assert!(
        handle
            .list_setup_attempts("ten_other", session_id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        handle
            .list_setup_attempts("ten_1", "missing")
            .await
            .unwrap()
            .is_empty()
    );
}
