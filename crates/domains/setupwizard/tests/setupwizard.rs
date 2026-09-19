//! Behavioral tests ported from the Go setupwizard package.

use std::sync::Arc;

use kura_identity::{LifecycleStatus, Role, TenantContext, permissions_for_role};
use kura_setupwizard::{
    MemoryStore, ServiceDependencies, SetupState, SetupStyle, SupportStatus,
    TARGET_OPENAI_COMPATIBLE, catalog_targets, new_service,
};

fn admin() -> TenantContext {
    TenantContext {
        tenant_id: "ten_1".to_string(),
        principal_id: "prn_admin".to_string(),
        role: Some(Role::Admin),
        permissions: permissions_for_role(Role::Admin, LifecycleStatus::Active),
        ..TenantContext::default()
    }
}

fn viewer() -> TenantContext {
    TenantContext {
        tenant_id: "ten_1".to_string(),
        principal_id: "prn_viewer".to_string(),
        role: Some(Role::Viewer),
        permissions: permissions_for_role(Role::Viewer, LifecycleStatus::Active),
        ..TenantContext::default()
    }
}

fn service() -> kura_setupwizard::Service {
    new_service(ServiceDependencies {
        store: Some(Arc::new(MemoryStore::default())),
        ..ServiceDependencies::default()
    })
}

#[test]
fn catalog_lists_supported_targets_sorted() {
    let targets = catalog_targets("ten_1");
    assert_eq!(targets.len(), 6);
    assert!(targets.windows(2).all(|w| w[0].target_id <= w[1].target_id));
    assert!(
        targets
            .iter()
            .all(|t| t.support_status == SupportStatus::Supported)
    );
}

#[tokio::test]
async fn start_creates_in_progress_session() {
    let service = service();
    let session = service
        .start(kura_setupwizard::StartInput {
            tenant_context: admin(),
            target_id: TARGET_OPENAI_COMPATIBLE.to_string(),
            setup_style: SetupStyle::SubmittedSecret,
            source: String::new(),
        })
        .await
        .expect("start");
    assert_eq!(session.state, SetupState::InProgress);
    assert_eq!(session.target_id, TARGET_OPENAI_COMPATIBLE);
    assert!(!session.setup_session_id.is_empty());
}

#[tokio::test]
async fn start_denies_viewer() {
    let service = service();
    let err = service
        .start(kura_setupwizard::StartInput {
            tenant_context: viewer(),
            target_id: TARGET_OPENAI_COMPATIBLE.to_string(),
            setup_style: SetupStyle::SubmittedSecret,
            source: String::new(),
        })
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        kura_setupwizard::SetupError::PermissionDenied
    ));
}

#[tokio::test]
async fn list_targets_projects_current_session() {
    let service = service();
    let _ = service
        .start(kura_setupwizard::StartInput {
            tenant_context: admin(),
            target_id: TARGET_OPENAI_COMPATIBLE.to_string(),
            setup_style: SetupStyle::SubmittedSecret,
            source: String::new(),
        })
        .await
        .expect("start");

    let targets = service.list_targets(&admin()).await.expect("list targets");
    let target = targets
        .iter()
        .find(|t| t.target_id == TARGET_OPENAI_COMPATIBLE)
        .expect("target");
    assert!(!target.current_session_id.is_empty());
    assert_eq!(target.current_state, SetupState::InProgress);
}
