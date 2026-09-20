//! SQLite-backed activation seam tests (wave 8 parity): the store adapter
//! (StateStore/IdentityRepository/AuditSink over `kura-store`), the billing
//! projector, the chat runner, and the full service wiring via
//! `Service::with_sqlite`. Ports the Go behavior from
//! `daemon/internal/store/activation_test.go` and the app wiring tests.

use std::sync::Arc;

use chrono::DateTime;
use chrono::TimeZone;
use chrono::Utc;
use kura_activation::ActivateInput;
use kura_activation::AuditSink;
use kura_activation::BillingProjector;
use kura_activation::BillingProjectorAdapter;
use kura_activation::ChatRunner;
use kura_activation::ChatRunnerAdapter;
use kura_activation::GetInput;
use kura_activation::IdentityRepository;
use kura_activation::QuotaBaseline;
use kura_activation::QuotaBaselineStatus;
use kura_activation::ReadinessItem;
use kura_activation::ReadinessKind;
use kura_activation::ReadinessStatus;
use kura_activation::ReasonCode;
use kura_activation::RemediationOwner;
use kura_activation::RunTestChatInput;
use kura_activation::STEP_QUOTA_BASELINE_READY;
use kura_activation::STEP_TENANT_RESOLVED;
use kura_activation::STEP_TEST_CHAT;
use kura_activation::Service;
use kura_activation::SqliteActivationStore;
use kura_activation::State;
use kura_activation::StateStore;
use kura_activation::Status;
use kura_activation::TestChatInput;
use kura_activation::TestChatStatus;
use kura_activation::default_test_chat_first_action;
use kura_activation::reason_code_from_error;
use kura_billing::BillingError;
use kura_identity::AuditEventFilter;
use kura_identity::LifecycleStatus;
use kura_identity::Membership;
use kura_identity::MembershipFilter;
use kura_identity::Principal;
use kura_identity::PrincipalFilter;
use kura_identity::PrincipalKind;
use kura_identity::Role;
use kura_identity::Tenant;
use kura_identity::TenantContext;
use kura_identity::TenantFilter;
use kura_identity::TenantKind;
use kura_identity::TokenAuthority;
use kura_identity::TokenTenantGrant;
use kura_store::SQLiteStore;

fn temp_dir(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("kura_activation_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.to_string_lossy().to_string()
}

fn test_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 5, 6, 10, 0, 0)
        .single()
        .unwrap_or_else(Utc::now)
}

fn open_store(name: &str) -> SqliteActivationStore {
    SqliteActivationStore::new(SQLiteStore::new(&temp_dir(name)).expect("open sqlite store"))
        .expect("open sqlite activation store")
}

fn sample_state(
    activation_id: &str,
    principal_id: &str,
    tenant_id: &str,
    now: DateTime<Utc>,
) -> State {
    State {
        activation_id: activation_id.to_string(),
        principal_id: principal_id.to_string(),
        tenant_id: tenant_id.to_string(),
        environment_scope: "test".to_string(),
        status: Status::ACTIVE.into(),
        current_step_id: STEP_TEST_CHAT.to_string(),
        completed_step_ids: vec![
            STEP_TENANT_RESOLVED.to_string(),
            STEP_QUOTA_BASELINE_READY.to_string(),
        ],
        blocking_reason_codes: Vec::new(),
        readiness_items: vec![ReadinessItem {
            item_id: "tenant-access".to_string(),
            item_kind: ReadinessKind::TENANT_ACCESS.into(),
            status: ReadinessStatus::READY.into(),
            reason_code: ReasonCode::default(),
            display_name: "Tenant access".to_string(),
            required_for_activation: true,
            retryable: false,
            remediation_owner: RemediationOwner::NONE_REQUIRED.into(),
            updated_at: now,
        }],
        quota_baseline: Some(QuotaBaseline {
            tenant_id: tenant_id.to_string(),
            plan_key: "free".to_string(),
            enforcement_mode: "enforced".to_string(),
            status: QuotaBaselineStatus::AVAILABLE.into(),
            quotas: Vec::new(),
            projected_at: now,
            projection_source: "activation_default".to_string(),
            reason_code: ReasonCode::default(),
        }),
        first_action: default_test_chat_first_action(true, Vec::new()),
        test_chat: None,
        failure_reason: None,
        created_at: now,
        updated_at: now,
        first_action_completed_at: None,
        last_evaluated_at: now,
        last_transition_audit_event: String::new(),
        metadata: None,
    }
}

fn active_token(token_id: &str, principal_id: &str) -> TokenAuthority {
    TokenAuthority {
        token_id: token_id.to_string(),
        principal_id: principal_id.to_string(),
        default_tenant_id: String::new(),
        status: LifecycleStatus::Active,
        expires_at: None,
    }
}

fn tenant_context(principal_id: &str, tenant_id: &str, token_id: &str) -> TenantContext {
    TenantContext {
        principal_id: principal_id.to_string(),
        tenant_id: tenant_id.to_string(),
        token_id: token_id.to_string(),
        ..TenantContext::default()
    }
}

fn active_principal(principal_id: &str, now: DateTime<Utc>) -> Principal {
    Principal {
        principal_id: principal_id.to_string(),
        principal_kind: PrincipalKind::User,
        display_name: "Hosted User".to_string(),
        status: LifecycleStatus::Active,
        default_tenant_id: String::new(),
        created_at: now,
        updated_at: now,
        disabled_at: None,
        removed_at: None,
    }
}

// ---------------------------------------------------------------------------
// StateStore
// ---------------------------------------------------------------------------

#[tokio::test]
async fn state_store_round_trips_activation_state() {
    let now = test_now();
    let store = open_store("state_roundtrip");
    let state = sample_state("act_prn_1_ten_personal", "prn_1", "ten_personal", now);

    store
        .upsert_activation_state(state.clone())
        .await
        .expect("upsert");

    let by_id = store
        .get_activation_state("act_prn_1_ten_personal")
        .await
        .expect("get by id")
        .expect("state present");
    assert_eq!(by_id, state);

    let by_key = store
        .get_activation_state_for_principal_tenant("prn_1", "ten_personal")
        .await
        .expect("get by principal tenant")
        .expect("state present");
    assert_eq!(by_key, state);

    assert!(
        store
            .get_activation_state("act_missing")
            .await
            .expect("get missing")
            .is_none()
    );
    assert!(
        store
            .get_activation_state_for_principal_tenant("prn_2", "ten_other")
            .await
            .expect("get missing key")
            .is_none()
    );
}

#[tokio::test]
async fn state_store_preserves_optional_columns() {
    let now = test_now();
    let store = open_store("state_optional");
    let mut state = sample_state("act_optional", "prn_opt", "ten_opt", now);
    state.first_action_completed_at = Some(now);
    state.last_transition_audit_event = "audit_activation_1234".to_string();
    state.metadata = Some(serde_json::Map::from_iter([(
        "source".to_string(),
        serde_json::json!("signup"),
    )]));

    store
        .upsert_activation_state(state.clone())
        .await
        .expect("upsert");
    let got = store
        .get_activation_state_for_principal_tenant("prn_opt", "ten_opt")
        .await
        .expect("get")
        .expect("state present");
    assert_eq!(got, state);
}

#[tokio::test]
async fn state_store_replaces_row_by_principal_tenant() {
    let now = test_now();
    let store = open_store("state_replace");
    let original = sample_state("act_prn_1_ten_personal_1", "prn_1", "ten_personal", now);
    let mut replacement = sample_state("act_prn_1_ten_personal_2", "prn_1", "ten_personal", now);
    replacement.status = Status::FIRST_ACTION_COMPLETED.into();

    store
        .upsert_activation_state(original)
        .await
        .expect("upsert original");
    store
        .upsert_activation_state(replacement)
        .await
        .expect("upsert replacement");

    assert!(
        store
            .get_activation_state("act_prn_1_ten_personal_1")
            .await
            .expect("get original")
            .is_none(),
        "replacement must remove the old activation row"
    );
    let got = store
        .get_activation_state_for_principal_tenant("prn_1", "ten_personal")
        .await
        .expect("get by principal tenant")
        .expect("replacement present");
    assert_eq!(got.status, Status::FIRST_ACTION_COMPLETED);
    assert_eq!(got.activation_id, "act_prn_1_ten_personal_2");
}

#[tokio::test]
async fn state_store_rejects_invalid_state() {
    let now = test_now();
    let store = open_store("state_invalid");
    let mut state = sample_state("act_invalid", "prn_invalid", "ten_invalid", now);
    state.status = Status::default();
    let err = store
        .upsert_activation_state(state)
        .await
        .expect_err("invalid state must be rejected");
    assert!(err.to_string().contains("status is required"), "{err}");
}

#[tokio::test]
async fn state_store_persists_across_reopen() {
    let now = test_now();
    let dir = temp_dir("state_reopen");
    let store = SqliteActivationStore::new(SQLiteStore::new(&dir).expect("open store"))
        .expect("open adapter");
    store
        .upsert_activation_state(sample_state("act_reopen", "prn_reopen", "ten_reopen", now))
        .await
        .expect("upsert");

    let reopened = SqliteActivationStore::new(SQLiteStore::new(&dir).expect("reopen store"))
        .expect("reopen adapter");
    let got = reopened
        .get_activation_state_for_principal_tenant("prn_reopen", "ten_reopen")
        .await
        .expect("get")
        .expect("persisted state");
    assert_eq!(got.activation_id, "act_reopen");
}

// ---------------------------------------------------------------------------
// IdentityRepository + AuditSink
// ---------------------------------------------------------------------------

#[tokio::test]
async fn identity_repository_round_trips_over_sqlite() {
    let now = test_now();
    let store = open_store("identity_roundtrip");

    let principal = active_principal("prn_1", now);
    store
        .upsert_principal(principal.clone())
        .await
        .expect("upsert principal");
    let got = store
        .get_principal("prn_1")
        .await
        .expect("get principal")
        .expect("principal present");
    assert_eq!(got.principal_id, principal.principal_id);
    assert_eq!(got.status, principal.status);
    assert!(
        store
            .get_principal("prn_missing")
            .await
            .expect("get missing")
            .is_none()
    );
    let listed = store
        .list_principals(&PrincipalFilter {
            tenant_id: String::new(),
            status: Some(LifecycleStatus::Active),
            limit: 0,
        })
        .await
        .expect("list principals");
    assert_eq!(listed.len(), 1);

    let tenant = Tenant {
        tenant_id: "ten_personal".to_string(),
        tenant_kind: TenantKind::Personal,
        display_name: "Personal tenant".to_string(),
        status: LifecycleStatus::Active,
        created_at: now,
        updated_at: now,
        created_by_principal_id: "prn_1".to_string(),
        default_owner_principal_id: "prn_1".to_string(),
        caller_membership_role: None,
        caller_membership_status: None,
        caller_permissions: Vec::new(),
        default_for_current_token: false,
        default_for_current_principal: false,
    };
    store
        .upsert_tenant(tenant.clone())
        .await
        .expect("upsert tenant");
    let got_tenant = store
        .get_tenant("ten_personal")
        .await
        .expect("get tenant")
        .expect("tenant present");
    assert_eq!(got_tenant.tenant_id, tenant.tenant_id);
    assert_eq!(got_tenant.tenant_kind, tenant.tenant_kind);
    assert_eq!(
        got_tenant.default_owner_principal_id,
        tenant.default_owner_principal_id
    );
    let tenants = store
        .list_tenants(&TenantFilter {
            tenant_kind: Some(TenantKind::Personal),
            status: Some(LifecycleStatus::Active),
            limit: 0,
        })
        .await
        .expect("list tenants");
    assert_eq!(tenants.len(), 1);

    let membership = Membership {
        membership_id: "mem_1".to_string(),
        tenant_id: "ten_personal".to_string(),
        principal_id: "prn_1".to_string(),
        role: Role::Owner,
        status: LifecycleStatus::Active,
        invitation_id: String::new(),
        created_at: now,
        updated_at: now,
        accepted_at: Some(now),
        removed_at: None,
    };
    store
        .upsert_membership(membership.clone())
        .await
        .expect("upsert membership");
    let memberships = store
        .list_memberships(&MembershipFilter {
            tenant_id: "ten_personal".to_string(),
            status: None,
            role: None,
            limit: 0,
        })
        .await
        .expect("list memberships");
    assert_eq!(memberships.len(), 1);
    assert_eq!(memberships[0].membership_id, membership.membership_id);
    assert_eq!(memberships[0].role, membership.role);
    assert_eq!(memberships[0].status, membership.status);

    let grant = TokenTenantGrant {
        grant_id: "grant_1".to_string(),
        token_id: "tok_1".to_string(),
        tenant_id: "ten_personal".to_string(),
        is_default: true,
        status: LifecycleStatus::Active,
        created_at: now,
        updated_at: now,
        revoked_at: None,
        granted_by_principal_id: "prn_1".to_string(),
    };
    store
        .upsert_token_tenant_grant(grant.clone())
        .await
        .expect("upsert grant");
    let grants = store
        .list_token_tenant_grants("tok_1")
        .await
        .expect("list grants");
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0].grant_id, grant.grant_id);
    assert_eq!(grants[0].tenant_id, grant.tenant_id);
    assert_eq!(grants[0].status, grant.status);
}

#[tokio::test]
async fn audit_sink_appends_tenant_audit_event() {
    let now = test_now();
    let store = open_store("audit_sink");
    let event = kura_identity::TenantAuditEvent {
        audit_event_id: String::new(),
        event_kind: "tenant.activation_started".to_string(),
        tenant_id: "ten_personal".to_string(),
        principal_id: "prn_1".to_string(),
        target_principal_id: String::new(),
        token_id: "tok_1".to_string(),
        outcome: "succeeded".to_string(),
        reason_code: String::new(),
        created_at: now,
        document: None,
    };

    let saved = store
        .append_tenant_audit_event(event)
        .await
        .expect("append audit event");
    assert!(
        !saved.audit_event_id.is_empty(),
        "store must assign an audit event id"
    );

    let events = store
        .store_handle()
        .lock()
        .list_tenant_audit_events(&AuditEventFilter {
            tenant_id: "ten_personal".to_string(),
            principal_id: String::new(),
            token_id: String::new(),
            event_kind: "tenant.activation_started".to_string(),
            outcome: String::new(),
            limit: 0,
        })
        .expect("list audit events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].audit_event_id, saved.audit_event_id);
    assert_eq!(events[0].event_kind, "tenant.activation_started");
    assert_eq!(events[0].created_at, now);
}

// ---------------------------------------------------------------------------
// BillingProjectorAdapter
// ---------------------------------------------------------------------------

#[tokio::test]
async fn billing_projector_adapter_projects_usage() {
    let billing = Arc::new(kura_billing::Manager::without_repo());
    let adapter = BillingProjectorAdapter::new(billing.clone());

    // Non-hosted tenants fall back to the development plan.
    let summary = adapter
        .usage_summary("ten_dev", false)
        .await
        .expect("development usage summary");
    assert!(!summary.plan_key.is_empty());
    assert!(
        !summary.quotas.is_empty(),
        "development plan projects catalog quotas"
    );

    // Hosted tenants fail closed without a billing repository.
    let err = adapter
        .usage_summary("ten_hosted", true)
        .await
        .expect_err("hosted usage must fail closed");
    assert!(matches!(err, BillingError::QuotaStateUnavailable));
}

// ---------------------------------------------------------------------------
// ChatRunnerAdapter
// ---------------------------------------------------------------------------

#[tokio::test]
async fn chat_runner_adapter_requires_configured_service() {
    let adapter = ChatRunnerAdapter::new(None);
    let failure = adapter
        .run_activation_test_chat(TestChatInput {
            activation_id: "act_1".to_string(),
            principal_id: "prn_1".to_string(),
            tenant_id: "ten_1".to_string(),
            environment_scope: "test".to_string(),
            message: "hello".to_string(),
        })
        .await
        .expect_err("missing chat service must fail");
    assert!(failure.message.contains("chat service is not configured"));
}

#[test]
fn chat_runner_adapter_runs_echo_test_chat() {
    let dispatcher = Arc::new(kura_llm::Dispatcher::new());
    let chat = Arc::new(kura_chat::Service::new_service(
        dispatcher, None, None, None, None,
    ));
    let adapter = ChatRunnerAdapter::new(Some(chat));

    let result = futures::executor::block_on(adapter.run_activation_test_chat(TestChatInput {
        activation_id: "act_1".to_string(),
        principal_id: "prn_1".to_string(),
        tenant_id: "ten_1".to_string(),
        environment_scope: "test".to_string(),
        message: String::new(), // defaults to the safe hosted activation message
    }))
    .expect("echo test chat");
    assert_eq!(result.status, TestChatStatus::COMPLETED);
    assert_eq!(result.provider, "echo");
    assert_eq!(result.model, "echo-v1");
    assert!(!result.dispatch_id.is_empty());
    assert!(result.usage.contains_key("inputTokens"));
    assert!(result.completed_at.is_some());
}

// ---------------------------------------------------------------------------
// Service wiring (Go app wiring parity)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn service_with_sqlite_persists_activation_and_audit() {
    let store = Arc::new(open_store("service_sqlite"));
    let svc = Service::with_sqlite(store.clone(), None, None, "test", true);

    let state = svc
        .activate(ActivateInput {
            token: active_token("tok_hosted", "prn_hosted"),
            tenant_context: TenantContext::default(),
            source: "signup".to_string(),
        })
        .await
        .expect("activate");

    assert_eq!(state.status, Status::ACTIVE);
    assert!(!state.tenant_id.is_empty());

    // Activation state is durable through the SQLite StateStore.
    let persisted = store
        .get_activation_state_for_principal_tenant("prn_hosted", &state.tenant_id)
        .await
        .expect("store")
        .expect("persisted activation state");
    assert_eq!(persisted.activation_id, state.activation_id);

    // Identity records (personal tenant + owner membership + token grant) are durable.
    let tenant = store
        .get_tenant(&state.tenant_id)
        .await
        .expect("get tenant")
        .expect("personal tenant persisted");
    assert_eq!(tenant.default_owner_principal_id, "prn_hosted");
    let memberships = store
        .list_memberships(&MembershipFilter {
            tenant_id: state.tenant_id.clone(),
            status: None,
            role: None,
            limit: 0,
        })
        .await
        .expect("list memberships");
    assert_eq!(memberships.len(), 1);
    assert_eq!(memberships[0].role, Role::Owner);
    let grants = store
        .list_token_tenant_grants("tok_hosted")
        .await
        .expect("list grants");
    assert_eq!(grants.len(), 1);

    // Audit transitions were appended through the SQLite AuditSink.
    let events = store
        .store_handle()
        .lock()
        .list_tenant_audit_events(&AuditEventFilter {
            tenant_id: state.tenant_id.clone(),
            principal_id: String::new(),
            token_id: String::new(),
            event_kind: String::new(),
            outcome: String::new(),
            limit: 0,
        })
        .expect("list audit events");
    let kinds: Vec<&str> = events
        .iter()
        .map(|event| event.event_kind.as_str())
        .collect();
    assert!(kinds.contains(&"tenant.activation_started"), "{kinds:?}");
    assert!(kinds.contains(&"tenant.activation_completed"), "{kinds:?}");

    // Get returns the persisted state.
    let got = svc
        .get(GetInput {
            token: active_token("tok_hosted", "prn_hosted"),
            tenant_context: tenant_context("prn_hosted", &state.tenant_id, "tok_hosted"),
        })
        .await
        .expect("get");
    assert_eq!(got, state);
}

#[tokio::test]
async fn service_with_sqlite_blocks_when_billing_unavailable() {
    let store = Arc::new(open_store("service_blocked"));
    let billing = Arc::new(kura_billing::Manager::without_repo());
    let svc = Service::with_sqlite(
        store,
        Some(Arc::new(BillingProjectorAdapter::new(billing))),
        None,
        "prod",
        true,
    );

    let state = svc
        .activate(ActivateInput {
            token: active_token("tok_blocked", "prn_blocked"),
            tenant_context: TenantContext::default(),
            source: String::new(),
        })
        .await
        .expect("retryable quota blocker must not error");
    assert_eq!(state.status, Status::BLOCKED);
    let baseline = state.quota_baseline.as_ref().expect("quota baseline");
    assert_eq!(baseline.status, QuotaBaselineStatus::UNAVAILABLE);
}

#[test]
fn service_with_sqlite_runs_activation_test_chat() {
    // The kura-chat service bridges into its own current-thread Tokio runtime,
    // so this flow must not run inside a Tokio runtime (matches chat/tests).
    futures::executor::block_on(async {
        let dispatcher = Arc::new(kura_llm::Dispatcher::new());
        let chat = Arc::new(kura_chat::Service::new_service(
            dispatcher, None, None, None, None,
        ));
        let store = Arc::new(open_store("service_chat"));
        let svc = Service::with_sqlite(
            store.clone(),
            None,
            Some(Arc::new(ChatRunnerAdapter::new(Some(chat)))),
            "test",
            true,
        );

        let started = svc
            .activate(ActivateInput {
                token: active_token("tok_chat", "prn_chat"),
                tenant_context: TenantContext::default(),
                source: String::new(),
            })
            .await
            .expect("activate");

        let (completed, metadata) = svc
            .run_test_chat(RunTestChatInput {
                token: active_token("tok_chat", "prn_chat"),
                tenant_context: tenant_context("prn_chat", &started.tenant_id, "tok_chat"),
                message: "Run the activation test".to_string(),
            })
            .await
            .expect("run test chat");

        assert_eq!(completed.status, Status::FIRST_ACTION_COMPLETED);
        assert_eq!(completed.current_step_id, "completed");
        assert!(completed.first_action_completed_at.is_some());
        assert_eq!(metadata.status, TestChatStatus::COMPLETED);
        assert_eq!(metadata.provider, "echo");

        let persisted = store
            .get_activation_state_for_principal_tenant("prn_chat", &started.tenant_id)
            .await
            .expect("store")
            .expect("persisted completion");
        assert_eq!(persisted.status, Status::FIRST_ACTION_COMPLETED);
        assert_eq!(
            persisted.test_chat.as_ref().expect("test chat").dispatch_id,
            metadata.dispatch_id
        );
    });
}

#[tokio::test]
async fn service_with_sqlite_denies_revoked_membership() {
    let now = test_now();
    let store = Arc::new(open_store("service_denied"));
    store
        .upsert_principal(Principal {
            default_tenant_id: "ten_personal".to_string(),
            ..active_principal("prn_revoked", now)
        })
        .await
        .expect("upsert principal");
    store
        .upsert_tenant(Tenant {
            tenant_id: "ten_personal".to_string(),
            tenant_kind: TenantKind::Personal,
            display_name: "Personal tenant".to_string(),
            status: LifecycleStatus::Active,
            created_at: now,
            updated_at: now,
            created_by_principal_id: String::new(),
            default_owner_principal_id: "prn_revoked".to_string(),
            caller_membership_role: None,
            caller_membership_status: None,
            caller_permissions: Vec::new(),
            default_for_current_token: false,
            default_for_current_principal: false,
        })
        .await
        .expect("upsert tenant");
    store
        .upsert_membership(Membership {
            membership_id: "mem_revoked".to_string(),
            tenant_id: "ten_personal".to_string(),
            principal_id: "prn_revoked".to_string(),
            role: Role::Owner,
            status: LifecycleStatus::Removed,
            invitation_id: String::new(),
            created_at: now,
            updated_at: now,
            accepted_at: None,
            removed_at: None,
        })
        .await
        .expect("upsert membership");

    let svc = Service::with_sqlite(store, None, None, "test", true);
    let err = svc
        .activate(ActivateInput {
            token: active_token("tok_active", "prn_revoked"),
            tenant_context: TenantContext::default(),
            source: String::new(),
        })
        .await
        .expect_err("revoked membership must be denied");
    assert_eq!(
        reason_code_from_error(&err),
        ReasonCode::TENANT_ACCESS_REVOKED
    );
}
