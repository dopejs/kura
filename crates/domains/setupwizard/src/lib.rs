//! Port of `daemon/internal/setupwizard`: the hosted signup/credential/OAuth setup wizard
//! (Roadmap 31). Owns the setup session state machine, submitted-secret and OAuth flows,
//! redaction, diagnostics, audit, and dependent-use decisions.

mod audit;
mod catalog;
mod helpers;
mod permissions;
mod probe;
mod service;
mod types;

pub use audit::{TenantAuditRecorder, audit_record_for_attempt};
pub use catalog::{catalog_targets, target_by_id};
pub use helpers::{
    contains_forbidden_evidence, fail_closed, redacted_oauth_evidence, redacted_secret_evidence,
};
pub use permissions::{can_inspect_setup, can_mutate_setup, require_inspection, require_mutation};
pub use probe::{DefaultDiagnosticProbe, classify_diagnostic_reason, diagnostic_for_session};
pub use service::{
    BoxFuture, DisableInput, MemoryStore, OAuthCallbackInput, OAuthStartInput, OAuthStartResult,
    ReplaceInput, Service, ServiceDependencies, StartInput, Store, SubmitSecretInput, new_service,
};
pub use types::*;
