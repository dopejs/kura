//! Plan item 4.3: load against the real assembly with the agent loop doing
//! what it does in production — several tenants, each turn calling tools in
//! parallel (2.6), swarm fan-outs running beside them — to find out where
//! the daemon queues before 4.1 decides how to share it.
//!
//! The model is a local OpenAI-compatible server: the first round of every
//! turn answers with three parallel `memory.lookup` calls, the second with
//! text. Every request authenticates with its tenant's bearer token, so the
//! token path (4.2) is under load too.
//!
//! Where the time goes is measured three ways:
//! - the pool's own accounting and `kura_store_lock_wait_seconds`, which
//!   see reads and `pool.write()` only;
//! - a probe that keeps acquiring the API writer mutex (`state.store`, which
//!   ~95 handler writes lock directly, unmeasured by the pool);
//! - a probe that keeps writing through its own connection, as the
//!   manager-owned connections do, so SQLite's file-level write lock and
//!   busy waits show up — no metric sees those.
//!
//! The assertions are structural (every turn completes with its tool calls,
//! every swarm child finishes, no write fails on a busy database); the
//! timings are printed, since wall-clock thresholds on shared CI hosts are
//! not a signal. Scale with `KURA_LOAD_TENANTS`, `KURA_LOAD_SESSIONS` (per
//! tenant) and `KURA_LOAD_ROUNDS`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use chrono::Utc;
use kura_app::App;
use kura_config::{Config, Environment};
use tower::ServiceExt;

const TOOL_CALLS_PER_TURN: usize = 3;
const SWARM_GOALS_PER_RUN: usize = 3;
/// What the fake model takes to answer, so turns overlap as they would
/// against a real provider instead of finishing back to back.
const MODEL_LATENCY: Duration = Duration::from_millis(25);

fn scale(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value: &usize| *value > 0)
        .unwrap_or(default)
}

// ---------------------------------------------------------------------------
// Fake model
// ---------------------------------------------------------------------------

fn sse(chunks: &[serde_json::Value]) -> String {
    let mut body = String::new();
    for chunk in chunks {
        body.push_str("data: ");
        body.push_str(&chunk.to_string());
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    body
}

/// A round whose prompt already carries tool results is answered with
/// text; any other round asks for three lookups at once.
async fn completions(
    axum::Json(request): axum::Json<serde_json::Value>,
) -> impl axum::response::IntoResponse {
    tokio::time::sleep(MODEL_LATENCY).await;
    let has_results = request["messages"]
        .as_array()
        .is_some_and(|messages| messages.iter().any(|m| m["role"] == "tool"));
    let body = if has_results {
        sse(&[
            serde_json::json!({"choices":[{"delta":{"content":"done"},"finish_reason":null}]}),
            serde_json::json!({"choices":[{"delta":{},"finish_reason":"stop"}]}),
        ])
    } else {
        let calls: Vec<serde_json::Value> = (0..TOOL_CALLS_PER_TURN)
            .map(|index| {
                serde_json::json!({
                    "index": index,
                    "id": format!("call_{index}"),
                    "type": "function",
                    "function": {
                        "name": "memory.lookup",
                        "arguments": serde_json::json!({ "query": format!("topic {index}") }).to_string(),
                    }
                })
            })
            .collect();
        sse(&[
            serde_json::json!({"choices":[{"delta":{"tool_calls": calls},"finish_reason":null}]}),
            serde_json::json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]}),
        ])
    };
    ([("content-type", "text/event-stream")], body)
}

async fn start_model() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake model");
    let addr = listener.local_addr().expect("addr");
    let router = axum::Router::new().route("/chat/completions", axum::routing::post(completions));
    tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("serve fake model");
    });
    format!("http://{addr}")
}

// ---------------------------------------------------------------------------
// Assembly
// ---------------------------------------------------------------------------

fn test_config(model_url: &str) -> (tempfile::TempDir, Config) {
    let dir = tempfile::Builder::new()
        .prefix("kura-app-load-")
        .tempdir()
        .expect("create temp data dir");
    let mut llm = kura_config::LlmConfig {
        default_provider: "openai_compatible".to_string(),
        default_model: "load-model".to_string(),
        default_timeout_ms: 30_000,
        ..Default::default()
    };
    llm.openai_compatible.base_url = model_url.to_string();
    llm.openai_compatible.api_key = "load-test".to_string();
    llm.openai_compatible.model = "load-model".to_string();
    llm.openai_compatible.timeout_ms = 30_000;
    llm.openai_compatible.stream_first_chunk_timeout_ms = 30_000;
    llm.openai_compatible.stream_idle_timeout_ms = 30_000;
    let config = Config {
        project_root: Default::default(),
        environment: Environment::Test,
        bind_addr: "127.0.0.1:0".to_string(),
        data_dir: dir.path().to_string_lossy().into_owned(),
        log_level: "info".to_string(),
        version: "dev".to_string(),
        llm,
        connectors: Default::default(),
        egress: Default::default(),
        store: kura_config::StoreConfig::default(),
    };
    std::fs::write(
        dir.path().join(kura_plugin::PROFILE_FILE_NAME),
        serde_json::json!({ "entries": { "swarm": { "config": {
            "enabled": true,
            "maxChildrenPerRun": SWARM_GOALS_PER_RUN,
            "maxConcurrentChildren": SWARM_GOALS_PER_RUN
        } } } })
        .to_string(),
    )
    .expect("write plugin profile");
    (dir, config)
}

/// One active tenant with an Owner principal and a granted token, as a
/// provisioned shared deployment has (same shape as `multi_tenant_e2e`).
fn seed_tenant(app: &App, label: &str) -> String {
    let now = Utc::now();
    let tenant_id = format!("ten_{label}");
    let principal_id = format!("prn_{label}");
    let (token, secret) = app
        .state
        .auth
        .as_ref()
        .expect("auth manager")
        .issue_token(kura_identity::auth::IssueTokenInput {
            principal_id: principal_id.clone(),
            label: label.to_string(),
            default_tenant_id: tenant_id.clone(),
            expires_at: None,
        })
        .expect("issue token");
    let store = app.state.store.lock();
    store
        .upsert_tenant(
            &serde_json::from_value(serde_json::json!({
                "tenantId": tenant_id, "tenantKind": "organization", "displayName": label,
                "status": "active", "createdAt": now, "updatedAt": now,
            }))
            .expect("tenant"),
        )
        .expect("upsert tenant");
    store
        .upsert_principal(
            &serde_json::from_value(serde_json::json!({
                "principalId": principal_id, "principalKind": "user", "displayName": label,
                "status": "active", "defaultTenantId": tenant_id,
                "createdAt": now, "updatedAt": now,
            }))
            .expect("principal"),
        )
        .expect("upsert principal");
    store
        .upsert_membership(
            &serde_json::from_value(serde_json::json!({
                "membershipId": format!("mem_{label}"), "tenantId": tenant_id,
                "principalId": principal_id, "role": "owner", "status": "active",
                "createdAt": now, "updatedAt": now,
            }))
            .expect("membership"),
        )
        .expect("upsert membership");
    store
        .upsert_token_tenant_grant(
            &serde_json::from_value(serde_json::json!({
                "grantId": format!("grant_{label}"), "tokenId": token.token_id,
                "tenantId": tenant_id, "isDefault": true, "status": "active",
                "createdAt": now, "updatedAt": now,
            }))
            .expect("grant"),
        )
        .expect("upsert grant");
    secret
}

async fn send(
    router: &axum::Router,
    secret: &str,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {secret}"));
    let request = match body {
        Some(json) => builder
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&json).expect("encode")))
            .expect("request"),
        None => builder.body(Body::empty()).expect("request"),
    };
    let response = router.clone().oneshot(request).await.expect("oneshot");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

// ---------------------------------------------------------------------------
// Probes and figures
// ---------------------------------------------------------------------------

#[derive(Debug)]
#[allow(dead_code, reason = "read through Debug, for the printed record")]
struct Spread {
    samples: usize,
    p50: Duration,
    p95: Duration,
    max: Duration,
}

fn spread(mut samples: Vec<Duration>) -> Spread {
    samples.sort();
    let at = |q: f64| {
        samples
            .get(((samples.len() as f64 * q) as usize).min(samples.len().saturating_sub(1)))
            .copied()
            .unwrap_or_default()
    };
    Spread {
        samples: samples.len(),
        p50: at(0.50),
        p95: at(0.95),
        max: samples.last().copied().unwrap_or_default(),
    }
}

/// Runs `probe` every `every` on its own thread until `stop`, collecting
/// how long each took. Blocking on purpose: both probes block exactly as
/// the handler code they stand in for does.
fn spawn_probe(
    stop: Arc<AtomicBool>,
    every: Duration,
    mut probe: impl FnMut() -> Result<(), String> + Send + 'static,
) -> std::thread::JoinHandle<(Vec<Duration>, Vec<String>)> {
    std::thread::spawn(move || {
        let mut taken = Vec::new();
        let mut errors = Vec::new();
        while !stop.load(Ordering::Relaxed) {
            let started = Instant::now();
            if let Err(err) = probe() {
                errors.push(err);
            }
            taken.push(started.elapsed());
            std::thread::sleep(every);
        }
        (taken, errors)
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn tenants_run_parallel_tool_turns_and_swarms_without_failing_writes() {
    let tenants = scale("KURA_LOAD_TENANTS", 4);
    let sessions = scale("KURA_LOAD_SESSIONS", 2);
    let rounds = scale("KURA_LOAD_ROUNDS", 3);

    let model_url = start_model().await;
    let (_temp_dir, config) = test_config(&model_url);
    let data_dir = config.data_dir.clone();
    let app = App::new(config).expect("build app");
    let secrets: Vec<String> = (0..tenants)
        .map(|i| seed_tenant(&app, &format!("load{i}")))
        .collect();
    // A tenant's swarm children reserve quota per child, and a hosted
    // reservation without a plan is refused; the development plan's limits
    // are far above this load.
    let billing = app.state.billing.clone().expect("billing manager");
    for i in 0..tenants {
        billing
            .assign_plan(
                kura_billing::development_plan(&format!("ten_load{i}"), Utc::now()),
                "prn_operator",
                "load test",
            )
            .await
            .expect("assign plan");
    }
    let router = app.router();
    let pool = app.state.store_pool.clone();
    let writer = app.state.store.clone();
    let swarm = app
        .state
        .swarm
        .clone()
        .expect("swarm enabled by the profile");
    let tool_calls_before = kura_telemetry::metrics::registry().counter_value(
        kura_telemetry::metrics::CHAT_TOOL_CALLS_TOTAL,
        &[("name", "memory.lookup"), ("outcome", "ok")],
    );

    let stop = Arc::new(AtomicBool::new(false));
    let writer_probe = spawn_probe(stop.clone(), Duration::from_millis(2), move || {
        drop(writer.lock());
        Ok(())
    });
    let probe_store = kura_store::SQLiteStore::new(&data_dir).expect("probe connection");
    let probe_writes = AtomicUsize::new(0);
    let sqlite_probe = spawn_probe(stop.clone(), Duration::from_millis(5), move || {
        let n = probe_writes.fetch_add(1, Ordering::Relaxed);
        kura_store::put_document(
            &probe_store,
            "load_probe",
            &format!("probe_{}", n % 16),
            "",
            "",
            &serde_json::json!({ "n": n }),
        )
    });

    let started = Instant::now();
    let mut workers = Vec::new();
    for (tenant, secret) in secrets.iter().enumerate() {
        for session in 0..sessions {
            let router = router.clone();
            let secret = secret.clone();
            workers.push(tokio::spawn(async move {
                let mut turns = Vec::new();
                for round in 0..rounds {
                    let turn_started = Instant::now();
                    let (status, body) = send(
                        &router,
                        &secret,
                        "POST",
                        "/v1/chat/query",
                        Some(serde_json::json!({
                            "query": format!("tenant {tenant} session {session} round {round}"),
                            "threadId": format!("thr_load_{tenant}_{session}"),
                        })),
                    )
                    .await;
                    turns.push(turn_started.elapsed());
                    assert_eq!(
                        status,
                        StatusCode::OK,
                        "turn {tenant}/{session}/{round}: {body}"
                    );
                    assert_eq!(
                        body["reply"], "done",
                        "turn {tenant}/{session}/{round}: {body}"
                    );
                    let (status, _) =
                        send(&router, &secret, "GET", "/v1/llm/dispatches", None).await;
                    assert_eq!(status, StatusCode::OK);
                }
                turns
            }));
        }
    }
    let mut runs = Vec::new();
    for (tenant, secret) in secrets.iter().enumerate() {
        let (status, body) = send(
            &router,
            secret,
            "POST",
            "/v1/swarm/runs",
            Some(serde_json::json!({
                "goals": (0..SWARM_GOALS_PER_RUN)
                    .map(|goal| format!("tenant {tenant} goal {goal}"))
                    .collect::<Vec<_>>(),
            })),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::ACCEPTED,
            "swarm launch {tenant}: {body}"
        );
        runs.push(body["runId"].as_str().expect("runId").to_string());
    }

    let mut turns = Vec::new();
    for worker in workers {
        turns.extend(worker.await.expect("worker"));
    }
    let turns_done = started.elapsed();
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut finished = Vec::new();
    for run_id in &runs {
        loop {
            let run = swarm.get(run_id).expect("run");
            if !matches!(
                run.status,
                kura_swarm::RunStatus::Queued | kura_swarm::RunStatus::Running
            ) {
                finished.push(run);
                break;
            }
            assert!(
                Instant::now() < deadline,
                "swarm run {run_id} still {:?}",
                run.status
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    let all_done = started.elapsed();
    stop.store(true, Ordering::Relaxed);
    let (writer_waits, writer_errors) = writer_probe.join().expect("writer probe");
    let (sqlite_writes, sqlite_errors) = sqlite_probe.join().expect("sqlite probe");

    let tool_calls = kura_telemetry::metrics::registry().counter_value(
        kura_telemetry::metrics::CHAT_TOOL_CALLS_TOTAL,
        &[("name", "memory.lookup"), ("outcome", "ok")],
    ) - tool_calls_before;
    let stats = pool.stats();
    let turn_count = tenants * sessions * rounds;
    eprintln!(
        "load: {tenants} tenants x {sessions} sessions x {rounds} rounds = {turn_count} turns, \
         {} swarm runs x {SWARM_GOALS_PER_RUN} goals, {tool_calls} tool calls",
        runs.len()
    );
    eprintln!("turns finished in {turns_done:?}, swarms in {all_done:?}");
    eprintln!("turn latency: {:?}", spread(turns));
    eprintln!("API writer mutex wait (probe): {:?}", spread(writer_waits));
    eprintln!("own-connection write (probe): {:?}", spread(sqlite_writes));
    eprintln!("pool: {stats:?}");

    for run in &finished {
        assert_eq!(run.status, kura_swarm::RunStatus::Completed, "{run:?}");
    }
    let children = runs.len() * SWARM_GOALS_PER_RUN;
    assert_eq!(
        tool_calls as usize,
        (turn_count + children) * TOOL_CALLS_PER_TURN,
        "every turn and every swarm child made its parallel lookups"
    );
    assert!(writer_errors.is_empty());
    assert!(
        sqlite_errors.is_empty(),
        "a write on its own connection failed under load: {sqlite_errors:?}"
    );
    assert_eq!(stats.reads_via_writer, 0, "reads stayed off the writer");
}
