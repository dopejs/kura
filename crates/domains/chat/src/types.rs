//! Wire and service types for the chat package (Go `daemon/internal/chat`).

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::ChatError;
use crate::store::ChatStore;
use kura_events::Scope;
use kura_llm::{Dispatch, Usage};
use kura_threads::{
    ContinuityMode, ContinuityPreviewItem, ContinuityStatus, ContinuityTurn, SourceKind,
};

/// Go `chat.QueryInput`. `continuity_mode` is `None` when the caller did not
/// supply a mode, matching Go's empty-string `ContinuityMode` (the service
/// normalizes it via `threads.NormalizeContinuityMode`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryInput {
    pub query: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    #[serde(default)]
    pub timeout_ms: i64,
    #[serde(default)]
    pub max_retries: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thread_id: String,
    #[serde(default)]
    pub continuity_mode: Option<ContinuityMode>,
    #[serde(default)]
    pub scope: Scope,
    #[serde(default)]
    pub source_kind: Option<SourceKind>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_linkage_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_message_id: String,
    #[serde(default)]
    pub source_timestamp: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_event_key: String,
    /// Tenant-owned channel identity (Roadmap 48); empty when the work has no
    /// channel origin.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub channel_scope_ref: String,
    /// Tenant-owned integration-account identity (Roadmap 37); empty when the
    /// work has no account origin.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub account_scope_ref: String,
    /// Runtime run link for binding evidence (FR-013/FR-016, SC-008); empty
    /// for chat work with no runtime run.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub run_id: String,
}

/// Go `chat.QueryResult`. `continuity_status` is `None` when continuity was
/// not assembled (Go leaves the zero empty string).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryResult {
    pub query: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skill_contracts: Vec<serde_json::Map<String, serde_json::Value>>,
    pub dispatch: Dispatch,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_segment_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub request_turn_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub response_turn_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub continuity_preview_id: String,
    #[serde(default)]
    pub continuity_applied: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuity_status: Option<ContinuityStatus>,
    /// Stage 9.0: every tool call the turn made, in order. Empty when the
    /// model answered without tools.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_trace: Vec<crate::tools::ToolTraceEntry>,
    #[serde(default)]
    pub continuity_included_count: i64,
    #[serde(default)]
    pub continuity_excluded_count: i64,
}

/// Go `chat.StreamChunk` forwarded to the stream emitter.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamChunk {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dispatch_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skill_contracts: Vec<serde_json::Map<String, serde_json::Value>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub delta: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reply: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub finish_reason: String,
    #[serde(default)]
    pub usage: Option<Usage>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_segment_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub request_turn_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub continuity_preview_id: String,
    #[serde(default)]
    pub continuity_applied: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuity_status: Option<ContinuityStatus>,
}

/// Go's `(QueryResult, error)` return pair for dispatch execution failures:
/// the service persists and publishes the final dispatch before surfacing the
/// exec error, so callers receive both (the Go API layer distinguishes a hard
/// failure — `result.Dispatch.DispatchID == ""` — from an exec failure that
/// still produced a result).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct QueryExecution {
    pub result: QueryResult,
    pub exec_error: Option<ChatError>,
}

/// Internal continuity assembly (Go `continuityAssembly`).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ContinuityAssembly {
    pub(crate) enabled: bool,
    pub(crate) tenant_id: String,
    pub(crate) thread_id: String,
    pub(crate) session_segment_id: String,
    pub(crate) request_turn_id: String,
    pub(crate) response_turn_id: String,
    pub(crate) preview_id: String,
    pub(crate) applied: bool,
    pub(crate) status: Option<ContinuityStatus>,
    pub(crate) included: Vec<ContinuityTurn>,
    pub(crate) excluded_items: Vec<ContinuityPreviewItem>,
    pub(crate) handoff_link_ids: Vec<String>,
    pub(crate) handoff_items: Vec<ContinuityPreviewItem>,
    pub(crate) started_at: Option<DateTime<Utc>>,
    pub(crate) completed_at: Option<DateTime<Utc>>,
}

/// The chat query/stream service (Go `chat.Service`).
///
/// Dependencies mirror the Go constructor: a shared `kura-llm` dispatcher, an
/// optional provider manager, an optional skill registry, an optional event
/// bus, and an optional store. The store is any [`ChatStore`] (the real
/// `kura_store::SQLiteStore` adapter or an in-memory fake in tests). Arc
/// handles make the service cloneable so streaming can run on a dedicated
/// `std::thread` (see [`Service::stream_channel`]).
#[derive(Clone)]
pub struct Service {
    pub(crate) dispatcher: Arc<kura_llm::Dispatcher>,
    pub(crate) providers: Option<Arc<kura_providers::Manager>>,
    pub(crate) skills: Option<Arc<kura_skills::Registry>>,
    pub(crate) event_bus: Option<kura_events::Bus>,
    pub(crate) store: Option<Arc<dyn ChatStore>>,
    /// The plugin hook bus (pluginization phase 2). Absent = no hook points
    /// run and the pipeline behaves exactly as before.
    pub(crate) hooks: Option<Arc<kura_plugin::HookBus>>,
    /// Stage 9.0: the tool seam. Absent = the model is offered no tools and
    /// the turn is exactly one dispatch, as before.
    pub(crate) tool_host: Option<Arc<dyn crate::tools::ToolHost>>,
    /// Bound on tool rounds per turn (see `DEFAULT_MAX_TOOL_ROUNDS`).
    pub(crate) max_tool_rounds: usize,
}

impl Service {
    /// Go `chat.NewService`. The dispatcher is required; every other
    /// dependency may be absent (matching Go's nil parameters, which the
    /// service guards per call).
    #[must_use]
    pub fn new_service(
        dispatcher: Arc<kura_llm::Dispatcher>,
        providers: Option<Arc<kura_providers::Manager>>,
        skills: Option<Arc<kura_skills::Registry>>,
        event_bus: Option<kura_events::Bus>,
        store: Option<Arc<dyn ChatStore>>,
    ) -> Self {
        Service {
            dispatcher,
            providers,
            skills,
            event_bus,
            store,
            hooks: None,
            tool_host: None,
            max_tool_rounds: crate::tools::DEFAULT_MAX_TOOL_ROUNDS,
        }
    }

    /// Attaches the tool host (Stage 9.0). `max_rounds` bounds tool rounds
    /// per turn; `0` falls back to the default rather than meaning
    /// "unbounded".
    pub fn set_tool_host(&mut self, host: Arc<dyn crate::tools::ToolHost>, max_rounds: usize) {
        self.tool_host = Some(host);
        self.max_tool_rounds = if max_rounds == 0 {
            crate::tools::DEFAULT_MAX_TOOL_ROUNDS
        } else {
            max_rounds
        };
    }

    /// Attaches the plugin hook bus; the `chat/turn-start`,
    /// `chat/pre-dispatch`, and `chat/turn-end` points run on every
    /// query/stream once set.
    pub fn set_hooks(&mut self, hooks: Arc<kura_plugin::HookBus>) {
        self.hooks = Some(hooks);
    }
}
