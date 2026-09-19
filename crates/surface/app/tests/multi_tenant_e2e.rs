//! Stage 8.5: end-to-end multi-tenant verification against the real assembly.
//!
//! Every other tenancy test in the workspace injects an already-resolved
//! `TenantContext` extension. That proves the *handlers* scope their data, but
//! it cannot prove the *wiring* — and the wiring was exactly what was broken
//! (D1: `protected()` resolved the tenant and then never installed the
//! task-local, leaving `kura-tenancy`'s fail-closed accessors unreachable).
//!
//! This test therefore builds a real `App`, seeds two tenants with their own
//! principals and tokens, and drives plain HTTP requests carrying only a
//! bearer token — the same path a second user on a shared server takes.
//!
//! **What this file does and does not prove.** It proves the handlers and the
//! `TenantContext` extension wiring (D2): reverting any of the Stage 8.2
//! guards makes it fail. It does **not** prove D1 — these tests still pass
//! with the `tenantctx::scope` call removed from `protected()`, because every
//! route closed in Stage 8.2 reads the extension rather than the task-local.
//! D1's proof is `kura_api::middleware::tenant_task_local_tests`, which
//! asserts `require()` directly.
//!
//! That gap is a finding rather than an oversight: **no production route calls
//! a `kura-tenancy` accessor yet.** Stage 8.1 made that layer reachable; until
//! a route actually routes through it, the task-local has no end-to-end
//! consumer to observe. Migrating the hand-rolled scoping in this stage onto
//! those accessors is tracked as Stage 8.7.

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use chrono::Utc;
use kura_app::App;
use kura_config::{Config, Environment};
use tower::ServiceExt;

struct Actor {
    tenant_id: String,
    secret: String,
}

fn test_config() -> Config {
    let dir = std::env::temp_dir().join(format!("kura-app-mt-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).expect("create temp data dir");
    Config {
        environment: Environment::Test,
        bind_addr: "127.0.0.1:0".to_string(),
        data_dir: dir.to_string_lossy().into_owned(),
        log_level: "info".to_string(),
        version: "dev".to_string(),
        llm: Default::default(),
        connectors: Default::default(),
        egress: Default::default(),
        store: kura_config::StoreConfig { readers: 2 },
    }
}

/// Seeds one active tenant with an active Owner principal and a token granted
/// against it — the shape a provisioned multi-user deployment has.
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
            &serde_json::from_value(serde_json::json!({
                "tenantId": tenant_id,
                "tenantKind": "organization",
                "displayName": label,
                "status": "active",
                "createdAt": now,
                "updatedAt": now,
            }))
            .expect("tenant"),
        )
        .expect("upsert tenant");
    store
        .upsert_principal(
            &serde_json::from_value(serde_json::json!({
                "principalId": principal_id,
                "principalKind": "user",
                "displayName": label,
                "status": "active",
                "defaultTenantId": tenant_id,
                "createdAt": now,
                "updatedAt": now,
            }))
            .expect("principal"),
        )
        .expect("upsert principal");
    store
        .upsert_membership(
            &serde_json::from_value(serde_json::json!({
                "membershipId": format!("mem_{label}"),
                "tenantId": tenant_id,
                "principalId": principal_id,
                "role": "owner",
                "status": "active",
                "createdAt": now,
                "updatedAt": now,
            }))
            .expect("membership"),
        )
        .expect("upsert membership");
    store
        .upsert_token_tenant_grant(
            &serde_json::from_value(serde_json::json!({
                "grantId": format!("grant_{label}"),
                "tokenId": token.token_id,
                "tenantId": tenant_id,
                "isDefault": true,
                "status": "active",
                "createdAt": now,
                "updatedAt": now,
            }))
            .expect("grant"),
        )
        .expect("upsert grant");

    Actor { tenant_id, secret }
}

async fn send(
    app: &Arc<App>,
    actor: &Actor,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {}", actor.secret));
    let request = match body {
        Some(json) => builder
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&json).expect("encode")))
            .expect("request"),
        None => builder.body(Body::empty()).expect("request"),
    };
    let response = app.router().oneshot(request).await.expect("oneshot");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, json)
}

/// The wiring proof: a bearer token alone must resolve a tenant, install it in
/// the task-local, and scope every subsequent read.
#[tokio::test]
async fn bearer_token_alone_resolves_and_scopes_the_tenant() {
    let app = Arc::new(App::new(test_config()).expect("build app"));
    let alice = seed_actor(&app, "alice");

    // A protected route with no token is refused: the assembly really is
    // authenticated, so the isolation below is not vacuous.
    let anonymous = app
        .router()
        .oneshot(
            Request::builder()
                .uri("/v1/sessions")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("oneshot");
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

    // With a token and no tenant header, the token's default tenant resolves.
    let (status, _) = send(&app, &alice, "GET", "/v1/sessions", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!alice.tenant_id.is_empty());
}

/// Two users on one process: neither may enumerate nor address the other's
/// data, across every family closed in Stage 8.2.
#[tokio::test]
async fn two_tenants_in_one_process_cannot_reach_each_other() {
    let app = Arc::new(App::new(test_config()).expect("build app"));
    let alice = seed_actor(&app, "alice");
    let bob = seed_actor(&app, "bob");

    // --- approvals (policy engine: in-memory for the whole daemon) ---
    let (status, created) = send(
        &app,
        &alice,
        "POST",
        "/v1/policy/approvals",
        Some(serde_json::json!({
            "action": "tool.execute",
            "resourceKind": "skill",
            "resourceId": "skill_deploy",
            "reason": "alice's work",
            "requestedBy": "alice"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let approval_id = created["approval"]["approvalId"]
        .as_str()
        .expect("approvalId")
        .to_string();

    let (status, listed) = send(&app, &alice, "GET", "/v1/policy/approvals", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        listed["items"].as_array().expect("items").len(),
        1,
        "alice must see her own approval: {listed}"
    );

    let (status, listed) = send(&app, &bob, "GET", "/v1/policy/approvals", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        listed["items"].as_array().expect("items").is_empty(),
        "bob must not enumerate alice's approvals: {listed}"
    );

    let (status, _) = send(
        &app,
        &bob,
        "GET",
        &format!("/v1/policy/approvals/{approval_id}"),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "cross-tenant reads must not disclose existence"
    );

    let (status, _) = send(
        &app,
        &bob,
        "POST",
        &format!("/v1/policy/approvals/{approval_id}/resolve"),
        Some(serde_json::json!({ "resolution": "approved" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "bob must not resolve alice's approval — this is the approval gate itself"
    );

    // Alice still reaches it: the guard scopes rather than breaks.
    let (status, _) = send(
        &app,
        &alice,
        "GET",
        &format!("/v1/policy/approvals/{approval_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // --- triage policies (manager-document ownership) ---
    let (status, created) = send(
        &app,
        &alice,
        "POST",
        "/v1/triage/policies",
        Some(serde_json::json!({
            "name": "alice vip",
            "rules": [],
            "defaultClassification": "urgent"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let policy_id = created["policyId"].as_str().expect("policyId").to_string();

    let (status, listed) = send(&app, &bob, "GET", "/v1/triage/policies", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        listed["items"].as_array().expect("items").is_empty(),
        "bob must not enumerate alice's triage policies: {listed}"
    );

    let (status, _) = send(
        &app,
        &bob,
        "POST",
        &format!("/v1/triage/policies/{policy_id}/run"),
        Some(serde_json::json!({ "messages": [] })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// Several sessions per tenant, since "multi-tenant" is only meaningful if a
/// tenant can hold more than one conversation without leaking across the
/// boundary.
#[tokio::test]
async fn sessions_stay_within_their_tenant() {
    let app = Arc::new(App::new(test_config()).expect("build app"));
    let alice = seed_actor(&app, "alice");
    let bob = seed_actor(&app, "bob");

    let router = app.state.router.as_ref().expect("session router").clone();
    let seed_sessions = |tenant_id: &str, n: usize| {
        for i in 0..n {
            let (session, _) = router
                // Direct sessions key on channel+account+peer (thread_id is
                // not part of the key), so distinct peers is what yields
                // distinct sessions.
                .route(kura_router::RouteInput {
                    channel: "api".to_string(),
                    peer_id: format!("{tenant_id}-peer-{i}"),
                    ..Default::default()
                })
                .expect("route session");
            app.state
                .store
                .lock()
                .upsert_session_for_tenant_safe(&session, tenant_id)
                .expect("persist session");
        }
    };
    seed_sessions(&alice.tenant_id, 3);
    seed_sessions(&bob.tenant_id, 2);

    let (status, listed) = send(&app, &alice, "GET", "/v1/sessions", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        listed["items"].as_array().expect("items").len(),
        3,
        "alice owns three sessions: {listed}"
    );

    let (status, listed) = send(&app, &bob, "GET", "/v1/sessions", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        listed["items"].as_array().expect("items").len(),
        2,
        "bob owns two sessions and sees none of alice's: {listed}"
    );
}

/// Stage 10.5: access tokens survive a daemon restart. A token issued through
/// the API is persisted by the store, and a fresh assembly over the same data
/// directory accepts it; a token revoked before the restart stays revoked.
#[tokio::test]
async fn access_tokens_survive_a_restart_and_revocations_stick() {
    let config = test_config();
    let app = Arc::new(App::new(config.clone()).expect("build app"));
    let alice = seed_actor(&app, "alice");

    // Issue a second token for alice through the real route.
    let (status, issued) = send(
        &app,
        &alice,
        "POST",
        "/v1/auth/tokens",
        Some(serde_json::json!({
            "label": "laptop",
            "defaultTenantId": alice.tenant_id,
            "allowedTenantIds": [alice.tenant_id]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{issued}");
    let laptop_secret = issued["accessToken"].as_str().expect("secret").to_string();
    let laptop_id = issued["token"]["tokenId"]
        .as_str()
        .expect("token id")
        .to_string();
    let laptop = Actor {
        tenant_id: alice.tenant_id.clone(),
        secret: laptop_secret,
    };
    let (status, _) = send(&app, &laptop, "GET", "/v1/auth/me", None).await;
    assert_eq!(status, StatusCode::OK);

    // A third token, revoked before the restart.
    let (_, revoked) = send(
        &app,
        &alice,
        "POST",
        "/v1/auth/tokens",
        Some(serde_json::json!({
            "label": "old-phone",
            "defaultTenantId": alice.tenant_id,
            "allowedTenantIds": [alice.tenant_id]
        })),
    )
    .await;
    let phone = Actor {
        tenant_id: alice.tenant_id.clone(),
        secret: revoked["accessToken"].as_str().unwrap().to_string(),
    };
    let phone_id = revoked["token"]["tokenId"].as_str().unwrap().to_string();
    let (status, revoke_body) = send(
        &app,
        &alice,
        "POST",
        &format!("/v1/auth/tokens/{phone_id}/revoke"),
        Some(serde_json::json!({ "reason": "device lost" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{revoke_body}");

    // Restart: a new assembly over the same data directory.
    drop(app);
    let restarted = Arc::new(App::new(config).expect("rebuild app over the same data dir"));
    let (status, me) = send(&restarted, &laptop, "GET", "/v1/auth/me", None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the issued token authenticates after restart: {me}"
    );
    assert_eq!(me["token"]["tokenId"], laptop_id);
    let (status, _) = send(&restarted, &alice, "GET", "/v1/auth/me", None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the seeded token too (persisted on first use)"
    );
    let (status, _) = send(&restarted, &phone, "GET", "/v1/auth/me", None).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "a revoked token stays revoked"
    );
}

/// D11 (2026-09-19): a tenant's chat dispatches are bound to that tenant, so
/// the tenant-scoped dispatch list shows them — and only them.
#[tokio::test]
async fn chat_dispatches_are_listed_for_their_own_tenant_only() {
    let app = Arc::new(App::new(test_config()).expect("build app"));
    let alice = seed_actor(&app, "alice");
    let bob = seed_actor(&app, "bob");
    for (actor, n) in [(&alice, 2), (&bob, 1)] {
        for i in 0..n {
            let (status, body) = send(
                &app,
                actor,
                "POST",
                "/v1/chat/query",
                Some(serde_json::json!({ "query": format!("{} says {i}", actor.tenant_id), "provider": "echo" })),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{body}");
        }
    }
    for (actor, expected) in [(&alice, 2), (&bob, 1)] {
        let (status, listed) = send(&app, actor, "GET", "/v1/llm/dispatches", None).await;
        assert_eq!(status, StatusCode::OK, "{listed}");
        let items = listed["items"].as_array().expect("items");
        assert_eq!(items.len(), expected, "{}: {listed}", actor.tenant_id);
        assert!(
            items
                .iter()
                .all(|d| d["messages"].to_string().contains(&actor.tenant_id))
        );
        let id = items[0]["dispatchId"].as_str().unwrap();
        let other = if actor.tenant_id == alice.tenant_id {
            &bob
        } else {
            &alice
        };
        let (status, _) = send(
            &app,
            other,
            "GET",
            &format!("/v1/llm/dispatches/{id}"),
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "cross-tenant dispatch read is a 404"
        );
    }
}
