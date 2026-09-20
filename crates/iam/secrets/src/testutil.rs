//! In-memory test doubles for the persistence traits. `FakeStore` mirrors
//! the Go `SQLiteStore` semantics the package tests exercise (notably
//! transactional rotate: version numbering and supersede). `kura-store`
//! (wave 5) owns the real SQLite implementation.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;

use crate::bridge::BridgeProgressStore;
use crate::error::{Result, SecretsError};
use crate::manager::{BoxFuture, Store};
use crate::types::{SecretVersion, SecretVersionStatus, TenantSecret};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Unique temp directory removed on drop (Go `t.TempDir()`).
pub(crate) struct TestDir {
    path: PathBuf,
}

impl TestDir {
    pub(crate) fn new(label: &str) -> Self {
        let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "kura-secrets-test-{}-{unique}-{label}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create test dir");
        Self { path }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// In-memory `Store` mirroring `SQLiteStore` secret semantics.
pub(crate) struct FakeStore {
    secrets: Mutex<HashMap<(String, String), TenantSecret>>,
    versions: Mutex<HashMap<(String, String), SecretVersion>>,
}

impl FakeStore {
    pub(crate) fn new() -> Self {
        Self {
            secrets: Mutex::new(HashMap::new()),
            versions: Mutex::new(HashMap::new()),
        }
    }
}

impl Store for FakeStore {
    fn create_secret<'a>(
        &'a self,
        secret: TenantSecret,
        version: SecretVersion,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.secrets.lock().insert(
                (secret.tenant_id.clone(), secret.secret_ref.clone()),
                secret,
            );
            self.versions.lock().insert(
                (version.tenant_id.clone(), version.secret_version_id.clone()),
                version,
            );
            Ok(())
        })
    }

    fn update_secret_metadata<'a>(&'a self, secret: TenantSecret) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.secrets.lock().insert(
                (secret.tenant_id.clone(), secret.secret_ref.clone()),
                secret,
            );
            Ok(())
        })
    }

    fn rotate_secret<'a>(
        &'a self,
        secret: TenantSecret,
        previous_version_id: &'a str,
        mut version: SecretVersion,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let mut versions = self.versions.lock();
            // SQL: SELECT COALESCE(MAX(version_number), 0) + 1 ...
            let next = versions
                .values()
                .filter(|v| v.tenant_id == secret.tenant_id && v.secret_id == secret.secret_id)
                .map(|v| v.version_number)
                .max()
                .unwrap_or(0)
                + 1;
            version.version_number = next;
            if !previous_version_id.is_empty() {
                if let Some(previous) =
                    versions.get_mut(&(secret.tenant_id.clone(), previous_version_id.to_string()))
                {
                    previous.status = SecretVersionStatus::Superseded;
                    previous.superseded_at = Some(secret.updated_at);
                }
            }
            versions.insert(
                (version.tenant_id.clone(), version.secret_version_id.clone()),
                version,
            );
            drop(versions);
            self.secrets.lock().insert(
                (secret.tenant_id.clone(), secret.secret_ref.clone()),
                secret,
            );
            Ok(())
        })
    }

    fn disable_secret<'a>(&'a self, secret: TenantSecret) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.secrets.lock().insert(
                (secret.tenant_id.clone(), secret.secret_ref.clone()),
                secret,
            );
            Ok(())
        })
    }

    fn get_secret_by_ref<'a>(
        &'a self,
        tenant_id: &'a str,
        secret_ref: &'a str,
    ) -> BoxFuture<'a, Result<Option<TenantSecret>>> {
        Box::pin(async move {
            Ok(self
                .secrets
                .lock()
                .get(&(tenant_id.to_string(), secret_ref.to_string()))
                .cloned())
        })
    }

    fn get_secret_version<'a>(
        &'a self,
        tenant_id: &'a str,
        secret_version_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<SecretVersion>>> {
        Box::pin(async move {
            Ok(self
                .versions
                .lock()
                .get(&(tenant_id.to_string(), secret_version_id.to_string()))
                .cloned())
        })
    }

    fn list_secrets<'a>(&'a self, tenant_id: &'a str) -> BoxFuture<'a, Result<Vec<TenantSecret>>> {
        Box::pin(async move {
            let mut items: Vec<TenantSecret> = self
                .secrets
                .lock()
                .values()
                .filter(|secret| secret.tenant_id == tenant_id)
                .cloned()
                .collect();
            items.sort_by(|a, b| a.secret_ref.cmp(&b.secret_ref));
            Ok(items)
        })
    }
}

#[derive(Default)]
struct ProgressStep {
    running: bool,
    completed: bool,
    chunks: Vec<String>,
}

/// In-memory `BridgeProgressStore`: fresh begin → `false` (not a resume);
/// begin after begin without complete → `true` (resume).
pub(crate) struct FakeProgressStore {
    steps: Mutex<HashMap<String, ProgressStep>>,
}

impl FakeProgressStore {
    pub(crate) fn new() -> Self {
        Self {
            steps: Mutex::new(HashMap::new()),
        }
    }
}

impl BridgeProgressStore for FakeProgressStore {
    fn register_migration_step<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.steps.lock().entry(name.to_string()).or_default();
            Ok(())
        })
    }

    fn is_migration_step_completed<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            Ok(self
                .steps
                .lock()
                .get(name)
                .map(|step| step.completed)
                .unwrap_or(false))
        })
    }

    fn begin_migration_step<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            let mut steps = self.steps.lock();
            let step = steps.entry(name.to_string()).or_default();
            let resume = step.running && !step.completed;
            step.running = true;
            Ok(resume)
        })
    }

    fn record_migration_chunk<'a>(
        &'a self,
        name: &'a str,
        last_processed_key: &'a str,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.steps
                .lock()
                .entry(name.to_string())
                .or_default()
                .chunks
                .push(last_processed_key.to_string());
            Ok(())
        })
    }

    fn complete_migration_step<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.steps
                .lock()
                .entry(name.to_string())
                .or_default()
                .completed = true;
            Ok(())
        })
    }

    fn fail_migration_step<'a>(
        &'a self,
        name: &'a str,
        cause: &'a SecretsError,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let _ = cause;
            self.steps
                .lock()
                .entry(name.to_string())
                .or_default()
                .running = false;
            Ok(())
        })
    }
}
