//! activation route family (port of daemon/internal/api/activation.go).
//!
//! Routes: GET/POST /v1/activation, POST /v1/activation/test-chat, and
//! GET /v1/activation/diagnostics. Handlers require a resolved tenant
//! context (403 denial otherwise), return 501 `activation_not_implemented`
//! when the service is not configured, and map domain failures to the
//! structured 403 payload Go writes via writeActivationError.

use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json as AxumJson, Router};
use serde::Deserialize;

use kura_activation as activation;
use kura_identity::{LifecycleStatus, TokenAuthority};

use crate::error::ApiError;
use crate::middleware::{auth_token_authority, AuthenticatedToken, TenantContext};
use crate::state::AppState;

const ACTIVATION_NOT_IMPLEMENTED: &str = "activation_not_implemented";

/// Route family router for the /v1/activation prefix.
#[must_use]
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/activation", get(get_activation).post(start_activation))
        .route("/v1/activation/test-chat", post(test_chat))
        .route("/v1/activation/diagnostics", get(activation_diagnostics))
}

// ---------------------------------------------------------------------------
// Request DTOs (Go activationStartRequest / activationTestChatRequest)
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActivationStartRequest {
    source: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActivationTestChatRequest {
    message: String,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /v1/activation — current activation state.
async fn get_activation(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    token: Option<Extension<AuthenticatedToken>>,
) -> Result<(StatusCode, AxumJson<serde_json::Value>), ApiError> {
    let tenant_context = tenant_or_deny(tenant.as_ref().map(|e| &e.0.0))?;
    let Some(service) = &state.activation else {
        return Ok(activation_not_implemented());
    };
    let input = activation::GetInput {
        token: token_authority_or_empty(token.as_ref().map(|e| &e.0)),
        tenant_context: tenant_context.clone(),
    };
    let state_value = match service.get(input).await {
        Ok(value) => value,
        Err(err) => return activation_result(err),
    };
    Ok((StatusCode::OK, AxumJson(serde_json::json!({ "activation": state_value }))))
}

/// POST /v1/activation — start (or refresh) activation for the caller.
async fn start_activation(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    token: Option<Extension<AuthenticatedToken>>,
    body: String,
) -> Result<(StatusCode, AxumJson<serde_json::Value>), ApiError> {
    if state.activation.is_none() {
        if tenant.is_none() {
            return Err(ApiError::Forbidden("tenant access denied".to_string()));
        }
        return Ok(activation_not_implemented());
    }
    let request: ActivationStartRequest = decode_json_body(&body)?;
    let has_token = token.is_some();
    let has_tenant = tenant.is_some();
    if !has_token && !has_tenant {
        return Err(ApiError::Forbidden("tenant access denied".to_string()));
    }
    let tenant_context = tenant.as_ref().map(|e| e.0.0.clone()).unwrap_or_default();
    let authority = match &token {
        Some(authenticated) => auth_token_authority(&authenticated.0.0),
        None => TokenAuthority {
            token_id: tenant_context.token_id.clone(),
            principal_id: tenant_context.principal_id.clone(),
            default_tenant_id: tenant_context.tenant_id.clone(),
            status: LifecycleStatus::Active,
            expires_at: None,
        },
    };
    let service = state.activation.as_ref().expect("checked above");
    let input = activation::ActivateInput {
        token: authority,
        tenant_context,
        source: request.source.trim().to_string(),
    };
    let state_value = match service.activate(input).await {
        Ok(value) => value,
        Err(err) => return activation_result(err),
    };
    Ok((StatusCode::OK, AxumJson(serde_json::json!({ "activation": state_value }))))
}

/// POST /v1/activation/test-chat — run the metadata-only activation chat.
async fn test_chat(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    token: Option<Extension<AuthenticatedToken>>,
    body: String,
) -> Result<(StatusCode, AxumJson<serde_json::Value>), ApiError> {
    let tenant_context = tenant_or_deny(tenant.as_ref().map(|e| &e.0.0))?;
    let Some(service) = &state.activation else {
        return Ok(activation_not_implemented());
    };
    let request: ActivationTestChatRequest = decode_json_body(&body)?;
    let input = activation::RunTestChatInput {
        token: token_authority_or_empty(token.as_ref().map(|e| &e.0)),
        tenant_context: tenant_context.clone(),
        message: request.message.trim().to_string(),
    };
    let (state_value, test_chat_metadata) = match service.run_test_chat(input).await {
        Ok(tuple) => tuple,
        Err(failure) => return activation_result(failure.source),
    };
    Ok((StatusCode::OK, AxumJson(serde_json::json!({
        "activation": state_value,
        "testChat": test_chat_metadata,
    }))))
}

/// GET /v1/activation/diagnostics — failure diagnostics for the activation.
async fn activation_diagnostics(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    token: Option<Extension<AuthenticatedToken>>,
) -> Result<(StatusCode, AxumJson<serde_json::Value>), ApiError> {
    let tenant_context = tenant_or_deny(tenant.as_ref().map(|e| &e.0.0))?;
    let Some(service) = &state.activation else {
        return Ok(activation_not_implemented());
    };
    let input = activation::GetInput {
        token: token_authority_or_empty(token.as_ref().map(|e| &e.0)),
        tenant_context: tenant_context.clone(),
    };
    let items = match service.diagnostics(input).await {
        Ok(items) => items,
        Err(err) => return activation_result(err),
    };
    Ok((StatusCode::OK, AxumJson(serde_json::json!({ "items": items }))))
}

// ---------------------------------------------------------------------------
// Helpers (Go writeActivationNotImplemented / writeActivationError)
// ---------------------------------------------------------------------------

/// 501 `{error, code: activation_not_implemented}` payload (Go
/// writeActivationNotImplemented).
fn activation_not_implemented() -> (StatusCode, AxumJson<serde_json::Value>) {
    (
        StatusCode::NOT_IMPLEMENTED,
        AxumJson(serde_json::json!({
            "error": ACTIVATION_NOT_IMPLEMENTED,
            "code": ACTIVATION_NOT_IMPLEMENTED,
        })),
    )
}

/// Go writeActivationError: domain failures carry the stable reason payload
/// at 403; dependency failures surface as 500.
fn activation_result(err: activation::ActivationError) -> Result<(StatusCode, AxumJson<serde_json::Value>), ApiError> {
    match err {
        activation::ActivationError::Domain(domain) => Ok((
            StatusCode::FORBIDDEN,
            AxumJson(serde_json::json!({
                "error": domain.to_string(),
                "code": domain.reason_code.as_str(),
                "reasonCode": domain.reason_code.as_str(),
                "stage": domain.stage.as_str(),
                "retryable": domain.retryable,
                "remediationOwner": domain.remediation_owner.as_str(),
            })),
        )),
        activation::ActivationError::Dependency(message) => Err(ApiError::internal(message)),
    }
}

/// Requires a resolved tenant context (Go tenantContextFromContext, missing →
/// 403 tenant denial).
fn tenant_or_deny(tenant: Option<&kura_identity::TenantContext>) -> Result<kura_identity::TenantContext, ApiError> {
    tenant
        .cloned()
        .ok_or_else(|| ApiError::Forbidden("tenant access denied".to_string()))
}

/// Projects an authenticated token onto the identity token authority, or a
/// zero authority when no token is present (Go `authTokenAuthority` under
/// `if token, ok := ...`).
fn token_authority_or_empty(token: Option<&AuthenticatedToken>) -> TokenAuthority {
    match token {
        Some(authenticated) => auth_token_authority(&authenticated.0),
        None => TokenAuthority {
            token_id: String::new(),
            principal_id: String::new(),
            default_tenant_id: String::new(),
            status: LifecycleStatus::Active,
            expires_at: None,
        },
    }
}

/// Go decodeJSONBody: empty body → 400 "request body is required"; malformed
/// JSON → 400 with the serde error text.
fn decode_json_body<T: serde::de::DeserializeOwned>(body: &str) -> Result<T, ApiError> {
    if body.is_empty() {
        return Err(ApiError::BadRequest("request body is required".to_string()));
    }
    serde_json::from_str(body).map_err(|e| ApiError::BadRequest(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;

    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use parking_lot::Mutex;
    use tower::ServiceExt;
    use uuid::Uuid;

    use kura_identity as identity;
    use kura_identity::{Membership, MembershipFilter, Principal, PrincipalFilter, Tenant, TenantFilter, TokenTenantGrant};

    fn test_config() -> kura_config::Config {
        kura_config::Config {
            project_root: String::new(),
            environment: kura_config::Environment::Test,
            bind_addr: "127.0.0.1:19192".to_string(),
            data_dir: "/tmp/kura-api-activation-test".to_string(),
            log_level: "info".to_string(),
            version: "0.1.0".to_string(),
            llm: kura_config::LlmConfig::default(),
            connectors: kura_config::ConnectorConfig {
                discord: kura_config::DiscordConnectorConfig { enabled: false, ..Default::default() },
                telegram: kura_config::TelegramConnectorConfig { enabled: false, ..Default::default() },
                slack: kura_config::SlackConnectorConfig { enabled: false, ..Default::default() },
                matrix: kura_config::MatrixConnectorConfig { enabled: false, ..Default::default() },
            },
        }
    }

    fn test_state() -> AppState {
        let dir = std::env::temp_dir().join(format!("kura-api-activation-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let store = Arc::new(Mutex::new(
            kura_store::SQLiteStore::new(dir.to_str().expect("path")).expect("store"),
        ));
        AppState::new(test_config(), Arc::new(kura_events::Bus::new()), store)
    }

    /// In-memory [`activation::StateStore`] keyed by activation id.
    #[derive(Clone, Default)]
    struct FakeStateStore {
        states: Arc<Mutex<HashMap<String, activation::State>>>,
    }

    impl activation::StateStore for FakeStateStore {
        fn upsert_activation_state(
            &self,
            state: activation::State,
        ) -> activation::BoxFuture<'_, Result<(), activation::StoreError>> {
            let states = self.states.clone();
            Box::pin(async move {
                states.lock().insert(state.activation_id.clone(), state);
                Ok(())
            })
        }

        fn get_activation_state(
            &self,
            activation_id: &str,
        ) -> activation::BoxFuture<'_, Result<Option<activation::State>, activation::StoreError>> {
            let states = self.states.clone();
            let activation_id = activation_id.to_string();
            Box::pin(async move { Ok(states.lock().get(&activation_id).cloned()) })
        }

        fn get_activation_state_for_principal_tenant(
            &self,
            principal_id: &str,
            tenant_id: &str,
        ) -> activation::BoxFuture<'_, Result<Option<activation::State>, activation::StoreError>> {
            let states = self.states.clone();
            let (principal_id, tenant_id) = (principal_id.to_string(), tenant_id.to_string());
            Box::pin(async move {
                Ok(states
                    .lock()
                    .values()
                    .find(|s| s.principal_id == principal_id && s.tenant_id == tenant_id)
                    .cloned(),)
            })
        }
    }

    /// In-memory [`activation::IdentityRepository`] (filters are ignored; the
    /// service shapes the queries for the personal-tenant flow).
    #[derive(Clone, Default)]
    struct FakeIdentityRepository {
        principals: Arc<Mutex<HashMap<String, Principal>>>,
        tenants: Arc<Mutex<HashMap<String, Tenant>>>,
        memberships: Arc<Mutex<HashMap<String, Membership>>>,
        grants: Arc<Mutex<HashMap<String, TokenTenantGrant>>>,
    }

    impl activation::IdentityRepository for FakeIdentityRepository {
        fn get_principal(
            &self,
            principal_id: &str,
        ) -> activation::BoxFuture<'_, Result<Option<Principal>, activation::StoreError>> {
            let repo = self.clone();
            let principal_id = principal_id.to_string();
            Box::pin(async move { Ok(repo.principals.lock().get(&principal_id).cloned()) })
        }

        fn list_principals(
            &self,
            _filter: &PrincipalFilter,
        ) -> activation::BoxFuture<'_, Result<Vec<Principal>, activation::StoreError>> {
            let repo = self.clone();
            Box::pin(async move { Ok(repo.principals.lock().values().cloned().collect()) })
        }

        fn upsert_principal(
            &self,
            principal: Principal,
        ) -> activation::BoxFuture<'_, Result<(), activation::StoreError>> {
            let repo = self.clone();
            Box::pin(async move {
                repo.principals.lock().insert(principal.principal_id.clone(), principal);
                Ok(())
            })
        }

        fn get_tenant(
            &self,
            tenant_id: &str,
        ) -> activation::BoxFuture<'_, Result<Option<Tenant>, activation::StoreError>> {
            let repo = self.clone();
            let tenant_id = tenant_id.to_string();
            Box::pin(async move { Ok(repo.tenants.lock().get(&tenant_id).cloned()) })
        }

        fn list_tenants(
            &self,
            _filter: &TenantFilter,
        ) -> activation::BoxFuture<'_, Result<Vec<Tenant>, activation::StoreError>> {
            let repo = self.clone();
            Box::pin(async move { Ok(repo.tenants.lock().values().cloned().collect()) })
        }

        fn upsert_tenant(
            &self,
            tenant: Tenant,
        ) -> activation::BoxFuture<'_, Result<(), activation::StoreError>> {
            let repo = self.clone();
            Box::pin(async move {
                repo.tenants.lock().insert(tenant.tenant_id.clone(), tenant);
                Ok(())
            })
        }

        fn list_memberships(
            &self,
            _filter: &MembershipFilter,
        ) -> activation::BoxFuture<'_, Result<Vec<Membership>, activation::StoreError>> {
            let repo = self.clone();
            Box::pin(async move { Ok(repo.memberships.lock().values().cloned().collect()) })
        }

        fn upsert_membership(
            &self,
            membership: Membership,
        ) -> activation::BoxFuture<'_, Result<(), activation::StoreError>> {
            let repo = self.clone();
            Box::pin(async move {
                repo.memberships.lock().insert(membership.membership_id.clone(), membership);
                Ok(())
            })
        }

        fn list_token_tenant_grants(
            &self,
            token_id: &str,
        ) -> activation::BoxFuture<'_, Result<Vec<TokenTenantGrant>, activation::StoreError>> {
            let repo = self.clone();
            let token_id = token_id.to_string();
            Box::pin(async move {
                Ok(repo
                    .grants
                    .lock()
                    .values()
                    .filter(|g| g.token_id == token_id)
                    .cloned()
                    .collect(),)
            })
        }

        fn upsert_token_tenant_grant(
            &self,
            grant: TokenTenantGrant,
        ) -> activation::BoxFuture<'_, Result<(), activation::StoreError>> {
            let repo = self.clone();
            Box::pin(async move {
                repo.grants.lock().insert(grant.grant_id.clone(), grant);
                Ok(())
            })
        }
    }

    /// In-memory [`activation::AuditSink`] recording event kinds.
    #[derive(Clone, Default)]
    struct FakeAuditSink {
        events: Arc<Mutex<Vec<String>>>,
    }

    impl activation::AuditSink for FakeAuditSink {
        fn append_tenant_audit_event(
            &self,
            event: identity::TenantAuditEvent,
        ) -> activation::BoxFuture<'_, Result<identity::TenantAuditEvent, activation::StoreError>> {
            let events = self.events.clone();
            Box::pin(async move {
                events.lock().push(event.event_kind.clone());
                Ok(event)
            })
        }
    }

    fn configured_service() -> (activation::Service, FakeStateStore, FakeIdentityRepository, FakeAuditSink) {
        let state_store = FakeStateStore::default();
        let identity_repo = FakeIdentityRepository::default();
        let audit = FakeAuditSink::default();
        let service = activation::Service::new(activation::Dependencies {
            state_store: Some(Arc::new(state_store.clone())),
            identity: Some(Arc::new(identity_repo.clone())),
            audit: Some(Arc::new(audit.clone())),
            environment_scope: "test".to_string(),
            ..activation::Dependencies::default()
        });
        (service, state_store, identity_repo, audit)
    }

    async fn request_json(
        app: &Router,
        method: &str,
        uri: &str,
        body: Option<&str>,
        tenant: Option<(&str, &str, &str)>,
    ) -> (StatusCode, serde_json::Value) {
        let mut builder = Request::builder().method(method).uri(uri);
        if body.is_some() {
            builder = builder.header(axum::http::header::CONTENT_TYPE, "application/json");
        }
        let mut req = builder.body(Body::from(body.unwrap_or("").to_string())).expect("request");
        if let Some((principal_id, token_id, tenant_id)) = tenant {
            let ctx = identity::TenantContext {
                principal_id: principal_id.to_string(),
                token_id: token_id.to_string(),
                tenant_id: tenant_id.to_string(),
                ..Default::default()
            };
            req.extensions_mut().insert(TenantContext(ctx));
        }
        let response = app.clone().oneshot(req).await.expect("oneshot");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.expect("body");
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    #[tokio::test]
    async fn activation_route_requires_tenant_context() {
        let app = router().with_state(test_state());
        let (status, _) = request_json(&app, "GET", "/v1/activation", None, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn activation_route_shell_methods() {
        let app = router().with_state(test_state());
        let tenant = Some(("prn_1", "tok_1", "ten_personal"));

        let (status, json) = request_json(&app, "GET", "/v1/activation", None, tenant).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
        assert_eq!(json["error"], "activation_not_implemented");

        let (status, json) = request_json(&app, "POST", "/v1/activation", Some("{}"), tenant).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
        assert_eq!(json["code"], "activation_not_implemented");

        let (status, _) = request_json(&app, "PATCH", "/v1/activation", Some("{}"), tenant).await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);

        let (status, json) = request_json(&app, "POST", "/v1/activation/test-chat", Some("{}"), tenant).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
        assert_eq!(json["error"], "activation_not_implemented");

        let (status, _) = request_json(&app, "GET", "/v1/activation/test-chat", None, tenant).await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);

        let (status, json) = request_json(&app, "GET", "/v1/activation/diagnostics", None, tenant).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
        assert_eq!(json["error"], "activation_not_implemented");

        let (status, _) = request_json(&app, "POST", "/v1/activation/diagnostics", Some("{}"), tenant).await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn activation_get_returns_fresh_projection() {
        let mut state = test_state();
        let (service, _state_store, _identity, _audit) = configured_service();
        state.activation = Some(Arc::new(service));
        let app = router().with_state(state);

        let (status, json) = request_json(
            &app,
            "GET",
            "/v1/activation",
            None,
            Some(("prn_api", "tok_api", "ten_personal")),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["activation"]["status"], "not_started");
        assert_eq!(json["activation"]["principalId"], "prn_api");
        assert_eq!(json["activation"]["tenantId"], "ten_personal");
    }

    #[tokio::test]
    async fn activation_get_unconfigured_service_fails_closed() {
        let mut state = test_state();
        state.activation = Some(Arc::new(activation::Service::new(activation::Dependencies::default())));
        let app = router().with_state(state);

        let (status, json) = request_json(
            &app,
            "GET",
            "/v1/activation",
            None,
            Some(("prn_api", "tok_api", "ten_personal")),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(json["code"], "activation_failed:unexpected");
        assert_eq!(json["reasonCode"], "activation_failed:unexpected");
        assert_eq!(json["stage"], "unexpected");
        assert_eq!(json["retryable"], false);
        assert_eq!(json["remediationOwner"], "operator");
    }

    #[tokio::test]
    async fn activation_post_resolves_stable_personal_tenant() {
        let mut state = test_state();
        let (service, _state_store, _identity, audit) = configured_service();
        state.activation = Some(Arc::new(service));
        let app = router().with_state(state);

        let (status, first) = request_json(
            &app,
            "POST",
            "/v1/activation",
            Some(r#"{"source":"signup"}"#),
            Some(("prn_hosted", "tok_hosted", "")),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let first_tenant = first["activation"]["tenantId"].as_str().expect("tenant id").to_string();
        assert!(!first_tenant.is_empty());
        assert_eq!(first["activation"]["status"], "active");

        let (status, second) = request_json(
            &app,
            "POST",
            "/v1/activation",
            Some(r#"{"source":"returning_user"}"#),
            Some(("prn_hosted", "tok_hosted", "")),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(second["activation"]["tenantId"], first_tenant, "expected stable tenant");
        assert_eq!(second["activation"]["activationId"], first["activation"]["activationId"]);

        let events = audit.events.lock().clone();
        assert!(events.contains(&"tenant.activation_started".to_string()));
        assert!(events.contains(&"tenant.activation_completed".to_string()));
    }

    #[tokio::test]
    async fn activation_post_rejects_bad_body() {
        let mut state = test_state();
        let (service, _state_store, _identity, _audit) = configured_service();
        state.activation = Some(Arc::new(service));
        let app = router().with_state(state);

        let (status, json) = request_json(
            &app,
            "POST",
            "/v1/activation",
            Some("not-json"),
            Some(("prn_api", "tok_api", "ten_personal")),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(!json["error"].as_str().unwrap_or("").is_empty());
    }

    #[tokio::test]
    async fn activation_test_chat_fails_closed_without_persisted_state() {
        let mut state = test_state();
        let (service, _state_store, _identity, _audit) = configured_service();
        state.activation = Some(Arc::new(service));
        let app = router().with_state(state);

        let (status, json) = request_json(
            &app,
            "POST",
            "/v1/activation/test-chat",
            Some(r#"{"message":"hello"}"#),
            Some(("prn_api", "tok_api", "ten_personal")),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(json["reasonCode"], "activation_denied:tenant_access_revoked");
    }

    #[tokio::test]
    async fn activation_diagnostics_returns_empty_for_unstarted() {
        let mut state = test_state();
        let (service, _state_store, _identity, _audit) = configured_service();
        state.activation = Some(Arc::new(service));
        let app = router().with_state(state);

        let (status, json) = request_json(
            &app,
            "GET",
            "/v1/activation/diagnostics",
            None,
            Some(("prn_api", "tok_api", "ten_personal")),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(json["items"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn activation_diagnostics_unconfigured_fails_closed() {
        let mut state = test_state();
        state.activation = Some(Arc::new(activation::Service::new(activation::Dependencies::default())));
        let app = router().with_state(state);

        let (status, json) = request_json(
            &app,
            "GET",
            "/v1/activation/diagnostics",
            None,
            Some(("prn_api", "tok_api", "ten_personal")),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(json["code"], "activation_failed:unexpected");
    }
}
