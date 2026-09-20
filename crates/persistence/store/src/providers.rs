//! SQLite CRUD for provider records (checks, auth states, models, preferences). Ported from
//! `daemon/internal/store/store.go` tenantless write paths. The tenant column on auth states,
//! models, and preferences is written as NULL until the tenancy package is ported.

use rusqlite::{Row, params};

use crate::SQLiteStore;
use crate::crud::{
    decode_opt_json, enum_str, marshal_json, now_rfc3339, null_string, opt_time_string, parse_enum,
    parse_opt_rfc3339, parse_rfc3339,
};

/// Go marshals nil slices/maps as the literal `null`; Go-era rows carry it in
/// these NOT NULL json columns. Decode ""/"null" as the type's default.
fn decode_json_or_default<T: serde::de::DeserializeOwned + Default>(
    raw: &str,
) -> Result<T, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "null" {
        return Ok(T::default());
    }
    serde_json::from_str(trimmed).map_err(|e| e.to_string())
}

fn scan_provider_check(row: &Row) -> Result<kura_providers::Check, String> {
    let check_id: String = row.get(0).map_err(|e| e.to_string())?;
    let provider_id: String = row.get(1).map_err(|e| e.to_string())?;
    let family: String = row.get(2).map_err(|e| e.to_string())?;
    let auth_mode: String = row.get(3).map_err(|e| e.to_string())?;
    let status: String = row.get(4).map_err(|e| e.to_string())?;
    let model: String = row.get(5).map_err(|e| e.to_string())?;
    let endpoint: Option<String> = row.get(6).map_err(|e| e.to_string())?;
    let error_class: Option<String> = row.get(7).map_err(|e| e.to_string())?;
    let error_code: Option<String> = row.get(8).map_err(|e| e.to_string())?;
    let error_message: Option<String> = row.get(9).map_err(|e| e.to_string())?;
    let usage_raw: String = row.get(10).map_err(|e| e.to_string())?;
    let created_at: String = row.get(11).map_err(|e| e.to_string())?;
    let completed_at: String = row.get(12).map_err(|e| e.to_string())?;

    Ok(kura_providers::Check {
        check_id,
        provider_id,
        family: parse_enum(&family)?,
        auth_mode: parse_enum(&auth_mode)?,
        status: parse_enum(&status)?,
        model,
        endpoint: endpoint.unwrap_or_default(),
        error_class: error_class.unwrap_or_default(),
        error_code: error_code.unwrap_or_default(),
        error_message: error_message.unwrap_or_default(),
        usage: decode_json_or_default(&usage_raw)
            .map_err(|e| format!("decode provider check usage: {e}"))?,
        created_at: parse_rfc3339(&created_at)?,
        completed_at: parse_rfc3339(&completed_at)?,
    })
}

fn scan_provider_auth_state(row: &Row) -> Result<kura_providers::AuthState, String> {
    let provider_id: String = row.get(0).map_err(|e| e.to_string())?;
    let tenant_id: Option<String> = row.get(1).map_err(|e| e.to_string())?;
    let family: String = row.get(2).map_err(|e| e.to_string())?;
    let auth_mode: String = row.get(3).map_err(|e| e.to_string())?;
    let status: String = row.get(4).map_err(|e| e.to_string())?;
    let cli_path: Option<String> = row.get(5).map_err(|e| e.to_string())?;
    let cli_available: bool = row.get(6).map_err(|e| e.to_string())?;
    let account_label: Option<String> = row.get(7).map_err(|e| e.to_string())?;
    let account_id: Option<String> = row.get(8).map_err(|e| e.to_string())?;
    let plan: Option<String> = row.get(9).map_err(|e| e.to_string())?;
    let auth_method: Option<String> = row.get(10).map_err(|e| e.to_string())?;
    let login_command_raw: String = row.get(11).map_err(|e| e.to_string())?;
    let logout_command_raw: String = row.get(12).map_err(|e| e.to_string())?;
    let last_checked_at: String = row.get(13).map_err(|e| e.to_string())?;
    let last_authenticated_at: Option<String> = row.get(14).map_err(|e| e.to_string())?;
    let last_error: Option<String> = row.get(15).map_err(|e| e.to_string())?;
    let metadata_raw: String = row.get(16).map_err(|e| e.to_string())?;
    let sandbox_raw: Option<String> = row.get(17).map_err(|e| e.to_string())?;

    Ok(kura_providers::AuthState {
        provider_id,
        tenant_id: tenant_id.unwrap_or_default(),
        family: parse_enum(&family)?,
        auth_mode: parse_enum(&auth_mode)?,
        status: parse_enum(&status)?,
        cli_path: cli_path.unwrap_or_default(),
        cli_available,
        account_label: account_label.unwrap_or_default(),
        account_id: account_id.unwrap_or_default(),
        plan: plan.unwrap_or_default(),
        auth_method: auth_method.unwrap_or_default(),
        login_command: decode_json_or_default(&login_command_raw)
            .map_err(|e| format!("decode provider auth login command: {e}"))?,
        logout_command: decode_json_or_default(&logout_command_raw)
            .map_err(|e| format!("decode provider auth logout command: {e}"))?,
        last_checked_at: parse_rfc3339(&last_checked_at)?,
        last_authenticated_at: parse_opt_rfc3339(last_authenticated_at)?,
        last_error: last_error.unwrap_or_default(),
        metadata: decode_json_or_default(&metadata_raw)
            .map_err(|e| format!("decode provider auth metadata: {e}"))?,
        sandbox: decode_opt_json(&sandbox_raw)?,
    })
}

fn scan_provider_model(row: &Row) -> Result<kura_providers::Model, String> {
    let provider_id: String = row.get(0).map_err(|e| e.to_string())?;
    let model_id: String = row.get(1).map_err(|e| e.to_string())?;
    let display_name: String = row.get(2).map_err(|e| e.to_string())?;
    let description: Option<String> = row.get(3).map_err(|e| e.to_string())?;
    let default_flag: bool = row.get(4).map_err(|e| e.to_string())?;
    let available_flag: bool = row.get(5).map_err(|e| e.to_string())?;
    let source: String = row.get(6).map_err(|e| e.to_string())?;
    let chat: bool = row.get(7).map_err(|e| e.to_string())?;
    let stream: bool = row.get(8).map_err(|e| e.to_string())?;
    let coding: bool = row.get(9).map_err(|e| e.to_string())?;
    let tool_use: bool = row.get(10).map_err(|e| e.to_string())?;
    let reasoning_levels_raw: String = row.get(11).map_err(|e| e.to_string())?;

    Ok(kura_providers::Model {
        provider_id,
        model_id,
        display_name,
        description: description.unwrap_or_default(),
        default: default_flag,
        available: available_flag,
        source,
        chat,
        stream,
        coding,
        tool_use,
        reasoning_levels: decode_json_or_default(&reasoning_levels_raw)
            .map_err(|e| format!("decode provider model reasoning levels: {e}"))?,
    })
}

fn scan_provider_preference(row: &Row) -> Result<kura_providers::Preference, String> {
    let provider_id: String = row.get(0).map_err(|e| e.to_string())?;
    let default_model: String = row.get(1).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(2).map_err(|e| e.to_string())?;
    Ok(kura_providers::Preference {
        provider_id,
        default_model,
        updated_at: parse_rfc3339(&updated_at)?,
    })
}

impl SQLiteStore {
    pub fn upsert_provider_check(&self, check: &kura_providers::Check) -> Result<(), String> {
        let usage_json = serde_json::to_string(&check.usage)
            .map_err(|e| format!("marshal provider check usage: {e}"))?;
        self.conn
            .execute(
                r#"INSERT INTO provider_checks (
                    check_id, provider_id, family, auth_mode, status, model, endpoint,
                    error_class, error_code, error_message, usage_json, created_at, completed_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                ON CONFLICT(check_id) DO UPDATE SET
                    provider_id = excluded.provider_id,
                    family = excluded.family,
                    auth_mode = excluded.auth_mode,
                    status = excluded.status,
                    model = excluded.model,
                    endpoint = excluded.endpoint,
                    error_class = excluded.error_class,
                    error_code = excluded.error_code,
                    error_message = excluded.error_message,
                    usage_json = excluded.usage_json,
                    created_at = excluded.created_at,
                    completed_at = excluded.completed_at"#,
                params![
                    check.check_id,
                    check.provider_id,
                    enum_str(&check.family),
                    enum_str(&check.auth_mode),
                    enum_str(&check.status),
                    check.model,
                    null_string(&check.endpoint),
                    null_string(&check.error_class),
                    null_string(&check.error_code),
                    null_string(&check.error_message),
                    usage_json,
                    now_rfc3339(&check.created_at),
                    now_rfc3339(&check.completed_at),
                ],
            )
            .map_err(|e| format!("upsert provider check {}: {e}", check.check_id))?;
        Ok(())
    }

    pub fn list_provider_checks(
        &self,
        provider_id: &str,
    ) -> Result<Vec<kura_providers::Check>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT check_id, provider_id, family, auth_mode, status, model, endpoint,
                    error_class, error_code, error_message, usage_json, created_at, completed_at
                FROM provider_checks
                WHERE provider_id = ?1
                ORDER BY created_at DESC, check_id DESC"#,
            )
            .map_err(|e| format!("list provider checks for {provider_id}: {e}"))?;
        let mut rows = stmt
            .query(params![provider_id])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_provider_check(row)?);
        }
        Ok(items)
    }

    pub fn get_provider_check(
        &self,
        provider_id: &str,
        check_id: &str,
    ) -> Result<Option<kura_providers::Check>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT check_id, provider_id, family, auth_mode, status, model, endpoint,
                    error_class, error_code, error_message, usage_json, created_at, completed_at
                FROM provider_checks
                WHERE provider_id = ?1 AND check_id = ?2"#,
            )
            .map_err(|e| e.to_string())?;
        let mut rows = stmt
            .query(params![provider_id, check_id])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        scan_provider_check(row).map(Some)
    }

    pub fn upsert_provider_auth_state(
        &self,
        state: &kura_providers::AuthState,
    ) -> Result<(), String> {
        let login_command_json = serde_json::to_string(&state.login_command)
            .map_err(|e| format!("marshal provider auth login command: {e}"))?;
        let logout_command_json = serde_json::to_string(&state.logout_command)
            .map_err(|e| format!("marshal provider auth logout command: {e}"))?;
        let metadata_json = serde_json::to_string(&state.metadata)
            .map_err(|e| format!("marshal provider auth metadata: {e}"))?;
        let sandbox_json = marshal_json(&state.sandbox)?;

        self.conn
            .execute(
                r#"INSERT INTO provider_auth_states (
                    provider_id, tenant_id, family, auth_mode, status, cli_path, cli_available,
                    account_label, account_id, plan, auth_method, login_command_json,
                    logout_command_json, last_checked_at, last_authenticated_at, last_error,
                    metadata_json, sandbox_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
                ON CONFLICT(provider_id) DO UPDATE SET
                    tenant_id = excluded.tenant_id,
                    family = excluded.family,
                    auth_mode = excluded.auth_mode,
                    status = excluded.status,
                    cli_path = excluded.cli_path,
                    cli_available = excluded.cli_available,
                    account_label = excluded.account_label,
                    account_id = excluded.account_id,
                    plan = excluded.plan,
                    auth_method = excluded.auth_method,
                    login_command_json = excluded.login_command_json,
                    logout_command_json = excluded.logout_command_json,
                    last_checked_at = excluded.last_checked_at,
                    last_authenticated_at = excluded.last_authenticated_at,
                    last_error = excluded.last_error,
                    metadata_json = excluded.metadata_json,
                    sandbox_json = excluded.sandbox_json"#,
                params![
                    state.provider_id,
                    null_string(&state.tenant_id),
                    enum_str(&state.family),
                    enum_str(&state.auth_mode),
                    enum_str(&state.status),
                    null_string(&state.cli_path),
                    state.cli_available,
                    null_string(&state.account_label),
                    null_string(&state.account_id),
                    null_string(&state.plan),
                    null_string(&state.auth_method),
                    login_command_json,
                    logout_command_json,
                    now_rfc3339(&state.last_checked_at),
                    opt_time_string(&state.last_authenticated_at),
                    null_string(&state.last_error),
                    metadata_json,
                    sandbox_json,
                ],
            )
            .map_err(|e| format!("upsert provider auth state {}: {e}", state.provider_id))?;
        Ok(())
    }

    pub fn list_provider_auth_states(&self) -> Result<Vec<kura_providers::AuthState>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT provider_id, tenant_id, family, auth_mode, status, cli_path, cli_available,
                    account_label, account_id, plan, auth_method, login_command_json,
                    logout_command_json, last_checked_at, last_authenticated_at, last_error,
                    metadata_json, sandbox_json
                FROM provider_auth_states
                ORDER BY provider_id ASC"#,
            )
            .map_err(|e| format!("list provider auth states: {e}"))?;
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_provider_auth_state(row)?);
        }
        Ok(items)
    }

    pub fn replace_provider_models(
        &self,
        provider_id: &str,
        models: &[kura_providers::Model],
    ) -> Result<(), String> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin provider model replace transaction: {e}"))?;

        tx.execute(
            "DELETE FROM provider_models WHERE provider_id = ?1",
            params![provider_id],
        )
        .map_err(|e| format!("delete provider models for {provider_id}: {e}"))?;

        for model in models {
            let reasoning_levels_json =
                serde_json::to_string(&model.reasoning_levels).map_err(|e| {
                    format!(
                        "marshal reasoning levels for {provider_id}/{}: {e}",
                        model.model_id
                    )
                })?;
            tx.execute(
                r#"INSERT INTO provider_models (
                    provider_id, model_id, display_name, description, default_flag,
                    available_flag, source, chat, stream, coding, tool_use, reasoning_levels_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"#,
                params![
                    model.provider_id,
                    model.model_id,
                    model.display_name,
                    null_string(&model.description),
                    model.default,
                    model.available,
                    model.source,
                    model.chat,
                    model.stream,
                    model.coding,
                    model.tool_use,
                    reasoning_levels_json,
                ],
            )
            .map_err(|e| {
                format!(
                    "insert provider model {provider_id}/{}: {e}",
                    model.model_id
                )
            })?;
        }

        tx.commit()
            .map_err(|e| format!("commit provider model replace transaction: {e}"))
    }

    pub fn list_provider_models(&self) -> Result<Vec<kura_providers::Model>, String> {
        self.query_provider_models(None)
    }

    pub fn list_provider_models_by_provider(
        &self,
        provider_id: &str,
    ) -> Result<Vec<kura_providers::Model>, String> {
        self.query_provider_models(Some(provider_id))
    }

    pub fn upsert_provider_preference(
        &self,
        preference: &kura_providers::Preference,
    ) -> Result<(), String> {
        self.conn
            .execute(
                r#"INSERT INTO provider_preferences (provider_id, default_model, updated_at, tenant_id)
                VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(provider_id) DO UPDATE SET
                    default_model = excluded.default_model,
                    updated_at = excluded.updated_at,
                    tenant_id = COALESCE(provider_preferences.tenant_id, excluded.tenant_id)"#,
                params![
                    preference.provider_id,
                    preference.default_model,
                    now_rfc3339(&preference.updated_at),
                    None::<String>,
                ],
            )
            .map_err(|e| format!("upsert provider preference {}: {e}", preference.provider_id))?;
        Ok(())
    }

    pub fn list_provider_preferences(&self) -> Result<Vec<kura_providers::Preference>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT provider_id, default_model, updated_at FROM provider_preferences ORDER BY provider_id ASC",
            )
            .map_err(|e| format!("list provider preferences: {e}"))?;
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_provider_preference(row)?);
        }
        Ok(items)
    }

    /// Persist a role assignment. Roles are keyed by name, so this replaces
    /// any existing binding for that role.
    pub fn upsert_model_role_binding(
        &self,
        binding: &kura_providers::RoleBinding,
    ) -> Result<(), String> {
        self.conn
            .execute(
                r#"INSERT INTO model_role_bindings (role, tenant_id, provider_id, model, updated_at)
                VALUES (?1, ?2, ?3, ?4, ?5)
                ON CONFLICT(role, tenant_id) DO UPDATE SET
                    provider_id = excluded.provider_id,
                    model = excluded.model,
                    updated_at = excluded.updated_at"#,
                params![
                    binding.role.as_str(),
                    None::<String>,
                    binding.provider_id,
                    binding.model,
                    now_rfc3339(&binding.updated_at),
                ],
            )
            .map_err(|e| format!("upsert model role binding {}: {e}", binding.role.as_str()))?;
        Ok(())
    }

    /// Clear a role assignment so the configured default applies again.
    /// Deleting an absent row is not an error.
    pub fn delete_model_role_binding(&self, role: kura_providers::ModelRole) -> Result<(), String> {
        self.conn
            .execute(
                "DELETE FROM model_role_bindings WHERE role = ?1 AND tenant_id IS NULL",
                params![role.as_str()],
            )
            .map_err(|e| format!("delete model role binding {}: {e}", role.as_str()))?;
        Ok(())
    }

    pub fn list_model_role_bindings(&self) -> Result<Vec<kura_providers::RoleBinding>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT role, provider_id, model, updated_at FROM model_role_bindings \
                 WHERE tenant_id IS NULL ORDER BY role ASC",
            )
            .map_err(|e| format!("list model role bindings: {e}"))?;
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let raw: String = row.get(0).map_err(|e| e.to_string())?;
            // A row whose role name is no longer recognized is skipped rather
            // than failing the read: an older daemon must not be bricked by a
            // role a newer one wrote.
            let Some(role) = kura_providers::ModelRole::parse(&raw) else {
                continue;
            };
            items.push(kura_providers::RoleBinding {
                role,
                provider_id: row.get(1).map_err(|e| e.to_string())?,
                model: row.get(2).map_err(|e| e.to_string())?,
                updated_at: parse_rfc3339(&row.get::<_, String>(3).map_err(|e| e.to_string())?)?,
            });
        }
        Ok(items)
    }

    fn query_provider_models(
        &self,
        provider_id: Option<&str>,
    ) -> Result<Vec<kura_providers::Model>, String> {
        let sql = if provider_id.is_some() {
            r#"SELECT provider_id, model_id, display_name, description, default_flag,
                available_flag, source, chat, stream, coding, tool_use, reasoning_levels_json
            FROM provider_models
            WHERE provider_id = ?1
            ORDER BY model_id ASC"#
        } else {
            r#"SELECT provider_id, model_id, display_name, description, default_flag,
                available_flag, source, chat, stream, coding, tool_use, reasoning_levels_json
            FROM provider_models
            ORDER BY provider_id ASC, model_id ASC"#
        };
        let mut stmt = self.conn.prepare(sql).map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        match provider_id {
            Some(pid) => {
                let mut rows = stmt.query(params![pid]).map_err(|e| e.to_string())?;
                while let Some(row) = rows.next().map_err(|e| e.to_string())? {
                    items.push(scan_provider_model(row)?);
                }
            }
            None => {
                let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
                while let Some(row) = rows.next().map_err(|e| e.to_string())? {
                    items.push(scan_provider_model(row)?);
                }
            }
        }
        Ok(items)
    }
}
