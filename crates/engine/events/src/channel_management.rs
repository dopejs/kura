//! Channel-management evidence events (port of `channel_management.go`).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::connectors::{
    CONNECTOR_EVENT_MANAGEMENT_REDACTION_FAILED, CONNECTOR_EVENT_MANAGEMENT_RETENTION_APPLIED,
    CONNECTOR_EVENT_SUPPORT_EVIDENCE_GENERATED,
};
use crate::util::{is_go_zero_time, now_utc, payload};
use crate::{Event, Resource, Scope};

/// Go: `ConnectorManagementEventInput`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorManagementEventInput {
    pub tenant_id: String,
    pub connector_id: String,
    pub evidence_id: String,
    pub action: String,
    pub outcome: String,
    pub reason_code: String,
    pub redaction_status: String,
    pub occurred_at: DateTime<Utc>,
}

/// Go: `ConnectorManagementSupportEvidenceGenerated`.
#[must_use]
pub fn connector_management_support_evidence_generated(
    input: ConnectorManagementEventInput,
) -> Event {
    connector_management_event(
        CONNECTOR_EVENT_SUPPORT_EVIDENCE_GENERATED,
        "channel_support_evidence",
        input,
    )
}

/// Go: `ConnectorManagementRedactionFailed`.
#[must_use]
pub fn connector_management_redaction_failed(input: ConnectorManagementEventInput) -> Event {
    connector_management_event(
        CONNECTOR_EVENT_MANAGEMENT_REDACTION_FAILED,
        "channel_management_redaction_failure",
        input,
    )
}

/// Go: `ConnectorManagementRetentionApplied`.
#[must_use]
pub fn connector_management_retention_applied(input: ConnectorManagementEventInput) -> Event {
    connector_management_event(
        CONNECTOR_EVENT_MANAGEMENT_RETENTION_APPLIED,
        "channel_management_retention",
        input,
    )
}

/// Go: `connectorManagementEvent` — action/outcome default to the event name
/// and `"succeeded"` respectively when left empty.
fn connector_management_event(
    name: &str,
    resource_kind: &str,
    input: ConnectorManagementEventInput,
) -> Event {
    let occurred_at = if is_go_zero_time(input.occurred_at) {
        now_utc()
    } else {
        input.occurred_at
    };
    let action = first_non_empty(&[input.action.as_str(), name]);
    let outcome = first_non_empty(&[input.outcome.as_str(), "succeeded"]);
    Event {
        tenant_id: input.tenant_id.clone(),
        category: "connector".to_string(),
        name: name.to_string(),
        occurred_at,
        scope: Scope {
            connector_id: input.connector_id.clone(),
            ..Scope::default()
        },
        resource: Resource {
            kind: resource_kind.to_string(),
            id: input.evidence_id.clone(),
        },
        payload: payload![
            "tenantId" => input.tenant_id,
            "connectorId" => input.connector_id,
            "evidenceId" => input.evidence_id,
            "action" => action,
            "outcome" => outcome,
            "reasonCode" => input.reason_code,
            "redactionStatus" => input.redaction_status,
        ],
        ..Event::default()
    }
}

/// Go: `firstNonEmptyConnectorManagement`.
fn first_non_empty(items: &[&str]) -> String {
    for item in items {
        if !item.is_empty() {
            return (*item).to_string();
        }
    }
    String::new()
}
