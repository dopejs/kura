//! `/v1/skills/proposals` — agent-managed skills (design root: skills are
//! memory assets in the 058 envelope, kind=skill).
//!
//! The proposal lifecycle **rides the memory plane's governance wholesale**:
//! a proposal is a `kind=skill`, `layer=l1` memory asset — the L1 validator
//! forces motivating evidence (source links), and the default write policy
//! forces agent-authored writes into `Pending`, which lands the proposal in
//! the existing memory review queue. Approval is the existing
//! `/v1/memory/assets/{id}/approve`. What this family adds is the
//! **publication bridge**: only an approved (Ready) proposal can publish,
//! and publication is the sole path into the skills registry — it writes
//! `<data_dir>/skills/<id>/SKILL.md`, registers a catalog item (kind=skill,
//! Community trust), and reloads the registry. Unpublished proposals are
//! never loadable (the registry only scans the skills dir), which is the
//! runtime guard the design fixes.

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::routing::{get, post};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use kura_memory as memory;

use crate::error::ApiError;
use crate::middleware::TenantContext;
use crate::response::Json;
use crate::state::AppState;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SkillProposalRequest {
    pub name: String,
    pub description: String,
    /// The skill instruction body (SKILL.md content below the frontmatter).
    pub body: String,
    pub tenant_id: String,
    /// Motivating evidence (conversation/run provenance). Required — the L1
    /// validator rejects proposals without it.
    pub evidence_links: Vec<memory::SourceLink>,
    /// Defaults to an agent actor; operator-authored proposals may say so.
    pub proposed_by: Option<memory::Actor>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillProposalResponse {
    pub asset: memory::MemoryAsset,
    pub decision: memory::WriteDecision,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillPublishResponse {
    pub skill_id: String,
    pub catalog_item_id: String,
    pub version: String,
}

/// Lowercase-kebab skill id from the proposal name (mirrors the registry's
/// normalization closely enough for directory naming).
fn skill_dir_id(name: &str) -> String {
    let id: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    id.trim_matches('-').to_string()
}

/// POST /v1/skills/proposals — draft a skill as a governed memory asset.
#[allow(clippy::unused_async)]
pub async fn propose(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<Json<SkillProposalResponse>, ApiError> {
    let request: SkillProposalRequest = super::decode_json_required(&body)?;
    if request.name.trim().is_empty() || request.body.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "name and body are required".to_string(),
        ));
    }
    if skill_dir_id(&request.name).is_empty() {
        return Err(ApiError::BadRequest(
            "name yields an empty skill id".to_string(),
        ));
    }
    let manager = state
        .memory
        .as_deref()
        .ok_or_else(|| ApiError::internal("memory manager is not configured"))?;
    let tenant_id = super::memory::route_tenant(&tenant, &request.tenant_id);
    let owner = request.proposed_by.clone().unwrap_or(memory::Actor {
        kind: memory::ActorKind::Agent,
        id: "agent".to_string(),
    });
    // The asset content IS the SKILL.md the publish step writes verbatim.
    let content = format!(
        "---\nname: {}\ndescription: {}\n---\n\n{}\n",
        request.name.trim(),
        request.description.trim(),
        request.body.trim()
    );
    let (asset, decision) = manager
        .create(memory::CreateAssetInput {
            kind: memory::AssetKind::Skill,
            layer: memory::MemoryLayer::L1,
            tenant_id,
            owner,
            visibility: memory::Visibility::Private,
            atom_type: Some(memory::AtomType::Reference),
            title: request.name.trim().to_string(),
            content,
            source_links: request.evidence_links,
            ..memory::CreateAssetInput::default()
        })
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    super::memory::persist_capture(&state, &asset);
    Ok(Json(SkillProposalResponse { asset, decision }))
}

/// GET /v1/skills/proposals — every skill-kind asset for the tenant.
#[allow(clippy::unused_async)]
pub async fn list(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let manager = state
        .memory
        .as_deref()
        .ok_or_else(|| ApiError::internal("memory manager is not configured"))?;
    let tenant_id = super::memory::route_tenant(&tenant, "");
    let mut items = manager.list(&tenant_id, Some(memory::MemoryLayer::L1), None);
    items.retain(|asset| asset.kind == memory::AssetKind::Skill);
    Ok(Json(serde_json::json!({ "items": items })))
}

/// POST /v1/skills/proposals/{asset_id}/publish — the only path from an
/// approved proposal into the runtime: skills dir + catalog + registry.
#[allow(clippy::unused_async)]
pub async fn publish(
    State(state): State<AppState>,
    Path(asset_id): Path<String>,
) -> Result<Json<SkillPublishResponse>, ApiError> {
    let manager = state
        .memory
        .as_deref()
        .ok_or_else(|| ApiError::internal("memory manager is not configured"))?;
    let Some(asset) = manager.get(&asset_id) else {
        return Err(ApiError::NotFound("skill proposal not found".to_string()));
    };
    if asset.kind != memory::AssetKind::Skill {
        return Err(ApiError::BadRequest(
            "asset is not a skill proposal".to_string(),
        ));
    }
    if asset.status != memory::AssetStatus::Ready {
        return Err(ApiError::Conflict(format!(
            "proposal is {}; only approved (ready) proposals publish",
            asset.status.as_str()
        )));
    }
    let skills = state
        .skills
        .as_deref()
        .ok_or_else(|| ApiError::internal("skills registry is not configured"))?;
    let catalog = state
        .catalog
        .as_deref()
        .ok_or_else(|| ApiError::internal("catalog manager is not configured"))?;

    // Directory name is kebab; the registry's runtime id is the normalized
    // (lowercased) frontmatter name — the response carries the registry id.
    let dir_id = skill_dir_id(&asset.title);
    let skill_id = asset.title.trim().to_lowercase();
    let dir = std::path::Path::new(&state.config.data_dir)
        .join("skills")
        .join(&dir_id);
    // Stage 3.3: republishing supersedes. The proposal asset already carries
    // its version chain; the on-disk bundle now carries the same provenance
    // in its frontmatter, so a SKILL.md can always be traced to the approved
    // asset it came from and to the one it replaced.
    let previous = std::fs::read_to_string(dir.join("SKILL.md")).ok();
    let previous_provenance = previous.as_deref().and_then(|prev| {
        prev.lines().find_map(|l| {
            l.strip_prefix("provenance: ")
                .map(str::trim)
                .map(String::from)
        })
    });
    // Version: the asset's own version when it is a supersede in the memory
    // plane; otherwise the next major after what the catalog already holds,
    // so two independently approved proposals for one skill never collide.
    let existing = catalog.list_items().into_iter().find(|i| {
        i.kind == kura_catalog::ItemKind::Skill
            && i.name.trim().eq_ignore_ascii_case(asset.title.trim())
    });
    let major =
        (asset.version.max(1) as usize).max(existing.as_ref().map_or(0, |i| i.versions.len()) + 1);
    let version = format!("{major}.0.0");
    let content = with_provenance_frontmatter(
        &asset.content,
        &asset.asset_id,
        &version,
        previous_provenance.as_deref(),
    );
    std::fs::create_dir_all(&dir)
        .and_then(|()| std::fs::write(dir.join("SKILL.md"), content.as_bytes()))
        .map_err(|err| ApiError::internal(&format!("write skill bundle: {err}")))?;

    // One catalog item per skill: a republish appends a version rather than
    // registering a second item with the same name.
    let now = Utc::now();
    let new_version = kura_catalog::Version {
        version: version.clone(),
        source: format!("memory:{}", asset.asset_id),
        checksum: String::new(),
        requirements: Vec::new(),
        published_at: now,
    };
    let item = match existing {
        Some(mut item) => {
            item.versions.retain(|v| v.version != version);
            item.versions.push(new_version);
            item.updated_at = now;
            catalog
                .register_item(item)
                .map_err(|err| ApiError::internal(&format!("catalog supersede: {err}")))?
        }
        None => catalog
            .register_item(kura_catalog::CatalogItem {
                item_id: String::new(),
                kind: kura_catalog::ItemKind::Skill,
                name: asset.title.clone(),
                trust_tier: kura_catalog::TrustTier::Community,
                permissions: Vec::new(),
                versions: vec![new_version],
                created_at: now,
                updated_at: now,
            })
            .map_err(|err| ApiError::internal(&format!("catalog register: {err}")))?,
    };
    skills
        .reload()
        .map_err(|err| ApiError::internal(&format!("skills reload: {err}")))?;

    // Audit event: the publication decision with its evidence chain.
    let mut payload = serde_json::Map::new();
    payload.insert("assetId".to_string(), serde_json::json!(asset.asset_id));
    payload.insert("skillId".to_string(), serde_json::json!(skill_id));
    payload.insert("catalogItemId".to_string(), serde_json::json!(item.item_id));
    payload.insert("version".to_string(), serde_json::json!(version));
    if let Some(prev) = &previous_provenance {
        payload.insert("supersedes".to_string(), serde_json::json!(prev));
    }
    let event = kura_events::Event {
        category: "skill".to_string(),
        name: "skill.proposal_published".to_string(),
        resource: kura_events::Resource {
            kind: "skill".to_string(),
            id: skill_id.clone(),
        },
        payload,
        ..kura_events::Event::default()
    };
    let event = state.store.lock().append_event(&event).unwrap_or(event);
    state.event_bus.publish(event);

    Ok(Json(SkillPublishResponse {
        skill_id,
        catalog_item_id: item.item_id,
        version,
    }))
}

/// Rewrites the SKILL.md frontmatter to carry `provenance: memory:<asset>`,
/// `version`, and — on a republish — `supersedes: <previous provenance>`.
fn with_provenance_frontmatter(
    content: &str,
    asset_id: &str,
    version: &str,
    supersedes: Option<&str>,
) -> String {
    let mut extra = format!("provenance: memory:{asset_id}\nversion: {version}\n");
    if let Some(prev) = supersedes.filter(|p| !p.is_empty() && *p != format!("memory:{asset_id}")) {
        extra.push_str(&format!("supersedes: {prev}\n"));
    }
    if let Some(rest) = content.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---") {
            let (front, tail) = rest.split_at(end);
            // Drop any provenance/version/supersedes lines a prior publish wrote.
            let front: String = front
                .lines()
                .filter(|l| {
                    !(l.starts_with("provenance:")
                        || l.starts_with("version:")
                        || l.starts_with("supersedes:"))
                })
                .map(|l| format!("{l}\n"))
                .collect();
            return format!("---\n{front}{extra}{}", tail.trim_start_matches('\n'));
        }
    }
    format!("---\n{extra}---\n\n{content}")
}

// ---------------------------------------------------------------------------
// 3.1 Distillation, 3.2 search, 3.4 usage
// ---------------------------------------------------------------------------

pub const DOC_KIND_SKILL_USAGE: &str = "skill_usage";

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DistillRequest {
    pub thread_id: String,
    pub tenant_id: String,
    /// Steering from the operator, folded into the distillation prompt.
    pub guidance: String,
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DistillResponse {
    /// The thread the distillation turn ran on. It is an ordinary chat
    /// thread: the operator can read it and reply to steer a second pass.
    pub distill_thread_id: String,
    pub dispatch_id: String,
    /// Present when the model's answer parsed into a proposal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposal: Option<SkillProposalResponse>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub parse_error: String,
}

/// The distillation instruction. The model is asked for one JSON object; the
/// field names are the proposal's.
pub const DISTILL_INSTRUCTION: &str = "You are distilling a reusable skill from the conversation below. \
Answer with exactly one JSON object and nothing else, of the form \
{\"name\": \"<short kebab-case name>\", \"description\": \"<one line>\", \"body\": \"<step-by-step instructions the agent should follow next time>\"}.";

/// Finds the first balanced `{...}` in `text` and parses it.
fn parse_skill_draft(text: &str) -> Result<(String, String, String), String> {
    let start = text.find('{').ok_or("no JSON object in the model output")?;
    let mut depth = 0i32;
    let mut end = None;
    for (i, ch) in text[start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(start + i + 1);
                    break;
                }
            }
            _ => {}
        }
    }
    let end = end.ok_or("unbalanced JSON object in the model output")?;
    let value: serde_json::Value =
        serde_json::from_str(&text[start..end]).map_err(|e| e.to_string())?;
    let get = |k: &str| {
        value
            .get(k)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    let (name, description, body) = (get("name"), get("description"), get("body"));
    if name.is_empty() || body.is_empty() {
        return Err("name and body are required in the draft".into());
    }
    Ok((name, description, body))
}

/// POST /v1/skills/proposals/distill — Stage 3.1.
///
/// Distillation runs **as a chat turn** on its own thread rather than as a
/// background job: the source conversation is loaded from continuity, the
/// instruction is the query, and the resulting dispatch is persisted like
/// any turn — visible, auditable, and steerable (reply on the distill thread
/// and call again with `guidance`). The parsed draft becomes an ordinary
/// Pending proposal, so review and publication are unchanged.
pub async fn distill(
    State(state): State<AppState>,
    tenant: Option<Extension<TenantContext>>,
    body: Bytes,
) -> Result<Json<DistillResponse>, ApiError> {
    let request: DistillRequest = super::decode_json_required(&body)?;
    let thread_id = request.thread_id.trim().to_string();
    if thread_id.is_empty() {
        return Err(ApiError::BadRequest("threadId is required".into()));
    }
    let tenant_id = super::memory::route_tenant(&tenant, &request.tenant_id);
    let chat = state
        .chat
        .clone()
        .ok_or_else(|| ApiError::internal("chat service is not configured"))?;

    // The source conversation, from continuity (safe content only).
    let turns = {
        let store = state.store.lock();
        let thread = store
            .get_thread_for_tenant(&tenant_id, &thread_id)
            .map_err(ApiError::from_store)?;
        match thread {
            Some(t) if !t.current_session_segment_id.trim().is_empty() => store
                .list_continuity_turns(&kura_store::thread_continuity::ContinuityLookupQuery {
                    tenant_id: tenant_id.clone(),
                    thread_id: thread_id.clone(),
                    session_segment_id: t.current_session_segment_id.clone(),
                    limit: 200,
                    now: Some(Utc::now()),
                })
                .map_err(ApiError::from_store)?,
            _ => Vec::new(),
        }
    };
    let mut transcript = String::new();
    for turn in turns.iter().rev() {
        let role = match turn.role {
            kura_threads::ContinuityRole::User => "user",
            kura_threads::ContinuityRole::Assistant => "assistant",
        };
        transcript.push_str(&format!("{role}: {}\n", turn.safe_content.trim()));
    }
    // Guidance goes first: it is the operator steering the pass, and it may
    // be as concrete as a corrected draft the model should adopt verbatim.
    let mut query = String::new();
    if !request.guidance.trim().is_empty() {
        query.push_str(&format!(
            "Operator guidance: {}\n\n",
            request.guidance.trim()
        ));
    }
    query.push_str(&format!(
        "{DISTILL_INSTRUCTION}\n\nConversation (thread {thread_id}):\n{transcript}"
    ));

    let distill_thread_id = format!("skill-distill:{thread_id}");
    let input = kura_chat::QueryInput {
        query,
        provider: request.provider,
        model: request.model,
        tenant_id: tenant_id.clone(),
        thread_id: distill_thread_id.clone(),
        ..Default::default()
    };
    // `chat.query` drives its own bridge runtime and blocks; it cannot run on
    // the axum task directly (the chat route uses the same pattern).
    let execution = tokio::task::spawn_blocking(move || {
        chat.query(input, &kura_chat::CancellationToken::new())
    })
    .await
    .map_err(|err| ApiError::internal(&format!("distillation task: {err}")))?
    .map_err(|err| ApiError::internal(&format!("distillation turn: {err}")))?;
    let dispatch_id = execution.result.dispatch.dispatch_id.clone();
    if let Some(err) = execution.exec_error {
        return Ok(Json(DistillResponse {
            distill_thread_id,
            dispatch_id,
            proposal: None,
            parse_error: err.to_string(),
        }));
    }

    let (name, description, body) = match parse_skill_draft(&execution.result.dispatch.output) {
        Ok(draft) => draft,
        Err(parse_error) => {
            return Ok(Json(DistillResponse {
                distill_thread_id,
                dispatch_id,
                proposal: None,
                parse_error,
            }));
        }
    };
    if skill_dir_id(&name).is_empty() {
        return Ok(Json(DistillResponse {
            distill_thread_id,
            dispatch_id,
            proposal: None,
            parse_error: "draft name yields an empty skill id".into(),
        }));
    }
    let manager = state
        .memory
        .as_deref()
        .ok_or_else(|| ApiError::internal("memory manager is not configured"))?;
    let content = format!("---\nname: {name}\ndescription: {description}\n---\n\n{body}\n");
    let (asset, decision) = manager
        .create(memory::CreateAssetInput {
            kind: memory::AssetKind::Skill,
            layer: memory::MemoryLayer::L1,
            tenant_id,
            owner: memory::Actor {
                kind: memory::ActorKind::Agent,
                id: "skill-distiller".into(),
            },
            visibility: memory::Visibility::Private,
            atom_type: Some(memory::AtomType::Reference),
            title: name,
            content,
            source_links: vec![
                memory::SourceLink {
                    kind: memory::SourceKind::Thread,
                    id: thread_id,
                    ..memory::SourceLink::default()
                },
                memory::SourceLink {
                    kind: memory::SourceKind::Thread,
                    id: distill_thread_id.clone(),
                    ..memory::SourceLink::default()
                },
            ],
            ..memory::CreateAssetInput::default()
        })
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    super::memory::persist_capture(&state, &asset);
    Ok(Json(DistillResponse {
        distill_thread_id,
        dispatch_id,
        proposal: Some(SkillProposalResponse { asset, decision }),
        parse_error: String::new(),
    }))
}

/// Per-skill usage record (Stage 3.4). Invocations are counted automatically
/// from `chat/turn-end`; `helpful`/`corrected` are explicit feedback, because
/// inferring a correction from the next message is a guess and this record
/// is meant to be trusted.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SkillUsage {
    pub skill_id: String,
    pub invocations: u64,
    pub helpful: u64,
    pub corrected: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<chrono::DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_feedback_at: Option<chrono::DateTime<Utc>>,
}

pub fn load_usage(state: &AppState, skill_id: &str) -> SkillUsage {
    let store = state.store.lock();
    kura_store::list_documents::<SkillUsage>(&store, DOC_KIND_SKILL_USAGE)
        .unwrap_or_default()
        .into_iter()
        .find(|u| u.skill_id == skill_id)
        .unwrap_or_else(|| SkillUsage {
            skill_id: skill_id.to_string(),
            ..Default::default()
        })
}

fn save_usage(state: &AppState, usage: &SkillUsage) {
    let store = state.store.lock();
    if let Err(err) =
        kura_store::put_document(&store, DOC_KIND_SKILL_USAGE, &usage.skill_id, "", "", usage)
    {
        eprintln!(
            "[kura] skills: persisting usage for {} failed: {err}",
            usage.skill_id
        );
    }
}

/// Called by the turn-end hook for every skill a turn selected.
pub fn record_invocation(state: &AppState, skill_id: &str) {
    let mut usage = load_usage(state, skill_id);
    usage.invocations += 1;
    usage.last_used_at = Some(Utc::now());
    save_usage(state, &usage);
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct FeedbackRequest {
    outcome: String,
}

/// POST /v1/skills/{skill_id}/feedback — `{"outcome": "helpful" | "corrected"}`.
async fn feedback(
    State(state): State<AppState>,
    Path(skill_id): Path<String>,
    body: Bytes,
) -> Result<Json<SkillUsage>, ApiError> {
    let request: FeedbackRequest = super::decode_json_required(&body)?;
    let mut usage = load_usage(&state, skill_id.trim());
    match request.outcome.trim() {
        "helpful" => usage.helpful += 1,
        "corrected" => usage.corrected += 1,
        other => {
            return Err(ApiError::BadRequest(format!(
                "outcome must be helpful or corrected, got {other:?}"
            )));
        }
    }
    usage.last_feedback_at = Some(Utc::now());
    save_usage(&state, &usage);
    Ok(Json(usage))
}

async fn usage(
    State(state): State<AppState>,
    Path(skill_id): Path<String>,
) -> Result<Json<SkillUsage>, ApiError> {
    Ok(Json(load_usage(&state, skill_id.trim())))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSearchHit {
    pub skill_id: String,
    pub name: String,
    pub description: String,
    pub rank: usize,
    pub usage: SkillUsage,
}

#[derive(Debug, Serialize)]
struct SkillSearchResponse {
    items: Vec<SkillSearchHit>,
}

/// GET /v1/skills/search?q= — Stage 3.2: "which skill covers this".
///
/// Reuses the Stage 1 fused ranker (BM25 + recency + vector, RRF) over the
/// registry rather than a second ranker. Vectors are computed per call, not
/// cached: the corpus is the installed skill set (tens, not thousands) and the
/// derived-index table is keyed to memory assets.
async fn search(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<SkillSearchResponse>, ApiError> {
    let q = params.get("q").map(|s| s.trim()).unwrap_or_default();
    if q.is_empty() {
        return Err(ApiError::BadRequest("q is required".into()));
    }
    let skills = state
        .skills
        .as_deref()
        .ok_or_else(|| ApiError::internal("skills registry is not configured"))?;
    let all = skills.list();
    let docs: Vec<kura_context::RetrievalDoc> = all
        .iter()
        .map(|s| kura_context::RetrievalDoc {
            asset_id: s.skill_id.clone(),
            layer: "skill".into(),
            title: s.name.clone(),
            content: format!("{} {}", s.description, s.body),
        })
        .collect();
    let embedder = kura_context::HashedNgramEmbedder::default();
    let items = kura_context::retrieve_fused(q, &docs, Some(&embedder))
        .into_iter()
        .enumerate()
        .map(|(rank, idx)| SkillSearchHit {
            skill_id: all[idx].skill_id.clone(),
            name: all[idx].name.clone(),
            description: all[idx].description.clone(),
            rank: rank + 1,
            usage: load_usage(&state, &all[idx].skill_id),
        })
        .collect();
    Ok(Json(SkillSearchResponse { items }))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/skills/proposals", post(propose).get(list))
        .route("/v1/skills/proposals/distill", post(distill))
        .route("/v1/skills/search", get(search))
        .route("/v1/skills/{skill_id}/usage", get(usage))
        .route("/v1/skills/{skill_id}/feedback", post(feedback))
        .route("/v1/skills/proposals/{asset_id}/publish", post(publish))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::http::StatusCode;

    use super::super::tests_support::{request_json, test_state};

    fn skill_state() -> crate::state::AppState {
        let mut state = test_state();
        let dir = std::env::temp_dir().join(format!("kura-skillprop-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        state.config.data_dir = dir.to_string_lossy().into_owned();
        state.memory = Some(Arc::new(kura_memory::Manager::new(
            "test", None, None, None,
        )));
        state.skills = Some(Arc::new(
            kura_skills::Registry::with_roots(
                &dir.join("home").to_string_lossy(),
                &state.config.data_dir,
            )
            .expect("registry"),
        ));
        state.catalog = Some(Arc::new(kura_catalog::Manager::new("test", None, None)));
        state
    }

    #[tokio::test]
    async fn propose_review_publish_lifecycle_with_runtime_guard() {
        let state = skill_state();

        // Agent-authored proposal: forced Pending by the write policy.
        let (status, json) = request_json(
            state.clone(),
            "POST",
            "/v1/skills/proposals",
            Some(serde_json::json!({
                "name": "Deploy Checklist",
                "description": "run before every deploy",
                "body": "1. run tests\n2. check CI",
                "evidenceLinks": [{ "kind": "thread", "id": "thr_evidence" }]
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{json}");
        assert!(
            json["decision"]["require_approval"].is_object(),
            "agent write forced into review: {json}"
        );
        let asset_id = json["asset"]["assetId"]
            .as_str()
            .expect("asset id")
            .to_string();

        // Runtime guard: pending proposals are not loadable and not
        // publishable.
        assert!(
            state
                .skills
                .as_deref()
                .unwrap()
                .get("deploy checklist")
                .is_none(),
            "pending proposal must not be loadable"
        );
        let (status, _) = request_json(
            state.clone(),
            "POST",
            &format!("/v1/skills/proposals/{asset_id}/publish"),
            Some(serde_json::json!({})),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "unapproved publish refused");

        // Approve through the existing memory review, then publish.
        let (status, _) = request_json(
            state.clone(),
            "POST",
            &format!("/v1/memory/assets/{asset_id}/approve"),
            Some(serde_json::json!({ "actor": { "kind": "operator", "id": "op" } })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, json) = request_json(
            state.clone(),
            "POST",
            &format!("/v1/skills/proposals/{asset_id}/publish"),
            Some(serde_json::json!({})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{json}");
        assert_eq!(json["skillId"], "deploy checklist");
        assert!(!json["catalogItemId"].as_str().unwrap().is_empty());

        // Published: loadable from the registry with provenance intact in
        // the catalog version source.
        let skill = state
            .skills
            .as_deref()
            .unwrap()
            .get("deploy checklist")
            .expect("published skill loads");
        assert!(skill.body.contains("run tests"));
        let items = state.catalog.as_deref().unwrap().list_items();
        assert!(items.iter().any(|item| {
            item.versions
                .iter()
                .any(|v| v.source == format!("memory:{asset_id}"))
        }));

        // Listed as a proposal asset.
        let (status, json) = request_json(state, "GET", "/v1/skills/proposals", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["items"][0]["kind"], "skill");
    }

    /// Stage 3.3: republishing a superseding proposal appends a catalog
    /// version to the same item and stamps provenance + supersedes into the
    /// bundle's frontmatter.
    #[tokio::test]
    async fn republish_supersedes_with_provenance() {
        let state = skill_state();
        let mut ids = Vec::new();
        for body in ["1. run tests", "1. run tests\n2. check CI"] {
            let (status, json) = request_json(
                state.clone(),
                "POST",
                "/v1/skills/proposals",
                Some(serde_json::json!({
                    "name": "Release Gate", "description": "gate", "body": body,
                    "evidenceLinks": [{ "kind": "thread", "id": "thr_1" }]
                })),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{json}");
            let id = json["asset"]["assetId"].as_str().unwrap().to_string();
            let (status, _) = request_json(
                state.clone(),
                "POST",
                &format!("/v1/memory/assets/{id}/approve"),
                Some(serde_json::json!({ "actor": { "kind": "operator", "id": "op" } })),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            let (status, json) = request_json(
                state.clone(),
                "POST",
                &format!("/v1/skills/proposals/{id}/publish"),
                Some(serde_json::json!({})),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{json}");
            ids.push(id);
        }
        let items: Vec<_> = state
            .catalog
            .as_deref()
            .unwrap()
            .list_items()
            .into_iter()
            .filter(|i| i.name == "Release Gate")
            .collect();
        assert_eq!(items.len(), 1, "one catalog item, not one per publish");
        assert_eq!(items[0].versions.len(), 2, "{:?}", items[0].versions);
        let skill = state
            .skills
            .as_deref()
            .unwrap()
            .get("release gate")
            .expect("loads");
        assert!(
            skill
                .frontmatter_raw
                .contains(&format!("provenance: memory:{}", ids[1])),
            "{}",
            skill.frontmatter_raw
        );
        assert!(
            skill
                .frontmatter_raw
                .contains(&format!("supersedes: memory:{}", ids[0])),
            "{}",
            skill.frontmatter_raw
        );
        assert!(skill.body.contains("check CI"));
    }

    /// Stage 3.2 + 3.4: search ranks by relevance and carries usage; feedback
    /// is explicit.
    #[tokio::test]
    async fn search_ranks_skills_and_carries_usage_feedback() {
        let state = skill_state();
        for (name, body) in [
            (
                "Deploy Checklist",
                "run tests, check CI, then deploy to production",
            ),
            ("Kitchen Notes", "recipes and groceries"),
        ] {
            let (_, json) = request_json(state.clone(), "POST", "/v1/skills/proposals", Some(serde_json::json!({
                "name": name, "description": name, "body": body, "evidenceLinks": [{ "kind": "thread", "id": "thr_1" }]
            }))).await;
            let id = json["asset"]["assetId"].as_str().unwrap().to_string();
            request_json(
                state.clone(),
                "POST",
                &format!("/v1/memory/assets/{id}/approve"),
                Some(serde_json::json!({ "actor": { "kind": "operator", "id": "op" } })),
            )
            .await;
            let (status, _) = request_json(
                state.clone(),
                "POST",
                &format!("/v1/skills/proposals/{id}/publish"),
                Some(serde_json::json!({})),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
        }
        let (status, json) = request_json(
            state.clone(),
            "GET",
            "/v1/skills/search?q=deploy%20to%20production",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{json}");
        let items = json["items"].as_array().unwrap();
        assert_eq!(items[0]["skillId"], "deploy checklist", "{json}");
        assert_eq!(items[0]["rank"], 1);
        assert_eq!(items[0]["usage"]["invocations"], 0);

        let (status, usage) = request_json(
            state.clone(),
            "POST",
            "/v1/skills/deploy%20checklist/feedback",
            Some(serde_json::json!({ "outcome": "corrected" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{usage}");
        assert_eq!(usage["corrected"], 1);
        let (status, _) = request_json(
            state.clone(),
            "POST",
            "/v1/skills/deploy%20checklist/feedback",
            Some(serde_json::json!({ "outcome": "meh" })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        super::record_invocation(&state, "deploy checklist");
        let (_, json) =
            request_json(state.clone(), "GET", "/v1/skills/search?q=deploy", None).await;
        assert_eq!(json["items"][0]["usage"]["invocations"], 1);
        assert_eq!(json["items"][0]["usage"]["corrected"], 1);
        let (status, _) = request_json(state.clone(), "GET", "/v1/skills/search", None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "q is required");
    }
}
