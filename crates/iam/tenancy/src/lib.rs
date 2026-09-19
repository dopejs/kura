//! Tenancy accessor layer: tenant-aware *ForTenant helpers over the store RAW primitives.
//!
//! **Status (recorded 2026-09-19, Stage 8.7):** no production route calls
//! these accessors. Each takes an `SQLiteStore` by value, while the API holds
//! one shared `Arc<Mutex<SQLiteStore>>`, so Stage 8.2 implemented tenant
//! scoping at the API layer against the raw store methods directly. The
//! decision — recorded rather than left as two parallel mechanisms — is that
//! **the API-layer helpers are the production path** and this crate is the
//! executable specification of the cross-tenant semantics (fail-closed reads,
//! audited by-id denials, refused cross-tenant writes) that those helpers and
//! the store's `*_for_tenant_raw` / `bind_row_tenant` methods must honour. Its
//! tests are the contract; its structs are not to be wired. Migrating the API
//! onto these structs would require re-plumbing a shared handle through 16
//! accessors for no behavioural gain and was rejected.
//!
//! Port of daemon/internal/store/tenancy. The Runtime accessor (runs, sessions, steps,
//! tool_calls, llm_dispatches, checkpoints) and the per-domain accessors (approvals,
//! bindings, calendar, computer_use, delivery, events, integrations, mail, profiles,
//! reminders, schedules, threads, workflows, r37_resources, evaluation) resolve the
//! acting tenant via kura_identity::tenantctx::require() (Go tenantctx.Require) and
//! call the store's tenant-aware RAW methods from kura_store's tenancy module.
//!
//! Cross-tenant semantics (fail-closed, FR-006):
//! - Reads return only rows whose tenant_id equals the caller's tenant; rows still NULL
//!   pre-backfill are NOT returned.
//! - By-id lookups whose target row exists in another tenant emit the audit denial
//!   (kura_audit::Emitter::emit -> audit.cross_tenant_access_denied) and return
//!   not-found WITHOUT leaking the row's existence.
//! - Upserts whose target row is owned by a different tenant refuse the write
//!   (ErrCrossTenantWrite) and emit the audit denial; the existing row is preserved.
//!
//! Not ported yet (depend on kura-store connector-domain / live-validation CRUD that has
//! not landed): slack_setup.rs / telegram_setup.rs (Save/GetSlackHostedSetup + telegram
//! equivalents), the live-validation family in evaluation.rs, and evaluation_product.rs
//! (discovery policies, campaigns, fixtures, dashboard projections). They are documented
//! at the module level and should be wired when the underlying store methods exist.

pub mod approvals;
pub mod bindings;
pub mod calendar;
pub mod computer_use;
pub mod delivery;
pub mod evaluation;
pub mod events;
pub mod integrations;
pub mod mail;
pub mod profiles;
pub mod r37_resources;
pub mod reminders;
pub mod runtime;
pub mod schedules;
pub mod threads;
pub mod workflows;

use std::fmt;

pub use bindings::BindingAccessScope;
pub use kura_store::SQLiteStore;
pub use profiles::ProfileAccessScope;
pub use threads::ThreadAccessScope;

/// Re-export of the store's cross-tenant sentinel so callers can match on it without
/// importing the store crate.
pub const ERR_CROSS_TENANT_ROW: &str = kura_store::SQLiteStore::ERR_CROSS_TENANT_ROW;

/// Error type for the tenancy accessor layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TenancyError {
    /// The acting tenant context is missing (re-export of
    /// kura_identity::IdentityError::TenantContextRequired). Fail-closed.
    TenantContextRequired,
    /// The target row is owned by a different tenant and the write was refused. The
    /// existing row is preserved and the audit denial was emitted.
    CrossTenantWrite,
    /// Any underlying store error, surfaced verbatim.
    Store(String),
}

impl fmt::Display for TenancyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TenancyError::TenantContextRequired => write!(f, "tenant context required"),
            TenancyError::CrossTenantWrite => {
                write!(
                    f,
                    "tenancy: refused write to row owned by a different tenant"
                )
            }
            TenancyError::Store(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for TenancyError {}

impl From<kura_identity::IdentityError> for TenancyError {
    fn from(e: kura_identity::IdentityError) -> Self {
        match e {
            kura_identity::IdentityError::TenantContextRequired => {
                TenancyError::TenantContextRequired
            }
            other => TenancyError::Store(other.to_string()),
        }
    }
}

impl From<String> for TenancyError {
    fn from(e: String) -> Self {
        TenancyError::Store(e)
    }
}

/// tenantctx.Require port: returns the resolved tenant id from the task-local carrier,
/// or TenancyError::TenantContextRequired when none is installed. Every tenant-owned
/// helper MUST call this first and reject on error.
pub fn require() -> Result<String, TenancyError> {
    kura_identity::tenantctx::require().map_err(TenancyError::from)
}

/// tenantctx.Must port: panics when the tenant context is missing. Intended only for
/// code paths where missing context is a programmer error.
pub fn must() -> String {
    require().unwrap_or_else(|e| panic!("tenancy: {e}"))
}

/// Emits the cross-tenant access denial audit event when an emitter is wired. Mirrors
/// the Go accessor emit(ctx, surface, resourceKind) -> audit.cross_tenant_access_denied.
pub(crate) fn emit_denial(
    emitter: &Option<kura_audit::Emitter>,
    surface: &str,
    resource_kind: &str,
) {
    if let Some(emitter) = emitter {
        let _ = emitter.emit(surface, resource_kind);
    }
}
