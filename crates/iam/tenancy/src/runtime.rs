//! Tenant-aware accessor for the runtime spine (runs, sessions, steps, tool_calls,
//! llm_dispatches, checkpoints). Port of daemon/internal/store/tenancy/runtime.go.
//!
//! Pass A semantics:
//! - Reads return only rows whose tenant_id equals the caller's tenant (rows still NULL
//!   pre-backfill are NOT returned, fail-closed).
//! - Writes go through the atomic *ForTenantSafe store primitives which bind tenant_id
//!   in the same INSERT; a row owned by a different tenant is preserved and the write is
//!   refused with ErrCrossTenantWrite.
//! - By-id lookups whose target row exists in another tenant return not-found AND emit
//!   audit.cross_tenant_access_denied. The existence of the row is never leaked.

use kura_llm::Dispatch;
use kura_router::Session;
use kura_runtime::{Run, RunCheckpoint, Step, ToolCall};

use crate::{TenancyError, emit_denial, require};

/// Tenant-aware accessor for the runtime spine.
pub struct Runtime {
    store: crate::SQLiteStore,
    emitter: Option<kura_audit::Emitter>,
}

impl Runtime {
    /// Constructs the accessor. emitter may be None (audit publication is skipped, but
    /// production callers should wire the daemon's shared emitter).
    #[must_use]
    pub fn new(store: crate::SQLiteStore, emitter: Option<kura_audit::Emitter>) -> Self {
        Runtime { store, emitter }
    }

    fn emit(&self, surface: &str, resource_kind: &str) {
        emit_denial(&self.emitter, surface, resource_kind);
    }

    // ----- runs -----

    pub fn list_runs_for_tenant(&self) -> Result<Vec<Run>, TenancyError> {
        let tenant_id = require()?;
        self.store
            .list_runs_for_tenant_raw(&tenant_id)
            .map_err(TenancyError::from)
    }

    pub fn get_run_for_tenant(&self, run_id: &str) -> Result<Option<Run>, TenancyError> {
        let tenant_id = require()?;
        match self.store.get_run_for_tenant_raw(run_id, &tenant_id) {
            Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                self.emit("store:GetRunForTenant", "run");
                Ok(None)
            }
            other => other.map_err(TenancyError::from),
        }
    }

    pub fn upsert_run_for_tenant(&self, run: &Run) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        match self.store.upsert_run_for_tenant_safe(run, &tenant_id) {
            Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                self.emit("store:UpsertRunForTenant", "run");
                Err(TenancyError::CrossTenantWrite)
            }
            other => other.map_err(TenancyError::from),
        }
    }

    pub fn delete_run_for_tenant(&self, run_id: &str) -> Result<bool, TenancyError> {
        let tenant_id = require()?;
        match self
            .store
            .delete_row_for_tenant("runs", "run_id", run_id, &tenant_id)
        {
            Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                self.emit("store:DeleteRunForTenant", "run");
                Ok(false)
            }
            other => other.map_err(TenancyError::from),
        }
    }

    // ----- sessions -----

    pub fn list_sessions_for_tenant(&self) -> Result<Vec<Session>, TenancyError> {
        let tenant_id = require()?;
        self.store
            .list_sessions_for_tenant_raw(&tenant_id)
            .map_err(TenancyError::from)
    }

    pub fn upsert_session_for_tenant(&self, session: &Session) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        match self
            .store
            .upsert_session_for_tenant_safe(session, &tenant_id)
        {
            Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                self.emit("store:UpsertSessionForTenant", "session");
                Err(TenancyError::CrossTenantWrite)
            }
            other => other.map_err(TenancyError::from),
        }
    }

    pub fn delete_session_for_tenant(&self, session_id: &str) -> Result<bool, TenancyError> {
        let tenant_id = require()?;
        match self
            .store
            .delete_row_for_tenant("sessions", "session_id", session_id, &tenant_id)
        {
            Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                self.emit("store:DeleteSessionForTenant", "session");
                Ok(false)
            }
            other => other.map_err(TenancyError::from),
        }
    }

    // ----- steps -----

    pub fn list_steps_for_tenant(&self, run_id: &str) -> Result<Vec<Step>, TenancyError> {
        let tenant_id = require()?;
        self.store
            .list_steps_for_tenant_raw(&tenant_id, run_id)
            .map_err(TenancyError::from)
    }

    pub fn upsert_step_for_tenant(&self, step: &Step) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        match self.store.upsert_step_for_tenant_safe(step, &tenant_id) {
            Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                self.emit("store:UpsertStepForTenant", "step");
                Err(TenancyError::CrossTenantWrite)
            }
            other => other.map_err(TenancyError::from),
        }
    }

    // ----- tool_calls -----

    pub fn list_tool_calls_for_tenant(
        &self,
        run_id: &str,
        step_id: &str,
    ) -> Result<Vec<ToolCall>, TenancyError> {
        let tenant_id = require()?;
        self.store
            .list_tool_calls_for_tenant_raw(&tenant_id, run_id, step_id)
            .map_err(TenancyError::from)
    }

    pub fn upsert_tool_call_for_tenant(&self, tool_call: &ToolCall) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        match self
            .store
            .upsert_tool_call_for_tenant_safe(tool_call, &tenant_id)
        {
            Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                self.emit("store:UpsertToolCallForTenant", "tool_call");
                Err(TenancyError::CrossTenantWrite)
            }
            other => other.map_err(TenancyError::from),
        }
    }

    // ----- llm_dispatches -----

    pub fn list_llm_dispatches_for_tenant(&self) -> Result<Vec<Dispatch>, TenancyError> {
        let tenant_id = require()?;
        self.store
            .list_llm_dispatches_for_tenant_raw(&tenant_id)
            .map_err(TenancyError::from)
    }

    pub fn get_llm_dispatch_for_tenant(
        &self,
        dispatch_id: &str,
    ) -> Result<Option<Dispatch>, TenancyError> {
        let tenant_id = require()?;
        match self
            .store
            .get_llm_dispatch_for_tenant_raw(dispatch_id, &tenant_id)
        {
            Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                self.emit("store:GetLLMDispatchForTenant", "llm_dispatch");
                Ok(None)
            }
            other => other.map_err(TenancyError::from),
        }
    }

    pub fn upsert_llm_dispatch_for_tenant(&self, dispatch: &Dispatch) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        match self
            .store
            .upsert_llm_dispatch_for_tenant_safe(dispatch, &tenant_id)
        {
            Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                self.emit("store:UpsertLLMDispatchForTenant", "llm_dispatch");
                Err(TenancyError::CrossTenantWrite)
            }
            other => other.map_err(TenancyError::from),
        }
    }

    // ----- checkpoints -----

    pub fn list_latest_checkpoints_for_tenant(&self) -> Result<Vec<RunCheckpoint>, TenancyError> {
        let tenant_id = require()?;
        self.store
            .list_latest_checkpoints_for_tenant_raw(&tenant_id)
            .map_err(TenancyError::from)
    }

    /// Writes a checkpoint row and binds tenant_id to it. Because checkpoint_id is
    /// generated server-side and not surfaced to callers, the bind step uses
    /// (run_id, captured_at) via the parent run.
    pub fn save_checkpoint_for_tenant(
        &self,
        checkpoint: &RunCheckpoint,
    ) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        self.store
            .save_checkpoint_for_tenant_safe(checkpoint, &tenant_id)
            .map_err(TenancyError::from)
    }
}

/// Resolves the tenant id for an opportunistic write path that does NOT yet go through
/// the Runtime accessor. Returns an empty string when the context carries no tenant; the
/// empty string maps to a NULL tenant_id column write, which is allowed pre-backfill.
pub fn runtime_tenant_id() -> String {
    require().unwrap_or_default()
}

/// Returns the SQL predicate fragment + arg used to scope a tenant-owned read against a
/// table whose tenant_id may still be NULL during the backfill window. The fragment is
/// suitable for appending after an existing WHERE clause via " AND ".
pub fn runtime_tenant_predicate(tenant_id: &str) -> (String, Option<String>) {
    if tenant_id.is_empty() {
        (String::new(), None)
    } else {
        (
            "(tenant_id = ? OR tenant_id IS NULL)".to_string(),
            Some(tenant_id.to_string()),
        )
    }
}
