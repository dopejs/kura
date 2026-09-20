//! Stage 9.4: the subprocess browser driver against the protocol-conformant
//! fake worker (`fake_browser_worker`), proving supervision without a
//! browser: round trip with capture decoding, crash → restart, hang →
//! timeout, garbage → failure report, and the driver behind the real
//! manager.

use std::sync::Arc;

use chrono::Utc;
use kura_computeruse::{
    Action, ActionKind, ActionStatus, ArtifactKind, CreateSessionInput, Driver, Session,
    SessionStatus, SubprocessDriver, SubprocessDriverConfig, WorkerSupervisor,
};
use parking_lot::Mutex;

#[derive(Default)]
struct Reports {
    healthy: Mutex<u64>,
    failures: Mutex<Vec<String>>,
}

impl WorkerSupervisor for Reports {
    fn report_healthy(&self, _capability_id: &str) {
        *self.healthy.lock() += 1;
    }
    fn report_failure(&self, _capability_id: &str, reason: &str) {
        self.failures.lock().push(reason.to_string());
    }
}

fn config(env: &[(&str, &str)]) -> SubprocessDriverConfig {
    SubprocessDriverConfig {
        command: env!("CARGO_BIN_EXE_fake_browser_worker").to_string(),
        args: Vec::new(),
        env: env
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect(),
        timeout_ms: 2_000,
        capability_id: "browser".to_string(),
    }
}

fn session() -> Session {
    let now = Utc::now();
    Session {
        computer_use_session_id: "cus_1".to_string(),
        run_id: "run_1".to_string(),
        status: SessionStatus::Starting,
        driver_kind: "browser".to_string(),
        started_at: now,
        updated_at: now,
        ..Session::default()
    }
}

fn action(kind: ActionKind, input: serde_json::Value) -> Action {
    let now = Utc::now();
    Action {
        computer_use_action_id: format!("cua_{}", kind.as_str()),
        computer_use_session_id: "cus_1".to_string(),
        run_id: "run_1".to_string(),
        action_kind: kind,
        status: ActionStatus::Requested,
        requested_at: now,
        updated_at: now,
        input: input.as_object().cloned().unwrap_or_default(),
        ..Action::default()
    }
}

#[test]
fn a_session_round_trips_through_the_worker_with_decoded_captures() {
    let reports = Arc::new(Reports::default());
    let driver = SubprocessDriver::new(config(&[]), Some(reports.clone()));

    let started = driver
        .start_session(
            session(),
            CreateSessionInput {
                initial_url: "https://example.org/start".to_string(),
                ..CreateSessionInput::default()
            },
        )
        .expect("start");
    assert_eq!(started.status, SessionStatus::Active);
    assert_eq!(
        started.current_page.as_ref().unwrap().url,
        "https://example.org/start"
    );

    let (after_nav, nav, captures) = driver
        .execute_action(
            started,
            action(
                ActionKind::Navigate,
                serde_json::json!({ "url": "https://example.org/next" }),
            ),
        )
        .expect("navigate");
    assert_eq!(nav.status, ActionStatus::Completed);
    assert_eq!(
        after_nav.current_page.as_ref().unwrap().url,
        "https://example.org/next"
    );
    assert!(captures.is_empty());

    let (after_shot, shot, captures) = driver
        .execute_action(
            after_nav,
            action(ActionKind::Screenshot, serde_json::json!({})),
        )
        .expect("screenshot");
    assert_eq!(shot.status, ActionStatus::Completed);
    assert_eq!(captures.len(), 1);
    assert_eq!(captures[0].kind, ArtifactKind::Screenshot);
    assert_eq!(
        captures[0].computer_use_action_id,
        shot.computer_use_action_id
    );
    let text = String::from_utf8(captures[0].content.clone()).expect("utf8");
    assert!(
        text.contains("https://example.org/next"),
        "capture decoded from base64: {text}"
    );
    assert_eq!(
        captures[0].estimated_byte_size,
        captures[0].content.len() as i64
    );

    let closed = driver.close_session(after_shot).expect("close");
    assert_eq!(closed.status, SessionStatus::Closed);
    assert_eq!(driver.restart_count(), 0);
    assert!(*reports.healthy.lock() >= 4);
    assert!(reports.failures.lock().is_empty());
}

#[test]
fn a_crashed_worker_is_reported_and_respawned_on_the_next_call() {
    let reports = Arc::new(Reports::default());
    let driver = SubprocessDriver::new(
        config(&[("FAKE_WORKER_EXIT_AFTER", "1")]),
        Some(reports.clone()),
    );
    let started = driver
        .start_session(session(), CreateSessionInput::default())
        .expect("first call answers, then the worker exits");
    let err = driver
        .execute_action(
            started.clone(),
            action(ActionKind::Wait, serde_json::json!({})),
        )
        .expect_err("dead worker");
    assert!(
        err.contains("exited") || err.contains("write failed"),
        "{err}"
    );
    assert_eq!(reports.failures.lock().len(), 1);
    // Next call respawns (the new worker also exits after one answer, which
    // is enough to prove the restart path).
    let (_, waited, _) = driver
        .execute_action(started, action(ActionKind::Wait, serde_json::json!({})))
        .expect("respawned worker answers");
    assert_eq!(waited.status, ActionStatus::Completed);
    assert_eq!(driver.restart_count(), 1);
}

#[test]
fn a_hung_worker_times_out_and_is_replaced() {
    let reports = Arc::new(Reports::default());
    let mut cfg = config(&[("FAKE_WORKER_HANG_ON", "1")]);
    cfg.timeout_ms = 300;
    let driver = SubprocessDriver::new(cfg, Some(reports.clone()));
    let err = driver
        .start_session(session(), CreateSessionInput::default())
        .expect_err("hang");
    assert!(err.contains("did not answer within 300ms"), "{err}");
    assert_eq!(reports.failures.lock().len(), 1);
    // The replacement hangs on *its* first request too, so a second timeout
    // is expected; what matters is that the driver did not stay wedged.
    let err = driver
        .start_session(session(), CreateSessionInput::default())
        .expect_err("replacement also hangs on request 1");
    assert!(err.contains("did not answer"), "{err}");
    assert_eq!(driver.restart_count(), 1);
}

#[test]
fn malformed_worker_output_is_a_failure_not_a_panic() {
    let reports = Arc::new(Reports::default());
    let driver = SubprocessDriver::new(
        config(&[("FAKE_WORKER_GARBAGE_ON", "1")]),
        Some(reports.clone()),
    );
    let err = driver
        .start_session(session(), CreateSessionInput::default())
        .expect_err("garbage");
    assert!(err.contains("malformed JSON"), "{err}");
    assert_eq!(reports.failures.lock()[0], err);
}

#[test]
fn a_missing_worker_binary_is_a_clear_error() {
    let mut cfg = config(&[]);
    cfg.command = "/nonexistent/kura-browser-worker".to_string();
    let driver = SubprocessDriver::new(cfg, None);
    let err = driver
        .start_session(session(), CreateSessionInput::default())
        .expect_err("missing");
    assert!(err.contains("spawn browser worker"), "{err}");
}
