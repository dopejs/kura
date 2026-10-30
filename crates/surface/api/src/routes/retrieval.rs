//! `/v1/retrieval/queries` — source-linked retrieval over the memory plane.
//!
//! The knowledge-retrieval query surface: the same fused ranking the
//! context plugin uses at `chat/pre-dispatch` (BM25 + recency + hashed
//! n-gram vector, RRF), exposed as an API so clients and agent tools can
//! recall memory on demand. Every hit carries its source links and
//! drill-down member ids — recalled results are evidence, never bare text.

use axum::body::Bytes;
use axum::extract::State;
use axum::routing::post;
use axum::{Extension, Router};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::middleware::TenantContext;
use crate::response::Json;
use crate::state::AppState;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RetrievalQueryRequest {
    pub query: String,
    pub tenant_id: String,
    /// Result cap; 0 uses the default (5).
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetrievalHit {
    pub asset_id: String,
    pub layer: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub title: String,
    pub content: String,
    /// 1-based fused rank (RRF order).
    pub rank: usize,
    pub source_links: Vec<kura_memory::SourceLink>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub member_asset_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetrievalQueryResponse {
    pub hits: Vec<RetrievalHit>,
}

const DEFAULT_LIMIT: usize = 5;

/// POST /v1/retrieval/queries — fused recall over Ready L1 atoms
/// (private/team visibility, scoped to the acting tenant; empty tenant = local
/// scope). A body `tenantId` naming another tenant is refused.
#[allow(clippy::unused_async)]
pub async fn query(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<Json<RetrievalQueryResponse>, ApiError> {
    let request: RetrievalQueryRequest = super::decode_json_required(&body)?;
    if request.query.trim().is_empty() {
        return Err(ApiError::BadRequest("query is required".to_string()));
    }
    let tenant_id = super::scoped_tenant(tenant.as_deref(), &request.tenant_id)?;
    let hits =
        run_query(&state, &tenant_id, &request.query, request.limit).map_err(ApiError::internal)?;
    Ok(Json(RetrievalQueryResponse { hits }))
}

/// The retrieval query proper, shared by the route and the `memory.lookup`
/// chat tool (Stage 9.0b) so an agent recalling memory mid-turn ranks over
/// exactly the corpus the API exposes. `limit == 0` uses the default.
pub fn run_query(
    state: &AppState,
    tenant_id: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<RetrievalHit>, String> {
    let Some(memory) = state.memory.as_deref() else {
        return Err("memory manager is not configured".to_string());
    };
    let atoms = recall_atoms(memory, tenant_id);
    let docs = retrieval_docs(&atoms);
    let default_embedder = kura_context::HashedNgramEmbedder::default();
    let embedder = active_embedder(state, &default_embedder);
    // Same persisted derived index as the turn-time path: this route used to
    // re-embed the whole corpus per request, which Stage 1.1 fixed for the
    // context plugin and missed here.
    let doc_vectors = corpus_vectors(state, embedder, &docs);
    let limit = if limit == 0 { DEFAULT_LIMIT } else { limit };
    Ok(
        kura_context::retrieve_fused_with_vectors(query, &docs, Some(embedder), Some(&doc_vectors))
            .into_iter()
            .take(limit)
            .enumerate()
            .map(|(position, idx)| {
                let asset = &atoms[idx];
                RetrievalHit {
                    asset_id: asset.asset_id.clone(),
                    layer: asset.layer.as_str().to_string(),
                    title: asset.title.clone(),
                    content: asset.content.clone(),
                    rank: position + 1,
                    source_links: asset.source_links.clone(),
                    member_asset_ids: asset.member_asset_ids.clone(),
                }
            })
            .collect(),
    )
}

/// The recallable corpus for `tenant_id`: Ready L1–L3 assets with private
/// or team visibility, newest first, bounded by
/// [`kura_context::RETRIEVAL_MAX_CORPUS`]. Shared by the query route and the
/// operator re-embed so both see the same docs.
#[must_use]
pub fn recall_atoms(
    memory: &kura_memory::Manager,
    tenant_id: &str,
) -> Vec<kura_memory::MemoryAsset> {
    // L1 atoms plus L2/L3: the same corpus the context plugin ranks over, so
    // the API and the turn-time path cannot disagree about what is recallable.
    let mut atoms: Vec<kura_memory::MemoryAsset> = [
        kura_memory::MemoryLayer::L1,
        kura_memory::MemoryLayer::L2,
        kura_memory::MemoryLayer::L3,
    ]
    .into_iter()
    .flat_map(|layer| {
        memory.list(
            tenant_id,
            Some(layer),
            Some(kura_memory::AssetStatus::Ready),
        )
    })
    .collect();
    atoms.retain(|asset| {
        matches!(
            asset.visibility,
            kura_memory::Visibility::Private | kura_memory::Visibility::Team
        )
    });
    // Newest first: the corpus index doubles as the recency rank.
    atoms.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    atoms.truncate(kura_context::RETRIEVAL_MAX_CORPUS);
    atoms
}

/// The ranker's view of `atoms`, positionally aligned.
#[must_use]
pub fn retrieval_docs(atoms: &[kura_memory::MemoryAsset]) -> Vec<kura_context::RetrievalDoc> {
    atoms
        .iter()
        .map(|asset| kura_context::RetrievalDoc {
            asset_id: asset.asset_id.clone(),
            layer: asset.layer.as_str().to_string(),
            title: asset.title.clone(),
            content: asset.content.clone(),
        })
        .collect()
}

/// The embedder retrieval ranks with: an installed one (external plugin or
/// the routed `embed` role), else `default` — the hashed n-gram.
#[must_use]
pub fn active_embedder<'a>(
    state: &'a AppState,
    default: &'a kura_context::HashedNgramEmbedder,
) -> &'a dyn kura_context::Embedder {
    match state.embedder.as_deref() {
        Some(installed) => installed,
        None => default,
    }
}

/// Vectors for a retrieval corpus, positionally aligned with `docs`.
///
/// Cache hits come from `memory_asset_embeddings`; misses are embedded once
/// and written back, so a corpus is embedded once rather than once per
/// request. A store failure degrades to computing in-process for this call
/// rather than dropping the vector ranker — availability first.
///
/// Misses past the embedder's [`kura_context::Embedder::inline_embed_limit`]
/// are left empty (BM25-only) rather than embedded on the reply path; the
/// newest docs come first, so the bound defers the oldest. Re-embedding the
/// whole corpus is the operator's `POST /v1/memory/indexes/rebuild?reembed=true`.
///
/// Lives here rather than in `kura-context` because it needs the store, and
/// the context crate is persistence-free by the workspace's inversion rule.
/// The context plugin in `kura-app` calls this same function, so the two
/// retrieval paths cannot drift.
#[must_use]
pub fn corpus_vectors(
    state: &AppState,
    embedder: &dyn kura_context::Embedder,
    docs: &[kura_context::RetrievalDoc],
) -> Vec<Vec<f32>> {
    embed_corpus(state, embedder, docs, embedder.inline_embed_limit()).vectors
}

/// Texts per embedding request when filling misses.
const EMBED_BATCH: usize = 64;

/// What [`embed_corpus`] did, per doc.
#[derive(Debug, Default)]
pub struct CorpusEmbedding {
    /// Positionally aligned with the docs; empty = no vector (BM25-only).
    pub vectors: Vec<Vec<f32>>,
    /// Served from the derived index.
    pub cached: usize,
    /// Embedded by this call (and written back).
    pub embedded: usize,
    /// The embedder had no vector for them.
    pub failed: usize,
    /// Left for a later call by `limit`.
    pub deferred: usize,
}

/// [`corpus_vectors`] with an explicit miss bound (`None` = embed every
/// miss, the operator re-embed) and the per-outcome counts.
pub fn embed_corpus(
    state: &AppState,
    embedder: &dyn kura_context::Embedder,
    docs: &[kura_context::RetrievalDoc],
    limit: Option<usize>,
) -> CorpusEmbedding {
    let fingerprint = embedder.fingerprint();
    let ids: Vec<String> = docs.iter().map(|d| d.asset_id.clone()).collect();
    let cached = state
        .store_pool
        .read()
        .get_memory_asset_embeddings(&fingerprint, &ids)
        .unwrap_or_default();

    let mut out = CorpusEmbedding {
        vectors: vec![Vec::new(); docs.len()],
        ..CorpusEmbedding::default()
    };
    let mut misses: Vec<usize> = Vec::new();
    for (i, doc) in docs.iter().enumerate() {
        if let Some(vector) = cached.get(&doc.asset_id) {
            out.vectors[i] = vector.clone();
            out.cached += 1;
        } else {
            misses.push(i);
        }
    }
    if let Some(limit) = limit
        && misses.len() > limit
    {
        out.deferred = misses.len() - limit;
        misses.truncate(limit);
    }

    for chunk in misses.chunks(EMBED_BATCH) {
        let texts: Vec<String> = chunk
            .iter()
            .map(|&i| format!("{} {}", docs[i].title, docs[i].content))
            .collect();
        let vectors = embedder.embed_batch(&texts);
        for (position, &i) in chunk.iter().enumerate() {
            let vector = vectors.get(position).cloned().unwrap_or_default();
            if vector.is_empty() {
                out.failed += 1;
                continue;
            }
            out.embedded += 1;
            if let Err(err) = state.store.lock().put_memory_asset_embedding(
                &docs[i].asset_id,
                &fingerprint,
                &vector,
            ) {
                eprintln!(
                    "[kura] retrieval: caching the embedding for {} failed ({err}); \
                     recomputing it every call until this is fixed",
                    docs[i].asset_id
                );
            }
            out.vectors[i] = vector;
        }
    }

    record_corpus_embedding(embedder.name(), &out);
    out
}

fn record_corpus_embedding(embedder: &str, outcome: &CorpusEmbedding) {
    use kura_telemetry::metrics::{RETRIEVAL_EMBEDDINGS_TOTAL, registry};
    for (kind, count) in [
        ("cached", outcome.cached),
        ("embedded", outcome.embedded),
        ("failed", outcome.failed),
        ("deferred", outcome.deferred),
    ] {
        if count > 0 {
            registry().add(
                RETRIEVAL_EMBEDDINGS_TOTAL,
                &[("embedder", embedder), ("outcome", kind)],
                count as u64,
            );
        }
    }
    if outcome.deferred > 0 && !DEFERRAL_WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        eprintln!(
            "[kura] retrieval: {} memory docs have no {embedder} vector and were left \
             BM25-only; run POST /v1/memory/indexes/rebuild?reembed=true to embed the corpus \
             (logged once; kura_retrieval_embeddings_total{{outcome=\"deferred\"}} keeps count)",
            outcome.deferred
        );
    }
}

static DEFERRAL_WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn router() -> Router<AppState> {
    Router::new().route("/v1/retrieval/queries", post(query))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::http::StatusCode;

    use super::super::tests_support::{request_json, request_json_as_tenant, test_state};

    fn seed_atom(manager: &kura_memory::Manager, content: &str) {
        manager
            .create(kura_memory::CreateAssetInput {
                kind: kura_memory::AssetKind::ChatMemory,
                layer: kura_memory::MemoryLayer::L1,
                owner: kura_memory::Actor {
                    kind: kura_memory::ActorKind::Operator,
                    id: "op".to_string(),
                },
                visibility: kura_memory::Visibility::Private,
                atom_type: Some(kura_memory::AtomType::Fact),
                content: content.to_string(),
                source_links: vec![kura_memory::SourceLink {
                    kind: kura_memory::SourceKind::Thread,
                    id: "thr_1".to_string(),
                    ..kura_memory::SourceLink::default()
                }],
                ..kura_memory::CreateAssetInput::default()
            })
            .expect("seed atom");
    }

    #[tokio::test]
    async fn query_returns_cited_hits_and_validates_input() {
        let mut state = test_state();
        let manager = Arc::new(kura_memory::Manager::new("test", None, None, None));
        seed_atom(&manager, "pnpm is the package manager for web projects");
        seed_atom(&manager, "lunch happens at noon");
        state.memory = Some(manager);

        let (status, json) = request_json(
            state.clone(),
            "POST",
            "/v1/retrieval/queries",
            Some(serde_json::json!({ "query": "which package manager" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let hits = json["hits"].as_array().expect("hits");
        assert!(!hits.is_empty());
        assert!(hits[0]["content"].as_str().unwrap().contains("pnpm"));
        assert_eq!(hits[0]["rank"], 1);
        assert_eq!(hits[0]["sourceLinks"][0]["id"], "thr_1", "citation intact");
        assert!(
            hits.iter()
                .all(|h| !h["content"].as_str().unwrap().contains("lunch")),
            "unrelated atom not recalled"
        );

        let (status, _) = request_json(
            state,
            "POST",
            "/v1/retrieval/queries",
            Some(serde_json::json!({ "query": "  " })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn query_cannot_name_another_tenant() {
        let mut state = test_state();
        state.memory = Some(Arc::new(kura_memory::Manager::new(
            "test", None, None, None,
        )));
        let (status, _) = request_json_as_tenant(
            state.clone(),
            "ten_a",
            "POST",
            "/v1/retrieval/queries",
            Some(serde_json::json!({ "query": "anything", "tenantId": "ten_b" })),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        let (status, _) = request_json_as_tenant(
            state,
            "ten_a",
            "POST",
            "/v1/retrieval/queries",
            Some(serde_json::json!({ "query": "anything", "tenantId": "ten_a" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "naming your own tenant still works");
    }
}
