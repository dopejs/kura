//! Roadmap 51 slack channel-connector fixture (port of
//! daemon/internal/store/migrationfixture/r51_slack_channel_connector.go).
//!
//! The Go fixture seeds via the store accessors SaveSlackHostedSetup /
//! SaveSlackRoutePolicy / SaveSlackSmokeEvidence / SaveSlackEventEvidence, which
//! are not yet ported to kura-store. The tables exist (migration v47) and the
//! rows below replicate those accessors' exact column writes and document_json
//! payloads.

use std::collections::HashMap;

use rusqlite::params;

use kura_store::SQLiteStore;

use crate::FIXTURE_TIMESTAMP;
use crate::records::{
    SlackConversationRouteDocument, slack_event_evidence_document, slack_hosted_setup_document,
    slack_route_policy_document, slack_smoke_evidence_document,
};
use crate::seeds::exec_insert;

/// Table names expected from the Roadmap 51 storage migration (migration v47).
pub static R51_SLACK_CHANNEL_CONNECTOR_TABLE_NAMES: [&str; 4] = [
    "slack_hosted_setups",
    "slack_route_policies",
    "slack_smoke_evidence",
    "slack_event_evidence",
];

/// Expected per-table row counts after seeding (Go R51SlackChannelConnectorFixture).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct R51SlackChannelConnectorFixture {
    pub tenant_ids: Vec<String>,
    pub expected_row_count: HashMap<String, i64>,
}

#[must_use]
pub fn build_r51_slack_channel_connector_fixture() -> R51SlackChannelConnectorFixture {
    let counts = HashMap::from([
        ("slack_hosted_setups".to_string(), 2),
        ("slack_route_policies".to_string(), 2),
        ("slack_smoke_evidence".to_string(), 2),
        ("slack_event_evidence".to_string(), 2),
    ]);
    R51SlackChannelConnectorFixture {
        tenant_ids: vec!["ten_slack_alpha".to_string(), "ten_slack_beta".to_string()],
        expected_row_count: counts,
    }
}

/// Seeds two tenants × every r51 table. Requires the store at head schema (v47+).
pub fn seed_r51_slack_channel_connector_rows(
    store: &SQLiteStore,
) -> Result<R51SlackChannelConnectorFixture, String> {
    let fixture = build_r51_slack_channel_connector_fixture();
    let conn = crate::open_fixture_connection(store.db_path())?;
    let ts = FIXTURE_TIMESTAMP;

    for (index, tenant_id) in fixture.tenant_ids.iter().enumerate() {
        let suffix = (index + 1).to_string();
        let connector_id = format!("slack-r51-{suffix}");
        let workspace_binding_id = format!("slack_workspace_binding_{suffix}");

        let hosted_document = slack_hosted_setup_document(
            tenant_id,
            &connector_id,
            "slack",
            "Slack R51",
            "degraded",
            "action-required",
            "grant_valid",
            "none",
            &workspace_binding_id,
            "blocked_route",
            "redacted",
            ts,
            ts,
            ts,
            ts,
        )?;
        exec_insert(
            &conn,
            "INSERT INTO slack_hosted_setups (tenant_id, connector_id, connector_kind, display_name, status, terminal_state, oauth_state, route_policy_state, delivery_eligible, workspace_binding_id, reason_code, redaction_status, created_at, updated_at, validated_at, retention_expires_at, document_json) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                tenant_id,
                connector_id,
                "slack",
                "Slack R51",
                "degraded",
                "action-required",
                "grant_valid",
                "none",
                0i64,
                workspace_binding_id,
                "blocked_route",
                "redacted",
                ts,
                ts,
                ts,
                ts,
                hosted_document
            ],
        )?;

        let route_document = slack_route_policy_document(
            tenant_id,
            &connector_id,
            &workspace_binding_id,
            vec![SlackConversationRouteDocument {
                conversation_id: format!("channel_{suffix}"),
                conversation_type: "channel".to_string(),
                selected_channel_state: "selected".to_string(),
                validation_state: "valid".to_string(),
                reason_code: String::new(),
                redaction_status: "redacted".to_string(),
                safe_evidence: std::collections::BTreeMap::new(),
            }],
            vec![format!("user_{suffix}")],
            vec![format!("group_{suffix}")],
            "agent_mention_required",
            "channel_mentions_thread_rooted",
            "valid",
            "healthy",
            ts,
            "redacted",
            &[("scope", "selected_channel_and_dm")],
        )?;
        exec_insert(
            &conn,
            "INSERT INTO slack_route_policies (tenant_id, connector_id, workspace_binding_id, validation_state, reason_code, validated_at, redaction_status, document_json) VALUES (?,?,?,?,?,?,?,?)",
            params![
                tenant_id,
                connector_id,
                workspace_binding_id,
                "valid",
                "healthy",
                ts,
                "redacted",
                route_document
            ],
        )?;

        let smoke_document = slack_smoke_evidence_document(
            &format!("slack_smoke_{suffix}"),
            tenant_id,
            &connector_id,
            &workspace_binding_id,
            "skipped",
            "unavailable",
            "operator",
            "safe_slack_authorization_unavailable",
            "live smoke skipped",
            ts,
            ts,
            "redacted",
            &[("policy", "structured_skip")],
        )?;
        exec_insert(
            &conn,
            "INSERT INTO slack_smoke_evidence (smoke_evidence_id, tenant_id, connector_id, workspace_binding_id, status, authorization_mode, owner, reason, remaining_risk, validated_at, retention_expires_at, redaction_status, document_json) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                format!("slack_smoke_{suffix}"),
                tenant_id,
                connector_id,
                workspace_binding_id,
                "skipped",
                "unavailable",
                "operator",
                "safe_slack_authorization_unavailable",
                "live smoke skipped",
                ts,
                ts,
                "redacted",
                smoke_document
            ],
        )?;

        let event_document = slack_event_evidence_document(
            tenant_id,
            &connector_id,
            &format!("workspace_{suffix}"),
            &format!("channel_{suffix}"),
            &format!("message_{suffix}"),
            &format!("event_{suffix}"),
            "accepted",
            "accepted",
            ts,
            ts,
            "redacted",
            &[("identityRule", "slack_workspace_conversation_message_id")],
        )?;
        exec_insert(
            &conn,
            "INSERT INTO slack_event_evidence (tenant_id, connector_id, workspace_id, conversation_id, message_id, event_id, route_outcome, reason_code, received_at, retention_expires_at, redaction_status, document_json) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                tenant_id,
                connector_id,
                format!("workspace_{suffix}"),
                format!("channel_{suffix}"),
                format!("message_{suffix}"),
                format!("event_{suffix}"),
                "accepted",
                "accepted",
                ts,
                ts,
                "redacted",
                event_document
            ],
        )?;
    }
    Ok(fixture)
}

/// Counts rows per r51 table (Go CountR51SlackChannelConnectorRows).
pub fn count_r51_slack_channel_connector_rows(
    store: &SQLiteStore,
) -> Result<HashMap<String, i64>, String> {
    let conn = crate::open_fixture_connection(store.db_path())?;
    let mut counts = HashMap::new();
    for table in R51_SLACK_CHANNEL_CONNECTOR_TABLE_NAMES {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .map_err(|e| format!("count {table}: {e}"))?;
        counts.insert(table.to_string(), count);
    }
    Ok(counts)
}
