//! Matrix connector setup-validation, route-outcome, and smoke-evidence
//! events (port of `connector_matrix.go`).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::connectors::{
    CONNECTOR_EVENT_MATRIX_SETUP_VALIDATED, CONNECTOR_EVENT_MATRIX_SMOKE_EVIDENCE_RECORDED,
    CONNECTOR_EVENT_ROUTE_OUTCOME_RECORDED,
};
use crate::util::payload;
use crate::{Event, Resource, Scope};

/// Go: `ConnectorMatrixSetupValidatedInput`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorMatrixSetupValidatedInput {
    pub tenant_id: String,
    pub connector_id: String,
    pub homeserver_binding_id: String,
    pub terminal_state: String,
    pub bot_credential_state: String,
    pub homeserver_state: String,
    pub route_policy_state: String,
    pub delivery_eligible: bool,
    pub reason_code: String,
    pub matrix_condition: String,
    pub redaction_status: String,
    pub validated_at: DateTime<Utc>,
}

/// Go: `ConnectorMatrixRouteOutcomeRecordedInput`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorMatrixRouteOutcomeRecordedInput {
    pub tenant_id: String,
    pub connector_id: String,
    pub homeserver_id: String,
    pub conversation_id: String,
    pub matrix_event_id: String,
    pub sync_batch_id: String,
    pub transaction_id: String,
    pub outcome: String,
    pub reason_code: String,
    pub surface: String,
    pub redaction_status: String,
}

/// Go: `ConnectorMatrixSmokeEvidenceRecordedInput`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorMatrixSmokeEvidenceRecordedInput {
    pub tenant_id: String,
    pub connector_id: String,
    pub smoke_evidence_id: String,
    pub homeserver_binding_id: String,
    pub status: String,
    pub authorization_mode: String,
    pub owner: String,
    pub reason: String,
    pub redaction_status: String,
    pub validated_at: DateTime<Utc>,
    pub retention_expires_at: DateTime<Utc>,
}

/// Go: `ConnectorMatrixSetupValidated`.
#[must_use]
pub fn connector_matrix_setup_validated(input: ConnectorMatrixSetupValidatedInput) -> Event {
    Event {
        category: "connector".to_string(),
        name: CONNECTOR_EVENT_MATRIX_SETUP_VALIDATED.to_string(),
        scope: Scope {
            connector_id: input.connector_id.clone(),
            ..Scope::default()
        },
        resource: Resource {
            kind: "matrix_hosted_setup".to_string(),
            id: input.connector_id.clone(),
        },
        payload: payload![
            "tenantId" => input.tenant_id,
            "connectorId" => input.connector_id,
            "homeserverBindingId" => input.homeserver_binding_id,
            "terminalState" => input.terminal_state,
            "botCredentialState" => input.bot_credential_state,
            "homeserverState" => input.homeserver_state,
            "routePolicyState" => input.route_policy_state,
            "deliveryEligible" => input.delivery_eligible,
            "reasonCode" => input.reason_code,
            "matrixCondition" => input.matrix_condition,
            "redactionStatus" => input.redaction_status,
            "validatedAt" => input.validated_at,
        ],
        ..Event::default()
    }
}

/// Go: `ConnectorMatrixRouteOutcomeRecorded`.
#[must_use]
pub fn connector_matrix_route_outcome_recorded(
    input: ConnectorMatrixRouteOutcomeRecordedInput,
) -> Event {
    Event {
        category: "connector".to_string(),
        name: CONNECTOR_EVENT_ROUTE_OUTCOME_RECORDED.to_string(),
        scope: Scope {
            connector_id: input.connector_id.clone(),
            ..Scope::default()
        },
        resource: Resource {
            kind: "connector_route_outcome".to_string(),
            id: input.matrix_event_id.clone(),
        },
        payload: payload![
            "tenantId" => input.tenant_id,
            "connectorId" => input.connector_id,
            "homeserverId" => input.homeserver_id,
            "conversationId" => input.conversation_id,
            "matrixEventId" => input.matrix_event_id,
            "syncBatchId" => input.sync_batch_id,
            "transactionId" => input.transaction_id,
            "outcome" => input.outcome,
            "reasonCode" => input.reason_code,
            "surface" => input.surface,
            "redactionStatus" => input.redaction_status,
        ],
        ..Event::default()
    }
}

/// Go: `ConnectorMatrixSmokeEvidenceRecorded`.
#[must_use]
pub fn connector_matrix_smoke_evidence_recorded(
    input: ConnectorMatrixSmokeEvidenceRecordedInput,
) -> Event {
    Event {
        tenant_id: input.tenant_id.clone(),
        category: "connector".to_string(),
        name: CONNECTOR_EVENT_MATRIX_SMOKE_EVIDENCE_RECORDED.to_string(),
        scope: Scope {
            connector_id: input.connector_id.clone(),
            ..Scope::default()
        },
        resource: Resource {
            kind: "matrix_smoke_evidence".to_string(),
            id: input.smoke_evidence_id.clone(),
        },
        payload: payload![
            "tenantId" => input.tenant_id,
            "connectorId" => input.connector_id,
            "smokeEvidenceId" => input.smoke_evidence_id,
            "homeserverBindingId" => input.homeserver_binding_id,
            "status" => input.status,
            "authorizationMode" => input.authorization_mode,
            "owner" => input.owner,
            "reason" => input.reason,
            "redactionStatus" => input.redaction_status,
            "validatedAt" => input.validated_at,
            "retentionExpiresAt" => input.retention_expires_at,
        ],
        ..Event::default()
    }
}
