//! Round-trip integration tests for the connector-domain CRUD methods ported from
//! `daemon/internal/store/store.go` into `connectors.rs`. Each test constructs a domain
//! value, upserts it, lists/gets it back, and asserts key fields. Wiring required before
//! these compile: add kura-connectors and kura-imtypes to the store crate's Cargo.toml and
//! declare `pub mod connectors;` in lib.rs so `ConnectorDeliveryBoundaryRecord` is
//! reachable from this integration test.

use std::collections::HashMap;

use chrono::{Duration, Utc};
use kura_connectors::{
    ConformanceResult, ConformanceResultStatus, Connector, ConnectorDiagnosticState,
    DiagnosticReasonCode, FreshnessState, LifecycleState, RedactionStatus, RemediationOwner,
    RetrySafety, Status,
};
use kura_imtypes::{DeliveryDirection, DeliveryStatus, MessageRecord};
use kura_store::{SQLiteStore, connectors::ConnectorDeliveryBoundaryRecord};

fn temp_dir(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("kura_store_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.to_string_lossy().to_string()
}

fn connector_fixture(connector_id: &str, now: chrono::DateTime<Utc>) -> Connector {
    Connector {
        connector_id: connector_id.to_string(),
        kind: "slack".to_string(),
        display_name: "Slack".to_string(),
        status: Status::Healthy,
        secret_refs: vec!["slack/bot_token".to_string()],
        created_at: now,
        updated_at: now,
        ..Connector::default()
    }
}

#[test]
fn connector_round_trips_through_sqlite() {
    let dir = temp_dir("connector");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();

    let mut connector = connector_fixture("conn_slack", now);
    store.upsert_connector(&connector).unwrap();
    // Upsert again through the ON CONFLICT path with a changed field.
    connector.status = Status::Degraded;
    store.upsert_connector(&connector).unwrap();

    let listed = store.list_connectors().unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.connector_id, "conn_slack");
    assert_eq!(got.kind, "slack");
    assert_eq!(got.display_name, "Slack");
    assert_eq!(got.status, Status::Degraded);
    assert_eq!(got.secret_refs, vec!["slack/bot_token".to_string()]);
    assert_eq!(got.failure_count, 0);
    assert_eq!(got.backoff_seconds, 0);
    // Optional columns round-trip as empty values.
    assert!(got.disabled_reason.is_empty());
    assert!(got.next_restart_at.is_none());
    assert!(got.last_failure_reason.is_empty());
}

#[test]
fn connector_message_round_trips_through_sqlite() {
    let dir = temp_dir("connector_msg");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();

    // Parent connector row (no FK in the ported schema, but the port contract expects
    // connector_messages to reference an existing connector).
    store
        .upsert_connector(&connector_fixture("conn_slack", now))
        .unwrap();

    let mut message = MessageRecord {
        delivery_id: "dlv_1".to_string(),
        tenant_id: String::new(),
        connector_id: "conn_slack".to_string(),
        direction: DeliveryDirection::Inbound,
        external_message_id: "ext_1".to_string(),
        connector_account_id: "acct_1".to_string(),
        channel_or_conversation_id: "chan_1".to_string(),
        provider_message_id: "prov_1".to_string(),
        channel_id: "C1".to_string(),
        content: "hello".to_string(),
        status: DeliveryStatus::Received,
        created_at: now,
        updated_at: now,
        ..MessageRecord::default()
    };
    store.upsert_connector_message(&message).unwrap();
    // Upsert again through the ON CONFLICT path with a changed field.
    message.status = DeliveryStatus::Replied;
    store.upsert_connector_message(&message).unwrap();

    let fetched = store
        .get_connector_message_by_external_id("conn_slack", DeliveryDirection::Inbound, "ext_1")
        .unwrap()
        .expect("found by external id");
    assert_eq!(fetched.delivery_id, "dlv_1");
    assert_eq!(fetched.status, DeliveryStatus::Replied);
    assert_eq!(fetched.content, "hello");
    assert_eq!(fetched.connector_account_id, "acct_1");
    assert_eq!(fetched.channel_or_conversation_id, "chan_1");

    let by_tenant = store
        .get_connector_message_by_external_id_for_tenant(
            "",
            "conn_slack",
            DeliveryDirection::Inbound,
            "ext_1",
        )
        .unwrap()
        .expect("found for tenant");
    assert_eq!(by_tenant.delivery_id, "dlv_1");

    let by_identity = store
        .get_connector_message_by_standard_identity(
            "",
            "acct_1",
            "chan_1",
            "prov_1",
            DeliveryDirection::Inbound,
            "",
        )
        .unwrap()
        .expect("found by standard identity");
    assert_eq!(by_identity.delivery_id, "dlv_1");
    assert_eq!(
        by_identity.equivalent_rule_id,
        "standard_provider_message_id"
    );

    assert!(
        store
            .get_connector_message_by_external_id(
                "conn_slack",
                DeliveryDirection::Inbound,
                "missing"
            )
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .get_connector_message_by_external_id_for_tenant(
                "",
                "conn_slack",
                DeliveryDirection::Inbound,
                ""
            )
            .unwrap()
            .is_none()
    );

    // A duplicate standard identity (same provider message, new delivery id) resolves
    // to the existing row with created = false.
    let dup = MessageRecord {
        delivery_id: "dlv_dup".to_string(),
        external_message_id: "ext_dup".to_string(),
        created_at: now,
        updated_at: now,
        ..message.clone()
    };
    let (existing, created) = store.create_connector_message_if_absent(&dup).unwrap();
    assert_eq!(existing.delivery_id, "dlv_1");
    assert!(!created);

    // A genuinely new message is created.
    let fresh = MessageRecord {
        delivery_id: "dlv_2".to_string(),
        external_message_id: "ext_2".to_string(),
        provider_message_id: "prov_2".to_string(),
        created_at: now,
        updated_at: now,
        ..message.clone()
    };
    let (saved, created) = store.create_connector_message_if_absent(&fresh).unwrap();
    assert_eq!(saved.delivery_id, "dlv_2");
    assert!(created);
}

#[test]
fn conformance_result_round_trips_through_sqlite() {
    let dir = temp_dir("conformance");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();

    let mut result = ConformanceResult {
        conformance_result_id: "cr_1".to_string(),
        tenant_id: String::new(),
        connector_kind: "slack".to_string(),
        connector_id: "conn_slack".to_string(),
        scenario_id: "sc_messaging".to_string(),
        area: "messaging".to_string(),
        result: ConformanceResultStatus::Pass,
        redaction_status: RedactionStatus::Redacted,
        evidence_timestamp: now,
        retention_expires_at: now + Duration::days(90),
        ..ConformanceResult::default()
    };
    store.save_connector_conformance_result(&result).unwrap();
    // Upsert again through the ON CONFLICT path with a changed field.
    result.result = ConformanceResultStatus::Limited;
    store.save_connector_conformance_result(&result).unwrap();

    let listed = store
        .list_connector_conformance_results("", "conn_slack", now)
        .unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.conformance_result_id, "cr_1");
    assert_eq!(got.connector_kind, "slack");
    assert_eq!(got.connector_id, "conn_slack");
    assert_eq!(got.scenario_id, "sc_messaging");
    assert_eq!(got.area, "messaging");
    assert_eq!(got.result, ConformanceResultStatus::Limited);
    assert_eq!(got.redaction_status, RedactionStatus::Redacted);

    // An empty id is generated on save; a second row lands for the same connector.
    let generated = ConformanceResult {
        conformance_result_id: String::new(),
        tenant_id: String::new(),
        connector_kind: "slack".to_string(),
        connector_id: "conn_slack".to_string(),
        scenario_id: "sc_auth".to_string(),
        area: "auth".to_string(),
        result: ConformanceResultStatus::Pass,
        redaction_status: RedactionStatus::Redacted,
        evidence_timestamp: now,
        retention_expires_at: now + Duration::days(90),
        ..ConformanceResult::default()
    };
    store.save_connector_conformance_result(&generated).unwrap();
    let listed = store
        .list_connector_conformance_results("", "conn_slack", now)
        .unwrap();
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().any(|r| r.conformance_result_id == "cr_1"));
    assert!(
        listed
            .iter()
            .any(|r| r.conformance_result_id.starts_with("conformance_result_"))
    );
}

#[test]
fn diagnostic_state_round_trips_through_sqlite() {
    let dir = temp_dir("diagnostic");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();

    let mut state = ConnectorDiagnosticState {
        diagnostic_state_id: "ds_1".to_string(),
        tenant_id: String::new(),
        connector_id: "conn_slack".to_string(),
        connector_account_id: "acct_1".to_string(),
        status: LifecycleState::Degraded,
        reason_code: DiagnosticReasonCode::RateLimited,
        remediation_owner: RemediationOwner::Operator,
        user_visible_severity: "medium".to_string(),
        retry_safety: RetrySafety::Retryable,
        evidence_timestamp: now,
        freshness_state: FreshnessState::Fresh,
        redaction_status: RedactionStatus::Redacted,
        retention_expires_at: now + Duration::days(90),
        safe_evidence: HashMap::new(),
        redaction_failure_id: String::new(),
    };
    store.save_connector_diagnostic_state(&state).unwrap();
    // Upsert again through the ON CONFLICT path with a changed field.
    state.status = LifecycleState::RateLimited;
    store.save_connector_diagnostic_state(&state).unwrap();

    let listed = store
        .list_connector_diagnostic_states("", "conn_slack", now)
        .unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.diagnostic_state_id, "ds_1");
    assert_eq!(got.connector_account_id, "acct_1");
    assert_eq!(got.status, LifecycleState::RateLimited);
    assert_eq!(got.reason_code, DiagnosticReasonCode::RateLimited);
    assert_eq!(got.remediation_owner, RemediationOwner::Operator);
    assert_eq!(got.user_visible_severity, "medium");
    assert_eq!(got.retry_safety, RetrySafety::Retryable);
    assert_eq!(got.freshness_state, FreshnessState::Fresh);
    assert_eq!(got.redaction_status, RedactionStatus::Redacted);
}

#[test]
fn diagnostic_state_freshness_recomputed_on_read() {
    let dir = temp_dir("diag_freshness");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();

    let stale = ConnectorDiagnosticState {
        diagnostic_state_id: "ds_stale".to_string(),
        tenant_id: String::new(),
        connector_id: "conn_slack".to_string(),
        connector_account_id: String::new(),
        status: LifecycleState::Failed,
        reason_code: DiagnosticReasonCode::NetworkFailed,
        remediation_owner: RemediationOwner::Operator,
        user_visible_severity: "high".to_string(),
        retry_safety: RetrySafety::Blocked,
        // Evidence older than the 15-minute staleness window.
        evidence_timestamp: now - Duration::minutes(30),
        freshness_state: FreshnessState::Fresh, // stored as fresh, must read back stale
        redaction_status: RedactionStatus::Redacted,
        retention_expires_at: now + Duration::days(90),
        safe_evidence: HashMap::new(),
        redaction_failure_id: String::new(),
    };
    store.save_connector_diagnostic_state(&stale).unwrap();

    let listed = store
        .list_connector_diagnostic_states("", "conn_slack", now)
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].freshness_state, FreshnessState::Stale);
}

#[test]
fn diagnostic_state_writes_redaction_failure_row() {
    let dir = temp_dir("diag_redaction");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();

    // Suppressed redaction with no failure id: the store generates one and upserts the
    // connector_diagnostic_redaction_failures row (no error).
    let state = ConnectorDiagnosticState {
        diagnostic_state_id: "ds_2".to_string(),
        tenant_id: String::new(),
        connector_id: "conn_slack".to_string(),
        connector_account_id: String::new(),
        status: LifecycleState::Failed,
        reason_code: DiagnosticReasonCode::UnknownConnectorFailure,
        remediation_owner: RemediationOwner::Operator,
        user_visible_severity: "high".to_string(),
        retry_safety: RetrySafety::Blocked,
        evidence_timestamp: now,
        freshness_state: FreshnessState::Fresh,
        redaction_status: RedactionStatus::Suppressed,
        retention_expires_at: now + Duration::days(90),
        safe_evidence: HashMap::new(),
        redaction_failure_id: String::new(),
    };
    store.save_connector_diagnostic_state(&state).unwrap();

    // A fixed failure id upserts the failure row through the ON CONFLICT path twice.
    let with_failure = ConnectorDiagnosticState {
        diagnostic_state_id: "ds_3".to_string(),
        redaction_status: RedactionStatus::Failed,
        redaction_failure_id: "rf_1".to_string(),
        ..state.clone()
    };
    store
        .save_connector_diagnostic_state(&with_failure)
        .unwrap();
    store
        .save_connector_diagnostic_state(&with_failure)
        .unwrap();
}

#[test]
fn delivery_boundary_saves_through_sqlite() {
    let dir = temp_dir("delivery_boundary");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();

    let mut record = ConnectorDeliveryBoundaryRecord {
        boundary_id: "b_1".to_string(),
        tenant_id: String::new(),
        connector_id: "conn_slack".to_string(),
        foreground_reply_outcome_id: "fro_1".to_string(),
        background_delivery_id: "bg_1".to_string(),
        transport_kind: "slack".to_string(),
        separation_status: "separate_truths".to_string(),
        created_at: now,
        document: "{\"kind\":\"boundary\"}".to_string(),
    };
    store.save_connector_delivery_boundary(&record).unwrap();
    // Upsert again through the ON CONFLICT path with a changed field.
    record.separation_status = "merged_truths".to_string();
    store.save_connector_delivery_boundary(&record).unwrap();

    // Empty boundary id + document exercise the store-side defaults (generated id,
    // default separation status, marshaled document).
    let generated = ConnectorDeliveryBoundaryRecord {
        boundary_id: String::new(),
        tenant_id: String::new(),
        connector_id: "conn_slack".to_string(),
        foreground_reply_outcome_id: String::new(),
        background_delivery_id: String::new(),
        transport_kind: "slack".to_string(),
        separation_status: String::new(),
        created_at: now,
        document: String::new(),
    };
    store.save_connector_delivery_boundary(&generated).unwrap();
}
