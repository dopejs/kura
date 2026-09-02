//! Port of `daemon/internal/mcp/manager.go`: the MCP server registry manager.
//!
//! The manager keeps servers, per-server runtime states, discovered tools, tool
//! exposure rules, and live sessions in memory behind a `parking_lot::RwLock` with
//! insertion-ordered server ids (the `kura-runtime` pattern). Every mutating method
//! persists through `kura-store`'s MCP CRUD when a store is installed and publishes
//! events through `kura-events` (plus the store event ledger).
//!
//! Conventions vs the Go original:
//! - `context.Context` is dropped (synchronous port). Background persistence in Go's
//!   detached goroutines is just direct calls.
//! - Tenant context (`tenantctx`) is not ported: `activeTenantID` is always "".
//! - The store `HasActiveMCPToolCalls` guard and approval/decision SQLite persistence
//!   are deferred (kura-store has no such CRUD yet); the corresponding checks are
//!   skipped and persist_approval/persist_decision are no-ops.
//! - watch_session / schedule_restart / schedule_websocket_reconnect run on detached
//!   threads holding an Arc clone of the manager, mirroring Go goroutines.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Instant;

use chrono::{DateTime, Utc};
use kura_events::Resource;
use serde_json::{Map, Value};

use crate::catalog::{
    bundled_catalog_entries, catalog_install_input_from_snapshot,
    evaluate_catalog_install_spec_availability, fingerprint_create_server_spec,
    install_snapshot_from_create_spec, look_path, requires_offline_verified_local_command,
    resolve_mcp_secrets, secret_refs_from_requirements,
};
use crate::transport::{Session, SessionPipes, Transport, TransportMux};
use crate::types::*;
use crate::{
    McpError, RESOURCE_KIND_SERVER, RESOURCE_KIND_TOOL, clean_strings, clone_backend_kinds,
    clone_strings, environment_scope, first_non_empty, mcp_backoff_delay, rfc3339_nano,
    session_start_timeout,
};

/// Go `websocketReconnectMaxAttempts`.
const WEBSOCKET_RECONNECT_MAX_ATTEMPTS: i64 = 3;

/// Go `isRestoreLifecycleRequester`.
#[must_use]
pub fn is_restore_lifecycle_requester(requested_by: &str) -> bool {
    requested_by.trim() == "system.restore"
}

/// Go `isWebsocketReconnectRequester`.
#[must_use]
pub fn is_websocket_reconnect_requester(requested_by: &str) -> bool {
    requested_by.trim() == "mcp.websocket_reconnect"
}

/// Go `sandbox.AttachedExecution` (not yet in kura-sandbox): the process pipes handed
/// to a stdio MCP transport. Ported here as part of the deferred sandbox integration.
pub struct AttachedExecution {
    pub execution: kura_sandbox::Execution,
    pub stdin: Option<Box<dyn std::io::Write + Send>>,
    pub stdout: Option<Box<dyn std::io::Read + Send>>,
    pub stderr: Option<Box<dyn std::io::Read + Send>>,
}

/// Go `attachedExecutionStarter` interface: the sandbox execution-plane collaborator.
/// No workspace implementation exists yet; the manager behaves like the Go manager with
/// a nil sandbox manager when `None`.
pub trait AttachedExecutionStarter: Send + Sync {
    fn start_attached_execution(
        &self,
        request: &kura_sandbox::ExecutionRequest,
    ) -> Result<(kura_sandbox::Execution, Option<AttachedExecution>), String>;
    fn cancel_execution(&self, execution_id: &str) -> Result<(kura_sandbox::Execution, bool), String>;
    fn get_execution(&self, execution_id: &str) -> Option<kura_sandbox::Execution>;
    fn persist_consumer_view(&self, view: &kura_sandbox::ConsumerContractView) -> Result<(), String>;
    fn get_profile(&self, profile_id: &str) -> Option<kura_sandbox::Profile>;
}

/// Sync secret resolver replacing the (async, tenant-scoped) kura-secrets manager. The
/// Go fallback path (no secret manager) reads `mcp-secrets.json` from the data dir.
pub trait SecretResolver: Send + Sync {
    fn resolve(&self, secret_ref: &str) -> Result<Option<String>, String>;
}

/// In-memory session registration (Go `sessionState`).
#[derive(Clone, Default)]
pub struct SessionState {
    pub session_id: String,
    pub execution_id: String,
    pub session: Option<Arc<dyn Session>>,
    pub transport_kind: TransportKind,
    pub stop_requested: bool,
    pub cancel_requested: bool,
}

#[derive(Default)]
struct ManagerState {
    servers: HashMap<String, Server>,
    server_ids: Vec<String>,
    states: HashMap<String, ServerState>,
    tools: HashMap<String, HashMap<String, Tool>>,
    exposure: HashMap<String, HashMap<String, HashMap<String, ToolExposureRule>>>,
    sessions: HashMap<String, SessionState>,
}

struct ManagerInner {
    cfg: kura_config::Config,
    store: Option<Arc<Mutex<kura_store::SQLiteStore>>>,
    event_bus: Option<kura_events::Bus>,
    policy: Option<kura_policy::Engine>,
    sandboxes: Option<Arc<dyn AttachedExecutionStarter>>,
    transport: Option<Arc<dyn Transport>>,
    secrets: parking_lot::RwLock<Option<Arc<dyn SecretResolver>>>,
    state: parking_lot::RwLock<ManagerState>,
}

/// Cloneable handle over the shared MCP manager state (port of `*Manager`). Methods
/// are synchronous; detached watcher/restart threads hold `Arc` clones.
#[derive(Clone)]
pub struct Manager {
    inner: Arc<ManagerInner>,
}

impl Default for Manager {
    fn default() -> Self {
        Self::new(
            kura_config::Config {
                environment: kura_config::Environment::Test,
                bind_addr: "127.0.0.1:19192".to_string(),
                data_dir: "~/.kura-test".to_string(),
                log_level: "info".to_string(),
                version: "dev".to_string(),
                llm: Default::default(),
                connectors: Default::default(),
            },
            None,
            None,
            None,
            None,
            None,
        )
    }
}

impl Manager {
    /// Go `NewManager`. A `None` transport defaults to a `TransportMux` with the
    /// concrete stdio / streamable-http / websocket transports installed.
    #[must_use]
    pub fn new(
        cfg: kura_config::Config,
        store: Option<Arc<Mutex<kura_store::SQLiteStore>>>,
        event_bus: Option<kura_events::Bus>,
        sandboxes: Option<Arc<dyn AttachedExecutionStarter>>,
        policy: Option<kura_policy::Engine>,
        transport: Option<Arc<dyn Transport>>,
    ) -> Self {
        let transport = match transport {
            Some(transport) => transport,
            None => Arc::new(TransportMux::default()),
        };
        Manager {
            inner: Arc::new(ManagerInner {
                cfg,
                store,
                event_bus,
                policy,
                sandboxes,
                transport: Some(transport),
                secrets: parking_lot::RwLock::new(None),
                state: parking_lot::RwLock::new(ManagerState::default()),
            }),
        }
    }

    /// Go `SetSecretManager`.
    pub fn set_secret_manager(&self, resolver: Arc<dyn SecretResolver>) {
        *self.inner.secrets.write() = Some(resolver);
    }

    /// Go `ListCatalog`.
    #[must_use]
    pub fn list_catalog(&self) -> Vec<CatalogEntry> {
        bundled_catalog_entries(&self.inner.cfg)
    }

    /// Go `GetCatalogEntry`.
    #[must_use]
    pub fn get_catalog_entry(&self, entry_id: &str) -> Option<CatalogEntry> {
        bundled_catalog_entries(&self.inner.cfg)
            .into_iter()
            .find(|entry| entry.id == entry_id.trim())
    }

    /// Go `ListTransportCapabilities`.
    #[must_use]
    pub fn list_transport_capabilities(&self) -> Vec<TransportCapability> {
        let environment = environment_scope(self.inner.cfg.environment);
        let mut items = vec![
            TransportCapability {
                transport_kind: TransportKind::Stdio,
                availability_status: AvailabilityStatus::Ready,
                health_status: TransportHealthStatus::Healthy,
                prerequisites: vec![
                    "stdio command must be configured per server".to_string(),
                    "sandbox profile must remain available for subprocess execution".to_string(),
                ],
                environment_scope: environment.clone(),
                daemon_managed_reconnect: false,
                recovery_summary: "stdio sessions restart through the existing daemon-owned lifecycle path".to_string(),
                ..TransportCapability::default()
            },
            TransportCapability {
                transport_kind: TransportKind::StreamableHTTP,
                availability_status: AvailabilityStatus::Ready,
                health_status: TransportHealthStatus::Healthy,
                prerequisites: vec![
                    "streamable-http endpoint must be configured per server".to_string(),
                    "remote endpoint reachability is evaluated per server".to_string(),
                ],
                environment_scope: environment.clone(),
                daemon_managed_reconnect: false,
                recovery_summary: "streamable-http sessions restart through the normal lifecycle path".to_string(),
                ..TransportCapability::default()
            },
            TransportCapability {
                transport_kind: TransportKind::Websocket,
                availability_status: AvailabilityStatus::Ready,
                health_status: TransportHealthStatus::Healthy,
                prerequisites: vec![
                    "websocket endpoint must be configured per server".to_string(),
                    "authenticated endpoints require secret-ref-backed header auth".to_string(),
                ],
                environment_scope: environment.clone(),
                supported_auth_kinds: vec![
                    WebsocketAuthMode::BearerHeader.as_str().to_string(),
                    WebsocketAuthMode::Header.as_str().to_string(),
                ],
                daemon_managed_reconnect: true,
                recovery_summary: "daemon manages bounded websocket reconnect and restore history".to_string(),
                ..TransportCapability::default()
            },
        ];
        let guard = self.inner.state.read();
        for server_id in &guard.server_ids {
            let Some(server) = guard.servers.get(server_id) else {
                continue;
            };
            if !server.environment_scope.is_empty() && server.environment_scope != environment {
                continue;
            }
            let state = guard.states.get(server_id).cloned().unwrap_or_default();
            for item in &mut items {
                if item.transport_kind != server.transport_kind {
                    continue;
                }
                match state.status {
                    LifecycleStatus::Degraded | LifecycleStatus::BackingOff => {
                        item.health_status = TransportHealthStatus::Degraded;
                        item.reason = first_non_empty(&[
                            item.reason.as_str(),
                            state.health_reason.as_str(),
                            "one or more servers are recovering",
                        ]);
                    }
                    LifecycleStatus::Unsupported => {
                        item.availability_status = AvailabilityStatus::Unsupported;
                        item.reason = first_non_empty(&[
                            item.reason.as_str(),
                            state.health_reason.as_str(),
                            "transport is unsupported for at least one configured server",
                        ]);
                    }
                    _ => {}
                }
            }
        }
        items
    }

    /// Go `Restore`: reloads servers/states/tools/exposure from the store, normalizes
    /// states and tool staleness, then auto-starts enabled servers.
    pub fn restore(&self) -> Result<(), McpError> {
        let Some(store) = self.inner.store.clone() else {
            return Ok(());
        };
        let lock = || {
            store
                .lock()
                .map_err(|_| McpError::Store("store lock poisoned".to_string()))
        };
        let server_records = lock()?.list_mcp_servers().map_err(McpError::Store)?;
        let state_records = lock()?.list_mcp_server_states().map_err(McpError::Store)?;
        let tool_records = lock()?.list_mcp_tools("").map_err(McpError::Store)?;
        let exposure_records = lock()?.list_mcp_tool_exposure_rules("").map_err(McpError::Store)?;

        let mut servers: HashMap<String, Server> = HashMap::new();
        let mut server_ids: Vec<String> = Vec::new();
        let mut states: HashMap<String, ServerState> = HashMap::new();
        let mut tools: HashMap<String, HashMap<String, Tool>> = HashMap::new();
        let mut exposure: HashMap<String, HashMap<String, HashMap<String, ToolExposureRule>>> = HashMap::new();

        for record in server_records {
            let server: Server = serde_json::from_str(&record.document)
                .map_err(|e| McpError::Store(format!("decode mcp server {}: {e}", record.server_id)))?;
            servers.insert(server.server_id.clone(), server.clone());
            server_ids.push(server.server_id.clone());
        }
        for record in state_records {
            let state: ServerState = serde_json::from_str(&record.document)
                .map_err(|e| McpError::Store(format!("decode mcp server state {}: {e}", record.server_id)))?;
            states.insert(state.server_id.clone(), state);
        }
        for record in tool_records {
            let tool: Tool = serde_json::from_str(&record.document)
                .map_err(|e| McpError::Store(format!("decode mcp tool {}/{}: {e}", record.server_id, record.tool_name)))?;
            tools.entry(tool.server_id.clone()).or_default().insert(tool.tool_name.clone(), tool);
        }
        for record in exposure_records {
            let rule: ToolExposureRule = serde_json::from_str(&record.document).map_err(|e| {
                McpError::Store(format!(
                    "decode mcp tool exposure rule {}/{}/{}: {e}",
                    record.server_id, record.tool_name, record.runtime_surface
                ))
            })?;
            exposure
                .entry(rule.server_id.clone())
                .or_default()
                .entry(rule.tool_name.clone())
                .or_default()
                .insert(rule.runtime_surface.clone(), rule);
        }

        for server_id in &server_ids {
            if !states.contains_key(server_id) {
                states.insert(server_id.clone(), default_state_for_server(&servers[server_id]));
                continue;
            }
            let server = &servers[server_id];
            let mut state = states[server_id].clone();
            if !server.enabled {
                state.status = LifecycleStatus::Disabled;
            } else if state.status != LifecycleStatus::Stopped && state.status != LifecycleStatus::Disabled {
                state.status = LifecycleStatus::Stopped;
                state.health_reason = "daemon restart cleared in-memory MCP session state".to_string();
                state.last_execution_id = String::new();
            }
            state.updated_at = Utc::now();
            states.insert(server_id.clone(), state);
        }

        {
            let mut guard = self.inner.state.write();
            guard.servers = servers;
            guard.server_ids = server_ids.clone();
            guard.states = states.clone();
            guard.tools = tools.clone();
            guard.exposure = exposure.clone();
            guard.sessions = HashMap::new();

            for server_id in &server_ids {
                if let Some(state) = guard.states.get(server_id) {
                    let _ = self.persist_state(state);
                }
            }
            for server_id in &server_ids {
                let Some(map) = guard.tools.get(server_id) else { continue };
                if map.is_empty() {
                    continue;
                }
                let server = &guard.servers[server_id];
                let healthy = server.enabled
                    && guard
                        .states
                        .get(server_id)
                        .is_some_and(|s| s.status == LifecycleStatus::Healthy);
                if !healthy {
                    if let Some(tool_map) = guard.tools.get_mut(server_id) {
                        for tool in tool_map.values_mut() {
                            if tool.discovery_status == DiscoveryStatus::Discovered {
                                tool.discovery_status = DiscoveryStatus::Stale;
                                tool.updated_at = Utc::now();
                            }
                        }
                    }
                }
            }
        }

        for server_id in &server_ids {
            let tools = {
                let guard = self.inner.state.read();
                guard.tools.get(server_id).cloned().unwrap_or_default()
            };
            let tool_list: Vec<Tool> = tools.into_values().collect();
            if let Err(err) = self.persist_tool_map(server_id, &tool_list) {
                return Err(err);
            }
        }

        for server_id in &server_ids {
            let Some(server) = self.get_server(server_id) else {
                continue;
            };
            if !server.enabled {
                continue;
            }
            let _ = self.start(server_id, "system.restore");
        }
        Ok(())
    }

    /// Go `ListServers` (insertion order).
    #[must_use]
    pub fn list_servers(&self) -> Vec<ServerResource> {
        let guard = self.inner.state.read();
        guard
            .server_ids
            .iter()
            .filter_map(|server_id| {
                let server = guard.servers.get(server_id)?;
                Some(self.build_server_resource_locked(&guard, server))
            })
            .collect()
    }

    /// Go `ListServersForTenant` (an empty tenant id lists everything).
    #[must_use]
    pub fn list_servers_for_tenant(&self, tenant_id: &str) -> Vec<ServerResource> {
        let tenant_id = tenant_id.trim().to_string();
        let guard = self.inner.state.read();
        guard
            .server_ids
            .iter()
            .filter_map(|server_id| {
                let server = guard.servers.get(server_id)?;
                if !tenant_id.is_empty() && server.tenant_id != tenant_id {
                    return None;
                }
                Some(self.build_server_resource_locked(&guard, server))
            })
            .collect()
    }

    /// Go `GetServer`.
    #[must_use]
    pub fn get_server(&self, server_id: &str) -> Option<Server> {
        self.inner.state.read().servers.get(server_id.trim()).cloned()
    }

    /// Go `GetServerForTenant`.
    #[must_use]
    pub fn get_server_for_tenant(&self, server_id: &str, tenant_id: &str) -> Option<Server> {
        let server = self.get_server(server_id)?;
        let tenant_id = tenant_id.trim().to_string();
        if !tenant_id.is_empty() && server.tenant_id != tenant_id {
            return None;
        }
        Some(server)
    }

    /// Go `GetServerResource`.
    #[must_use]
    pub fn get_server_resource(&self, server_id: &str) -> Option<ServerResource> {
        let guard = self.inner.state.read();
        let server = guard.servers.get(server_id.trim())?;
        Some(self.build_server_resource_locked(&guard, server))
    }

    /// Go `GetServerResourceForTenant`.
    #[must_use]
    pub fn get_server_resource_for_tenant(&self, server_id: &str, tenant_id: &str) -> Option<ServerResource> {
        let guard = self.inner.state.read();
        let server = guard.servers.get(server_id.trim())?;
        let tenant_id = tenant_id.trim().to_string();
        if !tenant_id.is_empty() && server.tenant_id != tenant_id {
            return None;
        }
        Some(self.build_server_resource_locked(&guard, server))
    }

    /// Go `ListTools`.
    pub fn list_tools(&self, server_id: &str) -> Result<Vec<ToolResource>, McpError> {
        let guard = self.inner.state.read();
        let server = guard
            .servers
            .get(server_id.trim())
            .cloned()
            .ok_or(McpError::ServerNotFound)?;
        let mut items = Vec::new();
        if let Some(map) = guard.tools.get(&server.server_id) {
            for tool in map.values() {
                items.push(self.build_tool_resource_locked(&guard, &server, tool));
            }
        }
        Ok(items)
    }

    /// Go `ListToolsForTenant`.
    pub fn list_tools_for_tenant(&self, server_id: &str, tenant_id: &str) -> Result<Vec<ToolResource>, McpError> {
        let guard = self.inner.state.read();
        let server = guard
            .servers
            .get(server_id.trim())
            .cloned()
            .ok_or(McpError::ServerNotFound)?;
        let tenant_id = tenant_id.trim().to_string();
        if !tenant_id.is_empty() && server.tenant_id != tenant_id {
            return Err(McpError::ServerNotFound);
        }
        let mut items = Vec::new();
        if let Some(map) = guard.tools.get(&server.server_id) {
            for tool in map.values() {
                items.push(self.build_tool_resource_locked(&guard, &server, tool));
            }
        }
        Ok(items)
    }

    /// Go `CreateServer`.
    pub fn create_server(&self, input: CreateServerInput) -> Result<(ServerResource, bool), McpError> {
        self.upsert_server(input, None)
    }

    /// Go `UpdateServer`.
    pub fn update_server(&self, server_id: &str, input: &UpdateServerInput) -> Result<ServerResource, McpError> {
        let (resource, _) = self.upsert_server(
            CreateServerInput::default(),
            Some(UpdateOperation {
                server_id: server_id.trim().to_string(),
                input: input.clone(),
            }),
        )?;
        Ok(resource)
    }

    /// Go `UpdateToolExposure`.
    pub fn update_tool_exposure(
        &self,
        server_id: &str,
        tool_name: &str,
        input: &UpdateExposureInput,
    ) -> Result<ToolResource, McpError> {
        let server_id = server_id.trim().to_string();
        let tool_name = tool_name.trim().to_string();
        if server_id.is_empty() {
            return Err(McpError::ServerIDRequired);
        }
        if tool_name.is_empty() {
            return Err(McpError::ToolNameRequired);
        }
        if input.runtime_surface.trim().is_empty() {
            return Err(McpError::RuntimeSurfaceRequired);
        }

        let now = Utc::now();
        let rule = ToolExposureRule {
            server_id: server_id.clone(),
            tool_name: tool_name.clone(),
            runtime_surface: input.runtime_surface.trim().to_string(),
            exposure_mode: input.exposure_mode,
            active: input.active,
            reason: input.reason.trim().to_string(),
            updated_at: now,
            ..ToolExposureRule::default()
        };

        let resource = {
            let mut guard = self.inner.state.write();
            let server = guard
                .servers
                .get(&server_id)
                .cloned()
                .ok_or(McpError::ServerNotFound)?;
            let tool = guard
                .tools
                .get(&server_id)
                .and_then(|map| map.get(&tool_name))
                .cloned()
                .ok_or(McpError::ToolNameRequired)?;
            guard
                .exposure
                .entry(server_id.clone())
                .or_default()
                .entry(tool_name.clone())
                .or_default()
                .insert(rule.runtime_surface.clone(), rule.clone());
            self.build_tool_resource_locked(&guard, &server, &tool)
        };

        self.persist_exposure_rule(&rule)?;
        let mut payload = Map::new();
        payload.insert("serverId".to_string(), Value::String(server_id.clone()));
        payload.insert("toolName".to_string(), Value::String(tool_name.clone()));
        payload.insert("runtimeSurface".to_string(), Value::String(rule.runtime_surface.clone()));
        payload.insert("exposureMode".to_string(), Value::String(rule.exposure_mode.as_str().to_string()));
        payload.insert("active".to_string(), Value::Bool(rule.active));
        payload.insert("reason".to_string(), Value::String(rule.reason.clone()));
        self.publish_event(
            "mcp",
            "mcp.tool_exposure_updated",
            Resource {
                kind: RESOURCE_KIND_TOOL.to_string(),
                id: format!("{server_id}:{tool_name}"),
            },
            payload,
        )?;
        Ok(resource)
    }

    /// Go `AuthorizeTool`: checks the exposure rule, then either allow-lists, requests
    /// approval through the policy engine, or resolves a previously issued approval.
    pub fn authorize_tool(
        &self,
        server_id: &str,
        tool_name: &str,
        input: &AuthorizeToolInput,
    ) -> Result<ToolAuthorizationResponse, McpError> {
        let server_id = server_id.trim().to_string();
        let tool_name = tool_name.trim().to_string();
        let runtime_surface = input.runtime_surface.trim().to_string();
        if server_id.is_empty() {
            return Err(McpError::ServerIDRequired);
        }
        if tool_name.is_empty() {
            return Err(McpError::ToolNameRequired);
        }
        if runtime_surface.is_empty() {
            return Err(McpError::RuntimeSurfaceRequired);
        }

        let (server, _tool, active, rule, resource) = {
            let guard = self.inner.state.read();
            let server = guard
                .servers
                .get(&server_id)
                .cloned()
                .ok_or(McpError::ServerNotFound)?;
            let tool = guard
                .tools
                .get(&server_id)
                .and_then(|map| map.get(&tool_name))
                .cloned()
                .ok_or(McpError::ToolNameRequired)?;
            let active = guard.sessions.get(&server_id).cloned();
            let rule = guard
                .exposure
                .get(&server_id)
                .and_then(|map| map.get(&tool_name))
                .and_then(|map| map.get(&runtime_surface))
                .cloned();
            let resource = self.build_tool_resource_locked(&guard, &server, &tool);
            (server, tool, active, rule, resource)
        };

        let rule_blocked = match &rule {
            None => true,
            Some(rule) => !rule.active || rule.exposure_mode == ExposureMode::Blocked,
        };
        if rule_blocked {
            let message = first_non_empty(&[
                resource.unavailable_reason.as_str(),
                "tool is not allowlisted for this runtime surface",
            ]);
            return Ok(ToolAuthorizationResponse {
                status: ToolAuthorizationStatus::Blocked,
                tool: resource,
                message,
                ..ToolAuthorizationResponse::default()
            });
        }
        let rule = rule.expect("rule present when not blocked");
        if resource.effective_availability != "available" {
            let unavailable_reason = resource.unavailable_reason.clone();
            return Ok(ToolAuthorizationResponse {
                status: ToolAuthorizationStatus::Blocked,
                tool: resource,
                message: first_non_empty(&[
                    unavailable_reason.as_str(),
                    "tool is not currently available",
                ]),
                ..ToolAuthorizationResponse::default()
            });
        }

        let approval_mode = if rule.exposure_mode == ExposureMode::ApprovalRequired {
            kura_sandbox::ApprovalMode::Ask
        } else {
            kura_sandbox::ApprovalMode::Allow
        };
        let consumer = self.build_tool_consumer_view(
            &server,
            &tool_name,
            &runtime_surface,
            &first_non_empty(&[input.requested_by.trim(), "mcp"]),
            approval_mode,
        )?;

        if rule.exposure_mode == ExposureMode::Allow {
            self.persist_consumer_view(&consumer)?;
            return Ok(ToolAuthorizationResponse {
                status: ToolAuthorizationStatus::Allowed,
                tool: resource,
                session_id: session_id(active.as_ref()),
                sandbox: Some(consumer),
                message: "tool use is allowed".to_string(),
                ..ToolAuthorizationResponse::default()
            });
        }

        let policy = self.inner.policy.as_ref().ok_or(McpError::PolicyNotConfigured)?;
        let approval_resource_id = format!("{server_id}:{tool_name}:{runtime_surface}");
        let requested_by = first_non_empty(&[input.requested_by.trim(), "mcp"]);
        if input.approval_id.trim().is_empty() {
            let (mut approval, mut decision) = policy
                .request_approval(kura_policy::RequestApprovalInput {
                    action: "tool_call.execute".to_string(),
                    resource_kind: RESOURCE_KIND_TOOL.to_string(),
                    resource_id: approval_resource_id,
                    reason: "MCP tool execution requires approval".to_string(),
                    requested_by: requested_by.clone(),
                    ..kura_policy::RequestApprovalInput::default()
                })
                .map_err(|e| McpError::Other(e.to_string()))?;
            approval.sandbox = consumer_view_map(&consumer);
            decision.sandbox = consumer_view_map(&consumer);
            let mut consumer = consumer;
            let record = consumer
                .policy_record
                .as_mut()
                .expect("consumer view always carries a policy record");
            record.approval_id = approval.approval_id.clone();
            record.decision_id = decision.decision_id.clone();
            record.decision = kura_sandbox::DecisionResolution::Ask;
            record.approval_status = kura_sandbox::DecisionApprovalStatus::Pending;
            record.status = kura_sandbox::PolicyRecordStatus::ApprovalPending;
            record.failure_class = kura_sandbox::ErrorClass::ApprovalRequired.as_str().to_string();
            self.persist_approval(&approval)?;
            self.persist_decision(&decision)?;
            self.persist_consumer_view(&consumer)?;
            self.publish_event(
                "policy",
                "policy.approval_requested",
                Resource {
                    kind: "approval".to_string(),
                    id: approval.approval_id.clone(),
                },
                approval_payload(&approval),
            )?;
            self.publish_event(
                "policy",
                "policy.decision_recorded",
                Resource {
                    kind: "decision".to_string(),
                    id: decision.decision_id.clone(),
                },
                decision_payload(&decision),
            )?;
            return Ok(ToolAuthorizationResponse {
                status: ToolAuthorizationStatus::Pending,
                tool: resource,
                session_id: session_id(active.as_ref()),
                message: "tool use requires approval".to_string(),
                approval: Some(approval),
                decision: Some(decision),
                sandbox: Some(consumer),
                ..ToolAuthorizationResponse::default()
            });
        }

        let approval = policy
            .get_approval(input.approval_id.trim())
            .ok_or(McpError::ApprovalNotFound)?;
        if approval.action != "tool_call.execute"
            || approval.resource_kind != RESOURCE_KIND_TOOL
            || approval.resource_id != approval_resource_id
        {
            return Err(McpError::ApprovalIDInvalid);
        }
        let mut consumer = consumer;
        consumer
            .policy_record
            .as_mut()
            .expect("consumer view always carries a policy record")
            .approval_id = approval.approval_id.clone();
        match approval.status {
            kura_policy::ApprovalStatus::Approved => {
                let record = consumer
                    .policy_record
                    .as_mut()
                    .expect("consumer view always carries a policy record");
                record.decision = kura_sandbox::DecisionResolution::Allow;
                record.approval_status = kura_sandbox::DecisionApprovalStatus::Approved;
                record.status = kura_sandbox::PolicyRecordStatus::PreflightAllowed;
                record.failure_class = String::new();
                self.persist_consumer_view(&consumer)?;
                Ok(ToolAuthorizationResponse {
                    status: ToolAuthorizationStatus::Allowed,
                    tool: resource,
                    session_id: session_id(active.as_ref()),
                    message: "tool use is allowed by approval".to_string(),
                    sandbox: Some(consumer),
                    ..ToolAuthorizationResponse::default()
                })
            }
            kura_policy::ApprovalStatus::Rejected => {
                let record = consumer
                    .policy_record
                    .as_mut()
                    .expect("consumer view always carries a policy record");
                record.decision = kura_sandbox::DecisionResolution::Deny;
                record.approval_status = kura_sandbox::DecisionApprovalStatus::Rejected;
                record.status = kura_sandbox::PolicyRecordStatus::Denied;
                record.failure_class = kura_sandbox::ErrorClass::ApprovalRejected.as_str().to_string();
                self.persist_consumer_view(&consumer)?;
                Ok(ToolAuthorizationResponse {
                    status: ToolAuthorizationStatus::Rejected,
                    tool: resource,
                    session_id: session_id(active.as_ref()),
                    message: "approval was rejected".to_string(),
                    approval: Some(approval),
                    sandbox: Some(consumer),
                    ..ToolAuthorizationResponse::default()
                })
            }
            kura_policy::ApprovalStatus::Pending => {
                let record = consumer
                    .policy_record
                    .as_mut()
                    .expect("consumer view always carries a policy record");
                record.decision = kura_sandbox::DecisionResolution::Ask;
                record.approval_status = kura_sandbox::DecisionApprovalStatus::Pending;
                record.status = kura_sandbox::PolicyRecordStatus::ApprovalPending;
                record.failure_class = kura_sandbox::ErrorClass::ApprovalRequired.as_str().to_string();
                self.persist_consumer_view(&consumer)?;
                Ok(ToolAuthorizationResponse {
                    status: ToolAuthorizationStatus::Pending,
                    tool: resource,
                    session_id: session_id(active.as_ref()),
                    message: "approval is still pending".to_string(),
                    approval: Some(approval),
                    sandbox: Some(consumer),
                    ..ToolAuthorizationResponse::default()
                })
            }
        }
    }

    /// Go `InstallCatalogEntry`.
    pub fn install_catalog_entry(
        &self,
        entry_id: &str,
        input: &CatalogInstallInput,
        method: InstallMethod,
    ) -> Result<CatalogInstallResult, McpError> {
        let entry = self.get_catalog_entry(entry_id).ok_or(McpError::ServerNotFound)?;
        let install_id = format!("mcp_install_{}", Utc::now().timestamp_nanos_opt().unwrap_or(0));
        let mut requested_payload = Map::new();
        requested_payload.insert("installId".to_string(), Value::String(install_id.clone()));
        requested_payload.insert("catalogEntryId".to_string(), Value::String(entry.id.clone()));
        requested_payload.insert("method".to_string(), Value::String(method.as_str().to_string()));
        requested_payload.insert(
            "environment".to_string(),
            Value::String(environment_scope(self.inner.cfg.environment)),
        );
        let requested_event = self.publish_audit_event(
            "mcp.catalog_install_requested",
            Resource {
                kind: "mcp_catalog_install".to_string(),
                id: install_id.clone(),
            },
            requested_payload,
        )?;

        let create_input = merge_catalog_install_input(&entry, input, method, self.inner.cfg.environment);
        let (install_availability, install_reason) = evaluate_catalog_install_spec_availability(
            &self.inner.cfg,
            &create_input,
            &entry.secret_requirements,
        );
        if install_availability != AvailabilityStatus::Ready {
            let mut result = CatalogInstallResult {
                install_id: install_id.clone(),
                status: "blocked".to_string(),
                catalog_entry_id: entry.id.clone(),
                server_id: create_input.server_id.clone(),
                availability_status: install_availability,
                availability_reason: install_reason.clone(),
                audit_event_ids: vec![requested_event.event_id.clone()],
                ..CatalogInstallResult::default()
            };
            let mut failed_payload = Map::new();
            failed_payload.insert("installId".to_string(), Value::String(install_id.clone()));
            failed_payload.insert("catalogEntryId".to_string(), Value::String(entry.id.clone()));
            failed_payload.insert("method".to_string(), Value::String(method.as_str().to_string()));
            failed_payload.insert("status".to_string(), Value::String(result.status.clone()));
            failed_payload.insert(
                "availabilityStatus".to_string(),
                Value::String(result.availability_status.as_str().to_string()),
            );
            failed_payload.insert(
                "availabilityReason".to_string(),
                Value::String(result.availability_reason.clone()),
            );
            if let Ok(failed_event) = self.publish_audit_event(
                "mcp.catalog_install_failed",
                Resource {
                    kind: "mcp_catalog_install".to_string(),
                    id: install_id,
                },
                failed_payload,
            ) {
                result.audit_event_ids.push(failed_event.event_id.clone());
            }
            return Ok(result);
        }

        if let Some(existing) = self.get_server(&create_input.server_id) {
            if let Some(reason) = catalog_install_conflict_reason(&existing, &entry.id) {
                let mut result = CatalogInstallResult {
                    install_id: install_id.clone(),
                    status: "blocked".to_string(),
                    catalog_entry_id: entry.id.clone(),
                    server_id: existing.server_id.clone(),
                    availability_status: AvailabilityStatus::Blocked,
                    availability_reason: reason.clone(),
                    audit_event_ids: vec![requested_event.event_id.clone()],
                    ..CatalogInstallResult::default()
                };
                let mut failed_payload = Map::new();
                failed_payload.insert("installId".to_string(), Value::String(install_id.clone()));
                failed_payload.insert("catalogEntryId".to_string(), Value::String(entry.id.clone()));
                failed_payload.insert("serverId".to_string(), Value::String(existing.server_id.clone()));
                failed_payload.insert("method".to_string(), Value::String(method.as_str().to_string()));
                failed_payload.insert("status".to_string(), Value::String(result.status.clone()));
                failed_payload.insert(
                    "availabilityStatus".to_string(),
                    Value::String(result.availability_status.as_str().to_string()),
                );
                failed_payload.insert(
                    "availabilityReason".to_string(),
                    Value::String(result.availability_reason.clone()),
                );
                if let Ok(failed_event) = self.publish_audit_event(
                    "mcp.catalog_install_failed",
                    Resource {
                        kind: "mcp_catalog_install".to_string(),
                        id: install_id,
                    },
                    failed_payload,
                ) {
                    result.audit_event_ids.push(failed_event.event_id.clone());
                }
                return Ok(result);
            }
        }

        let mut create_input = create_input;
        create_input.catalog_management = Some(catalog_management_for_create(
            &entry,
            &create_input,
            None,
            CatalogAction::Install,
            Utc::now(),
        ));
        let (resource, _) = match self.create_server(create_input.clone()) {
            Ok(created) => created,
            Err(err) => {
                let mut failed_payload = Map::new();
                failed_payload.insert("installId".to_string(), Value::String(install_id.clone()));
                failed_payload.insert("catalogEntryId".to_string(), Value::String(entry.id.clone()));
                failed_payload.insert("method".to_string(), Value::String(method.as_str().to_string()));
                failed_payload.insert("status".to_string(), Value::String("failed".to_string()));
                failed_payload.insert("reason".to_string(), Value::String(err.to_string()));
                let _ = self.publish_audit_event(
                    "mcp.catalog_install_failed",
                    Resource {
                        kind: "mcp_catalog_install".to_string(),
                        id: install_id,
                    },
                    failed_payload,
                );
                return Err(err);
            }
        };
        let mut result = CatalogInstallResult {
            install_id: install_id.clone(),
            status: "installed".to_string(),
            catalog_entry_id: entry.id.clone(),
            server_id: resource.server.server_id.clone(),
            availability_status: resource.availability_status,
            availability_reason: resource.availability_reason.clone(),
            audit_event_ids: vec![requested_event.event_id.clone()],
            server: Some(resource.clone()),
            ..CatalogInstallResult::default()
        };
        let mut completed_payload = Map::new();
        completed_payload.insert("installId".to_string(), Value::String(install_id.clone()));
        completed_payload.insert("catalogEntryId".to_string(), Value::String(entry.id.clone()));
        completed_payload.insert("serverId".to_string(), Value::String(resource.server.server_id.clone()));
        completed_payload.insert("method".to_string(), Value::String(method.as_str().to_string()));
        completed_payload.insert("status".to_string(), Value::String(result.status.clone()));
        completed_payload.insert(
            "availabilityStatus".to_string(),
            Value::String(result.availability_status.as_str().to_string()),
        );
        completed_payload.insert(
            "availabilityReason".to_string(),
            Value::String(result.availability_reason.clone()),
        );
        if let Ok(completed_event) = self.publish_audit_event(
            "mcp.catalog_install_completed",
            Resource {
                kind: "mcp_catalog_install".to_string(),
                id: install_id,
            },
            completed_payload,
        ) {
            result.audit_event_ids.push(completed_event.event_id.clone());
        }
        Ok(result)
    }

    /// Go `RefreshCatalogServer`.
    pub fn refresh_catalog_server(&self, server_id: &str) -> Result<CatalogLifecycleResult, McpError> {
        self.run_catalog_lifecycle_action(server_id, CatalogAction::Refresh)
    }

    /// Go `ReinstallCatalogServer`.
    pub fn reinstall_catalog_server(&self, server_id: &str) -> Result<CatalogLifecycleResult, McpError> {
        self.run_catalog_lifecycle_action(server_id, CatalogAction::Reinstall)
    }

    /// Go `UninstallCatalogServer`.
    pub fn uninstall_catalog_server(&self, server_id: &str) -> Result<CatalogLifecycleResult, McpError> {
        self.run_catalog_lifecycle_action(server_id, CatalogAction::Uninstall)
    }

    /// Go `RevalidateCatalogServer`.
    pub fn revalidate_catalog_server(&self, server_id: &str) -> Result<CatalogRevalidationResult, McpError> {
        let started_at = Instant::now();
        let server_id = server_id.trim().to_string();
        if server_id.is_empty() {
            return Err(McpError::ServerIDRequired);
        }
        let server = self.get_server(&server_id).ok_or(McpError::ServerNotFound)?;
        let mut result = CatalogRevalidationResult {
            action_id: format!("mcp_revalidate_{}", Utc::now().timestamp_nanos_opt().unwrap_or(0)),
            action: CatalogAction::Revalidate,
            server_id: server.server_id.clone(),
            catalog_entry_id: server.catalog_entry_id.clone(),
            ..CatalogRevalidationResult::default()
        };
        let mut requested_payload = Map::new();
        requested_payload.insert("actionId".to_string(), Value::String(result.action_id.clone()));
        requested_payload.insert("action".to_string(), Value::String(result.action.as_str().to_string()));
        requested_payload.insert("serverId".to_string(), Value::String(server.server_id.clone()));
        requested_payload.insert(
            "catalogEntryId".to_string(),
            Value::String(server.catalog_entry_id.clone()),
        );
        requested_payload.insert(
            "environment".to_string(),
            Value::String(environment_scope(self.inner.cfg.environment)),
        );
        let requested_event = self.publish_audit_event(
            "mcp.catalog_lifecycle_requested",
            Resource {
                kind: RESOURCE_KIND_SERVER.to_string(),
                id: server.server_id.clone(),
            },
            requested_payload,
        )?;
        result.audit_event_ids.push(requested_event.event_id.clone());

        if let Some(blocked) = self.catalog_target_block_result(&server) {
            return Ok(self.catalog_revalidation_blocked_result(&server, &result, &blocked, started_at));
        }
        if let Some(blocked) = self.catalog_revalidation_busy_block_result(&server)? {
            return Ok(self.catalog_revalidation_blocked_result(&server, &result, &blocked, started_at));
        }

        let management = self.build_catalog_management_locked(&server);
        let (issues, status, classification, reason) =
            self.collect_revalidation_issues(&server, management.as_ref());
        let checked_at = Utc::now();
        let mut server = server;
        server.catalog_management = management;
        if server.catalog_management.is_none() {
            server.catalog_management = Some(CatalogManagement::default());
        }
        if let Some(cm) = &mut server.catalog_management {
            cm.last_action = Some(CatalogAction::Revalidate);
            cm.last_action_status = Some(CatalogActionStatus::Completed);
            cm.last_action_failure_class = String::new();
            cm.last_action_reason = reason.clone();
            cm.last_action_at = Some(checked_at);
            cm.last_revalidation = Some(RevalidationSnapshot {
                checked_at,
                status,
                classification,
                reason: reason.clone(),
                issues: issues.clone(),
            });
        }
        self.set_server(&server);
        self.persist_server(&server)?;

        result.status = status;
        result.classification = classification;
        result.reason = reason.clone();
        result.issues = issues.clone();
        result.preflight_ms = started_at.elapsed().as_millis() as i64;
        if let Some(resource) = self.get_server_resource(&server.server_id) {
            result.server = Some(resource);
        }
        let mut completed_payload = Map::new();
        completed_payload.insert("actionId".to_string(), Value::String(result.action_id.clone()));
        completed_payload.insert("action".to_string(), Value::String(result.action.as_str().to_string()));
        completed_payload.insert("serverId".to_string(), Value::String(server.server_id.clone()));
        completed_payload.insert(
            "catalogEntryId".to_string(),
            Value::String(server.catalog_entry_id.clone()),
        );
        completed_payload.insert("status".to_string(), Value::String(result.status.as_str().to_string()));
        completed_payload.insert(
            "classification".to_string(),
            Value::String(result.classification.as_str().to_string()),
        );
        completed_payload.insert("reason".to_string(), Value::String(result.reason.clone()));
        completed_payload.insert("issues".to_string(), Value::Array(redacted_issues(&result.issues)));
        completed_payload.insert(
            "environment".to_string(),
            Value::String(environment_scope(self.inner.cfg.environment)),
        );
        if let Ok(completed_event) = self.publish_audit_event(
            "mcp.catalog_revalidation_completed",
            Resource {
                kind: RESOURCE_KIND_SERVER.to_string(),
                id: server.server_id.clone(),
            },
            completed_payload,
        ) {
            result.audit_event_ids.push(completed_event.event_id.clone());
        }
        Ok(result)
    }

    /// Go `CallTool`.
    pub fn call_tool(
        &self,
        server_id: &str,
        tool_name: &str,
        input: Value,
        authorization: &ToolAuthorizationResponse,
    ) -> Result<ToolInvocationResult, McpError> {
        let server_id = server_id.trim().to_string();
        let tool_name = tool_name.trim().to_string();
        if server_id.is_empty() {
            return Err(McpError::ServerIDRequired);
        }
        if tool_name.is_empty() {
            return Err(McpError::ToolNameRequired);
        }
        if authorization.status != ToolAuthorizationStatus::Allowed {
            return Ok(ToolInvocationResult {
                failure_class: "blocked".to_string(),
                error: first_non_empty(&[authorization.message.as_str(), "tool use is not allowed"]),
                ..ToolInvocationResult::default()
            });
        }
        let (active, server) = {
            let guard = self.inner.state.read();
            let active = guard.sessions.get(&server_id).cloned();
            let server = guard.servers.get(&server_id).cloned();
            (active, server)
        };
        let Some(server) = server else {
            // Go returns both the result and ErrServerNotFound; Rust surfaces the error.
            return Err(McpError::ServerNotFound);
        };
        let Some(active) = active else {
            return Ok(ToolInvocationResult {
                failure_class: "server_unhealthy".to_string(),
                error: "mcp server is not healthy".to_string(),
                ..ToolInvocationResult::default()
            });
        };
        let Some(session) = &active.session else {
            return Ok(ToolInvocationResult {
                failure_class: "server_unhealthy".to_string(),
                error: "mcp server is not healthy".to_string(),
                ..ToolInvocationResult::default()
            });
        };
        match session.call_tool(&tool_name, input) {
            Err(err) => Ok(ToolInvocationResult {
                session_id: active.session_id.clone(),
                failure_class: "transport_failed".to_string(),
                error: err,
                ..ToolInvocationResult::default()
            }),
            Ok(output) => {
                let redacted = self.redact_value(&server, Value::Object(output.clone()));
                let is_error = output
                    .get("isError")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if is_error {
                    Ok(ToolInvocationResult {
                        session_id: active.session_id.clone(),
                        output: Some(redacted),
                        failure_class: "remote_tool_error".to_string(),
                        error: first_non_empty(&[
                            string_from_map(&output, "message").as_str(),
                            "remote MCP tool returned an error",
                        ]),
                        ..ToolInvocationResult::default()
                    })
                } else {
                    Ok(ToolInvocationResult {
                        session_id: active.session_id.clone(),
                        output: Some(redacted),
                        ..ToolInvocationResult::default()
                    })
                }
            }
        }
    }

    /// Go `Start`: opens a transport session (stdio via the sandbox execution plane,
    /// streamable-http/websocket via the transport), discovers tools, and marks the
    /// server healthy. The transport mux dispatches on the server's transport kind and
    /// a failed open surfaces as `transport_runtime_failure`.
    pub fn start(&self, server_id: &str, requested_by: &str) -> Result<LifecycleResponse, McpError> {
        let started_at = Instant::now();
        let server_id = server_id.trim().to_string();
        if server_id.is_empty() {
            return Err(McpError::ServerIDRequired);
        }
        if self.inner.transport.is_none() {
            return Err(McpError::TransportNotConfigured);
        }
        let restore_request = is_restore_lifecycle_requester(requested_by);
        let reconnect_request = is_websocket_reconnect_requester(requested_by);

        let (server, mut state) = {
            let mut guard = self.inner.state.write();
            let server = match guard.servers.get(&server_id) {
                Some(server) => server.clone(),
                None => return Err(McpError::ServerNotFound),
            };
            if let Some(active) = guard.sessions.get(&server_id) {
                let resource = self.build_server_resource_locked(&guard, &server);
                return Ok(LifecycleResponse {
                    action: LifecycleAction::Start,
                    server: resource,
                    idempotent: true,
                    execution_id: active.execution_id.clone(),
                    preflight_ms: started_at.elapsed().as_millis() as i64,
                    ..LifecycleResponse::default()
                });
            }
            let mut state = guard.states.get(&server_id).cloned().unwrap_or_default();
            if !server.enabled {
                state.status = LifecycleStatus::Disabled;
                state.updated_at = Utc::now();
                guard.states.insert(server_id.clone(), state.clone());
                let resource = self.build_server_resource_locked(&guard, &server);
                let persisted = state.clone();
                drop(guard);
                let _ = self.persist_state(&persisted);
                return Ok(LifecycleResponse {
                    action: LifecycleAction::Start,
                    server: resource,
                    idempotent: true,
                    blocked: true,
                    blocked_reason: "server is disabled".to_string(),
                    preflight_ms: started_at.elapsed().as_millis() as i64,
                    ..LifecycleResponse::default()
                });
            }
            state.status = LifecycleStatus::Starting;
            state.health_reason = String::new();
            state.next_reconnect_at = None;
            if restore_request {
                state.last_recovery_at = Some(Utc::now());
                state.last_recovery_class = "restore_requested".to_string();
            } else if !reconnect_request {
                state.last_recovery_at = None;
                state.last_recovery_class = String::new();
            }
            state.updated_at = Utc::now();
            guard.states.insert(server_id.clone(), state.clone());
            (server, state)
        };
        if let Err(err) = self.persist_state(&state) {
            return Err(err);
        }

        let requested_by = first_non_empty(&[requested_by.trim(), "mcp"]);
        let consumer = match self.build_lifecycle_consumer_view(&server, &requested_by) {
            Ok(consumer) => consumer,
            Err(err) => {
                if restore_request {
                    self.record_restore_failure(
                        &server,
                        &mut state,
                        LifecycleStatus::Denied,
                        &err.to_string(),
                        "invalid_configuration",
                    );
                } else {
                    self.record_failure(
                        &server_id,
                        &mut state,
                        LifecycleStatus::Denied,
                        &err.to_string(),
                        "invalid_configuration",
                    );
                }
                let resource = self.get_server_resource(&server_id).unwrap_or_default();
                return Ok(LifecycleResponse {
                    action: LifecycleAction::Start,
                    server: resource,
                    failure_class: "invalid_configuration".to_string(),
                    blocked: true,
                    blocked_reason: err.to_string(),
                    preflight_ms: started_at.elapsed().as_millis() as i64,
                    ..LifecycleResponse::default()
                });
            }
        };

        let mut pipes = SessionPipes::default();
        let mut execution_id = String::new();
        let mut transport_server = server.clone();
        if server.transport_kind == TransportKind::Stdio {
            let sandboxes = match &self.inner.sandboxes {
                Some(sandboxes) => Arc::clone(sandboxes),
                None => return Err(McpError::SandboxManagerMissing),
            };
            let request = match self.build_execution_request(&server, &consumer, "") {
                Ok(request) => request,
                Err(err) => {
                    if restore_request {
                        self.record_restore_failure(
                            &server,
                            &mut state,
                            LifecycleStatus::Denied,
                            &err.to_string(),
                            "invalid_configuration",
                        );
                    } else {
                        self.record_failure(
                            &server_id,
                            &mut state,
                            LifecycleStatus::Denied,
                            &err.to_string(),
                            "invalid_configuration",
                        );
                    }
                    let resource = self.get_server_resource(&server_id).unwrap_or_default();
                    return Ok(LifecycleResponse {
                        action: LifecycleAction::Start,
                        server: resource,
                        failure_class: "invalid_configuration".to_string(),
                        blocked: true,
                        blocked_reason: err.to_string(),
                        preflight_ms: started_at.elapsed().as_millis() as i64,
                        ..LifecycleResponse::default()
                    });
                }
            };
            match sandboxes.start_attached_execution(&request) {
                Ok((_execution, Some(attached))) => {
                    execution_id = attached.execution.execution_id.clone();
                    pipes = SessionPipes {
                        stdin: attached.stdin,
                        stdout: attached.stdout,
                        stderr: attached.stderr,
                    };
                }
                Ok((execution, None)) => {
                    self.update_state_from_execution(&server_id, &mut state, &execution, false);
                    let resource = self.get_server_resource(&server_id).unwrap_or_default();
                    return Ok(LifecycleResponse {
                        action: LifecycleAction::Start,
                        server: resource,
                        execution_id: execution.execution_id.clone(),
                        failure_class: classify_execution_failure(&execution),
                        blocked: true,
                        blocked_reason: first_non_empty(&[
                            execution.result.error.as_str(),
                            execution.decision.explanation.as_str(),
                            state.health_reason.as_str(),
                        ]),
                        preflight_ms: started_at.elapsed().as_millis() as i64,
                        ..LifecycleResponse::default()
                    });
                }
                Err(err) => {
                    if restore_request {
                        self.record_restore_failure(
                            &server,
                            &mut state,
                            LifecycleStatus::Failed,
                            &err,
                            "launch_failed",
                        );
                    } else {
                        self.record_failure(&server_id, &mut state, LifecycleStatus::Failed, &err, "launch_failed");
                    }
                    let resource = self.get_server_resource(&server_id).unwrap_or_default();
                    return Ok(LifecycleResponse {
                        action: LifecycleAction::Start,
                        server: resource,
                        failure_class: "launch_failed".to_string(),
                        preflight_ms: started_at.elapsed().as_millis() as i64,
                        ..LifecycleResponse::default()
                    });
                }
            }
        }
        if server.transport_kind == TransportKind::Websocket {
            match self.resolve_websocket_headers(&server) {
                Ok(headers) => {
                    transport_server.resolved_websocket_headers = headers;
                }
                Err(err) => {
                    if restore_request {
                        self.record_restore_failure(
                            &server,
                            &mut state,
                            LifecycleStatus::Denied,
                            &err.to_string(),
                            "invalid_configuration",
                        );
                    } else {
                        self.record_failure(
                            &server_id,
                            &mut state,
                            LifecycleStatus::Denied,
                            &err.to_string(),
                            "invalid_configuration",
                        );
                    }
                    let resource = self.get_server_resource(&server_id).unwrap_or_default();
                    return Ok(LifecycleResponse {
                        action: LifecycleAction::Start,
                        server: resource,
                        failure_class: "invalid_configuration".to_string(),
                        blocked: true,
                        blocked_reason: err.to_string(),
                        preflight_ms: started_at.elapsed().as_millis() as i64,
                        ..LifecycleResponse::default()
                    });
                }
            }
        }

        let timeout = session_start_timeout();
        let session = match self
            .inner
            .transport
            .as_ref()
            .expect("transport defaults to a mux")
            .open(&transport_server, pipes, timeout)
        {
            Ok(session) => session,
            Err(err) => {
                if !execution_id.is_empty() {
                    if let Some(sandboxes) = &self.inner.sandboxes {
                        let _ = sandboxes.cancel_execution(&execution_id);
                    }
                }
                if restore_request {
                    self.record_restore_failure(
                        &server,
                        &mut state,
                        LifecycleStatus::Failed,
                        &err.to_string(),
                        "transport_runtime_failure",
                    );
                } else {
                    self.record_failure(
                        &server_id,
                        &mut state,
                        LifecycleStatus::Failed,
                        &err.to_string(),
                        "transport_runtime_failure",
                    );
                }
                let resource = self.get_server_resource(&server_id).unwrap_or_default();
                return Ok(LifecycleResponse {
                    action: LifecycleAction::Start,
                    server: resource,
                    execution_id: execution_id.clone(),
                    failure_class: "transport_runtime_failure".to_string(),
                    preflight_ms: started_at.elapsed().as_millis() as i64,
                    ..LifecycleResponse::default()
                });
            }
        };

        let tools = match session.list_tools(timeout) {
            Ok(tools) => tools,
            Err(err) => {
                let _ = session.close();
                if !execution_id.is_empty() {
                    if let Some(sandboxes) = &self.inner.sandboxes {
                        let _ = sandboxes.cancel_execution(&execution_id);
                    }
                }
                if restore_request {
                    self.record_restore_failure(
                        &server,
                        &mut state,
                        LifecycleStatus::Failed,
                        &err,
                        "transport_runtime_failure",
                    );
                } else {
                    self.record_failure(
                        &server_id,
                        &mut state,
                        LifecycleStatus::Failed,
                        &err,
                        "transport_runtime_failure",
                    );
                }
                let resource = self.get_server_resource(&server_id).unwrap_or_default();
                return Ok(LifecycleResponse {
                    action: LifecycleAction::Start,
                    server: resource,
                    execution_id: execution_id.clone(),
                    failure_class: "transport_runtime_failure".to_string(),
                    preflight_ms: started_at.elapsed().as_millis() as i64,
                    ..LifecycleResponse::default()
                });
            }
        };

        let now = Utc::now();
        let session_id = session.id();
        if state.last_started_at.is_some() {
            state.restart_count += 1;
        }
        state.status = LifecycleStatus::Healthy;
        state.last_execution_id = execution_id.clone();
        state.last_session_id = session_id.clone();
        state.last_started_at = Some(now);
        state.last_heartbeat_at = Some(now);
        state.health_reason = String::new();
        state.failure_count = 0;
        state.reconnect_attempt_count = 0;
        state.next_reconnect_at = None;
        if restore_request {
            state.last_recovery_at = Some(now);
            state.last_recovery_class = "restore_succeeded".to_string();
        }
        state.updated_at = now;

        let (_resource, persisted_tools) = {
            let mut guard = self.inner.state.write();
            guard.states.insert(server_id.clone(), state.clone());
            guard.sessions.insert(
                server_id.clone(),
                SessionState {
                    session_id: session_id.clone(),
                    execution_id: execution_id.clone(),
                    session: Some(Arc::clone(&session)),
                    transport_kind: server.transport_kind,
                    stop_requested: false,
                    cancel_requested: false,
                },
            );
            {
                let tool_map = guard.tools.entry(server_id.clone()).or_default();
                for existing in tool_map.values_mut() {
                    existing.discovery_status = DiscoveryStatus::Stale;
                    existing.updated_at = now;
                }
                for mut tool in tools {
                    tool.server_id = server_id.clone();
                    tool.discovery_status = DiscoveryStatus::Discovered;
                    tool.updated_at = now;
                    tool.last_discovered_at = Some(now);
                    tool_map.insert(tool.tool_name.clone(), tool);
                }
            }
            let resource = self.build_server_resource_locked(&guard, &server);
            let persisted_tools = guard
                .tools
                .get(&server_id)
                .map(clone_tool_map)
                .unwrap_or_default();
            (resource, persisted_tools)
        };

        self.persist_state(&state)?;
        self.persist_tool_map(&server_id, &persisted_tools)?;
        let mut start_payload = Map::new();
        start_payload.insert("serverId".to_string(), Value::String(server_id.clone()));
        start_payload.insert("status".to_string(), Value::String(state.status.as_str().to_string()));
        start_payload.insert("executionId".to_string(), Value::String(execution_id.clone()));
        start_payload.insert("sessionId".to_string(), Value::String(session_id.clone()));
        start_payload.insert("toolCount".to_string(), Value::Number(persisted_tools.len().into()));
        start_payload.insert(
            "transportKind".to_string(),
            Value::String(server.transport_kind.as_str().to_string()),
        );
        self.publish_event(
            "mcp",
            "mcp.server_started",
            Resource {
                kind: RESOURCE_KIND_SERVER.to_string(),
                id: server_id.clone(),
            },
            start_payload,
        )?;
        self.publish_health_changed(&server_id, state.status, &state.health_reason)?;
        if restore_request {
            let mut restore_payload = Map::new();
            restore_payload.insert("serverId".to_string(), Value::String(server_id.clone()));
            restore_payload.insert(
                "transportKind".to_string(),
                Value::String(server.transport_kind.as_str().to_string()),
            );
            restore_payload.insert("sessionId".to_string(), Value::String(session_id.clone()));
            restore_payload.insert("toolCount".to_string(), Value::Number(persisted_tools.len().into()));
            self.publish_event(
                "mcp",
                "mcp.server_restore_completed",
                Resource {
                    kind: RESOURCE_KIND_SERVER.to_string(),
                    id: server_id.clone(),
                },
                restore_payload,
            )?;
        }

        let this = self.clone();
        let watcher_server_id = server_id.clone();
        let watcher_execution_id = execution_id.clone();
        std::thread::spawn(move || this.watch_session(&watcher_server_id, &watcher_execution_id, session));

        let resource = self.get_server_resource(&server_id).unwrap_or_default();
        Ok(LifecycleResponse {
            action: LifecycleAction::Start,
            server: resource,
            execution_id: execution_id.clone(),
            preflight_ms: started_at.elapsed().as_millis() as i64,
            idempotent: false,
            failure_class: String::new(),
            ..LifecycleResponse::default()
        })
    }

    /// Go `Stop`.
    pub fn stop(&self, server_id: &str) -> Result<LifecycleResponse, McpError> {
        self.stop_or_cancel(server_id, false)
    }

    /// Go `Cancel`.
    pub fn cancel(&self, server_id: &str) -> Result<LifecycleResponse, McpError> {
        self.stop_or_cancel(server_id, true)
    }

    /// Go `Restart`.
    pub fn restart(&self, server_id: &str, requested_by: &str) -> Result<LifecycleResponse, McpError> {
        match self.stop_or_cancel(server_id, false) {
            Ok(_) => {}
            Err(McpError::ServerNotFound) => {}
            Err(err) => return Err(err),
        }
        let mut response = self.start(server_id, requested_by)?;
        response.action = LifecycleAction::Restart;
        Ok(response)
    }

    // ---------------------------------------------------------------------------
    // Internal helpers
    // ---------------------------------------------------------------------------

    /// Go `watchSession`: processes one session termination (blocking on the session's
    /// done receiver), then reconciles state and schedules restarts/reconnects. Runs on
    /// a detached thread from `start`.
    pub fn watch_session(&self, server_id: &str, execution_id: &str, session: Arc<dyn Session>) {
        let done = session.wait_done();
        let (active, server, state, stop_requested, cancel_requested, transport_kind) = {
            let mut guard = self.inner.state.write();
            let active = guard.sessions.get(server_id).cloned();
            if let Some(active_ref) = &active {
                if let Some(active_session) = &active_ref.session {
                    if Arc::ptr_eq(active_session, &session) {
                        guard.sessions.remove(server_id);
                    }
                }
            }
            let server = guard.servers.get(server_id).cloned();
            let state = guard.states.get(server_id).cloned().unwrap_or_default();
            let stop_requested = active.as_ref().is_some_and(|a| a.stop_requested);
            let cancel_requested = active.as_ref().is_some_and(|a| a.cancel_requested);
            let transport_kind = active
                .as_ref()
                .map(|a| a.transport_kind)
                .unwrap_or(TransportKind::Stdio);
            (active, server, state, stop_requested, cancel_requested, transport_kind)
        };
        let _ = active;
        let Some(server) = server else {
            return;
        };
        let mut state = state;
        if execution_id.is_empty() {
            if stop_requested || cancel_requested {
                let now = Utc::now();
                state.status = LifecycleStatus::Stopped;
                state.last_stopped_at = Some(now);
                state.updated_at = now;
                self.inner.state.write().states.insert(server_id.to_string(), state.clone());
                let _ = self.persist_state(&state);
                let _ = self.publish_health_changed(server_id, state.status, &state.health_reason);
                return;
            }
            if let Err(err) = &done {
                if transport_kind == TransportKind::Websocket && server.enabled && server.auto_restart {
                    self.schedule_websocket_reconnect(server_id, &state, Some(err));
                    return;
                }
                self.record_failure(
                    server_id,
                    &mut state,
                    LifecycleStatus::Failed,
                    err,
                    "transport_runtime_failure",
                );
            }
            if state.status == LifecycleStatus::Failed && server.enabled && server.auto_restart {
                self.schedule_restart(server_id, &state);
            }
            return;
        }

        if let Some(sandboxes) = &self.inner.sandboxes {
            if let Some(execution) = sandboxes.get_execution(execution_id) {
                self.update_state_from_execution(
                    server_id,
                    &mut state,
                    &execution,
                    stop_requested || cancel_requested,
                );
            } else if done.is_err() {
                self.record_failure(
                    server_id,
                    &mut state,
                    LifecycleStatus::Failed,
                    &crate::error_string(done.as_ref().err()),
                    "transport_runtime_failure",
                );
            }
        }
        if state.status == LifecycleStatus::Failed && server.enabled && server.auto_restart {
            self.schedule_restart(server_id, &state);
        }
    }

    /// Go `scheduleRestart`: marks the server backing-off and starts a detached timer
    /// thread that re-starts it after the backoff delay.
    fn schedule_restart(&self, server_id: &str, state: &ServerState) {
        let delay = mcp_backoff_delay(state.failure_count);
        let next = Utc::now() + chrono::Duration::from_std(delay).unwrap_or_default();
        let mut state = state.clone();
        state.status = LifecycleStatus::BackingOff;
        state.next_restart_at = Some(next);
        state.updated_at = Utc::now();
        self.inner.state.write().states.insert(server_id.to_string(), state.clone());
        let _ = self.persist_state(&state);
        let _ = self.publish_health_changed(server_id, state.status, &state.health_reason);

        let this = self.clone();
        let watcher_server_id = server_id.to_string();
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            let _ = this.start(&watcher_server_id, "mcp.auto_restart");
        });
    }

    /// Go `scheduleWebsocketReconnect`: bounded daemon-managed reconnect with backoff.
    fn schedule_websocket_reconnect(&self, server_id: &str, state: &ServerState, cause: Option<&String>) {
        let now = Utc::now();
        let attempt = state.reconnect_attempt_count + 1;
        let reason = first_non_empty(&[
            crate::error_string(cause).as_str(),
            state.health_reason.as_str(),
            "websocket session disconnected",
        ]);
        if attempt > WEBSOCKET_RECONNECT_MAX_ATTEMPTS {
            let mut state = state.clone();
            state.status = LifecycleStatus::Failed;
            state.health_reason = reason.clone();
            state.last_recovery_at = Some(now);
            state.last_recovery_class = "reconnect_failed".to_string();
            state.next_reconnect_at = None;
            state.updated_at = now;
            self.inner.state.write().states.insert(server_id.to_string(), state.clone());
            let _ = self.persist_state(&state);
            let mut payload = Map::new();
            payload.insert("serverId".to_string(), Value::String(server_id.to_string()));
            payload.insert(
                "transportKind".to_string(),
                Value::String(TransportKind::Websocket.as_str().to_string()),
            );
            payload.insert(
                "attempt".to_string(),
                Value::Number(state.reconnect_attempt_count.into()),
            );
            payload.insert("reason".to_string(), Value::String(reason.clone()));
            payload.insert("failureClass".to_string(), Value::String("reconnect_exhausted".to_string()));
            let _ = self.publish_event(
                "mcp",
                "mcp.server_reconnect_failed",
                Resource {
                    kind: RESOURCE_KIND_SERVER.to_string(),
                    id: server_id.to_string(),
                },
                payload,
            );
            let _ = self.publish_health_changed(server_id, state.status, &state.health_reason);
            return;
        }

        let delay = mcp_backoff_delay(attempt);
        let next = now + chrono::Duration::from_std(delay).unwrap_or_default();
        let mut state = state.clone();
        state.status = LifecycleStatus::Degraded;
        state.health_reason = reason.clone();
        state.reconnect_attempt_count = attempt;
        state.last_recovery_at = Some(now);
        state.last_recovery_class = "reconnect_scheduled".to_string();
        state.next_reconnect_at = Some(next);
        state.updated_at = now;
        self.inner.state.write().states.insert(server_id.to_string(), state.clone());
        let _ = self.persist_state(&state);
        let mut payload = Map::new();
        payload.insert("serverId".to_string(), Value::String(server_id.to_string()));
        payload.insert(
            "transportKind".to_string(),
            Value::String(TransportKind::Websocket.as_str().to_string()),
        );
        payload.insert("attempt".to_string(), Value::Number(attempt.into()));
        payload.insert("reason".to_string(), Value::String(reason.clone()));
        payload.insert(
            "nextRetryAt".to_string(),
            Value::String(rfc3339_nano(next)),
        );
        let _ = self.publish_event(
            "mcp",
            "mcp.server_reconnect_scheduled",
            Resource {
                kind: RESOURCE_KIND_SERVER.to_string(),
                id: server_id.to_string(),
            },
            payload,
        );
        let _ = self.publish_health_changed(server_id, state.status, &state.health_reason);

        let this = self.clone();
        let watcher_server_id = server_id.to_string();
        let expected_attempt = attempt;
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            let response = this
                .start(&watcher_server_id, "mcp.websocket_reconnect")
                .unwrap_or_default();
            if response.server.state.status == LifecycleStatus::Healthy {
                let recovered_at = Utc::now();
                let mut latest = {
                    let guard = this.inner.state.read();
                    guard.states.get(&watcher_server_id).cloned().unwrap_or_default()
                };
                latest.last_recovery_at = Some(recovered_at);
                latest.last_recovery_class = "reconnect_succeeded".to_string();
                latest.reconnect_attempt_count = 0;
                latest.next_reconnect_at = None;
                latest.updated_at = recovered_at;
                this.inner
                    .state
                    .write()
                    .states
                    .insert(watcher_server_id.clone(), latest.clone());
                let _ = this.persist_state(&latest);
                let mut payload = Map::new();
                payload.insert("serverId".to_string(), Value::String(watcher_server_id.clone()));
                payload.insert(
                    "transportKind".to_string(),
                    Value::String(TransportKind::Websocket.as_str().to_string()),
                );
                payload.insert("attempt".to_string(), Value::Number(expected_attempt.into()));
                payload.insert(
                    "sessionId".to_string(),
                    Value::String(latest.last_session_id.clone()),
                );
                let _ = this.publish_event(
                    "mcp",
                    "mcp.server_reconnect_completed",
                    Resource {
                        kind: RESOURCE_KIND_SERVER.to_string(),
                        id: watcher_server_id.clone(),
                    },
                    payload,
                );
                return;
            }
            let resource = this.get_server_resource(&watcher_server_id).unwrap_or_default();
            if !resource.server.enabled || !resource.server.auto_restart {
                return;
            }
            let cause = first_non_empty(&[
                response.blocked_reason.as_str(),
                resource.state.health_reason.as_str(),
                response.failure_class.as_str(),
                "websocket reconnect failed",
            ]);
            this.schedule_websocket_reconnect(&watcher_server_id, &resource.state, Some(&cause));
        });
    }
}

impl Manager {
    /// Go `UpdateServer` / `CreateServer` shared implementation (`upsertServer`).
    fn upsert_server(
        &self,
        create_input: CreateServerInput,
        update: Option<UpdateOperation>,
    ) -> Result<(ServerResource, bool), McpError> {
        let now = Utc::now();
        let mut guard = self.inner.state.write();
        let (mut server, created) = match &update {
            None => {
                let server_id = create_input.server_id.trim().to_string();
                if server_id.is_empty() {
                    return Err(McpError::ServerIDRequired);
                }
                let existing = guard.servers.get(&server_id).cloned();
                let (mut server, created) = if let Some(existing) = existing {
                    (existing, false)
                } else {
                    let server = Server {
                        // Go activeTenantID(ctx): tenant context is not ported.
                        tenant_id: String::new(),
                        server_id: server_id.clone(),
                        source: Source::Api,
                        origin_kind: OriginKind::Manual,
                        install_method: InstallMethod::Api,
                        environment_scope: environment_scope(self.inner.cfg.environment),
                        created_at: now,
                        transport_kind: TransportKind::Stdio,
                        declaration: default_declaration(),
                        ..Server::default()
                    };
                    (server, true)
                };
                if created {
                    guard.server_ids.push(server_id.clone());
                }
                // The tenant that created it, so the reads can find it again.
                //
                // This was hardcoded empty, with the Go call it stood in for
                // left in a comment. Every read filters by the requesting
                // tenant, so a server created through the API was invisible to
                // the very next request on the same API -- created, returned,
                // and gone.
                if !create_input.tenant_id.trim().is_empty() {
                    server.tenant_id = create_input.tenant_id.trim().to_string();
                }
                server.display_name = create_input.display_name.trim().to_string();
                if create_input.origin_kind != OriginKind::default() {
                    server.origin_kind = create_input.origin_kind;
                }
                server.catalog_entry_id = create_input.catalog_entry_id.trim().to_string();
                if create_input.install_method != InstallMethod::default() {
                    server.install_method = create_input.install_method;
                }
                if !create_input.environment_scope.trim().is_empty() {
                    server.environment_scope = create_input.environment_scope.trim().to_string();
                }
                server.enabled = create_input.enabled;
                server.sandbox_profile_id = create_input.sandbox_profile_id.trim().to_string();
                server.declaration_id = create_input.declaration_id.trim().to_string();
                server.declaration = match &create_input.declaration {
                    Some(declaration) => normalize_declaration(declaration.clone()),
                    None => normalize_declaration(server.declaration.clone()),
                };
                if create_input.transport_kind != TransportKind::default() {
                    server.transport_kind = create_input.transport_kind;
                }
                server.command = create_input.command.trim().to_string();
                server.args = clone_strings(&create_input.args);
                server.endpoint = create_input.endpoint.trim().to_string();
                server.websocket_config = clone_websocket_config(&create_input.websocket_config);
                server.working_dir = create_input.working_dir.trim().to_string();
                server.secret_refs = clean_strings(&create_input.secret_refs);
                server.auto_restart = create_input.auto_restart;
                server.operator_modified = create_input.operator_modified;
                server.catalog_management = clone_catalog_management(&create_input.catalog_management);
                server.source = Source::Api;
                server.updated_at = now;
                (server, created)
            }
            Some(operation) => {
                let existing = guard
                    .servers
                    .get(&operation.server_id)
                    .cloned()
                    .ok_or(McpError::ServerNotFound)?;
                let mut server = existing;
                // Go tenant ownership check (activeTenantID) is not ported.
                if let Some(display_name) = &operation.input.display_name {
                    server.display_name = display_name.trim().to_string();
                    server.operator_modified = true;
                }
                if let Some(enabled) = operation.input.enabled {
                    server.enabled = enabled;
                    server.operator_modified = true;
                    if let Some(state) = guard.states.get_mut(&server.server_id) {
                        state.status = if enabled {
                            if state.status == LifecycleStatus::Disabled {
                                LifecycleStatus::Stopped
                            } else {
                                state.status
                            }
                        } else {
                            LifecycleStatus::Disabled
                        };
                    }
                }
                if let Some(profile_id) = &operation.input.sandbox_profile_id {
                    server.sandbox_profile_id = profile_id.trim().to_string();
                    server.operator_modified = true;
                }
                if let Some(declaration_id) = &operation.input.declaration_id {
                    server.declaration_id = declaration_id.trim().to_string();
                    server.operator_modified = true;
                }
                if let Some(declaration) = &operation.input.declaration {
                    server.declaration = normalize_declaration(declaration.clone());
                    server.operator_modified = true;
                }
                if let Some(transport_kind) = operation.input.transport_kind {
                    server.transport_kind = transport_kind;
                    server.operator_modified = true;
                }
                if let Some(command) = &operation.input.command {
                    server.command = command.trim().to_string();
                    server.operator_modified = true;
                }
                if let Some(args) = &operation.input.args {
                    server.args = clone_strings(args);
                    server.operator_modified = true;
                }
                if let Some(endpoint) = &operation.input.endpoint {
                    server.endpoint = endpoint.trim().to_string();
                    server.operator_modified = true;
                }
                if let Some(websocket_config) = &operation.input.websocket_config {
                    server.websocket_config = clone_websocket_config(&Some(websocket_config.clone()));
                    server.operator_modified = true;
                }
                if let Some(working_dir) = &operation.input.working_dir {
                    server.working_dir = working_dir.trim().to_string();
                    server.operator_modified = true;
                }
                if let Some(secret_refs) = &operation.input.secret_refs {
                    server.secret_refs = clean_strings(secret_refs);
                    server.operator_modified = true;
                }
                if let Some(auto_restart) = operation.input.auto_restart {
                    server.auto_restart = auto_restart;
                    server.operator_modified = true;
                }
                server.updated_at = now;
                (server, false)
            }
        };
        // Go: an empty transport kind defaults to stdio; the closed enum cannot be empty.
        server.declaration = normalize_declaration(server.declaration);
        self.validate_server(&server)?;
        guard.servers.insert(server.server_id.clone(), server.clone());
        if !guard.states.contains_key(&server.server_id) {
            guard.states.insert(server.server_id.clone(), default_state_for_server(&server));
        }
        let resource = self.build_server_resource_locked(&guard, &server);
        let state = guard.states.get(&server.server_id).cloned().unwrap_or_default();
        drop(guard);

        self.persist_server(&server)?;
        self.persist_state(&state)?;
        self.persist_declaration_view(&server)?;

        let event_name = if created { "mcp.server_registered" } else { "mcp.server_updated" };
        let mut payload = Map::new();
        payload.insert("serverId".to_string(), Value::String(server.server_id.clone()));
        payload.insert("displayName".to_string(), Value::String(server.display_name.clone()));
        payload.insert("originKind".to_string(), Value::String(server.origin_kind.as_str().to_string()));
        payload.insert("catalogEntryId".to_string(), Value::String(server.catalog_entry_id.clone()));
        payload.insert("installMethod".to_string(), Value::String(server.install_method.as_str().to_string()));
        payload.insert("enabled".to_string(), Value::Bool(server.enabled));
        payload.insert("sandboxProfileId".to_string(), Value::String(server.sandbox_profile_id.clone()));
        payload.insert("declarationId".to_string(), Value::String(server.declaration_id.clone()));
        payload.insert("transportKind".to_string(), Value::String(server.transport_kind.as_str().to_string()));
        payload.insert(
            "availabilityStatus".to_string(),
            Value::String(resource.availability_status.as_str().to_string()),
        );
        payload.insert(
            "availabilityReason".to_string(),
            Value::String(resource.availability_reason.clone()),
        );
        payload.insert(
            "catalogManagement".to_string(),
            match resource.server.catalog_management.as_ref() {
                Some(management) => Value::Object(catalog_management_payload(management)),
                None => Value::Null,
            },
        );
        payload.insert("created".to_string(), Value::Bool(created));
        self.publish_event(
            "mcp",
            event_name,
            Resource {
                kind: RESOURCE_KIND_SERVER.to_string(),
                id: server.server_id.clone(),
            },
            payload,
        )?;
        Ok((resource, created))
    }

    /// Go `stopOrCancel`.
    fn stop_or_cancel(&self, server_id: &str, cancel: bool) -> Result<LifecycleResponse, McpError> {
        let server_id = server_id.trim().to_string();
        if server_id.is_empty() {
            return Err(McpError::ServerIDRequired);
        }
        let started_at = Instant::now();
        let (active, mut state) = {
            let mut guard = self.inner.state.write();
            let server = match guard.servers.get(&server_id) {
                Some(server) => server.clone(),
                None => return Err(McpError::ServerNotFound),
            };
            let state = guard.states.get(&server_id).cloned().unwrap_or_default();
            let active = guard.sessions.get(&server_id).cloned();
            if active.is_none() {
                let resource = self.build_server_resource_locked(&guard, &server);
                let action = if cancel {
                    LifecycleAction::Cancel
                } else {
                    LifecycleAction::Stop
                };
                return Ok(LifecycleResponse {
                    action,
                    server: resource,
                    idempotent: true,
                    preflight_ms: started_at.elapsed().as_millis() as i64,
                    ..LifecycleResponse::default()
                });
            }
            let mut state = state;
            if cancel {
                if let Some(stored) = guard.sessions.get_mut(&server_id) {
                    stored.cancel_requested = true;
                }
                state.health_reason = "cancelled by operator".to_string();
            } else {
                if let Some(stored) = guard.sessions.get_mut(&server_id) {
                    stored.stop_requested = true;
                }
                state.health_reason = "stopped by operator".to_string();
            }
            state.status = LifecycleStatus::Stopping;
            state.updated_at = Utc::now();
            guard.states.insert(server_id.clone(), state.clone());
            (active, state)
        };
        if let Err(err) = self.persist_state(&state) {
            return Err(err);
        }
        let active = active.expect("active session present when not idempotent");

        if active.execution_id.is_empty() {
            if let Some(session) = &active.session {
                let _ = session.close();
            }
            let now = Utc::now();
            state.status = LifecycleStatus::Stopped;
            state.last_stopped_at = Some(now);
            state.updated_at = now;
            {
                let mut guard = self.inner.state.write();
                guard.sessions.remove(&server_id);
                guard.states.insert(server_id.clone(), state.clone());
            }
            if let Err(err) = self.persist_state(&state) {
                return Err(err);
            }
            let action = if cancel {
                LifecycleAction::Cancel
            } else {
                LifecycleAction::Stop
            };
            let failure_class = if cancel { "cancelled".to_string() } else { String::new() };
            let resource = self.get_server_resource(&server_id).unwrap_or_default();
            let mut payload = Map::new();
            payload.insert("serverId".to_string(), Value::String(server_id.clone()));
            payload.insert("status".to_string(), Value::String(state.status.as_str().to_string()));
            payload.insert("executionId".to_string(), Value::String(String::new()));
            payload.insert("sessionId".to_string(), Value::String(active.session_id.clone()));
            payload.insert("cancelled".to_string(), Value::Bool(cancel));
            payload.insert(
                "transportKind".to_string(),
                Value::String(active.transport_kind.as_str().to_string()),
            );
            self.publish_event(
                "mcp",
                "mcp.server_stopped",
                Resource {
                    kind: RESOURCE_KIND_SERVER.to_string(),
                    id: server_id.clone(),
                },
                payload,
            )?;
            return Ok(LifecycleResponse {
                action,
                server: resource,
                execution_id: String::new(),
                failure_class,
                preflight_ms: started_at.elapsed().as_millis() as i64,
                ..LifecycleResponse::default()
            });
        }

        let sandboxes = self
            .inner
            .sandboxes
            .clone()
            .ok_or_else(|| McpError::Other("mcp sandbox manager is not configured".to_string()))?;
        let (execution, _) = sandboxes
            .cancel_execution(&active.execution_id)
            .map_err(McpError::Other)?;
        let action = if cancel {
            LifecycleAction::Cancel
        } else {
            LifecycleAction::Stop
        };
        let failure_class = if cancel { "cancelled".to_string() } else { String::new() };
        let resource = self.get_server_resource(&server_id).unwrap_or_default();
        let mut payload = Map::new();
        payload.insert("serverId".to_string(), Value::String(server_id.clone()));
        payload.insert("status".to_string(), Value::String(state.status.as_str().to_string()));
        payload.insert("executionId".to_string(), Value::String(execution.execution_id.clone()));
        payload.insert("cancelled".to_string(), Value::Bool(cancel));
        self.publish_event(
            "mcp",
            "mcp.server_stopped",
            Resource {
                kind: RESOURCE_KIND_SERVER.to_string(),
                id: server_id.clone(),
            },
            payload,
        )?;
        Ok(LifecycleResponse {
            action,
            server: resource,
            execution_id: active.execution_id.clone(),
            failure_class,
            preflight_ms: started_at.elapsed().as_millis() as i64,
            ..LifecycleResponse::default()
        })
    }

    /// Go `updateStateFromExecution`: reconciles the server state from a sandbox
    /// execution outcome.
    fn update_state_from_execution(
        &self,
        server_id: &str,
        state: &mut ServerState,
        execution: &kura_sandbox::Execution,
        requested_stop: bool,
    ) {
        let now = Utc::now();
        state.last_execution_id = execution.execution_id.clone();
        state.last_policy_record_id = execution
            .consumer
            .as_ref()
            .and_then(|consumer| consumer.policy_record.as_ref())
            .map(|record| record.policy_record_id.clone())
            .unwrap_or_default();
        state.updated_at = now;
        match execution.status {
            kura_sandbox::ExecutionStatus::Denied | kura_sandbox::ExecutionStatus::Unsupported => {
                state.status = lifecycle_status_from_execution(execution);
                state.health_reason =
                    first_non_empty(&[execution.result.error.as_str(), execution.decision.explanation.as_str()]);
            }
            kura_sandbox::ExecutionStatus::Cancelled => {
                state.status = LifecycleStatus::Stopped;
                if requested_stop {
                    state.health_reason = first_non_empty(&[state.health_reason.as_str(), "cancelled by operator"]);
                } else {
                    state.health_reason =
                        first_non_empty(&[execution.result.error.as_str(), "execution was cancelled"]);
                }
                state.last_stopped_at = Some(now);
            }
            kura_sandbox::ExecutionStatus::Completed => {
                if requested_stop {
                    state.status = LifecycleStatus::Stopped;
                    state.health_reason = first_non_empty(&[state.health_reason.as_str(), "stopped by operator"]);
                    state.last_stopped_at = Some(now);
                } else {
                    state.status = LifecycleStatus::Failed;
                    state.health_reason = "mcp transport exited unexpectedly".to_string();
                    state.failure_count += 1;
                }
            }
            kura_sandbox::ExecutionStatus::Failed => {
                state.status = LifecycleStatus::Failed;
                state.health_reason = first_non_empty(&[
                    execution.result.error.as_str(),
                    execution.result.error_code.as_str(),
                    "sandbox execution failed",
                ]);
                state.failure_count += 1;
            }
            _ => {
                state.status = LifecycleStatus::Degraded;
                state.health_reason = first_non_empty(&[execution.result.error.as_str(), "mcp server became unavailable"]);
            }
        }
        self.inner.state.write().states.insert(server_id.to_string(), state.clone());
        let _ = self.persist_state(state);
        let _ = self.publish_health_changed(server_id, state.status, &state.health_reason);
    }

    /// Go `recordFailure`.
    fn record_failure(
        &self,
        server_id: &str,
        state: &mut ServerState,
        status: LifecycleStatus,
        reason: &str,
        failure_class: &str,
    ) {
        let now = Utc::now();
        state.status = status;
        state.health_reason = reason.trim().to_string();
        state.failure_count += 1;
        state.updated_at = now;
        self.inner.state.write().states.insert(server_id.to_string(), state.clone());
        let _ = self.persist_state(state);
        let mut payload = Map::new();
        payload.insert("serverId".to_string(), Value::String(server_id.to_string()));
        payload.insert("status".to_string(), Value::String(status.as_str().to_string()));
        payload.insert("reason".to_string(), Value::String(state.health_reason.clone()));
        payload.insert("failureClass".to_string(), Value::String(failure_class.to_string()));
        let _ = self.publish_event(
            "mcp",
            "mcp.server_failed",
            Resource {
                kind: RESOURCE_KIND_SERVER.to_string(),
                id: server_id.to_string(),
            },
            payload,
        );
        let _ = self.publish_health_changed(server_id, state.status, &state.health_reason);
    }

    /// Go `recordRestoreFailure`.
    fn record_restore_failure(
        &self,
        server: &Server,
        state: &mut ServerState,
        status: LifecycleStatus,
        reason: &str,
        failure_class: &str,
    ) {
        self.record_failure(&server.server_id, state, status, reason, failure_class);
        let now = Utc::now();
        state.last_recovery_at = Some(now);
        state.last_recovery_class = "restore_failed".to_string();
        state.next_reconnect_at = None;
        state.updated_at = now;
        self.inner.state.write().states.insert(server.server_id.clone(), state.clone());
        let _ = self.persist_state(state);
        let mut payload = Map::new();
        payload.insert("serverId".to_string(), Value::String(server.server_id.clone()));
        payload.insert(
            "transportKind".to_string(),
            Value::String(server.transport_kind.as_str().to_string()),
        );
        payload.insert("reason".to_string(), Value::String(state.health_reason.clone()));
        payload.insert("failureClass".to_string(), Value::String(failure_class.to_string()));
        let _ = self.publish_event(
            "mcp",
            "mcp.server_restore_failed",
            Resource {
                kind: RESOURCE_KIND_SERVER.to_string(),
                id: server.server_id.clone(),
            },
            payload,
        );
    }

    /// Go `publishHealthChanged`.
    fn publish_health_changed(
        &self,
        server_id: &str,
        status: LifecycleStatus,
        reason: &str,
    ) -> Result<(), McpError> {
        let resource = self.get_server_resource(server_id).unwrap_or_default();
        let mut payload = Map::new();
        payload.insert("serverId".to_string(), Value::String(server_id.to_string()));
        payload.insert("status".to_string(), Value::String(status.as_str().to_string()));
        payload.insert("reason".to_string(), Value::String(reason.trim().to_string()));
        payload.insert(
            "availabilityStatus".to_string(),
            Value::String(resource.availability_status.as_str().to_string()),
        );
        payload.insert(
            "availabilityReason".to_string(),
            Value::String(resource.availability_reason.clone()),
        );
        payload.insert(
            "catalogManagement".to_string(),
            match resource.server.catalog_management.as_ref() {
                Some(management) => Value::Object(catalog_management_payload(management)),
                None => Value::Null,
            },
        );
        self.publish_event(
            "mcp",
            "mcp.server_health_changed",
            Resource {
                kind: RESOURCE_KIND_SERVER.to_string(),
                id: server_id.to_string(),
            },
            payload,
        )
    }

    /// Go `buildExecutionRequest`.
    fn build_execution_request(
        &self,
        server: &Server,
        consumer: &kura_sandbox::ConsumerContractView,
        approval_id: &str,
    ) -> Result<kura_sandbox::ExecutionRequest, McpError> {
        let env = self.resolve_secret_env(server)?;
        let mut access = kura_sandbox::AccessRequest {
            read_roots: clone_strings(&server.declaration.read_roots),
            write_roots: clone_strings(&server.declaration.write_roots),
            network_mode: Some(server.declaration.network_mode),
            allowed_hosts: clone_strings(&server.declaration.allowed_hosts),
            allowed_ports: server.declaration.allowed_ports.clone(),
            allow_loopback: server.declaration.allow_loopback,
        };
        if access.read_roots.is_empty() && !server.working_dir.trim().is_empty() {
            access.read_roots = vec![server.working_dir.trim().to_string()];
        }
        if access.write_roots.is_empty() && !server.working_dir.trim().is_empty() {
            access.write_roots = vec![server.working_dir.trim().to_string()];
        }
        let mut metadata = HashMap::new();
        metadata.insert("mcpServerId".to_string(), server.server_id.clone());
        metadata.insert("transportKind".to_string(), server.transport_kind.as_str().to_string());
        Ok(kura_sandbox::ExecutionRequest {
            profile_id: server.sandbox_profile_id.clone(),
            command: server.command.clone(),
            args: clone_strings(&server.args),
            cwd: server.working_dir.clone(),
            env,
            requested_by: "mcp".to_string(),
            resource_kind: RESOURCE_KIND_SERVER.to_string(),
            resource_id: server.server_id.clone(),
            scope: "mcp.lifecycle".to_string(),
            approval_id: approval_id.to_string(),
            reason: "mcp lifecycle".to_string(),
            metadata,
            access,
            consumer: Some(consumer.clone()),
            ..kura_sandbox::ExecutionRequest::default()
        })
    }

    /// Go `buildLifecycleConsumerView`.
    fn build_lifecycle_consumer_view(
        &self,
        server: &Server,
        requested_by: &str,
    ) -> Result<kura_sandbox::ConsumerContractView, McpError> {
        self.build_consumer_view(
            server,
            &first_non_empty(&[requested_by.trim(), "mcp"]),
            "lifecycle.start",
            &server.declaration_id,
            kura_sandbox::ApprovalMode::Allow,
            kura_sandbox::DecisionResolution::Allow,
            kura_sandbox::DecisionApprovalStatus::NotApplicable,
            kura_sandbox::PolicyRecordStatus::PreflightAllowed,
        )
    }

    /// Go `buildToolConsumerView`.
    fn build_tool_consumer_view(
        &self,
        server: &Server,
        tool_name: &str,
        runtime_surface: &str,
        requested_by: &str,
        approval_mode: kura_sandbox::ApprovalMode,
    ) -> Result<kura_sandbox::ConsumerContractView, McpError> {
        let declaration_id = format!(
            "{}:tool:{}:{}",
            server.declaration_id,
            runtime_surface.trim(),
            tool_name.trim()
        );
        self.build_consumer_view(
            server,
            &first_non_empty(&[requested_by.trim(), "mcp"]),
            "tool_call.execute",
            &declaration_id,
            approval_mode,
            kura_sandbox::DecisionResolution::Allow,
            kura_sandbox::DecisionApprovalStatus::NotApplicable,
            kura_sandbox::PolicyRecordStatus::PreflightAllowed,
        )
    }

    /// Go `buildConsumerView`.
    fn build_consumer_view(
        &self,
        server: &Server,
        requested_by: &str,
        operation_kind: &str,
        declaration_id: &str,
        approval_mode: kura_sandbox::ApprovalMode,
        decision: kura_sandbox::DecisionResolution,
        approval_status: kura_sandbox::DecisionApprovalStatus,
        status: kura_sandbox::PolicyRecordStatus,
    ) -> Result<kura_sandbox::ConsumerContractView, McpError> {
        let secret_scope = self.build_secret_scope(server)?;
        Ok(kura_sandbox::ConsumerContractView {
            declaration: Some(kura_sandbox::ConsumerRequirementDeclaration {
                declaration_id: declaration_id.trim().to_string(),
                consumer_kind: kura_sandbox::ConsumerKind::McpServer,
                consumer_id: server.server_id.clone(),
                operation_kind: operation_kind.to_string(),
                profile_id: server.sandbox_profile_id.clone(),
                execution_mode: server.declaration.execution_mode,
                allowed_backend_kinds: clone_backend_kinds(&server.declaration.allowed_backend_kinds),
                read_roots: clone_strings(&server.declaration.read_roots),
                write_roots: clone_strings(&server.declaration.write_roots),
                network_mode: Some(server.declaration.network_mode),
                allowed_hosts: clone_strings(&server.declaration.allowed_hosts),
                allowed_ports: server.declaration.allowed_ports.clone(),
                allow_loopback: server.declaration.allow_loopback,
                secret_refs: clean_strings(&server.secret_refs),
                approval_mode: Some(approval_mode),
                required_enforcement_strength: server
                    .declaration
                    .required_enforcement_strength
                    .trim()
                    .to_string(),
                active: server.declaration.active,
                source: kura_sandbox::Source::Builtin,
            }),
            secret_scope: secret_scope.clone(),
            policy_record: Some(kura_sandbox::ConsumerPolicyRecord {
                policy_record_id: format!(
                    "policy_mcp_{}_{}_{}",
                    server.server_id,
                    operation_kind.replace('.', "_"),
                    policy_record_timestamp()
                ),
                consumer_kind: kura_sandbox::ConsumerKind::McpServer,
                consumer_id: server.server_id.clone(),
                operation_kind: operation_kind.to_string(),
                declaration_id: declaration_id.trim().to_string(),
                requested_by: first_non_empty(&[requested_by.trim(), "mcp"]),
                decision,
                approval_status,
                secret_resolution: secret_resolution(&secret_scope),
                enforcement_strength: first_non_empty(&[
                    server.declaration.required_enforcement_strength.trim(),
                    "declared_only",
                ]),
                started_at: Utc::now(),
                status,
                ..kura_sandbox::ConsumerPolicyRecord::default()
            }),
        })
    }

    /// Go `buildCatalogManagementLocked` (the `_locked` name is legacy: it does not
    /// take the state lock itself).
    fn build_catalog_management_locked(&self, server: &Server) -> Option<CatalogManagement> {
        if server.origin_kind != OriginKind::Catalog && server.catalog_management.is_none() {
            return None;
        }
        let mut management = clone_catalog_management(&server.catalog_management);
        if management.is_none() {
            management = Some(CatalogManagement::default());
        }
        let management = management.as_mut().expect("management initialized");
        let entry = self.get_catalog_entry(&server.catalog_entry_id);
        let entry_ok = entry.is_some();
        if entry_ok && management.source_kind.is_empty() {
            management.source_kind = entry.as_ref().expect("entry present").source_kind.clone();
        }
        if management.installed_revision.is_empty() {
            if let Some(entry_ref) = entry.as_ref() {
                if let Some(spec) = self.catalog_spec_for_server(server, entry_ref, true) {
                    management.installed_revision = fingerprint_create_server_spec(&spec);
                }
            }
        }
        if entry_ok {
            if let Some(entry_ref) = entry.as_ref() {
                if let Some(spec) = self.catalog_spec_for_server(server, entry_ref, true) {
                    management.current_revision = fingerprint_create_server_spec(&spec);
                }
            }
        } else {
            management.current_revision = String::new();
        }
        let (drift_status, drift_reason) = assess_catalog_drift(server, management, entry_ok);
        management.drift_status = drift_status;
        management.drift_reason = drift_reason;
        Some(management.clone())
    }

    /// Go `catalogSpecForServer`.
    fn catalog_spec_for_server(
        &self,
        server: &Server,
        entry: &CatalogEntry,
        ok: bool,
    ) -> Option<CreateServerInput> {
        if !ok {
            return None;
        }
        let mut method = server.install_method;
        // Go: an unset server install method falls back to the snapshot's method. The
        // closed enum stands in for the empty string with the API default.
        if let Some(management) = &server.catalog_management {
            let snapshot_method = management.install_input_snapshot.install_method;
            if method == InstallMethod::default() && snapshot_method != InstallMethod::default() {
                method = snapshot_method;
            }
        }
        let mut snapshot = CatalogInstallSnapshot::default();
        if let Some(management) = &server.catalog_management {
            snapshot = clone_catalog_install_snapshot(&management.install_input_snapshot);
        }
        if snapshot.server_id.is_empty() {
            snapshot = install_snapshot_from_create_spec(&server_to_create_input(server));
        }
        Some(merge_catalog_install_input(
            entry,
            &catalog_install_input_from_snapshot(&snapshot),
            method,
            self.inner.cfg.environment,
        ))
    }

    /// Go `buildServerResourceLocked`. Callers must hold the state lock and pass the
    /// guard's deref.
    fn build_server_resource_locked(&self, state: &ManagerState, server: &Server) -> ServerResource {
        let mut projected = server.clone();
        if projected.transport_kind == TransportKind::Websocket {
            projected.endpoint = sanitize_websocket_endpoint_for_projection(&projected.endpoint);
        }
        projected.catalog_management =
            sanitize_catalog_management_projection(self.build_catalog_management_locked(server));
        let state_obj = state.states.get(&server.server_id).cloned().unwrap_or_default();
        let tool_count = state.tools.get(&server.server_id).map(|map| map.len()).unwrap_or(0);
        let mut tools = Vec::with_capacity(tool_count);
        if let Some(map) = state.tools.get(&server.server_id) {
            for tool in map.values() {
                tools.push(self.build_tool_resource_locked(state, &projected, tool));
            }
        }
        let (availability_status, availability_reason) =
            self.evaluate_server_availability_locked(state, &projected);
        ServerResource {
            server: projected.clone(),
            state: state_obj,
            secret_summary: self.build_secret_summaries(&projected),
            tool_count: tool_count as i64,
            tools,
            transport_config_summary: self.transport_config_summary(&projected),
            websocket_auth_summary: self.build_websocket_auth_summary(&projected),
            availability_status,
            availability_reason,
            ..ServerResource::default()
        }
    }

    /// Go `buildToolResourceLocked`. Callers must hold the state lock.
    fn build_tool_resource_locked(&self, state: &ManagerState, server: &Server, tool: &Tool) -> ToolResource {
        let mut tool = tool.clone();
        tool.tenant_id = server.tenant_id.clone();
        let mut exposure = Vec::new();
        let mut approval_required = false;
        let mut effective = "unavailable".to_string();
        let mut reason = String::new();
        if let Some(rules) = state
            .exposure
            .get(&server.server_id)
            .and_then(|map| map.get(&tool.tool_name))
        {
            for rule in rules.values() {
                let mut rule = rule.clone();
                rule.tenant_id = server.tenant_id.clone();
                exposure.push(rule.clone());
                if rule.active && rule.exposure_mode == ExposureMode::ApprovalRequired {
                    approval_required = true;
                }
                if rule.active
                    && (rule.exposure_mode == ExposureMode::Allow
                        || rule.exposure_mode == ExposureMode::ApprovalRequired)
                {
                    effective = "available".to_string();
                }
            }
        }
        let state_status = state
            .states
            .get(&server.server_id)
            .map(|s| s.status)
            .unwrap_or_default();
        let state_health_reason = state
            .states
            .get(&server.server_id)
            .map(|s| s.health_reason.clone())
            .unwrap_or_default();
        if !server.enabled {
            effective = "blocked".to_string();
            reason = "server is disabled".to_string();
        } else if state_status != LifecycleStatus::Healthy {
            effective = "unavailable".to_string();
            reason = first_non_empty(&[state_health_reason.as_str(), "server is not healthy"]);
        }
        if exposure.is_empty() {
            effective = "blocked".to_string();
            reason = "tool is not allowlisted for any runtime surface".to_string();
        }
        if tool.discovery_status != DiscoveryStatus::Discovered {
            effective = "unavailable".to_string();
            reason = first_non_empty(&[reason.as_str(), "tool is not currently discovered"]);
        }
        ToolResource {
            tool,
            exposure,
            effective_availability: effective,
            approval_required,
            unavailable_reason: reason,
            ..ToolResource::default()
        }
    }

    /// Go `buildSecretScope`.
    fn build_secret_scope(
        &self,
        server: &Server,
    ) -> Result<Vec<kura_sandbox::SecretScopeOutcome>, McpError> {
        self.list_secret_bindings(server)
    }

    /// Go `listSecretBindings`.
    fn list_secret_bindings(
        &self,
        server: &Server,
    ) -> Result<Vec<kura_sandbox::SecretScopeOutcome>, McpError> {
        let env_scope = match self.inner.cfg.environment {
            // Embedded deliberately shares the test secret scope: it is a
            // non-production deployment, and splitting the secret labels would
            // change the persisted scope format for every existing secret.
            kura_config::Environment::Test | kura_config::Environment::Embedded => {
                kura_sandbox::SecretEnvironmentScope::Test
            }
            kura_config::Environment::Prod => kura_sandbox::SecretEnvironmentScope::Prod,
        };
        let mut items = Vec::with_capacity(server.secret_refs.len());
        for secret_ref in &server.secret_refs {
            items.push(kura_sandbox::SecretScopeOutcome {
                consumer_kind: kura_sandbox::ConsumerKind::McpServer,
                consumer_id: server.server_id.clone(),
                secret_ref: secret_ref.clone(),
                environment_scope: env_scope,
                default_source: Some(kura_sandbox::SecretDefaultSource::InstanceOverride),
                default_rule_id: format!("mcp_server:{}", server.server_id),
                delivery_kind: "environment_variable".to_string(),
                redaction_rule: "value_redacted".to_string(),
                resolution: self.resolve_secret_ref(secret_ref, env_scope),
            });
        }
        Ok(items)
    }

    /// Go `resolveSecretEnv`.
    fn resolve_secret_env(&self, server: &Server) -> Result<HashMap<String, String>, McpError> {
        let secret_scope = self.build_secret_scope(server)?;
        let resolved_secrets = self.resolve_secret_values(&server.secret_refs)?;
        let mut env = HashMap::new();
        for item in secret_scope {
            if item.resolution != kura_sandbox::SecretResolution::Resolved {
                continue;
            }
            if let Some(value) = resolved_secrets.get(&item.secret_ref) {
                env.insert(item.secret_ref.clone(), value.clone());
            }
        }
        Ok(env)
    }

    /// Go `resolveSecretRef`.
    fn resolve_secret_ref(
        &self,
        secret_ref: &str,
        env_scope: kura_sandbox::SecretEnvironmentScope,
    ) -> kura_sandbox::SecretResolution {
        match self.inner.cfg.environment {
            // See `list_secret_bindings`: embedded resolves against the test scope.
            kura_config::Environment::Test | kura_config::Environment::Embedded => {
                if env_scope != kura_sandbox::SecretEnvironmentScope::Test
                    && env_scope != kura_sandbox::SecretEnvironmentScope::Both
                {
                    return kura_sandbox::SecretResolution::Denied;
                }
            }
            kura_config::Environment::Prod => {
                if env_scope != kura_sandbox::SecretEnvironmentScope::Prod
                    && env_scope != kura_sandbox::SecretEnvironmentScope::Both
                {
                    return kura_sandbox::SecretResolution::Denied;
                }
            }
        }
        match self.resolve_secret_values(&[secret_ref.to_string()]) {
            Ok(resolved) => {
                if resolved.contains_key(secret_ref) {
                    kura_sandbox::SecretResolution::Resolved
                } else {
                    kura_sandbox::SecretResolution::Unavailable
                }
            }
            // Go maps a missing tenant context to Denied; tenant context is not ported,
            // so every resolution failure is Unavailable.
            Err(_) => kura_sandbox::SecretResolution::Unavailable,
        }
    }

    /// Go `resolveSecretValues`: with no injected resolver, reads
    /// `mcp-secrets.json` from the data dir (the Go nil-secret-manager path).
    fn resolve_secret_values(&self, secret_refs: &[String]) -> Result<HashMap<String, String>, McpError> {
        let refs = clean_strings(secret_refs);
        if refs.is_empty() {
            return Ok(HashMap::new());
        }
        let resolver = self.inner.secrets.read().clone();
        match resolver {
            Some(resolver) => {
                let mut resolved = HashMap::with_capacity(refs.len());
                for secret_ref in &refs {
                    match resolver.resolve(secret_ref) {
                        Ok(Some(value)) if !value.trim().is_empty() => {
                            resolved.insert(secret_ref.clone(), value);
                        }
                        Ok(_) => {}
                        Err(err) => return Err(McpError::Other(format!("resolve secret {secret_ref}: {err}"))),
                    }
                }
                Ok(resolved)
            }
            None => resolve_mcp_secrets(&self.inner.cfg.data_dir, &refs).map_err(McpError::Store),
        }
    }

    /// Go `persistDeclarationView`.
    fn persist_declaration_view(&self, server: &Server) -> Result<(), McpError> {
        if self.inner.sandboxes.is_none() {
            return Ok(());
        }
        let mut view = self.build_lifecycle_consumer_view(server, "mcp")?;
        view.policy_record = None;
        self.persist_consumer_view(&view)
    }

    /// Go `persistConsumerView`.
    fn persist_consumer_view(&self, view: &kura_sandbox::ConsumerContractView) -> Result<(), McpError> {
        if let Some(sandboxes) = &self.inner.sandboxes {
            sandboxes
                .persist_consumer_view(view)
                .map_err(McpError::Other)?;
        }
        Ok(())
    }

    /// Go `persistApproval`. Deferred: kura-store has no approval CRUD yet, so this is
    /// a no-op (the in-memory policy engine already holds the record).
    fn persist_approval(&self, _approval: &kura_policy::Approval) -> Result<(), McpError> {
        Ok(())
    }

    /// Go `persistDecision`. Deferred: kura-store has no decision CRUD yet.
    fn persist_decision(&self, _decision: &kura_policy::Decision) -> Result<(), McpError> {
        Ok(())
    }

    /// Go `persistServer`.
    fn persist_server(&self, server: &Server) -> Result<(), McpError> {
        let Some(store) = &self.inner.store else {
            return Ok(());
        };
        let document = serde_json::to_string(server)
            .map_err(|e| McpError::Store(format!("marshal mcp server {}: {e}", server.server_id)))?;
        store
            .lock()
            .map_err(|_| McpError::Store("store lock poisoned".to_string()))?
            .upsert_mcp_server(&kura_store::mcp::MCPServerRecord {
                server_id: server.server_id.clone(),
                enabled: server.enabled,
                updated_at: server.updated_at,
                document,
            })
            .map_err(McpError::Store)
    }

    /// Go `persistState`.
    fn persist_state(&self, state: &ServerState) -> Result<(), McpError> {
        let Some(store) = &self.inner.store else {
            return Ok(());
        };
        let document = serde_json::to_string(state)
            .map_err(|e| McpError::Store(format!("marshal mcp server state {}: {e}", state.server_id)))?;
        store
            .lock()
            .map_err(|_| McpError::Store("store lock poisoned".to_string()))?
            .upsert_mcp_server_state(&kura_store::mcp::MCPServerStateRecord {
                server_id: state.server_id.clone(),
                status: state.status.as_str().to_string(),
                updated_at: state.updated_at,
                document,
            })
            .map_err(McpError::Store)
    }

    /// Go `persistToolMap` (and `persistTools`): replaces all tools of a server.
    fn persist_tool_map(&self, server_id: &str, tools: &[Tool]) -> Result<(), McpError> {
        let Some(store) = &self.inner.store else {
            return Ok(());
        };
        let mut records = Vec::with_capacity(tools.len());
        for tool in tools {
            let document = serde_json::to_string(tool).map_err(|e| {
                McpError::Store(format!("marshal mcp tool {}/{}: {e}", server_id, tool.tool_name))
            })?;
            records.push(kura_store::mcp::MCPToolRecord {
                server_id: tool.server_id.clone(),
                tool_name: tool.tool_name.clone(),
                discovery_status: tool.discovery_status.as_str().to_string(),
                updated_at: tool.updated_at,
                last_discovered_at: tool.last_discovered_at,
                document,
            });
        }
        store
            .lock()
            .map_err(|_| McpError::Store("store lock poisoned".to_string()))?
            .replace_mcp_tools(server_id, &records)
            .map_err(McpError::Store)
    }

    /// Go `persistExposureRule`.
    fn persist_exposure_rule(&self, rule: &ToolExposureRule) -> Result<(), McpError> {
        let Some(store) = &self.inner.store else {
            return Ok(());
        };
        let document = serde_json::to_string(rule).map_err(|e| {
            McpError::Store(format!(
                "marshal mcp tool exposure rule {}/{}/{}: {e}",
                rule.server_id, rule.tool_name, rule.runtime_surface
            ))
        })?;
        store
            .lock()
            .map_err(|_| McpError::Store("store lock poisoned".to_string()))?
            .upsert_mcp_tool_exposure_rule(&kura_store::mcp::MCPToolExposureRuleRecord {
                server_id: rule.server_id.clone(),
                tool_name: rule.tool_name.clone(),
                runtime_surface: rule.runtime_surface.clone(),
                exposure_mode: rule.exposure_mode.as_str().to_string(),
                active: rule.active,
                updated_at: rule.updated_at,
                document,
            })
            .map_err(McpError::Store)
    }

    /// Go `publishEvent`: appends to the store ledger (when a store exists), then
    /// publishes to the in-process bus.
    fn publish_event(
        &self,
        category: &str,
        name: &str,
        resource: Resource,
        payload: Map<String, Value>,
    ) -> Result<(), McpError> {
        if self.inner.event_bus.is_none() && self.inner.store.is_none() {
            return Ok(());
        }
        let mut event = kura_events::Event {
            event_id: event_id(name),
            category: category.to_string(),
            name: name.to_string(),
            occurred_at: Utc::now(),
            resource,
            payload,
            ..kura_events::Event::default()
        };
        if let Some(store) = &self.inner.store {
            let persisted = store
                .lock()
                .map_err(|_| McpError::Store("store lock poisoned".to_string()))?
                .append_event(&event)
                .map_err(McpError::Store)?;
            event = persisted;
        }
        if let Some(bus) = &self.inner.event_bus {
            bus.publish(event.clone());
        }
        Ok(())
    }

    /// Go `publishAuditEvent`: like `publishEvent` but returns the published event.
    fn publish_audit_event(
        &self,
        name: &str,
        resource: Resource,
        payload: Map<String, Value>,
    ) -> Result<kura_events::Event, McpError> {
        if self.inner.event_bus.is_none() && self.inner.store.is_none() {
            return Ok(kura_events::Event::default());
        }
        let mut event = kura_events::Event {
            event_id: event_id(name),
            category: "mcp".to_string(),
            name: name.to_string(),
            occurred_at: Utc::now(),
            resource,
            payload,
            ..kura_events::Event::default()
        };
        if let Some(store) = &self.inner.store {
            let persisted = store
                .lock()
                .map_err(|_| McpError::Store("store lock poisoned".to_string()))?
                .append_event(&event)
                .map_err(McpError::Store)?;
            event = persisted;
        }
        if let Some(bus) = &self.inner.event_bus {
            bus.publish(event.clone());
        }
        Ok(event)
    }

    /// Go `validateServer`.
    fn validate_server(&self, server: &Server) -> Result<(), McpError> {
        if server.server_id.trim().is_empty() {
            return Err(McpError::ServerIDRequired);
        }
        if server.declaration_id.trim().is_empty() {
            return Err(McpError::DeclarationIDRequired);
        }
        if server.sandbox_profile_id.trim().is_empty() {
            return Err(McpError::ProfileIDRequired);
        }
        match server.transport_kind {
            TransportKind::Stdio => {
                if server.command.trim().is_empty() {
                    return Err(McpError::CommandRequired);
                }
            }
            TransportKind::StreamableHTTP => {
                if server.endpoint.trim().is_empty() {
                    return Err(McpError::TransportUnavailable);
                }
            }
            TransportKind::Websocket => {
                validate_websocket_endpoint(&server.endpoint)?;
            }
            _ => return Err(McpError::UnsupportedTransport),
        }
        if server.auto_restart && !server.enabled {
            return Err(McpError::AutoRestartRequiresOn);
        }
        if !server.declaration.active && server.enabled {
            return Err(McpError::Other(
                "mcp declaration must be active before the server can be enabled".to_string(),
            ));
        }
        if let Some(sandboxes) = &self.inner.sandboxes {
            if server.enabled {
                let profile = sandboxes
                    .get_profile(&server.sandbox_profile_id)
                    .ok_or_else(|| {
                        McpError::Other(format!(
                            "sandbox profile {} was not found",
                            server.sandbox_profile_id
                        ))
                    })?;
                if !profile.active {
                    return Err(McpError::Other(format!(
                        "sandbox profile {} is inactive",
                        server.sandbox_profile_id
                    )));
                }
            }
        }
        Ok(())
    }

    /// Go `evaluateServerAvailabilityLocked`. Callers must hold the state lock.
    fn evaluate_server_availability_locked(
        &self,
        state: &ManagerState,
        server: &Server,
    ) -> (AvailabilityStatus, String) {
        if let Some(management) = &server.catalog_management {
            if let Some(snapshot) = &management.last_revalidation {
                if snapshot.status != AvailabilityStatus::Ready {
                    return (
                        snapshot.status,
                        first_non_empty(&[snapshot.reason.as_str(), "server requires revalidation"]),
                    );
                }
            }
        }
        match server.transport_kind {
            TransportKind::Stdio => {
                if server.command.trim().is_empty() {
                    return (
                        AvailabilityStatus::Unavailable,
                        "stdio command is not configured".to_string(),
                    );
                }
            }
            TransportKind::StreamableHTTP => {
                if server.endpoint.trim().is_empty() {
                    return (
                        AvailabilityStatus::Unsupported,
                        "streamable-http endpoint is not configured".to_string(),
                    );
                }
            }
            TransportKind::Websocket => {
                if server.endpoint.trim().is_empty() {
                    return (
                        AvailabilityStatus::Unsupported,
                        "websocket endpoint is not configured".to_string(),
                    );
                }
                if let Some(summary) = self.build_websocket_auth_summary(server) {
                    if summary.configured && !summary.resolved {
                        return (
                            AvailabilityStatus::Blocked,
                            first_non_empty(&[
                                summary.blocked_reason.as_str(),
                                "websocket auth secret is unavailable",
                            ]),
                        );
                    }
                }
            }
            _ => {
                return (
                    AvailabilityStatus::Unsupported,
                    "transport kind is unsupported".to_string(),
                );
            }
        }
        for summary in self.build_secret_summaries(server) {
            if summary.resolution != kura_sandbox::SecretResolution::Resolved.as_str() {
                return (
                    AvailabilityStatus::Blocked,
                    format!("{} is unavailable in {}", summary.secret_ref, summary.environment_scope),
                );
            }
        }
        let state_obj = state.states.get(&server.server_id);
        if let Some(s) = state_obj {
            match s.status {
                LifecycleStatus::Unsupported => {
                    return (
                        AvailabilityStatus::Unsupported,
                        first_non_empty(&[s.health_reason.as_str(), "transport is unsupported"]),
                    );
                }
                LifecycleStatus::Failed | LifecycleStatus::Denied | LifecycleStatus::Degraded => {
                    return (
                        AvailabilityStatus::Unavailable,
                        first_non_empty(&[s.health_reason.as_str(), "server is not healthy"]),
                    );
                }
                LifecycleStatus::Disabled => {
                    return (AvailabilityStatus::Blocked, "server is disabled".to_string());
                }
                _ => {}
            }
        }
        (AvailabilityStatus::Ready, String::new())
    }

    /// Go `transportConfigSummary`.
    fn transport_config_summary(&self, server: &Server) -> String {
        match server.transport_kind {
            TransportKind::StreamableHTTP => server.endpoint.trim().to_string(),
            TransportKind::Websocket => {
                let mut summary = server.endpoint.trim().to_string();
                if let Some(auth) = self.build_websocket_auth_summary(server) {
                    if auth.mode != WebsocketAuthMode::default() {
                        summary = format!("{} ({})", summary.trim(), auth.mode.as_str()).trim().to_string();
                    }
                }
                summary
            }
            _ => {
                if server.command.trim().is_empty() {
                    return String::new();
                }
                if server.args.is_empty() {
                    return server.command.trim().to_string();
                }
                let joined = clone_strings(&server.args).join(" ");
                format!("{} {}", server.command.trim(), joined)
            }
        }
    }

    /// Go `buildWebsocketAuthSummary`.
    fn build_websocket_auth_summary(&self, server: &Server) -> Option<WebsocketAuthSummary> {
        if server.transport_kind != TransportKind::Websocket
            || server.websocket_config.is_none()
            || server.websocket_config.as_ref()?.auth.is_none()
        {
            return None;
        }
        let auth = server.websocket_config.as_ref()?.auth.as_ref()?;
        let mut summary = WebsocketAuthSummary {
            mode: auth.mode,
            header_name: default_websocket_header_name(auth),
            scheme: default_websocket_scheme(auth),
            secret_ref: auth.secret_ref.trim().to_string(),
            configured: true,
            resolved: false,
            ..WebsocketAuthSummary::default()
        };
        if summary.secret_ref.is_empty() {
            summary.blocked_reason = "websocket auth secret ref is not configured".to_string();
            return Some(summary);
        }
        for item in self.build_secret_summaries(server) {
            if item.secret_ref != summary.secret_ref {
                continue;
            }
            summary.resolved = item.resolution == kura_sandbox::SecretResolution::Resolved.as_str();
            if !summary.resolved {
                summary.blocked_reason =
                    format!("{} is unavailable in {}", item.secret_ref, item.environment_scope);
            }
            return Some(summary);
        }
        summary.blocked_reason = format!(
            "{} is unavailable in {}",
            summary.secret_ref,
            environment_scope(self.inner.cfg.environment)
        );
        Some(summary)
    }

    /// Go `resolveWebsocketHeaders`.
    fn resolve_websocket_headers(&self, server: &Server) -> Result<HashMap<String, String>, McpError> {
        if server.transport_kind != TransportKind::Websocket
            || server.websocket_config.is_none()
            || server.websocket_config.as_ref().and_then(|c| c.auth.as_ref()).is_none()
        {
            return Ok(HashMap::new());
        }
        let auth = server
            .websocket_config
            .as_ref()
            .and_then(|c| c.auth.as_ref())
            .expect("auth present");
        let secret_ref = auth.secret_ref.trim().to_string();
        if secret_ref.is_empty() {
            return Err(McpError::Other("websocket auth secret ref is not configured".to_string()));
        }
        let resolved = self.resolve_secret_values(&[secret_ref.clone()])?;
        let value = resolved
            .get(&secret_ref)
            .cloned()
            .unwrap_or_default()
            .trim()
            .to_string();
        if value.is_empty() {
            return Err(McpError::Other(format!(
                "{} is unavailable in {}",
                secret_ref,
                environment_scope(self.inner.cfg.environment)
            )));
        }
        let header_name = default_websocket_header_name(auth);
        if header_name.is_empty() {
            return Err(McpError::Other("websocket auth header name is not configured".to_string()));
        }
        if auth.mode == WebsocketAuthMode::BearerHeader {
            let value = format!("{} {}", default_websocket_scheme(auth).trim(), value)
                .trim()
                .to_string();
            return Ok(HashMap::from([(header_name, value)]));
        }
        Ok(HashMap::from([(header_name, value)]))
    }

    /// Go `runCatalogLifecycleAction`.
    fn run_catalog_lifecycle_action(
        &self,
        server_id: &str,
        action: CatalogAction,
    ) -> Result<CatalogLifecycleResult, McpError> {
        let started_at = Instant::now();
        let server_id = server_id.trim().to_string();
        if server_id.is_empty() {
            return Err(McpError::ServerIDRequired);
        }
        let server = self.get_server(&server_id).ok_or(McpError::ServerNotFound)?;
        let mut result = CatalogLifecycleResult {
            action_id: format!(
                "mcp_catalog_{}_{}",
                action.as_str(),
                Utc::now().timestamp_nanos_opt().unwrap_or(0)
            ),
            action,
            server_id: server.server_id.clone(),
            catalog_entry_id: server.catalog_entry_id.clone(),
            ..CatalogLifecycleResult::default()
        };
        let mut requested_payload = Map::new();
        requested_payload.insert("actionId".to_string(), Value::String(result.action_id.clone()));
        requested_payload.insert("action".to_string(), Value::String(action.as_str().to_string()));
        requested_payload.insert("serverId".to_string(), Value::String(server.server_id.clone()));
        requested_payload.insert(
            "catalogEntryId".to_string(),
            Value::String(server.catalog_entry_id.clone()),
        );
        requested_payload.insert(
            "environment".to_string(),
            Value::String(environment_scope(self.inner.cfg.environment)),
        );
        let requested_event = self.publish_audit_event(
            "mcp.catalog_lifecycle_requested",
            Resource {
                kind: RESOURCE_KIND_SERVER.to_string(),
                id: server.server_id.clone(),
            },
            requested_payload,
        )?;
        result.audit_event_ids.push(requested_event.event_id.clone());

        if let Some(blocked) =
            self.catalog_lifecycle_block_result(&server, action, action != CatalogAction::Uninstall)?
        {
            return self.catalog_lifecycle_blocked_result(&server, &result, &blocked, started_at);
        }

        match action {
            CatalogAction::Uninstall => {
                if let Err(err) = self.delete_catalog_server(&server.server_id) {
                    return self.catalog_lifecycle_failed_result(
                        &server,
                        &result,
                        "failed",
                        &err.to_string(),
                        started_at,
                    );
                }
                result.status = CatalogActionStatus::Completed;
                result.removed = true;
            }
            CatalogAction::Refresh | CatalogAction::Reinstall => {
                let entry = match self.get_catalog_entry(&server.catalog_entry_id) {
                    Some(entry) => entry,
                    None => {
                        return self.catalog_lifecycle_blocked_result(
                            &server,
                            &result,
                            &CatalogLifecycleResult {
                                status: CatalogActionStatus::Blocked,
                                failure_class: "missing_entry".to_string(),
                                reason: "catalog entry is no longer available".to_string(),
                                ..CatalogLifecycleResult::default()
                            },
                            started_at,
                        );
                    }
                };
                let management = match &server.catalog_management {
                    Some(management) => management.clone(),
                    None => {
                        return self.catalog_lifecycle_blocked_result(
                            &server,
                            &result,
                            &CatalogLifecycleResult {
                                status: CatalogActionStatus::Blocked,
                                failure_class: "conflict".to_string(),
                                reason: "server is missing catalog install snapshot metadata".to_string(),
                                ..CatalogLifecycleResult::default()
                            },
                            started_at,
                        );
                    }
                };
                let mut create_input = merge_catalog_install_input(
                    &entry,
                    &catalog_install_input_from_snapshot(&management.install_input_snapshot),
                    server.install_method,
                    self.inner.cfg.environment,
                );
                create_input.catalog_management = Some(catalog_management_for_create(
                    &entry,
                    &create_input,
                    Some(&server),
                    action,
                    Utc::now(),
                ));
                let previous_input = server_to_create_input(&server);
                if action == CatalogAction::Reinstall {
                    if let Err(err) = self.delete_catalog_server(&server.server_id) {
                        return self.catalog_lifecycle_failed_result(
                            &server,
                            &result,
                            "failed",
                            &err.to_string(),
                            started_at,
                        );
                    }
                }
                match self.create_server(create_input) {
                    Ok((resource, _)) => {
                        result.status = CatalogActionStatus::Completed;
                        result.server = Some(resource);
                    }
                    Err(err) => {
                        if action == CatalogAction::Reinstall {
                            let _ = self.create_server(previous_input);
                        }
                        return self.catalog_lifecycle_failed_result(
                            &server,
                            &result,
                            "failed",
                            &err.to_string(),
                            started_at,
                        );
                    }
                }
            }
            _ => {
                return Err(McpError::Other(format!("unsupported catalog action {action}")));
            }
        }
        result.preflight_ms = started_at.elapsed().as_millis() as i64;
        let mut completed_payload = Map::new();
        completed_payload.insert("actionId".to_string(), Value::String(result.action_id.clone()));
        completed_payload.insert("action".to_string(), Value::String(action.as_str().to_string()));
        completed_payload.insert("serverId".to_string(), Value::String(server.server_id.clone()));
        completed_payload.insert(
            "catalogEntryId".to_string(),
            Value::String(server.catalog_entry_id.clone()),
        );
        completed_payload.insert("status".to_string(), Value::String(result.status.as_str().to_string()));
        completed_payload.insert("removed".to_string(), Value::Bool(result.removed));
        completed_payload.insert(
            "environment".to_string(),
            Value::String(environment_scope(self.inner.cfg.environment)),
        );
        if let Ok(completed_event) = self.publish_audit_event(
            "mcp.catalog_lifecycle_completed",
            Resource {
                kind: RESOURCE_KIND_SERVER.to_string(),
                id: server.server_id.clone(),
            },
            completed_payload,
        ) {
            result.audit_event_ids.push(completed_event.event_id.clone());
        }
        Ok(result)
    }

    /// Go `catalogLifecycleBlockResult`. Returns `Ok(None)` when the action may
    /// proceed. The store `HasActiveMCPToolCalls` guard is deferred.
    fn catalog_lifecycle_block_result(
        &self,
        server: &Server,
        action: CatalogAction,
        fail_on_modified: bool,
    ) -> Result<Option<CatalogLifecycleResult>, McpError> {
        if let Some(blocked) = self.catalog_target_block_result(server) {
            return Ok(Some(blocked));
        }
        let (active_session, state_status) = {
            let guard = self.inner.state.read();
            let active_session = guard.sessions.contains_key(&server.server_id);
            let state_status = guard
                .states
                .get(&server.server_id)
                .map(|s| s.status)
                .unwrap_or_default();
            (active_session, state_status)
        };
        if active_session
            || state_status == LifecycleStatus::Starting
            || state_status == LifecycleStatus::Stopping
            || state_status == LifecycleStatus::BackingOff
        {
            return Ok(Some(CatalogLifecycleResult {
                status: CatalogActionStatus::Blocked,
                failure_class: "busy".to_string(),
                reason: "server has an active lifecycle or transport session".to_string(),
                ..CatalogLifecycleResult::default()
            }));
        }
        // Go also blocks when store.HasActiveMCPToolCalls(server.ServerID); kura-store
        // does not expose that query yet, so the check is deferred.
        if fail_on_modified && server.operator_modified {
            return Ok(Some(CatalogLifecycleResult {
                status: CatalogActionStatus::Blocked,
                failure_class: "conflict".to_string(),
                reason: "server has local operator modifications".to_string(),
                ..CatalogLifecycleResult::default()
            }));
        }
        if (action == CatalogAction::Refresh || action == CatalogAction::Reinstall)
            && server.catalog_management.is_none()
        {
            return Ok(Some(CatalogLifecycleResult {
                status: CatalogActionStatus::Blocked,
                failure_class: "conflict".to_string(),
                reason: "server is missing catalog install snapshot metadata".to_string(),
                ..CatalogLifecycleResult::default()
            }));
        }
        Ok(None)
    }

    /// Go `catalogTargetBlockResult`.
    fn catalog_target_block_result(&self, server: &Server) -> Option<CatalogLifecycleResult> {
        if server.origin_kind != OriginKind::Catalog {
            return Some(CatalogLifecycleResult {
                status: CatalogActionStatus::Blocked,
                failure_class: "not_catalog_managed".to_string(),
                reason: "server is not catalog-managed".to_string(),
                ..CatalogLifecycleResult::default()
            });
        }
        let scope = server.environment_scope.trim();
        if !scope.is_empty() && scope != environment_scope(self.inner.cfg.environment) {
            return Some(CatalogLifecycleResult {
                status: CatalogActionStatus::Blocked,
                failure_class: "environment_mismatch".to_string(),
                reason: format!("server belongs to {scope} environment"),
                ..CatalogLifecycleResult::default()
            });
        }
        None
    }

    /// Go `catalogRevalidationBusyBlockResult`. The store active-tool-call check is
    /// deferred (kura-store has no such query).
    fn catalog_revalidation_busy_block_result(
        &self,
        _server: &Server,
    ) -> Result<Option<CatalogLifecycleResult>, McpError> {
        Ok(None)
    }

    /// Go `catalogLifecycleBlockedResult`.
    fn catalog_lifecycle_blocked_result(
        &self,
        server: &Server,
        result: &CatalogLifecycleResult,
        blocked: &CatalogLifecycleResult,
        started_at: Instant,
    ) -> Result<CatalogLifecycleResult, McpError> {
        let mut result = result.clone();
        result.status = blocked.status;
        result.failure_class = blocked.failure_class.clone();
        result.reason = blocked.reason.clone();
        result.preflight_ms = started_at.elapsed().as_millis() as i64;
        self.persist_catalog_action_outcome(
            server,
            result.action,
            result.status,
            &result.failure_class,
            &result.reason,
        )?;
        let mut failed_payload = Map::new();
        failed_payload.insert("actionId".to_string(), Value::String(result.action_id.clone()));
        failed_payload.insert("action".to_string(), Value::String(result.action.as_str().to_string()));
        failed_payload.insert("serverId".to_string(), Value::String(server.server_id.clone()));
        failed_payload.insert(
            "catalogEntryId".to_string(),
            Value::String(server.catalog_entry_id.clone()),
        );
        failed_payload.insert("status".to_string(), Value::String(result.status.as_str().to_string()));
        failed_payload.insert("failureClass".to_string(), Value::String(result.failure_class.clone()));
        failed_payload.insert("reason".to_string(), Value::String(result.reason.clone()));
        failed_payload.insert(
            "environment".to_string(),
            Value::String(environment_scope(self.inner.cfg.environment)),
        );
        if let Ok(failed_event) = self.publish_audit_event(
            "mcp.catalog_lifecycle_failed",
            Resource {
                kind: RESOURCE_KIND_SERVER.to_string(),
                id: server.server_id.clone(),
            },
            failed_payload,
        ) {
            result.audit_event_ids.push(failed_event.event_id.clone());
        }
        if let Some(resource) = self.get_server_resource(&server.server_id) {
            result.server = Some(resource);
        }
        Ok(result)
    }

    /// Go `catalogLifecycleFailedResult`.
    fn catalog_lifecycle_failed_result(
        &self,
        server: &Server,
        result: &CatalogLifecycleResult,
        failure_class: &str,
        reason: &str,
        started_at: Instant,
    ) -> Result<CatalogLifecycleResult, McpError> {
        self.catalog_lifecycle_blocked_result(
            server,
            result,
            &CatalogLifecycleResult {
                status: CatalogActionStatus::Failed,
                failure_class: failure_class.to_string(),
                reason: reason.to_string(),
                ..CatalogLifecycleResult::default()
            },
            started_at,
        )
    }

    /// Go `catalogRevalidationBlockedResult`.
    fn catalog_revalidation_blocked_result(
        &self,
        server: &Server,
        result: &CatalogRevalidationResult,
        blocked: &CatalogLifecycleResult,
        started_at: Instant,
    ) -> CatalogRevalidationResult {
        let mut result = result.clone();
        result.status = AvailabilityStatus::Blocked;
        result.classification = RevalidationClassification::PrerequisiteLost;
        result.reason = blocked.reason.clone();
        result.issues = vec![RevalidationIssue {
            kind: "configuration".to_string(),
            name: blocked.failure_class.clone(),
            status: RevalidationIssueStatus::Blocked,
            reason: blocked.reason.clone(),
            environment_scope: environment_scope(self.inner.cfg.environment),
        }];
        result.preflight_ms = started_at.elapsed().as_millis() as i64;
        let _ = self.persist_catalog_action_outcome(
            server,
            CatalogAction::Revalidate,
            CatalogActionStatus::Blocked,
            &blocked.failure_class,
            &blocked.reason,
        );
        let mut payload = Map::new();
        payload.insert("actionId".to_string(), Value::String(result.action_id.clone()));
        payload.insert("action".to_string(), Value::String(result.action.as_str().to_string()));
        payload.insert("serverId".to_string(), Value::String(server.server_id.clone()));
        payload.insert(
            "catalogEntryId".to_string(),
            Value::String(server.catalog_entry_id.clone()),
        );
        payload.insert("status".to_string(), Value::String(result.status.as_str().to_string()));
        payload.insert(
            "classification".to_string(),
            Value::String(result.classification.as_str().to_string()),
        );
        payload.insert("reason".to_string(), Value::String(result.reason.clone()));
        payload.insert("issues".to_string(), Value::Array(redacted_issues(&result.issues)));
        payload.insert(
            "environment".to_string(),
            Value::String(environment_scope(self.inner.cfg.environment)),
        );
        if let Ok(event) = self.publish_audit_event(
            "mcp.catalog_revalidation_completed",
            Resource {
                kind: RESOURCE_KIND_SERVER.to_string(),
                id: server.server_id.clone(),
            },
            payload,
        ) {
            result.audit_event_ids.push(event.event_id.clone());
        }
        if let Some(resource) = self.get_server_resource(&server.server_id) {
            result.server = Some(resource);
        }
        result
    }

    /// Go `persistCatalogActionOutcome`.
    fn persist_catalog_action_outcome(
        &self,
        server: &Server,
        action: CatalogAction,
        status: CatalogActionStatus,
        failure_class: &str,
        reason: &str,
    ) -> Result<(), McpError> {
        let now = Utc::now();
        let mut server = server.clone();
        server.catalog_management = self.build_catalog_management_locked(&server);
        if server.catalog_management.is_none() {
            server.catalog_management = Some(CatalogManagement::default());
        }
        if let Some(management) = &mut server.catalog_management {
            management.last_action = Some(action);
            management.last_action_status = Some(status);
            management.last_action_failure_class = failure_class.trim().to_string();
            management.last_action_reason = reason.trim().to_string();
            management.last_action_at = Some(now);
        }
        self.set_server(&server);
        self.persist_server(&server)
    }

    /// Go `deleteCatalogServer`.
    fn delete_catalog_server(&self, server_id: &str) -> Result<(), McpError> {
        if let Some(store) = &self.inner.store {
            store
                .lock()
                .map_err(|_| McpError::Store("store lock poisoned".to_string()))?
                .delete_mcp_server(server_id)
                .map_err(McpError::Store)?;
        }
        let mut guard = self.inner.state.write();
        guard.servers.remove(server_id);
        guard.states.remove(server_id);
        guard.tools.remove(server_id);
        guard.exposure.remove(server_id);
        guard.sessions.remove(server_id);
        guard.server_ids.retain(|item| item != server_id);
        Ok(())
    }

    /// Go `setServer`.
    fn set_server(&self, server: &Server) {
        let mut guard = self.inner.state.write();
        if !guard.servers.contains_key(&server.server_id) {
            guard.server_ids.push(server.server_id.clone());
        }
        guard.servers.insert(server.server_id.clone(), server.clone());
    }

    /// Go `collectRevalidationIssues`.
    fn collect_revalidation_issues(
        &self,
        server: &Server,
        management: Option<&CatalogManagement>,
    ) -> (Vec<RevalidationIssue>, AvailabilityStatus, RevalidationClassification, String) {
        let env_scope = environment_scope(self.inner.cfg.environment);
        let mut issues: Vec<RevalidationIssue> = Vec::new();
        let entry = match self.get_catalog_entry(&server.catalog_entry_id) {
            Some(entry) => entry,
            None => {
                issues.push(RevalidationIssue {
                    kind: "catalog".to_string(),
                    name: server.catalog_entry_id.clone(),
                    status: RevalidationIssueStatus::Unavailable,
                    reason: "catalog entry is no longer available".to_string(),
                    environment_scope: env_scope,
                });
                return (
                    issues,
                    AvailabilityStatus::Unavailable,
                    RevalidationClassification::MissingEntry,
                    "catalog entry is no longer available".to_string(),
                );
            }
        };
        let management = match management {
            Some(management) => Some(management.clone()),
            None => self.build_catalog_management_locked(server),
        };
        let spec = self
            .catalog_spec_for_server(server, &entry, true)
            .unwrap_or_default();
        if spec.transport_kind == TransportKind::Stdio {
            if spec.command.trim().is_empty() {
                issues.push(RevalidationIssue {
                    kind: "binary".to_string(),
                    name: "command".to_string(),
                    status: RevalidationIssueStatus::Unavailable,
                    reason: "stdio command is not configured".to_string(),
                    environment_scope: env_scope.clone(),
                });
            } else if look_path(&spec.command).is_none() {
                issues.push(RevalidationIssue {
                    kind: "binary".to_string(),
                    name: spec.command.trim().to_string(),
                    status: RevalidationIssueStatus::Unavailable,
                    reason: "required binary is unavailable".to_string(),
                    environment_scope: env_scope.clone(),
                });
            }
            if requires_offline_verified_local_command(&spec) {
                issues.push(RevalidationIssue {
                    kind: "configuration".to_string(),
                    name: "command".to_string(),
                    status: RevalidationIssueStatus::Unavailable,
                    reason: "default bundled stdio command requires a local command override because sandbox network is denied".to_string(),
                    environment_scope: env_scope.clone(),
                });
            }
        }
        if spec.transport_kind == TransportKind::StreamableHTTP && spec.endpoint.trim().is_empty() {
            issues.push(RevalidationIssue {
                kind: "endpoint".to_string(),
                name: "streamable-http".to_string(),
                status: RevalidationIssueStatus::Unsupported,
                reason: "streamable-http endpoint is not configured".to_string(),
                environment_scope: env_scope.clone(),
            });
        }
        let resolved = self
            .resolve_secret_values(&secret_refs_from_requirements(&entry.secret_requirements))
            .unwrap_or_default();
        for requirement in &entry.secret_requirements {
            if requirement.required && !resolved.contains_key(&requirement.secret_ref) {
                let message = format!("{} is required", requirement.secret_ref);
                issues.push(RevalidationIssue {
                    kind: "secret".to_string(),
                    name: requirement.secret_ref.clone(),
                    status: RevalidationIssueStatus::Blocked,
                    reason: first_non_empty(&[requirement.description.as_str(), &message]),
                    environment_scope: env_scope.clone(),
                });
            }
        }
        if let Some(management) = &management {
            match management.drift_status {
                CatalogDriftStatus::LocallyModified => {
                    issues.push(RevalidationIssue {
                        kind: "catalog".to_string(),
                        name: server.catalog_entry_id.clone(),
                        status: RevalidationIssueStatus::Warning,
                        reason: first_non_empty(&[
                            management.drift_reason.as_str(),
                            "server has local operator modifications",
                        ]),
                        environment_scope: env_scope.clone(),
                    });
                }
                CatalogDriftStatus::CatalogUpdated => {
                    issues.push(RevalidationIssue {
                        kind: "catalog".to_string(),
                        name: server.catalog_entry_id.clone(),
                        status: RevalidationIssueStatus::Warning,
                        reason: first_non_empty(&[
                            management.drift_reason.as_str(),
                            "installed server no longer matches current catalog revision",
                        ]),
                        environment_scope: env_scope.clone(),
                    });
                }
                _ => {}
            }
        }
        let (state_status, state_health_reason) = {
            let guard = self.inner.state.read();
            let state = guard.states.get(&server.server_id);
            (
                state.map(|s| s.status).unwrap_or_default(),
                state.map(|s| s.health_reason.clone()).unwrap_or_default(),
            )
        };
        match state_status {
            LifecycleStatus::Failed
            | LifecycleStatus::Denied
            | LifecycleStatus::Degraded
            | LifecycleStatus::Unsupported => {
                let status = if state_status == LifecycleStatus::Unsupported {
                    RevalidationIssueStatus::Unsupported
                } else {
                    RevalidationIssueStatus::Unavailable
                };
                issues.push(RevalidationIssue {
                    kind: "runtime".to_string(),
                    name: state_status.as_str().to_string(),
                    status,
                    reason: first_non_empty(&[state_health_reason.as_str(), "server is not healthy"]),
                    environment_scope: env_scope.clone(),
                });
            }
            LifecycleStatus::Disabled => {
                issues.push(RevalidationIssue {
                    kind: "runtime".to_string(),
                    name: state_status.as_str().to_string(),
                    status: RevalidationIssueStatus::Blocked,
                    reason: "server is disabled".to_string(),
                    environment_scope: env_scope.clone(),
                });
            }
            _ => {}
        }

        let mut classification = RevalidationClassification::Healthy;
        let mut status = AvailabilityStatus::Ready;
        let mut reason = String::new();
        for issue in &issues {
            if reason.is_empty() {
                reason = issue.reason.clone();
            }
            match issue.status {
                RevalidationIssueStatus::Unsupported => {
                    status = AvailabilityStatus::Unsupported;
                }
                RevalidationIssueStatus::Blocked => {
                    if status != AvailabilityStatus::Unsupported {
                        status = AvailabilityStatus::Blocked;
                    }
                }
                RevalidationIssueStatus::Unavailable => {
                    if status == AvailabilityStatus::Ready {
                        status = AvailabilityStatus::Unavailable;
                    }
                }
                RevalidationIssueStatus::Warning => {}
            }
            match issue.kind.as_str() {
                "secret" | "binary" | "endpoint" | "configuration" => {
                    if classification == RevalidationClassification::Healthy {
                        classification = RevalidationClassification::PrerequisiteLost;
                    }
                }
                "runtime" => {
                    if classification == RevalidationClassification::Healthy {
                        classification = RevalidationClassification::RuntimeUnhealthy;
                    }
                }
                _ => {}
            }
        }
        if classification == RevalidationClassification::Healthy {
            if let Some(management) = &management {
                match management.drift_status {
                    CatalogDriftStatus::LocallyModified => {
                        classification = RevalidationClassification::LocallyModified;
                        if reason.is_empty() {
                            reason = management.drift_reason.clone();
                        }
                    }
                    CatalogDriftStatus::CatalogUpdated => {
                        classification = RevalidationClassification::CatalogDrift;
                        if reason.is_empty() {
                            reason = management.drift_reason.clone();
                        }
                    }
                    _ => {}
                }
            }
        }
        (issues, status, classification, reason)
    }

    /// Go `buildSecretSummaries`.
    fn build_secret_summaries(&self, server: &Server) -> Vec<SecretSummary> {
        if server.secret_refs.is_empty() {
            return Vec::new();
        }
        let environment_scope = environment_scope(self.inner.cfg.environment);
        let mut resolution_by_ref: HashMap<String, kura_sandbox::SecretResolution> = HashMap::new();
        if let Ok(bindings) = self.list_secret_bindings(server) {
            for item in bindings {
                resolution_by_ref.insert(item.secret_ref.clone(), item.resolution);
            }
        }
        let mut items = Vec::with_capacity(server.secret_refs.len());
        for secret_ref in &server.secret_refs {
            let resolution = resolution_by_ref
                .get(secret_ref)
                .copied()
                .unwrap_or(kura_sandbox::SecretResolution::Unavailable);
            items.push(SecretSummary {
                consumer_id: server.server_id.clone(),
                secret_ref: secret_ref.clone(),
                environment_scope: environment_scope.clone(),
                default_rule_id: format!("mcp_server:{}", server.server_id),
                resolution: resolution.as_str().to_string(),
                delivery_kind: "environment_variable".to_string(),
                redaction_rule: "value_redacted".to_string(),
                ..SecretSummary::default()
            });
        }
        items
    }

    /// Go `redactValue`: recursively replaces secret-derived forms with [REDACTED].
    fn redact_value(&self, server: &Server, value: Value) -> Value {
        let secrets = match self.resolve_secret_values(&server.secret_refs) {
            Ok(secrets) => secrets,
            Err(_) => return value,
        };
        if secrets.is_empty() {
            return value;
        }
        match value {
            Value::String(text) => Value::String(redact_string(&text, &secrets)),
            Value::Array(items) => Value::Array(
                items
                    .into_iter()
                    .map(|item| self.redact_value(server, item))
                    .collect(),
            ),
            Value::Object(map) => Value::Object(
                map.into_iter()
                    .map(|(key, item)| (key, self.redact_value(server, item)))
                    .collect(),
            ),
            other => other,
        }
    }
}

/// Go `updateOperation`: the internal create-vs-update discriminator.
struct UpdateOperation {
    server_id: String,
    input: UpdateServerInput,
}

/// Go `defaultWebsocketHeaderName`.
#[must_use]
pub fn default_websocket_header_name(auth: &WebsocketAuthConfig) -> String {
    if auth.mode == WebsocketAuthMode::BearerHeader && auth.header_name.trim().is_empty() {
        return "Authorization".to_string();
    }
    auth.header_name.trim().to_string()
}

/// Go `defaultWebsocketScheme`.
#[must_use]
pub fn default_websocket_scheme(auth: &WebsocketAuthConfig) -> String {
    if auth.mode == WebsocketAuthMode::BearerHeader && auth.scheme.trim().is_empty() {
        return "Bearer".to_string();
    }
    auth.scheme.trim().to_string()
}

/// Go `validateWebsocketEndpoint`.
pub fn validate_websocket_endpoint(raw: &str) -> Result<(), McpError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(McpError::TransportUnavailable);
    }
    let parsed = url::Url::parse(raw)
        .map_err(|err| McpError::Other(format!("websocket endpoint is invalid: {err}")))?;
    if parsed.scheme() != "ws" && parsed.scheme() != "wss" {
        return Err(McpError::Other("websocket endpoint must use ws or wss".to_string()));
    }
    if parsed.host_str().is_none_or(|host| host.trim().is_empty()) {
        return Err(McpError::Other("websocket endpoint must include a host".to_string()));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(McpError::Other(
            "websocket endpoint must not include inline credentials; use websocketConfig.auth instead"
                .to_string(),
        ));
    }
    if !parsed.query().unwrap_or_default().trim().is_empty() {
        return Err(McpError::Other(
            "websocket endpoint must not include inline query parameters; use websocketConfig.auth instead"
                .to_string(),
        ));
    }
    Ok(())
}

/// Go `sanitizeWebsocketEndpointForProjection`: strips inline credentials/query/fragment
/// from the projected endpoint.
#[must_use]
pub fn sanitize_websocket_endpoint_for_projection(raw: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(raw.trim()) else {
        return raw.trim().to_string();
    };
    let _ = parsed.set_username("");
    let _ = parsed.set_password(None);
    parsed.set_query(None);
    parsed.set_fragment(None);
    parsed.as_str().to_string()
}

/// Go `defaultStateForServer`.
#[must_use]
pub fn default_state_for_server(server: &Server) -> ServerState {
    let now = Utc::now();
    let status = if server.enabled {
        LifecycleStatus::Stopped
    } else {
        LifecycleStatus::Disabled
    };
    ServerState {
        server_id: server.server_id.clone(),
        status,
        updated_at: now,
        ..ServerState::default()
    }
}

/// Go `lifecycleStatusFromExecution`.
#[must_use]
pub fn lifecycle_status_from_execution(execution: &kura_sandbox::Execution) -> LifecycleStatus {
    if execution.status == kura_sandbox::ExecutionStatus::Unsupported
        || execution.decision.selection_outcome == Some(kura_sandbox::BackendSelectionOutcome::Unsupported)
    {
        return LifecycleStatus::Unsupported;
    }
    if execution.result.error_code == "sandbox_profile_not_found"
        || execution.result.error_class == kura_sandbox::ErrorClass::InvalidProfile.as_str()
    {
        return LifecycleStatus::Denied;
    }
    LifecycleStatus::Denied
}

/// Go `classifyExecutionFailure`.
#[must_use]
pub fn classify_execution_failure(execution: &kura_sandbox::Execution) -> String {
    match execution.result.error_class.as_str() {
        value if value == kura_sandbox::ErrorClass::LaunchFailed.as_str() => "launch_failed".to_string(),
        value if value == kura_sandbox::ErrorClass::Timeout.as_str() => "timeout".to_string(),
        value if value == kura_sandbox::ErrorClass::Cancelled.as_str() => "cancelled".to_string(),
        value if value == kura_sandbox::ErrorClass::ProcessFailed.as_str() => {
            "transport_runtime_failure".to_string()
        }
        value
            if value == kura_sandbox::ErrorClass::PolicyDenied.as_str()
                || value == kura_sandbox::ErrorClass::ApprovalRequired.as_str()
                || value == kura_sandbox::ErrorClass::ApprovalRejected.as_str()
                || value == kura_sandbox::ErrorClass::InvalidProfile.as_str() =>
        {
            "policy_denied".to_string()
        }
        _ => {
            if execution.status == kura_sandbox::ExecutionStatus::Denied {
                "policy_denied".to_string()
            } else {
                execution.result.error_class.trim().to_string()
            }
        }
    }
}

/// Go `mergeCatalogInstallInput`.
#[must_use]
pub fn merge_catalog_install_input(
    entry: &CatalogEntry,
    input: &CatalogInstallInput,
    method: InstallMethod,
    environment: kura_config::Environment,
) -> CreateServerInput {
    let mut spec = entry.default_install_spec.clone();
    let mut server_id = input.server_id.trim().to_string();
    if server_id.is_empty() {
        server_id = entry.id.clone();
    }
    spec.server_id = server_id;
    spec.origin_kind = OriginKind::Catalog;
    spec.catalog_entry_id = entry.id.clone();
    spec.install_method = method;
    spec.environment_scope = environment_scope(environment);
    if !input.display_name.trim().is_empty() {
        spec.display_name = input.display_name.trim().to_string();
    }
    if let Some(enabled) = input.enabled {
        spec.enabled = enabled;
    }
    if !input.sandbox_profile_id.trim().is_empty() {
        spec.sandbox_profile_id = input.sandbox_profile_id.trim().to_string();
    }
    if !input.command.trim().is_empty() {
        spec.command = input.command.trim().to_string();
    }
    if !input.args.is_empty() {
        spec.args = input.args.clone();
    }
    if !input.endpoint.trim().is_empty() {
        spec.endpoint = input.endpoint.trim().to_string();
    }
    if !input.working_dir.trim().is_empty() {
        spec.working_dir = input.working_dir.trim().to_string();
    }
    if !input.secret_refs.is_empty() {
        spec.secret_refs = clean_strings(&input.secret_refs);
    }
    spec
}

/// Go `catalogManagementForCreate`.
#[must_use]
pub fn catalog_management_for_create(
    entry: &CatalogEntry,
    create_input: &CreateServerInput,
    previous: Option<&Server>,
    action: CatalogAction,
    now: DateTime<Utc>,
) -> CatalogManagement {
    let mut management = CatalogManagement {
        source_kind: entry.source_kind.clone(),
        installed_revision: fingerprint_create_server_spec(create_input),
        current_revision: fingerprint_create_server_spec(create_input),
        install_input_snapshot: install_snapshot_from_create_spec(create_input),
        last_action: Some(action),
        last_action_status: Some(CatalogActionStatus::Completed),
        last_action_at: Some(now),
        ..CatalogManagement::default()
    };
    if let Some(previous) = previous {
        if let Some(previous_management) = &previous.catalog_management {
            management.installed_at = previous_management.installed_at;
        }
    }
    if management.installed_at.is_none() {
        management.installed_at = Some(now);
    }
    if action == CatalogAction::Refresh || action == CatalogAction::Reinstall {
        management.last_maintained_at = Some(now);
    }
    management
}

/// Go `serverToCreateInput`.
#[must_use]
pub fn server_to_create_input(server: &Server) -> CreateServerInput {
    CreateServerInput {
        // Carried, or a round trip through this helper would move the server
        // to the tenant that has none and hide it from its owner.
        tenant_id: server.tenant_id.clone(),
        server_id: server.server_id.clone(),
        display_name: server.display_name.clone(),
        origin_kind: server.origin_kind,
        catalog_entry_id: server.catalog_entry_id.clone(),
        install_method: server.install_method,
        environment_scope: server.environment_scope.clone(),
        enabled: server.enabled,
        sandbox_profile_id: server.sandbox_profile_id.clone(),
        declaration_id: server.declaration_id.clone(),
        declaration: clone_declaration_ptr(server.declaration.clone()),
        transport_kind: server.transport_kind,
        command: server.command.clone(),
        args: clone_strings(&server.args),
        endpoint: server.endpoint.clone(),
        websocket_config: clone_websocket_config(&server.websocket_config),
        working_dir: server.working_dir.clone(),
        secret_refs: clean_strings(&server.secret_refs),
        auto_restart: server.auto_restart,
        operator_modified: server.operator_modified,
        catalog_management: clone_catalog_management(&server.catalog_management),
    }
}

/// Go `assessCatalogDrift`.
#[must_use]
pub fn assess_catalog_drift(
    server: &Server,
    management: &CatalogManagement,
    entry_present: bool,
) -> (CatalogDriftStatus, String) {
    if server.origin_kind != OriginKind::Catalog {
        return (CatalogDriftStatus::default(), String::new());
    }
    if !entry_present {
        return (
            CatalogDriftStatus::MissingEntry,
            "catalog entry is no longer available".to_string(),
        );
    }
    if server.operator_modified {
        if !management.installed_revision.is_empty()
            && !management.current_revision.is_empty()
            && management.installed_revision != management.current_revision
        {
            return (
                CatalogDriftStatus::LocallyModified,
                "server has local modifications and the catalog entry has changed".to_string(),
            );
        }
        return (
            CatalogDriftStatus::LocallyModified,
            "server has local operator modifications".to_string(),
        );
    }
    if !management.installed_revision.is_empty()
        && !management.current_revision.is_empty()
        && management.installed_revision != management.current_revision
    {
        return (
            CatalogDriftStatus::CatalogUpdated,
            "installed server no longer matches the current catalog revision".to_string(),
        );
    }
    (CatalogDriftStatus::InSync, String::new())
}

/// Go `sanitizeCatalogManagementProjection`.
#[must_use]
pub fn sanitize_catalog_management_projection(
    management: Option<CatalogManagement>,
) -> Option<CatalogManagement> {
    management.map(|mut projected| {
        projected.install_input_snapshot =
            sanitize_catalog_install_snapshot_projection(projected.install_input_snapshot);
        projected
    })
}

/// Go `sanitizeCatalogInstallSnapshotProjection`.
#[must_use]
pub fn sanitize_catalog_install_snapshot_projection(
    mut snapshot: CatalogInstallSnapshot,
) -> CatalogInstallSnapshot {
    snapshot.command = String::new();
    snapshot.args = Vec::new();
    snapshot.endpoint = String::new();
    snapshot.working_dir = String::new();
    snapshot
}

/// Go `redactedIssues`.
#[must_use]
pub fn redacted_issues(issues: &[RevalidationIssue]) -> Vec<Value> {
    issues
        .iter()
        .map(|issue| {
            serde_json::json!({
                "kind": issue.kind,
                "name": issue.name,
                "status": issue.status.as_str(),
                "reason": issue.reason,
                "environmentScope": issue.environment_scope,
            })
        })
        .collect()
}

/// Go `catalogManagementPayload` (non-nil input only; callers pass `Value::Null` for
/// a missing management block).
#[must_use]
pub fn catalog_management_payload(management: &CatalogManagement) -> Map<String, Value> {
    let mut payload = Map::new();
    payload.insert("sourceKind".to_string(), Value::String(management.source_kind.clone()));
    payload.insert(
        "installedRevision".to_string(),
        Value::String(management.installed_revision.clone()),
    );
    payload.insert(
        "currentRevision".to_string(),
        Value::String(management.current_revision.clone()),
    );
    payload.insert(
        "driftStatus".to_string(),
        Value::String(management.drift_status.as_str().to_string()),
    );
    payload.insert("driftReason".to_string(), Value::String(management.drift_reason.clone()));
    if let Some(installed_at) = management.installed_at {
        payload.insert("installedAt".to_string(), Value::String(rfc3339_nano(installed_at)));
    }
    if let Some(last_maintained_at) = management.last_maintained_at {
        payload.insert(
            "lastMaintainedAt".to_string(),
            Value::String(rfc3339_nano(last_maintained_at)),
        );
    }
    if let Some(last_action_at) = management.last_action_at {
        payload.insert("lastActionAt".to_string(), Value::String(rfc3339_nano(last_action_at)));
    }
    if let Some(last_action) = management.last_action {
        payload.insert("lastAction".to_string(), Value::String(last_action.as_str().to_string()));
    }
    if let Some(last_action_status) = management.last_action_status {
        payload.insert(
            "lastActionStatus".to_string(),
            Value::String(last_action_status.as_str().to_string()),
        );
    }
    if !management.last_action_failure_class.is_empty() {
        payload.insert(
            "lastActionFailureClass".to_string(),
            Value::String(management.last_action_failure_class.clone()),
        );
    }
    if !management.last_action_reason.is_empty() {
        payload.insert(
            "lastActionReason".to_string(),
            Value::String(management.last_action_reason.clone()),
        );
    }
    if let Some(last_revalidation) = &management.last_revalidation {
        payload.insert(
            "lastRevalidation".to_string(),
            serde_json::json!({
                "checkedAt": rfc3339_nano(last_revalidation.checked_at),
                "status": last_revalidation.status.as_str(),
                "classification": last_revalidation.classification.as_str(),
                "reason": last_revalidation.reason,
                "issues": redacted_issues(&last_revalidation.issues),
            }),
        );
    }
    payload
}

/// Go `catalogInstallConflictReason`.
#[must_use]
pub fn catalog_install_conflict_reason(existing: &Server, entry_id: &str) -> Option<String> {
    if existing.origin_kind != OriginKind::Catalog {
        return Some("server id is already owned by a manual MCP server".to_string());
    }
    if existing.catalog_entry_id != entry_id {
        return Some(format!(
            "server id is already owned by catalog entry {}",
            existing.catalog_entry_id
        ));
    }
    if existing.operator_modified {
        return Some("existing installed server has operator modifications".to_string());
    }
    None
}

/// Go `redactString`.
#[must_use]
pub fn redact_string(input: &str, secrets: &HashMap<String, String>) -> String {
    let mut redacted = input.to_string();
    for value in secrets.values() {
        for candidate in redaction_candidates(value) {
            redacted = redacted.replace(&candidate, "[REDACTED]");
        }
    }
    redacted
}

/// Go `redactionCandidates`: the secret plus its query-escaped, base64 (std/url,
/// padded/raw), and hex (lower/upper) forms.
#[must_use]
pub fn redaction_candidates(secret: &str) -> Vec<String> {
    const STD_ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    const URL_ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let trimmed = secret.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let bytes = trimmed.as_bytes();
    let mut seen: Vec<String> = Vec::new();
    let mut add = |value: String| {
        if !value.trim().is_empty() && !seen.contains(&value) {
            seen.push(value);
        }
    };
    add(trimmed.to_string());
    add(
        url::form_urlencoded::byte_serialize(bytes)
            .collect::<String>()
            .replace("%20", "+"),
    );
    add(base64_encode(bytes, STD_ALPHABET, true));
    add(base64_encode(bytes, STD_ALPHABET, false));
    add(base64_encode(bytes, URL_ALPHABET, true));
    add(base64_encode(bytes, URL_ALPHABET, false));
    let hex = hex_encode(bytes);
    add(hex.clone());
    add(hex.to_uppercase());
    seen
}

/// Minimal base64 encoder (Go encoding/base64 Std/Raw/URL/RawURL encodings).
fn base64_encode(input: &[u8], alphabet: &[u8; 64], pad: bool) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(alphabet[((n >> 18) & 63) as usize] as char);
        out.push(alphabet[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(alphabet[((n >> 6) & 63) as usize] as char);
        } else if pad {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(alphabet[(n & 63) as usize] as char);
        } else if pad {
            out.push('=');
        }
    }
    out
}

/// Lowercase hex encoding (Go encoding/hex).
fn hex_encode(input: &[u8]) -> String {
    input.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Go `stringFromMap`.
#[must_use]
pub fn string_from_map(input: &Map<String, Value>, key: &str) -> String {
    match input.get(key) {
        Some(Value::String(value)) => value.trim().to_string(),
        _ => String::new(),
    }
}

/// Go `sessionID`.
#[must_use]
pub fn session_id(active: Option<&SessionState>) -> String {
    match active {
        Some(active) => active.session_id.trim().to_string(),
        None => String::new(),
    }
}

/// Go `secretResolution`.
#[must_use]
pub fn secret_resolution(items: &[kura_sandbox::SecretScopeOutcome]) -> kura_sandbox::SecretResolution {
    if items.is_empty() {
        return kura_sandbox::SecretResolution::NotApplicable;
    }
    for item in items {
        if item.resolution == kura_sandbox::SecretResolution::Unavailable {
            return kura_sandbox::SecretResolution::Unavailable;
        }
    }
    for item in items {
        if item.resolution == kura_sandbox::SecretResolution::Denied {
            return kura_sandbox::SecretResolution::Denied;
        }
    }
    kura_sandbox::SecretResolution::Resolved
}

/// Go `consumeViewMap` (the name is a typo in the original): JSON round-trip of the
/// consumer view into an arbitrary value.
#[must_use]
pub fn consumer_view_map(view: &kura_sandbox::ConsumerContractView) -> Option<Value> {
    serde_json::to_value(view).ok()
}

/// Go `policy.approval_requested` event payload.
#[must_use]
pub fn approval_payload(approval: &kura_policy::Approval) -> Map<String, Value> {
    let mut payload = Map::new();
    payload.insert("action".to_string(), Value::String(approval.action.clone()));
    payload.insert("resourceKind".to_string(), Value::String(approval.resource_kind.clone()));
    payload.insert("resourceId".to_string(), Value::String(approval.resource_id.clone()));
    payload.insert("status".to_string(), Value::String(approval.status.as_str().to_string()));
    payload.insert(
        "sandbox".to_string(),
        approval.sandbox.clone().unwrap_or(Value::Null),
    );
    payload
}

/// Go `policy.decision_recorded` event payload.
#[must_use]
pub fn decision_payload(decision: &kura_policy::Decision) -> Map<String, Value> {
    let mut payload = Map::new();
    payload.insert("action".to_string(), Value::String(decision.action.clone()));
    payload.insert("resourceKind".to_string(), Value::String(decision.resource_kind.clone()));
    payload.insert("resourceId".to_string(), Value::String(decision.resource_id.clone()));
    payload.insert("outcome".to_string(), Value::String(decision.outcome.as_str().to_string()));
    payload.insert("approvalId".to_string(), Value::String(decision.approval_id.clone()));
    payload.insert(
        "sandbox".to_string(),
        decision.sandbox.clone().unwrap_or(Value::Null),
    );
    payload
}

/// Go event id: `evt_<name sanitized>_<unix nanos>`.
#[must_use]
pub fn event_id(name: &str) -> String {
    format!(
        "evt_{}_{}",
        name.replace('.', "_").replace(':', "_"),
        Utc::now().timestamp_nanos_opt().unwrap_or(0)
    )
}

/// Go policy-record timestamp: `YYYYMMDDHHMMSS` + 9-digit nanoseconds, dot stripped.
#[must_use]
pub fn policy_record_timestamp() -> String {
    let now = Utc::now();
    format!(
        "{}{:09}",
        now.format("%Y%m%d%H%M%S"),
        now.timestamp_subsec_nanos()
    )
}
