//! Port of daemon/internal/bindings. See rs/MIGRATION.md for conventions.
//!
//! Roadmap 58 workspace and capability binding domain logic: tenant-owned workspace
//! records, channel/integration-account binding rules, capability visibility policy,
//! deterministic precedence resolution, fail-closed handling of invalid bindings, and
//! runtime binding evidence construction.
//!
//! This crate is pure domain logic. It owns no persistence and no transport; the
//! store, API, chat, and event layers consume the types and resolvers defined here.

mod dto;
mod policy;
mod precedence;
mod projection;
mod redaction;
mod types;
mod visibility;

pub use dto::{
    BindingResource, BindingRuntimeEvidenceResource, CapabilityDecisionResource,
    CapabilityVisibilityResource, CreateBindingRequest, CreateWorkspaceRequest,
    SetVisibilityRequest, UpdateBindingRequest, UpdateWorkspaceRequest, WorkspaceResource,
    to_binding_resource, to_capability_visibility_resource, to_runtime_evidence_resource,
    to_workspace_resource,
};
pub use policy::{
    BindingError, BindingMutationInput, CapabilityVisibilityMutationInput, WorkspaceMutationInput,
    invalid_binding_reason, repair_status_for_references, validate_binding_mutation,
    validate_capability_visibility_mutation, validate_workspace_mutation,
};
pub use precedence::{ResolutionInput, resolve_selection};
pub use projection::{RuntimeBindingEvidenceInput, build_runtime_binding_evidence};
pub use redaction::{MAX_SAFE_LABEL_LEN, redaction_status_for, safe_label, safe_reason};
pub use types::{
    BindingRule, BindingRuntimeScope, BindingStatus, CapabilityDecision,
    CapabilityVisibilityPolicy, Classification, DEFERRED_BINDING_CLASSIFICATION_MARKER,
    EffectiveBindingSelection, EffectiveVisibility, RedactionStatus, RepairStatus,
    ResolutionOutcome, RuntimeBindingEvidence, ScopeKind, ValidationStatus, Visibility,
    VisibilityScopeKind, Workspace, WorkspaceStatus,
};
pub use visibility::{
    ScopeVisibility, VisibilityInput, enforce_executable, resolve_capability_visibility,
    resolve_visibility_set,
};
