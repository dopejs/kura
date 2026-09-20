//! Memory-asset persistence (Roadmap 78, spec 058).
//!
//! The `memory_assets` table is tenant-partitioned; the full asset document
//! is the JSON column and the indexed columns are query projections. Rows
//! are written by the memory manager and restored at boot.

use rusqlite::{Row, params};

use crate::SQLiteStore;
use crate::crud::{decode_json_field, now_rfc3339, null_string};

/// Query filter for listing memory assets.
#[derive(Debug, Clone, Default)]
pub struct MemoryAssetFilter {
    pub tenant_id: String,
    pub layer: String,
    pub status: String,
    pub kind: String,
}

fn scan_asset(row: &Row) -> Result<kura_memory::MemoryAsset, String> {
    let document: String = row.get(0).map_err(|e| e.to_string())?;
    decode_json_field(&document)
}

impl SQLiteStore {
    pub fn upsert_memory_asset(&self, asset: &kura_memory::MemoryAsset) -> Result<(), String> {
        let document = serde_json::to_string(asset)
            .map_err(|e| format!("marshal memory asset {}: {e}", asset.asset_id))?;
        self.conn
            .execute(
                r#"INSERT INTO memory_assets (
                    asset_id, tenant_id, kind, layer, status, visibility, atom_type,
                    owner_kind, owner_id, version, supersedes_asset_id,
                    created_at, updated_at, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                ON CONFLICT(asset_id) DO UPDATE SET
                    tenant_id = excluded.tenant_id,
                    kind = excluded.kind,
                    layer = excluded.layer,
                    status = excluded.status,
                    visibility = excluded.visibility,
                    atom_type = excluded.atom_type,
                    owner_kind = excluded.owner_kind,
                    owner_id = excluded.owner_id,
                    version = excluded.version,
                    supersedes_asset_id = excluded.supersedes_asset_id,
                    updated_at = excluded.updated_at,
                    document_json = excluded.document_json"#,
                params![
                    asset.asset_id,
                    null_string(&asset.tenant_id),
                    asset.kind.as_str(),
                    asset.layer.as_str(),
                    asset.status.as_str(),
                    asset.visibility.as_str(),
                    null_string(asset.atom_type.map(|a| a.as_str()).unwrap_or_default()),
                    asset.owner.kind.as_str(),
                    asset.owner.id,
                    asset.version,
                    null_string(&asset.supersedes_asset_id),
                    now_rfc3339(&asset.created_at),
                    now_rfc3339(&asset.updated_at),
                    document,
                ],
            )
            .map_err(|e| format!("upsert memory asset {}: {e}", asset.asset_id))?;
        Ok(())
    }

    pub fn get_memory_asset(
        &self,
        asset_id: &str,
    ) -> Result<Option<kura_memory::MemoryAsset>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT document_json FROM memory_assets WHERE asset_id = ?1")
            .map_err(|e| format!("get memory asset {asset_id}: {e}"))?;
        let mut rows = stmt.query(params![asset_id]).map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        scan_asset(row).map(Some)
    }

    pub fn list_memory_assets(
        &self,
        filter: &MemoryAssetFilter,
    ) -> Result<Vec<kura_memory::MemoryAsset>, String> {
        let mut sql = String::from("SELECT document_json FROM memory_assets WHERE 1=1");
        let mut args: Vec<String> = Vec::new();
        if !filter.tenant_id.trim().is_empty() {
            sql.push_str(&format!(" AND tenant_id = ?{}", args.len() + 1));
            args.push(filter.tenant_id.trim().to_string());
        }
        if !filter.layer.trim().is_empty() {
            sql.push_str(&format!(" AND layer = ?{}", args.len() + 1));
            args.push(filter.layer.trim().to_string());
        }
        if !filter.status.trim().is_empty() {
            sql.push_str(&format!(" AND status = ?{}", args.len() + 1));
            args.push(filter.status.trim().to_string());
        }
        if !filter.kind.trim().is_empty() {
            sql.push_str(&format!(" AND kind = ?{}", args.len() + 1));
            args.push(filter.kind.trim().to_string());
        }
        sql.push_str(" ORDER BY updated_at DESC, asset_id DESC");
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("list memory assets: {e}"))?;
        let mut rows = stmt
            .query(rusqlite::params_from_iter(args.iter()))
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_asset(row)?);
        }
        Ok(items)
    }

    /// Boot restore: every asset row (all tenants), oldest-first so
    /// supersede chains replay in order.
    // -----------------------------------------------------------------
    // Derived index: retrieval embeddings
    // -----------------------------------------------------------------

    /// Cached vectors for `asset_ids` under `fingerprint`. Misses are simply
    /// absent from the map; the caller computes and stores them.
    ///
    /// The cache exists because the vector ranker previously re-embedded the
    /// whole candidate corpus on **every turn** — tolerable for the in-process
    /// default, but one RPC per asset per turn for an external provider, on
    /// the reply path.
    pub fn get_memory_asset_embeddings(
        &self,
        fingerprint: &str,
        asset_ids: &[String],
    ) -> Result<std::collections::HashMap<String, Vec<f32>>, String> {
        let mut out = std::collections::HashMap::new();
        if asset_ids.is_empty() {
            return Ok(out);
        }
        let mut stmt = self
            .conn
            .prepare("SELECT vector_json FROM memory_asset_embeddings WHERE fingerprint = ?1 AND asset_id = ?2")
            .map_err(|e| format!("get memory asset embeddings: {e}"))?;
        for asset_id in asset_ids {
            let mut rows = stmt
                .query(params![fingerprint, asset_id])
                .map_err(|e| e.to_string())?;
            if let Some(row) = rows.next().map_err(|e| e.to_string())? {
                let json: String = row.get(0).map_err(|e| e.to_string())?;
                if let Ok(vector) = serde_json::from_str::<Vec<f32>>(&json) {
                    out.insert(asset_id.clone(), vector);
                }
            }
        }
        Ok(out)
    }

    pub fn put_memory_asset_embedding(
        &self,
        asset_id: &str,
        fingerprint: &str,
        vector: &[f32],
    ) -> Result<(), String> {
        let vector_json = serde_json::to_string(vector)
            .map_err(|e| format!("marshal embedding for {asset_id}: {e}"))?;
        self.conn
            .execute(
                r#"INSERT INTO memory_asset_embeddings
                    (asset_id, fingerprint, dim, vector_json, updated_at)
                   VALUES (?1, ?2, ?3, ?4, ?5)
                   ON CONFLICT(asset_id, fingerprint) DO UPDATE SET
                     dim = excluded.dim,
                     vector_json = excluded.vector_json,
                     updated_at = excluded.updated_at"#,
                params![
                    asset_id,
                    fingerprint,
                    vector.len() as i64,
                    vector_json,
                    now_rfc3339(&chrono::Utc::now())
                ],
            )
            .map_err(|e| format!("put embedding for {asset_id}: {e}"))?;
        Ok(())
    }

    /// Drops every cached vector for an asset, across all fingerprints.
    ///
    /// This is the enforcement half of the memory-system invariant "forget or
    /// redact operations also clear derived indexes and caches". Before the
    /// cache existed the invariant held for free — retrieval recomputed over
    /// Ready assets each turn, so a revoked asset simply stopped being a
    /// candidate. It no longer holds for free, which is why revocation calls
    /// this.
    pub fn delete_memory_asset_embeddings(&self, asset_id: &str) -> Result<usize, String> {
        self.conn
            .execute(
                "DELETE FROM memory_asset_embeddings WHERE asset_id = ?1",
                params![asset_id],
            )
            .map_err(|e| format!("delete embeddings for {asset_id}: {e}"))
    }

    /// Drops the whole derived index. The rebuild path (`memory reset`
    /// semantics): conversation truth in `memory_assets` is untouched, and the
    /// vectors are recomputed lazily on the next retrieval.
    pub fn clear_memory_asset_embeddings(&self) -> Result<usize, String> {
        self.conn
            .execute("DELETE FROM memory_asset_embeddings", [])
            .map_err(|e| format!("clear memory asset embeddings: {e}"))
    }

    /// Tenant-scoped rebuild: drops only the derived rows belonging to one
    /// tenant's assets. An operator repairing their own index must not discard
    /// every other tenant's, which is why the unscoped form above is not the
    /// one the API calls.
    pub fn clear_memory_asset_embeddings_for_tenant(
        &self,
        tenant_id: &str,
    ) -> Result<usize, String> {
        // `upsert_memory_asset` stores an empty tenant as NULL, so the
        // single-user assembly's own rows are the NULL ones — not `= ''`, and
        // not "everything", which would let a tenant-less request wipe every
        // tenant's index.
        if tenant_id.is_empty() {
            return self
                .conn
                .execute(
                    r#"DELETE FROM memory_asset_embeddings
                       WHERE asset_id IN (SELECT asset_id FROM memory_assets WHERE tenant_id IS NULL)"#,
                    [],
                )
                .map_err(|e| format!("clear memory asset embeddings for tenant: {e}"));
        }
        self.conn
            .execute(
                r#"DELETE FROM memory_asset_embeddings
                   WHERE asset_id IN (SELECT asset_id FROM memory_assets WHERE tenant_id = ?1)"#,
                params![tenant_id],
            )
            .map_err(|e| format!("clear memory asset embeddings for tenant: {e}"))
    }

    /// `(layer, status, count)` for one tenant, the shape the "what is
    /// remembered" surface reports. An empty `tenant_id` selects the NULL-tenant
    /// rows — the single-user assembly's own history — rather than every
    /// tenant's.
    pub fn count_memory_assets_by_layer_status(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<(String, String, i64)>, String> {
        let sql = if tenant_id.is_empty() {
            r#"SELECT layer, status, COUNT(*) FROM memory_assets
               WHERE tenant_id IS NULL GROUP BY layer, status ORDER BY layer, status"#
        } else {
            r#"SELECT layer, status, COUNT(*) FROM memory_assets
               WHERE tenant_id = ?1 GROUP BY layer, status ORDER BY layer, status"#
        };
        let mut stmt = self
            .conn
            .prepare(sql)
            .map_err(|e| format!("count memory assets: {e}"))?;
        let mut rows = if tenant_id.is_empty() {
            stmt.query([]).map_err(|e| e.to_string())?
        } else {
            stmt.query(params![tenant_id]).map_err(|e| e.to_string())?
        };
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            out.push((
                row.get::<_, String>(0).map_err(|e| e.to_string())?,
                row.get::<_, String>(1).map_err(|e| e.to_string())?,
                row.get::<_, i64>(2).map_err(|e| e.to_string())?,
            ));
        }
        Ok(out)
    }

    /// How many derived vectors exist for a tenant's assets. Reported beside
    /// the asset counts so an index that has silently stopped being written
    /// (see the cache-write warning on the retrieval path) is visible as a
    /// number rather than as unexplained latency.
    pub fn count_memory_asset_embeddings_for_tenant(&self, tenant_id: &str) -> Result<i64, String> {
        let sql = if tenant_id.is_empty() {
            r#"SELECT COUNT(*) FROM memory_asset_embeddings
               WHERE asset_id IN (SELECT asset_id FROM memory_assets WHERE tenant_id IS NULL)"#
        } else {
            r#"SELECT COUNT(*) FROM memory_asset_embeddings
               WHERE asset_id IN (SELECT asset_id FROM memory_assets WHERE tenant_id = ?1)"#
        };
        let mut stmt = self
            .conn
            .prepare(sql)
            .map_err(|e| format!("count memory asset embeddings: {e}"))?;
        let mut rows = if tenant_id.is_empty() {
            stmt.query([]).map_err(|e| e.to_string())?
        } else {
            stmt.query(params![tenant_id]).map_err(|e| e.to_string())?
        };
        match rows.next().map_err(|e| e.to_string())? {
            Some(row) => row.get::<_, i64>(0).map_err(|e| e.to_string()),
            None => Ok(0),
        }
    }

    pub fn list_all_memory_assets(&self) -> Result<Vec<kura_memory::MemoryAsset>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT document_json FROM memory_assets ORDER BY created_at ASC, asset_id ASC",
            )
            .map_err(|e| format!("list all memory assets: {e}"))?;
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_asset(row)?);
        }
        Ok(items)
    }
}
