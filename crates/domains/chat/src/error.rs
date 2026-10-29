//! Error taxonomy for the chat service (Go `daemon/internal/chat`).
//!
//! The Go package returns plain `error` values built from sentinel errors
//! (`ErrBindingRepairRequired`, `providers.ErrProviderAuthUnavailable`,
//! `skills.ErrSkillsRegistryMissing`) and wrapped store/LLM errors. This
//! crate surfaces the same vocabulary as a typed enum in the style of the
//! `kura-runtime` and `kura-orchestration` error enums.

use kura_llm::PrepareError;

/// Chat query/stream failures. Payload strings carry the Go error text where
/// the Go code wraps a plain error, so callers can surface the same messages.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ChatError {
    /// Go `errors.New("chat service is not configured")`. Unreachable in Rust
    /// because `new_service` requires a non-null dispatcher; kept for surface
    /// parity with the Go nil-receiver guard.
    #[error("chat service is not configured")]
    NotConfigured,
    /// Go `errors.New("query is required")`.
    #[error("query is required")]
    QueryRequired,
    /// Go `skills.ErrSkillsRegistryMissing`.
    #[error("skills registry is not configured")]
    SkillsRegistryMissing,
    /// Go `skills` resolution failures (e.g. "skill not found: X").
    #[error("skills: {0}")]
    Skills(String),
    /// Go `ErrBindingRepairRequired` wrapped with the repair reason.
    #[error("binding selection requires repair: {0}")]
    BindingRepairRequired(String),
    /// Go `bindings.EnforceExecutable` failure wrapped with the skill id.
    #[error("capability is not executable: {0}")]
    CapabilityNotExecutable(String),
    /// Go `fmt.Errorf("%w: %s", providers.ErrProviderAuthUnavailable, reason)`.
    #[error("tenant provider auth is unavailable: {0}")]
    ProviderAuthUnavailable(String),
    /// Provider-manager resolution failures (`providers.ResolveDispatchInput`).
    #[error("provider resolution failed: {0}")]
    Provider(String),
    /// Store CRUD failures.
    #[error("store operation failed: {0}")]
    Store(String),
    /// A store method the `kura-store` crate has not ported yet; the chat
    /// service is written against the full Go store surface, and the
    /// `kura-store` adapter fails these calls explicitly instead of silently
    /// degrading.
    #[error("store method not ported: {0}")]
    StoreMethodDeferred(String),
    /// `kura-llm` dispatch preparation failures.
    #[error(transparent)]
    Prepare(#[from] PrepareError),
    /// Dispatch execution failure (Go exec error). The final dispatch record
    /// is still persisted and returned alongside this error via
    /// `QueryExecution`.
    #[error("dispatch failed: {0}")]
    Dispatch(String),
    /// The stream emitter callback aborted the stream.
    #[error("stream emitter failed: {0}")]
    Emit(String),
    /// Sync/async bridge failures (per-call Tokio runtime).
    #[error("runtime bridge failed: {0}")]
    Runtime(String),
    /// A hook handler vetoed the turn (pluginization phase 2). Carries the
    /// hook point, the registering plugin id, and the handler's reason.
    #[error("turn vetoed by plugin `{plugin_id}` at {point}: {reason}")]
    HookVetoed {
        point: String,
        plugin_id: String,
        reason: String,
    },
    /// A hook handler left the payload in a shape the pipeline cannot read
    /// back (e.g. non-message objects in `messages`).
    #[error("hook payload invalid at {point}: {reason}")]
    HookPayload { point: String, reason: String },
}

impl ChatError {
    /// A stable, snake_case code for the failure, for traces and logs that
    /// must not carry the message text.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            ChatError::NotConfigured => "chat_not_configured",
            ChatError::QueryRequired => "query_required",
            ChatError::SkillsRegistryMissing => "skills_registry_missing",
            ChatError::Skills(_) => "skills",
            ChatError::BindingRepairRequired(_) => "binding_repair_required",
            ChatError::CapabilityNotExecutable(_) => "capability_not_executable",
            ChatError::ProviderAuthUnavailable(_) => "provider_auth_unavailable",
            ChatError::Provider(_) => "provider",
            ChatError::Store(_) | ChatError::StoreMethodDeferred(_) => "store",
            ChatError::Prepare(_) => "prepare",
            ChatError::Dispatch(_) => "dispatch",
            ChatError::Emit(_) => "emit",
            ChatError::Runtime(_) => "runtime",
            ChatError::HookVetoed { .. } => "hook_vetoed",
            ChatError::HookPayload { .. } => "hook_payload",
        }
    }

    /// Convenience wrapper for the deferred-store-method error so callers and
    /// the `kura-store` adapter share one message shape.
    #[must_use]
    pub fn deferred_store_method(name: &str) -> Self {
        ChatError::StoreMethodDeferred(format!("{name} (not ported to kura-store)"))
    }
}
