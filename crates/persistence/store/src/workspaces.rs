//! SQLite CRUD for the workspace domain. Ported from
//! `daemon/internal/store/workspace_store.go` (EnsureDefaultWorkspace,
//! CreateWorkspace, UpdateWorkspaceStatus, ListWorkspaces, GetWorkspace,
//! IsWorkspaceSelectable). Workspaces persist as `document_json` plus the
//! denormalized tenant columns, with a partial unique index enforcing one
//! default per tenant.

use chrono::Utc;
use rusqlite::{Transaction, params};

use crate::SQLiteStore;
use crate::crud::{now_rfc3339, null_string};

fn new_store_id(prefix: &str) -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!("{prefix}_{}", &hex[..16])
}

fn decode_workspace(raw: &str) -> Result<kura_bindings::Workspace, String> {
    serde_json::from_str(raw).map_err(|e| format!("decode workspace document: {e}"))
}

fn nullable_profile_time(value: &Option<chrono::DateTime<chrono::Utc>>) -> Option<String> {
    value
        .as_ref()
        .filter(|t| t.timestamp() != 0 || t.timestamp_subsec_nanos() != 0)
        .map(now_rfc3339)
}

fn bool_to_int(value: bool) -> i64 {
    if value { 1 } else { 0 }
}

impl SQLiteStore {
    /// Go `EnsureDefaultWorkspace`: lazily and idempotently provisions one
    /// default personal workspace per tenant. Concurrent first-access converges
    /// on a single record via the partial unique index.
    pub fn ensure_default_workspace(
        &self,
        tenant_id: &str,
    ) -> Result<kura_bindings::Workspace, String> {
        let tenant_id = tenant_id.trim().to_string();
        if tenant_id.is_empty() {
            return Err("tenant id is required".to_string());
        }
        if let Some(ws) = self.default_workspace(&tenant_id)? {
            return Ok(ws);
        }
        let now = Utc::now();
        let ws = kura_bindings::Workspace {
            workspace_id: new_store_id("ws"),
            tenant_id: tenant_id.clone(),
            display_name: "Personal Workspace".to_string(),
            status: kura_bindings::WorkspaceStatus::ACTIVE,
            is_default: true,
            owner_principal_id: "system".to_string(),
            repair_status: kura_bindings::RepairStatus::HEALTHY,
            redaction_status: kura_bindings::RedactionStatus::NOT_REQUIRED,
            created_at: now,
            updated_at: now,
            archived_at: None,
        };
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin workspace insert: {e}"))?;
        match insert_workspace_tx(&tx, &ws) {
            Ok(()) => {
                tx.commit()
                    .map_err(|e| format!("commit workspace insert: {e}"))?;
                Ok(ws)
            }
            Err(err) => {
                // A concurrent first-access won the unique-default race; load the winner.
                if let Some(winner) = self.default_workspace(&tenant_id)? {
                    return Ok(winner);
                }
                Err(err)
            }
        }
    }

    fn default_workspace(
        &self,
        tenant_id: &str,
    ) -> Result<Option<kura_bindings::Workspace>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT document_json FROM workspaces WHERE tenant_id = ?1 AND is_default = 1")
            .map_err(|e| format!("load default workspace: {e}"))?;
        let mut rows = stmt.query(params![tenant_id]).map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let raw: String = row.get(0).map_err(|e| e.to_string())?;
        Ok(Some(decode_workspace(&raw)?))
    }

    /// Go `CreateWorkspace`: creates a non-default tenant-scoped workspace.
    pub fn create_workspace(
        &self,
        actor: &kura_identity::TenantContext,
        display_name: &str,
    ) -> Result<(kura_bindings::Workspace, String), String> {
        if actor.tenant_id.trim().is_empty() || actor.principal_id.trim().is_empty() {
            return Err(kura_bindings::BindingError::ExplicitActorRequired.to_string());
        }
        kura_bindings::validate_workspace_mutation(&kura_bindings::WorkspaceMutationInput {
            display_name: display_name.to_string(),
            status: kura_bindings::WorkspaceStatus::ACTIVE,
        })
        .map_err(|e| e.to_string())?;
        let now = Utc::now();
        let audit_id = new_store_id("audit_binding");
        let ws = kura_bindings::Workspace {
            workspace_id: new_store_id("ws"),
            tenant_id: actor.tenant_id.clone(),
            display_name: kura_bindings::safe_label(display_name),
            status: kura_bindings::WorkspaceStatus::ACTIVE,
            is_default: false,
            owner_principal_id: actor.principal_id.clone(),
            repair_status: kura_bindings::RepairStatus::HEALTHY,
            redaction_status: kura_bindings::RedactionStatus::NOT_REQUIRED,
            created_at: now,
            updated_at: now,
            archived_at: None,
        };
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin create workspace: {e}"))?;
        insert_workspace_tx(&tx, &ws)?;
        insert_binding_audit_tx(
            &tx,
            &BindingAuditRow {
                audit_event_id: audit_id.clone(),
                tenant_id: actor.tenant_id.clone(),
                workspace_id: ws.workspace_id.clone(),
                actor_principal_id: actor.principal_id.clone(),
                event_kind: "workspace.created".to_string(),
                outcome: "succeeded".to_string(),
                permission_gate: "bindings.manage".to_string(),
                reason_code: "user_created_workspace".to_string(),
                safe_summary: "Workspace created".to_string(),
                occurred_at: now,
                ..BindingAuditRow::default()
            },
        )?;
        tx.commit()
            .map_err(|e| format!("commit create workspace: {e}"))?;
        Ok((ws, audit_id))
    }

    /// Go `UpdateWorkspaceStatus`: archives, disables, or reactivates a
    /// workspace. No hard delete.
    pub fn update_workspace_status(
        &self,
        actor: &kura_identity::TenantContext,
        workspace_id: &str,
        status: kura_bindings::WorkspaceStatus,
    ) -> Result<(kura_bindings::Workspace, String), String> {
        if actor.tenant_id.trim().is_empty() || actor.principal_id.trim().is_empty() {
            return Err(kura_bindings::BindingError::ExplicitActorRequired.to_string());
        }
        let is_known = status == kura_bindings::WorkspaceStatus::ARCHIVED
            || status == kura_bindings::WorkspaceStatus::DISABLED
            || status == kura_bindings::WorkspaceStatus::ACTIVE;
        if !is_known {
            return Err(
                kura_bindings::invalid_binding_reason("workspace_status_invalid").to_string(),
            );
        }
        let mut ws = self
            .get_workspace(&actor.tenant_id, workspace_id)?
            .ok_or_else(|| "workspace not found".to_string())?;
        if ws.is_default && status != kura_bindings::WorkspaceStatus::ACTIVE {
            return Err(
                kura_bindings::invalid_binding_reason("default_workspace_not_retirable")
                    .to_string(),
            );
        }
        let now = Utc::now();
        let audit_id = new_store_id("audit_binding");
        ws.status = status.clone();
        ws.updated_at = now;
        if status == kura_bindings::WorkspaceStatus::ARCHIVED {
            ws.archived_at = Some(now);
            ws.repair_status = kura_bindings::RepairStatus::DISABLED;
        } else if status == kura_bindings::WorkspaceStatus::DISABLED {
            ws.repair_status = kura_bindings::RepairStatus::DISABLED;
        } else {
            ws.repair_status = kura_bindings::RepairStatus::HEALTHY;
            ws.archived_at = None;
        }
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin update workspace status: {e}"))?;
        update_workspace_tx(&tx, &ws)?;
        insert_binding_audit_tx(
            &tx,
            &BindingAuditRow {
                audit_event_id: audit_id.clone(),
                tenant_id: actor.tenant_id.clone(),
                workspace_id: ws.workspace_id.clone(),
                actor_principal_id: actor.principal_id.clone(),
                event_kind: format!("workspace.{}", status.as_str()),
                outcome: "succeeded".to_string(),
                permission_gate: "bindings.manage".to_string(),
                reason_code: "user_updated_workspace".to_string(),
                safe_summary: format!("Workspace {}", status.as_str()),
                occurred_at: now,
                ..BindingAuditRow::default()
            },
        )?;
        tx.commit()
            .map_err(|e| format!("commit update workspace status: {e}"))?;
        Ok((ws, audit_id))
    }

    /// Go `ListWorkspaces`: tenant workspaces, default first.
    pub fn list_workspaces(
        &self,
        tenant_id: &str,
        limit: i64,
    ) -> Result<Vec<kura_bindings::Workspace>, String> {
        let limit = if limit <= 0 || limit > 200 { 50 } else { limit };
        let _ = self.ensure_default_workspace(tenant_id)?;
        let mut stmt = self
            .conn
            .prepare(
                "SELECT document_json FROM workspaces WHERE tenant_id = ?1
                 ORDER BY is_default DESC, updated_at DESC, workspace_id DESC LIMIT ?2",
            )
            .map_err(|e| format!("list workspaces {tenant_id}: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id, limit])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let raw: String = row.get(0).map_err(|e| e.to_string())?;
            items.push(decode_workspace(&raw)?);
        }
        Ok(items)
    }

    /// Go `GetWorkspace`: one workspace by id within the tenant.
    pub fn get_workspace(
        &self,
        tenant_id: &str,
        workspace_id: &str,
    ) -> Result<Option<kura_bindings::Workspace>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT document_json FROM workspaces WHERE tenant_id = ?1 AND workspace_id = ?2",
            )
            .map_err(|e| format!("get workspace {workspace_id}: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id.trim(), workspace_id.trim()])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let raw: String = row.get(0).map_err(|e| e.to_string())?;
        Ok(Some(decode_workspace(&raw)?))
    }

    /// Go `IsWorkspaceSelectable`: workspace exists and is active for the tenant.
    pub fn is_workspace_selectable(
        &self,
        tenant_id: &str,
        workspace_id: &str,
    ) -> Result<bool, String> {
        let workspace_id = workspace_id.trim();
        if workspace_id.is_empty() {
            return Ok(false);
        }
        match self.get_workspace(tenant_id, workspace_id)? {
            None => Ok(false),
            Some(ws) => Ok(ws.status == kura_bindings::WorkspaceStatus::ACTIVE),
        }
    }
}

/// Go `bindingAuditRow`.
#[derive(Debug, Clone, Default)]
pub(crate) struct BindingAuditRow {
    pub audit_event_id: String,
    pub tenant_id: String,
    pub binding_id: String,
    pub workspace_id: String,
    pub actor_principal_id: String,
    pub event_kind: String,
    pub outcome: String,
    pub permission_gate: String,
    pub reason_code: String,
    pub safe_summary: String,
    pub previous_selection_summary: String,
    pub resulting_selection_summary: String,
    pub occurred_at: chrono::DateTime<chrono::Utc>,
}

/// Go `insertBindingAuditTx`.
pub(crate) fn insert_binding_audit_tx(
    tx: &Transaction,
    row: &BindingAuditRow,
) -> Result<(), String> {
    let document = serde_json::json!({
        "auditEventId": row.audit_event_id,
        "tenantId": row.tenant_id,
        "bindingId": row.binding_id,
        "workspaceId": row.workspace_id,
        "actorPrincipalId": row.actor_principal_id,
        "eventKind": row.event_kind,
        "outcome": row.outcome,
        "permissionGate": row.permission_gate,
        "reasonCode": row.reason_code,
        "safeSummary": kura_bindings::safe_label(&row.safe_summary),
        "previousSelectionSummary": kura_bindings::safe_label(&row.previous_selection_summary),
        "resultingSelectionSummary": kura_bindings::safe_label(&row.resulting_selection_summary),
        "occurredAt": now_rfc3339(&row.occurred_at),
    })
    .to_string();
    tx.execute(
        r#"INSERT INTO binding_audit_events (
            audit_event_id, tenant_id, binding_id, workspace_id, actor_principal_id,
            event_kind, outcome, permission_gate, reason_code, safe_summary, occurred_at,
            redaction_status, document_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)"#,
        params![
            row.audit_event_id,
            row.tenant_id,
            null_string(&row.binding_id),
            null_string(&row.workspace_id),
            row.actor_principal_id,
            row.event_kind,
            row.outcome,
            row.permission_gate,
            row.reason_code,
            kura_bindings::safe_label(&row.safe_summary),
            now_rfc3339(&row.occurred_at),
            "redacted",
            document,
        ],
    )
    .map_err(|e| format!("insert binding audit event {}: {e}", row.audit_event_id))?;
    Ok(())
}

fn insert_workspace_tx(tx: &Transaction, ws: &kura_bindings::Workspace) -> Result<(), String> {
    let document_json = serde_json::to_string(ws).map_err(|e| format!("marshal workspace: {e}"))?;
    tx.execute(
        r#"INSERT INTO workspaces (
            workspace_id, tenant_id, display_name, status, is_default, owner_principal_id,
            repair_status, redaction_status, created_at, updated_at, archived_at, document_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"#,
        params![
            ws.workspace_id,
            ws.tenant_id,
            kura_bindings::safe_label(&ws.display_name),
            ws.status.as_str(),
            bool_to_int(ws.is_default),
            ws.owner_principal_id,
            ws.repair_status.as_str(),
            ws.redaction_status.as_str(),
            now_rfc3339(&ws.created_at),
            now_rfc3339(&ws.updated_at),
            nullable_profile_time(&ws.archived_at),
            document_json,
        ],
    )
    .map_err(|e| format!("insert workspace {}: {e}", ws.workspace_id))?;
    Ok(())
}

fn update_workspace_tx(tx: &Transaction, ws: &kura_bindings::Workspace) -> Result<(), String> {
    let document_json = serde_json::to_string(ws).map_err(|e| format!("marshal workspace: {e}"))?;
    tx.execute(
        r#"UPDATE workspaces SET
            display_name = ?1, status = ?2, repair_status = ?3, redaction_status = ?4,
            updated_at = ?5, archived_at = ?6, document_json = ?7
        WHERE tenant_id = ?8 AND workspace_id = ?9"#,
        params![
            kura_bindings::safe_label(&ws.display_name),
            ws.status.as_str(),
            ws.repair_status.as_str(),
            ws.redaction_status.as_str(),
            now_rfc3339(&ws.updated_at),
            nullable_profile_time(&ws.archived_at),
            document_json,
            ws.tenant_id,
            ws.workspace_id,
        ],
    )
    .map_err(|e| format!("update workspace {}: {e}", ws.workspace_id))?;
    Ok(())
}
