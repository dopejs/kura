//! Behavioral tests for the tenant-secrets DAOs (rs/store/src/secrets.rs),
//! ported from daemon/internal/store/secrets.go behavior: create/list/get
//! round-trips, transactional rotate with version superseding, disable, tenant
//! isolation, plus the kura_secrets::Store trait surface through
//! SecretStoreHandle.

use std::sync::Arc;

use chrono::{Duration, Utc};
use kura_secrets::{
    SecretStatus, SecretVersion, SecretVersionStatus, Store, TenantSecret, ValueBackend,
};
use kura_store::{SQLiteStore, SecretStoreHandle};

fn temp_dir(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("kura_store_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.to_string_lossy().to_string()
}

fn secret_fixture(secret_id: &str, tenant_id: &str, secret_ref: &str) -> TenantSecret {
    let now = Utc::now();
    TenantSecret {
        secret_id: secret_id.to_string(),
        tenant_id: tenant_id.to_string(),
        secret_ref: secret_ref.to_string(),
        display_name: format!("secret {secret_ref}"),
        status: SecretStatus::Active,
        active_version_id: format!("{secret_id}_v1"),
        disabled_reason: String::new(),
        remediation_reason: String::new(),
        created_at: now,
        updated_at: now,
        rotated_at: None,
        disabled_at: None,
        document: None,
    }
}

fn version_fixture(secret: &TenantSecret, version_id: &str) -> SecretVersion {
    SecretVersion {
        secret_version_id: version_id.to_string(),
        secret_id: secret.secret_id.clone(),
        tenant_id: secret.tenant_id.clone(),
        secret_ref: secret.secret_ref.clone(),
        version_number: 1,
        status: SecretVersionStatus::Active,
        value_backend_ref: format!("backend/{version_id}"),
        created_at: Utc::now(),
        activated_at: Some(Utc::now()),
        superseded_at: None,
    }
}

#[test]
fn secret_create_get_list_rotate_disable_round_trip() {
    let dir = temp_dir("secrets_roundtrip");
    let store = SQLiteStore::new(&dir).unwrap();

    let mut secret = secret_fixture("sec_1", "ten_1", "slack.token");
    store
        .create_tenant_secret(&secret, &version_fixture(&secret, "sec_1_v1"))
        .unwrap();

    // Get by ref within the tenant.
    let got = store
        .get_secret_by_ref("ten_1", "slack.token")
        .unwrap()
        .expect("present");
    assert_eq!(got.secret_id, "sec_1");
    assert_eq!(got.display_name, "secret slack.token");
    assert_eq!(got.status, SecretStatus::Active);
    assert_eq!(got.active_version_id, "sec_1_v1");
    assert!(
        store
            .get_secret_by_ref("ten_1", "missing.ref")
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .get_secret_by_ref("ten_2", "slack.token")
            .unwrap()
            .is_none()
    );

    // Version lookup.
    let version = store
        .get_secret_version("ten_1", "sec_1_v1")
        .unwrap()
        .expect("version present");
    assert_eq!(version.secret_id, "sec_1");
    assert_eq!(version.version_number, 1);
    assert_eq!(version.value_backend_ref, "backend/sec_1_v1");
    assert_eq!(version.status, SecretVersionStatus::Active);
    assert!(
        store
            .get_secret_version("ten_1", "sec_1_v2")
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .get_secret_version("ten_2", "sec_1_v1")
            .unwrap()
            .is_none()
    );

    // List orders by updated_at DESC.
    let listed = store.list_secrets("ten_1").unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].secret_ref, "slack.token");
    assert!(store.list_secrets("ten_2").unwrap().is_empty());

    // Metadata update.
    secret.display_name = "slack token v2".to_string();
    secret.updated_at = Utc::now();
    store.update_secret_metadata(&secret).unwrap();
    let updated = store
        .get_secret_by_ref("ten_1", "slack.token")
        .unwrap()
        .unwrap();
    assert_eq!(updated.display_name, "slack token v2");

    // Rotate: version_number increments and the prior version is superseded.
    let rotated_secret = TenantSecret {
        active_version_id: "sec_1_v2".to_string(),
        rotated_at: Some(Utc::now()),
        updated_at: Utc::now(),
        ..secret.clone()
    };
    store
        .rotate_tenant_secret(
            &rotated_secret,
            "sec_1_v1",
            version_fixture(&rotated_secret, "sec_1_v2"),
        )
        .unwrap();
    let superseded = store
        .get_secret_version("ten_1", "sec_1_v1")
        .unwrap()
        .unwrap();
    assert_eq!(superseded.status, SecretVersionStatus::Superseded);
    assert!(superseded.superseded_at.is_some());
    let rotated = store
        .get_secret_version("ten_1", "sec_1_v2")
        .unwrap()
        .unwrap();
    assert_eq!(rotated.version_number, 2, "next version allocated");
    assert_eq!(rotated.status, SecretVersionStatus::Active);
    let secret_after = store
        .get_secret_by_ref("ten_1", "slack.token")
        .unwrap()
        .unwrap();
    assert_eq!(secret_after.active_version_id, "sec_1_v2");

    // Disable.
    let disabled = TenantSecret {
        status: SecretStatus::Disabled,
        disabled_reason: "operator_rotated".to_string(),
        disabled_at: Some(Utc::now()),
        updated_at: Utc::now(),
        ..rotated_secret
    };
    store.disable_tenant_secret(&disabled).unwrap();
    let disabled_after = store
        .get_secret_by_ref("ten_1", "slack.token")
        .unwrap()
        .unwrap();
    assert_eq!(disabled_after.status, SecretStatus::Disabled);
    assert_eq!(disabled_after.disabled_reason, "operator_rotated");
    assert!(disabled_after.disabled_at.is_some());
}

#[tokio::test]
async fn secrets_store_trait_round_trip() {
    let dir = temp_dir("secrets_trait");
    let store = SQLiteStore::new(&dir).unwrap();
    let handle = Arc::new(SecretStoreHandle::new(store));

    let secret = secret_fixture("sec_t1", "ten_1", "trait.ref");
    let version = version_fixture(&secret, "sec_t1_v1");
    let manager = kura_secrets::Manager::new(handle.clone(), Arc::new(NoopBackend));

    // Manager.Create is not available with a no-op backend value, so exercise
    // the Store trait directly (metadata only; values stay in the backend).
    handle
        .create_secret(secret.clone(), version.clone())
        .await
        .expect("create via trait");
    let got = handle
        .get_secret_by_ref("ten_1", "trait.ref")
        .await
        .expect("get via trait")
        .expect("present");
    assert_eq!(got.secret_id, "sec_t1");
    let versions = handle.list_secrets("ten_1").await.expect("list via trait");
    assert_eq!(versions.len(), 1);
    assert_eq!(
        handle
            .get_secret_version("ten_1", "sec_t1_v1")
            .await
            .unwrap()
            .unwrap()
            .value_backend_ref,
        "backend/sec_t1_v1"
    );

    // Rotate through the trait.
    let rotated = TenantSecret {
        active_version_id: "sec_t1_v2".to_string(),
        updated_at: Utc::now(),
        rotated_at: Some(Utc::now()),
        ..secret
    };
    handle
        .rotate_secret(
            rotated.clone(),
            "sec_t1_v1",
            version_fixture(&rotated, "sec_t1_v2"),
        )
        .await
        .expect("rotate via trait");
    let old = handle
        .get_secret_version("ten_1", "sec_t1_v1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(old.status, SecretVersionStatus::Superseded);

    // Disable through the trait.
    let disabled = TenantSecret {
        status: SecretStatus::Disabled,
        disabled_reason: "operator_rotated".to_string(),
        disabled_at: Some(Utc::now()),
        updated_at: Utc::now(),
        ..rotated
    };
    handle
        .disable_secret(disabled)
        .await
        .expect("disable via trait");
    let final_state = handle
        .get_secret_by_ref("ten_1", "trait.ref")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(final_state.status, SecretStatus::Disabled);
    let _ = &manager;
}

/// No-op value backend used to satisfy the Manager constructor in tests.
struct NoopBackend;

impl ValueBackend for NoopBackend {
    fn put<'a>(
        &'a self,
        _tenant_id: &'a str,
        _secret_id: &'a str,
        _secret_version_id: &'a str,
        _value: &'a str,
    ) -> kura_secrets::BoxFuture<'a, kura_secrets::Result<String>> {
        Box::pin(async move { Ok("noop".to_string()) })
    }
    fn get<'a>(
        &'a self,
        _backend_ref: &'a str,
    ) -> kura_secrets::BoxFuture<'a, kura_secrets::Result<String>> {
        Box::pin(async move { Ok("value".to_string()) })
    }
    fn delete<'a>(
        &'a self,
        _backend_ref: &'a str,
    ) -> kura_secrets::BoxFuture<'a, kura_secrets::Result<()>> {
        Box::pin(async move { Ok(()) })
    }
}

#[test]
fn secret_tenant_isolation_and_refs() {
    let dir = temp_dir("secrets_isolation");
    let store = SQLiteStore::new(&dir).unwrap();

    let secret_a = secret_fixture("sec_a", "ten_1", "shared.ref");
    store
        .create_tenant_secret(&secret_a, &version_fixture(&secret_a, "sec_a_v1"))
        .unwrap();
    let secret_b = secret_fixture("sec_b", "ten_2", "shared.ref");
    store
        .create_tenant_secret(&secret_b, &version_fixture(&secret_b, "sec_b_v1"))
        .unwrap();

    let a = store
        .get_secret_by_ref("ten_1", "shared.ref")
        .unwrap()
        .unwrap();
    let b = store
        .get_secret_by_ref("ten_2", "shared.ref")
        .unwrap()
        .unwrap();
    assert_ne!(a.secret_id, b.secret_id, "same ref, different tenants");
    assert_eq!(a.tenant_id, "ten_1");
    assert_eq!(b.tenant_id, "ten_2");
    let _ = Duration::hours(1);
}
