//! Route-table parity gate (Roadmap 74).
//!
//! The Go daemon's final route table (recovered from commit `16ac318^`,
//! daemon/internal/api/server.go) is recorded below as one representative
//! probe per registered mux pattern. The gate fails when a probe hits the
//! router's bare fallback (a 404 with an empty body): every handler-produced
//! response — including handler 404s for missing resources and 405s for
//! method mismatches — carries a body or a non-404 status, so an empty-body
//! 404 means the route family was never mounted.
//!
//! When a route is intentionally removed or reshaped, update the probe list
//! with a comment recording the divergence instead of deleting the entry
//! silently.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use parking_lot::Mutex;
use tower::ServiceExt;
use uuid::Uuid;

fn test_state() -> kura_api::AppState {
    let dir = std::env::temp_dir().join(format!("kura-route-parity-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let store = Arc::new(Mutex::new(
        kura_store::SQLiteStore::new(dir.to_str().expect("path")).expect("store"),
    ));
    let config = kura_config::Config {
        project_root: String::new(),
        environment: kura_config::Environment::Test,
        bind_addr: "127.0.0.1:19192".to_string(),
        data_dir: dir.to_string_lossy().to_string(),
        log_level: "info".to_string(),
        version: "0.1.0".to_string(),
        llm: kura_config::LlmConfig::default(),
        connectors: kura_config::ConnectorConfig::default(),
    };
    kura_api::AppState::new(config, Arc::new(kura_events::Bus::new()), store)
}

/// One probe per Go route-table pattern: (method, representative URI).
/// Prefix patterns (`/v1/foo/`) probe a representative sub-path.
const GO_ROUTE_PROBES: &[(&str, &str)] = &[
    ("GET", "/healthz"),
    ("GET", "/version"),
    ("GET", "/v1/system/info"),
    ("GET", "/v1/activation"),
    ("GET", "/v1/activation/diagnostics"),
    ("POST", "/v1/activation/test-chat"),
    ("POST", "/v1/admin/billing/tenants/probe/plan"),
    ("GET", "/v1/auth/me"),
    ("POST", "/v1/auth/pairings/probe/complete"),
    ("POST", "/v1/auth/pairings/start"),
    ("GET", "/v1/auth/tokens"),
    ("POST", "/v1/auth/tokens/probe/rotate"),
    ("GET", "/v1/billing/plan"),
    ("GET", "/v1/bindings"),
    ("GET", "/v1/bindings/probe"),
    ("GET", "/v1/calendar/accounts"),
    ("GET", "/v1/calendar/accounts/probe"),
    ("GET", "/v1/calendar/availability/queries"),
    ("GET", "/v1/calendar/availability/queries/probe"),
    ("GET", "/v1/calendar/events"),
    ("GET", "/v1/calendar/events/probe"),
    ("GET", "/v1/calendar/operations"),
    ("GET", "/v1/calendar/operations/probe"),
    ("GET", "/v1/capabilities"),
    ("GET", "/v1/capabilities/probe"),
    ("GET", "/v1/capability-visibility"),
    ("GET", "/v1/catalog/items"),
    ("GET", "/v1/catalog/items/probe"),
    ("GET", "/v1/channel-management/connectors"),
    ("GET", "/v1/channel-management/connectors/probe"),
    ("POST", "/v1/chat/query"),
    ("POST", "/v1/chat/query/stream"),
    ("GET", "/v1/computer-use/artifacts/probe"),
    ("GET", "/v1/config"),
    ("GET", "/v1/connectors"),
    ("GET", "/v1/connectors/probe"),
    ("GET", "/v1/deliveries"),
    ("GET", "/v1/deliveries/probe"),
    ("GET", "/v1/delivery/preferences"),
    ("GET", "/v1/delivery/preferences/probe"),
    ("GET", "/v1/delivery/targets"),
    ("GET", "/v1/delivery/targets/probe"),
    ("GET", "/v1/delivery/windows"),
    ("GET", "/v1/delivery/windows/probe"),
    ("GET", "/v1/evaluation/dashboard"),
    ("GET", "/v1/events"),
    ("GET", "/v1/events/stream"),
    ("POST", "/v1/execution/explain"),
    ("GET", "/v1/execution/profiles"),
    ("GET", "/v1/execution/profiles/probe"),
    ("GET", "/v1/integration-diagnostics/reason-codes"),
    ("POST", "/v1/integration-diagnostics/retention/apply"),
    ("GET", "/v1/integration-diagnostics/runs"),
    ("GET", "/v1/integration-diagnostics/runs/probe"),
    ("POST", "/v1/integration-diagnostics/smoke"),
    ("GET", "/v1/integrations"),
    ("GET", "/v1/integrations/probe"),
    ("GET", "/v1/live-validations"),
    ("GET", "/v1/live-validations/probe"),
    ("GET", "/v1/llm/dispatches"),
    ("GET", "/v1/llm/dispatches/probe"),
    ("POST", "/v1/llm/dispatches/stream"),
    ("GET", "/v1/mail/accounts"),
    ("GET", "/v1/mail/accounts/probe"),
    ("POST", "/v1/mail/attachments/probe/download"),
    ("GET", "/v1/mail/drafts"),
    ("GET", "/v1/mail/drafts/probe"),
    ("GET", "/v1/mail/messages/probe"),
    ("POST", "/v1/mail/messages/send"),
    ("GET", "/v1/mail/operations"),
    ("GET", "/v1/mail/operations/probe"),
    ("GET", "/v1/mail/threads"),
    ("GET", "/v1/mail/threads/probe"),
    ("GET", "/v1/mcp/catalog"),
    ("GET", "/v1/mcp/catalog/probe"),
    ("GET", "/v1/mcp/servers"),
    ("GET", "/v1/mcp/servers/probe"),
    ("GET", "/v1/mcp/transports"),
    ("GET", "/v1/operator/activity"),
    ("GET", "/v1/operator/diagnostics"),
    ("GET", "/v1/operator/onboarding"),
    ("GET", "/v1/policy/approvals"),
    ("POST", "/v1/policy/approvals/probe"),
    ("GET", "/v1/principals"),
    ("PATCH", "/v1/principals/probe"),
    ("GET", "/v1/profiles"),
    ("GET", "/v1/profiles/probe"),
    ("GET", "/v1/providers"),
    ("GET", "/v1/providers/probe"),
    ("POST", "/v1/release/launch-gate"),
    ("GET", "/v1/reminders"),
    ("GET", "/v1/reminders/probe"),
    ("GET", "/v1/routines"),
    ("GET", "/v1/routines/probe"),
    ("GET", "/v1/runs"),
    ("GET", "/v1/runs/probe"),
    ("GET", "/v1/sandboxes/executions"),
    ("GET", "/v1/sandboxes/executions/probe"),
    ("POST", "/v1/sandboxes/explain"),
    ("GET", "/v1/sandboxes/profiles"),
    ("GET", "/v1/sandboxes/profiles/probe"),
    ("GET", "/v1/schedules"),
    ("GET", "/v1/schedules/probe"),
    ("GET", "/v1/sessions"),
    ("GET", "/v1/sessions/probe"),
    ("GET", "/v1/setup/sessions"),
    ("GET", "/v1/setup/sessions/probe"),
    ("GET", "/v1/setup/targets"),
    ("GET", "/v1/skills"),
    ("GET", "/v1/skills/probe"),
    ("GET", "/v1/support/evidence-bundles"),
    ("GET", "/v1/support/evidence-bundles/probe"),
    ("GET", "/v1/tenant-audit-events"),
    ("GET", "/v1/tenant-invitations"),
    ("POST", "/v1/tenant-invitations/probe/accept"),
    ("GET", "/v1/tenant-secrets"),
    ("GET", "/v1/tenant-secrets/probe"),
    ("GET", "/v1/tenants"),
    ("GET", "/v1/tenants/probe"),
    ("GET", "/v1/threads"),
    ("GET", "/v1/threads/probe"),
    ("GET", "/v1/triage/policies"),
    ("GET", "/v1/triage/policies/probe"),
    ("POST", "/v1/triggers/webhook/probe"),
    ("GET", "/v1/webhooks"),
    ("GET", "/v1/webhooks/probe"),
    ("GET", "/v1/workspaces"),
    ("GET", "/v1/workspaces/probe"),
];

#[tokio::test]
async fn every_go_route_is_mounted() {
    let app = kura_api::router(test_state());
    let mut missing = Vec::new();

    for (method, uri) in GO_ROUTE_PROBES {
        let request = Request::builder()
            .method(*method)
            .uri(*uri)
            .body(Body::empty())
            .expect("request");
        let response = app.clone().oneshot(request).await.expect("oneshot");
        let status = response.status();
        // The router fallback is a 404 with an empty body; handler 404s carry
        // a JSON body (non-zero content-length).
        let fallback_404 = status == StatusCode::NOT_FOUND
            && response
                .headers()
                .get(header::CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok())
                .map(str::trim)
                .unwrap_or("0")
                == "0";
        if fallback_404 {
            missing.push(format!("{method} {uri}"));
        }
    }

    assert!(
        missing.is_empty(),
        "routes in the Go daemon's table are not mounted in the Rust router:\n{}",
        missing.join("\n")
    );
}
