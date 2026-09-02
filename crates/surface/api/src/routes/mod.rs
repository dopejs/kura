//! Router assembly for the API surface.
//!
//! Port of the route registrations in Go `NewServer` (daemon/internal/api/
//! server.go). This foundation ships the unauthenticated introspection routes:
//! `/healthz`, `/version`, `/v1/system/info`. Route families attach their own
//! `Router`s (with `protected()` layers) in later waves.

use axum::extract::State;
use axum::routing::get;
use axum::Router;
use serde::Serialize;
use tower_http::trace::TraceLayer;

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
pub mod sessions;
pub mod setupwizard;
pub mod skill_proposals;
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
        .merge(policy::router())
        .merge(providers::router())
        .merge(release::router())
        .merge(reminders::router())
        .merge(resources::router())
        .merge(retrieval::router())
        .merge(routine::router())
        .merge(runs::router())
        .merge(sandboxes::router())
        .merge(sessions::router())
        .merge(setupwizard::router())
        .merge(skill_proposals::router())
        .merge(triage::router())
        .merge(workflows::router())
        .merge(workspace_bindings::router())
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::middleware::protected,
        ));

    open.merge(protected_routes)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Shared test scaffolding for the route-family test modules: a minimal
/// AppState over a temp store plus a JSON request helper against the full
/// router.
#[cfg(test)]
pub(crate) mod tests_support {
    use std::sync::Arc;

    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use parking_lot::Mutex;
    use tower::ServiceExt;
    use uuid::Uuid;

    use crate::state::AppState;

    pub(crate) fn test_config() -> kura_config::Config {
        kura_config::Config {
            project_root: String::new(),
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
        AppState::new(test_config(), Arc::new(kura_events::Bus::new()), store)
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
            project_root: String::new(),
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
        }
    }

    /// Builds an AppState with only the required core; managers stay None.
    fn test_state() -> AppState {
        let dir = std::env::temp_dir().join(format!("kura-api-routes-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let store = Arc::new(Mutex::new(
            kura_store::SQLiteStore::new(dir.to_str().expect("path")).expect("store"),
        ));
        AppState::new(test_config(), Arc::new(kura_events::Bus::new()), store)
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
