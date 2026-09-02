//! Integration tests for the kura-mcp crate: round-trip persistence (in-memory and via
//! kura-store), manager behavior (registry, exposure, authorization, lifecycle with a
//! fake transport, catalog install/lifecycle), and pure-helper coverage (framing,
//! redaction, backoff, websocket endpoint validation).

mod common;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use kura_mcp::catalog::{
    fingerprint_create_server_spec, requires_offline_verified_local_command,
};
use kura_mcp::manager::{
    redact_string, sanitize_websocket_endpoint_for_projection, validate_websocket_endpoint,
};
use kura_mcp::transport::read_framed_message;
use kura_mcp::types::*;
use kura_mcp::{
    McpError, Session, SessionPipes, Transport, is_terminal_status, live_validation_matrix_rows,
    restart_backoff_delay,
};
use serde_json::{Map, Value};

fn test_cfg(data_dir: &str) -> kura_config::Config {
    kura_config::Config {
        project_root: String::new(),
        environment: kura_config::Environment::Test,
        bind_addr: "127.0.0.1:19192".to_string(),
        data_dir: data_dir.to_string(),
        log_level: "info".to_string(),
        version: "dev".to_string(),
        llm: Default::default(),
        connectors: Default::default(),
    }
}

fn streamable_server_input(server_id: &str) -> CreateServerInput {
    CreateServerInput {
        server_id: server_id.to_string(),
        display_name: "Test Server".to_string(),
        enabled: true,
        sandbox_profile_id: kura_sandbox::PROFILE_ID_SUBPROCESS_DEFAULT.to_string(),
        declaration_id: format!("mcp_server:{server_id}:lifecycle.start"),
        transport_kind: TransportKind::StreamableHTTP,
        endpoint: "https://example.test/mcp".to_string(),
        auto_restart: true,
        ..CreateServerInput::default()
    }
}

// ---------------------------------------------------------------------------
// Fake transport / session for lifecycle tests
// ---------------------------------------------------------------------------

struct FakeSession {
    id: String,
    tools: Vec<Tool>,
    done_tx: Mutex<mpsc::Sender<Result<(), String>>>,
    done_rx: Mutex<mpsc::Receiver<Result<(), String>>>,
}

impl FakeSession {
    fn new(id: &str, tools: Vec<Tool>) -> Arc<Self> {
        let (tx, rx) = mpsc::channel();
        Arc::new(FakeSession {
            id: id.to_string(),
            tools,
            done_tx: Mutex::new(tx),
            done_rx: Mutex::new(rx),
        })
    }
}

impl Session for FakeSession {
    fn id(&self) -> String {
        self.id.clone()
    }
    fn list_tools(&self, _timeout: Duration) -> Result<Vec<Tool>, String> {
        Ok(self.tools.clone())
    }
    fn call_tool(&self, _tool_name: &str, _input: Value) -> Result<Map<String, Value>, String> {
        Ok(Map::new())
    }
    fn close(&self) -> Result<(), String> {
        let _ = self.done_tx.lock().unwrap().send(Ok(()));
        Ok(())
    }
    fn wait_done(&self) -> Result<(), String> {
        self.done_rx.lock().unwrap().recv().unwrap_or(Ok(()))
    }
}

#[derive(Clone)]
struct FakeTransport {
    session: Arc<FakeSession>,
}

impl Transport for FakeTransport {
    fn open(
        &self,
        _server: &Server,
        _pipes: SessionPipes,
        _timeout: Duration,
    ) -> Result<Arc<dyn Session>, McpError> {
        Ok(self.session.clone())
    }
}

/// Test-only sandbox execution starter: spawns the `fake-mcp-server` bin as the
/// attached process, tracks children by execution id, and kills them on cancel so the
/// stdio session read loop terminates (mirroring the real sandbox execution plane).
struct FakeAttachedExecutionStarter {
    next_id: AtomicU64,
    children: Mutex<HashMap<String, std::process::Child>>,
}

impl kura_mcp::AttachedExecutionStarter for FakeAttachedExecutionStarter {
    fn start_attached_execution(
        &self,
        _request: &kura_sandbox::ExecutionRequest,
    ) -> Result<(kura_sandbox::Execution, Option<kura_mcp::AttachedExecution>), String> {
        let mut child = std::process::Command::new(common::fake_mcp_server_bin())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|err| format!("spawn fake mcp server: {err}"))?;
        let execution_id = format!("fake-exec-{}", self.next_id.fetch_add(1, Ordering::SeqCst));
        let execution = kura_sandbox::Execution {
            execution_id: execution_id.clone(),
            status: kura_sandbox::ExecutionStatus::Running,
            ..kura_sandbox::Execution::default()
        };
        let attached = kura_mcp::AttachedExecution {
            execution: execution.clone(),
            stdin: Some(Box::new(child.stdin.take().expect("child stdin"))),
            stdout: Some(Box::new(child.stdout.take().expect("child stdout"))),
            stderr: Some(Box::new(child.stderr.take().expect("child stderr"))),
        };
        self.children.lock().unwrap().insert(execution_id, child);
        Ok((execution, Some(attached)))
    }

    fn cancel_execution(
        &self,
        execution_id: &str,
    ) -> Result<(kura_sandbox::Execution, bool), String> {
        if let Some(mut child) = self.children.lock().unwrap().remove(execution_id) {
            let _ = child.kill();
            let _ = child.wait();
        }
        Ok((
            kura_sandbox::Execution {
                execution_id: execution_id.to_string(),
                status: kura_sandbox::ExecutionStatus::Cancelled,
                ..kura_sandbox::Execution::default()
            },
            true,
        ))
    }

    fn get_execution(&self, execution_id: &str) -> Option<kura_sandbox::Execution> {
        Some(kura_sandbox::Execution {
            execution_id: execution_id.to_string(),
            status: kura_sandbox::ExecutionStatus::Completed,
            ..kura_sandbox::Execution::default()
        })
    }

    fn persist_consumer_view(
        &self,
        _view: &kura_sandbox::ConsumerContractView,
    ) -> Result<(), String> {
        Ok(())
    }

    fn get_profile(&self, profile_id: &str) -> Option<kura_sandbox::Profile> {
        Some(kura_sandbox::Profile {
            profile_id: profile_id.to_string(),
            backend_kind: kura_sandbox::BackendKind::Subprocess,
            active: true,
            ..kura_sandbox::Profile::default()
        })
    }
}

fn fake_tool(name: &str) -> Tool {
    Tool {
        tool_name: name.to_string(),
        discovery_status: DiscoveryStatus::Discovered,
        updated_at: Utc::now(),
        ..Tool::default()
    }
}

// ---------------------------------------------------------------------------
// Serde round-trip
// ---------------------------------------------------------------------------

#[test]
fn server_serde_round_trip_uses_camel_case_wire() {
    let now = Utc::now();
    let server = Server {
        tenant_id: "tenant-1".to_string(),
        server_id: "srv-1".to_string(),
        display_name: "Files".to_string(),
        source: Source::Config,
        origin_kind: OriginKind::Catalog,
        catalog_entry_id: "filesystem".to_string(),
        install_method: InstallMethod::Script,
        environment_scope: "test".to_string(),
        enabled: true,
        sandbox_profile_id: "subprocess_default".to_string(),
        declaration_id: "mcp_server:filesystem:lifecycle.start".to_string(),
        declaration: Declaration {
            execution_mode: kura_sandbox::ExecutionMode::Subprocess,
            allowed_backend_kinds: vec![kura_sandbox::BackendKind::Subprocess],
            read_roots: vec!["/tmp/root".to_string()],
            network_mode: kura_sandbox::NetworkMode::Deny,
            approval_mode: kura_sandbox::ApprovalMode::Allow,
            required_enforcement_strength: "declared_only".to_string(),
            active: true,
            ..Declaration::default()
        },
        transport_kind: TransportKind::StreamableHTTP,
        command: "npx".to_string(),
        args: vec!["-y".to_string(), "server".to_string()],
        endpoint: "https://example.test/mcp".to_string(),
        working_dir: "/tmp".to_string(),
        secret_refs: vec!["TOKEN".to_string()],
        auto_restart: true,
        operator_modified: false,
        created_at: now,
        updated_at: now,
        ..Server::default()
    };
    let value = serde_json::to_value(&server).unwrap();
    assert_eq!(value["serverId"], "srv-1");
    assert_eq!(value["displayName"], "Files");
    assert_eq!(value["source"], "config");
    assert_eq!(value["originKind"], "catalog");
    assert_eq!(value["installMethod"], "script");
    assert_eq!(value["transportKind"], "streamable-http");
    assert_eq!(value["declaration"]["executionMode"], "subprocess");
    assert_eq!(value["declaration"]["networkMode"], "deny");
    // resolvedWebsocketHeaders is json:"-" and must not serialize
    assert!(value.get("resolvedWebsocketHeaders").is_none());

    let decoded: Server = serde_json::from_value(value).unwrap();
    assert_eq!(decoded.server_id, "srv-1");
    assert_eq!(decoded.transport_kind, TransportKind::StreamableHTTP);
    assert_eq!(decoded.declaration.active, true);
    assert_eq!(decoded.source, Source::Config);
}

#[test]
fn exposure_rule_serde_round_trip() {
    let rule = ToolExposureRule {
        server_id: "s".to_string(),
        tool_name: "lookup".to_string(),
        runtime_surface: "chat".to_string(),
        exposure_mode: ExposureMode::ApprovalRequired,
        active: true,
        reason: "needs human gate".to_string(),
        updated_at: Utc::now(),
        ..ToolExposureRule::default()
    };
    let value = serde_json::to_value(&rule).unwrap();
    assert_eq!(value["exposureMode"], "approval_required");
    let decoded: ToolExposureRule = serde_json::from_value(value).unwrap();
    assert_eq!(decoded.exposure_mode, ExposureMode::ApprovalRequired);
}

// ---------------------------------------------------------------------------
// Registry behavior
// ---------------------------------------------------------------------------

#[test]
fn manager_registers_updates_and_lists_servers() {
    let manager = kura_mcp::Manager::new(test_cfg("~/.kura-test"), None, None, None, None, None);
    let (resource, created) = manager.create_server(streamable_server_input("srv-1")).unwrap();
    assert!(created);
    assert_eq!(resource.server.server_id, "srv-1");
    assert_eq!(resource.server.source, Source::Api);
    assert_eq!(resource.server.origin_kind, OriginKind::Manual);
    assert_eq!(resource.server.environment_scope, "test");
    // enabled => default state is Stopped
    assert_eq!(resource.state.status, LifecycleStatus::Stopped);
    assert_eq!(resource.availability_status, AvailabilityStatus::Ready);

    let servers = manager.list_servers();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].server.server_id, "srv-1");

    let server = manager.get_server("srv-1").unwrap();
    assert_eq!(server.display_name, "Test Server");

    let updated = manager
        .update_server(
            "srv-1",
            &UpdateServerInput {
                display_name: Some("Renamed".to_string()),
                enabled: Some(false),
                auto_restart: Some(false),
                ..UpdateServerInput::default()
            },
        )
        .unwrap();
    assert_eq!(updated.server.display_name, "Renamed");
    assert!(updated.server.operator_modified);
    assert!(!updated.server.enabled);
    assert_eq!(updated.state.status, LifecycleStatus::Disabled);
}

#[test]
fn manager_rejects_invalid_servers() {
    let manager = kura_mcp::Manager::new(test_cfg("~/.kura-test"), None, None, None, None, None);
    // missing server id
    let err = manager
        .create_server(CreateServerInput::default())
        .unwrap_err();
    assert_eq!(err, McpError::ServerIDRequired);
    // missing declaration id
    let err = manager
        .create_server(CreateServerInput {
            server_id: "s".to_string(),
            sandbox_profile_id: "p".to_string(),
            transport_kind: TransportKind::StreamableHTTP,
            endpoint: "https://example.test/mcp".to_string(),
            ..CreateServerInput::default()
        })
        .unwrap_err();
    assert_eq!(err, McpError::DeclarationIDRequired);
    // stdio requires a command
    let err = manager
        .create_server(CreateServerInput {
            server_id: "s".to_string(),
            sandbox_profile_id: "p".to_string(),
            declaration_id: "d".to_string(),
            transport_kind: TransportKind::Stdio,
            ..CreateServerInput::default()
        })
        .unwrap_err();
    assert_eq!(err, McpError::CommandRequired);
    // streamable-http requires an endpoint
    let err = manager
        .create_server(CreateServerInput {
            server_id: "s".to_string(),
            sandbox_profile_id: "p".to_string(),
            declaration_id: "d".to_string(),
            transport_kind: TransportKind::StreamableHTTP,
            ..CreateServerInput::default()
        })
        .unwrap_err();
    assert_eq!(err, McpError::TransportUnavailable);
    // auto-restart requires enabled
    let err = manager
        .create_server(CreateServerInput {
            server_id: "s".to_string(),
            sandbox_profile_id: "p".to_string(),
            declaration_id: "d".to_string(),
            transport_kind: TransportKind::StreamableHTTP,
            endpoint: "https://example.test/mcp".to_string(),
            auto_restart: true,
            enabled: false,
            ..CreateServerInput::default()
        })
        .unwrap_err();
    assert_eq!(err, McpError::AutoRestartRequiresOn);
    // websocket endpoint validation
    let err = manager
        .create_server(CreateServerInput {
            server_id: "s".to_string(),
            sandbox_profile_id: "p".to_string(),
            declaration_id: "d".to_string(),
            transport_kind: TransportKind::Websocket,
            endpoint: "https://example.test/mcp".to_string(),
            ..CreateServerInput::default()
        })
        .unwrap_err();
    assert!(matches!(err, McpError::Other(_)));
}

// ---------------------------------------------------------------------------
// Lifecycle with a fake transport
// ---------------------------------------------------------------------------

#[test]
fn manager_start_discovers_tools_and_supports_stop() {
    let session = FakeSession::new("session-1", vec![fake_tool("lookup")]);
    let manager = kura_mcp::Manager::new(
        test_cfg("~/.kura-test"),
        None,
        None,
        None,
        None,
        Some(Arc::new(FakeTransport { session })),
    );
    manager.create_server(streamable_server_input("srv-1")).unwrap();

    let response = manager.start("srv-1", "operator").unwrap();
    assert_eq!(response.action, LifecycleAction::Start);
    assert!(!response.idempotent);
    assert_eq!(response.server.state.status, LifecycleStatus::Healthy);
    assert_eq!(response.server.state.last_session_id, "session-1");
    assert_eq!(response.server.tool_count, 1);

    // second start is idempotent
    let again = manager.start("srv-1", "operator").unwrap();
    assert!(again.idempotent);

    let tools = manager.list_tools("srv-1").unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].tool.tool_name, "lookup");
    assert_eq!(tools[0].tool.discovery_status, DiscoveryStatus::Discovered);
    // no exposure rule yet => blocked
    assert_eq!(tools[0].effective_availability, "blocked");

    let stop = manager.stop("srv-1").unwrap();
    assert_eq!(stop.action, LifecycleAction::Stop);
    assert_eq!(stop.server.state.status, LifecycleStatus::Stopped);

    // idempotent stop
    let stop = manager.stop("srv-1").unwrap();
    assert!(stop.idempotent);
}

#[test]
fn manager_start_fails_for_stdio_without_sandbox_manager() {
    let manager = kura_mcp::Manager::new(test_cfg("~/.kura-test"), None, None, None, None, None);
    manager
        .create_server(CreateServerInput {
            server_id: "srv-1".to_string(),
            enabled: true,
            sandbox_profile_id: "subprocess_default".to_string(),
            declaration_id: "d".to_string(),
            transport_kind: TransportKind::Stdio,
            command: "npx".to_string(),
            args: vec!["-y".to_string(), "server".to_string()],
            ..CreateServerInput::default()
        })
        .unwrap();
    let err = manager.start("srv-1", "operator").unwrap_err();
    assert_eq!(err, McpError::SandboxManagerMissing);
}

#[test]
fn manager_start_fails_when_transport_open_fails() {
    // The default manager transport mux installs the concrete transports; a
    // streamable-http server pointing at a dead local port fails at transport open.
    let manager = kura_mcp::Manager::new(test_cfg("~/.kura-test"), None, None, None, None, None);
    manager
        .create_server(CreateServerInput {
            server_id: "srv-1".to_string(),
            display_name: "Test Server".to_string(),
            enabled: true,
            sandbox_profile_id: "subprocess_default".to_string(),
            declaration_id: "d".to_string(),
            transport_kind: TransportKind::StreamableHTTP,
            endpoint: "http://127.0.0.1:1/mcp".to_string(),
            auto_restart: false,
            ..CreateServerInput::default()
        })
        .unwrap();
    let response = manager.start("srv-1", "operator").unwrap();
    assert_eq!(response.failure_class, "transport_runtime_failure");
    assert_eq!(response.server.state.status, LifecycleStatus::Failed);
}

#[test]
fn manager_stdio_start_discovers_tools_and_calls_tool() {
    let starter = Arc::new(FakeAttachedExecutionStarter {
        next_id: AtomicU64::new(1),
        children: Mutex::new(HashMap::new()),
    });
    let manager = kura_mcp::Manager::new(
        test_cfg("~/.kura-test"),
        None,
        None,
        Some(starter),
        None,
        None,
    );
    manager
        .create_server(CreateServerInput {
            server_id: "srv-stdio".to_string(),
            display_name: "Stdio Server".to_string(),
            enabled: true,
            sandbox_profile_id: kura_sandbox::PROFILE_ID_SUBPROCESS_DEFAULT.to_string(),
            declaration_id: "d".to_string(),
            transport_kind: TransportKind::Stdio,
            command: common::fake_mcp_server_bin().to_string(),
            args: vec![],
            ..CreateServerInput::default()
        })
        .unwrap();

    // start spawns the fake server through the sandbox starter and discovers tools
    // through the stdio transport (initialize + tools/list over the framing).
    let response = manager.start("srv-stdio", "operator").unwrap();
    assert_eq!(response.server.state.status, LifecycleStatus::Healthy);
    assert_eq!(response.server.tool_count, 1);

    let tools = manager.list_tools("srv-stdio").unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].tool.tool_name, "echo");
    assert_eq!(tools[0].tool.discovery_status, DiscoveryStatus::Discovered);

    // allow the tool and invoke it through the live stdio session.
    manager
        .update_tool_exposure(
            "srv-stdio",
            "echo",
            &UpdateExposureInput {
                runtime_surface: "chat".to_string(),
                exposure_mode: ExposureMode::Allow,
                active: true,
                ..UpdateExposureInput::default()
            },
        )
        .unwrap();
    let allowed = manager
        .authorize_tool(
            "srv-stdio",
            "echo",
            &AuthorizeToolInput {
                runtime_surface: "chat".to_string(),
                ..AuthorizeToolInput::default()
            },
        )
        .unwrap();
    assert_eq!(allowed.status, ToolAuthorizationStatus::Allowed);

    let result = manager
        .call_tool(
            "srv-stdio",
            "echo",
            serde_json::json!({ "message": "hello via manager" }),
            &allowed,
        )
        .unwrap();
    assert_eq!(result.failure_class, "");
    let output = result.output.as_ref().unwrap();
    assert_eq!(output["content"][0]["text"], "hello via manager");

    // stop kills the child (fake starter), the session read loop ends, and the
    // watcher reconciles the server to Stopped.
    let stop = manager.stop("srv-stdio").unwrap();
    assert_eq!(stop.action, LifecycleAction::Stop);
    let final_status = wait_for_status(
        &manager,
        "srv-stdio",
        LifecycleStatus::Stopped,
        Duration::from_secs(5),
    );
    assert_eq!(final_status, LifecycleStatus::Stopped);
}

/// Polls the server state until it reaches `expected` or the timeout elapses.
fn wait_for_status(
    manager: &kura_mcp::Manager,
    server_id: &str,
    expected: LifecycleStatus,
    timeout: Duration,
) -> LifecycleStatus {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let current = manager
            .get_server_resource(server_id)
            .map(|resource| resource.state.status)
            .unwrap_or(LifecycleStatus::Disabled);
        if current == expected || std::time::Instant::now() >= deadline {
            return current;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

// ---------------------------------------------------------------------------
// Exposure + authorization
// ---------------------------------------------------------------------------

#[test]
fn manager_update_exposure_and_authorize_tool() {
    let session = FakeSession::new("session-1", vec![fake_tool("lookup")]);
    let policy = kura_policy::Engine::new();
    let manager = kura_mcp::Manager::new(
        test_cfg("~/.kura-test"),
        None,
        None,
        None,
        Some(policy),
        Some(Arc::new(FakeTransport { session })),
    );
    manager.create_server(streamable_server_input("srv-1")).unwrap();
    manager.start("srv-1", "operator").unwrap();

    // no rule => blocked
    let blocked = manager
        .authorize_tool(
            "srv-1",
            "lookup",
            &AuthorizeToolInput {
                runtime_surface: "chat".to_string(),
                ..AuthorizeToolInput::default()
            },
        )
        .unwrap();
    assert_eq!(blocked.status, ToolAuthorizationStatus::Blocked);

    // allow rule => allowed
    let updated = manager
        .update_tool_exposure(
            "srv-1",
            "lookup",
            &UpdateExposureInput {
                runtime_surface: "chat".to_string(),
                exposure_mode: ExposureMode::Allow,
                active: true,
                ..UpdateExposureInput::default()
            },
        )
        .unwrap();
    assert_eq!(updated.effective_availability, "available");

    let allowed = manager
        .authorize_tool(
            "srv-1",
            "lookup",
            &AuthorizeToolInput {
                runtime_surface: "chat".to_string(),
                ..AuthorizeToolInput::default()
            },
        )
        .unwrap();
    assert_eq!(allowed.status, ToolAuthorizationStatus::Allowed);
    assert_eq!(allowed.session_id, "session-1");

    // invoke the tool
    let result = manager
        .call_tool("srv-1", "lookup", serde_json::json!({ "q": 1 }), &allowed)
        .unwrap();
    assert_eq!(result.failure_class, "");
    assert_eq!(result.session_id, "session-1");
    assert!(result.output.is_some());

    // un-authorized invocation is blocked
    let unauthed = ToolAuthorizationResponse {
        status: ToolAuthorizationStatus::Blocked,
        message: "denied".to_string(),
        ..ToolAuthorizationResponse::default()
    };
    let blocked = manager
        .call_tool("srv-1", "lookup", serde_json::json!({}), &unauthed)
        .unwrap();
    assert_eq!(blocked.failure_class, "blocked");

    // approval-required rule => pending via policy engine
    let _ = manager
        .update_tool_exposure(
            "srv-1",
            "lookup",
            &UpdateExposureInput {
                runtime_surface: "cli".to_string(),
                exposure_mode: ExposureMode::ApprovalRequired,
                active: true,
                ..UpdateExposureInput::default()
            },
        )
        .unwrap();
    let pending = manager
        .authorize_tool(
            "srv-1",
            "lookup",
            &AuthorizeToolInput {
                runtime_surface: "cli".to_string(),
                ..AuthorizeToolInput::default()
            },
        )
        .unwrap();
    assert_eq!(pending.status, ToolAuthorizationStatus::Pending);
    let approval = pending.approval.as_ref().unwrap();
    assert_eq!(approval.action, "tool_call.execute");
    assert!(manager.get_server("srv-1").is_some());

    manager.stop("srv-1").unwrap();
}

// ---------------------------------------------------------------------------
// Catalog install + lifecycle
// ---------------------------------------------------------------------------

#[test]
fn bundled_catalog_entries_are_sorted_and_context7_installs() {
    let manager = kura_mcp::Manager::new(test_cfg("~/.kura-test"), None, None, None, None, None);
    let entries = manager.list_catalog();
    assert_eq!(entries.len(), 5);
    let ids: Vec<&str> = entries.iter().map(|entry| entry.id.as_str()).collect();
    assert_eq!(ids, vec!["context7", "filesystem", "github", "postgres", "slack"]);
    assert_eq!(entries[0].transport_kind, TransportKind::StreamableHTTP);

    let result = manager
        .install_catalog_entry("context7", &CatalogInstallInput::default(), InstallMethod::Api)
        .unwrap();
    assert_eq!(result.status, "installed");
    assert_eq!(result.server.as_ref().unwrap().server.server_id, "context7");

    let server = manager.get_server("context7").unwrap();
    assert_eq!(server.origin_kind, OriginKind::Catalog);
    assert_eq!(server.catalog_entry_id, "context7");
    let management = server.catalog_management.as_ref().unwrap();
    assert_eq!(management.last_action, Some(CatalogAction::Install));
    assert!(!management.installed_revision.is_empty());

    // revalidate: healthy classification (streamable-http endpoint configured, no secrets)
    let revalidated = manager.revalidate_catalog_server("context7").unwrap();
    assert_eq!(revalidated.status, AvailabilityStatus::Ready);
    assert_eq!(revalidated.classification, RevalidationClassification::Healthy);

    // uninstall removes the server
    let uninstalled = manager.uninstall_catalog_server("context7").unwrap();
    assert_eq!(uninstalled.status, CatalogActionStatus::Completed);
    assert!(uninstalled.removed);
    assert!(manager.get_server("context7").is_none());

    // lifecycle action on a manual server is blocked
    manager.create_server(streamable_server_input("manual-1")).unwrap();
    let blocked = manager.refresh_catalog_server("manual-1").unwrap();
    assert_eq!(blocked.status, CatalogActionStatus::Blocked);
    assert_eq!(blocked.failure_class, "not_catalog_managed");
}

#[test]
fn catalog_install_blocks_manual_server_id_collision() {
    let manager = kura_mcp::Manager::new(test_cfg("~/.kura-test"), None, None, None, None, None);
    manager.create_server(streamable_server_input("context7")).unwrap();
    let result = manager
        .install_catalog_entry("context7", &CatalogInstallInput::default(), InstallMethod::Api)
        .unwrap();
    assert_eq!(result.status, "blocked");
    assert!(result.availability_reason.contains("already owned by a manual MCP server"));
}

#[test]
fn requires_offline_verified_local_command_matches_bundled_stdio_default() {
    let spec = CreateServerInput {
        transport_kind: TransportKind::Stdio,
        command: "npx".to_string(),
        args: vec!["-y".to_string(), "@modelcontextprotocol/server-filesystem".to_string()],
        declaration: Some(Declaration {
            network_mode: kura_sandbox::NetworkMode::Deny,
            ..Declaration::default()
        }),
        ..CreateServerInput::default()
    };
    assert!(requires_offline_verified_local_command(&spec));
    let spec = CreateServerInput {
        transport_kind: TransportKind::Stdio,
        command: "npx".to_string(),
        args: vec!["-y".to_string(), "some-other-package".to_string()],
        declaration: Some(Declaration {
            network_mode: kura_sandbox::NetworkMode::Deny,
            ..Declaration::default()
        }),
        ..CreateServerInput::default()
    };
    assert!(!requires_offline_verified_local_command(&spec));
}

#[test]
fn fingerprint_create_server_spec_is_stable() {
    let spec = CreateServerInput {
        command: "npx".to_string(),
        args: vec!["-y".to_string(), "@modelcontextprotocol/server-filesystem".to_string()],
        declaration: Some(Declaration {
            network_mode: kura_sandbox::NetworkMode::Deny,
            ..Declaration::default()
        }),
        ..CreateServerInput::default()
    };
    let first = fingerprint_create_server_spec(&spec);
    let second = fingerprint_create_server_spec(&spec);
    assert!(first.starts_with("sha256:"));
    assert_eq!(first, second);
    let changed = fingerprint_create_server_spec(&CreateServerInput {
        endpoint: "https://example.test/mcp".to_string(),
        ..spec
    });
    assert_ne!(first, changed);
}

// ---------------------------------------------------------------------------
// Store-backed restore
// ---------------------------------------------------------------------------

fn temp_data_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("kura-mcp-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn manager_persists_and_restores_servers_from_store() {
    let dir = temp_data_dir("restore");
    let store = Arc::new(Mutex::new(kura_store::SQLiteStore::new(dir.to_str().unwrap()).unwrap()));
    let manager = kura_mcp::Manager::new(
        test_cfg(dir.to_str().unwrap()),
        Some(Arc::clone(&store)),
        None,
        None,
        None,
        None,
    );
    let (_, created) = manager
        .create_server(CreateServerInput {
            server_id: "srv-1".to_string(),
            display_name: "Persisted".to_string(),
            enabled: false,
            sandbox_profile_id: "subprocess_default".to_string(),
            declaration_id: "d".to_string(),
            transport_kind: TransportKind::StreamableHTTP,
            endpoint: "https://example.test/mcp".to_string(),
            ..CreateServerInput::default()
        })
        .unwrap();
    assert!(created);

    // a fresh manager over the same store restores the registry
    let manager2 = kura_mcp::Manager::new(
        test_cfg(dir.to_str().unwrap()),
        Some(Arc::clone(&store)),
        None,
        None,
        None,
        None,
    );
    manager2.restore().unwrap();
    let restored = manager2.get_server("srv-1").unwrap();
    assert_eq!(restored.display_name, "Persisted");
    assert!(!restored.enabled);
    assert_eq!(
        manager2.get_server_resource("srv-1").unwrap().state.status,
        LifecycleStatus::Disabled
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn manager_restore_marks_tools_stale_and_reloads_exposure() {
    let dir = temp_data_dir("restore-tools");
    let store = Arc::new(Mutex::new(kura_store::SQLiteStore::new(dir.to_str().unwrap()).unwrap()));

    let session = FakeSession::new("session-1", vec![fake_tool("lookup")]);
    let manager = kura_mcp::Manager::new(
        test_cfg(dir.to_str().unwrap()),
        Some(Arc::clone(&store)),
        None,
        None,
        None,
        Some(Arc::new(FakeTransport { session })),
    );
    manager.create_server(streamable_server_input("srv-1")).unwrap();
    let response = manager.start("srv-1", "operator").unwrap();
    assert_eq!(response.server.state.status, LifecycleStatus::Healthy);
    let _ = manager
        .update_tool_exposure(
            "srv-1",
            "lookup",
            &UpdateExposureInput {
                runtime_surface: "chat".to_string(),
                exposure_mode: ExposureMode::Allow,
                active: true,
                ..UpdateExposureInput::default()
            },
        )
        .unwrap();
    manager.stop("srv-1").unwrap();

    // fresh manager without a transport: restore must reload tools and mark them stale
    // (state is not Healthy after daemon restart) and reload the exposure rule.
    let manager2 = kura_mcp::Manager::new(
        test_cfg(dir.to_str().unwrap()),
        Some(Arc::clone(&store)),
        None,
        None,
        None,
        None,
    );
    manager2.restore().unwrap();
    let tools = manager2.list_tools("srv-1").unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].tool.tool_name, "lookup");
    assert_eq!(tools[0].tool.discovery_status, DiscoveryStatus::Stale);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

#[test]
fn read_framed_message_decodes_content_length() {
    let payload = b"{\"hello\":\"world\"}";
    let framed = format!("Content-Length: {}\r\n\r\n{}", payload.len(), String::from_utf8_lossy(payload));
    let mut cursor = std::io::Cursor::new(framed.into_bytes());
    let decoded = read_framed_message(&mut cursor).unwrap();
    assert_eq!(decoded, payload);

    // lowercase header is accepted too
    let framed = format!("content-length: {}\r\n\r\n{}", payload.len(), String::from_utf8_lossy(payload));
    let mut cursor = std::io::Cursor::new(framed.into_bytes());
    let decoded = read_framed_message(&mut cursor).unwrap();
    assert_eq!(decoded, payload);

    // missing header is an error
    let mut cursor = std::io::Cursor::new(b"{}".to_vec());
    assert!(read_framed_message(&mut cursor).is_err());
}

#[test]
fn redact_string_redacts_common_derived_secret_forms() {
    let secrets = HashMap::from([("K".to_string(), "secret 123".to_string())]);
    // raw secret and its form-encoded candidate ("secret+123") are both redacted
    let redacted = redact_string("token=secret 123 and secret+123", &secrets);
    assert_eq!(redacted, "token=[REDACTED] and [REDACTED]");
    // unrelated text is untouched
    let redacted = redact_string("nothing to see", &secrets);
    assert_eq!(redacted, "nothing to see");
}

#[test]
fn websocket_endpoint_validation() {
    assert!(validate_websocket_endpoint("ws://example.com/mcp").is_ok());
    assert!(validate_websocket_endpoint("wss://example.com:8443/mcp").is_ok());
    assert!(validate_websocket_endpoint("").is_err());
    assert!(validate_websocket_endpoint("http://example.com").is_err());
    assert!(validate_websocket_endpoint("ws://user:pass@example.com").is_err());
    assert!(validate_websocket_endpoint("ws://example.com/mcp?token=abc").is_err());

    assert_eq!(
        sanitize_websocket_endpoint_for_projection("wss://user:pass@example.com/mcp?token=abc#frag"),
        "wss://example.com/mcp"
    );
}

#[test]
fn transport_capabilities_and_terminal_status() {
    let manager = kura_mcp::Manager::new(test_cfg("~/.kura-test"), None, None, None, None, None);
    let capabilities = manager.list_transport_capabilities();
    assert_eq!(capabilities.len(), 3);
    assert_eq!(capabilities[0].transport_kind, TransportKind::Stdio);
    assert!(capabilities[2].daemon_managed_reconnect);

    assert!(is_terminal_status(LifecycleStatus::Failed));
    assert!(is_terminal_status(LifecycleStatus::Disabled));
    assert!(is_terminal_status(LifecycleStatus::Stopped));
    assert!(!is_terminal_status(LifecycleStatus::Healthy));
    assert!(!is_terminal_status(LifecycleStatus::Starting));
}

#[test]
fn restart_backoff_delay_doubles_and_caps() {
    assert_eq!(restart_backoff_delay(0), Duration::from_secs(5));
    assert_eq!(restart_backoff_delay(1), Duration::from_secs(5));
    assert_eq!(restart_backoff_delay(2), Duration::from_secs(10));
    assert_eq!(restart_backoff_delay(3), Duration::from_secs(20));
    assert_eq!(restart_backoff_delay(4), Duration::from_secs(40));
    assert_eq!(restart_backoff_delay(100), Duration::from_secs(300));
}

#[test]
fn live_validation_matrix_marks_mcp_tool_calls_unsupported() {
    let rows = live_validation_matrix_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].tool_class.as_str(), "mcp.tool_call");
    assert_eq!(rows[0].safety_class.as_str(), "unsupported");
}

#[test]
fn secret_resolution_falls_back_to_mcp_secrets_file() {
    let dir = temp_data_dir("secrets");
    std::fs::write(
        dir.join("mcp-secrets.json"),
        serde_json::json!({ "TOKEN": "  topsecret  " }).to_string(),
    )
    .unwrap();
    let manager = kura_mcp::Manager::new(test_cfg(dir.to_str().unwrap()), None, None, None, None, None);
    let (resolved, _) = manager
        .create_server(CreateServerInput {
            server_id: "srv-1".to_string(),
            enabled: true,
            sandbox_profile_id: "subprocess_default".to_string(),
            declaration_id: "d".to_string(),
            transport_kind: TransportKind::Websocket,
            endpoint: "wss://example.test/mcp".to_string(),
            secret_refs: vec!["TOKEN".to_string()],
            websocket_config: Some(WebsocketConfig {
                auth: Some(WebsocketAuthConfig {
                    mode: WebsocketAuthMode::BearerHeader,
                    secret_ref: "TOKEN".to_string(),
                    ..WebsocketAuthConfig::default()
                }),
                ..WebsocketConfig::default()
            }),
            ..CreateServerInput::default()
        })
        .unwrap();
    // availability is Blocked when the websocket auth secret cannot be resolved to a
    // value; here the file has TOKEN=topsecret so it resolves.
    assert_eq!(resolved.availability_status, AvailabilityStatus::Ready);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Tools the agent loop can call
//
// The loop in `kura-core` calls anything implementing its `Tool` trait; MCP
// already knows how to reach a server, what it publishes, and whether a surface
// may invoke it. What matters here is that the adapter between them does not
// route around the last of those: an exposure rule is the boundary a
// state-changing tool depends on, and a model asking nicely must not clear it.
// ---------------------------------------------------------------------------

fn started_server_with(tool: Tool) -> Arc<kura_mcp::Manager> {
    let session = FakeSession::new("session-1", vec![tool]);
    // With a policy engine, because without one an approval-required tool
    // fails to authorize at all rather than coming back pending -- safe, but
    // not the path this is about.
    let manager = Arc::new(kura_mcp::Manager::new(
        test_cfg("~/.kura-test"),
        None,
        None,
        None,
        Some(kura_policy::Engine::new()),
        Some(Arc::new(FakeTransport { session })),
    ));
    manager.create_server(streamable_server_input("srv-1")).unwrap();
    manager.start("srv-1", "operator").unwrap();
    manager
}

fn allow(manager: &kura_mcp::Manager, tool_name: &str, mode: ExposureMode) {
    manager
        .update_tool_exposure(
            "srv-1",
            tool_name,
            &UpdateExposureInput {
                runtime_surface: "chat".to_string(),
                exposure_mode: mode,
                active: true,
                reason: String::new(),
            },
        )
        .unwrap();
}

fn invocation(name: &str, arguments: &str) -> kura_core::ToolInvocation {
    kura_core::ToolInvocation {
        call_id: "call_1".to_string(),
        name: name.to_string(),
        arguments: arguments.to_string(),
    }
}

#[test]
fn a_published_tool_is_offered_with_the_schema_the_server_declared() {
    // Discovery kept only a fingerprint of the schema. A hash answers "did
    // this change"; it cannot answer "what does this take", which is the only
    // thing a model needs in order to call the tool at all.
    let mut tool = fake_tool("lookup");
    tool.description = "look something up".to_string();
    tool.input_schema = serde_json::json!({
        "type": "object",
        "properties": {"q": {"type": "string"}},
    });
    let manager = started_server_with(tool);
    allow(&manager, "lookup", ExposureMode::Allow);

    let tools = kura_mcp::tools_for_surface(&manager, "chat");
    assert_eq!(tools.len(), 1);
    let spec = tools[0].spec();
    assert_eq!(spec.name, "srv-1__lookup");
    assert_eq!(spec.description, "look something up");
    assert_eq!(spec.parameters["properties"]["q"]["type"], "string");
}

#[test]
fn a_tool_that_declared_no_schema_is_offered_as_taking_an_object() {
    // `null` would leave a provider to guess the shape of the arguments.
    let manager = started_server_with(fake_tool("ping"));
    allow(&manager, "ping", ExposureMode::Allow);

    let spec = kura_mcp::tools_for_surface(&manager, "chat")[0].spec();
    assert_eq!(spec.parameters["type"], "object");
}

#[tokio::test]
async fn an_allowed_tool_runs() {
    let manager = started_server_with(fake_tool("lookup"));
    allow(&manager, "lookup", ExposureMode::Allow);
    let tools = kura_mcp::tools_for_surface(&manager, "chat");

    let output = tools[0].call(&invocation("srv-1__lookup", "{}")).await.unwrap();

    assert!(output.success, "{}", output.content);
}

#[tokio::test]
async fn a_tool_needing_approval_is_refused_and_says_so() {
    // The boundary a state-changing tool depends on. The model may ask; a
    // person decides. Reported in words it can relay rather than as a fault.
    let manager = started_server_with(fake_tool("advance"));
    allow(&manager, "advance", ExposureMode::ApprovalRequired);
    let tools = kura_mcp::tools_for_surface(&manager, "chat");

    let output = tools[0].call(&invocation("srv-1__advance", "{}")).await.unwrap();

    assert!(!output.success);
    assert!(output.content.contains("approve"), "{}", output.content);
}

#[tokio::test]
async fn a_tool_with_no_exposure_rule_is_refused() {
    // Blocked is the default, and a tool nobody exposed must stay that way
    // however the model asks for it.
    let manager = started_server_with(fake_tool("lookup"));
    let tools = kura_mcp::tools_for_surface(&manager, "chat");

    let output = tools[0].call(&invocation("srv-1__lookup", "{}")).await.unwrap();

    assert!(!output.success, "an unexposed tool ran");
}

#[tokio::test]
async fn a_rule_for_another_surface_does_not_allow_this_one() {
    // Exposure is per surface: allowed in a scheduled run is not allowed in
    // chat, and the adapter asks with the surface it was built for.
    let manager = started_server_with(fake_tool("lookup"));
    manager
        .update_tool_exposure(
            "srv-1",
            "lookup",
            &UpdateExposureInput {
                runtime_surface: "scheduler".to_string(),
                exposure_mode: ExposureMode::Allow,
                active: true,
                reason: String::new(),
            },
        )
        .unwrap();

    let tools = kura_mcp::tools_for_surface(&manager, "chat");
    let output = tools[0].call(&invocation("srv-1__lookup", "{}")).await.unwrap();

    assert!(!output.success, "a rule for another surface allowed this one");
}

#[tokio::test]
async fn malformed_arguments_are_reported_to_the_model_not_raised() {
    // The model wrote them, so it is the one that can fix them. Raising would
    // end the turn over something the next round could correct.
    let manager = started_server_with(fake_tool("lookup"));
    allow(&manager, "lookup", ExposureMode::Allow);
    let tools = kura_mcp::tools_for_surface(&manager, "chat");

    let output = tools[0].call(&invocation("srv-1__lookup", "not json")).await.unwrap();

    assert!(!output.success);
    assert!(output.content.contains("valid JSON"), "{}", output.content);
}

#[test]
fn a_server_is_visible_to_the_tenant_that_created_it() {
    // It was not. `upsert_server` hardcoded an empty tenant id, with the Go
    // call it stood in for left in a comment, while every read filters by the
    // requesting tenant. So a server created through the API was created,
    // returned, and gone: the next request on the same API could not find it,
    // and a listing came back empty with nothing saying why.
    let manager = kura_mcp::Manager::new(test_cfg("~/.kura-test"), None, None, None, None, None);
    manager
        .create_server(CreateServerInput {
            tenant_id: "tenant-1".to_string(),
            ..streamable_server_input("srv-1")
        })
        .unwrap();

    assert!(manager.get_server_for_tenant("srv-1", "tenant-1").is_some());
    assert_eq!(manager.list_servers_for_tenant("tenant-1").len(), 1);
}

#[test]
fn a_server_is_not_visible_to_another_tenant() {
    // The filter still has a job. Recording the creator must not turn into
    // showing it to everyone.
    let manager = kura_mcp::Manager::new(test_cfg("~/.kura-test"), None, None, None, None, None);
    manager
        .create_server(CreateServerInput {
            tenant_id: "tenant-1".to_string(),
            ..streamable_server_input("srv-1")
        })
        .unwrap();

    assert!(manager.get_server_for_tenant("srv-1", "tenant-2").is_none());
    assert_eq!(manager.list_servers_for_tenant("tenant-2").len(), 0);
}

#[test]
fn a_server_created_without_a_tenant_stays_visible_to_everyone() {
    // Single-tenant deployments send no tenant at all, and their servers must
    // not become invisible now that the field is recorded.
    let manager = kura_mcp::Manager::new(test_cfg("~/.kura-test"), None, None, None, None, None);
    manager.create_server(streamable_server_input("srv-1")).unwrap();

    assert!(manager.get_server_for_tenant("srv-1", "tenant-1").is_none());
    assert!(manager.get_server("srv-1").is_some());
    assert_eq!(manager.list_servers().len(), 1);
}
