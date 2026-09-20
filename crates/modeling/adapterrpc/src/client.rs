//! The daemon-side [`Client`] that domain Backend shims (calendar, mail) use to dispatch
//! operations to an out-of-process adapter. Mirrors `client.go`.

use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::value::RawValue;

use crate::error::{AdapterError, Error};
use crate::transport::Transport;
use crate::types::{CONTRACT_VERSION, Request, Status};

/// Applied when a dispatch carries no explicit deadline (spec clarification Q1 / FR-007b).
/// Plan research R3 proposes 30s.
pub const DEFAULT_DEADLINE: Duration = Duration::from_secs(30);

/// Error type a credential resolver may fail with.
pub type ResolverError = Box<dyn std::error::Error + Send + Sync>;

/// Returns scoped, short-lived credential material for a single call (Roadmap 37 secret
/// path). It receives the marshaled resource so it can scope to the integration/tenant.
/// Returning an error fails the operation closed. `None` material means the operation
/// needs no provider credentials (e.g. the reference adapter / fake path).
pub type CredentialResolver = Box<
    dyn Fn(&str, Option<&RawValue>) -> Result<Option<Box<RawValue>>, ResolverError> + Send + Sync,
>;

/// Dispatches Backend operations to an out-of-process adapter over the RPC contract. It is
/// domain-agnostic: payloads are opaque JSON shaped by the calling domain shim.
pub struct Client {
    version: String,
    default_deadline: Duration,
    transport: Transport,
    seq: AtomicI64,
    cmd: Mutex<Option<Child>>, // Some when the client owns a spawned process
    credentials: Option<CredentialResolver>, // None when no provider credentials required
}

impl Client {
    /// Build a Client over an existing writer/reader pair (used in tests with an
    /// in-process adapter connected via pipes).
    pub fn new(w: impl Write + Send + 'static, r: impl Read + Send + 'static) -> Self {
        Client {
            version: CONTRACT_VERSION.to_owned(),
            default_deadline: DEFAULT_DEADLINE,
            transport: Transport::new(Box::new(w), Box::new(r)),
            seq: AtomicI64::new(0),
            cmd: Mutex::new(None),
            credentials: None,
        }
    }

    /// Spawn the adapter binary and connect to its stdio. The caller owns the returned
    /// Client and MUST call [`Client::close`] to terminate the process.
    pub fn new_process(name: &str, args: &[&str]) -> Result<Self, Error> {
        let mut child = Command::new(name)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|source| Error::Process {
                what: "start",
                source,
            })?;
        let stdin = child.stdin.take().ok_or_else(|| Error::Process {
            what: "stdin",
            source: std::io::Error::new(std::io::ErrorKind::BrokenPipe, "stdin not piped"),
        })?;
        let stdout = child.stdout.take().ok_or_else(|| Error::Process {
            what: "stdout",
            source: std::io::Error::new(std::io::ErrorKind::BrokenPipe, "stdout not piped"),
        })?;
        let client = Client::new(stdin, stdout);
        *client.cmd.lock() = Some(child);
        Ok(client)
    }

    /// Install a per-call scoped credential resolver and return the client.
    pub fn with_credentials(mut self, resolver: CredentialResolver) -> Self {
        self.credentials = Some(resolver);
        self
    }

    /// Override the deadline applied when a dispatch carries no explicit timeout.
    /// Non-positive durations are ignored (mirrors Go `WithDefaultDeadline`).
    pub fn with_default_deadline(mut self, d: Duration) -> Self {
        if !d.is_zero() {
            self.default_deadline = d;
        }
        self
    }

    /// Terminate a spawned adapter process. Safe to call when the client does not own one.
    pub fn close(&self) -> Result<(), Error> {
        let mut guard = self.cmd.lock();
        let Some(child) = guard.as_mut() else {
            return Ok(());
        };
        let _ = child.kill();
        child.wait().map_err(|source| Error::Process {
            what: "wait",
            source,
        })?;
        *guard = None;
        Ok(())
    }

    fn next_request_id(&self) -> String {
        format!("req-{}", self.seq.fetch_add(1, Ordering::SeqCst) + 1)
    }

    /// Contract-version readiness handshake. Returns [`Error::ContractMismatch`] when the
    /// adapter advertises a version that is not an exact match for the daemon's (FR-008),
    /// so the supervisor can refuse to mark the adapter ready before any operation is
    /// attempted.
    pub fn ready(&self) -> Result<(), Error> {
        self.dispatch::<serde_json::Value, serde_json::Value, serde_json::Value>(
            "capability",
            "Ready",
            None,
            None,
            None,
        )
    }

    /// Send one operation with the client's default deadline (Go: dispatch with a
    /// deadline-less context; the daemon always bounds the call so a hung adapter can
    /// never block indefinitely, FR-007b).
    pub fn dispatch<R, P, O>(
        &self,
        domain: &str,
        operation: &str,
        resource: Option<&R>,
        payload: Option<&P>,
        out: Option<&mut O>,
    ) -> Result<(), Error>
    where
        R: Serialize + ?Sized,
        P: Serialize + ?Sized,
        O: DeserializeOwned,
    {
        self.dispatch_with_timeout(
            self.default_deadline,
            domain,
            operation,
            resource,
            payload,
            out,
        )
    }

    /// Send one operation bounded by `timeout` (Go: dispatch with a context deadline). It
    /// marshals resource and payload, resolves per-call scoped credentials (failing closed
    /// on resolver error), validates the contract version and correlation, maps adapter
    /// failures to [`AdapterError`], and unmarshals an ok payload into `out`. `resource`
    /// and `out` may be `None`; `out` is `None` for operations with no return value.
    pub fn dispatch_with_timeout<R, P, O>(
        &self,
        timeout: Duration,
        domain: &str,
        operation: &str,
        resource: Option<&R>,
        payload: Option<&P>,
        out: Option<&mut O>,
    ) -> Result<(), Error>
    where
        R: Serialize + ?Sized,
        P: Serialize + ?Sized,
        O: DeserializeOwned,
    {
        let raw_resource = marshal_raw(resource).map_err(|source| Error::Marshal {
            domain: domain.to_owned(),
            operation: operation.to_owned(),
            what: "resource",
            source,
        })?;
        let raw_payload = marshal_raw(payload).map_err(|source| Error::Marshal {
            domain: domain.to_owned(),
            operation: operation.to_owned(),
            what: "payload",
            source,
        })?;

        let mut credential = None;
        if let Some(resolve) = &self.credentials {
            credential =
                resolve(domain, raw_resource.as_deref()).map_err(|source| Error::Credential {
                    domain: domain.to_owned(),
                    operation: operation.to_owned(),
                    source,
                })?;
        }

        let req = Request {
            request_id: self.next_request_id(),
            contract_version: self.version.clone(),
            domain: domain.to_owned(),
            operation: operation.to_owned(),
            deadline_ms: timeout.as_millis().min(i64::MAX as u128) as i64,
            resource: raw_resource,
            credential,
            payload: raw_payload,
        };

        let resp = self.transport.round_trip(&req, timeout)?;
        if resp.contract_version != self.version {
            return Err(Error::ContractMismatch);
        }
        if resp.request_id != req.request_id {
            return Err(Error::Correlation);
        }
        if resp.status == Status::Failure {
            return Err(AdapterError {
                kind: resp
                    .failure_kind
                    .clone()
                    .unwrap_or_else(|| crate::FailureKind::Other(String::new())),
                detail: diagnostic_detail(resp.diagnostic.as_deref()),
            }
            .into());
        }
        if let (Some(out), Some(raw)) = (out, resp.payload) {
            let val: O = serde_json::from_str(raw.get()).map_err(|e| {
                // An undecodable ok payload is an unconfirmed outcome for writes (FR-007a).
                Error::MalformedResponse(e.to_string())
            })?;
            *out = val;
        }
        Ok(())
    }
}

fn marshal_raw<T: Serialize + ?Sized>(
    v: Option<&T>,
) -> Result<Option<Box<RawValue>>, serde_json::Error> {
    match v {
        None => Ok(None),
        Some(v) => serde_json::value::to_raw_value(v).map(Some),
    }
}

/// Extract a human-readable detail from a failure diagnostic, preferring the structured
/// `detail`/`message` fields and falling back to the raw JSON (mirrors Go
/// `diagnosticDetail`).
fn diagnostic_detail(raw: Option<&RawValue>) -> String {
    let Some(raw) = raw else { return String::new() };
    #[derive(serde::Deserialize)]
    struct Diagnostic {
        detail: Option<String>,
        message: Option<String>,
    }
    if let Ok(d) = serde_json::from_str::<Diagnostic>(raw.get()) {
        if let Some(detail) = d.detail.filter(|s| !s.is_empty()) {
            return detail;
        }
        if let Some(message) = d.message.filter(|s| !s.is_empty()) {
            return message;
        }
    }
    raw.get().to_owned()
}
