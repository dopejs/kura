//! Roadmap 37 credential-boundary fixture (port of
//! daemon/internal/store/migrationfixture/r37_credentials.go): writes local
//! mcp-secrets.json / skill-secrets.json files and seeds provider auth state,
//! integration, connector, and MCP server/tool/exposure-rule rows through the
//! kura-store domain CRUD.

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::Path;

use chrono::Utc;
use kura_connectors::{Connector, Status as ConnectorStatus};
use kura_integrations::{
    AccountBinding, AuthState as IntegrationAuthState, BackendBinding, BackendKind, HealthState,
    ReadinessStatus, Resource,
};
use kura_providers::{AuthMode, AuthState, AuthStatus, Family};
use kura_store::SQLiteStore;
use kura_store::mcp::{
    MCPServerRecord, MCPServerStateRecord, MCPToolExposureRuleRecord, MCPToolRecord,
};

/// Fake secret material that must never appear in stored rows (used by the
/// migration regression suite to assert the boundary is not leaked).
pub const R37_FAKE_SECRET_TENANT_A: &str = "R37_FAKE_SECRET_TENANT_A_DO_NOT_LEAK";

/// IDs/refs produced by the r37 fixture (Go R37CredentialFixture).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct R37CredentialFixture {
    pub mcp_secret_refs: Vec<String>,
    pub skill_secret_refs: Vec<String>,
    pub conflict_ref: String,
    pub provider_id: String,
    pub integration_id: String,
    pub connector_id: String,
    pub mcp_server_id: String,
    pub mcp_tool_name: String,
}

impl R37CredentialFixture {
    #[must_use]
    pub fn new() -> Self {
        Self {
            mcp_secret_refs: vec![
                "R37_MCP_TOKEN".to_string(),
                "R37_SHARED_TOKEN".to_string(),
                "R37_CONFLICT_TOKEN".to_string(),
            ],
            skill_secret_refs: vec![
                "R37_SKILL_TOKEN".to_string(),
                "R37_SHARED_TOKEN".to_string(),
                "R37_CONFLICT_TOKEN".to_string(),
            ],
            conflict_ref: "R37_CONFLICT_TOKEN".to_string(),
            provider_id: "r37_legacy_provider".to_string(),
            integration_id: "r37_legacy_integration".to_string(),
            connector_id: "r37_legacy_connector".to_string(),
            mcp_server_id: "r37_legacy_mcp".to_string(),
            mcp_tool_name: "lookup".to_string(),
        }
    }
}

/// Writes the two local credential files (Go SeedR37LocalCredentialFiles).
pub fn seed_r37_local_credential_files(data_dir: &str) -> Result<R37CredentialFixture, String> {
    let fixture = R37CredentialFixture::new();
    let mut mcp = BTreeMap::new();
    mcp.insert(
        "R37_MCP_TOKEN".to_string(),
        R37_FAKE_SECRET_TENANT_A.to_string(),
    );
    mcp.insert(
        "R37_SHARED_TOKEN".to_string(),
        "shared-r37-value".to_string(),
    );
    mcp.insert("R37_CONFLICT_TOKEN".to_string(), "mcp-side".to_string());
    write_r37_credential_json(&Path::new(data_dir).join("mcp-secrets.json"), &mcp)?;

    let mut skill = BTreeMap::new();
    skill.insert("R37_SKILL_TOKEN".to_string(), "skill-r37-value".to_string());
    skill.insert(
        "R37_SHARED_TOKEN".to_string(),
        "shared-r37-value".to_string(),
    );
    skill.insert("R37_CONFLICT_TOKEN".to_string(), "skill-side".to_string());
    write_r37_credential_json(&Path::new(data_dir).join("skill-secrets.json"), &skill)?;
    Ok(fixture)
}

/// Writes the credential files and seeds the store rows through the domain CRUD
/// (Go SeedR37LocalCredentialState). Requires the store to be at head schema
/// (v34+ adds the tenant_id / secret_refs_json columns used by the CRUD).
pub fn seed_r37_local_credential_state(
    store: &SQLiteStore,
    data_dir: &str,
) -> Result<R37CredentialFixture, String> {
    let fixture = seed_r37_local_credential_files(data_dir)?;
    let now = Utc::now();

    store
        .upsert_provider_auth_state(&AuthState {
            provider_id: fixture.provider_id.clone(),
            family: Family::OpenAICompatible,
            auth_mode: AuthMode::ApiKey,
            status: AuthStatus::Authenticated,
            cli_available: true,
            account_label: "R37 legacy provider".to_string(),
            last_checked_at: now,
            metadata: HashMap::from([("source".to_string(), "r37_migration_fixture".to_string())]),
            ..AuthState::default()
        })
        .map_err(|e| format!("seed provider auth: {e}"))?;

    store
        .upsert_integration(&Resource {
            integration_id: fixture.integration_id.clone(),
            domain_kind: "calendar".to_string(),
            display_name: "R37 Legacy Integration".to_string(),
            environment_scope: "test".to_string(),
            readiness_status: ReadinessStatus::Healthy,
            auth_state: IntegrationAuthState::Authorized.as_str().to_string(),
            health_state: HealthState::Healthy.as_str().to_string(),
            account_binding: Some(AccountBinding {
                account_key: "r37-legacy@example.com".to_string(),
                ..AccountBinding::default()
            }),
            backend_binding: BackendBinding {
                backend_kind: BackendKind::ManagedProvider,
                backend_ref_id: fixture.provider_id.clone(),
                ..BackendBinding::default()
            },
            created_at: now,
            updated_at: now,
            last_transition_at: now,
            ..Resource::default()
        })
        .map_err(|e| format!("seed integration: {e}"))?;

    store
        .upsert_connector(&Connector {
            connector_id: fixture.connector_id.clone(),
            kind: "discord".to_string(),
            display_name: "R37 Legacy Connector".to_string(),
            status: ConnectorStatus::Healthy,
            secret_refs: vec![fixture.conflict_ref.clone()],
            backoff_seconds: 1,
            created_at: now,
            updated_at: now,
            ..Connector::default()
        })
        .map_err(|e| format!("seed connector: {e}"))?;

    store
        .upsert_mcp_server(&MCPServerRecord {
            server_id: fixture.mcp_server_id.clone(),
            enabled: true,
            updated_at: now,
            document: must_r37_json(serde_json::json!({
                "serverId": fixture.mcp_server_id.clone(),
                "displayName": "R37 Legacy MCP",
                "enabled": true,
                "transportKind": "stdio",
                "command": "fake",
                "secretRefs": [fixture.conflict_ref.clone()],
            })),
        })
        .map_err(|e| format!("seed mcp server: {e}"))?;

    store
        .upsert_mcp_server_state(&MCPServerStateRecord {
            server_id: fixture.mcp_server_id.clone(),
            status: "healthy".to_string(),
            updated_at: now,
            document: must_r37_json(serde_json::json!({
                "serverId": fixture.mcp_server_id.clone(),
                "status": "healthy",
            })),
        })
        .map_err(|e| format!("seed mcp server state: {e}"))?;

    store
        .upsert_mcp_tool(&MCPToolRecord {
            server_id: fixture.mcp_server_id.clone(),
            tool_name: fixture.mcp_tool_name.clone(),
            discovery_status: "discovered".to_string(),
            updated_at: now,
            document: must_r37_json(serde_json::json!({ "name": fixture.mcp_tool_name.clone() })),
            ..MCPToolRecord::default()
        })
        .map_err(|e| format!("seed mcp tool: {e}"))?;

    store
        .upsert_mcp_tool_exposure_rule(&MCPToolExposureRuleRecord {
            server_id: fixture.mcp_server_id.clone(),
            tool_name: fixture.mcp_tool_name.clone(),
            runtime_surface: "chat".to_string(),
            exposure_mode: "allow".to_string(),
            active: true,
            updated_at: now,
            document: must_r37_json(serde_json::json!({
                "toolName": fixture.mcp_tool_name.clone(),
                "runtimeSurface": "chat",
            })),
        })
        .map_err(|e| format!("seed mcp exposure: {e}"))?;

    Ok(fixture)
}

fn write_r37_credential_json(path: &Path, value: &BTreeMap<String, String>) -> Result<(), String> {
    let payload =
        serde_json::to_vec(value).map_err(|e| format!("marshal {}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true).mode(0o600);
        let mut file = options
            .open(path)
            .map_err(|e| format!("write {}: {e}", path.display()))?;
        file.write_all(&payload)
            .map_err(|e| format!("write {}: {e}", path.display()))?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, &payload).map_err(|e| format!("write {}: {e}", path.display()))?;
    }
    Ok(())
}

fn must_r37_json(value: serde_json::Value) -> String {
    serde_json::to_string(&value).expect("r37 fixture document is serializable")
}
