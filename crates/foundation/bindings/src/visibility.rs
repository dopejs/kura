//! Capability visibility resolution (port of visibility.go): strictest-wins
//! combination of tenant/connector limits and profile/workspace policy (FR-016,
//! FR-017, FR-019).

use crate::policy::BindingError;
use crate::types::{CapabilityDecision, EffectiveVisibility, Visibility};

/// Returns `Err(BindingError::CapabilityNotExecutable)` when the decision forbids
/// execution (FR-016).
pub fn enforce_executable(decision: &CapabilityDecision) -> Result<(), BindingError> {
    if decision.executable {
        return Ok(());
    }
    Err(BindingError::CapabilityNotExecutable)
}

/// One scope's contribution to the effective decision.
#[derive(Debug, Clone, Default)]
pub struct ScopeVisibility {
    /// Safe scope label, e.g. "tenant", "connector", "profile", "workspace".
    pub scope: String,
    /// The policy value at this scope; empty means "no opinion".
    pub visibility: Visibility,
    /// Marks a hard higher-level prohibition (tenant/connector limit).
    pub blocked: bool,
}

/// Resolves one capability's effective visibility from the strictest of the
/// higher-level tenant/connector limits and the profile/workspace policies.
#[derive(Debug, Clone, Default)]
pub struct VisibilityInput {
    pub capability_id: String,
    /// Higher-level tenant/connector constraints (enforced, not user-edited in this
    /// phase). A `blocked` limit wins over everything (FR-017).
    pub limits: Vec<ScopeVisibility>,
    /// The user-editable scopes (FR-018). Empty visibility means the scope set no
    /// policy for this capability.
    pub profile_policy: Visibility,
    pub workspace_policy: Visibility,
}

/// Orders visibility values; higher is stricter and wins.
fn strictness_rank(v: &Visibility) -> u8 {
    if *v == Visibility::DISABLED {
        3
    } else if *v == Visibility::HIDDEN {
        2
    } else if *v == Visibility::VISIBLE || *v == Visibility::DEFAULT_ENABLED {
        1
    } else {
        0
    }
}

/// Computes the effective decision for a single capability. Strictest wins across
/// tenant/connector limits and profile/workspace policy. `default_enabled` never
/// overrides a hidden/disabled/blocked outcome (FR-019).
pub fn resolve_capability_visibility(input: &VisibilityInput) -> CapabilityDecision {
    let mut decision = CapabilityDecision {
        capability_id: input.capability_id.trim().to_string(),
        ..Default::default()
    };

    // Higher-level hard prohibition wins unconditionally.
    for limit in &input.limits {
        if limit.blocked {
            decision.effective = EffectiveVisibility::BLOCKED;
            decision.offered = false;
            decision.executable = false;
            decision.reason = "blocked_by_higher_policy".into();
            decision.scope = safe_scope_label(&limit.scope);
            return decision;
        }
    }

    // Collect every scope's opinion with a stable scope label for the winning reason.
    let mut opinions: Vec<(&str, &Visibility)> = Vec::with_capacity(input.limits.len() + 2);
    for limit in &input.limits {
        if !limit.visibility.is_empty() {
            opinions.push((limit.scope.trim(), &limit.visibility));
        }
    }
    if !input.profile_policy.is_empty() {
        opinions.push(("profile", &input.profile_policy));
    }
    if !input.workspace_policy.is_empty() {
        opinions.push(("workspace", &input.workspace_policy));
    }

    // Default when no scope expresses an opinion: capability is visible but not
    // offered-by-default (it is allowable, not promoted).
    if opinions.is_empty() {
        decision.effective = EffectiveVisibility::VISIBLE;
        decision.offered = true;
        decision.executable = true;
        decision.reason = "no_policy_default_visible".into();
        decision.scope = "default".into();
        return decision;
    }

    // Strictest wins; record which scope produced it.
    let mut winner = opinions[0];
    let mut saw_default_enabled = false;
    for opinion in &opinions {
        if *opinion.1 == Visibility::DEFAULT_ENABLED {
            saw_default_enabled = true;
        }
        if strictness_rank(opinion.1) > strictness_rank(winner.1) {
            winner = *opinion;
        }
    }

    if *winner.1 == Visibility::DISABLED {
        decision.effective = EffectiveVisibility::DISABLED;
        decision.offered = false;
        decision.executable = false;
        decision.reason = "disabled_by_policy".into();
    } else if *winner.1 == Visibility::HIDDEN {
        decision.effective = EffectiveVisibility::HIDDEN;
        decision.offered = false;
        decision.executable = false;
        decision.reason = "hidden_by_policy".into();
    } else {
        // visible or default_enabled
        decision.effective = EffectiveVisibility::VISIBLE;
        decision.offered = true;
        decision.executable = true;
        // default_enabled only takes effect when nothing stricter applies (FR-019).
        decision.default_enabled = saw_default_enabled;
        decision.reason = if saw_default_enabled {
            "default_enabled".into()
        } else {
            "visible_by_policy".into()
        };
    }
    decision.scope = safe_scope_label(winner.0);
    decision
}

/// Resolves a batch of capabilities, preserving input order.
pub fn resolve_visibility_set(inputs: &[VisibilityInput]) -> Vec<CapabilityDecision> {
    inputs.iter().map(resolve_capability_visibility).collect()
}

pub(crate) fn safe_scope_label(scope: &str) -> String {
    let scope = scope.trim();
    if scope.is_empty() {
        return "unknown".to_string();
    }
    scope.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Port of TestVisibility_NoPolicyDefaultsVisible.
    #[test]
    fn no_policy_defaults_visible() {
        let d = resolve_capability_visibility(&VisibilityInput {
            capability_id: "cap.read".into(),
            ..Default::default()
        });
        assert_eq!(d.effective, EffectiveVisibility::VISIBLE);
        assert!(d.offered && d.executable);
        assert!(
            !d.default_enabled,
            "no policy should not be default-enabled"
        );
    }

    // Port of TestVisibility_HiddenWinsOverVisible.
    #[test]
    fn hidden_wins_over_visible() {
        let d = resolve_capability_visibility(&VisibilityInput {
            capability_id: "cap.x".into(),
            profile_policy: Visibility::VISIBLE,
            workspace_policy: Visibility::HIDDEN,
            ..Default::default()
        });
        assert_eq!(d.effective, EffectiveVisibility::HIDDEN);
        assert!(!d.offered && !d.executable);
        assert_eq!(d.scope, "workspace");
    }

    // Port of TestVisibility_DisabledWinsOverHidden.
    #[test]
    fn disabled_wins_over_hidden() {
        let d = resolve_capability_visibility(&VisibilityInput {
            capability_id: "cap.x".into(),
            profile_policy: Visibility::DISABLED,
            workspace_policy: Visibility::HIDDEN,
            ..Default::default()
        });
        assert_eq!(d.effective, EffectiveVisibility::DISABLED);
        assert!(!d.executable);
    }

    // Port of TestVisibility_BlockedLimitWins (FR-017).
    #[test]
    fn blocked_limit_wins() {
        let d = resolve_capability_visibility(&VisibilityInput {
            capability_id: "cap.x".into(),
            limits: vec![ScopeVisibility {
                scope: "connector".into(),
                blocked: true,
                ..Default::default()
            }],
            profile_policy: Visibility::DEFAULT_ENABLED,
            workspace_policy: Visibility::VISIBLE,
        });
        assert_eq!(d.effective, EffectiveVisibility::BLOCKED);
        assert!(!d.offered && !d.executable);
        assert_eq!(d.scope, "connector");
    }

    // Port of TestVisibility_DefaultEnabledDoesNotOverrideHidden (FR-019).
    #[test]
    fn default_enabled_does_not_override_hidden() {
        let d = resolve_capability_visibility(&VisibilityInput {
            capability_id: "cap.x".into(),
            profile_policy: Visibility::DEFAULT_ENABLED,
            workspace_policy: Visibility::HIDDEN,
            ..Default::default()
        });
        assert_eq!(d.effective, EffectiveVisibility::HIDDEN);
        assert!(!d.default_enabled);
    }

    // Port of TestVisibility_DefaultEnabledWhenAllowed.
    #[test]
    fn default_enabled_when_allowed() {
        let d = resolve_capability_visibility(&VisibilityInput {
            capability_id: "cap.x".into(),
            profile_policy: Visibility::DEFAULT_ENABLED,
            ..Default::default()
        });
        assert_eq!(d.effective, EffectiveVisibility::VISIBLE);
        assert!(d.default_enabled && d.offered);
    }

    // Port of TestVisibility_TenantLimitHiddenWins.
    #[test]
    fn tenant_limit_hidden_wins() {
        let d = resolve_capability_visibility(&VisibilityInput {
            capability_id: "cap.x".into(),
            limits: vec![ScopeVisibility {
                scope: "tenant".into(),
                visibility: Visibility::HIDDEN,
                ..Default::default()
            }],
            profile_policy: Visibility::VISIBLE,
            ..Default::default()
        });
        assert_eq!(d.effective, EffectiveVisibility::HIDDEN);
        assert_eq!(d.scope, "tenant");
    }

    // Port of TestResolveVisibilitySet_PreservesOrder.
    #[test]
    fn resolve_visibility_set_preserves_order() {
        let out = resolve_visibility_set(&[
            VisibilityInput {
                capability_id: "a".into(),
                ..Default::default()
            },
            VisibilityInput {
                capability_id: "b".into(),
                profile_policy: Visibility::HIDDEN,
                ..Default::default()
            },
        ]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].capability_id, "a");
        assert_eq!(out[1].capability_id, "b");
        assert_eq!(out[1].effective, EffectiveVisibility::HIDDEN);
    }

    // Port of TestEnforceExecutable (B9/FR-016): a hidden or disabled capability
    // decision blocks execution.
    #[test]
    fn enforce_executable_blocks_non_executable() {
        let visible = CapabilityDecision {
            effective: EffectiveVisibility::VISIBLE,
            executable: true,
            ..Default::default()
        };
        assert!(enforce_executable(&visible).is_ok());
        for effective in [
            EffectiveVisibility::HIDDEN,
            EffectiveVisibility::DISABLED,
            EffectiveVisibility::BLOCKED,
        ] {
            let decision = CapabilityDecision {
                effective,
                ..Default::default()
            };
            assert!(
                matches!(
                    enforce_executable(&decision),
                    Err(BindingError::CapabilityNotExecutable)
                ),
                "effective {} must not be executable",
                decision.effective
            );
        }
    }

    // Port of TestAutonomousSelectionBoundedToVisible (B62/FR-021/SC-015): the
    // resolver never reports an executable decision for a hidden/disabled capability
    // regardless of default_enabled.
    #[test]
    fn autonomous_selection_bounded_to_visible() {
        let d = resolve_capability_visibility(&VisibilityInput {
            capability_id: "tool.fs".into(),
            profile_policy: Visibility::DEFAULT_ENABLED,
            workspace_policy: Visibility::DISABLED,
            ..Default::default()
        });
        assert!(
            !d.offered && !d.executable,
            "disabled capability must not be selectable"
        );
        assert!(enforce_executable(&d).is_err());
    }
}
