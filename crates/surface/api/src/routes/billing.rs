//! billing route family (port of daemon/internal/api/hosted_billing.go plus
//! the `/v1/billing*` and `/v1/admin/billing*` registrations in server.go).
//!
//! Surface:
//! - `GET /v1/billing/plan` — active plan for the caller's tenant
//! - `GET /v1/billing/usage` — full usage summary (all catalog categories)
//! - `GET /v1/billing/quotas` — `ListResponse<EffectiveQuota>`
//! - `GET /v1/billing/quota-dashboard` — tenant quota dashboard
//! - `GET /v1/billing/denials` — `ListResponse<QuotaDenial>`
//! - `GET /v1/billing/denials/{denial_id}` — denial detail
//! - `POST /v1/billing/denials/{denial_id}/evidence-export` — redacted export
//! - `POST /v1/admin/billing/tenants/{tenant_id}/plan`
//! - `POST /v1/admin/billing/tenants/{tenant_id}/quota-overrides`
//! - `POST /v1/admin/billing/tenants/{tenant_id}/manual-adjustments`
//! - `POST /v1/admin/billing/tenants/{tenant_id}/reservations/{reservation_id}/resolve`
//!
//! Permission gates mirror the Go handlers: every view route requires
//! `billing.view`, the evidence export requires `billing.evidence.export`, and
//! the admin mutations require `billing.manage` with the target tenant equal to
//! the caller's resolved tenant. The `protected()` middleware is applied by the
//! app-wiring layer; handlers read the tenant context opportunistically via
//! `Option<Extension<TenantContext>>` (same convention as the other families).
//!
//! Error mapping: nil manager -> 500, projection failures -> 503 (Go
//! `writeError`), admin validation/manager failures -> 400, missing rows ->
//! 404, permission failures -> 403. `ApiError` has no generic 503 variant, so
//! the 503 `writeError` shape is carried by the local [`BillingApiError`]
//! (precedent: mail.rs `MailApiError`).

use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json as AxumJson, Router};
use chrono::Utc;
use serde::Deserialize;

use kura_billing::{
    BillingError, Category, EffectiveQuota, EnforcementMode, ManualAdjustment, PlanStatus,
    QuotaDenial, QuotaOverride, ReservationStatus, ResolveReservationInput, TenantPlan,
    UsageSummary,
};

use kura_identity::{has_permission, Permission};

use crate::error::ApiError;
use crate::middleware::TenantContext;
use crate::response::Json;
use crate::state::AppState;
use crate::types::ListResponse;

/// Go `billingPlanAssignmentRequest`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BillingPlanAssignmentRequest {
    #[serde(default)]
    plan_key: String,
    #[serde(default)]
    enforcement_mode: EnforcementMode,
    #[serde(default)]
    reason: String,
}

/// Go `billingQuotaOverrideRequest`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BillingQuotaOverrideRequest {
    #[serde(default)]
    category: Category,
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    reason: String,
}

/// Go `billingManualAdjustmentRequest`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BillingManualAdjustmentRequest {
    #[serde(default)]
    category: Category,
    #[serde(default)]
    quota_period_id: String,
    #[serde(default)]
    amount_delta: i64,
    #[serde(default)]
    reason: String,
}

/// Go `billingReservationResolutionRequest`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BillingReservationResolutionRequest {
    #[serde(default)]
    outcome: ReservationStatus,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    amount: i64,
}

/// Handler error for the billing family. The Go handlers write three distinct
/// shapes: `writeError` at 503 for view projection failures, `writeError` at
/// 400 for admin validation/manager failures, and the canonical 400/403/404/500
/// envelopes. `ApiError` covers the latter; the 503 shape is local.
#[derive(Debug)]
enum BillingApiError {
    Api(ApiError),
    /// Go `writeError(w, 503, message)` -> `{"error": message}`.
    ServiceUnavailable(String),
    /// Go `writeError(w, 400, message)` -> `{"error": message}`.
    BadRequest(String),
}

impl From<ApiError> for BillingApiError {
    fn from(err: ApiError) -> Self {
        Self::Api(err)
    }
}

impl IntoResponse for BillingApiError {
    fn into_response(self) -> Response {
        match self {
            Self::Api(err) => err.into_response(),
            Self::ServiceUnavailable(message) => (
                StatusCode::SERVICE_UNAVAILABLE,
                AxumJson(serde_json::json!({ "error": message })),
            )
                .into_response(),
            Self::BadRequest(message) => (
                StatusCode::BAD_REQUEST,
                AxumJson(serde_json::json!({ "error": message })),
            )
                .into_response(),
        }
    }
}

/// Route family router. Only the methods the Go handlers accept are registered;
/// axum answers the other methods with 405 (Go
/// `w.WriteHeader(http.StatusMethodNotAllowed)`).
#[must_use]
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/billing/plan", get(billing_plan))
        .route("/v1/billing/usage", get(billing_usage))
        .route("/v1/billing/quotas", get(billing_quotas))
        .route("/v1/billing/quota-dashboard", get(billing_quota_dashboard))
        .route("/v1/billing/denials", get(billing_denials))
        .route(
            "/v1/billing/denials/{denial_id}",
            get(billing_denial_detail),
        )
        .route(
            "/v1/billing/denials/{denial_id}/evidence-export",
            post(billing_evidence_export),
        )
        .route(
            "/v1/admin/billing/tenants/{tenant_id}/plan",
            post(admin_billing_plan),
        )
        .route(
            "/v1/admin/billing/tenants/{tenant_id}/quota-overrides",
            post(admin_billing_quota_override),
        )
        .route(
            "/v1/admin/billing/tenants/{tenant_id}/manual-adjustments",
            post(admin_billing_manual_adjustment),
        )
        .route(
            "/v1/admin/billing/tenants/{tenant_id}/reservations/{reservation_id}/resolve",
            post(admin_billing_reservation_resolve),
        )
}

// ---------------------------------------------------------------------------
// View routes (Go handleHostedBilling)
// ---------------------------------------------------------------------------

/// GET /v1/billing/plan — active plan for the caller's tenant.
async fn billing_plan(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Json<TenantPlan>, BillingApiError> {
    let manager = billing_manager(&state)?;
    let tc = require_billing_permission(tenant.as_ref().map(|e| &e.0), Permission::BillingView)?;
    let hosted = hosted(&state);
    let plan = manager
        .active_plan(&tc.tenant_id, hosted)
        .await
        .map_err(view_error)?;
    Ok(Json(plan))
}

/// GET /v1/billing/usage — full usage summary across every catalog category.
async fn billing_usage(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Json<UsageSummary>, BillingApiError> {
    let manager = billing_manager(&state)?;
    let tc = require_billing_permission(tenant.as_ref().map(|e| &e.0), Permission::BillingView)?;
    let summary = manager
        .usage_summary(&tc.tenant_id, hosted(&state))
        .await
        .map_err(view_error)?;
    Ok(Json(summary))
}

/// GET /v1/billing/quotas — `ListResponse<EffectiveQuota>` from the usage
/// summary (Go `ListResponse[billing.EffectiveQuota]{Items: summary.Quotas}`).
async fn billing_quotas(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Json<ListResponse<EffectiveQuota>>, BillingApiError> {
    let manager = billing_manager(&state)?;
    let tc = require_billing_permission(tenant.as_ref().map(|e| &e.0), Permission::BillingView)?;
    let summary = manager
        .usage_summary(&tc.tenant_id, hosted(&state))
        .await
        .map_err(view_error)?;
    Ok(Json(ListResponse { items: summary.quotas }))
}

/// GET /v1/billing/quota-dashboard — the full tenant quota dashboard.
async fn billing_quota_dashboard(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Json<kura_billing::TenantQuotaDashboard>, BillingApiError> {
    let manager = billing_manager(&state)?;
    let tc = require_billing_permission(tenant.as_ref().map(|e| &e.0), Permission::BillingView)?;
    let dashboard = manager
        .quota_dashboard(&tc.tenant_id, hosted(&state))
        .await
        .map_err(view_error)?;
    Ok(Json(dashboard))
}

/// GET /v1/billing/denials — `ListResponse<QuotaDenial>` from the usage
/// summary (Go `ListResponse[billing.QuotaDenial]{Items: summary.Denials}`).
async fn billing_denials(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Json<ListResponse<QuotaDenial>>, BillingApiError> {
    let manager = billing_manager(&state)?;
    let tc = require_billing_permission(tenant.as_ref().map(|e| &e.0), Permission::BillingView)?;
    let summary = manager
        .usage_summary(&tc.tenant_id, hosted(&state))
        .await
        .map_err(view_error)?;
    Ok(Json(ListResponse { items: summary.denials }))
}

/// GET /v1/billing/denials/{denial_id} — denial detail for the caller's
/// tenant; cross-tenant lookups hide the record as 404 (the detail projection
/// is tenant-scoped through the manager).
async fn billing_denial_detail(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(denial_id): Path<String>,
) -> Result<Json<kura_billing::QuotaDenialDetail>, BillingApiError> {
    let manager = billing_manager(&state)?;
    let tc = require_billing_permission(tenant.as_ref().map(|e| &e.0), Permission::BillingView)?;
    let detail = manager
        .denial_detail(&tc.tenant_id, &denial_id)
        .await
        .map_err(view_error)?
        .ok_or_else(|| ApiError::NotFound("not found".to_string()))?;
    Ok(Json(detail))
}

/// POST /v1/billing/denials/{denial_id}/evidence-export — redacted evidence
/// export. Requires `billing.evidence.export` (Go `RequirePermission`); the
/// generated-by principal is the caller's principal id.
async fn billing_evidence_export(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(denial_id): Path<String>,
) -> Result<Json<kura_billing::BillingEvidenceExport>, BillingApiError> {
    let manager = billing_manager(&state)?;
    let tc = require_billing_permission(
        tenant.as_ref().map(|e| &e.0),
        Permission::BillingEvidenceExport,
    )?;
    let export = manager
        .evidence_export(&tc.tenant_id, &denial_id, &tc.principal_id, hosted(&state))
        .await
        .map_err(view_error)?
        .ok_or_else(|| ApiError::NotFound("not found".to_string()))?;
    Ok(Json(export))
}

// ---------------------------------------------------------------------------
// Admin routes (Go handleHostedBillingAdmin + the four sub-handlers)
// ---------------------------------------------------------------------------

/// POST /v1/admin/billing/tenants/{tenant_id}/plan — assign a plan. Requires
/// `billing.manage` and the target tenant equal to the caller's tenant.
async fn admin_billing_plan(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(tenant_id): Path<String>,
    body: Bytes,
) -> Result<Json<TenantPlan>, BillingApiError> {
    let manager = billing_manager(&state)?;
    let tc = require_admin_target(tenant.as_ref().map(|e| &e.0), &tenant_id)?;
    let request: BillingPlanAssignmentRequest = decode_json_body(&body)?;
    let plan_key = request.plan_key.trim().to_string();
    let now = Utc::now();
    let plan = TenantPlan {
        // Go: "plan_" + tenantID + "_" + planKey(space->underscore) + "_" +
        // UTC timestamp in the 20060102150405 layout.
        plan_id: format!(
            "plan_{}_{}_{}",
            tenant_id,
            plan_key.replace(' ', "_"),
            now.format("%Y%m%d%H%M%S")
        ),
        tenant_id: tenant_id.clone(),
        plan_key: plan_key.clone(),
        status: PlanStatus::from(PlanStatus::ACTIVE),
        enforcement_mode: request.enforcement_mode.clone(),
        assigned_by_principal_id: tc.principal_id.clone(),
        assignment_reason: request.reason.clone(),
        effective_at: now,
        ..Default::default()
    };
    if plan.plan_key.is_empty() {
        return Err(BillingApiError::BadRequest("planKey is required".to_string()));
    }
    manager
        .assign_plan(plan.clone(), &tc.principal_id, &request.reason)
        .await
        .map_err(admin_error)?;
    Ok(Json(plan))
}

/// POST /v1/admin/billing/tenants/{tenant_id}/quota-overrides — apply a quota
/// override for one category.
async fn admin_billing_quota_override(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(tenant_id): Path<String>,
    body: Bytes,
) -> Result<Json<QuotaOverride>, BillingApiError> {
    let manager = billing_manager(&state)?;
    let tc = require_admin_target(tenant.as_ref().map(|e| &e.0), &tenant_id)?;
    let request: BillingQuotaOverrideRequest = decode_json_body(&body)?;
    let now = Utc::now();
    let override_ = QuotaOverride {
        // Go: "quota_override_" + tenantID + "_" + category + "_" + timestamp.
        quota_override_id: format!(
            "quota_override_{}_{}_{}",
            tenant_id,
            request.category.as_str(),
            now.format("%Y%m%d%H%M%S")
        ),
        tenant_id: tenant_id.clone(),
        category: request.category.clone(),
        limit: request.limit,
        effective_at: now,
        reason: request.reason.clone(),
        created_by_principal_id: tc.principal_id.clone(),
        ..Default::default()
    };
    if kura_billing::definition_for(&request.category).is_none() {
        return Err(BillingApiError::BadRequest(
            "unknown billing quota category".to_string(),
        ));
    }
    manager
        .apply_quota_override(override_.clone())
        .await
        .map_err(admin_error)?;
    Ok(Json(override_))
}

/// POST /v1/admin/billing/tenants/{tenant_id}/manual-adjustments — apply a
/// manual usage adjustment.
async fn admin_billing_manual_adjustment(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(tenant_id): Path<String>,
    body: Bytes,
) -> Result<Json<ManualAdjustment>, BillingApiError> {
    let manager = billing_manager(&state)?;
    let tc = require_admin_target(tenant.as_ref().map(|e| &e.0), &tenant_id)?;
    let request: BillingManualAdjustmentRequest = decode_json_body(&body)?;
    let now = Utc::now();
    let adjustment = ManualAdjustment {
        // Go: "manual_adjustment_" + tenantID + "_" + category + "_" + timestamp.
        adjustment_id: format!(
            "manual_adjustment_{}_{}_{}",
            tenant_id,
            request.category.as_str(),
            now.format("%Y%m%d%H%M%S")
        ),
        tenant_id: tenant_id.clone(),
        category: request.category.clone(),
        quota_period_id: request.quota_period_id.clone(),
        amount_delta: request.amount_delta,
        reason: request.reason.clone(),
        created_by_principal_id: tc.principal_id.clone(),
        created_at: now,
    };
    if adjustment.quota_period_id.is_empty() {
        return Err(BillingApiError::BadRequest(
            "quotaPeriodId is required".to_string(),
        ));
    }
    manager
        .apply_manual_adjustment(adjustment.clone())
        .await
        .map_err(admin_error)?;
    Ok(Json(adjustment))
}

/// POST /v1/admin/billing/tenants/{tenant_id}/reservations/{reservation_id}/resolve
/// — resolve a reservation to a terminal lifecycle outcome.
async fn admin_billing_reservation_resolve(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path((tenant_id, reservation_id)): Path<(String, String)>,
    body: Bytes,
) -> Result<Json<kura_billing::UsageReservation>, BillingApiError> {
    let manager = billing_manager(&state)?;
    let tc = require_admin_target(tenant.as_ref().map(|e| &e.0), &tenant_id)?;
    let request: BillingReservationResolutionRequest = decode_json_body(&body)?;
    let reservation = manager
        .resolve_reservation(ResolveReservationInput {
            tenant_id: tenant_id.clone(),
            reservation_id: reservation_id.clone(),
            outcome: request.outcome.clone(),
            amount: request.amount,
            reason: request.reason.clone(),
            actor_principal_id: tc.principal_id.clone(),
        })
        .await
        .map_err(admin_error)?;
    Ok(Json(reservation))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Go `hosted` flag: production environment enables fail-closed quota
/// accounting (`cfg.Environment == EnvironmentProd`).
fn hosted(state: &AppState) -> bool {
    matches!(state.config.environment, kura_config::Environment::Prod)
}

/// Go `handleHostedBilling`'s nil-manager guard: 500 "billing manager is not
/// configured".
fn billing_manager(state: &AppState) -> Result<&kura_billing::Manager, BillingApiError> {
    state.billing.as_deref().ok_or_else(|| {
        BillingApiError::Api(ApiError::internal("billing manager is not configured"))
    })
}

/// Go `RequirePermission` + `writeTenantDenial(403)`: billing routes need a
/// resolved tenant context carrying the requested permission.
fn require_billing_permission(
    tenant: Option<&TenantContext>,
    permission: Permission,
) -> Result<&kura_identity::TenantContext, BillingApiError> {
    let denied = || ApiError::Forbidden("tenant access denied".to_string());
    let Some(tc) = tenant else {
        return Err(denied().into());
    };
    if tc.0.tenant_id.trim().is_empty() || !has_permission(&tc.0.permissions, permission) {
        return Err(denied().into());
    }
    Ok(&tc.0)
}

/// Go `handleHostedBillingAdmin`: `billing.manage` plus the target tenant must
/// equal the caller's resolved tenant (cross-tenant admin is 403).
fn require_admin_target<'a>(
    tenant: Option<&'a TenantContext>,
    target_tenant_id: &str,
) -> Result<&'a kura_identity::TenantContext, BillingApiError> {
    let tc = require_billing_permission(tenant, Permission::BillingManage)?;
    if target_tenant_id != tc.tenant_id {
        return Err(ApiError::Forbidden("tenant access denied".to_string()).into());
    }
    Ok(tc)
}

/// View-route manager failure: Go `writeError(w, 503, err.Error())`.
fn view_error(err: BillingError) -> BillingApiError {
    BillingApiError::ServiceUnavailable(err.to_string())
}

/// Admin-route manager failure: Go `writeError(w, 400, err.Error())`.
fn admin_error(err: BillingError) -> BillingApiError {
    BillingApiError::BadRequest(err.to_string())
}

/// Go `decodeJSONBody`: empty body -> 400 "request body is required", JSON
/// decode failures -> 400.
fn decode_json_body<T: serde::de::DeserializeOwned>(body: &Bytes) -> Result<T, BillingApiError> {
    if body.is_empty() {
        return Err(BillingApiError::BadRequest(
            "request body is required".to_string(),
        ));
    }
    serde_json::from_slice(body).map_err(|err| BillingApiError::BadRequest(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;
    use std::sync::Arc;

    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use axum::http::header::CONTENT_TYPE;
    use chrono::{DateTime, Utc};
    use kura_billing::{
        BoxFuture, QuotaDefinition, QuotaPeriod, Repository, UsageCounter, UsageEvent,
        UsageReservation,
    };
    use parking_lot::Mutex;
    use tower::ServiceExt;
    use uuid::Uuid;

    /// Result alias for the in-memory repository (the billing crate Result alias is crate-private).
    type Result<T> = std::result::Result<T, kura_billing::BillingError>;

    // -- fixture state ------------------------------------------------------

    #[derive(Default)]
    struct TestBillingState {
        plans: HashMap<String, TenantPlan>,
        overrides: HashMap<String, QuotaOverride>,
        counters: HashMap<String, UsageCounter>,
        reservations: HashMap<String, UsageReservation>,
        events: Vec<UsageEvent>,
        denials: Vec<QuotaDenial>,
        adjustments: Vec<ManualAdjustment>,
    }

    /// Minimal in-memory billing repository for the route tests (the kura-store
    /// billing DAOs are not ported; this mirrors the billing crate's own
    /// `FixtureRepo` shape).
    #[derive(Default)]
    struct TestBillingRepo {
        state: Mutex<TestBillingState>,
    }

    fn counter_key(tenant_id: &str, category: &Category, period_id: &str) -> String {
        format!("{tenant_id}:{category}:{period_id}")
    }

    impl TestBillingRepo {
        fn seed_plan(&self, tenant_id: &str, plan_key: &str, enforcement: &str) {
            self.state.lock().plans.insert(
                tenant_id.to_string(),
                TenantPlan {
                    plan_id: format!("plan_{tenant_id}"),
                    tenant_id: tenant_id.to_string(),
                    plan_key: plan_key.to_string(),
                    status: PlanStatus::from(PlanStatus::ACTIVE),
                    enforcement_mode: EnforcementMode::from(enforcement),
                    effective_at: Utc::now() - chrono::Duration::hours(1),
                    ..Default::default()
                },
            );
        }

        fn seed_denial(
            &self,
            tenant_id: &str,
            denial_id: &str,
            category: &Category,
            reason_code: &str,
        ) {
            self.state.lock().denials.push(QuotaDenial {
                denial_id: denial_id.to_string(),
                tenant_id: tenant_id.to_string(),
                category: category.clone(),
                quota_period_id: format!("period_{tenant_id}_{category}"),
                operation_key: format!("tenant:{tenant_id}:{category}:client_1"),
                reason_code: reason_code.to_string(),
                requested_amount: 1,
                remaining_amount: 0,
                guarded_entry_point: "POST /v1/runs".to_string(),
                created_at: Utc::now(),
            });
        }

        fn seed_adjustment(&self, tenant_id: &str, adjustment_id: &str, category: &Category) {
            self.state.lock().adjustments.push(ManualAdjustment {
                adjustment_id: adjustment_id.to_string(),
                tenant_id: tenant_id.to_string(),
                category: category.clone(),
                quota_period_id: format!("period_{tenant_id}_{category}"),
                amount_delta: -1,
                reason: "tenant scoped correction".to_string(),
                created_by_principal_id: "prn_admin".to_string(),
                created_at: Utc::now(),
            });
        }

        fn seed_counter(&self, tenant_id: &str, category: &Category, period_id: &str, committed: i64, reserved: i64) {
            self.state.lock().counters.insert(
                counter_key(tenant_id, category, period_id),
                UsageCounter {
                    usage_counter_id: format!("counter_{tenant_id}_{category}"),
                    tenant_id: tenant_id.to_string(),
                    category: category.clone(),
                    quota_period_id: period_id.to_string(),
                    committed_amount: committed,
                    reserved_amount: reserved,
                    updated_at: Utc::now(),
                    ..Default::default()
                },
            );
        }

        fn seed_reservation(&self, reservation: UsageReservation) {
            self.state.lock().reservations.insert(
                reservation.reservation_id.clone(),
                reservation,
            );
        }
    }

    impl Repository for TestBillingRepo {
        fn active_plan(&self, tenant_id: &str) -> BoxFuture<'_, Result<Option<TenantPlan>>> {
            let plan = self.state.lock().plans.get(tenant_id).cloned();
            Box::pin(async move { Ok(plan) })
        }

        fn quota_override(
            &self,
            tenant_id: &str,
            category: &Category,
            _at: DateTime<Utc>,
        ) -> BoxFuture<'_, Result<Option<QuotaOverride>>> {
            let override_ = self
                .state
                .lock()
                .overrides
                .get(&format!("{tenant_id}:{category}"))
                .cloned();
            Box::pin(async move { Ok(override_) })
        }

        fn open_period(
            &self,
            tenant_id: &str,
            definition: &QuotaDefinition,
            at: DateTime<Utc>,
        ) -> BoxFuture<'_, Result<QuotaPeriod>> {
            let (start, end) = kura_billing::period_for(&definition.period_kind, at);
            let period = QuotaPeriod {
                quota_period_id: format!("period_{tenant_id}_{}", definition.category),
                tenant_id: tenant_id.to_string(),
                category: definition.category.clone(),
                period_kind: definition.period_kind.clone(),
                period_start: start,
                period_end: end,
                status: "open".to_string(),
                ..Default::default()
            };
            Box::pin(async move { Ok(period) })
        }

        fn usage_counter(
            &self,
            tenant_id: &str,
            category: &Category,
            quota_period_id: &str,
        ) -> BoxFuture<'_, Result<Option<UsageCounter>>> {
            let counter = self
                .state
                .lock()
                .counters
                .get(&counter_key(tenant_id, category, quota_period_id))
                .cloned();
            Box::pin(async move { Ok(counter) })
        }

        fn save_usage_counter(&self, counter: UsageCounter) -> BoxFuture<'_, Result<()>> {
            self.state.lock().counters.insert(
                counter_key(&counter.tenant_id, &counter.category, &counter.quota_period_id),
                counter,
            );
            Box::pin(async { Ok(()) })
        }

        fn reservation_by_operation(
            &self,
            tenant_id: &str,
            category: &Category,
            operation_key: &str,
        ) -> BoxFuture<'_, Result<Option<UsageReservation>>> {
            let reservation = self
                .state
                .lock()
                .reservations
                .values()
                .find(|item| {
                    item.tenant_id == tenant_id
                        && item.category == *category
                        && item.operation_key == operation_key
                })
                .cloned();
            Box::pin(async move { Ok(reservation) })
        }

        fn reservation_by_id(
            &self,
            tenant_id: &str,
            reservation_id: &str,
        ) -> BoxFuture<'_, Result<Option<UsageReservation>>> {
            let reservation = self
                .state
                .lock()
                .reservations
                .values()
                .find(|item| item.tenant_id == tenant_id && item.reservation_id == reservation_id)
                .cloned();
            Box::pin(async move { Ok(reservation) })
        }

        fn save_reservation(&self, reservation: UsageReservation) -> BoxFuture<'_, Result<()>> {
            self.state
                .lock()
                .reservations
                .insert(reservation.reservation_id.clone(), reservation);
            Box::pin(async { Ok(()) })
        }

        fn append_usage_event(&self, event: UsageEvent) -> BoxFuture<'_, Result<()>> {
            self.state.lock().events.push(event);
            Box::pin(async { Ok(()) })
        }

        fn append_quota_denial(&self, denial: QuotaDenial) -> BoxFuture<'_, Result<()>> {
            self.state.lock().denials.push(denial);
            Box::pin(async { Ok(()) })
        }

        fn list_quota_denials(
            &self,
            tenant_id: &str,
            limit: usize,
        ) -> BoxFuture<'_, Result<Vec<QuotaDenial>>> {
            let denials: Vec<QuotaDenial> = self
                .state
                .lock()
                .denials
                .iter()
                .filter(|item| item.tenant_id == tenant_id)
                .take(limit)
                .cloned()
                .collect();
            Box::pin(async move { Ok(denials) })
        }

        fn list_manual_adjustments(
            &self,
            tenant_id: &str,
            limit: usize,
        ) -> BoxFuture<'_, Result<Vec<ManualAdjustment>>> {
            let adjustments: Vec<ManualAdjustment> = self
                .state
                .lock()
                .adjustments
                .iter()
                .filter(|item| item.tenant_id == tenant_id)
                .take(limit)
                .cloned()
                .collect();
            Box::pin(async move { Ok(adjustments) })
        }

        fn quota_denial_by_id(
            &self,
            tenant_id: &str,
            denial_id: &str,
        ) -> BoxFuture<'_, Result<Option<QuotaDenial>>> {
            let denial = self
                .state
                .lock()
                .denials
                .iter()
                .find(|item| item.tenant_id == tenant_id && item.denial_id == denial_id)
                .cloned();
            Box::pin(async move { Ok(denial) })
        }

        fn list_usage_evidence_refs(
            &self,
            tenant_id: &str,
            operation_key: &str,
            _limit: usize,
        ) -> BoxFuture<'_, Result<Vec<String>>> {
            let refs: Vec<String> = self
                .state
                .lock()
                .events
                .iter()
                .filter(|item| item.tenant_id == tenant_id && item.operation_key == operation_key)
                .map(|item| format!("usage_event:{}", item.usage_event_id))
                .collect();
            Box::pin(async move { Ok(refs) })
        }

        fn list_pending_reservations(&self) -> BoxFuture<'_, Result<Vec<UsageReservation>>> {
            let pending: Vec<UsageReservation> = self
                .state
                .lock()
                .reservations
                .values()
                .filter(|item| item.status == ReservationStatus::RESERVED)
                .cloned()
                .collect();
            Box::pin(async move { Ok(pending) })
        }

        fn save_plan(&self, plan: TenantPlan) -> BoxFuture<'_, Result<()>> {
            self.state.lock().plans.insert(plan.tenant_id.clone(), plan);
            Box::pin(async { Ok(()) })
        }

        fn save_quota_override(&self, override_: QuotaOverride) -> BoxFuture<'_, Result<()>> {
            self.state.lock().overrides.insert(
                format!("{}:{}", override_.tenant_id, override_.category),
                override_,
            );
            Box::pin(async { Ok(()) })
        }

        fn save_manual_adjustment(&self, adjustment: ManualAdjustment) -> BoxFuture<'_, Result<()>> {
            self.state.lock().adjustments.push(adjustment);
            Box::pin(async { Ok(()) })
        }
    }

    // -- harness -------------------------------------------------------------

    fn test_config() -> kura_config::Config {
        kura_config::Config {
            project_root: String::new(),
            environment: kura_config::Environment::Test,
            bind_addr: "127.0.0.1:19192".to_string(),
            data_dir: "/tmp/kura-api-billing".to_string(),
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
        let dir = std::env::temp_dir().join(format!("kura-api-billing-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let store = Arc::new(Mutex::new(
            kura_store::SQLiteStore::new(dir.to_str().expect("path")).expect("store"),
        ));
        AppState::new(test_config(), Arc::new(kura_events::Bus::new()), store)
    }

    fn billing_state(repo: Arc<TestBillingRepo>) -> AppState {
        let mut state = test_state();
        state.billing = Some(Arc::new(kura_billing::Manager::new(repo)));
        state
    }

    fn request(method: &str, uri: &str, body: Option<&str>) -> Request<Body> {
        let builder = Request::builder()
            .method(method)
            .uri(uri)
            .header(CONTENT_TYPE, "application/json");
        let req = match body {
            Some(payload) => builder.body(Body::from(payload.to_string())).expect("request"),
            None => builder.body(Body::empty()).expect("request"),
        };
        req
    }

    /// Builds a request with a resolved tenant context extension (the
    /// protected() middleware installs this once auth is wired; tests inject it
    /// directly, matching resources.rs).
    fn tenant_request(
        method: &str,
        uri: &str,
        body: Option<&str>,
        tenant_id: &str,
        permissions: Vec<Permission>,
    ) -> Request<Body> {
        let mut req = request(method, uri, body);
        req.extensions_mut().insert(TenantContext(kura_identity::TenantContext {
            tenant_id: tenant_id.to_string(),
            principal_id: format!("prn_{tenant_id}"),
            permissions,
            ..Default::default()
        }));
        req
    }

    fn owner(_tenant_id: &str) -> Vec<Permission> {
        vec![
            Permission::BillingView,
            Permission::BillingManage,
            Permission::BillingEvidenceExport,
        ]
    }

    /// Go RoleViewer: the viewer role has no billing permissions, so every
    /// billing.view-gated route answers the 403 tenant denial.
    fn viewer(_tenant_id: &str) -> Vec<Permission> {
        vec![]
    }

    async fn send(app: &axum::Router, req: Request<Body>) -> (StatusCode, serde_json::Value) {
        let response = app.clone().oneshot(req).await.expect("oneshot");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.expect("body");
        let json = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("json body")
        };
        (status, json)
    }

    fn app(state: AppState) -> axum::Router {
        router().with_state(state)
    }

    // -- ports of the Go handler tests ---------------------------------------

    /// Port of TestHostedBillingInspectionIsTenantScoped.
    #[tokio::test]
    async fn billing_inspection_is_tenant_scoped() {
        let repo = Arc::new(TestBillingRepo::default());
        repo.seed_plan("ten_r38_a", "finite", EnforcementMode::ENFORCED);
        repo.seed_plan("ten_r38_b", "finite", EnforcementMode::ENFORCED);
        let app = app(billing_state(repo));

        let (status, json) = send(&app, tenant_request("GET", "/v1/billing/usage", None, "ten_r38_a", owner("ten_r38_a"))).await;
        assert_eq!(status, StatusCode::OK, "usage should be 200: {json}");
        assert_eq!(json["tenantId"], "ten_r38_a");
        assert_ne!(json["tenantId"], "ten_r38_b");

        let (status, _) = send(&app, tenant_request("GET", "/v1/billing/plan", None, "ten_r38_a", viewer("ten_r38_a"))).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "viewer without billing.view should be 403");
    }

    /// Port of TestHostedBillingInspectionProjectsFiniteUnlimitedAndDevelopmentPlans.
    #[tokio::test]
    async fn billing_inspection_projects_finite_unlimited_and_development_plans() {
        let repo = Arc::new(TestBillingRepo::default());
        repo.seed_plan("ten_r38_finite", "finite", EnforcementMode::ENFORCED);
        repo.seed_plan("ten_r38_unlimited", "unlimited", EnforcementMode::UNLIMITED);
        let app = app(billing_state(repo));

        for (tenant_id, want_plan_key, want_enforcement) in [
            ("ten_r38_finite", "finite", "enforced"),
            ("ten_r38_unlimited", "unlimited", "unlimited"),
            // No persisted plan: the non-hosted (test) environment falls back
            // to the unlimited development plan.
            ("ten_r38_development", "development", "unlimited"),
        ] {
            let (status, json) = send(&app, tenant_request("GET", "/v1/billing/usage", None, tenant_id, owner(tenant_id))).await;
            assert_eq!(status, StatusCode::OK, "usage for {tenant_id} should be 200: {json}");
            assert_eq!(json["tenantId"], tenant_id);
            assert_eq!(json["planKey"], want_plan_key);
            assert_eq!(json["enforcementMode"], want_enforcement);
            let quotas = json["quotas"].as_array().expect("quotas array");
            assert_eq!(quotas.len(), kura_billing::required_categories().len());
        }
    }

    /// Port of TestHostedBillingInspectionListsOnlyCurrentTenantEvidence.
    #[tokio::test]
    async fn billing_inspection_lists_only_current_tenant_evidence() {
        let repo = Arc::new(TestBillingRepo::default());
        repo.seed_plan("ten_r38_evidence_a", "finite", EnforcementMode::ENFORCED);
        repo.seed_plan("ten_r38_evidence_b", "finite", EnforcementMode::ENFORCED);
        let category = Category::from(Category::RUN_LAUNCHES);
        repo.seed_denial("ten_r38_evidence_a", "denial_ten_r38_evidence_a", &category, "quota_denied:run_launches_exhausted");
        repo.seed_denial("ten_r38_evidence_b", "denial_ten_r38_evidence_b", &category, "quota_denied:run_launches_exhausted");
        repo.seed_adjustment("ten_r38_evidence_a", "adjustment_ten_r38_evidence_a", &category);
        repo.seed_adjustment("ten_r38_evidence_b", "adjustment_ten_r38_evidence_b", &category);
        let app = app(billing_state(repo));

        let (status, json) = send(&app, tenant_request("GET", "/v1/billing/usage", None, "ten_r38_evidence_a", owner("ten_r38_evidence_a"))).await;
        assert_eq!(status, StatusCode::OK);
        let body = serde_json::to_string(&json).expect("body string");
        assert!(body.contains("denial_ten_r38_evidence_a"));
        assert!(!body.contains("denial_ten_r38_evidence_b"));
        assert!(body.contains("adjustment_ten_r38_evidence_a"));
        assert!(!body.contains("adjustment_ten_r38_evidence_b"));

        let (status, json) = send(&app, tenant_request("GET", "/v1/billing/denials", None, "ten_r38_evidence_a", owner("ten_r38_evidence_a"))).await;
        assert_eq!(status, StatusCode::OK);
        let body = serde_json::to_string(&json).expect("body string");
        assert!(body.contains("denial_ten_r38_evidence_a"));
        assert!(!body.contains("denial_ten_r38_evidence_b"));
    }

    /// Port of TestHostedBillingPublicQuotaUXRoutesAreTenantScopedAndPermissionGated.
    #[tokio::test]
    async fn billing_public_quota_ux_routes_are_tenant_scoped_and_permission_gated() {
        let repo = Arc::new(TestBillingRepo::default());
        repo.seed_plan("ten_r47_a", "finite", EnforcementMode::ENFORCED);
        repo.seed_plan("ten_r47_b", "finite", EnforcementMode::ENFORCED);
        let category = Category::from(Category::RUN_LAUNCHES);
        repo.seed_denial("ten_r47_a", "denial_ten_r47_a", &category, "quota_denied:run_launches_exhausted");
        repo.seed_denial("ten_r47_b", "denial_ten_r47_b", &category, "quota_denied:run_launches_exhausted");
        let app = app(billing_state(repo));

        let (status, json) = send(&app, tenant_request("GET", "/v1/billing/quota-dashboard", None, "ten_r47_a", owner("ten_r47_a"))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["tenantId"], "ten_r47_a");
        assert!(json["sections"].as_array().map(|s| !s.is_empty()).unwrap_or(false));

        let (status, json) = send(&app, tenant_request("GET", "/v1/billing/denials/denial_ten_r47_a", None, "ten_r47_a", owner("ten_r47_a"))).await;
        assert_eq!(status, StatusCode::OK);
        assert!(!serde_json::to_string(&json).unwrap().contains("ten_r47_b"));

        let (status, _) = send(&app, tenant_request("GET", "/v1/billing/denials/denial_ten_r47_b", None, "ten_r47_a", owner("ten_r47_a"))).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "cross-tenant denial lookup must hide the record");

        // billing.view alone cannot export evidence.
        let (status, _) = send(&app, tenant_request("POST", "/v1/billing/denials/denial_ten_r47_a/evidence-export", None, "ten_r47_a", viewer("ten_r47_a"))).await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        let (status, json) = send(&app, tenant_request("POST", "/v1/billing/denials/denial_ten_r47_a/evidence-export", None, "ten_r47_a", owner("ten_r47_a"))).await;
        assert_eq!(status, StatusCode::OK);
        let body = serde_json::to_string(&json).unwrap();
        assert!(body.contains("\"redactions\""));
        assert!(!body.contains("ten_r47_b"));
    }

    /// Port of TestHostedBillingDenialDetailCoversGuardedCategoriesAndStableClassifications.
    #[tokio::test]
    async fn billing_denial_detail_covers_guarded_categories_and_stable_classifications() {
        let repo = Arc::new(TestBillingRepo::default());
        repo.seed_plan("ten_r47_detail", "finite", EnforcementMode::ENFORCED);
        let category = Category::from(Category::RUN_LAUNCHES);
        for (denial_id, reason) in [
            ("denial_run_launches", "quota_denied:run_launches_exhausted"),
            ("denial_unavailable", "quota_denied:quota_state_unavailable"),
            ("denial_operator", "quota_denied:operator_action_needed"),
            ("denial_abuse", "abuse_restriction:temporary"),
        ] {
            repo.seed_denial("ten_r47_detail", denial_id, &category, reason);
        }
        let app = app(billing_state(repo));

        let (status, json) = send(&app, tenant_request("GET", "/v1/billing/denials/denial_run_launches", None, "ten_r47_detail", owner("ten_r47_detail"))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["category"], "run_launches");
        assert_eq!(json["classification"], "quota_exhaustion");
        assert_ne!(json["operationRef"], json["operationKey"], "operation ref must be redacted");

        for (denial_id, want) in [
            ("denial_unavailable", "quota_state_unavailable"),
            ("denial_operator", "operator_action_needed"),
            ("denial_abuse", "abuse_restriction"),
        ] {
            let (status, json) = send(&app, tenant_request("GET", &format!("/v1/billing/denials/{denial_id}"), None, "ten_r47_detail", owner("ten_r47_detail"))).await;
            assert_eq!(status, StatusCode::OK, "{denial_id} detail should be 200");
            assert_eq!(json["classification"], want, "{denial_id}");
        }

        let (status, _) = send(&app, tenant_request("GET", "/v1/billing/denials/denial_run_launches", None, "ten_r47_detail", viewer("ten_r47_detail"))).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "unauthorized denial detail must be 403");
    }

    /// Port of TestHostedBillingAdminRequiresManagePermission.
    #[tokio::test]
    async fn billing_admin_requires_manage_permission() {
        let repo = Arc::new(TestBillingRepo::default());
        repo.seed_plan("ten_r38_admin", "development", EnforcementMode::UNLIMITED);
        let app = app(billing_state(repo));

        // Operator (no billing.manage) -> 403.
        let (status, _) = send(&app, tenant_request(
            "POST",
            "/v1/admin/billing/tenants/ten_r38_admin/plan",
            Some(r#"{"planKey":"finite","enforcementMode":"enforced","reason":"test"}"#),
            "ten_r38_admin",
            vec![],
        )).await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        // Admin -> 200 with the assigned plan.
        let (status, json) = send(&app, tenant_request(
            "POST",
            "/v1/admin/billing/tenants/ten_r38_admin/plan",
            Some(r#"{"planKey":"finite","enforcementMode":"enforced","reason":"test assignment"}"#),
            "ten_r38_admin",
            owner("ten_r38_admin"),
        )).await;
        assert_eq!(status, StatusCode::OK, "admin plan assignment should be 200: {json}");
        assert_eq!(json["planKey"], "finite");

        // Cross-tenant target -> 403.
        let (status, _) = send(&app, tenant_request(
            "POST",
            "/v1/admin/billing/tenants/ten_other/plan",
            Some(r#"{"planKey":"finite","enforcementMode":"enforced","reason":"test"}"#),
            "ten_r38_admin",
            owner("ten_r38_admin"),
        )).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    /// Port of TestHostedBillingAdminPlanAssignmentPersistsEvidence.
    #[tokio::test]
    async fn billing_admin_plan_assignment_persists_evidence() {
        let repo = Arc::new(TestBillingRepo::default());
        repo.seed_plan("ten_r38_plan_assignment", "development", EnforcementMode::UNLIMITED);
        let app = app(billing_state(repo.clone()));

        let (status, json) = send(&app, tenant_request(
            "POST",
            "/v1/admin/billing/tenants/ten_r38_plan_assignment/plan",
            Some(r#"{"planKey":"finite","enforcementMode":"enforced","reason":"customer upgraded"}"#),
            "ten_r38_plan_assignment",
            owner("ten_r38_plan_assignment"),
        )).await;
        assert_eq!(status, StatusCode::OK, "plan assignment should be 200: {json}");

        let plan = repo.state.lock().plans.get("ten_r38_plan_assignment").cloned().expect("plan persisted");
        assert_eq!(plan.plan_key, "finite");
        assert_eq!(plan.assignment_reason, "customer upgraded");
        assert!(!plan.assigned_by_principal_id.is_empty());
    }

    /// Port of TestHostedBillingAdminQuotaOverrideLoweredBelowUsageDeniesNewWork.
    #[tokio::test]
    async fn billing_admin_quota_override_lowered_below_usage_denies_new_work() {
        let repo = Arc::new(TestBillingRepo::default());
        repo.seed_plan("ten_r38_lowered_override", "finite", EnforcementMode::ENFORCED);
        let category = Category::from(Category::RUN_LAUNCHES);
        let period_id = format!("period_ten_r38_lowered_override_{category}");
        repo.seed_counter("ten_r38_lowered_override", &category, &period_id, 1, 1);
        let app = app(billing_state(repo.clone()));

        let (status, json) = send(&app, tenant_request(
            "POST",
            "/v1/admin/billing/tenants/ten_r38_lowered_override/quota-overrides",
            Some(r#"{"category":"run_launches","limit":1,"reason":"downgrade"}"#),
            "ten_r38_lowered_override",
            owner("ten_r38_lowered_override"),
        )).await;
        assert_eq!(status, StatusCode::OK, "quota override should be 200: {json}");
        assert_eq!(json["limit"], 1);

        // The manager (backed by the same repo) must now deny new work.
        let manager = kura_billing::Manager::new(repo);
        let result = manager
            .reserve(kura_billing::ReserveInput {
                tenant_id: "ten_r38_lowered_override".to_string(),
                category: category.clone(),
                amount: 1,
                operation_key: "tenant:ten_r38_lowered_override:run:after_lowering".to_string(),
                hosted: true,
                ..Default::default()
            })
            .await
            .expect("reserve");
        assert!(matches!(result.failure, Some(BillingError::QuotaDenied)), "{result:?}");
        assert!(result.denial.is_some());
        assert!(result.quota.as_ref().map(|q| q.over_limit).unwrap_or(false));
    }

    /// Port of TestHostedBillingAdminMutationRoutesDenyViewerAndOperator.
    #[tokio::test]
    async fn billing_admin_mutation_routes_deny_viewer_and_operator() {
        let repo = Arc::new(TestBillingRepo::default());
        repo.seed_plan("ten_r38_admin_denied", "finite", EnforcementMode::ENFORCED);
        let category = Category::from(Category::RUN_LAUNCHES);
        let period_id = format!("period_ten_r38_admin_denied_{category}");
        repo.seed_reservation(kura_billing::UsageReservation {
            reservation_id: "reservation_denied_admin_route".to_string(),
            tenant_id: "ten_r38_admin_denied".to_string(),
            category: category.clone(),
            quota_period_id: period_id.clone(),
            operation_key: "tenant:ten_r38_admin_denied:run:pending".to_string(),
            amount_reserved: 1,
            status: ReservationStatus::from(ReservationStatus::OPERATOR_ACTION_NEEDED),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            ..Default::default()
        });
        let app = app(billing_state(repo));

        let routes: &[(&str, &str, &str)] = &[
            ("plan", "/v1/admin/billing/tenants/ten_r38_admin_denied/plan", r#"{"planKey":"finite","enforcementMode":"enforced","reason":"test"}"#),
            ("override", "/v1/admin/billing/tenants/ten_r38_admin_denied/quota-overrides", r#"{"category":"run_launches","limit":1,"reason":"test"}"#),
            ("adjustment", "/v1/admin/billing/tenants/ten_r38_admin_denied/manual-adjustments", r#"{"category":"run_launches","quotaPeriodId":"period_ten_r38_admin_denied_run_launches","amountDelta":1,"reason":"test"}"#),
            ("resolve", "/v1/admin/billing/tenants/ten_r38_admin_denied/reservations/reservation_denied_admin_route/resolve", r#"{"outcome":"released","reason":"test"}"#),
        ];
        for denied_permissions in [vec![], vec![Permission::BillingView]] {
            for (name, path, body) in routes {
                let (status, _) = send(&app, tenant_request("POST", path, Some(body), "ten_r38_admin_denied", denied_permissions.clone())).await;
                assert_eq!(status, StatusCode::FORBIDDEN, "{name} should be 403 for the denied context");
            }
        }
    }

    /// Port of TestHostedBillingAdminResolvesOperatorActionReservation.
    #[tokio::test]
    async fn billing_admin_resolves_operator_action_reservation() {
        let repo = Arc::new(TestBillingRepo::default());
        repo.seed_plan("ten_r38_resolve", "finite", EnforcementMode::ENFORCED);
        let category = Category::from(Category::RUN_LAUNCHES);
        let period_id = format!("period_ten_r38_resolve_{category}");
        repo.seed_counter("ten_r38_resolve", &category, &period_id, 0, 1);
        repo.seed_reservation(kura_billing::UsageReservation {
            reservation_id: "reservation_resolve".to_string(),
            tenant_id: "ten_r38_resolve".to_string(),
            category: category.clone(),
            quota_period_id: period_id.clone(),            operation_key: "tenant:ten_r38_resolve:run:client_1".to_string(),
            amount_reserved: 1,
            status: ReservationStatus::from(ReservationStatus::OPERATOR_ACTION_NEEDED),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            ..Default::default()
        });
        let app = app(billing_state(repo.clone()));

        let (status, json) = send(&app, tenant_request(
            "POST",
            "/v1/admin/billing/tenants/ten_r38_resolve/reservations/reservation_resolve/resolve",
            Some(r#"{"outcome":"released","reason":"operator verified work never started"}"#),
            "ten_r38_resolve",
            owner("ten_r38_resolve"),
        )).await;
        assert_eq!(status, StatusCode::OK, "reservation resolution should be 200: {json}");
        assert_eq!(json["status"], "released");

        let reservation = repo.state.lock().reservations.get("reservation_resolve").cloned().expect("reservation");
        assert_eq!(reservation.status, ReservationStatus::RELEASED);
        let counter = repo.state.lock().counters.get(&counter_key("ten_r38_resolve", &category, &period_id)).cloned().expect("counter");
        assert_eq!(counter.reserved_amount, 0, "reserved amount must be released");
    }

    /// Go handleHostedBilling's nil-manager guard: 500 when unconfigured.
    #[tokio::test]
    async fn billing_manager_not_configured_returns_500() {
        let state = test_state();
        let app = app(state);
        let (status, json) = send(&app, tenant_request("GET", "/v1/billing/usage", None, "ten_a", owner("ten_a"))).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(json["error"], "billing manager is not configured");
    }

    /// Hosted (prod) tenants without a persisted plan fail closed with 503 (Go
    /// writeError(503) for the QuotaStateUnavailable projection failure).
    #[tokio::test]
    async fn billing_hosted_tenant_without_plan_fails_closed_503() {
        let mut state = test_state();
        state.config.environment = kura_config::Environment::Prod;
        state.billing = Some(Arc::new(kura_billing::Manager::new(Arc::new(
            TestBillingRepo::default(),
        ))));
        let app = app(state);
        let (status, json) = send(&app, tenant_request("GET", "/v1/billing/usage", None, "ten_hosted_no_plan", owner("ten_hosted_no_plan"))).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "hosted tenant without a plan fails closed: {json}");
    }

    /// Go handleHostedBilling's unknown path default: 404.
    #[tokio::test]
    async fn billing_unknown_path_returns_404() {
        let repo = Arc::new(TestBillingRepo::default());
        repo.seed_plan("ten_a", "finite", EnforcementMode::ENFORCED);
        let app = app(billing_state(repo));
        let (status, _) = send(&app, tenant_request("GET", "/v1/billing/nope", None, "ten_a", owner("ten_a"))).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
