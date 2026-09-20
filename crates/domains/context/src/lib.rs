//! kura-context — the default context-management policy engine.
//!
//! This crate backs the `context` builtin plugin, which attaches at
//! `chat/pre-dispatch` and injects the tenant's memory bootstrap (Ready L3
//! persona first, then Ready L2 scenarios, newest first) into the dispatch's
//! system frame — **deterministically, under an explicit budget, with the
//! citation inline**. Design root: TencentDB-Agent-Memory (L3/L2 bootstrap
//! under budgets; L1 atoms are never bulk-injected — they arrive via
//! drill-down or retrieval).
//!
//! Everything included or excluded is recorded in an [`AssemblyRecord`]:
//! nothing enters or misses the context silently. The record rides a
//! `context.assembled` event so an engineer can reconstruct exactly what
//! memory the model saw for any dispatch.
//!
//! Because this runs on the hook waterfall, other plugins modify the result
//! by design: the `session-strategy` plugin shapes the window after
//! injection (memory bootstrap messages are system-frame and survive
//! elision), and any later builtin or external hook may rewrite or veto.

use serde::{Deserialize, Serialize};

/// One candidate memory asset for bootstrap injection (a structural subset
/// of the memory plane's asset; this crate stays decoupled on purpose).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapAsset {
    pub asset_id: String,
    /// `l3` or `l2` (already filtered/ordered by the caller).
    pub layer: String,
    pub title: String,
    pub content: String,
}

/// Plugin configuration (the `config` object of the `context` entry in
/// `plugins.json`). Zero values fall back to defaults.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ContextConfig {
    /// Total character budget for injected bootstrap content (L3/L2).
    pub memory_budget_chars: usize,
    /// Character budget for query-time recall of L1 atoms.
    pub retrieval_budget_chars: usize,
    /// Non-system messages longer than this externalize to a memory ref
    /// (symbolic compression); 0 uses the default.
    pub ref_threshold_chars: usize,
    /// Upper bound on the retrieval corpus per turn; 0 uses
    /// [`RETRIEVAL_MAX_CORPUS`]. Tunable (Stage 6.1) so the self-improvement
    /// plane can propose it from a measured effect.
    pub retrieval_max_corpus: usize,
    /// Minimum cosine similarity for vector-only candidacy; 0 uses
    /// [`VECTOR_MIN_SIMILARITY`].
    pub vector_min_similarity: f32,
}

impl ContextConfig {
    #[must_use]
    pub fn max_corpus(&self) -> usize {
        if self.retrieval_max_corpus > 0 {
            self.retrieval_max_corpus
        } else {
            RETRIEVAL_MAX_CORPUS
        }
    }
    #[must_use]
    pub fn min_similarity(&self) -> f32 {
        if self.vector_min_similarity > 0.0 {
            self.vector_min_similarity
        } else {
            VECTOR_MIN_SIMILARITY
        }
    }
}

/// Default memory bootstrap budget (chars of asset content).
pub const DEFAULT_MEMORY_BUDGET_CHARS: usize = 4000;
/// Default retrieval budget (chars of recalled L1 content).
pub const DEFAULT_RETRIEVAL_BUDGET_CHARS: usize = 2000;
/// Maximum scored candidates considered per retrieval pass.
pub const RETRIEVAL_MAX_CANDIDATES: usize = 8;

/// Upper bound on the retrieval corpus for a single turn. The candidate set is
/// the tenant's Ready atoms, which grows without limit as memory accumulates;
/// scoring is linear in it and sits on the reply path. Beyond this many atoms
/// the newest are kept and the remainder is recorded as
/// `over_candidate_limit` — truncation is never silent.
pub const RETRIEVAL_MAX_CORPUS: usize = 2000;
/// Default externalization threshold for oversized message content.
pub const DEFAULT_REF_THRESHOLD_CHARS: usize = 8000;

impl ContextConfig {
    #[must_use]
    pub fn budget(&self) -> usize {
        if self.memory_budget_chars > 0 {
            self.memory_budget_chars
        } else {
            DEFAULT_MEMORY_BUDGET_CHARS
        }
    }

    #[must_use]
    pub fn retrieval_budget(&self) -> usize {
        if self.retrieval_budget_chars > 0 {
            self.retrieval_budget_chars
        } else {
            DEFAULT_RETRIEVAL_BUDGET_CHARS
        }
    }

    #[must_use]
    pub fn ref_threshold(&self) -> usize {
        if self.ref_threshold_chars > 0 {
            self.ref_threshold_chars
        } else {
            DEFAULT_REF_THRESHOLD_CHARS
        }
    }
}

/// A message to inject (role is always `system` for bootstrap content).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextMessage {
    pub role: String,
    pub content: String,
}

/// Default item source in the assembly record (bootstrap injection).
fn default_source() -> String {
    "bootstrap".to_string()
}

/// One included asset in the assembly record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncludedItem {
    pub asset_id: String,
    pub layer: String,
    pub chars: usize,
    /// `bootstrap` | `retrieval`.
    #[serde(default = "default_source")]
    pub source: String,
}

/// One excluded asset and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExcludedItem {
    pub asset_id: String,
    pub layer: String,
    /// `over_budget` | `empty_content` | `visibility`.
    pub reason: String,
    /// `bootstrap` | `retrieval`.
    #[serde(default = "default_source")]
    pub source: String,
}

/// The full decision record of one assembly pass.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssemblyRecord {
    pub included: Vec<IncludedItem>,
    pub excluded: Vec<ExcludedItem>,
    pub budget_chars: usize,
    pub used_chars: usize,
    #[serde(default)]
    pub retrieval_budget_chars: usize,
    #[serde(default)]
    pub retrieval_used_chars: usize,
}

impl AssemblyRecord {
    /// True when the pass neither included nor excluded anything (no memory
    /// to speak of — callers skip the event in that case).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.included.is_empty() && self.excluded.is_empty()
    }
}

/// Renders one asset as its injected system message: the citation (layer +
/// asset id) stays inline so recalled memory is evidence, never bare text.
#[must_use]
pub fn render_bootstrap_message(asset: &BootstrapAsset) -> String {
    let title = asset.title.trim();
    if title.is_empty() {
        format!(
            "Memory[{} {}]: {}",
            asset.layer,
            asset.asset_id,
            asset.content.trim()
        )
    } else {
        format!(
            "Memory[{} {}] {}: {}",
            asset.layer,
            asset.asset_id,
            title,
            asset.content.trim()
        )
    }
}

/// Assembles the bootstrap injection: walks `assets` in the caller's order
/// (L3 first, then L2 newest-first), including while the content budget
/// holds. Deterministic: same input → same output. Assets with empty
/// content are excluded (`empty_content`), assets past the budget are
/// excluded (`over_budget`) — the walk continues so a small L2 can still
/// fit after a large one was excluded.
#[must_use]
pub fn assemble(
    assets: &[BootstrapAsset],
    budget_chars: usize,
) -> (Vec<ContextMessage>, AssemblyRecord) {
    let mut messages = Vec::new();
    let mut record = AssemblyRecord {
        budget_chars,
        ..AssemblyRecord::default()
    };
    for asset in assets {
        let content_len = asset.content.trim().len();
        if content_len == 0 {
            record.excluded.push(ExcludedItem {
                asset_id: asset.asset_id.clone(),
                layer: asset.layer.clone(),
                reason: "empty_content".to_string(),
                source: default_source(),
            });
            continue;
        }
        if record.used_chars + content_len > budget_chars {
            record.excluded.push(ExcludedItem {
                asset_id: asset.asset_id.clone(),
                layer: asset.layer.clone(),
                reason: "over_budget".to_string(),
                source: default_source(),
            });
            continue;
        }
        record.used_chars += content_len;
        record.included.push(IncludedItem {
            asset_id: asset.asset_id.clone(),
            layer: asset.layer.clone(),
            chars: content_len,
            source: default_source(),
        });
        messages.push(ContextMessage {
            role: "system".to_string(),
            content: render_bootstrap_message(asset),
        });
    }
    (messages, record)
}

// ---------------------------------------------------------------------------
// Query-time retrieval (BM25 + recency, RRF fusion)
// ---------------------------------------------------------------------------

/// One retrieval candidate document. Callers pass the corpus **newest
/// first** — the index doubles as the recency rank.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetrievalDoc {
    pub asset_id: String,
    /// The asset's layer. Carried so the AssemblyRecord and the citation name
    /// the real layer: retrieval covers L1 atoms **and** the L2/L3 assets that
    /// the bootstrap budget could not fit, and reporting all of them as `l1`
    /// would make the record lie about what the model saw.
    pub layer: String,
    pub title: String,
    pub content: String,
}

/// Lowercased unicode-word tokens (deterministic, language-naive; CJK text
/// splits on non-alphanumeric boundaries, which is coarse but predictable —
/// a vector ranker joins the fusion later for semantic recall).
fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(ToString::to_string)
        .collect()
}

/// BM25 scores (k1=1.2, b=0.75) for `query` over `docs` (title + content).
/// Only docs with a positive score participate in retrieval.
fn bm25_scores(query: &str, docs: &[RetrievalDoc]) -> Vec<f64> {
    const K1: f64 = 1.2;
    const B: f64 = 0.75;
    let query_terms = tokenize(query);
    let doc_terms: Vec<Vec<String>> = docs
        .iter()
        .map(|d| tokenize(&format!("{} {}", d.title, d.content)))
        .collect();
    let n = docs.len() as f64;
    if n == 0.0 || query_terms.is_empty() {
        return vec![0.0; docs.len()];
    }
    let avgdl: f64 = doc_terms.iter().map(Vec::len).sum::<usize>() as f64 / n;
    let mut unique_terms: Vec<&String> = query_terms.iter().collect();
    unique_terms.sort();
    unique_terms.dedup();
    let mut scores = vec![0.0; docs.len()];
    for term in unique_terms {
        let df = doc_terms
            .iter()
            .filter(|terms| terms.contains(term))
            .count() as f64;
        if df == 0.0 {
            continue;
        }
        let idf = (1.0 + (n - df + 0.5) / (df + 0.5)).ln();
        for (i, terms) in doc_terms.iter().enumerate() {
            let tf = terms.iter().filter(|t| *t == term).count() as f64;
            if tf == 0.0 {
                continue;
            }
            let dl = terms.len() as f64;
            let denom = tf + K1 * (1.0 - B + B * dl / avgdl.max(1.0));
            scores[i] += idf * (tf * (K1 + 1.0)) / denom;
        }
    }
    scores
}

// ---------------------------------------------------------------------------
// Embedder seam (the vector ranker's provider)
// ---------------------------------------------------------------------------

/// The embedding seam: any provider producing fixed-dimension vectors can
/// serve the vector ranker. The default is the deterministic
/// [`HashedNgramEmbedder`]; a neural embedding provider replaces it through
/// this seam without touching the fusion.
pub trait Embedder: Send + Sync {
    fn embed(&self, text: &str) -> Vec<f32>;
    fn name(&self) -> &str;

    /// Cache key for vectors produced by this provider. Two providers that
    /// could ever disagree on a vector for the same text **must** report
    /// different fingerprints: cached vectors are compared by cosine, and
    /// mixing two spaces silently produces nonsense rather than an error.
    ///
    /// Defaults to [`Embedder::name`]. A provider whose model can change
    /// behind a stable name should fold that version into the fingerprint —
    /// otherwise a model swap serves stale vectors until the derived index is
    /// rebuilt.
    fn fingerprint(&self) -> String {
        self.name().to_string()
    }
}

/// Deterministic character-trigram feature-hashing embedder (256-dim FNV,
/// L2-normalized). Not a neural embedding — it is a *character-level*
/// lexical vector space, which is precisely what the word-based BM25
/// tokenizer lacks: CJK text and morphological variants overlap heavily on
/// character trigrams while sharing zero word tokens.
#[derive(Debug, Clone)]
pub struct HashedNgramEmbedder {
    dim: usize,
}

impl Default for HashedNgramEmbedder {
    fn default() -> Self {
        HashedNgramEmbedder { dim: 256 }
    }
}

impl HashedNgramEmbedder {
    fn hash(gram: &[char]) -> u64 {
        // FNV-1a over the chars' UTF-32 bytes: stable across platforms.
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for c in gram {
            for byte in (*c as u32).to_le_bytes() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        hash
    }
}

impl Embedder for HashedNgramEmbedder {
    fn embed(&self, text: &str) -> Vec<f32> {
        let chars: Vec<char> = text
            .to_lowercase()
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect();
        let mut vector = vec![0.0f32; self.dim];
        if chars.len() < 3 {
            return vector;
        }
        for gram in chars.windows(3) {
            let hash = Self::hash(gram);
            let idx = (hash % self.dim as u64) as usize;
            // Sign bit from a higher hash bit spreads mass over +/-
            // (standard feature hashing; reduces collision bias).
            let sign = if hash & (1 << 63) == 0 { 1.0 } else { -1.0 };
            vector[idx] += sign;
        }
        let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 0.0 {
            for v in &mut vector {
                *v /= norm;
            }
        }
        vector
    }

    fn name(&self) -> &str {
        "hashed-ngram-256"
    }

    fn fingerprint(&self) -> String {
        // Algorithm, gram size and dimension: every input that changes the
        // vector space is in the key.
        format!("hashed-ngram/3/{}/v1", self.dim)
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Minimum cosine similarity for vector-only candidacy: below this a doc
/// with no BM25 overlap stays out of the corpus (unrelated memory must not
/// leak into context on hash noise).
pub const VECTOR_MIN_SIMILARITY: f32 = 0.25;

/// Two-ranker retrieval (BM25 + recency), kept for callers without an
/// embedder. See [`retrieve_fused`].
#[must_use]
pub fn retrieve(query: &str, docs: &[RetrievalDoc]) -> Vec<usize> {
    retrieve_fused(query, docs, None)
}

/// Reciprocal-rank fusion (k=60) over up to three rankers — BM25 (positive
/// scores only), recency (the caller's newest-first order), and, when an
/// embedder is supplied, cosine similarity over its vectors (docs at or
/// above [`VECTOR_MIN_SIMILARITY`]). A doc is a candidate when the BM25 or
/// the vector ranker admits it; recency alone never recalls unrelated
/// memory. Returns candidate indices, best first, capped at
/// [`RETRIEVAL_MAX_CANDIDATES`]. Deterministic: ties break on asset id.
#[must_use]
pub fn retrieve_fused(
    query: &str,
    docs: &[RetrievalDoc],
    embedder: Option<&dyn Embedder>,
) -> Vec<usize> {
    let vectors: Option<Vec<Vec<f32>>> = embedder.map(|embedder| {
        docs.iter()
            .map(|d| embedder.embed(&format!("{} {}", d.title, d.content)))
            .collect()
    });
    retrieve_fused_with_vectors(query, docs, embedder, vectors.as_ref().map(Vec::as_slice))
}

/// [`retrieve_fused`] over **precomputed** document vectors.
///
/// This is the form callers on the reply path should use: the corpus vectors
/// come from the persisted derived index, so a turn embeds exactly one text —
/// the query. `doc_vectors`, when present, is positionally aligned with
/// `docs`; an empty vector at a position means "no usable embedding for this
/// doc", and that doc simply does not participate in the vector ranker.
#[must_use]
pub fn retrieve_fused_with_vectors(
    query: &str,
    docs: &[RetrievalDoc],
    embedder: Option<&dyn Embedder>,
    doc_vectors: Option<&[Vec<f32>]>,
) -> Vec<usize> {
    retrieve_fused_with_options(query, docs, embedder, doc_vectors, VECTOR_MIN_SIMILARITY)
}

/// [`retrieve_fused_with_vectors`] with an explicit vector-candidacy
/// threshold (the tunable behind `context.vectorMinSimilarity`).
#[must_use]
pub fn retrieve_fused_with_options(
    query: &str,
    docs: &[RetrievalDoc],
    embedder: Option<&dyn Embedder>,
    doc_vectors: Option<&[Vec<f32>]>,
    min_similarity: f32,
) -> Vec<usize> {
    const RRF_K: f64 = 60.0;
    let scores = bm25_scores(query, docs);
    let mut bm25_ranked: Vec<usize> = (0..docs.len()).filter(|&i| scores[i] > 0.0).collect();
    bm25_ranked.sort_by(|&a, &b| {
        scores[b]
            .partial_cmp(&scores[a])
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| docs[a].asset_id.cmp(&docs[b].asset_id))
    });

    let mut vector_ranked: Vec<usize> = Vec::new();
    if let (Some(embedder), Some(doc_vectors)) = (embedder, doc_vectors) {
        let query_vector = embedder.embed(query);
        if query_vector.iter().any(|v| *v != 0.0) {
            let similarities: Vec<f32> = docs
                .iter()
                .enumerate()
                .map(|(i, _)| match doc_vectors.get(i) {
                    Some(v) if !v.is_empty() => cosine(&query_vector, v),
                    // No cached vector for this doc: it stays a BM25-only
                    // candidate rather than scoring 0 against the query, which
                    // would be indistinguishable from "embedded and unrelated".
                    _ => f32::NEG_INFINITY,
                })
                .collect();
            vector_ranked = (0..docs.len())
                .filter(|&i| similarities[i] >= min_similarity)
                .collect();
            vector_ranked.sort_by(|&a, &b| {
                similarities[b]
                    .partial_cmp(&similarities[a])
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| docs[a].asset_id.cmp(&docs[b].asset_id))
            });
        }
    }

    // Rank lookup tables indexed by doc index. The previous form probed each
    // ranking with a linear `position()` scan inside the candidate loop, and
    // tested membership with `Vec::contains`, making the fusion O(n^2) over
    // the corpus; both are one table read here.
    let rank_table = |ranking: &[usize]| {
        let mut table = vec![None; docs.len()];
        for (rank, &idx) in ranking.iter().enumerate() {
            table[idx] = Some(rank);
        }
        table
    };
    let bm25_rank = rank_table(&bm25_ranked);
    let vector_rank = rank_table(&vector_ranked);

    // Candidates: admitted by BM25 or by the vector ranker. `bm25_ranked`
    // holds distinct indices, so "already a candidate" is exactly "ranked by
    // BM25".
    let mut candidates: Vec<usize> = bm25_ranked.clone();
    for &idx in &vector_ranked {
        if bm25_rank[idx].is_none() {
            candidates.push(idx);
        }
    }
    if candidates.is_empty() {
        return Vec::new();
    }

    let mut fused: Vec<(usize, f64)> = candidates
        .iter()
        .map(|&idx| {
            let mut rrf = 0.0;
            if let Some(rank) = bm25_rank[idx] {
                rrf += 1.0 / (RRF_K + rank as f64 + 1.0);
            }
            if let Some(rank) = vector_rank[idx] {
                rrf += 1.0 / (RRF_K + rank as f64 + 1.0);
            }
            // Recency rank = the caller's newest-first corpus index.
            rrf += 1.0 / (RRF_K + idx as f64 + 1.0);
            (idx, rrf)
        })
        .collect();
    fused.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| docs[a.0].asset_id.cmp(&docs[b.0].asset_id))
    });
    fused.truncate(RETRIEVAL_MAX_CANDIDATES);
    fused.into_iter().map(|(idx, _)| idx).collect()
}

/// Renders one recalled atom as its injected system message.
#[must_use]
pub fn render_recall_message(doc: &RetrievalDoc) -> String {
    let title = doc.title.trim();
    let layer = if doc.layer.trim().is_empty() {
        "l1"
    } else {
        doc.layer.trim()
    };
    if title.is_empty() {
        format!(
            "Memory[{layer} {}] (recalled): {}",
            doc.asset_id,
            doc.content.trim()
        )
    } else {
        format!(
            "Memory[{layer} {}] {} (recalled): {}",
            doc.asset_id,
            title,
            doc.content.trim()
        )
    }
}

/// Runs retrieval for `query` over `docs` (newest first) and walks the
/// fused candidates under `budget_chars`, appending to `messages` and
/// `record` with `source: "retrieval"`. Deterministic.
pub fn retrieve_and_assemble(
    query: &str,
    docs: &[RetrievalDoc],
    embedder: Option<&dyn Embedder>,
    budget_chars: usize,
    messages: &mut Vec<ContextMessage>,
    record: &mut AssemblyRecord,
) {
    retrieve_and_assemble_with_vectors(query, docs, embedder, None, budget_chars, messages, record);
}

/// [`retrieve_and_assemble`] over precomputed document vectors. See
/// [`retrieve_fused_with_vectors`].
pub fn retrieve_and_assemble_with_vectors(
    query: &str,
    docs: &[RetrievalDoc],
    embedder: Option<&dyn Embedder>,
    doc_vectors: Option<&[Vec<f32>]>,
    budget_chars: usize,
    messages: &mut Vec<ContextMessage>,
    record: &mut AssemblyRecord,
) {
    retrieve_and_assemble_with_options(
        query,
        docs,
        embedder,
        doc_vectors,
        budget_chars,
        VECTOR_MIN_SIMILARITY,
        messages,
        record,
    );
}

/// [`retrieve_and_assemble_with_vectors`] with an explicit similarity
/// threshold.
#[allow(clippy::too_many_arguments)]
pub fn retrieve_and_assemble_with_options(
    query: &str,
    docs: &[RetrievalDoc],
    embedder: Option<&dyn Embedder>,
    doc_vectors: Option<&[Vec<f32>]>,
    budget_chars: usize,
    min_similarity: f32,
    messages: &mut Vec<ContextMessage>,
    record: &mut AssemblyRecord,
) {
    record.retrieval_budget_chars = budget_chars;
    let ranked = match doc_vectors {
        Some(vectors) => {
            retrieve_fused_with_options(query, docs, embedder, Some(vectors), min_similarity)
        }
        None => retrieve_fused(query, docs, embedder),
    };
    for idx in ranked {
        let doc = &docs[idx];
        let content_len = doc.content.trim().len();
        if content_len == 0 {
            continue;
        }
        if record.retrieval_used_chars + content_len > budget_chars {
            record.excluded.push(ExcludedItem {
                asset_id: doc.asset_id.clone(),
                layer: doc.layer.clone(),
                reason: "over_budget".to_string(),
                source: "retrieval".to_string(),
            });
            continue;
        }
        record.retrieval_used_chars += content_len;
        record.included.push(IncludedItem {
            asset_id: doc.asset_id.clone(),
            layer: doc.layer.clone(),
            chars: content_len,
            source: "retrieval".to_string(),
        });
        messages.push(ContextMessage {
            role: "system".to_string(),
            content: render_recall_message(doc),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(id: &str, layer: &str, content: &str) -> BootstrapAsset {
        BootstrapAsset {
            asset_id: id.to_string(),
            layer: layer.to_string(),
            title: String::new(),
            content: content.to_string(),
        }
    }

    #[test]
    fn includes_in_order_under_budget_with_citations() {
        let assets = vec![
            BootstrapAsset {
                title: "persona".to_string(),
                ..asset("mem_l3", "l3", "Chinese-speaking operator")
            },
            asset("mem_l2a", "l2", "prefers pnpm"),
        ];
        let (messages, record) = assemble(&assets, 1000);
        assert_eq!(messages.len(), 2);
        assert_eq!(
            messages[0].content,
            "Memory[l3 mem_l3] persona: Chinese-speaking operator"
        );
        assert_eq!(messages[1].content, "Memory[l2 mem_l2a]: prefers pnpm");
        assert!(messages.iter().all(|m| m.role == "system"));
        assert_eq!(record.included.len(), 2);
        assert!(record.excluded.is_empty());
        assert_eq!(
            record.used_chars,
            "Chinese-speaking operator".len() + "prefers pnpm".len()
        );
    }

    #[test]
    fn over_budget_excludes_with_reason_but_keeps_walking() {
        let assets = vec![
            asset("big", "l3", &"x".repeat(100)),
            asset("small", "l2", "tiny"),
        ];
        let (messages, record) = assemble(&assets, 50);
        assert_eq!(
            messages.len(),
            1,
            "the small asset still fits after the big one missed"
        );
        assert!(messages[0].content.contains("small"));
        assert_eq!(record.excluded.len(), 1);
        assert_eq!(record.excluded[0].asset_id, "big");
        assert_eq!(record.excluded[0].reason, "over_budget");
    }

    #[test]
    fn empty_content_is_excluded_and_empty_pass_is_flagged() {
        let (messages, record) = assemble(&[asset("void", "l2", "   ")], 100);
        assert!(messages.is_empty());
        assert_eq!(record.excluded[0].reason, "empty_content");
        assert!(!record.is_empty());
        let (_, record) = assemble(&[], 100);
        assert!(record.is_empty());
    }

    fn doc(id: &str, content: &str) -> RetrievalDoc {
        RetrievalDoc {
            asset_id: id.to_string(),
            layer: "l1".to_string(),
            title: String::new(),
            content: content.to_string(),
        }
    }

    #[test]
    fn bm25_ranks_term_overlap_and_zero_score_never_recalls() {
        let docs = vec![
            doc("mem_rust", "prefers rust for backend daemons"),
            doc("mem_lunch", "eats lunch at noon"),
            doc("mem_rustfmt", "rust formatting uses rustfmt defaults"),
        ];
        let picked = retrieve("which rust formatter do we use", &docs);
        assert!(!picked.is_empty());
        // Both rust docs are candidates; the lunch doc never appears.
        assert!(picked.iter().all(|&i| docs[i].asset_id != "mem_lunch"));
        assert!(
            picked.contains(&2),
            "doc with both `rust` and `formatting` terms recalled"
        );

        // No lexical overlap at all: nothing is recalled (recency alone
        // never pulls unrelated memory in).
        assert!(retrieve("完全无关的查询", &docs).is_empty());
    }

    #[test]
    fn rrf_fusion_prefers_recent_on_equal_relevance_and_is_deterministic() {
        // Same content => same BM25 score; the newer doc (lower corpus
        // index) must win via the recency ranker.
        let docs = vec![
            doc("mem_new", "deploy checklist"),
            doc("mem_old", "deploy checklist"),
        ];
        let picked = retrieve("deploy checklist", &docs);
        assert_eq!(picked[0], 0, "newest-first corpus index wins on ties");
        assert_eq!(retrieve("deploy checklist", &docs), picked);
    }

    #[test]
    fn retrieve_and_assemble_records_source_and_budget() {
        let docs = vec![
            doc("mem_a", "pnpm is the package manager"),
            doc("mem_b", &format!("pnpm workspaces {}", "x".repeat(100))),
        ];
        let mut messages = Vec::new();
        let mut record = AssemblyRecord::default();
        retrieve_and_assemble("pnpm", &docs, None, 50, &mut messages, &mut record);
        assert_eq!(messages.len(), 1);
        assert!(messages[0].content.contains("Memory[l1 mem_a] (recalled):"));
        assert_eq!(record.included.len(), 1);
        assert_eq!(record.included[0].source, "retrieval");
        assert_eq!(record.excluded.len(), 1);
        assert_eq!(record.excluded[0].reason, "over_budget");
        assert_eq!(record.excluded[0].source, "retrieval");
        assert_eq!(record.retrieval_budget_chars, 50);
    }

    #[test]
    fn vector_ranker_recalls_cjk_that_bm25_misses() {
        let docs = vec![
            doc("mem_lang", "中文回复偏好：所有回答使用中文"),
            doc("mem_deploy", "docker compose up runs the stack"),
        ];
        // Word-level BM25 sees no shared token ("用中文回复" vs the doc's
        // longer phrases), so the two-ranker path recalls nothing…
        assert!(retrieve("请用中文回复我", &docs).is_empty());
        // …while character-trigram vectors overlap heavily.
        let embedder = HashedNgramEmbedder::default();
        let picked = retrieve_fused("请用中文回复我", &docs, Some(&embedder));
        assert_eq!(picked, vec![0], "CJK doc recalled, unrelated doc stays out");
    }

    #[test]
    fn vector_similarity_threshold_blocks_unrelated_docs() {
        let embedder = HashedNgramEmbedder::default();
        let docs = vec![doc("mem_weather", "今天天气不错适合散步")];
        assert!(
            retrieve_fused("docker deployment pipeline", &docs, Some(&embedder)).is_empty(),
            "no lexical or character overlap -> nothing recalled"
        );
        // Embedding determinism.
        assert_eq!(embedder.embed("中文回复"), embedder.embed("中文回复"));
        assert_eq!(embedder.name(), "hashed-ngram-256");
    }

    #[test]
    fn fusion_keeps_bm25_results_and_is_deterministic_with_embedder() {
        let embedder = HashedNgramEmbedder::default();
        let docs = vec![
            doc("mem_rust", "prefers rust for backend daemons"),
            doc("mem_lunch", "eats lunch at noon"),
        ];
        let picked = retrieve_fused("rust backend", &docs, Some(&embedder));
        assert_eq!(picked[0], 0);
        assert!(picked.iter().all(|&i| docs[i].asset_id != "mem_lunch"));
        assert_eq!(
            retrieve_fused("rust backend", &docs, Some(&embedder)),
            picked
        );
    }

    #[test]
    fn deterministic_and_config_defaults() {
        let assets = vec![asset("a", "l3", "one"), asset("b", "l2", "two")];
        assert_eq!(assemble(&assets, 100), assemble(&assets, 100));
        assert_eq!(
            ContextConfig::default().budget(),
            DEFAULT_MEMORY_BUDGET_CHARS
        );
        let parsed: ContextConfig =
            serde_json::from_value(serde_json::json!({ "memoryBudgetChars": 123 })).expect("parse");
        assert_eq!(parsed.budget(), 123);
    }

    /// The point of the precomputed-vector form: a turn embeds the query and
    /// nothing else. Previously the corpus was re-embedded on every turn —
    /// one call per doc per turn, which for an external provider is one RPC
    /// per doc, synchronously on the reply path.
    #[test]
    fn precomputed_vectors_embed_only_the_query() {
        // `Embedder` is Send + Sync, so the call log needs a Mutex rather
        // than a RefCell.
        struct CountingSync {
            inner: HashedNgramEmbedder,
            calls: std::sync::Mutex<Vec<String>>,
        }
        impl Embedder for CountingSync {
            fn embed(&self, text: &str) -> Vec<f32> {
                self.calls.lock().expect("lock").push(text.to_string());
                self.inner.embed(text)
            }
            fn name(&self) -> &str {
                "counting"
            }
        }
        let docs = vec![
            doc("mem_1", "replies in Chinese"),
            doc("mem_2", "prefers dark mode"),
            doc("mem_3", "works in Shanghai"),
        ];
        let embedder = CountingSync {
            inner: HashedNgramEmbedder::default(),
            calls: std::sync::Mutex::new(Vec::new()),
        };
        let plain = HashedNgramEmbedder::default();
        let vectors: Vec<Vec<f32>> = docs
            .iter()
            .map(|d| plain.embed(&format!("{} {}", d.title, d.content)))
            .collect();

        let ranked = retrieve_fused_with_vectors(
            "what language should replies use",
            &docs,
            Some(&embedder),
            Some(&vectors),
        );
        assert!(!ranked.is_empty(), "the query still recalls");
        let calls = embedder.calls.lock().expect("lock");
        assert_eq!(
            calls.len(),
            1,
            "exactly one embed call for a warm corpus, got {calls:?}"
        );
        assert_eq!(calls[0], "what language should replies use");
    }

    /// A doc with no cached vector must not be scored as "embedded and
    /// unrelated" — it stays a BM25-only candidate instead of being ranked
    /// against a zero vector.
    #[test]
    fn a_missing_vector_does_not_become_a_zero_similarity() {
        let docs = vec![
            doc("mem_1", "replies in Chinese"),
            doc("mem_2", "unrelated text"),
        ];
        let plain = HashedNgramEmbedder::default();
        let vectors = vec![
            Vec::new(), // cache miss for mem_1
            plain.embed("unrelated text"),
        ];
        let ranked =
            retrieve_fused_with_vectors("Chinese replies", &docs, Some(&plain), Some(&vectors));
        // mem_1 has lexical overlap, so BM25 still admits it despite the miss.
        assert!(
            ranked.contains(&0),
            "BM25 still recalls a doc with no vector"
        );
    }

    /// Stage 1.4: the corpus is no longer L1-only, so the record and the
    /// citation must name the asset's real layer. Reporting an L2 scenario as
    /// `l1` would make the AssemblyRecord lie about what the model saw.
    #[test]
    fn a_recalled_l2_asset_is_cited_and_recorded_as_l2() {
        let docs = vec![RetrievalDoc {
            asset_id: "mem_l2".to_string(),
            layer: "l2".to_string(),
            title: "deployment workflow".to_string(),
            content: "the operator deploys on Fridays".to_string(),
        }];
        let mut messages = Vec::new();
        let mut record = AssemblyRecord::default();
        retrieve_and_assemble(
            "deploys on Fridays",
            &docs,
            None,
            4000,
            &mut messages,
            &mut record,
        );

        assert_eq!(record.included.len(), 1, "{record:?}");
        assert_eq!(record.included[0].layer, "l2");
        assert_eq!(record.included[0].source, "retrieval");
        assert!(
            messages[0].content.contains("Memory[l2 mem_l2]"),
            "the citation names the real layer: {}",
            messages[0].content
        );
    }

    /// An over-budget exclusion must also carry the real layer.
    #[test]
    fn an_over_budget_exclusion_names_the_real_layer() {
        let docs = vec![RetrievalDoc {
            asset_id: "mem_l3".to_string(),
            layer: "l3".to_string(),
            title: "persona".to_string(),
            content: "a very long persona ".repeat(50),
        }];
        let mut messages = Vec::new();
        let mut record = AssemblyRecord::default();
        retrieve_and_assemble("persona", &docs, None, 10, &mut messages, &mut record);

        assert!(messages.is_empty());
        let excluded = record
            .excluded
            .iter()
            .find(|item| item.asset_id == "mem_l3")
            .expect("recorded");
        assert_eq!(excluded.layer, "l3");
        assert_eq!(excluded.reason, "over_budget");
    }
}
