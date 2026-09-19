//! Telegram connector setup-validation event (port of `connector_telegram.go`).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::util::payload;
use crate::{Event, Resource, Scope};

/// Go: `ConnectorTelegramSetupValidatedInput`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorTelegramSetupValidatedInput {
    pub tenant_id: String,
    pub connector_id: String,
    pub terminal_state: String,
    pub hosted_ready: bool,
    pub credential_state: String,
    pub allowment_state: String,
    pub reason_code: String,
    pub redaction_status: String,
    pub validated_at: DateTime<Utc>,
}

/// Go: `ConnectorTelegramSetupValidated`.
#[must_use]
pub fn connector_telegram_setup_validated(input: ConnectorTelegramSetupValidatedInput) -> Event {
    Event {
        category: "connector".to_string(),
        name: "connector.telegram_setup_validated".to_string(),
        scope: Scope {
            connector_id: input.connector_id.clone(),
            ..Scope::default()
        },
        resource: Resource {
            kind: "telegram_hosted_setup".to_string(),
            id: input.connector_id.clone(),
        },
        payload: payload![
            "tenantId" => input.tenant_id,
            "connectorId" => input.connector_id,
            "terminalState" => input.terminal_state,
            "hostedReady" => input.hosted_ready,
            "credentialState" => input.credential_state,
            "allowmentState" => input.allowment_state,
            "reasonCode" => input.reason_code,
            "redactionStatus" => input.redaction_status,
            "validatedAt" => input.validated_at,
        ],
        ..Event::default()
    }
}
