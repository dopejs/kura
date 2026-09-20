//! Connector foreground-reply failure and delivery-separation events
//! (port of `connector_delivery.go`).

use serde::{Deserialize, Serialize};

use crate::util::payload;
use crate::{Event, Resource, Scope};

/// Go: `ConnectorForegroundReplyFailedInput`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorForegroundReplyFailedInput {
    pub tenant_id: String,
    pub connector_id: String,
    pub message_delivery_id: String,
    pub reason_code: String,
    pub retry_safety: String,
    pub background_delivery_id: String,
    pub separation_status: String,
}

/// Go: `ConnectorForegroundReplyFailed`.
#[must_use]
pub fn connector_foreground_reply_failed(input: ConnectorForegroundReplyFailedInput) -> Event {
    Event {
        tenant_id: input.tenant_id.clone(),
        category: "connector".to_string(),
        name: "connector.foreground_reply_failed".to_string(),
        scope: Scope {
            connector_id: input.connector_id.clone(),
            ..Scope::default()
        },
        resource: Resource {
            kind: "connector_foreground_reply".to_string(),
            id: input.message_delivery_id.clone(),
        },
        payload: payload![
            "tenantId" => input.tenant_id,
            "connectorId" => input.connector_id,
            "messageDeliveryId" => input.message_delivery_id,
            "status" => "failed",
            "reasonCode" => input.reason_code,
            "retrySafety" => input.retry_safety,
            "backgroundDeliveryId" => input.background_delivery_id,
            "separationStatus" => input.separation_status,
            "redactionStatus" => "redacted",
        ],
        ..Event::default()
    }
}

/// Go: `ConnectorDeliverySeparationInput`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorDeliverySeparationInput {
    pub tenant_id: String,
    pub connector_id: String,
    pub boundary_id: String,
    pub foreground_reply_outcome_id: String,
    pub background_delivery_id: String,
    pub transport_kind: String,
    pub separation_status: String,
}

/// Go: `ConnectorDeliverySeparationRecorded`.
#[must_use]
pub fn connector_delivery_separation_recorded(input: ConnectorDeliverySeparationInput) -> Event {
    Event {
        tenant_id: input.tenant_id.clone(),
        category: "connector".to_string(),
        name: "connector.delivery_separation_recorded".to_string(),
        scope: Scope {
            connector_id: input.connector_id.clone(),
            ..Scope::default()
        },
        resource: Resource {
            kind: "connector_delivery_boundary".to_string(),
            id: input.boundary_id.clone(),
        },
        payload: payload![
            "tenantId" => input.tenant_id,
            "connectorId" => input.connector_id,
            "foregroundReplyOutcomeId" => input.foreground_reply_outcome_id,
            "backgroundDeliveryId" => input.background_delivery_id,
            "transportKind" => input.transport_kind,
            "separationStatus" => input.separation_status,
            "redactionStatus" => "redacted",
        ],
        ..Event::default()
    }
}
