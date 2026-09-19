//! Stage 9.4: the real browser driver seam — a supervised worker process
//! behind the existing [`Driver`] trait.
//!
//! The daemon never links a browser. It speaks a line-delimited JSON protocol
//! over the worker's stdio (`capabilities/browser/PROTOCOL.md`), so the heavy,
//! fragile, dependency-rich part (Playwright + Chromium) lives outside the
//! process boundary as the capability README prescribes. The manager, policy
//! gating and artifact recorder are unchanged: they see a `Driver`.
//!
//! Protocol (one JSON object per line, request then response, in order):
//!
//! ```text
//! → {"id":1,"op":"start_session","session":{…},"input":{…}}
//! ← {"id":1,"ok":true,"session":{…}}
//! → {"id":2,"op":"execute_action","session":{…},"action":{…}}
//! ← {"id":2,"ok":true,"session":{…},"action":{…},"captures":[{"kind":"screenshot","mimeType":"image/png","fileName":"screenshot.png","contentBase64":"…"}]}
//! → {"id":3,"op":"close_session","session":{…}}
//! ← {"id":3,"ok":true,"session":{…}}
//! ← {"id":n,"ok":false,"error":"…"}            (any op)
//! ```
//!
//! Supervision: a worker that exits, hangs past `timeout_ms`, or answers
//! malformed JSON is reported to the capability supervisor as a failure and
//! respawned on the next call; a healthy answer reports health. The
//! supervisor's backoff/circuit-break state is therefore the operator's view
//! of the browser's condition (`/v1/capabilities`).

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::{Action, ArtifactCaptureRequest, ArtifactKind, CreateSessionInput, Driver, Session};

/// The capability kind the browser worker registers under.
pub const KIND_BROWSER_WORKER: &str = "browser_worker";
pub const DEFAULT_CAPABILITY_ID: &str = "browser";
const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// How the supervisor is told about the worker. Kept as a trait so the
/// driver crate does not depend on `kura-capabilities` (which sits above it
/// in the workspace), and so tests can observe reports.
pub trait WorkerSupervisor: Send + Sync {
    fn report_healthy(&self, capability_id: &str);
    fn report_failure(&self, capability_id: &str, reason: &str);
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SubprocessDriverConfig {
    /// The worker executable (e.g. `node`).
    pub command: String,
    /// Its arguments (e.g. `["capabilities/browser/worker.mjs"]`).
    pub args: Vec<String>,
    /// Extra environment for the worker (e.g. a Playwright path). Values
    /// here are configuration, never secrets: the worker has no credentials.
    pub env: Vec<(String, String)>,
    /// Per-request bound; a hung browser must not hang a turn.
    pub timeout_ms: u64,
    pub capability_id: String,
}

impl Default for SubprocessDriverConfig {
    fn default() -> Self {
        SubprocessDriverConfig {
            command: String::new(),
            args: Vec::new(),
            env: Vec::new(),
            timeout_ms: DEFAULT_TIMEOUT_MS,
            capability_id: DEFAULT_CAPABILITY_ID.to_string(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Request<'a> {
    id: u64,
    op: &'static str,
    session: &'a Session,
    #[serde(skip_serializing_if = "Option::is_none")]
    input: Option<&'a CreateSessionInput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    action: Option<&'a Action>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Response {
    id: u64,
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    error: String,
    #[serde(default)]
    session: Option<Session>,
    #[serde(default)]
    action: Option<Action>,
    #[serde(default)]
    captures: Vec<WireCapture>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireCapture {
    kind: ArtifactKind,
    #[serde(default)]
    mime_type: String,
    #[serde(default)]
    file_name: String,
    #[serde(default)]
    content_base64: String,
}

struct Worker {
    child: Child,
    stdin: ChildStdin,
    /// Lines from the worker's stdout, read on a dedicated thread so a
    /// request can time out without blocking on a dead pipe.
    lines: mpsc::Receiver<std::io::Result<String>>,
}

impl Worker {
    fn spawn(config: &SubprocessDriverConfig) -> Result<Self, String> {
        let mut command = Command::new(&config.command);
        command
            .args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        for (key, value) in &config.env {
            command.env(key, value);
        }
        let mut child = command
            .spawn()
            .map_err(|err| format!("spawn browser worker `{}`: {err}", config.command))?;
        let stdin = child.stdin.take().ok_or("worker stdin unavailable")?;
        let stdout: ChildStdout = child.stdout.take().ok_or("worker stdout unavailable")?;
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("browser-worker-stdout".to_string())
            .spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines() {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            })
            .map_err(|err| format!("spawn worker reader thread: {err}"))?;
        Ok(Worker {
            child,
            stdin,
            lines: rx,
        })
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.kill();
    }
}

/// A [`Driver`] backed by a supervised worker process.
pub struct SubprocessDriver {
    config: SubprocessDriverConfig,
    supervisor: Option<Arc<dyn WorkerSupervisor>>,
    worker: Mutex<Option<Worker>>,
    next_id: std::sync::atomic::AtomicU64,
    restarts: std::sync::atomic::AtomicU64,
    spawned_once: std::sync::atomic::AtomicBool,
}

impl SubprocessDriver {
    #[must_use]
    pub fn new(
        config: SubprocessDriverConfig,
        supervisor: Option<Arc<dyn WorkerSupervisor>>,
    ) -> Self {
        SubprocessDriver {
            config,
            supervisor,
            worker: Mutex::new(None),
            next_id: std::sync::atomic::AtomicU64::new(1),
            restarts: std::sync::atomic::AtomicU64::new(0),
            spawned_once: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Times the worker was (re)spawned after the first start.
    #[must_use]
    pub fn restart_count(&self) -> u64 {
        self.restarts.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn fail(&self, reason: &str) -> String {
        if let Some(sup) = &self.supervisor {
            sup.report_failure(&self.config.capability_id, reason);
        }
        reason.to_string()
    }

    /// Sends one request; the worker lock is held for the whole exchange so
    /// requests and responses stay in order. A dead or unresponsive worker is
    /// killed and dropped so the next call respawns it.
    fn exchange(&self, request: &Request<'_>) -> Result<Response, String> {
        let mut guard = self.worker.lock();
        if guard.is_none() {
            let worker = Worker::spawn(&self.config).map_err(|err| self.fail(&err))?;
            if self
                .spawned_once
                .swap(true, std::sync::atomic::Ordering::SeqCst)
            {
                self.restarts
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            *guard = Some(worker);
        }
        let worker = guard.as_mut().expect("worker spawned");
        let mut line = serde_json::to_string(request).map_err(|err| err.to_string())?;
        line.push('\n');
        if let Err(err) = worker
            .stdin
            .write_all(line.as_bytes())
            .and_then(|()| worker.stdin.flush())
        {
            worker.kill();
            *guard = None;
            return Err(self.fail(&format!("browser worker write failed: {err}")));
        }
        let timeout = Duration::from_millis(self.config.timeout_ms.max(1));
        let received = match worker.lines.recv_timeout(timeout) {
            Ok(Ok(text)) => text,
            Ok(Err(err)) => {
                worker.kill();
                *guard = None;
                return Err(self.fail(&format!("browser worker read failed: {err}")));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                worker.kill();
                *guard = None;
                return Err(self.fail(&format!(
                    "browser worker did not answer within {}ms",
                    self.config.timeout_ms
                )));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                worker.kill();
                *guard = None;
                return Err(self.fail("browser worker exited"));
            }
        };
        let response: Response = match serde_json::from_str(&received) {
            Ok(response) => response,
            Err(err) => {
                worker.kill();
                *guard = None;
                return Err(self.fail(&format!("browser worker answered malformed JSON: {err}")));
            }
        };
        if response.id != request.id {
            worker.kill();
            *guard = None;
            return Err(self.fail(&format!(
                "browser worker answered out of order (expected {}, got {})",
                request.id, response.id
            )));
        }
        if let Some(sup) = &self.supervisor {
            sup.report_healthy(&self.config.capability_id);
        }
        Ok(response)
    }

    fn request_id(&self) -> u64 {
        self.next_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }
}

impl Driver for SubprocessDriver {
    fn start_session(
        &self,
        session: Session,
        input: CreateSessionInput,
    ) -> Result<Session, String> {
        let request = Request {
            id: self.request_id(),
            op: "start_session",
            session: &session,
            input: Some(&input),
            action: None,
        };
        let response = self.exchange(&request)?;
        if !response.ok {
            return Err(worker_error(&response));
        }
        response
            .session
            .ok_or_else(|| "worker returned no session".to_string())
    }

    fn execute_action(
        &self,
        session: Session,
        action: Action,
    ) -> Result<(Session, Action, Vec<ArtifactCaptureRequest>), String> {
        let request = Request {
            id: self.request_id(),
            op: "execute_action",
            session: &session,
            input: None,
            action: Some(&action),
        };
        let response = self.exchange(&request)?;
        if !response.ok {
            return Err(worker_error(&response));
        }
        let next_session = response.session.ok_or("worker returned no session")?;
        let next_action = response.action.ok_or("worker returned no action")?;
        let mut captures = Vec::with_capacity(response.captures.len());
        for capture in response.captures {
            let content = decode_base64(&capture.content_base64)
                .map_err(|err| format!("capture {}: {err}", capture.file_name))?;
            captures.push(ArtifactCaptureRequest {
                run_id: next_session.run_id.clone(),
                computer_use_session_id: next_session.computer_use_session_id.clone(),
                computer_use_action_id: next_action.computer_use_action_id.clone(),
                kind: capture.kind,
                mime_type: capture.mime_type,
                file_name: capture.file_name,
                estimated_byte_size: content.len() as i64,
                content,
            });
        }
        Ok((next_session, next_action, captures))
    }

    fn close_session(&self, session: Session) -> Result<Session, String> {
        let request = Request {
            id: self.request_id(),
            op: "close_session",
            session: &session,
            input: None,
            action: None,
        };
        let response = self.exchange(&request)?;
        if !response.ok {
            return Err(worker_error(&response));
        }
        response
            .session
            .ok_or_else(|| "worker returned no session".to_string())
    }
}

fn worker_error(response: &Response) -> String {
    if response.error.is_empty() {
        "browser worker reported an error".to_string()
    } else {
        format!("browser worker: {}", response.error)
    }
}

/// Standard-alphabet base64 (with or without padding). Hand-rolled to keep
/// the driver dependency-free; captures are small (screenshots, snapshots).
pub fn decode_base64(input: &str) -> Result<Vec<u8>, String> {
    fn value(c: u8) -> Result<u32, String> {
        Ok(match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a' + 26) as u32,
            b'0'..=b'9' => (c - b'0' + 52) as u32,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return Err(format!("invalid base64 byte {c:#x}")),
        })
    }
    let bytes: Vec<u8> = input
        .bytes()
        .filter(|b| !matches!(b, b'\n' | b'\r' | b' ' | b'='))
        .collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut acc: u32 = 0;
        for (i, b) in chunk.iter().enumerate() {
            acc |= value(*b)? << (18 - 6 * i);
        }
        match chunk.len() {
            4 => out.extend_from_slice(&[(acc >> 16) as u8, (acc >> 8) as u8, acc as u8]),
            3 => out.extend_from_slice(&[(acc >> 16) as u8, (acc >> 8) as u8]),
            2 => out.push((acc >> 16) as u8),
            _ => return Err("truncated base64 input".to_string()),
        }
    }
    Ok(out)
}

pub fn encode_base64(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((n >> 18) & 63) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips_binary() {
        for sample in [&b""[..], b"f", b"fo", b"foo", b"\x00\xff\x10PNG\r\n"] {
            assert_eq!(decode_base64(&encode_base64(sample)).unwrap(), sample);
        }
        assert_eq!(decode_base64("Zm9v").unwrap(), b"foo");
        assert_eq!(decode_base64("Zm8=").unwrap(), b"fo");
        assert!(decode_base64("Zm9v!").is_err());
    }
}
