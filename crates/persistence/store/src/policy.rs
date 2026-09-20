//! SQLite CRUD for policy approvals and decisions. Ported from `daemon/internal/store/store.go`
//! tenantless write paths (the tenant column is written as NULL until tenancy is ported).

use rusqlite::{Row, params};

use crate::SQLiteStore;
use crate::crud::{
    decode_vec, enum_str, marshal_vec, now_rfc3339, null_string, opt_time_string, parse_enum,
    parse_opt_rfc3339, parse_rfc3339,
};

fn scan_approval(row: &Row) -> Result<kura_policy::Approval, String> {
    let approval_id: String = row.get(0).map_err(|e| e.to_string())?;
    let action: String = row.get(1).map_err(|e| e.to_string())?;
    let resource_kind: Option<String> = row.get(2).map_err(|e| e.to_string())?;
    let resource_id: Option<String> = row.get(3).map_err(|e| e.to_string())?;
    let reason: String = row.get(4).map_err(|e| e.to_string())?;
    let requested_by: Option<String> = row.get(5).map_err(|e| e.to_string())?;
    let status: String = row.get(6).map_err(|e| e.to_string())?;
    let created_at: String = row.get(7).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(8).map_err(|e| e.to_string())?;
    let resolved_at: Option<String> = row.get(9).map_err(|e| e.to_string())?;
    let resolution: Option<String> = row.get(10).map_err(|e| e.to_string())?;
    let comment: Option<String> = row.get(11).map_err(|e| e.to_string())?;
    let integration_bindings_json: Option<String> = row.get(12).map_err(|e| e.to_string())?;

    Ok(kura_policy::Approval {
        approval_id,
        action,
        resource_kind: resource_kind.unwrap_or_default(),
        resource_id: resource_id.unwrap_or_default(),
        reason,
        requested_by: requested_by.unwrap_or_default(),
        status: parse_enum(&status)?,
        created_at: parse_rfc3339(&created_at)?,
        updated_at: parse_rfc3339(&updated_at)?,
        resolved_at: parse_opt_rfc3339(resolved_at)?,
        resolution: resolution.unwrap_or_default(),
        comment: comment.unwrap_or_default(),
        sandbox: None,
        integration_bindings: decode_vec(&integration_bindings_json)?,
    })
}

fn scan_decision(row: &Row) -> Result<kura_policy::Decision, String> {
    let decision_id: String = row.get(0).map_err(|e| e.to_string())?;
    let action: String = row.get(1).map_err(|e| e.to_string())?;
    let resource_kind: Option<String> = row.get(2).map_err(|e| e.to_string())?;
    let resource_id: Option<String> = row.get(3).map_err(|e| e.to_string())?;
    let outcome: String = row.get(4).map_err(|e| e.to_string())?;
    let reason: String = row.get(5).map_err(|e| e.to_string())?;
    let approval_id: Option<String> = row.get(6).map_err(|e| e.to_string())?;
    let created_at: String = row.get(7).map_err(|e| e.to_string())?;

    Ok(kura_policy::Decision {
        decision_id,
        action,
        resource_kind: resource_kind.unwrap_or_default(),
        resource_id: resource_id.unwrap_or_default(),
        outcome: parse_enum(&outcome)?,
        reason,
        approval_id: approval_id.unwrap_or_default(),
        created_at: parse_rfc3339(&created_at)?,
        sandbox: None,
    })
}

impl SQLiteStore {
    pub fn upsert_approval(&self, approval: &kura_policy::Approval) -> Result<(), String> {
        let integration_bindings_json = marshal_vec(&approval.integration_bindings)?;
        self.conn
            .execute(
                r#"INSERT INTO approvals (
                    approval_id, action, resource_kind, resource_id, reason, requested_by, status,
                    created_at, updated_at, resolved_at, resolution, comment,
                    integration_bindings_json, tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                ON CONFLICT(approval_id) DO UPDATE SET
                    action = excluded.action,
                    resource_kind = excluded.resource_kind,
                    resource_id = excluded.resource_id,
                    reason = excluded.reason,
                    requested_by = excluded.requested_by,
                    status = excluded.status,
                    created_at = excluded.created_at,
                    updated_at = excluded.updated_at,
                    resolved_at = excluded.resolved_at,
                    resolution = excluded.resolution,
                    comment = excluded.comment,
                    integration_bindings_json = excluded.integration_bindings_json,
                    tenant_id = COALESCE(approvals.tenant_id, excluded.tenant_id)"#,
                params![
                    approval.approval_id,
                    approval.action,
                    null_string(&approval.resource_kind),
                    null_string(&approval.resource_id),
                    approval.reason,
                    null_string(&approval.requested_by),
                    enum_str(&approval.status),
                    now_rfc3339(&approval.created_at),
                    now_rfc3339(&approval.updated_at),
                    opt_time_string(&approval.resolved_at),
                    null_string(&approval.resolution),
                    null_string(&approval.comment),
                    integration_bindings_json,
                    None::<String>,
                ],
            )
            .map_err(|e| format!("upsert approval {}: {e}", approval.approval_id))?;
        Ok(())
    }

    pub fn list_approvals(&self) -> Result<Vec<kura_policy::Approval>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT approval_id, action, resource_kind, resource_id, reason, requested_by, status,
                    created_at, updated_at, resolved_at, resolution, comment, integration_bindings_json
                FROM approvals
                ORDER BY created_at ASC, approval_id ASC"#,
            )
            .map_err(|e| format!("list approvals: {e}"))?;
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_approval(row)?);
        }
        Ok(items)
    }

    pub fn upsert_decision(&self, decision: &kura_policy::Decision) -> Result<(), String> {
        self.conn
            .execute(
                r#"INSERT INTO decisions (
                    decision_id, action, resource_kind, resource_id, outcome, reason, approval_id,
                    created_at, tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                ON CONFLICT(decision_id) DO UPDATE SET
                    action = excluded.action,
                    resource_kind = excluded.resource_kind,
                    resource_id = excluded.resource_id,
                    outcome = excluded.outcome,
                    reason = excluded.reason,
                    approval_id = excluded.approval_id,
                    created_at = excluded.created_at,
                    tenant_id = COALESCE(decisions.tenant_id, excluded.tenant_id)"#,
                params![
                    decision.decision_id,
                    decision.action,
                    null_string(&decision.resource_kind),
                    null_string(&decision.resource_id),
                    enum_str(&decision.outcome),
                    decision.reason,
                    null_string(&decision.approval_id),
                    now_rfc3339(&decision.created_at),
                    None::<String>,
                ],
            )
            .map_err(|e| format!("upsert decision {}: {e}", decision.decision_id))?;
        Ok(())
    }

    pub fn list_decisions(&self) -> Result<Vec<kura_policy::Decision>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT decision_id, action, resource_kind, resource_id, outcome, reason, approval_id, created_at
                FROM decisions
                ORDER BY created_at ASC, decision_id ASC"#,
            )
            .map_err(|e| format!("list decisions: {e}"))?;
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_decision(row)?);
        }
        Ok(items)
    }
}
