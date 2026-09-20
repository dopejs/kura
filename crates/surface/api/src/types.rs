//! Shared request/response DTO vocabulary — a faithful port of
//! daemon/internal/api/types.go. Every route family in this crate speaks these
//! types; wire shape follows the Go json tags: camelCase + omitempty mapped to
//! `skip_serializing_if` (empty string / empty vec / None / zero / false).
//!
//! Reminders DTO fields still use `serde_json::Value` placeholders where the
//! real `kura_reminders` types should be wired (type-migration follow-on; the
//! crate is ported but the DTO wiring is not yet switched over).

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use kura_calendar as calendar;
use kura_computeruse as computeruse;
use kura_config as config;
use kura_delivery as delivery;
use kura_events as events;
use kura_identity as identity;
use kura_integrations as integrations;
use kura_llm as llm;
use kura_mail as mail;
use kura_mcp as mcp;
use kura_orchestration as orchestration;
use kura_policy as policy;
use kura_providers as providers;
use kura_router as router;
use kura_runtime as runtime;
use kura_sandbox as sandbox;
use kura_scheduler as scheduler;
use kura_skills as skills;

// ---------------------------------------------------------------------------
// Serde helpers (Go omitempty equivalents)
// ---------------------------------------------------------------------------

/// `omitempty` for bool fields.
pub(crate) fn is_false(b: &bool) -> bool {
    !*b
}

/// `omitempty` for int fields (0 is omitted).
pub(crate) fn is_zero(n: &i64) -> bool {
    *n == 0
}

// ---------------------------------------------------------------------------
// System / config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemInfoResponse {
    pub service: String,
    pub environment: String,
    pub version: String,
    pub bind_addr: String,
    pub data_dir: String,
    pub log_level: String,
}

/// Go `buildSystemInfoResponse`.
#[must_use]
pub fn build_system_info_response(cfg: &config::Config) -> SystemInfoResponse {
    SystemInfoResponse {
        service: "kura".to_string(),
        environment: effective_environment(cfg),
        version: cfg.version.clone(),
        bind_addr: cfg.bind_addr.clone(),
        data_dir: cfg.data_dir.clone(),
        log_level: cfg.log_level.clone(),
    }
}

/// The deployment shape this daemon reports for itself. Unlike
/// `environment_scope`, which is a data-isolation label constrained to
/// `test|prod`, this names how the daemon is deployed — including `embedded`,
/// where a host application owns the process and supplies its data directory.
#[must_use]
pub fn effective_environment(cfg: &config::Config) -> String {
    match cfg.environment {
        config::Environment::Prod => "prod".to_string(),
        config::Environment::Test => "test".to_string(),
        config::Environment::Embedded => "embedded".to_string(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigResponse {
    pub environment: String,
    pub bind_addr: String,
    pub data_dir: String,
    pub config_file_path: String,
    pub log_level: String,
    pub version: String,
    pub llm: ConfigLlmResponse,
    pub connectors: ConfigConnectorsResponse,
    pub mcp: ConfigMcpResponse,
    pub sandbox: ConfigSandboxResponse,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redacted_fields: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigLlmResponse {
    pub default_provider: String,
    pub default_model: String,
    pub default_timeout_ms: i64,
    pub default_max_retries: i64,
    pub openai_compatible: ConfigOpenaiCompatibleProviderResponse,
    pub claude: ConfigManagedCliProviderResponse,
    pub codex: ConfigManagedCliProviderResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigOpenaiCompatibleProviderResponse {
    pub configured: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub base_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    pub timeout_ms: i64,
    pub stream_first_chunk_timeout_ms: i64,
    pub stream_idle_timeout_ms: i64,
    pub stream_max_duration_ms: i64,
    pub api_key_configured: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub api_key_env: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigManagedCliProviderResponse {
    pub configured: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cli_path: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub default_model: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub work_dir: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigConnectorsResponse {
    pub discord: ConfigDiscordConnectorResponse,
    pub telegram: ConfigTelegramConnectorResponse,
    pub slack: ConfigSlackConnectorResponse,
    pub matrix: ConfigMatrixConnectorResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigMcpResponse {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub servers: Vec<mcp::ServerResource>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub catalog: Vec<mcp::CatalogEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transports: Vec<mcp::TransportCapability>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSandboxResponse {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub backends: Vec<sandbox::BackendCapabilityProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigDiscordConnectorResponse {
    pub enabled: bool,
    pub configured: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub display_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub delivery_mode: String,
    pub require_mention: bool,
    pub respond_in_dm: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_guild_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_channel_ids: Vec<String>,
    pub bot_token_configured: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bot_token_env: String,
    pub hosted_readiness: config::DiscordHostedReadinessProjection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigTelegramConnectorResponse {
    pub enabled: bool,
    pub configured: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub display_name: String,
    pub bot_token_configured: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bot_token_env: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bot_username: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_user_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_direct_chat_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_group_ids: Vec<String>,
    pub hosted_readiness: config::TelegramHostedReadinessProjection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSlackConnectorResponse {
    pub enabled: bool,
    pub configured: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub display_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub api_base_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bot_token_secret_ref: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workspace_binding_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workspace_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bot_user_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_channel_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_dm_user_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_dm_user_groups: Vec<String>,
    pub hosted_readiness: config::SlackHostedReadinessProjection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigMatrixConnectorResponse {
    pub enabled: bool,
    pub configured: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub display_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub homeserver_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub homeserver_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bot_user_id: String,
    pub bot_access_token_set: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bot_access_token_env: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selected_room_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_direct_user_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub configured_commands: Vec<String>,
    pub hosted_readiness: config::MatrixHostedReadinessProjection,
}

// ---------------------------------------------------------------------------
// Integration diagnostics
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateIntegrationDiagnosticRunRequest {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "crate::types::is_false")]
    pub force_refresh: bool,
    pub client_key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationDiagnosticListResponse {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub integration_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub freshness_summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<integrations::DiagnosticResult>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub next_cursor: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationDiagnosticRunListResponse {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<integrations::DiagnosticRun>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub next_cursor: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateIntegrationDiagnosticSmokeRequest {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub report_id: String,
    pub integration_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub probes: Vec<CreateIntegrationDiagnosticSmokeProbe>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateIntegrationDiagnosticSmokeProbe {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub integration_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub domain_kind: String,
    pub probe_action: String,
    pub safe_credentials_available: bool,
    pub tenant_approval_available: bool,
    pub provider_available: bool,
    pub supported: bool,
    pub read_only_or_reversible: bool,
    #[serde(default, skip_serializing_if = "crate::types::is_false")]
    pub tenant_admin_approved: bool,
    #[serde(default, skip_serializing_if = "crate::types::is_false")]
    pub operator_approved: bool,
    #[serde(default, skip_serializing_if = "crate::types::is_false")]
    pub operator_deferred: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_evidence: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_refs: Vec<String>,
}

// ---------------------------------------------------------------------------
// Chat
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatQueryResponse {
    pub dispatch_id: String,
    pub provider: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skill_contracts: Vec<serde_json::Map<String, serde_json::Value>>,
    pub query: String,
    pub status: String,
    pub partial: bool,
    pub reply: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub finish_reason: String,
    pub usage: llm::Usage,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error_code: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_segment_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub request_turn_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub response_turn_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub continuity_preview_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuity_applied: Option<bool>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub continuity_status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuity_included_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuity_excluded_count: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatQueryStreamStarted {
    pub dispatch_id: String,
    pub provider: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skill_contracts: Vec<serde_json::Map<String, serde_json::Value>>,
    pub query: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_segment_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub request_turn_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub continuity_preview_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuity_applied: Option<bool>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub continuity_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatQueryStreamDelta {
    pub dispatch_id: String,
    pub delta: String,
    pub reply: String,
}

// ---------------------------------------------------------------------------
// Operator
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperatorOnboardingResponse {
    pub environment_scope: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub current_step_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub completed_step_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocking_item_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub optional_follow_up_item_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub recommended_action_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub readiness_items: Vec<OperatorReadinessItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub first_useful_actions: Vec<OperatorFirstUsefulAction>,
    pub last_evaluated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperatorReadinessItem {
    pub item_id: String,
    pub item_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub resource_id: String,
    pub display_name: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub health_state: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub diagnostic_freshness: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub remediation_owner: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub retry_safety: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub required_operator_action: String,
    pub required_for_selected_action: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail_route: String,
    pub environment_scope: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperatorFirstUsefulAction {
    pub action_id: String,
    pub action_kind: String,
    pub display_name: String,
    pub recommended: bool,
    pub available: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocking_item_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    pub invoke_route: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub result_route: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperatorResourceRef {
    pub kind: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub route: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperatorActivityRecord {
    pub activity_id: String,
    pub source_kind: String,
    pub source_id: String,
    pub title: String,
    pub status: String,
    pub summary: String,
    pub attention_level: String,
    pub occurred_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail_route: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related_resource_refs: Vec<OperatorResourceRef>,
    pub environment_scope: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperatorActivityListResponse {
    pub environment_scope: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<OperatorActivityRecord>,
    pub generated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperatorDiagnosticFinding {
    pub finding_id: String,
    pub source_kind: String,
    pub source_id: String,
    pub plane: String,
    pub severity: String,
    pub status: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub recommended_action: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail_route: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related_resource_refs: Vec<OperatorResourceRef>,
    pub environment_scope: String,
    pub captured_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperatorDiagnosticListResponse {
    pub environment_scope: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<OperatorDiagnosticFinding>,
    pub generated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Runs / workflows / sessions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRouteRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<router::SessionKind>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub channel: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub account_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub peer_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thread_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRunRequest {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<SessionRouteRequest>,
    pub entrypoint: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub goal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateWorkflowRequest {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub goal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar_action: Option<CalendarWorkflowActionRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mail_action: Option<MailWorkflowActionRequest>,
}

// ---------------------------------------------------------------------------
// Integrations
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateIntegrationRequest {
    pub integration_id: String,
    pub domain_kind: String,
    pub display_name: String,
    pub backend_kind: integrations::BackendKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub backend_ref_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub backend_display_name: String,
    #[serde(default)]
    pub account_binding: integrations::AccountBinding,
    #[serde(default, skip_serializing_if = "crate::types::is_false")]
    pub canonical_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationListResponse {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<integrations::Resource>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportIntegrationReadinessRequest {
    pub readiness_status: integrations::ReadinessStatus,
    #[serde(default)]
    pub auth_state: integrations::AuthState,
    #[serde(default)]
    pub health_state: integrations::HealthState,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub required_operator_action: String,
    #[serde(default)]
    pub account_binding: integrations::AccountBinding,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub secret_resolution: String,
}

/// Go `SetIntegrationDefaultRequest struct{}` — marker type.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SetIntegrationDefaultRequest {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateIntegrationProbeRequest {
    pub probe_kind: integrations::ProbeKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub approval_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationProbeResponse {
    pub run_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub step_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tool_call_id: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub integration_bindings: Vec<integrations::BindingSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<policy::Approval>,
}

// ---------------------------------------------------------------------------
// Calendar
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarSourceLinkageRequest {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub run_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub step_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tool_call_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workflow_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workflow_step_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub schedule_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub schedule_attempt_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub delivery_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarAttendeeRequest {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub email: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub display_name: String,
    /// required | optional (default required)
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarWorkflowActionRequest {
    pub operation_class: calendar::OperationClass,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub integration_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub external_event_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub window_start: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub window_end: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub location: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub starts_at: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ends_at: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub timezone: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub calendar_ref: String,
    #[serde(default, skip_serializing_if = "crate::types::is_false")]
    pub all_day: bool,
    #[serde(default, skip_serializing_if = "crate::types::is_false")]
    pub recurring: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attendees: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailSourceLinkageRequest {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub run_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub step_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tool_call_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workflow_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workflow_step_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub schedule_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub schedule_attempt_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub delivery_id: String,
    #[serde(default, skip_serializing_if = "crate::types::is_false")]
    pub allow_send_side_effects: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadMailAttachmentRequest {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub integration_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub display_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub media_type: String,
    #[serde(default, skip_serializing_if = "crate::types::is_zero")]
    pub size_bytes: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<MailSourceLinkageRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailAttachmentResponse {
    pub account: mail::AccountProjection,
    pub attachment: mail::AttachmentReference,
    pub operation: mail::Operation,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<mail::Artifact>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailAttachmentRefRequest {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub attachment_ref_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub display_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub media_type: String,
    #[serde(default, skip_serializing_if = "crate::types::is_zero")]
    pub size_bytes: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailWorkflowActionRequest {
    pub operation_class: mail::OperationClass,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub integration_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub draft_id: String,
    #[serde(default)]
    pub compose_mode: mail::ComposeMode,
    #[serde(default)]
    pub result_mode: mail::ReplyForwardResultMode,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub to: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cc: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bcc: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub subject: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachment_refs: Vec<MailAttachmentRefRequest>,
    #[serde(default, skip_serializing_if = "crate::types::is_false")]
    pub allow_send_side_effects: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateCalendarAvailabilityQueryRequest {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub integration_id: String,
    pub window_start: String,
    pub window_end: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub timezone: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<CalendarSourceLinkageRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateCalendarEventRequest {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub integration_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub calendar_ref: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub location: String,
    pub starts_at: String,
    pub ends_at: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub timezone: String,
    #[serde(default, skip_serializing_if = "crate::types::is_false")]
    pub all_day: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub start_date: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub end_date: String,
    #[serde(default, skip_serializing_if = "crate::types::is_false")]
    pub recurring: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub recurrence_rule: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub recurrence_scope: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attendees: Vec<CalendarAttendeeRequest>,
    #[serde(default, skip_serializing_if = "crate::types::is_false")]
    pub notify_attendees: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<CalendarSourceLinkageRequest>,
}

/// Go `type UpdateCalendarEventRequest = CreateCalendarEventRequest`.
pub type UpdateCalendarEventRequest = CreateCalendarEventRequest;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelCalendarEventRequest {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub integration_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub calendar_ref: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub recurrence_scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<CalendarSourceLinkageRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateMailDraftRequest {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub integration_id: String,
    pub compose_mode: mail::ComposeMode,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_message_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub to: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cc: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bcc: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub subject: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachment_refs: Vec<MailAttachmentRefRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<MailSourceLinkageRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMailDraftRequest {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub integration_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub to: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cc: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bcc: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub subject: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachment_refs: Vec<MailAttachmentRefRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<MailSourceLinkageRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMailMessageRequest {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub integration_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub to: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cc: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bcc: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub subject: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachment_refs: Vec<MailAttachmentRefRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<MailSourceLinkageRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMailDraftRequest {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub integration_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<MailSourceLinkageRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplyMailMessageRequest {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub integration_id: String,
    #[serde(default)]
    pub result_mode: mail::ReplyForwardResultMode,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub subject: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachment_refs: Vec<MailAttachmentRefRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<MailSourceLinkageRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForwardMailMessageRequest {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub integration_id: String,
    #[serde(default)]
    pub result_mode: mail::ReplyForwardResultMode,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub to: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cc: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bcc: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub subject: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachment_refs: Vec<MailAttachmentRefRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<MailSourceLinkageRequest>,
}

pub type CalendarAccountListResponse = ListResponse<calendar::AccountProjection>;
pub type CalendarOperationListResponse = ListResponse<calendar::Operation>;
pub type MailAccountListResponse = ListResponse<mail::AccountProjection>;
pub type MailOperationListResponse = ListResponse<mail::Operation>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarEventListResponse {
    pub account: calendar::AccountProjection,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<calendar::Event>,
    pub operation: calendar::Operation,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<calendar::Artifact>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarEventResponse {
    pub account: calendar::AccountProjection,
    pub event: calendar::Event,
    pub operation: calendar::Operation,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<calendar::Artifact>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarAvailabilityQueryResponse {
    pub account: calendar::AccountProjection,
    pub query: calendar::AvailabilityQuery,
    pub operation: calendar::Operation,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<calendar::Artifact>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarOperationResponse {
    pub operation: calendar::Operation,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<calendar::Artifact>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailThreadListResponse {
    pub account: mail::AccountProjection,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<mail::ThreadSnapshot>,
    pub operation: mail::Operation,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<mail::Artifact>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailThreadResponse {
    pub account: mail::AccountProjection,
    pub thread: mail::ThreadSnapshot,
    pub operation: mail::Operation,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<mail::Artifact>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailMessageResponse {
    pub account: mail::AccountProjection,
    pub message: mail::MessageSnapshot,
    pub operation: mail::Operation,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<mail::Artifact>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailDraftListResponse {
    pub account: mail::AccountProjection,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<mail::DraftSnapshot>,
    pub operation: mail::Operation,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<mail::Artifact>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailDraftResponse {
    pub account: mail::AccountProjection,
    pub draft: mail::DraftSnapshot,
    pub operation: mail::Operation,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<mail::Artifact>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailOperationResponse {
    pub operation: mail::Operation,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<mail::Artifact>,
}

// ---------------------------------------------------------------------------
// Computer use
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateComputerUseSessionRequest {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workflow_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workflow_step_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub driver_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub initial_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateComputerUseActionRequest {
    pub action_kind: computeruse::ActionKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub value: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub selected_value: String,
    #[serde(default, skip_serializing_if = "crate::types::is_zero")]
    pub wait_ms: i64,
    #[serde(default)]
    pub page_target: computeruse::PageTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_match_context: Option<computeruse::TargetMatchContext>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputerUseArtifactContentResponse {
    pub artifact_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mime_type: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub file_name: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content: String,
}

// ---------------------------------------------------------------------------
// Schedules / reminders
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateScheduleRequest {
    pub trigger: ScheduleTriggerRequest,
    pub target: ScheduleTargetRequest,
    pub retry_policy: scheduler::RetryPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateReminderRequest {
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub details: String,
    // kura-reminders is ported; migrate this serde_json::Value to reminders::BehaviorMode.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub behavior_mode: serde_json::Value,
    pub trigger: ScheduleTriggerRequest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_launch_config: Option<ReminderWorkflowLaunchRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_up_link: Option<ReminderFollowUpLinkRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReminderWorkflowLaunchRequest {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    pub entrypoint: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub run_goal: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workflow_goal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar_action: Option<CalendarWorkflowActionRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mail_action: Option<MailWorkflowActionRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReminderFollowUpLinkRequest {
    // kura-reminders is ported; migrate to reminders::FollowUpLinkKind.
    pub link_kind: serde_json::Value,
    pub source_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub environment_scope: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_summary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_display_state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReminderTransitionRequest {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub occurrence_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    // kura-reminders is ported; migrate to reminders::ActorKind.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub actor_kind: serde_json::Value,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub snoozed_until: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<ScheduleTriggerRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleTriggerRequest {
    pub kind: scheduler::TriggerKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fire_at: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cron_expr: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub timezone: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleTargetRequest {
    pub kind: scheduler::TargetKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<scheduler::RunTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<ScheduleWorkflowTargetRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleWorkflowTargetRequest {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    pub entrypoint: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub run_goal: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workflow_goal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar_action: Option<CalendarWorkflowActionRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mail_action: Option<MailWorkflowActionRequest>,
}

pub type ScheduleListResponse = ListResponse<scheduler::Schedule>;
// kura-reminders is ported; migrate to reminders::{Reminder,Occurrence,ActionRecord}.
pub type ReminderListResponse = ListResponse<serde_json::Value>;
pub type ReminderOccurrenceListResponse = ListResponse<serde_json::Value>;
pub type ReminderActionListResponse = ListResponse<serde_json::Value>;

// ---------------------------------------------------------------------------
// Delivery
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateDeliveryTargetRequest {
    pub target_id: String,
    pub display_name: String,
    pub target_kind: delivery::TargetKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connector_binding: Option<delivery::ConnectorBinding>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub address_summary: String,
}

/// Go `UpdateDeliveryTargetStatusRequest struct{}` — marker type.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateDeliveryTargetStatusRequest {}

pub type DeliveryTargetListResponse = ListResponse<delivery::DeliveryTarget>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpsertDeliveryPreferenceRequest {
    pub preference_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub environment_scope: String,
    pub scope_kind: delivery::PreferenceScopeKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub integration_id: String,
    #[serde(default)]
    pub preferred_targets_by_class: HashMap<delivery::ResultClass, String>,
    #[serde(default)]
    pub summary_policy: delivery::SummaryPolicy,
    #[serde(default)]
    pub suppression_policy: delivery::SuppressionPolicy,
}

pub type DeliveryPreferenceListResponse = ListResponse<delivery::DeliveryPreference>;
pub type DeliveryOutcomeListResponse = ListResponse<delivery::DeliveryOutcome>;
pub type DeliverySummaryWindowListResponse = ListResponse<delivery::SummaryWindow>;

// ---------------------------------------------------------------------------
// Connector ingress / events / providers / skills / sandbox
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorIngressMessage {
    pub message_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connector_account_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub channel_or_conversation_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider_message_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub equivalent_rule_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorIngressRunRequest {
    pub entrypoint: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub goal: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorIngressMessageRequest {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub route: SessionRouteRequest,
    pub message: ConnectorIngressMessage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<ConnectorIngressRunRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorIngressMessageResponse {
    pub ingress_id: String,
    pub connector_id: String,
    pub outcome: String,
    pub reason_code: String,
    pub redaction_status: String,
    pub accepted_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<router::Session>,
    pub session_created: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<runtime::Run>,
    pub run_created: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventListResponse {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<events::Event>,
    #[serde(default, skip_serializing_if = "crate::types::is_zero")]
    pub next_cursor: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderListResponse {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<providers::Profile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCheckListResponse {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<providers::Check>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAuthStateResponse {
    pub auth: providers::AuthState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModelListResponse {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<providers::Model>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDefaultModelRequest {
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDefaultModelResponse {
    pub provider_id: String,
    pub default_model: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillFileResponse {
    pub path: String,
    pub size_bytes: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSummaryResponse {
    pub skill_id: String,
    pub name: String,
    pub description: String,
    pub source: String,
    pub root_path: String,
    pub skill_path: String,
    pub instruction_path: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<SkillFileResponse>,
    #[serde(default)]
    pub frontmatter: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_manifest: Option<skills::ExecutableManifest>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub availability_status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub availability_reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillDetailResponse {
    /// Embedded Go `SkillSummaryResponse` (fields promoted into this struct).
    #[serde(flatten)]
    pub summary: SkillSummaryResponse,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub frontmatter_raw: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillOverlayResponse {
    pub overlay_id: String,
    pub source: String,
    pub path: String,
    pub size_bytes: i64,
    pub modified_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillRegistryResponse {
    pub loaded_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<SkillSummaryResponse>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overlays: Vec<SkillOverlayResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxExplainResponse {
    pub decision: sandbox::Decision,
}

// ---------------------------------------------------------------------------
// Generic list envelope / auth / tenants / workflows / computer-use
// ---------------------------------------------------------------------------

/// Generic `{items: [...]}` envelope (Go `ListResponse[T]`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListResponse<T> {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<T>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthMeResponse {
    pub token: identity::auth::AccessToken,
    pub principal: identity::Principal,
    pub default_tenant: identity::Tenant,
    pub current_tenant: identity::Tenant,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_tenants: Vec<identity::Tenant>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub token_grants: Vec<identity::TokenTenantGrant>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub permissions: Vec<identity::Permission>,
    pub tenant_context: identity::TenantContext,
}

pub type TenantListResponse = ListResponse<identity::Tenant>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TenantDetailResponse {
    pub tenant: identity::Tenant,
    pub tenant_context: identity::TenantContext,
}

pub type WorkflowListResponse = ListResponse<orchestration::Workflow>;
pub type ComputerUseSessionListResponse = ListResponse<computeruse::Session>;
pub type ComputerUseActionListResponse = ListResponse<computeruse::Action>;

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::DeserializeOwned;

    /// Serialize -> deserialize -> serialize and assert the wire form is stable.
    fn round_trip<T>(value: &T)
    where
        T: Serialize + DeserializeOwned + std::fmt::Debug + Clone,
    {
        let json = serde_json::to_value(value).expect("serialize");
        let back: T = serde_json::from_value(json.clone()).expect("deserialize");
        assert_eq!(json, serde_json::to_value(&back).expect("reserialize"));
    }

    #[test]
    fn system_info_response_round_trip() {
        let value = SystemInfoResponse {
            service: "kura".to_string(),
            environment: "test".to_string(),
            version: "0.1.0".to_string(),
            bind_addr: "127.0.0.1:19192".to_string(),
            data_dir: "/tmp/kura".to_string(),
            log_level: "info".to_string(),
        };
        round_trip(&value);
        let json = serde_json::to_value(&value).expect("serialize");
        // Go json tags: camelCase.
        assert_eq!(json["bindAddr"], "127.0.0.1:19192");
        assert_eq!(json["dataDir"], "/tmp/kura");
        assert_eq!(json["logLevel"], "info");
    }

    #[test]
    fn chat_query_response_round_trip() {
        let value = ChatQueryResponse {
            dispatch_id: "dispatch_1".to_string(),
            provider: "echo".to_string(),
            model: "echo-1".to_string(),
            skills: vec!["skill_a".to_string()],
            skill_contracts: vec![
                serde_json::json!({ "name": "x" })
                    .as_object()
                    .expect("obj")
                    .clone(),
            ],
            query: "hello".to_string(),
            status: "completed".to_string(),
            partial: false,
            reply: "hi".to_string(),
            finish_reason: "stop".to_string(),
            usage: llm::Usage {
                input_tokens: 3,
                output_tokens: 1,
                total_tokens: 4,
            },
            error_code: String::new(),
            error: String::new(),
            thread_id: "thread_1".to_string(),
            session_segment_id: String::new(),
            request_turn_id: String::new(),
            response_turn_id: String::new(),
            continuity_preview_id: String::new(),
            continuity_applied: Some(true),
            continuity_status: "applied".to_string(),
            continuity_included_count: Some(2),
            continuity_excluded_count: None,
        };
        round_trip(&value);
        let json = serde_json::to_value(&value).expect("serialize");
        assert_eq!(json["dispatchId"], "dispatch_1");
        assert_eq!(json["usage"]["inputTokens"], 3);
        assert!(json.get("errorCode").is_none(), "empty string omitted");
        assert_eq!(json["continuityApplied"], true);
        assert!(
            json.get("continuityExcludedCount").is_none(),
            "None omitted"
        );
    }

    #[test]
    fn create_run_request_round_trip_with_route() {
        let value = CreateRunRequest {
            session_id: "session_1".to_string(),
            route: Some(SessionRouteRequest {
                kind: Some(router::SessionKind::Group),
                channel: "discord".to_string(),
                account_id: "acct".to_string(),
                peer_id: "peer".to_string(),
                thread_id: String::new(),
            }),
            entrypoint: "main".to_string(),
            goal: "do the thing".to_string(),
            input: Some(serde_json::json!({ "dryRun": true })),
        };
        round_trip(&value);
        let json = serde_json::to_value(&value).expect("serialize");
        assert_eq!(json["route"]["kind"], "group");
        assert_eq!(json["entrypoint"], "main");
        assert_eq!(json["input"]["dryRun"], true);
    }

    #[test]
    fn omitted_optional_fields_are_not_serialized() {
        let value = CreateRunRequest {
            session_id: String::new(),
            route: None,
            entrypoint: "main".to_string(),
            goal: String::new(),
            input: None,
        };
        let json = serde_json::to_value(&value).expect("serialize");
        assert_eq!(json, serde_json::json!({ "entrypoint": "main" }));
    }

    #[test]
    fn create_calendar_event_request_camel_case() {
        let value = CreateCalendarEventRequest {
            integration_id: "cal_1".to_string(),
            calendar_ref: String::new(),
            title: "Sync".to_string(),
            description: String::new(),
            location: String::new(),
            starts_at: "2026-05-01T09:00:00Z".to_string(),
            ends_at: "2026-05-01T10:00:00Z".to_string(),
            timezone: "UTC".to_string(),
            all_day: false,
            start_date: String::new(),
            end_date: String::new(),
            recurring: false,
            recurrence_rule: String::new(),
            recurrence_scope: String::new(),
            attendees: Vec::new(),
            notify_attendees: false,
            source: None,
        };
        round_trip(&value);
        let json = serde_json::to_value(&value).expect("serialize");
        assert_eq!(json["integrationId"], "cal_1");
        assert_eq!(json["startsAt"], "2026-05-01T09:00:00Z");
        assert!(json.get("allDay").is_none(), "false bool omitted");
    }

    #[test]
    fn schedule_trigger_request_round_trip() {
        let value = ScheduleTriggerRequest {
            kind: scheduler::TriggerKind::Once,
            fire_at: "2026-05-01T09:00:00Z".to_string(),
            cron_expr: String::new(),
            timezone: "UTC".to_string(),
        };
        round_trip(&value);
        let json = serde_json::to_value(&value).expect("serialize");
        assert_eq!(json["kind"], "once");
        assert_eq!(json["fireAt"], "2026-05-01T09:00:00Z");
        assert!(json.get("cronExpr").is_none());
    }

    #[test]
    fn create_delivery_target_request_round_trip() {
        let value = CreateDeliveryTargetRequest {
            target_id: "target_1".to_string(),
            display_name: "DM".to_string(),
            target_kind: delivery::TargetKind::ConnectorRoute,
            connector_binding: None,
            address_summary: String::new(),
        };
        round_trip(&value);
        let json = serde_json::to_value(&value).expect("serialize");
        assert_eq!(json["targetKind"], "connector_route");
        assert!(json.get("connectorBinding").is_none());
    }

    #[test]
    fn list_response_round_trip() {
        let value = ListResponse {
            items: vec!["a".to_string(), "b".to_string()],
        };
        round_trip(&value);
        let json = serde_json::to_value(&value).expect("serialize");
        assert_eq!(json["items"][0], "a");
    }

    #[test]
    fn connector_ingress_message_response_round_trip() {
        let value = ConnectorIngressMessageResponse {
            ingress_id: "ingress_1".to_string(),
            connector_id: "discord-main".to_string(),
            outcome: "accepted".to_string(),
            reason_code: String::new(),
            redaction_status: "none".to_string(),
            accepted_at: DateTime::<Utc>::from_timestamp(1_700_000_000, 0).expect("ts"),
            session: None,
            session_created: false,
            run: None,
            run_created: false,
        };
        round_trip(&value);
        let json = serde_json::to_value(&value).expect("serialize");
        assert_eq!(json["acceptedAt"], "2023-11-14T22:13:20Z");
        assert!(json.get("session").is_none());
    }
}
