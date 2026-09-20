//! Port of `daemon/internal/integrations/adapterref`: the reference integration
//! adapter skeleton (Roadmap 59). It speaks the capability RPC contract over stdio but
//! contains NO real provider: reads return empty deterministic payloads and mutations
//! return empty objects. Seeded failure modes let the conformance harness and
//! failure-isolation tests exercise the daemon's classification. Real providers replace
//! it in Roadmap 60/63.

use std::collections::HashSet;
use std::io::{BufReader, Read, Write};
use std::time::Duration;

use kura_adapterrpc::{
    CONTRACT_VERSION, Client, CodecError, FailureKind, Request, Response, Status, read_request,
    write_message,
};

/// Seeds a deterministic failure for testing/conformance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FailMode {
    #[default]
    None,
    /// Respond StatusFailure auth (confirmed non-commit).
    Auth,
    /// Emit a non-contract frame (undecodable response).
    Malformed,
    /// Sleep past any deadline before responding.
    Hang,
    /// Stop serving (process/stream dies) without responding.
    Crash,
}

impl FailMode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            FailMode::None => "",
            FailMode::Auth => "auth",
            FailMode::Malformed => "malformed",
            FailMode::Hang => "hang",
            FailMode::Crash => "crash",
        }
    }
}

impl From<&str> for FailMode {
    fn from(s: &str) -> Self {
        match s {
            "auth" => FailMode::Auth,
            "malformed" => FailMode::Malformed,
            "hang" => FailMode::Hang,
            "crash" => FailMode::Crash,
            _ => FailMode::None,
        }
    }
}

/// Configures the reference adapter.
#[derive(Debug, Clone, Default)]
pub struct Options {
    pub fail_mode: FailMode,
    /// Restricts `fail_mode` to these operation names; empty means every operation.
    pub fail_operations: HashSet<String>,
    /// Used with [`FailMode::Hang`]; defaults to 2s.
    pub hang_for: Duration,
    /// Override advertised contract version (for mismatch tests).
    pub contract_ver: String,
}

impl Options {
    fn fail_applies(&self, operation: &str) -> bool {
        if self.fail_mode == FailMode::None {
            return false;
        }
        self.fail_operations.is_empty() || self.fail_operations.contains(operation)
    }
}

/// Operations that return a JSON array; all other operations return a JSON object.
const ARRAY_OPS: [&str; 4] = [
    "ListEvents",
    "ListThreads",
    "ListDrafts",
    "ResolveAttachments",
];

/// Runs the stdio loop with default (no-failure) options.
pub fn serve(input: impl Read, output: impl Write) -> Result<(), CodecError> {
    serve_with_options(input, output, Options::default())
}

/// Runs the stdio request/response loop honoring the seeded failure mode.
pub fn serve_with_options(
    input: impl Read,
    mut output: impl Write,
    opts: Options,
) -> Result<(), CodecError> {
    let mut reader = BufReader::new(input);
    loop {
        let req = match read_request(&mut reader) {
            Ok(req) => req,
            Err(CodecError::Io(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Ok(());
            }
            Err(e) => return Err(e),
        };

        let apply = opts.fail_applies(&req.operation);
        if apply {
            match opts.fail_mode {
                FailMode::Crash => return Ok(()),
                FailMode::Hang => {
                    let d = if opts.hang_for.is_zero() {
                        Duration::from_secs(2)
                    } else {
                        opts.hang_for
                    };
                    std::thread::sleep(d);
                }
                FailMode::Malformed => {
                    output.write_all(b"this-is-not-json\n")?;
                    continue;
                }
                _ => {}
            }
        }

        write_message(&mut output, &handle_internal(&req, &opts, apply))?;
    }
}

/// Produces a deterministic, contract-valid OK response for a request. Exposed for tests
/// that drive a single request.
#[must_use]
pub fn handle(req: Request) -> Response {
    handle_internal(&req, &Options::default(), false)
}

fn handle_internal(req: &Request, opts: &Options, apply: bool) -> Response {
    let version = if opts.contract_ver.is_empty() {
        CONTRACT_VERSION.to_string()
    } else {
        opts.contract_ver.clone()
    };
    if apply && opts.fail_mode == FailMode::Auth {
        return Response {
            request_id: req.request_id.clone(),
            contract_version: version,
            status: Status::Failure,
            failure_kind: Some(FailureKind::Auth),
            payload: None,
            diagnostic: None,
        };
    }
    let payload = if ARRAY_OPS.contains(&req.operation.as_str()) {
        raw_value("[]")
    } else {
        raw_value("{}")
    };
    Response {
        request_id: req.request_id.clone(),
        contract_version: version,
        status: Status::Ok,
        failure_kind: None,
        payload,
        diagnostic: None,
    }
}

/// Parses a compile-time JSON literal into a raw value. The literals are statically
/// valid, so failure is impossible; this keeps the infallible `handle` signature.
fn raw_value(literal: &str) -> Option<Box<serde_json::value::RawValue>> {
    serde_json::value::RawValue::from_string(literal.to_string()).ok()
}

/// Runs the reference adapter in-process over unix stream pairs and returns a connected
/// client. Dropping the client closes the daemon-side stream, which ends the adapter loop
/// (mirrors a real process exit).
#[must_use]
pub fn new_pipe_client() -> Client {
    new_pipe_client_with_options(Options::default())
}

/// [`new_pipe_client`] with seeded failure/version options.
#[must_use]
pub fn new_pipe_client_with_options(opts: Options) -> Client {
    let (adapter_reader, daemon_writer) =
        std::os::unix::net::UnixStream::pair().expect("unix pair");
    let (daemon_reader, adapter_writer) =
        std::os::unix::net::UnixStream::pair().expect("unix pair");
    std::thread::spawn(move || {
        let _ = serve_with_options(adapter_reader, adapter_writer, opts);
    });
    Client::new(daemon_writer, daemon_reader)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_does_not_leak_credential() {
        let req = Request {
            request_id: "r1".to_string(),
            contract_version: CONTRACT_VERSION.to_string(),
            domain: "calendar".to_string(),
            operation: "ProjectAccount".to_string(),
            deadline_ms: 0,
            resource: None,
            credential: Some(
                serde_json::value::to_raw_value(&serde_json::json!("top-secret-material"))
                    .expect("json"),
            ),
            payload: None,
        };
        let resp = handle(req);
        let blob = serde_json::to_string(&resp).expect("serialize");
        assert!(
            !blob.contains("top-secret-material"),
            "reference adapter response leaked credential material: {blob}"
        );
    }
}
