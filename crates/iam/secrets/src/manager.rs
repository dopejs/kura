//! Secret lifecycle manager: create/rotate/disable/resolve transitions over
//! a [`Store`] (metadata) plus a [`ValueBackend`](crate::ValueBackend) (raw
//! values). Faithful port of Go `Manager`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use chrono::Utc;

use crate::backend::{ValueBackend, random_hex};
use crate::error::{Result, SecretsError};
use crate::types::{
    CreateDisabledMetadataInput, CreateInput, DisableInput, ResolveInput, ResolvedSecret,
    RotateInput, SecretStatus, SecretVersion, SecretVersionStatus, TenantSecret,
    UpdateMetadataInput,
};

/// Object-safe boxed future used by the traits in this crate (workspace
/// convention for async object-safe interfaces).
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Persistence abstraction for secret metadata (Go `Store` interface,
/// implemented by `internal/store.SQLiteStore`). `kura-store` will provide
/// the SQLite implementation; this crate stays persistence-free.
///
/// Contract notes carried over from the Go SQLite implementation:
/// - `create_secret` persists the secret row and its initial version row.
/// - `rotate_secret` is transactional: it assigns `version.version_number`
///   (`MAX(version_number) + 1` for the secret), marks
///   `previous_version_id` superseded with `superseded_at =
///   secret.updated_at`, inserts the new version, and updates the secret's
///   `active_version_id` / `rotated_at` / `updated_at`.
pub trait Store: Send + Sync {
    fn create_secret<'a>(
        &'a self,
        secret: TenantSecret,
        version: SecretVersion,
    ) -> BoxFuture<'a, Result<()>>;
    fn update_secret_metadata<'a>(&'a self, secret: TenantSecret) -> BoxFuture<'a, Result<()>>;
    fn rotate_secret<'a>(
        &'a self,
        secret: TenantSecret,
        previous_version_id: &'a str,
        version: SecretVersion,
    ) -> BoxFuture<'a, Result<()>>;
    fn disable_secret<'a>(&'a self, secret: TenantSecret) -> BoxFuture<'a, Result<()>>;
    fn get_secret_by_ref<'a>(
        &'a self,
        tenant_id: &'a str,
        secret_ref: &'a str,
    ) -> BoxFuture<'a, Result<Option<TenantSecret>>>;
    fn get_secret_version<'a>(
        &'a self,
        tenant_id: &'a str,
        secret_version_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<SecretVersion>>>;
    fn list_secrets<'a>(&'a self, tenant_id: &'a str) -> BoxFuture<'a, Result<Vec<TenantSecret>>>;
}

/// Secret lifecycle manager (Go `Manager`). Mutations that read-then-write
/// by ref (`create`, `rotate`, `create_disabled_metadata`) serialize on an
/// internal async mutex, matching Go's `sync.Mutex`.
///
/// Unlike the Go struct, `nil` manager/store/backend states are not
/// representable: construction requires both collaborators, so the Go
/// "secret manager is not configured" errors are unrepresentable here.
pub struct Manager {
    store: Arc<dyn Store>,
    backend: Arc<dyn ValueBackend>,
    mu: tokio::sync::Mutex<()>,
}

impl Manager {
    pub fn new(store: Arc<dyn Store>, backend: Arc<dyn ValueBackend>) -> Self {
        Self {
            store,
            backend,
            mu: tokio::sync::Mutex::new(()),
        }
    }

    pub async fn create(&self, input: CreateInput) -> Result<TenantSecret> {
        validate_secret_input(&input.tenant_id, &input.secret_ref, &input.value)?;

        let _guard = self.mu.lock().await;

        if let Some(_existing) = self
            .store
            .get_secret_by_ref(&input.tenant_id, &input.secret_ref)
            .await?
        {
            return Err(SecretsError::SecretAlreadyExists(input.secret_ref.clone()));
        }

        let now = Utc::now();
        let secret_id = format!("sec_{}", random_hex(12));
        let version_id = format!("secver_{}", random_hex(12));
        let backend_ref = self
            .backend
            .put(&input.tenant_id, &secret_id, &version_id, &input.value)
            .await?;
        let secret = TenantSecret {
            secret_id: secret_id.clone(),
            tenant_id: input.tenant_id.clone(),
            secret_ref: input.secret_ref.trim().to_string(),
            display_name: input.display_name.clone(),
            status: SecretStatus::Active,
            active_version_id: version_id.clone(),
            disabled_reason: String::new(),
            remediation_reason: String::new(),
            created_at: now,
            updated_at: now,
            rotated_at: Some(now),
            disabled_at: None,
            document: input.document.clone(),
        };
        let version = SecretVersion {
            secret_version_id: version_id,
            secret_id,
            tenant_id: input.tenant_id.clone(),
            secret_ref: input.secret_ref.trim().to_string(),
            version_number: 1,
            status: SecretVersionStatus::Active,
            value_backend_ref: backend_ref.clone(),
            created_at: now,
            activated_at: Some(now),
            superseded_at: None,
        };
        if let Err(err) = self.store.create_secret(secret.clone(), version).await {
            // Compensate: do not leak the orphaned value file.
            let _ = self.backend.delete(&backend_ref).await;
            return Err(err);
        }
        Ok(secret)
    }

    pub async fn list(&self, tenant_id: &str) -> Result<Vec<TenantSecret>> {
        if tenant_id.is_empty() {
            return Err(SecretsError::TenantRequired);
        }
        self.store.list_secrets(tenant_id).await
    }

    pub async fn get(&self, tenant_id: &str, secret_ref: &str) -> Result<TenantSecret> {
        if tenant_id.is_empty() {
            return Err(SecretsError::TenantRequired);
        }
        if secret_ref.trim().is_empty() {
            return Err(SecretsError::SecretRefRequired);
        }
        self.store
            .get_secret_by_ref(tenant_id, secret_ref)
            .await?
            .ok_or(SecretsError::SecretNotFound)
    }

    pub async fn update_metadata(&self, input: UpdateMetadataInput) -> Result<TenantSecret> {
        if input.tenant_id.is_empty() {
            return Err(SecretsError::TenantRequired);
        }
        if input.secret_ref.trim().is_empty() {
            return Err(SecretsError::SecretRefRequired);
        }
        let mut secret = self
            .store
            .get_secret_by_ref(&input.tenant_id, &input.secret_ref)
            .await?
            .ok_or(SecretsError::SecretNotFound)?;
        if let Some(display_name) = &input.display_name {
            secret.display_name = display_name.clone();
        }
        if input.document.is_some() {
            secret.document = input.document.clone();
        }
        secret.updated_at = Utc::now();
        self.store.update_secret_metadata(secret.clone()).await?;
        Ok(secret)
    }

    pub async fn rotate(&self, input: RotateInput) -> Result<TenantSecret> {
        validate_secret_input(&input.tenant_id, &input.secret_ref, &input.value)?;

        let _guard = self.mu.lock().await;

        let mut secret = self
            .store
            .get_secret_by_ref(&input.tenant_id, &input.secret_ref)
            .await?
            .ok_or(SecretsError::SecretNotFound)?;
        if secret.status != SecretStatus::Active {
            return Err(SecretsError::SecretDisabled);
        }
        let previous_version_id = secret.active_version_id.clone();
        let now = Utc::now();
        let version_id = format!("secver_{}", random_hex(12));
        let backend_ref = self
            .backend
            .put(
                &input.tenant_id,
                &secret.secret_id,
                &version_id,
                &input.value,
            )
            .await?;
        // The store assigns the real version number transactionally (Go:
        // `VersionNumber: 0` here, SQL `MAX(version_number)+1` in RotateSecret).
        let version = SecretVersion {
            secret_version_id: version_id.clone(),
            secret_id: secret.secret_id.clone(),
            tenant_id: secret.tenant_id.clone(),
            secret_ref: secret.secret_ref.clone(),
            version_number: 0,
            status: SecretVersionStatus::Active,
            value_backend_ref: backend_ref.clone(),
            created_at: now,
            activated_at: Some(now),
            superseded_at: None,
        };
        secret.active_version_id = version_id;
        secret.rotated_at = Some(now);
        secret.updated_at = now;
        if let Err(err) = self
            .store
            .rotate_secret(secret.clone(), &previous_version_id, version)
            .await
        {
            let _ = self.backend.delete(&backend_ref).await;
            return Err(err);
        }
        Ok(secret)
    }

    pub async fn disable(&self, input: DisableInput) -> Result<TenantSecret> {
        if input.tenant_id.is_empty() {
            return Err(SecretsError::TenantRequired);
        }
        if input.secret_ref.trim().is_empty() {
            return Err(SecretsError::SecretRefRequired);
        }
        let mut secret = self
            .store
            .get_secret_by_ref(&input.tenant_id, &input.secret_ref)
            .await?
            .ok_or(SecretsError::SecretNotFound)?;
        let now = Utc::now();
        secret.status = SecretStatus::Disabled;
        secret.disabled_reason = input.disabled_reason.clone();
        secret.disabled_at = Some(now);
        secret.updated_at = now;
        self.store.disable_secret(secret.clone()).await?;
        Ok(secret)
    }

    pub async fn create_disabled_metadata(
        &self,
        input: CreateDisabledMetadataInput,
    ) -> Result<TenantSecret> {
        if input.tenant_id.is_empty() {
            return Err(SecretsError::TenantRequired);
        }
        if input.secret_ref.trim().is_empty() {
            return Err(SecretsError::SecretRefRequired);
        }

        let _guard = self.mu.lock().await;

        if let Some(_existing) = self
            .store
            .get_secret_by_ref(&input.tenant_id, &input.secret_ref)
            .await?
        {
            return Err(SecretsError::SecretAlreadyExists(input.secret_ref.clone()));
        }

        let now = Utc::now();
        let secret_id = format!("sec_{}", random_hex(12));
        let version_id = format!("secver_{}", random_hex(12));
        let secret = TenantSecret {
            secret_id: secret_id.clone(),
            tenant_id: input.tenant_id.clone(),
            secret_ref: input.secret_ref.trim().to_string(),
            display_name: input.display_name.clone(),
            status: SecretStatus::PendingRemediation,
            active_version_id: version_id.clone(),
            disabled_reason: input.disabled_reason.clone(),
            remediation_reason: input.remediation_reason.clone(),
            created_at: now,
            updated_at: now,
            rotated_at: None,
            disabled_at: Some(now),
            document: input.document.clone(),
        };
        let version = SecretVersion {
            secret_version_id: version_id,
            secret_id,
            tenant_id: input.tenant_id.clone(),
            secret_ref: input.secret_ref.trim().to_string(),
            version_number: 1,
            status: SecretVersionStatus::PendingRemediation,
            value_backend_ref: String::new(),
            created_at: now,
            activated_at: None,
            superseded_at: None,
        };
        self.store.create_secret(secret.clone(), version).await?;
        Ok(secret)
    }

    /// Resolves a tenant secret ref to its raw value. The returned
    /// [`ResolvedSecret`] never serializes or debug-prints the value.
    pub async fn resolve(&self, input: ResolveInput) -> Result<ResolvedSecret> {
        if input.tenant_id.is_empty() {
            return Err(SecretsError::TenantRequired);
        }
        if input.secret_ref.trim().is_empty() {
            return Err(SecretsError::SecretRefRequired);
        }
        let secret = self
            .store
            .get_secret_by_ref(&input.tenant_id, &input.secret_ref)
            .await?
            .ok_or(SecretsError::SecretNotFound)?;
        if secret.tenant_id != input.tenant_id {
            return Err(SecretsError::CrossTenantSecret);
        }
        if secret.status != SecretStatus::Active {
            return Err(SecretsError::SecretDisabled);
        }
        let version = self
            .store
            .get_secret_version(&input.tenant_id, &secret.active_version_id)
            .await?
            .ok_or(SecretsError::SecretVersionNotFound)?;
        let value = self.backend.get(&version.value_backend_ref).await?;
        Ok(ResolvedSecret {
            tenant_id: input.tenant_id.clone(),
            secret_id: secret.secret_id,
            secret_ref: secret.secret_ref,
            secret_version_id: version.secret_version_id,
            resolution: crate::types::ResolutionStatus::Resolved,
            value,
            resolved_at: Utc::now(),
        })
    }
}

fn validate_secret_input(tenant_id: &str, secret_ref: &str, value: &str) -> Result<()> {
    if tenant_id.is_empty() {
        return Err(SecretsError::TenantRequired);
    }
    if secret_ref.trim().is_empty() {
        return Err(SecretsError::SecretRefRequired);
    }
    if value.is_empty() {
        return Err(SecretsError::SecretValueRequired);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::LocalBackend;
    use crate::testutil::{FakeStore, TestDir};
    use crate::types::{Document, ResolutionStatus};

    fn manager(dir_suffix: &str) -> (Arc<Manager>, Arc<FakeStore>, Arc<LocalBackend>, TestDir) {
        let store_dir = TestDir::new(&format!("store-{dir_suffix}"));
        let backend_dir = TestDir::new(&format!("backend-{dir_suffix}"));
        let store = Arc::new(FakeStore::new());
        let backend = Arc::new(LocalBackend::new(backend_dir.path()).expect("local backend"));
        let manager = Arc::new(Manager::new(store.clone(), backend.clone()));
        (manager, store, backend, store_dir)
    }

    #[tokio::test]
    async fn rotation_keeps_prior_version_snapshot() {
        let (manager, store, backend, _dir) = manager("rotate");
        let created = manager
            .create(CreateInput {
                tenant_id: "ten_r37_a".into(),
                secret_ref: "service/token".into(),
                value: "old-token".into(),
                ..CreateInput::default()
            })
            .await
            .expect("create secret");
        let old_version_id = created.active_version_id.clone();

        let rotated = manager
            .rotate(RotateInput {
                tenant_id: "ten_r37_a".into(),
                secret_ref: "service/token".into(),
                value: "new-token".into(),
            })
            .await
            .expect("rotate secret");
        assert_ne!(rotated.active_version_id, old_version_id);

        let resolved = manager
            .resolve(ResolveInput {
                tenant_id: "ten_r37_a".into(),
                secret_ref: "service/token".into(),
            })
            .await
            .expect("resolve rotated secret");
        assert_eq!(resolved.value, "new-token");
        assert_eq!(resolved.resolution, ResolutionStatus::Resolved);

        let old_version = store
            .get_secret_version("ten_r37_a", &old_version_id)
            .await
            .expect("get old version")
            .expect("old version missing after rotation");
        assert_eq!(old_version.status, SecretVersionStatus::Superseded);
        assert!(old_version.superseded_at.is_some());
        let new_version = store
            .get_secret_version("ten_r37_a", &rotated.active_version_id)
            .await
            .expect("get new version")
            .expect("new version missing");
        assert_eq!(new_version.version_number, 2);

        let old_value = backend
            .get(&old_version.value_backend_ref)
            .await
            .expect("get old backend value");
        assert_eq!(old_value, "old-token");
    }

    #[tokio::test]
    async fn cross_tenant_isolation() {
        let (manager, _store, _backend, _dir) = manager("isolation");
        manager
            .create(CreateInput {
                tenant_id: "ten_r37_a".into(),
                secret_ref: "shared/ref".into(),
                value: "tenant-a".into(),
                ..CreateInput::default()
            })
            .await
            .expect("create tenant A");
        manager
            .create(CreateInput {
                tenant_id: "ten_r37_b".into(),
                secret_ref: "shared/ref".into(),
                value: "tenant-b".into(),
                ..CreateInput::default()
            })
            .await
            .expect("create tenant B same ref");
        manager
            .create(CreateInput {
                tenant_id: "ten_r37_a".into(),
                secret_ref: "tenant-a-only".into(),
                value: "private-a".into(),
                ..CreateInput::default()
            })
            .await
            .expect("create tenant A private ref");

        let resolved_a = manager
            .resolve(ResolveInput {
                tenant_id: "ten_r37_a".into(),
                secret_ref: "shared/ref".into(),
            })
            .await
            .expect("resolve tenant A");
        let resolved_b = manager
            .resolve(ResolveInput {
                tenant_id: "ten_r37_b".into(),
                secret_ref: "shared/ref".into(),
            })
            .await
            .expect("resolve tenant B");
        assert_eq!(resolved_a.value, "tenant-a");
        assert_eq!(resolved_b.value, "tenant-b");

        let err = manager
            .rotate(RotateInput {
                tenant_id: "ten_r37_b".into(),
                secret_ref: "tenant-a-only".into(),
                value: "x".into(),
            })
            .await
            .expect_err("rotate missing cross-tenant ref");
        assert_eq!(err, SecretsError::SecretNotFound);
        let err = manager
            .disable(DisableInput {
                tenant_id: "ten_r37_b".into(),
                secret_ref: "tenant-a-only".into(),
                disabled_reason: "test".into(),
            })
            .await
            .expect_err("disable missing cross-tenant ref");
        assert_eq!(err, SecretsError::SecretNotFound);
    }

    #[tokio::test]
    async fn validation_and_lifecycle_edges() {
        let (manager, _store, _backend, _dir) = manager("lifecycle");

        // Validation (Go validateSecretInput / per-method checks).
        assert_eq!(
            manager
                .create(CreateInput {
                    tenant_id: "".into(),
                    secret_ref: "r".into(),
                    value: "v".into(),
                    ..CreateInput::default()
                })
                .await
                .expect_err("tenant required"),
            SecretsError::TenantRequired
        );
        assert_eq!(
            manager
                .create(CreateInput {
                    tenant_id: "ten_1".into(),
                    secret_ref: "   ".into(),
                    value: "v".into(),
                    ..CreateInput::default()
                })
                .await
                .expect_err("ref required"),
            SecretsError::SecretRefRequired
        );
        assert_eq!(
            manager
                .create(CreateInput {
                    tenant_id: "ten_1".into(),
                    secret_ref: "r".into(),
                    value: "".into(),
                    ..CreateInput::default()
                })
                .await
                .expect_err("value required"),
            SecretsError::SecretValueRequired
        );

        // Create stores the trimmed ref; duplicates (looked up by the exact
        // stored ref, matching the SQLite `secret_ref = ?` lookup) are rejected.
        let created = manager
            .create(CreateInput {
                tenant_id: "ten_1".into(),
                secret_ref: " svc/token ".into(),
                display_name: "Service token".into(),
                value: "v1".into(),
                document: Some(Document::from_iter([(
                    "source".to_string(),
                    serde_json::Value::String("test".to_string()),
                )])),
            })
            .await
            .expect("create");
        assert_eq!(created.secret_ref, "svc/token");
        assert_eq!(created.status, SecretStatus::Active);
        assert!(created.rotated_at.is_some());
        assert_eq!(
            manager
                .create(CreateInput {
                    tenant_id: "ten_1".into(),
                    secret_ref: "svc/token".into(),
                    value: "v2".into(),
                    ..CreateInput::default()
                })
                .await
                .expect_err("duplicate create"),
            SecretsError::SecretAlreadyExists("svc/token".into())
        );

        // Update metadata: None fields unchanged, Some applied.
        let updated = manager
            .update_metadata(UpdateMetadataInput {
                tenant_id: "ten_1".into(),
                secret_ref: "svc/token".into(),
                display_name: Some("Renamed".into()),
                document: None,
            })
            .await
            .expect("update metadata");
        assert_eq!(updated.display_name, "Renamed");
        assert!(updated.document.is_some());

        // Disable blocks rotation and resolution but not metadata reads.
        let disabled = manager
            .disable(DisableInput {
                tenant_id: "ten_1".into(),
                secret_ref: "svc/token".into(),
                disabled_reason: "compromised".into(),
            })
            .await
            .expect("disable");
        assert_eq!(disabled.status, SecretStatus::Disabled);
        assert_eq!(disabled.disabled_reason, "compromised");
        assert!(disabled.disabled_at.is_some());
        assert_eq!(
            manager
                .resolve(ResolveInput {
                    tenant_id: "ten_1".into(),
                    secret_ref: "svc/token".into(),
                })
                .await
                .expect_err("disabled resolve"),
            SecretsError::SecretDisabled
        );
        assert_eq!(
            manager
                .rotate(RotateInput {
                    tenant_id: "ten_1".into(),
                    secret_ref: "svc/token".into(),
                    value: "v3".into(),
                })
                .await
                .expect_err("disabled rotate"),
            SecretsError::SecretDisabled
        );
        let fetched = manager
            .get("ten_1", "svc/token")
            .await
            .expect("get disabled");
        assert_eq!(fetched.status, SecretStatus::Disabled);
        let listed = manager.list("ten_1").await.expect("list");
        assert_eq!(listed.len(), 1);
    }

    #[tokio::test]
    async fn create_disabled_metadata_creates_pending_remediation() {
        let (manager, _store, _backend, _dir) = manager("pending");
        let secret = manager
            .create_disabled_metadata(CreateDisabledMetadataInput {
                tenant_id: "ten_1".into(),
                secret_ref: "legacy/ref".into(),
                display_name: "legacy/ref".into(),
                disabled_reason: "legacy_secret_ref_conflict".into(),
                remediation_reason: "rotate manually".into(),
                document: None,
            })
            .await
            .expect("create disabled metadata");
        assert_eq!(secret.status, SecretStatus::PendingRemediation);
        assert!(secret.disabled_at.is_some());
        assert!(secret.rotated_at.is_none());
        assert_eq!(
            manager
                .resolve(ResolveInput {
                    tenant_id: "ten_1".into(),
                    secret_ref: "legacy/ref".into(),
                })
                .await
                .expect_err("pending remediation must not resolve"),
            SecretsError::SecretDisabled
        );
        assert_eq!(
            manager
                .create_disabled_metadata(CreateDisabledMetadataInput {
                    tenant_id: "ten_1".into(),
                    secret_ref: "legacy/ref".into(),
                    display_name: String::new(),
                    disabled_reason: String::new(),
                    remediation_reason: String::new(),
                    document: None,
                })
                .await
                .expect_err("duplicate disabled metadata"),
            SecretsError::SecretAlreadyExists("legacy/ref".into())
        );
    }

    #[tokio::test]
    async fn resolved_secret_never_serializes_or_debugs_value() {
        let (manager, _store, _backend, _dir) = manager("redact");
        manager
            .create(CreateInput {
                tenant_id: "ten_1".into(),
                secret_ref: "svc/token".into(),
                value: "R37_FAKE_SECRET_DO_NOT_LEAK".into(),
                ..CreateInput::default()
            })
            .await
            .expect("create");
        let resolved = manager
            .resolve(ResolveInput {
                tenant_id: "ten_1".into(),
                secret_ref: "svc/token".into(),
            })
            .await
            .expect("resolve");
        assert_eq!(resolved.value, "R37_FAKE_SECRET_DO_NOT_LEAK");

        let json = serde_json::to_string(&resolved).expect("serialize resolved");
        assert!(!json.contains("R37_FAKE_SECRET_DO_NOT_LEAK"));
        assert!(!json.contains("value"));
        let debug = format!("{resolved:?}");
        assert!(!debug.contains("R37_FAKE_SECRET_DO_NOT_LEAK"));
        assert!(debug.contains("[REDACTED]"));

        // Inputs carrying raw values redact in Debug as well.
        let debug = format!(
            "{:?}",
            CreateInput {
                tenant_id: "ten_1".into(),
                secret_ref: "r".into(),
                value: "RAW_VALUE_DO_NOT_LEAK".into(),
                ..CreateInput::default()
            }
        );
        assert!(!debug.contains("RAW_VALUE_DO_NOT_LEAK"));
        let debug = format!(
            "{:?}",
            RotateInput {
                tenant_id: "ten_1".into(),
                secret_ref: "r".into(),
                value: "RAW_VALUE_DO_NOT_LEAK".into(),
            }
        );
        assert!(!debug.contains("RAW_VALUE_DO_NOT_LEAK"));

        // The persisted metadata rows must not contain the value either.
        let secret = manager.get("ten_1", "svc/token").await.expect("get");
        let json = serde_json::to_string(&secret).expect("serialize secret");
        assert!(!json.contains("R37_FAKE_SECRET_DO_NOT_LEAK"));
    }
}
