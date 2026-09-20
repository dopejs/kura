//! Request middleware: environment scope, bearer-token auth, tenant
//! resolution, and the by-id tenant ownership guard.
//!
//! Ports of the Go helpers in daemon/internal/api/server.go:
//! - `withEnvironment` -> [`with_environment`]
//! - `protected()`     -> [`protected`]
//! - `authenticateRequest` / `authTokenAuthority` -> private fns here
//! - `withByIDTenantGuard` / `guardResourceForTenant` -> [`guard_resource_for_tenant`]
//!   + [`ByIDTenantGuardLayer`]

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::extract::{Request, State};
use axum::http::header;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tower::Service;

use kura_identity::auth::{AccessToken, AuthError, TokenStatus};
use kura_identity::tenantctx;
use kura_identity::{LifecycleStatus, TenantContext as ResolvedTenantContext, TokenAuthority};

use crate::error::ApiError;
use crate::state::AppState;

/// Environment scope string attached to every request (Go
/// `events.WithEnvironmentScope(ctx, env)` carried through the context).
#[derive(Debug, Clone)]
pub struct EnvironmentScope(pub String);

/// Authenticated access token attached by [`protected`] (Go
/// `withAuthenticatedToken`).
#[derive(Debug, Clone)]
pub struct AuthenticatedToken(pub AccessToken);

/// Resolved tenant context attached by [`protected`] (Go `withTenantContext`).
#[derive(Debug, Clone)]
pub struct TenantContext(pub ResolvedTenantContext);

/// Stage 10.3: per-request correlation shared between the outer
/// `observe_request` layer and the inner `protected` layer. The outer layer
/// creates it (with the request id) before `protected` runs and reads the
/// tenant back after the handler returns; `protected` fills the tenant in.
#[derive(Debug, Clone, Default)]
pub struct RequestContext(pub Arc<parking_lot::Mutex<RequestCorrelation>>);

#[derive(Debug, Clone, Default)]
pub struct RequestCorrelation {
    pub request_id: String,
    pub tenant_id: String,
}

/// The request id header: echoed back when the client sent one, generated
/// otherwise, so every response can be found in the access log.
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// Outer layer: request id, route-template metrics and the structured
/// access log (Stage 10.3). Runs for open and protected routes alike.
pub async fn observe_request(req: Request, next: Next) -> Response {
    let started = std::time::Instant::now();
    let method = req.method().to_string();
    let route = req
        .extensions()
        .get::<axum::extract::MatchedPath>()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| "unmatched".to_string());
    let request_id = req
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty() && v.len() <= 128)
        .map(str::to_string)
        .unwrap_or_else(|| format!("req_{}", uuid::Uuid::now_v7().simple()));
    let correlation = RequestContext(Arc::new(parking_lot::Mutex::new(RequestCorrelation {
        request_id: request_id.clone(),
        tenant_id: String::new(),
    })));
    let mut req = req;
    req.extensions_mut().insert(correlation.clone());

    let mut response = next.run(req).await;
    let status = response.status().as_u16();
    let elapsed = started.elapsed();
    if let Ok(value) = axum::http::HeaderValue::from_str(&request_id) {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
    let tenant_id = correlation.0.lock().tenant_id.clone();
    let metrics = kura_telemetry::metrics::registry();
    metrics.inc(
        kura_telemetry::metrics::HTTP_REQUESTS_TOTAL,
        &[
            ("route", route.as_str()),
            ("method", method.as_str()),
            ("status", kura_telemetry::metrics::status_class(status)),
        ],
    );
    metrics.observe(
        kura_telemetry::metrics::HTTP_REQUEST_DURATION_SECONDS,
        &[("route", route.as_str())],
        elapsed.as_secs_f64(),
    );
    kura_telemetry::access_log(
        &request_id,
        &tenant_id,
        &method,
        &route,
        status,
        elapsed.as_millis(),
    );
    response
}

/// Reads the environment scope from the request extensions, if present.
#[must_use]
pub fn environment_scope(req: &Request) -> Option<&str> {
    req.extensions()
        .get::<EnvironmentScope>()
        .map(|s| s.0.as_str())
}

/// Canonical environment string from the config (Go `effectiveEnvironment`).
#[must_use]
pub fn environment_scope_from_config(config: &kura_config::Config) -> String {
    match config.environment {
        kura_config::Environment::Prod => "prod".to_string(),
        // Embedded shares the test isolation scope; see `environment_scope`.
        kura_config::Environment::Test | kura_config::Environment::Embedded => "test".to_string(),
    }
}

/// `withEnvironment` equivalent: injects the daemon environment scope into the
/// request extensions for unauthenticated routes (pairing entry points).
#[allow(clippy::unused_async)]
pub async fn with_environment(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    req.extensions_mut()
        .insert(EnvironmentScope(environment_scope_from_config(
            &state.config,
        )));
    next.run(req).await
}

/// Bearer-token authentication + tenant resolution, mirroring the Go
/// `protected()` middleware.
///
/// 1. Attaches the environment scope.
/// 2. Refuses tenant-owned requests while the tenant backfill migration is in
///    progress (503 + stable `tenant_migration_in_progress`).
/// 3. When an auth manager is configured: authenticates the `Authorization:
///    Bearer <secret>` token, persists it (store), resolves the tenant context
///    via the identity manager (when configured) using the
///    `X-Kura-Tenant-ID` header, and attaches [`AuthenticatedToken`] +
///    [`TenantContext`] extensions.
/// 4. Drives the downstream handler inside
///    [`tenantctx::scope`](kura_identity::tenantctx::scope), so the
///    store-layer tenant guards — every `*ForTenant` accessor in
///    `kura-tenancy`, which reads the context back through
///    `tenantctx::require()` — actually fire. Before this, the context was
///    attached as an extension only; `require()` failed for every request
///    except the one route family that installed the task-local itself,
///    leaving the fail-closed accessor layer unreachable from HTTP.
///
/// **Propagation limit:** a tokio task-local follows `.await` points within
/// the scope but is *not* inherited by `tokio::spawn`. Work detached from the
/// request task must carry the tenant explicitly, or re-enter a scope of its
/// own.
///
/// When no auth manager is configured the request passes through
/// unauthenticated (matching the Go nil-auth behavior).
#[allow(clippy::unused_async)]
pub async fn protected(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    req.extensions_mut()
        .insert(EnvironmentScope(environment_scope_from_config(
            &state.config,
        )));

    // Roadmap 35 (finding #4): the protected middleware refuses tenant-owned
    // requests while backfills are running so clients can backoff coherently.
    if let Some(status) = &state.tenant_migration_status {
        if status.in_progress() {
            return Err(ApiError::TenantMigrationInProgress);
        }
    }

    let tenant_header = req
        .headers()
        .get("x-kura-tenant-id")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .unwrap_or_default()
        .to_string();

    let Some(auth) = &state.auth else {
        // No auth manager configured: pass through (Go nil-auth behavior).
        return Ok(next.run(req).await);
    };

    let token = authenticate_request(auth, &req)?;

    // Go: persistAccessToken(r.Context(), store, token). The store owns the
    // access-token table; failures surface as 500.
    if let Err(e) = state.store.lock().upsert_access_token(&token) {
        return Err(ApiError::from_store(e));
    }

    req.extensions_mut()
        .insert(AuthenticatedToken(token.clone()));

    if let Some(identity) = &state.identity {
        // Go: deps.Identity.Resolve(ctx, authTokenAuthority(token), tenantID).
        let resolved = identity
            .resolve(&auth_token_authority(&token), &tenant_header)
            .map_err(|err| match err {
                kura_identity::IdentityError::TenantAccessDenied => {
                    // Go: recordTenantAccessDenied + writeTenantDenial (403).
                    // The tenant_resolution_denied audit write and the full
                    // stable Denial body (error/errorCode/requestId) remain
                    // deferred; the 403 itself is ported.
                    ApiError::Forbidden("tenant access denied".to_string())
                }
                other => ApiError::internal(other),
            })?;
        if let Some(correlation) = req.extensions().get::<RequestContext>() {
            correlation.0.lock().tenant_id = resolved.tenant_id.clone();
        }
        req.extensions_mut().insert(TenantContext(resolved.clone()));
        // The extension serves handlers that read the tenant directly; the
        // task-local serves the store-layer guards underneath them. Both are
        // installed from the same resolved value so they cannot disagree.
        return Ok(tenantctx::scope(resolved, next.run(req)).await);
    }

    Ok(next.run(req).await)
}

/// `authenticateRequest` equivalent: extracts and validates the bearer token.
/// The auth-manager-nil case is handled by the caller (`protected` only calls
/// this when a manager is configured); an empty header maps to
/// `ErrAuthRequired` like the Go `if deps.Auth != nil && !ok` branch.
fn authenticate_request(
    auth: &kura_identity::auth::Manager,
    req: &Request,
) -> Result<AccessToken, ApiError> {
    let header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .unwrap_or_default();
    if header.is_empty() {
        return Err(ApiError::Unauthorized(AuthError::AuthRequired.to_string()));
    }
    let secret = header
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::Unauthorized(AuthError::TokenInvalid.to_string()))?;
    let token = auth.authenticate(secret).map_err(|err| {
        // Go: token-expired requests additionally record a
        // tenant.access_denied audit event (reason_code=token_expired)
        // when the token identity is known. The Rust auth manager returns
        // no token alongside the error, so that audit path remains deferred.
        ApiError::Unauthorized(err.to_string())
    })?;
    Ok(token)
}

/// `authTokenAuthority` equivalent: projects an access token onto the identity
/// layer's token authority (Go defaults an empty status to Active).
#[must_use]
pub fn auth_token_authority(token: &AccessToken) -> TokenAuthority {
    let status = match token.status {
        TokenStatus::Active => LifecycleStatus::Active,
        TokenStatus::Revoked => LifecycleStatus::Revoked,
        TokenStatus::Expired => LifecycleStatus::Expired,
        TokenStatus::Rotated => LifecycleStatus::Rotated,
    };
    TokenAuthority {
        token_id: token.token_id.clone(),
        principal_id: token.principal_id.clone(),
        default_tenant_id: token.default_tenant_id.clone(),
        status,
        expires_at: token.expires_at,
    }
}

/// Core tenant-ownership check (Go `guardResourceForTenant`).
///
/// Returns `Ok(())` when the request may proceed: no tenant context, row
/// absent (the downstream handler decides), row owned by the caller, or a
/// legacy pre-backfill row (`tenant_id IS NULL`). Returns `Err(NotFound)` after
/// emitting an audit event when the row belongs to a different tenant (the
/// cross-tenant access is never leaked as such — 404). Any store failure
/// surfaces as `Err(Internal)`.
///
/// `surface` is the canonical `api:<METHOD route>` label used in audit
/// emissions; `table`/`pk_column` must be trusted compile-time constants.
pub async fn guard_resource_for_tenant(
    state: &AppState,
    tenant: Option<&TenantContext>,
    surface: &str,
    table: &str,
    pk_column: &str,
    pk_value: &str,
    resource_kind: &str,
) -> Result<(), ApiError> {
    let Some(tc) = tenant else {
        return Ok(());
    };
    if tc.0.tenant_id.is_empty() {
        return Ok(());
    }
    let owner = state
        .store_pool
        .read()
        .lookup_row_tenant(table, pk_column, pk_value)
        .map_err(ApiError::from_store)?;
    match owner {
        // Pre-backfill rows (NULL tenant) stay visible to every tenant.
        None => Ok(()),
        Some(o) if o.is_empty() || o == tc.0.tenant_id => Ok(()),
        Some(_) => {
            emit_tenant_breach(state, &tc.0, surface, resource_kind);
            Err(ApiError::NotFound("not found".to_string()))
        }
    }
}

/// Guard for **daemon-global** mutations — changes that rewrite the running
/// assembly for every tenant at once (the plugin profile, self-improvement
/// applies, capability registration). These are not per-tenant rows, so tenant
/// scoping is the wrong model for them; the question is whether the caller may
/// reconfigure the daemon at all.
///
/// Passes when no tenant is acting (the single-user assembly, where the
/// operator is the only principal) and when the caller holds `Role::Owner`.
/// Anything else is a 403: unlike the by-id guard there is nothing to
/// disclose, so the denial is stated rather than hidden behind a 404.
///
/// **Recorded decision (2026-09-18):** the bar is `Role::Owner` rather than a
/// new `Permission` variant. No existing permission expresses "may change the
/// daemon's assembly", and adding one is a cross-language contract change
/// (the enum is serialized into `schemas/`); that deserves a deliberate
/// decision rather than arriving as a side effect of closing this guard.
pub fn require_daemon_global_operator(
    tenant: Option<&TenantContext>,
    surface: &str,
) -> Result<(), ApiError> {
    let Some(tc) = tenant else { return Ok(()) };
    if tc.0.tenant_id.trim().is_empty() {
        return Ok(());
    }
    match tc.0.role {
        Some(kura_identity::Role::Owner) => Ok(()),
        _ => Err(ApiError::Forbidden(format!(
            "{surface} changes the daemon assembly and requires the tenant owner role"
        ))),
    }
}

/// Publishes the cross-tenant denial audit event. Safe no-op when no emitter
/// is configured. The emitter reads the acting tenant from the task-local
/// carrier, so the resolved tenant context is installed around the emit.
fn emit_tenant_breach(
    state: &AppState,
    tenant: &ResolvedTenantContext,
    surface: &str,
    resource_kind: &str,
) {
    let Some(emitter) = &state.audit_emitter else {
        return;
    };
    let _ = tenantctx::with_context(tenant.clone(), || emitter.emit(surface, resource_kind));
}

/// Canonical surface label for audit emissions (Go `surfaceFromRequest`):
/// `api:<METHOD route>`.
#[must_use]
pub fn surface_from_request(req: &Request) -> String {
    format!("api:{} {}", req.method(), req.uri().path())
}

/// A `tower::Layer` implementing the Go `withByIDTenantGuard` wrapper: before
/// the inner handler runs, the resource id (first path segment after
/// `prefix`) is verified against the caller's resolved tenant via
/// [`guard_resource_for_tenant`]. Route families attach it with
/// `route_layer` / `layer`.
#[derive(Clone)]
pub struct ByIDTenantGuardLayer {
    state: AppState,
    prefix: &'static str,
    table: &'static str,
    pk_column: &'static str,
    resource_kind: &'static str,
}

impl ByIDTenantGuardLayer {
    /// Creates the guard layer for a `/v1/<prefix>` route family.
    #[must_use]
    pub fn new(
        state: AppState,
        prefix: &'static str,
        table: &'static str,
        pk_column: &'static str,
        resource_kind: &'static str,
    ) -> Self {
        Self {
            state,
            prefix,
            table,
            pk_column,
            resource_kind,
        }
    }
}

impl<S> tower::Layer<S> for ByIDTenantGuardLayer {
    type Service = ByIDTenantGuard<S>;

    fn layer(&self, inner: S) -> Self::Service {
        ByIDTenantGuard {
            inner,
            state: self.state.clone(),
            prefix: self.prefix,
            table: self.table,
            pk_column: self.pk_column,
            resource_kind: self.resource_kind,
        }
    }
}

/// Service produced by [`ByIDTenantGuardLayer`].
#[derive(Clone)]
pub struct ByIDTenantGuard<S> {
    inner: S,
    state: AppState,
    prefix: &'static str,
    table: &'static str,
    pk_column: &'static str,
    resource_kind: &'static str,
}

impl<S> Service<Request> for ByIDTenantGuard<S>
where
    S: Service<Request, Response = Response, Error = Infallible> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = Response;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Response, Infallible>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request) -> Self::Future {
        let mut inner = self.inner.clone();
        let state = self.state.clone();
        let prefix = self.prefix;
        let table = self.table;
        let pk_column = self.pk_column;
        let resource_kind = self.resource_kind;
        Box::pin(async move {
            let tenant = req.extensions().get::<TenantContext>().cloned();
            let id = id_from_path(req.uri().path(), prefix);
            let surface = surface_from_request(&req);
            if let Some(id) = id {
                if let Err(err) = guard_resource_for_tenant(
                    &state,
                    tenant.as_ref(),
                    &surface,
                    table,
                    pk_column,
                    id,
                    resource_kind,
                )
                .await
                {
                    return Ok(err.into_response());
                }
            }
            inner.call(req).await
        })
    }
}

/// Extracts the resource id from a request path relative to the route prefix:
/// the first segment after the prefix (Go `withByIDTenantGuard`). Returns
/// `None` when the path does not start with the prefix or has no id segment.
fn id_from_path<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = path.strip_prefix(prefix)?;
    let id = rest.split('/').next().unwrap_or("");
    if id.is_empty() { None } else { Some(id) }
}

#[cfg(test)]
mod tenant_task_local_tests {
    //! Regression cover for the defect where `protected()` attached the
    //! resolved tenant as an extension but never installed the task-local,
    //! leaving `tenantctx::require()` — and therefore every `kura-tenancy`
    //! fail-closed accessor — inoperative for all but one route family.

    use std::sync::Arc;

    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use tower::ServiceExt;

    use kura_identity::auth::{IssueTokenInput, Manager as AuthManager};
    use kura_identity::tenantctx;
    use kura_identity::{
        AuditStore, IdentityError, InvitationFilter, Membership, MembershipFilter, Principal,
        PrincipalFilter, ResolverStore, Store as IdentityStore, Tenant, TenantAuditEvent,
        TenantFilter, TenantInvitation, TokenAuthority, TokenTenantGrant,
    };

    use crate::routes::tests_support::test_state;

    const TENANT: &str = "tnt_probe";
    const PRINCIPAL: &str = "prn_probe";

    /// Minimal identity store: enough for `Resolver::resolve` to succeed for
    /// one active principal holding one active membership in one active
    /// tenant. Every mutation path is out of scope for this test.
    struct FakeIdentityStore;

    impl ResolverStore for FakeIdentityStore {
        fn get_principal(&self, principal_id: &str) -> Result<Option<Principal>, IdentityError> {
            if principal_id != PRINCIPAL {
                return Ok(None);
            }
            Ok(Some(
                serde_json::from_value(serde_json::json!({
                    "principalId": PRINCIPAL,
                    "principalKind": "local_operator",
                    "displayName": "probe",
                    "status": "active",
                    "defaultTenantId": TENANT,
                    "createdAt": "2026-01-01T00:00:00Z",
                    "updatedAt": "2026-01-01T00:00:00Z",
                }))
                .expect("principal"),
            ))
        }

        fn get_tenant(&self, tenant_id: &str) -> Result<Option<Tenant>, IdentityError> {
            if tenant_id != TENANT {
                return Ok(None);
            }
            Ok(Some(
                serde_json::from_value(serde_json::json!({
                    "tenantId": TENANT,
                    "tenantKind": "organization",
                    "displayName": "probe",
                    "status": "active",
                    "createdAt": "2026-01-01T00:00:00Z",
                    "updatedAt": "2026-01-01T00:00:00Z",
                }))
                .expect("tenant"),
            ))
        }

        fn list_memberships(
            &self,
            filter: &MembershipFilter,
        ) -> Result<Vec<Membership>, IdentityError> {
            if filter.tenant_id != TENANT {
                return Ok(Vec::new());
            }
            Ok(vec![
                serde_json::from_value(serde_json::json!({
                    "membershipId": "mem_probe",
                    "tenantId": TENANT,
                    "principalId": PRINCIPAL,
                    "role": "owner",
                    "status": "active",
                    "createdAt": "2026-01-01T00:00:00Z",
                    "updatedAt": "2026-01-01T00:00:00Z",
                }))
                .expect("membership"),
            ])
        }

        fn list_token_tenant_grants(
            &self,
            token_id: &str,
        ) -> Result<Vec<TokenTenantGrant>, IdentityError> {
            Ok(vec![
                serde_json::from_value(serde_json::json!({
                    "grantId": "grt_probe",
                    "tokenId": token_id,
                    "tenantId": TENANT,
                    "isDefault": true,
                    "status": "active",
                    "createdAt": "2026-01-01T00:00:00Z",
                    "updatedAt": "2026-01-01T00:00:00Z",
                }))
                .expect("grant"),
            ])
        }
    }

    impl AuditStore for FakeIdentityStore {
        fn append_tenant_audit_event(
            &self,
            event: TenantAuditEvent,
        ) -> Result<TenantAuditEvent, IdentityError> {
            Ok(event)
        }
    }

    impl IdentityStore for FakeIdentityStore {
        fn upsert_tenant(&self, _: &Tenant) -> Result<(), IdentityError> {
            unimplemented!("not exercised by the task-local probe")
        }
        fn upsert_principal(&self, _: &Principal) -> Result<(), IdentityError> {
            unimplemented!("not exercised by the task-local probe")
        }
        fn upsert_membership(&self, _: &Membership) -> Result<(), IdentityError> {
            unimplemented!("not exercised by the task-local probe")
        }
        fn upsert_tenant_invitation(&self, _: &TenantInvitation) -> Result<(), IdentityError> {
            unimplemented!("not exercised by the task-local probe")
        }
        fn upsert_token_tenant_grant(&self, _: &TokenTenantGrant) -> Result<(), IdentityError> {
            unimplemented!("not exercised by the task-local probe")
        }
        fn list_tenants(&self, _: &TenantFilter) -> Result<Vec<Tenant>, IdentityError> {
            unimplemented!("not exercised by the task-local probe")
        }
        fn list_principals(&self, _: &PrincipalFilter) -> Result<Vec<Principal>, IdentityError> {
            unimplemented!("not exercised by the task-local probe")
        }
        fn list_tenant_invitations(
            &self,
            _: &InvitationFilter,
        ) -> Result<Vec<TenantInvitation>, IdentityError> {
            unimplemented!("not exercised by the task-local probe")
        }
        fn list_token_authorities(&self) -> Result<Vec<TokenAuthority>, IdentityError> {
            unimplemented!("not exercised by the task-local probe")
        }
    }

    /// Stands in for any handler sitting under a store-layer tenant guard:
    /// it reads the tenant the way `kura-tenancy` accessors do.
    async fn probe() -> String {
        match tenantctx::require() {
            Ok(tenant_id) => tenant_id,
            Err(err) => format!("ERR:{err}"),
        }
    }

    async fn body_text(response: axum::response::Response) -> String {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        String::from_utf8(bytes.to_vec()).expect("utf8")
    }

    #[tokio::test]
    async fn protected_installs_tenant_task_local_for_downstream_handlers() {
        let mut state = test_state();
        let auth = Arc::new(AuthManager::new());
        let (_token, secret) = auth
            .issue_token(IssueTokenInput {
                principal_id: PRINCIPAL.to_string(),
                label: "probe".to_string(),
                default_tenant_id: TENANT.to_string(),
                expires_at: None,
            })
            .expect("issue token");
        state.auth = Some(auth);

        let erased: Arc<dyn IdentityStore + Send + Sync> = Arc::new(FakeIdentityStore);
        state.identity = Some(Arc::new(kura_identity::Manager::new(erased)));

        let app =
            Router::new()
                .route("/probe", get(probe))
                .layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    super::protected,
                ));

        let request = Request::builder()
            .uri("/probe")
            .header("authorization", format!("Bearer {secret}"))
            .body(Body::empty())
            .expect("request");
        let response = app.oneshot(request).await.expect("oneshot");

        assert_eq!(response.status(), StatusCode::OK);
        // Before the fix this asserted `ERR:tenant context is required`.
        assert_eq!(body_text(response).await, TENANT);
    }

    #[tokio::test]
    async fn without_identity_manager_the_task_local_stays_absent() {
        // Single-user assembly: no identity manager, so no tenant context and
        // no task-local. `require()` must fail closed rather than default to
        // some ambient tenant.
        let mut state = test_state();
        let auth = Arc::new(AuthManager::new());
        let (_token, secret) = auth
            .issue_token(IssueTokenInput {
                principal_id: PRINCIPAL.to_string(),
                label: "probe".to_string(),
                default_tenant_id: TENANT.to_string(),
                expires_at: None,
            })
            .expect("issue token");
        state.auth = Some(auth);

        let app =
            Router::new()
                .route("/probe", get(probe))
                .layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    super::protected,
                ));

        let request = Request::builder()
            .uri("/probe")
            .header("authorization", format!("Bearer {secret}"))
            .body(Body::empty())
            .expect("request");
        let response = app.oneshot(request).await.expect("oneshot");

        assert_eq!(response.status(), StatusCode::OK);
        assert!(body_text(response).await.starts_with("ERR:"));
    }
}
