//! Validation policy (port of policy.go): safe, bounded mutation inputs and
//! user-visible reason codes (FR-009, FR-018, FR-022, FR-032).

use thiserror::Error;

use crate::types::{
    BindingStatus, RepairStatus, ScopeKind, Visibility, VisibilityScopeKind, WorkspaceStatus,
};

/// All binding-domain failures. `Validation` wraps the safe, user-visible reason
/// code carried by Go's `ValidationError`; `InvalidBinding` is the bare sentinel.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BindingError {
    /// Bare sentinel (Go `ErrInvalidBinding`).
    #[error("binding validation failed")]
    InvalidBinding,
    /// Validation failure carrying a safe reason code (FR-009).
    #[error("binding validation failed: {0}")]
    Validation(String),
    /// Returned when a hidden or disabled capability is requested for execution
    /// under the active binding (FR-016).
    #[error("capability is not executable under the active binding")]
    CapabilityNotExecutable,
    /// Guards mutations that must name an authorized actor.
    #[error("binding mutation requires an explicit authorized actor")]
    ExplicitActorRequired,
}

impl BindingError {
    /// Extracts the safe reason code from a validation error (Go
    /// `ValidationReasonCode`). Returns `None` for non-validation errors.
    pub fn reason_code(&self) -> Option<&str> {
        match self {
            BindingError::Validation(code) if !code.trim().is_empty() => Some(code.trim()),
            BindingError::InvalidBinding => Some("binding_validation_failed"),
            _ => None,
        }
    }
}

/// Builds a validation error with a safe reason code (Go `InvalidBindingReason`).
pub fn invalid_binding_reason(reason_code: &str) -> BindingError {
    let reason_code = reason_code.trim();
    if reason_code.is_empty() {
        return BindingError::Validation("binding_validation_failed".to_string());
    }
    BindingError::Validation(reason_code.to_string())
}

/// The validated shape for creating/updating a workspace.
#[derive(Debug, Clone, Default)]
pub struct WorkspaceMutationInput {
    pub display_name: String,
    pub status: WorkspaceStatus,
}

/// Enforces safe, bounded workspace fields (FR-002, FR-032).
pub fn validate_workspace_mutation(input: &WorkspaceMutationInput) -> Result<(), BindingError> {
    let name = input.display_name.trim();
    if name.is_empty() {
        return Err(invalid_binding_reason("workspace_display_name_required"));
    }
    if over_limit(name, 120) {
        return Err(invalid_binding_reason("workspace_display_name_too_long"));
    }
    if contains_unsafe(name) {
        return Err(invalid_binding_reason("unsafe_workspace_content"));
    }
    if !(input.status.is_empty()
        || input.status == WorkspaceStatus::ACTIVE
        || input.status == WorkspaceStatus::ARCHIVED
        || input.status == WorkspaceStatus::DISABLED)
    {
        return Err(invalid_binding_reason("workspace_status_invalid"));
    }
    Ok(())
}

/// The validated shape for creating/updating a binding rule.
///
/// `cross_tenant`, `scope_ref_available`, `scope_connector_supported`,
/// `profile_selectable`, and `workspace_selectable` carry the caller's tenant-scoped
/// validation findings so policy can reject cross-tenant and unavailable references
/// with safe reasons (FR-009).
#[derive(Debug, Clone, Default)]
pub struct BindingMutationInput {
    pub scope_kind: ScopeKind,
    pub scope_ref: String,
    pub selected_profile_id: String,
    pub selected_workspace_id: String,
    pub cross_tenant: bool,
    pub scope_ref_available: bool,
    pub scope_connector_supported: bool,
    pub profile_selectable: bool,
    pub workspace_selectable: bool,
}

/// Rejects cross-tenant resources, unavailable channels, disconnected accounts,
/// archived/disabled profiles, unavailable workspaces, unsupported connector
/// surfaces, malformed values, and policy-conflicting selections with safe
/// user-visible reasons (FR-009).
pub fn validate_binding_mutation(input: &BindingMutationInput) -> Result<(), BindingError> {
    if input.cross_tenant {
        return Err(invalid_binding_reason("cross_tenant_reference_denied"));
    }
    if input.scope_kind != ScopeKind::CHANNEL && input.scope_kind != ScopeKind::INTEGRATION_ACCOUNT
    {
        return Err(invalid_binding_reason("binding_scope_kind_invalid"));
    }
    let scope_ref = input.scope_ref.trim();
    if scope_ref.is_empty()
        || over_limit(scope_ref, 256)
        || contains_unsafe(scope_ref)
        || !identifier_like(scope_ref)
    {
        return Err(invalid_binding_reason("binding_scope_ref_malformed"));
    }
    if !input.scope_ref_available {
        if input.scope_kind == ScopeKind::CHANNEL {
            return Err(invalid_binding_reason("channel_unavailable"));
        }
        return Err(invalid_binding_reason("integration_account_disconnected"));
    }
    if !input.scope_connector_supported {
        return Err(invalid_binding_reason("connector_binding_unsupported"));
    }

    let profile_id = input.selected_profile_id.trim();
    let workspace_id = input.selected_workspace_id.trim();

    // Integration-account defaults supply profile only; they must not carry a workspace.
    if input.scope_kind == ScopeKind::INTEGRATION_ACCOUNT && !workspace_id.is_empty() {
        return Err(invalid_binding_reason(
            "account_binding_workspace_not_allowed",
        ));
    }
    // A binding must select at least one of profile/workspace to be meaningful.
    if profile_id.is_empty() && workspace_id.is_empty() {
        return Err(invalid_binding_reason("binding_selects_nothing"));
    }
    if !profile_id.is_empty() {
        if over_limit(profile_id, 256)
            || contains_unsafe(profile_id)
            || !identifier_like(profile_id)
        {
            return Err(invalid_binding_reason("selected_profile_malformed"));
        }
        if !input.profile_selectable {
            return Err(invalid_binding_reason("selected_profile_unavailable"));
        }
    }
    if !workspace_id.is_empty() {
        if over_limit(workspace_id, 256)
            || contains_unsafe(workspace_id)
            || !identifier_like(workspace_id)
        {
            return Err(invalid_binding_reason("selected_workspace_malformed"));
        }
        if !input.workspace_selectable {
            return Err(invalid_binding_reason("selected_workspace_unavailable"));
        }
    }
    Ok(())
}

/// The validated shape for setting visibility.
#[derive(Debug, Clone, Default)]
pub struct CapabilityVisibilityMutationInput {
    pub scope_kind: VisibilityScopeKind,
    pub scope_ref: String,
    pub capability_id: String,
    pub visibility: Visibility,
}

/// Enforces that visibility is only user-edited at profile and workspace scope this
/// phase (FR-018) with safe, known values.
pub fn validate_capability_visibility_mutation(
    input: &CapabilityVisibilityMutationInput,
) -> Result<(), BindingError> {
    if input.scope_kind != VisibilityScopeKind::PROFILE
        && input.scope_kind != VisibilityScopeKind::WORKSPACE
    {
        return Err(invalid_binding_reason("visibility_scope_not_editable"));
    }
    let scope_ref = input.scope_ref.trim();
    if scope_ref.is_empty()
        || over_limit(scope_ref, 256)
        || contains_unsafe(scope_ref)
        || !identifier_like(scope_ref)
    {
        return Err(invalid_binding_reason("visibility_scope_ref_malformed"));
    }
    let capability = input.capability_id.trim();
    if capability.is_empty()
        || over_limit(capability, 256)
        || contains_unsafe(capability)
        || !identifier_like(capability)
    {
        return Err(invalid_binding_reason("capability_id_malformed"));
    }
    if input.visibility != Visibility::VISIBLE
        && input.visibility != Visibility::HIDDEN
        && input.visibility != Visibility::DISABLED
        && input.visibility != Visibility::DEFAULT_ENABLED
    {
        return Err(invalid_binding_reason("visibility_value_invalid"));
    }
    Ok(())
}

/// Derives the repair status of a binding from the availability of the resources it
/// references (FR-022). Healthy only when every referenced resource is present and
/// the binding is active.
pub fn repair_status_for_references(
    status: &BindingStatus,
    profile_selectable: bool,
    workspace_selectable: bool,
    scope_ref_available: bool,
    connector_supported: bool,
) -> RepairStatus {
    if *status == BindingStatus::DISABLED {
        return RepairStatus::DISABLED;
    }
    if !connector_supported {
        return RepairStatus::UNSUPPORTED;
    }
    if !scope_ref_available {
        return RepairStatus::STALE;
    }
    if !profile_selectable || !workspace_selectable {
        return RepairStatus::NEEDS_REPAIR;
    }
    RepairStatus::HEALTHY
}

// --- local safe-value helpers (kept crate-local to preserve the bindings boundary) ---

pub(crate) fn over_limit(value: &str, limit: usize) -> bool {
    // Byte length, matching Go's len(strings.TrimSpace(value)).
    value.trim().len() > limit
}

pub(crate) fn identifier_like(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() {
        return false;
    }
    value.chars().all(|r| {
        // Approximates Go's unicode.IsLetter || unicode.IsDigit.
        r.is_alphabetic() || r.is_numeric() || matches!(r, '-' | '_' | '.' | ':' | '/' | '@' | '#')
    })
}

/// Case-insensitive substrings that indicate a label/reason may carry a secret,
/// credential, or raw provider payload and must not be surfaced verbatim (FR-028,
/// SC-014). The list is intentionally conservative-broad: a false positive only
/// forces a generic safe label, while a miss could leak sensitive material.
const UNSAFE_MARKERS: &[&str] = &[
    "secret=",
    "token=",
    "api_key",
    "apikey",
    "password=",
    "passwd=",
    "authorization",
    "bearer ",
    "x-api-key",
    "access_key",
    "private_key",
    "eyj", // JWT header prefix (base64 of {"alg/typ...)
];

pub(crate) fn contains_unsafe(value: &str) -> bool {
    let lower = value.to_lowercase();
    UNSAFE_MARKERS.iter().any(|marker| lower.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_binding_input() -> BindingMutationInput {
        BindingMutationInput {
            scope_kind: ScopeKind::CHANNEL,
            scope_ref: "discord:chan_123".into(),
            selected_profile_id: "prof_1".into(),
            selected_workspace_id: "ws_1".into(),
            cross_tenant: false,
            scope_ref_available: true,
            scope_connector_supported: true,
            profile_selectable: true,
            workspace_selectable: true,
        }
    }

    // Port of TestValidateBindingMutation_OK.
    #[test]
    fn validate_binding_mutation_ok() {
        assert!(validate_binding_mutation(&valid_binding_input()).is_ok());
    }

    // Port of TestValidateBindingMutation_Reasons.
    #[test]
    fn validate_binding_mutation_reasons() {
        let cases: Vec<(&str, Box<dyn Fn(&mut BindingMutationInput)>, &str)> = vec![
            (
                "cross tenant",
                Box::new(|b| b.cross_tenant = true),
                "cross_tenant_reference_denied",
            ),
            (
                "bad scope kind",
                Box::new(|b| b.scope_kind = ScopeKind::new("nope")),
                "binding_scope_kind_invalid",
            ),
            (
                "empty ref",
                Box::new(|b| b.scope_ref.clear()),
                "binding_scope_ref_malformed",
            ),
            (
                "channel unavailable",
                Box::new(|b| b.scope_ref_available = false),
                "channel_unavailable",
            ),
            (
                "unsupported connector",
                Box::new(|b| b.scope_connector_supported = false),
                "connector_binding_unsupported",
            ),
            (
                "profile gone",
                Box::new(|b| b.profile_selectable = false),
                "selected_profile_unavailable",
            ),
            (
                "workspace gone",
                Box::new(|b| b.workspace_selectable = false),
                "selected_workspace_unavailable",
            ),
            (
                "selects nothing",
                Box::new(|b| {
                    b.selected_profile_id.clear();
                    b.selected_workspace_id.clear();
                }),
                "binding_selects_nothing",
            ),
        ];
        for (name, mutate, reason) in cases {
            let mut input = valid_binding_input();
            mutate(&mut input);
            let err = validate_binding_mutation(&input).unwrap_err();
            assert!(
                matches!(
                    err,
                    BindingError::Validation(_) | BindingError::InvalidBinding
                ),
                "{name}: expected validation error, got {err:?}"
            );
            assert_eq!(err.reason_code(), Some(reason), "{name}");
        }
    }

    // Port of TestValidateBindingMutation_AccountWorkspaceRejected.
    #[test]
    fn validate_binding_mutation_account_workspace_rejected() {
        let mut input = valid_binding_input();
        input.scope_kind = ScopeKind::INTEGRATION_ACCOUNT;
        input.scope_ref = "integration_acct_1".into();
        input.selected_workspace_id = "ws_1".into();
        assert_eq!(
            validate_binding_mutation(&input).unwrap_err().reason_code(),
            Some("account_binding_workspace_not_allowed")
        );
    }

    // Port of TestValidateWorkspaceMutation.
    #[test]
    fn validate_workspace_mutation_behavior() {
        assert!(
            validate_workspace_mutation(&WorkspaceMutationInput {
                display_name: "Personal".into(),
                status: WorkspaceStatus::default(),
            })
            .is_ok()
        );
        assert_eq!(
            validate_workspace_mutation(&WorkspaceMutationInput::default())
                .unwrap_err()
                .reason_code(),
            Some("workspace_display_name_required")
        );
        assert_eq!(
            validate_workspace_mutation(&WorkspaceMutationInput {
                display_name: "token=abc".into(),
                status: WorkspaceStatus::default(),
            })
            .unwrap_err()
            .reason_code(),
            Some("unsafe_workspace_content")
        );
    }

    // Port of TestValidateCapabilityVisibilityMutation.
    #[test]
    fn validate_capability_visibility_mutation_behavior() {
        let ok = CapabilityVisibilityMutationInput {
            scope_kind: VisibilityScopeKind::WORKSPACE,
            scope_ref: "ws_1".into(),
            capability_id: "cap.x".into(),
            visibility: Visibility::HIDDEN,
        };
        assert!(validate_capability_visibility_mutation(&ok).is_ok());

        let mut bad = ok.clone();
        bad.scope_kind = VisibilityScopeKind::new("channel");
        assert_eq!(
            validate_capability_visibility_mutation(&bad)
                .unwrap_err()
                .reason_code(),
            Some("visibility_scope_not_editable")
        );

        let mut bad_vis = ok;
        bad_vis.visibility = Visibility::new("nope");
        assert_eq!(
            validate_capability_visibility_mutation(&bad_vis)
                .unwrap_err()
                .reason_code(),
            Some("visibility_value_invalid")
        );
    }

    // Port of TestRepairStatusForReferences.
    #[test]
    fn repair_status_for_references_behavior() {
        assert_eq!(
            repair_status_for_references(&BindingStatus::ACTIVE, true, true, true, true),
            RepairStatus::HEALTHY
        );
        assert_eq!(
            repair_status_for_references(&BindingStatus::DISABLED, true, true, true, true),
            RepairStatus::DISABLED
        );
        assert_eq!(
            repair_status_for_references(&BindingStatus::ACTIVE, true, true, true, false),
            RepairStatus::UNSUPPORTED
        );
        assert_eq!(
            repair_status_for_references(&BindingStatus::ACTIVE, true, true, false, true),
            RepairStatus::STALE
        );
        assert_eq!(
            repair_status_for_references(&BindingStatus::ACTIVE, false, true, true, true),
            RepairStatus::NEEDS_REPAIR
        );
    }
}
