//! Precedence resolution (port of precedence.go): fixed channel → integration-account
//! → tenant-default ordering (FR-006) with fail-closed handling of invalid explicit
//! selections (FR-031).

use crate::types::{
    BindingRule, BindingRuntimeScope, BindingStatus, EffectiveBindingSelection, RepairStatus,
    ResolutionOutcome,
};

/// Carries the candidate bindings and tenant defaults needed to resolve an effective
/// binding selection at work-start. Availability oracles let the resolver fail closed
/// (FR-031) without depending on the store.
#[derive(Default)]
pub struct ResolutionInput {
    /// The active channel-scoped binding for the originating channel, if one exists.
    /// `None` means no channel binding.
    pub channel_binding: Option<BindingRule>,
    /// The active integration-account default binding, if one exists.
    pub account_binding: Option<BindingRule>,

    pub tenant_default_profile_id: String,
    pub tenant_default_profile_version_id: String,
    pub tenant_default_workspace_id: String,

    /// Reports whether a profile id is currently selectable (active, not
    /// archived/disabled/removed). `None` fails closed.
    pub profile_available: Option<Box<dyn Fn(&str) -> bool + Send + Sync>>,
    /// Reports whether a workspace id is currently selectable. `None` fails closed.
    pub workspace_available: Option<Box<dyn Fn(&str) -> bool + Send + Sync>>,
}

impl std::fmt::Debug for ResolutionInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolutionInput")
            .field("channel_binding", &self.channel_binding)
            .field("account_binding", &self.account_binding)
            .field("tenant_default_profile_id", &self.tenant_default_profile_id)
            .field(
                "tenant_default_profile_version_id",
                &self.tenant_default_profile_version_id,
            )
            .field(
                "tenant_default_workspace_id",
                &self.tenant_default_workspace_id,
            )
            .field(
                "profile_available",
                &self.profile_available.as_ref().map(|_| "<fn>"),
            )
            .field(
                "workspace_available",
                &self.workspace_available.as_ref().map(|_| "<fn>"),
            )
            .finish()
    }
}

/// Applies the fixed precedence order — channel binding, then integration-account
/// default, then tenant default (FR-006) — and fails closed when an explicit binding
/// selects an invalid profile or workspace (FR-031). It never silently substitutes a
/// different profile, workspace, or lower-precedence binding.
pub fn resolve_selection(input: &ResolutionInput) -> EffectiveBindingSelection {
    let profile_available = |id: &str| input.profile_available.as_ref().is_some_and(|f| f(id));
    let workspace_available = |id: &str| input.workspace_available.as_ref().is_some_and(|f| f(id));

    let channel = active_binding(input.channel_binding.as_ref());
    let account = active_binding(input.account_binding.as_ref());

    let mut sel = EffectiveBindingSelection::default();

    // --- Determine the profile source by precedence. ---
    let profile_id: String;
    let profile_version_id: String;
    let mut explicit = false;
    if let Some(channel) = channel.filter(|c| !c.selected_profile_id.trim().is_empty()) {
        profile_id = channel.selected_profile_id.trim().to_string();
        profile_version_id = channel.selected_profile_version_id.trim().to_string();
        sel.binding_scope = BindingRuntimeScope::CHANNEL;
        sel.binding_id = channel.binding_id.clone();
        explicit = true;
    } else if let Some(account) = account.filter(|a| !a.selected_profile_id.trim().is_empty()) {
        profile_id = account.selected_profile_id.trim().to_string();
        profile_version_id = account.selected_profile_version_id.trim().to_string();
        sel.binding_scope = BindingRuntimeScope::INTEGRATION_ACCOUNT;
        sel.binding_id = account.binding_id.clone();
        explicit = true;
    } else {
        profile_id = input.tenant_default_profile_id.trim().to_string();
        profile_version_id = input.tenant_default_profile_version_id.trim().to_string();
        if sel.binding_scope.is_empty() {
            sel.binding_scope = BindingRuntimeScope::TENANT_DEFAULT;
        }
    }

    // --- Determine the workspace source. Only channel bindings carry a workspace;
    // integration-account defaults supply profile only (FR-005). ---
    let workspace_id: String;
    if let Some(channel) = channel.filter(|c| !c.selected_workspace_id.trim().is_empty()) {
        workspace_id = channel.selected_workspace_id.trim().to_string();
        // A channel binding that only set a workspace still makes channel the scope.
        if sel.binding_scope == BindingRuntimeScope::TENANT_DEFAULT {
            sel.binding_scope = BindingRuntimeScope::CHANNEL;
            sel.binding_id = channel.binding_id.clone();
        }
        explicit = true;
    } else {
        workspace_id = input.tenant_default_workspace_id.trim().to_string();
    }

    sel.selected_profile_id = profile_id;
    sel.selected_profile_version_id = profile_version_id;
    sel.selected_workspace_id = workspace_id;

    // --- Fail closed on invalid explicit selections (FR-031). ---
    if sel.selected_profile_id.is_empty() || !profile_available(&sel.selected_profile_id) {
        sel.outcome = ResolutionOutcome::REPAIR_REQUIRED;
        sel.repair_status = RepairStatus::INVALID;
        sel.repair_reason = "selected_profile_unavailable".into();
        return sel;
    }
    if sel.selected_workspace_id.is_empty() || !workspace_available(&sel.selected_workspace_id) {
        sel.outcome = ResolutionOutcome::REPAIR_REQUIRED;
        sel.repair_status = RepairStatus::INVALID;
        sel.repair_reason = "selected_workspace_unavailable".into();
        return sel;
    }

    sel.outcome = if explicit {
        ResolutionOutcome::RESOLVED
    } else {
        ResolutionOutcome::DEFAULT
    };
    sel
}

/// Returns the binding only if it is present and active; disabled bindings are treated
/// as absent for resolution.
fn active_binding(binding: Option<&BindingRule>) -> Option<&BindingRule> {
    binding.filter(|b| b.status != BindingStatus::DISABLED)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ScopeKind;

    fn all_available(_: &str) -> bool {
        true
    }
    fn none_available(_: &str) -> bool {
        false
    }

    fn base_input() -> ResolutionInput {
        ResolutionInput {
            tenant_default_profile_id: "prof_default".into(),
            tenant_default_profile_version_id: "profv_default".into(),
            tenant_default_workspace_id: "ws_default".into(),
            profile_available: Some(Box::new(all_available)),
            workspace_available: Some(Box::new(all_available)),
            ..Default::default()
        }
    }

    fn channel_binding(binding_id: &str, profile_id: &str, workspace_id: &str) -> BindingRule {
        BindingRule {
            binding_id: binding_id.into(),
            scope_kind: ScopeKind::CHANNEL,
            status: BindingStatus::ACTIVE,
            selected_profile_id: profile_id.into(),
            selected_workspace_id: workspace_id.into(),
            ..Default::default()
        }
    }

    fn account_binding(binding_id: &str, profile_id: &str) -> BindingRule {
        BindingRule {
            binding_id: binding_id.into(),
            scope_kind: ScopeKind::INTEGRATION_ACCOUNT,
            status: BindingStatus::ACTIVE,
            selected_profile_id: profile_id.into(),
            ..Default::default()
        }
    }

    // Port of TestResolveSelection_NoBinding_UsesTenantDefault.
    #[test]
    fn no_binding_uses_tenant_default() {
        let sel = resolve_selection(&base_input());
        assert_eq!(sel.outcome, ResolutionOutcome::DEFAULT);
        assert_eq!(sel.binding_scope, BindingRuntimeScope::TENANT_DEFAULT);
        assert_eq!(sel.selected_profile_id, "prof_default");
        assert_eq!(sel.selected_workspace_id, "ws_default");
    }

    // Port of TestResolveSelection_ChannelBindingWins.
    #[test]
    fn channel_binding_wins() {
        let input = ResolutionInput {
            channel_binding: Some(channel_binding("bnd_chan", "prof_chan", "ws_chan")),
            account_binding: Some(account_binding("bnd_acct", "prof_acct")),
            ..base_input()
        };
        let sel = resolve_selection(&input);
        assert_eq!(sel.outcome, ResolutionOutcome::RESOLVED);
        assert_eq!(sel.binding_scope, BindingRuntimeScope::CHANNEL);
        assert_eq!(sel.binding_id, "bnd_chan");
        assert_eq!(sel.selected_profile_id, "prof_chan");
        assert_eq!(sel.selected_workspace_id, "ws_chan");
    }

    // Port of TestResolveSelection_AccountDefaultWhenNoChannel.
    #[test]
    fn account_default_when_no_channel() {
        let input = ResolutionInput {
            account_binding: Some(account_binding("bnd_acct", "prof_acct")),
            ..base_input()
        };
        let sel = resolve_selection(&input);
        assert_eq!(sel.outcome, ResolutionOutcome::RESOLVED);
        assert_eq!(sel.binding_scope, BindingRuntimeScope::INTEGRATION_ACCOUNT);
        assert_eq!(sel.selected_profile_id, "prof_acct");
        // Account binding supplies no workspace; tenant default workspace applies.
        assert_eq!(sel.selected_workspace_id, "ws_default");
    }

    // Port of TestResolveSelection_ChannelStableWhenAccountChanges (B4).
    #[test]
    fn channel_stable_when_account_changes() {
        let input = ResolutionInput {
            channel_binding: Some(channel_binding("bnd_chan", "prof_chan", "")),
            account_binding: Some(account_binding("bnd_acct_v2", "prof_acct_changed")),
            ..base_input()
        };
        let sel = resolve_selection(&input);
        assert_eq!(
            sel.selected_profile_id, "prof_chan",
            "channel binding must be stable"
        );
    }

    // Port of TestResolveSelection_InvalidProfileFailsClosed (B5/FR-031).
    #[test]
    fn invalid_profile_fails_closed() {
        let input = ResolutionInput {
            channel_binding: Some(channel_binding("bnd_chan", "prof_archived", "")),
            profile_available: Some(Box::new(|id: &str| id != "prof_archived")),
            ..base_input()
        };
        let sel = resolve_selection(&input);
        assert_eq!(sel.outcome, ResolutionOutcome::REPAIR_REQUIRED);
        assert_eq!(sel.repair_reason, "selected_profile_unavailable");
        // Must NOT have substituted the tenant default.
        assert_ne!(sel.selected_profile_id, "prof_default");
    }

    // Port of TestResolveSelection_InvalidWorkspaceFailsClosed.
    #[test]
    fn invalid_workspace_fails_closed() {
        let input = ResolutionInput {
            channel_binding: Some(channel_binding("bnd_chan", "prof_chan", "ws_gone")),
            workspace_available: Some(Box::new(|id: &str| id != "ws_gone")),
            ..base_input()
        };
        let sel = resolve_selection(&input);
        assert_eq!(sel.outcome, ResolutionOutcome::REPAIR_REQUIRED);
        assert_eq!(sel.repair_reason, "selected_workspace_unavailable");
    }

    // Port of TestResolveSelection_DisabledBindingIgnored.
    #[test]
    fn disabled_binding_ignored() {
        let mut binding = channel_binding("bnd_chan", "prof_chan", "");
        binding.status = BindingStatus::DISABLED;
        let input = ResolutionInput {
            channel_binding: Some(binding),
            ..base_input()
        };
        let sel = resolve_selection(&input);
        assert_eq!(sel.outcome, ResolutionOutcome::DEFAULT);
        assert_eq!(sel.selected_profile_id, "prof_default");
    }

    // Port of TestResolveSelection_NilOraclesFailClosed.
    #[test]
    fn missing_oracles_fail_closed() {
        let sel = resolve_selection(&ResolutionInput {
            tenant_default_profile_id: "p".into(),
            tenant_default_workspace_id: "w".into(),
            profile_available: Some(Box::new(none_available)),
            workspace_available: Some(Box::new(none_available)),
            ..Default::default()
        });
        assert_eq!(sel.outcome, ResolutionOutcome::REPAIR_REQUIRED);

        // Truly absent oracles must also fail closed.
        let sel = resolve_selection(&ResolutionInput {
            tenant_default_profile_id: "p".into(),
            tenant_default_workspace_id: "w".into(),
            ..Default::default()
        });
        assert_eq!(sel.outcome, ResolutionOutcome::REPAIR_REQUIRED);
    }

    // Port of TestResolvedSelectionExposesIdentityNotAccess (FR-020): a resolved
    // selection exposes workspace identity but never a filesystem path.
    #[test]
    fn resolved_selection_exposes_identity_not_access() {
        let sel = resolve_selection(&ResolutionInput {
            tenant_default_profile_id: "prof_1".into(),
            tenant_default_workspace_id: "ws_1".into(),
            profile_available: Some(Box::new(all_available)),
            workspace_available: Some(Box::new(all_available)),
            ..Default::default()
        });
        assert_eq!(sel.selected_workspace_id, "ws_1");
        assert!(
            !sel.selected_workspace_id.contains('/'),
            "workspace identity must not look like a filesystem path"
        );
    }
}
