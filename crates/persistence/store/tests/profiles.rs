//! Behavioral tests for the agent-profile DAOs (rs/store/src/profiles.rs),
//! ported from daemon/internal/store/profile_store_test.go and
//! profile_projection_test.go: create/detail/update/activate/rollback/retire
//! round-trips, version listing, tenant isolation, list ordering, and the
//! tenant-default active-selection + runtime projection paths.

use chrono::{Duration, Utc};
use kura_identity::TenantContext;
use kura_profiles::{
    ActivationInput, AgentProfile, MutationInput, MutationResult, ProfileDetail, ProfileVersion,
    RetirementInput, RollbackInput, RuntimeProjection, Status,
};
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

fn input(display_name: &str) -> MutationInput {
    MutationInput {
        display_name: display_name.to_string(),
        activate: true,
        reason_code: "test".to_string(),
        overlay_references: kura_profiles::default_legacy_overlay_reference_inputs(),
        ..MutationInput::default()
    }
}

fn assert_created(result: &MutationResult, tenant_id: &str, name: &str) {
    assert_eq!(result.profile.tenant_id, tenant_id);
    assert_eq!(result.profile.display_name, name);
    assert_eq!(result.profile.status, Status::ACTIVE);
    assert_eq!(result.version.version_number, 1);
    assert_eq!(
        result.version.change_kind,
        kura_profiles::ChangeKind::CREATED
    );
    assert!(!result.audit_event_id.is_empty());
    assert!(!result.profile.profile_id.is_empty());
}

#[test]
fn profile_create_detail_update_round_trip() {
    let dir = temp_dir("profiles_roundtrip");
    let store = SQLiteStore::new(&dir).unwrap();

    let created = store
        .create_agent_profile(&actor("ten_1", "prn_1"), &input("Ops Agent"))
        .unwrap();
    assert_created(&created, "ten_1", "Ops Agent");
    let profile_id = created.profile.profile_id.clone();

    let detail: ProfileDetail = store
        .get_agent_profile_detail("ten_1", &profile_id)
        .unwrap()
        .expect("detail present");
    assert_eq!(detail.profile.display_name, "Ops Agent");
    assert_eq!(detail.profile.status, Status::ACTIVE);
    assert!(
        store
            .is_tenant_default_profile("ten_1", &profile_id)
            .unwrap(),
        "activated profile is the tenant default"
    );
    // The overlay count is computed for list views, matching Go.
    assert_eq!(
        store.list_agent_profiles("ten_1", 50).unwrap().items[0].overlay_reference_count,
        2
    );
    assert_eq!(detail.versions.len(), 1);
    assert_eq!(detail.audit_events.len(), 1);
    assert_eq!(detail.audit_events[0].event_kind, "profile.created");
    assert_eq!(detail.audit_events[0].reason_code, "test");
    assert_eq!(detail.overlay_references.len(), 2);

    // Missing profile → None, cross-tenant isolation.
    assert!(
        store
            .get_agent_profile_detail("ten_1", "missing")
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .get_agent_profile_detail("ten_2", &profile_id)
            .unwrap()
            .is_none()
    );

    // Update creates version 2 and keeps the id stable.
    let mut update_input = input("Ops Agent v2");
    update_input.reason_code = "user_updated_profile".to_string();
    let updated = store
        .update_agent_profile(&actor("ten_1", "prn_1"), &profile_id, &update_input)
        .unwrap();
    assert_eq!(updated.profile.profile_id, profile_id);
    assert_eq!(updated.profile.display_name, "Ops Agent v2");
    assert_eq!(updated.version.version_number, 2);
    assert_eq!(
        updated.version.source_version_id,
        created.version.profile_version_id
    );
    assert_eq!(
        updated.version.change_kind,
        kura_profiles::ChangeKind::UPDATED
    );

    let versions = store
        .list_agent_profile_versions("ten_1", &profile_id, 10)
        .unwrap();
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[0].version_number, 2, "newest version first");
    assert_eq!(versions[1].version_number, 1);
    // Cross-tenant version isolation.
    assert!(
        store
            .list_agent_profile_versions("ten_2", &profile_id, 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn profile_list_orders_by_updated_at_and_is_tenant_scoped() {
    let dir = temp_dir("profiles_list");
    let store = SQLiteStore::new(&dir).unwrap();

    let a = store
        .create_agent_profile(&actor("ten_1", "prn_1"), &input("Alpha"))
        .unwrap();
    let b = store
        .create_agent_profile(&actor("ten_1", "prn_1"), &input("Beta"))
        .unwrap();

    // Update Alpha afterwards so it becomes the newest → first.
    let mut update_input = input("Alpha touched");
    update_input.reason_code = "touched".to_string();
    store
        .update_agent_profile(
            &actor("ten_1", "prn_1"),
            &a.profile.profile_id,
            &update_input,
        )
        .unwrap();

    let listed = store.list_agent_profiles("ten_1", 50).unwrap();
    assert_eq!(listed.tenant_id, "ten_1");
    assert_eq!(listed.page.order, "updated_at_desc");
    assert_eq!(listed.items.len(), 2);
    assert_eq!(
        listed.items[0].profile_id, a.profile.profile_id,
        "most recently updated first"
    );
    assert_eq!(listed.items[1].profile_id, b.profile.profile_id);
    assert!(listed.items[0].updated_at >= listed.items[1].updated_at);

    // Tenant isolation: the second tenant sees nothing.
    let other = store.list_agent_profiles("ten_2", 50).unwrap();
    assert!(other.items.is_empty());

    // Limit normalization clamps out-of-range values.
    assert_eq!(
        store.list_agent_profiles("ten_1", 0).unwrap().items.len(),
        2
    );
    assert_eq!(
        store.list_agent_profiles("ten_1", 500).unwrap().items.len(),
        2
    );
    let _ = (a, b);
}

#[test]
fn profile_activate_rollback_and_retire() {
    let dir = temp_dir("profiles_lifecycle");
    let store = SQLiteStore::new(&dir).unwrap();

    // Draft profile is not the default; activation makes it so.
    let mut draft_input = input("Support");
    draft_input.activate = false;
    let draft = store
        .create_agent_profile(&actor("ten_1", "prn_1"), &draft_input)
        .unwrap();
    assert_eq!(draft.profile.status, Status::DRAFT);
    let profile_id = draft.profile.profile_id.clone();

    // Activating an arbitrary version.
    let selection = store
        .activate_agent_profile(
            &actor("ten_1", "prn_1"),
            &profile_id,
            &ActivationInput {
                profile_version_id: draft.version.profile_version_id.clone(),
                reason_code: "user_selected_default".to_string(),
            },
        )
        .unwrap();
    assert_eq!(selection.selection_scope, "tenant_default");
    assert_eq!(selection.profile_id, profile_id);
    let detail = store
        .get_agent_profile_detail("ten_1", &profile_id)
        .unwrap()
        .unwrap();
    assert_eq!(detail.profile.status, Status::ACTIVE);
    assert!(
        store
            .is_tenant_default_profile("ten_1", &profile_id)
            .unwrap()
    );

    // Roll back to version 1 (which is the current snapshot) → new version 2.
    let rolled = store
        .rollback_agent_profile(
            &actor("ten_1", "prn_1"),
            &profile_id,
            &RollbackInput {
                source_profile_version_id: draft.version.profile_version_id.clone(),
                reason_code: "operator_reverted_persona".to_string(),
            },
        )
        .unwrap();
    assert_eq!(rolled.version.version_number, 2);
    assert_eq!(
        rolled.version.change_kind,
        kura_profiles::ChangeKind::ROLLED_BACK
    );
    assert_eq!(
        rolled.version.source_version_id,
        draft.version.profile_version_id
    );
    assert!(
        store
            .is_tenant_default_profile("ten_1", &profile_id)
            .unwrap()
    );

    // Retire (archive) removes the default selection and, being the tenant
    // default, seeds a system-fallback default profile selection.
    let retired = store
        .retire_agent_profile(
            &actor("ten_1", "prn_1"),
            &profile_id,
            Status::ARCHIVED,
            &RetirementInput {
                reason_code: "operator_retired_profile".to_string(),
            },
        )
        .unwrap();
    assert_eq!(retired.profile.status, Status::ARCHIVED);
    assert!(retired.profile.archived_at.is_some());
    assert_eq!(
        retired.version.change_kind,
        kura_profiles::ChangeKind::ARCHIVED
    );
    assert_eq!(
        retired.version.rollback_eligibility,
        kura_profiles::RollbackEligibility::PROFILE_ARCHIVED
    );
    assert_eq!(
        retired.selection.selection_reason,
        kura_profiles::SelectionReason::SYSTEM_FALLBACK
    );
    assert!(
        !store
            .is_tenant_default_profile("ten_1", &profile_id)
            .unwrap()
    );

    // Activating an archived profile is rejected.
    assert!(
        store
            .activate_agent_profile(
                &actor("ten_1", "prn_1"),
                &profile_id,
                &ActivationInput::default()
            )
            .is_err()
    );
}

#[test]
fn active_selection_and_runtime_projections() {
    let dir = temp_dir("profiles_projections");
    let store = SQLiteStore::new(&dir).unwrap();

    // Lazy default: no selection yet → ensure_default_agent_profile seeds one.
    let (profile, selection) = store
        .active_agent_profile_selection("ten_1")
        .unwrap()
        .expect("default selection");
    assert_eq!(profile.display_name, "Default Agent");
    assert_eq!(profile.display_identity.name, "Kura");
    assert_eq!(selection.selection_scope, "tenant_default");
    assert_eq!(
        selection.selection_reason,
        kura_profiles::SelectionReason::DEFAULT_SEEDED
    );

    // A second tenant is independent.
    let (profile_b, _) = store
        .active_agent_profile_selection("ten_2")
        .unwrap()
        .expect("second tenant default");
    assert_ne!(profile_b.tenant_id, profile.tenant_id);

    let now = Utc::now();
    let projection = kura_profiles::build_runtime_projection(
        &profile,
        &selection,
        kura_profiles::RuntimeProjectionInput {
            resource_kind: kura_profiles::RuntimeResourceKind::RUN,
            resource_id: "run_1".to_string(),
            run_id: "run_1".to_string(),
            occurred_at: Some(now),
            ..kura_profiles::RuntimeProjectionInput::default()
        },
    );
    let stored: RuntimeProjection = store
        .record_runtime_profile_projection(projection.clone())
        .unwrap();
    assert!(!stored.runtime_profile_projection_id.is_empty());
    assert_eq!(stored.resource_id, "run_1");
    assert_eq!(stored.safe_display_name, "Default Agent");
    assert_eq!(stored.occurred_at, now);

    let listed = store
        .list_runtime_profile_projections("ten_1", "", "", "", 20)
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(
        listed[0].runtime_profile_projection_id,
        stored.runtime_profile_projection_id
    );
    assert_eq!(
        listed[0].deferred_binding_classification,
        "roadmap_58_deferred_binding_unapplied"
    );

    // Resource-kind + id filter narrows; other tenants see nothing.
    let filtered = store
        .list_runtime_profile_projections("ten_1", "run", "run_1", "", 20)
        .unwrap();
    assert_eq!(filtered.len(), 1);
    let other = store
        .list_runtime_profile_projections("ten_2", "", "", "", 20)
        .unwrap();
    assert!(other.is_empty());
    let _: ProfileVersion = ProfileVersion::default();
    let _ = Duration::hours(1);
}

#[test]
fn profile_mutation_requires_explicit_actor_and_valid_input() {
    let dir = temp_dir("profiles_validation");
    let store = SQLiteStore::new(&dir).unwrap();

    let bad_actor = actor("", "");
    assert!(store.create_agent_profile(&bad_actor, &input("X")).is_err());

    let mut bad_input = input("");
    bad_input.display_name.clear();
    let err = store
        .create_agent_profile(&actor("ten_1", "prn_1"), &bad_input)
        .unwrap_err();
    assert!(
        err.contains("display_name_required"),
        "reason code embedded: {err}"
    );

    // Unknown profile updates are rejected.
    let err = store
        .update_agent_profile(&actor("ten_1", "prn_1"), "prof_missing", &input("X"))
        .unwrap_err();
    assert!(err.contains("agent profile not found"), "{err}");

    // Unsupported retirement status is rejected.
    let created = store
        .create_agent_profile(&actor("ten_1", "prn_1"), &input("Keep"))
        .unwrap();
    let err = store
        .retire_agent_profile(
            &actor("ten_1", "prn_1"),
            &created.profile.profile_id,
            Status::DRAFT,
            &RetirementInput::default(),
        )
        .unwrap_err();
    assert!(err.contains("unsupported retirement status"), "{err}");
    let _: AgentProfile = AgentProfile::default();
}
