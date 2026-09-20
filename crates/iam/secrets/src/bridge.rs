//! Legacy local-credential bridge: migrates `mcp-secrets.json` /
//! `skill-secrets.json` files from a pre-tenancy data directory into tenant
//! secrets, idempotently, with resumable progress tracking and
//! conflict-quarantine semantics. Faithful port of Go `bridge.go`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{Result, SecretsError};
use crate::manager::{BoxFuture, Manager};
use crate::redaction::RedactedSecretSummary;
use crate::types::{
    BridgedCredentialResource, CreateDisabledMetadataInput, CreateInput, DisabledResource,
    Document, LegacyCredentialResourceBridgeInput, LegacyCredentialResourceBridgeResult,
    ResolutionStatus, ResourceKind, SecretStatus,
};

const LOCAL_MCP_SECRETS_FILE_NAME: &str = "mcp-secrets.json";
const LOCAL_SKILL_SECRETS_FILE_NAME: &str = "skill-secrets.json";

/// Migration-step progress tracking (Go `BridgeProgressStore`, implemented
/// by `internal/store.SQLiteStore`). `begin_migration_step` returns whether
/// the step is resuming an earlier incomplete run.
pub trait BridgeProgressStore: Send + Sync {
    fn register_migration_step<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<()>>;
    fn is_migration_step_completed<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<bool>>;
    fn begin_migration_step<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<bool>>;
    fn record_migration_chunk<'a>(
        &'a self,
        name: &'a str,
        last_processed_key: &'a str,
    ) -> BoxFuture<'a, Result<()>>;
    fn complete_migration_step<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<()>>;
    fn fail_migration_step<'a>(
        &'a self,
        name: &'a str,
        cause: &'a SecretsError,
    ) -> BoxFuture<'a, Result<()>>;
}

/// Rewrites legacy resources (integrations, connectors, MCP servers, ...) to
/// reference bridged tenant secrets (Go `LegacyCredentialResourceStore`).
pub trait LegacyCredentialResourceStore: Send + Sync {
    fn bridge_legacy_credential_resources<'a>(
        &'a self,
        input: LegacyCredentialResourceBridgeInput,
    ) -> BoxFuture<'a, Result<LegacyCredentialResourceBridgeResult>>;
}

pub struct LocalCredentialBridgeInput {
    pub data_dir: String,
    pub tenant_id: String,
    /// Migration step name for progress tracking; empty disables it (Go
    /// checks `strings.TrimSpace(input.StepName) != ""`).
    pub step_name: String,
    pub manager: Arc<Manager>,
    pub progress_store: Option<Arc<dyn BridgeProgressStore>>,
    pub resource_store: Option<Arc<dyn LegacyCredentialResourceStore>>,
}

/// Bridge outcome. Contains only redacted summaries — never secret values.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalCredentialBridgeResult {
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scanned_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub created: Vec<RedactedSecretSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped_existing: Vec<RedactedSecretSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled: Vec<DisabledResource>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bridged_resources: Vec<BridgedCredentialResource>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled_resources: Vec<BridgedCredentialResource>,
    pub secret_ref_count: i64,
    pub completed_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub already_completed: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Default)]
struct LocalCredentialCandidate {
    // Go tracks secretRef implicitly via the map key; the key is the ref.
    value: String,
    sources: Vec<String>,
    conflict: bool,
}

/// Migrates local credential files into tenant secrets (Go
/// `BridgeLocalCredentialFiles`).
///
/// Unlike Go, the partially-filled result is not returned on error (Rust
/// `Result` carries one value); Go callers that used it saw best-effort
/// progress that is also recorded via [`BridgeProgressStore`] chunks.
pub async fn bridge_local_credential_files(
    input: &LocalCredentialBridgeInput,
) -> Result<LocalCredentialBridgeResult> {
    let mut result = LocalCredentialBridgeResult {
        tenant_id: input.tenant_id.trim().to_string(),
        ..LocalCredentialBridgeResult::default()
    };
    if result.tenant_id.is_empty() {
        return Err(SecretsError::TenantRequired);
    }
    let progress = input
        .progress_store
        .as_ref()
        .filter(|_| !input.step_name.trim().is_empty());

    if let Some(progress) = progress {
        progress.register_migration_step(&input.step_name).await?;
        if progress
            .is_migration_step_completed(&input.step_name)
            .await?
        {
            result.already_completed = true;
            return Ok(result);
        }
        let resume = progress.begin_migration_step(&input.step_name).await?;
        if !resume {
            if let Err(err) = bridge_files(input, &mut result).await {
                let _ = progress.fail_migration_step(&input.step_name, &err).await;
                return Err(err);
            }
            progress.complete_migration_step(&input.step_name).await?;
            result.completed_at = Utc::now();
            return Ok(result);
        }
    }

    if let Err(err) = bridge_files(input, &mut result).await {
        if let Some(progress) = progress {
            let _ = progress.fail_migration_step(&input.step_name, &err).await;
        }
        return Err(err);
    }
    if let Some(progress) = progress {
        progress.complete_migration_step(&input.step_name).await?;
    }
    result.completed_at = Utc::now();
    Ok(result)
}

async fn bridge_files(
    input: &LocalCredentialBridgeInput,
    result: &mut LocalCredentialBridgeResult,
) -> Result<()> {
    let (candidates, scanned) = load_local_credential_candidates(&input.data_dir)?;
    result.scanned_files = scanned;
    let mut refs: Vec<&String> = candidates.keys().collect();
    refs.sort();
    result.secret_ref_count = refs.len() as i64;

    for secret_ref in refs {
        let candidate = &candidates[secret_ref];
        if candidate.conflict {
            let secret =
                match input.manager.get(&result.tenant_id, secret_ref).await {
                    Ok(secret) => secret,
                    Err(SecretsError::SecretNotFound) => input
                        .manager
                        .create_disabled_metadata(CreateDisabledMetadataInput {
                            tenant_id: result.tenant_id.clone(),
                            secret_ref: secret_ref.clone(),
                            display_name: secret_ref.clone(),
                            disabled_reason: "legacy_secret_ref_conflict".to_string(),
                            remediation_reason:
                                "move the ambiguous local credential into a tenant secret manually"
                                    .to_string(),
                            document: Some(bridge_document(&candidate.sources)),
                        })
                        .await?,
                    Err(err) => return Err(err),
                };
            result.disabled.push(DisabledResource {
                tenant_id: result.tenant_id.clone(),
                resource_kind: ResourceKind::DisabledCredential,
                resource_id: secret.secret_id,
                status: secret.status,
                disabled_reason: secret.disabled_reason,
                remediation_reason: secret.remediation_reason,
                secret_refs: vec![secret_ref.clone()],
                updated_at: secret.updated_at,
            });
            continue;
        }

        match input.manager.get(&result.tenant_id, secret_ref).await {
            Ok(existing) => {
                result.skipped_existing.push(RedactedSecretSummary {
                    secret_ref: existing.secret_ref,
                    secret_version_id: existing.active_version_id,
                    resolution: Some(ResolutionStatus::Unavailable),
                    status: Some(existing.status),
                    disabled_reason: existing.disabled_reason,
                    redaction_rule: "secret_metadata_only".to_string(),
                });
                continue;
            }
            Err(SecretsError::SecretNotFound) => {}
            Err(err) => return Err(err),
        }

        let created = input
            .manager
            .create(CreateInput {
                tenant_id: result.tenant_id.clone(),
                secret_ref: secret_ref.clone(),
                display_name: secret_ref.clone(),
                value: candidate.value.clone(),
                document: Some(bridge_document(&candidate.sources)),
            })
            .await?;
        result.created.push(RedactedSecretSummary {
            secret_ref: created.secret_ref,
            secret_version_id: created.active_version_id,
            resolution: Some(ResolutionStatus::Unavailable),
            status: Some(created.status),
            disabled_reason: String::new(),
            redaction_rule: "secret_metadata_only".to_string(),
        });
        if let Some(progress) = input
            .progress_store
            .as_ref()
            .filter(|_| !input.step_name.trim().is_empty())
        {
            progress
                .record_migration_chunk(&input.step_name, secret_ref)
                .await?;
        }
    }

    if let Some(resource_store) = &input.resource_store {
        let legacy = resource_store
            .bridge_legacy_credential_resources(LegacyCredentialResourceBridgeInput {
                tenant_id: result.tenant_id.clone(),
                active_secret_refs: active_secret_refs(result),
                disabled_secret_refs: disabled_secret_refs(result),
            })
            .await?;
        result.bridged_resources.extend(legacy.bridged);
        result.disabled_resources.extend(legacy.disabled);
    }
    Ok(())
}

fn bridge_document(sources: &[String]) -> Document {
    Document::from_iter([
        (
            "source".to_string(),
            serde_json::Value::String("local_credential_bridge".to_string()),
        ),
        (
            "files".to_string(),
            serde_json::Value::Array(
                sources
                    .iter()
                    .map(|source| serde_json::Value::String(source.clone()))
                    .collect(),
            ),
        ),
    ])
}

/// Sorted, deduped refs of active secrets in the bridge result (Go
/// `activeSecretRefs`).
fn active_secret_refs(result: &LocalCredentialBridgeResult) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut add = |secret_ref: &str, status: Option<SecretStatus>| {
        let trimmed = secret_ref.trim();
        if trimmed.is_empty() || status != Some(SecretStatus::Active) {
            return;
        }
        seen.insert(trimmed.to_string());
    };
    for item in &result.created {
        add(&item.secret_ref, item.status);
    }
    for item in &result.skipped_existing {
        add(&item.secret_ref, item.status);
    }
    seen.into_iter().collect()
}

/// Sorted, deduped refs of disabled resources in the bridge result (Go
/// `disabledSecretRefs`).
fn disabled_secret_refs(result: &LocalCredentialBridgeResult) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    for item in &result.disabled {
        for secret_ref in &item.secret_refs {
            let trimmed = secret_ref.trim();
            if !trimmed.is_empty() {
                seen.insert(trimmed.to_string());
            }
        }
    }
    seen.into_iter().collect()
}

/// Loads and merges `mcp-secrets.json` + `skill-secrets.json` from
/// `data_dir`. Same ref with differing values across files is a conflict;
/// same value merges sources. Returns candidates plus the scanned file
/// names, in fixed scan order.
fn load_local_credential_candidates(
    data_dir: &str,
) -> Result<(HashMap<String, LocalCredentialCandidate>, Vec<String>)> {
    let files = [LOCAL_MCP_SECRETS_FILE_NAME, LOCAL_SKILL_SECRETS_FILE_NAME];
    let mut items: HashMap<String, LocalCredentialCandidate> = HashMap::new();
    let mut scanned = Vec::with_capacity(files.len());
    for file_name in files {
        let path = Path::new(data_dir.trim()).join(file_name);
        let Some(values) = load_local_credential_file(&path)? else {
            continue;
        };
        scanned.push(file_name.to_string());
        for (key, value) in values {
            let secret_ref = key.trim();
            let secret_value = value.trim();
            if secret_ref.is_empty() || secret_value.is_empty() {
                continue;
            }
            let candidate = items
                .entry(secret_ref.to_string())
                .or_insert_with(LocalCredentialCandidate::default);
            if candidate.value.is_empty() && candidate.sources.is_empty() {
                candidate.value = secret_value.to_string();
            } else if candidate.value != secret_value {
                candidate.conflict = true;
            }
            append_if_missing(&mut candidate.sources, file_name);
        }
    }
    Ok((items, scanned))
}

/// Reads one `{"REF": "value"}` JSON file. Missing file → `None` (Go:
/// `os.ErrNotExist` tolerated). Go's `json.Unmarshal` maps JSON `null` to the
/// zero string, so `null` values decode as empty and are skipped upstream.
fn load_local_credential_file(path: &Path) -> Result<Option<HashMap<String, String>>> {
    let payload = match std::fs::read(path) {
        Ok(payload) => payload,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(SecretsError::CredentialFile(format!(
                "read local credential file {}: {err}",
                base_name(path)
            )));
        }
    };
    let values: HashMap<String, Option<String>> =
        serde_json::from_slice(&payload).map_err(|err| {
            SecretsError::CredentialFile(format!(
                "decode local credential file {}: {err}",
                base_name(path)
            ))
        })?;
    Ok(Some(
        values
            .into_iter()
            .map(|(key, value)| (key, value.unwrap_or_default()))
            .collect(),
    ))
}

fn base_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn append_if_missing(items: &mut Vec<String>, value: &str) {
    if !items.iter().any(|item| item == value) {
        items.push(value.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::LocalBackend;
    use crate::redaction::REDACTED_VALUE;
    use crate::testutil::{FakeProgressStore, FakeStore, TestDir};
    use crate::types::ResolveInput;

    fn write_json_file(dir: &Path, name: &str, value: serde_json::Value) {
        let payload = serde_json::to_vec(&value).expect("marshal fixture");
        std::fs::write(dir.join(name), payload).expect("write fixture");
    }

    fn bridge_manager(suffix: &str) -> (Arc<Manager>, TestDir, TestDir) {
        let store_dir = TestDir::new(&format!("bridge-store-{suffix}"));
        let backend_dir = TestDir::new(&format!("bridge-backend-{suffix}"));
        let store = Arc::new(FakeStore::new());
        let backend = Arc::new(LocalBackend::new(backend_dir.path()).expect("local backend"));
        (
            Arc::new(Manager::new(store, backend)),
            store_dir,
            backend_dir,
        )
    }

    #[tokio::test]
    async fn bridge_creates_tenant_secrets_idempotently() {
        let (manager, _store_dir, _backend_dir) = bridge_manager("idem");
        let data_dir = TestDir::new("bridge-data-idem");
        write_json_file(
            data_dir.path(),
            LOCAL_MCP_SECRETS_FILE_NAME,
            serde_json::json!({"MCP_TOKEN": "mcp-secret", "SHARED_TOKEN": "shared-secret"}),
        );
        write_json_file(
            data_dir.path(),
            LOCAL_SKILL_SECRETS_FILE_NAME,
            serde_json::json!({"SKILL_TOKEN": "skill-secret", "SHARED_TOKEN": "shared-secret"}),
        );
        let progress = Arc::new(FakeProgressStore::new());

        let input = LocalCredentialBridgeInput {
            data_dir: data_dir.path().to_string_lossy().into_owned(),
            tenant_id: "ten_bridge".to_string(),
            step_name: "hosted_credential_bridge".to_string(),
            manager: manager.clone(),
            progress_store: Some(progress.clone()),
            resource_store: None,
        };
        let result = bridge_local_credential_files(&input)
            .await
            .expect("first bridge");
        assert_eq!(result.created.len(), 3, "result {result:?}");
        assert_eq!(result.secret_ref_count, 3);
        assert_eq!(
            result.scanned_files,
            vec![
                LOCAL_MCP_SECRETS_FILE_NAME.to_string(),
                LOCAL_SKILL_SECRETS_FILE_NAME.to_string()
            ]
        );
        for summary in &result.created {
            assert_eq!(summary.redaction_rule, "secret_metadata_only");
            assert_eq!(summary.resolution, Some(ResolutionStatus::Unavailable));
            assert_eq!(summary.status, Some(SecretStatus::Active));
        }
        for secret_ref in ["MCP_TOKEN", "SKILL_TOKEN", "SHARED_TOKEN"] {
            let resolved = manager
                .resolve(ResolveInput {
                    tenant_id: "ten_bridge".to_string(),
                    secret_ref: secret_ref.to_string(),
                })
                .await
                .expect("resolve bridged secret");
            assert!(!resolved.value.is_empty());
            assert_ne!(resolved.value, REDACTED_VALUE);
        }

        let second = bridge_local_credential_files(&input)
            .await
            .expect("second bridge");
        assert!(second.already_completed);
        assert!(second.created.is_empty());
        assert!(second.skipped_existing.is_empty());
        assert_eq!(second.tenant_id, "ten_bridge");
    }

    #[tokio::test]
    async fn bridge_reports_conflicts_without_creating_ambiguous_secret() {
        let (manager, _store_dir, _backend_dir) = bridge_manager("conflict");
        let data_dir = TestDir::new("bridge-data-conflict");
        write_json_file(
            data_dir.path(),
            LOCAL_MCP_SECRETS_FILE_NAME,
            serde_json::json!({"DUP_TOKEN": "one"}),
        );
        write_json_file(
            data_dir.path(),
            LOCAL_SKILL_SECRETS_FILE_NAME,
            serde_json::json!({"DUP_TOKEN": "two"}),
        );

        let input = LocalCredentialBridgeInput {
            data_dir: data_dir.path().to_string_lossy().into_owned(),
            tenant_id: "ten_bridge".to_string(),
            step_name: String::new(),
            manager: manager.clone(),
            progress_store: None,
            resource_store: None,
        };
        let result = bridge_local_credential_files(&input).await.expect("bridge");
        assert_eq!(result.disabled.len(), 1, "result {result:?}");
        assert_eq!(result.disabled[0].status, SecretStatus::PendingRemediation);
        assert_eq!(
            result.disabled[0].secret_refs,
            vec!["DUP_TOKEN".to_string()]
        );
        assert!(result.created.is_empty());

        let secret = manager
            .get("ten_bridge", "DUP_TOKEN")
            .await
            .expect("get conflicting bridged metadata");
        assert_eq!(secret.status, SecretStatus::PendingRemediation);
        assert!(!secret.disabled_reason.is_empty());
        assert!(!secret.remediation_reason.is_empty());
        assert!(
            manager
                .resolve(ResolveInput {
                    tenant_id: "ten_bridge".to_string(),
                    secret_ref: "DUP_TOKEN".to_string(),
                })
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn bridge_skips_existing_secrets_and_feeds_resource_store() {
        struct RecordingResourceStore {
            seen: parking_lot::Mutex<Option<LegacyCredentialResourceBridgeInput>>,
        }
        impl LegacyCredentialResourceStore for RecordingResourceStore {
            fn bridge_legacy_credential_resources<'a>(
                &'a self,
                input: LegacyCredentialResourceBridgeInput,
            ) -> BoxFuture<'a, Result<LegacyCredentialResourceBridgeResult>> {
                Box::pin(async move {
                    *self.seen.lock() = Some(input);
                    Ok(LegacyCredentialResourceBridgeResult::default())
                })
            }
        }

        let (manager, _store_dir, _backend_dir) = bridge_manager("skip");
        let data_dir = TestDir::new("bridge-data-skip");
        // Pre-existing secret with the same ref: must be skipped, not
        // overwritten.
        manager
            .create(CreateInput {
                tenant_id: "ten_bridge".into(),
                secret_ref: "EXISTING".into(),
                value: "pre-existing-value".into(),
                ..CreateInput::default()
            })
            .await
            .expect("pre-create");
        write_json_file(
            data_dir.path(),
            LOCAL_MCP_SECRETS_FILE_NAME,
            serde_json::json!({"EXISTING": "different-value", "NEW_REF": "new-value"}),
        );

        let resource_store = Arc::new(RecordingResourceStore {
            seen: parking_lot::Mutex::new(None),
        });
        let input = LocalCredentialBridgeInput {
            data_dir: data_dir.path().to_string_lossy().into_owned(),
            tenant_id: "ten_bridge".to_string(),
            step_name: String::new(),
            manager: manager.clone(),
            progress_store: None,
            resource_store: Some(resource_store.clone()),
        };
        let result = bridge_local_credential_files(&input).await.expect("bridge");
        assert_eq!(result.skipped_existing.len(), 1);
        assert_eq!(result.skipped_existing[0].secret_ref, "EXISTING");
        assert_eq!(result.created.len(), 1);
        assert_eq!(result.created[0].secret_ref, "NEW_REF");

        // The pre-existing value was not overwritten.
        let resolved = manager
            .resolve(ResolveInput {
                tenant_id: "ten_bridge".into(),
                secret_ref: "EXISTING".to_string(),
            })
            .await
            .expect("resolve existing");
        assert_eq!(resolved.value, "pre-existing-value");

        // Resource store received the sorted active refs.
        let seen = resource_store
            .seen
            .lock()
            .clone()
            .expect("resource store input");
        assert_eq!(seen.tenant_id, "ten_bridge");
        assert_eq!(
            seen.active_secret_refs,
            vec!["EXISTING".to_string(), "NEW_REF".to_string()]
        );
        assert!(seen.disabled_secret_refs.is_empty());

        // Bridge output never carries raw secret values.
        let json = serde_json::to_string(&result).expect("serialize result");
        assert!(!json.contains("different-value"));
        assert!(!json.contains("new-value"));
        assert!(!json.contains("pre-existing-value"));
    }

    #[tokio::test]
    async fn bridge_requires_tenant_and_survives_missing_files() {
        let (manager, _store_dir, _backend_dir) = bridge_manager("empty");
        let input = LocalCredentialBridgeInput {
            data_dir: "/nonexistent-dir-for-bridge".to_string(),
            tenant_id: "  ".to_string(),
            step_name: String::new(),
            manager: manager.clone(),
            progress_store: None,
            resource_store: None,
        };
        assert_eq!(
            bridge_local_credential_files(&input)
                .await
                .expect_err("tenant required"),
            SecretsError::TenantRequired
        );

        // No credential files present: empty but successful bridge.
        let data_dir = TestDir::new("bridge-data-empty");
        let input = LocalCredentialBridgeInput {
            data_dir: data_dir.path().to_string_lossy().into_owned(),
            tenant_id: "ten_bridge".to_string(),
            step_name: String::new(),
            manager,
            progress_store: None,
            resource_store: None,
        };
        let result = bridge_local_credential_files(&input).await.expect("bridge");
        assert_eq!(result.secret_ref_count, 0);
        assert!(result.scanned_files.is_empty());
        assert!(result.created.is_empty());
    }

    #[tokio::test]
    async fn malformed_credential_file_fails_with_base_name() {
        let (manager, _store_dir, _backend_dir) = bridge_manager("badjson");
        let data_dir = TestDir::new("bridge-data-badjson");
        std::fs::write(
            data_dir.path().join(LOCAL_SKILL_SECRETS_FILE_NAME),
            b"{not json",
        )
        .expect("write malformed fixture");
        let input = LocalCredentialBridgeInput {
            data_dir: data_dir.path().to_string_lossy().into_owned(),
            tenant_id: "ten_bridge".to_string(),
            step_name: String::new(),
            manager,
            progress_store: None,
            resource_store: None,
        };
        let err = bridge_local_credential_files(&input)
            .await
            .expect_err("malformed file must fail");
        let message = err.to_string();
        assert!(
            message.contains("decode local credential file skill-secrets.json"),
            "unexpected error: {message}"
        );
    }

    #[test]
    fn empty_and_null_entries_are_skipped() {
        let dir = TestDir::new("bridge-filter");
        write_json_file(
            dir.path(),
            LOCAL_MCP_SECRETS_FILE_NAME,
            serde_json::json!({"": "x", "EMPTY": "", "NULL_REF": null, "OK": "v"}),
        );
        let (candidates, scanned) =
            load_local_credential_candidates(&dir.path().to_string_lossy()).expect("load");
        assert_eq!(scanned, vec![LOCAL_MCP_SECRETS_FILE_NAME.to_string()]);
        assert_eq!(candidates.len(), 1);
        assert!(candidates.contains_key("OK"));
    }
}
