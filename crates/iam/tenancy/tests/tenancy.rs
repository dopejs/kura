//! Integration tests for the kura-tenancy accessor layer.
//!
//! Covers: tenant-context requirement (fail-closed), Runtime accessor round-trips and
//! tenant scoping, cross-tenant reads returning not-found WITHOUT leaking existence,
//! audit denial emission on cross-tenant access, cross-tenant write refusal, per-domain
//! accessors, and the permission-based access scopes.

use std::sync::Arc;

use kura_audit::Emitter;
use kura_events::{Bus, Filter};
use kura_identity::tenantctx;
use kura_identity::{Permission, TenantContext};
use kura_runtime::{Run, RunStatus};
use kura_store::SQLiteStore;
use kura_store::delivery::DeliveryTargetRecord;
use kura_store::schedule::ScheduleRecord;
use kura_tenancy::runtime::{runtime_tenant_id, runtime_tenant_predicate};
use kura_tenancy::{BindingAccessScope, ProfileAccessScope, TenancyError};

fn temp_dir(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("kura_tenancy_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.to_string_lossy().to_string()
}

fn ctx(tenant: &str) -> TenantContext {
    TenantContext {
        principal_id: "prn_1".to_string(),
        tenant_id: tenant.to_string(),
        token_id: "tok_1".to_string(),
        ..TenantContext::default()
    }
}

fn make_run(id: &str) -> Run {
    let now = chrono::Utc::now();
    Run {
        run_id: id.to_string(),
        entrypoint: "test entrypoint".to_string(),
        status: RunStatus::Running,
        goal: "test goal".to_string(),
        created_at: now,
        updated_at: now,
        ..Run::default()
    }
}

#[test]
fn require_without_context_fails_closed() {
    assert_eq!(
        kura_tenancy::require(),
        Err(TenancyError::TenantContextRequired)
    );
    assert!(tenantctx::from_context().is_none());
    // Must panics when the context is missing.
    let result = std::panic::catch_unwind(kura_tenancy::must);
    assert!(result.is_err());
}

#[test]
fn runtime_accessor_round_trip_and_tenant_scoping() {
    let store = SQLiteStore::new(&temp_dir("rt_scope")).unwrap();
    let rt = kura_tenancy::runtime::Runtime::new(store, None);

    tenantctx::with_context(ctx("ten_a"), || {
        rt.upsert_run_for_tenant(&make_run("run_a")).unwrap();
        let runs = rt.list_runs_for_tenant().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].run_id, "run_a");
        assert!(rt.get_run_for_tenant("run_a").unwrap().is_some());
    });

    // Tenant B cannot see A's run: not-found, no error.
    tenantctx::with_context(ctx("ten_b"), || {
        assert!(rt.list_runs_for_tenant().unwrap().is_empty());
        assert!(rt.get_run_for_tenant("run_a").unwrap().is_none());
    });
}

#[test]
fn cross_tenant_read_emits_audit_denial_without_leaking_existence() {
    let store = SQLiteStore::new(&temp_dir("rt_audit")).unwrap();
    let bus = Arc::new(Bus::new());
    let rt = kura_tenancy::runtime::Runtime::new(store, Some(Emitter::new(bus.clone())));

    tenantctx::with_context(ctx("ten_a"), || {
        rt.upsert_run_for_tenant(&make_run("run_a")).unwrap();
    });

    tenantctx::with_context(ctx("ten_b"), || {
        // Cross-tenant by-id read: not-found (existence not leaked)...
        assert!(rt.get_run_for_tenant("run_a").unwrap().is_none());
        // ...and the audit denial was published.
        let audit = bus.list(&Filter {
            category: "audit".to_string(),
            ..Filter::default()
        });
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].name, "audit.cross_tenant_access_denied");
    });

    // A subsequent read by the owning tenant is unaffected (denial only on the attempt).
    tenantctx::with_context(ctx("ten_a"), || {
        assert!(rt.get_run_for_tenant("run_a").unwrap().is_some());
    });
}

#[test]
fn cross_tenant_write_refused_and_audited() {
    let store = SQLiteStore::new(&temp_dir("rt_write")).unwrap();
    let bus = Arc::new(Bus::new());
    let rt = kura_tenancy::runtime::Runtime::new(store, Some(Emitter::new(bus.clone())));

    tenantctx::with_context(ctx("ten_a"), || {
        rt.upsert_run_for_tenant(&make_run("run_a")).unwrap();
    });

    tenantctx::with_context(ctx("ten_b"), || {
        let err = rt.upsert_run_for_tenant(&make_run("run_a")).unwrap_err();
        assert_eq!(err, TenancyError::CrossTenantWrite);
        let audit = bus.list(&Filter {
            category: "audit".to_string(),
            ..Filter::default()
        });
        assert_eq!(audit.len(), 1);
    });

    // A's row is preserved.
    tenantctx::with_context(ctx("ten_a"), || {
        assert!(rt.get_run_for_tenant("run_a").unwrap().is_some());
    });
}

#[test]
fn checkpoints_round_trip_via_runtime() {
    let store = SQLiteStore::new(&temp_dir("rt_ck")).unwrap();
    let rt = kura_tenancy::runtime::Runtime::new(store, None);
    tenantctx::with_context(ctx("ten_a"), || {
        let run = make_run("run_ck");
        rt.upsert_run_for_tenant(&run).unwrap();
        let checkpoint = kura_runtime::RunCheckpoint {
            run: run.clone(),
            steps: Vec::new(),
            tool_calls: Vec::new(),
            captured_at: chrono::Utc::now(),
        };
        rt.save_checkpoint_for_tenant(&checkpoint).unwrap();
        let latest = rt.list_latest_checkpoints_for_tenant().unwrap();
        assert_eq!(latest.len(), 1);
        assert_eq!(latest[0].run.run_id, "run_ck");
    });
}

#[test]
fn approvals_accessor_cross_tenant_write_refused() {
    let store = SQLiteStore::new(&temp_dir("approvals")).unwrap();
    let accessor = kura_tenancy::approvals::Approvals::new(store, None);
    let now = chrono::Utc::now();
    let approval = kura_policy::Approval {
        approval_id: "apr_1".to_string(),
        action: "run_tool".to_string(),
        reason: "allow".to_string(),
        status: kura_policy::ApprovalStatus::Pending,
        created_at: now,
        updated_at: now,
        ..kura_policy::Approval::default()
    };
    tenantctx::with_context(ctx("ten_a"), || {
        accessor.upsert_approval_for_tenant(&approval).unwrap();
        assert_eq!(accessor.list_approvals_for_tenant().unwrap().len(), 1);
    });
    tenantctx::with_context(ctx("ten_b"), || {
        assert!(accessor.list_approvals_for_tenant().unwrap().is_empty());
        let err = accessor.upsert_approval_for_tenant(&approval).unwrap_err();
        assert_eq!(err, TenancyError::CrossTenantWrite);
    });
}

#[test]
fn schedules_accessor_cross_tenant_not_found() {
    let store = SQLiteStore::new(&temp_dir("schedules")).unwrap();
    let accessor = kura_tenancy::schedules::Schedules::new(store, None);
    let mut record = ScheduleRecord::default();
    record.schedule_id = "sch_1".to_string();
    record.environment_scope = "test".to_string();
    record.kind = "cron".to_string();
    record.status = "active".to_string();

    tenantctx::with_context(ctx("ten_a"), || {
        accessor.upsert_schedule_for_tenant(&record).unwrap();
        let got = accessor.get_schedule_for_tenant("test", "sch_1").unwrap();
        assert!(got.is_some());
        assert_eq!(got.unwrap().schedule_id, "sch_1");
        assert_eq!(accessor.list_schedules_for_tenant("test").unwrap().len(), 1);
    });
    tenantctx::with_context(ctx("ten_b"), || {
        // Cross-tenant get -> not-found (no existence leak).
        assert!(
            accessor
                .get_schedule_for_tenant("test", "sch_1")
                .unwrap()
                .is_none()
        );
        assert!(
            accessor
                .list_schedules_for_tenant("test")
                .unwrap()
                .is_empty()
        );
    });
}

#[test]
fn delivery_accessor_binds_tenant() {
    let store = SQLiteStore::new(&temp_dir("delivery")).unwrap();
    let accessor = kura_tenancy::delivery::Delivery::new(store, None);
    let mut record = DeliveryTargetRecord::default();
    record.target_id = "tgt_1".to_string();
    record.environment_scope = "test".to_string();

    tenantctx::with_context(ctx("ten_a"), || {
        accessor.upsert_target_for_tenant(&record).unwrap();
    });

    // A different tenant cannot re-bind the same row: the write is refused (the row
    // was bound to ten_a by the accessor's bind_row_tenant step).
    tenantctx::with_context(ctx("ten_b"), || {
        let err = accessor.upsert_target_for_tenant(&record).unwrap_err();
        assert_eq!(err, TenancyError::CrossTenantWrite);
    });
}

fn events_accessor_requires_tenant_and_rejects_global() {
    let store = SQLiteStore::new(&temp_dir("events")).unwrap();
    let accessor = kura_tenancy::events::Events::new(store, None);

    // No tenant context: fail-closed.
    assert_eq!(
        accessor.append_event_for_tenant(&kura_events::Event::default()),
        Err(TenancyError::TenantContextRequired)
    );
    assert_eq!(
        accessor.list_events_for_tenant(&Filter::default()),
        Err(TenancyError::TenantContextRequired)
    );

    // Global category refused even with a tenant context.
    let mut global = kura_events::Event::default();
    global.event_id = "evt_global".to_string();
    global.category = "system".to_string();
    tenantctx::with_context(ctx("ten_a"), || {
        let err = accessor.append_event_for_tenant(&global).unwrap_err();
        assert!(matches!(err, TenancyError::Store(_)));
    });

    // Tenant-owned category appends and lists under the tenant.
    let mut owned = kura_events::Event::default();
    owned.event_id = "evt_a".to_string();
    owned.category = "run".to_string();
    owned.name = "run.created".to_string();
    tenantctx::with_context(ctx("ten_a"), || {
        let persisted = accessor.append_event_for_tenant(&owned).unwrap();
        assert_eq!(persisted.tenant_id, "ten_a");
        let list = accessor.list_events_for_tenant(&Filter::default()).unwrap();
        assert_eq!(list.len(), 1);
    });
    tenantctx::with_context(ctx("ten_b"), || {
        assert!(
            accessor
                .list_events_for_tenant(&Filter::default())
                .unwrap()
                .is_empty()
        );
    });
}

#[test]
fn binding_and_profile_scope_checks() {
    let scope = BindingAccessScope {
        tenant_id: "ten_a".to_string(),
        permissions: vec![Permission::BindingsInspect, Permission::BindingsManage],
    };
    let ws_a = kura_bindings::Workspace {
        tenant_id: "ten_a".to_string(),
        ..kura_bindings::Workspace::default()
    };
    let ws_b = kura_bindings::Workspace {
        tenant_id: "ten_b".to_string(),
        ..kura_bindings::Workspace::default()
    };
    assert!(scope.can_inspect_workspace(&ws_a));
    assert!(scope.can_manage_workspace(&ws_a));
    assert!(!scope.can_inspect_workspace(&ws_b));
    assert!(!scope.can_manage_workspace(&ws_b));
    assert!(scope.can_inspect_tenant("ten_a"));
    assert!(!scope.can_inspect_tenant("ten_b"));

    let profiles = ProfileAccessScope {
        tenant_id: "ten_a".to_string(),
        permissions: vec![Permission::ProfilesInspect],
    };
    let prof_a = kura_profiles::AgentProfile {
        tenant_id: "ten_a".to_string(),
        ..kura_profiles::AgentProfile::default()
    };
    let prof_b = kura_profiles::AgentProfile {
        tenant_id: "ten_b".to_string(),
        ..kura_profiles::AgentProfile::default()
    };
    assert!(profiles.can_inspect(&prof_a));
    assert!(!profiles.can_manage(&prof_a)); // inspect-only scope
    assert!(!profiles.can_inspect(&prof_b));
}

#[test]
fn runtime_tenant_helpers() {
    assert_eq!(runtime_tenant_id(), "");
    assert_eq!(runtime_tenant_predicate(""), (String::new(), None));
    let (sql, arg) = runtime_tenant_predicate("ten_a");
    assert_eq!(sql, "(tenant_id = ? OR tenant_id IS NULL)");
    assert_eq!(arg, Some("ten_a".to_string()));
}
