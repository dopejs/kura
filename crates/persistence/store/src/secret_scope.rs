//! SQLite CRUD for secret scope bindings. Ported from `daemon/internal/store/store.go`
//! (UpsertSecretScopeBinding, ListSecretScopeBindings). The tenant column is written as NULL
//! until the tenancy package is ported; `document_json` holds the whole document.

use rusqlite::{Row, params};

use crate::SQLiteStore;

/// A secret scope binding ledger row. `document` is the JSON-serialized binding document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SecretScopeBindingRecord {
    pub binding_id: String,
    pub consumer_kind: String,
    pub consumer_id: String,
    pub environment_scope: String,
    pub secret_ref: String,
    pub default_source: String,
    pub delivery_kind: String,
    pub active: bool,
    pub document: String,
}

fn scan_secret_scope_binding(row: &Row) -> Result<SecretScopeBindingRecord, String> {
    let binding_id: String = row.get(0).map_err(|e| e.to_string())?;
    let consumer_kind: String = row.get(1).map_err(|e| e.to_string())?;
    let consumer_id: String = row.get(2).map_err(|e| e.to_string())?;
    let environment_scope: String = row.get(3).map_err(|e| e.to_string())?;
    let secret_ref: String = row.get(4).map_err(|e| e.to_string())?;
    let default_source: String = row.get(5).map_err(|e| e.to_string())?;
    let delivery_kind: String = row.get(6).map_err(|e| e.to_string())?;
    let active: bool = row.get(7).map_err(|e| e.to_string())?;
    let document: String = row.get(8).map_err(|e| e.to_string())?;

    Ok(SecretScopeBindingRecord {
        binding_id,
        consumer_kind,
        consumer_id,
        environment_scope,
        secret_ref,
        default_source,
        delivery_kind,
        active,
        document,
    })
}

impl SQLiteStore {
    pub fn upsert_secret_scope_binding(
        &self,
        record: &SecretScopeBindingRecord,
    ) -> Result<(), String> {
        self.conn
            .execute(
                r#"INSERT INTO secret_scope_bindings (
                    binding_id, consumer_kind, consumer_id, environment_scope, secret_ref,
                    default_source, delivery_kind, active, document_json, tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                ON CONFLICT(binding_id) DO UPDATE SET
                    consumer_kind = excluded.consumer_kind,
                    consumer_id = excluded.consumer_id,
                    environment_scope = excluded.environment_scope,
                    secret_ref = excluded.secret_ref,
                    default_source = excluded.default_source,
                    delivery_kind = excluded.delivery_kind,
                    active = excluded.active,
                    document_json = excluded.document_json,
                    tenant_id = COALESCE(secret_scope_bindings.tenant_id, excluded.tenant_id)"#,
                params![
                    record.binding_id,
                    record.consumer_kind,
                    record.consumer_id,
                    record.environment_scope,
                    record.secret_ref,
                    record.default_source,
                    record.delivery_kind,
                    record.active,
                    record.document,
                    None::<String>,
                ],
            )
            .map_err(|e| format!("upsert secret scope binding {}: {e}", record.binding_id))?;
        Ok(())
    }

    pub fn list_secret_scope_bindings(
        &self,
        consumer_kind: &str,
        consumer_id: &str,
    ) -> Result<Vec<SecretScopeBindingRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT binding_id, consumer_kind, consumer_id, environment_scope, secret_ref, default_source, delivery_kind, active, document_json
                FROM secret_scope_bindings
                WHERE consumer_kind = ?1 AND consumer_id = ?2
                ORDER BY secret_ref ASC, binding_id ASC"#,
            )
            .map_err(|e| format!("list secret scope bindings for {consumer_kind}/{consumer_id}: {e}"))?;
        let mut rows = stmt
            .query(params![consumer_kind, consumer_id])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_secret_scope_binding(row)?);
        }
        Ok(items)
    }
}
