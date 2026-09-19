//! Shared test helpers for the kura-mcp integration tests: a fake stdio MCP server
//! subprocess launcher (the `fake-mcp-server` bin target), a minimal in-process HTTP
//! server speaking the MCP streamable-http JSON-RPC shape, and a minimal in-process
//! WebSocket server (RFC 6455 handshake + masked text frames) answering the same
//! methods. All three speak the same fake protocol as the `fake-mcp-server` bin.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};

use kura_mcp::transport::{RpcRequest, RpcResponse};
use serde_json::json;

/// Path of the fake stdio MCP server binary (built by cargo for this package).
#[must_use]
pub fn fake_mcp_server_bin() -> &'static str {
    env!("CARGO_BIN_EXE_fake_mcp_server")
}

/// Builds the fake RPC response for one request (shared by all three servers).
#[allow(dead_code)]
pub fn fake_response(request: &RpcRequest) -> RpcResponse {
    let id = request.id.clone();
    match request.method.as_str() {
        "initialize" => RpcResponse {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "fake-mcp-server", "version": "0.1.0" },
            })),
            error: None,
        },
        "tools/list" => RpcResponse {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(json!({
                "tools": [{
                    "name": "echo",
                    "title": "Echo",
                    "description": "Echoes the message argument back",
                    "inputSchema": {
                        "type": "object",
                        "properties": { "message": { "type": "string" } },
                        "required": ["message"],
                    },
                }],
            })),
            error: None,
        },
        "tools/call" => {
            let arguments = request
                .params
                .as_ref()
                .and_then(|params| params.get("arguments"))
                .cloned()
                .unwrap_or_else(|| json!({}));
            let text = arguments
                .get("message")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string();
            RpcResponse {
                jsonrpc: "2.0".to_string(),
                id,
                result: Some(json!({
                    "content": [{ "type": "text", "text": text }],
                    "isError": false,
                })),
                error: None,
            }
        }
        _ => RpcResponse {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(kura_mcp::transport::RpcError {
                code: -32601,
                message: format!("method not found: {}", request.method),
            }),
        },
    }
}

// ---------------------------------------------------------------------------
// HTTP server (streamable-http test)
// ---------------------------------------------------------------------------

/// Starts a one-shot MCP HTTP server on an ephemeral localhost port and returns its
/// address. Notifications get a 202 with an empty body; requests get a JSON-RPC
/// response.
pub fn spawn_mcp_http_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind http server");
    let addr = listener.local_addr().expect("http server addr");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                break;
            };
            if handle_http_connection(&mut stream).is_err() {
                break;
            }
        }
    });
    addr
}

fn handle_http_connection(stream: &mut TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    // request line
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    // headers
    let mut content_length: usize = 0;
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            return Ok(());
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        let lower = trimmed.to_ascii_lowercase();
        if let Some(value) = lower.strip_prefix("content-length:") {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body)?;
    }
    let request: RpcRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => {
            write_http(stream, 400, b"bad request")?;
            return Ok(());
        }
    };
    if request.id.trim().is_empty() {
        // notification: 202 Accepted, empty body
        write_http(stream, 202, b"")?;
        return Ok(());
    }
    let response = fake_response(&request);
    let payload = serde_json::to_vec(&response).unwrap_or_default();
    write_http(stream, 200, &payload)
}

fn write_http(stream: &mut TcpStream, status: u16, body: &[u8]) -> std::io::Result<()> {
    let reason = if status == 200 {
        "OK"
    } else if status == 202 {
        "Accepted"
    } else {
        "Bad Request"
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()
}

// ---------------------------------------------------------------------------
// WebSocket server (websocket test)
// ---------------------------------------------------------------------------

/// Starts a minimal RFC 6455 MCP server on an ephemeral localhost port and returns its
/// address. Text requests are answered with the fake JSON-RPC responses; the close
/// frame is echoed back. Handles exactly one connection.
pub fn spawn_mcp_ws_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ws server");
    let addr = listener.local_addr().expect("ws server addr");
    std::thread::spawn(move || {
        // Exactly one connection: accept it and serve it, nothing more.
        if let Some(Ok(mut stream)) = listener.incoming().next() {
            let _ = handle_ws_connection(&mut stream);
        }
    });
    addr
}

fn handle_ws_connection(stream: &mut TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut key = String::new();
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            return Ok(());
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some(colon) = trimmed.find(':') {
            let name = &trimmed[..colon];
            if name.eq_ignore_ascii_case("sec-websocket-key") {
                key = trimmed[colon + 1..].trim().to_string();
            }
        }
    }
    if key.is_empty() {
        return Ok(());
    }
    let accept = ws_accept_key(&key);
    write!(
        stream,
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    )?;
    stream.flush()?;
    loop {
        let frame = match read_ws_frame(&mut reader) {
            Ok(frame) => frame,
            Err(_) => return Ok(()),
        };
        if frame.opcode == 8 {
            // close: echo a close frame back and end the connection
            let _ = write_ws_frame(stream, 8, &[]);
            return Ok(());
        }
        if frame.opcode != 1 {
            continue;
        }
        let text = String::from_utf8_lossy(&frame.payload);
        let request: RpcRequest = match serde_json::from_str(&text) {
            Ok(request) => request,
            Err(_) => continue,
        };
        if request.id.trim().is_empty() {
            continue;
        }
        let response = fake_response(&request);
        if let Ok(payload) = serde_json::to_vec(&response) {
            if write_ws_frame(stream, 1, &payload).is_err() {
                return Ok(());
            }
        }
    }
}

/// One parsed websocket frame (client->server frames are masked).
struct WsFrame {
    opcode: u8,
    payload: Vec<u8>,
}

fn read_ws_frame(reader: &mut impl Read) -> std::io::Result<WsFrame> {
    let mut b0 = [0u8; 1];
    reader.read_exact(&mut b0)?;
    let opcode = b0[0] & 0x0f;
    let mut b1 = [0u8; 1];
    reader.read_exact(&mut b1)?;
    let masked = b1[0] & 0x80 != 0;
    let len7 = (b1[0] & 0x7f) as u64;
    let mut len = len7;
    if len7 == 126 {
        let mut ext = [0u8; 2];
        reader.read_exact(&mut ext)?;
        len = u16::from_be_bytes(ext) as u64;
    } else if len7 == 127 {
        let mut ext = [0u8; 8];
        reader.read_exact(&mut ext)?;
        len = u64::from_be_bytes(ext);
    }
    let mut mask = [0u8; 4];
    if masked {
        reader.read_exact(&mut mask)?;
    }
    let mut payload = vec![0u8; len as usize];
    reader.read_exact(&mut payload)?;
    if masked {
        for (i, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[i % 4];
        }
    }
    Ok(WsFrame { opcode, payload })
}

fn write_ws_frame(writer: &mut impl Write, opcode: u8, payload: &[u8]) -> std::io::Result<()> {
    writer.write_all(&[0x80 | opcode])?;
    if payload.len() < 126 {
        writer.write_all(&[payload.len() as u8])?;
    } else if payload.len() < 65_536 {
        writer.write_all(&[126])?;
        writer.write_all(&(payload.len() as u16).to_be_bytes())?;
    } else {
        writer.write_all(&[127])?;
        writer.write_all(&(payload.len() as u64).to_be_bytes())?;
    }
    writer.write_all(payload)?;
    writer.flush()
}

/// RFC 6455 Sec-WebSocket-Accept = base64(sha1(key + magic GUID)).
fn ws_accept_key(key: &str) -> String {
    use sha1::{Digest, Sha1};
    let mut hasher = Sha1::new();
    hasher.update(key.as_bytes());
    hasher.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    base64_encode(&hasher.finalize())
}

fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((n >> 18) & 63) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[((n >> 6) & 63) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(n & 63) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}
