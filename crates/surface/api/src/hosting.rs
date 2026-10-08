//! Hosting mode: one owner's daemon (`single`) or one daemon shared by many
//! tenants (`shared`). Design: `docs/harness/shared-hosting.md` §4.
//!
//! Single mode changes nothing. In shared mode:
//!
//! - one tenant is the **platform operator** (the bootstrapped local tenant);
//!   only its Owner may change the daemon;
//! - every protected route is operator-only unless [`TENANT_ROUTES`] lists it
//!   ([`shared_route_policy`]), so a route added later is closed until someone
//!   decides otherwise;
//! - pairing answers only direct loopback peers ([`loopback_pairing`]).

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, MatchedPath, Request, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::middleware::TenantContext;
use crate::state::AppState;

/// The env var `kura-app` reads the mode from.
pub const HOSTING_ENV: &str = "KURA_HOSTING";

/// Stable error code of the shared-mode route denial.
pub const OPERATOR_ONLY_CODE: &str = "hosting_operator_only";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Hosting {
    /// One owner; today's behavior.
    #[default]
    Single,
    /// Many tenants on one daemon; `operator_tenant_id` runs the platform.
    Shared { operator_tenant_id: String },
}

impl Hosting {
    /// Parses `KURA_HOSTING`: unset or empty is single mode; anything other
    /// than `single` / `shared` is an error rather than a silent default, so a
    /// typo cannot start a shared daemon in single mode.
    pub fn parse(value: Option<&str>, operator_tenant_id: &str) -> Result<Self, String> {
        match value.map(str::trim).unwrap_or_default() {
            "" | "single" => Ok(Self::Single),
            "shared" => {
                let operator_tenant_id = operator_tenant_id.trim();
                if operator_tenant_id.is_empty() {
                    return Err("shared hosting needs an operator tenant".to_string());
                }
                Ok(Self::Shared {
                    operator_tenant_id: operator_tenant_id.to_string(),
                })
            }
            other => Err(format!(
                "{HOSTING_ENV}={other:?} is not a hosting mode (use \"single\" or \"shared\")"
            )),
        }
    }

    #[must_use]
    pub fn is_shared(&self) -> bool {
        matches!(self, Self::Shared { .. })
    }

    /// Whether the caller may change the daemon itself. Single mode: always
    /// (route handlers keep their own guards). Shared mode: only the Owner of
    /// the operator tenant; a request without a tenant is not the operator.
    #[must_use]
    pub fn is_platform_operator(&self, tenant: Option<&kura_identity::TenantContext>) -> bool {
        match self {
            Self::Single => true,
            Self::Shared { operator_tenant_id } => tenant.is_some_and(|tc| {
                tc.tenant_id == *operator_tenant_id && tc.role == Some(kura_identity::Role::Owner)
            }),
        }
    }
}

/// Routes any tenant member may call in shared mode, as `(method, path
/// template)`; the handlers' own permission and tenant checks still apply.
/// Everything else is operator-only. Each entry is here because the handler
/// acts only on the caller's own tenant (or principal); see §4.3 for what is
/// deliberately left out.
pub const TENANT_ROUTES: &[(&str, &str)] = &[
    // Self-service identity. Token management is confined to the caller's
    // tenant (G14); tenant creation, invitations and membership changes wait
    // for M2 provisioning.
    ("GET", "/v1/auth/me"),
    ("GET", "/v1/auth/tokens"),
    ("POST", "/v1/auth/tokens"),
    ("POST", "/v1/auth/tokens/{token_id}/rotate"),
    ("POST", "/v1/auth/tokens/{token_id}/revoke"),
    ("PATCH", "/v1/auth/tokens/{token_id}/tenant-grants"),
    ("GET", "/v1/tenants"),
    ("GET", "/v1/tenants/{tenant_id}"),
    ("GET", "/v1/tenants/{tenant_id}/memberships"),
    ("GET", "/v1/tenants/{tenant_id}/permissions"),
    ("GET", "/v1/tenant-invitations"),
    ("POST", "/v1/tenant-invitations/{invitation_id}/accept"),
    ("POST", "/v1/tenant-invitations/{invitation_id}/reject"),
    ("GET", "/v1/principals"),
    ("GET", "/v1/tenant-audit-events"),
    // Billing reads.
    ("GET", "/v1/billing/plan"),
    ("GET", "/v1/billing/usage"),
    ("GET", "/v1/billing/quotas"),
    ("GET", "/v1/billing/quota-dashboard"),
    ("GET", "/v1/billing/denials"),
    ("GET", "/v1/billing/denials/{denial_id}"),
    // Conversation. Chat tools are the tenant's own (G15).
    ("POST", "/v1/chat/query"),
    ("POST", "/v1/chat/query/stream"),
    ("GET", "/v1/sessions"),
    ("GET", "/v1/sessions/{session_id}"),
    ("POST", "/v1/sessions/{session_id}/reset"),
    ("GET", "/v1/sessions/{session_id}/events"),
    ("GET", "/v1/threads"),
    ("GET", "/v1/threads/{thread_id}"),
    ("POST", "/v1/threads/{thread_id}/handoffs"),
    (
        "GET",
        "/v1/threads/{thread_id}/continuity-previews/{preview_id}",
    ),
    ("POST", "/v1/threads/{thread_id}/reset"),
    ("POST", "/v1/threads/{thread_id}/archive"),
    ("POST", "/v1/threads/{thread_id}/reopen"),
    ("GET", "/v1/threads/{thread_id}/frame"),
    ("PUT", "/v1/threads/{thread_id}/frame"),
    ("DELETE", "/v1/threads/{thread_id}/frame"),
    ("GET", "/v1/runs"),
    ("GET", "/v1/runs/{run_id}"),
    ("GET", "/v1/events"),
    ("GET", "/v1/events/stream"),
    // Memory and recall. Consolidation and index rebuilds are daemon-wide
    // maintenance.
    ("GET", "/v1/memory/assets"),
    ("POST", "/v1/memory/assets"),
    ("GET", "/v1/memory/assets/{asset_id}"),
    ("GET", "/v1/memory/assets/{asset_id}/drilldown"),
    ("POST", "/v1/memory/assets/{asset_id}/approve"),
    ("POST", "/v1/memory/assets/{asset_id}/reject"),
    ("POST", "/v1/memory/assets/{asset_id}/revoke"),
    ("POST", "/v1/memory/assets/{asset_id}/visibility"),
    ("POST", "/v1/memory/capture"),
    ("GET", "/v1/memory/overview"),
    ("POST", "/v1/retrieval/queries"),
    // Reminders.
    ("GET", "/v1/reminders"),
    ("POST", "/v1/reminders"),
    ("GET", "/v1/reminders/{id}"),
    ("GET", "/v1/reminders/{id}/actions"),
    ("POST", "/v1/reminders/{id}/acknowledge"),
    ("POST", "/v1/reminders/{id}/snooze"),
    ("POST", "/v1/reminders/{id}/complete"),
    ("POST", "/v1/reminders/{id}/dismiss"),
    ("POST", "/v1/reminders/{id}/cancel"),
    ("POST", "/v1/reminders/{id}/reschedule"),
    ("GET", "/v1/reminders/occurrences"),
    ("GET", "/v1/reminders/occurrences/{occurrence_id}"),
    // Approvals raised for the caller's own tool calls (by-id tenant guard).
    ("GET", "/v1/policy/approvals"),
    ("GET", "/v1/policy/approvals/{approval_id}"),
    ("POST", "/v1/policy/approvals/{approval_id}/resolve"),
    // Providers and models, read only: the dispatcher is global until BYOK.
    ("GET", "/v1/providers"),
    ("GET", "/v1/providers/{provider_id}"),
    ("GET", "/v1/providers/{provider_id}/models"),
    ("GET", "/v1/model-roles"),
];

fn tenant_route(method: &Method, path: &str) -> bool {
    TENANT_ROUTES
        .iter()
        .any(|(m, p)| *p == path && method.as_str() == *m)
}

/// Shared-mode route policy. Installed inside `protected()`, so the tenant is
/// already resolved. Single mode passes everything through.
pub async fn shared_route_policy(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    if !state.hosting.is_shared() {
        return next.run(req).await;
    }
    let tenant = req.extensions().get::<TenantContext>().map(|tc| &tc.0);
    if state.hosting.is_platform_operator(tenant) {
        return next.run(req).await;
    }
    let allowed = tenant.is_some()
        && req
            .extensions()
            .get::<MatchedPath>()
            .is_some_and(|path| tenant_route(req.method(), path.as_str()));
    if allowed {
        return next.run(req).await;
    }
    operator_only()
}

fn operator_only() -> Response {
    let message = "this route is reserved for the platform operator on a shared daemon";
    (
        StatusCode::FORBIDDEN,
        axum::Json(serde_json::json!({
            "code": OPERATOR_ONLY_CODE,
            "message": message,
            "error": message,
        })),
    )
        .into_response()
}

/// Shared-mode guard on the unauthenticated pairing routes: only a direct
/// loopback peer (the host, `docker exec`) may pair. A same-host reverse
/// proxy also connects from loopback, so a request carrying forwarding
/// headers is refused too. Without the peer address (the server was not
/// started with connect info) pairing is refused: fail closed. The refusal is
/// a bare 404, the same as an unknown route.
pub async fn loopback_pairing(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if !state.hosting.is_shared() {
        return next.run(req).await;
    }
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0);
    if peer.is_some_and(|addr| addr.ip().is_loopback()) && !forwarded(req.headers()) {
        return next.run(req).await;
    }
    StatusCode::NOT_FOUND.into_response()
}

fn forwarded(headers: &HeaderMap) -> bool {
    ["forwarded", "x-forwarded-for", "x-real-ip"]
        .iter()
        .any(|name| headers.contains_key(*name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(tenant_id: &str, role: Option<kura_identity::Role>) -> kura_identity::TenantContext {
        kura_identity::TenantContext {
            tenant_id: tenant_id.to_string(),
            role,
            ..kura_identity::TenantContext::default()
        }
    }

    #[test]
    fn parse_accepts_the_two_modes_and_rejects_typos() {
        assert_eq!(Hosting::parse(None, "ten_local"), Ok(Hosting::Single));
        assert_eq!(Hosting::parse(Some(" "), "ten_local"), Ok(Hosting::Single));
        assert_eq!(
            Hosting::parse(Some("single"), "ten_local"),
            Ok(Hosting::Single)
        );
        assert_eq!(
            Hosting::parse(Some("shared"), "ten_local"),
            Ok(Hosting::Shared {
                operator_tenant_id: "ten_local".to_string()
            })
        );
        assert!(Hosting::parse(Some("shard"), "ten_local").is_err());
        assert!(Hosting::parse(Some("Shared"), "ten_local").is_err());
        assert!(Hosting::parse(Some("shared"), " ").is_err());
    }

    #[test]
    fn only_the_operator_tenants_owner_is_the_platform_operator() {
        use kura_identity::Role;
        let shared = Hosting::Shared {
            operator_tenant_id: "ten_local".to_string(),
        };
        assert!(shared.is_platform_operator(Some(&context("ten_local", Some(Role::Owner)))));
        assert!(!shared.is_platform_operator(Some(&context("ten_local", Some(Role::Admin)))));
        assert!(!shared.is_platform_operator(Some(&context("ten_user", Some(Role::Owner)))));
        assert!(!shared.is_platform_operator(None));
        assert!(Hosting::Single.is_platform_operator(None));
    }

    use axum::body::{Body, to_bytes};
    use axum::http::Request as HttpRequest;
    use tower::ServiceExt;

    use crate::routes::tests_support::test_state;

    const OPERATOR_TENANT: &str = "ten_local";

    fn shared_state() -> AppState {
        let mut state = test_state();
        state.hosting = Hosting::Shared {
            operator_tenant_id: OPERATOR_TENANT.to_string(),
        };
        state
    }

    /// Sends one request through the full router and returns the status and
    /// the error code, if the body is a JSON error. Streaming bodies are not
    /// read (they would not end).
    async fn send(
        state: AppState,
        tenant: Option<(&str, kura_identity::Role)>,
        method: &str,
        uri: &str,
        prepare: impl FnOnce(&mut HttpRequest<Body>),
    ) -> (StatusCode, Option<String>, bool) {
        let body = if method == "GET" || method == "DELETE" {
            Body::empty()
        } else {
            Body::from("{}")
        };
        let mut request = HttpRequest::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(body)
            .expect("request");
        if let Some((tenant_id, role)) = tenant {
            request
                .extensions_mut()
                .insert(TenantContext(context(tenant_id, Some(role))));
        }
        prepare(&mut request);
        let response = crate::routes::router(state)
            .oneshot(request)
            .await
            .expect("oneshot");
        let status = response.status();
        let json = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("application/json"));
        if !json {
            return (status, None, false);
        }
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
        let code = value
            .get("code")
            .and_then(|code| code.as_str())
            .map(str::to_string);
        (status, code, true)
    }

    fn probe_uri(template: &str) -> String {
        template
            .split('/')
            .map(|segment| {
                if segment.starts_with('{') {
                    "probe"
                } else {
                    segment
                }
            })
            .collect::<Vec<_>>()
            .join("/")
    }

    /// Routes that stay closed to tenants in M1, one per family that matters.
    const OPERATOR_ROUTES: &[(&str, &str)] = &[
        ("POST", "/v1/sandboxes/executions"),
        ("PUT", "/v1/providers/probe/credential"),
        ("PUT", "/v1/providers/probe/account"),
        ("PUT", "/v1/model-roles/probe"),
        ("POST", "/v1/mcp/servers"),
        ("GET", "/v1/tenant-secrets"),
        ("POST", "/v1/tenants"),
        ("POST", "/v1/tenants/probe/invitations"),
        ("PATCH", "/v1/principals/probe"),
        ("POST", "/v1/llm/dispatches"),
        ("POST", "/v1/runs"),
        ("POST", "/v1/routines"),
        ("POST", "/v1/memory/consolidate"),
        ("GET", "/metrics"),
    ];

    #[tokio::test]
    async fn every_tenant_route_reaches_its_handler_in_shared_mode() {
        for (method, template) in TENANT_ROUTES {
            let uri = probe_uri(template);
            let (status, code, json) = send(
                shared_state(),
                Some(("ten_user", kura_identity::Role::Owner)),
                method,
                &uri,
                |_| {},
            )
            .await;
            assert_ne!(
                code.as_deref(),
                Some(OPERATOR_ONLY_CODE),
                "{method} {template}"
            );
            // A bare 404 is the router's fallback and a 405 a method the path
            // does not have: either way the entry names no real route.
            assert!(
                status != StatusCode::NOT_FOUND || json,
                "{method} {template} is not a route"
            );
            assert_ne!(
                status,
                StatusCode::METHOD_NOT_ALLOWED,
                "{method} {template}"
            );
        }
    }

    #[tokio::test]
    async fn everything_else_is_reserved_for_the_platform_operator() {
        use kura_identity::Role;
        for (method, uri) in OPERATOR_ROUTES {
            for tenant in [
                Some(("ten_user", Role::Owner)),
                Some((OPERATOR_TENANT, Role::Admin)),
                None,
            ] {
                let (status, code, _) = send(shared_state(), tenant, method, uri, |_| {}).await;
                assert_eq!(
                    status,
                    StatusCode::FORBIDDEN,
                    "{method} {uri} as {tenant:?}"
                );
                assert_eq!(code.as_deref(), Some(OPERATOR_ONLY_CODE), "{method} {uri}");
            }
            let operator = Some((OPERATOR_TENANT, Role::Owner));
            let (_, code, _) = send(shared_state(), operator, method, uri, |_| {}).await;
            assert_ne!(
                code.as_deref(),
                Some(OPERATOR_ONLY_CODE),
                "operator: {method} {uri}"
            );
            // Single mode is unchanged.
            let tenant = Some(("ten_user", Role::Owner));
            let (_, code, _) = send(test_state(), tenant, method, uri, |_| {}).await;
            assert_ne!(
                code.as_deref(),
                Some(OPERATOR_ONLY_CODE),
                "single: {method} {uri}"
            );
        }
    }

    #[tokio::test]
    async fn shared_mode_pairs_only_direct_loopback_peers() {
        let uri = "/v1/auth/pairings/start";
        let peer = |addr: &str| {
            let addr: SocketAddr = addr.parse().expect("addr");
            move |request: &mut HttpRequest<Body>| {
                request.extensions_mut().insert(ConnectInfo(addr));
            }
        };
        let hidden = |status: StatusCode, json: bool| status == StatusCode::NOT_FOUND && !json;

        let (status, _, json) =
            send(shared_state(), None, "POST", uri, peer("127.0.0.1:5000")).await;
        assert!(!hidden(status, json), "loopback peer: {status}");
        let (status, _, json) = send(shared_state(), None, "POST", uri, peer("[::1]:5000")).await;
        assert!(!hidden(status, json), "ipv6 loopback peer: {status}");

        let (status, _, json) =
            send(shared_state(), None, "POST", uri, peer("10.0.0.7:5000")).await;
        assert!(hidden(status, json), "remote peer: {status}");
        let proxied = |request: &mut HttpRequest<Body>| {
            peer("127.0.0.1:5000")(request);
            request
                .headers_mut()
                .insert("x-forwarded-for", "203.0.113.9".parse().expect("header"));
        };
        let (status, _, json) = send(shared_state(), None, "POST", uri, proxied).await;
        assert!(hidden(status, json), "proxied peer: {status}");
        // No peer address at all: fail closed.
        let (status, _, json) = send(shared_state(), None, "POST", uri, |_| {}).await;
        assert!(hidden(status, json), "unknown peer: {status}");

        // Single mode is unchanged.
        let (status, _, json) = send(test_state(), None, "POST", uri, |_| {}).await;
        assert!(!hidden(status, json), "single mode: {status}");
    }

    #[test]
    fn tenant_routes_are_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for entry in TENANT_ROUTES {
            assert!(seen.insert(entry), "duplicate entry {entry:?}");
        }
    }
}
