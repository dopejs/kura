//! Port of `daemon/internal/chat/service.go`: the chat query/stream service.
//!
//! The service compiles prompts (agent overlays + selected skills), resolves
//! active-profile, binding, and continuity context, prepares and executes an
//! LLM dispatch through `kura-llm`, persists the dispatch lifecycle and
//! continuity turns/previews through the store, and publishes `llm`,
//! `thread`, `agent_profile`, and `binding` events on the bus.
//!
//! Go's `context.Context` maps to the synchronous [`CancellationToken`] API:
//! `query` and `stream` block the caller (like Go) and bridge into the async
//! `kura-llm` dispatcher through a per-call current-thread Tokio runtime.
//! Streaming is a callback emitter (`stream`, the faithful Go shape) plus a
//! thread + `std::sync::mpsc` variant (`stream_channel`).

use std::sync::Arc;
use std::sync::mpsc;
use std::thread::JoinHandle;

use chrono::{DateTime, Utc};
use kura_bindings::{
    EffectiveBindingSelection, ResolutionOutcome, RuntimeBindingEvidenceInput,
    build_runtime_binding_evidence, enforce_executable,
};
use kura_llm::{
    CreateDispatchInput, Dispatch, DispatchStatus, Message, MessageRole, ProviderError,
};
use kura_profiles::{
    APPLIED_BINDING_CLASSIFICATION_MARKER, ActiveSelection, AgentProfile,
    DEFERRED_BINDING_CLASSIFICATION_MARKER, RuntimeProjectionInput, RuntimeResourceKind,
    build_runtime_projection, safe_profile_summary,
};
use kura_setupwizard::{SafeUseMode, ServiceDependencies, TARGET_OPENAI_COMPATIBLE, new_service};
use kura_protocol::{ResponseItem, Role};
use kura_skills::{Overlay, Registry, Skill};
use kura_threads::{
    ContinuityDecision, ContinuityItemKind, ContinuityMode, ContinuityPreview,
    ContinuityPreviewItem, ContinuityReason, ContinuityRole, ContinuityStatus, ContinuityTurn,
    HandoffSourceReference, HandoffSourceReferenceDecision, HandoffSourceReferenceEligibility,
    HandoffSourceReferenceStatus, HandoffStatus, LifecycleState, RedactionStatus, SourceKind,
    default_continuity_policy, eligible_continuity_turns, normalize_continuity_mode,
    preview_item_for_turn, preview_items_for_artifact_excerpts, reset_boundary_preview_items,
    safe_continuity_content,
};
use serde_json::{Map, Value};

use crate::cancel::CancellationToken;
use crate::error::ChatError;
use crate::events::{
    agent_profile_runtime_projected_event, binding_runtime_projected_event, dispatch_status_str,
    new_continuity_preview_id, normalize_event, thread_continuity_preview_recorded_event,
    thread_continuity_turn_recorded_event,
};
use crate::store::{BindingResolutionParams, ChatStore, ContinuityLookupQuery};
use crate::types::{
    ContinuityAssembly, QueryExecution, QueryInput, QueryResult, Service, StreamChunk,
};

/// Go `llm.OpenAICompatibleProviderName`.
pub const OPENAI_COMPATIBLE_PROVIDER_NAME: &str = "openai_compatible";

impl Service {
    // ------------------------------------------------------------------
    // Query / stream entry points
    // ------------------------------------------------------------------

    /// Go `Service.Query`: prepares and executes a non-streaming dispatch,
    /// persisting the dispatch lifecycle and continuity evidence. Blocking.
    pub fn query(
        &self,
        input: QueryInput,
        cancel: &CancellationToken,
    ) -> Result<QueryExecution, ChatError> {
        self.ensure_configured()?;
        let mut input = input;
        self.run_turn_start_hooks(&mut input)?;

        let (mut dispatch_input, selected_skills) = self.build_dispatch_input(&input)?;
        let (active_profile, active_selection, has_active_profile) =
            self.resolve_active_profile(&input, &mut dispatch_input)?;
        let (binding_selection, has_binding, binding_res) = self.resolve_binding_for_work(
            &input,
            &active_profile,
            &active_selection,
            has_active_profile,
        );
        let (binding_selection, has_binding) = match binding_res {
            Ok(()) => (binding_selection, has_binding),
            Err(err) => {
                // Go: recordBlockedBindingEvidence(input, bindingSelection,
                // hasBinding, err) with the zero selection returned alongside
                // the error; the helper no-ops unless the cause is
                // ErrBindingRepairRequired with hasBinding=true.
                self.record_blocked_binding_evidence(&input, &binding_selection, has_binding, &err);
                return Err(err);
            }
        };
        if let Err(err) = self.enforce_capability_visibility(
            &input,
            &selected_skills,
            &binding_selection,
            has_binding,
        ) {
            return Err(err);
        }
        if let Some(providers) = &self.providers {
            let (_, effective) = providers
                .resolve_dispatch_input(dispatch_input.clone())
                .map_err(|err| ChatError::Provider(err.to_string()))?;
            dispatch_input = effective;
        }
        self.enforce_provider_setup_gate(&input.tenant_id, &dispatch_input.provider, "chat")?;
        let mut continuity = self.prepare_continuity(&input, &mut dispatch_input)?;
        let agent_profile_id =
            if has_active_profile { active_profile.profile_id.clone() } else { String::new() };
        // The turn runs as an agent loop, and a round of that loop is a
        // dispatch. `kura-core` owns the looping; what this supplies is what a
        // round means here -- hooked, prepared, persisted, evented.
        let (final_dispatch, exec_error_message) = self.run_agent_turn(
            &input,
            &agent_profile_id,
            &selected_skills,
            &dispatch_input,
            cancel,
            Box::new({
                let service = self.clone();
                let input = input.clone();
                let active_profile = active_profile.clone();
                let active_selection = active_selection.clone();
                let binding_selection = binding_selection.clone();
                let continuity_snapshot = continuity.clone();
                move |dispatch: &kura_llm::Dispatch| {
                    // First round only: a profile projection and a binding
                    // record describe the turn, and continuity records the
                    // question, which was asked once however many rounds
                    // answering it takes.
                    if has_active_profile {
                        service.record_active_profile_projection(
                            &input,
                            &active_profile,
                            &active_selection,
                            &continuity_snapshot,
                            &binding_selection,
                            has_binding,
                        )?;
                    }
                    if has_binding {
                        service.record_runtime_binding_evidence(&input, &binding_selection, false)?;
                    }
                    let _ = dispatch;
                    Ok(())
                }
            }),
        )?;
        // Outside the per-round callback: continuity is mutated here and read
        // after, so it cannot be borrowed by a closure the loop owns.
        self.persist_continuity_request(
            &mut continuity,
            &input,
            &final_dispatch.dispatch_id,
            input.query.trim(),
        )?;
        self.persist_continuity_response(&mut continuity, &input, &final_dispatch)?;

        let mut result = QueryResult {
            query: input.query.trim().to_string(),
            skills: selected_skill_ids_from_skills(&selected_skills),
            skill_contracts: selected_skill_contracts(&selected_skills),
            dispatch: final_dispatch,
            ..QueryResult::default()
        };
        apply_continuity_result(&mut result, &continuity);
        self.run_turn_end_hooks(&input, &result);
        let exec_error = exec_error_message.map(ChatError::Dispatch);
        Ok(QueryExecution { result, exec_error })
    }

    /// Go `Service.Stream`: prepares and executes a streaming dispatch,
    /// forwarding each chunk through `emit` (Go's callback emitter). Blocking.
    pub fn stream<E>(
        &self,
        input: QueryInput,
        cancel: &CancellationToken,
        mut emit: Option<E>,
    ) -> Result<QueryExecution, ChatError>
    where
        E: FnMut(StreamChunk) -> Result<(), ChatError> + Send,
    {
        self.ensure_configured()?;
        let mut input = input;
        self.run_turn_start_hooks(&mut input)?;

        let (mut dispatch_input, selected_skills) = self.build_dispatch_input(&input)?;
        let (active_profile, active_selection, has_active_profile) =
            self.resolve_active_profile(&input, &mut dispatch_input)?;
        let (binding_selection, has_binding, binding_res) = self.resolve_binding_for_work(
            &input,
            &active_profile,
            &active_selection,
            has_active_profile,
        );
        let (binding_selection, has_binding) = match binding_res {
            Ok(()) => (binding_selection, has_binding),
            Err(err) => {
                self.record_blocked_binding_evidence(&input, &binding_selection, has_binding, &err);
                return Err(err);
            }
        };
        if let Err(err) = self.enforce_capability_visibility(
            &input,
            &selected_skills,
            &binding_selection,
            has_binding,
        ) {
            return Err(err);
        }
        if let Some(providers) = &self.providers {
            let (_, effective) = providers
                .resolve_dispatch_input(dispatch_input.clone())
                .map_err(|err| ChatError::Provider(err.to_string()))?;
            dispatch_input = effective;
        }
        self.enforce_provider_setup_gate(&input.tenant_id, &dispatch_input.provider, "chat")?;
        let mut continuity = self.prepare_continuity(&input, &mut dispatch_input)?;
        let agent_profile_id =
            if has_active_profile { active_profile.profile_id.clone() } else { String::new() };
        self.run_pre_dispatch_hooks(&input, &agent_profile_id, &mut dispatch_input)?;

        let dispatch = self
            .dispatcher
            .prepare(dispatch_input, true)
            .map_err(ChatError::Prepare)?;
        let dispatch_id = dispatch.dispatch_id.clone();
        let dispatch_provider = dispatch.provider.clone();
        let dispatch_model = dispatch.model.clone();
        persist_dispatch(self.store.as_deref(), &dispatch)?;
        if has_active_profile {
            self.record_active_profile_projection(
                &input,
                &active_profile,
                &active_selection,
                &continuity,
                &binding_selection,
                has_binding,
            )?;
        }
        if has_binding {
            self.record_runtime_binding_evidence(&input, &binding_selection, false)?;
        }
        self.persist_continuity_request(&mut continuity, &input, &dispatch_id, input.query.trim())?;
        publish_dispatch_event(
            self.event_bus.as_ref(),
            self.store.as_deref(),
            &input.scope,
            &dispatch,
            &selected_skills,
            "llm.dispatch.requested",
        )?;

        let runtime = bridge_runtime()?;
        let kura_cancel = kura_llm::CancelToken::new();
        let child = cancel.child();
        let _link = child.link_to(&kura_cancel);
        let mut emit = emit.take();
        let exec_result = runtime.block_on(async {
            let mut adapter = |chunk: kura_llm::StreamChunk| -> Result<(), ProviderError> {
                let Some(emit) = emit.as_mut() else {
                    return Ok(());
                };
                let out = StreamChunk {
                    dispatch_id: dispatch_id.clone(),
                    provider: dispatch_provider.clone(),
                    model: dispatch_model.clone(),
                    skills: selected_skill_ids_from_skills(&selected_skills),
                    skill_contracts: selected_skill_contracts(&selected_skills),
                    delta: chunk.delta,
                    reply: chunk.output,
                    finish_reason: chunk.finish_reason,
                    usage: chunk.usage,
                    thread_id: continuity.thread_id.clone(),
                    session_segment_id: continuity.session_segment_id.clone(),
                    request_turn_id: continuity.request_turn_id.clone(),
                    continuity_preview_id: continuity.preview_id.clone(),
                    continuity_applied: continuity.applied,
                    continuity_status: continuity.status,
                };
                emit(out).map_err(|err| ProviderError::Other(err.to_string()))
            };
            self.dispatcher
                .dispatch_stream(dispatch, &kura_cancel, &mut adapter)
                .await
        });

        let final_dispatch = match &exec_result {
            Ok(d) => d.clone(),
            Err(failed) => failed.dispatch.clone(),
        };
        persist_dispatch(self.store.as_deref(), &final_dispatch)?;
        publish_dispatch_event(
            self.event_bus.as_ref(),
            self.store.as_deref(),
            &input.scope,
            &final_dispatch,
            &selected_skills,
            &terminal_dispatch_event(&final_dispatch),
        )?;
        self.persist_continuity_response(&mut continuity, &input, &final_dispatch)?;

        let mut result = QueryResult {
            query: input.query.trim().to_string(),
            skills: selected_skill_ids_from_skills(&selected_skills),
            skill_contracts: selected_skill_contracts(&selected_skills),
            dispatch: final_dispatch,
            ..QueryResult::default()
        };
        apply_continuity_result(&mut result, &continuity);
        self.run_turn_end_hooks(&input, &result);
        let exec_error = match exec_result {
            Ok(_) => None,
            Err(failed) => Some(ChatError::Dispatch(failed.error.to_string())),
        };
        Ok(QueryExecution { result, exec_error })
    }

    /// Thread + `std::sync::mpsc` streaming variant: runs the same pipeline as
    /// [`Service::stream`] on a dedicated `std::thread`, forwarding chunks over
    /// a bounded sync channel. Dropping the receiver aborts the stream (the
    /// emit error surfaces on the join handle).
    pub fn stream_channel(
        &self,
        input: QueryInput,
        cancel: CancellationToken,
        capacity: usize,
    ) -> Result<
        (
            mpsc::Receiver<StreamChunk>,
            JoinHandle<Result<QueryExecution, ChatError>>,
        ),
        ChatError,
    > {
        let service = self.clone();
        let (tx, rx) = mpsc::sync_channel(capacity.max(1));
        let handle = std::thread::Builder::new()
            .name("kura-chat-stream".to_string())
            .spawn(move || {
                service.stream(
                    input,
                    &cancel,
                    Some(move |chunk| {
                        tx.send(chunk)
                            .map_err(|_| ChatError::Emit("stream receiver closed".to_string()))
                    }),
                )
            })
            .map_err(|err| ChatError::Runtime(format!("spawn chat stream thread: {err}")))?;
        Ok((rx, handle))
    }

    /// Go's `s == nil || s.dispatcher == nil` guard: unreachable in Rust
    /// because `new_service` requires a dispatcher. Kept for surface parity.
    fn ensure_configured(&self) -> Result<(), ChatError> {
        Ok(())
    }

    // ------------------------------------------------------------------
    // Plugin hook points (pluginization phase 2)
    // ------------------------------------------------------------------

    /// `chat/turn-start`: hooks may rewrite the query or veto the turn.
    /// Run one turn as an agent loop, and report the dispatch that answered.
    ///
    /// The loop is `kura-core`'s. It streams a model, runs whatever calls come
    /// back, and goes around until the model answers without asking for more;
    /// what this supplies is a `ModelProvider` for which a round means a
    /// dispatch through this daemon -- hooked, prepared, persisted, evented.
    ///
    /// Chat briefly carried a second loop of its own. There is one.
    pub(crate) fn run_agent_turn(
        &self,
        input: &QueryInput,
        agent_profile_id: &str,
        selected_skills: &[Skill],
        dispatch_input: &CreateDispatchInput,
        cancel: &CancellationToken,
        on_first_round: crate::round::OnFirstRound,
    ) -> Result<(Dispatch, Option<String>), ChatError> {
        // The assembled conversation, split the way the loop reads it: a
        // leading system message is the standing instruction, the last user
        // message is this turn's question, and everything between is what came
        // before it.
        let (instructions, history, question) = split_for_turn(&dispatch_input.messages);

        // One token for the turn, linked to the caller's. The link has to
        // outlive every round, which is why it is held here rather than inside
        // one.
        let kura_cancel = kura_llm::CancelToken::new();
        let child = cancel.child();
        let _link = child.link_to(&kura_cancel);

        let log = Arc::new(parking_lot::Mutex::new(crate::round::RoundLog::default()));
        let context = crate::round::RoundContext {
            cancel: kura_cancel,
            service: self.clone(),
            input: input.clone(),
            agent_profile_id: agent_profile_id.to_string(),
            selected_skills: selected_skills.to_vec(),
            provider: dispatch_input.provider.clone(),
            model: dispatch_input.model.clone(),
            timeout_ms: dispatch_input.timeout_ms,
            max_retries: dispatch_input.max_retries,
        };
        let provider: Arc<dyn kura_model_provider::ModelProvider> = Arc::new(
            crate::round::DispatcherProvider::new(context, Arc::clone(&log), on_first_round),
        );

        // No registry means an empty one: no tools are offered, the model asks
        // for none, and the loop takes exactly one round -- the single
        // dispatch a turn was before any of this.
        let tools = match &self.tools {
            Some(source) => source.registry(),
            None => Arc::new(kura_core::ToolRegistry::new()),
        };
        let mut session = kura_core::Session::new(provider, tools)
            .with_history(history)
            .with_max_tool_rounds(self.max_tool_rounds);
        if let Some(instructions) = instructions {
            session = session.with_instructions(instructions);
        }

        let runtime = bridge_runtime()?;
        let mut emit = |_event: kura_protocol::Event| {};
        let outcome = runtime.block_on(session.run_turn(&question, &mut emit));

        let mut log = log.lock();
        let Some(dispatch) = log.last.take() else {
            // The loop never completed a round: a hook vetoed the turn, or the
            // dispatch was rejected before it was sent. Either way there is no
            // dispatch to report, and the reason travelled in the error.
            return Err(match outcome {
                Err(error) => ChatError::Provider(error.to_string()),
                Ok(_) => ChatError::Provider("the turn produced no dispatch".to_string()),
            });
        };
        // A round cap reached is a turn that never finished. Answering with
        // the last text would present it as one that did.
        if let Err(error) = outcome {
            return Err(ChatError::Provider(error.to_string()));
        }
        Ok((dispatch, log.failure.take()))
    }

    /// Persist and announce a round that is about to be sent.
    pub(crate) fn record_round_requested(
        &self,
        context: &crate::round::RoundContext,
        dispatch: &Dispatch,
    ) -> Result<(), ChatError> {
        persist_dispatch(self.store.as_deref(), dispatch)?;
        publish_dispatch_event(
            self.event_bus.as_ref(),
            self.store.as_deref(),
            &context.input.scope,
            dispatch,
            &context.selected_skills,
            "llm.dispatch.requested",
        )
        .map(|_| ())
    }

    /// Persist and announce a round that has settled, however it settled.
    pub(crate) fn record_round_settled(
        &self,
        context: &crate::round::RoundContext,
        dispatch: &Dispatch,
    ) -> Result<(), ChatError> {
        persist_dispatch(self.store.as_deref(), dispatch)?;
        publish_dispatch_event(
            self.event_bus.as_ref(),
            self.store.as_deref(),
            &context.input.scope,
            dispatch,
            &context.selected_skills,
            &terminal_dispatch_event(dispatch),
        )
        .map(|_| ())
    }

    fn run_turn_start_hooks(&self, input: &mut QueryInput) -> Result<(), ChatError> {
        let Some(hooks) = &self.hooks else {
            return Ok(());
        };
        let mut payload = serde_json::json!({
            "tenantId": input.tenant_id,
            "threadId": input.thread_id,
            "query": input.query,
            "sourceKind": serde_json::to_value(input.source_kind).unwrap_or(Value::Null),
        });
        let outcome = hooks.run(kura_plugin::points::CHAT_TURN_START, &mut payload);
        if let Some((plugin_id, reason)) = outcome.halted {
            self.publish_hook_veto(
                kura_plugin::points::CHAT_TURN_START,
                &plugin_id,
                &reason,
                &input.scope,
            );
            return Err(ChatError::HookVetoed {
                point: kura_plugin::points::CHAT_TURN_START.to_string(),
                plugin_id,
                reason,
            });
        }
        if outcome.ran > 0 {
            if let Some(query) = payload.get("query").and_then(Value::as_str) {
                input.query = query.to_string();
            }
        }
        Ok(())
    }

    /// `chat/pre-dispatch`: runs after full context assembly and before the
    /// dispatch is prepared/persisted, so whatever the hooks leave in
    /// provider/model/messages is exactly what is logged on the dispatch
    /// record and what the model sees ("model-visible = logged").
    pub(crate) fn run_pre_dispatch_hooks(
        &self,
        input: &QueryInput,
        agent_profile_id: &str,
        dispatch_input: &mut CreateDispatchInput,
    ) -> Result<(), ChatError> {
        let Some(hooks) = &self.hooks else {
            return Ok(());
        };
        let mut payload = serde_json::json!({
            "tenantId": input.tenant_id,
            "threadId": input.thread_id,
            "agentProfileId": agent_profile_id,
            "sourceKind": serde_json::to_value(input.source_kind).unwrap_or(Value::Null),
            "provider": dispatch_input.provider,
            "model": dispatch_input.model,
            "messages": serde_json::to_value(&dispatch_input.messages)
                .unwrap_or_else(|_| Value::Array(Vec::new())),
        });
        let outcome = hooks.run(kura_plugin::points::CHAT_PRE_DISPATCH, &mut payload);
        if let Some((plugin_id, reason)) = outcome.halted {
            self.publish_hook_veto(
                kura_plugin::points::CHAT_PRE_DISPATCH,
                &plugin_id,
                &reason,
                &input.scope,
            );
            return Err(ChatError::HookVetoed {
                point: kura_plugin::points::CHAT_PRE_DISPATCH.to_string(),
                plugin_id,
                reason,
            });
        }
        if outcome.ran > 0 {
            if let Some(provider) = payload.get("provider").and_then(Value::as_str) {
                dispatch_input.provider = provider.to_string();
            }
            if let Some(model) = payload.get("model").and_then(Value::as_str) {
                dispatch_input.model = model.to_string();
            }
            if let Some(messages) = payload.get("messages") {
                dispatch_input.messages = serde_json::from_value(messages.clone()).map_err(
                    |err| ChatError::HookPayload {
                        point: kura_plugin::points::CHAT_PRE_DISPATCH.to_string(),
                        reason: format!("messages: {err}"),
                    },
                )?;
            }
        }
        Ok(())
    }

    /// `chat/turn-end`: observational; a halt only stops later handlers.
    fn run_turn_end_hooks(&self, input: &QueryInput, result: &QueryResult) {
        let Some(hooks) = &self.hooks else {
            return;
        };
        let mut payload = serde_json::json!({
            "dispatchId": result.dispatch.dispatch_id,
            "tenantId": input.tenant_id,
            "threadId": result.thread_id,
            "query": result.query,
            "output": result.dispatch.output,
            "status": dispatch_status_str(result.dispatch.status),
            "sourceKind": serde_json::to_value(input.source_kind).unwrap_or(Value::Null),
            "sourceMessageId": input.source_message_id,
            "requestTurnId": result.request_turn_id,
            "responseTurnId": result.response_turn_id,
        });
        let _ = hooks.run(kura_plugin::points::CHAT_TURN_END, &mut payload);
    }

    /// Best-effort `chat.hook.vetoed` event so vetoes are auditable.
    fn publish_hook_veto(&self, point: &str, plugin_id: &str, reason: &str, scope: &kura_events::Scope) {
        let mut payload: Map<String, Value> = Map::new();
        payload.insert("point".to_string(), Value::String(point.to_string()));
        payload.insert("pluginId".to_string(), Value::String(plugin_id.to_string()));
        payload.insert("reason".to_string(), Value::String(reason.to_string()));
        let event = normalize_event(kura_events::Event {
            category: "chat".to_string(),
            name: "chat.hook.vetoed".to_string(),
            scope: scope.clone(),
            resource: kura_events::Resource {
                kind: "hook_point".to_string(),
                id: point.to_string(),
            },
            payload,
            ..kura_events::Event::default()
        });
        let event = match self.store.as_deref() {
            Some(store) => store.append_event(&event).unwrap_or(event),
            None => event,
        };
        if let Some(bus) = &self.event_bus {
            bus.publish(event);
        }
    }

    // ------------------------------------------------------------------
    // Provider setup gate (Go enforceProviderSetupGate)
    // ------------------------------------------------------------------

    fn enforce_provider_setup_gate(
        &self,
        tenant_id: &str,
        provider_id: &str,
        capability: &str,
    ) -> Result<(), ChatError> {
        let Some(store) = self.store.as_deref() else {
            return Ok(());
        };
        let tenant_id = tenant_id.trim();
        let provider_id = provider_id.trim();
        if tenant_id.is_empty() || provider_id != OPENAI_COMPATIBLE_PROVIDER_NAME {
            return Ok(());
        }
        let sessions = store
            .list_setup_sessions(tenant_id)
            .map_err(ChatError::Store)?;
        for session in sessions {
            if session.target_id != TARGET_OPENAI_COMPATIBLE {
                continue;
            }
            let service = new_service(ServiceDependencies::default());
            let decision = service.dependent_use_decision(&session, capability);
            if decision.safe_use_mode == SafeUseMode::Blocked {
                let mut reason = decision.reason_code;
                if reason.is_empty() {
                    reason = session.state.as_str().to_string();
                }
                return Err(ChatError::ProviderAuthUnavailable(reason));
            }
            return Ok(());
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Active profile (Go resolveActiveProfile)
    // ------------------------------------------------------------------

    fn resolve_active_profile(
        &self,
        input: &QueryInput,
        dispatch_input: &mut CreateDispatchInput,
    ) -> Result<(AgentProfile, ActiveSelection, bool), ChatError> {
        let Some(store) = self.store.as_deref() else {
            return Ok((AgentProfile::default(), ActiveSelection::default(), false));
        };
        let tenant_id = input.tenant_id.trim();
        if tenant_id.is_empty() {
            return Ok((AgentProfile::default(), ActiveSelection::default(), false));
        }
        let Some((profile, selection)) = store
            .active_agent_profile_selection(tenant_id)
            .map_err(ChatError::Store)?
        else {
            return Ok((AgentProfile::default(), ActiveSelection::default(), false));
        };
        if input.provider.trim().is_empty()
            && !profile
                .default_provider_preference
                .provider_id
                .trim()
                .is_empty()
        {
            dispatch_input.provider = profile
                .default_provider_preference
                .provider_id
                .trim()
                .to_string();
        }
        if input.model.trim().is_empty()
            && !profile.default_provider_preference.model.trim().is_empty()
        {
            dispatch_input.model = profile.default_provider_preference.model.trim().to_string();
        }
        let profile_messages = profile_context_messages(&profile);
        if !profile_messages.is_empty() {
            dispatch_input.messages.extend(profile_messages);
        }
        Ok((profile, selection, true))
    }

    // ------------------------------------------------------------------
    // Binding resolution (Go resolveBindingForWork / resolveBindingSelection)
    // ------------------------------------------------------------------

    fn resolve_binding_for_work(
        &self,
        input: &QueryInput,
        profile: &AgentProfile,
        selection: &ActiveSelection,
        has_active_profile: bool,
    ) -> (EffectiveBindingSelection, bool, Result<(), ChatError>) {
        if !has_active_profile {
            return (EffectiveBindingSelection::default(), false, Ok(()));
        }
        let (resolution, has_binding, err) =
            self.resolve_binding_selection(input, profile, selection, has_active_profile);
        if let Err(err) = err {
            return (EffectiveBindingSelection::default(), false, Err(err));
        }
        if has_binding && resolution.outcome == ResolutionOutcome::REPAIR_REQUIRED {
            return (
                resolution.clone(),
                has_binding,
                Err(ChatError::BindingRepairRequired(
                    resolution.repair_reason.clone(),
                )),
            );
        }
        (resolution, has_binding, Ok(()))
    }

    fn resolve_binding_selection(
        &self,
        input: &QueryInput,
        profile: &AgentProfile,
        selection: &ActiveSelection,
        _has_active_profile: bool,
    ) -> (EffectiveBindingSelection, bool, Result<(), ChatError>) {
        let Some(store) = self.store.as_deref() else {
            return (EffectiveBindingSelection::default(), false, Ok(()));
        };
        let tenant_id = input.tenant_id.trim().to_string();
        if tenant_id.is_empty() {
            return (EffectiveBindingSelection::default(), false, Ok(()));
        }
        // Go: callers that have not adopted ChannelScopeRef may still pass the
        // channel identity via SourceLinkageID.
        let mut channel_ref = input.channel_scope_ref.trim().to_string();
        if channel_ref.is_empty() {
            channel_ref = input.source_linkage_id.trim().to_string();
        }
        match store.resolve_binding_selection(&BindingResolutionParams {
            tenant_id,
            channel_scope_ref: channel_ref,
            account_scope_ref: input.account_scope_ref.trim().to_string(),
            tenant_default_profile_id: profile.profile_id.clone(),
            tenant_default_profile_version_id: selection.profile_version_id.clone(),
        }) {
            Ok(resolution) => (resolution, true, Ok(())),
            Err(err) => (
                EffectiveBindingSelection::default(),
                false,
                Err(ChatError::Store(err)),
            ),
        }
    }

    // ------------------------------------------------------------------
    // Capability visibility (Go enforceCapabilityVisibility)
    // ------------------------------------------------------------------

    fn enforce_capability_visibility(
        &self,
        input: &QueryInput,
        selected_skills: &[Skill],
        binding: &EffectiveBindingSelection,
        has_binding: bool,
    ) -> Result<(), ChatError> {
        let Some(store) = self.store.as_deref() else {
            return Ok(());
        };
        let tenant_id = input.tenant_id.trim();
        if !has_binding || tenant_id.is_empty() {
            return Ok(());
        }
        for skill in selected_skills {
            let decision = store
                .effective_capability_visibility(
                    tenant_id,
                    &binding.selected_profile_id,
                    &binding.selected_workspace_id,
                    &skill.skill_id,
                )
                .map_err(ChatError::Store)?;
            if let Err(err) = enforce_executable(&decision) {
                return Err(ChatError::CapabilityNotExecutable(format!(
                    "{err}: {}",
                    skill.skill_id
                )));
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Evidence recording (Go recordActiveProfileProjection /
    // recordRuntimeBindingEvidence / recordBlockedBindingEvidence)
    // ------------------------------------------------------------------

    fn record_active_profile_projection(
        &self,
        input: &QueryInput,
        profile: &AgentProfile,
        selection: &ActiveSelection,
        continuity: &ContinuityAssembly,
        binding: &EffectiveBindingSelection,
        has_binding: bool,
    ) -> Result<(), ChatError> {
        let Some(store) = self.store.as_deref() else {
            return Ok(());
        };
        let thread_id = input.thread_id.trim();
        if thread_id.is_empty() {
            return Ok(());
        }
        let classification = if has_binding && binding.outcome == ResolutionOutcome::RESOLVED {
            APPLIED_BINDING_CLASSIFICATION_MARKER.to_string()
        } else {
            DEFERRED_BINDING_CLASSIFICATION_MARKER.to_string()
        };
        let projection = build_runtime_projection(
            profile,
            selection,
            RuntimeProjectionInput {
                resource_kind: RuntimeResourceKind::THREAD,
                resource_id: thread_id.to_string(),
                thread_id: thread_id.to_string(),
                session_id: continuity.session_segment_id.clone(),
                occurred_at: Some(Utc::now()),
                binding_classification: classification,
                ..RuntimeProjectionInput::default()
            },
        );
        let recorded = store
            .record_runtime_profile_projection(&projection)
            .map_err(ChatError::Store)?;
        if let Some(bus) = &self.event_bus {
            bus.publish(agent_profile_runtime_projected_event(&recorded));
        }
        Ok(())
    }

    fn record_runtime_binding_evidence(
        &self,
        input: &QueryInput,
        binding: &EffectiveBindingSelection,
        legacy_default: bool,
    ) -> Result<(), ChatError> {
        let Some(store) = self.store.as_deref() else {
            return Ok(());
        };
        let tenant_id = input.tenant_id.trim();
        if tenant_id.is_empty() {
            return Ok(());
        }
        // Go: evidence is recorded per resource the selection influenced:
        // always the thread, and additionally the runtime run when linked.
        let mut targets: Vec<(String, String)> = Vec::with_capacity(2);
        if !input.thread_id.trim().is_empty() {
            targets.push(("thread".to_string(), input.thread_id.trim().to_string()));
        }
        if !input.run_id.trim().is_empty() {
            targets.push(("run".to_string(), input.run_id.trim().to_string()));
        }
        for (kind, id) in targets {
            let evidence = build_runtime_binding_evidence(
                binding,
                &RuntimeBindingEvidenceInput {
                    projection_id: String::new(),
                    tenant_id: tenant_id.to_string(),
                    resource_kind: kind,
                    resource_id: id,
                    occurred_at: Utc::now(),
                    legacy_default,
                },
            );
            let recorded = store
                .record_runtime_binding_evidence(&evidence)
                .map_err(ChatError::Store)?;
            if let Some(bus) = &self.event_bus {
                bus.publish(binding_runtime_projected_event(&recorded));
            }
        }
        Ok(())
    }

    /// Go `recordBlockedBindingEvidence`: best-effort evidence for work
    /// blocked by fail-closed binding resolution (FR-031). Never masks the
    /// originating error.
    fn record_blocked_binding_evidence(
        &self,
        input: &QueryInput,
        binding: &EffectiveBindingSelection,
        has_binding: bool,
        cause: &ChatError,
    ) {
        if !has_binding || !matches!(cause, ChatError::BindingRepairRequired(_)) {
            return;
        }
        let _ = self.record_runtime_binding_evidence(input, binding, false);
    }

    // ------------------------------------------------------------------
    // Dispatch input (Go buildDispatchInput)
    // ------------------------------------------------------------------

    fn build_dispatch_input(
        &self,
        input: &QueryInput,
    ) -> Result<(CreateDispatchInput, Vec<Skill>), ChatError> {
        let query = input.query.trim();
        if query.is_empty() {
            return Err(ChatError::QueryRequired);
        }
        let selected_skills = resolve_selected_skills(self.skills.as_deref(), &input.skills)?;
        let messages = compile_prompt_messages(
            query,
            &selected_skills,
            available_overlays(self.skills.as_deref()),
        );
        Ok((
            CreateDispatchInput {
                // Chat offers none yet; the loop that would is the next slice.
                tools: Vec::new(),
                provider: input.provider.trim().to_string(),
                model: input.model.trim().to_string(),
                messages,
                timeout_ms: input.timeout_ms,
                max_retries: input.max_retries,
            },
            selected_skills,
        ))
    }

    // ------------------------------------------------------------------
    // Continuity (Go prepareContinuity + persistence)
    // ------------------------------------------------------------------

    fn prepare_continuity(
        &self,
        input: &QueryInput,
        dispatch_input: &mut CreateDispatchInput,
    ) -> Result<ContinuityAssembly, ChatError> {
        let thread_id = input.thread_id.trim().to_string();
        let tenant_id = input.tenant_id.trim().to_string();
        let Some(store) = self.store.as_deref() else {
            return Ok(ContinuityAssembly::default());
        };
        if thread_id.is_empty() || tenant_id.is_empty() {
            return Ok(ContinuityAssembly::default());
        }
        let started = Utc::now();
        let mode = normalize_continuity_mode(input.continuity_mode);
        let thread = store
            .get_thread_for_tenant(&tenant_id, &thread_id)
            .map_err(ChatError::Store)?;
        let Some(thread) = thread else {
            return Ok(ContinuityAssembly::default());
        };
        if thread.current_session_segment_id.trim().is_empty() {
            return Ok(ContinuityAssembly::default());
        }
        let mut assembly = ContinuityAssembly {
            enabled: true,
            tenant_id: tenant_id.clone(),
            thread_id: thread.thread_id.clone(),
            session_segment_id: thread.current_session_segment_id.clone(),
            preview_id: new_continuity_preview_id(),
            status: Some(ContinuityStatus::Empty),
            started_at: Some(started),
            ..ContinuityAssembly::default()
        };
        if mode == ContinuityMode::Disabled {
            assembly.status = Some(ContinuityStatus::Disabled);
            assembly.completed_at = Some(Utc::now());
            return Ok(assembly);
        }
        if thread.lifecycle_state == LifecycleState::Archived {
            assembly.status = Some(ContinuityStatus::Blocked);
            assembly.excluded_items.push(ContinuityPreviewItem {
                preview_item_id: String::new(),
                continuity_preview_id: String::new(),
                tenant_id: tenant_id.clone(),
                thread_id: thread_id.clone(),
                item_kind: ContinuityItemKind::Turn,
                continuity_turn_id: String::new(),
                role: None,
                artifact_ref: String::new(),
                artifact_excerpt_id: String::new(),
                handoff_source_reference_id: String::new(),
                decision: ContinuityDecision::Excluded,
                reason_code: ContinuityReason::LifecycleBlocked,
                acceptance_sequence: 0,
                source_timestamp: None,
                safe_summary: "Continuity blocked while thread is archived".to_string(),
                redaction_status: RedactionStatus::Redacted,
                item_order: 0,
            });
            assembly.completed_at = Some(Utc::now());
            return Ok(assembly);
        }
        let turns = store
            .list_continuity_turns(&ContinuityLookupQuery {
                tenant_id: tenant_id.clone(),
                thread_id: thread_id.clone(),
                session_segment_id: thread.current_session_segment_id.clone(),
                now: Some(started),
                ..ContinuityLookupQuery::default()
            })
            .map_err(ChatError::Store)?;
        if thread.lifecycle_state == LifecycleState::Reset {
            let reset_turns = store
                .list_continuity_turns_outside_session_segment(&ContinuityLookupQuery {
                    tenant_id: tenant_id.clone(),
                    thread_id: thread_id.clone(),
                    session_segment_id: thread.current_session_segment_id.clone(),
                    now: Some(started),
                    ..ContinuityLookupQuery::default()
                })
                .map_err(ChatError::Store)?;
            let start_order = assembly.excluded_items.len() as i32;
            assembly
                .excluded_items
                .extend(reset_boundary_preview_items(&reset_turns, start_order));
        }
        let (handoff_refs, handoff_link_ids, handoff_items) =
            self.available_handoff_source_references(&tenant_id, &thread_id)?;
        if !handoff_refs.is_empty() {
            dispatch_input.messages =
                inject_handoff_source_reference_messages(&dispatch_input.messages, &handoff_refs);
            assembly.handoff_link_ids = handoff_link_ids;
        }
        assembly.handoff_items = handoff_items;
        let (included, excluded) =
            eligible_continuity_turns(&turns, &default_continuity_policy(), Some(started));
        assembly.included = included;
        assembly.excluded_items.extend(excluded);
        if !assembly.included.is_empty() {
            dispatch_input.messages =
                inject_continuity_messages(&dispatch_input.messages, &assembly.included);
            assembly.applied = true;
            assembly.status = Some(ContinuityStatus::Applied);
        }
        assembly.completed_at = Some(Utc::now());
        Ok(assembly)
    }

    /// Go `availableHandoffSourceReferences`: collects referenced, eligible,
    /// unexpired handoff source summaries from succeeded links targeting the
    /// destination thread.
    fn available_handoff_source_references(
        &self,
        tenant_id: &str,
        destination_thread_id: &str,
    ) -> Result<
        (
            Vec<HandoffSourceReference>,
            Vec<String>,
            Vec<ContinuityPreviewItem>,
        ),
        ChatError,
    > {
        let Some(store) = self.store.as_deref() else {
            return Ok((Vec::new(), Vec::new(), Vec::new()));
        };
        let links = store
            .list_handoff_links_for_thread(tenant_id, destination_thread_id, 20)
            .map_err(ChatError::Store)?;
        let mut refs: Vec<HandoffSourceReference> = Vec::new();
        let mut link_ids: Vec<String> = Vec::new();
        let mut items: Vec<ContinuityPreviewItem> = Vec::new();
        let now = Utc::now();
        for link in links {
            if link.destination_thread_id != destination_thread_id
                || link.status != HandoffStatus::Succeeded
                || link.source_reference_status != HandoffSourceReferenceStatus::Available
            {
                continue;
            }
            let link_refs = store
                .list_handoff_source_references_for_link(tenant_id, &link.handoff_link_id)
                .map_err(ChatError::Store)?;
            let mut has_referenced = false;
            for reference in link_refs {
                let eligible = reference.decision == HandoffSourceReferenceDecision::Referenced
                    && reference.eligibility_status == HandoffSourceReferenceEligibility::Eligible
                    && !reference.safe_summary.trim().is_empty()
                    && reference
                        .retention_expires_at
                        .map_or(true, |expires| expires > now);
                items.push(preview_item_for_handoff_source_reference(
                    &reference,
                    eligible,
                    items.len() as i32,
                ));
                if eligible {
                    refs.push(reference);
                    has_referenced = true;
                }
            }
            if has_referenced {
                link_ids.push(link.handoff_link_id);
            }
        }
        Ok((refs, link_ids, items))
    }

    /// Go `persistContinuityRequest`: records the user turn and publishes the
    /// turn-recorded event.
    fn persist_continuity_request(
        &self,
        assembly: &mut ContinuityAssembly,
        input: &QueryInput,
        dispatch_id: &str,
        query: &str,
    ) -> Result<(), ChatError> {
        let Some(store) = self.store.as_deref() else {
            return Ok(());
        };
        if !assembly.enabled {
            return Ok(());
        }
        let now = Utc::now();
        let safe = safe_continuity_content(query);
        let turn = ContinuityTurn {
            continuity_turn_id: String::new(),
            tenant_id: assembly.tenant_id.clone(),
            thread_id: assembly.thread_id.clone(),
            session_segment_id: assembly.session_segment_id.clone(),
            acceptance_sequence: 0,
            role: ContinuityRole::User,
            source_kind: continuity_source_kind(input.source_kind),
            source_linkage_id: input.source_linkage_id.trim().to_string(),
            source_message_id: input.source_message_id.trim().to_string(),
            source_timestamp: input.source_timestamp,
            dispatch_id: dispatch_id.to_string(),
            response_to_turn_id: String::new(),
            safe_content: safe.text,
            content_redaction_status: safe.status,
            artifact_excerpt_refs: Vec::new(),
            recorded_at: now,
            retention_expires_at: None,
            source_event_key: input.source_event_key.trim().to_string(),
        };
        let saved = store
            .save_continuity_turn(&turn)
            .map_err(ChatError::Store)?;
        assembly.request_turn_id = saved.continuity_turn_id.clone();
        self.publish_continuity_event(thread_continuity_turn_recorded_event(&saved, "recorded"))
    }

    /// Go `persistContinuityResponse`: records the assistant turn (when the
    /// dispatch produced output), consumes referenced handoff sources, and
    /// saves the continuity preview.
    fn persist_continuity_response(
        &self,
        assembly: &mut ContinuityAssembly,
        input: &QueryInput,
        dispatch: &Dispatch,
    ) -> Result<(), ChatError> {
        let Some(store) = self.store.as_deref() else {
            return Ok(());
        };
        if !assembly.enabled {
            return Ok(());
        }
        let tenant_id = assembly.tenant_id.clone();
        let now = Utc::now();
        if !dispatch.output.is_empty() {
            let safe = safe_continuity_content(&dispatch.output);
            let turn = ContinuityTurn {
                continuity_turn_id: String::new(),
                tenant_id: tenant_id.clone(),
                thread_id: assembly.thread_id.clone(),
                session_segment_id: assembly.session_segment_id.clone(),
                acceptance_sequence: 0,
                role: ContinuityRole::Assistant,
                source_kind: continuity_source_kind(input.source_kind),
                source_linkage_id: input.source_linkage_id.trim().to_string(),
                source_message_id: String::new(),
                source_timestamp: input.source_timestamp,
                dispatch_id: dispatch.dispatch_id.clone(),
                response_to_turn_id: assembly.request_turn_id.clone(),
                safe_content: safe.text,
                content_redaction_status: safe.status,
                artifact_excerpt_refs: Vec::new(),
                recorded_at: now,
                retention_expires_at: None,
                source_event_key: response_continuity_source_event_key(&input.source_event_key),
            };
            let saved = store
                .save_continuity_turn(&turn)
                .map_err(ChatError::Store)?;
            assembly.response_turn_id = saved.continuity_turn_id.clone();
            self.publish_continuity_event(thread_continuity_turn_recorded_event(
                &saved, "recorded",
            ))?;
        }
        for handoff_link_id in &assembly.handoff_link_ids {
            store
                .mark_handoff_source_references_consumed(
                    &tenant_id,
                    handoff_link_id,
                    &assembly.response_turn_id,
                    now,
                )
                .map_err(ChatError::Store)?;
        }
        let mut items: Vec<ContinuityPreviewItem> = Vec::with_capacity(
            assembly.handoff_items.len() + assembly.included.len() + assembly.excluded_items.len(),
        );
        for mut item in assembly.handoff_items.clone() {
            item.item_order = items.len() as i32;
            items.push(item);
        }
        for turn in &assembly.included {
            items.push(preview_item_for_turn(
                turn,
                ContinuityDecision::Included,
                ContinuityReason::IncludedRecent,
                items.len() as i32,
            ));
            items.extend(preview_items_for_artifact_excerpts(
                turn,
                items.len() as i32,
                Some(now),
            ));
        }
        for mut item in assembly.excluded_items.clone() {
            item.item_order = items.len() as i32;
            items.push(item);
        }
        let (included_count, excluded_count) = continuity_preview_item_counts(&items);
        let preview = ContinuityPreview {
            continuity_preview_id: assembly.preview_id.clone(),
            tenant_id: tenant_id.clone(),
            thread_id: assembly.thread_id.clone(),
            session_segment_id: assembly.session_segment_id.clone(),
            dispatch_id: dispatch.dispatch_id.clone(),
            request_turn_id: assembly.request_turn_id.clone(),
            response_turn_id: assembly.response_turn_id.clone(),
            // Store-defaulted fields (Go store fills the policy and retention
            // defaults when empty):
            window_policy_id: String::new(),
            max_prior_turns: 0,
            active_window_days: 0,
            included_count,
            excluded_count,
            continuity_applied: assembly.applied,
            status: assembly.status.unwrap_or(ContinuityStatus::Empty),
            failure_class: String::new(),
            assembly_started_at: assembly.started_at.unwrap_or_else(Utc::now),
            assembly_completed_at: assembly.completed_at.unwrap_or_else(Utc::now),
            assembly_duration_ms: 0,
            retention_expires_at: DateTime::<Utc>::UNIX_EPOCH,
            redaction_status: RedactionStatus::Redacted,
        };
        let saved = store
            .save_continuity_preview(&preview, &items)
            .map_err(ChatError::Store)?;
        assembly.preview_id = saved.continuity_preview_id.clone();
        self.publish_continuity_event(thread_continuity_preview_recorded_event(&saved))
    }

    /// Go `publishContinuityEvent`: normalizes, persists, and publishes one
    /// thread-continuity event.
    fn publish_continuity_event(&self, event: kura_events::Event) -> Result<(), ChatError> {
        let event = normalize_event(event);
        let event = if let Some(store) = self.store.as_deref() {
            store.append_event(&event).map_err(ChatError::Store)?
        } else {
            event
        };
        if let Some(bus) = &self.event_bus {
            bus.publish(event);
        }
        Ok(())
    }
}

// ------------------------------------------------------------------------
// Free helper functions (Go unexported package functions)
// ------------------------------------------------------------------------

/// Go `profileContextMessages`.
#[must_use]
pub fn profile_context_messages(profile: &AgentProfile) -> Vec<Message> {
    let mut messages = Vec::with_capacity(2);
    let summary = safe_profile_summary(profile).trim().to_string();
    if !summary.is_empty() {
        messages.push(Message {
            role: MessageRole::System,
            content: format!("Agent profile persona: {summary}"),
            ..Default::default()
        });
    }
    let safety = profile.safety_defaults.approval_posture.trim().to_string();
    if !safety.is_empty() {
        messages.push(Message {
            role: MessageRole::System,
            content: format!("Agent profile safety posture: {safety}"),
            ..Default::default()
        });
    }
    messages
}

/// Go `resolveSelectedSkills`.
pub fn resolve_selected_skills(
    registry: Option<&Registry>,
    selected: &[String],
) -> Result<Vec<Skill>, ChatError> {
    if selected.is_empty() {
        return Ok(Vec::new());
    }
    let Some(registry) = registry else {
        return Err(ChatError::SkillsRegistryMissing);
    };
    // Go: `ResolveSelected` returns "skill not found: X" / other registry
    // errors verbatim; the display text is preserved in `Skills`.
    registry
        .resolve_selected(selected)
        .map_err(|err| ChatError::Skills(err.to_string()))
}

/// Go `availableOverlays`.
#[must_use]
pub fn available_overlays(registry: Option<&Registry>) -> Vec<Overlay> {
    match registry {
        Some(registry) => registry.overlays(),
        None => Vec::new(),
    }
}

/// Go `compilePromptMessages`: agent overlays, then selected skills, then the
/// trimmed user query.
#[must_use]
pub fn compile_prompt_messages(
    query: &str,
    selected: &[Skill],
    overlays: Vec<Overlay>,
) -> Vec<Message> {
    let mut messages = Vec::with_capacity(overlays.len() + selected.len() + 1);
    for overlay in overlays {
        if overlay.body.trim().is_empty() {
            continue;
        }
        messages.push(Message {
            role: MessageRole::System,
            content: format!(
                "Agent overlay ({}):\n{}",
                overlay.source.as_str(),
                overlay.body
            )
            .trim()
            .to_string(),
            ..Default::default()
        });
    }
    for skill in selected {
        let mut builder = String::new();
        builder.push_str("Skill: ");
        builder.push_str(&skill.name);
        let description = skill.description.trim();
        if !description.is_empty() {
            builder.push_str("\nDescription: ");
            builder.push_str(description);
        }
        let body = skill.body.trim();
        if !body.is_empty() {
            builder.push_str("\nInstructions:\n");
            builder.push_str(body);
        }
        messages.push(Message {
            role: MessageRole::System,
            content: builder,
            ..Default::default()
        });
    }
    messages.push(Message {
        role: MessageRole::User,
        content: query.trim().to_string(),
        ..Default::default()
    });
    messages
}

/// Go `selectedSkillIDsFromSkills`.
#[must_use]
pub fn selected_skill_ids_from_skills(selected: &[Skill]) -> Vec<String> {
    selected
        .iter()
        .map(|skill| skill.skill_id.clone())
        .collect()
}

/// Go `selectedSkillContracts`: deep-cloned sandbox consumer views for the
/// selected skills (Go round-trips through JSON; a plain clone is equivalent).
#[must_use]
pub fn selected_skill_contracts(
    selected: &[Skill],
) -> Vec<serde_json::Map<String, serde_json::Value>> {
    selected
        .iter()
        .filter_map(|skill| skill.sandbox.clone())
        .collect()
}

/// Go `terminalDispatchEvent`.
#[must_use]
/// Split an assembled conversation the way the agent loop reads it.
///
/// Returns the standing instruction, everything said before this turn, and
/// this turn's question. The loop appends the question itself, so it must not
/// also appear in the history -- the model would see it twice and answer the
/// duplicate.
///
/// A leading system message becomes the instruction because that is where the
/// request builders put it; later system messages stay in the conversation,
/// where a mid-turn instruction belongs.
pub(crate) fn split_for_turn(messages: &[Message]) -> (Option<String>, Vec<ResponseItem>, String) {
    let mut instructions = None;
    let mut items = Vec::with_capacity(messages.len());
    for (index, message) in messages.iter().enumerate() {
        if index == 0 && message.role == MessageRole::System {
            instructions = Some(message.content.clone());
            continue;
        }
        items.push(message.clone());
    }
    // The last user message is the question. Anything after it -- a trailing
    // system note, say -- stays in the history where it was.
    let question_at = items.iter().rposition(|message| message.role == MessageRole::User);
    let question = match question_at {
        Some(at) => items.remove(at).content,
        None => String::new(),
    };
    let history = items
        .into_iter()
        .map(|message| ResponseItem::Message {
            role: match message.role {
                MessageRole::System => Role::System,
                MessageRole::User => Role::User,
                MessageRole::Assistant => Role::Assistant,
                MessageRole::Tool => Role::Tool,
            },
            content: message.content,
        })
        .collect();
    (instructions, history, question)
}

pub fn terminal_dispatch_event(dispatch: &Dispatch) -> String {
    match dispatch.status {
        DispatchStatus::PartialFailed => "llm.dispatch.partial_failed".to_string(),
        DispatchStatus::Failed => "llm.dispatch.failed".to_string(),
        DispatchStatus::Cancelled => "llm.dispatch.cancelled".to_string(),
        _ => "llm.dispatch.completed".to_string(),
    }
}

/// Go `continuitySourceKind`: unknown/absent source kinds normalize to chat.
#[must_use]
pub fn continuity_source_kind(kind: Option<SourceKind>) -> SourceKind {
    match kind {
        Some(
            SourceKind::Channel
            | SourceKind::Workflow
            | SourceKind::Schedule
            | SourceKind::Shell
            | SourceKind::Legacy,
        ) => kind.unwrap(),
        _ => SourceKind::Chat,
    }
}

/// Go `responseContinuitySourceEventKey`: `key:assistant` for connector
/// source events.
#[must_use]
pub fn response_continuity_source_event_key(source_event_key: &str) -> String {
    let trimmed = source_event_key.trim();
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}:assistant")
    }
}

/// Go `persistDispatch`.
fn persist_dispatch(store: Option<&dyn ChatStore>, dispatch: &Dispatch) -> Result<(), ChatError> {
    let Some(store) = store else {
        return Ok(());
    };
    store
        .upsert_llm_dispatch(dispatch)
        .map_err(ChatError::Store)
}

/// Go `publishDispatchEvent`: builds the `llm.*` event payload and persists
/// and publishes it. A nil bus short-circuits (Go returns a zero event).
fn publish_dispatch_event(
    event_bus: Option<&kura_events::Bus>,
    store: Option<&dyn ChatStore>,
    scope: &kura_events::Scope,
    dispatch: &Dispatch,
    selected: &[Skill],
    name: &str,
) -> Result<kura_events::Event, ChatError> {
    let Some(bus) = event_bus else {
        return Ok(kura_events::Event::default());
    };

    let mut payload: Map<String, Value> = Map::new();
    payload.insert(
        "provider".to_string(),
        Value::String(dispatch.provider.clone()),
    );
    payload.insert("model".to_string(), Value::String(dispatch.model.clone()));
    match name {
        "llm.dispatch.requested" => {
            payload.insert("stream".to_string(), Value::Bool(dispatch.stream));
            payload.insert(
                "timeoutMs".to_string(),
                Value::Number(dispatch.timeout_ms.into()),
            );
            payload.insert(
                "maxRetries".to_string(),
                Value::Number(dispatch.max_retries.into()),
            );
            payload.insert(
                "status".to_string(),
                Value::String(dispatch_status_str(dispatch.status).to_string()),
            );
        }
        _ => {
            payload.insert(
                "status".to_string(),
                Value::String(dispatch_status_str(dispatch.status).to_string()),
            );
            if name != "llm.dispatch.cancelled" {
                payload.insert("partial".to_string(), Value::Bool(dispatch.partial));
            }
            payload.insert(
                "attemptCount".to_string(),
                Value::Number(dispatch.attempt_count.into()),
            );
            payload.insert(
                "finishReason".to_string(),
                Value::String(dispatch.finish_reason.clone()),
            );
            payload.insert(
                "usage".to_string(),
                serde_json::to_value(&dispatch.usage).unwrap_or(Value::Null),
            );
            payload.insert(
                "errorCode".to_string(),
                Value::String(dispatch.error_code.clone()),
            );
            payload.insert("error".to_string(), Value::String(dispatch.error.clone()));
        }
    }
    let skill_ids = selected_skill_ids_from_skills(selected);
    if !skill_ids.is_empty() {
        payload.insert(
            "skills".to_string(),
            Value::Array(skill_ids.into_iter().map(Value::String).collect()),
        );
    }
    let contracts = selected_skill_contracts(selected);
    if !contracts.is_empty() {
        payload.insert(
            "skillContracts".to_string(),
            Value::Array(contracts.into_iter().map(Value::Object).collect()),
        );
    }

    let mut event = kura_events::Event {
        category: "llm".to_string(),
        name: name.to_string(),
        scope: scope.clone(),
        resource: kura_events::Resource {
            kind: "llm_dispatch".to_string(),
            id: dispatch.dispatch_id.clone(),
        },
        payload,
        ..kura_events::Event::default()
    };
    event = normalize_event(event);
    let event = if let Some(store) = store {
        store.append_event(&event).map_err(ChatError::Store)?
    } else {
        event
    };
    Ok(bus.publish(event))
}

/// Go `injectHandoffSourceReferenceMessages`: inserts prior handoff-source
/// summaries immediately before the first user message.
#[must_use]
pub fn inject_handoff_source_reference_messages(
    messages: &[Message],
    refs: &[HandoffSourceReference],
) -> Vec<Message> {
    if refs.is_empty() {
        return messages.to_vec();
    }
    let mut prior: Vec<Message> = Vec::with_capacity(refs.len());
    for reference in refs {
        let summary = reference.safe_summary.trim();
        if !summary.is_empty() {
            prior.push(Message {
                role: MessageRole::User,
                content: summary.to_string(),
                ..Default::default()
            });
        }
    }
    if prior.is_empty() {
        return messages.to_vec();
    }
    let mut out: Vec<Message> = Vec::with_capacity(messages.len() + prior.len());
    let mut inserted = false;
    for message in messages {
        if !inserted && message.role == MessageRole::User {
            out.extend(prior.iter().cloned());
            inserted = true;
        }
        out.push(message.clone());
    }
    if !inserted {
        out.extend(prior);
    }
    out
}

/// Go `injectContinuityMessages`: inserts redaction-safe prior turns (and
/// redacted artifact excerpt summaries) immediately before the first user
/// message.
#[must_use]
pub fn inject_continuity_messages(messages: &[Message], turns: &[ContinuityTurn]) -> Vec<Message> {
    if turns.is_empty() {
        return messages.to_vec();
    }
    let mut prior: Vec<Message> = Vec::with_capacity(turns.len());
    for turn in turns {
        let role = if turn.role == ContinuityRole::Assistant {
            MessageRole::Assistant
        } else {
            MessageRole::User
        };
        let content = turn.safe_content.trim();
        if !content.is_empty() {
            prior.push(Message {
                role,
                content: content.to_string(),
                ..Default::default()
            });
        }
        for excerpt in &turn.artifact_excerpt_refs {
            if excerpt.redaction_status != RedactionStatus::Redacted {
                continue;
            }
            let summary = safe_continuity_content(&excerpt.excerpt_text);
            if summary.status != RedactionStatus::Redacted || summary.text.trim().is_empty() {
                continue;
            }
            prior.push(Message {
                role,
                content: summary.text,
                ..Default::default()
            });
        }
    }
    if prior.is_empty() {
        return messages.to_vec();
    }
    let mut out: Vec<Message> = Vec::with_capacity(messages.len() + prior.len());
    let mut inserted = false;
    for message in messages {
        if !inserted && message.role == MessageRole::User {
            out.extend(prior.iter().cloned());
            inserted = true;
        }
        out.push(message.clone());
    }
    if !inserted {
        out.extend(prior);
    }
    out
}

/// Go `applyContinuityResult`.
fn apply_continuity_result(result: &mut QueryResult, assembly: &ContinuityAssembly) {
    if !assembly.enabled {
        return;
    }
    result.thread_id = assembly.thread_id.clone();
    result.session_segment_id = assembly.session_segment_id.clone();
    result.request_turn_id = assembly.request_turn_id.clone();
    result.response_turn_id = assembly.response_turn_id.clone();
    result.continuity_preview_id = assembly.preview_id.clone();
    result.continuity_applied = assembly.applied;
    result.continuity_status = assembly.status;
    result.continuity_included_count = assembly.included.len() as i64;
    result.continuity_excluded_count = assembly.excluded_items.len() as i64;
}

/// Go `continuityPreviewItemCounts`.
fn continuity_preview_item_counts(items: &[ContinuityPreviewItem]) -> (i32, i32) {
    let mut included = 0;
    let mut excluded = 0;
    for item in items {
        match item.decision {
            ContinuityDecision::Included => included += 1,
            ContinuityDecision::Excluded => excluded += 1,
        }
    }
    (included, excluded)
}

/// Go `previewItemForHandoffSourceReference`.
#[must_use]
pub fn preview_item_for_handoff_source_reference(
    reference: &HandoffSourceReference,
    included: bool,
    order: i32,
) -> ContinuityPreviewItem {
    let (decision, reason) = if included {
        (
            ContinuityDecision::Included,
            ContinuityReason::IncludedRecent,
        )
    } else {
        (
            ContinuityDecision::Excluded,
            handoff_reference_continuity_reason(reference),
        )
    };
    ContinuityPreviewItem {
        preview_item_id: String::new(),
        continuity_preview_id: String::new(),
        tenant_id: reference.tenant_id.clone(),
        thread_id: reference.destination_thread_id.clone(),
        item_kind: ContinuityItemKind::HandoffSourceReference,
        continuity_turn_id: reference.continuity_turn_id.clone(),
        role: None,
        artifact_ref: String::new(),
        artifact_excerpt_id: String::new(),
        handoff_source_reference_id: reference.handoff_source_reference_id.clone(),
        decision,
        reason_code: reason,
        acceptance_sequence: 0,
        source_timestamp: None,
        safe_summary: reference.safe_summary.clone(),
        redaction_status: reference.redaction_status,
        item_order: order,
    }
}

/// Go `handoffReferenceContinuityReason`.
#[must_use]
pub fn handoff_reference_continuity_reason(reference: &HandoffSourceReference) -> ContinuityReason {
    if let Some(expires) = reference.retention_expires_at {
        if expires <= Utc::now() {
            return ContinuityReason::RetentionExpired;
        }
    }
    match reference.eligibility_status {
        HandoffSourceReferenceEligibility::PermissionDenied => ContinuityReason::PermissionDenied,
        HandoffSourceReferenceEligibility::RedactionFailed => ContinuityReason::RedactionFailed,
        HandoffSourceReferenceEligibility::RetentionExpired => ContinuityReason::RetentionExpired,
        HandoffSourceReferenceEligibility::ResetBoundary => ContinuityReason::ResetBoundary,
        HandoffSourceReferenceEligibility::IncompleteEvidence => {
            ContinuityReason::IncompleteEvidence
        }
        HandoffSourceReferenceEligibility::Unsupported => ContinuityReason::UnsupportedSource,
        HandoffSourceReferenceEligibility::Eligible => {
            if reference.decision != HandoffSourceReferenceDecision::Referenced {
                ContinuityReason::ContinuityUnavailable
            } else {
                ContinuityReason::IncludedRecent
            }
        }
    }
}

/// Builds the per-call current-thread Tokio runtime that bridges the sync
/// service into the async `kura-llm` dispatcher.
///
/// Time is enabled for the dispatcher's per-attempt timeout, and IO because a
/// provider may reach the network. Only time was enabled until an HTTP
/// provider was first dispatched here: the built-in `echo` and the managed CLI
/// providers do no socket work, so every test passed while any real endpoint
/// would panic on its first connection.
fn bridge_runtime() -> Result<tokio::runtime::Runtime, ChatError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|err| ChatError::Runtime(format!("build dispatch runtime: {err}")))
}

#[cfg(test)]
mod bridge_runtime_tests {
    use super::bridge_runtime;

    /// The dispatcher runs HTTP providers on this runtime.
    ///
    /// Without the IO driver the first socket panics with "IO is disabled",
    /// which no test caught because the built-in `echo` provider and the
    /// managed CLI providers never open one. This binds a listener because
    /// that is the cheapest operation that needs the driver at all.
    #[test]
    fn the_bridge_runtime_can_reach_the_network() {
        let runtime = bridge_runtime().expect("build the bridge runtime");
        runtime.block_on(async {
            tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("the bridge runtime must have an IO driver");
        });
    }
}
