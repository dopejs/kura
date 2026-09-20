//! Round-trip tests for the identity CRUD port (rs/store/src/identity.rs). Each test
//! opens a fresh store in a unique temp dir, upserts a domain record, lists/gets it
//! back, and asserts the persisted key fields.

use chrono::Utc;
use kura_identity::auth::{AccessToken, Pairing, PairingMode, PairingStatus, TokenStatus};
use kura_identity::{
    AuditEventFilter, InvitationFilter, LifecycleStatus, Membership, MembershipFilter, Principal,
    PrincipalFilter, PrincipalKind, Role, Tenant, TenantAuditEvent, TenantFilter, TenantInvitation,
    TenantKind, TokenTenantGrant,
};
use kura_store::SQLiteStore;

fn temp_dir(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("kura_store_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.to_string_lossy().to_string()
}

fn make_pairing() -> Pairing {
    let now = Utc::now();
    Pairing {
        pairing_id: "pair_1".to_string(),
        mode: PairingMode::Local,
        label: "web-ui".to_string(),
        status: PairingStatus::Pending,
        code_hash: "hash123".to_string(),
        code_preview: "123456".to_string(),
        created_at: now,
        updated_at: now,
        expires_at: now + chrono::Duration::minutes(10),
        completed_at: None,
    }
}

fn make_token() -> AccessToken {
    let now = Utc::now();
    AccessToken {
        token_id: "tok_1".to_string(),
        principal_id: "prn_1".to_string(),
        label: "automation".to_string(),
        mode: PairingMode::Token,
        token_hash: "tokhash".to_string(),
        token_preview: "kura_preview".to_string(),
        status: TokenStatus::Active,
        default_tenant_id: "ten_1".to_string(),
        created_at: now,
        updated_at: now,
        last_used_at: None,
        expires_at: Some(now + chrono::Duration::hours(1)),
        revoked_at: None,
        rotated_from_token_id: String::new(),
        rotated_to_token_id: String::new(),
    }
}

fn make_tenant(tenant_id: &str, kind: TenantKind) -> Tenant {
    let now = Utc::now();
    Tenant {
        tenant_id: tenant_id.to_string(),
        tenant_kind: kind,
        display_name: format!("tenant {tenant_id}"),
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
    }
}

fn make_principal(principal_id: &str) -> Principal {
    let now = Utc::now();
    Principal {
        principal_id: principal_id.to_string(),
        principal_kind: PrincipalKind::User,
        display_name: format!("principal {principal_id}"),
        status: LifecycleStatus::Active,
        default_tenant_id: "ten_1".to_string(),
        created_at: now,
        updated_at: now,
        disabled_at: None,
        removed_at: None,
    }
}

#[test]
fn pairing_round_trips_through_sqlite() {
    let dir = temp_dir("identity_pairing");
    let store = SQLiteStore::new(&dir).unwrap();

    let mut pairing = make_pairing();
    store.upsert_pairing(&pairing).unwrap();

    let listed = store.list_pairings().unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.pairing_id, "pair_1");
    assert_eq!(got.mode, PairingMode::Local);
    assert_eq!(got.label, "web-ui");
    assert_eq!(got.status, PairingStatus::Pending);
    assert_eq!(got.code_hash, "hash123");
    assert_eq!(got.code_preview, "123456");
    assert!(got.completed_at.is_none());

    // Upsert again with a changed status exercises the ON CONFLICT update path.
    pairing.status = PairingStatus::Completed;
    pairing.completed_at = Some(Utc::now());
    pairing.code_preview = String::new();
    store.upsert_pairing(&pairing).unwrap();
    let updated = store.list_pairings().unwrap();
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0].status, PairingStatus::Completed);
    assert_eq!(updated[0].code_preview, "");
    assert!(updated[0].completed_at.is_some());
}

#[test]
fn access_token_and_authority_round_trip() {
    let dir = temp_dir("identity_token");
    let store = SQLiteStore::new(&dir).unwrap();

    let token = make_token();
    store.upsert_access_token(&token).unwrap();

    let listed = store.list_access_tokens().unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.token_id, "tok_1");
    assert_eq!(got.principal_id, "prn_1");
    assert_eq!(got.label, "automation");
    assert_eq!(got.mode, PairingMode::Token);
    assert_eq!(got.token_hash, "tokhash");
    assert_eq!(got.token_preview, "kura_preview");
    assert_eq!(got.status, TokenStatus::Active);
    assert_eq!(got.default_tenant_id, "ten_1");
    assert!(got.expires_at.is_some());
    assert!(got.rotated_from_token_id.is_empty());
    assert!(got.rotated_to_token_id.is_empty());

    let authorities = store.list_token_authorities().unwrap();
    assert_eq!(authorities.len(), 1);
    let authority = &authorities[0];
    assert_eq!(authority.token_id, "tok_1");
    assert_eq!(authority.principal_id, "prn_1");
    assert_eq!(authority.default_tenant_id, "ten_1");
    assert_eq!(authority.status, LifecycleStatus::Active);
    assert_eq!(authority.expires_at, token.expires_at);

    // A revoked token surfaces as a revoked authority.
    let mut revoked = make_token();
    revoked.token_id = "tok_2".to_string();
    revoked.status = TokenStatus::Revoked;
    revoked.revoked_at = Some(Utc::now());
    store.upsert_access_token(&revoked).unwrap();
    let authorities = store.list_token_authorities().unwrap();
    assert_eq!(authorities.len(), 2);
    assert_eq!(authorities[1].status, LifecycleStatus::Revoked);
}

#[test]
fn tenant_round_trips_and_filters() {
    let dir = temp_dir("identity_tenant");
    let store = SQLiteStore::new(&dir).unwrap();

    store
        .upsert_tenant(&make_tenant("ten_1", TenantKind::Personal))
        .unwrap();
    store
        .upsert_tenant(&make_tenant("ten_2", TenantKind::Organization))
        .unwrap();

    let got = store.get_tenant("ten_1").unwrap().expect("tenant present");
    assert_eq!(got.tenant_id, "ten_1");
    assert_eq!(got.tenant_kind, TenantKind::Personal);
    assert_eq!(got.status, LifecycleStatus::Active);
    assert_eq!(got.created_by_principal_id, "prn_1");
    assert_eq!(got.default_owner_principal_id, "prn_1");
    assert!(store.get_tenant("missing").unwrap().is_none());

    let all = store.list_tenants(&TenantFilter::default()).unwrap();
    assert_eq!(all.len(), 2);

    let orgs = store
        .list_tenants(&TenantFilter {
            tenant_kind: Some(TenantKind::Organization),
            status: None,
            limit: 0,
        })
        .unwrap();
    assert_eq!(orgs.len(), 1);
    assert_eq!(orgs[0].tenant_id, "ten_2");

    let active = store
        .list_tenants(&TenantFilter {
            tenant_kind: None,
            status: Some(LifecycleStatus::Active),
            limit: 1,
        })
        .unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].tenant_id, "ten_1");
}

#[test]
fn principal_round_trips_and_membership_join_filter() {
    let dir = temp_dir("identity_principal");
    let store = SQLiteStore::new(&dir).unwrap();

    store
        .upsert_tenant(&make_tenant("ten_1", TenantKind::Personal))
        .unwrap();
    let principal = make_principal("prn_1");
    store.upsert_principal(&principal).unwrap();
    store.upsert_principal(&make_principal("prn_2")).unwrap();

    let got = store
        .get_principal("prn_1")
        .unwrap()
        .expect("principal present");
    assert_eq!(got.principal_id, "prn_1");
    assert_eq!(got.principal_kind, PrincipalKind::User);
    assert_eq!(got.display_name, "principal prn_1");
    assert_eq!(got.status, LifecycleStatus::Active);
    assert_eq!(got.default_tenant_id, "ten_1");
    assert!(store.get_principal("missing").unwrap().is_none());

    let all = store.list_principals(&PrincipalFilter::default()).unwrap();
    assert_eq!(all.len(), 2);

    // Only prn_1 holds a membership in ten_1, so the join filter narrows to it.
    store
        .upsert_membership(&Membership {
            membership_id: "mem_1".to_string(),
            tenant_id: "ten_1".to_string(),
            principal_id: "prn_1".to_string(),
            role: Role::Owner,
            status: LifecycleStatus::Active,
            invitation_id: String::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            accepted_at: None,
            removed_at: None,
        })
        .unwrap();
    let in_tenant = store
        .list_principals(&PrincipalFilter {
            tenant_id: "ten_1".to_string(),
            status: Some(LifecycleStatus::Active),
            limit: 0,
        })
        .unwrap();
    assert_eq!(in_tenant.len(), 1);
    assert_eq!(in_tenant[0].principal_id, "prn_1");
}

#[test]
fn membership_round_trips_through_sqlite() {
    let dir = temp_dir("identity_membership");
    let store = SQLiteStore::new(&dir).unwrap();

    let membership = Membership {
        membership_id: "mem_1".to_string(),
        tenant_id: "ten_1".to_string(),
        principal_id: "prn_1".to_string(),
        role: Role::Admin,
        status: LifecycleStatus::Active,
        invitation_id: "inv_1".to_string(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        accepted_at: Some(Utc::now()),
        removed_at: None,
    };
    store.upsert_membership(&membership).unwrap();

    let listed = store
        .list_memberships(&MembershipFilter {
            tenant_id: "ten_1".to_string(),
            status: Some(LifecycleStatus::Active),
            role: Some(Role::Admin),
            limit: 0,
        })
        .unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.membership_id, "mem_1");
    assert_eq!(got.tenant_id, "ten_1");
    assert_eq!(got.principal_id, "prn_1");
    assert_eq!(got.role, Role::Admin);
    assert_eq!(got.status, LifecycleStatus::Active);
    assert_eq!(got.invitation_id, "inv_1");
    assert!(got.accepted_at.is_some());
    assert!(got.removed_at.is_none());

    // Role filter excludes the record.
    let viewers = store
        .list_memberships(&MembershipFilter {
            tenant_id: String::new(),
            status: None,
            role: Some(Role::Viewer),
            limit: 0,
        })
        .unwrap();
    assert!(viewers.is_empty());
}

#[test]
fn tenant_invitation_round_trips_through_sqlite() {
    let dir = temp_dir("identity_invitation");
    let store = SQLiteStore::new(&dir).unwrap();

    let invitation = TenantInvitation {
        invitation_id: "inv_1".to_string(),
        tenant_id: "ten_1".to_string(),
        invited_principal_id: "prn_2".to_string(),
        invited_by_principal_id: "prn_1".to_string(),
        role: Role::Admin,
        status: LifecycleStatus::Invited,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        expires_at: Some(Utc::now() + chrono::Duration::days(7)),
        decided_at: None,
    };
    store.upsert_tenant_invitation(&invitation).unwrap();

    let listed = store
        .list_tenant_invitations(&InvitationFilter {
            tenant_id: "ten_1".to_string(),
            principal_id: "prn_2".to_string(),
            status: Some(LifecycleStatus::Invited),
            limit: 0,
        })
        .unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.invitation_id, "inv_1");
    assert_eq!(got.invited_principal_id, "prn_2");
    assert_eq!(got.invited_by_principal_id, "prn_1");
    assert_eq!(got.role, Role::Admin);
    assert_eq!(got.status, LifecycleStatus::Invited);
    assert!(got.expires_at.is_some());
    assert!(got.decided_at.is_none());
}

#[test]
fn token_tenant_grant_round_trips_through_sqlite() {
    let dir = temp_dir("identity_grant");
    let store = SQLiteStore::new(&dir).unwrap();

    let grant = TokenTenantGrant {
        grant_id: "grant_1".to_string(),
        token_id: "tok_1".to_string(),
        tenant_id: "ten_1".to_string(),
        is_default: true,
        status: LifecycleStatus::Active,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        revoked_at: None,
        granted_by_principal_id: "prn_1".to_string(),
    };
    store.upsert_token_tenant_grant(&grant).unwrap();

    let listed = store.list_token_tenant_grants("tok_1").unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.grant_id, "grant_1");
    assert_eq!(got.tenant_id, "ten_1");
    assert_eq!(got.is_default, true);
    assert_eq!(got.status, LifecycleStatus::Active);
    assert_eq!(got.granted_by_principal_id, "prn_1");
    assert!(
        store
            .list_token_tenant_grants("tok_missing")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn tenant_audit_event_round_trips_through_sqlite() {
    let dir = temp_dir("identity_audit");
    let store = SQLiteStore::new(&dir).unwrap();

    let mut document = serde_json::Map::new();
    document.insert("resource".to_string(), serde_json::json!("ten_1"));
    document.insert("count".to_string(), serde_json::json!(3));
    let mut event = TenantAuditEvent {
        audit_event_id: String::new(),
        event_kind: "membership.updated".to_string(),
        tenant_id: "ten_1".to_string(),
        principal_id: "prn_1".to_string(),
        target_principal_id: "prn_2".to_string(),
        token_id: "tok_1".to_string(),
        outcome: "succeeded".to_string(),
        reason_code: "role_changed".to_string(),
        created_at: chrono::DateTime::<chrono::Utc>::UNIX_EPOCH,
        document: Some(document),
    };
    let stored = store.append_tenant_audit_event(&event).unwrap();
    // Missing id and zero created_at are filled in, mirroring the Go port.
    assert!(stored.audit_event_id.starts_with("audit_"));
    assert_ne!(
        stored.created_at,
        chrono::DateTime::<chrono::Utc>::UNIX_EPOCH
    );
    event.audit_event_id = stored.audit_event_id.clone();

    let listed = store
        .list_tenant_audit_events(&AuditEventFilter {
            tenant_id: "ten_1".to_string(),
            principal_id: String::new(),
            token_id: String::new(),
            event_kind: "membership.updated".to_string(),
            outcome: String::new(),
            limit: 0,
        })
        .unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.audit_event_id, event.audit_event_id);
    assert_eq!(got.event_kind, "membership.updated");
    assert_eq!(got.tenant_id, "ten_1");
    assert_eq!(got.principal_id, "prn_1");
    assert_eq!(got.target_principal_id, "prn_2");
    assert_eq!(got.token_id, "tok_1");
    assert_eq!(got.outcome, "succeeded");
    assert_eq!(got.reason_code, "role_changed");
    let document = got.document.as_ref().expect("document persisted");
    assert_eq!(document.get("resource"), Some(&serde_json::json!("ten_1")));
    assert_eq!(document.get("count"), Some(&serde_json::json!(3)));
}
