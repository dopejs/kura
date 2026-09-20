//! Behavioral tests for the workspace DAOs (rs/store/src/workspaces.rs),
//! ported from daemon/internal/store/workspace_store_test.go: default
//! provisioning, create/update-status round-trips, default-first ordering,
//! tenant isolation, and selection gates.

use kura_bindings::{RepairStatus, WorkspaceStatus};
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

#[test]
fn default_workspace_is_provisioned_and_idempotent() {
    let dir = temp_dir("workspaces_default");
    let store = SQLiteStore::new(&dir).unwrap();

    let first = store.ensure_default_workspace("ten_1").unwrap();
    assert_eq!(first.display_name, "Personal Workspace");
    assert!(first.is_default);
    assert_eq!(first.status, WorkspaceStatus::ACTIVE);
    assert_eq!(first.owner_principal_id, "system");
    assert_eq!(first.repair_status, RepairStatus::HEALTHY);

    let second = store.ensure_default_workspace("ten_1").unwrap();
    assert_eq!(second.workspace_id, first.workspace_id, "idempotent");
    assert_eq!(
        store.ensure_default_workspace("ten_2").unwrap().tenant_id,
        "ten_2"
    );

    // Listing provisions the default lazily and puts it first.
    let listed = store.list_workspaces("ten_1", 50).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].workspace_id, first.workspace_id);
}

#[test]
fn workspace_create_list_order_and_status_transitions() {
    let dir = temp_dir("workspaces_lifecycle");
    let store = SQLiteStore::new(&dir).unwrap();

    let (ws, audit_id) = store
        .create_workspace(&actor("ten_1", "prn_1"), "Support")
        .unwrap();
    assert_eq!(ws.display_name, "Support");
    assert!(!ws.is_default);
    assert_eq!(ws.status, WorkspaceStatus::ACTIVE);
    assert_eq!(ws.owner_principal_id, "prn_1");
    assert!(!audit_id.is_empty());

    let (ws2, _) = store
        .create_workspace(&actor("ten_1", "prn_1"), "Research")
        .unwrap();

    // Default first, then updated_at DESC.
    let listed = store.list_workspaces("ten_1", 50).unwrap();
    assert_eq!(listed.len(), 3);
    assert!(listed[0].is_default, "default first");
    assert_eq!(
        listed[1].workspace_id, ws2.workspace_id,
        "newest created workspace second"
    );
    assert_eq!(listed[2].workspace_id, ws.workspace_id);

    // Get by id within the tenant.
    let got = store
        .get_workspace("ten_1", &ws.workspace_id)
        .unwrap()
        .expect("present");
    assert_eq!(got.workspace_id, ws.workspace_id);
    assert!(
        store
            .get_workspace("ten_1", "ws_missing")
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .get_workspace("ten_2", &ws.workspace_id)
            .unwrap()
            .is_none()
    );

    // Archive: status, archived_at, repair_status disabled.
    let (archived, _) = store
        .update_workspace_status(
            &actor("ten_1", "prn_1"),
            &ws.workspace_id,
            WorkspaceStatus::ARCHIVED,
        )
        .unwrap();
    assert_eq!(archived.status, WorkspaceStatus::ARCHIVED);
    assert!(archived.archived_at.is_some());
    assert_eq!(archived.repair_status, RepairStatus::DISABLED);

    // Reactivate clears archived_at and restores healthy.
    let (active, _) = store
        .update_workspace_status(
            &actor("ten_1", "prn_1"),
            &ws.workspace_id,
            WorkspaceStatus::ACTIVE,
        )
        .unwrap();
    assert_eq!(active.status, WorkspaceStatus::ACTIVE);
    assert!(active.archived_at.is_none());
    assert_eq!(active.repair_status, RepairStatus::HEALTHY);

    // The default workspace cannot be retired.
    let err = store
        .update_workspace_status(
            &actor("ten_1", "prn_1"),
            &listed[0].workspace_id,
            WorkspaceStatus::ARCHIVED,
        )
        .unwrap_err();
    assert!(err.contains("default_workspace_not_retirable"), "{err}");

    // Unknown status is rejected.
    assert!(
        store
            .update_workspace_status(
                &actor("ten_1", "prn_1"),
                &ws.workspace_id,
                WorkspaceStatus::new("nope")
            )
            .is_err()
    );

    // Missing workspace is rejected.
    assert!(
        store
            .update_workspace_status(
                &actor("ten_1", "prn_1"),
                "ws_missing",
                WorkspaceStatus::ARCHIVED
            )
            .is_err()
    );
}

#[test]
fn workspace_selectable_gates() {
    let dir = temp_dir("workspaces_selectable");
    let store = SQLiteStore::new(&dir).unwrap();

    let (ws, _) = store
        .create_workspace(&actor("ten_1", "prn_1"), "Support")
        .unwrap();
    assert!(
        store
            .is_workspace_selectable("ten_1", &ws.workspace_id)
            .unwrap()
    );
    assert!(
        !store
            .is_workspace_selectable("ten_1", "ws_missing")
            .unwrap()
    );
    assert!(
        !store
            .is_workspace_selectable("ten_2", &ws.workspace_id)
            .unwrap()
    );
    assert!(!store.is_workspace_selectable("ten_1", "").unwrap());

    store
        .update_workspace_status(
            &actor("ten_1", "prn_1"),
            &ws.workspace_id,
            WorkspaceStatus::DISABLED,
        )
        .unwrap();
    assert!(
        !store
            .is_workspace_selectable("ten_1", &ws.workspace_id)
            .unwrap()
    );

    // Explicit actor required for mutations.
    assert!(store.create_workspace(&actor("", ""), "X").is_err());
    assert!(
        store
            .update_workspace_status(&actor("", ""), &ws.workspace_id, WorkspaceStatus::ACTIVE)
            .is_err()
    );
}
