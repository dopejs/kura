//! providers route family (port of the /v1/providers handlers in Go
//! daemon/internal/api/server.go, Roadmaps 9/10).
//!
//! Routes: GET /v1/providers, GET /v1/providers/{provider_id}, GET
//! /v1/providers/{provider_id}/auth, POST
//! /v1/providers/{provider_id}/auth/{start|complete|refresh|revoke}, GET
//! /v1/providers/{provider_id}/models, POST
//! /v1/providers/{provider_id}/default-model, GET/POST
//! /v1/providers/{provider_id}/checks, and GET
//! /v1/providers/{provider_id}/checks/{check_id}.
//!
//! Tenant integration (Roadmap 76 pre-soak): with a resolved tenant context
//! the auth read requires the integrations-manage credential permission and
//! reads the tenant-scoped state; the managed-auth actions run the
//! per-tenant variants and persist under the R37 composite storage key with
//! the row bound to the tenant. Go's RunCheck builds a failed Check record
//! for run errors; the Rust manager surfaces the error instead, so the
//! handler synthesizes the failed record before persisting it.

use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use kura_events as events;
use kura_providers as providers;

use crate::error::ApiError;
use crate::middleware::{environment_scope_from_config, TenantContext};
use crate::state::AppState;

use super::{decode_json_or_default, decode_json_required};

/// Body of `PUT /v1/providers/{provider_id}/credential`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetCredentialRequest {
    /// The bearer to send from now on. An empty string clears it.
    #[serde(default)]
    api_key: String,
}

/// Replaces the credential a configured provider sends, without a restart.
///
/// This exists for OAuth: an access token lasts about an hour while the daemon
/// runs for far longer, so whatever holds the grant has to be able to hand the
/// refreshed token over in place. Restarting instead would drop any run in
/// flight, and letting the token go stale turns every later dispatch into a
/// 401 that reads as a configuration fault.
///
/// Only the configured HTTP provider has a credential to replace. The managed
/// bridges borrow a CLI's own session and hold nothing to swap, so asking for
/// one is a 404 rather than a silent success.
async fn set_credential(
    State(state): State<AppState>,
    Path(provider_id): Path<String>,
    body: Bytes,
) -> Result<Response, ApiError> {
    if provider_id != "openai_compatible" {
        return Err(ApiError::NotFound(format!(
            "provider has no replaceable credential: {provider_id}"
        )));
    }
    // Required, not defaulted: a malformed or empty body must not be read as
    // "clear the credential", which would silently disable the provider.
    let request: SetCredentialRequest = decode_json_required(&body)?;
    let Some(credential) = state.openai_credential.clone() else {
        return Err(ApiError::NotFound(
            "the openai_compatible provider is not configured".to_string(),
        ));
    };
    let trimmed = request.api_key.trim();
    credential.set((!trimmed.is_empty()).then(|| trimmed.to_string()));
    // Never echoed back: it went in, and reading it out again is not something
    // this endpoint should make possible.
    Ok((StatusCode::OK, Json(serde_json::json!({"updated": true}))).into_response())
}

/// Route family router.
#[must_use]
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/providers", get(list_providers))
        .route("/v1/providers/{provider_id}", get(get_provider))
        .route("/v1/providers/{provider_id}/auth", get(get_auth_state))
        .route("/v1/providers/{provider_id}/auth/{action}", post(auth_action))
        .route("/v1/providers/{provider_id}/models", get(list_models))
        .route("/v1/providers/{provider_id}/default-model", post(set_default_model))
        .route("/v1/providers/{provider_id}/credential", put(set_credential))
        .route("/v1/providers/{provider_id}/checks", get(list_checks).post(run_check))
        .route("/v1/providers/{provider_id}/checks/{check_id}", get(get_check))
        .route("/v1/model-roles", get(list_model_roles))
        .route("/v1/model-roles/{role}", put(set_model_role).delete(clear_model_role))
}

#[derive(Debug, Serialize)]
struct ProviderListResponse {
    items: Vec<providers::Profile>,
    /// Present only when `?include=models`, keyed by provider id. Kept beside
    /// `items` rather than inlined so the `Profile` wire shape is unchanged
    /// for clients that do not ask for models.
    #[serde(skip_serializing_if = "Option::is_none")]
    models: Option<std::collections::BTreeMap<String, Vec<providers::Model>>>,
}

#[derive(Debug, Default, Deserialize)]
struct ProviderListQuery {
    /// Comma-separated expansions. Only `models` is recognized; unknown values
    /// are ignored so a newer client cannot break against an older daemon.
    #[serde(default)]
    include: Option<String>,
}

impl ProviderListQuery {
    fn includes_models(&self) -> bool {
        self.include
            .as_deref()
            .unwrap_or_default()
            .split(',')
            .any(|part| part.trim().eq_ignore_ascii_case("models"))
    }
}

#[derive(Debug, Serialize)]
struct ProviderAuthStateResponse {
    auth: providers::AuthState,
}

#[derive(Debug, Serialize)]
struct ProviderModelListResponse {
    items: Vec<providers::Model>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderDefaultModelResponse {
    provider_id: String,
    default_model: String,
    updated_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Serialize)]
struct ProviderCheckListResponse {
    items: Vec<providers::Check>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct ProviderDefaultModelRequest {
    model: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelRoleResource {
    role: String,
    provider_id: String,
    model: String,
    /// False when no provider serves the role. Callers must treat an unrouted
    /// role as "capability unavailable" rather than falling back to the
    /// default provider, so a missing image model is visible instead of a text
    /// model being asked for a picture.
    routed: bool,
    /// Where the binding came from: a stored assignment or daemon config.
    source: &'static str,
}

#[derive(Debug, Serialize)]
struct ModelRoleListResponse {
    items: Vec<ModelRoleResource>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelRoleRequest {
    provider_id: String,
    #[serde(default)]
    model: String,
}

fn parse_role(raw: &str) -> Result<kura_config::ModelRole, ApiError> {
    kura_config::ModelRole::parse(raw)
        .ok_or_else(|| ApiError::BadRequest(format!("unknown model role: {raw}")))
}

/// Resolve every role: a stored binding wins, otherwise daemon config, and
/// `primary` finally falls back to the default provider so deployments that
/// never configured roles keep working.
fn resolve_roles(state: &AppState) -> Result<Vec<ModelRoleResource>, ApiError> {
    let stored = state
        .store
        .lock()
        .list_model_role_bindings()
        .map_err(ApiError::from_store)?;
    let llm = &state.config.llm;
    let mut items = Vec::with_capacity(kura_config::ModelRole::ALL.len());
    for role in kura_config::ModelRole::ALL {
        if let Some(binding) = stored.iter().find(|candidate| candidate.role == role) {
            items.push(ModelRoleResource {
                role: role.as_str().to_string(),
                provider_id: binding.provider_id.clone(),
                model: binding.model.clone(),
                routed: !binding.provider_id.trim().is_empty(),
                source: "store",
            });
            continue;
        }
        let configured = llm.roles.get(role);
        let (provider_id, model) = if configured.is_routed() {
            (configured.provider.clone(), configured.model.clone())
        } else if role == kura_config::ModelRole::Primary {
            (llm.default_provider.clone(), llm.default_model.clone())
        } else {
            (String::new(), String::new())
        };
        let routed = !provider_id.trim().is_empty();
        items.push(ModelRoleResource {
            role: role.as_str().to_string(),
            provider_id,
            model,
            routed,
            source: if routed { "config" } else { "unrouted" },
        });
    }
    Ok(items)
}

async fn list_model_roles(
    State(state): State<AppState>,
) -> Result<Json<ModelRoleListResponse>, ApiError> {
    Ok(Json(ModelRoleListResponse { items: resolve_roles(&state)? }))
}

async fn set_model_role(
    State(state): State<AppState>,
    Path(role): Path<String>,
    body: Bytes,
) -> Result<Json<ModelRoleResource>, ApiError> {
    let role = parse_role(&role)?;
    let request: ModelRoleRequest = decode_json_required(&body)?;
    let provider_id = request.provider_id.trim().to_string();
    if provider_id.is_empty() {
        return Err(ApiError::BadRequest(
            "providerId is required; use DELETE to unroute a role".to_string(),
        ));
    }
    // Routing to a provider that does not exist would fail only at dispatch
    // time, far from the change that caused it.
    let manager = manager(&state)?;
    if manager.get_profile(&provider_id).is_none() {
        return Err(ApiError::NotFound(format!("unknown provider: {provider_id}")));
    }
    let binding = providers::RoleBinding {
        role,
        provider_id,
        model: request.model.trim().to_string(),
        updated_at: Utc::now(),
    };
    state
        .store
        .lock()
        .upsert_model_role_binding(&binding)
        .map_err(ApiError::from_store)?;

    let mut payload = serde_json::Map::new();
    payload.insert("role".to_string(), serde_json::json!(role.as_str()));
    payload.insert("providerId".to_string(), serde_json::json!(binding.provider_id));
    payload.insert("model".to_string(), serde_json::json!(binding.model));
    publish_provider_event(
        &state,
        "provider.model_role_changed",
        "model_role",
        role.as_str(),
        payload,
    )?;
    Ok(Json(ModelRoleResource {
        role: role.as_str().to_string(),
        provider_id: binding.provider_id,
        model: binding.model,
        routed: true,
        source: "store",
    }))
}

async fn clear_model_role(
    State(state): State<AppState>,
    Path(role): Path<String>,
) -> Result<StatusCode, ApiError> {
    let role = parse_role(&role)?;
    state
        .store
        .lock()
        .delete_model_role_binding(role)
        .map_err(ApiError::from_store)?;
    let mut payload = serde_json::Map::new();
    payload.insert("role".to_string(), serde_json::json!(role.as_str()));
    publish_provider_event(
        &state,
        "provider.model_role_cleared",
        "model_role",
        role.as_str(),
        payload,
    )?;
    Ok(StatusCode::NO_CONTENT)
}

fn manager(state: &AppState) -> Result<&providers::Manager, ApiError> {
    state
        .providers
        .as_deref()
        .ok_or_else(|| ApiError::internal("provider manager is not configured"))
}

/// Go writeCredentialDenial body.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CredentialDenial {
    error: &'static str,
    reason_code: &'static str,
}

fn credential_denial(reason_code: &'static str) -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(CredentialDenial { error: "credential_access_denied", reason_code }),
    )
        .into_response()
}

/// Go requireHostedCredentialReadAny/Permission over IntegrationsManage:
/// with a resolved tenant the caller must hold credential-inspection rights.
/// Returns the tenant id ("" without a tenant context) or the denial.
fn hosted_credential_tenant(
    tenant: &Option<Extension<TenantContext>>,
) -> Result<String, Response> {
    let Some(tc) = tenant.as_ref().map(|extension| &extension.0.0) else {
        return Ok(String::new());
    };
    if tc.tenant_id.trim().is_empty() {
        return Ok(String::new());
    }
    if !kura_identity::can_inspect_credentials(
        tc,
        &[kura_identity::Permission::IntegrationsManage],
    ) {
        return Err(credential_denial("missing_permission"));
    }
    Ok(tc.tenant_id.trim().to_string())
}

/// Go persistManagedProviderState: the tenant path stores the auth state
/// under the R37 composite key with the row tenant-bound; models replace
/// under the plain provider id in both paths.
fn persist_managed_provider_state(
    state: &AppState,
    tenant_id: &str,
    auth: &providers::AuthState,
    models: &[providers::Model],
) -> Result<(), ApiError> {
    let store = state.store.lock();
    if tenant_id.is_empty() {
        store
            .upsert_provider_auth_state(auth)
            .map_err(ApiError::from_store)?;
    } else {
        let mut stored = auth.clone();
        stored.tenant_id = tenant_id.to_string();
        stored.provider_id = format!("{}::{}", tenant_id, auth.provider_id.trim());
        store
            .upsert_provider_auth_state(&stored)
            .map_err(ApiError::from_store)?;
        store
            .bind_row_tenant(
                "provider_auth_states",
                "provider_id",
                &stored.provider_id,
                tenant_id,
            )
            .map_err(ApiError::from_store)?;
    }
    store
        .replace_provider_models(&auth.provider_id, models)
        .map_err(ApiError::from_store)
}

/// Go llmPrepareStatusCode over ProvidersError: model/auth validation is 400,
/// the rest is 500.
fn map_providers_error(err: &providers::ProvidersError) -> ApiError {
    match err {
        providers::ProvidersError::ModelNotSupported { .. }
        | providers::ProvidersError::ManagedAuthUnsupported
        | providers::ProvidersError::Prepare(_) => ApiError::BadRequest(err.to_string()),
        _ => ApiError::Internal(err.to_string()),
    }
}

fn publish_provider_event(
    state: &AppState,
    name: &str,
    resource_kind: &str,
    resource_id: &str,
    payload: serde_json::Map<String, serde_json::Value>,
) -> Result<(), ApiError> {
    let event = events::Event {
        category: "provider".to_string(),
        name: name.to_string(),
        environment_scope: environment_scope_from_config(&state.config),
        resource: events::Resource {
            kind: resource_kind.to_string(),
            id: resource_id.to_string(),
        },
        payload,
        ..events::Event::default()
    };
    let stored = state
        .store
        .lock()
        .append_event(&event)
        .map_err(ApiError::from_store)?;
    state.event_bus.publish(stored);
    Ok(())
}

fn json_value<T: Serialize>(value: &T) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or(serde_json::Value::Null)
}

/// Go publishProviderAuthEvent payload.
fn auth_event_payload(auth: &providers::AuthState) -> serde_json::Map<String, serde_json::Value> {
    let mut payload = serde_json::Map::new();
    payload.insert("tenantId".to_string(), serde_json::json!(auth.tenant_id));
    payload.insert("providerId".to_string(), serde_json::json!(auth.provider_id));
    payload.insert("family".to_string(), json_value(&auth.family));
    payload.insert("authMode".to_string(), json_value(&auth.auth_mode));
    payload.insert("status".to_string(), json_value(&auth.status));
    payload.insert("cliAvailable".to_string(), serde_json::json!(auth.cli_available));
    payload.insert("accountLabel".to_string(), serde_json::json!(auth.account_label));
    payload.insert("accountId".to_string(), serde_json::json!(auth.account_id));
    payload.insert("plan".to_string(), serde_json::json!(auth.plan));
    payload.insert("authMethod".to_string(), serde_json::json!(auth.auth_method));
    payload.insert("lastError".to_string(), serde_json::json!(auth.last_error));
    if !auth.metadata.is_empty() {
        payload.insert("metadata".to_string(), json_value(&auth.metadata));
    }
    if auth.sandbox.is_some() {
        payload.insert("sandbox".to_string(), json_value(&auth.sandbox));
    }
    payload
}

/// Go publishProviderCheckEvent payload.
fn check_event_payload(check: &providers::Check) -> serde_json::Map<String, serde_json::Value> {
    let mut payload = serde_json::Map::new();
    payload.insert("providerId".to_string(), serde_json::json!(check.provider_id));
    payload.insert("family".to_string(), json_value(&check.family));
    payload.insert("authMode".to_string(), json_value(&check.auth_mode));
    payload.insert("status".to_string(), json_value(&check.status));
    payload.insert("model".to_string(), serde_json::json!(check.model));
    payload.insert("endpoint".to_string(), serde_json::json!(check.endpoint));
    payload.insert("usage".to_string(), json_value(&check.usage));
    if !check.error_class.is_empty() {
        payload.insert("errorClass".to_string(), serde_json::json!(check.error_class));
    }
    if !check.error_code.is_empty() {
        payload.insert("errorCode".to_string(), serde_json::json!(check.error_code));
    }
    if !check.error_message.is_empty() {
        payload.insert("errorMessage".to_string(), serde_json::json!(check.error_message));
    }
    payload
}

/// GET /v1/providers (Go handleProviders).
async fn list_providers(
    State(state): State<AppState>,
    Query(query): Query<ProviderListQuery>,
) -> Result<Json<ProviderListResponse>, ApiError> {
    let manager = manager(&state)?;
    let items = manager.list_profiles();
    let models = query.includes_models().then(|| {
        items
            .iter()
            .map(|profile| {
                let models = manager.list_models(&profile.provider_id).unwrap_or_default();
                (profile.provider_id.clone(), models)
            })
            .collect()
    });
    Ok(Json(ProviderListResponse { items, models }))
}

/// GET /v1/providers/{provider_id} (Go handleProviderRoutes profile branch).
async fn get_provider(
    State(state): State<AppState>,
    Path(provider_id): Path<String>,
) -> Result<Json<providers::Profile>, ApiError> {
    let manager = manager(&state)?;
    manager
        .get_profile(provider_id.trim())
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("not found".to_string()))
}

/// GET /v1/providers/{provider_id}/auth (Go auth-state branch).
async fn get_auth_state(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(provider_id): Path<String>,
) -> Response {
    let manager = match manager(&state) {
        Ok(manager) => manager,
        Err(err) => return err.into_response(),
    };
    let provider_id = provider_id.trim();
    if manager.get_profile(provider_id).is_none() {
        return ApiError::NotFound("not found".to_string()).into_response();
    }
    let tenant_id = match hosted_credential_tenant(&tenant) {
        Ok(tenant_id) => tenant_id,
        Err(denial) => return denial,
    };
    let auth = if tenant_id.is_empty() {
        manager.get_auth_state(provider_id)
    } else {
        manager.get_auth_state_for_tenant(provider_id, &tenant_id)
    };
    match auth {
        Some(auth) => Json(ProviderAuthStateResponse { auth }).into_response(),
        None => ApiError::NotFound("not found".to_string()).into_response(),
    }
}

/// POST /v1/providers/{provider_id}/auth/{start|complete|refresh|revoke}
/// (Go managed-auth branch): run the managed flow, persist the auth state
/// and model list, and publish the provider.auth_* event.
async fn auth_action(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path((provider_id, action)): Path<(String, String)>,
) -> Response {
    let manager = match manager(&state) {
        Ok(manager) => manager,
        Err(err) => return err.into_response(),
    };
    let provider_id = provider_id.trim().to_string();
    if manager.get_profile(&provider_id).is_none() {
        return ApiError::NotFound("not found".to_string()).into_response();
    }
    let tenant_id = match hosted_credential_tenant(&tenant) {
        Ok(tenant_id) => tenant_id,
        Err(denial) => return denial,
    };
    let (result, event_name) = if tenant_id.is_empty() {
        match action.as_str() {
            "start" => (manager.start_managed_auth(&provider_id).await, "provider.auth_started"),
            "complete" => {
                (manager.complete_managed_auth(&provider_id).await, "provider.auth_completed")
            }
            "refresh" => {
                (manager.refresh_managed_auth(&provider_id).await, "provider.auth_refreshed")
            }
            "revoke" => (manager.revoke_managed_auth(&provider_id).await, "provider.auth_revoked"),
            _ => return ApiError::NotFound("not found".to_string()).into_response(),
        }
    } else {
        match action.as_str() {
            "start" => (
                manager.start_managed_auth_for_tenant(&provider_id, &tenant_id).await,
                "provider.auth_started",
            ),
            "complete" => (
                manager.complete_managed_auth_for_tenant(&provider_id, &tenant_id).await,
                "provider.auth_completed",
            ),
            "refresh" => (
                manager.refresh_managed_auth_for_tenant(&provider_id, &tenant_id).await,
                "provider.auth_refreshed",
            ),
            "revoke" => (
                manager.revoke_managed_auth_for_tenant(&provider_id, &tenant_id).await,
                "provider.auth_revoked",
            ),
            _ => return ApiError::NotFound("not found".to_string()).into_response(),
        }
    };
    let (auth, models) = match result {
        Ok(pair) => pair,
        Err(err) => return map_providers_error(&err).into_response(),
    };

    if let Err(err) = persist_managed_provider_state(&state, &tenant_id, &auth, &models) {
        return err.into_response();
    }
    if let Err(err) = publish_provider_event(
        &state,
        event_name,
        "provider_auth",
        &auth.provider_id,
        auth_event_payload(&auth),
    ) {
        return err.into_response();
    }
    Json(ProviderAuthStateResponse { auth }).into_response()
}

/// GET /v1/providers/{provider_id}/models (Go models branch).
async fn list_models(
    State(state): State<AppState>,
    Path(provider_id): Path<String>,
) -> Result<Json<ProviderModelListResponse>, ApiError> {
    let manager = manager(&state)?;
    let provider_id = provider_id.trim();
    if manager.get_profile(provider_id).is_none() {
        return Err(ApiError::NotFound("not found".to_string()));
    }
    manager
        .list_models(provider_id)
        .map(|items| Json(ProviderModelListResponse { items }))
        .ok_or_else(|| ApiError::NotFound("not found".to_string()))
}

/// POST /v1/providers/{provider_id}/default-model (Go default-model branch).
async fn set_default_model(
    State(state): State<AppState>,
    Path(provider_id): Path<String>,
    body: Bytes,
) -> Result<Json<ProviderDefaultModelResponse>, ApiError> {
    let request: ProviderDefaultModelRequest = decode_json_required(&body)?;
    let manager = manager(&state)?;
    let provider_id = provider_id.trim();
    if manager.get_profile(provider_id).is_none() {
        return Err(ApiError::NotFound("not found".to_string()));
    }
    let preference = manager
        .set_default_model(provider_id, request.model.trim())
        .map_err(|err| map_providers_error(&err))?;
    state
        .store
        .lock()
        .upsert_provider_preference(&preference)
        .map_err(ApiError::from_store)?;

    let mut payload = serde_json::Map::new();
    payload.insert("providerId".to_string(), serde_json::json!(preference.provider_id));
    payload.insert("defaultModel".to_string(), serde_json::json!(preference.default_model));
    publish_provider_event(
        &state,
        "provider.default_model_changed",
        "provider_preference",
        &preference.provider_id,
        payload,
    )?;
    Ok(Json(ProviderDefaultModelResponse {
        provider_id: preference.provider_id,
        default_model: preference.default_model,
        updated_at: preference.updated_at,
    }))
}

/// GET /v1/providers/{provider_id}/checks (Go checks list branch).
async fn list_checks(
    State(state): State<AppState>,
    Path(provider_id): Path<String>,
) -> Result<Json<ProviderCheckListResponse>, ApiError> {
    let manager = manager(&state)?;
    let provider_id = provider_id.trim();
    if manager.get_profile(provider_id).is_none() {
        return Err(ApiError::NotFound("not found".to_string()));
    }
    let items = state
        .store
        .lock()
        .list_provider_checks(provider_id)
        .map_err(ApiError::from_store)?;
    Ok(Json(ProviderCheckListResponse { items }))
}

/// POST /v1/providers/{provider_id}/checks (Go checks run branch) — 201 with
/// the persisted check, passed or failed.
async fn run_check(
    State(state): State<AppState>,
    Path(provider_id): Path<String>,
    body: Bytes,
) -> Result<(StatusCode, Json<providers::Check>), ApiError> {
    let input: providers::CheckInput = decode_json_or_default(&body)?;
    let manager = manager(&state)?;
    let provider_id = provider_id.trim().to_string();
    let Some(profile) = manager.get_profile(&provider_id) else {
        return Err(ApiError::NotFound("not found".to_string()));
    };

    let check_id = providers::new_check_id();
    let (check, event_name) = match manager.run_check(&provider_id, &check_id, input).await {
        Ok(check) => (check, "provider.check_completed"),
        // Go RunCheck returns a failed Check alongside the error; the Rust
        // manager surfaces only the error, so synthesize the failed record.
        Err(err) => (
            providers::Check {
                check_id,
                provider_id: profile.provider_id.clone(),
                family: profile.family,
                auth_mode: profile.auth_mode,
                status: providers::CheckStatus::Failed,
                error_class: "check_failed".to_string(),
                error_message: err.to_string(),
                created_at: Utc::now(),
                completed_at: Utc::now(),
                ..providers::Check::default()
            },
            "provider.check_failed",
        ),
    };
    state
        .store
        .lock()
        .upsert_provider_check(&check)
        .map_err(ApiError::from_store)?;
    publish_provider_event(
        &state,
        event_name,
        "provider_check",
        &check.provider_id,
        check_event_payload(&check),
    )?;
    Ok((StatusCode::CREATED, Json(check)))
}

/// GET /v1/providers/{provider_id}/checks/{check_id} (Go check detail).
async fn get_check(
    State(state): State<AppState>,
    Path((provider_id, check_id)): Path<(String, String)>,
) -> Result<Json<providers::Check>, ApiError> {
    manager(&state)?;
    let check = state
        .store
        .lock()
        .get_provider_check(provider_id.trim(), check_id.trim())
        .map_err(ApiError::from_store)?;
    check
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("not found".to_string()))
}

#[cfg(test)]
mod tests {
    use super::super::tests_support::{request_json, test_state};
    use axum::http::StatusCode;
    use std::sync::Arc;

    /// A manager with one provider actually configured.
    ///
    /// Nothing is listed until something is set up, so a test that wants to
    /// read a provider has to configure one. It used to rely on the inventory
    /// being seeded with built-ins whatever the configuration said, which is
    /// the behaviour these routes no longer have.
    /// A manager with one provider actually configured.
    ///
    /// Asking for echo by name is what puts it in the inventory now, so this
    /// doubles as the check that an explicitly configured provider appears.
    /// The routes used to be exercised against whatever the inventory happened
    /// to be seeded with regardless of configuration.
    fn state_with_manager() -> crate::state::AppState {
        let mut state = test_state();
        let mut llm = state.config.llm.clone();
        llm.default_provider = "echo".to_string();
        let manager = kura_providers::new_manager(llm, None, Vec::new());
        state.providers = Some(Arc::new(manager));
        state
    }

    #[tokio::test]
    async fn nothing_is_listed_until_something_is_configured() {
        // An untouched daemon is empty, not broken. Seeding the inventory made
        // a fresh install list several providers, most of them reporting
        // faults, with no way for a user to remove any of them.
        let mut state = test_state();
        let manager = kura_providers::new_manager(state.config.llm.clone(), None, Vec::new());
        state.providers = Some(Arc::new(manager));

        let (status, listed) = request_json(state, "GET", "/v1/providers", None).await;

        assert_eq!(status, StatusCode::OK, "{listed}");
        assert!(listed["items"].as_array().expect("items").is_empty(), "{listed}");
    }

    #[tokio::test]
    async fn list_get_models_and_default_model() {
        let state = state_with_manager();
        let (status, listed) = request_json(state.clone(), "GET", "/v1/providers", None).await;
        assert_eq!(status, StatusCode::OK, "{listed}");
        let items = listed["items"].as_array().expect("items");
        assert!(!items.is_empty(), "{listed}");
        let provider_id = items[0]["providerId"].as_str().expect("providerId").to_string();

        let (status, fetched) =
            request_json(state.clone(), "GET", &format!("/v1/providers/{provider_id}"), None).await;
        assert_eq!(status, StatusCode::OK, "{fetched}");

        let (status, models) = request_json(
            state.clone(),
            "GET",
            &format!("/v1/providers/{provider_id}/models"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{models}");

        let (status, _) =
            request_json(state, "GET", "/v1/providers/provider_missing", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
