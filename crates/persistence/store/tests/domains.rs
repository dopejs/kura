//! Round-trip integration tests for the integration / calendar / mail domain CRUD methods
//! ported from `daemon/internal/store/store.go`. Each test constructs a domain type, upserts
//! it, lists/gets it back, and asserts key fields. Wiring required before these compile: add
//! kura-integrations, kura-calendar, and kura-mail to the store crate's Cargo.toml and declare
//! the modules in lib.rs (`calendar` and `mail` must be public so the filter types are
//! reachable from this integration test).

use chrono::Utc;
use kura_calendar::{
    AccountProjection as CalendarAccount, Artifact as CalendarArtifact,
    ArtifactKind as CalendarArtifactKind, Operation as CalendarOperation,
    OperationClass as CalendarOperationClass, OperationStatus as CalendarOperationStatus,
};
use kura_integrations::{AccountBinding, BackendBinding, BackendKind, ReadinessStatus, Resource};
use kura_mail::{
    AccountProjection as MailAccount, Artifact as MailArtifact, ArtifactKind as MailArtifactKind,
    Operation as MailOperation, OperationClass as MailOperationClass,
    OperationStatus as MailOperationStatus, ResultMode,
};
use kura_store::{SQLiteStore, calendar::CalendarOperationFilter, mail::MailOperationFilter};

fn temp_dir(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("kura_store_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.to_string_lossy().to_string()
}

#[test]
fn integration_round_trips_through_sqlite() {
    let dir = temp_dir("integration");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();

    let mut resource = Resource {
        integration_id: "int_cal".to_string(),
        domain_kind: "calendar".to_string(),
        display_name: "Google Calendar".to_string(),
        environment_scope: "test".to_string(),
        readiness_status: ReadinessStatus::Healthy,
        canonical_default: true,
        account_binding: Some(AccountBinding {
            account_key: "alice@example.com".to_string(),
            ..AccountBinding::default()
        }),
        backend_binding: BackendBinding {
            backend_kind: BackendKind::Native,
            ..BackendBinding::default()
        },
        created_at: now,
        updated_at: now,
        last_transition_at: now,
        ..Resource::default()
    };
    store.upsert_integration(&resource).unwrap();
    // Upsert again through the ON CONFLICT path with a changed field.
    resource.readiness_status = ReadinessStatus::Degraded;
    store.upsert_integration(&resource).unwrap();

    let listed = store.list_integrations("test").unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.integration_id, "int_cal");
    assert_eq!(got.domain_kind, "calendar");
    assert_eq!(got.display_name, "Google Calendar");
    assert_eq!(got.environment_scope, "test");
    assert_eq!(got.readiness_status, ReadinessStatus::Degraded);
    assert_eq!(got.canonical_default, true);
    assert_eq!(got.backend_binding.backend_kind, BackendKind::Native);
    assert_eq!(
        got.account_binding.as_ref().unwrap().account_key,
        "alice@example.com"
    );
    // No rows in a different environment scope.
    assert!(store.list_integrations("prod").unwrap().is_empty());
}

#[test]
fn calendar_domain_round_trips_through_sqlite() {
    let dir = temp_dir("calendar");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();

    let account = CalendarAccount {
        calendar_account_id: "cal_acct_1".to_string(),
        integration_id: "int_cal".to_string(),
        domain_kind: "calendar".to_string(),
        environment_scope: "test".to_string(),
        account_key: "alice@example.com".to_string(),
        readiness_status: "healthy".to_string(),
        canonical_default: true,
        last_synced_at: now,
        updated_at: now,
        ..CalendarAccount::default()
    };
    store.upsert_calendar_account(&account).unwrap();
    let accounts = store.list_calendar_accounts("test").unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].calendar_account_id, "cal_acct_1");
    assert_eq!(accounts[0].domain_kind, "calendar");
    assert_eq!(accounts[0].account_key, "alice@example.com");
    assert_eq!(accounts[0].readiness_status, "healthy");
    assert_eq!(accounts[0].canonical_default, true);

    let operation = CalendarOperation {
        operation_id: "cal_op_1".to_string(),
        integration_id: "int_cal".to_string(),
        calendar_account_id: "cal_acct_1".to_string(),
        environment_scope: "test".to_string(),
        operation_class: CalendarOperationClass::CreateEvent,
        status: CalendarOperationStatus::Completed,
        external_event_id: "evt_1".to_string(),
        run_id: "run_1".to_string(),
        workflow_id: "wf_1".to_string(),
        schedule_id: "sched_1".to_string(),
        delivery_id: "deliv_1".to_string(),
        created_at: now,
        updated_at: now,
        ..CalendarOperation::default()
    };
    store.upsert_calendar_operation(&operation).unwrap();

    let ops = store
        .list_calendar_operations("test", &CalendarOperationFilter::default())
        .unwrap();
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].operation_id, "cal_op_1");
    assert_eq!(ops[0].operation_class, CalendarOperationClass::CreateEvent);
    assert_eq!(ops[0].status, CalendarOperationStatus::Completed);
    assert_eq!(ops[0].external_event_id, "evt_1");
    assert_eq!(ops[0].run_id, "run_1");
    assert_eq!(ops[0].workflow_id, "wf_1");
    assert_eq!(ops[0].schedule_id, "sched_1");
    assert_eq!(ops[0].delivery_id, "deliv_1");

    // Dynamic filter: status match and non-match.
    let status_filter = CalendarOperationFilter {
        status: CalendarOperationStatus::Completed.as_str().to_string(),
        ..CalendarOperationFilter::default()
    };
    assert_eq!(
        store
            .list_calendar_operations("test", &status_filter)
            .unwrap()
            .len(),
        1
    );
    let missed_filter = CalendarOperationFilter {
        status: "failed".to_string(),
        ..CalendarOperationFilter::default()
    };
    assert!(
        store
            .list_calendar_operations("test", &missed_filter)
            .unwrap()
            .is_empty()
    );

    // Get by id matches Go's in-memory lookup over the unfiltered list.
    let got = store
        .get_calendar_operation_by_id("test", "cal_op_1")
        .unwrap()
        .expect("found");
    assert_eq!(got.operation_id, "cal_op_1");
    assert_eq!(
        store
            .get_calendar_operation_by_id("test", "missing")
            .unwrap(),
        None
    );

    let artifact = CalendarArtifact {
        artifact_id: "cal_art_1".to_string(),
        operation_id: "cal_op_1".to_string(),
        integration_id: "int_cal".to_string(),
        environment_scope: "test".to_string(),
        kind: CalendarArtifactKind::EventSnapshot,
        external_event_id: "evt_1".to_string(),
        created_at: now,
        ..CalendarArtifact::default()
    };
    store.upsert_calendar_artifact(&artifact).unwrap();
    let artifacts = store.list_calendar_artifacts("test", "cal_op_1").unwrap();
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].artifact_id, "cal_art_1");
    assert_eq!(artifacts[0].kind, CalendarArtifactKind::EventSnapshot);
    assert_eq!(artifacts[0].external_event_id, "evt_1");
    assert!(
        store
            .list_calendar_artifacts("test", "other_op")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn mail_domain_round_trips_through_sqlite() {
    let dir = temp_dir("mail");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();

    let account = MailAccount {
        mail_account_id: "mail_acct_1".to_string(),
        integration_id: "int_mail".to_string(),
        domain_kind: "mail".to_string(),
        environment_scope: "test".to_string(),
        account_key: "bob@example.com".to_string(),
        readiness_status: "healthy".to_string(),
        canonical_default: true,
        last_synced_at: now,
        updated_at: now,
        ..MailAccount::default()
    };
    store.upsert_mail_account(&account).unwrap();
    let accounts = store.list_mail_accounts("test").unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].mail_account_id, "mail_acct_1");
    assert_eq!(accounts[0].domain_kind, "mail");
    assert_eq!(accounts[0].account_key, "bob@example.com");
    assert_eq!(accounts[0].readiness_status, "healthy");
    assert_eq!(accounts[0].canonical_default, true);

    let operation = MailOperation {
        operation_id: "mail_op_1".to_string(),
        integration_id: "int_mail".to_string(),
        mail_account_id: "mail_acct_1".to_string(),
        environment_scope: "test".to_string(),
        operation_class: MailOperationClass::SendMessage,
        status: MailOperationStatus::Completed,
        result_mode: ResultMode::Sent,
        thread_id: "thr_1".to_string(),
        message_id: "msg_1".to_string(),
        draft_id: "drf_1".to_string(),
        run_id: "run_1".to_string(),
        workflow_id: "wf_1".to_string(),
        schedule_id: "sched_1".to_string(),
        delivery_id: "deliv_1".to_string(),
        created_at: now,
        updated_at: now,
        ..MailOperation::default()
    };
    store.upsert_mail_operation(&operation).unwrap();

    let ops = store
        .list_mail_operations("test", &MailOperationFilter::default())
        .unwrap();
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].operation_id, "mail_op_1");
    assert_eq!(ops[0].operation_class, MailOperationClass::SendMessage);
    assert_eq!(ops[0].status, MailOperationStatus::Completed);
    assert_eq!(ops[0].result_mode, ResultMode::Sent);
    assert_eq!(ops[0].thread_id, "thr_1");
    assert_eq!(ops[0].message_id, "msg_1");
    assert_eq!(ops[0].draft_id, "drf_1");
    assert_eq!(ops[0].run_id, "run_1");
    assert_eq!(ops[0].workflow_id, "wf_1");
    assert_eq!(ops[0].schedule_id, "sched_1");
    assert_eq!(ops[0].delivery_id, "deliv_1");

    // Dynamic filter: result_mode match and non-match.
    let mode_filter = MailOperationFilter {
        result_mode: ResultMode::Sent.as_str().to_string(),
        ..MailOperationFilter::default()
    };
    assert_eq!(
        store
            .list_mail_operations("test", &mode_filter)
            .unwrap()
            .len(),
        1
    );
    let missed_filter = MailOperationFilter {
        thread_id: "other_thread".to_string(),
        ..MailOperationFilter::default()
    };
    assert!(
        store
            .list_mail_operations("test", &missed_filter)
            .unwrap()
            .is_empty()
    );

    let got = store
        .get_mail_operation_by_id("test", "mail_op_1")
        .unwrap()
        .expect("found");
    assert_eq!(got.operation_id, "mail_op_1");
    assert_eq!(
        store.get_mail_operation_by_id("test", "missing").unwrap(),
        None
    );

    let artifact = MailArtifact {
        artifact_id: "mail_art_1".to_string(),
        operation_id: "mail_op_1".to_string(),
        integration_id: "int_mail".to_string(),
        environment_scope: "test".to_string(),
        kind: MailArtifactKind::MessageSnapshot,
        thread_id: "thr_1".to_string(),
        message_id: "msg_1".to_string(),
        draft_id: "drf_1".to_string(),
        attachment_ref_id: "att_1".to_string(),
        created_at: now,
        ..MailArtifact::default()
    };
    store.upsert_mail_artifact(&artifact).unwrap();
    let artifacts = store.list_mail_artifacts("test", "mail_op_1").unwrap();
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].artifact_id, "mail_art_1");
    assert_eq!(artifacts[0].kind, MailArtifactKind::MessageSnapshot);
    assert_eq!(artifacts[0].thread_id, "thr_1");
    assert_eq!(artifacts[0].message_id, "msg_1");
    assert_eq!(artifacts[0].draft_id, "drf_1");
    assert_eq!(artifacts[0].attachment_ref_id, "att_1");
    assert!(
        store
            .list_mail_artifacts("test", "other_op")
            .unwrap()
            .is_empty()
    );
}
