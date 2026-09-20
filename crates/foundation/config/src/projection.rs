//! Hosted-readiness projections for local connector configs.
//!
//! Each connector config projects into a redacted, token-free readiness view
//! consumed by the hosted-control-plane surface (Go `ProjectHostedReadiness`
//! methods).

use serde::{Deserialize, Serialize};

use crate::types::{
    DiscordConnectorConfig, MatrixConnectorConfig, SlackConnectorConfig, TelegramConnectorConfig,
};

/// Redacted hosted-readiness view of the Slack connector config.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlackHostedReadinessProjection {
    /// Tenant the projection was built for.
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub tenant_id: String,
    /// Connector identifier.
    pub connector_id: String,
    /// Human-readable connector name.
    pub display_name: String,
    /// Terminal readiness state (`action-required` or `cancelled`).
    pub terminal_state: String,
    /// Whether the connector is hosted-ready (always false locally).
    pub hosted_ready: bool,
    /// Whether the local config is sufficient for local operation.
    pub local_compatible: bool,
    /// Machine-readable reason for the terminal state.
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub reason_code: String,
    /// Hosted workspace binding identifier.
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub workspace_binding_id: String,
    /// Slack workspace (team) ID.
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub workspace_id: String,
    /// Bot user ID inside the workspace.
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub bot_user_id: String,
    /// Allowlisted channel IDs.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub allowed_channel_ids: Vec<String>,
    /// Allowlisted DM user IDs.
    #[serde(
        rename = "allowedDMUserIds",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub allowed_dm_user_ids: Vec<String>,
    /// Allowlisted DM user groups.
    #[serde(
        rename = "allowedDMUserGroups",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub allowed_dm_user_groups: Vec<String>,
    /// Redaction marker; always `redacted`.
    pub redaction_status: String,
}

impl SlackConnectorConfig {
    /// Project this local Slack config into the hosted-readiness view
    /// (Go `SlackConnectorConfig.ProjectHostedReadiness`).
    pub fn project_hosted_readiness(&self, tenant_id: &str) -> SlackHostedReadinessProjection {
        let mut projection = SlackHostedReadinessProjection {
            tenant_id: tenant_id.trim().to_string(),
            connector_id: self.connector_id.trim().to_string(),
            display_name: self.display_name.trim().to_string(),
            terminal_state: "action-required".to_string(),
            hosted_ready: false,
            local_compatible: self.enabled && !self.workspace_id.trim().is_empty(),
            reason_code: String::new(),
            workspace_binding_id: self.workspace_binding_id.trim().to_string(),
            workspace_id: self.workspace_id.trim().to_string(),
            bot_user_id: self.bot_user_id.trim().to_string(),
            allowed_channel_ids: self.allowed_channel_ids.clone(),
            allowed_dm_user_ids: self.allowed_dm_user_ids.clone(),
            allowed_dm_user_groups: self.allowed_dm_user_groups.clone(),
            redaction_status: "redacted".to_string(),
        };
        if !projection.local_compatible {
            if !self.enabled {
                projection.terminal_state = "cancelled".to_string();
                projection.reason_code = "disabled".to_string();
                return projection;
            }
            projection.reason_code = "slack_workspace_binding_missing".to_string();
            return projection;
        }
        if self.allowed_channel_ids.is_empty()
            && self.allowed_dm_user_ids.is_empty()
            && self.allowed_dm_user_groups.is_empty()
        {
            projection.reason_code = "slack_route_policy_missing".to_string();
            return projection;
        }
        projection.reason_code = "slack_route_policy_validation_required".to_string();
        projection
    }
}

/// Redacted hosted-readiness view of the Matrix connector config.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatrixHostedReadinessProjection {
    /// Tenant the projection was built for.
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub tenant_id: String,
    /// Connector identifier.
    pub connector_id: String,
    /// Human-readable connector name.
    pub display_name: String,
    /// Terminal readiness state (`action-required` or `cancelled`).
    pub terminal_state: String,
    /// Whether the connector is hosted-ready (always false locally).
    pub hosted_ready: bool,
    /// Whether the local config is sufficient for local operation.
    pub local_compatible: bool,
    /// Machine-readable reason for the terminal state.
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub reason_code: String,
    /// Homeserver base URL.
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub homeserver_url: String,
    /// Homeserver identifier (server name).
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub homeserver_id: String,
    /// Bot user ID.
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub bot_user_id: String,
    /// Whether a bot access token is configured (never exposes the token).
    pub bot_access_token_set: bool,
    /// Room IDs the bot operates in.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub selected_room_ids: Vec<String>,
    /// Allowlisted direct-message user IDs.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub allowed_direct_user_ids: Vec<String>,
    /// Configured bot command prefixes.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub configured_commands: Vec<String>,
    /// Hosted homeserver policy; always `unsupported`.
    pub hosted_homeserver_policy: String,
    /// Redaction marker; always `redacted`.
    pub redaction_status: String,
}

impl MatrixConnectorConfig {
    /// Project this local Matrix config into the hosted-readiness view
    /// (Go `MatrixConnectorConfig.ProjectHostedReadiness`).
    pub fn project_hosted_readiness(&self, tenant_id: &str) -> MatrixHostedReadinessProjection {
        let mut projection = MatrixHostedReadinessProjection {
            tenant_id: tenant_id.trim().to_string(),
            connector_id: self.connector_id.trim().to_string(),
            display_name: self.display_name.trim().to_string(),
            terminal_state: "action-required".to_string(),
            hosted_ready: false,
            local_compatible: self.enabled
                && !self.homeserver_url.trim().is_empty()
                && !self.bot_access_token.trim().is_empty()
                && !self.bot_user_id.trim().is_empty(),
            reason_code: String::new(),
            homeserver_url: self.homeserver_url.trim().to_string(),
            homeserver_id: self.homeserver_id.trim().to_string(),
            bot_user_id: self.bot_user_id.trim().to_string(),
            bot_access_token_set: !self.bot_access_token.trim().is_empty(),
            selected_room_ids: self.selected_room_ids.clone(),
            allowed_direct_user_ids: self.allowed_direct_user_ids.clone(),
            configured_commands: self.configured_commands.clone(),
            hosted_homeserver_policy: "unsupported".to_string(),
            redaction_status: "redacted".to_string(),
        };
        if !self.enabled {
            projection.terminal_state = "cancelled".to_string();
            projection.reason_code = "disabled".to_string();
            return projection;
        }
        if self.homeserver_url.trim().is_empty() {
            projection.reason_code = "matrix_homeserver_missing".to_string();
            return projection;
        }
        if self.bot_access_token.trim().is_empty() || self.bot_user_id.trim().is_empty() {
            projection.reason_code = "matrix_bot_credential_missing".to_string();
            return projection;
        }
        if self.selected_room_ids.is_empty() && self.allowed_direct_user_ids.is_empty() {
            projection.reason_code = "matrix_route_policy_missing".to_string();
            return projection;
        }
        projection.reason_code = "matrix_route_policy_validation_required".to_string();
        projection
    }
}

/// Redacted hosted-readiness view of the Telegram connector config.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramHostedReadinessProjection {
    /// Tenant the projection was built for.
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub tenant_id: String,
    /// Connector identifier.
    pub connector_id: String,
    /// Human-readable connector name.
    pub display_name: String,
    /// Terminal readiness state (`action-required` or `cancelled`).
    pub terminal_state: String,
    /// Whether the connector is hosted-ready (always false locally).
    pub hosted_ready: bool,
    /// Whether the local config is sufficient for local operation.
    pub local_compatible: bool,
    /// Machine-readable reason for the terminal state.
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub reason_code: String,
    /// Whether a bot token is configured (never exposes the token).
    pub bot_token_configured: bool,
    /// Bot username (without `@`).
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub bot_username: String,
    /// Allowlisted user IDs.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub allowed_user_ids: Vec<String>,
    /// Allowlisted direct-chat IDs.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub allowed_direct_chat_ids: Vec<String>,
    /// Allowlisted group IDs.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub allowed_group_ids: Vec<String>,
    /// Redaction marker; always `redacted`.
    pub redaction_status: String,
}

impl TelegramConnectorConfig {
    /// Project this local Telegram config into the hosted-readiness view
    /// (Go `TelegramConnectorConfig.ProjectHostedReadiness`).
    pub fn project_hosted_readiness(&self, tenant_id: &str) -> TelegramHostedReadinessProjection {
        let mut projection = TelegramHostedReadinessProjection {
            tenant_id: tenant_id.trim().to_string(),
            connector_id: self.connector_id.trim().to_string(),
            display_name: self.display_name.trim().to_string(),
            terminal_state: "action-required".to_string(),
            hosted_ready: false,
            local_compatible: self.enabled && !self.bot_token.trim().is_empty(),
            reason_code: String::new(),
            bot_token_configured: !self.bot_token.trim().is_empty(),
            bot_username: self.bot_username.trim().to_string(),
            allowed_user_ids: self.allowed_user_ids.clone(),
            allowed_direct_chat_ids: self.allowed_direct_chat_ids.clone(),
            allowed_group_ids: self.allowed_group_ids.clone(),
            redaction_status: "redacted".to_string(),
        };
        if !self.enabled {
            projection.terminal_state = "cancelled".to_string();
            projection.reason_code = "disabled".to_string();
            return projection;
        }
        if self.bot_token.trim().is_empty() {
            projection.reason_code = "auth_missing".to_string();
            return projection;
        }
        if self.allowed_user_ids.is_empty()
            && self.allowed_direct_chat_ids.is_empty()
            && self.allowed_group_ids.is_empty()
        {
            projection.reason_code = "telegram_allowment_missing".to_string();
            return projection;
        }
        projection.reason_code = "telegram_allowment_validation_required".to_string();
        projection
    }
}

/// Redacted hosted-readiness view of the Discord connector config.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscordHostedReadinessProjection {
    /// Tenant the projection was built for.
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub tenant_id: String,
    /// Connector identifier.
    pub connector_id: String,
    /// Human-readable connector name.
    pub display_name: String,
    /// Effective delivery mode.
    pub delivery_mode: String,
    /// Readiness state (`disabled`, `failed`, or `degraded_needs_repair`).
    pub readiness_state: String,
    /// Whether the connector is hosted-ready (always false locally).
    pub hosted_ready: bool,
    /// Whether the local config is sufficient for local operation.
    pub local_compatible: bool,
    /// Machine-readable reason for the readiness state.
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub reason_code: String,
    /// Whether guild messages must mention the bot.
    pub require_mention: bool,
    /// Whether the bot responds in direct messages.
    #[serde(rename = "respondInDM")]
    pub respond_in_dm: bool,
    /// Allowlisted guild IDs.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub allowed_guild_ids: Vec<String>,
    /// Allowlisted channel IDs.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub allowed_channel_ids: Vec<String>,
    /// Whether a bot token is configured (never exposes the token).
    pub bot_token_configured: bool,
    /// Token material is never serialized (Go `json:"-"`).
    #[serde(skip)]
    pub bot_token: String,
    /// Redaction marker; always `redacted`.
    pub redaction_status: String,
}

impl DiscordConnectorConfig {
    /// Project this local Discord config into the hosted-readiness view
    /// (Go `DiscordConnectorConfig.ProjectHostedReadiness`).
    pub fn project_hosted_readiness(&self, tenant_id: &str) -> DiscordHostedReadinessProjection {
        let mode = {
            let trimmed = self.delivery_mode.trim();
            if trimmed.is_empty() {
                "gateway"
            } else {
                trimmed
            }
        };
        let mut projection = DiscordHostedReadinessProjection {
            tenant_id: tenant_id.trim().to_string(),
            connector_id: self.connector_id.trim().to_string(),
            display_name: self.display_name.trim().to_string(),
            delivery_mode: mode.to_string(),
            readiness_state: "degraded_needs_repair".to_string(),
            hosted_ready: false,
            local_compatible: self.enabled && !self.bot_token.trim().is_empty(),
            reason_code: String::new(),
            require_mention: self.require_mention,
            respond_in_dm: self.respond_in_dm,
            allowed_guild_ids: self.allowed_guild_ids.clone(),
            allowed_channel_ids: self.allowed_channel_ids.clone(),
            bot_token_configured: !self.bot_token.trim().is_empty(),
            bot_token: String::new(),
            redaction_status: "redacted".to_string(),
        };
        if !self.enabled {
            projection.readiness_state = "disabled".to_string();
            projection.hosted_ready = false;
            projection.reason_code = "disabled".to_string();
            return projection;
        }
        if self.bot_token.trim().is_empty() {
            projection.readiness_state = "failed".to_string();
            projection.hosted_ready = false;
            projection.reason_code = "auth_missing".to_string();
            return projection;
        }
        if self.allowed_guild_ids.is_empty() && self.allowed_channel_ids.is_empty() {
            projection.readiness_state = "degraded_needs_repair".to_string();
            projection.hosted_ready = false;
            projection.reason_code = "missing_explicit_destination".to_string();
            return projection;
        }
        projection.reason_code = "destination_validation_required".to_string();
        projection
    }
}
