//! Behavioral tests ported from the Go adapterrpc package (transport_test.go,
//! conformance_test.go, failure_test.go, isolation_test.go, single_ledger_test.go,
//! credentials_test.go). The reference-adapter replica below mirrors
//! `daemon/internal/integrations/adapterref` (not yet ported) closely enough to exercise
//! the daemon-side classification.

use std::io::BufReader;
use std::os::unix::net::UnixStream;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use kura_adapterrpc::{
    AdapterError, CONTRACT_VERSION, Client, Error, FailureKind, Request, Response, Status,
    is_ambiguous, run_conformance, scoped_resolver, write_message,
};
use serde_json::value::RawValue;

// ---------------------------------------------------------------------------
// Test plumbing
// ---------------------------------------------------------------------------

fn stream_pair() -> (UnixStream, UnixStream) {
    UnixStream::pair().expect("unix stream pair")
}

fn raw(v: &str) -> Box<RawValue> {
    RawValue::from_string(v.to_owned()).expect("valid raw json")
}

/// Run a minimal responder over pipes, returning the response produced by `respond` for
/// each request (Go `inlineAdapter`).
fn inline_adapter<F>(respond: F) -> Client
where
    F: Fn(Request) -> Response + Send + 'static,
{
    let (adapter_reader, daemon_writer) = stream_pair(); // daemon -> adapter
    let (daemon_reader, adapter_writer) = stream_pair(); // adapter -> daemon
    thread::spawn(move || {
        let mut br = BufReader::new(adapter_reader);
        let mut w = adapter_writer;
        loop {
            match kura_adapterrpc::read_request(&mut br) {
                Ok(req) => {
                    if write_message(&mut w, &respond(req)).is_err() {
                        return;
                    }
                }
                Err(_) => return,
            }
        }
    });
    Client::new(daemon_writer, daemon_reader)
}

fn ok_response(req: &Request, payload: &str) -> Response {
    Response {
        request_id: req.request_id.clone(),
        contract_version: CONTRACT_VERSION.to_owned(),
        status: Status::Ok,
        failure_kind: None,
        payload: Some(raw(payload)),
        diagnostic: None,
    }
}

// ---------------------------------------------------------------------------
// Reference-adapter replica (adapterref.ServeWithOptions)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum FailMode {
    Auth,      // respond StatusFailure auth (confirmed non-commit)
    Malformed, // emit a non-contract frame (undecodable response)
    Hang,      // sleep past any deadline before responding
    Crash,     // stop serving (stream dies) without responding
}

#[derive(Default)]
struct RefOptions {
    fail_mode: Option<FailMode>,
    hang_for: Option<Duration>, // used with Hang; Go defaults to 2s
    contract_ver: Option<String>,
}

const ARRAY_OPS: &[&str] = &[
    "ListEvents",
    "ListThreads",
    "ListDrafts",
    "ResolveAttachments",
];

fn serve_reference(reader: UnixStream, writer: UnixStream, opts: RefOptions) {
    let mut br = BufReader::new(reader);
    let mut w = writer;
    loop {
        let req = match kura_adapterrpc::read_request(&mut br) {
            Ok(req) => req,
            Err(_) => return, // EOF: daemon closed the stream
        };
        let apply = opts.fail_mode.is_some();
        if apply {
            match opts.fail_mode.expect("checked") {
                FailMode::Crash => {
                    // Simulate the adapter dying mid-operation: stop without responding.
                    // Dropping `w` closes the stream so the daemon observes EOF.
                    return;
                }
                FailMode::Hang => {
                    thread::sleep(opts.hang_for.unwrap_or(Duration::from_secs(2)));
                }
                FailMode::Malformed => {
                    use std::io::Write as _;
                    if w.write_all(b"this-is-not-json\n").is_err() {
                        return;
                    }
                    continue;
                }
                _ => {}
            }
        }
        let version = opts
            .contract_ver
            .clone()
            .unwrap_or_else(|| CONTRACT_VERSION.to_owned());
        let resp = if apply && opts.fail_mode == Some(FailMode::Auth) {
            Response {
                request_id: req.request_id.clone(),
                contract_version: version,
                status: Status::Failure,
                failure_kind: Some(FailureKind::Auth),
                payload: None,
                diagnostic: None,
            }
        } else {
            let payload = if ARRAY_OPS.contains(&req.operation.as_str()) {
                "[]"
            } else {
                "{}"
            };
            Response {
                request_id: req.request_id.clone(),
                contract_version: version,
                status: Status::Ok,
                failure_kind: None,
                payload: Some(raw(payload)),
                diagnostic: None,
            }
        };
        if write_message(&mut w, &resp).is_err() {
            return;
        }
    }
}

/// Run the reference adapter in-process over pipes and return a connected client
/// (Go `adapterref.NewPipeClientWithOptions`).
fn pipe_client(opts: RefOptions) -> Client {
    let (adapter_reader, daemon_writer) = stream_pair();
    let (daemon_reader, adapter_writer) = stream_pair();
    thread::spawn(move || serve_reference(adapter_reader, adapter_writer, opts));
    Client::new(daemon_writer, daemon_reader)
}

fn dispatch_no_out(client: &Client, domain: &str, operation: &str) -> Result<(), Error> {
    client.dispatch::<serde_json::Value, serde_json::Value, serde_json::Value>(
        domain, operation, None, None, None,
    )
}

// ---------------------------------------------------------------------------
// transport_test.go
// ---------------------------------------------------------------------------

#[test]
fn dispatch_round_trip() {
    let c = inline_adapter(|req| {
        assert_eq!(req.domain, "calendar");
        assert_eq!(req.operation, "ProjectAccount");
        ok_response(&req, r#"{"integrationId":"int-1"}"#)
    });

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Out {
        integration_id: String,
    }
    let mut out = Out {
        integration_id: String::new(),
    };
    let payload = serde_json::json!({"x": 1});
    c.dispatch::<serde_json::Value, _, _>(
        "calendar",
        "ProjectAccount",
        None,
        Some(&payload),
        Some(&mut out),
    )
    .expect("dispatch");
    assert_eq!(out.integration_id, "int-1", "payload not decoded");
}

#[test]
fn dispatch_contract_mismatch() {
    let c = inline_adapter(|req| {
        let mut r = ok_response(&req, "{}");
        r.contract_version = "999".to_owned();
        r
    });
    let err = dispatch_no_out(&c, "calendar", "ProjectAccount").unwrap_err();
    assert!(
        matches!(err, Error::ContractMismatch),
        "want ContractMismatch, got {err}"
    );
}

#[test]
fn dispatch_failure_mapped() {
    let c = inline_adapter(|req| Response {
        request_id: req.request_id.clone(),
        contract_version: CONTRACT_VERSION.to_owned(),
        status: Status::Failure,
        failure_kind: Some(FailureKind::Auth),
        payload: None,
        diagnostic: None,
    });
    let err = dispatch_no_out(&c, "mail", "SendMessage").unwrap_err();
    match err {
        Error::Adapter(ae) => assert_eq!(ae.kind, FailureKind::Auth),
        other => panic!("want AdapterError auth, got {other}"),
    }
}

#[test]
fn dispatch_deadline() {
    let c = inline_adapter(|req| {
        thread::sleep(Duration::from_millis(200));
        ok_response(&req, "{}")
    });
    let err = c
        .dispatch_with_timeout::<serde_json::Value, serde_json::Value, serde_json::Value>(
            Duration::from_millis(20),
            "calendar",
            "ListEvents",
            None,
            None,
            None,
        )
        .unwrap_err();
    assert!(
        is_ambiguous(&err),
        "want ambiguous deadline error, got {err}"
    );
}

// ---------------------------------------------------------------------------
// conformance_test.go (US4, FR-008, FR-009)
// ---------------------------------------------------------------------------

#[test]
fn conformance_passes_reference_adapter() {
    let client = pipe_client(RefOptions::default());
    let report = run_conformance(&client);
    assert!(
        report.passed(),
        "reference adapter failed conformance: ready={:?} results={:?}",
        report.ready_err,
        report.results
    );
}

#[test]
fn conformance_refuses_version_mismatch() {
    let client = pipe_client(RefOptions {
        contract_ver: Some("999".to_owned()),
        ..Default::default()
    });
    let report = run_conformance(&client);
    assert!(
        matches!(report.ready_err, Some(Error::ContractMismatch)),
        "want ContractMismatch at readiness, got {:?}",
        report.ready_err
    );
    assert!(
        !report.passed(),
        "conformance must fail on version mismatch"
    );
}

#[test]
fn conformance_fails_contract_violating_adapter() {
    let client = pipe_client(RefOptions {
        fail_mode: Some(FailMode::Malformed),
        ..Default::default()
    });
    let report = run_conformance(&client);
    assert!(
        !report.passed(),
        "conformance must fail for a contract-violating adapter"
    );
}

// ---------------------------------------------------------------------------
// failure_test.go (US2, FR-007a/b)
// ---------------------------------------------------------------------------

#[test]
fn crash_is_ambiguous_unreachable() {
    let client = pipe_client(RefOptions {
        fail_mode: Some(FailMode::Crash),
        ..Default::default()
    });
    let payload = serde_json::json!({});
    let err = client
        .dispatch::<serde_json::Value, _, serde_json::Value>(
            "calendar",
            "CreateEvent",
            None,
            Some(&payload),
            None,
        )
        .unwrap_err();
    assert!(is_ambiguous(&err), "crash: want ambiguous error, got {err}");
}

#[test]
fn malformed_response_is_ambiguous() {
    let client = pipe_client(RefOptions {
        fail_mode: Some(FailMode::Malformed),
        ..Default::default()
    });
    let payload = serde_json::json!({});
    let err = client
        .dispatch::<serde_json::Value, _, serde_json::Value>(
            "calendar",
            "CreateEvent",
            None,
            Some(&payload),
            None,
        )
        .unwrap_err();
    assert!(
        is_ambiguous(&err),
        "malformed: want ambiguous error, got {err}"
    );
}

#[test]
fn hang_beyond_deadline_is_ambiguous() {
    let client = pipe_client(RefOptions {
        fail_mode: Some(FailMode::Hang),
        hang_for: Some(Duration::from_secs(1)),
        ..Default::default()
    });
    let payload = serde_json::json!({});
    let err = client
        .dispatch_with_timeout::<serde_json::Value, _, serde_json::Value>(
            Duration::from_millis(20),
            "calendar",
            "CreateEvent",
            None,
            Some(&payload),
            None,
        )
        .unwrap_err();
    assert!(is_ambiguous(&err), "hang: want ambiguous error, got {err}");
}

#[test]
fn auth_failure_is_confirmed_not_ambiguous() {
    let client = pipe_client(RefOptions {
        fail_mode: Some(FailMode::Auth),
        ..Default::default()
    });
    let payload = serde_json::json!({});
    let err = client
        .dispatch::<serde_json::Value, _, serde_json::Value>(
            "mail",
            "SendMessage",
            None,
            Some(&payload),
            None,
        )
        .unwrap_err();
    assert!(
        !is_ambiguous(&err),
        "auth failure must be confirmed (not ambiguous), got {err}"
    );
    match err {
        Error::Adapter(ae) => assert_eq!(ae.kind, FailureKind::Auth, "want AdapterError auth"),
        other => panic!("want AdapterError auth, got {other}"),
    }
}

#[test]
fn dispatch_without_caller_deadline_is_still_bounded() {
    // A caller that supplies no deadline must still be bounded by the client default, so a
    // hung adapter cannot block forever.
    let client = pipe_client(RefOptions {
        fail_mode: Some(FailMode::Hang),
        hang_for: Some(Duration::from_secs(3)),
        ..Default::default()
    })
    .with_default_deadline(Duration::from_millis(40));

    let start = Instant::now();
    let err = dispatch_no_out(&client, "calendar", "CreateEvent").unwrap_err();
    assert!(
        is_ambiguous(&err),
        "want ambiguous (timeout) error, got {err}"
    );
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "dispatch was not bounded by the default deadline (hung)"
    );
}

#[test]
fn transport_poisoned_after_timeout_fails_fast() {
    let client = pipe_client(RefOptions {
        fail_mode: Some(FailMode::Hang),
        hang_for: Some(Duration::from_secs(3)),
        ..Default::default()
    })
    .with_default_deadline(Duration::from_millis(40));

    let err = dispatch_no_out(&client, "calendar", "CreateEvent").unwrap_err();
    assert!(
        is_ambiguous(&err),
        "first call: want ambiguous timeout, got {err}"
    );

    // Subsequent call must fail fast (broken transport), not spawn a competing reader.
    let start = Instant::now();
    let err = dispatch_no_out(&client, "calendar", "GetEvent").unwrap_err();
    assert!(
        matches!(err, Error::Unreachable),
        "second call: want Unreachable (poisoned), got {err}"
    );
    assert!(
        start.elapsed() <= Duration::from_millis(500),
        "poisoned transport did not fail fast"
    );
}

// ---------------------------------------------------------------------------
// isolation_test.go (US3 / FR-015)
// ---------------------------------------------------------------------------

#[test]
fn concurrent_calls_are_isolated_per_call() {
    // The echo adapter returns each request's credential as its payload; every concurrent
    // caller must get back its own.
    let c = inline_adapter(|req| Response {
        request_id: req.request_id.clone(),
        contract_version: CONTRACT_VERSION.to_owned(),
        status: Status::Ok,
        failure_kind: None,
        payload: req.credential.clone(),
        diagnostic: None,
    })
    .with_credentials(scoped_resolver(Some(Box::new(|integration_id: &str| {
        Ok(Some(raw(&format!("\"{integration_id}\""))))
    }))));

    const N: usize = 25;
    let c = std::sync::Arc::new(c);
    let (tx, rx) = mpsc::channel();
    let mut handles = Vec::new();
    for i in 0..N {
        let c = std::sync::Arc::clone(&c);
        let tx = tx.clone();
        handles.push(thread::spawn(move || {
            let id = format!("int-{i}");
            let mut got = String::new();
            let resource = serde_json::json!({"integrationId": id});
            if let Err(err) = c.dispatch::<_, serde_json::Value, _>(
                "calendar",
                "ProjectAccount",
                Some(&resource),
                None,
                Some(&mut got),
            ) {
                tx.send(format!("{id}: {err}")).unwrap();
                return;
            }
            if got != id {
                tx.send(format!("cross-bleed: caller {id} received {got:?}"))
                    .unwrap();
            }
        }));
    }
    drop(tx);
    for h in handles {
        h.join().expect("worker panicked");
    }
    let errs: Vec<String> = rx.iter().collect();
    assert!(errs.is_empty(), "isolation failures: {errs:?}");
}

// ---------------------------------------------------------------------------
// single_ledger_test.go (US6 / FR-003 / FR-012)
// ---------------------------------------------------------------------------

#[test]
fn envelopes_carry_no_ledger_state() {
    // "operation" is intentionally NOT forbidden: Request.operation is the op name, not
    // ledger state. These tokens denote daemon-owned ledger/evidence state an adapter must
    // never carry.
    let forbidden = ["ledger", "idempotency", "evidence", "artifact", "commit"];

    let req = Request {
        request_id: "req-1".to_owned(),
        contract_version: CONTRACT_VERSION.to_owned(),
        domain: "calendar".to_owned(),
        operation: "CreateEvent".to_owned(),
        deadline_ms: 30_000,
        resource: Some(raw("{}")),
        credential: Some(raw("{}")),
        payload: Some(raw("{}")),
    };
    let resp = Response {
        request_id: "req-1".to_owned(),
        contract_version: CONTRACT_VERSION.to_owned(),
        status: Status::Failure,
        failure_kind: Some(FailureKind::Auth),
        payload: Some(raw("{}")),
        diagnostic: Some(raw("{}")),
    };

    for (name, value) in [
        ("Request", serde_json::to_value(&req).unwrap()),
        ("Response", serde_json::to_value(&resp).unwrap()),
    ] {
        let obj = value.as_object().expect("envelope serializes to object");
        for key in obj.keys() {
            let lower = key.to_lowercase();
            for f in forbidden {
                assert!(
                    !lower.contains(f),
                    "{name} has forbidden ledger-state field {key:?} (adapters must not own the ledger)"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// credentials_test.go
// ---------------------------------------------------------------------------

/// Record the credential each request carried and reply OK (Go `captureAdapter`).
fn capture_adapter(creds: mpsc::Sender<String>) -> Client {
    inline_adapter(move |req| {
        creds
            .send(
                req.credential
                    .as_deref()
                    .map(RawValue::get)
                    .unwrap_or("")
                    .to_owned(),
            )
            .expect("record credential");
        ok_response(&req, "{}")
    })
}

#[test]
fn credentials_injected_per_call_scoped_to_integration() {
    let (tx, rx) = mpsc::channel();
    let c = capture_adapter(tx).with_credentials(scoped_resolver(Some(Box::new(
        |integration_id: &str| -> Result<Option<Box<RawValue>>, kura_adapterrpc::ResolverError> {
            Ok(Some(raw(&format!("\"secret-for-{integration_id}\""))))
        },
    ))));

    for id in ["int-1", "int-2"] {
        let resource = serde_json::json!({"integrationId": id});
        c.dispatch::<_, serde_json::Value, serde_json::Value>(
            "calendar",
            "ProjectAccount",
            Some(&resource),
            None,
            None,
        )
        .unwrap_or_else(|e| panic!("dispatch {id}: {e}"));
    }
    let got: Vec<String> = [rx.recv().unwrap(), rx.recv().unwrap()].into();
    assert_eq!(
        got,
        vec![
            "\"secret-for-int-1\"".to_owned(),
            "\"secret-for-int-2\"".to_owned()
        ],
        "credentials not scoped per call"
    );
}

#[test]
fn credential_resolver_fails_closed() {
    let (tx, rx) = mpsc::channel();
    let c = capture_adapter(tx).with_credentials(scoped_resolver(Some(Box::new(
        |_integration_id: &str| {
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "secret unavailable").into())
        },
    ))));

    let resource = serde_json::json!({"integrationId": "int-1"});
    let err = c
        .dispatch::<_, serde_json::Value, serde_json::Value>(
            "calendar",
            "CreateEvent",
            Some(&resource),
            None,
            None,
        )
        .unwrap_err();
    assert!(
        err.to_string().contains("credentials"),
        "want fail-closed credential error, got {err}"
    );
    assert!(
        rx.try_recv().is_err(),
        "operation dispatched despite credential failure"
    );
}

// ---------------------------------------------------------------------------
// AdapterError formatting (Go AdapterError.Error)
// ---------------------------------------------------------------------------

#[test]
fn adapter_error_message_format() {
    let with_detail = AdapterError::new(FailureKind::RateLimited, "quota exhausted");
    assert_eq!(
        with_detail.to_string(),
        "integration adapter failure (rate_limited): quota exhausted"
    );
    let bare = AdapterError::new(FailureKind::Auth, "");
    assert_eq!(bare.to_string(), "integration adapter failure (auth)");
}
