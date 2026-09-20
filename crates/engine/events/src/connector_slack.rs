//! Slack connector setup-validation and route-outcome events (port of
//! `connector_slack.go`).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::util::payload;
use crate::{Event, Resource, Scope};

/// Go: `ConnectorSlackSetupValidatedInput`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorSlackSetupValidatedInput {
    pub tenant_id: String,
    pub connector_id: String,
    pub workspace_binding_id: String,
    pub terminal_state: String,
    pub oauth_state: String,
    pub route_policy_state: String,
    pub delivery_eligible: bool,
    pub reason_code: String,
    pub slack_condition: String,
    pub redaction_status: String,
    pub validated_at: DateTime<Utc>,
}

/// Go: `ConnectorSlackRouteOutcomeRecordedInput`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorSlackRouteOutcomeRecordedInput {
    pub tenant_id: String,
    pub connector_id: String,
    pub workspace_id: String,
    pub conversation_id: String,
    pub message_id: String,
    pub event_id: String,
    pub outcome: String,
    pub reason_code: String,
    pub surface: String,
    pub redaction_status: String,
}

/// Go: `ConnectorSlackSetupValidated`.
#[must_use]
pub fn connector_slack_setup_validated(input: ConnectorSlackSetupValidatedInput) -> Event {
    Event {
        category: "connector".to_string(),
        name: "connector.slack_setup_validated".to_string(),
        scope: Scope {
            connector_id: input.connector_id.clone(),
            ..Scope::default()
        },
        resource: Resource {
            kind: "slack_hosted_setup".to_string(),
            id: input.connector_id.clone(),
        },
        payload: payload![
            "tenantId" => input.tenant_id,
            "connectorId" => input.connector_id,
            "workspaceBindingId" => input.workspace_binding_id,
            "terminalState" => input.terminal_state,
            "oauthState" => input.oauth_state,
            "routePolicyState" => input.route_policy_state,
            "deliveryEligible" => input.delivery_eligible,
            "reasonCode" => input.reason_code,
            "slackCondition" => input.slack_condition,
            "redactionStatus" => input.redaction_status,
            "validatedAt" => input.validated_at,
        ],
        ..Event::default()
    }
}

/// Go: `ConnectorSlackRouteOutcomeRecorded` — shares the connector
/// route-outcome contract with Matrix.
#[must_use]
pub fn connector_slack_route_outcome_recorded(
    input: ConnectorSlackRouteOutcomeRecordedInput,
) -> Event {
    Event {
        category: "connector".to_string(),
        name: "connector.route_outcome_recorded".to_string(),
        scope: Scope {
            connector_id: input.connector_id.clone(),
            ..Scope::default()
        },
        resource: Resource {
            kind: "connector_route_outcome".to_string(),
            id: input.message_id.clone(),
        },
        payload: payload![
            "tenantId" => input.tenant_id,
            "connectorId" => input.connector_id,
            "workspaceId" => input.workspace_id,
            "conversationId" => input.conversation_id,
            "messageId" => input.message_id,
            "eventId" => input.event_id,
            "outcome" => input.outcome,
            "reasonCode" => input.reason_code,
            "surface" => input.surface,
            "redactionStatus" => input.redaction_status,
        ],
        ..Event::default()
    }
}
