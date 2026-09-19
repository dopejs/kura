//! r51 slack channel-connector fixture: two tenants × every Roadmap 51 table,
//! with document_json matching what the Go Save* accessors store.

mod common;

use common::{open_conn, temp_dir};
use kura_migrationfixture::{
    apply_head_migrations, build_pre_tenant_v21_fixture, count_r51_slack_channel_connector_rows,
    seed_r51_slack_channel_connector_rows,
};

fn head_store() -> (kura_store::SQLiteStore, String) {
    let dir = temp_dir("r51");
    let store = build_pre_tenant_v21_fixture(&dir).unwrap();
    apply_head_migrations(&store).unwrap();
    (store, dir)
}

#[test]
fn r51_seeds_two_tenants_per_table() {
    let (store, _dir) = head_store();
    let fixture = seed_r51_slack_channel_connector_rows(&store).unwrap();
    assert_eq!(
        fixture.tenant_ids,
        vec!["ten_slack_alpha", "ten_slack_beta"]
    );

    let counts = count_r51_slack_channel_connector_rows(&store).unwrap();
    for (table, expected) in &fixture.expected_row_count {
        assert_eq!(counts.get(table), Some(expected), "table {table}");
    }
}

#[test]
fn r51_hosted_setup_row_matches_go_document_shape() {
    let (store, _dir) = head_store();
    seed_r51_slack_channel_connector_rows(&store).unwrap();
    let conn = open_conn(store.db_path());

    let row: (String, String, String, i64, String, String, String) = conn
        .query_row(
            "SELECT terminal_state, oauth_state, route_policy_state, delivery_eligible, workspace_binding_id, reason_code, document_json FROM slack_hosted_setups WHERE tenant_id = 'ten_slack_alpha' AND connector_id = 'slack-r51-1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?)),
        )
        .unwrap();
    assert_eq!(row.0, "action-required");
    assert_eq!(row.1, "grant_valid");
    assert_eq!(row.2, "none");
    assert_eq!(row.3, 0);
    assert_eq!(row.4, "slack_workspace_binding_1");
    assert_eq!(row.5, "blocked_route");

    let doc: serde_json::Value = serde_json::from_str(&row.6).unwrap();
    assert_eq!(doc["tenantId"], "ten_slack_alpha");
    assert_eq!(doc["connectorId"], "slack-r51-1");
    assert_eq!(doc["connectorKind"], "slack");
    assert_eq!(doc["displayName"], "Slack R51");
    assert_eq!(doc["status"], "degraded");
    assert_eq!(doc["terminalState"], "action-required");
    assert_eq!(doc["oauthState"], "grant_valid");
    assert_eq!(doc["routePolicyState"], "none");
    assert_eq!(doc["deliveryEligible"], false);
    assert_eq!(doc["workspaceBindingId"], "slack_workspace_binding_1");
    assert_eq!(doc["reasonCode"], "blocked_route");
    assert_eq!(doc["redactionStatus"], "redacted");
}

#[test]
fn r51_route_policy_smoke_event_rows_load_back() {
    let (store, _dir) = head_store();
    seed_r51_slack_channel_connector_rows(&store).unwrap();
    let conn = open_conn(store.db_path());

    // Route policy: selected channel embedded in the document.
    let policy: (String, String, String, String, String) = conn
        .query_row(
            "SELECT workspace_binding_id, validation_state, reason_code, redaction_status, document_json FROM slack_route_policies WHERE tenant_id = 'ten_slack_beta' AND connector_id = 'slack-r51-2'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .unwrap();
    assert_eq!(policy.0, "slack_workspace_binding_2");
    assert_eq!(policy.1, "valid");
    assert_eq!(policy.2, "healthy");
    assert_eq!(policy.3, "redacted");

    let doc: serde_json::Value = serde_json::from_str(&policy.4).unwrap();
    assert_eq!(doc["tenantId"], "ten_slack_beta");
    assert_eq!(doc["connectorId"], "slack-r51-2");
    assert_eq!(doc["workspaceBindingId"], "slack_workspace_binding_2");
    assert_eq!(doc["mentionGate"], "agent_mention_required");
    assert_eq!(doc["threadReplyMode"], "channel_mentions_thread_rooted");
    assert_eq!(doc["validationState"], "valid");
    assert_eq!(doc["allowedDMUsers"][0], "user_2");
    assert_eq!(doc["allowedDMUserGroups"][0], "group_2");
    assert_eq!(doc["selectedChannels"][0]["conversationId"], "channel_2");
    assert_eq!(
        doc["selectedChannels"][0]["selectedChannelState"],
        "selected"
    );
    assert_eq!(doc["selectedChannels"][0]["validationState"], "valid");
    assert_eq!(doc["selectedChannels"][0]["redactionStatus"], "redacted");
    assert_eq!(doc["safeEvidence"]["scope"], "selected_channel_and_dm");

    // Smoke evidence.
    let smoke: (String, String, String, String) = conn
        .query_row(
            "SELECT status, authorization_mode, owner, reason FROM slack_smoke_evidence WHERE smoke_evidence_id = 'slack_smoke_1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        smoke,
        (
            "skipped".to_string(),
            "unavailable".to_string(),
            "operator".to_string(),
            "safe_slack_authorization_unavailable".to_string()
        )
    );

    // Event evidence: composite identity + route outcome.
    let event: (String, String, String, String, String, String) = conn
        .query_row(
            "SELECT workspace_id, conversation_id, message_id, event_id, route_outcome, document_json FROM slack_event_evidence WHERE tenant_id = 'ten_slack_alpha' AND connector_id = 'slack-r51-1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
        )
        .unwrap();
    assert_eq!(event.0, "workspace_1");
    assert_eq!(event.1, "channel_1");
    assert_eq!(event.2, "message_1");
    assert_eq!(event.3, "event_1");
    assert_eq!(event.4, "accepted");
    let edoc: serde_json::Value = serde_json::from_str(&event.5).unwrap();
    assert_eq!(
        edoc["safeEvidence"]["identityRule"],
        "slack_workspace_conversation_message_id"
    );
}
