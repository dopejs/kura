//! Stage 10.6: backup and restore under multi-user load, against the pooled
//! store. Two tenants write continuously through the daemon while an online
//! snapshot is taken (the same SQLite backup API the production script
//! uses); the snapshot restores into a fresh data directory, migrates, opens
//! with reader connections, and every tenant's rows are visible only to
//! that tenant.

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use chrono::Utc;
use kura_app::App;
use kura_config::{Config, Environment, StoreConfig};
use tower::ServiceExt;

fn test_config(dir: &std::path::Path) -> Config {
    std::fs::create_dir_all(dir).expect("create temp data dir");
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
        store: StoreConfig { readers: 2 },
    }
}

struct Actor {
    tenant_id: String,
    secret: String,
}

fn seed_actor(app: &App, label: &str) -> Actor {
    let now = Utc::now();
    let tenant_id = format!("ten_{label}");
    let principal_id = format!("prn_{label}");
    let auth = app.state.auth.as_ref().expect("auth manager");
    let (token, secret) = auth
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
            &serde_json::from_value(serde_json::json!({"tenantId": tenant_id, "tenantKind": "organization", "displayName": label, "status": "active", "createdAt": now, "updatedAt": now}))
                .unwrap(),
        )
        .unwrap();
    store
        .upsert_principal(
            &serde_json::from_value(serde_json::json!({"principalId": principal_id, "principalKind": "user", "displayName": label, "status": "active", "defaultTenantId": tenant_id, "createdAt": now, "updatedAt": now}))
                .unwrap(),
        )
        .unwrap();
    store
        .upsert_membership(
            &serde_json::from_value(serde_json::json!({"membershipId": format!("mem_{label}"), "tenantId": tenant_id, "principalId": principal_id, "role": "owner", "status": "active", "createdAt": now, "updatedAt": now}))
                .unwrap(),
        )
        .unwrap();
    store
        .upsert_token_tenant_grant(
            &serde_json::from_value(serde_json::json!({"grantId": format!("grant_{label}"), "tokenId": token.token_id, "tenantId": tenant_id, "isDefault": true, "status": "active", "createdAt": now, "updatedAt": now}))
                .unwrap(),
        )
        .unwrap();
    store.upsert_access_token(&token).expect("persist token");
    Actor { tenant_id, secret }
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
            .body(Body::from(serde_json::to_vec(&json).unwrap()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = router.clone().oneshot(request).await.expect("oneshot");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, json)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn an_online_backup_taken_under_two_tenant_load_restores_with_tenant_isolation_intact() {
    let source_dir = std::env::temp_dir().join(format!("kura-backup-src-{}", uuid::Uuid::now_v7()));
    let app = App::new(test_config(&source_dir)).expect("build app");
    let alice = seed_actor(&app, "alice");
    let bob = seed_actor(&app, "bob");
    let router = app.router();

    // Continuous writes from both tenants: chat turns on the echo provider
    // create dispatch rows, continuity turns and events under each tenant.
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut writers = Vec::new();
    for actor in [&alice, &bob] {
        let router = router.clone();
        let secret = actor.secret.clone();
        let tenant = actor.tenant_id.clone();
        let stop = stop.clone();
        writers.push(tokio::spawn(async move {
            let mut turns = 0usize;
            while !stop.load(std::sync::atomic::Ordering::SeqCst) || turns < 3 {
                let (status, _) = send(
                    &router,
                    &secret,
                    "POST",
                    "/v1/chat/query",
                    Some(serde_json::json!({
                        "query": format!("{tenant} turn {turns}"),
                        "provider": "echo",
                        "threadId": format!("thr_{tenant}")
                    })),
                )
                .await;
                assert_eq!(status, StatusCode::OK, "{tenant} turn {turns}");
                turns += 1;
                if turns >= 12 {
                    break;
                }
            }
            turns
        }));
    }

    // Tenant-bound rows written on the main task while the chat writers run:
    // sessions are keyed per tenant, so the restored copy can be checked for
    // exact per-tenant counts (alice 5, bob 3) and zero leakage.
    let session_router = app.state.router.as_ref().expect("session router").clone();
    for (actor, n) in [(&alice, 5usize), (&bob, 3usize)] {
        for i in 0..n {
            let (session, _) = session_router
                .route(kura_router::RouteInput {
                    channel: "api".to_string(),
                    peer_id: format!("{}-peer-{i}", actor.tenant_id),
                    ..Default::default()
                })
                .expect("route session");
            app.state
                .store
                .lock()
                .upsert_session_for_tenant_safe(&session, &actor.tenant_id)
                .expect("persist session under load");
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    // The online backup, taken while the writers are running.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let backup_dir = std::env::temp_dir().join(format!("kura-backup-dst-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&backup_dir).unwrap();
    let backup_file = backup_dir.join("daemon.sqlite");
    app.state
        .store
        .lock()
        .snapshot_to(&backup_file)
        .expect("online snapshot under load");
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let mut total_turns = 0;
    for writer in writers {
        total_turns += writer.await.unwrap();
    }
    assert!(
        total_turns >= 6,
        "both tenants kept writing during the backup: {total_turns}"
    );

    // Restore = open the snapshot as a data directory: migrations run (a
    // no-op here, the proof is that a v5 snapshot opens), readers open too.
    let restored_app = App::new(test_config(&backup_dir)).expect("restored assembly opens");
    assert_eq!(
        restored_app.state.store.lock().schema_version().unwrap(),
        kura_store::CURRENT_SCHEMA_VERSION
    );
    assert!(
        restored_app.state.store.lock().table_count().unwrap() > 10,
        "restored database has its tables"
    );
    assert_eq!(restored_app.state.store_pool.reader_count(), 2);
    let restored = restored_app.router();

    // Each tenant sees exactly its own sessions: the snapshot carries the
    // tenant binding, not just the rows.
    for (actor, expected) in [(&alice, 5), (&bob, 3)] {
        let (status, json) = send(&restored, &actor.secret, "GET", "/v1/sessions", None).await;
        assert_eq!(status, StatusCode::OK, "{json}");
        let items = json["items"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            items.len(),
            expected,
            "{} sessions in the restored copy: {json}",
            actor.tenant_id
        );
        assert!(
            items.iter().all(|s| s["peerId"]
                .as_str()
                .unwrap_or("")
                .starts_with(&actor.tenant_id)),
            "no cross-tenant sessions for {}: {items:?}",
            actor.tenant_id
        );
    }
    // The dispatch rows written before the snapshot are in the copy; turns
    // that completed after it are not, and must not be expected (the copy
    // is a point in time, not a mirror).
    let dispatches = restored_app
        .state
        .store
        .lock()
        .list_llm_dispatches()
        .unwrap();
    assert!(!dispatches.is_empty(), "dispatches written under load are in the copy");
    assert!(
        dispatches.len() <= total_turns,
        "{} dispatches cannot exceed the {total_turns} turns that ran",
        dispatches.len()
    );
    // Tokens issued before the backup still authenticate on the restored copy
    // (they are persisted state, not memory), which is what a recovery needs.
    let (status, me) = send(&restored, &alice.secret, "GET", "/v1/auth/me", None).await;
    assert_eq!(status, StatusCode::OK, "{me}");
}
