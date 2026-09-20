//! Behavioral tests for the binding-rule + capability-visibility DAOs
//! (rs/store/src/bindings.rs), ported from daemon/internal/store/binding_store_test.go
//! and binding_projection_test.go: create/update/repair/remove round-trips,
//! tenant isolation, capability visibility upsert/list, and runtime binding
//! evidence ordering.

use chrono::{Duration, Utc};
use kura_bindings::{
    BindingStatus, CreateBindingRequest, RepairStatus, RuntimeBindingEvidence,
    SetVisibilityRequest, UpdateBindingRequest, Visibility, VisibilityScopeKind, WorkspaceStatus,
};
use kura_identity::TenantContext;
use kura_store::SQLiteStore;

fn temp_dir(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("kura_store_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.to_string_lossy().to_string()
}

fn actor(tenant_id: &str, principal_id: &str) -> TenantContext {
    TenantContext {
        tenant_id: tenant_id.to_string(),
        principal_id: principal_id.to_string(),
        ..TenantContext::default()
    }
}

/// Seeds an active profile + workspace so channel bindings pass selection gates.
fn seed_references(store: &SQLiteStore, tenant_id: &str) -> (String, String) {
    let profile = store
        .create_agent_profile(
            &actor(tenant_id, "prn_1"),
            &kura_profiles::MutationInput {
                display_name: "Binding Profile".to_string(),
                activate: true,
                reason_code: "test".to_string(),
                ..kura_profiles::MutationInput::default()
            },
        )
        .unwrap();
    let workspace = store
        .create_workspace(&actor(tenant_id, "prn_1"), "Binding Workspace")
        .unwrap();
    (profile.profile.profile_id, workspace.0.workspace_id)
}

fn channel_req(
    _tenant_id: &str,
    scope_ref: &str,
    profile_id: &str,
    workspace_id: &str,
) -> CreateBindingRequest {
    CreateBindingRequest {
        scope_kind: kura_bindings::ScopeKind::CHANNEL,
        scope_ref: scope_ref.to_string(),
        selected_profile_id: profile_id.to_string(),
        selected_workspace_id: workspace_id.to_string(),
        reason_code: "user_created_binding".to_string(),
    }
}

#[test]
fn binding_rule_create_update_repair_remove_round_trip() {
    let dir = temp_dir("bindings_lifecycle");
    let store = SQLiteStore::new(&dir).unwrap();
    let (profile_id, workspace_id) = seed_references(&store, "ten_1");

    let (rule, audit_id) = store
        .create_binding_rule(
            &actor("ten_1", "prn_1"),
            &channel_req("ten_1", "discord:chan_1", &profile_id, &workspace_id),
        )
        .unwrap();
    assert_eq!(rule.status, BindingStatus::ACTIVE);
    assert_eq!(rule.scope_kind, kura_bindings::ScopeKind::CHANNEL);
    assert_eq!(rule.scope_ref, "discord:chan_1");
    assert_eq!(rule.selected_profile_id, profile_id);
    assert_eq!(rule.resulting_selection_summary, "profile+workspace");
    assert_eq!(rule.repair_status, RepairStatus::HEALTHY);
    assert!(!audit_id.is_empty());

    // Duplicate active scope → rejected.
    let err = store
        .create_binding_rule(
            &actor("ten_1", "prn_1"),
            &channel_req("ten_1", "discord:chan_1", &profile_id, &workspace_id),
        )
        .unwrap_err();
    assert!(err.contains("active_binding_already_exists"), "{err}");

    // Get + list carry fresh repair status.
    let got = store
        .get_binding_rule("ten_1", &rule.binding_id)
        .unwrap()
        .expect("present");
    assert_eq!(got.binding_id, rule.binding_id);
    let listed = store.list_binding_rules("ten_1", 50).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].binding_id, rule.binding_id);

    // Cross-tenant isolation.
    assert!(
        store
            .get_binding_rule("ten_2", &rule.binding_id)
            .unwrap()
            .is_none()
    );
    assert!(store.list_binding_rules("ten_2", 50).unwrap().is_empty());

    // Update selection (profile only) recomputes the summary.
    let (updated, _) = store
        .update_binding_rule(
            &actor("ten_1", "prn_1"),
            &rule.binding_id,
            &UpdateBindingRequest {
                selected_profile_id: profile_id.clone(),
                reason_code: "user_updated_binding".to_string(),
                ..UpdateBindingRequest::default()
            },
        )
        .unwrap();
    assert_eq!(updated.resulting_selection_summary, "profile+workspace");
    assert_eq!(updated.previous_selection_summary, "profile+workspace");

    // Disable via update.
    let (disabled, _) = store
        .update_binding_rule(
            &actor("ten_1", "prn_1"),
            &rule.binding_id,
            &UpdateBindingRequest {
                disable: true,
                reason_code: "user_disabled_binding".to_string(),
                ..UpdateBindingRequest::default()
            },
        )
        .unwrap();
    assert_eq!(disabled.status, BindingStatus::DISABLED);
    assert_eq!(disabled.repair_status, RepairStatus::DISABLED);
    assert!(disabled.disabled_at.is_some());

    // Repair evaluates references: profile still selectable → needs_repair
    // (the workspace is still selectable, so status health depends on state).
    let (repaired, _) = store
        .repair_binding_rule(&actor("ten_1", "prn_1"), &rule.binding_id)
        .unwrap();
    assert_eq!(
        repaired.repair_status,
        RepairStatus::DISABLED,
        "disabled bindings stay disabled"
    );

    // Remove deletes the rule while returning an audit id.
    let remove_audit = store
        .remove_binding_rule(&actor("ten_1", "prn_1"), &rule.binding_id)
        .unwrap();
    assert!(!remove_audit.is_empty());
    assert!(
        store
            .get_binding_rule("ten_1", &rule.binding_id)
            .unwrap()
            .is_none()
    );
}

#[test]
fn binding_repair_tracks_reference_health() {
    let dir = temp_dir("bindings_repair");
    let store = SQLiteStore::new(&dir).unwrap();
    let (profile_id, workspace_id) = seed_references(&store, "ten_1");

    let (rule, _) = store
        .create_binding_rule(
            &actor("ten_1", "prn_1"),
            &channel_req("ten_1", "discord:chan_1", &profile_id, &workspace_id),
        )
        .unwrap();

    // Retire the profile → repair status becomes needs_repair.
    store
        .retire_agent_profile(
            &actor("ten_1", "prn_1"),
            &profile_id,
            kura_profiles::Status::ARCHIVED,
            &kura_profiles::RetirementInput::default(),
        )
        .unwrap();
    let (repaired, _) = store
        .repair_binding_rule(&actor("ten_1", "prn_1"), &rule.binding_id)
        .unwrap();
    assert_eq!(repaired.repair_status, RepairStatus::NEEDS_REPAIR);

    // Disable the workspace → still needs_repair (profile already gone).
    store
        .update_workspace_status(
            &actor("ten_1", "prn_1"),
            &workspace_id,
            WorkspaceStatus::DISABLED,
        )
        .unwrap();
    let (repaired2, _) = store
        .repair_binding_rule(&actor("ten_1", "prn_1"), &rule.binding_id)
        .unwrap();
    assert_eq!(repaired2.repair_status, RepairStatus::NEEDS_REPAIR);
}

#[test]
fn resolve_channel_binding_returns_active_rule_only() {
    let dir = temp_dir("bindings_resolve");
    let store = SQLiteStore::new(&dir).unwrap();
    let (profile_id, workspace_id) = seed_references(&store, "ten_1");

    let (rule, _) = store
        .create_binding_rule(
            &actor("ten_1", "prn_1"),
            &channel_req("ten_1", "discord:chan_9", &profile_id, &workspace_id),
        )
        .unwrap();
    let resolved = store
        .resolve_channel_binding("ten_1", "discord:chan_9")
        .unwrap()
        .expect("resolved");
    assert_eq!(resolved.binding_id, rule.binding_id);
    assert!(
        store
            .resolve_channel_binding("ten_1", "discord:other")
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .resolve_channel_binding("ten_2", "discord:chan_9")
            .unwrap()
            .is_none()
    );
}

#[test]
fn capability_visibility_upsert_and_list() {
    let dir = temp_dir("bindings_visibility");
    let store = SQLiteStore::new(&dir).unwrap();

    let (policy, audit_id) = store
        .set_capability_visibility(
            &actor("ten_1", "prn_1"),
            &SetVisibilityRequest {
                scope_kind: VisibilityScopeKind::PROFILE,
                scope_ref: "prof_1".to_string(),
                capability_id: "cap.terminal".to_string(),
                visibility: Visibility::HIDDEN,
                reason_code: "user_set_capability_visibility".to_string(),
            },
        )
        .unwrap();
    assert_eq!(policy.capability_id, "cap.terminal");
    assert_eq!(policy.visibility, Visibility::HIDDEN);
    assert!(!audit_id.is_empty());

    // Upsert preserves the policy id and updates visibility.
    let (updated, _) = store
        .set_capability_visibility(
            &actor("ten_1", "prn_1"),
            &SetVisibilityRequest {
                scope_kind: VisibilityScopeKind::PROFILE,
                scope_ref: "prof_1".to_string(),
                capability_id: "cap.terminal".to_string(),
                visibility: Visibility::DISABLED,
                reason_code: "user_set_capability_visibility".to_string(),
            },
        )
        .unwrap();
    assert_eq!(updated.policy_id, policy.policy_id, "stable id on upsert");
    assert_eq!(updated.visibility, Visibility::DISABLED);

    // List orders by capability id; scoped to scope/tenant.
    let listed = store
        .list_capability_visibility("ten_1", &VisibilityScopeKind::PROFILE, "prof_1")
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].policy_id, policy.policy_id);
    assert!(
        store
            .list_capability_visibility("ten_1", &VisibilityScopeKind::WORKSPACE, "prof_1")
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .list_capability_visibility("ten_2", &VisibilityScopeKind::PROFILE, "prof_1")
            .unwrap()
            .is_empty()
    );

    // Invalid visibility value is rejected with a reason code.
    let err = store
        .set_capability_visibility(
            &actor("ten_1", "prn_1"),
            &SetVisibilityRequest {
                scope_kind: VisibilityScopeKind::PROFILE,
                scope_ref: "prof_1".to_string(),
                capability_id: "cap.x".to_string(),
                visibility: Visibility::new("nope"),
                reason_code: String::new(),
            },
        )
        .unwrap_err();
    assert!(err.contains("visibility_value_invalid"), "{err}");
}

#[test]
fn runtime_binding_evidence_lists_newest_first_and_latest() {
    let dir = temp_dir("bindings_evidence");
    let store = SQLiteStore::new(&dir).unwrap();
    let now = Utc::now();

    let mut older = RuntimeBindingEvidence::default();
    older.tenant_id = "ten_1".to_string();
    older.resource_kind = "run".to_string();
    older.resource_id = "run_1".to_string();
    older.binding_scope = kura_bindings::BindingRuntimeScope::TENANT_DEFAULT;
    older.classification = kura_bindings::Classification::DEFAULT;
    older.selection_reason = "tenant_default".to_string();
    older.occurred_at = now;

    let mut newer = older.clone();
    newer.selected_profile_id = "prof_1".to_string();
    newer.classification = kura_bindings::Classification::APPLIED;
    newer.occurred_at = now + Duration::minutes(5);

    let stored_older = store
        .record_runtime_binding_evidence(older.clone())
        .unwrap();
    let stored_newer = store
        .record_runtime_binding_evidence(newer.clone())
        .unwrap();
    assert!(!stored_older.projection_id.is_empty());
    assert!(!stored_newer.projection_id.is_empty());
    assert_eq!(
        stored_newer.redaction_status,
        kura_bindings::RedactionStatus::REDACTED
    );

    let listed = store
        .list_runtime_binding_evidence("ten_1", "run", "run_1", 20)
        .unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(
        listed[0].projection_id, stored_newer.projection_id,
        "newest first"
    );
    assert_eq!(listed[1].projection_id, stored_older.projection_id);

    let latest = store
        .latest_runtime_binding_evidence("ten_1", "run", "run_1")
        .unwrap()
        .expect("latest");
    assert_eq!(latest.projection_id, stored_newer.projection_id);
    assert!(
        store
            .latest_runtime_binding_evidence("ten_1", "run", "run_other")
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .latest_runtime_binding_evidence("ten_2", "run", "run_1")
            .unwrap()
            .is_none()
    );

    // Limit clamps to 20 max.
    for i in 0..25 {
        let mut ev = older.clone();
        ev.resource_id = "run_many".to_string();
        ev.occurred_at = now + Duration::minutes(i);
        store.record_runtime_binding_evidence(ev).unwrap();
    }
    assert_eq!(
        store
            .list_runtime_binding_evidence("ten_1", "run", "run_many", 0)
            .unwrap()
            .len(),
        20
    );
}
