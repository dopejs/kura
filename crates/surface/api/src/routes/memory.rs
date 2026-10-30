//! memory route family (Roadmap 78, spec 058 — the memory plane foundation).
//!
//! Routes: GET/POST /v1/memory/assets, GET /v1/memory/assets/{asset_id},
//! GET /v1/memory/assets/{asset_id}/drilldown,
//! POST /v1/memory/assets/{asset_id}/{approve|reject|revoke|visibility},
//! POST /v1/memory/capture (L0 episode reference + turn bookkeeping),
//! POST /v1/memory/consolidate (manual consolidation trigger).
//!
//! Every mutation persists through the store DAO, publishes the matching
//! memory.* event, and re-renders the white-box Markdown projection for
//! ready L2/L3 assets under `<data_dir>/memory/`. A resolved tenant
//! context overrides any caller-supplied tenant.

use std::path::PathBuf;

use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use kura_events as events;
use kura_memory as memory;

use crate::error::ApiError;
use crate::middleware::{TenantContext, environment_scope_from_config};
use crate::state::AppState;

use super::{decode_json_or_default, decode_json_required};

/// Route family router.
#[must_use]
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/memory/assets", get(list_assets).post(create_asset))
        .route("/v1/memory/assets/{asset_id}", get(get_asset))
        .route("/v1/memory/assets/{asset_id}/drilldown", get(drilldown))
        .route("/v1/memory/assets/{asset_id}/approve", post(approve_asset))
        .route("/v1/memory/assets/{asset_id}/reject", post(reject_asset))
        .route("/v1/memory/assets/{asset_id}/revoke", post(revoke_asset))
        .route(
            "/v1/memory/assets/{asset_id}/visibility",
            post(set_visibility),
        )
        .route("/v1/memory/capture", post(capture))
        .route("/v1/memory/consolidate", post(consolidate))
        .route("/v1/memory/overview", get(overview))
        .route("/v1/memory/indexes/rebuild", post(rebuild_indexes))
}

#[derive(Debug, Serialize)]
struct AssetListResponse {
    items: Vec<memory::MemoryAsset>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AssetDecisionResponse {
    asset: memory::MemoryAsset,
    decision: memory::WriteDecision,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct ListQuery {
    tenant_id: String,
    layer: String,
    status: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct ReviewRequest {
    actor: memory::Actor,
    reason: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct VisibilityRequest {
    visibility: Option<memory::Visibility>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct CaptureRequest {
    tenant_id: String,
    owner: memory::Actor,
    title: String,
    source_links: Vec<memory::SourceLink>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct ConsolidateRequest {
    tenant_id: String,
    trigger: String,
    window: Vec<memory::L0Item>,
}

fn manager(state: &AppState) -> Result<&memory::Manager, ApiError> {
    state
        .memory
        .as_deref()
        .ok_or_else(|| ApiError::internal("memory manager is not configured"))
}

fn map_memory_error(err: memory::MemoryError) -> ApiError {
    let message = err.to_string();
    match err {
        memory::MemoryError::AssetNotFound => ApiError::NotFound(message),
        memory::MemoryError::Rejected(_) => ApiError::Forbidden(message),
        memory::MemoryError::NotPending
        | memory::MemoryError::NotActive
        | memory::MemoryError::InvalidVisibilityChange => ApiError::Conflict(message),
        _ => ApiError::BadRequest(message),
    }
}

fn context_tenant(tenant: &Option<Extension<TenantContext>>, fallback: &str) -> String {
    if let Some(tc) = tenant.as_ref().map(|extension| &extension.0.0) {
        if !tc.tenant_id.trim().is_empty() {
            return tc.tenant_id.trim().to_string();
        }
    }
    fallback.trim().to_string()
}

fn persist_asset(state: &AppState, asset: &memory::MemoryAsset) -> Result<(), ApiError> {
    state
        .store
        .lock()
        .upsert_memory_asset(asset)
        .map_err(ApiError::from_store)
}

fn publish_memory_event(
    state: &AppState,
    name: &str,
    asset: &memory::MemoryAsset,
) -> Result<(), ApiError> {
    let mut payload = serde_json::Map::new();
    payload.insert("tenantId".to_string(), serde_json::json!(asset.tenant_id));
    payload.insert("kind".to_string(), serde_json::json!(asset.kind.as_str()));
    payload.insert("layer".to_string(), serde_json::json!(asset.layer.as_str()));
    payload.insert(
        "status".to_string(),
        serde_json::json!(asset.status.as_str()),
    );
    payload.insert(
        "visibility".to_string(),
        serde_json::json!(asset.visibility.as_str()),
    );
    payload.insert("version".to_string(), serde_json::json!(asset.version));
    if !asset.supersedes_asset_id.is_empty() {
        payload.insert(
            "supersedesAssetId".to_string(),
            serde_json::json!(asset.supersedes_asset_id),
        );
    }
    let event = events::Event {
        category: "memory".to_string(),
        name: name.to_string(),
        environment_scope: environment_scope_from_config(&state.config),
        resource: events::Resource {
            kind: "memory_asset".to_string(),
            id: asset.asset_id.clone(),
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

/// Writes the white-box Markdown projection for ready L2/L3 assets.
fn project_markdown(state: &AppState, asset: &memory::MemoryAsset) {
    if asset.status != memory::AssetStatus::Ready
        || !matches!(
            asset.layer,
            memory::MemoryLayer::L2 | memory::MemoryLayer::L3
        )
    {
        return;
    }
    let Ok(manager) = manager(state) else { return };
    let tenant_dir = if asset.tenant_id.is_empty() {
        "local"
    } else {
        &asset.tenant_id
    };
    let dir = PathBuf::from(&state.config.data_dir)
        .join("memory")
        .join(tenant_dir);
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let _ = std::fs::write(
        dir.join(format!("{}.md", asset.asset_id)),
        manager.render_markdown(asset),
    );
}

fn finish_mutation(
    state: &AppState,
    event_name: &str,
    asset: &memory::MemoryAsset,
) -> Result<(), ApiError> {
    persist_asset(state, asset)?;
    clear_derived_indexes_if_unrecallable(state, asset);
    publish_memory_event(state, event_name, asset)?;
    project_markdown(state, asset);
    Ok(())
}

/// Drops derived-index rows for an asset that is no longer recallable.
///
/// `memory-system.md` requires that "forget or redact operations also clear
/// derived indexes and caches". That invariant used to hold for free: the
/// vector ranker recomputed over Ready assets every turn, so a revoked asset
/// simply stopped being a candidate. Persisting embeddings removed the free
/// guarantee, so it is enforced here — at the single choke point every asset
/// mutation already passes through, rather than at each call site.
///
/// Superseded assets are included: the superseding version is a different
/// asset id with its own vector, and the old one must stop being recallable.
///
/// A failure is logged, not propagated: the state transition itself already
/// committed, and a stale derived row cannot resurrect a non-Ready asset into
/// context (retrieval selects on status first). Losing the write would be a
/// leak of compute, not of memory.
fn clear_derived_indexes_if_unrecallable(state: &AppState, asset: &memory::MemoryAsset) {
    if matches!(
        asset.status,
        memory::AssetStatus::Ready | memory::AssetStatus::Pending
    ) {
        return;
    }
    if let Err(err) = state
        .store
        .lock()
        .delete_memory_asset_embeddings(&asset.asset_id)
    {
        eprintln!(
            "memory: failed to clear derived index for {} ({}): {err}",
            asset.asset_id,
            asset.status.as_str()
        );
    }
}

// ---------------------------------------------------------------------------
// Reusable write-path helpers (spec 058 phase 2): the capture hooks in the
// chat/connector/workflow families and the app scheduler tick call these.
// ---------------------------------------------------------------------------

/// Resolved-tenant helper shared with the skill-proposal family: a resolved
/// tenant context overrides any caller-supplied tenant.
pub fn route_tenant(tenant: &Option<Extension<TenantContext>>, fallback: &str) -> String {
    context_tenant(tenant, fallback)
}

/// Persists + publishes + projects one manager-created asset (the
/// full-content capture path for context refs, which must not go through
/// capture_l0's excerpt truncation).
pub fn persist_capture(state: &AppState, asset: &memory::MemoryAsset) {
    if let Err(err) = finish_mutation(state, "memory.asset_written", asset) {
        eprintln!("memory: persist capture failed: {err:?}");
    }
}

/// Fire-and-forget L0 capture + turn bookkeeping. Content is a bounded
/// excerpt (truth stays behind the source links). Returns Some((asset id,
/// extraction due)) on success; failures log and return None — capture
/// never fails the originating request.
pub fn capture_l0(
    state: &AppState,
    tenant_id: &str,
    owner: memory::Actor,
    role: &str,
    text: &str,
    source_links: Vec<memory::SourceLink>,
) -> Option<(String, bool)> {
    let manager = state.memory.as_deref()?;
    let excerpt: String = text.chars().take(2000).collect();
    let result = manager.create(memory::CreateAssetInput {
        kind: memory::AssetKind::ChatMemory,
        layer: memory::MemoryLayer::L0Ref,
        tenant_id: tenant_id.trim().to_string(),
        owner,
        visibility: memory::Visibility::Private,
        title: role.trim().to_string(),
        content: excerpt,
        source_links,
        ..memory::CreateAssetInput::default()
    });
    match result {
        Ok((asset, _)) => {
            if let Err(err) = finish_mutation(state, "memory.asset_written", &asset) {
                eprintln!("memory: persist capture failed: {err:?}");
            }
            let due = manager.record_turn(tenant_id.trim(), chrono::Utc::now());
            Some((asset.asset_id, due))
        }
        Err(err) => {
            eprintln!("memory: capture failed: {err}");
            None
        }
    }
}

/// Runs one consolidation pass: builds the pending L0 window when none is
/// supplied, refines through the Consolidator, persists + publishes every
/// written asset, and records the run event.
pub fn execute_consolidation(
    state: &AppState,
    tenant_id: &str,
    trigger: &str,
    window: Option<Vec<memory::L0Item>>,
) -> Result<memory::ConsolidationRun, ApiError> {
    let manager = manager(state)?;
    let tenant_id = tenant_id.trim();
    let window = match window {
        Some(window) if !window.is_empty() => window,
        _ => manager.pending_l0_window(tenant_id),
    };
    let (run, written) = manager.consolidate(tenant_id, trigger, &window);
    for asset in &written {
        finish_mutation(state, "memory.asset_written", asset)?;
    }
    let mut payload = serde_json::Map::new();
    payload.insert("tenantId".to_string(), serde_json::json!(run.tenant_id));
    payload.insert("trigger".to_string(), serde_json::json!(run.trigger));
    payload.insert(
        "extractedL1".to_string(),
        serde_json::json!(run.extracted_l1),
    );
    payload.insert(
        "aggregatedL2".to_string(),
        serde_json::json!(run.aggregated_l2),
    );
    payload.insert(
        "distilledL3".to_string(),
        serde_json::json!(run.distilled_l3),
    );
    if !run.error.is_empty() {
        payload.insert("error".to_string(), serde_json::json!(run.error));
    }
    let event = events::Event {
        category: "memory".to_string(),
        name: "memory.consolidation_run".to_string(),
        environment_scope: environment_scope_from_config(&state.config),
        resource: events::Resource {
            kind: "memory_consolidation".to_string(),
            id: run.run_id.clone(),
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
    Ok(run)
}

/// The 60s scheduler tick: idle-triggered consolidation for every tenant
/// with bookkeeping, plus the retention sweep. Errors log; the tick never
/// aborts.
pub fn memory_tick(state: &AppState) {
    let Some(manager) = state.memory.as_deref() else {
        return;
    };
    let now = chrono::Utc::now();
    for tenant in manager.tenants_with_bookkeeping() {
        if manager.idle_due(&tenant, now) {
            if let Err(err) = execute_consolidation(state, &tenant, "idle", None) {
                eprintln!("memory: idle consolidation for {tenant} failed: {err:?}");
            }
        }
    }
    for expired in manager.sweep_retention(now) {
        if let Err(err) = finish_mutation(state, "memory.asset_expired", &expired) {
            eprintln!("memory: retention persist failed: {err:?}");
        }
    }
}

/// GET /v1/memory/assets.
async fn list_assets(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Query(query): Query<ListQuery>,
) -> Result<Json<AssetListResponse>, ApiError> {
    let manager = manager(&state)?;
    let tenant_id = context_tenant(&tenant, &query.tenant_id);
    let layer = if query.layer.trim().is_empty() {
        None
    } else {
        serde_json::from_value(serde_json::json!(query.layer.trim())).ok()
    };
    let status = if query.status.trim().is_empty() {
        None
    } else {
        serde_json::from_value(serde_json::json!(query.status.trim())).ok()
    };
    Ok(Json(AssetListResponse {
        items: manager.list(&tenant_id, layer, status),
    }))
}

/// POST /v1/memory/assets — a policy-gated write; 201 with the stored asset
/// and the policy decision.
async fn create_asset(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<(StatusCode, Json<AssetDecisionResponse>), ApiError> {
    let mut input: memory::CreateAssetInput = decode_json_required(&body)?;
    input.tenant_id = context_tenant(&tenant, &input.tenant_id);
    let manager = manager(&state)?;
    let (asset, decision) = manager.create(input).map_err(map_memory_error)?;
    finish_mutation(&state, "memory.asset_written", &asset)?;
    if !asset.supersedes_asset_id.is_empty() && asset.status == memory::AssetStatus::Ready {
        if let Some(previous) = manager.get(&asset.supersedes_asset_id) {
            finish_mutation(&state, "memory.asset_superseded", &previous)?;
        }
    }
    Ok((
        StatusCode::CREATED,
        Json(AssetDecisionResponse { asset, decision }),
    ))
}

/// GET /v1/memory/assets/{asset_id}.
async fn get_asset(
    State(state): State<AppState>,
    Path(asset_id): Path<String>,
) -> Result<Json<memory::MemoryAsset>, ApiError> {
    let manager = manager(&state)?;
    manager
        .get(asset_id.trim())
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("not found".to_string()))
}

/// GET /v1/memory/assets/{asset_id}/drilldown — the deterministic path down
/// to L1 source links (the L0 citation).
async fn drilldown(
    State(state): State<AppState>,
    Path(asset_id): Path<String>,
) -> Result<Json<memory::DrilldownNode>, ApiError> {
    let manager = manager(&state)?;
    manager
        .drilldown(asset_id.trim())
        .map(Json)
        .map_err(map_memory_error)
}

/// POST /v1/memory/assets/{asset_id}/approve.
async fn approve_asset(
    State(state): State<AppState>,
    Path(asset_id): Path<String>,
    body: Bytes,
) -> Result<Json<memory::MemoryAsset>, ApiError> {
    let request: ReviewRequest = decode_json_or_default(&body)?;
    let manager = manager(&state)?;
    let (asset, superseded) = manager
        .approve(asset_id.trim(), &request.actor)
        .map_err(map_memory_error)?;
    finish_mutation(&state, "memory.asset_written", &asset)?;
    if let Some(previous) = superseded {
        finish_mutation(&state, "memory.asset_superseded", &previous)?;
    }
    Ok(Json(asset))
}

/// POST /v1/memory/assets/{asset_id}/reject.
async fn reject_asset(
    State(state): State<AppState>,
    Path(asset_id): Path<String>,
    body: Bytes,
) -> Result<Json<memory::MemoryAsset>, ApiError> {
    let request: ReviewRequest = decode_json_or_default(&body)?;
    let manager = manager(&state)?;
    let asset = manager
        .reject(asset_id.trim(), &request.reason)
        .map_err(map_memory_error)?;
    finish_mutation(&state, "memory.asset_revoked", &asset)?;
    Ok(Json(asset))
}

/// POST /v1/memory/assets/{asset_id}/revoke — reversibility: tombstone.
async fn revoke_asset(
    State(state): State<AppState>,
    Path(asset_id): Path<String>,
    body: Bytes,
) -> Result<Json<memory::MemoryAsset>, ApiError> {
    let request: ReviewRequest = decode_json_or_default(&body)?;
    let manager = manager(&state)?;
    let asset = manager
        .revoke(asset_id.trim(), &request.reason)
        .map_err(map_memory_error)?;
    finish_mutation(&state, "memory.asset_revoked", &asset)?;
    Ok(Json(asset))
}

/// POST /v1/memory/assets/{asset_id}/visibility — narrowing applies
/// immediately; widening runs the policy gate.
async fn set_visibility(
    State(state): State<AppState>,
    Path(asset_id): Path<String>,
    body: Bytes,
) -> Result<Json<AssetDecisionResponse>, ApiError> {
    let request: VisibilityRequest = decode_json_required(&body)?;
    let visibility = request
        .visibility
        .ok_or_else(|| ApiError::BadRequest("visibility is required".to_string()))?;
    let manager = manager(&state)?;
    let (asset, decision) = manager
        .set_visibility(asset_id.trim(), visibility)
        .map_err(map_memory_error)?;
    finish_mutation(&state, "memory.asset_written", &asset)?;
    Ok(Json(AssetDecisionResponse { asset, decision }))
}

/// POST /v1/memory/capture — records an L0 episode reference and the turn
/// bookkeeping; responds with whether an extraction pass is now due.
async fn capture(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let request: CaptureRequest = decode_json_required(&body)?;
    let manager = manager(&state)?;
    let tenant_id = context_tenant(&tenant, &request.tenant_id);
    let (asset, _) = manager
        .create(memory::CreateAssetInput {
            kind: memory::AssetKind::ChatMemory,
            layer: memory::MemoryLayer::L0Ref,
            tenant_id: tenant_id.clone(),
            owner: request.owner,
            visibility: memory::Visibility::Private,
            title: request.title,
            content: String::new(),
            source_links: request.source_links,
            ..memory::CreateAssetInput::default()
        })
        .map_err(map_memory_error)?;
    finish_mutation(&state, "memory.asset_written", &asset)?;
    let extraction_due = manager.record_turn(&tenant_id, chrono::Utc::now());
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "asset": asset,
            "extractionDue": extraction_due,
        })),
    ))
}

/// POST /v1/memory/consolidate — the manual consolidation trigger; the L0
/// window defaults to the tenant's pending captures.
async fn consolidate(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<Json<memory::ConsolidationRun>, ApiError> {
    let request: ConsolidateRequest = decode_json_or_default(&body)?;
    let tenant_id = context_tenant(&tenant, &request.tenant_id);
    let trigger = if request.trigger.trim().is_empty() {
        "manual"
    } else {
        request.trigger.trim()
    };
    let window = if request.window.is_empty() {
        None
    } else {
        Some(request.window)
    };
    let run = execute_consolidation(&state, &tenant_id, trigger, window)?;
    Ok(Json(run))
}

// ---------------------------------------------------------------------------
// What is remembered (Stage 2.1) and index rebuild (Stage 2.2)
// ---------------------------------------------------------------------------

/// One `(layer, status)` bucket of the tenant's memory plane.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryLayerCount {
    pub layer: String,
    pub status: String,
    pub count: i64,
}

/// A compact record of something the agent recently started or stopped
/// remembering. Content is deliberately absent — this is an inventory, and the
/// asset routes already serve content with their visibility rules applied.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryChangeEntry {
    pub asset_id: String,
    pub layer: String,
    pub status: String,
    pub title: String,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryOverviewResponse {
    pub tenant_id: String,
    pub counts: Vec<MemoryLayerCount>,
    /// Vectors held in the derived retrieval index for this tenant's assets.
    /// A count far below the Ready total means the index is not being written
    /// — visible here as a number instead of as unexplained latency.
    pub derived_embeddings: i64,
    pub recently_remembered: Vec<MemoryChangeEntry>,
    pub recently_forgotten: Vec<MemoryChangeEntry>,
}

const OVERVIEW_RECENT_LIMIT: usize = 20;

fn change_entry(asset: &memory::MemoryAsset) -> MemoryChangeEntry {
    MemoryChangeEntry {
        asset_id: asset.asset_id.clone(),
        layer: asset.layer.as_str().to_string(),
        status: asset.status.as_str().to_string(),
        title: asset.title.clone(),
        updated_at: asset.updated_at,
    }
}

/// GET /v1/memory/overview — the tenant-level answer to "what does it
/// remember, and what did it forget".
///
/// The memory plane could already be read asset by asset, which answers the
/// question only for someone who already knows what to look for. This is the
/// inventory: counts per layer and status, the size of the derived index, and
/// the two recency lists that matter — what was just written, and what was
/// just revoked or expired.
async fn overview(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<MemoryOverviewResponse>, ApiError> {
    let manager = manager(&state)?;
    let tenant_id = context_tenant(&tenant, params.get("tenantId").map_or("", String::as_str));

    let counts = state
        .store_pool
        .read()
        .count_memory_assets_by_layer_status(&tenant_id)
        .map_err(ApiError::from_store)?
        .into_iter()
        .map(|(layer, status, count)| MemoryLayerCount {
            layer,
            status,
            count,
        })
        .collect();
    let derived_embeddings = state
        .store_pool
        .read()
        .count_memory_asset_embeddings_for_tenant(&tenant_id)
        .map_err(ApiError::from_store)?;

    let mut assets = manager.list(&tenant_id, None, None);
    assets.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

    let recently_remembered = assets
        .iter()
        .filter(|a| a.status == memory::AssetStatus::Ready)
        .take(OVERVIEW_RECENT_LIMIT)
        .map(change_entry)
        .collect();
    // "Forgotten" is every terminal state that removes an asset from recall,
    // not just explicit revocation: superseding and retention expiry are also
    // things the operator did not necessarily watch happen.
    let recently_forgotten = assets
        .iter()
        .filter(|a| {
            matches!(
                a.status,
                memory::AssetStatus::Revoked
                    | memory::AssetStatus::Expired
                    | memory::AssetStatus::Superseded
            )
        })
        .take(OVERVIEW_RECENT_LIMIT)
        .map(change_entry)
        .collect();

    Ok(Json(MemoryOverviewResponse {
        tenant_id,
        counts,
        derived_embeddings,
        recently_remembered,
        recently_forgotten,
    }))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RebuildIndexesResponse {
    pub tenant_id: String,
    /// Derived rows dropped. Without `reembed` they are recomputed lazily by
    /// the next retrievals (up to the embedder's reply-path bound).
    pub cleared_embeddings: usize,
    /// Present when `reembed=true` asked for the corpus to be embedded now.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reembed: Option<ReembedSummary>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReembedSummary {
    /// The embedder that wrote the vectors, and the space they are cached
    /// under.
    pub embedder: String,
    pub fingerprint: String,
    /// Docs in the recallable corpus.
    pub corpus: usize,
    pub embedded: usize,
    /// Docs the embedder had no vector for; they stay BM25-only. Non-zero
    /// means run this again once the provider is healthy.
    pub failed: usize,
}

/// POST /v1/memory/indexes/rebuild — drops this tenant's derived retrieval
/// index so it is recomputed from the assets.
///
/// Deliberately **not** a delete of anything the operator would miss:
/// conversation truth and the memory assets themselves are untouched. This is
/// the repair path a persisted index needs — without it, a corrupt or stale
/// index has no remedy short of deleting the memory plane, which is exactly
/// the trade `memory reset` exists to avoid.
///
/// `reembed=true` then embeds the whole recallable corpus with the active
/// embedder before answering. That is the explicit re-embed a model-backed
/// embedder needs after its model changes: the reply path embeds only a few
/// misses per turn, so a corpus is never re-embedded as a side effect of
/// boot or of a chat turn. It makes provider requests (in batches) and can
/// take a while on a large corpus; it runs off the async workers.
async fn rebuild_indexes(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<RebuildIndexesResponse>, ApiError> {
    let tenant_id = context_tenant(&tenant, params.get("tenantId").map_or("", String::as_str));
    let reembed = match params.get("reembed").map(String::as_str) {
        None | Some("false" | "0" | "") => false,
        Some("true" | "1") => true,
        Some(other) => {
            return Err(ApiError::BadRequest(format!(
                "reembed must be true or false, got {other}"
            )));
        }
    };
    if reembed {
        // Refuse before clearing: a re-embed that cannot run must not leave
        // the operator with an emptied index they did not ask for alone.
        manager(&state)?;
    }
    let cleared = state
        .store
        .lock()
        .clear_memory_asset_embeddings_for_tenant(&tenant_id)
        .map_err(ApiError::from_store)?;
    let reembed = if reembed {
        let state = state.clone();
        let tenant_id = tenant_id.clone();
        let summary = tokio::task::spawn_blocking(move || {
            let memory = manager(&state)?;
            let docs = super::retrieval::retrieval_docs(&super::retrieval::recall_atoms(
                memory, &tenant_id,
            ));
            let default_embedder = kura_context::HashedNgramEmbedder::default();
            let embedder = super::retrieval::active_embedder(&state, &default_embedder);
            let outcome = super::retrieval::embed_corpus(&state, embedder, &docs, None);
            Ok::<_, ApiError>(ReembedSummary {
                embedder: embedder.name().to_string(),
                fingerprint: embedder.fingerprint(),
                corpus: docs.len(),
                embedded: outcome.embedded,
                failed: outcome.failed,
            })
        })
        .await
        .map_err(|err| ApiError::internal(format!("re-embed task: {err}")))??;
        Some(summary)
    } else {
        None
    };
    Ok(Json(RebuildIndexesResponse {
        tenant_id,
        cleared_embeddings: cleared,
        reembed,
    }))
}

#[cfg(test)]
mod tests {
    use super::super::tests_support::{request_json, request_json_as_tenant, test_state};
    use axum::http::StatusCode;
    use std::sync::Arc;

    fn state_with_manager() -> crate::state::AppState {
        let mut state = test_state();
        state.memory = Some(Arc::new(kura_memory::Manager::new(
            "test", None, None, None,
        )));
        state
    }

    fn atom_body(content: &str) -> serde_json::Value {
        serde_json::json!({
            "kind": "chat_memory",
            "layer": "l1",
            "owner": { "kind": "operator", "id": "op_1" },
            "atomType": "preference",
            "title": "reply language",
            "content": content,
            "sourceLinks": [{ "kind": "thread", "id": "thr_1", "excerpt": "用中文回复" }]
        })
    }

    #[tokio::test]
    async fn create_list_drilldown_and_revoke_atom() {
        let state = state_with_manager();
        let (status, created) = request_json(
            state.clone(),
            "POST",
            "/v1/memory/assets",
            Some(atom_body("prefers Chinese replies")),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        assert_eq!(created["decision"], "accept", "{created}");
        assert_eq!(created["asset"]["status"], "ready");
        let atom_id = created["asset"]["assetId"]
            .as_str()
            .expect("assetId")
            .to_string();

        // L2 scenario over the atom; drill-down resolves to the atom's
        // source links.
        let (status, scenario) = request_json(
            state.clone(),
            "POST",
            "/v1/memory/assets",
            Some(serde_json::json!({
                "kind": "chat_memory",
                "layer": "l2",
                "owner": { "kind": "operator", "id": "op_1" },
                "title": "communication preferences",
                "content": "The user prefers Chinese replies.",
                "memberAssetIds": [atom_id]
            })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{scenario}");
        let scenario_id = scenario["asset"]["assetId"]
            .as_str()
            .expect("assetId")
            .to_string();

        let (status, tree) = request_json(
            state.clone(),
            "GET",
            &format!("/v1/memory/assets/{scenario_id}/drilldown"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{tree}");
        assert_eq!(tree["members"][0]["asset"]["assetId"], atom_id.as_str());
        assert_eq!(
            tree["members"][0]["asset"]["sourceLinks"][0]["id"], "thr_1",
            "{tree}"
        );

        // The white-box Markdown projection exists on disk.
        let path = std::path::PathBuf::from(&state.config.data_dir)
            .join("memory")
            .join("local")
            .join(format!("{scenario_id}.md"));
        assert!(path.exists(), "markdown projection missing at {path:?}");

        let (status, listed) =
            request_json(state.clone(), "GET", "/v1/memory/assets?layer=l1", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(listed["items"].as_array().expect("items").len(), 1);

        let (status, revoked) = request_json(
            state,
            "POST",
            &format!("/v1/memory/assets/{atom_id}/revoke"),
            Some(serde_json::json!({ "reason": "user asked to forget" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{revoked}");
        assert_eq!(revoked["status"], "revoked");
    }

    #[tokio::test]
    async fn agent_writes_require_approval_and_unattributed_writes_reject() {
        let state = state_with_manager();
        let mut body = atom_body("agent noticed a deadline");
        body["owner"] = serde_json::json!({ "kind": "agent", "id": "agent_main" });
        let (status, pending) =
            request_json(state.clone(), "POST", "/v1/memory/assets", Some(body)).await;
        assert_eq!(status, StatusCode::CREATED, "{pending}");
        assert_eq!(pending["asset"]["status"], "pending");
        let asset_id = pending["asset"]["assetId"]
            .as_str()
            .expect("assetId")
            .to_string();

        let (status, approved) = request_json(
            state.clone(),
            "POST",
            &format!("/v1/memory/assets/{asset_id}/approve"),
            Some(serde_json::json!({ "actor": { "kind": "operator", "id": "op_1" } })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{approved}");
        assert_eq!(approved["status"], "ready");

        // No source links -> rejected at the boundary.
        let mut unattributed = atom_body("floating claim");
        unattributed["sourceLinks"] = serde_json::json!([]);
        let (status, err) =
            request_json(state, "POST", "/v1/memory/assets", Some(unattributed)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");
    }

    #[tokio::test]
    async fn capture_and_manual_consolidation_record_runs() {
        let state = state_with_manager();
        let (status, captured) = request_json(
            state.clone(),
            "POST",
            "/v1/memory/capture",
            Some(serde_json::json!({
                "owner": { "kind": "system", "id": "chat" },
                "title": "session turn",
                "sourceLinks": [{ "kind": "message", "id": "msg_1" }]
            })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{captured}");
        // Warm-up doubling: the first turn is immediately extraction-due.
        assert_eq!(captured["extractionDue"], true, "{captured}");

        let (status, run) = request_json(
            state,
            "POST",
            "/v1/memory/consolidate",
            Some(serde_json::json!({ "trigger": "manual" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{run}");
        assert_eq!(run["trigger"], "manual");
        // The Noop consolidator records the run with zero drafts.
        assert_eq!(run["extractedL1"], 0);
    }

    /// Stage 2.1: the inventory answers "what does it remember, and what did
    /// it forget" without the reader having to already know which asset to
    /// look up.
    #[tokio::test]
    async fn overview_reports_counts_and_both_recency_lists() {
        let state = state_with_manager();

        let (status, created) = request_json(
            state.clone(),
            "POST",
            "/v1/memory/assets",
            Some(atom_body("prefers Chinese replies")),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let kept = created["asset"]["assetId"]
            .as_str()
            .expect("assetId")
            .to_string();

        let (status, created) = request_json(
            state.clone(),
            "POST",
            "/v1/memory/assets",
            Some(atom_body("uses dark mode")),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let forgotten = created["asset"]["assetId"]
            .as_str()
            .expect("assetId")
            .to_string();

        let (status, _) = request_json(
            state.clone(),
            "POST",
            &format!("/v1/memory/assets/{forgotten}/revoke"),
            Some(serde_json::json!({ "reason": "wrong" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, overview) =
            request_json(state.clone(), "GET", "/v1/memory/overview", None).await;
        assert_eq!(status, StatusCode::OK, "{overview}");

        let counts = overview["counts"].as_array().expect("counts");
        let ready: i64 = counts
            .iter()
            .filter(|c| c["status"] == "ready")
            .map(|c| c["count"].as_i64().unwrap_or(0))
            .sum();
        let revoked: i64 = counts
            .iter()
            .filter(|c| c["status"] == "revoked")
            .map(|c| c["count"].as_i64().unwrap_or(0))
            .sum();
        assert_eq!(ready, 1, "one asset still remembered: {overview}");
        assert_eq!(revoked, 1, "one asset forgotten: {overview}");

        let remembered: Vec<&str> = overview["recentlyRemembered"]
            .as_array()
            .expect("recentlyRemembered")
            .iter()
            .filter_map(|e| e["assetId"].as_str())
            .collect();
        assert_eq!(remembered, [kept.as_str()]);

        let gone: Vec<&str> = overview["recentlyForgotten"]
            .as_array()
            .expect("recentlyForgotten")
            .iter()
            .filter_map(|e| e["assetId"].as_str())
            .collect();
        assert_eq!(gone, [forgotten.as_str()]);
    }

    /// Stage 2.2: the rebuild is a repair, not a delete — it drops derived
    /// rows and leaves every asset in place.
    #[tokio::test]
    async fn index_rebuild_clears_derived_rows_and_keeps_the_assets() {
        let state = state_with_manager();
        let (status, created) = request_json(
            state.clone(),
            "POST",
            "/v1/memory/assets",
            Some(atom_body("prefers Chinese replies")),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let asset_id = created["asset"]["assetId"]
            .as_str()
            .expect("assetId")
            .to_string();

        // Stand in for what the retrieval path writes on a cache miss.
        state
            .store
            .lock()
            .put_memory_asset_embedding(&asset_id, "fp-test", &[0.5, 0.5])
            .expect("seed derived row");
        assert_eq!(
            state
                .store
                .lock()
                .count_memory_asset_embeddings_for_tenant("")
                .expect("count"),
            1
        );

        let (status, rebuilt) =
            request_json(state.clone(), "POST", "/v1/memory/indexes/rebuild", None).await;
        assert_eq!(status, StatusCode::OK, "{rebuilt}");
        assert_eq!(rebuilt["clearedEmbeddings"], 1);

        assert_eq!(
            state
                .store
                .lock()
                .count_memory_asset_embeddings_for_tenant("")
                .expect("count"),
            0,
            "derived rows dropped"
        );
        // Conversation truth untouched: this is the whole point of a rebuild
        // rather than a reset.
        let (status, asset) = request_json(
            state.clone(),
            "GET",
            &format!("/v1/memory/assets/{asset_id}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{asset}");
        assert_eq!(asset["status"], "ready");
    }

    async fn create_atom(state: &crate::state::AppState, content: &str) -> String {
        let (status, created) = request_json(
            state.clone(),
            "POST",
            "/v1/memory/assets",
            Some(atom_body(content)),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        created["asset"]["assetId"]
            .as_str()
            .expect("assetId")
            .to_string()
    }

    /// G10: `reembed=true` is the operator's explicit re-embed — the whole
    /// recallable corpus is embedded with the active embedder before the
    /// reply, and the summary names the space it was written in.
    #[tokio::test]
    async fn index_rebuild_reembeds_the_corpus_when_asked() {
        let state = state_with_manager();
        create_atom(&state, "prefers Chinese replies").await;
        create_atom(&state, "uses pnpm for web projects").await;

        let (status, rebuilt) = request_json(
            state.clone(),
            "POST",
            "/v1/memory/indexes/rebuild?reembed=true",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{rebuilt}");
        assert_eq!(rebuilt["reembed"]["embedder"], "hashed-ngram-256");
        assert_eq!(rebuilt["reembed"]["fingerprint"], "hashed-ngram/3/256/v1");
        assert_eq!(rebuilt["reembed"]["corpus"], 2);
        assert_eq!(rebuilt["reembed"]["embedded"], 2);
        assert_eq!(rebuilt["reembed"]["failed"], 0);
        assert_eq!(
            state
                .store
                .lock()
                .count_memory_asset_embeddings_for_tenant("")
                .expect("count"),
            2
        );

        // Without the flag the reply shape is unchanged.
        let (_, plain) =
            request_json(state.clone(), "POST", "/v1/memory/indexes/rebuild", None).await;
        assert!(plain.get("reembed").is_none(), "{plain}");

        let (status, _) = request_json(
            state.clone(),
            "POST",
            "/v1/memory/indexes/rebuild?reembed=maybe",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    /// A model-backed embedder: batches, a reply-path bound, and a text it
    /// cannot embed.
    struct BoundedEmbedder {
        batches: std::sync::Mutex<Vec<usize>>,
    }

    impl kura_context::Embedder for BoundedEmbedder {
        fn embed(&self, text: &str) -> Vec<f32> {
            if text.contains("unembeddable") {
                Vec::new()
            } else {
                vec![1.0, 0.0]
            }
        }
        fn embed_batch(&self, texts: &[String]) -> Vec<Vec<f32>> {
            self.batches.lock().expect("lock").push(texts.len());
            texts.iter().map(|text| self.embed(text)).collect()
        }
        fn name(&self) -> &str {
            "bounded"
        }
        fn inline_embed_limit(&self) -> Option<usize> {
            Some(2)
        }
    }

    /// The reply path embeds at most the embedder's bound of misses, in one
    /// batch, newest first; a failed doc is not cached, and the rest wait
    /// for the operator's re-embed instead of costing this turn a request
    /// each.
    #[tokio::test]
    async fn corpus_vectors_bound_reply_path_embedding_and_never_cache_a_failure() {
        let state = state_with_manager();
        for content in [
            "oldest fact",
            "older fact",
            "unembeddable fact",
            "newest fact",
        ] {
            create_atom(&state, content).await;
            // updated_at orders the corpus; keep it strictly increasing.
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let atoms =
            super::super::retrieval::recall_atoms(state.memory.as_deref().expect("memory"), "");
        let docs = super::super::retrieval::retrieval_docs(&atoms);
        assert!(docs[0].content.contains("newest"), "newest first");
        let embedder = BoundedEmbedder {
            batches: std::sync::Mutex::new(Vec::new()),
        };

        let first = super::super::retrieval::embed_corpus(
            &state,
            &embedder,
            &docs,
            kura_context::Embedder::inline_embed_limit(&embedder),
        );
        assert_eq!(
            *embedder.batches.lock().expect("lock"),
            vec![2],
            "one batch"
        );
        assert_eq!(
            (first.cached, first.embedded, first.failed, first.deferred),
            (0, 1, 1, 2)
        );
        assert_eq!(first.vectors[0], vec![1.0, 0.0], "newest embedded");
        assert!(first.vectors[1].is_empty(), "failure is BM25-only");
        assert!(first.vectors[3].is_empty(), "deferred is BM25-only");

        // The failure was not cached, so it is a miss again: the next bounded
        // call retries it alongside the next-newest miss, and serves the
        // cached doc without a request.
        let second = super::super::retrieval::embed_corpus(&state, &embedder, &docs, Some(2));
        assert_eq!(
            (
                second.cached,
                second.embedded,
                second.failed,
                second.deferred
            ),
            (1, 1, 1, 1)
        );
        assert_eq!(second.vectors[2], vec![1.0, 0.0], "older embedded now");
        let unbounded = super::super::retrieval::embed_corpus(&state, &embedder, &docs, None);
        assert_eq!(
            (
                unbounded.cached,
                unbounded.embedded,
                unbounded.failed,
                unbounded.deferred
            ),
            (2, 1, 1, 0),
            "the operator path embeds every remaining miss"
        );
    }

    /// A rebuild must repair the caller's own index, never every tenant's.
    #[tokio::test]
    async fn index_rebuild_is_scoped_to_the_acting_tenant() {
        let state = state_with_manager();

        let seed = |tenant: &str| {
            let body = atom_body("prefers Chinese replies");
            let state = state.clone();
            let tenant = tenant.to_string();
            async move {
                let (status, created) = request_json_as_tenant(
                    state.clone(),
                    &tenant,
                    "POST",
                    "/v1/memory/assets",
                    Some(body),
                )
                .await;
                assert_eq!(status, StatusCode::CREATED, "{created}");
                let asset_id = created["asset"]["assetId"]
                    .as_str()
                    .expect("assetId")
                    .to_string();
                state
                    .store
                    .lock()
                    .put_memory_asset_embedding(&asset_id, "fp-test", &[0.5, 0.5])
                    .expect("seed derived row");
                asset_id
            }
        };
        let _a = seed("tnt_a").await;
        let _b = seed("tnt_b").await;

        let count = |tenant: &str| {
            state
                .store
                .lock()
                .count_memory_asset_embeddings_for_tenant(tenant)
                .expect("count")
        };
        assert_eq!(count("tnt_a"), 1);
        assert_eq!(count("tnt_b"), 1);

        let (status, rebuilt) = request_json_as_tenant(
            state.clone(),
            "tnt_a",
            "POST",
            "/v1/memory/indexes/rebuild",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{rebuilt}");
        assert_eq!(rebuilt["clearedEmbeddings"], 1);
        assert_eq!(count("tnt_a"), 0);
        assert_eq!(count("tnt_b"), 1, "another tenant's index is untouched");
    }
}
