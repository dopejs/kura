//! tools route family (Stage 9.1b,
//! `docs/providers/tool-provider-architecture.md`).
//!
//! Routes: GET /v1/tools/capabilities, GET|POST /v1/tools/profiles,
//! PATCH|DELETE /v1/tools/profiles/{profile_id}, POST
//! /v1/tools/profiles/{profile_id}/check.
//!
//! **Credentials are never written or read here.** A profile carries a
//! `secretRef`; values are written through the tenant-secret routes, which is
//! the one way to store a secret in this system. The only place a value is
//! touched is `/check`, where it is resolved, tested for presence, and
//! dropped — it is never returned, logged, or persisted.
//!
//! **Tenancy.** Profiles are per tenant: the manager is keyed by tenant id and
//! the by-id routes verify ownership before acting, so one tenant's search
//! provider is neither listed nor addressable by another.

use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;

use kura_tools as tools;

use crate::error::ApiError;
use crate::middleware::TenantContext;
use crate::state::AppState;

/// Screens a profile's egress target against the daemon's outbound policy.
///
/// Configuration time is the right place for the **syntactic** check: the host
/// may not resolve yet, and the operator needs a stable answer. Call-time
/// egress additionally resolves — see `kura_egress` for why the two are
/// separate calls rather than one that sometimes resolves.
fn screen_base_url(state: &AppState, base_url: &str) -> Result<(), ApiError> {
    let url = base_url.trim();
    if url.is_empty() {
        return Ok(());
    }
    state
        .config
        .egress
        .check_url(url)
        .map(|_| ())
        .map_err(|denial| ApiError::BadRequest(denial.to_string()))
}

use super::{decode_json_or_default, decode_json_required};

#[must_use]
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/tools/capabilities", get(list_capabilities))
        .route(
            "/v1/tools/profiles",
            get(list_profiles).post(create_profile),
        )
        .route(
            "/v1/tools/profiles/{profile_id}",
            axum::routing::patch(update_profile).delete(delete_profile),
        )
        .route("/v1/tools/profiles/{profile_id}/check", post(check_profile))
}

#[derive(Debug, Serialize)]
struct CapabilityListResponse {
    items: Vec<tools::CapabilityStatus>,
}

#[derive(Debug, Serialize)]
struct ProfileListResponse {
    items: Vec<tools::ToolProfile>,
}

fn manager(state: &AppState) -> Result<&tools::Manager, ApiError> {
    state
        .tools
        .as_deref()
        .ok_or_else(|| ApiError::internal("tool profile manager is not configured"))
}

fn map_error(err: tools::ToolsError) -> ApiError {
    match err {
        tools::ToolsError::NotFound => ApiError::NotFound("not found".to_string()),
        other => ApiError::BadRequest(other.to_string()),
    }
}

fn acting_tenant(tenant: Option<&TenantContext>, fallback: &str) -> String {
    if let Some(tc) = tenant {
        let id = tc.0.tenant_id.trim();
        if !id.is_empty() {
            return id.to_string();
        }
    }
    fallback.trim().to_string()
}

/// Verifies the profile belongs to the acting tenant before acting on it.
/// Answers 404 rather than 403 for another tenant's profile: the existence of
/// a profile is itself information.
fn owned_profile(
    manager: &tools::Manager,
    tenant_id: &str,
    profile_id: &str,
) -> Result<tools::ToolProfile, ApiError> {
    let profile = manager
        .get(profile_id.trim())
        .ok_or_else(|| ApiError::NotFound("not found".to_string()))?;
    if profile.tenant_id != tenant_id {
        return Err(ApiError::NotFound("not found".to_string()));
    }
    Ok(profile)
}

fn persist(state: &AppState, profile: &tools::ToolProfile) -> Result<(), ApiError> {
    state
        .store
        .lock()
        .upsert_tool_profile(profile)
        .map_err(ApiError::from_store)
}

/// GET /v1/tools/capabilities — what the agent could do here.
///
/// Reports every capability the daemon knows about, configured or not: "this
/// deployment cannot search" is an answer the caller needs, not an omission.
async fn list_capabilities(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<CapabilityListResponse>, ApiError> {
    let manager = manager(&state)?;
    let tenant_id = acting_tenant(
        tenant.as_ref().map(|e| &e.0),
        params.get("tenantId").map_or("", String::as_str),
    );
    Ok(Json(CapabilityListResponse {
        items: manager.capabilities(&tenant_id),
    }))
}

/// GET /v1/tools/profiles — the tenant's profiles. Carries `secretRef` and
/// `secretConfigured`; never a credential value.
async fn list_profiles(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<ProfileListResponse>, ApiError> {
    let manager = manager(&state)?;
    let tenant_id = acting_tenant(
        tenant.as_ref().map(|e| &e.0),
        params.get("tenantId").map_or("", String::as_str),
    );
    let capability = params
        .get("capability")
        .map(String::as_str)
        .and_then(tools::Capability::parse);
    Ok(Json(ProfileListResponse {
        items: manager.list(&tenant_id, capability),
    }))
}

/// POST /v1/tools/profiles — 201 with the created profile.
async fn create_profile(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<(StatusCode, Json<tools::ToolProfile>), ApiError> {
    let input: tools::CreateProfileInput = decode_json_required(&body)?;
    let manager = manager(&state)?;
    screen_base_url(&state, &input.base_url)?;
    let tenant_id = acting_tenant(tenant.as_ref().map(|e| &e.0), "");
    let profile = manager.create(&tenant_id, input).map_err(map_error)?;
    // Promoting a default demotes the previous one in the manager; both rows
    // must be persisted or a restart would restore two defaults.
    for sibling in manager.list(&tenant_id, Some(profile.capability)) {
        persist(&state, &sibling)?;
    }
    Ok((StatusCode::CREATED, Json(profile)))
}

/// PATCH /v1/tools/profiles/{profile_id} — edits configuration.
///
/// `secretRef` is editable because pointing a profile at a different secret is
/// configuration. The secret's **value** is not reachable from this route.
async fn update_profile(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(profile_id): Path<String>,
    body: Bytes,
) -> Result<Json<tools::ToolProfile>, ApiError> {
    let input: tools::UpdateProfileInput = decode_json_or_default(&body)?;
    let manager = manager(&state)?;
    if let Some(base_url) = input.base_url.as_deref() {
        screen_base_url(&state, base_url)?;
    }
    let tenant_id = acting_tenant(tenant.as_ref().map(|e| &e.0), "");
    let existing = owned_profile(manager, &tenant_id, &profile_id)?;
    let updated = manager
        .update(&existing.profile_id, input)
        .map_err(map_error)?;
    for sibling in manager.list(&tenant_id, Some(updated.capability)) {
        persist(&state, &sibling)?;
    }
    Ok(Json(updated))
}

/// DELETE /v1/tools/profiles/{profile_id}.
async fn delete_profile(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(profile_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let manager = manager(&state)?;
    let tenant_id = acting_tenant(tenant.as_ref().map(|e| &e.0), "");
    let existing = owned_profile(manager, &tenant_id, &profile_id)?;
    manager.delete(&existing.profile_id).map_err(map_error)?;
    state
        .store
        .lock()
        .delete_tool_profile(&existing.profile_id)
        .map_err(ApiError::from_store)?;
    Ok(StatusCode::NO_CONTENT)
}

/// POST /v1/tools/profiles/{profile_id}/check — preflight.
///
/// The one place a credential value is touched: resolved through the tenant
/// secret plane, tested for presence, and dropped. Without this route a
/// mistyped key surfaces as a failed turn in front of the user; with it, it
/// surfaces where the operator is already standing.
async fn check_profile(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(profile_id): Path<String>,
) -> Result<Json<tools::CheckOutcome>, ApiError> {
    let manager = manager(&state)?;
    let tenant_id = acting_tenant(tenant.as_ref().map(|e| &e.0), "");
    let profile = owned_profile(manager, &tenant_id, &profile_id)?;

    let credential = resolve_credential(&state, &tenant_id, &profile).await;
    Ok(Json(tools::check_profile(&profile, credential.as_deref())))
}

/// Resolves the profile's credential reference, or `None`.
///
/// A resolution failure is deliberately indistinguishable from "no reference"
/// at this layer: both mean the profile cannot authenticate, and
/// `check_profile` reports that as `auth_error`. The failure reason is not
/// propagated because secret-plane errors can echo the reference.
async fn resolve_credential(
    state: &AppState,
    tenant_id: &str,
    profile: &tools::ToolProfile,
) -> Option<String> {
    let secret_ref = profile.secret_ref.trim();
    if secret_ref.is_empty() {
        return None;
    }
    let secrets = state.secrets.clone()?;
    let resolved = secrets
        .resolve(kura_secrets::ResolveInput {
            tenant_id: tenant_id.to_string(),
            secret_ref: secret_ref.to_string(),
        })
        .await
        .ok()?;
    Some(resolved.value)
}

#[cfg(test)]
mod tests {
    use super::super::tests_support::{request_json, request_json_as_tenant, test_state};
    use axum::http::StatusCode;
    use std::sync::Arc;

    fn state_with_manager() -> crate::state::AppState {
        let mut state = test_state();
        state.tools = Some(Arc::new(kura_tools::Manager::new()));
        state
    }

    fn profile_body(title: &str) -> serde_json::Value {
        serde_json::json!({
            "title": title,
            "capability": "web.search",
            "family": "builtin_stub",
            "authMode": "api_key",
            "secretRef": "secret://tools/search-key"
        })
    }

    #[tokio::test]
    async fn create_list_check_and_delete_a_profile() {
        let state = state_with_manager();

        // Nothing configured: capabilities still enumerate, so the caller can
        // tell "cannot search" from "route missing".
        let (status, caps) =
            request_json(state.clone(), "GET", "/v1/tools/capabilities", None).await;
        assert_eq!(status, StatusCode::OK, "{caps}");
        let items = caps["items"].as_array().expect("items");
        assert_eq!(items.len(), 4);
        assert!(items.iter().all(|c| c["configured"] == false));

        let (status, created) = request_json(
            state.clone(),
            "POST",
            "/v1/tools/profiles",
            Some(profile_body("search")),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let profile_id = created["profileId"]
            .as_str()
            .expect("profileId")
            .to_string();
        assert_eq!(created["readiness"], "ready");
        assert_eq!(created["secretConfigured"], true);

        let (status, caps) =
            request_json(state.clone(), "GET", "/v1/tools/capabilities", None).await;
        assert_eq!(status, StatusCode::OK);
        let search = caps["items"]
            .as_array()
            .expect("items")
            .iter()
            .find(|c| c["capability"] == "web.search")
            .expect("search");
        assert_eq!(search["configured"], true);
        assert_eq!(search["defaultProfileId"], profile_id.as_str());

        // /check with no secrets manager wired: the reference cannot resolve,
        // which is exactly the mistyped-key case, reported as auth_error.
        let (status, checked) = request_json(
            state.clone(),
            "POST",
            &format!("/v1/tools/profiles/{profile_id}/check"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{checked}");
        assert_eq!(checked["passed"], false);
        assert_eq!(checked["errorClass"], "auth_error");

        let (status, updated) = request_json(
            state.clone(),
            "PATCH",
            &format!("/v1/tools/profiles/{profile_id}"),
            Some(serde_json::json!({ "title": "renamed", "enabled": false })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{updated}");
        assert_eq!(updated["title"], "renamed");
        assert_eq!(updated["readiness"], "disabled");

        let (status, _) = request_json(
            state.clone(),
            "DELETE",
            &format!("/v1/tools/profiles/{profile_id}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        let (status, listed) = request_json(state.clone(), "GET", "/v1/tools/profiles", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(listed["items"].as_array().expect("items").is_empty());
    }

    /// The design's first hard rule, asserted at the API boundary rather than
    /// only on the struct: no response carries credential material.
    #[tokio::test]
    async fn no_response_carries_a_credential_value() {
        let state = state_with_manager();
        let (status, created) = request_json(
            state.clone(),
            "POST",
            "/v1/tools/profiles",
            Some(profile_body("search")),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);

        let (_, listed) = request_json(state.clone(), "GET", "/v1/tools/profiles", None).await;
        for body in [&created, &listed] {
            let text = body.to_string();
            // The reference is expected; a value-shaped field is not.
            assert!(text.contains("secretRef"), "{text}");
            for forbidden in ["\"apiKey\"", "\"token\"", "\"credential\"", "\"password\""] {
                assert!(!text.contains(forbidden), "{forbidden} leaked into {text}");
            }
        }
    }

    /// Profiles are per tenant: one tenant's provider is neither listed nor
    /// addressable by another, and the refusal is a 404.
    #[tokio::test]
    async fn profiles_are_scoped_to_the_acting_tenant() {
        let state = state_with_manager();

        let (status, created) = request_json_as_tenant(
            state.clone(),
            "tnt_a",
            "POST",
            "/v1/tools/profiles",
            Some(profile_body("a's search")),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let profile_id = created["profileId"]
            .as_str()
            .expect("profileId")
            .to_string();

        let (status, listed) =
            request_json_as_tenant(state.clone(), "tnt_a", "GET", "/v1/tools/profiles", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(listed["items"].as_array().expect("items").len(), 1);

        let (status, listed) =
            request_json_as_tenant(state.clone(), "tnt_b", "GET", "/v1/tools/profiles", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            listed["items"].as_array().expect("items").is_empty(),
            "{listed}"
        );

        // Another tenant sees no capability configured either.
        let (_, caps) = request_json_as_tenant(
            state.clone(),
            "tnt_b",
            "GET",
            "/v1/tools/capabilities",
            None,
        )
        .await;
        assert!(
            caps["items"]
                .as_array()
                .expect("items")
                .iter()
                .all(|c| c["configured"] == false)
        );

        for (method, suffix) in [("PATCH", ""), ("POST", "/check"), ("DELETE", "")] {
            let body = if method == "PATCH" {
                Some(serde_json::json!({ "title": "stolen" }))
            } else {
                None
            };
            let (status, _) = request_json_as_tenant(
                state.clone(),
                "tnt_b",
                method,
                &format!("/v1/tools/profiles/{profile_id}{suffix}"),
                body,
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{method} {suffix}");
        }
    }

    /// A credentialed profile with no reference is refused at configuration
    /// time rather than failing at call time.
    #[tokio::test]
    async fn a_credentialed_profile_without_a_reference_is_a_400() {
        let state = state_with_manager();
        let (status, body) = request_json(
            state.clone(),
            "POST",
            "/v1/tools/profiles",
            Some(serde_json::json!({
                "title": "search",
                "capability": "web.search",
                "family": "builtin_stub",
                "authMode": "api_key"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    }

    /// Stage 7.3: a profile's egress target faces the outbound policy at
    /// configuration time. The metadata address is the payload of most real
    /// SSRF, so a profile pointed at it is refused where the operator is
    /// standing rather than at call time.
    #[tokio::test]
    async fn a_profile_pointed_at_a_blocked_address_is_refused() {
        let state = state_with_manager();

        let attempt = |base_url: &str| {
            let state = state.clone();
            let base_url = base_url.to_string();
            async move {
                request_json(
                    state,
                    "POST",
                    "/v1/tools/profiles",
                    Some(serde_json::json!({
                        "title": "search",
                        "capability": "web.search",
                        "family": "builtin_stub",
                        "authMode": "none",
                        "baseUrl": base_url
                    })),
                )
                .await
            }
        };

        for blocked in [
            "http://169.254.169.254/latest/meta-data/",
            "http://[::ffff:169.254.169.254]/",
            "http://127.0.0.1:8080/",
            "http://10.0.0.5/",
            "file:///etc/passwd",
        ] {
            let (status, body) = attempt(blocked).await;
            assert_eq!(
                status,
                StatusCode::BAD_REQUEST,
                "{blocked} must be refused: {body}"
            );
        }

        let (status, created) = attempt("https://api.example.org/search").await;
        assert_eq!(status, StatusCode::CREATED, "{created}");

        // The same screen applies on edit, or a profile could be created clean
        // and then repointed.
        let profile_id = created["profileId"]
            .as_str()
            .expect("profileId")
            .to_string();
        let (status, _) = request_json(
            state.clone(),
            "PATCH",
            &format!("/v1/tools/profiles/{profile_id}"),
            Some(serde_json::json!({ "baseUrl": "http://169.254.169.254/" })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "edits are screened too");
    }

    /// Stage 9.2: an `mcp_backed` profile must name its server and tool;
    /// one that does is accepted with no URL and no secret (both belong to
    /// the MCP server's own configuration).
    #[tokio::test]
    async fn an_mcp_backed_profile_needs_its_server_and_tool() {
        let state = state_with_manager();
        let (status, body) = request_json(
            state.clone(),
            "POST",
            "/v1/tools/profiles",
            Some(serde_json::json!({
                "title": "my search",
                "capability": "web.search",
                "family": "mcp_backed",
                "authMode": "none"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

        let (status, body) = request_json(
            state,
            "POST",
            "/v1/tools/profiles",
            Some(serde_json::json!({
                "title": "my search",
                "capability": "web.search",
                "family": "mcp_backed",
                "authMode": "none",
                "mcpServerId": "mcp_search",
                "mcpToolName": "search"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        assert_eq!(body["family"], "mcp_backed");
        assert_eq!(body["mcpServerId"], "mcp_search");
        assert_eq!(body["mcpToolName"], "search");
        assert_eq!(body["readiness"], "ready");
    }
}
