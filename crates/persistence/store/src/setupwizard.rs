//! SQLite CRUD for the setup wizard (migration r46 `setup_sessions` /
//! `setup_attempts` tables) plus the `kura_setupwizard::Store` trait impl.
//! Follows the evaluation.rs convention: `document_json` holds the whole
//! serialized domain value and the denormalized columns drive filtering.
//! Session ids are deterministic (`setup_<tenant>_<target>_<style>`, see
//! setupwizard/helpers.rs), so the `UNIQUE(tenant_id, target_id,
//! setup_style)` constraint is satisfied by the upsert key.

use rusqlite::{params, params_from_iter, types::Value};

use crate::SQLiteStore;
use crate::crud::{enum_str, now_rfc3339, null_string};

impl SQLiteStore {
    pub fn save_setup_session(&self, item: &kura_setupwizard::SetupSession) -> Result<(), String> {
        let document_json =
            serde_json::to_string(item).map_err(|e| format!("marshal setup session: {e}"))?;
        self.conn
            .execute(
                r#"INSERT INTO setup_sessions (
                    setup_session_id, tenant_id, actor_principal_id, target_id, target_kind,
                    setup_style, state, reason_code, diagnostic_result_id, redaction_status,
                    created_at, updated_at, last_transition_at, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                ON CONFLICT(setup_session_id) DO UPDATE SET
                    tenant_id = excluded.tenant_id,
                    actor_principal_id = excluded.actor_principal_id,
                    target_id = excluded.target_id,
                    target_kind = excluded.target_kind,
                    setup_style = excluded.setup_style,
                    state = excluded.state,
                    reason_code = excluded.reason_code,
                    diagnostic_result_id = excluded.diagnostic_result_id,
                    redaction_status = excluded.redaction_status,
                    created_at = excluded.created_at,
                    updated_at = excluded.updated_at,
                    last_transition_at = excluded.last_transition_at,
                    document_json = excluded.document_json"#,
                params![
                    item.setup_session_id,
                    item.tenant_id,
                    null_string(&item.actor_principal_id),
                    item.target_id,
                    enum_str(&item.target_kind),
                    enum_str(&item.setup_style),
                    enum_str(&item.state),
                    null_string(&item.reason_code),
                    null_string(&item.diagnostic_result_id),
                    enum_str(&item.redaction_status),
                    now_rfc3339(&item.created_at),
                    now_rfc3339(&item.updated_at),
                    now_rfc3339(&item.last_transition_at),
                    document_json,
                ],
            )
            .map_err(|e| format!("save setup session {}: {e}", item.setup_session_id))?;
        Ok(())
    }

    pub fn get_setup_session(
        &self,
        tenant_id: &str,
        session_id: &str,
    ) -> Result<Option<kura_setupwizard::SetupSession>, String> {
        let mut sql =
            String::from("SELECT document_json FROM setup_sessions WHERE setup_session_id = ?");
        let mut args: Vec<Value> = vec![Value::Text(session_id.trim().to_string())];
        if !tenant_id.trim().is_empty() {
            sql.push_str(" AND tenant_id = ?");
            args.push(Value::Text(tenant_id.trim().to_string()));
        }
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("get setup session {session_id}: {e}"))?;
        let mut rows = stmt
            .query(params_from_iter(&args))
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let raw: String = row.get(0).map_err(|e| e.to_string())?;
        let item = serde_json::from_str(&raw)
            .map_err(|e| format!("decode setup session {session_id}: {e}"))?;
        Ok(Some(item))
    }

    pub fn list_setup_sessions(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<kura_setupwizard::SetupSession>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT document_json FROM setup_sessions WHERE tenant_id = ?1 ORDER BY updated_at DESC, setup_session_id DESC",
            )
            .map_err(|e| format!("list setup sessions: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id.trim()])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let raw: String = row.get(0).map_err(|e| e.to_string())?;
            let item =
                serde_json::from_str(&raw).map_err(|e| format!("decode setup session: {e}"))?;
            items.push(item);
        }
        Ok(items)
    }

    pub fn append_setup_attempt(
        &self,
        item: &kura_setupwizard::SetupAttempt,
    ) -> Result<(), String> {
        let document_json =
            serde_json::to_string(item).map_err(|e| format!("marshal setup attempt: {e}"))?;
        self.conn
            .execute(
                r#"INSERT INTO setup_attempts (
                    attempt_id, setup_session_id, tenant_id, actor_principal_id, operation,
                    from_state, to_state, reason_code, diagnostic_result_id, redaction_status,
                    created_at, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                ON CONFLICT(attempt_id) DO UPDATE SET
                    setup_session_id = excluded.setup_session_id,
                    tenant_id = excluded.tenant_id,
                    actor_principal_id = excluded.actor_principal_id,
                    operation = excluded.operation,
                    from_state = excluded.from_state,
                    to_state = excluded.to_state,
                    reason_code = excluded.reason_code,
                    diagnostic_result_id = excluded.diagnostic_result_id,
                    redaction_status = excluded.redaction_status,
                    created_at = excluded.created_at,
                    document_json = excluded.document_json"#,
                params![
                    item.attempt_id,
                    item.setup_session_id,
                    item.tenant_id,
                    null_string(&item.actor_principal_id),
                    enum_str(&item.operation),
                    enum_str(&item.from_state),
                    enum_str(&item.to_state),
                    null_string(&item.reason_code),
                    null_string(&item.diagnostic_result_id),
                    enum_str(&item.redaction_status),
                    now_rfc3339(&item.created_at),
                    document_json,
                ],
            )
            .map_err(|e| format!("append setup attempt {}: {e}", item.attempt_id))?;
        Ok(())
    }

    pub fn list_setup_attempts(
        &self,
        tenant_id: &str,
        session_id: &str,
    ) -> Result<Vec<kura_setupwizard::SetupAttempt>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT document_json FROM setup_attempts WHERE setup_session_id = ?1 AND tenant_id = ?2 ORDER BY created_at ASC, attempt_id ASC",
            )
            .map_err(|e| format!("list setup attempts: {e}"))?;
        let mut rows = stmt
            .query(params![session_id.trim(), tenant_id.trim()])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let raw: String = row.get(0).map_err(|e| e.to_string())?;
            let item =
                serde_json::from_str(&raw).map_err(|e| format!("decode setup attempt: {e}"))?;
            items.push(item);
        }
        Ok(items)
    }
}

// --- kura_setupwizard::Store trait impl (async wrapper over the DAOs) ---
//
// rusqlite's Connection is Send but not Sync, so SQLiteStore cannot be the
// trait's `Send + Sync` self type directly. The mutex is wrapped in the
// local `SetupWizardStoreHandle` newtype (same convention as
// SecretStoreHandle / ComputerUseStoreHandle).

/// Send + Sync handle over the SQLite store implementing
/// [`kura_setupwizard::Store`]. Construct from a fresh store and share as
/// `Arc<SetupWizardStoreHandle>` with the setup-wizard service.
pub struct SetupWizardStoreHandle(pub parking_lot::Mutex<SQLiteStore>);

impl SetupWizardStoreHandle {
    pub fn new(store: SQLiteStore) -> Self {
        Self(parking_lot::Mutex::new(store))
    }
}

impl kura_setupwizard::Store for SetupWizardStoreHandle {
    fn save_setup_session(
        &self,
        session: kura_setupwizard::SetupSession,
    ) -> kura_setupwizard::BoxFuture<'_, Result<(), kura_setupwizard::SetupError>> {
        Box::pin(async move {
            let store = self.0.lock();
            store
                .save_setup_session(&session)
                .map_err(kura_setupwizard::SetupError::Store)
        })
    }

    fn get_setup_session(
        &self,
        tenant_id: &str,
        session_id: &str,
    ) -> kura_setupwizard::BoxFuture<
        '_,
        Result<Option<kura_setupwizard::SetupSession>, kura_setupwizard::SetupError>,
    > {
        let tenant_id = tenant_id.to_string();
        let session_id = session_id.to_string();
        Box::pin(async move {
            let store = self.0.lock();
            store
                .get_setup_session(&tenant_id, &session_id)
                .map_err(kura_setupwizard::SetupError::Store)
        })
    }

    fn list_setup_sessions(
        &self,
        tenant_id: &str,
    ) -> kura_setupwizard::BoxFuture<
        '_,
        Result<Vec<kura_setupwizard::SetupSession>, kura_setupwizard::SetupError>,
    > {
        let tenant_id = tenant_id.to_string();
        Box::pin(async move {
            let store = self.0.lock();
            store
                .list_setup_sessions(&tenant_id)
                .map_err(kura_setupwizard::SetupError::Store)
        })
    }

    fn append_setup_attempt(
        &self,
        attempt: kura_setupwizard::SetupAttempt,
    ) -> kura_setupwizard::BoxFuture<'_, Result<(), kura_setupwizard::SetupError>> {
        Box::pin(async move {
            let store = self.0.lock();
            store
                .append_setup_attempt(&attempt)
                .map_err(kura_setupwizard::SetupError::Store)
        })
    }

    fn list_setup_attempts(
        &self,
        tenant_id: &str,
        session_id: &str,
    ) -> kura_setupwizard::BoxFuture<
        '_,
        Result<Vec<kura_setupwizard::SetupAttempt>, kura_setupwizard::SetupError>,
    > {
        let tenant_id = tenant_id.to_string();
        let session_id = session_id.to_string();
        Box::pin(async move {
            let store = self.0.lock();
            store
                .list_setup_attempts(&tenant_id, &session_id)
                .map_err(kura_setupwizard::SetupError::Store)
        })
    }
}
