//! SQLite CRUD for integration resources. Ported from `daemon/internal/store/store.go`
//! (UpsertIntegration, ListIntegrations). The tenant column is written as NULL until the
//! tenancy package is ported; `document_json` holds the whole resource, matching Go.

use rusqlite::{Row, params};

use crate::SQLiteStore;
use crate::crud::{enum_str, now_rfc3339, null_string, parse_rfc3339};

fn scan_integration(row: &Row) -> Result<kura_integrations::Resource, String> {
    // Go scans the explicit columns for queryability, then unmarshals document_json into
    // the item; the document is authoritative.
    let updated_at: String = row.get(7).map_err(|e| e.to_string())?;
    parse_rfc3339(&updated_at)?;
    let document_json: String = row.get(8).map_err(|e| e.to_string())?;
    // Go's json.Unmarshal is lenient (missing fields become zero values); mirror that so
    // pre-tenant seed rows whose document_json is a bare object round-trip as a default.
    Ok(crate::crud::decode_json_field(&document_json).unwrap_or_default())
}

impl SQLiteStore {
    pub fn upsert_integration(&self, item: &kura_integrations::Resource) -> Result<(), String> {
        let document_json = serde_json::to_string(item)
            .map_err(|e| format!("marshal integration {}: {e}", item.integration_id))?;

        self.conn
            .execute(
                r#"INSERT INTO integrations (
                    integration_id,
                    domain_kind,
                    environment_scope,
                    account_key,
                    backend_kind,
                    readiness_status,
                    canonical_default,
                    updated_at,
                    document_json,
                    tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                ON CONFLICT(integration_id) DO UPDATE SET
                    domain_kind = excluded.domain_kind,
                    environment_scope = excluded.environment_scope,
                    account_key = excluded.account_key,
                    backend_kind = excluded.backend_kind,
                    readiness_status = excluded.readiness_status,
                    canonical_default = excluded.canonical_default,
                    updated_at = excluded.updated_at,
                    document_json = excluded.document_json,
                    tenant_id = COALESCE(integrations.tenant_id, excluded.tenant_id)"#,
                params![
                    item.integration_id,
                    item.domain_kind,
                    item.environment_scope,
                    null_string(
                        item.account_binding
                            .as_ref()
                            .map(|b| b.account_key.as_str())
                            .unwrap_or_default()
                    ),
                    enum_str(&item.backend_binding.backend_kind),
                    enum_str(&item.readiness_status),
                    item.canonical_default,
                    now_rfc3339(&item.updated_at),
                    document_json,
                    None::<String>,
                ],
            )
            .map_err(|e| format!("upsert integration {}: {e}", item.integration_id))?;
        Ok(())
    }

    pub fn list_integrations(
        &self,
        environment_scope: &str,
    ) -> Result<Vec<kura_integrations::Resource>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT integration_id, domain_kind, environment_scope, account_key, backend_kind, readiness_status, canonical_default, updated_at, document_json
                FROM integrations
                WHERE environment_scope = ?1
                ORDER BY updated_at ASC, integration_id ASC"#,
            )
            .map_err(|e| format!("list integrations for {environment_scope}: {e}"))?;
        let mut rows = stmt
            .query(params![environment_scope.trim()])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_integration(row)?);
        }
        Ok(items)
    }
}
