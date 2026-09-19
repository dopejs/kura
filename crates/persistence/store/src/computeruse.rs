//! SQLite CRUD for the computer-use plane (sessions, actions, artifacts). Ported from
//! `daemon/internal/store/store.go` tenantless write paths. The tenant column on all three
//! tables is written as NULL until the tenancy package is ported.
//!
//! Persistence matches the Go implementation: each row carries an explicit column set for
//! filtering/ordering plus a full document_json snapshot of the struct, and reads decode
//! only document_json.

use rusqlite::{Row, params};

use crate::SQLiteStore;
use crate::crud::{enum_str, marshal_map, now_rfc3339, null_string, opt_time_string};

fn scan_session_document(row: &Row) -> Result<kura_computeruse::Session, String> {
    let document: String = row.get(0).map_err(|e| e.to_string())?;
    serde_json::from_str(&document).map_err(|e| format!("decode computer-use session: {e}"))
}

fn scan_action_document(row: &Row) -> Result<kura_computeruse::Action, String> {
    let document: String = row.get(0).map_err(|e| e.to_string())?;
    serde_json::from_str(&document).map_err(|e| format!("decode computer-use action: {e}"))
}

fn scan_artifact_document(row: &Row) -> Result<kura_computeruse::Artifact, String> {
    let document: String = row.get(0).map_err(|e| e.to_string())?;
    serde_json::from_str(&document).map_err(|e| format!("decode computer-use artifact: {e}"))
}

impl SQLiteStore {
    pub fn upsert_computer_use_session(
        &self,
        session: &kura_computeruse::Session,
    ) -> Result<(), String> {
        let document_json = serde_json::to_string(session)
            .map_err(|e| format!("marshal computer-use session: {e}"))?;
        let trusted_scope_json = match &session.trusted_page_scope {
            Some(scope) => Some(
                serde_json::to_string(scope)
                    .map_err(|e| format!("marshal trusted page scope: {e}"))?,
            ),
            None => None,
        };
        let current_page_json = match &session.current_page {
            Some(page) => Some(
                serde_json::to_string(page).map_err(|e| format!("marshal current page: {e}"))?,
            ),
            None => None,
        };

        self.conn
            .execute(
                r#"INSERT INTO computer_use_sessions (
                    computer_use_session_id, environment_scope, run_id, workflow_id,
                    workflow_step_id, status, driver_kind, trusted_page_scope_json,
                    current_page_json, last_action_id, started_at, updated_at, closed_at,
                    interrupted_at, document_json, tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
                ON CONFLICT(computer_use_session_id) DO UPDATE SET
                    environment_scope = excluded.environment_scope,
                    run_id = excluded.run_id,
                    workflow_id = excluded.workflow_id,
                    workflow_step_id = excluded.workflow_step_id,
                    status = excluded.status,
                    driver_kind = excluded.driver_kind,
                    trusted_page_scope_json = excluded.trusted_page_scope_json,
                    current_page_json = excluded.current_page_json,
                    last_action_id = excluded.last_action_id,
                    started_at = excluded.started_at,
                    updated_at = excluded.updated_at,
                    closed_at = excluded.closed_at,
                    interrupted_at = excluded.interrupted_at,
                    document_json = excluded.document_json,
                    tenant_id = COALESCE(computer_use_sessions.tenant_id, excluded.tenant_id)"#,
                params![
                    session.computer_use_session_id,
                    session.environment_scope,
                    session.run_id,
                    null_string(&session.workflow_id),
                    null_string(&session.workflow_step_id),
                    enum_str(&session.status),
                    session.driver_kind,
                    trusted_scope_json,
                    current_page_json,
                    null_string(&session.last_action_id),
                    now_rfc3339(&session.started_at),
                    now_rfc3339(&session.updated_at),
                    opt_time_string(&session.closed_at),
                    opt_time_string(&session.interrupted_at),
                    document_json,
                    None::<String>,
                ],
            )
            .map_err(|e| {
                format!(
                    "upsert computer-use session {}: {e}",
                    session.computer_use_session_id
                )
            })?;
        Ok(())
    }

    pub fn list_computer_use_sessions(
        &self,
        environment_scope: &str,
        run_id: &str,
    ) -> Result<Vec<kura_computeruse::Session>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT document_json
                FROM computer_use_sessions
                WHERE environment_scope = ?1 AND run_id = ?2
                ORDER BY updated_at DESC, computer_use_session_id DESC"#,
            )
            .map_err(|e| format!("list computer-use sessions: {e}"))?;
        let mut rows = stmt
            .query(params![environment_scope.trim(), run_id.trim()])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_session_document(row)?);
        }
        Ok(items)
    }

    pub fn get_computer_use_session(
        &self,
        environment_scope: &str,
        run_id: &str,
        session_id: &str,
    ) -> Result<Option<kura_computeruse::Session>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT document_json
                FROM computer_use_sessions
                WHERE environment_scope = ?1 AND run_id = ?2 AND computer_use_session_id = ?3"#,
            )
            .map_err(|e| format!("get computer-use session {session_id}: {e}"))?;
        let mut rows = stmt
            .query(params![
                environment_scope.trim(),
                run_id.trim(),
                session_id.trim()
            ])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        scan_session_document(row).map(Some)
    }
    pub fn upsert_computer_use_action(
        &self,
        action: &kura_computeruse::Action,
    ) -> Result<(), String> {
        let document_json = serde_json::to_string(action)
            .map_err(|e| format!("marshal computer-use action: {e}"))?;
        let target_match_context_json = match &action.target_match_context {
            Some(ctx) => Some(
                serde_json::to_string(ctx)
                    .map_err(|e| format!("marshal target match context: {e}"))?,
            ),
            None => None,
        };
        let page_before_json = match &action.page_before {
            Some(page) => {
                Some(serde_json::to_string(page).map_err(|e| format!("marshal page before: {e}"))?)
            }
            None => None,
        };
        let page_after_json = match &action.page_after {
            Some(page) => {
                Some(serde_json::to_string(page).map_err(|e| format!("marshal page after: {e}"))?)
            }
            None => None,
        };
        let input_json = marshal_map(&action.input)?;

        self.conn
            .execute(
                r#"INSERT INTO computer_use_actions (
                    computer_use_action_id, environment_scope, computer_use_session_id,
                    run_id, step_id, tool_call_id, workflow_id, workflow_step_id,
                    action_kind, status, risk_level, approval_id, target_match_context_json,
                    page_before_json, page_after_json, failure_class, failure_reason,
                    requested_at, updated_at, completed_at, input_json, document_json,
                    tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)
                ON CONFLICT(computer_use_action_id) DO UPDATE SET
                    environment_scope = excluded.environment_scope,
                    computer_use_session_id = excluded.computer_use_session_id,
                    run_id = excluded.run_id,
                    step_id = excluded.step_id,
                    tool_call_id = excluded.tool_call_id,
                    workflow_id = excluded.workflow_id,
                    workflow_step_id = excluded.workflow_step_id,
                    action_kind = excluded.action_kind,
                    status = excluded.status,
                    risk_level = excluded.risk_level,
                    approval_id = excluded.approval_id,
                    target_match_context_json = excluded.target_match_context_json,
                    page_before_json = excluded.page_before_json,
                    page_after_json = excluded.page_after_json,
                    failure_class = excluded.failure_class,
                    failure_reason = excluded.failure_reason,
                    requested_at = excluded.requested_at,
                    updated_at = excluded.updated_at,
                    completed_at = excluded.completed_at,
                    input_json = excluded.input_json,
                    document_json = excluded.document_json,
                    tenant_id = COALESCE(computer_use_actions.tenant_id, excluded.tenant_id)"#,
                params![
                    action.computer_use_action_id,
                    action.environment_scope,
                    action.computer_use_session_id,
                    action.run_id,
                    null_string(&action.step_id),
                    null_string(&action.tool_call_id),
                    null_string(&action.workflow_id),
                    null_string(&action.workflow_step_id),
                    enum_str(&action.action_kind),
                    enum_str(&action.status),
                    enum_str(&action.risk_level),
                    null_string(&action.approval_id),
                    target_match_context_json,
                    page_before_json,
                    page_after_json,
                    null_string(&action.failure_class),
                    null_string(&action.failure_reason),
                    now_rfc3339(&action.requested_at),
                    now_rfc3339(&action.updated_at),
                    opt_time_string(&action.completed_at),
                    input_json,
                    document_json,
                    None::<String>,
                ],
            )
            .map_err(|e| format!("upsert computer-use action {}: {e}", action.computer_use_action_id))?;
        Ok(())
    }

    pub fn list_computer_use_actions(
        &self,
        environment_scope: &str,
        run_id: &str,
        session_id: &str,
    ) -> Result<Vec<kura_computeruse::Action>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT document_json
                FROM computer_use_actions
                WHERE environment_scope = ?1 AND run_id = ?2 AND computer_use_session_id = ?3
                ORDER BY requested_at ASC, computer_use_action_id ASC"#,
            )
            .map_err(|e| format!("list computer-use actions: {e}"))?;
        let mut rows = stmt
            .query(params![
                environment_scope.trim(),
                run_id.trim(),
                session_id.trim()
            ])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_action_document(row)?);
        }
        Ok(items)
    }

    pub fn get_computer_use_action(
        &self,
        environment_scope: &str,
        run_id: &str,
        session_id: &str,
        action_id: &str,
    ) -> Result<Option<kura_computeruse::Action>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT document_json
                FROM computer_use_actions
                WHERE environment_scope = ?1 AND run_id = ?2 AND computer_use_session_id = ?3 AND computer_use_action_id = ?4"#,
            )
            .map_err(|e| format!("get computer-use action {action_id}: {e}"))?;
        let mut rows = stmt
            .query(params![
                environment_scope.trim(),
                run_id.trim(),
                session_id.trim(),
                action_id.trim(),
            ])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        scan_action_document(row).map(Some)
    }

    pub fn find_pending_computer_use_action_by_approval(
        &self,
        environment_scope: &str,
        approval_id: &str,
    ) -> Result<Option<kura_computeruse::Action>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT document_json
                FROM computer_use_actions
                WHERE environment_scope = ?1 AND approval_id = ?2 AND status = ?3
                ORDER BY requested_at DESC, computer_use_action_id DESC
                LIMIT 1"#,
            )
            .map_err(|e| format!("find pending computer-use action: {e}"))?;
        let mut rows = stmt
            .query(params![
                environment_scope.trim(),
                approval_id.trim(),
                kura_computeruse::ActionStatus::WaitingApproval.as_str(),
            ])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        scan_action_document(row).map(Some)
    }

    pub fn upsert_computer_use_artifact(
        &self,
        artifact: &kura_computeruse::Artifact,
    ) -> Result<(), String> {
        let document_json = serde_json::to_string(artifact)
            .map_err(|e| format!("marshal computer-use artifact: {e}"))?;

        self.conn
            .execute(
                r#"INSERT INTO computer_use_artifacts (
                    artifact_id, environment_scope, computer_use_session_id,
                    computer_use_action_id, run_id, kind, status, mime_type, file_name,
                    byte_size, storage_key, sha256, capture_failure_reason, created_at,
                    available_at, document_json, tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
                ON CONFLICT(artifact_id) DO UPDATE SET
                    environment_scope = excluded.environment_scope,
                    computer_use_session_id = excluded.computer_use_session_id,
                    computer_use_action_id = excluded.computer_use_action_id,
                    run_id = excluded.run_id,
                    kind = excluded.kind,
                    status = excluded.status,
                    mime_type = excluded.mime_type,
                    file_name = excluded.file_name,
                    byte_size = excluded.byte_size,
                    storage_key = excluded.storage_key,
                    sha256 = excluded.sha256,
                    capture_failure_reason = excluded.capture_failure_reason,
                    created_at = excluded.created_at,
                    available_at = excluded.available_at,
                    document_json = excluded.document_json,
                    tenant_id = COALESCE(computer_use_artifacts.tenant_id, excluded.tenant_id)"#,
                params![
                    artifact.artifact_id,
                    artifact.environment_scope,
                    artifact.computer_use_session_id,
                    artifact.computer_use_action_id,
                    artifact.run_id,
                    enum_str(&artifact.kind),
                    enum_str(&artifact.status),
                    null_string(&artifact.mime_type),
                    null_string(&artifact.file_name),
                    artifact.byte_size,
                    null_string(&artifact.storage_key),
                    null_string(&artifact.sha256),
                    null_string(&artifact.capture_failure_reason),
                    now_rfc3339(&artifact.created_at),
                    opt_time_string(&artifact.available_at),
                    document_json,
                    None::<String>,
                ],
            )
            .map_err(|e| format!("upsert computer-use artifact {}: {e}", artifact.artifact_id))?;
        Ok(())
    }

    pub fn list_computer_use_artifacts_for_action(
        &self,
        environment_scope: &str,
        run_id: &str,
        action_id: &str,
    ) -> Result<Vec<kura_computeruse::Artifact>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT document_json
                FROM computer_use_artifacts
                WHERE environment_scope = ?1 AND run_id = ?2 AND computer_use_action_id = ?3
                ORDER BY created_at ASC, artifact_id ASC"#,
            )
            .map_err(|e| format!("list computer-use artifacts: {e}"))?;
        let mut rows = stmt
            .query(params![
                environment_scope.trim(),
                run_id.trim(),
                action_id.trim()
            ])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_artifact_document(row)?);
        }
        Ok(items)
    }

    pub fn get_computer_use_artifact(
        &self,
        environment_scope: &str,
        artifact_id: &str,
    ) -> Result<Option<kura_computeruse::Artifact>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT document_json
                FROM computer_use_artifacts
                WHERE environment_scope = ?1 AND artifact_id = ?2"#,
            )
            .map_err(|e| format!("get computer-use artifact {artifact_id}: {e}"))?;
        let mut rows = stmt
            .query(params![environment_scope.trim(), artifact_id.trim()])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        scan_artifact_document(row).map(Some)
    }

    /// Marks every in-flight session (starting/active/blocked) and action
    /// (requested/waiting_approval/running) for the environment as interrupted, persisting the
    /// interruption and returning the updated rows. Mirrors
    /// `(*SQLiteStore).MarkInFlightComputerUseInterrupted`.
    pub fn mark_inflight_computer_use_interrupted(
        &self,
        environment_scope: &str,
        interrupted_at: &chrono::DateTime<chrono::Utc>,
    ) -> Result<
        (
            Vec<kura_computeruse::Session>,
            Vec<kura_computeruse::Action>,
        ),
        String,
    > {
        let mut updated_sessions = Vec::new();
        {
            let mut stmt = self
                .conn
                .prepare(
                    r#"SELECT document_json
                    FROM computer_use_sessions
                    WHERE environment_scope = ?1 AND status IN (?2, ?3, ?4)"#,
                )
                .map_err(|e| format!("list in-flight computer-use sessions: {e}"))?;
            let mut rows = stmt
                .query(params![
                    environment_scope.trim(),
                    kura_computeruse::SessionStatus::Starting.as_str(),
                    kura_computeruse::SessionStatus::Active.as_str(),
                    kura_computeruse::SessionStatus::Blocked.as_str(),
                ])
                .map_err(|e| e.to_string())?;
            while let Some(row) = rows.next().map_err(|e| e.to_string())? {
                updated_sessions.push(scan_session_document(row)?);
            }
        }

        for session in updated_sessions.iter_mut() {
            session.status = kura_computeruse::SessionStatus::Interrupted;
            session.interrupted_at = Some(*interrupted_at);
            session.updated_at = *interrupted_at;
            self.upsert_computer_use_session(session)?;
        }

        let mut updated_actions = Vec::new();
        {
            let mut stmt = self
                .conn
                .prepare(
                    r#"SELECT document_json
                    FROM computer_use_actions
                    WHERE environment_scope = ?1 AND status IN (?2, ?3, ?4)"#,
                )
                .map_err(|e| format!("list in-flight computer-use actions: {e}"))?;
            let mut rows = stmt
                .query(params![
                    environment_scope.trim(),
                    kura_computeruse::ActionStatus::Requested.as_str(),
                    kura_computeruse::ActionStatus::WaitingApproval.as_str(),
                    kura_computeruse::ActionStatus::Running.as_str(),
                ])
                .map_err(|e| e.to_string())?;
            while let Some(row) = rows.next().map_err(|e| e.to_string())? {
                updated_actions.push(scan_action_document(row)?);
            }
        }

        for action in updated_actions.iter_mut() {
            action.status = kura_computeruse::ActionStatus::Interrupted;
            action.failure_class = kura_computeruse::FailureClass::Interrupted
                .as_str()
                .to_string();
            action.failure_reason =
                "daemon restarted before computer-use action completed".to_string();
            action.updated_at = *interrupted_at;
            action.completed_at = Some(*interrupted_at);
            self.upsert_computer_use_action(action)?;
        }

        Ok((updated_sessions, updated_actions))
    }
}

// --- kura_computeruse::Store trait impl (Send + Sync handle over the DAOs) ---
//
// rusqlite's Connection is Send but not Sync, so SQLiteStore cannot be the
// trait's `Send + Sync` self type directly. Following the kura_secrets::Store
// precedent (see secrets.rs), the workspace convention shares the store as
// `Arc<parking_lot::Mutex<SQLiteStore>>`; the mutex is wrapped in the local
// `ComputerUseStoreHandle` newtype, which satisfies Send + Sync and serializes
// access exactly like the Go daemon's single-connection store.

/// Send + Sync handle over the SQLite store implementing
/// kura_computeruse::Store. Construct from a fresh store and share as
/// `Arc<ComputerUseStoreHandle>` with the computer-use manager.
pub struct ComputerUseStoreHandle(pub parking_lot::Mutex<SQLiteStore>);

impl ComputerUseStoreHandle {
    pub fn new(store: SQLiteStore) -> Self {
        Self(parking_lot::Mutex::new(store))
    }
}

impl kura_computeruse::Store for ComputerUseStoreHandle {
    fn upsert_computer_use_session(
        &self,
        session: &kura_computeruse::Session,
    ) -> Result<(), String> {
        self.0.lock().upsert_computer_use_session(session)
    }

    fn list_computer_use_sessions(
        &self,
        environment: &str,
        run_id: &str,
    ) -> Result<Vec<kura_computeruse::Session>, String> {
        self.0
            .lock()
            .list_computer_use_sessions(environment, run_id)
    }

    fn get_computer_use_session(
        &self,
        environment: &str,
        run_id: &str,
        session_id: &str,
    ) -> Result<Option<kura_computeruse::Session>, String> {
        self.0
            .lock()
            .get_computer_use_session(environment, run_id, session_id)
    }

    fn upsert_computer_use_action(&self, action: &kura_computeruse::Action) -> Result<(), String> {
        self.0.lock().upsert_computer_use_action(action)
    }

    fn list_computer_use_actions(
        &self,
        environment: &str,
        run_id: &str,
        session_id: &str,
    ) -> Result<Vec<kura_computeruse::Action>, String> {
        self.0
            .lock()
            .list_computer_use_actions(environment, run_id, session_id)
    }

    fn get_computer_use_action(
        &self,
        environment: &str,
        run_id: &str,
        session_id: &str,
        action_id: &str,
    ) -> Result<Option<kura_computeruse::Action>, String> {
        self.0
            .lock()
            .get_computer_use_action(environment, run_id, session_id, action_id)
    }

    fn find_pending_computer_use_action_by_approval(
        &self,
        environment: &str,
        approval_id: &str,
    ) -> Result<Option<kura_computeruse::Action>, String> {
        self.0
            .lock()
            .find_pending_computer_use_action_by_approval(environment, approval_id)
    }

    fn upsert_computer_use_artifact(
        &self,
        artifact: &kura_computeruse::Artifact,
    ) -> Result<(), String> {
        self.0.lock().upsert_computer_use_artifact(artifact)
    }

    fn list_computer_use_artifacts_for_action(
        &self,
        environment: &str,
        run_id: &str,
        action_id: &str,
    ) -> Result<Vec<kura_computeruse::Artifact>, String> {
        self.0
            .lock()
            .list_computer_use_artifacts_for_action(environment, run_id, action_id)
    }

    fn get_computer_use_artifact(
        &self,
        environment: &str,
        artifact_id: &str,
    ) -> Result<Option<kura_computeruse::Artifact>, String> {
        self.0
            .lock()
            .get_computer_use_artifact(environment, artifact_id)
    }

    fn mark_in_flight_computer_use_interrupted(
        &self,
        environment: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<
        (
            Vec<kura_computeruse::Session>,
            Vec<kura_computeruse::Action>,
        ),
        String,
    > {
        self.0
            .lock()
            .mark_inflight_computer_use_interrupted(environment, &now)
    }
}
