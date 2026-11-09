//! The harness measured against a run whose answers are known.
//!
//! Everything real except the model and the computer: the daemon's own
//! assembly serves HTTP on a loopback port, the real `kura-device` agent and
//! link connect to it over a WebSocket after pairing with a code, and the
//! runner drives tasks through `/v1/chat/query` exactly as on a Mac. The
//! model is a script per task, behind an OpenAI-compatible endpoint; the
//! computer is a fake "editor" whose typed text is written to the opened
//! file on cmd+s. So a check passes only when the scripted actions really
//! reached the device, and the runner's numbers can be compared with what
//! the scripts did:
//!
//! - `pass-save` passes in 2 steps;
//! - `claims-but-fails` says it succeeded and fails the check;
//! - `runaway` would pass its check but overruns its step budget and is
//!   cancelled mid-turn;
//! - `risky-and-errors` raises an approval (rejected by the runner) and an
//!   invalid action, then passes: both errors are counted by code;
//! - `denied` is refused control by the person and is not counted;
//! - `already-open` is skipped by its precondition.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use image::{Rgba, RgbaImage};
use kura_cua_eval::client::Client;
use kura_cua_eval::desktop::Desktop;
use kura_cua_eval::report;
use kura_cua_eval::runner::{ApprovalMode, Outcome, RunConfig, Runner};
use kura_cua_eval::suite::Suite;
use kura_device::agent::{Activity, Agent, Policy, system_clock};
use kura_device::keys::Key;
use kura_device::link::{self, LinkConfig};
use kura_device::platform::{
    Answer, Consent, Display, Input, Performed, Platform, PlatformError, Stop, Window,
};
use kura_device::shot::Rect;
use parking_lot::Mutex;
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// The fake computer: one editor, shared by the device and the runner.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Desk {
    open: Option<PathBuf>,
    front: Option<String>,
    buffer: String,
    running: BTreeSet<String>,
    /// What the runner's teardown asked for.
    teardown: Vec<String>,
}

#[derive(Clone, Default)]
struct FakeComputer(Arc<Mutex<Desk>>);

impl Platform for FakeComputer {
    fn display(&self) -> Result<Display, PlatformError> {
        Ok(Display {
            pixels: (320, 200),
            scale: 1,
            bounds: Rect {
                x: 0.0,
                y: 0.0,
                width: 320.0,
                height: 200.0,
            },
        })
    }

    fn capture(&self) -> Result<RgbaImage, PlatformError> {
        Ok(RgbaImage::from_pixel(320, 200, Rgba([240, 240, 240, 255])))
    }

    fn frontmost_app(&self) -> Option<String> {
        None
    }

    fn windows(&self) -> Vec<Window> {
        Vec::new()
    }

    fn secure_input(&self) -> bool {
        false
    }

    fn perform(&self, input: &Input, _stop: Stop<'_>) -> Result<Performed, PlatformError> {
        let mut desk = self.0.lock();
        match input {
            Input::Type { text } => desk.buffer.push_str(text),
            Input::Key { chord } if chord.modifiers.cmd && chord.key == Key::Char('s') => {
                if let Some(path) = desk.open.clone() {
                    std::fs::write(path, &desk.buffer)
                        .map_err(|err| PlatformError(err.to_string()))?;
                }
            }
            _ => {}
        }
        Ok(Performed::Done)
    }
}

impl Desktop for FakeComputer {
    fn open(&self, path: &Path, app: Option<&str>) -> Result<(), String> {
        let mut desk = self.0.lock();
        desk.buffer = std::fs::read_to_string(path).map_err(|err| err.to_string())?;
        desk.open = Some(path.to_path_buf());
        desk.front = app.map(str::to_string);
        Ok(())
    }

    fn app_running(&self, app: &str) -> Result<bool, String> {
        Ok(self.0.lock().running.contains(app))
    }

    fn frontmost_app(&self) -> Result<Option<String>, String> {
        Ok(self.0.lock().front.clone())
    }

    fn quit_app(&self, app: &str) -> Result<(), String> {
        self.0.lock().teardown.push(format!("quit {app}"));
        Ok(())
    }

    fn close_documents(&self, app: &str, marker: &str) -> Result<(), String> {
        let mut desk = self.0.lock();
        if desk
            .open
            .as_ref()
            .is_some_and(|open| open.to_string_lossy().contains(marker))
        {
            desk.open = None;
            desk.front = None;
            desk.buffer.clear();
        }
        desk.teardown.push(format!("close {app} docs in {marker}"));
        Ok(())
    }

    fn close_finder_windows(&self, marker: &str) -> Result<(), String> {
        self.0
            .lock()
            .teardown
            .push(format!("close finder in {marker}"));
        Ok(())
    }
}

/// Allows every request for control, except one whose reason says not to.
struct Person;

impl Consent for Person {
    fn ask(&self, reason: &str, _ttl_s: u64, _timeout: Duration, _cancel: Stop<'_>) -> Answer {
        if reason.contains("deny-me") {
            Answer::Deny
        } else {
            Answer::Allow
        }
    }
}

// ---------------------------------------------------------------------------
// The scripted model, OpenAI chat-completions compatible.
// ---------------------------------------------------------------------------

struct Script {
    calls: Vec<(&'static str, Value)>,
    reply: &'static str,
    delay: Duration,
}

fn scripts() -> BTreeMap<&'static str, Script> {
    let start = |reason: &str| ("computer.start", json!({ "reason": reason }));
    let act = |args: Value| ("computer.act", args);
    let stop = ("computer.stop", json!({}));
    let mut map = BTreeMap::new();
    map.insert(
        "pass-save",
        Script {
            calls: vec![
                start("type into the note"),
                act(json!({"op": "type", "text": "hello world"})),
                act(json!({"op": "key", "keys": "cmd+s"})),
                stop.clone(),
            ],
            reply: "Saved.",
            delay: Duration::ZERO,
        },
    );
    map.insert(
        "claims-but-fails",
        Script {
            calls: vec![
                start("type into the note"),
                act(json!({"op": "type", "text": "goodbye"})),
                act(json!({"op": "key", "keys": "cmd+s"})),
                stop.clone(),
            ],
            reply: "I typed hello world and saved it.",
            delay: Duration::ZERO,
        },
    );
    let mut wander = vec![start("look around")];
    wander.extend((0..12).map(|_| act(json!({"op": "screenshot"}))));
    wander.push(stop.clone());
    map.insert(
        "runaway",
        Script {
            calls: wander,
            reply: "Done looking.",
            // Slow enough that the runner sees the overrun mid-turn.
            delay: Duration::from_millis(250),
        },
    );
    map.insert(
        "risky-and-errors",
        Script {
            calls: vec![
                start("type into the note"),
                act(json!({"op": "key", "keys": "cmd+q"})),
                act(json!({"op": "wait", "ms": 60000})),
                act(json!({"op": "type", "text": "hello world"})),
                act(json!({"op": "key", "keys": "cmd+s"})),
                stop.clone(),
            ],
            reply: "Saved.",
            delay: Duration::ZERO,
        },
    );
    map.insert(
        "denied",
        Script {
            calls: vec![start("deny-me: open an app")],
            reply: "They said no.",
            delay: Duration::ZERO,
        },
    );
    map
}

fn text_of(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

async fn completions(
    axum::extract::State(scripts): axum::extract::State<Arc<BTreeMap<&'static str, Script>>>,
    axum::Json(body): axum::Json<Value>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let messages = body["messages"].as_array().cloned().unwrap_or_default();
    let asked: String = messages
        .iter()
        .filter(|m| m["role"] == "user")
        .map(|m| text_of(&m["content"]))
        .collect::<Vec<_>>()
        .join("\n");
    let offered: Vec<String> = body["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter_map(|t| t["function"]["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let done = messages.iter().filter(|m| m["role"] == "tool").count();
    let script = scripts
        .iter()
        .find(|(name, _)| asked.contains(&format!("selftest:{name} ")))
        .map(|(_, script)| script);
    let (message, finish) = match script {
        Some(script) => {
            tokio::time::sleep(script.delay).await;
            match script.calls.get(done) {
                Some((name, args)) if offered.iter().any(|t| t == "computer_start") => (
                    json!({"role": "assistant", "content": null, "tool_calls": [{
                        "index": 0, "id": format!("call_{done}"), "type": "function",
                        "function": {"name": name.replace('.', "_"), "arguments": args.to_string()}
                    }]}),
                    "tool_calls",
                ),
                Some(_) => (
                    json!({"role": "assistant", "content": format!("no computer tools in {offered:?}")}),
                    "stop",
                ),
                None => (
                    json!({"role": "assistant", "content": script.reply}),
                    "stop",
                ),
            }
        }
        None => (
            json!({"role": "assistant", "content": "no script for this"}),
            "stop",
        ),
    };
    let usage = json!({"prompt_tokens": 10, "completion_tokens": 2, "total_tokens": 12});
    if body["stream"] == true {
        let chunks = [
            json!({"id": "r", "object": "chat.completion.chunk", "model": "fake",
                   "choices": [{"index": 0, "delta": message, "finish_reason": null}]}),
            json!({"id": "r", "object": "chat.completion.chunk", "model": "fake",
                   "choices": [{"index": 0, "delta": {}, "finish_reason": finish}], "usage": usage}),
        ];
        let mut sse: String = chunks.iter().map(|c| format!("data: {c}\n\n")).collect();
        sse.push_str("data: [DONE]\n\n");
        return ([("content-type", "text/event-stream")], sse).into_response();
    }
    axum::Json(json!({
        "id": "r", "object": "chat.completion", "model": "fake",
        "choices": [{"index": 0, "message": message, "finish_reason": finish}],
        "usage": usage
    }))
    .into_response()
}

async fn start_model() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind model");
    let addr = listener.local_addr().expect("addr");
    let router = axum::Router::new()
        .route("/v1/chat/completions", axum::routing::post(completions))
        .with_state(Arc::new(scripts()));
    tokio::spawn(async move { axum::serve(listener, router).await.expect("serve model") });
    format!("http://{addr}/v1")
}

// ---------------------------------------------------------------------------
// The daemon and the device.
// ---------------------------------------------------------------------------

/// The daemon's URL, and the app, which must outlive the test.
async fn start_daemon(data_dir: &Path, model_url: &str) -> (String, kura_app::App) {
    // As the operator configures a daemon for an evaluation: the tools on,
    // and enough rounds for a desktop task.
    std::fs::write(
        data_dir.join("plugins.json"),
        json!({"entries": {
            "computer-use": {"config": {"remote": {"enabled": true}}},
            "chat": {"config": {"toolMaxRounds": 40}}
        }})
        .to_string(),
    )
    .expect("plugins.json");
    let mut llm = kura_config::LlmConfig {
        default_provider: "openai_compatible".to_string(),
        default_model: "fake".to_string(),
        default_timeout_ms: 30_000,
        ..Default::default()
    };
    llm.openai_compatible.base_url = model_url.to_string();
    llm.openai_compatible.api_key = "test-key".to_string();
    llm.openai_compatible.model = "fake".to_string();
    let config = kura_config::Config {
        project_root: Default::default(),
        environment: kura_config::Environment::Embedded,
        bind_addr: "127.0.0.1:0".to_string(),
        data_dir: data_dir.to_string_lossy().into_owned(),
        log_level: "warn".to_string(),
        version: "dev".to_string(),
        llm,
        connectors: Default::default(),
        egress: Default::default(),
        store: Default::default(),
    };
    let app = kura_app::App::new(config).expect("build app");
    let router = app.router();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind daemon");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .expect("serve daemon");
    });
    (format!("http://{addr}"), app)
}

async fn connect_device(server: &str, client: &Client, computer: FakeComputer) {
    let code = client.device_pairing_code().await.expect("pairing code");
    let paired = kura_device::pair::pair(server, &code, "selftest-mac")
        .await
        .expect("pair");
    let agent = Arc::new(Agent::new(
        Arc::new(computer),
        Arc::new(Person),
        Arc::new(Activity::default()),
        Policy {
            settle: Duration::ZERO,
            ..Policy::default()
        },
        system_clock(),
    ));
    let url = link::connect_url(server).expect("url");
    tokio::spawn(async move {
        let (_kill_tx, mut kills) = tokio::sync::mpsc::channel(1);
        let (_stop_tx, mut shutdown) = tokio::sync::mpsc::channel(1);
        link::run(
            agent,
            url,
            paired.token,
            LinkConfig::default(),
            &mut kills,
            &mut shutdown,
        )
        .await
    });
    for _ in 0..200 {
        if client
            .devices()
            .await
            .is_ok_and(|devices| devices.iter().any(|d| d.online))
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the device never came online");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_runner_counts_what_really_happened() {
    let data = tempfile::tempdir().expect("data dir");
    let work = tempfile::tempdir().expect("work dir");
    let model_url = start_model().await;
    let (server, _app) = start_daemon(data.path(), &model_url).await;
    let token = Client::login_local(&server, "selftest")
        .await
        .expect("login");
    let client = Client::new(&server, &token).expect("client");

    let computer = FakeComputer::default();
    computer.0.lock().running.insert("Busy".into());
    connect_device(&server, &client, computer.clone()).await;

    let suite =
        Suite::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/selftest.v1.json"))
            .expect("suite");
    let work_root = work.path().join("cua-selftest-run");
    let runner = Runner {
        client: client.clone(),
        desktop: Arc::new(computer.clone()),
        config: RunConfig {
            run_id: "selftest".into(),
            work_root: work_root.clone(),
            provider: String::new(),
            model: String::new(),
            repeats: 1,
            only: Vec::new(),
            approvals: ApprovalMode::Reject,
            poll: Duration::from_millis(50),
            settle: Duration::ZERO,
            cancel_grace: Duration::from_secs(20),
        },
    };
    let run = runner.run(&suite).await;
    assert_eq!(run.aborted, "", "{run:#?}");
    let by_id: BTreeMap<&str, _> = run
        .attempts
        .iter()
        .map(|a| (a.task_id.as_str(), a))
        .collect();
    let summary_of = |id: &str| format!("{:#?}", by_id[id]);

    let pass = by_id["pass-save"];
    assert_eq!(pass.outcome, Outcome::Passed, "{}", summary_of("pass-save"));
    assert_eq!(pass.steps, 2);
    assert_eq!(
        pass.ops,
        BTreeMap::from([("key".to_string(), 1), ("type".to_string(), 1)])
    );
    assert!(pass.control_granted && pass.errors.is_empty() && pass.approvals.is_empty());
    assert!(
        pass.screenshot_bytes.is_some_and(|bytes| bytes > 0),
        "{:?}",
        pass.screenshot_bytes
    );
    assert_eq!(pass.turn.status, "completed");
    assert_eq!(pass.turn.reply_excerpt, "Saved.");

    let claim = by_id["claims-but-fails"];
    assert_eq!(
        claim.outcome,
        Outcome::CheckFailed,
        "{}",
        summary_of("claims-but-fails")
    );
    assert_eq!(claim.steps, 2);
    assert!(
        claim.turn.reply_excerpt.contains("hello world"),
        "the model claimed success"
    );
    assert!(
        claim.checks[0].detail.contains("goodbye"),
        "{:?}",
        claim.checks
    );

    let runaway = by_id["runaway"];
    assert_eq!(
        runaway.outcome,
        Outcome::OverBudget,
        "{}",
        summary_of("runaway")
    );
    assert!(
        runaway.check_passed,
        "over budget even though the file is right"
    );
    assert!(
        runaway.steps > 3 && runaway.steps < 12,
        "cancelled mid-turn, not run to the end: {} steps",
        runaway.steps
    );
    assert!(
        runaway.cancelled.contains("over the budget of 3"),
        "{}",
        runaway.cancelled
    );

    let risky = by_id["risky-and-errors"];
    assert_eq!(
        risky.outcome,
        Outcome::Passed,
        "{}",
        summary_of("risky-and-errors")
    );
    assert_eq!(risky.steps, 4);
    assert_eq!(
        risky.errors,
        BTreeMap::from([
            ("approval_rejected".to_string(), 1),
            ("invalid".to_string(), 1)
        ])
    );
    assert_eq!(risky.approvals.len(), 1, "{:?}", risky.approvals);
    assert_eq!(risky.approvals[0].handling, "rejected_by_runner");
    assert_eq!(risky.approvals[0].final_status, "rejected");
    assert!(
        risky.approvals[0].reason.contains("quits the app"),
        "{:?}",
        risky.approvals
    );

    let denied = by_id["denied"];
    assert_eq!(
        denied.outcome,
        Outcome::NoControl,
        "{}",
        summary_of("denied")
    );
    assert_eq!((denied.start_calls, denied.steps), (1, 0));
    assert!(!denied.control_granted);
    assert_eq!(
        denied.errors,
        BTreeMap::from([("session_denied".to_string(), 1)])
    );

    let skipped = by_id["already-open"];
    assert_eq!(skipped.outcome, Outcome::Skipped);
    assert!(skipped.setup_error.contains("Busy is already running"));
    assert_eq!(skipped.turn.http_status, 0, "no turn was started");

    // Teardown ran for every editor task, matched on the run's root, and
    // never quit an app the run found open.
    let teardown = computer.0.lock().teardown.clone();
    let marker = work_root.to_string_lossy().into_owned();
    assert_eq!(
        teardown,
        vec![format!("close Editor docs in {marker}"); 3],
        "{teardown:?}"
    );

    let info = || report::RunInfo {
        run_id: "selftest",
        provider: "",
        model: "",
        started_at: "start".into(),
        finished_at: "end".into(),
    };
    // Without the two tasks that were never counted, the rule has enough
    // to decide: 2 of 4 is under the 60% native threshold.
    let mut decided = run.clone();
    decided
        .attempts
        .retain(|a| a.outcome != Outcome::NoControl && a.outcome != Outcome::Skipped);
    let decided = report::build(&suite, info(), decided);
    assert_eq!(
        decided.summary.verdict.decision, "native_mapping_worth_measuring",
        "{:?}",
        decided.summary.verdict
    );
    let report = report::build(&suite, info(), run);
    let s = &report.summary;
    assert_eq!((s.attempts, s.counted, s.passed), (6, 4, 2));
    assert_eq!(s.success_rate, Some(0.5));
    assert_eq!(s.median_steps_passed, Some(3.0));
    assert_eq!(s.median_step_ratio, Some(1.5));
    assert_eq!(
        s.outcomes,
        BTreeMap::from([
            ("check_failed".to_string(), 1),
            ("no_control".to_string(), 1),
            ("over_budget".to_string(), 1),
            ("passed".to_string(), 2),
            ("skipped".to_string(), 1),
        ])
    );
    assert_eq!(
        s.errors,
        BTreeMap::from([
            ("approval_rejected".to_string(), 1),
            ("invalid".to_string(), 1),
            ("session_denied".to_string(), 1),
        ])
    );
    assert_eq!(s.approvals, 1);
    assert!(s.screenshot_bytes.is_some_and(|b| b > 0));
    // A task with no counted attempt leaves the suite unmeasured: no verdict.
    assert_eq!(s.verdict.decision, "insufficient_data", "{:?}", s.verdict);
    assert!(
        s.verdict.reason.contains("denied, already-open"),
        "{:?}",
        s.verdict
    );
    let md = report::markdown(&report);
    for part in [
        "50% (2 of 4 counted; 6 attempts)",
        "`over_budget`: 1",
        "runaway r1",
        "| pass-save | edit | 1/1 |",
    ] {
        assert!(md.contains(part), "{part:?} not in\n{md}");
    }
    let json = serde_json::to_value(&report).expect("json");
    assert_eq!(json["summary"]["successRate"], 0.5);
    assert_eq!(json["attempts"].as_array().map(Vec::len), Some(6));
}
