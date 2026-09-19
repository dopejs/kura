//! Generic JSON-document store backing the Roadmap 65-71 in-memory managers (triage, routine,
//! webhook, catalog, execprofile, evidence). Ported from `daemon/internal/store/manager_documents.go`.

use chrono::{DateTime, Utc};
use rusqlite::{Row, params};

use crate::SQLiteStore;
use crate::crud::{now_rfc3339, null_string, parse_rfc3339};

/// One manager document row keyed by (doc_kind, doc_id).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ManagerDocument {
    pub doc_kind: String,
    pub doc_id: String,
    pub environment_scope: String,
    pub tenant_id: String,
    pub document_json: String,
    pub updated_at: DateTime<Utc>,
}

fn scan_manager_document(row: &Row) -> Result<ManagerDocument, String> {
    let doc_kind: String = row.get(0).map_err(|e| e.to_string())?;
    let doc_id: String = row.get(1).map_err(|e| e.to_string())?;
    let environment_scope: Option<String> = row.get(2).map_err(|e| e.to_string())?;
    let tenant_id: Option<String> = row.get(3).map_err(|e| e.to_string())?;
    let document_json: String = row.get(4).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(5).map_err(|e| e.to_string())?;

    Ok(ManagerDocument {
        doc_kind,
        doc_id,
        environment_scope: environment_scope.unwrap_or_default(),
        tenant_id: tenant_id.unwrap_or_default(),
        document_json,
        updated_at: parse_rfc3339(&updated_at)?,
    })
}

impl SQLiteStore {
    pub fn put_manager_document(&self, doc: &ManagerDocument) -> Result<(), String> {
        self.conn
            .execute(
                r#"INSERT INTO manager_documents (doc_kind, doc_id, environment_scope, tenant_id, document_json, updated_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                ON CONFLICT(doc_kind, doc_id) DO UPDATE SET
                    environment_scope = excluded.environment_scope,
                    tenant_id = excluded.tenant_id,
                    document_json = excluded.document_json,
                    updated_at = excluded.updated_at"#,
                params![
                    doc.doc_kind,
                    doc.doc_id,
                    null_string(&doc.environment_scope),
                    null_string(&doc.tenant_id),
                    doc.document_json,
                    now_rfc3339(&doc.updated_at),
                ],
            )
            .map_err(|e| format!("put manager document {}/{}: {e}", doc.doc_kind, doc.doc_id))?;
        Ok(())
    }

    pub fn delete_manager_document(&self, doc_kind: &str, doc_id: &str) -> Result<(), String> {
        self.conn
            .execute(
                "DELETE FROM manager_documents WHERE doc_kind = ?1 AND doc_id = ?2",
                params![doc_kind, doc_id],
            )
            .map_err(|e| format!("delete manager document {doc_kind}/{doc_id}: {e}"))?;
        Ok(())
    }

    /// Binds a manager document to a tenant, mirroring
    /// [`SQLiteStore::bind_row_tenant`] for the composite-key
    /// `manager_documents` table. Pre-tenancy rows carry `tenant_id = ''`
    /// (the managers persisted an empty tenant), so empty counts as unbound
    /// alongside NULL. A document owned by another tenant refuses the bind
    /// with [`SQLiteStore::ERR_CROSS_TENANT_ROW`] rather than being
    /// silently reassigned.
    /// Returns `false` when no such document exists. Callers must not treat a
    /// missing document as success: an item whose ownership could not be
    /// recorded is invisible to every tenant-filtered list, which is silent
    /// data loss from the caller's point of view.
    pub fn bind_manager_document_tenant(
        &self,
        doc_kind: &str,
        doc_id: &str,
        tenant_id: &str,
    ) -> Result<bool, String> {
        if tenant_id.is_empty() {
            return Err("BindManagerDocumentTenant: empty tenantID".to_string());
        }
        let affected = self
            .conn
            .execute(
                r#"UPDATE manager_documents SET tenant_id = ?1
                   WHERE doc_kind = ?2 AND doc_id = ?3
                     AND (tenant_id IS NULL OR tenant_id = '' OR tenant_id = ?1)"#,
                params![tenant_id, doc_kind, doc_id],
            )
            .map_err(|e| format!("bind tenant for {doc_kind}/{doc_id}: {e}"))?;
        if affected > 0 {
            return Ok(true);
        }
        match self.lookup_manager_document_tenant(doc_kind, doc_id)? {
            Some(existing) if !existing.is_empty() && existing != tenant_id => {
                Err(Self::ERR_CROSS_TENANT_ROW.to_string())
            }
            Some(_) => Ok(true),
            None => Ok(false),
        }
    }

    /// The owning tenant of a manager document: `None` when the document does
    /// not exist, `Some("")` when it predates tenancy.
    pub fn lookup_manager_document_tenant(
        &self,
        doc_kind: &str,
        doc_id: &str,
    ) -> Result<Option<String>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT COALESCE(tenant_id, '') FROM manager_documents WHERE doc_kind = ?1 AND doc_id = ?2",
            )
            .map_err(|e| format!("lookup manager document tenant: {e}"))?;
        let mut rows = stmt
            .query(params![doc_kind, doc_id])
            .map_err(|e| e.to_string())?;
        match rows.next().map_err(|e| e.to_string())? {
            Some(row) => Ok(Some(row.get::<_, String>(0).map_err(|e| e.to_string())?)),
            None => Ok(None),
        }
    }

    /// Document ids of `doc_kind` owned by `tenant_id`. Pre-tenancy rows
    /// (`tenant_id` NULL or empty) are excluded: that is the `kura-tenancy`
    /// read convention — unbound history stops being enumerable, while the
    /// by-id paths still admit it.
    pub fn list_manager_document_ids_for_tenant(
        &self,
        doc_kind: &str,
        tenant_id: &str,
    ) -> Result<Vec<String>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT doc_id FROM manager_documents WHERE doc_kind = ?1 AND tenant_id = ?2 ORDER BY doc_id",
            )
            .map_err(|e| format!("list manager document ids for tenant: {e}"))?;
        let mut rows = stmt
            .query(params![doc_kind, tenant_id])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(row.get::<_, String>(0).map_err(|e| e.to_string())?);
        }
        Ok(items)
    }

    pub fn list_manager_documents(&self, doc_kind: &str) -> Result<Vec<ManagerDocument>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT doc_kind, doc_id, environment_scope, tenant_id, document_json, updated_at
                FROM manager_documents
                WHERE doc_kind = ?1
                ORDER BY doc_id"#,
            )
            .map_err(|e| format!("list manager documents for {doc_kind}: {e}"))?;
        let mut rows = stmt.query(params![doc_kind]).map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_manager_document(row)?);
        }
        Ok(items)
    }
}

/// Write-through-persists value as a JSON document keyed by (kind, id).
pub fn put_document<T: serde::Serialize>(
    store: &SQLiteStore,
    kind: &str,
    id: &str,
    env: &str,
    tenant: &str,
    value: &T,
) -> Result<(), String> {
    let document_json =
        serde_json::to_string(value).map_err(|e| format!("marshal {kind} document: {e}"))?;
    store.put_manager_document(&ManagerDocument {
        doc_kind: kind.to_string(),
        doc_id: id.to_string(),
        environment_scope: env.to_string(),
        tenant_id: tenant.to_string(),
        document_json,
        updated_at: chrono::Utc::now(),
    })
}

/// Removes a document.
pub fn delete_document(store: &SQLiteStore, kind: &str, id: &str) -> Result<(), String> {
    store.delete_manager_document(kind, id)
}

/// Reloads all documents of a kind as typed values, skipping any that fail to decode.
pub fn list_documents<T: serde::de::DeserializeOwned>(
    store: &SQLiteStore,
    kind: &str,
) -> Result<Vec<T>, String> {
    let docs = store.list_manager_documents(kind)?;
    let mut out = Vec::with_capacity(docs.len());
    for doc in docs {
        if let Ok(value) = serde_json::from_str::<T>(&doc.document_json) {
            out.push(value);
        }
    }
    Ok(out)
}
