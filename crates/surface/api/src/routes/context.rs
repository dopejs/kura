//! `/v1/context/assemblies` — what memory the model actually saw.
//!
//! The context plugin records every pre-dispatch assembly as a
//! `context.assembled` event: what was injected, what was excluded and why,
//! and the budgets. Reading that back through the generic event API means
//! knowing the category, the event name and the payload shape; during an
//! incident that is three things to remember. This route is the one call.
//!
//! **Correlation boundary, stated because it is structural rather than an
//! oversight:** an assembly cannot carry a dispatch id. The hook runs at
//! `chat/pre-dispatch`, *before* the dispatch record is prepared — that
//! ordering is the "model-visible = logged" invariant, so the id does not
//! exist yet. Assemblies are therefore correlated by tenant and thread, in
//! time order, and each carries its own `assemblyId`.

use axum::Router;
use axum::extract::{Extension, Query, State};
use axum::routing::get;
use serde::Serialize;

use crate::error::ApiError;
use crate::middleware::TenantContext;
use crate::response::Json;
use crate::state::AppState;

const DEFAULT_LIMIT: usize = 20;
const MAX_LIMIT: usize = 200;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextAssembly {
    /// The recording event's resource id; addressable and stable.
    pub assembly_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub thread_id: String,
    pub recorded_at: chrono::DateTime<chrono::Utc>,
    /// The `AssemblyRecord` verbatim: inclusions with their layer and size,
    /// exclusions with their reason.
    pub record: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextAssemblyListResponse {
    pub items: Vec<ContextAssembly>,
}

/// GET /v1/context/assemblies
///
/// Newest first. Filters: `tenantId` (defaults to the acting tenant),
/// `threadId`, `limit`.
async fn list_assemblies(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<ContextAssemblyListResponse>, ApiError> {
    let acting_tenant = tenant
        .as_ref()
        .map(|e| e.0.0.tenant_id.trim().to_string())
        .filter(|id| !id.is_empty());
    // A resolved tenant wins over the query parameter: a caller must not be
    // able to read another tenant's assemblies by naming it.
    let tenant_filter = acting_tenant
        .clone()
        .unwrap_or_else(|| params.get("tenantId").cloned().unwrap_or_default());
    let thread_filter = params
        .get("threadId")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let limit = params
        .get("limit")
        .and_then(|raw| raw.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_LIMIT)
        .min(MAX_LIMIT);

    let events = state
        .store_pool
        .read()
        .list_events(&kura_events::Filter {
            category: "context".to_string(),
            ..kura_events::Filter::default()
        })
        .map_err(ApiError::from_store)?;

    let mut items: Vec<ContextAssembly> = events
        .into_iter()
        .filter(|event| event.name == "context.assembled")
        .filter_map(|event| {
            let payload_tenant = event
                .payload
                .get("tenantId")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            if !tenant_filter.is_empty() && payload_tenant != tenant_filter {
                return None;
            }
            let payload_thread = event
                .payload
                .get("threadId")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            if !thread_filter.is_empty() && payload_thread != thread_filter {
                return None;
            }
            Some(ContextAssembly {
                assembly_id: event.resource.id.clone(),
                tenant_id: payload_tenant,
                thread_id: payload_thread,
                recorded_at: event.occurred_at,
                record: event
                    .payload
                    .get("record")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            })
        })
        .collect();

    // Newest first: during an incident the question is almost always "what
    // just happened", not "what happened first".
    items.reverse();
    items.truncate(limit);
    Ok(Json(ContextAssemblyListResponse { items }))
}

pub fn router() -> Router<AppState> {
    Router::new().route("/v1/context/assemblies", get(list_assemblies))
}

#[cfg(test)]
mod tests {
    use super::super::tests_support::{request_json, request_json_as_tenant, test_state};
    use axum::http::StatusCode;

    fn record_assembly(
        state: &crate::state::AppState,
        tenant_id: &str,
        thread_id: &str,
        included_asset: &str,
    ) -> String {
        let mut payload = serde_json::Map::new();
        payload.insert(
            "record".to_string(),
            serde_json::json!({
                "included": [{ "assetId": included_asset, "layer": "l1", "chars": 12, "source": "bootstrap" }],
                "excluded": [],
                "budgetChars": 4000,
                "usedChars": 12
            }),
        );
        payload.insert(
            "tenantId".to_string(),
            serde_json::Value::String(tenant_id.to_string()),
        );
        if !thread_id.is_empty() {
            payload.insert(
                "threadId".to_string(),
                serde_json::Value::String(thread_id.to_string()),
            );
        }
        let event = kura_events::Event {
            category: "context".to_string(),
            name: "context.assembled".to_string(),
            resource: kura_events::Resource {
                kind: "context_assembly".to_string(),
                id: uuid::Uuid::now_v7().to_string(),
            },
            payload,
            ..kura_events::Event::default()
        };
        let appended = state.store.lock().append_event(&event).expect("append");
        appended.resource.id
    }

    /// The one-call answer to "what memory did the model see": newest first,
    /// filterable by thread, with the record verbatim.
    #[tokio::test]
    async fn lists_assemblies_newest_first_and_filters_by_thread() {
        let state = test_state();
        let first = record_assembly(&state, "", "thr_a", "mem_1");
        let second = record_assembly(&state, "", "thr_b", "mem_2");
        let third = record_assembly(&state, "", "thr_a", "mem_3");

        let (status, body) =
            request_json(state.clone(), "GET", "/v1/context/assemblies", None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let ids: Vec<&str> = body["items"]
            .as_array()
            .expect("items")
            .iter()
            .filter_map(|i| i["assemblyId"].as_str())
            .collect();
        assert_eq!(ids, [third.as_str(), second.as_str(), first.as_str()]);
        assert_eq!(
            body["items"][0]["record"]["included"][0]["assetId"],
            "mem_3"
        );

        let (status, body) = request_json(
            state.clone(),
            "GET",
            "/v1/context/assemblies?threadId=thr_a",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let ids: Vec<&str> = body["items"]
            .as_array()
            .expect("items")
            .iter()
            .filter_map(|i| i["assemblyId"].as_str())
            .collect();
        assert_eq!(ids, [third.as_str(), first.as_str()]);
    }

    /// A resolved tenant wins over the query parameter: naming another tenant
    /// must not read its assemblies.
    #[tokio::test]
    async fn a_tenant_cannot_read_another_tenants_assemblies() {
        let state = test_state();
        record_assembly(&state, "tnt_a", "thr_1", "mem_a");
        record_assembly(&state, "tnt_b", "thr_2", "mem_b");

        let (status, body) = request_json_as_tenant(
            state.clone(),
            "tnt_a",
            "GET",
            "/v1/context/assemblies",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let items = body["items"].as_array().expect("items");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["tenantId"], "tnt_a");

        // Naming tnt_b in the query is ignored for a resolved tnt_a caller.
        let (status, body) = request_json_as_tenant(
            state.clone(),
            "tnt_a",
            "GET",
            "/v1/context/assemblies?tenantId=tnt_b",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let items = body["items"].as_array().expect("items");
        assert!(items.iter().all(|i| i["tenantId"] == "tnt_a"), "{body}");
    }
}
