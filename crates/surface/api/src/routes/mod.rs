//! Router assembly for the API surface.
//!
//! Port of the route registrations in Go `NewServer` (daemon/internal/api/
//! server.go). This foundation ships the unauthenticated introspection routes:
//! `/healthz`, `/version`, `/v1/system/info`. Route families attach their own
//! `Router`s (with `protected()` layers) in later waves.

use axum::Router;
use axum::extract::State;
use axum::routing::get;
use serde::Serialize;
use tower_http::trace::TraceLayer;

use crate::error::ApiError;
use crate::response::Json;
use crate::state::AppState;
use crate::types::{self, SystemInfoResponse};

pub mod activation;
pub mod auth;
pub mod billing;
pub mod calendar;
pub mod capabilities;
pub mod catalog;
pub mod channel_management;
pub mod chat;
pub mod computer_use;
pub mod config;
pub mod connectors;
pub mod context;
pub mod evaluation;
pub mod evidence;
pub mod execprofile;
pub mod improvement;
pub mod integrations;
pub mod llm;
pub mod mail;
pub mod mcp;
pub mod memory;
pub mod plugins;
pub mod policy;
pub mod providers;
pub mod release;
pub mod reminders;
pub mod resources;
pub mod retrieval;
pub mod routine;
pub mod runs;
pub mod sandboxes;
pub mod session_frames;
pub mod sessions;
pub mod setupwizard;
pub mod skill_proposals;
pub mod swarm;
pub mod tools;
pub mod triage;
pub mod workflows;
pub mod workspace_bindings;

/// Decodes a JSON request body that Go's decodeJSONBody treats as required:
/// an empty body is a 400 "request body is required", malformed JSON is a 400
/// with the decoder message.
pub(crate) fn decode_json_required<T: serde::de::DeserializeOwned>(
    body: &axum::body::Bytes,
) -> Result<T, crate::error::ApiError> {
    if body.is_empty() {
        return Err(crate::error::ApiError::BadRequest(
            "request body is required".to_string(),
        ));
    }
    serde_json::from_slice(body).map_err(|err| crate::error::ApiError::BadRequest(err.to_string()))
}

/// Decodes a JSON request body where the Go handler tolerates the EOF decode
/// error (an absent body behaves like an empty request object); malformed
/// JSON is still a 400.
pub(crate) fn decode_json_or_default<T: Default + serde::de::DeserializeOwned>(
    body: &axum::body::Bytes,
) -> Result<T, crate::error::ApiError> {
    if body.is_empty() {
        return Ok(T::default());
    }
    serde_json::from_slice(body).map_err(|err| crate::error::ApiError::BadRequest(err.to_string()))
}

/// `/healthz` payload (Go: `{"ok": true, "service": "kura"}`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthzResponse {
    pub ok: bool,
    pub service: &'static str,
}

/// `/version` payload (Go: `{"version": cfg.Version}`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionResponse {
    pub version: String,
}

/// GET /healthz — liveness probe.
#[allow(clippy::unused_async)]
pub async fn healthz() -> Json<HealthzResponse> {
    Json(HealthzResponse {
        ok: true,
        service: "kura",
    })
}

/// GET /version — daemon version.
#[allow(clippy::unused_async)]
pub async fn version(State(state): State<AppState>) -> Json<VersionResponse> {
    Json(VersionResponse {
        version: state.config.version.clone(),
    })
}

/// GET /v1/system/info — environment introspection (Go
/// `buildSystemInfoResponse`).
#[allow(clippy::unused_async)]
pub async fn system_info(State(state): State<AppState>) -> Json<SystemInfoResponse> {
    Json(types::build_system_info_response(&state.config))
}

/// Builds the API router with shared layers (tracing) and the Go
/// authentication topology: everything runs behind the `protected()`
/// middleware except the Go unauthenticated allowlist — `/healthz`,
/// `/version`, `/v1/system/info`, the pairing entry points (Go
/// withEnvironment), and the signature-authenticated webhook ingress.
#[must_use]
pub fn router(state: AppState) -> Router {
    let open = Router::new()
        .route("/healthz", get(healthz))
        .route("/version", get(version))
        .route("/v1/system/info", get(system_info))
        .merge(mcp::ingress_router())
        .merge(
            auth::open_router().route_layer(axum::middleware::from_fn_with_state(
                state.clone(),
                crate::middleware::with_environment,
            )),
        );

    let protected_routes = Router::new()
        .merge(activation::router())
        .merge(auth::router())
        .merge(billing::router())
        .merge(catalog::router())
        .merge(chat::router())
        .merge(calendar::router())
        .merge(capabilities::router())
        .merge(channel_management::router())
        .merge(computer_use::router())
        .merge(config::router())
        .merge(connectors::router())
        .merge(context::router())
        .merge(evaluation::router())
        .merge(evidence::router())
        .merge(execprofile::router())
        .merge(improvement::router())
        .merge(integrations::router())
        .merge(llm::router())
        .merge(mail::router())
        .merge(mcp::router())
        .merge(memory::router())
        .merge(plugins::router())
        // The by-id tenant guard needs the state, so it is attached here
        // rather than inside the family's stateless `router()`. It answers 404
        // (never a disclosure) for rows owned by another tenant and admits
        // pre-backfill NULL rows.
        .merge(
            policy::router().layer(crate::middleware::ByIDTenantGuardLayer::new(
                state.clone(),
                "/v1/policy/approvals/",
                "approvals",
                "approval_id",
                "approval",
            )),
        )
        .merge(providers::router())
        .merge(release::router())
        .merge(reminders::router())
        .merge(resources::router())
        .merge(retrieval::router())
        .merge(routine::router())
        .merge(runs::router())
        // Sandbox executions are per-principal; profiles are operator
        // configuration and stay reachable (the reload mutation carries the
        // daemon-global operator guard instead).
        .merge(
            sandboxes::router().layer(crate::middleware::ByIDTenantGuardLayer::new(
                state.clone(),
                "/v1/sandboxes/executions/",
                "sandbox_executions",
                "execution_id",
                "sandbox_execution",
            )),
        )
        .merge(session_frames::router())
        .merge(sessions::router())
        .merge(setupwizard::router())
        .merge(skill_proposals::router())
        .merge(swarm::router())
        .merge(tools::router())
        .merge(triage::router())
        // Workflows are addressed under their owning run, so the run is the
        // tenant-owned resource the guard checks; all four routes in the
        // family are covered by the one layer.
        .merge(
            workflows::router().layer(crate::middleware::ByIDTenantGuardLayer::new(
                state.clone(),
                "/v1/runs/",
                "runs",
                "run_id",
                "run",
            )),
        )
        .merge(workspace_bindings::router())
        // Stage 10.3: the metrics endpoint is a protected route (a scraper
        // authenticates with a bearer token like any client), because the
        // exposition carries tenant ids as labels.
        .route("/metrics", get(metrics))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::middleware::protected,
        ));

    open.merge(protected_routes)
        .layer(axum::middleware::from_fn(
            crate::middleware::observe_request,
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// GET /metrics — Prometheus text exposition (Stage 10.3).
#[allow(clippy::unused_async)]
pub async fn metrics() -> axum::response::Response {
    use axum::response::IntoResponse;
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        kura_telemetry::metrics::registry().render_prometheus(),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Tenancy for manager-document-backed families
// ---------------------------------------------------------------------------
//
// `kura-routine` and `kura-triage` keep their state in memory for the whole
// daemon and write through to the `manager_documents` table, so ownership
// lives on the document row rather than in the domain type (adding a tenant
// field to `Routine`/`Policy` would be a serialized-contract change). These
// three helpers apply the same conventions the row-based `kura-tenancy`
// accessors use.

/// Binds a just-persisted manager document to the acting tenant. No-op in the
/// single-user assembly. A document owned by another tenant answers 404 rather
/// than disclosing the conflict.
pub(crate) fn bind_document_tenant(
    state: &AppState,
    tenant: Option<&crate::middleware::TenantContext>,
    doc_kind: &str,
    doc_id: &str,
) -> Result<(), ApiError> {
    let Some(tc) = tenant else { return Ok(()) };
    let tenant_id = tc.0.tenant_id.trim();
    if tenant_id.is_empty() {
        return Ok(());
    }
    match state
        .store
        .lock()
        .bind_manager_document_tenant(doc_kind, doc_id, tenant_id)
    {
        Ok(true) => Ok(()),
        // No document to bind: the manager did not write through (no store
        // handle, or a failed persist). Failing loudly here beats returning a
        // success the caller can never see again — a tenant-filtered list
        // would silently omit the item forever.
        Ok(false) => Err(ApiError::internal(&format!(
            "{doc_kind}/{doc_id} was not persisted, so its tenant ownership could not be recorded"
        ))),
        Err(e) if kura_store::SQLiteStore::is_cross_tenant_row(&e) => {
            Err(ApiError::NotFound("not found".to_string()))
        }
        Err(e) => Err(ApiError::from_store(e)),
    }
}

/// Document ids the caller's tenant may enumerate, or `None` when no tenant is
/// acting and the manager's full view is the answer. Pre-tenancy documents
/// (empty tenant) are not enumerable — see
/// [`SQLiteStore::list_manager_document_ids_for_tenant`].
pub(crate) fn tenant_visible_document_ids(
    state: &AppState,
    tenant: Option<&crate::middleware::TenantContext>,
    doc_kind: &str,
) -> Result<Option<std::collections::HashSet<String>>, ApiError> {
    let Some(tc) = tenant else { return Ok(None) };
    let tenant_id = tc.0.tenant_id.trim();
    if tenant_id.is_empty() {
        return Ok(None);
    }
    let ids = state
        .store_pool
        .read()
        .list_manager_document_ids_for_tenant(doc_kind, tenant_id)
        .map_err(ApiError::from_store)?;
    Ok(Some(ids.into_iter().collect()))
}

/// By-id guard for manager documents: the composite `(doc_kind, doc_id)` key
/// does not fit `ByIDTenantGuardLayer`, so the check is explicit. Admits
/// pre-tenancy documents and documents the caller owns; anything else is 404.
pub(crate) fn guard_document_tenant(
    state: &AppState,
    tenant: Option<&crate::middleware::TenantContext>,
    doc_kind: &str,
    doc_id: &str,
) -> Result<(), ApiError> {
    let Some(tc) = tenant else { return Ok(()) };
    let tenant_id = tc.0.tenant_id.trim();
    if tenant_id.is_empty() {
        return Ok(());
    }
    let owner = state
        .store_pool
        .read()
        .lookup_manager_document_tenant(doc_kind, doc_id)
        .map_err(ApiError::from_store)?;
    match owner {
        // Absent: the handler's own not-found path answers.
        None => Ok(()),
        Some(o) if o.is_empty() || o == tenant_id => Ok(()),
        Some(_) => Err(ApiError::NotFound("not found".to_string())),
    }
}

/// Shared test scaffolding for the route-family test modules: a minimal
/// AppState over a temp store plus a JSON request helper against the full
/// router.
#[cfg(test)]
pub(crate) mod tests_support {
    use std::sync::Arc;

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use parking_lot::Mutex;
    use tower::ServiceExt;
    use uuid::Uuid;

    use crate::state::AppState;

    pub(crate) fn test_config() -> kura_config::Config {
        kura_config::Config {
            store: Default::default(),
            environment: kura_config::Environment::Test,
            bind_addr: "127.0.0.1:19192".to_string(),
            data_dir: "/tmp/kura-api-test".to_string(),
            log_level: "info".to_string(),
            version: "0.1.0".to_string(),
            llm: kura_config::LlmConfig::default(),
            connectors: kura_config::ConnectorConfig {
                discord: kura_config::DiscordConnectorConfig {
                    enabled: false,
                    ..Default::default()
                },
                telegram: kura_config::TelegramConnectorConfig {
                    enabled: false,
                    ..Default::default()
                },
                slack: kura_config::SlackConnectorConfig {
                    enabled: false,
                    ..Default::default()
                },
                matrix: kura_config::MatrixConnectorConfig {
                    enabled: false,
                    ..Default::default()
                },
            },
            egress: Default::default(),
        }
    }

    /// AppState with only the required core; managers stay None and are
    /// injected per test module.
    pub(crate) fn test_state() -> AppState {
        let dir = std::env::temp_dir().join(format!("kura-api-routes-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let store = Arc::new(Mutex::new(
            kura_store::SQLiteStore::new(dir.to_str().expect("path")).expect("store"),
        ));
        let mut state = AppState::new(
            test_config(),
            Arc::new(kura_events::Bus::new()),
            store.clone(),
        );
        // Stage 10.2: every route test runs with reader connections so a
        // handler that mutates through `store_pool.read()` fails here
        // (query-only readers refuse writes) rather than in production.
        state.store_pool =
            Arc::new(kura_store::StorePool::new(store, 2).expect("open reader connections"));
        state
    }

    /// Sends one request against the assembled router and decodes the JSON
    /// response (Null for empty bodies, e.g. bare 405s).
    pub(crate) async fn request_json(
        state: AppState,
        method: &str,
        uri: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        let app = super::router(state);
        let builder = Request::builder().method(method).uri(uri);
        let request = match body {
            Some(json) => builder
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&json).expect("encode body")))
                .expect("request"),
            None => builder.body(Body::empty()).expect("request"),
        };
        let response = app.oneshot(request).await.expect("oneshot");
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

    /// [`request_json`] with a resolved tenant attached, standing in for what
    /// `protected()` installs in a real multi-tenant assembly. Test states
    /// carry no auth manager, so the middleware passes through and this
    /// extension is what the handlers and the by-id guard see.
    ///
    /// The tenant acts as `Role::Owner`; use [`request_json_as_role`] to
    /// exercise the daemon-global guard with a weaker role.
    pub(crate) async fn request_json_as_tenant(
        state: AppState,
        tenant_id: &str,
        method: &str,
        uri: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        let app = super::router(state);
        let builder = Request::builder().method(method).uri(uri);
        let mut request = match body {
            Some(json) => builder
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&json).expect("encode body")))
                .expect("request"),
            None => builder.body(Body::empty()).expect("request"),
        };
        request
            .extensions_mut()
            .insert(crate::middleware::TenantContext(
                kura_identity::TenantContext {
                    tenant_id: tenant_id.to_string(),
                    principal_id: format!("prn_{tenant_id}"),
                    role: Some(kura_identity::Role::Owner),
                    ..Default::default()
                },
            ));
        let response = app.oneshot(request).await.expect("oneshot");
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

    /// [`request_json_as_tenant`] with an explicit role, for the
    /// daemon-global operator guard.
    pub(crate) async fn request_json_as_role(
        state: AppState,
        tenant_id: &str,
        role: kura_identity::Role,
        method: &str,
        uri: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        let app = super::router(state);
        let builder = Request::builder().method(method).uri(uri);
        let mut request = match body {
            Some(json) => builder
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&json).expect("encode body")))
                .expect("request"),
            None => builder.body(Body::empty()).expect("request"),
        };
        request
            .extensions_mut()
            .insert(crate::middleware::TenantContext(
                kura_identity::TenantContext {
                    tenant_id: tenant_id.to_string(),
                    principal_id: format!("prn_{tenant_id}"),
                    role: Some(role),
                    ..Default::default()
                },
            ));
        let response = app.oneshot(request).await.expect("oneshot");
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use axum::body::to_bytes;
    use axum::http::{Request, StatusCode};
    use parking_lot::Mutex;
    use tower::ServiceExt;
    use uuid::Uuid;

    fn test_config() -> kura_config::Config {
        kura_config::Config {
            store: Default::default(),
            environment: kura_config::Environment::Test,
            bind_addr: "127.0.0.1:19192".to_string(),
            data_dir: "/tmp/kura-api-test".to_string(),
            log_level: "info".to_string(),
            version: "0.1.0".to_string(),
            llm: kura_config::LlmConfig::default(),
            connectors: kura_config::ConnectorConfig {
                discord: kura_config::DiscordConnectorConfig {
                    enabled: false,
                    ..Default::default()
                },
                telegram: kura_config::TelegramConnectorConfig {
                    enabled: false,
                    ..Default::default()
                },
                slack: kura_config::SlackConnectorConfig {
                    enabled: false,
                    ..Default::default()
                },
                matrix: kura_config::MatrixConnectorConfig {
                    enabled: false,
                    ..Default::default()
                },
            },
            egress: Default::default(),
        }
    }

    /// Builds an AppState with only the required core; managers stay None.
    fn test_state() -> AppState {
        let dir = std::env::temp_dir().join(format!("kura-api-routes-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let store = Arc::new(Mutex::new(
            kura_store::SQLiteStore::new(dir.to_str().expect("path")).expect("store"),
        ));
        let mut state = AppState::new(
            test_config(),
            Arc::new(kura_events::Bus::new()),
            store.clone(),
        );
        state.store_pool =
            Arc::new(kura_store::StorePool::new(store, 2).expect("open reader connections"));
        state
    }

    async fn get_json(uri: &str) -> (StatusCode, serde_json::Value) {
        let app = router(test_state());
        let request = Request::builder()
            .uri(uri)
            .body(axum::body::Body::empty())
            .expect("request");
        let response = app.oneshot(request).await.expect("oneshot");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let json = serde_json::from_slice(&bytes).expect("json body");
        (status, json)
    }

    #[tokio::test]
    async fn healthz_returns_ok() {
        let (status, json) = get_json("/healthz").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json, serde_json::json!({ "ok": true, "service": "kura" }));
    }

    #[tokio::test]
    async fn version_returns_config_version() {
        let (status, json) = get_json("/version").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json, serde_json::json!({ "version": "0.1.0" }));
    }

    #[tokio::test]
    async fn system_info_returns_environment_projection() {
        let (status, json) = get_json("/v1/system/info").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["service"], "kura");
        assert_eq!(json["environment"], "test");
        assert_eq!(json["version"], "0.1.0");
        assert_eq!(json["bindAddr"], "127.0.0.1:19192");
        assert_eq!(json["dataDir"], "/tmp/kura-api-test");
        assert_eq!(json["logLevel"], "info");
    }

    #[tokio::test]
    async fn protected_routes_require_auth_when_auth_is_configured() {
        let mut state = test_state();
        state.auth = Some(Arc::new(kura_identity::auth::Manager::new()));
        let app = router(state);

        // Protected route without a token: 401 (Go protected()).
        let request = Request::builder()
            .uri("/v1/config")
            .body(axum::body::Body::empty())
            .expect("request");
        let response = app.clone().oneshot(request).await.expect("oneshot");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        // Allowlisted routes stay open: healthz and the pairing entry point.
        let request = Request::builder()
            .uri("/healthz")
            .body(axum::body::Body::empty())
            .expect("request");
        let response = app.clone().oneshot(request).await.expect("oneshot");
        assert_eq!(response.status(), StatusCode::OK);

        let request = Request::builder()
            .method("POST")
            .uri("/v1/auth/pairings/start")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(r#"{"mode":"local","label":"cli"}"#))
            .expect("request");
        let response = app.oneshot(request).await.expect("oneshot");
        assert_ne!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn unknown_route_is_404() {
        // axum returns an empty 404 body; check the status only.
        let app = router(test_state());
        let request = Request::builder()
            .uri("/v1/does-not-exist")
            .body(axum::body::Body::empty())
            .expect("request");
        let response = app.oneshot(request).await.expect("oneshot");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}

#[cfg(test)]
mod observability_tests {
    //! Stage 10.3: every request is counted by route template, timed, and
    //! tagged with a request id that the client can correlate.

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use super::tests_support::test_state;

    #[tokio::test]
    async fn requests_are_counted_by_route_template_and_carry_a_request_id() {
        let state = test_state();
        let app = super::router(state.clone());
        let before = kura_telemetry::metrics::registry().counter_value(
            kura_telemetry::metrics::HTTP_REQUESTS_TOTAL,
            &[("route", "/healthz"), ("method", "GET"), ("status", "2xx")],
        );
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .header("x-request-id", "req-from-client")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("x-request-id").unwrap(),
            "req-from-client",
            "a client-supplied id is echoed"
        );
        let generated = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let id = generated
            .headers()
            .get("x-request-id")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(id.starts_with("req_"), "a generated id is attached: {id}");
        let after = kura_telemetry::metrics::registry().counter_value(
            kura_telemetry::metrics::HTTP_REQUESTS_TOTAL,
            &[("route", "/healthz"), ("method", "GET"), ("status", "2xx")],
        );
        assert!(
            after >= before + 2,
            "counted by route template: {before} -> {after}"
        );

        // A store read through the pool is observed as lock wait.
        let _ = state
            .store_pool
            .read()
            .list_events(&kura_events::Filter::default());
        // The exposition is served and names the series.
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response
                .headers()
                .get("content-type")
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("text/plain; version=0.0.4")
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            text.contains("# TYPE kura_http_requests_total counter"),
            "{text}"
        );
        assert!(text.contains("route=\"/healthz\""), "{text}");
        assert!(
            text.contains("kura_http_request_duration_seconds_bucket"),
            "{text}"
        );
        assert!(
            text.contains("kura_store_lock_wait_seconds"),
            "store waits are observed: {text}"
        );
    }
}
