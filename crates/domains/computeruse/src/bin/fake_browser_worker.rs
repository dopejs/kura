//! A protocol-conformant browser worker with no browser: answers the
//! stdio line-JSON protocol of `SubprocessDriver` with `MemoryDriver`
//! semantics. Used by the driver tests so the supervision path (spawn,
//! timeout, crash, restart, capture decoding) is proven without Playwright.
//!
//! Knobs (environment): `FAKE_WORKER_EXIT_AFTER=n` exits after answering n
//! requests; `FAKE_WORKER_HANG_ON=n` never answers request n;
//! `FAKE_WORKER_GARBAGE_ON=n` answers request n with a non-JSON line.

use std::io::{BufRead, Write};

use kura_computeruse::{
    Action, CreateSessionInput, Driver, MemoryDriver, Session, subprocess_driver::encode_base64,
};
use serde_json::{Value, json};

fn main() {
    let driver = MemoryDriver::new();
    let exit_after: Option<u64> = std::env::var("FAKE_WORKER_EXIT_AFTER")
        .ok()
        .and_then(|v| v.parse().ok());
    let hang_on: Option<u64> = std::env::var("FAKE_WORKER_HANG_ON")
        .ok()
        .and_then(|v| v.parse().ok());
    let garbage_on: Option<u64> = std::env::var("FAKE_WORKER_GARBAGE_ON")
        .ok()
        .and_then(|v| v.parse().ok());
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut answered: u64 = 0;
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
        let id = request.get("id").and_then(Value::as_u64).unwrap_or(0);
        let seq = answered + 1;
        if hang_on == Some(seq) {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
        if garbage_on == Some(seq) {
            let _ = writeln!(stdout, "this is not json");
            let _ = stdout.flush();
            answered += 1;
            continue;
        }
        let response = handle(&driver, id, &request);
        let _ = writeln!(stdout, "{response}");
        let _ = stdout.flush();
        answered += 1;
        if exit_after == Some(answered) {
            std::process::exit(0);
        }
    }
}

fn handle(driver: &MemoryDriver, id: u64, request: &Value) -> Value {
    let session: Session =
        match serde_json::from_value(request.get("session").cloned().unwrap_or(Value::Null)) {
            Ok(session) => session,
            Err(err) => {
                return json!({ "id": id, "ok": false, "error": format!("bad session: {err}") });
            }
        };
    match request.get("op").and_then(Value::as_str).unwrap_or("") {
        "start_session" => {
            let input: CreateSessionInput =
                serde_json::from_value(request.get("input").cloned().unwrap_or(Value::Null))
                    .unwrap_or_default();
            match driver.start_session(session, input) {
                Ok(session) => json!({ "id": id, "ok": true, "session": session }),
                Err(err) => json!({ "id": id, "ok": false, "error": err }),
            }
        }
        "execute_action" => {
            let action: Action = match serde_json::from_value(
                request.get("action").cloned().unwrap_or(Value::Null),
            ) {
                Ok(action) => action,
                Err(err) => {
                    return json!({ "id": id, "ok": false, "error": format!("bad action: {err}") });
                }
            };
            match driver.execute_action(session, action) {
                Ok((session, action, captures)) => {
                    let captures: Vec<Value> = captures
                        .into_iter()
                        .map(|c| {
                            json!({
                                "kind": c.kind,
                                "mimeType": c.mime_type,
                                "fileName": c.file_name,
                                "contentBase64": encode_base64(&c.content),
                            })
                        })
                        .collect();
                    json!({ "id": id, "ok": true, "session": session, "action": action, "captures": captures })
                }
                Err(err) => json!({ "id": id, "ok": false, "error": err }),
            }
        }
        "close_session" => match driver.close_session(session) {
            Ok(session) => json!({ "id": id, "ok": true, "session": session }),
            Err(err) => json!({ "id": id, "ok": false, "error": err }),
        },
        other => json!({ "id": id, "ok": false, "error": format!("unknown op: {other}") }),
    }
}
