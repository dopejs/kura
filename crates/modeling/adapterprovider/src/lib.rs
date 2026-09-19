//! Port of `daemon/internal/integrations/adapterprovider`: the adapter side of the
//! capability RPC contract run against a real provider Handler, replacing the
//! empty-payload reference skeleton for real providers (Roadmap 60/63). It owns the stdio
//! loop, the contract-version readiness handshake, per-call deadline derivation, and
//! failure/diagnostic shaping. It performs provider request/response mapping only: it
//! never records ledger, idempotency, or side-effect state, and it MUST NOT emit
//! credential or token material in any payload or diagnostic.

use std::io::{BufReader, Read, Write};
use std::time::Duration;

use kura_adapterrpc::{
    CONTRACT_VERSION, CodecError, FailureKind, Request, Response, Status, read_request,
    write_message,
};
use serde_json::value::RawValue;

/// Maps one capability RPC operation to a result payload. Returning a
/// [`HandlerError::Fault`] yields a `StatusFailure` response carrying the typed failure
/// kind and a redacted diagnostic; any other non-fault error yields a `StatusFailure`
/// internal. The returned payload and any fault MUST be free of secret material.
pub trait Handler: Send + Sync {
    fn handle(
        &self,
        op: Operation,
        deadline: Option<Duration>,
    ) -> Result<Option<Box<RawValue>>, HandlerError>;
}

/// The decoded operation request handed to a [`Handler`].
#[derive(Debug, Clone)]
pub struct Operation {
    pub domain: String,
    pub operation: String,
    pub resource: Option<Box<RawValue>>,
    pub credential: Option<Box<RawValue>>,
    pub payload: Option<Box<RawValue>>,
}

/// Error a [`Handler`] returns. Mirrors the Go `(json.RawMessage, error)` contract: an
/// ambiguous outcome, a typed [`Fault`], or any other error (mapped to internal).
#[derive(Debug)]
pub enum HandlerError {
    /// Unconfirmed write outcome (FR-008): conveyed to the daemon over the contract's
    /// undecodable-response channel, which the daemon classifies as ambiguous-commit.
    Ambiguous,
    /// A confirmed provider failure.
    Fault(Fault),
    /// Any other error; yields a `StatusFailure` internal.
    Other(Box<dyn std::error::Error + Send + Sync>),
}

/// A confirmed provider failure. `kind` classifies it for daemon diagnostics and
/// live-validation; `code` is a stable, redacted token surfaced as the diagnostic detail.
#[derive(Debug, Clone)]
pub struct Fault {
    pub kind: FailureKind,
    pub code: String,
    pub message: String,
}

impl Fault {
    #[must_use]
    pub fn new(kind: FailureKind, code: impl Into<String>) -> Self {
        Fault {
            kind,
            code: code.into(),
            message: String::new(),
        }
    }
}

impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.message.is_empty() {
            f.write_str(&self.code)
        } else {
            f.write_str(&self.message)
        }
    }
}

impl std::error::Error for Fault {}

/// Caps the per-call deadline the adapter derives from the request so a missing/oversized
/// `deadlineMs` cannot let a provider call run unbounded.
const MAX_HANDLER_DEADLINE: Duration = Duration::from_secs(120);

/// Runs the request/response loop against `h` until EOF.
pub fn serve(input: impl Read, output: impl Write, h: &dyn Handler) -> Result<(), CodecError> {
    let mut reader = BufReader::new(input);
    let mut output = output;
    loop {
        let req = match read_request(&mut reader) {
            Ok(req) => req,
            Err(CodecError::Io(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        match dispatch(&req, h) {
            Some(resp) => write_message(&mut output, &resp)?,
            None => output.write_all(b"ambiguous-commit-unconfirmed\n")?,
        }
    }
}

/// Dispatches one request to the handler. `None` means an ambiguous outcome, which the
/// caller conveys over the undecodable-response channel.
fn dispatch(req: &Request, h: &dyn Handler) -> Option<Response> {
    // The contract-version readiness handshake is answered locally; it carries no
    // provider work.
    if req.domain == "capability" && req.operation == "Ready" {
        return Some(ok_response(req, None));
    }

    let deadline = if req.deadline_ms > 0 {
        Some(Duration::from_millis(req.deadline_ms as u64).min(MAX_HANDLER_DEADLINE))
    } else {
        None
    };

    let op = Operation {
        domain: req.domain.clone(),
        operation: req.operation.clone(),
        resource: req.resource.clone(),
        credential: req.credential.clone(),
        payload: req.payload.clone(),
    };

    match h.handle(op, deadline) {
        Ok(payload) => Some(ok_response(req, payload)),
        Err(HandlerError::Ambiguous) => None,
        Err(HandlerError::Fault(fault)) => Some(failure_response(req, Some(&fault))),
        Err(HandlerError::Other(_)) => Some(failure_response(req, None)),
    }
}

fn ok_response(req: &Request, payload: Option<Box<RawValue>>) -> Response {
    Response {
        request_id: req.request_id.clone(),
        contract_version: CONTRACT_VERSION.to_string(),
        status: Status::Ok,
        failure_kind: None,
        payload,
        diagnostic: None,
    }
}

fn failure_response(req: &Request, fault: Option<&Fault>) -> Response {
    let (kind, code, message) = match fault {
        Some(f) => {
            let code = if f.code.is_empty() {
                "provider_internal_error".to_string()
            } else {
                f.code.clone()
            };
            (f.kind.clone(), code, f.message.clone())
        }
        None => (
            FailureKind::Internal,
            "provider_internal_error".to_string(),
            String::new(),
        ),
    };
    let diagnostic = serde_json::value::to_raw_value(&serde_json::json!({
        "detail": code,
        "message": message,
    }))
    .ok();
    Response {
        request_id: req.request_id.clone(),
        contract_version: CONTRACT_VERSION.to_string(),
        status: Status::Failure,
        failure_kind: Some(kind),
        payload: None,
        diagnostic,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;
    use std::thread;

    use kura_adapterrpc::{Client, Error, is_ambiguous};

    struct HandlerFn(
        Box<
            dyn Fn(Operation, Option<Duration>) -> Result<Option<Box<RawValue>>, HandlerError>
                + Send
                + Sync,
        >,
    );

    impl Handler for HandlerFn {
        fn handle(
            &self,
            op: Operation,
            deadline: Option<Duration>,
        ) -> Result<Option<Box<RawValue>>, HandlerError> {
            (self.0)(op, deadline)
        }
    }

    fn raw(payload: &str) -> Option<Box<RawValue>> {
        RawValue::from_string(payload.to_string()).ok()
    }

    /// Wires a [`Handler`] behind [`serve`] over unix stream pairs and returns a
    /// connected RPC client.
    fn pipe_client(h: HandlerFn) -> Client {
        let (adapter_reader, daemon_writer) = UnixStream::pair().expect("pair");
        let (daemon_reader, adapter_writer) = UnixStream::pair().expect("pair");
        thread::spawn(move || {
            let _ = serve(adapter_reader, adapter_writer, &h);
        });
        Client::new(daemon_writer, daemon_reader)
    }

    #[test]
    fn ready_handshake_is_answered_locally() {
        let h = HandlerFn(Box::new(|_op, _deadline| {
            panic!("Ready must be answered locally, not dispatched to the handler");
        }));
        let client = pipe_client(h);
        client.ready().expect("ready");
    }

    #[test]
    fn serve_ok_and_fault() {
        let h = HandlerFn(Box::new(|op, _deadline| {
            if op.operation == "Fail" {
                Err(HandlerError::Fault(Fault {
                    kind: FailureKind::Scope,
                    code: "scope_not_granted".to_string(),
                    message: "denied".to_string(),
                }))
            } else {
                Ok(raw(r#"{"ok":true}"#))
            }
        }));
        let client = pipe_client(h);

        let mut out = serde_json::Value::Null;
        client
            .dispatch::<serde_json::Value, serde_json::Value, serde_json::Value>(
                "calendar",
                "Read",
                None,
                None,
                Some(&mut out),
            )
            .expect("dispatch ok");
        assert_eq!(out["ok"], true);

        let err = client
            .dispatch::<serde_json::Value, serde_json::Value, serde_json::Value>(
                "calendar", "Fail", None, None, None,
            )
            .unwrap_err();
        let Error::Adapter(ae) = err else {
            panic!("want AdapterError, got {err}");
        };
        assert_eq!(ae.kind, FailureKind::Scope);
        assert_eq!(ae.detail, "scope_not_granted");
    }

    #[test]
    fn serve_ambiguous_becomes_undecodable() {
        let h = HandlerFn(Box::new(|_op, _deadline| Err(HandlerError::Ambiguous)));
        let client = pipe_client(h);
        let err = client
            .dispatch::<serde_json::Value, serde_json::Value, serde_json::Value>(
                "calendar", "Write", None, None, None,
            )
            .unwrap_err();
        assert!(
            is_ambiguous(&err),
            "err = {err}, want ambiguous classification"
        );
    }
}
