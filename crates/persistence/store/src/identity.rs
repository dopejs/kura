//! SQLite CRUD for identity records: pairings, access tokens, tenants, principals,
//! memberships, tenant invitations, token tenant grants, and tenant audit events.
//! Ported from `daemon/internal/store/store.go` (UpsertPairing &c.). The tenancy
//! tables carry their own tenant/principal columns, so no legacy tenant_id columns
//! are involved here.

use rusqlite::{Row, params, params_from_iter, types::Value};

use crate::SQLiteStore;
use crate::crud::{
    decode_opt_json, enum_str, now_rfc3339, null_string, opt_time_string, parse_enum,
    parse_opt_rfc3339, parse_rfc3339,
};

fn new_tenant_audit_event_id() -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!("audit_{}", &hex[..16])
}

fn scan_pairing(row: &Row) -> Result<kura_identity::auth::Pairing, String> {
    let pairing_id: String = row.get(0).map_err(|e| e.to_string())?;
    let mode: String = row.get(1).map_err(|e| e.to_string())?;
    let label: String = row.get(2).map_err(|e| e.to_string())?;
    let status: String = row.get(3).map_err(|e| e.to_string())?;
    let code_hash: String = row.get(4).map_err(|e| e.to_string())?;
    let code_preview: Option<String> = row.get(5).map_err(|e| e.to_string())?;
    let created_at: String = row.get(6).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(7).map_err(|e| e.to_string())?;
    let expires_at: String = row.get(8).map_err(|e| e.to_string())?;
    let completed_at: Option<String> = row.get(9).map_err(|e| e.to_string())?;

    Ok(kura_identity::auth::Pairing {
        pairing_id,
        mode: parse_enum(&mode)?,
        label,
        status: parse_enum(&status)?,
        code_hash,
        code_preview: code_preview.unwrap_or_default(),
        created_at: parse_rfc3339(&created_at)?,
        updated_at: parse_rfc3339(&updated_at)?,
        expires_at: parse_rfc3339(&expires_at)?,
        completed_at: parse_opt_rfc3339(completed_at)?,
    })
}

fn scan_access_token(row: &Row) -> Result<kura_identity::auth::AccessToken, String> {
    let token_id: String = row.get(0).map_err(|e| e.to_string())?;
    let principal_id: Option<String> = row.get(1).map_err(|e| e.to_string())?;
    let label: String = row.get(2).map_err(|e| e.to_string())?;
    let mode: String = row.get(3).map_err(|e| e.to_string())?;
    let token_hash: String = row.get(4).map_err(|e| e.to_string())?;
    let token_preview: String = row.get(5).map_err(|e| e.to_string())?;
    let status: String = row.get(6).map_err(|e| e.to_string())?;
    let default_tenant_id: Option<String> = row.get(7).map_err(|e| e.to_string())?;
    let created_at: String = row.get(8).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(9).map_err(|e| e.to_string())?;
    let last_used_at: Option<String> = row.get(10).map_err(|e| e.to_string())?;
    let expires_at: Option<String> = row.get(11).map_err(|e| e.to_string())?;
    let revoked_at: Option<String> = row.get(12).map_err(|e| e.to_string())?;
    let rotated_from_token_id: Option<String> = row.get(13).map_err(|e| e.to_string())?;
    let rotated_to_token_id: Option<String> = row.get(14).map_err(|e| e.to_string())?;

    Ok(kura_identity::auth::AccessToken {
        token_id,
        principal_id: principal_id.unwrap_or_default(),
        label,
        mode: parse_enum(&mode)?,
        token_hash,
        token_preview,
        status: parse_enum(&status)?,
        default_tenant_id: default_tenant_id.unwrap_or_default(),
        created_at: parse_rfc3339(&created_at)?,
        updated_at: parse_rfc3339(&updated_at)?,
        last_used_at: parse_opt_rfc3339(last_used_at)?,
        expires_at: parse_opt_rfc3339(expires_at)?,
        revoked_at: parse_opt_rfc3339(revoked_at)?,
        rotated_from_token_id: rotated_from_token_id.unwrap_or_default(),
        rotated_to_token_id: rotated_to_token_id.unwrap_or_default(),
    })
}

fn scan_tenant(row: &Row) -> Result<kura_identity::Tenant, String> {
    let tenant_id: String = row.get(0).map_err(|e| e.to_string())?;
    let tenant_kind: String = row.get(1).map_err(|e| e.to_string())?;
    let display_name: String = row.get(2).map_err(|e| e.to_string())?;
    let status: String = row.get(3).map_err(|e| e.to_string())?;
    let created_at: String = row.get(4).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(5).map_err(|e| e.to_string())?;
    let created_by_principal_id: Option<String> = row.get(6).map_err(|e| e.to_string())?;
    let default_owner_principal_id: Option<String> = row.get(7).map_err(|e| e.to_string())?;

    Ok(kura_identity::Tenant {
        tenant_id,
        tenant_kind: parse_enum(&tenant_kind)?,
        display_name,
        status: parse_enum(&status)?,
        created_at: parse_rfc3339(&created_at)?,
        updated_at: parse_rfc3339(&updated_at)?,
        created_by_principal_id: created_by_principal_id.unwrap_or_default(),
        default_owner_principal_id: default_owner_principal_id.unwrap_or_default(),
        caller_membership_role: None,
        caller_membership_status: None,
        caller_permissions: Vec::new(),
        default_for_current_token: false,
        default_for_current_principal: false,
    })
}

fn scan_principal(row: &Row) -> Result<kura_identity::Principal, String> {
    let principal_id: String = row.get(0).map_err(|e| e.to_string())?;
    let principal_kind: String = row.get(1).map_err(|e| e.to_string())?;
    let display_name: String = row.get(2).map_err(|e| e.to_string())?;
    let status: String = row.get(3).map_err(|e| e.to_string())?;
    let default_tenant_id: String = row.get(4).map_err(|e| e.to_string())?;
    let created_at: String = row.get(5).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(6).map_err(|e| e.to_string())?;
    let disabled_at: Option<String> = row.get(7).map_err(|e| e.to_string())?;
    let removed_at: Option<String> = row.get(8).map_err(|e| e.to_string())?;

    Ok(kura_identity::Principal {
        principal_id,
        principal_kind: parse_enum(&principal_kind)?,
        display_name,
        status: parse_enum(&status)?,
        default_tenant_id,
        created_at: parse_rfc3339(&created_at)?,
        updated_at: parse_rfc3339(&updated_at)?,
        disabled_at: parse_opt_rfc3339(disabled_at)?,
        removed_at: parse_opt_rfc3339(removed_at)?,
    })
}

fn scan_membership(row: &Row) -> Result<kura_identity::Membership, String> {
    let membership_id: String = row.get(0).map_err(|e| e.to_string())?;
    let tenant_id: String = row.get(1).map_err(|e| e.to_string())?;
    let principal_id: String = row.get(2).map_err(|e| e.to_string())?;
    let role: String = row.get(3).map_err(|e| e.to_string())?;
    let status: String = row.get(4).map_err(|e| e.to_string())?;
    let invitation_id: Option<String> = row.get(5).map_err(|e| e.to_string())?;
    let created_at: String = row.get(6).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(7).map_err(|e| e.to_string())?;
    let accepted_at: Option<String> = row.get(8).map_err(|e| e.to_string())?;
    let removed_at: Option<String> = row.get(9).map_err(|e| e.to_string())?;

    Ok(kura_identity::Membership {
        membership_id,
        tenant_id,
        principal_id,
        role: parse_enum(&role)?,
        status: parse_enum(&status)?,
        invitation_id: invitation_id.unwrap_or_default(),
        created_at: parse_rfc3339(&created_at)?,
        updated_at: parse_rfc3339(&updated_at)?,
        accepted_at: parse_opt_rfc3339(accepted_at)?,
        removed_at: parse_opt_rfc3339(removed_at)?,
    })
}

fn scan_tenant_invitation(row: &Row) -> Result<kura_identity::TenantInvitation, String> {
    let invitation_id: String = row.get(0).map_err(|e| e.to_string())?;
    let tenant_id: String = row.get(1).map_err(|e| e.to_string())?;
    let invited_principal_id: String = row.get(2).map_err(|e| e.to_string())?;
    let invited_by_principal_id: String = row.get(3).map_err(|e| e.to_string())?;
    let role: String = row.get(4).map_err(|e| e.to_string())?;
    let status: String = row.get(5).map_err(|e| e.to_string())?;
    let created_at: String = row.get(6).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(7).map_err(|e| e.to_string())?;
    let expires_at: Option<String> = row.get(8).map_err(|e| e.to_string())?;
    let decided_at: Option<String> = row.get(9).map_err(|e| e.to_string())?;

    Ok(kura_identity::TenantInvitation {
        invitation_id,
        tenant_id,
        invited_principal_id,
        invited_by_principal_id,
        role: parse_enum(&role)?,
        status: parse_enum(&status)?,
        created_at: parse_rfc3339(&created_at)?,
        updated_at: parse_rfc3339(&updated_at)?,
        expires_at: parse_opt_rfc3339(expires_at)?,
        decided_at: parse_opt_rfc3339(decided_at)?,
    })
}

fn scan_token_tenant_grant(row: &Row) -> Result<kura_identity::TokenTenantGrant, String> {
    let grant_id: String = row.get(0).map_err(|e| e.to_string())?;
    let token_id: String = row.get(1).map_err(|e| e.to_string())?;
    let tenant_id: String = row.get(2).map_err(|e| e.to_string())?;
    let is_default: bool = row.get(3).map_err(|e| e.to_string())?;
    let status: String = row.get(4).map_err(|e| e.to_string())?;
    let created_at: String = row.get(5).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(6).map_err(|e| e.to_string())?;
    let revoked_at: Option<String> = row.get(7).map_err(|e| e.to_string())?;
    let granted_by_principal_id: Option<String> = row.get(8).map_err(|e| e.to_string())?;

    Ok(kura_identity::TokenTenantGrant {
        grant_id,
        token_id,
        tenant_id,
        is_default,
        status: parse_enum(&status)?,
        created_at: parse_rfc3339(&created_at)?,
        updated_at: parse_rfc3339(&updated_at)?,
        revoked_at: parse_opt_rfc3339(revoked_at)?,
        granted_by_principal_id: granted_by_principal_id.unwrap_or_default(),
    })
}

fn scan_tenant_audit_event(row: &Row) -> Result<kura_identity::TenantAuditEvent, String> {
    let audit_event_id: String = row.get(0).map_err(|e| e.to_string())?;
    let event_kind: String = row.get(1).map_err(|e| e.to_string())?;
    let tenant_id: Option<String> = row.get(2).map_err(|e| e.to_string())?;
    let principal_id: Option<String> = row.get(3).map_err(|e| e.to_string())?;
    let target_principal_id: Option<String> = row.get(4).map_err(|e| e.to_string())?;
    let token_id: Option<String> = row.get(5).map_err(|e| e.to_string())?;
    let outcome: String = row.get(6).map_err(|e| e.to_string())?;
    let reason_code: String = row.get(7).map_err(|e| e.to_string())?;
    let created_at: String = row.get(8).map_err(|e| e.to_string())?;
    let document_json: Option<String> = row.get(9).map_err(|e| e.to_string())?;

    let document = match decode_opt_json(&document_json)? {
        None => None,
        Some(serde_json::Value::Object(map)) => Some(map),
        Some(value) => {
            return Err(format!(
                "decode tenant audit event document: expected object, got {value}"
            ));
        }
    };

    Ok(kura_identity::TenantAuditEvent {
        audit_event_id,
        event_kind,
        tenant_id: tenant_id.unwrap_or_default(),
        principal_id: principal_id.unwrap_or_default(),
        target_principal_id: target_principal_id.unwrap_or_default(),
        token_id: token_id.unwrap_or_default(),
        outcome,
        reason_code,
        created_at: parse_rfc3339(&created_at)?,
        document,
    })
}

impl SQLiteStore {
    pub fn upsert_pairing(&self, pairing: &kura_identity::auth::Pairing) -> Result<(), String> {
        self.conn
            .execute(
                r#"INSERT INTO auth_pairings (
                    pairing_id, mode, label, status, code_hash, code_preview,
                    created_at, updated_at, expires_at, completed_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                ON CONFLICT(pairing_id) DO UPDATE SET
                    mode = excluded.mode,
                    label = excluded.label,
                    status = excluded.status,
                    code_hash = excluded.code_hash,
                    code_preview = excluded.code_preview,
                    created_at = excluded.created_at,
                    updated_at = excluded.updated_at,
                    expires_at = excluded.expires_at,
                    completed_at = excluded.completed_at"#,
                params![
                    pairing.pairing_id,
                    enum_str(&pairing.mode),
                    pairing.label,
                    enum_str(&pairing.status),
                    pairing.code_hash,
                    null_string(&pairing.code_preview),
                    now_rfc3339(&pairing.created_at),
                    now_rfc3339(&pairing.updated_at),
                    now_rfc3339(&pairing.expires_at),
                    opt_time_string(&pairing.completed_at),
                ],
            )
            .map_err(|e| format!("upsert pairing {}: {e}", pairing.pairing_id))?;
        Ok(())
    }

    pub fn list_pairings(&self) -> Result<Vec<kura_identity::auth::Pairing>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT pairing_id, mode, label, status, code_hash, code_preview,
                    created_at, updated_at, expires_at, completed_at
                FROM auth_pairings
                ORDER BY created_at ASC, pairing_id ASC"#,
            )
            .map_err(|e| format!("list pairings: {e}"))?;
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_pairing(row)?);
        }
        Ok(items)
    }

    pub fn upsert_access_token(
        &self,
        token: &kura_identity::auth::AccessToken,
    ) -> Result<(), String> {
        self.conn
            .execute(
                r#"INSERT INTO auth_tokens (
                    token_id, principal_id, label, mode, token_hash, token_preview, status,
                    default_tenant_id, created_at, updated_at, last_used_at, expires_at,
                    revoked_at, rotated_from_token_id, rotated_to_token_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
                ON CONFLICT(token_id) DO UPDATE SET
                    principal_id = excluded.principal_id,
                    label = excluded.label,
                    mode = excluded.mode,
                    token_hash = excluded.token_hash,
                    token_preview = excluded.token_preview,
                    status = excluded.status,
                    default_tenant_id = excluded.default_tenant_id,
                    created_at = excluded.created_at,
                    updated_at = excluded.updated_at,
                    last_used_at = excluded.last_used_at,
                    expires_at = excluded.expires_at,
                    revoked_at = excluded.revoked_at,
                    rotated_from_token_id = excluded.rotated_from_token_id,
                    rotated_to_token_id = excluded.rotated_to_token_id"#,
                params![
                    token.token_id,
                    null_string(&token.principal_id),
                    token.label,
                    enum_str(&token.mode),
                    token.token_hash,
                    token.token_preview,
                    enum_str(&token.status),
                    null_string(&token.default_tenant_id),
                    now_rfc3339(&token.created_at),
                    now_rfc3339(&token.updated_at),
                    opt_time_string(&token.last_used_at),
                    opt_time_string(&token.expires_at),
                    opt_time_string(&token.revoked_at),
                    null_string(&token.rotated_from_token_id),
                    null_string(&token.rotated_to_token_id),
                ],
            )
            .map_err(|e| format!("upsert access token {}: {e}", token.token_id))?;
        Ok(())
    }

    pub fn list_access_tokens(&self) -> Result<Vec<kura_identity::auth::AccessToken>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT token_id, principal_id, label, mode, token_hash, token_preview, status,
                    default_tenant_id, created_at, updated_at, last_used_at, expires_at,
                    revoked_at, rotated_from_token_id, rotated_to_token_id
                FROM auth_tokens
                ORDER BY created_at ASC, token_id ASC"#,
            )
            .map_err(|e| format!("list access tokens: {e}"))?;
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_access_token(row)?);
        }
        Ok(items)
    }

    pub fn list_token_authorities(&self) -> Result<Vec<kura_identity::TokenAuthority>, String> {
        let tokens = self.list_access_tokens()?;
        let mut items = Vec::with_capacity(tokens.len());
        for token in tokens {
            let status = match token.status {
                kura_identity::auth::TokenStatus::Active => kura_identity::LifecycleStatus::Active,
                kura_identity::auth::TokenStatus::Revoked => {
                    kura_identity::LifecycleStatus::Revoked
                }
                kura_identity::auth::TokenStatus::Expired => {
                    kura_identity::LifecycleStatus::Expired
                }
                kura_identity::auth::TokenStatus::Rotated => {
                    kura_identity::LifecycleStatus::Rotated
                }
            };
            items.push(kura_identity::TokenAuthority {
                token_id: token.token_id,
                principal_id: token.principal_id,
                default_tenant_id: token.default_tenant_id,
                status,
                expires_at: token.expires_at,
            });
        }
        Ok(items)
    }

    pub fn upsert_tenant(&self, tenant: &kura_identity::Tenant) -> Result<(), String> {
        self.conn
            .execute(
                r#"INSERT INTO tenants (
                    tenant_id, tenant_kind, display_name, status, created_at, updated_at,
                    created_by_principal_id, default_owner_principal_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                ON CONFLICT(tenant_id) DO UPDATE SET
                    tenant_kind = excluded.tenant_kind,
                    display_name = excluded.display_name,
                    status = excluded.status,
                    created_at = excluded.created_at,
                    updated_at = excluded.updated_at,
                    created_by_principal_id = excluded.created_by_principal_id,
                    default_owner_principal_id = excluded.default_owner_principal_id"#,
                params![
                    tenant.tenant_id,
                    enum_str(&tenant.tenant_kind),
                    tenant.display_name,
                    enum_str(&tenant.status),
                    now_rfc3339(&tenant.created_at),
                    now_rfc3339(&tenant.updated_at),
                    null_string(&tenant.created_by_principal_id),
                    null_string(&tenant.default_owner_principal_id),
                ],
            )
            .map_err(|e| format!("upsert tenant {}: {e}", tenant.tenant_id))?;
        Ok(())
    }

    pub fn get_tenant(&self, tenant_id: &str) -> Result<Option<kura_identity::Tenant>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT tenant_id, tenant_kind, display_name, status, created_at, updated_at,
                    created_by_principal_id, default_owner_principal_id
                FROM tenants
                WHERE tenant_id = ?1"#,
            )
            .map_err(|e| format!("get tenant {tenant_id}: {e}"))?;
        let mut rows = stmt.query(params![tenant_id]).map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        scan_tenant(row).map(Some)
    }

    pub fn list_tenants(
        &self,
        filter: &kura_identity::TenantFilter,
    ) -> Result<Vec<kura_identity::Tenant>, String> {
        let mut sql = String::from(
            r#"SELECT tenant_id, tenant_kind, display_name, status, created_at, updated_at,
                created_by_principal_id, default_owner_principal_id
            FROM tenants
            WHERE 1 = 1"#,
        );
        let mut args: Vec<Value> = Vec::new();
        if let Some(tenant_kind) = filter.tenant_kind {
            sql.push_str(" AND tenant_kind = ?");
            args.push(Value::Text(enum_str(&tenant_kind)));
        }
        if let Some(status) = filter.status {
            sql.push_str(" AND status = ?");
            args.push(Value::Text(enum_str(&status)));
        }
        sql.push_str(" ORDER BY created_at ASC, tenant_id ASC");
        if filter.limit > 0 {
            sql.push_str(" LIMIT ?");
            args.push(Value::Integer(filter.limit as i64));
        }

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("list tenants: {e}"))?;
        let mut rows = stmt
            .query(params_from_iter(&args))
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_tenant(row)?);
        }
        Ok(items)
    }

    pub fn upsert_principal(&self, principal: &kura_identity::Principal) -> Result<(), String> {
        self.conn
            .execute(
                r#"INSERT INTO principals (
                    principal_id, principal_kind, display_name, status, default_tenant_id,
                    created_at, updated_at, disabled_at, removed_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                ON CONFLICT(principal_id) DO UPDATE SET
                    principal_kind = excluded.principal_kind,
                    display_name = excluded.display_name,
                    status = excluded.status,
                    default_tenant_id = excluded.default_tenant_id,
                    created_at = excluded.created_at,
                    updated_at = excluded.updated_at,
                    disabled_at = excluded.disabled_at,
                    removed_at = excluded.removed_at"#,
                params![
                    principal.principal_id,
                    enum_str(&principal.principal_kind),
                    principal.display_name,
                    enum_str(&principal.status),
                    principal.default_tenant_id,
                    now_rfc3339(&principal.created_at),
                    now_rfc3339(&principal.updated_at),
                    opt_time_string(&principal.disabled_at),
                    opt_time_string(&principal.removed_at),
                ],
            )
            .map_err(|e| format!("upsert principal {}: {e}", principal.principal_id))?;
        Ok(())
    }

    pub fn get_principal(
        &self,
        principal_id: &str,
    ) -> Result<Option<kura_identity::Principal>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT principal_id, principal_kind, display_name, status, default_tenant_id,
                    created_at, updated_at, disabled_at, removed_at
                FROM principals
                WHERE principal_id = ?1"#,
            )
            .map_err(|e| format!("get principal {principal_id}: {e}"))?;
        let mut rows = stmt
            .query(params![principal_id])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        scan_principal(row).map(Some)
    }

    pub fn list_principals(
        &self,
        filter: &kura_identity::PrincipalFilter,
    ) -> Result<Vec<kura_identity::Principal>, String> {
        let mut sql = String::from(
            r#"SELECT p.principal_id, p.principal_kind, p.display_name, p.status,
                p.default_tenant_id, p.created_at, p.updated_at, p.disabled_at, p.removed_at
            FROM principals p"#,
        );
        let mut args: Vec<Value> = Vec::new();
        if !filter.tenant_id.is_empty() {
            sql.push_str(" JOIN memberships m ON m.principal_id = p.principal_id");
        }
        sql.push_str(" WHERE 1 = 1");
        if !filter.tenant_id.is_empty() {
            sql.push_str(" AND m.tenant_id = ?");
            args.push(Value::Text(filter.tenant_id.clone()));
        }
        if let Some(status) = filter.status {
            sql.push_str(" AND p.status = ?");
            args.push(Value::Text(enum_str(&status)));
        }
        sql.push_str(" ORDER BY p.created_at ASC, p.principal_id ASC");
        if filter.limit > 0 {
            sql.push_str(" LIMIT ?");
            args.push(Value::Integer(filter.limit as i64));
        }

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("list principals: {e}"))?;
        let mut rows = stmt
            .query(params_from_iter(&args))
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_principal(row)?);
        }
        Ok(items)
    }

    pub fn upsert_membership(&self, membership: &kura_identity::Membership) -> Result<(), String> {
        self.conn
            .execute(
                r#"INSERT INTO memberships (
                    membership_id, tenant_id, principal_id, role, status, invitation_id,
                    created_at, updated_at, accepted_at, removed_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                ON CONFLICT(membership_id) DO UPDATE SET
                    tenant_id = excluded.tenant_id,
                    principal_id = excluded.principal_id,
                    role = excluded.role,
                    status = excluded.status,
                    invitation_id = excluded.invitation_id,
                    created_at = excluded.created_at,
                    updated_at = excluded.updated_at,
                    accepted_at = excluded.accepted_at,
                    removed_at = excluded.removed_at"#,
                params![
                    membership.membership_id,
                    membership.tenant_id,
                    membership.principal_id,
                    enum_str(&membership.role),
                    enum_str(&membership.status),
                    null_string(&membership.invitation_id),
                    now_rfc3339(&membership.created_at),
                    now_rfc3339(&membership.updated_at),
                    opt_time_string(&membership.accepted_at),
                    opt_time_string(&membership.removed_at),
                ],
            )
            .map_err(|e| format!("upsert membership {}: {e}", membership.membership_id))?;
        Ok(())
    }

    pub fn list_memberships(
        &self,
        filter: &kura_identity::MembershipFilter,
    ) -> Result<Vec<kura_identity::Membership>, String> {
        let mut sql = String::from(
            r#"SELECT membership_id, tenant_id, principal_id, role, status, invitation_id,
                created_at, updated_at, accepted_at, removed_at
            FROM memberships
            WHERE 1 = 1"#,
        );
        let mut args: Vec<Value> = Vec::new();
        if !filter.tenant_id.is_empty() {
            sql.push_str(" AND tenant_id = ?");
            args.push(Value::Text(filter.tenant_id.clone()));
        }
        if let Some(status) = filter.status {
            sql.push_str(" AND status = ?");
            args.push(Value::Text(enum_str(&status)));
        }
        if let Some(role) = filter.role {
            sql.push_str(" AND role = ?");
            args.push(Value::Text(enum_str(&role)));
        }
        sql.push_str(" ORDER BY created_at ASC, membership_id ASC");
        if filter.limit > 0 {
            sql.push_str(" LIMIT ?");
            args.push(Value::Integer(filter.limit as i64));
        }

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("list memberships: {e}"))?;
        let mut rows = stmt
            .query(params_from_iter(&args))
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_membership(row)?);
        }
        Ok(items)
    }

    pub fn upsert_tenant_invitation(
        &self,
        invitation: &kura_identity::TenantInvitation,
    ) -> Result<(), String> {
        self.conn
            .execute(
                r#"INSERT INTO tenant_invitations (
                    invitation_id, tenant_id, invited_principal_id, invited_by_principal_id,
                    role, status, created_at, updated_at, expires_at, decided_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                ON CONFLICT(invitation_id) DO UPDATE SET
                    tenant_id = excluded.tenant_id,
                    invited_principal_id = excluded.invited_principal_id,
                    invited_by_principal_id = excluded.invited_by_principal_id,
                    role = excluded.role,
                    status = excluded.status,
                    created_at = excluded.created_at,
                    updated_at = excluded.updated_at,
                    expires_at = excluded.expires_at,
                    decided_at = excluded.decided_at"#,
                params![
                    invitation.invitation_id,
                    invitation.tenant_id,
                    invitation.invited_principal_id,
                    invitation.invited_by_principal_id,
                    enum_str(&invitation.role),
                    enum_str(&invitation.status),
                    now_rfc3339(&invitation.created_at),
                    now_rfc3339(&invitation.updated_at),
                    opt_time_string(&invitation.expires_at),
                    opt_time_string(&invitation.decided_at),
                ],
            )
            .map_err(|e| format!("upsert tenant invitation {}: {e}", invitation.invitation_id))?;
        Ok(())
    }

    pub fn list_tenant_invitations(
        &self,
        filter: &kura_identity::InvitationFilter,
    ) -> Result<Vec<kura_identity::TenantInvitation>, String> {
        let mut sql = String::from(
            r#"SELECT invitation_id, tenant_id, invited_principal_id, invited_by_principal_id,
                role, status, created_at, updated_at, expires_at, decided_at
            FROM tenant_invitations
            WHERE 1 = 1"#,
        );
        let mut args: Vec<Value> = Vec::new();
        if !filter.tenant_id.is_empty() {
            sql.push_str(" AND tenant_id = ?");
            args.push(Value::Text(filter.tenant_id.clone()));
        }
        if !filter.principal_id.is_empty() {
            sql.push_str(" AND invited_principal_id = ?");
            args.push(Value::Text(filter.principal_id.clone()));
        }
        if let Some(status) = filter.status {
            sql.push_str(" AND status = ?");
            args.push(Value::Text(enum_str(&status)));
        }
        sql.push_str(" ORDER BY created_at ASC, invitation_id ASC");
        if filter.limit > 0 {
            sql.push_str(" LIMIT ?");
            args.push(Value::Integer(filter.limit as i64));
        }

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("list tenant invitations: {e}"))?;
        let mut rows = stmt
            .query(params_from_iter(&args))
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_tenant_invitation(row)?);
        }
        Ok(items)
    }

    pub fn upsert_token_tenant_grant(
        &self,
        grant: &kura_identity::TokenTenantGrant,
    ) -> Result<(), String> {
        self.conn
            .execute(
                r#"INSERT INTO token_tenant_grants (
                    grant_id, token_id, tenant_id, is_default, status, created_at, updated_at,
                    revoked_at, granted_by_principal_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                ON CONFLICT(grant_id) DO UPDATE SET
                    token_id = excluded.token_id,
                    tenant_id = excluded.tenant_id,
                    is_default = excluded.is_default,
                    status = excluded.status,
                    created_at = excluded.created_at,
                    updated_at = excluded.updated_at,
                    revoked_at = excluded.revoked_at,
                    granted_by_principal_id = excluded.granted_by_principal_id"#,
                params![
                    grant.grant_id,
                    grant.token_id,
                    grant.tenant_id,
                    grant.is_default,
                    enum_str(&grant.status),
                    now_rfc3339(&grant.created_at),
                    now_rfc3339(&grant.updated_at),
                    opt_time_string(&grant.revoked_at),
                    null_string(&grant.granted_by_principal_id),
                ],
            )
            .map_err(|e| format!("upsert token tenant grant {}: {e}", grant.grant_id))?;
        Ok(())
    }

    pub fn list_token_tenant_grants(
        &self,
        token_id: &str,
    ) -> Result<Vec<kura_identity::TokenTenantGrant>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT grant_id, token_id, tenant_id, is_default, status, created_at, updated_at,
                    revoked_at, granted_by_principal_id
                FROM token_tenant_grants
                WHERE token_id = ?1
                ORDER BY created_at ASC, grant_id ASC"#,
            )
            .map_err(|e| format!("list token tenant grants: {e}"))?;
        let mut rows = stmt.query(params![token_id]).map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_token_tenant_grant(row)?);
        }
        Ok(items)
    }

    pub fn append_tenant_audit_event(
        &self,
        event: &kura_identity::TenantAuditEvent,
    ) -> Result<kura_identity::TenantAuditEvent, String> {
        let mut event = event.clone();
        if event.audit_event_id.is_empty() {
            event.audit_event_id = new_tenant_audit_event_id();
        }
        if event.created_at == chrono::DateTime::<chrono::Utc>::UNIX_EPOCH {
            event.created_at = chrono::Utc::now();
        }
        let document_json = match &event.document {
            None => None,
            Some(map) => Some(serde_json::to_string(map).map_err(|e| {
                format!(
                    "encode tenant audit event {} document: {e}",
                    event.audit_event_id
                )
            })?),
        };

        self.conn
            .execute(
                r#"INSERT INTO tenant_audit_events (
                    audit_event_id, event_kind, tenant_id, principal_id, target_principal_id,
                    token_id, outcome, reason_code, created_at, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"#,
                params![
                    event.audit_event_id,
                    event.event_kind,
                    null_string(&event.tenant_id),
                    null_string(&event.principal_id),
                    null_string(&event.target_principal_id),
                    null_string(&event.token_id),
                    event.outcome,
                    event.reason_code,
                    now_rfc3339(&event.created_at),
                    document_json,
                ],
            )
            .map_err(|e| format!("append tenant audit event {}: {e}", event.audit_event_id))?;
        Ok(event)
    }

    pub fn list_tenant_audit_events(
        &self,
        filter: &kura_identity::AuditEventFilter,
    ) -> Result<Vec<kura_identity::TenantAuditEvent>, String> {
        let mut sql = String::from(
            r#"SELECT audit_event_id, event_kind, tenant_id, principal_id, target_principal_id,
                token_id, outcome, reason_code, created_at, document_json
            FROM tenant_audit_events
            WHERE 1 = 1"#,
        );
        let mut args: Vec<Value> = Vec::new();
        if !filter.tenant_id.is_empty() {
            sql.push_str(" AND tenant_id = ?");
            args.push(Value::Text(filter.tenant_id.clone()));
        }
        if !filter.principal_id.is_empty() {
            sql.push_str(" AND principal_id = ?");
            args.push(Value::Text(filter.principal_id.clone()));
        }
        if !filter.token_id.is_empty() {
            sql.push_str(" AND token_id = ?");
            args.push(Value::Text(filter.token_id.clone()));
        }
        if !filter.event_kind.is_empty() {
            sql.push_str(" AND event_kind = ?");
            args.push(Value::Text(filter.event_kind.clone()));
        }
        if !filter.outcome.is_empty() {
            sql.push_str(" AND outcome = ?");
            args.push(Value::Text(filter.outcome.clone()));
        }
        sql.push_str(" ORDER BY created_at DESC, audit_event_id DESC");
        if filter.limit > 0 {
            sql.push_str(" LIMIT ?");
            args.push(Value::Integer(filter.limit as i64));
        }

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("list tenant audit events: {e}"))?;
        let mut rows = stmt
            .query(params_from_iter(&args))
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_tenant_audit_event(row)?);
        }
        Ok(items)
    }
}

// --- kura_identity::Store trait implementation -------------------------------
//
// The manager-facing persistence surface (rs/identity/src/manager.rs) is backed
// by the sync DAOs above; every store failure is wrapped into
// IdentityError::Store so the identity manager can surface it without losing
// the underlying message.

/// Minimal Send+Sync error wrapper for String store failures.
#[derive(Debug)]
struct IdentityStoreError(String);

impl std::fmt::Display for IdentityStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for IdentityStoreError {}

fn identity_store_err(message: String) -> kura_identity::IdentityError {
    kura_identity::IdentityError::Store(Box::new(IdentityStoreError(message)))
}

impl kura_identity::ResolverStore for SQLiteStore {
    fn get_principal(
        &self,
        principal_id: &str,
    ) -> Result<Option<kura_identity::Principal>, kura_identity::IdentityError> {
        self.get_principal(principal_id).map_err(identity_store_err)
    }

    fn get_tenant(
        &self,
        tenant_id: &str,
    ) -> Result<Option<kura_identity::Tenant>, kura_identity::IdentityError> {
        self.get_tenant(tenant_id).map_err(identity_store_err)
    }

    fn list_memberships(
        &self,
        filter: &kura_identity::MembershipFilter,
    ) -> Result<Vec<kura_identity::Membership>, kura_identity::IdentityError> {
        self.list_memberships(filter).map_err(identity_store_err)
    }

    fn list_token_tenant_grants(
        &self,
        token_id: &str,
    ) -> Result<Vec<kura_identity::TokenTenantGrant>, kura_identity::IdentityError> {
        self.list_token_tenant_grants(token_id)
            .map_err(identity_store_err)
    }
}

impl kura_identity::AuditStore for SQLiteStore {
    fn append_tenant_audit_event(
        &self,
        event: kura_identity::TenantAuditEvent,
    ) -> Result<kura_identity::TenantAuditEvent, kura_identity::IdentityError> {
        self.append_tenant_audit_event(&event)
            .map_err(identity_store_err)
    }
}

impl kura_identity::Store for SQLiteStore {
    fn upsert_tenant(
        &self,
        tenant: &kura_identity::Tenant,
    ) -> Result<(), kura_identity::IdentityError> {
        self.upsert_tenant(tenant).map_err(identity_store_err)
    }

    fn upsert_principal(
        &self,
        principal: &kura_identity::Principal,
    ) -> Result<(), kura_identity::IdentityError> {
        self.upsert_principal(principal).map_err(identity_store_err)
    }

    fn upsert_membership(
        &self,
        membership: &kura_identity::Membership,
    ) -> Result<(), kura_identity::IdentityError> {
        self.upsert_membership(membership)
            .map_err(identity_store_err)
    }

    fn upsert_tenant_invitation(
        &self,
        invitation: &kura_identity::TenantInvitation,
    ) -> Result<(), kura_identity::IdentityError> {
        self.upsert_tenant_invitation(invitation)
            .map_err(identity_store_err)
    }

    fn upsert_token_tenant_grant(
        &self,
        grant: &kura_identity::TokenTenantGrant,
    ) -> Result<(), kura_identity::IdentityError> {
        self.upsert_token_tenant_grant(grant)
            .map_err(identity_store_err)
    }

    fn list_tenants(
        &self,
        filter: &kura_identity::TenantFilter,
    ) -> Result<Vec<kura_identity::Tenant>, kura_identity::IdentityError> {
        self.list_tenants(filter).map_err(identity_store_err)
    }

    fn list_principals(
        &self,
        filter: &kura_identity::PrincipalFilter,
    ) -> Result<Vec<kura_identity::Principal>, kura_identity::IdentityError> {
        self.list_principals(filter).map_err(identity_store_err)
    }

    fn list_tenant_invitations(
        &self,
        filter: &kura_identity::InvitationFilter,
    ) -> Result<Vec<kura_identity::TenantInvitation>, kura_identity::IdentityError> {
        self.list_tenant_invitations(filter)
            .map_err(identity_store_err)
    }

    fn list_token_authorities(
        &self,
    ) -> Result<Vec<kura_identity::TokenAuthority>, kura_identity::IdentityError> {
        self.list_token_authorities().map_err(identity_store_err)
    }
}
