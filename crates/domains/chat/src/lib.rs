//! Port of `daemon/internal/chat`: the chat query/stream service.
//!
//! The service compiles prompts from agent overlays and selected skills,
//! resolves active-profile / binding / continuity context, prepares and
//! executes LLM dispatches through `kura-llm`, persists the dispatch and
//! continuity lifecycle through the store, and publishes `llm`, `thread`,
//! `agent_profile`, and `binding` events on the bus.
//!
//! Conventions follow the reference crates (`kura-runtime`,
//! `kura-orchestration`): camelCase serde wire types, `kura-threads` enums
//! for domain vocabularies, and typed errors. Go's `context.Context` maps to
//! the synchronous [`CancellationToken`] API; the async `kura-llm` dispatcher
//! is bridged through a per-call current-thread Tokio runtime. Streaming
//! supports both the Go callback-emitter shape ([`Service::stream`]) and a
//! `std::thread` + `std::sync::mpsc` variant ([`Service::stream_channel`]).

mod cancel;
mod error;
mod events;
mod round;
mod service;
mod store;
mod types;

pub use cancel::{CancellationToken, KillLink};
pub use error::ChatError;
pub use events::{
    AGENT_PROFILE_RUNTIME_PROJECTED_NAME, BINDING_RUNTIME_PROJECTED_NAME,
    THREAD_CONTINUITY_PREVIEW_RECORDED_NAME, THREAD_CONTINUITY_TURN_RECORDED_NAME,
    agent_profile_runtime_projected_event, binding_runtime_projected_event, new_event_id,
    thread_continuity_preview_recorded_event, thread_continuity_turn_recorded_event,
};
pub use service::{
    OPENAI_COMPATIBLE_PROVIDER_NAME, available_overlays, compile_prompt_messages,
    continuity_source_kind, handoff_reference_continuity_reason, inject_continuity_messages,
    inject_handoff_source_reference_messages, preview_item_for_handoff_source_reference,
    profile_context_messages, resolve_selected_skills, response_continuity_source_event_key,
    selected_skill_contracts, selected_skill_ids_from_skills, terminal_dispatch_event,
};
pub use store::{BindingResolutionParams, ChatStore, ContinuityLookupQuery};
pub use types::{QueryExecution, QueryInput, QueryResult, Service, StreamChunk};
