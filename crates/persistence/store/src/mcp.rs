//! SQLite CRUD for MCP records (servers, server states, tools, tool exposure rules). Ported from
//! `daemon/internal/store/store.go` (UpsertMCPServer, ListMCPServers, DeleteMCPServer,
//! UpsertMCPServerState, ListMCPServerStates, UpsertMCPTool, ReplaceMCPTools, ListMCPTools,
//! UpsertMCPToolExposureRule, ListMCPToolExposureRules). The tenant column is written as NULL
//! until the tenancy package is ported; UpsertMCPTool follows the Go writer and omits it.

use chrono::{DateTime, Utc};
use rusqlite::{Row, params};

use crate::SQLiteStore;
use crate::crud::{now_rfc3339, opt_time_string, parse_opt_rfc3339, parse_rfc3339};

/// An MCP server catalog row. `document` is the JSON-serialized server document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MCPServerRecord {
    pub server_id: String,
    pub enabled: bool,
    pub updated_at: DateTime<Utc>,
    pub document: String,
}

/// An MCP server runtime state row. `document` is the JSON-serialized state document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MCPServerStateRecord {
    pub server_id: String,
    pub status: String,
    pub updated_at: DateTime<Utc>,
    pub document: String,
}

/// An MCP tool catalog row. `document` is the JSON-serialized tool document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MCPToolRecord {
    pub server_id: String,
    pub tool_name: String,
    pub discovery_status: String,
    pub updated_at: DateTime<Utc>,
    pub last_discovered_at: Option<DateTime<Utc>>,
    pub document: String,
}

/// An MCP tool exposure rule row. `document` is the JSON-serialized rule document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MCPToolExposureRuleRecord {
    pub server_id: String,
    pub tool_name: String,
    pub runtime_surface: String,
    pub exposure_mode: String,
    pub active: bool,
    pub updated_at: DateTime<Utc>,
    pub document: String,
}

fn scan_mcp_server(row: &Row) -> Result<MCPServerRecord, String> {
    let server_id: String = row.get(0).map_err(|e| e.to_string())?;
    let enabled: bool = row.get(1).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(2).map_err(|e| e.to_string())?;
    let document: String = row.get(3).map_err(|e| e.to_string())?;

    Ok(MCPServerRecord {
        server_id,
        enabled,
        updated_at: parse_rfc3339(&updated_at)?,
        document,
    })
}

fn scan_mcp_server_state(row: &Row) -> Result<MCPServerStateRecord, String> {
    let server_id: String = row.get(0).map_err(|e| e.to_string())?;
    let status: String = row.get(1).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(2).map_err(|e| e.to_string())?;
    let document: String = row.get(3).map_err(|e| e.to_string())?;

    Ok(MCPServerStateRecord {
        server_id,
        status,
        updated_at: parse_rfc3339(&updated_at)?,
        document,
    })
}

fn scan_mcp_tool(row: &Row) -> Result<MCPToolRecord, String> {
    let server_id: String = row.get(0).map_err(|e| e.to_string())?;
    let tool_name: String = row.get(1).map_err(|e| e.to_string())?;
    let discovery_status: String = row.get(2).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(3).map_err(|e| e.to_string())?;
    let last_discovered_at: Option<String> = row.get(4).map_err(|e| e.to_string())?;
    let document: String = row.get(5).map_err(|e| e.to_string())?;

    Ok(MCPToolRecord {
        server_id,
        tool_name,
        discovery_status,
        updated_at: parse_rfc3339(&updated_at)?,
        last_discovered_at: parse_opt_rfc3339(last_discovered_at)?,
        document,
    })
}

fn scan_mcp_tool_exposure_rule(row: &Row) -> Result<MCPToolExposureRuleRecord, String> {
    let server_id: String = row.get(0).map_err(|e| e.to_string())?;
    let tool_name: String = row.get(1).map_err(|e| e.to_string())?;
    let runtime_surface: String = row.get(2).map_err(|e| e.to_string())?;
    let exposure_mode: String = row.get(3).map_err(|e| e.to_string())?;
    let active: bool = row.get(4).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(5).map_err(|e| e.to_string())?;
    let document: String = row.get(6).map_err(|e| e.to_string())?;

    Ok(MCPToolExposureRuleRecord {
        server_id,
        tool_name,
        runtime_surface,
        exposure_mode,
        active,
        updated_at: parse_rfc3339(&updated_at)?,
        document,
    })
}

impl SQLiteStore {
    pub fn upsert_mcp_server(&self, record: &MCPServerRecord) -> Result<(), String> {
        self.conn
            .execute(
                r#"INSERT INTO mcp_servers (
                    server_id, enabled, updated_at, document_json, tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5)
                ON CONFLICT(server_id) DO UPDATE SET
                    enabled = excluded.enabled,
                    updated_at = excluded.updated_at,
                    document_json = excluded.document_json,
                    tenant_id = COALESCE(mcp_servers.tenant_id, excluded.tenant_id)"#,
                params![
                    record.server_id,
                    record.enabled,
                    now_rfc3339(&record.updated_at),
                    record.document,
                    None::<String>,
                ],
            )
            .map_err(|e| format!("upsert mcp server {}: {e}", record.server_id))?;
        Ok(())
    }

    pub fn list_mcp_servers(&self) -> Result<Vec<MCPServerRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT server_id, enabled, updated_at, document_json
                FROM mcp_servers
                ORDER BY updated_at ASC, server_id ASC"#,
            )
            .map_err(|e| format!("list mcp servers: {e}"))?;
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_mcp_server(row)?);
        }
        Ok(items)
    }

    pub fn delete_mcp_server(&self, server_id: &str) -> Result<(), String> {
        self.conn
            .execute(
                "DELETE FROM mcp_servers WHERE server_id = ?1",
                params![server_id],
            )
            .map_err(|e| format!("delete mcp server {server_id}: {e}"))?;
        Ok(())
    }

    pub fn upsert_mcp_server_state(&self, record: &MCPServerStateRecord) -> Result<(), String> {
        self.conn
            .execute(
                r#"INSERT INTO mcp_server_states (
                    server_id, status, updated_at, document_json, tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5)
                ON CONFLICT(server_id) DO UPDATE SET
                    status = excluded.status,
                    updated_at = excluded.updated_at,
                    document_json = excluded.document_json,
                    tenant_id = COALESCE(mcp_server_states.tenant_id, excluded.tenant_id)"#,
                params![
                    record.server_id,
                    record.status,
                    now_rfc3339(&record.updated_at),
                    record.document,
                    None::<String>,
                ],
            )
            .map_err(|e| format!("upsert mcp server state {}: {e}", record.server_id))?;
        Ok(())
    }

    pub fn list_mcp_server_states(&self) -> Result<Vec<MCPServerStateRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT server_id, status, updated_at, document_json
                FROM mcp_server_states
                ORDER BY updated_at ASC, server_id ASC"#,
            )
            .map_err(|e| format!("list mcp server states: {e}"))?;
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_mcp_server_state(row)?);
        }
        Ok(items)
    }

    pub fn upsert_mcp_tool(&self, record: &MCPToolRecord) -> Result<(), String> {
        self.conn
            .execute(
                r#"INSERT INTO mcp_tools (
                    server_id, tool_name, discovery_status, updated_at, last_discovered_at,
                    document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                ON CONFLICT(server_id, tool_name) DO UPDATE SET
                    discovery_status = excluded.discovery_status,
                    updated_at = excluded.updated_at,
                    last_discovered_at = excluded.last_discovered_at,
                    document_json = excluded.document_json"#,
                params![
                    record.server_id,
                    record.tool_name,
                    record.discovery_status,
                    now_rfc3339(&record.updated_at),
                    opt_time_string(&record.last_discovered_at),
                    record.document,
                ],
            )
            .map_err(|e| {
                format!(
                    "upsert mcp tool {}/{}: {e}",
                    record.server_id, record.tool_name
                )
            })?;
        Ok(())
    }

    pub fn replace_mcp_tools(
        &self,
        server_id: &str,
        records: &[MCPToolRecord],
    ) -> Result<(), String> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin replace mcp tools: {e}"))?;

        tx.execute(
            "DELETE FROM mcp_tools WHERE server_id = ?1 AND (?2 IS NULL OR ?2 = '' OR tenant_id = ?3 OR tenant_id IS NULL)",
            params![server_id, None::<String>, None::<String>],
        )
        .map_err(|e| format!("clear mcp tools for {server_id}: {e}"))?;

        for record in records {
            tx.execute(
                r#"INSERT INTO mcp_tools (
                    server_id, tool_name, discovery_status, updated_at, last_discovered_at,
                    document_json, tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"#,
                params![
                    record.server_id,
                    record.tool_name,
                    record.discovery_status,
                    now_rfc3339(&record.updated_at),
                    opt_time_string(&record.last_discovered_at),
                    record.document,
                    None::<String>,
                ],
            )
            .map_err(|e| {
                format!(
                    "insert mcp tool {}/{}: {e}",
                    record.server_id, record.tool_name
                )
            })?;
        }

        tx.commit()
            .map_err(|e| format!("commit replace mcp tools: {e}"))
    }

    pub fn list_mcp_tools(&self, server_id: &str) -> Result<Vec<MCPToolRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT server_id, tool_name, discovery_status, updated_at, last_discovered_at,
                    document_json
                FROM mcp_tools
                WHERE (?1 = '' OR server_id = ?1)
                ORDER BY server_id ASC, tool_name ASC"#,
            )
            .map_err(|e| format!("list mcp tools for {server_id}: {e}"))?;
        let mut rows = stmt.query(params![server_id]).map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_mcp_tool(row)?);
        }
        Ok(items)
    }

    pub fn upsert_mcp_tool_exposure_rule(
        &self,
        record: &MCPToolExposureRuleRecord,
    ) -> Result<(), String> {
        self.conn
            .execute(
                r#"INSERT INTO mcp_tool_exposure_rules (
                    server_id, tool_name, runtime_surface, exposure_mode, active, updated_at,
                    document_json, tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                ON CONFLICT(server_id, tool_name, runtime_surface) DO UPDATE SET
                    exposure_mode = excluded.exposure_mode,
                    active = excluded.active,
                    updated_at = excluded.updated_at,
                    document_json = excluded.document_json,
                    tenant_id = COALESCE(mcp_tool_exposure_rules.tenant_id, excluded.tenant_id)"#,
                params![
                    record.server_id,
                    record.tool_name,
                    record.runtime_surface,
                    record.exposure_mode,
                    record.active,
                    now_rfc3339(&record.updated_at),
                    record.document,
                    None::<String>,
                ],
            )
            .map_err(|e| {
                format!(
                    "upsert mcp tool exposure rule {}/{}/{}: {e}",
                    record.server_id, record.tool_name, record.runtime_surface
                )
            })?;
        Ok(())
    }

    pub fn list_mcp_tool_exposure_rules(
        &self,
        server_id: &str,
    ) -> Result<Vec<MCPToolExposureRuleRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT server_id, tool_name, runtime_surface, exposure_mode, active,
                    updated_at, document_json
                FROM mcp_tool_exposure_rules
                WHERE (?1 = '' OR server_id = ?1)
                ORDER BY server_id ASC, tool_name ASC, runtime_surface ASC"#,
            )
            .map_err(|e| format!("list mcp tool exposure rules for {server_id}: {e}"))?;
        let mut rows = stmt.query(params![server_id]).map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_mcp_tool_exposure_rule(row)?);
        }
        Ok(items)
    }
}
