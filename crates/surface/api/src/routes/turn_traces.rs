//! Per-turn traces (agent-runtime-program.md 2.1):
//!
//! - `GET /v1/turn-traces/{turn_id}` — one turn's span tree.
//! - `GET /v1/turn-traces?requestId=&limit=` — recent turns, newest first,
//!   optionally only those serving one request (`x-request-id`). This is how
//!   a turn that failed before it could answer, and so returned no `turnId`,
//!   is still found.
//!
//! Traces live in the chat service's in-process store and are bounded by turn
//! count, so an old turn is a 404. A tenant reads only its own turns, and
//! another tenant's turn is a 404, not a 403, so ids cannot be probed. The
//! shared-mode platform operator reads every turn, the daemon's own
//! (tenantless) work included; see [`Visible`] for single mode.

use axum::Router;
use axum::extract::{Extension, Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use kura_telemetry::trace::TurnTrace;
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::hosting::Hosting;
use crate::middleware::TenantContext;
use crate::response::Json;
use crate::state::AppState;

const DEFAULT_LIMIT: usize = 20;
const MAX_LIMIT: usize = 100;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/turn-traces", get(list_turn_traces))
        .route("/v1/turn-traces/{turn_id}", get(get_turn_trace))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListQuery {
    #[serde(default)]
    request_id: String,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Serialize)]
struct TurnTraceList {
    items: Vec<TurnTrace>,
}

/// Which turns the caller may read.
enum Visible {
    /// The platform operator: every turn, the daemon's own included.
    All,
    /// A tenant: its own turns only.
    Tenant(String),
    /// No tenant and not the operator. The route policy refuses this in
    /// shared mode; it is handled here too rather than matching the
    /// daemon's own (tenantless) turns.
    Nothing,
}

impl Visible {
    fn of(state: &AppState, tenant: Option<&Extension<TenantContext>>) -> Self {
        let context = tenant.map(|extension| &extension.0.0);
        // Not `is_platform_operator` alone: in single mode that is true for
        // every caller and leaves the guard to the handler. A single-owner
        // daemon can still hold other tenants' members, so there a caller
        // with a tenant reads that tenant's turns, and only a caller with no
        // tenant at all (auth off) reads everything.
        let everything = match &state.hosting {
            Hosting::Shared { .. } => state.hosting.is_platform_operator(context),
            Hosting::Single => context.is_none(),
        };
        if everything {
            return Self::All;
        }
        match context.map(|context| context.tenant_id.trim()) {
            Some(tenant_id) if !tenant_id.is_empty() => Self::Tenant(tenant_id.to_string()),
            _ => Self::Nothing,
        }
    }

    fn may_read(&self, trace: &TurnTrace) -> bool {
        match self {
            Self::All => true,
            Self::Tenant(tenant_id) => trace.tenant_id == *tenant_id,
            Self::Nothing => false,
        }
    }
}

#[allow(clippy::unused_async)]
async fn get_turn_trace(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Path(turn_id): Path<String>,
) -> Response {
    let Some(chat) = state.chat.as_deref() else {
        return ApiError::Internal("chat service is not configured".to_string()).into_response();
    };
    let visible = Visible::of(&state, tenant.as_ref());
    match chat.traces().get(turn_id.trim()) {
        Some(trace) if visible.may_read(&trace) => Json(trace).into_response(),
        _ => ApiError::NotFound("turn trace not found".to_string()).into_response(),
    }
}

#[allow(clippy::unused_async)]
async fn list_turn_traces(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Query(query): Query<ListQuery>,
) -> Response {
    let Some(chat) = state.chat.as_deref() else {
        return ApiError::Internal("chat service is not configured".to_string()).into_response();
    };
    let visible = Visible::of(&state, tenant.as_ref());
    let request_id = query.request_id.trim().to_string();
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let items = chat.traces().list(limit, |trace| {
        visible.may_read(trace) && (request_id.is_empty() || trace.request_id == request_id)
    });
    Json(TurnTraceList { items }).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::tests_support::test_state;
    use kura_identity::Role;

    fn caller(tenant_id: &str, role: Role) -> Extension<TenantContext> {
        Extension(TenantContext(kura_identity::TenantContext {
            tenant_id: tenant_id.to_string(),
            role: Some(role),
            ..kura_identity::TenantContext::default()
        }))
    }

    fn turn(tenant_id: &str) -> TurnTrace {
        let store = kura_telemetry::trace::TraceStore::new(1);
        let recorder = store.start(kura_telemetry::trace::TurnStart {
            tenant_id: tenant_id.to_string(),
            ..Default::default()
        });
        store.get(&recorder.turn_id()).expect("trace")
    }

    fn reads(state: &AppState, who: Option<&Extension<TenantContext>>, owner: &str) -> bool {
        Visible::of(state, who).may_read(&turn(owner))
    }

    #[test]
    fn a_tenant_reads_only_its_own_turns_in_either_mode() {
        let single = test_state();
        let mut shared = test_state();
        shared.hosting = Hosting::Shared {
            operator_tenant_id: "ten_local".to_string(),
        };
        // An Owner of some other tenant is still only that tenant: single mode
        // can hold more than the operator's tenant.
        for state in [&single, &shared] {
            let alice = caller("ten_alice", Role::Owner);
            assert!(reads(state, Some(&alice), "ten_alice"));
            assert!(!reads(state, Some(&alice), "ten_bob"));
            assert!(!reads(state, Some(&alice), ""), "the daemon's own work");
        }
    }

    #[test]
    fn the_shared_operator_and_a_tenantless_local_caller_read_everything() {
        let mut shared = test_state();
        shared.hosting = Hosting::Shared {
            operator_tenant_id: "ten_local".to_string(),
        };
        let operator = caller("ten_local", Role::Owner);
        assert!(reads(&shared, Some(&operator), "ten_bob"));
        assert!(reads(&shared, Some(&operator), ""));
        // A member of the operator tenant who is not its Owner is not the
        // operator.
        let member = caller("ten_local", Role::Admin);
        assert!(!reads(&shared, Some(&member), "ten_bob"));
        // No tenant on a shared daemon reads nothing.
        assert!(!reads(&shared, None, ""));
        assert!(!reads(&shared, None, "ten_bob"));

        assert!(reads(&test_state(), None, "ten_bob"));
    }
}
