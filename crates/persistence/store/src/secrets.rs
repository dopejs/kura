//! SQLite CRUD for tenant secrets and secret versions plus the
//! `kura_secrets::Store` trait implementation. Ported from
//! `daemon/internal/store/secrets.go` (CreateSecret, UpdateSecretMetadata,
//! RotateSecret, DisableSecret, GetSecretByRef, GetSecretVersion,
//! ListSecrets). The raw secret value never touches this store; only the
//! metadata row and version rows (with value backend refs) are persisted.

use chrono::{DateTime, Utc};
use rusqlite::{Row, params};

use crate::SQLiteStore;
use crate::crud::{now_rfc3339, null_string, parse_opt_rfc3339, parse_rfc3339};

fn is_unset_time(dt: &DateTime<Utc>) -> bool {
    dt.timestamp() == 0 && dt.timestamp_subsec_nanos() == 0
}

fn nullable_time_string(value: &Option<DateTime<Utc>>) -> Option<String> {
    value
        .as_ref()
        .filter(|t| !is_unset_time(t))
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true))
}

fn secret_status_str(status: kura_secrets::SecretStatus) -> &'static str {
    match status {
        kura_secrets::SecretStatus::Active => "active",
        kura_secrets::SecretStatus::Disabled => "disabled",
        kura_secrets::SecretStatus::PendingRemediation => "pending_remediation",
    }
}

fn secret_version_status_str(status: kura_secrets::SecretVersionStatus) -> &'static str {
    match status {
        kura_secrets::SecretVersionStatus::Active => "active",
        kura_secrets::SecretVersionStatus::Superseded => "superseded",
        kura_secrets::SecretVersionStatus::Disabled => "disabled",
        kura_secrets::SecretVersionStatus::PendingRemediation => "pending_remediation",
    }
}

fn secret_status_from_str(value: &str) -> Result<kura_secrets::SecretStatus, String> {
    match value {
        "active" => Ok(kura_secrets::SecretStatus::Active),
        "disabled" => Ok(kura_secrets::SecretStatus::Disabled),
        "pending_remediation" => Ok(kura_secrets::SecretStatus::PendingRemediation),
        other => Err(format!("invalid secret status {other}")),
    }
}

fn secret_version_status_from_str(
    value: &str,
) -> Result<kura_secrets::SecretVersionStatus, String> {
    match value {
        "active" => Ok(kura_secrets::SecretVersionStatus::Active),
        "superseded" => Ok(kura_secrets::SecretVersionStatus::Superseded),
        "disabled" => Ok(kura_secrets::SecretVersionStatus::Disabled),
        "pending_remediation" => Ok(kura_secrets::SecretVersionStatus::PendingRemediation),
        other => Err(format!("invalid secret version status {other}")),
    }
}

fn marshal_document(document: &Option<kura_secrets::Document>) -> Result<String, String> {
    match document {
        None => Ok("{}".to_string()),
        Some(doc) => {
            serde_json::to_string(doc).map_err(|e| format!("marshal tenant secret document: {e}"))
        }
    }
}

type TenantSecretRow = (
    String,
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn decode_tenant_secret(
    secret_id: String,
    tenant_id: String,
    secret_ref: String,
    display_name: Option<String>,
    status: String,
    active_version_id: Option<String>,
    disabled_reason: Option<String>,
    remediation_reason: Option<String>,
    created_at: String,
    updated_at: String,
    rotated_at: Option<String>,
    disabled_at: Option<String>,
    document_raw: Option<String>,
) -> Result<kura_secrets::TenantSecret, String> {
    let document = match document_raw {
        Some(raw) if !raw.trim().is_empty() && raw.trim() != "{}" => Some(
            serde_json::from_str(&raw)
                .map_err(|e| format!("decode tenant secret document: {e}"))?,
        ),
        _ => None,
    };
    Ok(kura_secrets::TenantSecret {
        secret_id,
        tenant_id,
        secret_ref,
        display_name: display_name.unwrap_or_default(),
        status: secret_status_from_str(&status)?,
        active_version_id: active_version_id.unwrap_or_default(),
        disabled_reason: disabled_reason.unwrap_or_default(),
        remediation_reason: remediation_reason.unwrap_or_default(),
        created_at: parse_rfc3339(&created_at)?,
        updated_at: parse_rfc3339(&updated_at)?,
        rotated_at: parse_opt_rfc3339(rotated_at)?,
        disabled_at: parse_opt_rfc3339(disabled_at)?,
        document,
    })
}

fn tenant_secret_row(row: &Row) -> Result<TenantSecretRow, rusqlite::Error> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
        row.get(12)?,
    ))
}

type SecretVersionRow = (
    String,
    String,
    String,
    String,
    i64,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
);

fn decode_secret_version(
    secret_version_id: String,
    secret_id: String,
    tenant_id: String,
    secret_ref: String,
    version_number: i64,
    status: String,
    value_backend_ref: String,
    created_at: String,
    activated_at: Option<String>,
    superseded_at: Option<String>,
) -> Result<kura_secrets::SecretVersion, String> {
    Ok(kura_secrets::SecretVersion {
        secret_version_id,
        secret_id,
        tenant_id,
        secret_ref,
        version_number,
        status: secret_version_status_from_str(&status)?,
        value_backend_ref,
        created_at: parse_rfc3339(&created_at)?,
        activated_at: parse_opt_rfc3339(activated_at)?,
        superseded_at: parse_opt_rfc3339(superseded_at)?,
    })
}

fn secret_version_row(row: &Row) -> Result<SecretVersionRow, rusqlite::Error> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
    ))
}

impl SQLiteStore {
    pub fn create_tenant_secret(
        &self,
        secret: &kura_secrets::TenantSecret,
        version: &kura_secrets::SecretVersion,
    ) -> Result<(), String> {
        let document_json = marshal_document(&secret.document)?;
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin create tenant secret transaction: {e}"))?;
        insert_tenant_secret(&tx, secret, &document_json)?;
        insert_tenant_secret_version(&tx, version)?;
        tx.commit()
            .map_err(|e| format!("commit create tenant secret transaction: {e}"))
    }

    pub fn update_secret_metadata(
        &self,
        secret: &kura_secrets::TenantSecret,
    ) -> Result<(), String> {
        let document_json = marshal_document(&secret.document)?;
        self.conn
            .execute(
                "UPDATE tenant_secrets
                 SET display_name = ?1, disabled_reason = ?2, remediation_reason = ?3,
                     updated_at = ?4, document_json = ?5
                 WHERE tenant_id = ?6 AND secret_ref = ?7",
                params![
                    null_string(&secret.display_name),
                    null_string(&secret.disabled_reason),
                    null_string(&secret.remediation_reason),
                    now_rfc3339(&secret.updated_at),
                    document_json,
                    secret.tenant_id,
                    secret.secret_ref,
                ],
            )
            .map_err(|e| format!("update tenant secret metadata: {e}"))?;
        Ok(())
    }

    pub fn rotate_tenant_secret(
        &self,
        secret: &kura_secrets::TenantSecret,
        previous_version_id: &str,
        mut version: kura_secrets::SecretVersion,
    ) -> Result<(), String> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin rotate tenant secret transaction: {e}"))?;
        let next_version: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(version_number), 0) + 1 FROM tenant_secret_versions
                 WHERE tenant_id = ?1 AND secret_id = ?2",
                params![secret.tenant_id, secret.secret_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("select next tenant secret version: {e}"))?;
        version.version_number = next_version;
        let now = secret.updated_at;
        if !previous_version_id.is_empty() {
            tx.execute(
                "UPDATE tenant_secret_versions
                 SET status = ?1, superseded_at = ?2
                 WHERE tenant_id = ?3 AND secret_version_id = ?4",
                params![
                    secret_version_status_str(kura_secrets::SecretVersionStatus::Superseded),
                    now_rfc3339(&now),
                    secret.tenant_id,
                    previous_version_id,
                ],
            )
            .map_err(|e| format!("supersede tenant secret version: {e}"))?;
        }
        insert_tenant_secret_version(&tx, &version)?;
        tx.execute(
            "UPDATE tenant_secrets
             SET active_version_id = ?1, rotated_at = ?2, updated_at = ?3
             WHERE tenant_id = ?4 AND secret_ref = ?5",
            params![
                secret.active_version_id,
                nullable_time_string(&secret.rotated_at),
                now_rfc3339(&secret.updated_at),
                secret.tenant_id,
                secret.secret_ref,
            ],
        )
        .map_err(|e| format!("update rotated tenant secret: {e}"))?;
        tx.commit()
            .map_err(|e| format!("commit rotate tenant secret transaction: {e}"))
    }

    pub fn disable_tenant_secret(&self, secret: &kura_secrets::TenantSecret) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE tenant_secrets
                 SET status = ?1, disabled_reason = ?2, disabled_at = ?3, updated_at = ?4
                 WHERE tenant_id = ?5 AND secret_ref = ?6",
                params![
                    secret_status_str(secret.status),
                    null_string(&secret.disabled_reason),
                    nullable_time_string(&secret.disabled_at),
                    now_rfc3339(&secret.updated_at),
                    secret.tenant_id,
                    secret.secret_ref,
                ],
            )
            .map_err(|e| format!("disable tenant secret: {e}"))?;
        Ok(())
    }

    pub fn get_secret_by_ref(
        &self,
        tenant_id: &str,
        secret_ref: &str,
    ) -> Result<Option<kura_secrets::TenantSecret>, String> {
        let result: Result<TenantSecretRow, rusqlite::Error> = self.conn.query_row(
            r#"SELECT secret_id, tenant_id, secret_ref, display_name, status, active_version_id,
                      disabled_reason, remediation_reason, created_at, updated_at, rotated_at,
                      disabled_at, document_json
               FROM tenant_secrets WHERE tenant_id = ?1 AND secret_ref = ?2"#,
            params![tenant_id, secret_ref],
            tenant_secret_row,
        );
        match result {
            Ok(row) => Ok(Some(decode_tenant_secret(
                row.0, row.1, row.2, row.3, row.4, row.5, row.6, row.7, row.8, row.9, row.10,
                row.11, row.12,
            )?)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(format!("get tenant secret by ref: {e}")),
        }
    }

    pub fn get_secret_version(
        &self,
        tenant_id: &str,
        secret_version_id: &str,
    ) -> Result<Option<kura_secrets::SecretVersion>, String> {
        let result: Result<SecretVersionRow, rusqlite::Error> = self.conn.query_row(
            r#"SELECT secret_version_id, secret_id, tenant_id, secret_ref, version_number, status,
                      value_backend_ref, created_at, activated_at, superseded_at
               FROM tenant_secret_versions WHERE tenant_id = ?1 AND secret_version_id = ?2"#,
            params![tenant_id, secret_version_id],
            secret_version_row,
        );
        match result {
            Ok(row) => Ok(Some(decode_secret_version(
                row.0, row.1, row.2, row.3, row.4, row.5, row.6, row.7, row.8, row.9,
            )?)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(format!("get tenant secret version: {e}")),
        }
    }

    pub fn list_secrets(&self, tenant_id: &str) -> Result<Vec<kura_secrets::TenantSecret>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT secret_id, tenant_id, secret_ref, display_name, status, active_version_id,
                          disabled_reason, remediation_reason, created_at, updated_at, rotated_at,
                          disabled_at, document_json
                   FROM tenant_secrets WHERE tenant_id = ?1
                   ORDER BY updated_at DESC, secret_id DESC"#,
            )
            .map_err(|e| format!("list tenant secrets: {e}"))?;
        let mut rows = stmt.query(params![tenant_id]).map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let row = tenant_secret_row(&row).map_err(|e| e.to_string())?;
            items.push(decode_tenant_secret(
                row.0, row.1, row.2, row.3, row.4, row.5, row.6, row.7, row.8, row.9, row.10,
                row.11, row.12,
            )?);
        }
        Ok(items)
    }
}

fn insert_tenant_secret(
    tx: &rusqlite::Transaction,
    secret: &kura_secrets::TenantSecret,
    document_json: &str,
) -> Result<(), String> {
    tx.execute(
        r#"INSERT INTO tenant_secrets (
            secret_id, tenant_id, secret_ref, display_name, status, active_version_id,
            disabled_reason, remediation_reason, created_at, updated_at, rotated_at,
            disabled_at, document_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)"#,
        params![
            secret.secret_id,
            secret.tenant_id,
            secret.secret_ref,
            null_string(&secret.display_name),
            secret_status_str(secret.status),
            null_string(&secret.active_version_id),
            null_string(&secret.disabled_reason),
            null_string(&secret.remediation_reason),
            now_rfc3339(&secret.created_at),
            now_rfc3339(&secret.updated_at),
            nullable_time_string(&secret.rotated_at),
            nullable_time_string(&secret.disabled_at),
            document_json,
        ],
    )
    .map_err(|e| format!("insert tenant secret: {e}"))?;
    Ok(())
}

fn insert_tenant_secret_version(
    tx: &rusqlite::Transaction,
    version: &kura_secrets::SecretVersion,
) -> Result<(), String> {
    tx.execute(
        r#"INSERT INTO tenant_secret_versions (
            secret_version_id, secret_id, tenant_id, secret_ref, version_number, status,
            value_backend_ref, created_at, activated_at, superseded_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"#,
        params![
            version.secret_version_id,
            version.secret_id,
            version.tenant_id,
            version.secret_ref,
            version.version_number,
            secret_version_status_str(version.status),
            version.value_backend_ref,
            now_rfc3339(&version.created_at),
            nullable_time_string(&version.activated_at),
            nullable_time_string(&version.superseded_at),
        ],
    )
    .map_err(|e| format!("insert tenant secret version: {e}"))?;
    Ok(())
}
// --- kura_secrets::Store trait impl (async wrapper over the sync DAOs) ---
//
// rusqlite's Connection is Send but not Sync, so SQLiteStore cannot be the
// trait's `Send + Sync` self type directly. The workspace convention shares
// the store as `Arc<parking_lot::Mutex<SQLiteStore>>`; because the orphan rule
// forbids implementing an external trait for a foreign type, the mutex is
// wrapped in the local `SecretStoreHandle` newtype, which satisfies Send + Sync
// (SQLiteStore is Send) and serializes access exactly like the Go daemon's
// single-connection store.

/// Send + Sync handle over the SQLite store implementing
/// [`kura_secrets::Store`]. Construct from a fresh store and share as
/// `Arc<SecretStoreHandle>` with the secrets manager.
pub struct SecretStoreHandle(pub parking_lot::Mutex<SQLiteStore>);

impl SecretStoreHandle {
    pub fn new(store: SQLiteStore) -> Self {
        Self(parking_lot::Mutex::new(store))
    }
}

impl kura_secrets::Store for SecretStoreHandle {
    fn create_secret<'a>(
        &'a self,
        secret: kura_secrets::TenantSecret,
        version: kura_secrets::SecretVersion,
    ) -> kura_secrets::BoxFuture<'a, kura_secrets::Result<()>> {
        Box::pin(async move {
            let store = self.0.lock();
            store
                .create_tenant_secret(&secret, &version)
                .map_err(kura_secrets::SecretsError::Store)
        })
    }

    fn update_secret_metadata<'a>(
        &'a self,
        secret: kura_secrets::TenantSecret,
    ) -> kura_secrets::BoxFuture<'a, kura_secrets::Result<()>> {
        Box::pin(async move {
            let store = self.0.lock();
            store
                .update_secret_metadata(&secret)
                .map_err(kura_secrets::SecretsError::Store)
        })
    }

    fn rotate_secret<'a>(
        &'a self,
        secret: kura_secrets::TenantSecret,
        previous_version_id: &'a str,
        version: kura_secrets::SecretVersion,
    ) -> kura_secrets::BoxFuture<'a, kura_secrets::Result<()>> {
        Box::pin(async move {
            let store = self.0.lock();
            store
                .rotate_tenant_secret(&secret, previous_version_id, version)
                .map_err(kura_secrets::SecretsError::Store)
        })
    }

    fn disable_secret<'a>(
        &'a self,
        secret: kura_secrets::TenantSecret,
    ) -> kura_secrets::BoxFuture<'a, kura_secrets::Result<()>> {
        Box::pin(async move {
            let store = self.0.lock();
            store
                .disable_tenant_secret(&secret)
                .map_err(kura_secrets::SecretsError::Store)
        })
    }

    fn get_secret_by_ref<'a>(
        &'a self,
        tenant_id: &'a str,
        secret_ref: &'a str,
    ) -> kura_secrets::BoxFuture<'a, kura_secrets::Result<Option<kura_secrets::TenantSecret>>> {
        Box::pin(async move {
            let store = self.0.lock();
            store
                .get_secret_by_ref(tenant_id, secret_ref)
                .map_err(kura_secrets::SecretsError::Store)
        })
    }

    fn get_secret_version<'a>(
        &'a self,
        tenant_id: &'a str,
        secret_version_id: &'a str,
    ) -> kura_secrets::BoxFuture<'a, kura_secrets::Result<Option<kura_secrets::SecretVersion>>>
    {
        Box::pin(async move {
            let store = self.0.lock();
            store
                .get_secret_version(tenant_id, secret_version_id)
                .map_err(kura_secrets::SecretsError::Store)
        })
    }

    fn list_secrets<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> kura_secrets::BoxFuture<'a, kura_secrets::Result<Vec<kura_secrets::TenantSecret>>> {
        Box::pin(async move {
            let store = self.0.lock();
            store
                .list_secrets(tenant_id)
                .map_err(kura_secrets::SecretsError::Store)
        })
    }
}
