//! Connector diagnostic state-change and redaction-failure events
//! (port of `connector_diagnostics.go`).

use serde::{Deserialize, Serialize};

use crate::util::payload;
use crate::{Event, Resource, Scope};

/// Go: `ConnectorDiagnosticStateChangedInput`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorDiagnosticStateChangedInput {
    pub tenant_id: String,
    pub connector_id: String,
    pub diagnostic_state_id: String,
    pub previous_status: String,
    pub status: String,
    pub reason_code: String,
    pub remediation_owner: String,
    pub retry_safety: String,
    pub freshness_state: String,
    pub redaction_status: String,
}

/// Go: `ConnectorDiagnosticStateChanged`.
#[must_use]
pub fn connector_diagnostic_state_changed(input: ConnectorDiagnosticStateChangedInput) -> Event {
    Event {
        tenant_id: input.tenant_id.clone(),
        category: "connector".to_string(),
        name: "connector.diagnostic_state_changed".to_string(),
        scope: Scope {
            connector_id: input.connector_id.clone(),
            ..Scope::default()
        },
        resource: Resource {
            kind: "connector_diagnostic_state".to_string(),
            id: input.diagnostic_state_id.clone(),
        },
        payload: payload![
            "tenantId" => input.tenant_id,
            "diagnosticStateId" => input.diagnostic_state_id,
            "connectorId" => input.connector_id,
            "previousStatus" => input.previous_status,
            "status" => input.status,
            "reasonCode" => input.reason_code,
            "remediationOwner" => input.remediation_owner,
            "retrySafety" => input.retry_safety,
            "freshnessState" => input.freshness_state,
            "redactionStatus" => input.redaction_status,
        ],
        ..Event::default()
    }
}

/// Go: `ConnectorDiagnosticRedactionFailedInput`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorDiagnosticRedactionFailedInput {
    pub tenant_id: String,
    pub connector_id: String,
    pub redaction_failure_id: String,
    pub target_kind: String,
    pub target_id: String,
    pub reason_code: String,
    pub retention_expires_at: String,
}

/// Go: `ConnectorDiagnosticRedactionFailed`.
#[must_use]
pub fn connector_diagnostic_redaction_failed(
    input: ConnectorDiagnosticRedactionFailedInput,
) -> Event {
    Event {
        tenant_id: input.tenant_id.clone(),
        category: "connector".to_string(),
        name: "connector.diagnostic_redaction_failed".to_string(),
        scope: Scope {
            connector_id: input.connector_id.clone(),
            ..Scope::default()
        },
        resource: Resource {
            kind: "connector_diagnostic_redaction_failure".to_string(),
            id: input.redaction_failure_id.clone(),
        },
        payload: payload![
            "tenantId" => input.tenant_id,
            "connectorId" => input.connector_id,
            "targetKind" => input.target_kind,
            "targetId" => input.target_id,
            "reasonCode" => input.reason_code,
            "redactionStatus" => "suppressed",
            "retentionExpiresAt" => input.retention_expires_at,
        ],
        ..Event::default()
    }
}
