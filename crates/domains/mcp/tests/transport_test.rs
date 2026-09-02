//! Transport-level integration tests for the concrete MCP transports:
//!
//! - stdio: a real `fake-mcp-server` subprocess speaking the Content-Length framing.
//! - streamable-http: a local in-process HTTP server answering the JSON-RPC shape.
//! - websocket: a local in-process RFC 6455 server answering text-frame JSON-RPC.
//! - mux dispatch on the server's transport kind.

mod common;

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use kura_mcp::types::{Server, TransportKind};
use kura_mcp::{
    McpError, SessionPipes, StdioTransport, StreamableHTTPTransport, Transport,
    TransportMux, WebsocketTransport,
};

fn stdio_server(id: &str) -> Server {
    Server {
        server_id: id.to_string(),
        transport_kind: TransportKind::Stdio,
        ..Server::default()
    }
}

#[test]
fn stdio_transport_speaks_framing_end_to_end() {
    let mut child = std::process::Command::new(common::fake_mcp_server_bin())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn fake mcp server");
    let pipes = SessionPipes {
        stdin: Some(Box::new(child.stdin.take().unwrap())),
        stdout: Some(Box::new(child.stdout.take().unwrap())),
        stderr: Some(Box::new(child.stderr.take().unwrap())),
    };

    let session = StdioTransport::new()
        .open(&stdio_server("fake-stdio"), pipes, Duration::from_secs(5))
        .expect("open stdio session");

    // initialize + tools/list handshake already succeeded inside open
    assert!(!session.id().is_empty());
    let tools = session
        .list_tools(Duration::from_secs(5))
        .expect("tools/list");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].tool_name, "echo");
    assert_eq!(tools[0].server_id, "fake-stdio");
    assert!(!tools[0].schema_fingerprint.is_empty());

    // tools/call round-trips arguments through the subprocess
    let output = session
        .call_tool("echo", serde_json::json!({ "message": "hello world" }))
        .expect("tools/call");
    assert_eq!(output["content"][0]["text"], "hello world");
    assert_eq!(output["isError"], false);

    // close drops stdin -> the child sees EOF and exits; done resolves cleanly
    session.close().expect("close");
    let status = child.wait().expect("wait child");
    assert!(status.success());
    assert_eq!(session.wait_done().expect("wait_done"), ());
}

#[test]
fn stdio_transport_rejects_missing_pipes() {
    let err = open_error(
        &StdioTransport::new(),
        &stdio_server("no-pipes"),
        SessionPipes::default(),
        Duration::from_secs(1),
    );
    assert_eq!(err, McpError::TransportUnavailable);
}

#[test]
fn streamable_http_transport_speaks_json_rpc() {
    let addr = common::spawn_mcp_http_server();
    let server = Server {
        server_id: "fake-http".to_string(),
        transport_kind: TransportKind::StreamableHTTP,
        endpoint: format!("http://{addr}/mcp"),
        ..Server::default()
    };
    let session = StreamableHTTPTransport::new()
        .open(&server, SessionPipes::default(), Duration::from_secs(5))
        .expect("open streamable-http session");

    let tools = session
        .list_tools(Duration::from_secs(5))
        .expect("tools/list");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].tool_name, "echo");

    let output = session
        .call_tool("echo", serde_json::json!({ "message": "over http" }))
        .expect("tools/call");
    assert_eq!(output["content"][0]["text"], "over http");

    session.close().expect("close");
    assert_eq!(session.wait_done().expect("wait_done"), ());
}

#[test]
fn streamable_http_transport_rejects_missing_endpoint() {
    let err = open_error(
        &StreamableHTTPTransport::new(),
        &Server {
            server_id: "no-endpoint".to_string(),
            transport_kind: TransportKind::StreamableHTTP,
            ..Server::default()
        },
        SessionPipes::default(),
        Duration::from_secs(1),
    );
    assert_eq!(err, McpError::TransportUnavailable);
}

#[test]
fn websocket_transport_speaks_json_rpc() {
    let addr = common::spawn_mcp_ws_server();
    let server = Server {
        server_id: "fake-ws".to_string(),
        transport_kind: TransportKind::Websocket,
        endpoint: format!("ws://{addr}/mcp"),
        ..Server::default()
    };
    let session = WebsocketTransport::new()
        .open(&server, SessionPipes::default(), Duration::from_secs(5))
        .expect("open websocket session");

    let tools = session
        .list_tools(Duration::from_secs(5))
        .expect("tools/list");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].tool_name, "echo");

    let output = session
        .call_tool("echo", serde_json::json!({ "message": "over ws" }))
        .expect("tools/call");
    assert_eq!(output["content"][0]["text"], "over ws");

    session.close().expect("close");
    assert_eq!(session.wait_done().expect("wait_done"), ());
}

#[test]
fn websocket_transport_rejects_missing_endpoint() {
    let err = open_error(
        &WebsocketTransport::new(),
        &Server {
            server_id: "no-endpoint".to_string(),
            transport_kind: TransportKind::Websocket,
            ..Server::default()
        },
        SessionPipes::default(),
        Duration::from_secs(1),
    );
    assert_eq!(err, McpError::TransportUnavailable);
}

#[test]
fn transport_mux_dispatches_on_server_kind() {
    let mux = Arc::new(TransportMux::default()) as Arc<dyn Transport>;

    // stdio without pipes: the stdio transport rejects before any subprocess work
    let err = open_error(
        mux.as_ref(),
        &stdio_server("srv"),
        SessionPipes::default(),
        Duration::from_secs(1),
    );
    assert_eq!(err, McpError::TransportUnavailable);

    // streamable-http to a dead local port: the concrete HTTP transport attempts the
    // call and fails (connection refused), proving the mux reached the HTTP transport.
    let server = Server {
        server_id: "srv-http".to_string(),
        transport_kind: TransportKind::StreamableHTTP,
        endpoint: "http://127.0.0.1:1/mcp".to_string(),
        ..Server::default()
    };
    assert!(mux
        .open(&server, SessionPipes::default(), Duration::from_secs(2))
        .is_err());

    // websocket without an endpoint: rejected before any connect attempt
    let server = Server {
        server_id: "srv-ws".to_string(),
        transport_kind: TransportKind::Websocket,
        ..Server::default()
    };
    let err = open_error(
        mux.as_ref(),
        &server,
        SessionPipes::default(),
        Duration::from_secs(1),
    );
    assert_eq!(err, McpError::TransportUnavailable);
}


/// Opens a transport expecting failure (the session type is not Debug, so
/// `unwrap_err` is unavailable).
fn open_error(
    transport: &dyn Transport,
    server: &Server,
    pipes: SessionPipes,
    timeout: Duration,
) -> McpError {
    match transport.open(server, pipes, timeout) {
        Ok(_) => panic!("expected transport open to fail"),
        Err(err) => err,
    }
}

#[test]
fn discovery_keeps_the_schema_a_server_published() {
    // It kept a fingerprint and dropped the schema. A hash answers "did this
    // change"; it cannot answer "what does this take", which is the only thing
    // a model needs in order to call the tool -- so a tool could be
    // discovered, listed, authorized, and still impossible to offer.
    let raw = serde_json::json!({
        "tools": [{
            "name": "lookup",
            "description": "look something up",
            "inputSchema": {
                "type": "object",
                "properties": {"q": {"type": "string"}},
                "required": ["q"],
            },
        }],
    });

    let tools = kura_mcp::transport::decode_tools_list(&raw, "srv-1", chrono::Utc::now()).unwrap();

    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].input_schema["properties"]["q"]["type"], "string");
    assert_eq!(tools[0].input_schema["required"][0], "q");
    // The fingerprint is still there; the two answer different questions.
    assert!(!tools[0].schema_fingerprint.is_empty());
}

#[test]
fn a_server_that_publishes_no_schema_leaves_it_null() {
    // Absent is not the same as empty, and the adapter substitutes a shape
    // only when it knows there was none.
    let raw = serde_json::json!({"tools": [{"name": "ping"}]});

    let tools = kura_mcp::transport::decode_tools_list(&raw, "srv-1", chrono::Utc::now()).unwrap();

    assert!(tools[0].input_schema.is_null());
}
