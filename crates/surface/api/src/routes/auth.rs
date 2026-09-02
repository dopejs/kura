//! auth route family (port of daemon/internal/api/server.go inline auth/
//! tenant handlers plus auth_tokens.go, tenants.go, hosted_credentials.go).
//!
//! Routes under /v1/auth/*, /v1/tenants*, /v1/tenant-invitations,
//! /v1/principals, /v1/tenant-audit-events and /v1/tenant-secrets:
//! - POST /v1/auth/pairings/start, POST /v1/auth/pairings/{id}/complete
//!   (unauthenticated - Go withEnvironment)
//! - GET /v1/auth/me
//! - GET/POST /v1/auth/tokens, POST .../{tokenId}/rotate,
//!   POST .../{tokenId}/revoke, PATCH .../{tokenId}/tenant-grants
//! - GET/POST /v1/tenants, GET /v1/tenants/{tenantId},
//!   GET /v1/tenants/{tenantId}/memberships,
//!   PATCH|DELETE /v1/tenants/{tenantId}/memberships/{membershipId},
//!   GET|POST /v1/tenants/{tenantId}/invitations,
//!   GET /v1/tenants/{tenantId}/permissions
//! - GET /v1/tenant-invitations, POST .../{invitationId}/{accept|reject}
//! - GET /v1/principals, PATCH /v1/principals/{principalId}
//! - GET /v1/tenant-audit-events
//! - GET|POST /v1/tenant-secrets, GET|PATCH /v1/tenant-secrets/{ref},
//!   POST /v1/tenant-secrets/{ref}/{rotate|disable}
//!
//! Wire shape follows the Go json tags (camelCase); status-code mapping
//! mirrors the Go handlers (200/201/400/401/403/404), including the stable
//! tenant denial body ({error, errorCode}) and the credential denial body
//! ({error: "credential_access_denied", reasonCode}) for the 403s.
//!
//! Middleware note (same as the other route families): router() takes no
//! state (the outer mod::router owns it and applies .with_state at the
//! end), so axum's from_fn_with_state(protected) cannot be constructed here.
//! Handlers read the TenantContext / AuthenticatedToken extensions that
//! protected() installs once an auth manager is wired; with no auth manager
//! configured requests pass through unauthenticated, exactly like Go's
//! nil-auth behavior.
//!
//! Deliberately not ported (reported, not duplicated):
//! - the middleware-layer tenant-resolution denial audit record
//!   (tenant.access_denied / tenant.token_expiry_denied) - the Rust
//!   protected() middleware still defers that audit write; handler-level
//!   permission denials DO write the tenant.permission_denied audit exactly
//!   like Go's RequirePermission.
//! - kura-identity::Store for SQLiteStore does not exist in kura-store yet
//!   (crates/persistence/store has every backing method; the trait impl is
//!   missing). Tests bridge it with a local wrapper. Same for
//!   kura-secrets::Store (no SQLite impl in kura-store) - tests use a local
//!   in-memory store mirroring the secrets crate's FakeStore semantics.
//! - the tenant-by-id guard (withByIDTenantGuard) does not apply to this
//!   family in Go either; tenant scoping is the handler-level
//!   tenantContext.TenantID == tenantID check (403 denial otherwise).

use std::collections::HashMap;

use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde::Serialize;

use kura_identity::auth::{self, AccessToken, Pairing};
use kura_identity::{
    can_inspect_credentials, evaluate_permission, has_permission, permissions_for_role,
    AuditEventFilter, InvitationFilter, LifecycleStatus, MembershipFilter, Permission,
    PrincipalFilter, Role, Tenant, TenantAuditEvent, TokenTenantGrant,
};
use kura_secrets::SecretsError;

use crate::error::ApiError;
use crate::middleware::{AuthenticatedToken, TenantContext};
use crate::state::AppState;
use crate::types::{AuthMeResponse, ListResponse, TenantDetailResponse, TenantListResponse};

/// Unauthenticated pairing entry points (Go registers these with
/// withEnvironment instead of protected()); the assembly in routes/mod.rs
/// mounts this router outside the protected() layer.
#[must_use]
pub fn open_router() -> Router<AppState> {
    Router::new()
        .route("/v1/auth/pairings/start", post(auth_pairing_start))
        .route("/v1/auth/pairings/{pairing_id}/complete", post(auth_pairing_complete))
}

/// Route family router. Only the methods the Go handlers accept are
/// registered; axum answers the other methods with 405 (Go
/// w.WriteHeader(http.StatusMethodNotAllowed)). Pairing entry points live in
/// [`open_router`]; everything here runs behind protected().
#[must_use]
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/auth/me", get(auth_me))
        .route("/v1/auth/tokens", get(auth_tokens_list).post(auth_token_create))
        .route("/v1/auth/tokens/{token_id}/rotate", post(auth_token_rotate))
        .route("/v1/auth/tokens/{token_id}/revoke", post(auth_token_revoke))
        .route("/v1/auth/tokens/{token_id}/tenant-grants", patch(auth_token_grant_update))
        .route("/v1/tenants", get(tenants_list).post(tenant_create))
        .route("/v1/tenants/{tenant_id}", get(tenant_detail))
        .route("/v1/tenants/{tenant_id}/memberships", get(tenant_memberships_list))
        .route(
            "/v1/tenants/{tenant_id}/memberships/{membership_id}",
            patch(tenant_membership_update).delete(tenant_membership_remove),
        )
        .route(
            "/v1/tenants/{tenant_id}/invitations",
            get(tenant_invitations_list).post(tenant_invitation_create),
        )
        .route("/v1/tenants/{tenant_id}/permissions", get(tenant_permissions))
        .route("/v1/tenant-invitations", get(tenant_invitations_self_list))
        .route("/v1/tenant-invitations/{invitation_id}/accept", post(tenant_invitation_accept))
        .route("/v1/tenant-invitations/{invitation_id}/reject", post(tenant_invitation_reject))
        .route("/v1/principals", get(principals_list))
        .route("/v1/principals/{principal_id}", patch(principal_update))
        .route("/v1/tenant-audit-events", get(tenant_audit_events_list))
        .route("/v1/tenant-secrets", get(tenant_secrets_list).post(tenant_secret_create))
        .route(
            "/v1/tenant-secrets/{secret_ref}",
            get(tenant_secret_get).patch(tenant_secret_patch),
        )
        .route("/v1/tenant-secrets/{secret_ref}/rotate", post(tenant_secret_rotate))
        .route("/v1/tenant-secrets/{secret_ref}/disable", post(tenant_secret_disable))
}
// ---------------------------------------------------------------------------
// Handler error type (stable denial bodies, port of writeTenantDenial /
// writeCredentialDenial)
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum AuthApiError {
    Api(ApiError),
    /// 403 stable tenant denial (Go writeTenantDenial -> identity.StableDenial).
    TenantDenial,
    /// 403 credential denial (Go writeCredentialDenial):
    /// {"error": "credential_access_denied", "reasonCode": <reason>}.
    CredentialDenial { reason_code: &'static str },
}

impl From<ApiError> for AuthApiError {
    fn from(err: ApiError) -> Self {
        Self::Api(err)
    }
}

impl IntoResponse for AuthApiError {
    fn into_response(self) -> Response {
        match self {
            Self::Api(err) => err.into_response(),
            Self::TenantDenial => {
                (StatusCode::FORBIDDEN, Json(kura_identity::stable_denial())).into_response()
            }
            Self::CredentialDenial { reason_code } => (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({
                    "error": "credential_access_denied",
                    "reasonCode": reason_code,
                })),
            )
                .into_response(),
        }
    }
}

// ---------------------------------------------------------------------------
// Request DTOs (ports of the Go api-package inline structs)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateTokenRequest {
    #[serde(default)]
    label: String,
    #[serde(default)]
    expires_at: String,
    #[serde(default)]
    default_tenant_id: String,
    #[serde(default)]
    allowed_tenant_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RotateTokenRequest {
    #[serde(default)]
    expires_at: String,
    #[serde(default)]
    reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RevokeTokenRequest {
    #[serde(default)]
    reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GrantUpdateRequest {
    #[serde(default)]
    default_tenant_id: String,
    #[serde(default)]
    allowed_tenant_ids: Vec<String>,
    #[serde(default)]
    #[allow(dead_code)]
    reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateTenantRequest {
    display_name: String,
    #[serde(default)]
    tenant_kind: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateMembershipRoleRequest {
    role: Role,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateInvitationRequest {
    invited_principal_id: String,
    role: Role,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrincipalUpdateRequest {
    #[serde(default)]
    status: Option<LifecycleStatus>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateTenantSecretRequest {
    secret_ref: String,
    #[serde(default)]
    display_name: String,
    value: String,
    #[serde(default)]
    document: Option<kura_secrets::Document>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateTenantSecretRequest {
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    document: Option<kura_secrets::Document>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RotateTenantSecretRequest {
    value: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DisableTenantSecretRequest {
    disabled_reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TokensQuery {
    #[serde(default)]
    status: String,
    #[serde(default)]
    principal_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuditEventsQuery {
    #[serde(default)]
    tenant_id: String,
}
// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Go decodeJSONBody: an empty body maps to "request body is required" (400);
/// malformed JSON maps to the decoder error (400).
fn decode_json_body<T: DeserializeOwned>(body: &Bytes) -> Result<T, ApiError> {
    if body.is_empty() {
        return Err(ApiError::BadRequest("request body is required".to_string()));
    }
    serde_json::from_slice(body).map_err(|err| ApiError::BadRequest(err.to_string()))
}

/// Go parseOptionalTime: empty value -> None; RFC3339 parse error -> 400.
fn parse_optional_time(value: &str) -> Result<Option<DateTime<Utc>>, ApiError> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let parsed = DateTime::parse_from_rfc3339(value)
        .map_err(|err| ApiError::BadRequest(err.to_string()))?
        .with_timezone(&Utc);
    Ok(Some(parsed))
}

/// Manager lookups (Go `if manager == nil { 500 }`).
fn auth_manager(state: &AppState) -> Result<&kura_identity::auth::Manager, ApiError> {
    state
        .auth
        .as_deref()
        .ok_or_else(|| ApiError::Internal("auth manager is not configured".to_string()))
}

fn identity_manager(
    state: &AppState,
) -> Result<&kura_identity::Manager<dyn kura_identity::Store + Send + Sync>, ApiError> {
    state
        .identity
        .as_deref()
        .ok_or_else(|| ApiError::Internal("identity manager is not configured".to_string()))
}

fn secrets_manager(state: &AppState) -> Result<&kura_secrets::Manager, ApiError> {
    state
        .secrets
        .as_deref()
        .ok_or_else(|| ApiError::Internal("tenant secret manager is not configured".to_string()))
}

/// Go tenantContextFromContext: absent context -> 403 stable tenant denial.
/// Returns the middleware wrapper so handlers use `tc.0` for the resolved
/// identity tenant context.
fn tenant_context(
    tenant: &Option<Extension<TenantContext>>,
) -> Result<&TenantContext, AuthApiError> {
    tenant
        .as_ref()
        .map(|extension| &extension.0)
        .ok_or(AuthApiError::TenantDenial)
}

/// Go RequirePermission: on denial writes a tenant.permission_denied audit
/// (reasonCode "permission_denied:<permission>") and returns the 403 stable
/// tenant denial.
fn require_permission(
    state: &AppState,
    tenant: &kura_identity::TenantContext,
    permission: Permission,
) -> Result<(), AuthApiError> {
    if evaluate_permission(tenant, permission).allowed {
        return Ok(());
    }
    let event = TenantAuditEvent {
        event_kind: "tenant.permission_denied".to_string(),
        tenant_id: tenant.tenant_id.clone(),
        principal_id: tenant.principal_id.clone(),
        token_id: tenant.token_id.clone(),
        outcome: kura_identity::AUDIT_OUTCOME_DENIED.to_string(),
        reason_code: format!("permission_denied:{}", permission_wire(permission)),
        created_at: Utc::now(),
        ..TenantAuditEvent::default()
    };
    let _ = state.store.lock().append_tenant_audit_event(&event);
    Err(AuthApiError::TenantDenial)
}

/// Wire string for a permission (Go `string(permission)`).
fn permission_wire(permission: Permission) -> String {
    serde_json::to_value(permission)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// Go appendTokenAudit: tenant.token_issued / token_rotated / token_revoked.
fn append_token_audit(
    state: &AppState,
    event_kind: &str,
    tenant_id: &str,
    principal_id: &str,
    token_id: &str,
    reason_code: &str,
) -> Result<(), ApiError> {
    let event = TenantAuditEvent {
        event_kind: event_kind.to_string(),
        tenant_id: tenant_id.to_string(),
        principal_id: principal_id.to_string(),
        token_id: token_id.to_string(),
        outcome: kura_identity::AUDIT_OUTCOME_SUCCEEDED.to_string(),
        reason_code: reason_code.to_string(),
        created_at: Utc::now(),
        ..TenantAuditEvent::default()
    };
    state
        .store
        .lock()
        .append_tenant_audit_event(&event)
        .map_err(ApiError::from_store)?;
    Ok(())
}

/// Go activeGrantSet: active tenant ids + the default (first active when no
/// default flag).
fn active_grant_set(grants: &[TokenTenantGrant]) -> (Vec<String>, String) {
    let mut tenant_ids = Vec::with_capacity(grants.len());
    let mut default_tenant_id = String::new();
    for grant in grants {
        if grant.status != LifecycleStatus::Active {
            continue;
        }
        tenant_ids.push(grant.tenant_id.clone());
        if grant.is_default {
            default_tenant_id = grant.tenant_id.clone();
        }
    }
    if default_tenant_id.is_empty() && !tenant_ids.is_empty() {
        default_tenant_id = tenant_ids[0].clone();
    }
    (tenant_ids, default_tenant_id)
}

/// Go allowedTenantsForToken: active grants x active memberships of the
/// caller's principal, projected with caller membership fields.
fn allowed_tenants_for_token(
    store: &kura_store::SQLiteStore,
    token: &AccessToken,
    tenant_context: &kura_identity::TenantContext,
) -> Result<Vec<Tenant>, String> {
    let grants = store.list_token_tenant_grants(&token.token_id)?;
    let granted: HashMap<String, TokenTenantGrant> = grants
        .iter()
        .filter(|grant| grant.status == LifecycleStatus::Active)
        .map(|grant| (grant.tenant_id.clone(), grant.clone()))
        .collect();
    let memberships = store.list_memberships(&MembershipFilter {
        status: Some(LifecycleStatus::Active),
        limit: 500,
        ..MembershipFilter::default()
    })?;
    let mut items = Vec::new();
    for membership in memberships {
        if membership.principal_id != tenant_context.principal_id
            || membership.status != LifecycleStatus::Active
        {
            continue;
        }
        let Some(grant) = granted.get(&membership.tenant_id) else {
            continue;
        };
        let Some(mut tenant) = store.get_tenant(&membership.tenant_id)? else {
            continue;
        };
        if tenant.status != LifecycleStatus::Active {
            continue;
        }
        tenant.caller_membership_role = Some(membership.role);
        tenant.caller_membership_status = Some(membership.status);
        tenant.caller_permissions = permissions_for_role(membership.role, membership.status);
        tenant.default_for_current_principal = tenant.tenant_id == tenant_context.tenant_id
            && tenant.tenant_id == token.default_tenant_id;
        tenant.default_for_current_token =
            grant.is_default || tenant.tenant_id == token.default_tenant_id;
        items.push(tenant);
    }
    Ok(items)
}

/// Go buildAuthMeResponse.
fn build_auth_me_response(
    state: &AppState,
    token: &AccessToken,
    tenant_context: &kura_identity::TenantContext,
) -> Result<AuthMeResponse, ApiError> {
    let store = state.store.lock();
    let principal = store
        .get_principal(&tenant_context.principal_id)
        .map_err(ApiError::from_store)?
        .ok_or_else(|| ApiError::internal(kura_identity::IdentityError::TenantAccessDenied))?;
    let mut default_tenant = store
        .get_tenant(&principal.default_tenant_id)
        .map_err(ApiError::from_store)?
        .ok_or_else(|| ApiError::internal(kura_identity::IdentityError::TenantAccessDenied))?;
    let mut current_tenant = store
        .get_tenant(&tenant_context.tenant_id)
        .map_err(ApiError::from_store)?
        .ok_or_else(|| ApiError::internal(kura_identity::IdentityError::TenantAccessDenied))?;
    let allowed_tenants =
        allowed_tenants_for_token(&store, token, tenant_context).map_err(ApiError::from_store)?;
    let token_grants = store
        .list_token_tenant_grants(&token.token_id)
        .map_err(ApiError::from_store)?;
    default_tenant.default_for_current_principal =
        default_tenant.tenant_id == principal.default_tenant_id;
    default_tenant.default_for_current_token = default_tenant.tenant_id == token.default_tenant_id;
    current_tenant.caller_membership_role = tenant_context.role;
    current_tenant.caller_membership_status = Some(LifecycleStatus::Active);
    current_tenant.caller_permissions = tenant_context.permissions.clone();
    Ok(AuthMeResponse {
        token: token.clone(),
        principal,
        default_tenant,
        current_tenant,
        allowed_tenants,
        token_grants,
        permissions: tenant_context.permissions.clone(),
        tenant_context: tenant_context.clone(),
    })
}

/// Go projectTenantAuditEvent: redacts credential-bearing document keys.
fn project_tenant_audit_event(mut event: TenantAuditEvent) -> TenantAuditEvent {
    if event.event_kind != "credential.audit_recorded" {
        return event;
    }
    let Some(document) = event.document.as_mut() else {
        return event;
    };
    for key in ["value", "secretValue", "rawSecret", "accessToken", "refreshToken", "apiKey"] {
        if document.contains_key(key) {
            document.insert(
                key.to_string(),
                serde_json::Value::String(kura_secrets::REDACTED_VALUE.to_string()),
            );
        }
    }
    if let Some(value) = document.get("secretRefs").cloned() {
        let refs: Vec<String> = match value {
            serde_json::Value::Array(items) => items
                .into_iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect(),
            _ => Vec::new(),
        };
        if !refs.is_empty() {
            document.insert(
                "secretRefs".to_string(),
                serde_json::json!(kura_secrets::redact_secret_refs(&refs)),
            );
        }
    }
    event
}

/// Go publishEvent for the pairing lifecycle (category "system"): store append
/// then bus publish. Payload matches the Go handlers.
fn publish_pairing_event(
    state: &AppState,
    name: &str,
    pairing: &Pairing,
    extra: &[(&str, serde_json::Value)],
) -> Result<(), ApiError> {
    let mut payload = serde_json::Map::new();
    payload.insert("mode".to_string(), serde_json::json!(pairing.mode));
    payload.insert("status".to_string(), serde_json::json!(pairing.status));
    if name == "auth.pairing_started" {
        payload.insert("expiresAt".to_string(), serde_json::json!(pairing.expires_at));
        payload.insert("label".to_string(), serde_json::json!(pairing.label));
    }
    for (key, value) in extra {
        payload.insert(key.to_string(), value.clone());
    }
    let event = kura_events::Event {
        category: "system".to_string(),
        name: name.to_string(),
        environment_scope: crate::middleware::environment_scope_from_config(&state.config),
        resource: kura_events::Resource {
            kind: "pairing".to_string(),
            id: pairing.pairing_id.clone(),
        },
        payload,
        ..kura_events::Event::default()
    };
    let stored = state
        .store
        .lock()
        .append_event(&event)
        .map_err(ApiError::from_store)?;
    state.event_bus.publish(stored);
    Ok(())
}
// ---------------------------------------------------------------------------
// Hosted credential guards (port of requireHostedCredentialRead /
// requireHostedCredentialPermission / writeTenantSecretError)
// ---------------------------------------------------------------------------

const CREDENTIAL_DENIAL_MISSING_TENANT: &str = "credential_denied:missing_tenant";
const CREDENTIAL_DENIAL_MISSING_PERMISSION: &str = "credential_denied:missing_permission";
const CREDENTIAL_DENIAL_CROSS_TENANT: &str = "credential_denied:cross_tenant";

fn credential_missing_tenant() -> AuthApiError {
    AuthApiError::CredentialDenial {
        reason_code: CREDENTIAL_DENIAL_MISSING_TENANT,
    }
}

fn credential_missing_permission() -> AuthApiError {
    AuthApiError::CredentialDenial {
        reason_code: CREDENTIAL_DENIAL_MISSING_PERMISSION,
    }
}

/// Go requireHostedCredentialRead: any inspect path needs CredentialsInspect
/// or a manage permission (here SecretsManage).
fn require_hosted_credential_read(
    tenant: &Option<Extension<TenantContext>>,
) -> Result<&kura_identity::TenantContext, AuthApiError> {
    let tc = tenant
        .as_ref()
        .map(|extension| &extension.0 .0)
        .ok_or_else(credential_missing_tenant)?;
    if tc.tenant_id.is_empty() {
        return Err(credential_missing_tenant());
    }
    if !can_inspect_credentials(tc, &[Permission::SecretsManage]) {
        return Err(credential_missing_permission());
    }
    Ok(tc)
}

/// Go requireHostedCredentialPermission (resourceTenantID always "" here).
fn require_hosted_credential_permission(
    tenant: &Option<Extension<TenantContext>>,
    permission: Permission,
) -> Result<&kura_identity::TenantContext, AuthApiError> {
    let tc = tenant
        .as_ref()
        .map(|extension| &extension.0 .0)
        .ok_or_else(credential_missing_tenant)?;
    if tc.tenant_id.is_empty() {
        return Err(credential_missing_tenant());
    }
    if !has_permission(&tc.permissions, permission) {
        return Err(credential_missing_permission());
    }
    Ok(tc)
}

/// Go writeTenantSecretError.
fn map_secret_error(err: SecretsError) -> AuthApiError {
    match err {
        SecretsError::TenantRequired
        | SecretsError::SecretRefRequired
        | SecretsError::SecretValueRequired => {
            AuthApiError::Api(ApiError::BadRequest(err.to_string()))
        }
        SecretsError::SecretNotFound | SecretsError::SecretVersionNotFound => {
            AuthApiError::Api(ApiError::NotFound(err.to_string()))
        }
        SecretsError::SecretDisabled | SecretsError::CrossTenantSecret => {
            AuthApiError::CredentialDenial {
                reason_code: CREDENTIAL_DENIAL_CROSS_TENANT,
            }
        }
        other => AuthApiError::Api(ApiError::Internal(other.to_string())),
    }
}

struct CredentialAuditInput {
    tenant_id: String,
    principal_id: String,
    resource_kind: kura_secrets::ResourceKind,
    resource_id: String,
    action: kura_secrets::AuditAction,
    secret_ref: String,
    secret_version_id: String,
}

/// Go recordCredentialAudit (audit.BuildCredentialAuditEvent). The document
/// only ever carries identifiers - never raw secret values.
fn record_credential_audit(state: &AppState, input: CredentialAuditInput) -> Result<(), ApiError> {
    let mut document = serde_json::Map::new();
    document.insert(
        "resourceKind".to_string(),
        serde_json::json!(wire_enum(&input.resource_kind)),
    );
    document.insert("action".to_string(), serde_json::json!(wire_enum(&input.action)));
    if !input.resource_id.is_empty() {
        document.insert("resourceId".to_string(), serde_json::json!(input.resource_id));
    }
    if !input.secret_ref.is_empty() {
        document.insert("secretRef".to_string(), serde_json::json!(input.secret_ref));
    }
    if !input.secret_version_id.is_empty() {
        document.insert(
            "secretVersionId".to_string(),
            serde_json::json!(input.secret_version_id),
        );
    }
    let event = TenantAuditEvent {
        event_kind: "credential.audit_recorded".to_string(),
        tenant_id: input.tenant_id,
        principal_id: input.principal_id,
        outcome: kura_identity::AUDIT_OUTCOME_SUCCEEDED.to_string(),
        reason_code: String::new(),
        created_at: Utc::now(),
        document: Some(document),
        ..TenantAuditEvent::default()
    };
    state
        .store
        .lock()
        .append_tenant_audit_event(&event)
        .map_err(ApiError::from_store)?;
    Ok(())
}

/// Wire string for a renamed enum (Go `string(...)`).
fn wire_enum<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value)
        .map(|s| s.trim_matches('"').to_string())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Pairing handlers (unauthenticated; Go withEnvironment)
// ---------------------------------------------------------------------------

/// POST /v1/auth/pairings/start - Go handleAuthPairingStart. 201 with the
/// pairing record and the one-time code.
#[allow(clippy::unused_async)]
async fn auth_pairing_start(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Response, AuthApiError> {
    let manager = auth_manager(&state)?;
    let input: auth::StartPairingInput = decode_json_body(&body)?;
    let (pairing, code) = manager
        .start_pairing(input)
        .map_err(|err| AuthApiError::Api(ApiError::BadRequest(err.to_string())))?;
    state
        .store
        .lock()
        .upsert_pairing(&pairing)
        .map_err(ApiError::from_store)?;
    publish_pairing_event(&state, "auth.pairing_started", &pairing, &[])?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "pairing": pairing, "pairingCode": code })),
    )
        .into_response())
}

/// POST /v1/auth/pairings/{pairing_id}/complete - Go
/// handleAuthPairingComplete. Pairing-not-found maps to 404; other manager
/// errors to 400.
#[allow(clippy::unused_async)]
async fn auth_pairing_complete(
    State(state): State<AppState>,
    Path(pairing_id): Path<String>,
    body: Bytes,
) -> Result<Response, AuthApiError> {
    let manager = auth_manager(&state)?;
    let input: auth::CompletePairingInput = decode_json_body(&body)?;
    let (pairing, token, token_secret) = match manager.complete_pairing(&pairing_id, input) {
        Ok(result) => result,
        Err(auth::AuthError::PairingNotFound) => {
            return Err(AuthApiError::Api(ApiError::NotFound("not found".to_string())));
        }
        Err(err) => return Err(AuthApiError::Api(ApiError::BadRequest(err.to_string()))),
    };
    state
        .store
        .lock()
        .upsert_pairing(&pairing)
        .map_err(ApiError::from_store)?;
    state
        .store
        .lock()
        .upsert_access_token(&token)
        .map_err(ApiError::from_store)?;
    // A token only resolves against a tenant it has been granted. Pairing
    // normally issues no tenant, but a deployment that bootstraps a local
    // identity stamps one onto the token (see the embedded environment), and
    // without a matching grant that token would authenticate and then be
    // denied at tenant resolution -- the confusing 401-then-403 sequence.
    if !token.default_tenant_id.trim().is_empty() {
        let now = token.created_at;
        state
            .store
            .lock()
            .upsert_token_tenant_grant(&kura_identity::TokenTenantGrant {
                grant_id: format!("grant_{}", token.token_id),
                token_id: token.token_id.clone(),
                tenant_id: token.default_tenant_id.clone(),
                is_default: true,
                status: kura_identity::LifecycleStatus::Active,
                created_at: now,
                updated_at: now,
                revoked_at: None,
                granted_by_principal_id: token.principal_id.clone(),
            })
            .map_err(ApiError::from_store)?;
    }
    publish_pairing_event(
        &state,
        "auth.pairing_completed",
        &pairing,
        &[("tokenId", serde_json::json!(token.token_id))],
    )?;
    Ok((
        StatusCode::OK,
        Json(serde_json::json!({
            "pairing": pairing,
            "token": token,
            "accessToken": token_secret,
        })),
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// Auth me / token lifecycle
// ---------------------------------------------------------------------------

/// GET /v1/auth/me - Go handleAuthMe. Without a resolved tenant context the
/// bare token is returned (Go writes the token when the identity manager is
/// not configured); with one, the full AuthMeResponse is built.
#[allow(clippy::unused_async)]
async fn auth_me(
    State(state): State<AppState>,
    token: Option<Extension<AuthenticatedToken>>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Response, AuthApiError> {
    let token = token.ok_or_else(|| {
        AuthApiError::Api(ApiError::Unauthorized(auth::AuthError::AuthRequired.to_string()))
    })?;
    state
        .store
        .lock()
        .upsert_access_token(&token.0 .0)
        .map_err(ApiError::from_store)?;
    let Some(tenant_context) = tenant else {
        return Ok(Json(token.0 .0).into_response());
    };
    let response = build_auth_me_response(&state, &token.0 .0, &tenant_context.0 .0)?;
    Ok((StatusCode::OK, Json(response)).into_response())
}

/// GET /v1/auth/tokens - Go handleAuthTokenList. Non-managers only see their
/// own tokens; status/principalId query filters mirror Go.
#[allow(clippy::unused_async)]
async fn auth_tokens_list(
    State(state): State<AppState>,
    Query(params): Query<TokensQuery>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Response, AuthApiError> {
    let tc = tenant_context(&tenant)?;
    let tokens = state
        .store
        .lock()
        .list_access_tokens()
        .map_err(ApiError::from_store)?;
    let mut principal_id = params.principal_id.trim().to_string();
    if principal_id.is_empty() || !has_permission(&tc.0.permissions, Permission::TenantManage) {
        principal_id = tc.0.principal_id.clone();
    }
    let status = params.status.trim().to_string();
    let items = tokens
        .into_iter()
        .filter(|token| {
            if !principal_id.is_empty() && token.principal_id != principal_id {
                return false;
            }
            if !status.is_empty() && token_status_wire(&token.status) != status {
                return false;
            }
            true
        })
        .collect::<Vec<_>>();
    Ok((StatusCode::OK, Json(ListResponse { items })).into_response())
}

fn token_status_wire(status: &auth::TokenStatus) -> &'static str {
    match status {
        auth::TokenStatus::Active => "active",
        auth::TokenStatus::Revoked => "revoked",
        auth::TokenStatus::Expired => "expired",
        auth::TokenStatus::Rotated => "rotated",
    }
}
/// POST /v1/auth/tokens - Go handleAuthTokenCreate. 201 with
/// {token, accessToken, grants}.
#[allow(clippy::unused_async)]
async fn auth_token_create(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<Response, AuthApiError> {
    let tc = tenant_context(&tenant)?;
    require_permission(&state, &tc.0, Permission::TenantManage)?;
    let request: CreateTokenRequest = decode_json_body(&body)?;
    let expires_at = parse_optional_time(&request.expires_at)?;
    let identity = identity_manager(&state)?;
    identity
        .validate_token_tenant_grants(
            &tc.0,
            &tc.0.token_id,
            &request.allowed_tenant_ids,
            &request.default_tenant_id,
        )
        .map_err(|err| AuthApiError::Api(ApiError::BadRequest(err.to_string())))?;
    append_token_audit(
        &state,
        "tenant.token_issued",
        &request.default_tenant_id,
        &tc.0.principal_id,
        "",
        "token_issued",
    )?;
    let manager = auth_manager(&state)?;
    let (token, secret) = manager
        .issue_token(auth::IssueTokenInput {
            principal_id: tc.0.principal_id.clone(),
            label: request.label.clone(),
            default_tenant_id: request.default_tenant_id.clone(),
            expires_at,
        })
        .map_err(|err| AuthApiError::Api(ApiError::BadRequest(err.to_string())))?;
    state
        .store
        .lock()
        .upsert_access_token(&token)
        .map_err(ApiError::from_store)?;
    let grants = identity
        .replace_token_tenant_grants(
            &tc.0,
            &token.token_id,
            &request.allowed_tenant_ids,
            &request.default_tenant_id,
        )
        .map_err(|err| AuthApiError::Api(ApiError::BadRequest(err.to_string())))?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "token": token, "accessToken": secret, "grants": grants })),
    )
        .into_response())
}

/// POST /v1/auth/tokens/{token_id}/rotate - Go handleAuthTokenRotate.
#[allow(clippy::unused_async)]
async fn auth_token_rotate(
    State(state): State<AppState>,
    Path(token_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<Response, AuthApiError> {
    let tc = tenant_context(&tenant)?;
    require_permission(&state, &tc.0, Permission::TenantManage)?;
    let request: RotateTokenRequest = decode_json_body(&body)?;
    let expires_at = parse_optional_time(&request.expires_at)?;
    let old_grants = state
        .store
        .lock()
        .list_token_tenant_grants(&token_id)
        .map_err(ApiError::from_store)?;
    let (allowed_tenant_ids, default_tenant_id) = active_grant_set(&old_grants);
    if allowed_tenant_ids.is_empty() {
        return Err(AuthApiError::Api(ApiError::BadRequest(
            kura_identity::IdentityError::TokenGrantInvalid.to_string(),
        )));
    }
    let reason = if request.reason.trim().is_empty() {
        "token_rotated".to_string()
    } else {
        request.reason.clone()
    };
    append_token_audit(
        &state,
        "tenant.token_rotated",
        &default_tenant_id,
        &tc.0.principal_id,
        &token_id,
        &reason,
    )?;
    let manager = auth_manager(&state)?;
    let (old_token, new_token, secret) = manager
        .rotate_token(token_id.as_str(), auth::RotateTokenInput {
            expires_at,
            reason: request.reason.clone(),
        })
        .map_err(|err| AuthApiError::Api(ApiError::BadRequest(err.to_string())))?;
    state
        .store
        .lock()
        .upsert_access_token(&old_token)
        .map_err(ApiError::from_store)?;
    state
        .store
        .lock()
        .upsert_access_token(&new_token)
        .map_err(ApiError::from_store)?;
    let identity = identity_manager(&state)?;
    let grants = identity
        .replace_token_tenant_grants(
            &tc.0,
            &new_token.token_id,
            &allowed_tenant_ids,
            &default_tenant_id,
        )
        .map_err(|err| AuthApiError::Api(ApiError::BadRequest(err.to_string())))?;
    Ok((
        StatusCode::OK,
        Json(serde_json::json!({
            "oldToken": old_token,
            "newToken": new_token,
            "accessToken": secret,
            "grants": grants,
        })),
    )
        .into_response())
}

/// POST /v1/auth/tokens/{token_id}/revoke - Go handleAuthTokenRevoke.
#[allow(clippy::unused_async)]
async fn auth_token_revoke(
    State(state): State<AppState>,
    Path(token_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<Response, AuthApiError> {
    let tc = tenant_context(&tenant)?;
    require_permission(&state, &tc.0, Permission::TenantManage)?;
    let request: RevokeTokenRequest = decode_json_body(&body)?;
    let reason = if request.reason.trim().is_empty() {
        "token_revoked".to_string()
    } else {
        request.reason.clone()
    };
    append_token_audit(
        &state,
        "tenant.token_revoked",
        &tc.0.tenant_id,
        &tc.0.principal_id,
        &token_id,
        &reason,
    )?;
    let manager = auth_manager(&state)?;
    let token = manager
        .revoke_token(&token_id)
        .map_err(|err| AuthApiError::Api(ApiError::BadRequest(err.to_string())))?;
    state
        .store
        .lock()
        .upsert_access_token(&token)
        .map_err(ApiError::from_store)?;
    Ok((StatusCode::OK, Json(serde_json::json!({ "token": token }))).into_response())
}

/// PATCH /v1/auth/tokens/{token_id}/tenant-grants - Go
/// handleAuthTokenGrantUpdate.
#[allow(clippy::unused_async)]
async fn auth_token_grant_update(
    State(state): State<AppState>,
    Path(token_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<Response, AuthApiError> {
    let tc = tenant_context(&tenant)?;
    require_permission(&state, &tc.0, Permission::TenantManage)?;
    let request: GrantUpdateRequest = decode_json_body(&body)?;
    let identity = identity_manager(&state)?;
    let grants = identity
        .replace_token_tenant_grants(
            &tc.0,
            &token_id,
            &request.allowed_tenant_ids,
            &request.default_tenant_id,
        )
        .map_err(|err| AuthApiError::Api(ApiError::BadRequest(err.to_string())))?;
    let manager = auth_manager(&state)?;
    if let Some(mut token) = manager.get_token(&token_id) {
        token.default_tenant_id = request.default_tenant_id.clone();
        token.updated_at = Utc::now();
        manager.update_token(token.clone());
        state
            .store
            .lock()
            .upsert_access_token(&token)
            .map_err(ApiError::from_store)?;
    }
    Ok((
        StatusCode::OK,
        Json(serde_json::json!({ "tokenId": token_id, "grants": grants })),
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// Tenants
// ---------------------------------------------------------------------------

/// GET /v1/tenants - Go handleTenants GET branch: allowed tenants for the
/// current token.
#[allow(clippy::unused_async)]
async fn tenants_list(
    State(state): State<AppState>,
    token: Option<Extension<AuthenticatedToken>>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Response, AuthApiError> {
    let tc = tenant_context(&tenant)?;
    let token = token.ok_or_else(|| {
        AuthApiError::Api(ApiError::Unauthorized(auth::AuthError::AuthRequired.to_string()))
    })?;
    let store = state.store.lock();
    let allowed =
        allowed_tenants_for_token(&store, &token.0 .0, &tc.0).map_err(ApiError::from_store)?;
    Ok((StatusCode::OK, Json(TenantListResponse { items: allowed })).into_response())
}

/// POST /v1/tenants - Go handleCreateTenant. Only organization kind is
/// accepted; 201 with {tenant, membership}.
#[allow(clippy::unused_async)]
async fn tenant_create(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<Response, AuthApiError> {
    let tc = tenant_context(&tenant)?;
    require_permission(&state, &tc.0, Permission::TenantManage)?;
    let request: CreateTenantRequest = decode_json_body(&body)?;
    if !request.tenant_kind.is_empty() && request.tenant_kind != "organization" {
        return Err(AuthApiError::Api(ApiError::BadRequest(
            kura_identity::IdentityError::TenantInvalid.to_string(),
        )));
    }
    let identity = identity_manager(&state)?;
    let (created_tenant, membership) = identity
        .create_organization_tenant(&tc.0, &request.display_name)
        .map_err(|err| AuthApiError::Api(ApiError::BadRequest(err.to_string())))?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "tenant": created_tenant, "membership": membership })),
    )
        .into_response())
}

/// GET /v1/tenants/{tenant_id} - Go handleTenantRoutes single-segment branch.
/// The caller's tenant must match the path tenant; missing rows surface as the
/// stable 403 denial (Go GetTenant !ok -> writeTenantDenial).
#[allow(clippy::unused_async)]
async fn tenant_detail(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Response, AuthApiError> {
    let tc = tenant_context(&tenant)?;
    if tc.0.tenant_id != tenant_id {
        return Err(AuthApiError::TenantDenial);
    }
    let store = state.store.lock();
    let Some(mut row) = store.get_tenant(&tenant_id).map_err(ApiError::from_store)? else {
        return Err(AuthApiError::TenantDenial);
    };
    row.caller_membership_role = tc.0.role;
    row.caller_membership_status = Some(LifecycleStatus::Active);
    row.caller_permissions = tc.0.permissions.clone();
    Ok((
        StatusCode::OK,
        Json(TenantDetailResponse {
            tenant: row,
            tenant_context: tc.0.clone(),
        }),
    )
        .into_response())
}
/// GET /v1/tenants/{tenant_id}/memberships - Go handleTenantMembershipRoutes
/// list branch (TenantManage required).
#[allow(clippy::unused_async)]
async fn tenant_memberships_list(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Response, AuthApiError> {
    let tc = tenant_context(&tenant)?;
    if tc.0.tenant_id != tenant_id {
        return Err(AuthApiError::TenantDenial);
    }
    require_permission(&state, &tc.0, Permission::TenantManage)?;
    let items = state
        .store
        .lock()
        .list_memberships(&MembershipFilter {
            tenant_id,
            limit: 500,
            ..MembershipFilter::default()
        })
        .map_err(ApiError::from_store)?;
    Ok((StatusCode::OK, Json(ListResponse { items })).into_response())
}

/// PATCH /v1/tenants/{tenant_id}/memberships/{membership_id} - Go
/// handleTenantMembershipRoutes PATCH branch.
#[allow(clippy::unused_async)]
async fn tenant_membership_update(
    State(state): State<AppState>,
    Path((tenant_id, membership_id)): Path<(String, String)>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<Response, AuthApiError> {
    let tc = tenant_context(&tenant)?;
    if tc.0.tenant_id != tenant_id {
        return Err(AuthApiError::TenantDenial);
    }
    require_permission(&state, &tc.0, Permission::TenantManage)?;
    let request: UpdateMembershipRoleRequest = decode_json_body(&body)?;
    let identity = identity_manager(&state)?;
    let membership = identity
        .update_membership_role(&tc.0, &tenant_id, &membership_id, request.role)
        .map_err(|err| AuthApiError::Api(ApiError::BadRequest(err.to_string())))?;
    Ok((StatusCode::OK, Json(serde_json::json!({ "membership": membership }))).into_response())
}

/// DELETE /v1/tenants/{tenant_id}/memberships/{membership_id} - Go
/// handleTenantMembershipRoutes DELETE branch.
#[allow(clippy::unused_async)]
async fn tenant_membership_remove(
    State(state): State<AppState>,
    Path((tenant_id, membership_id)): Path<(String, String)>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Response, AuthApiError> {
    let tc = tenant_context(&tenant)?;
    if tc.0.tenant_id != tenant_id {
        return Err(AuthApiError::TenantDenial);
    }
    require_permission(&state, &tc.0, Permission::TenantManage)?;
    let identity = identity_manager(&state)?;
    let membership = identity
        .remove_membership(&tc.0, &tenant_id, &membership_id)
        .map_err(|err| AuthApiError::Api(ApiError::BadRequest(err.to_string())))?;
    Ok((StatusCode::OK, Json(serde_json::json!({ "membership": membership }))).into_response())
}

/// GET /v1/tenants/{tenant_id}/invitations - Go
/// handleTenantInvitationCollection GET branch (no permission gate).
#[allow(clippy::unused_async)]
async fn tenant_invitations_list(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Response, AuthApiError> {
    let tc = tenant_context(&tenant)?;
    if tc.0.tenant_id != tenant_id {
        return Err(AuthApiError::TenantDenial);
    }
    let items = state
        .store
        .lock()
        .list_tenant_invitations(&InvitationFilter {
            tenant_id,
            limit: 500,
            ..InvitationFilter::default()
        })
        .map_err(ApiError::from_store)?;
    Ok((StatusCode::OK, Json(ListResponse { items })).into_response())
}

/// POST /v1/tenants/{tenant_id}/invitations - Go
/// handleTenantInvitationCollection POST branch (TenantManage required).
#[allow(clippy::unused_async)]
async fn tenant_invitation_create(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<Response, AuthApiError> {
    let tc = tenant_context(&tenant)?;
    if tc.0.tenant_id != tenant_id {
        return Err(AuthApiError::TenantDenial);
    }
    require_permission(&state, &tc.0, Permission::TenantManage)?;
    let request: CreateInvitationRequest = decode_json_body(&body)?;
    let identity = identity_manager(&state)?;
    let invitation = identity
        .create_invitation(
            &tc.0,
            kura_identity::CreateInvitationInput {
                tenant_id,
                invited_principal_id: request.invited_principal_id,
                role: Some(request.role),
                expires_at: None,
            },
        )
        .map_err(|err| AuthApiError::Api(ApiError::BadRequest(err.to_string())))?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "invitation": invitation })),
    )
        .into_response())
}

/// GET /v1/tenants/{tenant_id}/permissions - Go handleTenantPermissions:
/// every sensitive permission + read_only.inspect evaluated for the caller.
#[allow(clippy::unused_async)]
async fn tenant_permissions(
    State(_state): State<AppState>,
    Path(tenant_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Response, AuthApiError> {
    let tc = tenant_context(&tenant)?;
    if tc.0.tenant_id != tenant_id {
        return Err(AuthApiError::TenantDenial);
    }
    let mut items = Vec::with_capacity(kura_identity::ALL_SENSITIVE_PERMISSIONS.len() + 1);
    for permission in kura_identity::ALL_SENSITIVE_PERMISSIONS
        .iter()
        .copied()
        .chain(std::iter::once(Permission::ReadOnlyInspect))
    {
        items.push(evaluate_permission(&tc.0, permission));
    }
    Ok((StatusCode::OK, Json(ListResponse { items })).into_response())
}

// ---------------------------------------------------------------------------
// Tenant invitations (self-service accept/reject)
// ---------------------------------------------------------------------------

/// GET /v1/tenant-invitations - Go handleTenantInvitations: invitations
/// addressed to the caller's principal.
#[allow(clippy::unused_async)]
async fn tenant_invitations_self_list(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Response, AuthApiError> {
    let tc = tenant_context(&tenant)?;
    let items = state
        .store
        .lock()
        .list_tenant_invitations(&InvitationFilter {
            principal_id: tc.0.principal_id.clone(),
            limit: 500,
            ..InvitationFilter::default()
        })
        .map_err(ApiError::from_store)?;
    Ok((StatusCode::OK, Json(ListResponse { items })).into_response())
}

/// POST /v1/tenant-invitations/{invitation_id}/accept - Go
/// handleTenantInvitationRoutes accept branch.
#[allow(clippy::unused_async)]
async fn tenant_invitation_accept(
    State(state): State<AppState>,
    Path(invitation_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Response, AuthApiError> {
    let tc = tenant_context(&tenant)?;
    let identity = identity_manager(&state)?;
    let membership = identity
        .accept_invitation(&tc.0.principal_id, &invitation_id)
        .map_err(|err| AuthApiError::Api(ApiError::BadRequest(err.to_string())))?;
    Ok((StatusCode::OK, Json(serde_json::json!({ "membership": membership }))).into_response())
}

/// POST /v1/tenant-invitations/{invitation_id}/reject - Go
/// handleTenantInvitationRoutes reject branch.
#[allow(clippy::unused_async)]
async fn tenant_invitation_reject(
    State(state): State<AppState>,
    Path(invitation_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Response, AuthApiError> {
    let tc = tenant_context(&tenant)?;
    let identity = identity_manager(&state)?;
    let invitation = identity
        .decide_invitation(&tc.0.principal_id, &invitation_id, LifecycleStatus::Rejected)
        .map_err(|err| AuthApiError::Api(ApiError::BadRequest(err.to_string())))?;
    Ok((StatusCode::OK, Json(serde_json::json!({ "invitation": invitation }))).into_response())
}

// ---------------------------------------------------------------------------
// Principals
// ---------------------------------------------------------------------------

/// GET /v1/principals - Go handlePrincipals: managers see the tenant's
/// principals; everyone else only themselves.
#[allow(clippy::unused_async)]
async fn principals_list(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Response, AuthApiError> {
    let tc = tenant_context(&tenant)?;
    let manage = has_permission(&tc.0.permissions, Permission::TenantManage);
    let filter = if manage {
        PrincipalFilter {
            tenant_id: tc.0.tenant_id.clone(),
            limit: 500,
            ..PrincipalFilter::default()
        }
    } else {
        PrincipalFilter {
            limit: 500,
            ..PrincipalFilter::default()
        }
    };
    let items = state
        .store
        .lock()
        .list_principals(&filter)
        .map_err(ApiError::from_store)?;
    let items = if manage {
        items
    } else {
        items
            .into_iter()
            .filter(|principal| principal.principal_id == tc.0.principal_id)
            .collect::<Vec<_>>()
    };
    Ok((StatusCode::OK, Json(ListResponse { items })).into_response())
}

/// PATCH /v1/principals/{principal_id} - Go handlePrincipalRoutes
/// (TenantManage required; empty status defaults to active).
#[allow(clippy::unused_async)]
async fn principal_update(
    State(state): State<AppState>,
    Path(principal_id): Path<String>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<Response, AuthApiError> {
    let tc = tenant_context(&tenant)?;
    require_permission(&state, &tc.0, Permission::TenantManage)?;
    let request: PrincipalUpdateRequest = decode_json_body(&body)?;
    let store = state.store.lock();
    let Some(mut principal) = store.get_principal(&principal_id).map_err(ApiError::from_store)?
    else {
        return Err(AuthApiError::Api(ApiError::NotFound("not found".to_string())));
    };
    principal.status = request.status.unwrap_or(LifecycleStatus::Active);
    principal.updated_at = tc.0.resolved_at;
    store.upsert_principal(&principal).map_err(ApiError::from_store)?;
    let event = TenantAuditEvent {
        event_kind: "tenant.principal_lifecycle_updated".to_string(),
        tenant_id: tc.0.tenant_id.clone(),
        principal_id: tc.0.principal_id.clone(),
        target_principal_id: principal.principal_id.clone(),
        outcome: kura_identity::AUDIT_OUTCOME_SUCCEEDED.to_string(),
        reason_code: "principal_lifecycle_updated".to_string(),
        created_at: Utc::now(),
        ..TenantAuditEvent::default()
    };
    store.append_tenant_audit_event(&event).map_err(ApiError::from_store)?;
    drop(store);
    Ok((StatusCode::OK, Json(serde_json::json!({ "principal": principal }))).into_response())
}

// ---------------------------------------------------------------------------
// Tenant audit events
// ---------------------------------------------------------------------------

/// GET /v1/tenant-audit-events - Go handleTenantAuditEvents. The tenantId
/// query must match the caller's tenant (403 stable denial otherwise);
/// credential documents are redacted.
#[allow(clippy::unused_async)]
async fn tenant_audit_events_list(
    State(state): State<AppState>,
    Query(params): Query<AuditEventsQuery>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Response, AuthApiError> {
    let tc = tenant_context(&tenant)?;
    let tenant_id = if params.tenant_id.trim().is_empty() {
        tc.0.tenant_id.clone()
    } else {
        params.tenant_id.trim().to_string()
    };
    if tenant_id != tc.0.tenant_id {
        return Err(AuthApiError::TenantDenial);
    }
    let items = state
        .store
        .lock()
        .list_tenant_audit_events(&AuditEventFilter {
            tenant_id,
            limit: 500,
            ..AuditEventFilter::default()
        })
        .map_err(ApiError::from_store)?;
    let items = items.into_iter().map(project_tenant_audit_event).collect::<Vec<_>>();
    Ok((StatusCode::OK, Json(ListResponse { items })).into_response())
}
// ---------------------------------------------------------------------------
// Tenant secrets (hosted credentials)
// ---------------------------------------------------------------------------

/// GET /v1/tenant-secrets - Go handleTenantSecrets GET branch. Readable by
/// credential inspectors and secrets managers.
#[allow(clippy::unused_async)]
async fn tenant_secrets_list(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Response, AuthApiError> {
    let manager = secrets_manager(&state)?;
    let tc = require_hosted_credential_read(&tenant)?;
    let items = manager.list(&tc.tenant_id).await.map_err(map_secret_error)?;
    Ok((StatusCode::OK, Json(ListResponse { items })).into_response())
}

/// POST /v1/tenant-secrets - Go handleTenantSecrets POST branch
/// (secrets.manage required); 201 with {secret}. The raw value never leaves
/// the manager.
async fn tenant_secret_create(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<Response, AuthApiError> {
    let manager = secrets_manager(&state)?;
    let tc = require_hosted_credential_permission(&tenant, Permission::SecretsManage)?;
    let request: CreateTenantSecretRequest = decode_json_body(&body)?;
    let secret = manager
        .create(kura_secrets::CreateInput {
            tenant_id: tc.tenant_id.clone(),
            secret_ref: request.secret_ref,
            display_name: request.display_name,
            value: request.value,
            document: request.document,
        })
        .await
        .map_err(map_secret_error)?;
    record_credential_audit(
        &state,
        CredentialAuditInput {
            tenant_id: tc.tenant_id.clone(),
            principal_id: tc.principal_id.clone(),
            resource_kind: kura_secrets::ResourceKind::TenantSecret,
            resource_id: secret.secret_id.clone(),
            action: kura_secrets::AuditAction::SecretCreate,
            secret_ref: secret.secret_ref.clone(),
            secret_version_id: String::new(),
        },
    )?;
    Ok((StatusCode::CREATED, Json(serde_json::json!({ "secret": secret }))).into_response())
}

/// GET /v1/tenant-secrets/{secret_ref} - Go handleTenantSecretRoutes GET
/// branch (credential inspect).
#[allow(clippy::unused_async)]
async fn tenant_secret_get(
    State(state): State<AppState>,
    Path(secret_ref): Path<String>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Response, AuthApiError> {
    let manager = secrets_manager(&state)?;
    let tc = require_hosted_credential_read(&tenant)?;
    let secret = manager.get(&tc.tenant_id, &secret_ref).await.map_err(map_secret_error)?;
    Ok((StatusCode::OK, Json(serde_json::json!({ "secret": secret }))).into_response())
}

/// PATCH /v1/tenant-secrets/{secret_ref} - Go handleTenantSecretRoutes PATCH
/// branch (metadata only).
async fn tenant_secret_patch(
    State(state): State<AppState>,
    Path(secret_ref): Path<String>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<Response, AuthApiError> {
    let manager = secrets_manager(&state)?;
    let tc = require_hosted_credential_permission(&tenant, Permission::SecretsManage)?;
    let request: UpdateTenantSecretRequest = decode_json_body(&body)?;
    let secret = manager
        .update_metadata(kura_secrets::UpdateMetadataInput {
            tenant_id: tc.tenant_id.clone(),
            secret_ref,
            display_name: request.display_name,
            document: request.document,
        })
        .await
        .map_err(map_secret_error)?;
    record_credential_audit(
        &state,
        CredentialAuditInput {
            tenant_id: tc.tenant_id.clone(),
            principal_id: tc.principal_id.clone(),
            resource_kind: kura_secrets::ResourceKind::TenantSecret,
            resource_id: secret.secret_id.clone(),
            action: kura_secrets::AuditAction::SecretUpdate,
            secret_ref: secret.secret_ref.clone(),
            secret_version_id: String::new(),
        },
    )?;
    Ok((StatusCode::OK, Json(serde_json::json!({ "secret": secret }))).into_response())
}

/// POST /v1/tenant-secrets/{secret_ref}/rotate - Go handleTenantSecretRoutes
/// rotate branch.
async fn tenant_secret_rotate(
    State(state): State<AppState>,
    Path(secret_ref): Path<String>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<Response, AuthApiError> {
    let manager = secrets_manager(&state)?;
    let tc = require_hosted_credential_permission(&tenant, Permission::SecretsManage)?;
    let request: RotateTenantSecretRequest = decode_json_body(&body)?;
    let secret = manager
        .rotate(kura_secrets::RotateInput {
            tenant_id: tc.tenant_id.clone(),
            secret_ref,
            value: request.value,
        })
        .await
        .map_err(map_secret_error)?;
    record_credential_audit(
        &state,
        CredentialAuditInput {
            tenant_id: tc.tenant_id.clone(),
            principal_id: tc.principal_id.clone(),
            resource_kind: kura_secrets::ResourceKind::SecretVersion,
            resource_id: secret.secret_id.clone(),
            action: kura_secrets::AuditAction::SecretRotate,
            secret_ref: secret.secret_ref.clone(),
            secret_version_id: secret.active_version_id.clone(),
        },
    )?;
    Ok((StatusCode::OK, Json(serde_json::json!({ "secret": secret }))).into_response())
}

/// POST /v1/tenant-secrets/{secret_ref}/disable - Go handleTenantSecretRoutes
/// disable branch.
async fn tenant_secret_disable(
    State(state): State<AppState>,
    Path(secret_ref): Path<String>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<Response, AuthApiError> {
    let manager = secrets_manager(&state)?;
    let tc = require_hosted_credential_permission(&tenant, Permission::SecretsManage)?;
    let request: DisableTenantSecretRequest = decode_json_body(&body)?;
    let secret = manager
        .disable(kura_secrets::DisableInput {
            tenant_id: tc.tenant_id.clone(),
            secret_ref,
            disabled_reason: request.disabled_reason,
        })
        .await
        .map_err(map_secret_error)?;
    record_credential_audit(
        &state,
        CredentialAuditInput {
            tenant_id: tc.tenant_id.clone(),
            principal_id: tc.principal_id.clone(),
            resource_kind: kura_secrets::ResourceKind::TenantSecret,
            resource_id: secret.secret_id.clone(),
            action: kura_secrets::AuditAction::SecretDisable,
            secret_ref: secret.secret_ref.clone(),
            secret_version_id: String::new(),
        },
    )?;
    Ok((StatusCode::OK, Json(serde_json::json!({ "secret": secret }))).into_response())
}
#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;
    use std::sync::Arc;

    use axum::body::to_bytes;
    use axum::http::Request as HttpRequest;
    use kura_events::Bus;
    use kura_secrets::{LocalBackend, SecretVersionStatus};
    use kura_store::SQLiteStore;
    use parking_lot::Mutex;
    use tower::ServiceExt;
    use uuid::Uuid;

    // -----------------------------------------------------------------------
    // Test doubles: kura-store has not implemented kura_identity::Store for
    // SQLiteStore nor kura_secrets::Store; these local wrappers bridge the
    // managers over the real SQLiteStore / in-memory secret rows.
    // -----------------------------------------------------------------------

    #[derive(Debug)]
    struct StoreError(String);

    impl std::fmt::Display for StoreError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.0)
        }
    }

    impl std::error::Error for StoreError {}

    fn identity_store_err(err: String) -> kura_identity::IdentityError {
        kura_identity::IdentityError::Store(Box::new(StoreError(err)))
    }

    struct TestIdentityStore {
        store: Arc<Mutex<SQLiteStore>>,
    }

    impl kura_identity::ResolverStore for TestIdentityStore {
        fn get_principal(
            &self,
            principal_id: &str,
        ) -> Result<Option<kura_identity::Principal>, kura_identity::IdentityError> {
            self.store.lock().get_principal(principal_id).map_err(identity_store_err)
        }
        fn get_tenant(
            &self,
            tenant_id: &str,
        ) -> Result<Option<kura_identity::Tenant>, kura_identity::IdentityError> {
            self.store.lock().get_tenant(tenant_id).map_err(identity_store_err)
        }
        fn list_memberships(
            &self,
            filter: &kura_identity::MembershipFilter,
        ) -> Result<Vec<kura_identity::Membership>, kura_identity::IdentityError> {
            self.store.lock().list_memberships(filter).map_err(identity_store_err)
        }
        fn list_token_tenant_grants(
            &self,
            token_id: &str,
        ) -> Result<Vec<kura_identity::TokenTenantGrant>, kura_identity::IdentityError> {
            self.store.lock().list_token_tenant_grants(token_id).map_err(identity_store_err)
        }
    }

    impl kura_identity::AuditStore for TestIdentityStore {
        fn append_tenant_audit_event(
            &self,
            event: kura_identity::TenantAuditEvent,
        ) -> Result<kura_identity::TenantAuditEvent, kura_identity::IdentityError> {
            self.store
                .lock()
                .append_tenant_audit_event(&event)
                .map_err(identity_store_err)
        }
    }

    impl kura_identity::Store for TestIdentityStore {
        fn upsert_tenant(
            &self,
            tenant: &kura_identity::Tenant,
        ) -> Result<(), kura_identity::IdentityError> {
            self.store.lock().upsert_tenant(tenant).map_err(identity_store_err)
        }
        fn upsert_principal(
            &self,
            principal: &kura_identity::Principal,
        ) -> Result<(), kura_identity::IdentityError> {
            self.store.lock().upsert_principal(principal).map_err(identity_store_err)
        }
        fn upsert_membership(
            &self,
            membership: &kura_identity::Membership,
        ) -> Result<(), kura_identity::IdentityError> {
            self.store.lock().upsert_membership(membership).map_err(identity_store_err)
        }
        fn upsert_tenant_invitation(
            &self,
            invitation: &kura_identity::TenantInvitation,
        ) -> Result<(), kura_identity::IdentityError> {
            self.store.lock().upsert_tenant_invitation(invitation).map_err(identity_store_err)
        }
        fn upsert_token_tenant_grant(
            &self,
            grant: &kura_identity::TokenTenantGrant,
        ) -> Result<(), kura_identity::IdentityError> {
            self.store.lock().upsert_token_tenant_grant(grant).map_err(identity_store_err)
        }
        fn list_tenants(
            &self,
            filter: &kura_identity::TenantFilter,
        ) -> Result<Vec<kura_identity::Tenant>, kura_identity::IdentityError> {
            self.store.lock().list_tenants(filter).map_err(identity_store_err)
        }
        fn list_principals(
            &self,
            filter: &kura_identity::PrincipalFilter,
        ) -> Result<Vec<kura_identity::Principal>, kura_identity::IdentityError> {
            self.store.lock().list_principals(filter).map_err(identity_store_err)
        }
        fn list_tenant_invitations(
            &self,
            filter: &kura_identity::InvitationFilter,
        ) -> Result<Vec<kura_identity::TenantInvitation>, kura_identity::IdentityError> {
            self.store.lock().list_tenant_invitations(filter).map_err(identity_store_err)
        }
        fn list_token_authorities(
            &self,
        ) -> Result<Vec<kura_identity::TokenAuthority>, kura_identity::IdentityError> {
            self.store.lock().list_token_authorities().map_err(identity_store_err)
        }
    }

    /// In-memory kura_secrets::Store mirroring the secrets crate's FakeStore
    /// semantics (transactional rotate: next version number + supersede).
    struct TestSecretStore {
        secrets: Mutex<HashMap<(String, String), kura_secrets::TenantSecret>>,
        versions: Mutex<HashMap<(String, String), kura_secrets::SecretVersion>>,
    }

    impl TestSecretStore {
        fn new() -> Self {
            Self {
                secrets: Mutex::new(HashMap::new()),
                versions: Mutex::new(HashMap::new()),
            }
        }
    }

    impl kura_secrets::Store for TestSecretStore {
        fn create_secret<'a>(
            &'a self,
            secret: kura_secrets::TenantSecret,
            version: kura_secrets::SecretVersion,
        ) -> kura_secrets::BoxFuture<'a, kura_secrets::Result<()>> {
            Box::pin(async move {
                self.secrets
                    .lock()
                    .insert((secret.tenant_id.clone(), secret.secret_ref.clone()), secret);
                self.versions
                    .lock()
                    .insert(
                        (version.tenant_id.clone(), version.secret_version_id.clone()),
                        version,
                    );
                Ok(())
            })
        }
        fn update_secret_metadata<'a>(
            &'a self,
            secret: kura_secrets::TenantSecret,
        ) -> kura_secrets::BoxFuture<'a, kura_secrets::Result<()>> {
            Box::pin(async move {
                self.secrets
                    .lock()
                    .insert((secret.tenant_id.clone(), secret.secret_ref.clone()), secret);
                Ok(())
            })
        }
        fn rotate_secret<'a>(
            &'a self,
            secret: kura_secrets::TenantSecret,
            previous_version_id: &'a str,
            mut version: kura_secrets::SecretVersion,
        ) -> kura_secrets::BoxFuture<'a, kura_secrets::Result<()>> {
            Box::pin(async move {
                let mut versions = self.versions.lock();
                let next = versions
                    .values()
                    .filter(|v| v.tenant_id == secret.tenant_id && v.secret_id == secret.secret_id)
                    .map(|v| v.version_number)
                    .max()
                    .unwrap_or(0)
                    + 1;
                version.version_number = next;
                if !previous_version_id.is_empty() {
                    if let Some(previous) = versions.get_mut(&(
                        secret.tenant_id.clone(),
                        previous_version_id.to_string(),
                    )) {
                        previous.status = SecretVersionStatus::Superseded;
                        previous.superseded_at = Some(secret.updated_at);
                    }
                }
                versions.insert(
                    (version.tenant_id.clone(), version.secret_version_id.clone()),
                    version,
                );
                drop(versions);
                self.secrets
                    .lock()
                    .insert((secret.tenant_id.clone(), secret.secret_ref.clone()), secret);
                Ok(())
            })
        }
        fn disable_secret<'a>(
            &'a self,
            secret: kura_secrets::TenantSecret,
        ) -> kura_secrets::BoxFuture<'a, kura_secrets::Result<()>> {
            Box::pin(async move {
                self.secrets
                    .lock()
                    .insert((secret.tenant_id.clone(), secret.secret_ref.clone()), secret);
                Ok(())
            })
        }
        fn get_secret_by_ref<'a>(
            &'a self,
            tenant_id: &'a str,
            secret_ref: &'a str,
        ) -> kura_secrets::BoxFuture<'a, kura_secrets::Result<Option<kura_secrets::TenantSecret>>> {
            Box::pin(async move {
                Ok(self
                    .secrets
                    .lock()
                    .get(&(tenant_id.to_string(), secret_ref.to_string()))
                    .cloned())
            })
        }
        fn get_secret_version<'a>(
            &'a self,
            tenant_id: &'a str,
            secret_version_id: &'a str,
        ) -> kura_secrets::BoxFuture<'a, kura_secrets::Result<Option<kura_secrets::SecretVersion>>> {
            Box::pin(async move {
                Ok(self
                    .versions
                    .lock()
                    .get(&(tenant_id.to_string(), secret_version_id.to_string()))
                    .cloned())
            })
        }
        fn list_secrets<'a>(
            &'a self,
            tenant_id: &'a str,
        ) -> kura_secrets::BoxFuture<'a, kura_secrets::Result<Vec<kura_secrets::TenantSecret>>> {
            Box::pin(async move {
                Ok(self
                    .secrets
                    .lock()
                    .values()
                    .filter(|secret| secret.tenant_id == tenant_id)
                    .cloned()
                    .collect())
            })
        }
    }



    // -----------------------------------------------------------------------
    // Harness (port of newTenantAuthHarness)
    // -----------------------------------------------------------------------

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

    struct Harness {
        state: AppState,
        store: Arc<Mutex<SQLiteStore>>,
        auth_manager: Arc<kura_identity::auth::Manager>,
        auth_header: String,
        token: AccessToken,
        principal: kura_identity::Principal,
        default_tenant: kura_identity::Tenant,
        other_tenant: kura_identity::Tenant,
        now: DateTime<Utc>,
    }

    fn harness(with_identity: bool, seed_token: bool) -> Harness {
        let dir = std::env::temp_dir().join(format!("kura-api-auth-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let store = Arc::new(Mutex::new(
            SQLiteStore::new(dir.to_str().expect("path")).expect("store"),
        ));
        let now = Utc::now();

        let principal = kura_identity::Principal {
            principal_id: "prn_api_local".to_string(),
            principal_kind: kura_identity::PrincipalKind::LocalOperator,
            display_name: "API local operator".to_string(),
            status: LifecycleStatus::Active,
            default_tenant_id: "ten_api_default".to_string(),
            created_at: now,
            updated_at: now,
            disabled_at: None,
            removed_at: None,
        };
        let default_tenant = kura_identity::Tenant {
            tenant_id: principal.default_tenant_id.clone(),
            tenant_kind: kura_identity::TenantKind::Personal,
            display_name: "Default tenant".to_string(),
            status: LifecycleStatus::Active,
            created_at: now,
            updated_at: now,
            created_by_principal_id: principal.principal_id.clone(),
            default_owner_principal_id: principal.principal_id.clone(),
            caller_membership_role: None,
            caller_membership_status: None,
            caller_permissions: Vec::new(),
            default_for_current_token: false,
            default_for_current_principal: false,
        };
        let other_tenant = kura_identity::Tenant {
            tenant_id: "ten_api_other".to_string(),
            tenant_kind: kura_identity::TenantKind::Organization,
            display_name: "Other tenant".to_string(),
            status: LifecycleStatus::Active,
            created_at: now,
            updated_at: now,
            created_by_principal_id: "prn_other".to_string(),
            default_owner_principal_id: "prn_other".to_string(),
            caller_membership_role: None,
            caller_membership_status: None,
            caller_permissions: Vec::new(),
            default_for_current_token: false,
            default_for_current_principal: false,
        };

        {
            let store = store.lock();
            store.upsert_principal(&principal).expect("upsert principal");
            store.upsert_tenant(&default_tenant).expect("upsert default tenant");
            store.upsert_tenant(&other_tenant).expect("upsert other tenant");
            store
                .upsert_membership(&kura_identity::Membership {
                    membership_id: "mem_api_owner".to_string(),
                    tenant_id: default_tenant.tenant_id.clone(),
                    principal_id: principal.principal_id.clone(),
                    role: kura_identity::Role::Owner,
                    status: LifecycleStatus::Active,
                    invitation_id: String::new(),
                    created_at: now,
                    updated_at: now,
                    accepted_at: Some(now),
                    removed_at: None,
                })
                .expect("upsert membership");
        }

        let auth_manager = Arc::new(kura_identity::auth::Manager::new());
        // The pairing-flow test starts from an empty token store (Go
        // TestAuthPairingAndProtectedRoutes), so token seeding is optional.
        let (token, auth_header) = if seed_token {
            let (pairing, code) = auth_manager
                .start_pairing(kura_identity::auth::StartPairingInput {
                    mode: Some(kura_identity::auth::PairingMode::Local),
                    label: "tenant-api".to_string(),
                    ttl_seconds: 0,
                })
                .expect("start pairing");
            let (_, mut token, token_secret) = auth_manager
                .complete_pairing(
                    &pairing.pairing_id,
                    kura_identity::auth::CompletePairingInput { code },
                )
                .expect("complete pairing");
            token.principal_id = principal.principal_id.clone();
            token.default_tenant_id = default_tenant.tenant_id.clone();
            token.status = kura_identity::auth::TokenStatus::Active;
            auth_manager.restore(Vec::new(), vec![token.clone()]);
            {
                let store = store.lock();
                store.upsert_access_token(&token).expect("upsert token");
                store
                    .upsert_token_tenant_grant(&kura_identity::TokenTenantGrant {
                        grant_id: "grant_api_default".to_string(),
                        token_id: token.token_id.clone(),
                        tenant_id: default_tenant.tenant_id.clone(),
                        is_default: true,
                        status: LifecycleStatus::Active,
                        created_at: now,
                        updated_at: now,
                        revoked_at: None,
                        granted_by_principal_id: principal.principal_id.clone(),
                    })
                    .expect("upsert grant");
            }
            (token, format!("Bearer {token_secret}"))
        } else {
            (
                AccessToken {
                    token_id: String::new(),
                    principal_id: principal.principal_id.clone(),
                    label: String::new(),
                    mode: kura_identity::auth::PairingMode::Local,
                    token_hash: String::new(),
                    token_preview: String::new(),
                    status: kura_identity::auth::TokenStatus::Active,
                    default_tenant_id: default_tenant.tenant_id.clone(),
                    created_at: now,
                    updated_at: now,
                    last_used_at: None,
                    expires_at: None,
                    revoked_at: None,
                    rotated_from_token_id: String::new(),
                    rotated_to_token_id: String::new(),
                },
                String::new(),
            )
        };

        let mut state = AppState::new(test_config(), Arc::new(Bus::new()), store.clone());
        state.auth = Some(auth_manager.clone());
        if with_identity {
            // Erase the store to the object-safe Store trait first: Manager
            // stores S in several fields, so Arc<Manager<Concrete>> cannot
            // unsize to Arc<Manager<dyn Store>>; the manager is constructed
            // directly over the erased store (same as app wiring).
            let erased: Arc<dyn kura_identity::Store + Send + Sync> =
                Arc::new(TestIdentityStore { store: store.clone() });
            let manager = kura_identity::Manager::new(erased);
            let identity: Arc<
                kura_identity::Manager<dyn kura_identity::Store + Send + Sync>,
            > = Arc::new(manager);
            state.identity = Some(identity);
        }

        Harness {
            state,
            store,
            auth_manager,
            auth_header,
            token,
            principal,
            default_tenant,
            other_tenant,
            now,
        }
    }

    impl Harness {
        /// Go tenantAuthHarness.issuePrincipalToken: viewer membership on the
        /// default tenant + a default grant for the issued token.
        fn issue_principal_token(
            &self,
            principal_id: &str,
            display_name: &str,
        ) -> (String, kura_identity::Principal) {
            let principal = kura_identity::Principal {
                principal_id: principal_id.to_string(),
                principal_kind: kura_identity::PrincipalKind::User,
                display_name: display_name.to_string(),
                status: LifecycleStatus::Active,
                default_tenant_id: self.default_tenant.tenant_id.clone(),
                created_at: self.now,
                updated_at: self.now,
                disabled_at: None,
                removed_at: None,
            };
            self.store.lock().upsert_principal(&principal).expect("upsert principal");
            let (pairing, code) = self
                .auth_manager
                .start_pairing(kura_identity::auth::StartPairingInput {
                    mode: Some(kura_identity::auth::PairingMode::Local),
                    label: display_name.to_string(),
                    ttl_seconds: 0,
                })
                .expect("start pairing");
            let (_, mut token, secret) = self
                .auth_manager
                .complete_pairing(
                    &pairing.pairing_id,
                    kura_identity::auth::CompletePairingInput { code },
                )
                .expect("complete pairing");
            token.principal_id = principal_id.to_string();
            token.default_tenant_id = self.default_tenant.tenant_id.clone();
            token.status = kura_identity::auth::TokenStatus::Active;
            self.auth_manager.update_token(token.clone());
            {
                let store = self.store.lock();
                store.upsert_access_token(&token).expect("upsert token");
                store
                    .upsert_membership(&kura_identity::Membership {
                        membership_id: format!("mem_{principal_id}"),
                        tenant_id: self.default_tenant.tenant_id.clone(),
                        principal_id: principal_id.to_string(),
                        role: kura_identity::Role::Viewer,
                        status: LifecycleStatus::Active,
                        invitation_id: String::new(),
                        created_at: self.now,
                        updated_at: self.now,
                        accepted_at: None,
                        removed_at: None,
                    })
                    .expect("upsert membership");
                store
                    .upsert_token_tenant_grant(&kura_identity::TokenTenantGrant {
                        grant_id: format!("grant_{principal_id}"),
                        token_id: token.token_id.clone(),
                        tenant_id: self.default_tenant.tenant_id.clone(),
                        is_default: true,
                        status: LifecycleStatus::Active,
                        created_at: self.now,
                        updated_at: self.now,
                        revoked_at: None,
                        granted_by_principal_id: self.principal.principal_id.clone(),
                    })
                    .expect("upsert grant");
            }
            (format!("Bearer {secret}"), principal)
        }

        /// Go tenantAuthHarness.setDefaultMembershipRole.
        fn set_default_membership_role(
            &self,
            principal_id: &str,
            role: kura_identity::Role,
            status: LifecycleStatus,
        ) {
            let membership = kura_identity::Membership {
                membership_id: format!("mem_{principal_id}"),
                tenant_id: self.default_tenant.tenant_id.clone(),
                principal_id: principal_id.to_string(),
                role,
                status,
                invitation_id: String::new(),
                created_at: self.now,
                updated_at: self.now,
                accepted_at: None,
                removed_at: None,
            };
            self.store.lock().upsert_membership(&membership).expect("upsert membership");
        }

        /// Go tenantAuthHarness.setTokenStatus.
        fn set_token_status(&self, principal_id: &str, status: kura_identity::auth::TokenStatus) {
            for token in self.auth_manager.list_tokens() {
                if token.principal_id != principal_id {
                    continue;
                }
                let mut token = token;
                token.status = status;
                self.auth_manager.update_token(token.clone());
                self.store.lock().upsert_access_token(&token).expect("upsert token");
                return;
            }
            panic!("token for principal {principal_id} not found");
        }
    }

    // -----------------------------------------------------------------------
    // App builders: protected_app exercises the real protected() middleware
    // (like the Go server), plain_app hits the handlers directly with manually
    // installed extensions (like the Go direct-handler tests).
    // -----------------------------------------------------------------------

    /// The auth family without the protected() middleware, for tests that
    /// inject tenant/token extensions directly and exercise the handlers'
    /// own permission checks.
    fn plain_app(state: AppState) -> axum::Router {
        Router::new()
            .merge(super::router())
            .merge(super::open_router())
            .with_state(state)
    }

    fn protected_app(state: AppState) -> axum::Router {
        use axum::middleware::from_fn_with_state;
        let protected_routes = Router::new()
            .route("/v1/auth/me", get(auth_me))
            .route("/v1/auth/tokens", get(auth_tokens_list).post(auth_token_create))
            .route("/v1/auth/tokens/{token_id}/rotate", post(auth_token_rotate))
            .route("/v1/auth/tokens/{token_id}/revoke", post(auth_token_revoke))
            .route(
                "/v1/auth/tokens/{token_id}/tenant-grants",
                patch(auth_token_grant_update),
            )
            .route("/v1/tenants", get(tenants_list).post(tenant_create))
            .route("/v1/tenants/{tenant_id}", get(tenant_detail))
            .route("/v1/tenants/{tenant_id}/memberships", get(tenant_memberships_list))
            .route(
                "/v1/tenants/{tenant_id}/memberships/{membership_id}",
                patch(tenant_membership_update).delete(tenant_membership_remove),
            )
            .route(
                "/v1/tenants/{tenant_id}/invitations",
                get(tenant_invitations_list).post(tenant_invitation_create),
            )
            .route("/v1/tenants/{tenant_id}/permissions", get(tenant_permissions))
            .route("/v1/tenant-invitations", get(tenant_invitations_self_list))
            .route(
                "/v1/tenant-invitations/{invitation_id}/accept",
                post(tenant_invitation_accept),
            )
            .route(
                "/v1/tenant-invitations/{invitation_id}/reject",
                post(tenant_invitation_reject),
            )
            .route("/v1/principals", get(principals_list))
            .route("/v1/principals/{principal_id}", patch(principal_update))
            .route("/v1/tenant-audit-events", get(tenant_audit_events_list))
            .route_layer(from_fn_with_state(state.clone(), crate::middleware::protected));
        let unprotected = Router::new()
            .route("/v1/auth/pairings/start", post(auth_pairing_start))
            .route(
                "/v1/auth/pairings/{pairing_id}/complete",
                post(auth_pairing_complete),
            );
        unprotected.merge(protected_routes).with_state(state)
    }

    fn request(method: &str, uri: &str, body: Option<&str>) -> HttpRequest<axum::body::Body> {
        request_with(method, uri, body, None, None)
    }

    fn request_with(
        method: &str,
        uri: &str,
        body: Option<&str>,
        auth: Option<&str>,
        tenant_id: Option<&str>,
    ) -> HttpRequest<axum::body::Body> {
        let mut builder = HttpRequest::builder().method(method).uri(uri);
        if let Some(auth) = auth {
            builder = builder.header("authorization", auth);
        }
        if let Some(tenant_id) = tenant_id {
            builder = builder.header("x-kura-tenant-id", tenant_id);
        }
        builder
            .body(match body {
                Some(body) => axum::body::Body::from(body.to_string()),
                None => axum::body::Body::empty(),
            })
            .expect("request")
    }

    fn with_tenant_extension(
        mut req: HttpRequest<axum::body::Body>,
        tc: kura_identity::TenantContext,
    ) -> HttpRequest<axum::body::Body> {
        req.extensions_mut().insert(TenantContext(tc));
        req
    }

    async fn send(
        app: &axum::Router,
        req: HttpRequest<axum::body::Body>,
    ) -> (StatusCode, serde_json::Value) {
        let response = app.clone().oneshot(req).await.expect("oneshot");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.expect("body");
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }



    // -----------------------------------------------------------------------
    // Ported Go handler tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn auth_pairing_and_protected_routes() {
        // Port of Go TestAuthPairingAndProtectedRoutes (no identity manager).
        let h = harness(false, false);
        let app = protected_app(h.state.clone());

        let (status, _) = send(&app, request("GET", "/v1/auth/me", None)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let (status, json) = send(
            &app,
            request(
                "POST",
                "/v1/auth/pairings/start",
                Some(r#"{"mode":"local","label":"web-ui"}"#),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "body: {json}");
        let pairing_id = json["pairing"]["pairingId"].as_str().expect("pairing id").to_string();
        let pairing_code = json["pairingCode"].as_str().expect("pairing code").to_string();
        assert!(!pairing_id.is_empty() && !pairing_code.is_empty());

        let complete_uri = format!("/v1/auth/pairings/{pairing_id}/complete");
        let complete_body = format!(r#"{{"code":"{pairing_code}"}}"#);
        let (status, json) = send(&app, request("POST", &complete_uri, Some(&complete_body))).await;
        assert_eq!(status, StatusCode::OK, "body: {json}");
        let access_token = json["accessToken"].as_str().expect("access token").to_string();
        let token_id = json["token"]["tokenId"].as_str().expect("token id").to_string();
        assert!(!access_token.is_empty());

        let (status, json) = send(
            &app,
            request_with("GET", "/v1/auth/me", None, Some(&format!("Bearer {access_token}")), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {json}");
        assert_eq!(json["tokenId"], serde_json::json!(token_id));

        let pairings = h.store.lock().list_pairings().expect("list pairings");
        assert_eq!(pairings.len(), 1);
        assert_eq!(pairings[0].status, kura_identity::auth::PairingStatus::Completed);
        let tokens = h.store.lock().list_access_tokens().expect("list tokens");
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].token_id, token_id);
    }

    #[tokio::test]
    async fn auth_me_includes_resolved_tenant_context() {
        // Port of Go TestAuthMeIncludesResolvedTenantContext.
        let h = harness(true, true);
        let app = protected_app(h.state.clone());

        let (status, json) =
            send(&app, request_with("GET", "/v1/auth/me", None, Some(&h.auth_header), None)).await;
        assert_eq!(status, StatusCode::OK, "body: {json}");
        assert_eq!(json["token"]["tokenId"], serde_json::json!(h.token.token_id));
        assert_eq!(
            json["principal"]["principalId"],
            serde_json::json!(h.principal.principal_id)
        );
        assert_eq!(
            json["defaultTenant"]["tenantId"],
            serde_json::json!(h.default_tenant.tenant_id)
        );
        assert_eq!(
            json["currentTenant"]["tenantId"],
            serde_json::json!(h.default_tenant.tenant_id)
        );
        assert_eq!(
            json["tenantContext"]["tenantId"],
            serde_json::json!(h.default_tenant.tenant_id)
        );
        let allowed = json["allowedTenants"].as_array().expect("allowed tenants");
        assert_eq!(allowed.len(), 1);
        assert_eq!(allowed[0]["tenantId"], serde_json::json!(h.default_tenant.tenant_id));
        let grants = json["tokenGrants"].as_array().expect("token grants");
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0]["tenantId"], serde_json::json!(h.default_tenant.tenant_id));
        let permissions = json["permissions"].as_array().expect("permissions");
        assert!(permissions.contains(&serde_json::json!("tenant.manage")));
    }

    #[tokio::test]
    async fn tenant_inspection_honors_explicit_selection_and_stable_denial() {
        // Port of Go TestTenantInspectionHonorsExplicitTenantSelectionAndStableDenial.
        let h = harness(true, true);
        let app = protected_app(h.state.clone());

        let uri = format!("/v1/tenants/{}", h.default_tenant.tenant_id);
        let (status, json) = send(
            &app,
            request_with(
                "GET",
                &uri,
                None,
                Some(&h.auth_header),
                Some(&h.default_tenant.tenant_id),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {json}");
        assert_eq!(json["tenant"]["tenantId"], serde_json::json!(h.default_tenant.tenant_id));
        assert_eq!(
            json["tenantContext"]["tenantSource"],
            serde_json::json!("explicit_header")
        );

        // The denied case surfaces the stable tenant denial through the
        // handler's tenant-ownership check (default tenant context, foreign
        // path tenant): {error, errorCode}.
        let app = plain_app(h.state.clone());
        let uri = format!("/v1/tenants/{}", h.other_tenant.tenant_id);
        let mut req = request("GET", &uri, None);
        req = with_tenant_extension(
            req,
            kura_identity::TenantContext {
                principal_id: h.principal.principal_id.clone(),
                token_id: h.token.token_id.clone(),
                tenant_id: h.default_tenant.tenant_id.clone(),
                tenant_source: "default".to_string(),
                role: Some(kura_identity::Role::Owner),
                permissions: permissions_for_role(
                    kura_identity::Role::Owner,
                    LifecycleStatus::Active,
                ),
                resolved_at: h.now,
                ..kura_identity::TenantContext::default()
            },
        );
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "body: {json}");
        assert_eq!(json["errorCode"], serde_json::json!("tenant_access_denied"));
        assert_eq!(json["error"], serde_json::json!("tenant access denied"));
    }

    #[tokio::test]
    async fn auth_token_lifecycle_routes_cover_issue_grant_rotate_and_revoke() {
        // Port of Go TestAuthTokenLifecycleRoutesCoverIssueGrantRotateAndRevoke.
        let h = harness(true, true);
        let app = protected_app(h.state.clone());

        let (status, json) = send(
            &app,
            request_with(
                "POST",
                "/v1/tenants",
                Some(r#"{"displayName":"Token Org","tenantKind":"organization"}"#),
                Some(&h.auth_header),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "body: {json}");
        let org_tenant_id = json["tenant"]["tenantId"].as_str().expect("tenant id").to_string();

        let issue_body = format!(
            r#"{{"label":"automation","defaultTenantId":"{}","allowedTenantIds":["{}"]}}"#,
            h.default_tenant.tenant_id, h.default_tenant.tenant_id
        );
        let (status, json) = send(
            &app,
            request_with("POST", "/v1/auth/tokens", Some(&issue_body), Some(&h.auth_header), None),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "body: {json}");
        let issued_secret = json["accessToken"].as_str().expect("access token").to_string();
        let issued_token_id = json["token"]["tokenId"].as_str().expect("token id").to_string();
        assert_eq!(json["grants"].as_array().map(Vec::len), Some(1));
        assert_eq!(json["grants"][0]["tenantId"], serde_json::json!(h.default_tenant.tenant_id));

        let (status, _) = send(
            &app,
            request_with("GET", "/v1/auth/tokens", None, Some(&h.auth_header), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let grants_body = format!(
            r#"{{"defaultTenantId":"{}","allowedTenantIds":["{}"]}}"#,
            org_tenant_id, org_tenant_id
        );
        let grants_uri = format!("/v1/auth/tokens/{issued_token_id}/tenant-grants");
        let (status, json) = send(
            &app,
            request_with(
                "PATCH",
                &grants_uri,
                Some(&grants_body),
                Some(&h.auth_header),
                Some(&org_tenant_id),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {json}");

        let rotate_uri = format!("/v1/auth/tokens/{issued_token_id}/rotate");
        let (status, json) = send(
            &app,
            request_with(
                "POST",
                &rotate_uri,
                Some(r#"{"reason":"scheduled"}"#),
                Some(&h.auth_header),
                Some(&org_tenant_id),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {json}");
        assert_eq!(json["oldToken"]["status"], serde_json::json!("rotated"));
        assert_eq!(
            json["newToken"]["rotatedFromTokenId"],
            serde_json::json!(issued_token_id)
        );
        let rotated_secret = json["accessToken"].as_str().expect("rotated secret").to_string();
        let rotated_token_id = json["newToken"]["tokenId"].as_str().expect("new token id").to_string();
        assert!(!rotated_secret.is_empty());

        let (status, _) = send(
            &app,
            request_with("GET", "/v1/auth/me", None, Some(&format!("Bearer {issued_secret}")), None),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let revoke_uri = format!("/v1/auth/tokens/{rotated_token_id}/revoke");
        let (status, json) = send(
            &app,
            request_with(
                "POST",
                &revoke_uri,
                Some(r#"{"reason":"done"}"#),
                Some(&h.auth_header),
                Some(&org_tenant_id),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {json}");

        let (status, _) = send(
            &app,
            request_with(
                "GET",
                "/v1/auth/me",
                None,
                Some(&format!("Bearer {rotated_secret}")),
                Some(&org_tenant_id),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn tenant_permissions_route_reflects_role_derived_permissions() {
        // Port of Go TestTenantPermissionsRouteReflectsRoleDerivedPermissions.
        let h = harness(true, true);
        let app = protected_app(h.state.clone());
        let uri = format!("/v1/tenants/{}/permissions", h.default_tenant.tenant_id);
        let (status, json) =
            send(&app, request_with("GET", &uri, None, Some(&h.auth_header), None)).await;
        assert_eq!(status, StatusCode::OK, "body: {json}");
        let items = json["items"].as_array().expect("items");
        assert!(!items.is_empty());
        for permission in kura_identity::ALL_SENSITIVE_PERMISSIONS {
            let wire = serde_json::json!(permission);
            let allowed = items.iter().any(|item| {
                item["permission"] == wire && item["allowed"] == serde_json::json!(true)
            });
            assert!(allowed, "expected owner to have {wire} allowed");
        }
    }

    #[tokio::test]
    async fn tenant_management_routes_cover_membership_invitation_and_audit() {
        // Port of Go TestTenantManagementRoutesCoverMembershipInvitationAndAudit.
        let h = harness(true, true);
        let app = protected_app(h.state.clone());
        let (invited_header, invited_principal) =
            h.issue_principal_token("prn_invited_api", "Invited API");

        let (status, json) = send(
            &app,
            request_with(
                "POST",
                "/v1/tenants",
                Some(r#"{"displayName":"Acme","tenantKind":"organization"}"#),
                Some(&h.auth_header),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "body: {json}");
        let org_id = json["tenant"]["tenantId"].as_str().expect("tenant id").to_string();
        assert_eq!(json["tenant"]["tenantKind"], serde_json::json!("organization"));

        let invite_body = format!(
            r#"{{"invitedPrincipalId":"{}","role":"operator"}}"#,
            invited_principal.principal_id
        );
        let invite_uri = format!("/v1/tenants/{org_id}/invitations");
        let (status, json) = send(
            &app,
            request_with(
                "POST",
                &invite_uri,
                Some(&invite_body),
                Some(&h.auth_header),
                Some(&org_id),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "body: {json}");
        let invitation_id = json["invitation"]["invitationId"].as_str().expect("invitation id").to_string();

        let accept_uri = format!("/v1/tenant-invitations/{invitation_id}/accept");
        let (status, json) = send(
            &app,
            request_with("POST", &accept_uri, None, Some(&invited_header), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {json}");
        let membership_id = json["membership"]["membershipId"].as_str().expect("membership id").to_string();
        assert_eq!(json["membership"]["role"], serde_json::json!("operator"));

        let membership_uri = format!("/v1/tenants/{org_id}/memberships/{membership_id}");
        let (status, json) = send(
            &app,
            request_with(
                "PATCH",
                &membership_uri,
                Some(r#"{"role":"viewer"}"#),
                Some(&h.auth_header),
                Some(&org_id),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {json}");

        let (status, json) = send(
            &app,
            request_with("DELETE", &membership_uri, None, Some(&h.auth_header), Some(&org_id)),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {json}");

        let principal_uri = format!("/v1/principals/{}", invited_principal.principal_id);
        let (status, json) = send(
            &app,
            request_with(
                "PATCH",
                &principal_uri,
                Some(r#"{"status":"disabled"}"#),
                Some(&h.auth_header),
                Some(&org_id),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {json}");

        let audit_uri = format!("/v1/tenant-audit-events?tenantId={org_id}");
        let (status, json) = send(
            &app,
            request_with("GET", &audit_uri, None, Some(&h.auth_header), Some(&org_id)),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {json}");
    }



    #[tokio::test]
    async fn tenant_audit_events_expose_tenant_scoped_billing_evidence() {
        // Port of Go TestTenantAuditEventsExposeTenantScopedBillingEvidence.
        let h = harness(true, true);
        let app = protected_app(h.state.clone());
        {
            let store = h.store.lock();
            store
                .append_tenant_audit_event(&kura_identity::TenantAuditEvent {
                    audit_event_id: "audit_billing_visible".to_string(),
                    event_kind: "billing.audit_recorded".to_string(),
                    tenant_id: h.default_tenant.tenant_id.clone(),
                    principal_id: h.principal.principal_id.clone(),
                    outcome: "denied".to_string(),
                    reason_code: "quota_denied:run_launches_exhausted".to_string(),
                    created_at: Utc::now(),
                    document: Some(serde_json::Map::new()),
                    ..kura_identity::TenantAuditEvent::default()
                })
                .expect("append visible audit");
            store
                .append_tenant_audit_event(&kura_identity::TenantAuditEvent {
                    audit_event_id: "audit_billing_other".to_string(),
                    event_kind: "billing.audit_recorded".to_string(),
                    tenant_id: "ten_other_billing_audit".to_string(),
                    principal_id: "prn_other".to_string(),
                    outcome: "denied".to_string(),
                    reason_code: "quota_denied:run_launches_exhausted".to_string(),
                    created_at: Utc::now(),
                    document: Some(serde_json::Map::new()),
                    ..kura_identity::TenantAuditEvent::default()
                })
                .expect("append other audit");
        }

        let visible_uri =
            format!("/v1/tenant-audit-events?tenantId={}", h.default_tenant.tenant_id);
        let (status, json) = send(
            &app,
            request_with(
                "GET",
                &visible_uri,
                None,
                Some(&h.auth_header),
                Some(&h.default_tenant.tenant_id),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {json}");
        assert!(json.to_string().contains("audit_billing_visible"));
        assert!(!json.to_string().contains("audit_billing_other"));

        let (status, json) = send(
            &app,
            request_with(
                "GET",
                "/v1/tenant-audit-events?tenantId=ten_other_billing_audit",
                None,
                Some(&h.auth_header),
                Some(&h.default_tenant.tenant_id),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "body: {json}");
        assert_eq!(json["errorCode"], serde_json::json!("tenant_access_denied"));
    }

    #[tokio::test]
    async fn membership_role_update_leaves_audit_visible_role_change_state() {
        // Port of Go TestMembershipRoleUpdateLeavesAuditVisibleRoleChangeState.
        let h = harness(true, true);
        let app = protected_app(h.state.clone());
        let (_header, member_principal) = h.issue_principal_token("prn_role_member", "Role Member");

        let (status, json) = send(
            &app,
            request_with(
                "POST",
                "/v1/tenants",
                Some(r#"{"displayName":"Audit Org","tenantKind":"organization"}"#),
                Some(&h.auth_header),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "body: {json}");
        let org_id = json["tenant"]["tenantId"].as_str().expect("tenant id").to_string();

        h.store
            .lock()
            .upsert_membership(&kura_identity::Membership {
                membership_id: "mem_role_member".to_string(),
                tenant_id: org_id.clone(),
                principal_id: member_principal.principal_id.clone(),
                role: kura_identity::Role::Operator,
                status: LifecycleStatus::Active,
                invitation_id: String::new(),
                created_at: h.now,
                updated_at: h.now,
                accepted_at: None,
                removed_at: None,
            })
            .expect("upsert member membership");

        let uri = format!("/v1/tenants/{org_id}/memberships/mem_role_member");
        let (status, json) = send(
            &app,
            request_with(
                "PATCH",
                &uri,
                Some(r#"{"role":"admin"}"#),
                Some(&h.auth_header),
                Some(&org_id),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {json}");

        let audits = h
            .store
            .lock()
            .list_tenant_audit_events(&kura_identity::AuditEventFilter {
                tenant_id: org_id.clone(),
                event_kind: "tenant.membership_role_updated".to_string(),
                limit: 10,
                ..kura_identity::AuditEventFilter::default()
            })
            .expect("list audits");
        assert_eq!(audits.len(), 1, "audits: {audits:?}");
        assert_eq!(audits[0].principal_id, h.principal.principal_id);
        assert_eq!(audits[0].target_principal_id, member_principal.principal_id);
        assert_eq!(audits[0].tenant_id, org_id);
        let document = audits[0].document.as_ref().expect("audit document");
        assert_eq!(document["membershipId"], serde_json::json!("mem_role_member"));
        assert_eq!(document["oldRole"], serde_json::json!("operator"));
        assert_eq!(document["newRole"], serde_json::json!("admin"));
    }

    #[tokio::test]
    async fn membership_role_update_and_removal_prevent_last_owner_loss() {
        // Port of Go TestMembershipRoleUpdateAndRemovalPreventLastOwnerLoss.
        let h = harness(true, true);
        let app = protected_app(h.state.clone());

        let (status, json) = send(
            &app,
            request_with(
                "POST",
                "/v1/tenants",
                Some(r#"{"displayName":"Owner Guard Org","tenantKind":"organization"}"#),
                Some(&h.auth_header),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "body: {json}");
        let org_id = json["tenant"]["tenantId"].as_str().expect("tenant id").to_string();
        let membership_id = json["membership"]["membershipId"].as_str().expect("membership id").to_string();

        let uri = format!("/v1/tenants/{org_id}/memberships/{membership_id}");
        let (status, json) = send(
            &app,
            request_with(
                "PATCH",
                &uri,
                Some(r#"{"role":"viewer"}"#),
                Some(&h.auth_header),
                Some(&org_id),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "body: {json}");
        assert!(json.to_string().contains("at least one active owner"));

        let (status, json) = send(
            &app,
            request_with("DELETE", &uri, None, Some(&h.auth_header), Some(&org_id)),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "body: {json}");
        assert!(json.to_string().contains("at least one active owner"));

        let memberships = h
            .store
            .lock()
            .list_memberships(&kura_identity::MembershipFilter {
                tenant_id: org_id,
                status: Some(LifecycleStatus::Active),
                role: Some(kura_identity::Role::Owner),
                limit: 10,
                ..kura_identity::MembershipFilter::default()
            })
            .expect("list memberships");
        assert_eq!(memberships.len(), 1);
        assert_eq!(memberships[0].membership_id, membership_id);
    }

    #[tokio::test]
    async fn sensitive_tenant_management_permission_outcomes() {
        // Port of Go TestSensitiveTenantManagementPermissionOutcomes.
        let h = harness(true, true);
        let app = protected_app(h.state.clone());
        let (viewer_header, viewer) = h.issue_principal_token("prn_viewer_api", "Viewer API");
        let (operator_header, _operator) = h.issue_principal_token("prn_operator_api", "Operator API");
        let (admin_header, _admin) = h.issue_principal_token("prn_admin_api", "Admin API");
        let (disabled_header, disabled) = h.issue_principal_token("prn_disabled_api", "Disabled API");
        let (removed_header, _removed) = h.issue_principal_token("prn_removed_api", "Removed API");
        let (revoked_header, _revoked) = h.issue_principal_token("prn_revoked_api", "Revoked API");

        h.set_default_membership_role("prn_operator_api", kura_identity::Role::Operator, LifecycleStatus::Active);
        h.set_default_membership_role("prn_admin_api", kura_identity::Role::Admin, LifecycleStatus::Active);
        h.set_default_membership_role("prn_removed_api", kura_identity::Role::Owner, LifecycleStatus::Removed);
        let mut disabled = disabled;
        disabled.status = LifecycleStatus::Disabled;
        h.store.lock().upsert_principal(&disabled).expect("upsert disabled principal");
        h.set_token_status("prn_revoked_api", kura_identity::auth::TokenStatus::Revoked);

        for (name, header, want) in [
            ("owner", h.auth_header.clone(), StatusCode::CREATED),
            ("admin", admin_header, StatusCode::CREATED),
            ("operator", operator_header, StatusCode::FORBIDDEN),
            ("viewer", viewer_header, StatusCode::FORBIDDEN),
            ("disabled principal", disabled_header, StatusCode::FORBIDDEN),
            ("removed membership", removed_header, StatusCode::FORBIDDEN),
            ("revoked token", revoked_header, StatusCode::UNAUTHORIZED),
        ] {
            let body = format!(r#"{{"displayName":"{name} org","tenantKind":"organization"}}"#);
            let (status, json) = send(
                &app,
                request_with("POST", "/v1/tenants", Some(&body), Some(&header), None),
            )
            .await;
            assert_eq!(status, want, "{name}: body: {json}");
        }

        let audits = h
            .store
            .lock()
            .list_tenant_audit_events(&kura_identity::AuditEventFilter {
                tenant_id: h.default_tenant.tenant_id.clone(),
                principal_id: viewer.principal_id.clone(),
                outcome: "denied".to_string(),
                ..kura_identity::AuditEventFilter::default()
            })
            .expect("list audits");
        assert!(!audits.is_empty(), "expected viewer permission denial audit");
        assert!(
            audits[0].reason_code.contains("tenant.manage"),
            "reason: {}",
            audits[0].reason_code
        );
    }

    #[tokio::test]
    async fn tenant_list_handles_low_hundreds_allowed_tenants() {
        // Port of Go TestTenantListHandlesLowHundredsAllowedTenants.
        let h = harness(true, true);
        let app = protected_app(h.state.clone());
        for idx in 0..220 {
            let tenant_id = format!("ten_bulk_{idx}");
            let store = h.store.lock();
            store
                .upsert_tenant(&kura_identity::Tenant {
                    tenant_id: tenant_id.clone(),
                    tenant_kind: kura_identity::TenantKind::Organization,
                    display_name: format!("Bulk {idx}"),
                    status: LifecycleStatus::Active,
                    created_at: h.now,
                    updated_at: h.now,
                    created_by_principal_id: h.principal.principal_id.clone(),
                    default_owner_principal_id: h.principal.principal_id.clone(),
                    caller_membership_role: None,
                    caller_membership_status: None,
                    caller_permissions: Vec::new(),
                    default_for_current_token: false,
                    default_for_current_principal: false,
                })
                .expect("upsert tenant");
            store
                .upsert_membership(&kura_identity::Membership {
                    membership_id: format!("mem_{tenant_id}"),
                    tenant_id: tenant_id.clone(),
                    principal_id: h.principal.principal_id.clone(),
                    role: kura_identity::Role::Viewer,
                    status: LifecycleStatus::Active,
                    invitation_id: String::new(),
                    created_at: h.now,
                    updated_at: h.now,
                    accepted_at: None,
                    removed_at: None,
                })
                .expect("upsert membership");
            store
                .upsert_token_tenant_grant(&kura_identity::TokenTenantGrant {
                    grant_id: format!("grant_{tenant_id}"),
                    token_id: h.token.token_id.clone(),
                    tenant_id: tenant_id.clone(),
                    is_default: false,
                    status: LifecycleStatus::Active,
                    created_at: h.now,
                    updated_at: h.now,
                    revoked_at: None,
                    granted_by_principal_id: h.principal.principal_id.clone(),
                })
                .expect("upsert grant");
        }

        let (status, json) =
            send(&app, request_with("GET", "/v1/tenants", None, Some(&h.auth_header), None)).await;
        assert_eq!(status, StatusCode::OK, "body: {json}");
        assert_eq!(json["items"].as_array().map(Vec::len), Some(221));
    }



    // -----------------------------------------------------------------------
    // Tenant-secret tests (port of the r37 hosted-credentials handler tests;
    // direct handler calls with tenant contexts, like the Go tests)
    // -----------------------------------------------------------------------

    fn r37_context(
        tenant_id: &str,
        role: kura_identity::Role,
        permissions: Vec<kura_identity::Permission>,
    ) -> kura_identity::TenantContext {
        kura_identity::TenantContext {
            principal_id: format!("principal_{tenant_id}"),
            token_id: format!("token_{tenant_id}"),
            tenant_id: tenant_id.to_string(),
            tenant_source: "test".to_string(),
            role: Some(role),
            permissions,
            resolved_at: Utc::now(),
            ..kura_identity::TenantContext::default()
        }
    }

    fn r37_admin_context(tenant_id: &str) -> kura_identity::TenantContext {
        r37_context(
            tenant_id,
            kura_identity::Role::Admin,
            permissions_for_role(kura_identity::Role::Admin, LifecycleStatus::Active),
        )
    }

    fn r37_viewer_context(tenant_id: &str) -> kura_identity::TenantContext {
        r37_context(
            tenant_id,
            kura_identity::Role::Viewer,
            vec![Permission::ReadOnlyInspect],
        )
    }

    fn secrets_harness() -> (AppState, Arc<kura_secrets::Manager>) {
        let dir = std::env::temp_dir().join(format!("kura-api-secrets-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let store = Arc::new(Mutex::new(
            SQLiteStore::new(dir.to_str().expect("path")).expect("store"),
        ));
        let backend = Arc::new(LocalBackend::new(dir.join("backend")).expect("backend"));
        let manager = Arc::new(kura_secrets::Manager::new(
            Arc::new(TestSecretStore::new()) as Arc<dyn kura_secrets::Store>,
            backend,
        ));
        let mut state = AppState::new(test_config(), Arc::new(Bus::new()), store);
        state.secrets = Some(manager.clone());
        (state, manager)
    }

    #[tokio::test]
    async fn r37_tenant_secret_create_list_metadata_disable_api() {
        // Port of Go TestR37TenantSecretCreateListMetadataDisableAPI.
        let (state, _manager) = secrets_harness();
        let app = plain_app(state.clone());
        let admin = r37_admin_context("ten_r37_a");

        let mut req = request(
            "POST",
            "/v1/tenant-secrets",
            Some(
                r#"{"secretRef":"shared-key","displayName":"Shared Key","value":"R37_FAKE_SECRET_TENANT_A_DO_NOT_LEAK","document":{"owner":"ops"}}"#,
            ),
        );
        req = with_tenant_extension(req, admin.clone());
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::CREATED, "body: {json}");
        assert!(!json.to_string().contains("R37_FAKE_SECRET_TENANT_A_DO_NOT_LEAK"));

        let mut req = request("GET", "/v1/tenant-secrets", None);
        req = with_tenant_extension(req, admin.clone());
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::OK, "body: {json}");
        let items = json["items"].as_array().expect("items");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["tenantId"], serde_json::json!("ten_r37_a"));
        assert_eq!(items[0]["secretRef"], serde_json::json!("shared-key"));

        let mut req = request(
            "PATCH",
            "/v1/tenant-secrets/shared-key",
            Some(r#"{"displayName":"Rotatable Key","document":{"owner":"secops"}}"#),
        );
        req = with_tenant_extension(req, admin.clone());
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::OK, "body: {json}");

        let mut req = request(
            "POST",
            "/v1/tenant-secrets/shared-key/disable",
            Some(r#"{"disabledReason":"operator_request"}"#),
        );
        req = with_tenant_extension(req, admin.clone());
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::OK, "body: {json}");
    }

    #[tokio::test]
    async fn r37_tenant_secret_inspect_get_is_redacted_and_permissioned() {
        // Port of Go TestR37TenantSecretInspectGetIsRedactedAndPermissioned.
        let (state, manager) = secrets_harness();
        let app = plain_app(state.clone());
        manager
            .create(kura_secrets::CreateInput {
                tenant_id: "ten_r37_a".to_string(),
                secret_ref: "inspect-key".to_string(),
                display_name: String::new(),
                value: "R37_FAKE_SECRET_TENANT_A_DO_NOT_LEAK".to_string(),
                document: None,
            })
            .await
            .expect("seed secret");

        let operator = r37_context(
            "ten_r37_a",
            kura_identity::Role::Operator,
            vec![Permission::CredentialsInspect],
        );
        let mut req = request("GET", "/v1/tenant-secrets/inspect-key", None);
        req = with_tenant_extension(req, operator);
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::OK, "body: {json}");
        assert!(!json.to_string().contains("R37_FAKE_SECRET_TENANT_A_DO_NOT_LEAK"));

        let viewer = r37_viewer_context("ten_r37_a");
        let mut req = request("GET", "/v1/tenant-secrets/inspect-key", None);
        req = with_tenant_extension(req, viewer);
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "body: {json}");
        assert!(json.to_string().contains("credential_denied:missing_permission"));
    }

    #[tokio::test]
    async fn r37_tenant_secret_rotate_uses_new_active_version() {
        // Port of Go TestR37TenantSecretRotateAPIUsesNewActiveVersion.
        let (state, manager) = secrets_harness();
        let app = plain_app(state.clone());
        manager
            .create(kura_secrets::CreateInput {
                tenant_id: "ten_r37_a".to_string(),
                secret_ref: "rotating-key".to_string(),
                display_name: String::new(),
                value: "old-value".to_string(),
                document: None,
            })
            .await
            .expect("seed secret");

        let admin = r37_admin_context("ten_r37_a");
        let mut req = request(
            "POST",
            "/v1/tenant-secrets/rotating-key/rotate",
            Some(r#"{"value":"new-value"}"#),
        );
        req = with_tenant_extension(req, admin);
        let (status, json) = send(&app, req).await;
        assert_eq!(status, StatusCode::OK, "body: {json}");

        let resolved = manager
            .resolve(kura_secrets::ResolveInput {
                tenant_id: "ten_r37_a".to_string(),
                secret_ref: "rotating-key".to_string(),
            })
            .await
            .expect("resolve rotated secret");
        assert_eq!(resolved.value, "new-value");
    }

    #[tokio::test]
    async fn unconfigured_managers_return_500() {
        // Pairing start without an auth manager (Go nil-auth-manager 500).
        let dir = std::env::temp_dir().join(format!("kura-api-auth-empty-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let store = Arc::new(Mutex::new(
            SQLiteStore::new(dir.to_str().expect("path")).expect("store"),
        ));
        let state = AppState::new(test_config(), Arc::new(Bus::new()), store);
        let app = plain_app(state.clone());
        let (status, json) = send(
            &app,
            request("POST", "/v1/auth/pairings/start", Some(r#"{"mode":"local","label":"x"}"#)),
        )
        .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "body: {json}");
        assert_eq!(json["message"], "auth manager is not configured");

        // Tenant-secret list without a secrets manager.
        let (status, json) = send(&app, request("GET", "/v1/tenant-secrets", None)).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "body: {json}");
        assert_eq!(json["message"], "tenant secret manager is not configured");
    }
}

