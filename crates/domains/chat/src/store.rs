//! The store surface the chat service depends on.
//!
//! Go's `Service` holds a `*store.SQLiteStore` and calls a fixed set of CRUD
//! methods (dispatch persistence, event append, setup-session lookup, profile
//! selection, binding resolution, continuity turns/previews, handoff links).
//! The `kura-store` crate has ported only the first two so far, so this
//! module declares the full surface as a `ChatStore` trait (with the Go
//! query/parameter structs) and implements it for `kura_store::SQLiteStore`:
//! the ported methods delegate to SQLite, and the not-yet-ported methods fail
//! with [`ChatError::StoreMethodDeferred`] instead of silently degrading.
//! Tests exercise the whole pipeline against an in-memory fake store.

use chrono::{DateTime, Utc};
use kura_bindings::{CapabilityDecision, EffectiveBindingSelection, RuntimeBindingEvidence};
use kura_events::Event;
use kura_llm::Dispatch;
use kura_profiles::{ActiveSelection, AgentProfile, RuntimeProjection};
use kura_setupwizard::SetupSession;
use kura_threads::{
    ContinuityPreview, ContinuityPreviewItem, ContinuityTurn, HandoffLink, HandoffSourceReference,
    Thread,
};

/// Go `store.ContinuityLookupQuery`. A `now` of `None` maps to Go's zero
/// `time.Time` (the store substitutes the current time); `limit <= 0` lets
/// the store apply its default (`DefaultContinuityMaxPriorTurns + 64`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ContinuityLookupQuery {
    pub tenant_id: String,
    pub thread_id: String,
    pub session_segment_id: String,
    pub limit: i64,
    pub now: Option<DateTime<Utc>>,
}

/// Go `store.BindingResolutionParams` (binding_resolution.go).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BindingResolutionParams {
    pub tenant_id: String,
    pub channel_scope_ref: String,
    pub account_scope_ref: String,
    pub tenant_default_profile_id: String,
    pub tenant_default_profile_version_id: String,
}

/// The store methods `daemon/internal/chat/service.go` calls on
/// `store.SQLiteStore`, in Go signature order.
pub trait ChatStore: Send + Sync {
    // -- llm dispatch CRUD (ported in kura-store) -------------------------
    fn upsert_llm_dispatch(&self, dispatch: &Dispatch) -> Result<(), String>;
    /// D11 (2026-09-19): binds a persisted dispatch row to its tenant so the
    /// tenant-scoped list/get paths can see it. The dispatch row itself has
    /// no tenant column in its upsert; without this binding every chat
    /// dispatch stayed a NULL-tenant row, visible only to the single-user
    /// assembly. Default no-op keeps store fakes unchanged.
    fn bind_llm_dispatch_tenant(&self, _dispatch_id: &str, _tenant_id: &str) -> Result<(), String> {
        Ok(())
    }
    // -- event ledger (ported in kura-store) ------------------------------
    fn append_event(&self, event: &Event) -> Result<Event, String>;
    // -- setup wizard (not yet ported to kura-store) ----------------------
    fn list_setup_sessions(&self, tenant_id: &str) -> Result<Vec<SetupSession>, String>;
    // -- profile projection (not yet ported) ------------------------------
    fn active_agent_profile_selection(
        &self,
        tenant_id: &str,
    ) -> Result<Option<(AgentProfile, ActiveSelection)>, String>;
    fn record_runtime_profile_projection(
        &self,
        projection: &RuntimeProjection,
    ) -> Result<RuntimeProjection, String>;
    // -- binding resolution + evidence (not yet ported) -------------------
    fn resolve_binding_selection(
        &self,
        params: &BindingResolutionParams,
    ) -> Result<EffectiveBindingSelection, String>;
    fn effective_capability_visibility(
        &self,
        tenant_id: &str,
        profile_id: &str,
        workspace_id: &str,
        capability_id: &str,
    ) -> Result<CapabilityDecision, String>;
    fn record_runtime_binding_evidence(
        &self,
        evidence: &RuntimeBindingEvidence,
    ) -> Result<RuntimeBindingEvidence, String>;
    // -- thread/continuity/handoff (not yet ported) -----------------------
    fn get_thread_for_tenant(
        &self,
        tenant_id: &str,
        thread_id: &str,
    ) -> Result<Option<Thread>, String>;
    fn list_continuity_turns(
        &self,
        query: &ContinuityLookupQuery,
    ) -> Result<Vec<ContinuityTurn>, String>;
    fn list_continuity_turns_outside_session_segment(
        &self,
        query: &ContinuityLookupQuery,
    ) -> Result<Vec<ContinuityTurn>, String>;
    fn list_handoff_links_for_thread(
        &self,
        tenant_id: &str,
        thread_id: &str,
        limit: i64,
    ) -> Result<Vec<HandoffLink>, String>;
    fn list_handoff_source_references_for_link(
        &self,
        tenant_id: &str,
        link_id: &str,
    ) -> Result<Vec<HandoffSourceReference>, String>;
    fn save_continuity_turn(&self, turn: &ContinuityTurn) -> Result<ContinuityTurn, String>;
    fn mark_handoff_source_references_consumed(
        &self,
        tenant_id: &str,
        link_id: &str,
        response_turn_id: &str,
        now: DateTime<Utc>,
    ) -> Result<(), String>;
    fn save_continuity_preview(
        &self,
        preview: &ContinuityPreview,
        items: &[ContinuityPreviewItem],
    ) -> Result<ContinuityPreview, String>;
}

/// Adapter for the `kura-store` SQLite store: every ChatStore method
/// delegates to the native kura-store implementation. `resolve_binding_selection`
/// composes the store's channel/account rule lookups and selectability
/// checks through `kura_bindings::resolve_selection` (the Go precedence
/// port), preserving the fail-closed semantics.
impl ChatStore for std::sync::Mutex<kura_store::SQLiteStore> {
    fn bind_llm_dispatch_tenant(&self, dispatch_id: &str, tenant_id: &str) -> Result<(), String> {
        if tenant_id.trim().is_empty() {
            return Ok(());
        }
        self.lock()
            .map_err(|_| "chat store mutex poisoned".to_string())?
            .bind_row_tenant(
                "llm_dispatches",
                "dispatch_id",
                dispatch_id,
                tenant_id.trim(),
            )
    }

    fn upsert_llm_dispatch(&self, dispatch: &Dispatch) -> Result<(), String> {
        self.lock()
            .map_err(|_| "lock sqlite store".to_string())?
            .upsert_llm_dispatch(dispatch)
    }
    fn append_event(&self, event: &Event) -> Result<Event, String> {
        self.lock()
            .map_err(|_| "lock sqlite store".to_string())?
            .append_event(event)
    }
    fn list_setup_sessions(&self, tenant_id: &str) -> Result<Vec<SetupSession>, String> {
        self.lock()
            .map_err(|_| "lock sqlite store".to_string())?
            .list_setup_sessions(tenant_id)
    }
    fn active_agent_profile_selection(
        &self,
        tenant_id: &str,
    ) -> Result<Option<(AgentProfile, ActiveSelection)>, String> {
        self.lock()
            .map_err(|_| "lock sqlite store".to_string())?
            .active_agent_profile_selection(tenant_id)
    }
    fn record_runtime_profile_projection(
        &self,
        projection: &RuntimeProjection,
    ) -> Result<RuntimeProjection, String> {
        self.lock()
            .map_err(|_| "lock sqlite store".to_string())?
            .record_runtime_profile_projection(projection.clone())
    }
    fn resolve_binding_selection(
        &self,
        params: &BindingResolutionParams,
    ) -> Result<EffectiveBindingSelection, String> {
        let guard = self.lock().map_err(|_| "lock sqlite store".to_string())?;
        let tenant_id = params.tenant_id.trim();
        let channel_binding =
            guard.resolve_channel_binding(tenant_id, &params.channel_scope_ref)?;
        let account_binding =
            guard.resolve_account_binding(tenant_id, &params.account_scope_ref)?;
        let tenant_default_workspace_id = guard
            .ensure_default_workspace(tenant_id)
            .map(|ws| ws.workspace_id)
            .unwrap_or_default();

        // The resolver's availability oracles only ever probe the candidate
        // ids below; precompute their selectability so the closures own
        // plain sets (the store guard is neither Send nor Sync).
        let mut profile_candidates: Vec<String> = Vec::new();
        let mut workspace_candidates: Vec<String> = vec![tenant_default_workspace_id.clone()];
        profile_candidates.push(params.tenant_default_profile_id.clone());
        for rule in [channel_binding.as_ref(), account_binding.as_ref()]
            .into_iter()
            .flatten()
        {
            profile_candidates.push(rule.selected_profile_id.clone());
            workspace_candidates.push(rule.selected_workspace_id.clone());
        }
        let mut selectable_profiles = std::collections::HashSet::new();
        for id in profile_candidates {
            let id = id.trim().to_string();
            if !id.is_empty() && guard.is_profile_selectable(tenant_id, &id)? {
                selectable_profiles.insert(id);
            }
        }
        let mut selectable_workspaces = std::collections::HashSet::new();
        for id in workspace_candidates {
            let id = id.trim().to_string();
            if !id.is_empty() && guard.is_workspace_selectable(tenant_id, &id)? {
                selectable_workspaces.insert(id);
            }
        }
        drop(guard);

        let input = kura_bindings::ResolutionInput {
            channel_binding,
            account_binding,
            tenant_default_profile_id: params.tenant_default_profile_id.clone(),
            tenant_default_profile_version_id: params.tenant_default_profile_version_id.clone(),
            tenant_default_workspace_id,
            profile_available: Some(Box::new(move |id: &str| {
                selectable_profiles.contains(id.trim())
            })),
            workspace_available: Some(Box::new(move |id: &str| {
                selectable_workspaces.contains(id.trim())
            })),
        };
        Ok(kura_bindings::resolve_selection(&input))
    }
    fn effective_capability_visibility(
        &self,
        tenant_id: &str,
        profile_id: &str,
        workspace_id: &str,
        capability_id: &str,
    ) -> Result<CapabilityDecision, String> {
        self.lock()
            .map_err(|_| "lock sqlite store".to_string())?
            // The chat pipeline carries no scope-visibility limits (Go parity:
            // the service call passes none; limits ride the bindings routes).
            .effective_capability_visibility(
                tenant_id,
                profile_id,
                workspace_id,
                capability_id,
                &[],
            )
    }
    fn record_runtime_binding_evidence(
        &self,
        evidence: &RuntimeBindingEvidence,
    ) -> Result<RuntimeBindingEvidence, String> {
        self.lock()
            .map_err(|_| "lock sqlite store".to_string())?
            .record_runtime_binding_evidence(evidence.clone())
    }
    fn get_thread_for_tenant(
        &self,
        tenant_id: &str,
        thread_id: &str,
    ) -> Result<Option<Thread>, String> {
        self.lock()
            .map_err(|_| "lock sqlite store".to_string())?
            .get_thread_for_tenant(tenant_id, thread_id)
    }
    fn list_continuity_turns(
        &self,
        query: &ContinuityLookupQuery,
    ) -> Result<Vec<ContinuityTurn>, String> {
        self.lock()
            .map_err(|_| "lock sqlite store".to_string())?
            .list_continuity_turns(&store_continuity_query(query))
    }
    fn list_continuity_turns_outside_session_segment(
        &self,
        query: &ContinuityLookupQuery,
    ) -> Result<Vec<ContinuityTurn>, String> {
        self.lock()
            .map_err(|_| "lock sqlite store".to_string())?
            .list_continuity_turns_outside_session_segment(&store_continuity_query(query))
    }
    fn list_handoff_links_for_thread(
        &self,
        tenant_id: &str,
        thread_id: &str,
        limit: i64,
    ) -> Result<Vec<HandoffLink>, String> {
        self.lock()
            .map_err(|_| "lock sqlite store".to_string())?
            .list_handoff_links(tenant_id, thread_id, limit)
    }
    fn list_handoff_source_references_for_link(
        &self,
        tenant_id: &str,
        link_id: &str,
    ) -> Result<Vec<HandoffSourceReference>, String> {
        self.lock()
            .map_err(|_| "lock sqlite store".to_string())?
            .list_handoff_source_references_for_link(tenant_id, link_id)
    }
    fn save_continuity_turn(&self, turn: &ContinuityTurn) -> Result<ContinuityTurn, String> {
        self.lock()
            .map_err(|_| "lock sqlite store".to_string())?
            .save_continuity_turn(turn)
    }
    fn mark_handoff_source_references_consumed(
        &self,
        tenant_id: &str,
        link_id: &str,
        response_turn_id: &str,
        now: DateTime<Utc>,
    ) -> Result<(), String> {
        self.lock()
            .map_err(|_| "lock sqlite store".to_string())?
            .mark_handoff_source_references_consumed(
                tenant_id,
                link_id,
                response_turn_id,
                Some(now),
            )
    }
    fn save_continuity_preview(
        &self,
        preview: &ContinuityPreview,
        items: &[ContinuityPreviewItem],
    ) -> Result<ContinuityPreview, String> {
        // The store mutates item ids/order in place; the trait exposes an
        // immutable slice, so hand it an owned copy.
        let mut items = items.to_vec();
        self.lock()
            .map_err(|_| "lock sqlite store".to_string())?
            .save_continuity_preview(preview.clone(), &mut items)
    }
}

/// Maps the chat-crate lookup query onto the store's own query struct.
fn store_continuity_query(
    query: &ContinuityLookupQuery,
) -> kura_store::thread_continuity::ContinuityLookupQuery {
    kura_store::thread_continuity::ContinuityLookupQuery {
        tenant_id: query.tenant_id.clone(),
        thread_id: query.thread_id.clone(),
        session_segment_id: query.session_segment_id.clone(),
        limit: query.limit,
        now: query.now,
    }
}
