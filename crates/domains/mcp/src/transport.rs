//! Port of the MCP client transport layer (transport.go / streamable_http.go /
//! websocket.go).
//!
//! The transport/session traits, the JSON-RPC wire types, the stdio
//! content-length framing helper, the shared tools/list decoding, and the transport
//! mux are ported faithfully, and the three concrete transports are implemented:
//!
//! - `StdioTransport`: consumes the process pipes handed in `SessionPipes` (the
//!   subprocess is spawned by the sandbox `AttachedExecutionStarter` in the manager,
//!   exactly like Go) and speaks JSON-RPC over LSP-style `Content-Length` framing.
//! - `StreamableHTTPTransport`: one JSON-RPC request per HTTP POST to the server's
//!   endpoint (ureq), mirroring Go's `streamable_http.go`.
//! - `WebsocketTransport`: a tokio-tungstenite client speaking JSON-RPC over text
//!   frames, driven by a small multi-thread Tokio runtime owned by the session (the
//!   sync `Session` API does `Runtime::block_on` for writes and a spawned read-loop
//!   task correlates responses by id).
//!
//! `TransportMux` defaults to all three concrete transports, matching Go's
//! `NewTransportMux` (nil transports substitute the defaults).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Digest;

use crate::types::{DiscoveryStatus, Server, Tool, TransportKind};
use crate::McpError;

/// MCP protocol version used for session initialization (Go hard-codes this).
pub const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

/// Timeout applied to `tools/call` requests (the manager's `Session::call_tool` has
/// no explicit timeout; Go used the caller's context deadline). `tools/list` and the
/// session handshake use the timeout passed to `Transport::open` instead.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// Websocket handshake deadline (Go `NewWebsocketTransport` sets 15s).
pub const WEBSOCKET_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);

/// Stdio subprocess pipes handed to a stdio transport (Go SessionPipes). The pipe
/// handles are not Clone/Debug (Go io.WriteCloser/io.ReadCloser), only Default.
#[derive(Default)]
pub struct SessionPipes {
    pub stdin: Option<Box<dyn Write + Send>>,
    pub stdout: Option<Box<dyn Read + Send>>,
    pub stderr: Option<Box<dyn Read + Send>>,
}

impl SessionPipes {
    /// Whether both stdin and stdout are present (Go `stdioTransport.Open` rejects a
    /// session without them).
    #[must_use]
    pub fn has_stdio(&self) -> bool {
        self.stdin.is_some() && self.stdout.is_some()
    }
}

/// Opens an MCP session for a server (Go Transport interface).
pub trait Transport: Send + Sync {
    fn open(
        &self,
        server: &Server,
        pipes: SessionPipes,
        timeout: Duration,
    ) -> Result<Arc<dyn Session>, McpError>;
}

/// An open MCP client session (Go Session interface). `wait_done` blocks until the
/// session terminates and returns its terminal result (equivalent to the manager's use
/// of Go `<-session.Done()`; a disconnected channel is an implicit nil/clean close).
/// The trait is Sync-friendly (no channel handles are exposed).
pub trait Session: Send + Sync {
    fn id(&self) -> String;
    fn list_tools(&self, timeout: Duration) -> Result<Vec<Tool>, String>;
    fn call_tool(
        &self,
        tool_name: &str,
        input: Value,
    ) -> Result<serde_json::Map<String, Value>, String>;
    fn close(&self) -> Result<(), String>;
    fn wait_done(&self) -> Result<(), String>;
}

/// JSON-RPC request wire type (Go rpcRequest).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcRequest {
    pub jsonrpc: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// JSON-RPC response wire type (Go rpcResponse).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcResponse {
    pub jsonrpc: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<RpcError>,
}

/// JSON-RPC error body (Go rpcError).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

/// Go `stdioSession.initialize` params: protocol version, empty capabilities, and the
/// kura-daemon client info.
#[must_use]
pub fn initialize_params() -> Value {
    serde_json::json!({
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": { "name": "kura-daemon", "version": "dev" },
    })
}

/// Go `schemaFingerprint`: sha256 hex of the JSON-serialized input schema (empty for
/// an empty schema).
#[must_use]
pub fn schema_fingerprint(value: &Value) -> String {
    let empty = match value {
        Value::Null => true,
        Value::Object(map) => map.is_empty(),
        _ => false,
    };
    if empty {
        return String::new();
    }
    let Ok(payload) = serde_json::to_string(value) else {
        return String::new();
    };
    let mut hasher = sha2::Sha256::new();
    hasher.update(payload.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Go `normalizeToolArguments`: nil input becomes an empty object.
#[must_use]
pub fn normalize_tool_arguments(input: Value) -> Value {
    if input.is_null() {
        serde_json::json!({})
    } else {
        input
    }
}

/// Decodes a `tools/list` response payload into discovered tools (Go payload struct in
/// `ListTools`).
pub fn decode_tools_list(
    raw: &Value,
    server_id: &str,
    now: DateTime<Utc>,
) -> Result<Vec<Tool>, String> {
    #[derive(Deserialize)]
    struct ToolsListTool {
        #[serde(default)]
        name: String,
        #[serde(default)]
        title: String,
        #[serde(default)]
        description: String,
        #[serde(default, rename = "inputSchema")]
        input_schema: Value,
    }
    #[derive(Deserialize)]
    struct ToolsListPayload {
        #[serde(default)]
        tools: Vec<ToolsListTool>,
    }
    let payload: ToolsListPayload =
        serde_json::from_value(raw.clone()).map_err(|e| format!("decode tools/list response: {e}"))?;
    let mut tools = Vec::with_capacity(payload.tools.len());
    for item in payload.tools {
        tools.push(Tool {
            server_id: server_id.to_string(),
            tool_name: item.name.trim().to_string(),
            title: item.title.trim().to_string(),
            description: item.description.trim().to_string(),
            schema_fingerprint: schema_fingerprint(&item.input_schema),
            input_schema: item.input_schema,
            discovery_status: DiscoveryStatus::Discovered,
            last_discovered_at: Some(now),
            updated_at: now,
            ..Tool::default()
        });
    }
    Ok(tools)
}

/// Go `readFramedMessage`: reads one LSP-style `Content-Length` framed message from
/// a stdio reader.
pub fn read_framed_message(reader: &mut impl BufRead) -> Result<Vec<u8>, String> {
    let mut length: i64 = -1;
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line).map_err(|e| e.to_string())?;
        if read == 0 {
            return Err("read framed message: unexpected EOF".to_string());
        }
        let trimmed = line.trim_end_matches(['\r', '\n']).to_string();
        if trimmed.is_empty() {
            break;
        }
        if !trimmed.to_lowercase().starts_with("content-length:") {
            continue;
        }
        let value = if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
            rest.trim().to_string()
        } else if let Some(rest) = trimmed.strip_prefix("content-length:") {
            rest.trim().to_string()
        } else {
            continue;
        };
        let parsed: i64 = value
            .parse()
            .map_err(|_| format!("parse content length: {value}"))?;
        length = parsed;
    }
    if length < 0 {
        return Err("missing content length header".to_string());
    }
    let mut payload = vec![0u8; length as usize];
    reader.read_exact(&mut payload).map_err(|e| e.to_string())?;
    Ok(payload)
}

/// Go `fmt.Sprintf("%s-%d", serverID, unixNano)` session id.
fn session_id(server_id: &str) -> String {
    format!(
        "{}-{}",
        server_id.trim(),
        Utc::now().timestamp_nanos_opt().unwrap_or(0)
    )
}

// ---------------------------------------------------------------------------
// Shared request/response plumbing
// ---------------------------------------------------------------------------

/// A pending JSON-RPC request: the method name (for error messages) and the channel
/// that receives the decoded result.
type PendingEntry = (String, Sender<Result<Value, String>>);

/// Builds the "mcp <method> failed: <message>" error, matching Go's `call`.
fn rpc_error(method: &str, response: &RpcResponse) -> String {
    let message = response
        .error
        .as_ref()
        .map(|err| err.message.clone())
        .unwrap_or_default();
    format!("mcp {method} failed: {message}")
}

// ---------------------------------------------------------------------------
// Stdio transport
// ---------------------------------------------------------------------------

/// Concrete stdio transport (Go `stdioTransport`): JSON-RPC over the process pipes in
/// `SessionPipes` using LSP `Content-Length` framing. The subprocess itself is spawned
/// by the sandbox `AttachedExecutionStarter` (manager), never here.
#[derive(Debug, Default, Clone, Copy)]
pub struct StdioTransport;

impl StdioTransport {
    #[must_use]
    pub fn new() -> Self {
        StdioTransport
    }
}

impl Transport for StdioTransport {
    fn open(
        &self,
        server: &Server,
        pipes: SessionPipes,
        timeout: Duration,
    ) -> Result<Arc<dyn Session>, McpError> {
        if !pipes.has_stdio() {
            return Err(McpError::TransportUnavailable);
        }
        let session = StdioSession::spawn(server, pipes)?;
        if let Err(err) = session.initialize(timeout) {
            let _ = session.close();
            return Err(McpError::Other(err));
        }
        Ok(session)
    }
}

/// One stdio MCP session: a dedicated reader thread consumes the framed stdout
/// stream and fans responses out to in-flight calls by id; a second thread drains
/// stderr so the child never blocks on a full pipe.
struct StdioSession {
    server_id: String,
    session_id: String,
    stdin: Mutex<Option<Box<dyn Write + Send>>>,
    pending: Mutex<HashMap<String, PendingEntry>>,
    done: Mutex<Option<Sender<Result<(), String>>>>,
    done_rx: Mutex<Option<Receiver<Result<(), String>>>>,
    terminal: Mutex<Option<Result<(), String>>>,
    closed: AtomicBool,
    request_id: AtomicU64,
}

impl StdioSession {
    fn spawn(server: &Server, pipes: SessionPipes) -> Result<Arc<Self>, McpError> {
        let (done_tx, done_rx) = mpsc::channel();
        let stdout = pipes.stdout.expect("checked by has_stdio");
        let stderr = pipes.stderr;
        let session = Arc::new(StdioSession {
            server_id: server.server_id.trim().to_string(),
            session_id: session_id(&server.server_id),
            stdin: Mutex::new(pipes.stdin),
            pending: Mutex::new(HashMap::new()),
            done: Mutex::new(Some(done_tx)),
            done_rx: Mutex::new(Some(done_rx)),
            terminal: Mutex::new(None),
            closed: AtomicBool::new(false),
            request_id: AtomicU64::new(0),
        });

        let read_session = Arc::clone(&session);
        std::thread::Builder::new()
            .name("mcp-stdio-read".to_string())
            .spawn(move || read_session.read_loop(stdout))
            .map_err(|err| McpError::Other(format!("spawn mcp stdio read loop: {err}")))?;

        if let Some(stderr) = stderr {
            std::thread::Builder::new()
                .name("mcp-stdio-stderr".to_string())
                .spawn(move || drain(stderr))
                .map_err(|err| McpError::Other(format!("spawn mcp stdio stderr drain: {err}")))?;
        }
        Ok(session)
    }

    fn read_loop(self: &Arc<Self>, stdout: Box<dyn Read + Send>) {
        let mut reader = BufReader::new(stdout);
        loop {
            let payload = match read_framed_message(&mut reader) {
                Ok(payload) => payload,
                Err(err) => {
                    // EOF / closed pipe is a clean end (Go finish(nil)); anything else
                    // is a transport failure.
                    let clean = err.contains("unexpected EOF")
                        || self.closed.load(Ordering::SeqCst)
                        || err.contains("closed pipe");
                    self.finish(if clean { Ok(()) } else { Err(err) });
                    return;
                }
            };
            let response: RpcResponse = match serde_json::from_slice(&payload) {
                Ok(response) => response,
                Err(err) => {
                    self.finish(Err(format!("decode mcp transport response: {err}")));
                    return;
                }
            };
            if response.id.trim().is_empty() {
                continue;
            }
            let delivered = self.pending.lock().unwrap().remove(&response.id);
            if let Some((method, tx)) = delivered {
                let outcome = match response.error.as_ref() {
                    Some(_) => Err(rpc_error(&method, &response)),
                    None => Ok(response.result.unwrap_or(Value::Null)),
                };
                let _ = tx.send(outcome);
            }
        }
    }

    fn initialize(&self, timeout: Duration) -> Result<(), String> {
        self.call("initialize", initialize_params(), timeout)
            .map_err(|err| format!("initialize mcp session for {}: {err}", self.server_id))?;
        self.notify("notifications/initialized", serde_json::json!({}))
            .map_err(|err| format!("send initialized notification for {}: {err}", self.server_id))
    }

    fn call(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        let request_id = format!("{}", self.request_id.fetch_add(1, Ordering::SeqCst));
        let (tx, rx) = mpsc::channel();
        {
            let mut pending = self.pending.lock().unwrap();
            if self.closed.load(Ordering::SeqCst) {
                return Err(McpError::TransportClosed.to_string());
            }
            pending.insert(request_id.clone(), (method.to_string(), tx));
        }
        if let Err(err) = self.write_message(RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: request_id.clone(),
            method: method.to_string(),
            params: Some(params),
        }) {
            self.pending.lock().unwrap().remove(&request_id);
            return Err(err);
        }
        match rx.recv_timeout(timeout) {
            Ok(outcome) => outcome,
            Err(RecvTimeoutError::Timeout) => {
                self.pending.lock().unwrap().remove(&request_id);
                Err(format!("mcp {method} timed out after {}s", timeout.as_secs()))
            }
            Err(RecvTimeoutError::Disconnected) => Err(McpError::TransportClosed.to_string()),
        }
    }

    fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.write_message(RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: String::new(),
            method: method.to_string(),
            params: Some(params),
        })
    }

    fn write_message(&self, request: RpcRequest) -> Result<(), String> {
        let payload = serde_json::to_vec(&request)
            .map_err(|err| format!("marshal mcp transport payload: {err}"))?;
        let frame = format!("Content-Length: {}\r\n\r\n", payload.len());
        let mut stdin = self.stdin.lock().unwrap();
        let Some(stdin) = stdin.as_mut() else {
            return Err(McpError::TransportClosed.to_string());
        };
        stdin
            .write_all(frame.as_bytes())
            .and_then(|_| stdin.write_all(&payload))
            .map_err(|err| format!("write mcp transport payload: {err}"))
    }

    fn finish(&self, outcome: Result<(), String>) {
        let pending = std::mem::take(&mut *self.pending.lock().unwrap());
        for (_, (_, tx)) in pending {
            let _ = tx.send(Err(McpError::TransportClosed.to_string()));
        }
        let done = self.done.lock().unwrap().take();
        if let Some(done) = done {
            self.terminal.lock().unwrap().replace(outcome.clone());
            let _ = done.send(outcome);
        }
    }
}

impl Session for StdioSession {
    fn id(&self) -> String {
        self.session_id.clone()
    }

    fn list_tools(&self, timeout: Duration) -> Result<Vec<Tool>, String> {
        let raw = self.call("tools/list", serde_json::json!({}), timeout)?;
        decode_tools_list(&raw, &self.server_id, Utc::now())
    }

    fn call_tool(
        &self,
        tool_name: &str,
        input: Value,
    ) -> Result<serde_json::Map<String, Value>, String> {
        let raw = self.call(
            "tools/call",
            serde_json::json!({
                "name": tool_name.trim(),
                "arguments": normalize_tool_arguments(input),
            }),
            CALL_TIMEOUT,
        )?;
        serde_json::from_value(raw).map_err(|err| format!("decode tools/call response: {err}"))
    }

    fn close(&self) -> Result<(), String> {
        if self.closed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let pending = std::mem::take(&mut *self.pending.lock().unwrap());
        for (_, (_, tx)) in pending {
            let _ = tx.send(Err(McpError::TransportClosed.to_string()));
        }
        // Dropping the stdin handle sends EOF to the child, which terminates it and
        // unblocks the read loop.
        if let Some(mut stdin) = self.stdin.lock().unwrap().take() {
            let _ = stdin.flush();
            drop(stdin);
        }
        let done = self.done.lock().unwrap().take();
        if let Some(done) = done {
            self.terminal.lock().unwrap().replace(Ok(()));
            let _ = done.send(Ok(()));
        }
        Ok(())
    }

    fn wait_done(&self) -> Result<(), String> {
        if let Some(terminal) = self.terminal.lock().unwrap().clone() {
            return terminal;
        }
        let rx = self.done_rx.lock().unwrap().take();
        match rx {
            Some(rx) => match rx.recv() {
                Ok(outcome) => {
                    self.terminal.lock().unwrap().replace(outcome.clone());
                    outcome
                }
                Err(_) => Ok(()),
            },
            None => Ok(()),
        }
    }
}

/// Drains a reader to EOF (Go `io.Copy(io.Discard, stderr)`).
fn drain(mut reader: Box<dyn Read + Send>) {
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Streamable-http transport
// ---------------------------------------------------------------------------

/// Concrete streamable-http transport (Go `streamableHTTPTransport`): one JSON-RPC
/// request per HTTP POST via ureq.
#[derive(Debug, Default, Clone, Copy)]
pub struct StreamableHTTPTransport;

impl StreamableHTTPTransport {
    #[must_use]
    pub fn new() -> Self {
        StreamableHTTPTransport
    }
}

impl Transport for StreamableHTTPTransport {
    fn open(
        &self,
        server: &Server,
        _pipes: SessionPipes,
        timeout: Duration,
    ) -> Result<Arc<dyn Session>, McpError> {
        let endpoint = server.endpoint.trim().to_string();
        if endpoint.is_empty() {
            return Err(McpError::TransportUnavailable);
        }
        let (done_tx, done_rx) = mpsc::channel();
        let session = Arc::new(StreamableHTTPSession {
            server_id: server.server_id.trim().to_string(),
            session_id: session_id(&server.server_id),
            endpoint,
            timeout,
            done: Mutex::new(Some(done_tx)),
            done_rx: Mutex::new(Some(done_rx)),
            terminal: Mutex::new(None),
            closed: AtomicBool::new(false),
        });
        if let Err(err) = session.initialize() {
            let _ = session.close();
            return Err(McpError::Other(err));
        }
        Ok(session)
    }
}

/// One streamable-http MCP session. Every call is a synchronous request/response over
/// HTTP; there is no read loop or pending map (Go mirrors this).
struct StreamableHTTPSession {
    server_id: String,
    session_id: String,
    endpoint: String,
    timeout: Duration,
    done: Mutex<Option<Sender<Result<(), String>>>>,
    done_rx: Mutex<Option<Receiver<Result<(), String>>>>,
    terminal: Mutex<Option<Result<(), String>>>,
    closed: AtomicBool,
}

impl StreamableHTTPSession {
    fn initialize(&self) -> Result<(), String> {
        self.call("initialize", initialize_params(), self.timeout)
            .map_err(|err| format!("initialize mcp session for {}: {err}", self.server_id))?;
        self.notify("notifications/initialized", serde_json::json!({}), self.timeout)
            .map_err(|err| format!("send initialized notification for {}: {err}", self.server_id))
    }

    fn call(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": format!("{}", Utc::now().timestamp_nanos_opt().unwrap_or(0)),
            "method": method,
            "params": params,
        });
        let rpc = self.post_json(body, timeout)?;
        if rpc.error.is_some() {
            return Err(rpc_error(method, &rpc));
        }
        Ok(rpc.result.unwrap_or(Value::Null))
    }

    fn notify(&self, method: &str, params: Value, timeout: Duration) -> Result<(), String> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.post_raw(body, timeout).map(|_| ())
    }

    /// POST and parse a JSON-RPC response (Go `streamableHTTPSession.call`).
    fn post_json(&self, body: Value, timeout: Duration) -> Result<RpcResponse, String> {
        let (status, payload) = self.post_raw(body, timeout)?;
        if status >= 400 {
            return Err(format!(
                "mcp streamable-http returned {status}: {}",
                String::from_utf8_lossy(&payload).trim()
            ));
        }
        serde_json::from_slice(&payload)
            .map_err(|err| format!("decode mcp streamable-http response: {err}"))
    }

    /// POST and return status + body (Go's notify only checks the status, so a 202
    /// with an empty body must not be JSON-decoded).
    fn post_raw(&self, body: Value, timeout: Duration) -> Result<(u16, Vec<u8>), String> {
        let payload = serde_json::to_vec(&body)
            .map_err(|err| format!("marshal mcp streamable-http payload: {err}"))?;
        let response = ureq::post(&self.endpoint)
            .set("Content-Type", "application/json")
            .set("Accept", "application/json, text/event-stream")
            .timeout(timeout)
            .send_bytes(&payload);
        match response {
            Ok(resp) => {
                let status = resp.status();
                let mut buf = Vec::new();
                let mut reader = resp.into_reader();
                let _ = reader.read_to_end(&mut buf);
                Ok((status, buf))
            }
            Err(ureq::Error::Status(status, resp)) => {
                let mut buf = Vec::new();
                let mut reader = resp.into_reader();
                let _ = reader.read_to_end(&mut buf);
                Ok((status, buf))
            }
            Err(ureq::Error::Transport(err)) => {
                Err(format!("call mcp streamable-http endpoint: {err}"))
            }
        }
    }
}

impl Session for StreamableHTTPSession {
    fn id(&self) -> String {
        self.session_id.clone()
    }

    fn list_tools(&self, timeout: Duration) -> Result<Vec<Tool>, String> {
        let raw = self.call("tools/list", serde_json::json!({}), timeout)?;
        decode_tools_list(&raw, &self.server_id, Utc::now())
    }

    fn call_tool(
        &self,
        tool_name: &str,
        input: Value,
    ) -> Result<serde_json::Map<String, Value>, String> {
        let raw = self.call(
            "tools/call",
            serde_json::json!({
                "name": tool_name.trim(),
                "arguments": normalize_tool_arguments(input),
            }),
            CALL_TIMEOUT,
        )?;
        serde_json::from_value(raw).map_err(|err| format!("decode tools/call response: {err}"))
    }

    fn close(&self) -> Result<(), String> {
        if self.closed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let done = self.done.lock().unwrap().take();
        if let Some(done) = done {
            self.terminal.lock().unwrap().replace(Ok(()));
            let _ = done.send(Ok(()));
        }
        Ok(())
    }

    fn wait_done(&self) -> Result<(), String> {
        if let Some(terminal) = self.terminal.lock().unwrap().clone() {
            return terminal;
        }
        let rx = self.done_rx.lock().unwrap().take();
        match rx {
            Some(rx) => match rx.recv() {
                Ok(outcome) => {
                    self.terminal.lock().unwrap().replace(outcome.clone());
                    outcome
                }
                Err(_) => Ok(()),
            },
            None => Ok(()),
        }
    }
}

// ---------------------------------------------------------------------------
// Websocket transport
// ---------------------------------------------------------------------------

use futures_util::{SinkExt, StreamExt};
use tokio::runtime::Runtime;
use tokio::sync::Mutex as AsyncMutex;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{self, HeaderValue};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// The write half of the websocket connection, guarded so sync `call`s and the async
/// read loop never interleave `SinkExt::send`.
type WsSink = futures_util::stream::SplitSink<
    WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;
type WsRead = futures_util::stream::SplitStream<
    WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
>;

/// Concrete websocket transport (Go `websocketTransport`): JSON-RPC over websocket
/// text frames using tokio-tungstenite. The session owns a small multi-thread Tokio
/// runtime: the connect and each write run through `Runtime::block_on` from the sync
/// `Session` API, and a spawned read-loop task correlates responses by id.
#[derive(Debug, Default, Clone, Copy)]
pub struct WebsocketTransport;

impl WebsocketTransport {
    #[must_use]
    pub fn new() -> Self {
        WebsocketTransport
    }
}

impl Transport for WebsocketTransport {
    fn open(
        &self,
        server: &Server,
        _pipes: SessionPipes,
        timeout: Duration,
    ) -> Result<Arc<dyn Session>, McpError> {
        let endpoint = server.endpoint.trim().to_string();
        if endpoint.is_empty() {
            return Err(McpError::TransportUnavailable);
        }
        let runtime = ws_runtime()?;
        let request = build_ws_request(server)?;
        let (ws, _response) = run_blocking(&runtime, async {
                match tokio::time::timeout(
                    WEBSOCKET_HANDSHAKE_TIMEOUT,
                    tokio_tungstenite::connect_async(request),
                )
                .await
                {
                    Ok(Ok(connected)) => Ok(connected),
                    Ok(Err(err)) => Err(err),
                    Err(_) => Err(tokio_tungstenite::tungstenite::Error::Io(
                        std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "websocket handshake timed out",
                        ),
                    )),
                }
            })
            .map_err(|err| McpError::Other(format!("open mcp websocket transport: {err}")))?;
        let (sink, read) = ws.split();
        let session = WebsocketSession::new(server, sink, read, runtime)?;
        if let Err(err) = session.initialize(timeout) {
            let _ = session.close();
            return Err(McpError::Other(err));
        }
        Ok(session)
    }
}

/// Builds the websocket handshake request: headers from the resolved websocket auth
/// headers plus any configured subprotocols (Go `websocketTransport.Open`).
/// The shared websocket IO runtime. One process-wide runtime backs every
/// websocket session and is intentionally never dropped: per-session runtimes
/// panic when their last Arc drops inside an async context (e.g. a session
/// torn down from an API handler or startup restore).
fn ws_runtime() -> Result<Arc<tokio::runtime::Runtime>, McpError> {
    static RUNTIME: std::sync::OnceLock<Arc<tokio::runtime::Runtime>> = std::sync::OnceLock::new();
    if let Some(runtime) = RUNTIME.get() {
        return Ok(Arc::clone(runtime));
    }
    let built = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("mcp-ws")
        .worker_threads(2)
        .build()
        .map_err(|err| McpError::Other(format!("build mcp websocket runtime: {err}")))?;
    let _ = RUNTIME.set(Arc::new(built));
    Ok(Arc::clone(RUNTIME.get().expect("ws runtime initialized")))
}

/// Blocks on `fut` using the transport's private runtime. When the caller is
/// already inside a tokio runtime (e.g. daemon startup restore on the main
/// runtime), `block_in_place` releases the worker so the nested `block_on`
/// is legal; from plain threads it blocks directly.
fn run_blocking<F: std::future::Future>(runtime: &tokio::runtime::Runtime, fut: F) -> F::Output {
    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::task::block_in_place(|| runtime.block_on(fut))
    } else {
        runtime.block_on(fut)
    }
}

fn build_ws_request(server: &Server) -> Result<http::Request<()>, McpError> {
    let mut request = server
        .endpoint
        .trim()
        .into_client_request()
        .map_err(|err| McpError::Other(format!("build mcp websocket request: {err}")))?;
    for (name, value) in &server.resolved_websocket_headers {
        let name = name.trim();
        let value = value.trim();
        if name.is_empty() || value.is_empty() {
            continue;
        }
        let Ok(header_name) = http::HeaderName::from_bytes(name.as_bytes()) else {
            continue;
        };
        let Ok(header_value) = HeaderValue::from_str(value) else {
            continue;
        };
        request.headers_mut().insert(header_name, header_value);
    }
    if let Some(config) = &server.websocket_config {
        let protocols: Vec<&str> = config
            .subprotocols
            .iter()
            .map(|protocol| protocol.trim())
            .filter(|protocol| !protocol.is_empty())
            .collect();
        if !protocols.is_empty() {
            let Ok(header_value) = HeaderValue::from_str(&protocols.join(", ")) else {
                return Err(McpError::Other(
                    "websocket subprotocol header is not representable".to_string(),
                ));
            };
            request
                .headers_mut()
                .insert("sec-websocket-protocol", header_value);
        }
    }
    Ok(request)
}

/// One websocket MCP session. See `WebsocketTransport` for the threading model.
struct WebsocketSession {
    server_id: String,
    session_id: String,
    pending: Arc<Mutex<HashMap<String, PendingEntry>>>,
    writer: Arc<AsyncMutex<WsSink>>,
    runtime: Arc<Runtime>,
    closed: Arc<AtomicBool>,
    done: Arc<Mutex<Option<Sender<Result<(), String>>>>>,
    done_rx: Arc<Mutex<Option<Receiver<Result<(), String>>>>>,
    terminal: Arc<Mutex<Option<Result<(), String>>>>,
    request_id: AtomicU64,
}

impl WebsocketSession {
    fn new(
        server: &Server,
        sink: WsSink,
        read: WsRead,
        runtime: Arc<Runtime>,
    ) -> Result<Arc<Self>, McpError> {
        let (done_tx, done_rx) = mpsc::channel();
        let session = Arc::new(WebsocketSession {
            server_id: server.server_id.trim().to_string(),
            session_id: session_id(&server.server_id),
            pending: Arc::new(Mutex::new(HashMap::new())),
            writer: Arc::new(AsyncMutex::new(sink)),
            runtime,
            closed: Arc::new(AtomicBool::new(false)),
            done: Arc::new(Mutex::new(Some(done_tx))),
            done_rx: Arc::new(Mutex::new(Some(done_rx))),
            terminal: Arc::new(Mutex::new(None)),
            request_id: AtomicU64::new(0),
        });
        session.spawn_read_loop(read);
        Ok(session)
    }

    /// Spawns the read-loop task on the session runtime. It consumes text frames,
    /// correlates responses by id, replies to pings (tungstenite queues the pong
    /// automatically), and terminates the session on a close frame / transport error.
    fn spawn_read_loop(self: &Arc<Self>, read: WsRead) {
        let finish_pending = Arc::clone(&self.pending);
        let finish_done = Arc::clone(&self.done);
        let finish_terminal = Arc::clone(&self.terminal);
        let finish = move |outcome: Result<(), String>| {
            let entries = std::mem::take(&mut *finish_pending.lock().unwrap());
            for (_, (_, tx)) in entries {
                let _ = tx.send(Err(McpError::TransportClosed.to_string()));
            }
            let done = finish_done.lock().unwrap().take();
            if let Some(done) = done {
                finish_terminal.lock().unwrap().replace(outcome.clone());
                let _ = done.send(outcome);
            }
        };
        let pending = Arc::clone(&self.pending);
        self.runtime.spawn(async move {
            let mut read = read;
            while let Some(message) = read.next().await {
                match message {
                    Ok(Message::Text(text)) => {
                        let response: RpcResponse = match serde_json::from_str(&text) {
                            Ok(response) => response,
                            Err(err) => {
                                finish(Err(format!("decode mcp websocket response: {err}")));
                                return;
                            }
                        };
                        if response.id.trim().is_empty() {
                            continue;
                        }
                        let delivered = pending.lock().unwrap().remove(&response.id);
                        if let Some((method, tx)) = delivered {
                            let outcome = match response.error.as_ref() {
                                Some(_) => Err(rpc_error(&method, &response)),
                                None => Ok(response.result.unwrap_or(Value::Null)),
                            };
                            let _ = tx.send(outcome);
                        }
                    }
                    Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => {}
                    Ok(Message::Close(_)) => {
                        // Go's gorilla readLoop surfaces a close frame as an error and
                        // the manager decides whether to reconnect.
                        finish(Err("mcp websocket connection closed by server".to_string()));
                        return;
                    }
                    Ok(_) => {}
                    Err(err) => {
                        finish(Err(format!("mcp websocket transport: {err}")));
                        return;
                    }
                }
            }
            finish(Err("mcp websocket connection closed".to_string()));
        });
    }

    fn initialize(&self, timeout: Duration) -> Result<(), String> {
        self.call("initialize", initialize_params(), timeout)
            .map_err(|err| format!("initialize mcp session for {}: {err}", self.server_id))?;
        self.notify("notifications/initialized", serde_json::json!({}))
            .map_err(|err| format!("send initialized notification for {}: {err}", self.server_id))
    }

    fn call(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        let request_id = format!("{}", self.request_id.fetch_add(1, Ordering::SeqCst));
        let (tx, rx) = mpsc::channel();
        {
            let mut pending = self.pending.lock().unwrap();
            if self.closed.load(Ordering::SeqCst) {
                return Err(McpError::TransportClosed.to_string());
            }
            pending.insert(request_id.clone(), (method.to_string(), tx));
        }
        let payload = match serde_json::to_string(&RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: request_id.clone(),
            method: method.to_string(),
            params: Some(params),
        }) {
            Ok(payload) => payload,
            Err(err) => {
                self.pending.lock().unwrap().remove(&request_id);
                return Err(format!("marshal mcp websocket payload: {err}"));
            }
        };
        let write_result = {
            let writer = Arc::clone(&self.writer);
            let runtime = Arc::clone(&self.runtime);
            run_blocking(&runtime, async move {
                let mut writer = writer.lock().await;
                writer.send(Message::Text(payload)).await
            })
        };
        if let Err(err) = write_result {
            self.pending.lock().unwrap().remove(&request_id);
            return Err(format!("write mcp websocket payload: {err}"));
        }
        match rx.recv_timeout(timeout) {
            Ok(outcome) => outcome,
            Err(RecvTimeoutError::Timeout) => {
                self.pending.lock().unwrap().remove(&request_id);
                Err(format!("mcp {method} timed out after {}s", timeout.as_secs()))
            }
            Err(RecvTimeoutError::Disconnected) => Err(McpError::TransportClosed.to_string()),
        }
    }

    fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        let payload = serde_json::to_string(&RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: String::new(),
            method: method.to_string(),
            params: Some(params),
        })
        .map_err(|err| format!("marshal mcp websocket payload: {err}"))?;
        let writer = Arc::clone(&self.writer);
        let runtime = Arc::clone(&self.runtime);
        run_blocking(&runtime, async move {
            let mut writer = writer.lock().await;
            writer.send(Message::Text(payload)).await
        })
        .map_err(|err| format!("write mcp websocket payload: {err}"))
    }
}

impl Session for WebsocketSession {
    fn id(&self) -> String {
        self.session_id.clone()
    }

    fn list_tools(&self, timeout: Duration) -> Result<Vec<Tool>, String> {
        let raw = self.call("tools/list", serde_json::json!({}), timeout)?;
        decode_tools_list(&raw, &self.server_id, Utc::now())
    }

    fn call_tool(
        &self,
        tool_name: &str,
        input: Value,
    ) -> Result<serde_json::Map<String, Value>, String> {
        let raw = self.call(
            "tools/call",
            serde_json::json!({
                "name": tool_name.trim(),
                "arguments": normalize_tool_arguments(input),
            }),
            CALL_TIMEOUT,
        )?;
        serde_json::from_value(raw).map_err(|err| format!("decode tools/call response: {err}"))
    }

    fn close(&self) -> Result<(), String> {
        if self.closed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let pending = std::mem::take(&mut *self.pending.lock().unwrap());
        for (_, (_, tx)) in pending {
            let _ = tx.send(Err(McpError::TransportClosed.to_string()));
        }
        // Best-effort close frame; the read-loop task tears down on its own.
        let writer = Arc::clone(&self.writer);
        let runtime = Arc::clone(&self.runtime);
        let _ = run_blocking(&runtime, async move {
            let mut writer = writer.lock().await;
            let _ = writer.close().await;
        });
        let done = self.done.lock().unwrap().take();
        if let Some(done) = done {
            self.terminal.lock().unwrap().replace(Ok(()));
            let _ = done.send(Ok(()));
        }
        Ok(())
    }

    fn wait_done(&self) -> Result<(), String> {
        if let Some(terminal) = self.terminal.lock().unwrap().clone() {
            return terminal;
        }
        let rx = self.done_rx.lock().unwrap().take();
        match rx {
            Some(rx) => match rx.recv() {
                Ok(outcome) => {
                    self.terminal.lock().unwrap().replace(outcome.clone());
                    outcome
                }
                Err(_) => Ok(()),
            },
            None => Ok(()),
        }
    }
}

// ---------------------------------------------------------------------------
// Transport mux
// ---------------------------------------------------------------------------

/// Dispatch on `TransportKind`, mirroring Go `transportMux` / `NewTransportMux`.
/// `Default` installs all three concrete transports; `new` substitutes the concrete
/// default for any `None` slot, exactly like Go.
#[derive(Clone)]
pub struct TransportMux {
    pub stdio: Arc<dyn Transport>,
    pub remote: Arc<dyn Transport>,
    pub websocket: Arc<dyn Transport>,
}

impl Default for TransportMux {
    fn default() -> Self {
        TransportMux {
            stdio: Arc::new(StdioTransport::new()),
            remote: Arc::new(StreamableHTTPTransport::new()),
            websocket: Arc::new(WebsocketTransport::new()),
        }
    }
}

impl TransportMux {
    /// Go `NewTransportMux` (nil transports substitute the defaults). Pass `None` for
    /// the concrete defaults; there is no way to leave a slot unavailable (Go cannot
    /// either once defaults are substituted).
    #[must_use]
    pub fn new(
        stdio: Option<Arc<dyn Transport>>,
        remote: Option<Arc<dyn Transport>>,
        websocket: Option<Arc<dyn Transport>>,
    ) -> Self {
        TransportMux {
            stdio: stdio.unwrap_or_else(|| Arc::new(StdioTransport::new())),
            remote: remote.unwrap_or_else(|| Arc::new(StreamableHTTPTransport::new())),
            websocket: websocket.unwrap_or_else(|| Arc::new(WebsocketTransport::new())),
        }
    }
}

impl Transport for TransportMux {
    fn open(
        &self,
        server: &Server,
        pipes: SessionPipes,
        timeout: Duration,
    ) -> Result<Arc<dyn Session>, McpError> {
        let selected = match server.transport_kind {
            TransportKind::Stdio => self.stdio.as_ref(),
            TransportKind::StreamableHTTP => self.remote.as_ref(),
            TransportKind::Websocket => self.websocket.as_ref(),
        };
        selected.open(server, pipes, timeout)
    }
}
