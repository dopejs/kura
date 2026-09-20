//! Discord connector setup-validation event (port of `connector_discord.go`).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::util::payload;
use crate::{Event, Resource, Scope};

/// Go: `ConnectorDiscordSetupValidatedInput`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorDiscordSetupValidatedInput {
    pub tenant_id: String,
    pub connector_id: String,
    pub readiness_state: String,
    pub hosted_ready: bool,
    pub credential_state: String,
    pub reason_code: String,
    pub redaction_status: String,
    pub validated_at: DateTime<Utc>,
}

/// Go: `ConnectorDiscordSetupValidated`.
#[must_use]
pub fn connector_discord_setup_validated(input: ConnectorDiscordSetupValidatedInput) -> Event {
    Event {
        category: "connector".to_string(),
        name: "connector.discord_setup_validated".to_string(),
        scope: Scope {
            connector_id: input.connector_id.clone(),
            ..Scope::default()
        },
        resource: Resource {
            kind: "discord_hosted_setup".to_string(),
            id: input.connector_id.clone(),
        },
        payload: payload![
            "tenantId" => input.tenant_id,
            "connectorId" => input.connector_id,
            "readinessState" => input.readiness_state,
            "hostedReady" => input.hosted_ready,
            "credentialState" => input.credential_state,
            "reasonCode" => input.reason_code,
            "redactionStatus" => input.redaction_status,
            "validatedAt" => input.validated_at,
        ],
        ..Event::default()
    }
}
