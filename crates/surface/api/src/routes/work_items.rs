//! `/v1/work-items` — long-lived work (docs/harness/work-items.md, slice 1).
//!
//! - `POST /v1/work-items` — opens an item for the caller's tenant on an
//!   explicit `api` mandate and starts its first round (201, stage
//!   `working`). The round runs after the response, as an ordinary
//!   non-streamed chat turn: request id `<item>_r<seq>`, the item's own
//!   thread, the report schema as its response format. Its turn is admitted
//!   before anything is recorded, so a refusal is a 429 and opens nothing.
//! - `GET /v1/work-items` — the tenant's items (`?state=open|ended`,
//!   `?limit=`), stage included.
//! - `GET /v1/work-items/{itemId}` — one item with its rounds and inputs.
//!
//! Execution lives here, as for swarm runs: `kura-workitems` records what
//! a round is and how it ended, the chat service runs it.
//!
//! **Tenancy.** Every read and write is for the acting tenant (the default
//! tenant without one); another tenant's item answers 404. **Off by
//! default:** without `entries.work-items.config.enabled` every route
//! answers 404, as before the feature existed.

use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use kura_chat::{Admitted, CancellationToken, ChatError, QueryInput};
use kura_workitems::{
    ItemState, Manager, OpenRequest, RoundOutcome, RoundTurn, WorkItem, WorkItemError,
};

use crate::error::ApiError;
use crate::middleware::{ActingTenant, RequestContext};
use crate::response::Json;
use crate::state::AppState;

/// Items a list returns when the caller does not say.
const DEFAULT_LIST_LIMIT: usize = 50;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/work-items", get(list_items).post(create_item))
        .route("/v1/work-items/{item_id}", get(get_item))
}

/// Body of `POST /v1/work-items` (`create-work-item.request.schema.json`).
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateWorkItemRequest {
    title: String,
    scope: String,
    #[serde(default)]
    provider: String,
    #[serde(default)]
    model: String,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct ListQuery {
    state: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, serde::Serialize)]
struct WorkItemListResponse {
    items: Vec<WorkItem>,
}

fn manager(state: &AppState) -> Result<Arc<Manager>, ApiError> {
    state
        .work_items
        .clone()
        .ok_or_else(|| ApiError::NotFound("work items are not enabled".to_string()))
}

/// The acting tenant, or the default tenant when the request has none
/// (single-owner daemon without auth), as reminders do. Never empty.
fn tenant_id(state: &AppState, tenant: &ActingTenant) -> Result<String, ApiError> {
    match tenant.id() {
        "" => state
            .store_pool
            .read()
            .resolve_default_tenant_binding()
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| ApiError::BadRequest("a work item needs a tenant".to_string())),
        acting => Ok(acting.to_string()),
    }
}

fn api_error(err: WorkItemError) -> ApiError {
    match err {
        WorkItemError::Invalid(message) => ApiError::BadRequest(message),
        WorkItemError::NotFound => ApiError::NotFound("work item not found".to_string()),
        err @ (WorkItemError::OpenLimit { .. } | WorkItemError::RoundNotRunning(_)) => {
            ApiError::Conflict(err.to_string())
        }
        WorkItemError::Store(message) => ApiError::Internal(message),
    }
}

async fn create_item(
    State(state): State<AppState>,
    tenant: ActingTenant,
    correlation: Option<Extension<RequestContext>>,
    body: Bytes,
) -> Response {
    match create(state, tenant, correlation, body).await {
        Ok(item) => (StatusCode::CREATED, Json(item)).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn create(
    state: AppState,
    tenant: ActingTenant,
    correlation: Option<Extension<RequestContext>>,
    body: Bytes,
) -> Result<WorkItem, ApiError> {
    let manager = manager(&state)?;
    let chat = state
        .chat
        .clone()
        .ok_or_else(|| ApiError::Internal("chat service is not configured".to_string()))?;
    let request: CreateWorkItemRequest = super::decode_json_required(&body)?;
    let tenant_id = tenant_id(&state, &tenant)?;
    let open = OpenRequest {
        tenant_id: tenant_id.clone(),
        principal_id: tenant
            .context()
            .map(|tc| tc.0.principal_id.trim().to_string())
            .unwrap_or_default(),
        request_ref: correlation
            .map(|extension| extension.0.0.lock().request_id.clone())
            .unwrap_or_default(),
        title: request.title,
        scope: request.scope,
        provider: request.provider,
        model: request.model,
    };
    // Refused before the turn is admitted: a bad request costs no turn.
    kura_workitems::check_open(&open).map_err(api_error)?;
    // Admitted before anything is recorded, so a throttled request opens
    // nothing. Admission may wait for a slot, so it runs off the worker.
    let admitted = {
        let chat = chat.clone();
        tokio::task::spawn_blocking(move || chat.admit(&tenant_id))
            .await
            .map_err(|err| ApiError::Internal(format!("work item admission task failed: {err}")))?
            .map_err(|err| admission_error(&err))?
    };
    let (item, turn) = {
        let manager = manager.clone();
        tokio::task::spawn_blocking(move || manager.open(open))
            .await
            .map_err(|err| ApiError::Internal(format!("work item open task failed: {err}")))?
            .map_err(api_error)?
    };
    std::thread::Builder::new()
        .name("kura-work-item-round".to_string())
        .spawn(move || run_round(&chat, &manager, &turn, admitted))
        .map_err(|err| ApiError::Internal(format!("spawn work item round: {err}")))?;
    Ok(item)
}

fn admission_error(err: &ChatError) -> ApiError {
    match err {
        ChatError::Throttled(throttled) => ApiError::TooManyRequests {
            code: throttled.reason.code(),
            message: err.to_string(),
            retry_after_secs: throttled.retry_after.as_secs_f64().ceil() as u64,
        },
        other => ApiError::Internal(other.to_string()),
    }
}

/// Runs one round as a chat turn holding `admitted`, then records how it
/// ended. Blocking: the caller gives it a thread. A panic in the turn is
/// recorded as a failed round rather than leaving it running.
pub fn run_round(
    chat: &kura_chat::Service,
    manager: &Manager,
    turn: &RoundTurn,
    admitted: Admitted,
) {
    let input = QueryInput {
        query: turn.query_text.clone(),
        provider: turn.provider.clone(),
        model: turn.model.clone(),
        tenant_id: turn.tenant_id.clone(),
        thread_id: turn.thread_id.clone(),
        request_id: turn.request_id.clone(),
        response_format: Some(kura_llm::ResponseFormat {
            name: kura_workitems::REPORT_FORMAT_NAME.to_string(),
            schema: kura_workitems::report_schema(),
        }),
        ..QueryInput::default()
    };
    let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        chat.query_admitted(admitted, input, &CancellationToken::new())
    }));
    let outcome = match ran {
        Ok(Ok(execution)) => {
            let turn_id = Some(execution.result.turn_id.clone()).filter(|id| !id.is_empty());
            match execution.exec_error {
                None => RoundOutcome::Answered {
                    structured: execution.result.structured_output,
                    turn_id,
                },
                Some(err) => {
                    log_turn_failure(turn, &err.to_string());
                    RoundOutcome::Failed {
                        error_code: Some(execution.result.dispatch.error_code.clone())
                            .filter(|code| !code.is_empty())
                            .unwrap_or_else(|| err.code().to_string()),
                        turn_id,
                    }
                }
            }
        }
        Ok(Err(ChatError::StructuredOutputInvalid(detail))) => RoundOutcome::Unreadable {
            detail,
            turn_id: None,
        },
        Ok(Err(err)) => {
            log_turn_failure(turn, &err.to_string());
            RoundOutcome::Failed {
                error_code: err.code().to_string(),
                turn_id: None,
            }
        }
        Err(_) => {
            log_turn_failure(turn, "the turn panicked");
            RoundOutcome::Failed {
                error_code: "panic".to_string(),
                turn_id: None,
            }
        }
    };
    if let Err(err) = manager.settle_round(turn, outcome) {
        eprintln!(
            "[kura] work item: tenant={} item={} settling round {} failed: {err}",
            turn.tenant_id, turn.item_id, turn.seq
        );
    }
}

/// The round's record keeps only the error code; the message goes to the
/// log, where an operator looks for why.
fn log_turn_failure(turn: &RoundTurn, message: &str) {
    eprintln!(
        "[kura] work item: tenant={} item={} round {} ended without a report: {message}",
        turn.tenant_id, turn.item_id, turn.seq
    );
}

async fn list_items(
    State(state): State<AppState>,
    tenant: ActingTenant,
    Query(query): Query<ListQuery>,
) -> Result<Json<WorkItemListResponse>, ApiError> {
    let manager = manager(&state)?;
    let tenant_id = tenant_id(&state, &tenant)?;
    let filter = match query.state.as_deref().map(str::trim) {
        None | Some("") => None,
        Some("open") => Some(ItemState::Open),
        Some("ended") => Some(ItemState::Ended),
        Some(other) => {
            return Err(ApiError::BadRequest(format!(
                "state must be open or ended, not {other:?}"
            )));
        }
    };
    let limit = query.limit.unwrap_or(DEFAULT_LIST_LIMIT);
    let items = tokio::task::spawn_blocking(move || manager.list(&tenant_id, filter, limit))
        .await
        .map_err(|err| ApiError::Internal(format!("work item list task failed: {err}")))?
        .map_err(api_error)?;
    Ok(Json(WorkItemListResponse { items }))
}

async fn get_item(
    State(state): State<AppState>,
    tenant: ActingTenant,
    Path(item_id): Path<String>,
) -> Result<Json<WorkItem>, ApiError> {
    let manager = manager(&state)?;
    let tenant_id = tenant_id(&state, &tenant)?;
    let item = tokio::task::spawn_blocking(move || manager.get(&tenant_id, &item_id))
        .await
        .map_err(|err| ApiError::Internal(format!("work item read task failed: {err}")))?
        .map_err(api_error)?;
    Ok(Json(item))
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::{Body, to_bytes};
    use axum::http::Request as HttpRequest;
    use kura_events::Bus;
    use kura_llm::Dispatcher;
    use tower::ServiceExt;

    use crate::middleware::TenantContext;

    /// Answers every request with the same text.
    struct Fixed(&'static str);

    impl kura_llm::Provider for Fixed {
        fn name(&self) -> &str {
            "fixed"
        }

        fn complete<'a>(
            &'a self,
            _request: kura_llm::ProviderRequest,
        ) -> futures::future::BoxFuture<
            'a,
            Result<kura_llm::ProviderResponse, kura_llm::ProviderError>,
        > {
            let output = self.0.to_string();
            Box::pin(async move {
                Ok(kura_llm::ProviderResponse {
                    output,
                    finish_reason: "stop".to_string(),
                    ..Default::default()
                })
            })
        }

        fn stream<'a>(
            &'a self,
            request: kura_llm::ProviderRequest,
            _emit: kura_llm::StreamEmitter<'a>,
        ) -> futures::future::BoxFuture<
            'a,
            Result<kura_llm::ProviderResponse, kura_llm::ProviderError>,
        > {
            self.complete(request)
        }
    }

    /// A state with the feature on and a chat service whose `fixed`
    /// provider answers `reply`, optionally behind turn limits.
    fn state(reply: &'static str, limits: Option<kura_chat::TurnLimits>) -> AppState {
        let dispatcher = Dispatcher::new();
        dispatcher.register_provider(Arc::new(Fixed(reply)));
        let mut chat = kura_chat::Service::new_service(
            Arc::new(dispatcher),
            None,
            None,
            Some(Bus::new()),
            None,
        );
        if let Some(limits) = limits {
            chat.set_turn_limits(limits);
        }
        let mut state = crate::routes::tests_support::test_state();
        let pool = state.store_pool.clone();
        state.chat = Some(Arc::new(chat));
        state.work_items = Some(Arc::new(Manager::new(
            pool,
            state.event_bus.clone(),
            kura_workitems::WorkItemsConfig {
                enabled: true,
                ..Default::default()
            },
        )));
        state
    }

    async fn send(
        state: &AppState,
        tenant: &str,
        method: &str,
        uri: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        let mut request = HttpRequest::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
            .expect("request");
        let tc = kura_identity::TenantContext {
            tenant_id: tenant.to_string(),
            principal_id: format!("prn_{tenant}"),
            ..Default::default()
        };
        request.extensions_mut().insert(TenantContext(tc.clone()));
        let response = kura_identity::tenantctx::scope(
            tc,
            crate::routes::router(state.clone()).oneshot(request),
        )
        .await
        .expect("oneshot");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        (status, serde_json::from_slice(&bytes).unwrap_or_default())
    }

    fn open_body() -> serde_json::Value {
        serde_json::json!({
            "title": "Fix CI",
            "scope": "Find why main fails and fix it.",
            "provider": "fixed",
            "model": "fixed-model",
        })
    }

    /// Polls the item until its first round is no longer running.
    async fn settled(state: &AppState, tenant: &str, item_id: &str) -> serde_json::Value {
        for _ in 0..200 {
            let (status, item) = send(
                state,
                tenant,
                "GET",
                &format!("/v1/work-items/{item_id}"),
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{item}");
            if item["rounds"][0]["state"] != "running" {
                return item;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        panic!("the round of {item_id} never settled");
    }

    #[tokio::test]
    async fn every_route_answers_404_while_the_feature_is_off() {
        let mut state = state("{}", None);
        state.work_items = None;
        for (method, uri, body) in [
            ("POST", "/v1/work-items", Some(open_body())),
            ("GET", "/v1/work-items", None),
            ("GET", "/v1/work-items/wi_1", None),
        ] {
            let (status, json) = send(&state, "ten_a", method, uri, body).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}");
            assert_eq!(json["message"], "work items are not enabled");
        }
    }

    #[tokio::test]
    async fn an_opened_item_runs_one_round_and_records_its_report() {
        let state = state(
            r#"{"status":"waiting","summary":"Waiting for CI.","attention":{"question":"Merge when green?"}}"#,
            None,
        );
        let (status, item) =
            send(&state, "ten_a", "POST", "/v1/work-items", Some(open_body())).await;
        assert_eq!(status, StatusCode::CREATED, "{item}");
        assert_eq!(item["state"], "open");
        assert_eq!(item["stage"], "working");
        assert_eq!(item["mandate"]["basis"], "api");
        assert_eq!(item["mandate"]["sender"]["principalId"], "prn_ten_a");
        let item_id = item["itemId"].as_str().expect("id").to_string();

        let item = settled(&state, "ten_a", &item_id).await;
        assert_eq!(item["stage"], "person/answer", "{item}");
        assert_eq!(item["lastReport"]["status"], "waiting");
        assert_eq!(item["attention"]["question"], "Merge when green?");
        let round = &item["rounds"][0];
        assert_eq!(round["state"], "settled");
        assert_eq!(round["outcome"], "waiting");
        assert_eq!(round["requestId"], format!("{item_id}_r1"));
        assert!(round["turnId"].as_str().is_some_and(|id| !id.is_empty()));
    }

    #[tokio::test]
    async fn a_reply_that_is_not_a_report_is_recorded_as_report_unreadable() {
        let state = state("I fixed it, all good.", None);
        let (status, item) =
            send(&state, "ten_a", "POST", "/v1/work-items", Some(open_body())).await;
        assert_eq!(status, StatusCode::CREATED, "{item}");
        let item = settled(&state, "ten_a", item["itemId"].as_str().unwrap()).await;
        assert_eq!(item["stage"], "person/blocked", "{item}");
        assert_eq!(item["lastReport"]["status"], "blocked");
        assert_eq!(item["lastReport"]["reason"], "report_unreadable");
        assert_eq!(item["rounds"][0]["outcome"], "report_unreadable");
    }

    #[tokio::test]
    async fn another_tenants_item_is_not_found_and_not_listed() {
        let state = state(r#"{"status":"ready","summary":"Done."}"#, None);
        let (_, mine) = send(&state, "ten_a", "POST", "/v1/work-items", Some(open_body())).await;
        let (_, theirs) = send(&state, "ten_b", "POST", "/v1/work-items", Some(open_body())).await;
        let mine = mine["itemId"].as_str().unwrap().to_string();
        let theirs = theirs["itemId"].as_str().unwrap().to_string();
        settled(&state, "ten_a", &mine).await;
        settled(&state, "ten_b", &theirs).await;

        let (status, json) = send(
            &state,
            "ten_b",
            "GET",
            &format!("/v1/work-items/{mine}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
        for (tenant, own) in [("ten_a", &mine), ("ten_b", &theirs)] {
            let (status, list) = send(&state, tenant, "GET", "/v1/work-items", None).await;
            assert_eq!(status, StatusCode::OK);
            let ids: Vec<&str> = list["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item["itemId"].as_str().unwrap())
                .collect();
            assert_eq!(ids, [own.as_str()], "{tenant}");
        }
        // Ended items have no stage and are filtered by state.
        let (_, ended) = send(&state, "ten_a", "GET", "/v1/work-items?state=ended", None).await;
        assert_eq!(ended["items"][0]["endReason"], "completed");
        assert!(ended["items"][0].get("stage").is_none());
        let (_, open) = send(&state, "ten_a", "GET", "/v1/work-items?state=open", None).await;
        assert_eq!(open["items"], serde_json::json!([]));
        let (status, _) = send(&state, "ten_a", "GET", "/v1/work-items?state=done", None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn a_bad_request_is_refused_and_opens_nothing() {
        let state = state("{}", None);
        for body in [
            serde_json::json!({"title": "t"}),
            serde_json::json!({"title": " ", "scope": "s"}),
            serde_json::json!({"title": "t", "scope": "s", "scopee": "typo"}),
        ] {
            let (status, json) = send(
                &state,
                "ten_a",
                "POST",
                "/v1/work-items",
                Some(body.clone()),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body} -> {json}");
        }
        let (_, list) = send(&state, "ten_a", "GET", "/v1/work-items", None).await;
        assert_eq!(list["items"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn a_throttled_turn_answers_429_and_opens_nothing() {
        let state = state(
            r#"{"status":"waiting","summary":"Waiting."}"#,
            Some(kura_chat::TurnLimits {
                global_concurrency: 8,
                tenant_concurrency: 8,
                turns_per_minute: 1,
                tokens_per_minute: 0,
                tenant_queue_depth: 8,
                global_queue_depth: 8,
                queue_timeout: std::time::Duration::from_secs(5),
                exempt_tenants: Vec::new(),
            }),
        );
        let (status, _) = send(&state, "ten_a", "POST", "/v1/work-items", Some(open_body())).await;
        assert_eq!(status, StatusCode::CREATED);
        let (status, json) =
            send(&state, "ten_a", "POST", "/v1/work-items", Some(open_body())).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{json}");
        assert_eq!(json["code"], "turn_rate_limited");
        let (_, list) = send(&state, "ten_a", "GET", "/v1/work-items", None).await;
        assert_eq!(list["items"].as_array().unwrap().len(), 1);
    }
}
