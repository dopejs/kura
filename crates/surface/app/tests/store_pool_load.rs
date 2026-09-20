//! Stage 10.2 verification: a concurrent-session load test against the real
//! assembly, with and without reader connections, recording store lock wait
//! through the pool's own accounting (the same numbers `/metrics` exposes).
//!
//! The assertion is structural — with readers configured, reads are served
//! by readers and never queue behind the writer; with `readers = 0` every
//! read goes through the writer — because wall-clock comparisons on shared
//! CI hosts are not a signal. The measured waits are printed for the record.

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use kura_app::App;
use kura_config::{Config, Environment, StoreConfig};
use tower::ServiceExt;

fn test_config(readers: usize) -> Config {
    let dir = std::env::temp_dir().join(format!("kura-app-pool-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).expect("create temp data dir");
    Config {
        project_root: Default::default(),
        environment: Environment::Test,
        bind_addr: "127.0.0.1:0".to_string(),
        data_dir: dir.to_string_lossy().into_owned(),
        log_level: "info".to_string(),
        version: "dev".to_string(),
        // Echo is listed by the providers manager only when asked for by
        // name (upstream change 9d2d389); these tests dispatch at it.
        llm: kura_config::LlmConfig {
            default_provider: "echo".to_string(),
            ..Default::default()
        },
        connectors: Default::default(),
        egress: Default::default(),
        store: StoreConfig { readers },
    }
}

async fn request(
    router: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> StatusCode {
    let builder = Request::builder().method(method).uri(uri);
    let request = match body {
        Some(json) => builder
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&json).unwrap()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = router.clone().oneshot(request).await.expect("oneshot");
    let status = response.status();
    let _ = to_bytes(response.into_body(), usize::MAX).await;
    status
}

/// Mixed load: `sessions` concurrent workers each doing a chat turn (a write
/// path: dispatch rows, continuity, events) interleaved with list reads.
async fn run_load(readers: usize, sessions: usize, rounds: usize) -> kura_store::PoolStats {
    let mut app = App::new(test_config(readers)).expect("build app");
    app.state.auth = None; // single-user assembly: no bearer tokens needed
    // `App` holds a non-Sync store handle for the connector runtimes, so the
    // workers share the router (Send + Clone) and the pool, not the App.
    let router = app.router();
    let pool = app.state.store_pool.clone();
    let mut workers = Vec::new();
    for i in 0..sessions {
        let router = router.clone();
        workers.push(tokio::spawn(async move {
            for round in 0..rounds {
                let status = request(
                    &router,
                    "POST",
                    "/v1/chat/query",
                    Some(serde_json::json!({
                        "query": format!("session {i} round {round}"),
                        "provider": "echo",
                        "threadId": format!("thr_load_{i}")
                    })),
                )
                .await;
                assert_eq!(status, StatusCode::OK, "chat turn {i}/{round}");
                // Read paths that serve the single-user assembly and go
                // through the pool's readers (agent profiles, context
                // assemblies, provider checks) plus the dispatch list.
                for uri in [
                    "/v1/llm/dispatches",
                    "/v1/agent-profiles",
                    "/v1/context/assemblies",
                    "/v1/providers/checks",
                ] {
                    let status = request(&router, "GET", uri, None).await;
                    assert!(
                        status.is_success() || status == StatusCode::NOT_FOUND,
                        "{uri}: {status}"
                    );
                }
            }
        }));
    }
    for worker in workers {
        worker.await.expect("worker");
    }
    pool.stats()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_sessions_read_through_readers_when_the_pool_is_enabled() {
    let pooled = run_load(4, 8, 3).await;
    let single = run_load(0, 8, 3).await;
    eprintln!("pool readers=4: {pooled:?}");
    eprintln!("pool readers=0: {single:?}");

    assert_eq!(pooled.readers, 4);
    assert!(
        pooled.reads_via_readers > 0,
        "reads served by readers: {pooled:?}"
    );
    assert_eq!(
        pooled.reads_via_writer, 0,
        "no read queued behind the writer: {pooled:?}"
    );
    // Writes still take the writer mutex directly (`state.store.lock()`), so
    // the pool's writer counters only see `pool.write()` callers; what the
    // pool measures is the read side, which is the side it changed.

    assert_eq!(single.readers, 0);
    assert_eq!(single.reads_via_readers, 0);
    assert!(
        single.reads_via_writer > 0,
        "pool disabled: every read is a writer acquisition"
    );
    // Lock wait is measured (the 10.3 signal the program asked for): reader
    // waits with the pool, writer waits without it.
    assert!(pooled.reader.acquisitions > 0 && single.writer.acquisitions > 0);
    assert!(
        pooled.reader.total_wait_ns < single.writer.total_wait_ns,
        "reads waited less on readers than on the shared writer: {pooled:?} vs {single:?}"
    );
    assert!(
        kura_telemetry::metrics::registry().histogram_count(
            kura_telemetry::metrics::STORE_LOCK_WAIT_SECONDS,
            &[("role", "reader")]
        ) > 0,
        "reader waits are exported as a metric"
    );
}
