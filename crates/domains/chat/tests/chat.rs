//! Port of `daemon/internal/chat/service_test.go`: chat service behavior over
//! an in-memory fake store, plus wire round-trip coverage. The Go tests run
//! against a real SQLite store; here the un-ported store surface is exercised
//! through a `ChatStore` fake (mirroring the Go store semantics for turn/
//! preview saving), and the ported dispatch/event subset is additionally
//! verified against the real `kura_store::SQLiteStore` adapter.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures::future::BoxFuture;
use kura_bindings::{
    CapabilityDecision, EffectiveBindingSelection, EffectiveVisibility, ResolutionOutcome,
    RuntimeBindingEvidence,
};
use kura_chat::{
    CancellationToken, ChatError, ChatStore, OPENAI_COMPATIBLE_PROVIDER_NAME, QueryInput,
    QueryResult, Service, StreamChunk, compile_prompt_messages, continuity_source_kind,
    inject_continuity_messages, response_continuity_source_event_key, terminal_dispatch_event,
};
use kura_events::{Bus, Event, Filter, Scope};
use kura_llm::{
    CancelToken, Dispatch, DispatchStatus, Dispatcher, Message, MessageRole, Provider,
    ProviderError, ProviderRequest, ProviderResponse, StreamEmitter, Usage,
};
use kura_profiles::{ActiveSelection, AgentProfile, RuntimeProjection};
use kura_setupwizard::{
    RemediationOwner, SafeUseMode, SetupSession, SetupState, SetupStyle, TARGET_OPENAI_COMPATIBLE,
    TargetKind,
};
use kura_threads::{
    ContinuityDecision, ContinuityItemKind, ContinuityMode, ContinuityPreview,
    ContinuityPreviewItem, ContinuityReason, ContinuityRole, ContinuityStatus, ContinuityTurn,
    HandoffLink, HandoffSourceReference, HandoffSourceReferenceDecision,
    HandoffSourceReferenceStatus, LifecycleState, RedactionStatus, RuntimeArtifactExcerpt,
    SourceKind, Thread,
};
use parking_lot::RwLock;

// ------------------------------------------------------------------------
// Test providers
// ------------------------------------------------------------------------

/// Go `testProvider`: echo-complete and two-chunk stream, recording requests.
#[derive(Clone)]
struct TestProvider {
    name: String,
    requests: Arc<RwLock<Vec<ProviderRequest>>>,
}

impl TestProvider {
    fn new(name: &str) -> Self {
        TestProvider {
            name: name.to_string(),
            requests: Arc::new(RwLock::new(Vec::new())),
        }
    }

    fn saw_message(&self, content: &str) -> bool {
        self.requests.read().iter().any(|request| {
            request
                .messages
                .iter()
                .any(|message| message.content == content)
        })
    }
}

impl Provider for TestProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn complete<'a>(
        &'a self,
        request: ProviderRequest,
    ) -> BoxFuture<'a, Result<ProviderResponse, ProviderError>> {
        let requests = Arc::clone(&self.requests);
        Box::pin(async move {
            requests.write().push(request.clone());
            Ok(ProviderResponse {
                tool_calls: Vec::new(),
                output: format!("reply:{}", request.model),
                finish_reason: "stop".to_string(),
                usage: Usage {
                    input_tokens: 1,
                    output_tokens: 1,
                    total_tokens: 2,
                    ..Default::default()
                },
            })
        })
    }

    fn stream<'a>(
        &'a self,
        request: ProviderRequest,
        mut emit: StreamEmitter<'a>,
    ) -> BoxFuture<'a, Result<ProviderResponse, ProviderError>> {
        let requests = Arc::clone(&self.requests);
        Box::pin(async move {
            requests.write().push(request.clone());
            emit(kura_llm::StreamChunk {
                delta: "reply:".to_string(),
                output: "reply:".to_string(),
                ..Default::default()
            })?;
            emit(kura_llm::StreamChunk {
                delta: request.model.clone(),
                output: format!("reply:{}", request.model),
                finish_reason: "stop".to_string(),
                usage: Some(Usage {
                    input_tokens: 1,
                    output_tokens: 1,
                    total_tokens: 2,
                    ..Default::default()
                }),
                ..Default::default()
            })?;
            Ok(ProviderResponse {
                tool_calls: Vec::new(),
                output: format!("reply:{}", request.model),
                finish_reason: "stop".to_string(),
                usage: Usage {
                    input_tokens: 1,
                    output_tokens: 1,
                    total_tokens: 2,
                    ..Default::default()
                },
            })
        })
    }
}

/// Provider whose `complete` sleeps before answering, used for cancellation.
#[derive(Clone)]
struct SlowProvider {
    name: String,
    delay: Duration,
    requests: Arc<RwLock<Vec<ProviderRequest>>>,
}

impl Provider for SlowProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn complete<'a>(
        &'a self,
        request: ProviderRequest,
    ) -> BoxFuture<'a, Result<ProviderResponse, ProviderError>> {
        let requests = Arc::clone(&self.requests);
        let delay = self.delay;
        Box::pin(async move {
            requests.write().push(request.clone());
            tokio::time::sleep(delay).await;
            Ok(ProviderResponse {
                tool_calls: Vec::new(),
                output: "late reply".to_string(),
                finish_reason: "stop".to_string(),
                usage: Usage::default(),
            })
        })
    }

    fn stream<'a>(
        &'a self,
        _request: ProviderRequest,
        _emit: StreamEmitter<'a>,
    ) -> BoxFuture<'a, Result<ProviderResponse, ProviderError>> {
        Box::pin(async {
            Err(ProviderError::other(
                "stream not supported by slow provider",
            ))
        })
    }
}

// ------------------------------------------------------------------------
// Fake store (Go store semantics for the chat service surface)
// ------------------------------------------------------------------------

fn hex_token() -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    hex[..16].to_string()
}

fn thread_key(tenant_id: &str, thread_id: &str) -> String {
    format!("{tenant_id}\u{0}{thread_id}")
}

#[derive(Default)]
struct FakeInner {
    dispatches: Vec<Dispatch>,
    events: Vec<Event>,
    setup_sessions: Vec<SetupSession>,
    threads: HashMap<String, Thread>,
    turns: Vec<ContinuityTurn>,
    previews: Vec<(ContinuityPreview, Vec<ContinuityPreviewItem>)>,
    handoff_links: Vec<HandoffLink>,
    handoff_refs: Vec<HandoffSourceReference>,
    active_selection: Option<(AgentProfile, ActiveSelection)>,
    binding_resolution: Option<EffectiveBindingSelection>,
    capability_visibility: HashMap<String, CapabilityDecision>,
    profile_projections: Vec<RuntimeProjection>,
    binding_evidence: Vec<RuntimeBindingEvidence>,
    turn_checkpoints: HashMap<String, kura_store::TurnCheckpointRecord>,
    turn_checkpoint_writes: usize,
}

struct FakeStore {
    inner: RwLock<FakeInner>,
}

impl FakeStore {
    fn new() -> Arc<Self> {
        Arc::new(FakeStore {
            inner: RwLock::new(FakeInner::default()),
        })
    }

    fn add_thread(&self, thread: Thread) {
        self.inner
            .write()
            .threads
            .insert(thread_key(&thread.tenant_id, &thread.thread_id), thread);
    }

    fn add_turn(&self, turn: ContinuityTurn) {
        self.inner.write().turns.push(turn);
    }

    fn add_setup_session(&self, session: SetupSession) {
        self.inner.write().setup_sessions.push(session);
    }

    fn set_active_selection(&self, profile: AgentProfile, selection: ActiveSelection) {
        self.inner.write().active_selection = Some((profile, selection));
    }

    fn set_binding_resolution(&self, resolution: EffectiveBindingSelection) {
        self.inner.write().binding_resolution = Some(resolution);
    }

    fn set_capability_visibility(&self, capability_id: &str, decision: CapabilityDecision) {
        self.inner
            .write()
            .capability_visibility
            .insert(capability_id.to_string(), decision);
    }

    fn add_handoff_link(&self, link: HandoffLink) {
        self.inner.write().handoff_links.push(link);
    }

    fn add_handoff_ref(&self, reference: HandoffSourceReference) {
        self.inner.write().handoff_refs.push(reference);
    }

    fn turns(&self) -> Vec<ContinuityTurn> {
        self.inner.read().turns.clone()
    }

    fn previews(&self) -> Vec<(ContinuityPreview, Vec<ContinuityPreviewItem>)> {
        self.inner.read().previews.clone()
    }

    fn profile_projections(&self) -> Vec<RuntimeProjection> {
        self.inner.read().profile_projections.clone()
    }

    fn binding_evidence(&self) -> Vec<RuntimeBindingEvidence> {
        self.inner.read().binding_evidence.clone()
    }

    fn dispatches(&self) -> Vec<Dispatch> {
        self.inner.read().dispatches.clone()
    }
}

const DEFAULT_TURN_LIMIT: usize = 76;

impl ChatStore for FakeStore {
    fn get_turn_checkpoint(
        &self,
        turn_key: &str,
    ) -> Result<Option<kura_store::TurnCheckpointRecord>, String> {
        Ok(self.inner.read().turn_checkpoints.get(turn_key).cloned())
    }

    fn put_turn_checkpoint(&self, record: &kura_store::TurnCheckpointRecord) -> Result<(), String> {
        let mut inner = self.inner.write();
        inner.turn_checkpoint_writes += 1;
        inner
            .turn_checkpoints
            .insert(record.turn_key.clone(), record.clone());
        Ok(())
    }

    fn delete_turn_checkpoint(&self, turn_key: &str) -> Result<(), String> {
        self.inner.write().turn_checkpoints.remove(turn_key);
        Ok(())
    }

    fn upsert_llm_dispatch(&self, dispatch: &Dispatch) -> Result<(), String> {
        let mut inner = self.inner.write();
        if let Some(existing) = inner
            .dispatches
            .iter_mut()
            .find(|d| d.dispatch_id == dispatch.dispatch_id)
        {
            *existing = dispatch.clone();
        } else {
            inner.dispatches.push(dispatch.clone());
        }
        Ok(())
    }

    fn append_event(&self, event: &Event) -> Result<Event, String> {
        let mut inner = self.inner.write();
        let mut out = event.clone();
        out.sequence = (inner.events.len() + 1) as i64;
        inner.events.push(out.clone());
        Ok(out)
    }

    fn list_setup_sessions(&self, tenant_id: &str) -> Result<Vec<SetupSession>, String> {
        let inner = self.inner.read();
        Ok(inner
            .setup_sessions
            .iter()
            .filter(|s| s.tenant_id == tenant_id)
            .cloned()
            .collect())
    }

    fn active_agent_profile_selection(
        &self,
        tenant_id: &str,
    ) -> Result<Option<(AgentProfile, ActiveSelection)>, String> {
        let inner = self.inner.read();
        Ok(inner
            .active_selection
            .clone()
            .filter(|(profile, _)| profile.tenant_id == tenant_id))
    }

    fn record_runtime_profile_projection(
        &self,
        projection: &RuntimeProjection,
    ) -> Result<RuntimeProjection, String> {
        let mut inner = self.inner.write();
        let mut out = projection.clone();
        out.runtime_profile_projection_id = format!("rpp_{}", hex_token());
        inner.profile_projections.push(out.clone());
        Ok(out)
    }

    fn resolve_binding_selection(
        &self,
        _params: &kura_chat::BindingResolutionParams,
    ) -> Result<EffectiveBindingSelection, String> {
        let inner = self.inner.read();
        Ok(inner.binding_resolution.clone().unwrap_or_default())
    }

    fn effective_capability_visibility(
        &self,
        _tenant_id: &str,
        _profile_id: &str,
        _workspace_id: &str,
        capability_id: &str,
    ) -> Result<CapabilityDecision, String> {
        let inner = self.inner.read();
        Ok(inner
            .capability_visibility
            .get(capability_id)
            .cloned()
            .unwrap_or_else(|| CapabilityDecision {
                capability_id: capability_id.to_string(),
                effective: EffectiveVisibility::VISIBLE,
                default_enabled: false,
                offered: true,
                executable: true,
                reason: "default_executable".to_string(),
                scope: "workspace".to_string(),
            }))
    }

    fn record_runtime_binding_evidence(
        &self,
        evidence: &RuntimeBindingEvidence,
    ) -> Result<RuntimeBindingEvidence, String> {
        let mut inner = self.inner.write();
        let mut out = evidence.clone();
        out.projection_id = format!("rbe_{}", hex_token());
        inner.binding_evidence.push(out.clone());
        Ok(out)
    }

    fn get_thread_for_tenant(
        &self,
        tenant_id: &str,
        thread_id: &str,
    ) -> Result<Option<Thread>, String> {
        Ok(self
            .inner
            .read()
            .threads
            .get(&thread_key(tenant_id, thread_id))
            .cloned())
    }

    fn list_continuity_turns(
        &self,
        query: &kura_chat::ContinuityLookupQuery,
    ) -> Result<Vec<ContinuityTurn>, String> {
        let inner = self.inner.read();
        let now = query.now.unwrap_or_else(Utc::now);
        let limit = if query.limit <= 0 {
            DEFAULT_TURN_LIMIT
        } else {
            query.limit as usize
        };
        let mut items: Vec<ContinuityTurn> = inner
            .turns
            .iter()
            .filter(|turn| {
                turn.tenant_id == query.tenant_id
                    && turn.thread_id == query.thread_id
                    && turn.session_segment_id == query.session_segment_id
            })
            .filter(|turn| {
                turn.retention_expires_at
                    .is_some_and(|expires| expires >= now)
            })
            .cloned()
            .collect();
        items.sort_by(|a, b| b.acceptance_sequence.cmp(&a.acceptance_sequence));
        items.truncate(limit);
        Ok(items)
    }

    fn list_continuity_turns_outside_session_segment(
        &self,
        query: &kura_chat::ContinuityLookupQuery,
    ) -> Result<Vec<ContinuityTurn>, String> {
        let inner = self.inner.read();
        let now = query.now.unwrap_or_else(Utc::now);
        let limit = if query.limit <= 0 {
            DEFAULT_TURN_LIMIT
        } else {
            query.limit as usize
        };
        let mut items: Vec<ContinuityTurn> = inner
            .turns
            .iter()
            .filter(|turn| {
                turn.tenant_id == query.tenant_id
                    && turn.thread_id == query.thread_id
                    && turn.session_segment_id != query.session_segment_id
            })
            .filter(|turn| {
                turn.retention_expires_at
                    .is_some_and(|expires| expires >= now)
            })
            .cloned()
            .collect();
        items.sort_by(|a, b| b.acceptance_sequence.cmp(&a.acceptance_sequence));
        items.truncate(limit);
        Ok(items)
    }

    fn list_handoff_links_for_thread(
        &self,
        tenant_id: &str,
        thread_id: &str,
        limit: i64,
    ) -> Result<Vec<HandoffLink>, String> {
        let inner = self.inner.read();
        let mut items: Vec<HandoffLink> = inner
            .handoff_links
            .iter()
            .filter(|link| link.tenant_id == tenant_id && link.destination_thread_id == thread_id)
            .cloned()
            .collect();
        if limit > 0 {
            items.truncate(limit as usize);
        }
        Ok(items)
    }

    fn list_handoff_source_references_for_link(
        &self,
        tenant_id: &str,
        link_id: &str,
    ) -> Result<Vec<HandoffSourceReference>, String> {
        let inner = self.inner.read();
        Ok(inner
            .handoff_refs
            .iter()
            .filter(|reference| {
                reference.tenant_id == tenant_id && reference.handoff_link_id == link_id
            })
            .cloned()
            .collect())
    }

    fn save_continuity_turn(&self, turn: &ContinuityTurn) -> Result<ContinuityTurn, String> {
        let mut inner = self.inner.write();
        let now = Utc::now();
        let mut out = turn.clone();
        if out.recorded_at == DateTime::<Utc>::UNIX_EPOCH {
            out.recorded_at = now;
        }
        if out.retention_expires_at.is_none() {
            out.retention_expires_at = Some(out.recorded_at + chrono::Duration::days(90));
        }
        // Go: unique(source_event_key) returns the existing turn.
        if !out.source_event_key.trim().is_empty() {
            if let Some(existing) = inner.turns.iter().find(|t| {
                t.tenant_id == out.tenant_id && t.source_event_key == out.source_event_key
            }) {
                return Ok(existing.clone());
            }
        }
        if out.continuity_turn_id.is_empty() {
            out.continuity_turn_id = format!("turn_{}", hex_token());
        }
        if out.acceptance_sequence == 0 {
            let max = inner
                .turns
                .iter()
                .filter(|t| t.tenant_id == out.tenant_id && t.thread_id == out.thread_id)
                .map(|t| t.acceptance_sequence)
                .max()
                .unwrap_or(0);
            out.acceptance_sequence = max + 1;
        }
        inner.turns.push(out.clone());
        Ok(out)
    }

    fn mark_handoff_source_references_consumed(
        &self,
        tenant_id: &str,
        link_id: &str,
        response_turn_id: &str,
        now: DateTime<Utc>,
    ) -> Result<(), String> {
        let mut inner = self.inner.write();
        for link in inner.handoff_links.iter_mut() {
            if link.tenant_id == tenant_id && link.handoff_link_id == link_id {
                link.source_reference_status = HandoffSourceReferenceStatus::Consumed;
                link.first_destination_response_id = response_turn_id.to_string();
                link.consumed_at = Some(now);
            }
        }
        for reference in inner.handoff_refs.iter_mut() {
            if reference.tenant_id == tenant_id && reference.handoff_link_id == link_id {
                reference.consumed_at = Some(now);
                if reference.decision == HandoffSourceReferenceDecision::Referenced {
                    reference.decision = HandoffSourceReferenceDecision::Consumed;
                }
            }
        }
        Ok(())
    }

    fn save_continuity_preview(
        &self,
        preview: &ContinuityPreview,
        items: &[ContinuityPreviewItem],
    ) -> Result<ContinuityPreview, String> {
        let mut inner = self.inner.write();
        let now = Utc::now();
        let mut out = preview.clone();
        if out.continuity_preview_id.is_empty() {
            out.continuity_preview_id = format!("contprev_{}", hex_token());
        }
        let policy = kura_threads::default_continuity_policy();
        if out.window_policy_id.is_empty() {
            out.window_policy_id = policy.window_policy_id;
            out.max_prior_turns = policy.max_prior_turns;
            out.active_window_days = policy.active_window_days;
        }
        if out.max_prior_turns == 0 {
            out.max_prior_turns = policy.max_prior_turns;
        }
        if out.active_window_days == 0 {
            out.active_window_days = policy.active_window_days;
        }
        if out.assembly_started_at == DateTime::<Utc>::UNIX_EPOCH {
            out.assembly_started_at = now;
        }
        if out.assembly_completed_at == DateTime::<Utc>::UNIX_EPOCH {
            out.assembly_completed_at = now;
        }
        if out.assembly_duration_ms == 0 {
            out.assembly_duration_ms =
                (out.assembly_completed_at - out.assembly_started_at).num_milliseconds();
        }
        if out.retention_expires_at == DateTime::<Utc>::UNIX_EPOCH {
            out.retention_expires_at = out.assembly_completed_at + chrono::Duration::days(90);
        }
        let saved_items: Vec<ContinuityPreviewItem> = items
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, mut item)| {
                if item.preview_item_id.is_empty() {
                    item.preview_item_id = format!("contitem_{}", hex_token());
                }
                item.continuity_preview_id = out.continuity_preview_id.clone();
                item.tenant_id = out.tenant_id.clone();
                item.thread_id = out.thread_id.clone();
                if item.item_order == 0 {
                    item.item_order = index as i32;
                }
                item
            })
            .collect();
        inner.previews.push((out.clone(), saved_items));
        Ok(out)
    }
}

// ------------------------------------------------------------------------
// Fixtures
// ------------------------------------------------------------------------

fn write_file(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().expect("fixture parent")).expect("create dirs");
    std::fs::write(path, content).expect("write fixture");
}

/// Registry with a `shared` skill (sandbox declaration) and two overlays,
/// mirroring Go `newChatSkillRegistry`.
fn skill_registry() -> Arc<kura_skills::Registry> {
    let home = tempfile::tempdir().expect("home temp dir");
    let data = tempfile::tempdir().expect("data temp dir");
    write_file(&home.path().join("AGENTS.md"), "home overlay");
    write_file(&data.path().join("AGENTS.md"), "data overlay");
    let skill = [
        "---",
        "name: shared",
        "description: \"data skill\"",
        "---",
        "data instructions",
    ]
    .join("\n");
    write_file(
        &data.path().join("skills").join("shared").join("SKILL.md"),
        &skill,
    );
    let registry = kura_skills::Registry::with_roots(
        &home.path().join(".agents").to_string_lossy(),
        data.path().to_str().expect("data path"),
    )
    .expect("registry");
    Arc::new(registry)
}

fn seed_continuity_thread(store: &FakeStore, now: DateTime<Utc>) {
    store.add_thread(Thread {
        thread_id: "thr_1".to_string(),
        tenant_id: "ten_1".to_string(),
        lifecycle_state: LifecycleState::Active,
        current_session_segment_id: "seg_1".to_string(),
        source_kind: SourceKind::Chat,
        source_summary: String::new(),
        last_activity_at: now,
        created_at: now,
        updated_at: now,
        retention_expires_at: Some(now + chrono::Duration::days(90)),
        redaction_status: RedactionStatus::Redacted,
    });
}

fn continuity_turn(
    id: &str,
    segment: &str,
    sequence: i64,
    role: ContinuityRole,
    content: &str,
    recorded_at: DateTime<Utc>,
) -> ContinuityTurn {
    ContinuityTurn {
        continuity_turn_id: id.to_string(),
        tenant_id: "ten_1".to_string(),
        thread_id: "thr_1".to_string(),
        session_segment_id: segment.to_string(),
        acceptance_sequence: sequence,
        role,
        source_kind: SourceKind::Chat,
        source_linkage_id: String::new(),
        source_message_id: String::new(),
        source_timestamp: None,
        dispatch_id: String::new(),
        response_to_turn_id: String::new(),
        safe_content: content.to_string(),
        content_redaction_status: RedactionStatus::Redacted,
        artifact_excerpt_refs: Vec::new(),
        recorded_at,
        retention_expires_at: Some(recorded_at + chrono::Duration::days(90)),
        source_event_key: String::new(),
    }
}

fn base_query(provider: &str, model: &str, query: &str) -> QueryInput {
    QueryInput {
        query: query.to_string(),
        provider: provider.to_string(),
        model: model.to_string(),
        ..QueryInput::default()
    }
}

fn service(
    dispatcher: Arc<Dispatcher>,
    skills: Option<Arc<kura_skills::Registry>>,
    store: Option<Arc<dyn ChatStore>>,
) -> Service {
    Service::new_service(dispatcher, None, skills, Some(Bus::new()), store)
}

fn new_dispatcher(provider: Arc<dyn Provider>) -> Arc<Dispatcher> {
    let dispatcher = Dispatcher::new();
    dispatcher.register_provider(provider);
    Arc::new(dispatcher)
}

// ------------------------------------------------------------------------
// Behavior: query
// ------------------------------------------------------------------------

/// Go `TestQueryReturnsSelectedSkillContractsAndEvents`.
#[test]
fn query_returns_selected_skill_contracts_and_events() {
    let provider = TestProvider::new("chat-test");
    let dispatcher = new_dispatcher(Arc::new(provider));
    let registry = skill_registry();
    let store = FakeStore::new();
    let bus = Bus::new();
    let svc = Service::new_service(
        dispatcher,
        None,
        Some(registry),
        Some(bus.clone()),
        Some(store.clone() as Arc<dyn ChatStore>),
    );

    let execution = svc
        .query(
            QueryInput {
                provider: "chat-test".to_string(),
                model: "model-a".to_string(),
                skills: vec!["shared".to_string()],
                query: "hello".to_string(),
                scope: Scope {
                    run_id: "run_1".to_string(),
                    ..Scope::default()
                },
                ..QueryInput::default()
            },
            &CancellationToken::new(),
        )
        .expect("query");
    assert!(execution.exec_error.is_none());
    let result = execution.result;
    assert_eq!(result.skills, vec!["shared".to_string()]);
    assert_eq!(
        result.skill_contracts.len(),
        1,
        "expected one selected skill contract"
    );
    let declaration = result.skill_contracts[0]
        .get("declaration")
        .expect("declaration payload")
        .as_object()
        .expect("declaration object");
    assert_eq!(declaration["consumerKind"], "skill");
    assert_eq!(declaration["consumerId"], "shared");
    assert_eq!(declaration["operationKind"], "skill_selection");

    let llm_events = bus.list(&Filter {
        category: "llm".to_string(),
        ..Filter::default()
    });
    assert_eq!(
        llm_events.len(),
        2,
        "expected requested and terminal llm events"
    );
    assert!(llm_events[0].payload.contains_key("skillContracts"));
    assert_eq!(llm_events[0].name, "llm.dispatch.requested");
    assert_eq!(llm_events[1].name, "llm.dispatch.completed");
    assert_eq!(store.dispatches().len(), 1, "dispatch upserted once by id");
    assert_eq!(store.dispatches()[0].status, DispatchStatus::Completed);
}

/// Go `TestQueryBlocksOpenAICompatibleWhenSetupSessionBlocksDependentUse`.
#[test]
fn query_blocks_openai_compatible_when_setup_session_blocks_dependent_use() {
    use kura_config::{LlmConfig, OpenAiCompatibleProviderConfig};
    use kura_providers::new_manager;

    let dispatcher = new_dispatcher(Arc::new(TestProvider::new(OPENAI_COMPATIBLE_PROVIDER_NAME)));
    let provider_manager = new_manager(
        LlmConfig {
            default_provider: OPENAI_COMPATIBLE_PROVIDER_NAME.to_string(),
            openai_compatible: OpenAiCompatibleProviderConfig {
                base_url: "https://example.com".to_string(),
                api_key: "secret".to_string(),
                model: "gpt-4.1-mini".to_string(),
                ..OpenAiCompatibleProviderConfig::default()
            },
            ..LlmConfig::default()
        },
        Some(dispatcher.clone()),
        Vec::new(),
    );
    let store = FakeStore::new();
    store.add_setup_session(blocked_openai_setup_session());
    let svc = Service::new_service(
        dispatcher,
        Some(Arc::new(provider_manager)),
        None,
        Some(Bus::new()),
        Some(store.clone() as Arc<dyn ChatStore>),
    );

    let err = svc
        .query(
            QueryInput {
                tenant_id: "ten_chat_setup".to_string(),
                query: "hello".to_string(),
                ..QueryInput::default()
            },
            &CancellationToken::new(),
        )
        .expect_err("setup gate must block");
    assert_eq!(
        err,
        ChatError::ProviderAuthUnavailable("credential_missing".to_string())
    );
}

/// Go `TestQueryFailsClosedOnRepairRequiredBinding` semantics (FR-031):
/// repair-required selection blocks new work and records durable evidence.
#[test]
fn query_fails_closed_on_repair_required_binding() {
    let provider = TestProvider::new("chat-binding");
    let dispatcher = new_dispatcher(Arc::new(provider));
    let store = FakeStore::new();
    store.set_active_selection(
        AgentProfile {
            tenant_id: "ten_1".to_string(),
            profile_id: "prof_1".to_string(),
            ..AgentProfile::default()
        },
        ActiveSelection::default(),
    );
    store.set_binding_resolution(EffectiveBindingSelection {
        outcome: ResolutionOutcome::REPAIR_REQUIRED,
        repair_reason: "workspace disabled".to_string(),
        ..EffectiveBindingSelection::default()
    });
    let bus = Bus::new();
    let svc = Service::new_service(
        dispatcher,
        None,
        None,
        Some(bus.clone()),
        Some(store.clone() as Arc<dyn ChatStore>),
    );

    let err = svc
        .query(
            QueryInput {
                tenant_id: "ten_1".to_string(),
                thread_id: "thr_1".to_string(),
                provider: "chat-binding".to_string(),
                model: "m".to_string(),
                query: "hello".to_string(),
                ..QueryInput::default()
            },
            &CancellationToken::new(),
        )
        .expect_err("repair-required binding blocks new work");
    assert!(matches!(
        err,
        ChatError::BindingRepairRequired(reason) if reason == "workspace disabled"
    ));
    // FR-031: durable runtime evidence was recorded for the blocked work.
    assert_eq!(store.binding_evidence().len(), 1);
    let binding_events = bus.list(&Filter {
        category: "binding".to_string(),
        ..Filter::default()
    });
    assert_eq!(binding_events.len(), 1);
    assert_eq!(binding_events[0].name, "binding.runtime_projected");
}

/// FR-016: an explicitly selected skill whose capability is hidden or disabled
/// under the active binding blocks execution.
#[test]
fn query_blocks_hidden_capability_under_binding() {
    let provider = TestProvider::new("chat-cap");
    let dispatcher = new_dispatcher(Arc::new(provider));
    let registry = skill_registry();
    let store = FakeStore::new();
    store.set_active_selection(
        AgentProfile {
            tenant_id: "ten_1".to_string(),
            profile_id: "prof_1".to_string(),
            ..AgentProfile::default()
        },
        ActiveSelection::default(),
    );
    store.set_binding_resolution(EffectiveBindingSelection {
        outcome: ResolutionOutcome::RESOLVED,
        selected_profile_id: "prof_1".to_string(),
        selected_workspace_id: "ws_1".to_string(),
        ..EffectiveBindingSelection::default()
    });
    store.set_capability_visibility(
        "shared",
        CapabilityDecision {
            capability_id: "shared".to_string(),
            effective: EffectiveVisibility::HIDDEN,
            default_enabled: false,
            offered: false,
            executable: false,
            reason: "hidden_by_workspace".to_string(),
            scope: "workspace".to_string(),
        },
    );
    let svc = service(
        dispatcher,
        Some(registry),
        Some(store.clone() as Arc<dyn ChatStore>),
    );
    let err = svc
        .query(
            QueryInput {
                tenant_id: "ten_1".to_string(),
                thread_id: "thr_1".to_string(),
                provider: "chat-cap".to_string(),
                model: "m".to_string(),
                skills: vec!["shared".to_string()],
                query: "hello".to_string(),
                ..QueryInput::default()
            },
            &CancellationToken::new(),
        )
        .expect_err("hidden capability blocks execution");
    assert!(matches!(
        err,
        ChatError::CapabilityNotExecutable(message) if message.contains("shared")
    ));
}

/// Active profile + resolved binding record the runtime projection and binding
/// evidence for thread-linked work.
#[test]
fn query_records_profile_projection_and_binding_evidence() {
    let provider = TestProvider::new("chat-profile");
    let dispatcher = new_dispatcher(Arc::new(provider));
    let store = FakeStore::new();
    store.set_active_selection(
        AgentProfile {
            tenant_id: "ten_1".to_string(),
            profile_id: "prof_1".to_string(),
            ..AgentProfile::default()
        },
        ActiveSelection::default(),
    );
    store.set_binding_resolution(EffectiveBindingSelection {
        outcome: ResolutionOutcome::RESOLVED,
        selected_profile_id: "prof_1".to_string(),
        ..EffectiveBindingSelection::default()
    });
    let bus = Bus::new();
    let svc = Service::new_service(
        dispatcher,
        None,
        None,
        Some(bus.clone()),
        Some(store.clone() as Arc<dyn ChatStore>),
    );
    let execution = svc
        .query(
            QueryInput {
                tenant_id: "ten_1".to_string(),
                thread_id: "thr_1".to_string(),
                provider: "chat-profile".to_string(),
                model: "m".to_string(),
                query: "hello".to_string(),
                ..QueryInput::default()
            },
            &CancellationToken::new(),
        )
        .expect("query");
    assert!(execution.exec_error.is_none());
    assert_eq!(store.profile_projections().len(), 1);
    assert_eq!(store.binding_evidence().len(), 1);
    let profile_events = bus.list(&Filter {
        category: "agent_profile".to_string(),
        ..Filter::default()
    });
    assert_eq!(profile_events.len(), 1);
    assert_eq!(profile_events[0].name, "agent_profile.runtime_projected");
    let binding_events = bus.list(&Filter {
        category: "binding".to_string(),
        ..Filter::default()
    });
    assert_eq!(binding_events.len(), 1);
    assert_eq!(binding_events[0].name, "binding.runtime_projected");
}

// ------------------------------------------------------------------------
// Behavior: continuity
// ------------------------------------------------------------------------

/// Go `TestQueryAssemblesBoundedCurrentSegmentContinuity`.
#[test]
fn query_assembles_bounded_current_segment_continuity() {
    let now = Utc::now();

    struct Case {
        name: &'static str,
        seed: Vec<ContinuityTurn>,
        want_status: ContinuityStatus,
        want_applied: bool,
        want_included: i64,
        want_excluded: i64,
        want_contains: Vec<&'static str>,
        want_not_contains: Vec<&'static str>,
    }

    let cases = vec![
        Case {
            name: "empty",
            seed: Vec::new(),
            want_status: ContinuityStatus::Empty,
            want_applied: false,
            want_included: 0,
            want_excluded: 0,
            want_contains: Vec::new(),
            want_not_contains: Vec::new(),
        },
        Case {
            name: "within-limit",
            seed: vec![
                continuity_turn(
                    "turn_1",
                    "seg_1",
                    1,
                    ContinuityRole::User,
                    "prior user",
                    now,
                ),
                continuity_turn(
                    "turn_2",
                    "seg_1",
                    2,
                    ContinuityRole::Assistant,
                    "prior assistant",
                    now + chrono::Duration::minutes(1),
                ),
            ],
            want_status: ContinuityStatus::Applied,
            want_applied: true,
            want_included: 2,
            want_excluded: 0,
            want_contains: vec!["prior user", "prior assistant"],
            want_not_contains: Vec::new(),
        },
        Case {
            name: "over-limit",
            seed: (1..=14)
                .map(|i| {
                    continuity_turn(
                        &format!("turn_{i:02}"),
                        "seg_1",
                        i,
                        ContinuityRole::User,
                        &format!("prior-{i:02}"),
                        now + chrono::Duration::minutes(i),
                    )
                })
                .collect(),
            want_status: ContinuityStatus::Applied,
            want_applied: true,
            want_included: 12,
            want_excluded: 2,
            want_contains: vec!["prior-03", "prior-14"],
            want_not_contains: vec!["prior-01", "prior-02"],
        },
        Case {
            name: "age-limited",
            seed: vec![
                continuity_turn(
                    "turn_old",
                    "seg_1",
                    1,
                    ContinuityRole::User,
                    "too old",
                    now - chrono::Duration::days(31),
                ),
                continuity_turn("turn_new", "seg_1", 2, ContinuityRole::User, "fresh", now),
            ],
            want_status: ContinuityStatus::Applied,
            want_applied: true,
            want_included: 1,
            want_excluded: 1,
            want_contains: vec!["fresh"],
            want_not_contains: vec!["too old"],
        },
        Case {
            name: "current-segment-only",
            seed: vec![
                continuity_turn(
                    "turn_other",
                    "seg_old",
                    1,
                    ContinuityRole::User,
                    "old segment",
                    now,
                ),
                continuity_turn(
                    "turn_current",
                    "seg_1",
                    2,
                    ContinuityRole::User,
                    "current segment",
                    now,
                ),
            ],
            want_status: ContinuityStatus::Applied,
            want_applied: true,
            want_included: 1,
            want_excluded: 0,
            want_contains: vec!["current segment"],
            want_not_contains: vec!["old segment"],
        },
    ];

    for case in cases {
        let store = FakeStore::new();
        seed_continuity_thread(&store, now);
        for turn in case.seed {
            store.add_turn(turn);
        }
        let provider = TestProvider::new("continuity-test");
        let dispatcher = new_dispatcher(Arc::new(provider.clone()));
        let svc = service(dispatcher, None, Some(store.clone() as Arc<dyn ChatStore>));
        let execution = svc
            .query(
                QueryInput {
                    tenant_id: "ten_1".to_string(),
                    thread_id: "thr_1".to_string(),
                    provider: "continuity-test".to_string(),
                    model: "model-a".to_string(),
                    query: "follow up".to_string(),
                    ..QueryInput::default()
                },
                &CancellationToken::new(),
            )
            .expect("query");
        assert!(execution.exec_error.is_none(), "case {}", case.name);
        let result = execution.result;
        assert_eq!(
            result.continuity_status,
            Some(case.want_status),
            "case {}",
            case.name
        );
        assert_eq!(
            result.continuity_applied, case.want_applied,
            "case {}",
            case.name
        );
        assert_eq!(
            result.continuity_included_count, case.want_included,
            "case {}",
            case.name
        );
        assert_eq!(
            result.continuity_excluded_count, case.want_excluded,
            "case {}",
            case.name
        );
        assert!(!result.request_turn_id.is_empty(), "case {}", case.name);
        assert!(!result.response_turn_id.is_empty(), "case {}", case.name);
        assert!(
            !result.continuity_preview_id.is_empty(),
            "case {}",
            case.name
        );
        for content in &case.want_contains {
            assert!(
                provider.saw_message(content),
                "case {} missing {content:?}",
                case.name
            );
        }
        for content in &case.want_not_contains {
            assert!(
                !provider.saw_message(content),
                "case {} saw {content:?}",
                case.name
            );
        }
    }
}

/// Go `TestQueryRecordsResetBoundaryExclusions`.
#[test]
fn query_records_reset_boundary_exclusions() {
    let now = Utc::now();
    let store = FakeStore::new();
    store.add_thread(Thread {
        thread_id: "thr_1".to_string(),
        tenant_id: "ten_1".to_string(),
        lifecycle_state: LifecycleState::Reset,
        current_session_segment_id: "seg_1".to_string(),
        source_kind: SourceKind::Chat,
        source_summary: String::new(),
        last_activity_at: now,
        created_at: now,
        updated_at: now,
        retention_expires_at: Some(now + chrono::Duration::days(90)),
        redaction_status: RedactionStatus::Redacted,
    });
    store.add_turn(continuity_turn(
        "turn_pre_reset",
        "seg_old",
        1,
        ContinuityRole::User,
        "pre reset context",
        now,
    ));
    let provider = TestProvider::new("continuity-reset");
    let dispatcher = new_dispatcher(Arc::new(provider.clone()));
    let svc = service(dispatcher, None, Some(store.clone() as Arc<dyn ChatStore>));
    let execution = svc
        .query(
            QueryInput {
                tenant_id: "ten_1".to_string(),
                thread_id: "thr_1".to_string(),
                provider: "continuity-reset".to_string(),
                model: "model-a".to_string(),
                query: "follow up after reset".to_string(),
                ..QueryInput::default()
            },
            &CancellationToken::new(),
        )
        .expect("query");
    let result = execution.result;
    assert!(!result.continuity_applied);
    assert_eq!(result.continuity_included_count, 0);
    assert_eq!(result.continuity_excluded_count, 1);
    assert!(!provider.saw_message("pre reset context"));
    let (preview, items) = store.previews().pop().expect("preview persisted");
    assert_eq!(preview.continuity_preview_id, result.continuity_preview_id);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].decision, ContinuityDecision::Excluded);
    assert_eq!(items[0].reason_code, ContinuityReason::ResetBoundary);
}

/// Go `TestQueryInjectsSafeArtifactExcerptsAndPreviewEvidence`.
#[test]
fn query_injects_safe_artifact_excerpts_and_preview_evidence() {
    let now = Utc::now();
    let store = FakeStore::new();
    seed_continuity_thread(&store, now);
    let mut turn = continuity_turn(
        "turn_with_artifact",
        "seg_1",
        1,
        ContinuityRole::User,
        "prior user",
        now,
    );
    turn.artifact_excerpt_refs = vec![RuntimeArtifactExcerpt {
        artifact_excerpt_id: "artex_1".to_string(),
        tenant_id: "ten_1".to_string(),
        thread_id: "thr_1".to_string(),
        session_segment_id: "seg_1".to_string(),
        continuity_turn_id: "turn_with_artifact".to_string(),
        resource_kind: "run".to_string(),
        resource_id: "run_1".to_string(),
        excerpt_text: "visible artifact excerpt".to_string(),
        excerpt_source: String::new(),
        created_at: now,
        retention_expires_at: Some(now + chrono::Duration::days(90)),
        redaction_status: RedactionStatus::Redacted,
    }];
    store.add_turn(turn);
    let provider = TestProvider::new("continuity-artifact");
    let dispatcher = new_dispatcher(Arc::new(provider.clone()));
    let svc = service(dispatcher, None, Some(store.clone() as Arc<dyn ChatStore>));
    let execution = svc
        .query(
            QueryInput {
                tenant_id: "ten_1".to_string(),
                thread_id: "thr_1".to_string(),
                provider: "continuity-artifact".to_string(),
                model: "model-a".to_string(),
                query: "follow up".to_string(),
                ..QueryInput::default()
            },
            &CancellationToken::new(),
        )
        .expect("query");
    assert!(provider.saw_message("visible artifact excerpt"));
    let (preview, items) = store.previews().pop().expect("preview persisted");
    assert_eq!(preview.included_count, 2, "turn + artifact items counted");
    assert!(items.iter().any(|item| {
        item.item_kind == ContinuityItemKind::ArtifactExcerpt
            && item.decision == ContinuityDecision::Included
            && item.safe_summary == "visible artifact excerpt"
    }));
}

/// Go `TestQuerySuppressesUnsafeContinuityContent`.
#[test]
fn query_suppresses_unsafe_continuity_content() {
    let now = Utc::now();
    let store = FakeStore::new();
    seed_continuity_thread(&store, now);
    let provider = TestProvider::new("continuity-redaction");
    let dispatcher = new_dispatcher(Arc::new(provider));
    let svc = service(dispatcher, None, Some(store.clone() as Arc<dyn ChatStore>));
    let execution = svc
        .query(
            QueryInput {
                tenant_id: "ten_1".to_string(),
                thread_id: "thr_1".to_string(),
                provider: "continuity-redaction".to_string(),
                model: "model-a".to_string(),
                query: "api_key=sk-secretsecretsecret".to_string(),
                ..QueryInput::default()
            },
            &CancellationToken::new(),
        )
        .expect("query");
    let result = execution.result;
    let turns = store.turns();
    let request_turn = turns
        .iter()
        .find(|turn| turn.continuity_turn_id == result.request_turn_id)
        .expect("request turn persisted");
    assert_eq!(request_turn.safe_content, "suppressed");
    assert_eq!(
        request_turn.content_redaction_status,
        RedactionStatus::Suppressed
    );
}

/// Go `TestQueryDeduplicatesConnectorSourceEventRequestAndResponseTurns`.
#[test]
fn query_deduplicates_connector_source_event_request_and_response_turns() {
    let now = Utc::now();
    let store = FakeStore::new();
    seed_continuity_thread(&store, now);
    let provider = TestProvider::new("continuity-dedupe");
    let dispatcher = new_dispatcher(Arc::new(provider));
    let svc = service(dispatcher, None, Some(store.clone() as Arc<dyn ChatStore>));
    let input = QueryInput {
        tenant_id: "ten_1".to_string(),
        thread_id: "thr_1".to_string(),
        provider: "continuity-dedupe".to_string(),
        model: "model-a".to_string(),
        query: "connector message".to_string(),
        source_kind: Some(SourceKind::Channel),
        source_linkage_id: "src_1".to_string(),
        source_message_id: "msg_1".to_string(),
        source_timestamp: Some(now),
        source_event_key: "connector:delivery_1".to_string(),
        ..QueryInput::default()
    };
    svc.query(input.clone(), &CancellationToken::new())
        .expect("first query");
    svc.query(input, &CancellationToken::new())
        .expect("second query");
    assert_eq!(
        store.turns().len(),
        2,
        "duplicate connector event keeps one request/response pair"
    );
}

// ------------------------------------------------------------------------
// Behavior: stream
// ------------------------------------------------------------------------

/// Go `TestStreamEmitsSelectedSkillContractsOnChunks`.
#[test]
fn stream_emits_selected_skill_contracts_on_chunks() {
    let provider = TestProvider::new("chat-stream");
    let dispatcher = new_dispatcher(Arc::new(provider));
    let registry = skill_registry();
    let svc = service(dispatcher, Some(registry), None);
    let mut chunks: Vec<StreamChunk> = Vec::new();
    let execution = svc
        .stream(
            QueryInput {
                provider: "chat-stream".to_string(),
                model: "model-b".to_string(),
                skills: vec!["shared".to_string()],
                query: "stream hello".to_string(),
                ..QueryInput::default()
            },
            &CancellationToken::new(),
            Some(|chunk| {
                chunks.push(chunk);
                Ok(())
            }),
        )
        .expect("stream");
    assert!(execution.exec_error.is_none());
    assert_eq!(chunks.len(), 2, "expected two stream chunks");
    for chunk in &chunks {
        assert_eq!(
            chunk.skill_contracts.len(),
            1,
            "expected skill contracts on each chunk"
        );
        assert!(!chunk.delta.is_empty());
    }
    assert_eq!(execution.result.skill_contracts.len(), 1);
    assert_eq!(chunks[0].reply, "reply:");
    assert_eq!(chunks[1].reply, "reply:model-b");
    assert_eq!(
        chunks[1].usage,
        Some(Usage {
            input_tokens: 1,
            output_tokens: 1,
            total_tokens: 2,
            ..Default::default()
        })
    );
}

/// Thread + mpsc streaming variant: chunks arrive on the receiver and the
/// join handle carries the final execution.
#[test]
fn stream_channel_emits_chunks_over_mpsc() {
    let provider = TestProvider::new("chat-channel");
    let dispatcher = new_dispatcher(Arc::new(provider));
    let svc = service(dispatcher, None, None);
    let cancel = CancellationToken::new();
    let (rx, handle) = svc
        .stream_channel(
            QueryInput {
                provider: "chat-channel".to_string(),
                model: "model-c".to_string(),
                query: "channel hello".to_string(),
                ..QueryInput::default()
            },
            cancel,
            8,
        )
        .expect("stream_channel");
    let mut chunks: Vec<StreamChunk> = Vec::new();
    while let Ok(chunk) = rx.recv() {
        chunks.push(chunk);
    }
    assert_eq!(chunks.len(), 2, "expected two stream chunks over mpsc");
    let execution = handle
        .join()
        .expect("stream thread panicked")
        .expect("stream execution");
    assert!(execution.exec_error.is_none());
    assert_eq!(execution.result.dispatch.status, DispatchStatus::Completed);
}

/// Killing the caller token cancels the blocking dispatch (Go context
/// cancellation): the final dispatch is Cancelled and surfaced as exec error.
#[test]
fn query_returns_dispatch_cancelled_when_token_killed() {
    let provider = SlowProvider {
        name: "chat-slow".to_string(),
        delay: Duration::from_millis(400),
        requests: Arc::new(RwLock::new(Vec::new())),
    };
    let dispatcher = new_dispatcher(Arc::new(provider));
    let store = FakeStore::new();
    let svc = service(dispatcher, None, Some(store.clone() as Arc<dyn ChatStore>));
    let cancel = CancellationToken::new();
    let cancel_thread = cancel.clone();
    let handle = std::thread::spawn(move || {
        svc.query(base_query("chat-slow", "model-a", "hello"), &cancel_thread)
    });
    std::thread::sleep(Duration::from_millis(80));
    cancel.kill();
    let execution = handle
        .join()
        .expect("query thread panicked")
        .expect("query returns execution");
    let exec_error = execution
        .exec_error
        .expect("exec error expected on cancellation");
    assert!(matches!(exec_error, ChatError::Dispatch(_)));
    assert_eq!(execution.result.dispatch.status, DispatchStatus::Cancelled);
    let persisted = store.dispatches();
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].status, DispatchStatus::Cancelled);
    assert_eq!(persisted[0].error_code, "cancelled");
}
/// Compile-time guard: this manager must be usable from axum `AppState` (Send + Sync).
#[test]
fn manager_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<kura_chat::Service>();
}

// ------------------------------------------------------------------------
// Cancellation token semantics
// ------------------------------------------------------------------------

#[test]
fn cancellation_token_parent_kill_cancels_children() {
    let parent = CancellationToken::new();
    let child = parent.child();
    assert!(!parent.is_cancelled());
    assert!(!child.is_cancelled());
    parent.kill();
    assert!(parent.is_cancelled());
    assert!(child.is_cancelled());
    // Killing a child leaves the parent untouched (Go context semantics).
    let parent = CancellationToken::new();
    let child = parent.child();
    child.kill();
    assert!(child.is_cancelled());
    assert!(!parent.is_cancelled());
    // link_to bridges into the kura-llm cancel token polled by the dispatcher.
    let parent = CancellationToken::new();
    let kura = CancelToken::new();
    let _link = parent.link_to(&kura);
    assert!(!kura.is_cancelled());
    parent.kill();
    assert!(kura.is_cancelled());
}

// ------------------------------------------------------------------------
// kura-store adapter (ported subset + explicit deferred surface)
// ------------------------------------------------------------------------

#[test]
fn sqlite_store_adapter_ports_dispatch_and_defers_continuity() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = kura_store::SQLiteStore::new(dir.path().to_str().expect("path")).expect("store");
    let now = Utc::now();
    let dispatch = Dispatch {
        response_format: None,
        tenant_id: String::new(),
        tools: Vec::new(),
        tool_calls: Vec::new(),
        dispatch_id: "d_1".to_string(),
        provider: "echo".to_string(),
        model: "m".to_string(),
        messages: vec![Message {
            role: MessageRole::User,
            content: "hi".to_string(),
            ..Default::default()
        }],
        stream: false,
        status: DispatchStatus::Completed,
        output: "hello".to_string(),
        finish_reason: "stop".to_string(),
        usage: Usage {
            input_tokens: 1,
            output_tokens: 1,
            total_tokens: 2,
            ..Default::default()
        },
        error_code: String::new(),
        error: String::new(),
        timeout_ms: 30_000,
        partial: false,
        max_retries: 0,
        attempt_count: 1,
        created_at: now,
        updated_at: now,
        started_at: Some(now),
        completed_at: Some(now),
    };
    store
        .upsert_llm_dispatch(&dispatch)
        .expect("upsert dispatch");
    let got = store
        .get_llm_dispatch("d_1")
        .expect("get dispatch")
        .expect("found");
    assert_eq!(got, dispatch);

    // The full ChatStore surface now delegates to the native kura-store
    // implementations: continuity round-trips against real SQLite.
    // Continuity turns FK onto threads + segments: seed those first.
    store
        .upsert_thread(&Thread {
            thread_id: "thr_adapter".to_string(),
            tenant_id: "ten_adapter".to_string(),
            lifecycle_state: kura_threads::LifecycleState::Active,
            current_session_segment_id: "seg".to_string(),
            source_kind: SourceKind::Chat,
            source_summary: "adapter test".to_string(),
            last_activity_at: now,
            created_at: now,
            updated_at: now,
            retention_expires_at: None,
            redaction_status: RedactionStatus::Redacted,
        })
        .expect("seed thread");
    store
        .upsert_thread_session_segment(&kura_threads::SessionSegment {
            session_segment_id: "seg".to_string(),
            thread_id: "thr_adapter".to_string(),
            tenant_id: "ten_adapter".to_string(),
            session_id: String::new(),
            generation: 1,
            state: "active".to_string(),
            started_at: now,
            ended_at: None,
            last_active_at: now,
            reset_from_session_segment_id: String::new(),
            partial_evidence: false,
        })
        .expect("seed segment");
    let chat_store: &dyn ChatStore = &std::sync::Mutex::new(store);
    let mut turn = continuity_turn("t", "seg", 1, ContinuityRole::User, "x", now);
    turn.tenant_id = "ten_adapter".to_string();
    turn.thread_id = "thr_adapter".to_string();
    let saved = chat_store
        .save_continuity_turn(&turn)
        .expect("save continuity turn against real sqlite");
    assert!(!saved.continuity_turn_id.is_empty());
    let listed = chat_store
        .list_continuity_turns(&kura_chat::ContinuityLookupQuery {
            tenant_id: "ten_adapter".to_string(),
            thread_id: "thr_adapter".to_string(),
            session_segment_id: "seg".to_string(),
            ..Default::default()
        })
        .expect("list continuity turns against real sqlite");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].continuity_turn_id, saved.continuity_turn_id);

    // Profile selection resolves (the store seeds a default profile), and
    // binding resolution composes through the precedence port.
    let selection = chat_store
        .active_agent_profile_selection("ten_adapter")
        .expect("active profile selection");
    assert!(selection.is_some(), "default profile seeded and selected");
    let resolution = chat_store
        .resolve_binding_selection(&kura_chat::BindingResolutionParams {
            tenant_id: "ten_adapter".to_string(),
            ..Default::default()
        })
        .expect("binding resolution");
    // No bindings and no defaults on a fresh tenant: outcome is a
    // deterministic non-panicking selection (exact outcome is the
    // precedence port's business; the adapter just must not defer).
    let _ = resolution;
}

// ------------------------------------------------------------------------
// Wire round-trips (camelCase, Go wire values)
// ------------------------------------------------------------------------

#[test]
fn query_input_round_trips_camel_case_wire() {
    let input = QueryInput {
        query: "hello".to_string(),
        provider: "echo".to_string(),
        model: "m".to_string(),
        skills: vec!["s1".to_string()],
        timeout_ms: 1000,
        max_retries: 2,
        tenant_id: "ten_1".to_string(),
        thread_id: "thr_1".to_string(),
        continuity_mode: Some(ContinuityMode::Disabled),
        scope: Scope {
            run_id: "run_1".to_string(),
            ..Scope::default()
        },
        source_kind: Some(SourceKind::Channel),
        source_linkage_id: "src_1".to_string(),
        source_message_id: "msg_1".to_string(),
        source_timestamp: Some(Utc::now()),
        source_event_key: "k".to_string(),
        channel_scope_ref: "chan_1".to_string(),
        account_scope_ref: "acct_1".to_string(),
        run_id: "run_9".to_string(),
        // Not on the wire, so empty for the round trip to compare equal.
        request_id: String::new(),
        instructions: String::new(),
        response_format: Some(kura_llm::ResponseFormat {
            name: "answer".to_string(),
            schema: serde_json::json!({"type": "object"}),
        }),
    };
    let json = serde_json::to_value(&input).expect("serialize");
    assert_eq!(json["query"], "hello");
    assert_eq!(json["continuityMode"], "disabled");
    assert_eq!(json["sourceKind"], "channel");
    assert_eq!(json["scope"]["runId"], "run_1");
    assert_eq!(json["channelScopeRef"], "chan_1");
    let back: QueryInput = serde_json::from_value(json).expect("deserialize");
    assert_eq!(back, input);
}

#[test]
fn query_result_round_trips_with_dispatch() {
    let now = Utc::now();
    let dispatch = Dispatch {
        response_format: None,
        tenant_id: String::new(),
        tools: Vec::new(),
        tool_calls: Vec::new(),
        dispatch_id: "d_1".to_string(),
        provider: "echo".to_string(),
        model: "m".to_string(),
        messages: vec![Message {
            role: MessageRole::User,
            content: "hi".to_string(),
            ..Default::default()
        }],
        stream: true,
        status: DispatchStatus::Completed,
        output: "hello".to_string(),
        finish_reason: "stop".to_string(),
        usage: Usage {
            input_tokens: 1,
            output_tokens: 1,
            total_tokens: 2,
            ..Default::default()
        },
        error_code: String::new(),
        error: String::new(),
        timeout_ms: 30_000,
        partial: false,
        max_retries: 0,
        attempt_count: 1,
        created_at: now,
        updated_at: now,
        started_at: Some(now),
        completed_at: Some(now),
    };
    let result = QueryResult {
        turn_id: "turn_0".to_string(),
        query: "hello".to_string(),
        skills: vec!["shared".to_string()],
        skill_contracts: vec![
            serde_json::json!({"declaration": {"consumerKind": "skill"}})
                .as_object()
                .expect("object")
                .clone(),
        ],
        dispatch,
        thread_id: "thr_1".to_string(),
        session_segment_id: "seg_1".to_string(),
        request_turn_id: "turn_1".to_string(),
        response_turn_id: "turn_2".to_string(),
        continuity_preview_id: "contprev_1".to_string(),
        continuity_applied: true,
        continuity_status: Some(ContinuityStatus::Applied),
        continuity_included_count: 1,
        continuity_excluded_count: 0,
        structured_output: Some(serde_json::json!({"ok": true})),
    };
    let json = serde_json::to_value(&result).expect("serialize");
    assert_eq!(json["structuredOutput"]["ok"], true);
    assert_eq!(json["continuityStatus"], "applied");
    assert_eq!(json["dispatch"]["status"], "completed");
    let back: QueryResult = serde_json::from_value(json).expect("deserialize");
    assert_eq!(back, result);
}

#[test]
fn stream_chunk_round_trips_camel_case_wire() {
    let chunk = StreamChunk {
        dispatch_id: "d_1".to_string(),
        provider: "echo".to_string(),
        model: "m".to_string(),
        skills: vec!["s".to_string()],
        skill_contracts: vec![serde_json::json!({"a": 1}).as_object().expect("o").clone()],
        delta: "hi".to_string(),
        reply: "hi".to_string(),
        finish_reason: "stop".to_string(),
        usage: Some(Usage {
            input_tokens: 1,
            output_tokens: 1,
            total_tokens: 2,
            ..Default::default()
        }),
        thread_id: "thr".to_string(),
        session_segment_id: "seg".to_string(),
        request_turn_id: "turn".to_string(),
        continuity_preview_id: "contprev".to_string(),
        continuity_applied: true,
        continuity_status: Some(ContinuityStatus::Applied),
    };
    let json = serde_json::to_value(&chunk).expect("serialize");
    assert_eq!(json["finishReason"], "stop");
    assert_eq!(json["usage"]["totalTokens"], 2);
    let back: StreamChunk = serde_json::from_value(json).expect("deserialize");
    assert_eq!(back, chunk);
}

// ------------------------------------------------------------------------
// Pure helpers
// ------------------------------------------------------------------------

#[test]
fn compile_prompt_messages_lays_out_overlays_skills_and_query() {
    let overlay = kura_skills::Overlay {
        overlay_id: "o1".to_string(),
        source: kura_skills::Source::DataDir,
        body: "be helpful".to_string(),
        ..kura_skills::Overlay::default()
    };
    let skill = kura_skills::Skill {
        skill_id: "shared".to_string(),
        name: "shared".to_string(),
        description: "data skill".to_string(),
        body: "data instructions".to_string(),
        ..kura_skills::Skill::default()
    };
    let messages = compile_prompt_messages("hello", &[skill], vec![overlay]);
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].role, MessageRole::System);
    assert_eq!(messages[0].content, "Agent overlay (data_dir):\nbe helpful");
    assert!(messages[1].content.starts_with("Skill: shared"));
    assert!(messages[1].content.contains("Description: data skill"));
    assert!(
        messages[1]
            .content
            .contains("Instructions:\ndata instructions")
    );
    assert_eq!(
        messages[2],
        Message {
            role: MessageRole::User,
            content: "hello".to_string(),
            ..Default::default()
        }
    );
}

#[test]
fn inject_continuity_messages_inserts_before_first_user_message() {
    let now = Utc::now();
    let turn = continuity_turn("t1", "seg", 1, ContinuityRole::User, "prior", now);
    let base = vec![
        Message {
            role: MessageRole::System,
            content: "sys".to_string(),
            ..Default::default()
        },
        Message {
            role: MessageRole::User,
            content: "current".to_string(),
            ..Default::default()
        },
    ];
    let out = inject_continuity_messages(&base, &[turn]);
    assert_eq!(out.len(), 3);
    assert_eq!(out[0], base[0]);
    assert_eq!(
        out[1],
        Message {
            role: MessageRole::User,
            content: "prior".to_string(),
            ..Default::default()
        }
    );
    assert_eq!(out[2], base[1]);
}

#[test]
fn terminal_dispatch_event_names_match_go() {
    let now = Utc::now();
    let base = Dispatch {
        response_format: None,
        tenant_id: String::new(),
        tools: Vec::new(),
        tool_calls: Vec::new(),
        dispatch_id: "d".to_string(),
        provider: "p".to_string(),
        model: "m".to_string(),
        messages: Vec::new(),
        stream: false,
        status: DispatchStatus::Completed,
        output: String::new(),
        finish_reason: String::new(),
        usage: Usage::default(),
        error_code: String::new(),
        error: String::new(),
        timeout_ms: 0,
        partial: false,
        max_retries: 0,
        attempt_count: 0,
        created_at: now,
        updated_at: now,
        started_at: None,
        completed_at: None,
    };
    let with_status = |status| Dispatch {
        status,
        ..base.clone()
    };
    assert_eq!(
        terminal_dispatch_event(&with_status(DispatchStatus::Completed)),
        "llm.dispatch.completed"
    );
    assert_eq!(
        terminal_dispatch_event(&with_status(DispatchStatus::PartialFailed)),
        "llm.dispatch.partial_failed"
    );
    assert_eq!(
        terminal_dispatch_event(&with_status(DispatchStatus::Failed)),
        "llm.dispatch.failed"
    );
    assert_eq!(
        terminal_dispatch_event(&with_status(DispatchStatus::Cancelled)),
        "llm.dispatch.cancelled"
    );
}

#[test]
fn continuity_source_kind_normalizes_unknown_to_chat() {
    assert_eq!(continuity_source_kind(None), SourceKind::Chat);
    assert_eq!(
        continuity_source_kind(Some(SourceKind::Chat)),
        SourceKind::Chat
    );
    assert_eq!(
        continuity_source_kind(Some(SourceKind::Channel)),
        SourceKind::Channel
    );
    assert_eq!(
        continuity_source_kind(Some(SourceKind::Workflow)),
        SourceKind::Workflow
    );
    assert_eq!(
        continuity_source_kind(Some(SourceKind::Schedule)),
        SourceKind::Schedule
    );
    assert_eq!(
        continuity_source_kind(Some(SourceKind::Shell)),
        SourceKind::Shell
    );
    assert_eq!(
        continuity_source_kind(Some(SourceKind::Legacy)),
        SourceKind::Legacy
    );
}

#[test]
fn response_continuity_source_event_key_appends_assistant_suffix() {
    assert_eq!(response_continuity_source_event_key(""), "");
    assert_eq!(response_continuity_source_event_key("  "), "");
    assert_eq!(
        response_continuity_source_event_key("connector:delivery_1"),
        "connector:delivery_1:assistant"
    );
}

fn blocked_openai_setup_session() -> SetupSession {
    SetupSession {
        setup_session_id: "setup_blocked_openai".to_string(),
        tenant_id: "ten_chat_setup".to_string(),
        actor_principal_id: String::new(),
        target_id: TARGET_OPENAI_COMPATIBLE.to_string(),
        target_kind: TargetKind::Provider,
        setup_style: SetupStyle::SubmittedSecret,
        state: SetupState::ActionRequired,
        reason_code: "credential_missing".to_string(),
        retryable: true,
        remediation_owner: RemediationOwner::TenantAdmin,
        safe_use_mode: SafeUseMode::Blocked,
        allowed_capabilities: Vec::new(),
        current_attempt_id: String::new(),
        diagnostic_result_id: String::new(),
        diagnostic_run_id: String::new(),
        diagnostic_stage: String::new(),
        diagnostic_source_kind: String::new(),
        diagnostic_source_id: String::new(),
        diagnostic_allowed_use: Vec::new(),
        redaction_status: kura_setupwizard::RedactionStatus::Redacted,
        resource_refs: Vec::new(),
        redacted_evidence: HashMap::new(),
        oauth_state_ref: String::new(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        last_transition_at: Utc::now(),
        last_transition_audit_id: String::new(),
        operator_remediation: String::new(),
        user_remediation: String::new(),
        unsupported_reason_code: String::new(),
    }
}

// ------------------------------------------------------------------------
// Behavior: plugin hook points (pluginization phase 2)
// ------------------------------------------------------------------------

struct RewriteQueryHook;
impl kura_plugin::Hook for RewriteQueryHook {
    fn handle(&self, payload: &mut serde_json::Value) -> kura_plugin::HookOutcome {
        payload["query"] = serde_json::Value::String("rewritten query".to_string());
        kura_plugin::HookOutcome::Continue
    }
}

struct InjectWindowHook;
impl kura_plugin::Hook for InjectWindowHook {
    fn handle(&self, payload: &mut serde_json::Value) -> kura_plugin::HookOutcome {
        let messages = payload["messages"].as_array_mut().expect("messages array");
        messages.insert(
            0,
            serde_json::json!({ "role": "system", "content": "session-strategy window" }),
        );
        kura_plugin::HookOutcome::Continue
    }
}

struct VetoHook;
impl kura_plugin::Hook for VetoHook {
    fn handle(&self, _payload: &mut serde_json::Value) -> kura_plugin::HookOutcome {
        kura_plugin::HookOutcome::Halt("tenant policy forbids this turn".to_string())
    }
}

struct RecordTurnEndHook(Arc<RwLock<Option<serde_json::Value>>>);
impl kura_plugin::Hook for RecordTurnEndHook {
    fn handle(&self, payload: &mut serde_json::Value) -> kura_plugin::HookOutcome {
        *self.0.write() = Some(payload.clone());
        kura_plugin::HookOutcome::Continue
    }
}

/// A turn-start veto surfaces as HookVetoed before any dispatch exists, and
/// the veto is recorded as a `chat.hook.vetoed` event.
#[test]
fn turn_start_hook_veto_blocks_the_turn() {
    let provider = TestProvider::new("chat-test");
    let dispatcher = new_dispatcher(Arc::new(provider.clone()));
    let store = FakeStore::new();
    let mut svc = service(dispatcher, None, Some(store.clone() as Arc<dyn ChatStore>));
    let hooks = Arc::new(kura_plugin::HookBus::new());
    hooks.register(
        kura_plugin::points::CHAT_TURN_START,
        "policy-plugin",
        Arc::new(VetoHook),
    );
    svc.set_hooks(hooks);

    let err = svc
        .query(
            base_query("chat-test", "m1", "hello"),
            &CancellationToken::new(),
        )
        .expect_err("veto must fail the turn");
    match err {
        ChatError::HookVetoed {
            point,
            plugin_id,
            reason,
        } => {
            assert_eq!(point, kura_plugin::points::CHAT_TURN_START);
            assert_eq!(plugin_id, "policy-plugin");
            assert_eq!(reason, "tenant policy forbids this turn");
        }
        other => panic!("expected HookVetoed, got {other:?}"),
    }
    assert!(
        provider.requests.read().is_empty(),
        "no dispatch reached the provider"
    );
    assert!(store.dispatches().is_empty(), "no dispatch persisted");
    assert!(
        store
            .inner
            .read()
            .events
            .iter()
            .any(|event| event.name == "chat.hook.vetoed"),
        "veto recorded as chat.hook.vetoed event"
    );
}

/// Pre-dispatch mutations are both model-visible (the provider request
/// carries them) and logged (the persisted dispatch record carries the same
/// messages): the "model-visible = logged" invariant.
#[test]
fn pre_dispatch_mutation_is_model_visible_and_logged() {
    let provider = TestProvider::new("chat-test");
    let dispatcher = new_dispatcher(Arc::new(provider.clone()));
    let store = FakeStore::new();
    let mut svc = service(dispatcher, None, Some(store.clone() as Arc<dyn ChatStore>));
    let hooks = Arc::new(kura_plugin::HookBus::new());
    hooks.register(
        kura_plugin::points::CHAT_PRE_DISPATCH,
        "session-strategy",
        Arc::new(InjectWindowHook),
    );
    svc.set_hooks(hooks);

    let execution = svc
        .query(
            base_query("chat-test", "m1", "hello"),
            &CancellationToken::new(),
        )
        .expect("query succeeds");
    assert!(execution.exec_error.is_none());

    assert!(
        provider.saw_message("session-strategy window"),
        "the model saw the hook-injected message"
    );
    let dispatches = store.dispatches();
    assert!(!dispatches.is_empty());
    assert!(
        dispatches.iter().all(|dispatch| dispatch
            .messages
            .first()
            .is_some_and(|message| message.content == "session-strategy window")),
        "every persisted dispatch record logs exactly what the model saw"
    );
}

/// turn-start can rewrite the query and turn-end observes the settled turn.
#[test]
fn turn_start_rewrite_and_turn_end_observation() {
    let provider = TestProvider::new("chat-test");
    let dispatcher = new_dispatcher(Arc::new(provider.clone()));
    let store = FakeStore::new();
    let mut svc = service(dispatcher, None, Some(store.clone() as Arc<dyn ChatStore>));
    let hooks = Arc::new(kura_plugin::HookBus::new());
    let seen = Arc::new(RwLock::new(None));
    hooks.register(
        kura_plugin::points::CHAT_TURN_START,
        "rewriter",
        Arc::new(RewriteQueryHook),
    );
    hooks.register(
        kura_plugin::points::CHAT_TURN_END,
        "observer",
        Arc::new(RecordTurnEndHook(Arc::clone(&seen))),
    );
    svc.set_hooks(hooks);

    let execution = svc
        .query(
            base_query("chat-test", "m1", "original query"),
            &CancellationToken::new(),
        )
        .expect("query succeeds");
    assert_eq!(execution.result.query, "rewritten query");
    assert!(
        provider.saw_message("rewritten query"),
        "the model saw the rewritten query"
    );
    let payload = seen.read().clone().expect("turn-end hook ran");
    assert_eq!(payload["query"], "rewritten query");
    assert_eq!(payload["output"], "reply:m1");
    assert_eq!(payload["status"], "completed");
    assert_eq!(
        payload["dispatchId"],
        execution.result.dispatch.dispatch_id.as_str()
    );
}

/// The stream path runs the same hook points as query.
#[test]
fn stream_runs_hook_points() {
    let provider = TestProvider::new("chat-test");
    let dispatcher = new_dispatcher(Arc::new(provider.clone()));
    let store = FakeStore::new();
    let mut svc = service(dispatcher, None, Some(store.clone() as Arc<dyn ChatStore>));
    let hooks = Arc::new(kura_plugin::HookBus::new());
    hooks.register(
        kura_plugin::points::CHAT_PRE_DISPATCH,
        "session-strategy",
        Arc::new(InjectWindowHook),
    );
    svc.set_hooks(hooks);

    let mut chunks: Vec<StreamChunk> = Vec::new();
    let execution = svc
        .stream(
            base_query("chat-test", "m1", "hello"),
            &CancellationToken::new(),
            Some(|chunk: StreamChunk| {
                chunks.push(chunk);
                Ok(())
            }),
        )
        .expect("stream succeeds");
    assert!(execution.exec_error.is_none());
    assert!(!chunks.is_empty());
    assert!(provider.saw_message("session-strategy window"));
    let dispatches = store.dispatches();
    assert!(dispatches.iter().all(|dispatch| {
        dispatch
            .messages
            .first()
            .is_some_and(|message| message.content == "session-strategy window")
    }));
}

// ------------------------------------------------------------------------
// Tool loop
//
// A turn is one or more dispatches. When the model asks to call something the
// calls are run, their results are appended, and the next round is prepared and
// dispatched like any other -- so every round is hooked, persisted and evented,
// and a turn that used tools can be read back round by round. Before this a
// turn was always exactly one dispatch: the model was offered no tools, and a
// call it asked for anyway was discarded before the service could see it.
// ------------------------------------------------------------------------

/// A provider that replays one scripted response per call.
struct ScriptedProvider {
    rounds: parking_lot::Mutex<std::collections::VecDeque<ProviderResponse>>,
    seen: Arc<parking_lot::RwLock<Vec<ProviderRequest>>>,
}

impl ScriptedProvider {
    fn new(
        rounds: Vec<ProviderResponse>,
    ) -> (Arc<Self>, Arc<parking_lot::RwLock<Vec<ProviderRequest>>>) {
        let seen = Arc::new(parking_lot::RwLock::new(Vec::new()));
        (
            Arc::new(Self {
                rounds: parking_lot::Mutex::new(rounds.into()),
                seen: Arc::clone(&seen),
            }),
            seen,
        )
    }
}

impl Provider for ScriptedProvider {
    fn name(&self) -> &str {
        OPENAI_COMPATIBLE_PROVIDER_NAME
    }

    fn complete<'a>(
        &'a self,
        request: ProviderRequest,
    ) -> BoxFuture<'a, Result<ProviderResponse, ProviderError>> {
        self.seen.write().push(request);
        let next = self.rounds.lock().pop_front();
        Box::pin(async move {
            Ok(next.unwrap_or(ProviderResponse {
                output: "unscripted".to_string(),
                ..ProviderResponse::default()
            }))
        })
    }

    fn stream<'a>(
        &'a self,
        request: ProviderRequest,
        _emit: kura_llm::StreamEmitter<'a>,
    ) -> BoxFuture<'a, Result<ProviderResponse, ProviderError>> {
        self.complete(request)
    }
}

/// A source that always answers with the same registry.
struct FixedTools(Arc<kura_core::ToolRegistry>);

impl kura_chat::ToolSource for FixedTools {
    fn registry(&self, _turn: &kura_chat::ToolTurn) -> Arc<kura_core::ToolRegistry> {
        Arc::clone(&self.0)
    }
}

/// A tool that records what it was asked and answers with fixed text.
struct RecordingTool {
    name: String,
    answer: String,
    calls: Arc<parking_lot::RwLock<Vec<String>>>,
}

impl kura_core::Tool for RecordingTool {
    fn spec(&self) -> kura_llm::ToolSpec {
        kura_llm::ToolSpec {
            name: self.name.clone(),
            description: "a test tool".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        }
    }

    fn call<'a>(
        &'a self,
        invocation: &'a kura_core::ToolInvocation,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<kura_core::ToolOutput, kura_core::ToolError>>
                + Send
                + 'a,
        >,
    > {
        self.calls.write().push(invocation.arguments.clone());
        let answer = self.answer.clone();
        Box::pin(async move { Ok(kura_core::ToolOutput::ok(answer)) })
    }
}

fn asked_for(call_id: &str, name: &str, arguments: &str) -> ProviderResponse {
    ProviderResponse {
        tool_calls: vec![kura_llm::ToolCall {
            call_id: call_id.to_string(),
            name: name.to_string(),
            arguments: arguments.to_string(),
        }],
        finish_reason: "tool_calls".to_string(),
        ..ProviderResponse::default()
    }
}

fn answered(text: &str) -> ProviderResponse {
    ProviderResponse {
        output: text.to_string(),
        finish_reason: "stop".to_string(),
        ..ProviderResponse::default()
    }
}

fn service_with_tool(
    rounds: Vec<ProviderResponse>,
    tool: Arc<RecordingTool>,
) -> (Service, Arc<parking_lot::RwLock<Vec<ProviderRequest>>>) {
    let (provider, seen) = ScriptedProvider::new(rounds);
    let mut registry = kura_core::ToolRegistry::new();
    registry.register(tool);
    let mut svc = service(new_dispatcher(provider), None, None);
    svc.set_tools(Arc::new(FixedTools(Arc::new(registry))));
    (svc, seen)
}

#[test]
fn a_turn_runs_the_call_the_model_asked_for_and_answers_with_the_result() {
    let calls = Arc::new(parking_lot::RwLock::new(Vec::new()));
    let tool = Arc::new(RecordingTool {
        name: "loopforge_status".to_string(),
        answer: "{\"stage\":\"DISCOVERY\"}".to_string(),
        calls: Arc::clone(&calls),
    });
    let (svc, seen) = service_with_tool(
        vec![
            asked_for("call_1", "loopforge_status", "{}"),
            answered("You are in DISCOVERY."),
        ],
        tool,
    );

    let execution = svc
        .query(
            base_query(OPENAI_COMPATIBLE_PROVIDER_NAME, "m", "where am i"),
            &CancellationToken::new(),
        )
        .expect("query");

    assert!(execution.exec_error.is_none());
    assert_eq!(execution.result.dispatch.output, "You are in DISCOVERY.");
    // The tool ran, once, with what the model sent.
    assert_eq!(*calls.read(), vec!["{}".to_string()]);
    // Two rounds: one that asked, one that answered.
    assert_eq!(seen.read().len(), 2);
}

#[test]
fn the_second_round_shows_the_model_its_own_call_and_the_result() {
    // Without both, the model sees a result attached to nothing and asks for
    // the same call again.
    let tool = Arc::new(RecordingTool {
        name: "loopforge_status".to_string(),
        answer: "{\"stage\":\"DISCOVERY\"}".to_string(),
        calls: Arc::new(parking_lot::RwLock::new(Vec::new())),
    });
    let (svc, seen) = service_with_tool(
        vec![
            asked_for("call_1", "loopforge_status", "{}"),
            answered("done"),
        ],
        tool,
    );

    svc.query(
        base_query(OPENAI_COMPATIBLE_PROVIDER_NAME, "m", "where am i"),
        &CancellationToken::new(),
    )
    .expect("query");

    let requests = seen.read();
    let second = &requests[1];
    let assistant = second
        .messages
        .iter()
        .find(|m| m.role == MessageRole::Assistant)
        .expect("the round that asked must be replayed");
    assert_eq!(assistant.tool_calls.len(), 1);
    let result = second
        .messages
        .iter()
        .find(|m| m.role == MessageRole::Tool)
        .expect("the result must be replayed");
    assert_eq!(result.tool_call_id, "call_1");
    // Framed as data the tool returned (2.8, G9).
    assert_eq!(
        result.content,
        "<tool_output>\n{\"stage\":\"DISCOVERY\"}\n</tool_output>"
    );
    let system = second
        .messages
        .iter()
        .find(|m| m.role == MessageRole::System)
        .expect("the notice rides on a system message");
    assert!(system.content.contains("<tool_output>"));
}

#[test]
fn every_round_is_offered_the_same_tools() {
    let tool = Arc::new(RecordingTool {
        name: "loopforge_status".to_string(),
        answer: "ok".to_string(),
        calls: Arc::new(parking_lot::RwLock::new(Vec::new())),
    });
    let (svc, seen) = service_with_tool(
        vec![
            asked_for("call_1", "loopforge_status", "{}"),
            answered("done"),
        ],
        tool,
    );

    svc.query(
        base_query(OPENAI_COMPATIBLE_PROVIDER_NAME, "m", "hi"),
        &CancellationToken::new(),
    )
    .expect("query");

    for request in seen.read().iter() {
        assert_eq!(request.tools.len(), 1, "a round was offered no tools");
        assert_eq!(request.tools[0].name, "loopforge_status");
    }
}

#[test]
fn a_turn_that_needs_no_tool_is_still_one_dispatch() {
    // The ordinary path must not gain a round.
    let tool = Arc::new(RecordingTool {
        name: "loopforge_status".to_string(),
        answer: "ok".to_string(),
        calls: Arc::new(parking_lot::RwLock::new(Vec::new())),
    });
    let (svc, seen) = service_with_tool(vec![answered("hello")], tool);

    let execution = svc
        .query(
            base_query(OPENAI_COMPATIBLE_PROVIDER_NAME, "m", "hi"),
            &CancellationToken::new(),
        )
        .expect("query");

    assert_eq!(execution.result.dispatch.output, "hello");
    assert_eq!(seen.read().len(), 1);
}

#[test]
fn a_service_with_no_registry_offers_nothing_and_runs_one_round() {
    // Absent registry = the pipeline before any of this existed.
    let (provider, seen) = ScriptedProvider::new(vec![answered("hello")]);
    let svc = service(new_dispatcher(provider), None, None);

    svc.query(
        base_query(OPENAI_COMPATIBLE_PROVIDER_NAME, "m", "hi"),
        &CancellationToken::new(),
    )
    .expect("query");

    assert_eq!(seen.read().len(), 1);
    assert!(seen.read()[0].tools.is_empty());
}

#[test]
fn a_failing_tool_is_reported_to_the_model_rather_than_ending_the_turn() {
    // The model asked for something and is owed an answer; "no such tool" is
    // something it can act on. Aborting would strand the user with nothing.
    let tool = Arc::new(RecordingTool {
        name: "loopforge_status".to_string(),
        answer: "ok".to_string(),
        calls: Arc::new(parking_lot::RwLock::new(Vec::new())),
    });
    let (svc, seen) = service_with_tool(
        vec![
            asked_for("call_1", "no_such_tool", "{}"),
            answered("I could not do that."),
        ],
        tool,
    );

    let execution = svc
        .query(
            base_query(OPENAI_COMPATIBLE_PROVIDER_NAME, "m", "do it"),
            &CancellationToken::new(),
        )
        .expect("query");

    assert!(execution.exec_error.is_none());
    assert_eq!(execution.result.dispatch.output, "I could not do that.");
    let requests = seen.read();
    let reported = requests[1]
        .messages
        .iter()
        .find(|m| m.role == MessageRole::Tool)
        .expect("the failure must reach the model");
    assert!(
        reported.content.contains("no_such_tool"),
        "{}",
        reported.content
    );
}

#[test]
fn a_turn_that_never_stops_calling_is_failed_rather_than_run_forever() {
    // A misbehaving model, or a tool that always provokes another call, would
    // otherwise burn provider quota without bound.
    let tool = Arc::new(RecordingTool {
        name: "loopforge_status".to_string(),
        answer: "ok".to_string(),
        calls: Arc::new(parking_lot::RwLock::new(Vec::new())),
    });
    let rounds: Vec<ProviderResponse> = (0..10)
        .map(|_| asked_for("call_1", "loopforge_status", "{}"))
        .collect();
    let (mut svc, seen) = service_with_tool(rounds, tool);
    svc.set_max_tool_rounds(3);

    let error = svc
        .query(
            base_query(OPENAI_COMPATIBLE_PROVIDER_NAME, "m", "loop"),
            &CancellationToken::new(),
        )
        .expect_err("a turn that never stops must fail");

    // The loop's own wording: the cap belongs to `kura-core`, which is what
    // counts the rounds.
    assert!(
        error.to_string().contains("exceeded 3 tool rounds"),
        "{error}"
    );
    assert_eq!(seen.read().len(), 3);
}

/// A source that counts how often it was asked, and can change its answer.
struct CountingTools {
    asked: Arc<parking_lot::RwLock<usize>>,
    registry: Arc<parking_lot::RwLock<Arc<kura_core::ToolRegistry>>>,
}

impl kura_chat::ToolSource for CountingTools {
    fn registry(&self, _turn: &kura_chat::ToolTurn) -> Arc<kura_core::ToolRegistry> {
        *self.asked.write() += 1;
        Arc::clone(&self.registry.read())
    }
}

#[test]
fn the_tools_are_asked_for_per_turn_rather_than_snapshotted() {
    // MCP servers are connected and stopped while the daemon runs. A registry
    // captured at assembly would be whatever existed at boot -- which is
    // nothing, because a server registered a moment later would never appear
    // and every turn would silently offer no tools.
    let asked = Arc::new(parking_lot::RwLock::new(0usize));
    let registry = Arc::new(parking_lot::RwLock::new(Arc::new(
        kura_core::ToolRegistry::new(),
    )));
    let (provider, seen) = ScriptedProvider::new(vec![answered("one"), answered("two")]);
    let mut svc = service(new_dispatcher(provider), None, None);
    svc.set_tools(Arc::new(CountingTools {
        asked: Arc::clone(&asked),
        registry: Arc::clone(&registry),
    }));

    svc.query(
        base_query(OPENAI_COMPATIBLE_PROVIDER_NAME, "m", "hi"),
        &CancellationToken::new(),
    )
    .expect("first turn");

    // A tool appears between the turns, the way one does when a user connects
    // a server.
    let mut grown = kura_core::ToolRegistry::new();
    grown.register(Arc::new(RecordingTool {
        name: "loopforge_status".to_string(),
        answer: "ok".to_string(),
        calls: Arc::new(parking_lot::RwLock::new(Vec::new())),
    }));
    *registry.write() = Arc::new(grown);

    svc.query(
        base_query(OPENAI_COMPATIBLE_PROVIDER_NAME, "m", "again"),
        &CancellationToken::new(),
    )
    .expect("second turn");

    assert_eq!(*asked.read(), 2, "the source was not consulted per turn");
    let requests = seen.read();
    assert!(
        requests[0].tools.is_empty(),
        "the first turn should have had none"
    );
    assert_eq!(
        requests[1].tools.len(),
        1,
        "the second turn did not see the new tool"
    );
}

// ------------------------------------------------------------------------
// Turn trace (agent-runtime-program.md 2.1)
//
// Every turn leaves a span tree -- turn, rounds, the dispatch each round sent,
// the calls it asked for -- readable by turn id, whether it answered, failed
// or never got as far as a dispatch.
// ------------------------------------------------------------------------

fn with_usage(mut response: ProviderResponse, input: i64, output: i64) -> ProviderResponse {
    response.usage = kura_llm::Usage {
        input_tokens: input,
        output_tokens: output,
        total_tokens: input + output,
        ..Default::default()
    };
    response
}

fn spans_of<'a>(
    trace: &'a kura_telemetry::trace::TurnTrace,
    kind: &str,
) -> Vec<&'a kura_telemetry::trace::Span> {
    trace
        .spans
        .iter()
        .filter(|span| span.kind == kind)
        .collect()
}

#[test]
fn a_tool_turn_is_traced_round_by_round_with_its_calls_and_usage() {
    let tool = Arc::new(RecordingTool {
        name: "loopforge_status".to_string(),
        answer: "SECRET-TOOL-OUTPUT".to_string(),
        calls: Arc::new(parking_lot::RwLock::new(Vec::new())),
    });
    let (svc, _) = service_with_tool(
        vec![
            with_usage(
                asked_for("call_1", "loopforge_status", "{\"q\":\"SECRET-ARG\"}"),
                10,
                2,
            ),
            {
                // Round 2 resends round 1's prefix, which the provider cached.
                let mut response = with_usage(answered("done"), 15, 3);
                response.usage.cache_read_input_tokens = 12;
                response
            },
        ],
        tool,
    );
    let mut input = base_query(OPENAI_COMPATIBLE_PROVIDER_NAME, "m", "SECRET-QUESTION");
    input.tenant_id = "ten_a".to_string();
    input.request_id = "req_trace".to_string();

    let execution = svc.query(input, &CancellationToken::new()).expect("query");

    let turn_id = execution.result.turn_id.clone();
    assert!(turn_id.starts_with("turn_"), "{turn_id}");
    let trace = svc.traces().get(&turn_id).expect("the turn is traced");
    assert_eq!(trace.status, kura_telemetry::trace::SpanStatus::Ok);
    assert_eq!(trace.tenant_id, "ten_a");
    assert_eq!(trace.request_id, "req_trace");
    assert_eq!(trace.usage.total_tokens, 30, "summed over both rounds");

    let rounds = spans_of(&trace, "round");
    assert_eq!(rounds.len(), 2);
    assert!(rounds.iter().all(|round| round.parent_span_id == Some(0)));
    let dispatches = spans_of(&trace, "dispatch");
    assert_eq!(dispatches.len(), 2);
    for (round, dispatch) in rounds.iter().zip(&dispatches) {
        assert_eq!(dispatch.parent_span_id, Some(round.span_id));
        assert_eq!(dispatch.attributes["attemptCount"], 1);
    }
    // The estimator's prediction sits next to what the provider reported;
    // round 2 carries round 1's call and result, so it predicts more.
    let estimated = |span: &kura_telemetry::trace::Span| {
        span.attributes["estimatedInputTokens"]
            .as_u64()
            .expect("estimatedInputTokens")
    };
    assert!(estimated(dispatches[0]) > 0);
    assert!(estimated(dispatches[1]) > estimated(dispatches[0]));
    assert_eq!(dispatches[0].attributes["inputTokens"], 10);
    assert!(
        !dispatches[0]
            .attributes
            .contains_key("cacheReadInputTokens")
    );
    assert_eq!(dispatches[1].attributes["cacheReadInputTokens"], 12);
    assert_eq!(trace.usage.cache_read_input_tokens, 12);
    assert_eq!(
        dispatches[1].attributes["dispatchId"],
        execution.result.dispatch.dispatch_id.as_str()
    );
    // The call hangs off the round that asked for it.
    let calls = spans_of(&trace, "tool_call");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "loopforge_status");
    assert_eq!(calls[0].parent_span_id, Some(rounds[0].span_id));
    assert_eq!(calls[0].status, kura_telemetry::trace::SpanStatus::Ok);
    assert_eq!(trace.spans[0].attributes["toolCalls"], 1);
    assert!(
        trace
            .spans
            .iter()
            .all(|span| span.status != kura_telemetry::trace::SpanStatus::Running),
        "a finished turn leaves nothing running"
    );

    // What ran, never what was said.
    let json = serde_json::to_string(&trace).expect("serialize");
    for secret in ["SECRET-QUESTION", "SECRET-ARG", "SECRET-TOOL-OUTPUT"] {
        assert!(!json.contains(secret), "{secret} leaked into the trace");
    }
}

/// A read-only tool that takes `arguments` milliseconds.
struct SlowReadTool;

impl kura_core::Tool for SlowReadTool {
    fn spec(&self) -> kura_llm::ToolSpec {
        kura_llm::ToolSpec {
            name: "slow_read".to_string(),
            description: "a read-only test tool".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        }
    }

    fn parallel_safe(&self, _invocation: &kura_core::ToolInvocation) -> bool {
        true
    }

    fn call<'a>(
        &'a self,
        invocation: &'a kura_core::ToolInvocation,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<kura_core::ToolOutput, kura_core::ToolError>>
                + Send
                + 'a,
        >,
    > {
        let ms: u64 = invocation.arguments.parse().unwrap_or(0);
        let call_id = invocation.call_id.clone();
        Box::pin(async move {
            tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
            Ok(kura_core::ToolOutput::ok(format!("read {call_id}")))
        })
    }
}

/// 2.6 end to end: read-only calls of one round overlap inside a real turn,
/// each keeps its own span (closed when *it* ends, under the round that
/// asked), and the next round sees the results in the order it asked.
#[test]
fn read_only_calls_of_one_round_overlap_with_a_span_each() {
    let ask = |call_id: &str, ms: u64| kura_llm::ToolCall {
        call_id: call_id.to_string(),
        name: "slow_read".to_string(),
        arguments: ms.to_string(),
    };
    let (provider, seen) = ScriptedProvider::new(vec![
        ProviderResponse {
            tool_calls: vec![ask("a", 400), ask("b", 10), ask("c", 300)],
            finish_reason: "tool_calls".to_string(),
            ..ProviderResponse::default()
        },
        answered("done"),
    ]);
    let mut registry = kura_core::ToolRegistry::new();
    registry.register(Arc::new(SlowReadTool));
    let mut svc = service(new_dispatcher(provider), None, None);
    svc.set_tools(Arc::new(FixedTools(Arc::new(registry))));

    let started = std::time::Instant::now();
    let execution = svc
        .query(
            base_query(OPENAI_COMPATIBLE_PROVIDER_NAME, "m", "read three things"),
            &CancellationToken::new(),
        )
        .expect("query");
    let elapsed = started.elapsed();
    assert!(execution.exec_error.is_none());
    // Serial would be 710ms of tool time alone.
    assert!(
        elapsed < std::time::Duration::from_millis(650),
        "calls did not overlap: {elapsed:?}"
    );

    let trace = svc.traces().get(&execution.result.turn_id).expect("trace");
    let calls = spans_of(&trace, "tool_call");
    assert_eq!(calls.len(), 3);
    let round = spans_of(&trace, "round")[0].span_id;
    let duration = |name: &str| {
        let span = calls
            .iter()
            .find(|s| s.attributes.get("callId").and_then(|v| v.as_str()) == Some(name))
            .expect("span per call");
        assert_eq!(span.status, kura_telemetry::trace::SpanStatus::Ok);
        assert_eq!(span.parent_span_id, Some(round));
        span.duration_ms.expect("closed")
    };
    assert!(
        duration("b") < 200 && duration("a") >= 400,
        "each span measures its own call"
    );

    let requests = seen.read();
    let replayed: Vec<&str> = requests[1]
        .messages
        .iter()
        .filter(|m| m.role == MessageRole::Tool)
        .map(|m| m.tool_call_id.as_str())
        .collect();
    assert_eq!(replayed, ["a", "b", "c"]);
}

#[test]
fn a_failed_tool_call_is_an_error_span_in_a_turn_that_still_answers() {
    let tool = Arc::new(RecordingTool {
        name: "loopforge_status".to_string(),
        answer: "ok".to_string(),
        calls: Arc::new(parking_lot::RwLock::new(Vec::new())),
    });
    let (svc, _) = service_with_tool(
        vec![asked_for("call_1", "no_such_tool", "{}"), answered("sorry")],
        tool,
    );

    let execution = svc
        .query(
            base_query(OPENAI_COMPATIBLE_PROVIDER_NAME, "m", "do it"),
            &CancellationToken::new(),
        )
        .expect("query");

    let trace = svc.traces().get(&execution.result.turn_id).expect("trace");
    assert_eq!(trace.status, kura_telemetry::trace::SpanStatus::Ok);
    let calls = spans_of(&trace, "tool_call");
    assert_eq!(calls[0].status, kura_telemetry::trace::SpanStatus::Error);
    assert_eq!(calls[0].error_code, "tool_failed");
}

#[test]
fn a_turn_that_hits_the_round_cap_is_traced_as_that() {
    let tool = Arc::new(RecordingTool {
        name: "loopforge_status".to_string(),
        answer: "ok".to_string(),
        calls: Arc::new(parking_lot::RwLock::new(Vec::new())),
    });
    let rounds: Vec<ProviderResponse> = (0..10)
        .map(|_| asked_for("call_1", "loopforge_status", "{}"))
        .collect();
    let (mut svc, _) = service_with_tool(rounds, tool);
    svc.set_max_tool_rounds(2);
    let mut input = base_query(OPENAI_COMPATIBLE_PROVIDER_NAME, "m", "loop");
    input.request_id = "req_cap".to_string();

    svc.query(input, &CancellationToken::new())
        .expect_err("the cap fails the turn");

    // The failed turn returned no id; it is found by its request id.
    let traces = svc.traces().list(10, |trace| trace.request_id == "req_cap");
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0].status, kura_telemetry::trace::SpanStatus::Error);
    assert_eq!(traces[0].error_code, "max_tool_rounds");
    assert_eq!(spans_of(&traces[0], "round").len(), 2);
}

#[test]
fn a_turn_refused_before_any_dispatch_is_still_traced() {
    let (provider, seen) = ScriptedProvider::new(vec![answered("never")]);
    let svc = service(new_dispatcher(provider), None, None);
    let mut input = base_query(OPENAI_COMPATIBLE_PROVIDER_NAME, "m", "  ");
    input.request_id = "req_empty".to_string();

    let error = svc
        .query(input, &CancellationToken::new())
        .expect_err("an empty query is refused");

    assert!(seen.read().is_empty());
    let traces = svc
        .traces()
        .list(10, |trace| trace.request_id == "req_empty");
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0].status, kura_telemetry::trace::SpanStatus::Error);
    assert_eq!(traces[0].error_code, error.code());
    assert!(spans_of(&traces[0], "dispatch").is_empty());
}

#[test]
fn a_stream_is_traced_as_one_round_of_one_dispatch() {
    let (provider, _) = ScriptedProvider::new(vec![with_usage(answered("hi"), 4, 1)]);
    let svc = service(new_dispatcher(provider), None, None);

    let execution = svc
        .stream(
            base_query(OPENAI_COMPATIBLE_PROVIDER_NAME, "m", "hello"),
            &CancellationToken::new(),
            Some(|_chunk: StreamChunk| Ok(())),
        )
        .expect("stream");

    let trace = svc.traces().get(&execution.result.turn_id).expect("trace");
    assert_eq!(trace.status, kura_telemetry::trace::SpanStatus::Ok);
    assert_eq!(trace.spans[0].name, "chat.stream");
    assert_eq!(spans_of(&trace, "round").len(), 1);
    let dispatches = spans_of(&trace, "dispatch");
    assert_eq!(dispatches.len(), 1);
    assert_eq!(dispatches[0].status, kura_telemetry::trace::SpanStatus::Ok);
    assert_eq!(trace.usage.total_tokens, 5);
}

// ------------------------------------------------------------------------
// Turn resume (agent-runtime-program.md 2.7): a retried request picks up
// from its last settled call.
// ------------------------------------------------------------------------

/// Replays one scripted result per round, failures included.
struct FallibleProvider {
    rounds: parking_lot::Mutex<std::collections::VecDeque<Result<ProviderResponse, ProviderError>>>,
    seen: Arc<parking_lot::RwLock<Vec<ProviderRequest>>>,
}

impl Provider for FallibleProvider {
    fn name(&self) -> &str {
        OPENAI_COMPATIBLE_PROVIDER_NAME
    }

    fn complete<'a>(
        &'a self,
        request: ProviderRequest,
    ) -> BoxFuture<'a, Result<ProviderResponse, ProviderError>> {
        self.seen.write().push(request);
        let next = self
            .rounds
            .lock()
            .pop_front()
            .unwrap_or_else(|| Ok(answered("unscripted")));
        Box::pin(async move { next })
    }

    fn stream<'a>(
        &'a self,
        request: ProviderRequest,
        _emit: kura_llm::StreamEmitter<'a>,
    ) -> BoxFuture<'a, Result<ProviderResponse, ProviderError>> {
        self.complete(request)
    }
}

fn provider_down() -> ProviderError {
    ProviderError::Provider {
        code: "unavailable".to_string(),
        message: "down".to_string(),
        retryable: false,
    }
}

struct ResumeFixture {
    svc: Service,
    store: Arc<FakeStore>,
    calls: Arc<parking_lot::RwLock<Vec<String>>>,
    seen: Arc<parking_lot::RwLock<Vec<ProviderRequest>>>,
}

fn resume_fixture(rounds: Vec<Result<ProviderResponse, ProviderError>>) -> ResumeFixture {
    let seen = Arc::new(parking_lot::RwLock::new(Vec::new()));
    let provider = Arc::new(FallibleProvider {
        rounds: parking_lot::Mutex::new(rounds.into()),
        seen: Arc::clone(&seen),
    });
    let calls = Arc::new(parking_lot::RwLock::new(Vec::new()));
    let mut registry = kura_core::ToolRegistry::new();
    registry.register(Arc::new(RecordingTool {
        name: "send_mail".to_string(),
        answer: "sent".to_string(),
        calls: Arc::clone(&calls),
    }));
    let store = FakeStore::new();
    let mut svc = service(
        new_dispatcher(provider),
        None,
        Some(store.clone() as Arc<dyn ChatStore>),
    );
    svc.set_tools(Arc::new(FixedTools(Arc::new(registry))));
    ResumeFixture {
        svc,
        store,
        calls,
        seen,
    }
}

fn resumable_query(request_id: &str) -> QueryInput {
    let mut input = base_query(OPENAI_COMPATIBLE_PROVIDER_NAME, "m", "mail the report");
    input.tenant_id = "tenant_a".to_string();
    input.request_id = request_id.to_string();
    input
}

fn shows_call_output(request: &ProviderRequest, call_id: &str) -> bool {
    request
        .messages
        .iter()
        .any(|message| message.tool_call_id == call_id)
}

#[test]
fn a_retry_after_a_failed_round_resumes_and_does_not_send_twice() {
    let fixture = resume_fixture(vec![
        Ok(asked_for("call_1", "send_mail", "{\"to\":\"a@b\"}")),
        Err(provider_down()),
        Ok(answered("Sent it.")),
    ]);

    let first = fixture
        .svc
        .query(resumable_query("req_retry"), &CancellationToken::new())
        .expect("first attempt");
    assert!(first.exec_error.is_some(), "the second round failed");
    assert_eq!(fixture.calls.read().len(), 1);
    assert_eq!(fixture.store.inner.read().turn_checkpoints.len(), 1);

    let second = fixture
        .svc
        .query(resumable_query("req_retry"), &CancellationToken::new())
        .expect("retry");
    assert!(second.exec_error.is_none());
    assert_eq!(second.result.dispatch.output, "Sent it.");
    // The mail went once: the retry read its result back from the checkpoint.
    assert_eq!(fixture.calls.read().len(), 1);
    let seen = fixture.seen.read();
    assert_eq!(seen.len(), 3);
    assert!(shows_call_output(&seen[2], "call_1"));
    // Answered: nothing left to resume.
    assert!(fixture.store.inner.read().turn_checkpoints.is_empty());

    let trace = fixture
        .svc
        .traces()
        .get(&second.result.turn_id)
        .expect("trace");
    let attributes = &trace.spans[0].attributes;
    assert_eq!(attributes["resumedRounds"], serde_json::json!(1));
    assert_eq!(attributes["replayedCalls"], serde_json::json!(1));
    assert_eq!(attributes["interruptedCalls"], serde_json::json!(0));
}

#[test]
fn another_request_id_is_another_turn() {
    let fixture = resume_fixture(vec![
        Ok(asked_for("call_1", "send_mail", "{}")),
        Err(provider_down()),
        Ok(asked_for("call_2", "send_mail", "{}")),
        Ok(answered("Sent it.")),
    ]);
    fixture
        .svc
        .query(resumable_query("req_a"), &CancellationToken::new())
        .expect("first attempt");
    let other = fixture
        .svc
        .query(resumable_query("req_b"), &CancellationToken::new())
        .expect("a different request");
    assert!(other.exec_error.is_none());
    // Nothing carried over: the new request ran its own call.
    assert_eq!(fixture.calls.read().len(), 2);
    // The first request's checkpoint is still there for its own retry.
    assert_eq!(fixture.store.inner.read().turn_checkpoints.len(), 1);
}

#[test]
fn a_turn_without_a_request_id_keeps_no_checkpoint() {
    let fixture = resume_fixture(vec![
        Ok(asked_for("call_1", "send_mail", "{}")),
        Err(provider_down()),
    ]);
    fixture
        .svc
        .query(resumable_query(""), &CancellationToken::new())
        .expect("query");
    assert_eq!(fixture.calls.read().len(), 1);
    assert_eq!(fixture.store.inner.read().turn_checkpoint_writes, 0);
}

#[test]
fn with_resume_off_a_retry_runs_from_the_start() {
    let mut fixture = resume_fixture(vec![
        Ok(asked_for("call_1", "send_mail", "{}")),
        Err(provider_down()),
        Ok(asked_for("call_2", "send_mail", "{}")),
        Ok(answered("Sent it.")),
    ]);
    fixture.svc.set_turn_resume(false);
    for _ in 0..2 {
        fixture
            .svc
            .query(resumable_query("req_retry"), &CancellationToken::new())
            .expect("query");
    }
    assert_eq!(fixture.calls.read().len(), 2);
    assert_eq!(fixture.store.inner.read().turn_checkpoint_writes, 0);
}

// ------------------------------------------------------------------------
// Structured output (agent-runtime-program.md 2.8, G7).
// ------------------------------------------------------------------------

fn person_format() -> kura_llm::ResponseFormat {
    kura_llm::ResponseFormat {
        name: "person".to_string(),
        schema: serde_json::json!({
            "type": "object",
            "properties": {"name": {"type": "string"}, "age": {"type": "integer"}},
            "required": ["name", "age"],
            "additionalProperties": false
        }),
    }
}

fn structured_query(format: kura_llm::ResponseFormat) -> QueryInput {
    let mut input = base_query(OPENAI_COMPATIBLE_PROVIDER_NAME, "m", "who wrote it?");
    input.response_format = Some(format);
    input
}

#[test]
fn a_conforming_reply_comes_back_parsed() {
    let (provider, seen) = ScriptedProvider::new(vec![answered(
        "```json\n{\"name\":\"Ada\",\"age\":36}\n```",
    )]);
    let svc = service(new_dispatcher(provider), None, None);

    let execution = svc
        .query(structured_query(person_format()), &CancellationToken::new())
        .expect("query");

    assert!(execution.exec_error.is_none());
    assert_eq!(
        execution.result.structured_output,
        Some(serde_json::json!({"name": "Ada", "age": 36}))
    );
    // The model's own text is what the dispatch records.
    assert!(execution.result.dispatch.output.starts_with("```json"));
    let seen = seen.read();
    assert_eq!(seen.len(), 1);
    // Handed to the provider too, for a wire that can take it natively.
    assert_eq!(seen[0].response_format, Some(person_format()));
    let system = &seen[0].messages[0];
    assert_eq!(system.role, MessageRole::System);
    assert!(
        system.content.contains("named `person`"),
        "{}",
        system.content
    );
}

/// A caller's instructions (a work item's mandate) open the system frame
/// and never reach the question.
#[test]
fn caller_instructions_lead_the_system_frame() {
    let (provider, seen) = ScriptedProvider::new(vec![answered("{\"name\":\"Ada\",\"age\":36}")]);
    let svc = service(new_dispatcher(provider), None, None);
    let mut input = structured_query(person_format());
    input.instructions = "Only work on the CI fix.".to_string();
    svc.query(input, &CancellationToken::new()).expect("query");

    let seen = seen.read();
    let system = &seen[0].messages[0];
    assert_eq!(system.role, MessageRole::System);
    assert!(
        system.content.starts_with("Only work on the CI fix."),
        "{}",
        system.content
    );
    assert!(system.content.contains("named `person`"));
    let question = seen[0].messages.last().expect("question");
    assert_eq!(question.role, MessageRole::User);
    assert!(!question.content.contains("CI fix"));
}

#[test]
fn a_reply_that_does_not_conform_is_corrected_once() {
    let (provider, seen) = ScriptedProvider::new(vec![
        answered("{\"name\":\"Ada\"}"),
        answered("{\"name\":\"Ada\",\"age\":36}"),
    ]);
    let svc = service(new_dispatcher(provider), None, None);

    let execution = svc
        .query(structured_query(person_format()), &CancellationToken::new())
        .expect("query");

    assert_eq!(
        execution.result.structured_output,
        Some(serde_json::json!({"name": "Ada", "age": 36}))
    );
    let seen = seen.read();
    assert_eq!(seen.len(), 2);
    // The correction round asks for the same shape.
    assert_eq!(seen[1].response_format, Some(person_format()));
    let feedback = seen[1].messages.last().expect("feedback");
    assert_eq!(feedback.role, MessageRole::User);
    assert!(
        feedback.content.contains("missing required property `age`"),
        "{}",
        feedback.content
    );
}

#[test]
fn a_reply_that_still_does_not_conform_fails_the_query() {
    let (provider, seen) = ScriptedProvider::new(vec![
        answered("Ada, 36"),
        answered("{\"name\":\"Ada\",\"age\":\"36\"}"),
        answered("{\"name\":\"Ada\",\"age\":36}"),
    ]);
    let svc = service(new_dispatcher(provider), None, None);

    let error = svc
        .query(structured_query(person_format()), &CancellationToken::new())
        .expect_err("rejected twice");

    assert_eq!(error.code(), "structured_output_invalid");
    assert!(
        error.to_string().contains("$.age: expected integer"),
        "{error}"
    );
    assert_eq!(seen.read().len(), 2, "corrected once, not again");
}

#[test]
fn a_format_that_cannot_be_enforced_is_refused_before_anything_runs() {
    let (provider, seen) = ScriptedProvider::new(vec![answered("{}")]);
    let svc = service(new_dispatcher(provider), None, None);
    let mut format = person_format();
    format.schema = serde_json::json!({"type": "object", "if": {"required": ["a"]}});

    let error = svc
        .query(structured_query(format), &CancellationToken::new())
        .expect_err("unsupported keyword");

    assert_eq!(error.code(), "invalid_response_format");
    assert!(seen.read().is_empty());
}

#[test]
fn a_stream_refuses_a_response_format() {
    let (provider, seen) = ScriptedProvider::new(vec![answered("{}")]);
    let svc = service(new_dispatcher(provider), None, None);

    let error = svc
        .stream(
            structured_query(person_format()),
            &CancellationToken::new(),
            Some(|_chunk: StreamChunk| Ok(())),
        )
        .expect_err("not on a stream");

    assert_eq!(error.code(), "invalid_response_format");
    assert!(seen.read().is_empty());
}

// ------------------------------------------------------------------------
// Steering a running turn (agent-runtime-program.md 2.8, G8).
// ------------------------------------------------------------------------

/// What a person does while a turn's tool runs, done from inside the tool:
/// the turn is running then, and the call is where the next round waits.
enum Meanwhile {
    Steer(&'static str),
    Cancel,
}

struct InterruptingTool {
    service: Arc<std::sync::OnceLock<Service>>,
    meanwhile: Meanwhile,
    outcome: Arc<parking_lot::RwLock<Vec<String>>>,
}

impl kura_core::Tool for InterruptingTool {
    fn spec(&self) -> kura_llm::ToolSpec {
        kura_llm::ToolSpec {
            name: "list_files".to_string(),
            description: "a test tool".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        }
    }

    fn call<'a>(
        &'a self,
        _invocation: &'a kura_core::ToolInvocation,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<kura_core::ToolOutput, kura_core::ToolError>>
                + Send
                + 'a,
        >,
    > {
        let service = self.service.get().expect("service");
        let outcome = match self.meanwhile {
            Meanwhile::Steer(message) => service
                .steer_turn("tenant_a", "req_steer", message)
                .map(|pending| format!("pending {pending}")),
            Meanwhile::Cancel => service
                .cancel_turn("tenant_a", "req_steer")
                .map(|()| "cancelled".to_string()),
        };
        self.outcome
            .write()
            .push(outcome.unwrap_or_else(|error| error.code().to_string()));
        Box::pin(async move { Ok(kura_core::ToolOutput::ok("a.pdf b.txt")) })
    }
}

/// The service, what the provider was asked, and what the tool's
/// interruption came back with.
type SteeringFixture = (
    Service,
    Arc<parking_lot::RwLock<Vec<ProviderRequest>>>,
    Arc<parking_lot::RwLock<Vec<String>>>,
);

fn steering_service(rounds: Vec<ProviderResponse>, meanwhile: Meanwhile) -> SteeringFixture {
    let (provider, seen) = ScriptedProvider::new(rounds);
    let slot = Arc::new(std::sync::OnceLock::new());
    let outcome = Arc::new(parking_lot::RwLock::new(Vec::new()));
    let mut registry = kura_core::ToolRegistry::new();
    registry.register(Arc::new(InterruptingTool {
        service: Arc::clone(&slot),
        meanwhile,
        outcome: Arc::clone(&outcome),
    }));
    let mut svc = service(new_dispatcher(provider), None, None);
    svc.set_tools(Arc::new(FixedTools(Arc::new(registry))));
    let _ = slot.set(svc.clone());
    (svc, seen, outcome)
}

fn steerable_query() -> QueryInput {
    let mut input = base_query(OPENAI_COMPATIBLE_PROVIDER_NAME, "m", "list my files");
    input.tenant_id = "tenant_a".to_string();
    input.request_id = "req_steer".to_string();
    input
}

#[test]
fn a_message_sent_to_a_running_turn_is_read_on_its_next_round() {
    let (svc, seen, outcome) = steering_service(
        vec![asked_for("call_1", "list_files", "{}"), answered("a.pdf")],
        Meanwhile::Steer("only the PDFs"),
    );

    let execution = svc
        .query(steerable_query(), &CancellationToken::new())
        .expect("query");

    assert!(execution.exec_error.is_none());
    assert_eq!(execution.result.dispatch.output, "a.pdf");
    assert_eq!(*outcome.read(), vec!["pending 1"]);
    let seen = seen.read();
    assert_eq!(seen.len(), 2);
    let last = seen[1].messages.last().expect("message");
    assert_eq!(last.role, MessageRole::User);
    assert_eq!(last.content, "only the PDFs");
    // The call's result still comes before it.
    assert!(shows_call_output(&seen[1], "call_1"));
    // Finished: nothing can reach it now.
    assert_eq!(
        svc.steer_turn("tenant_a", "req_steer", "late")
            .map_err(|e| e.code()),
        Err("turn_not_found")
    );
}

#[test]
fn another_tenant_cannot_reach_the_turn() {
    let (svc, _, outcome) = steering_service(
        vec![asked_for("call_1", "list_files", "{}"), answered("done")],
        Meanwhile::Steer("hijack"),
    );
    let mut input = steerable_query();
    input.tenant_id = "tenant_b".to_string();

    svc.query(input, &CancellationToken::new()).expect("query");

    assert_eq!(*outcome.read(), vec!["turn_not_found"]);
}

#[test]
fn a_cancelled_turn_stops_before_its_next_round() {
    let (svc, seen, outcome) = steering_service(
        vec![
            asked_for("call_1", "list_files", "{}"),
            answered("never sent"),
        ],
        Meanwhile::Cancel,
    );

    let execution = svc
        .query(steerable_query(), &CancellationToken::new())
        .expect("query");

    assert_eq!(*outcome.read(), vec!["cancelled"]);
    assert!(execution.exec_error.is_some());
    assert_eq!(
        execution.result.dispatch.status,
        kura_llm::DispatchStatus::Cancelled
    );
    assert_eq!(seen.read().len(), 1);
}

/// Streams each scripted round word by word, then answers with it, tool
/// calls included: what a streaming provider does.
struct StreamingScripted {
    rounds: parking_lot::Mutex<std::collections::VecDeque<ProviderResponse>>,
    seen: Arc<parking_lot::RwLock<Vec<ProviderRequest>>>,
}

impl Provider for StreamingScripted {
    fn name(&self) -> &str {
        OPENAI_COMPATIBLE_PROVIDER_NAME
    }

    fn complete<'a>(
        &'a self,
        _request: ProviderRequest,
    ) -> BoxFuture<'a, Result<ProviderResponse, ProviderError>> {
        panic!("a streamed turn must stream every round");
    }

    fn stream<'a>(
        &'a self,
        request: ProviderRequest,
        emit: kura_llm::StreamEmitter<'a>,
    ) -> BoxFuture<'a, Result<ProviderResponse, ProviderError>> {
        self.seen.write().push(request);
        let next = self
            .rounds
            .lock()
            .pop_front()
            .unwrap_or_else(|| answered("unscripted"));
        Box::pin(async move {
            for word in next.output.split_inclusive(' ') {
                emit(kura_llm::StreamChunk {
                    delta: word.to_string(),
                    ..kura_llm::StreamChunk::default()
                })?;
            }
            Ok(next)
        })
    }
}

fn streaming_service_with_tool(
    rounds: Vec<ProviderResponse>,
    tool: Arc<RecordingTool>,
) -> (Service, Arc<parking_lot::RwLock<Vec<ProviderRequest>>>) {
    let seen = Arc::new(parking_lot::RwLock::new(Vec::new()));
    let provider = Arc::new(StreamingScripted {
        rounds: parking_lot::Mutex::new(rounds.into()),
        seen: Arc::clone(&seen),
    });
    let mut registry = kura_core::ToolRegistry::new();
    registry.register(tool);
    let mut svc = service(new_dispatcher(provider), None, None);
    svc.set_tools(Arc::new(FixedTools(Arc::new(registry))));
    (svc, seen)
}

/// A streamed turn is the same agent turn as a query: the model asks for a
/// tool, the tool runs, the model answers, and every round's text reaches
/// the caller as it is written. A new round is a new dispatch id, and its
/// `reply` starts over, so text before a tool call is interim, as in a query.
#[test]
fn a_streamed_turn_runs_tools_and_streams_every_round() {
    let tool = Arc::new(RecordingTool {
        name: "lookup".to_string(),
        answer: "42".to_string(),
        calls: Arc::new(parking_lot::RwLock::new(Vec::new())),
    });
    let mut asked = asked_for("call_1", "lookup", "{\"q\":\"answer\"}");
    asked.output = "Let me check. ".to_string();
    let (svc, seen) = streaming_service_with_tool(
        vec![asked, answered("The answer is 42.")],
        Arc::clone(&tool),
    );

    let mut chunks: Vec<StreamChunk> = Vec::new();
    let execution = svc
        .stream(
            base_query(OPENAI_COMPATIBLE_PROVIDER_NAME, "m", "what is the answer?"),
            &CancellationToken::new(),
            Some(|chunk: StreamChunk| {
                chunks.push(chunk);
                Ok(())
            }),
        )
        .expect("stream");
    assert!(execution.exec_error.is_none(), "{:?}", execution.exec_error);
    assert_eq!(tool.calls.read().len(), 1, "the tool ran once");
    assert_eq!(seen.read().len(), 2, "two rounds");
    assert!(
        seen.read()[1]
            .messages
            .iter()
            .any(|m| m.tool_call_id == "call_1"),
        "the second round saw the tool's output"
    );

    // Both rounds streamed, in order, each under its own dispatch.
    let rounds: Vec<&str> =
        chunks
            .iter()
            .map(|c| c.dispatch_id.as_str())
            .fold(Vec::new(), |mut ids, id| {
                if ids.last() != Some(&id) {
                    ids.push(id);
                }
                ids
            });
    assert_eq!(rounds.len(), 2, "{rounds:?}");
    let text: String = chunks.iter().map(|c| c.delta.as_str()).collect();
    assert_eq!(text, "Let me check. The answer is 42.");
    let last = chunks.last().expect("chunks");
    assert_eq!(
        last.reply, "The answer is 42.",
        "reply is the current round's text"
    );
    assert_eq!(last.dispatch_id, execution.result.dispatch.dispatch_id);
    // The answer is the last round's, as for a query.
    assert_eq!(execution.result.dispatch.output, "The answer is 42.");
}

/// A caller that stops listening cancels the turn, and the turn says why.
#[test]
fn a_stream_whose_caller_stops_listening_ends_the_turn() {
    let tool = Arc::new(RecordingTool {
        name: "lookup".to_string(),
        answer: "42".to_string(),
        calls: Arc::new(parking_lot::RwLock::new(Vec::new())),
    });
    let mut asked = asked_for("call_1", "lookup", "{}");
    asked.output = "Let me check. ".to_string();
    let (svc, seen) = streaming_service_with_tool(
        vec![asked, answered("never seen"), answered("never seen")],
        tool,
    );
    let execution = svc
        .stream(
            base_query(OPENAI_COMPATIBLE_PROVIDER_NAME, "m", "what is the answer?"),
            &CancellationToken::new(),
            Some(|_: StreamChunk| Err(ChatError::Emit("receiver closed".to_string()))),
        )
        .expect("stream");
    let error = execution
        .exec_error
        .expect("the turn reports the failure")
        .to_string();
    assert!(error.contains("stream emitter failed"), "{error}");
    assert!(
        seen.read().len() <= 2,
        "the turn stopped instead of going on"
    );
}

// ------------------------------------------------------------------------
// Behavior: the token budget counts every dispatch (4.1)
// ------------------------------------------------------------------------

fn token_limits(tokens_per_minute: u64) -> kura_chat::TurnLimits {
    kura_chat::TurnLimits {
        global_concurrency: 0,
        tenant_concurrency: 0,
        turns_per_minute: 0,
        tokens_per_minute,
        tenant_queue_depth: 0,
        global_queue_depth: 0,
        queue_timeout: Duration::from_secs(5),
        exempt_tenants: Vec::new(),
    }
}

/// The session summarizer and memory consolidator dispatch on the chat's
/// dispatcher outside any turn. Their tokens were spent on the tenant's
/// behalf, so they count against its budget.
#[test]
fn dispatches_outside_a_turn_are_charged_to_the_tenants_token_budget() {
    let dispatcher = new_dispatcher(Arc::new(TestProvider::new("chat-test")));
    let mut svc = service(Arc::clone(&dispatcher), None, None);
    svc.set_turn_limits(token_limits(3));
    let admission = Arc::clone(svc.admission().expect("admission"));

    // Two background calls of 2 tokens each, as the summarizer makes them.
    for _ in 0..2 {
        let mut dispatch = dispatcher
            .prepare(
                kura_llm::CreateDispatchInput {
                    provider: "chat-test".to_string(),
                    model: "m".to_string(),
                    messages: vec![Message {
                        role: MessageRole::User,
                        content: "summarize".to_string(),
                        ..Default::default()
                    }],
                    ..kura_llm::CreateDispatchInput::default()
                },
                false,
            )
            .expect("prepare");
        dispatch.tenant_id = "ten_a".to_string();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(dispatcher.dispatch(dispatch, &CancelToken::new()))
            .expect("dispatch");
    }
    assert!(
        admission.admit("ten_a").is_err(),
        "4 tokens against a 3-token budget: the next turn waits"
    );
    assert!(
        admission.admit("ten_b").is_ok(),
        "another tenant is unaffected"
    );
}

/// A turn's rounds are charged once: by the dispatcher, not again when the
/// chat persists the dispatch.
#[test]
fn a_turns_tokens_are_charged_once() {
    let dispatcher = new_dispatcher(Arc::new(TestProvider::new("chat-test")));
    let mut svc = service(dispatcher, None, None);
    svc.set_turn_limits(token_limits(3));
    let admission = Arc::clone(svc.admission().expect("admission"));
    let execution = svc
        .query(
            QueryInput {
                tenant_id: "ten_a".to_string(),
                ..base_query("chat-test", "m", "hello")
            },
            &CancellationToken::new(),
        )
        .expect("query");
    assert!(execution.exec_error.is_none());
    // 2 of 3 tokens spent; charged twice it would be 4 and refused.
    assert!(admission.admit("ten_a").is_ok());
}
