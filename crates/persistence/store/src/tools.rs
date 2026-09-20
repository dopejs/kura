//! Tool-profile persistence (Stage 9.1b,
//! `docs/providers/tool-provider-architecture.md`).
//!
//! The `tool_profiles` table is tenant-partitioned; the full profile document
//! is the JSON column and the indexed columns are query projections, matching
//! `memory_assets`.
//!
//! **There is no credential column.** A profile's document carries a
//! `secretRef`; the value lives only in the tenant secret plane. Nothing here
//! can store one, which is what makes "no credential in the database" a
//! property of the schema rather than a rule callers must remember.

use rusqlite::{Row, params};

use crate::SQLiteStore;
use crate::crud::null_string;

/// Strict decode, deliberately not `decode_json_field`: that helper maps an
/// empty or `null` document to `T::default()`, which for a tool profile would
/// mean a silently materialised configuration with an empty id and no
/// capability. A corrupt row must be an error.
fn scan_profile(row: &Row) -> Result<kura_tools::ToolProfile, String> {
    let document: String = row.get(0).map_err(|e| e.to_string())?;
    serde_json::from_str(document.trim()).map_err(|e| format!("decode tool profile document: {e}"))
}

impl SQLiteStore {
    pub fn upsert_tool_profile(&self, profile: &kura_tools::ToolProfile) -> Result<(), String> {
        let document = serde_json::to_string(profile)
            .map_err(|e| format!("marshal tool profile {}: {e}", profile.profile_id))?;
        self.conn
            .execute(
                r#"INSERT INTO tool_profiles (
                    profile_id, tenant_id, capability, family, auth_mode, source,
                    enabled, is_default, created_at, updated_at, document_json
                   ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                   ON CONFLICT(profile_id) DO UPDATE SET
                     tenant_id = excluded.tenant_id,
                     capability = excluded.capability,
                     family = excluded.family,
                     auth_mode = excluded.auth_mode,
                     source = excluded.source,
                     enabled = excluded.enabled,
                     is_default = excluded.is_default,
                     updated_at = excluded.updated_at,
                     document_json = excluded.document_json"#,
                params![
                    profile.profile_id,
                    null_string(&profile.tenant_id),
                    profile.capability.as_str(),
                    profile.family.as_str(),
                    profile.auth_mode.as_str(),
                    profile.source.as_str(),
                    i64::from(profile.enabled),
                    i64::from(profile.is_default),
                    profile.created_at.to_rfc3339(),
                    profile.updated_at.to_rfc3339(),
                    document,
                ],
            )
            .map_err(|e| format!("upsert tool profile {}: {e}", profile.profile_id))?;
        Ok(())
    }

    pub fn delete_tool_profile(&self, profile_id: &str) -> Result<(), String> {
        self.conn
            .execute(
                "DELETE FROM tool_profiles WHERE profile_id = ?1",
                params![profile_id],
            )
            .map_err(|e| format!("delete tool profile {profile_id}: {e}"))?;
        Ok(())
    }

    /// Every profile, for restoring the in-memory manager at boot.
    pub fn list_all_tool_profiles(&self) -> Result<Vec<kura_tools::ToolProfile>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT document_json FROM tool_profiles ORDER BY created_at ASC, profile_id ASC",
            )
            .map_err(|e| format!("list all tool profiles: {e}"))?;
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_profile(row)?);
        }
        Ok(items)
    }

    /// Profile ids owned by a tenant. An empty `tenant_id` selects the
    /// NULL-tenant rows — the single-user assembly's own — rather than every
    /// tenant's.
    pub fn list_tool_profile_ids_for_tenant(&self, tenant_id: &str) -> Result<Vec<String>, String> {
        let sql = if tenant_id.is_empty() {
            "SELECT profile_id FROM tool_profiles WHERE tenant_id IS NULL ORDER BY profile_id"
        } else {
            "SELECT profile_id FROM tool_profiles WHERE tenant_id = ?1 ORDER BY profile_id"
        };
        let mut stmt = self
            .conn
            .prepare(sql)
            .map_err(|e| format!("list tool profile ids for tenant: {e}"))?;
        let mut rows = if tenant_id.is_empty() {
            stmt.query([]).map_err(|e| e.to_string())?
        } else {
            stmt.query(params![tenant_id]).map_err(|e| e.to_string())?
        };
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(row.get::<_, String>(0).map_err(|e| e.to_string())?);
        }
        Ok(items)
    }
}
