//! Connector inbound-duplicate and route-outcome events (port of
//! `connector_ingress.go`).

use serde::{Deserialize, Serialize};

use crate::util::payload;
use crate::{Event, Resource, Scope};

/// Go: `ConnectorInboundDuplicateInput`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorInboundDuplicateInput {
    pub tenant_id: String,
    pub connector_id: String,
    pub connector_account_id: String,
    pub channel_or_conversation_id: String,
    pub provider_message_id: String,
    pub equivalent_rule_id: String,
    pub existing_delivery_id: String,
}

/// Go: `ConnectorInboundDuplicateDetected`.
#[must_use]
pub fn connector_inbound_duplicate_detected(input: ConnectorInboundDuplicateInput) -> Event {
    Event {
        tenant_id: input.tenant_id.clone(),
        category: "connector".to_string(),
        name: "connector.inbound_duplicate_detected".to_string(),
        scope: Scope {
            connector_id: input.connector_id.clone(),
            ..Scope::default()
        },
        resource: Resource {
            kind: "connector_message".to_string(),
            id: input.existing_delivery_id.clone(),
        },
        payload: payload![
            "tenantId" => input.tenant_id,
            "connectorId" => input.connector_id,
            "connectorAccountId" => input.connector_account_id,
            "channelOrConversationId" => input.channel_or_conversation_id,
            "providerMessageId" => input.provider_message_id,
            "equivalentRuleId" => input.equivalent_rule_id,
            "existingDeliveryId" => input.existing_delivery_id,
            "redactionStatus" => "redacted",
        ],
        ..Event::default()
    }
}

/// Go: `ConnectorRouteOutcomeInput`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorRouteOutcomeInput {
    pub tenant_id: String,
    pub connector_id: String,
    pub outcome: String,
    pub reason_code: String,
    pub surface: String,
    pub message_delivery_id: String,
    pub connector_account_id: String,
    pub channel_or_conversation_id: String,
    pub provider_message_id: String,
    pub equivalent_rule_id: String,
}

/// Go: `ConnectorRouteOutcomeRecorded` — the resource id falls back to the
/// connector id when no message delivery id is present.
#[must_use]
pub fn connector_route_outcome_recorded(input: ConnectorRouteOutcomeInput) -> Event {
    let resource_id = if input.message_delivery_id.is_empty() {
        input.connector_id.clone()
    } else {
        input.message_delivery_id.clone()
    };
    Event {
        tenant_id: input.tenant_id.clone(),
        category: "connector".to_string(),
        name: "connector.route_outcome_recorded".to_string(),
        scope: Scope {
            connector_id: input.connector_id.clone(),
            ..Scope::default()
        },
        resource: Resource {
            kind: "connector_route_outcome".to_string(),
            id: resource_id,
        },
        payload: payload![
            "tenantId" => input.tenant_id,
            "connectorId" => input.connector_id,
            "outcome" => input.outcome,
            "reasonCode" => input.reason_code,
            "surface" => input.surface,
            "messageDeliveryId" => input.message_delivery_id,
            "connectorAccountId" => input.connector_account_id,
            "channelOrConversationId" => input.channel_or_conversation_id,
            "providerMessageId" => input.provider_message_id,
            "equivalentRuleId" => input.equivalent_rule_id,
            "redactionStatus" => "redacted",
        ],
        ..Event::default()
    }
}
