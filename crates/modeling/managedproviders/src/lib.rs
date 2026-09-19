//! Port of daemon/internal/managedproviders: the managed provider bridge
//! registry (Claude Code, Codex), the synchronous Bridge/Runner
//! abstractions, the sandbox-backed preflight evaluation machinery, and a
//! store-backed manager for state/check management.
//!
//! # Layout
//!
//! - bridge: Bridge, Runner, RunResult, RunError, ExecRunner, SandboxRunner,
//!   SandboxManager (trait), Registry, and the kura_providers ManagedRegistry
//!   / ManagedBridge adapters.
//! - claude: the Claude Code CLI bridge + kura_llm Provider shim.
//! - codex: the Codex CLI bridge + kura_llm Provider shim.
//! - evaluate: operation plans, preflight evaluation, consumer contract
//!   views, and execution finalization (the bridges.go machinery).
//! - manager: store-backed state/check management + the setup-wizard gate.
//!
//! # Porting notes / deferrals
//!
//! - context.Context is ported as kura_llm::CancelToken; all bridge methods
//!   are synchronous, matching the Go code (nothing streams).
//! - The operation plan Go threads through context.Context is passed
//!   explicitly to Runner::run.
//! - ExecRunner is fully ported on std::process. Cancellation is honored
//!   before spawning; killing an in-flight child process on cancellation is
//!   deferred (a sync port has no per-call worker to kill).
//! - The sandbox-backed execution plane (sandbox.Manager) is not yet ported
//!   in kura-sandbox (types only). The runner and preflight evaluation are
//!   ported against the SandboxManager trait, and Registry::new accepts
//!   Option<Arc<dyn SandboxManager>>; when the concrete manager lands it
//!   implements the trait. Live CLI auth-bridge execution through the sandbox
//!   is therefore deferred until then — the registry, bridge logic, evaluation,
//!   and store persistence are complete and testable with stub runners.
//! - Tenant-scoped persistence (Go tenancy) is not yet ported; the tenantless
//!   store write paths are used, like the Go daemon's non-tenant branch.

mod bridge;
mod claude;
mod codex;
mod error;
mod evaluate;
mod helpers;
mod manager;

pub use bridge::{
    Bridge, ExecRunner, ManagedBridgeAdapter, Registry, RunError, RunResult, Runner,
    SandboxManager, SandboxRunner,
};
pub use claude::{CLAUDE_PROVIDER_ID, ClaudeBridge, ClaudeCLIProvider, SettingsEvaluation};
pub use codex::{CODEX_PROVIDER_ID, CodexBridge, CodexCLIProvider, classify_cli_error};
pub use error::{DeniedEvaluation, Error};
pub use evaluate::{
    METADATA_ACCESS_SUMMARY, METADATA_ACTION, METADATA_DECISION, METADATA_FAILURE_CLASS,
    METADATA_OPERATION_ID, METADATA_PROFILE_ID, METADATA_PROVIDER_ID, METADATA_SENSITIVE_STATES,
    METADATA_STRENGTH, ManagedProviderOperationEvaluation, ManagedProviderOperationPlan,
    REDACTION_RULE, REQUESTED_BY_PREFIX, build_managed_provider_consumer_view,
    clone_access_request, clone_local_state_summaries, clone_operation_plan, consumer_view_json,
    evaluate_managed_provider_operation, finalize_managed_provider_execution_failure,
    finalize_managed_provider_execution_success, finalize_managed_provider_metadata,
    local_state_class_list, local_state_summary, new_managed_provider_operation_id,
    operation_metadata, operation_metadata_from_plan, secret_resolution_from_local_state,
};
pub use helpers::{
    base_name, clean_path_str, clone_roots, decode_jwt_payload, filepath_join,
    first_available_path, first_non_empty, home_fallback_workdir, latest_user_message,
    merge_string_maps, now_ptr, path_within_any, paths_within_declared, redacted_path_summary,
    resolve_path, user_home_dir,
};
pub use manager::{Manager, SyncResult, classify_dispatch_failure, failed_check};
