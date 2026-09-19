//! SQLite CRUD for the core run/step/tool-call lifecycle ledger plus checkpoints.
//!
//! Ported from `daemon/internal/store/store.go` (UpsertRun, RunTenantID, UpsertStep,
//! UpsertToolCall, ListToolCalls, ListSteps, SaveCheckpoint, ListLatestCheckpoints). The
//! tenant-binding variants (UpsertRunForTenantSafe &c.) are ported with the tenancy package;
//! these legacy paths write the pre-tenant column set so the migration + auth bootstrap flow
//! keeps working before a personal tenant exists.

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{Row, params};

use crate::SQLiteStore;

const RFC3339_NANO: SecondsFormat = SecondsFormat::Nanos;

pub(crate) fn now_rfc3339(dt: &DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(RFC3339_NANO, true)
}

pub(crate) fn parse_rfc3339(s: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| format!("parse timestamp {s}: {e}"))
}

pub(crate) fn opt_time_string(dt: &Option<DateTime<Utc>>) -> Option<String> {
    dt.as_ref().map(now_rfc3339)
}

pub(crate) fn parse_opt_rfc3339(raw: Option<String>) -> Result<Option<DateTime<Utc>>, String> {
    match raw {
        None => Ok(None),
        Some(s) if s.is_empty() => Ok(None),
        Some(s) => parse_rfc3339(&s).map(Some),
    }
}

pub(crate) fn parse_enum<T: serde::de::DeserializeOwned>(s: &str) -> Result<T, String> {
    serde_json::from_str(&format!("\"{s}\"")).map_err(|e| format!("invalid enum value {s}: {e}"))
}

pub(crate) fn enum_str<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value)
        .map(|s| s.trim_matches('"').to_string())
        .unwrap_or_default()
}

pub(crate) fn null_string(value: &str) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

pub(crate) fn marshal_json(value: &Option<serde_json::Value>) -> Result<Option<String>, String> {
    match value {
        None => Ok(None),
        Some(v) => Ok(Some(v.to_string())),
    }
}

pub(crate) fn marshal_map(
    value: &serde_json::Map<String, serde_json::Value>,
) -> Result<Option<String>, String> {
    if value.is_empty() {
        Ok(None)
    } else {
        Ok(Some(
            serde_json::to_string(value).map_err(|e| e.to_string())?,
        ))
    }
}

pub(crate) fn marshal_vec<T: serde::Serialize>(value: &[T]) -> Result<Option<String>, String> {
    if value.is_empty() {
        Ok(None)
    } else {
        Ok(Some(
            serde_json::to_string(value).map_err(|e| e.to_string())?,
        ))
    }
}

pub(crate) fn decode_opt_json(raw: &Option<String>) -> Result<Option<serde_json::Value>, String> {
    match raw {
        None => Ok(None),
        Some(s) if s.is_empty() => Ok(None),
        Some(s) => serde_json::from_str(s).map(Some).map_err(|e| e.to_string()),
    }
}

// Go marshals nil slices/maps as the literal `null`, and Go-era rows carry it
// in these JSON columns; treat it as empty like the absent-column cases.
pub(crate) fn decode_map(
    raw: &Option<String>,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    match raw {
        None => Ok(serde_json::Map::new()),
        Some(s) if s.is_empty() || s == "null" => Ok(serde_json::Map::new()),
        Some(s) => serde_json::from_str(s).map_err(|e| e.to_string()),
    }
}

/// Null-tolerant JSON decode for NOT NULL text columns: Go marshals nil
/// slices/maps/pointers as the literal `null` and Go-era rows carry it.
pub(crate) fn decode_json_field<T: serde::de::DeserializeOwned + Default>(
    raw: &str,
) -> Result<T, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "null" {
        return Ok(T::default());
    }
    serde_json::from_str(trimmed).map_err(|e| e.to_string())
}

pub(crate) fn decode_vec<T: serde::de::DeserializeOwned>(
    raw: &Option<String>,
) -> Result<Vec<T>, String> {
    match raw {
        None => Ok(Vec::new()),
        Some(s) if s.is_empty() || s == "null" => Ok(Vec::new()),
        Some(s) => serde_json::from_str(s).map_err(|e| e.to_string()),
    }
}

fn new_checkpoint_id() -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!("ckpt_{}", &hex[..16])
}

fn scan_step(row: &Row) -> Result<kura_runtime::Step, String> {
    let step_id: String = row.get(0).map_err(|e| e.to_string())?;
    let run_id: String = row.get(1).map_err(|e| e.to_string())?;
    let workflow_id: Option<String> = row.get(2).map_err(|e| e.to_string())?;
    let workflow_step_id: Option<String> = row.get(3).map_err(|e| e.to_string())?;
    let attempt: i64 = row.get(4).map_err(|e| e.to_string())?;
    let title: String = row.get(5).map_err(|e| e.to_string())?;
    let kind: String = row.get(6).map_err(|e| e.to_string())?;
    let status: String = row.get(7).map_err(|e| e.to_string())?;
    let input_json: Option<String> = row.get(8).map_err(|e| e.to_string())?;
    let output_json: Option<String> = row.get(9).map_err(|e| e.to_string())?;
    let created_at: String = row.get(10).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(11).map_err(|e| e.to_string())?;

    Ok(kura_runtime::Step {
        step_id,
        run_id,
        workflow_id: workflow_id.unwrap_or_default(),
        workflow_step_id: workflow_step_id.unwrap_or_default(),
        attempt,
        title,
        kind,
        status: parse_enum(&status)?,
        created_at: parse_rfc3339(&created_at)?,
        updated_at: parse_rfc3339(&updated_at)?,
        input: decode_opt_json(&input_json)?,
        output: decode_opt_json(&output_json)?,
    })
}

fn scan_tool_call(row: &Row) -> Result<kura_runtime::ToolCall, String> {
    let tool_call_id: String = row.get(0).map_err(|e| e.to_string())?;
    let run_id: String = row.get(1).map_err(|e| e.to_string())?;
    let step_id: String = row.get(2).map_err(|e| e.to_string())?;
    let workflow_id: Option<String> = row.get(3).map_err(|e| e.to_string())?;
    let workflow_step_id: Option<String> = row.get(4).map_err(|e| e.to_string())?;
    let attempt: i64 = row.get(5).map_err(|e| e.to_string())?;
    let computer_use_session_id: Option<String> = row.get(6).map_err(|e| e.to_string())?;
    let computer_use_action_id: Option<String> = row.get(7).map_err(|e| e.to_string())?;
    let invocation_kind: Option<String> = row.get(8).map_err(|e| e.to_string())?;
    let capability_id: String = row.get(9).map_err(|e| e.to_string())?;
    let skill_id: Option<String> = row.get(10).map_err(|e| e.to_string())?;
    let mcp_server_id: Option<String> = row.get(11).map_err(|e| e.to_string())?;
    let mcp_server_name: Option<String> = row.get(12).map_err(|e| e.to_string())?;
    let mcp_tool_name: Option<String> = row.get(13).map_err(|e| e.to_string())?;
    let mcp_transport_kind: Option<String> = row.get(14).map_err(|e| e.to_string())?;
    let mcp_session_id: Option<String> = row.get(15).map_err(|e| e.to_string())?;
    let authorization_result: Option<String> = row.get(16).map_err(|e| e.to_string())?;
    let tool_name: String = row.get(17).map_err(|e| e.to_string())?;
    let status: String = row.get(18).map_err(|e| e.to_string())?;
    let sandbox_execution_id: Option<String> = row.get(19).map_err(|e| e.to_string())?;
    let failure_class: Option<String> = row.get(20).map_err(|e| e.to_string())?;
    let input_json: Option<String> = row.get(21).map_err(|e| e.to_string())?;
    let output_json: Option<String> = row.get(22).map_err(|e| e.to_string())?;
    let sandbox_json: Option<String> = row.get(23).map_err(|e| e.to_string())?;
    let integration_bindings_json: Option<String> = row.get(24).map_err(|e| e.to_string())?;
    let error_text: Option<String> = row.get(25).map_err(|e| e.to_string())?;
    let created_at: String = row.get(26).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(27).map_err(|e| e.to_string())?;

    Ok(kura_runtime::ToolCall {
        tool_call_id,
        run_id,
        step_id,
        workflow_id: workflow_id.unwrap_or_default(),
        workflow_step_id: workflow_step_id.unwrap_or_default(),
        attempt,
        computer_use_session_id: computer_use_session_id.unwrap_or_default(),
        computer_use_action_id: computer_use_action_id.unwrap_or_default(),
        invocation_kind: invocation_kind.unwrap_or_default(),
        capability_id,
        skill_id: skill_id.unwrap_or_default(),
        mcp_server_id: mcp_server_id.unwrap_or_default(),
        mcp_server_name: mcp_server_name.unwrap_or_default(),
        mcp_tool_name: mcp_tool_name.unwrap_or_default(),
        mcp_transport_kind: mcp_transport_kind.unwrap_or_default(),
        mcp_session_id: mcp_session_id.unwrap_or_default(),
        authorization_result: authorization_result.unwrap_or_default(),
        tool_name,
        status: parse_enum(&status)?,
        sandbox_execution_id: sandbox_execution_id.unwrap_or_default(),
        failure_class: failure_class.unwrap_or_default(),
        created_at: parse_rfc3339(&created_at)?,
        updated_at: parse_rfc3339(&updated_at)?,
        input: decode_opt_json(&input_json)?,
        output: decode_opt_json(&output_json)?,
        sandbox: decode_map(&sandbox_json)?,
        integration_bindings: decode_vec(&integration_bindings_json)?,
        error: error_text.unwrap_or_default(),
        ..kura_runtime::ToolCall::default()
    })
}

impl SQLiteStore {
    pub fn upsert_run(&self, run: &kura_runtime::Run) -> Result<(), String> {
        self.conn
            .execute(
                r#"INSERT INTO runs (
                    run_id,
                    session_id,
                    schedule_id,
                    schedule_attempt_id,
                    reminder_id,
                    reminder_occurrence_id,
                    entrypoint,
                    status,
                    goal,
                    created_at,
                    updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                ON CONFLICT(run_id) DO UPDATE SET
                    session_id = excluded.session_id,
                    schedule_id = excluded.schedule_id,
                    schedule_attempt_id = excluded.schedule_attempt_id,
                    reminder_id = excluded.reminder_id,
                    reminder_occurrence_id = excluded.reminder_occurrence_id,
                    entrypoint = excluded.entrypoint,
                    status = excluded.status,
                    goal = excluded.goal,
                    created_at = excluded.created_at,
                    updated_at = excluded.updated_at"#,
                params![
                    run.run_id,
                    null_string(&run.session_id),
                    null_string(&run.schedule_id),
                    null_string(&run.schedule_attempt_id),
                    null_string(&run.reminder_id),
                    null_string(&run.reminder_occurrence_id),
                    run.entrypoint,
                    run.status.as_str(),
                    run.goal,
                    now_rfc3339(&run.created_at),
                    now_rfc3339(&run.updated_at),
                ],
            )
            .map_err(|e| format!("upsert run {}: {e}", run.run_id))?;
        Ok(())
    }

    pub fn run_tenant_id(&self, run_id: &str) -> Result<Option<String>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT tenant_id FROM runs WHERE run_id = ?1")
            .map_err(|e| format!("get run tenant {run_id}: {e}"))?;
        let mut rows = stmt
            .query(params![run_id.trim()])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let tenant_id: Option<String> = row.get(0).map_err(|e| e.to_string())?;
        Ok(tenant_id
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim().to_string()))
    }

    pub fn upsert_step(&self, step: &kura_runtime::Step) -> Result<(), String> {
        let input_json = marshal_json(&step.input)?;
        let output_json = marshal_json(&step.output)?;

        self.conn
            .execute(
                r#"INSERT INTO steps (
                    step_id,
                    run_id,
                    workflow_id,
                    workflow_step_id,
                    attempt,
                    title,
                    kind,
                    status,
                    input_json,
                    output_json,
                    created_at,
                    updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                ON CONFLICT(step_id) DO UPDATE SET
                    run_id = excluded.run_id,
                    workflow_id = excluded.workflow_id,
                    workflow_step_id = excluded.workflow_step_id,
                    attempt = excluded.attempt,
                    title = excluded.title,
                    kind = excluded.kind,
                    status = excluded.status,
                    input_json = excluded.input_json,
                    output_json = excluded.output_json,
                    created_at = excluded.created_at,
                    updated_at = excluded.updated_at"#,
                params![
                    step.step_id,
                    step.run_id,
                    null_string(&step.workflow_id),
                    null_string(&step.workflow_step_id),
                    step.attempt,
                    step.title,
                    step.kind,
                    step.status.as_str(),
                    input_json,
                    output_json,
                    now_rfc3339(&step.created_at),
                    now_rfc3339(&step.updated_at),
                ],
            )
            .map_err(|e| format!("upsert step {}: {e}", step.step_id))?;
        Ok(())
    }

    pub fn upsert_tool_call(&self, tool_call: &kura_runtime::ToolCall) -> Result<(), String> {
        let input_json = marshal_json(&tool_call.input)?;
        let output_json = marshal_json(&tool_call.output)?;
        let sandbox_json = marshal_map(&tool_call.sandbox)?;
        let integration_bindings_json = marshal_vec(&tool_call.integration_bindings)?;

        self.conn
            .execute(
                r#"INSERT INTO tool_calls (
                    tool_call_id,
                    run_id,
                    step_id,
                    workflow_id,
                    workflow_step_id,
                    attempt,
                    computer_use_session_id,
                    computer_use_action_id,
                    invocation_kind,
                    capability_id,
                    skill_id,
                    mcp_server_id,
                    mcp_server_name,
                    mcp_tool_name,
                    mcp_transport_kind,
                    mcp_session_id,
                    authorization_result,
                    tool_name,
                    status,
                    sandbox_execution_id,
                    failure_class,
                    input_json,
                    output_json,
                    sandbox_json,
                    integration_bindings_json,
                    error_text,
                    created_at,
                    updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28)
                ON CONFLICT(tool_call_id) DO UPDATE SET
                    run_id = excluded.run_id,
                    step_id = excluded.step_id,
                    workflow_id = excluded.workflow_id,
                    workflow_step_id = excluded.workflow_step_id,
                    attempt = excluded.attempt,
                    computer_use_session_id = excluded.computer_use_session_id,
                    computer_use_action_id = excluded.computer_use_action_id,
                    invocation_kind = excluded.invocation_kind,
                    capability_id = excluded.capability_id,
                    skill_id = excluded.skill_id,
                    mcp_server_id = excluded.mcp_server_id,
                    mcp_server_name = excluded.mcp_server_name,
                    mcp_tool_name = excluded.mcp_tool_name,
                    mcp_transport_kind = excluded.mcp_transport_kind,
                    mcp_session_id = excluded.mcp_session_id,
                    authorization_result = excluded.authorization_result,
                    tool_name = excluded.tool_name,
                    status = excluded.status,
                    sandbox_execution_id = excluded.sandbox_execution_id,
                    failure_class = excluded.failure_class,
                    input_json = excluded.input_json,
                    output_json = excluded.output_json,
                    sandbox_json = excluded.sandbox_json,
                    integration_bindings_json = excluded.integration_bindings_json,
                    error_text = excluded.error_text,
                    created_at = excluded.created_at,
                    updated_at = excluded.updated_at"#,
                params![
                    tool_call.tool_call_id,
                    tool_call.run_id,
                    tool_call.step_id,
                    null_string(&tool_call.workflow_id),
                    null_string(&tool_call.workflow_step_id),
                    tool_call.attempt,
                    null_string(&tool_call.computer_use_session_id),
                    null_string(&tool_call.computer_use_action_id),
                    null_string(&tool_call.invocation_kind),
                    tool_call.capability_id,
                    null_string(&tool_call.skill_id),
                    null_string(&tool_call.mcp_server_id),
                    null_string(&tool_call.mcp_server_name),
                    null_string(&tool_call.mcp_tool_name),
                    null_string(&tool_call.mcp_transport_kind),
                    null_string(&tool_call.mcp_session_id),
                    null_string(&tool_call.authorization_result),
                    tool_call.tool_name,
                    tool_call.status.as_str(),
                    null_string(&tool_call.sandbox_execution_id),
                    null_string(&tool_call.failure_class),
                    input_json,
                    output_json,
                    sandbox_json,
                    integration_bindings_json,
                    null_string(&tool_call.error),
                    now_rfc3339(&tool_call.created_at),
                    now_rfc3339(&tool_call.updated_at),
                ],
            )
            .map_err(|e| format!("upsert tool call {}: {e}", tool_call.tool_call_id))?;
        Ok(())
    }

    pub fn list_tool_calls(
        &self,
        run_id: &str,
        step_id: &str,
    ) -> Result<Vec<kura_runtime::ToolCall>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT tool_call_id, run_id, step_id, workflow_id, workflow_step_id, attempt, computer_use_session_id, computer_use_action_id, invocation_kind, capability_id, skill_id, mcp_server_id, mcp_server_name, mcp_tool_name, mcp_transport_kind, mcp_session_id, authorization_result, tool_name, status, sandbox_execution_id, failure_class, input_json, output_json, sandbox_json, integration_bindings_json, error_text, created_at, updated_at
                FROM tool_calls
                WHERE run_id = ?1 AND step_id = ?2
                ORDER BY created_at ASC, tool_call_id ASC"#,
            )
            .map_err(|e| format!("list tool calls for run {run_id} step {step_id}: {e}"))?;
        let mut rows = stmt
            .query(params![run_id, step_id])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_tool_call(row)?);
        }
        Ok(items)
    }

    pub fn list_steps(&self, run_id: &str) -> Result<Vec<kura_runtime::Step>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT step_id, run_id, workflow_id, workflow_step_id, attempt, title, kind, status, input_json, output_json, created_at, updated_at
                FROM steps
                WHERE run_id = ?1
                ORDER BY created_at ASC, step_id ASC"#,
            )
            .map_err(|e| format!("list steps for run {run_id}: {e}"))?;
        let mut rows = stmt.query(params![run_id]).map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_step(row)?);
        }
        Ok(items)
    }

    pub fn save_checkpoint(&self, checkpoint: &kura_runtime::RunCheckpoint) -> Result<(), String> {
        let snapshot_json =
            serde_json::to_string(checkpoint).map_err(|e| format!("marshal checkpoint: {e}"))?;

        let parent_tenant: Option<String> = self
            .conn
            .query_row(
                "SELECT tenant_id FROM runs WHERE run_id = ?1",
                params![checkpoint.run.run_id],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;

        self.conn
            .execute(
                r#"INSERT INTO checkpoints (
                    checkpoint_id,
                    run_id,
                    captured_at,
                    snapshot_json,
                    tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5)"#,
                params![
                    new_checkpoint_id(),
                    checkpoint.run.run_id,
                    now_rfc3339(&checkpoint.captured_at),
                    snapshot_json,
                    parent_tenant.filter(|s| !s.trim().is_empty()),
                ],
            )
            .map_err(|e| format!("save checkpoint for run {}: {e}", checkpoint.run.run_id))?;
        Ok(())
    }

    pub fn list_latest_checkpoints(&self) -> Result<Vec<kura_runtime::RunCheckpoint>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT checkpoint_id, run_id, captured_at, snapshot_json
                FROM checkpoints
                WHERE (run_id, captured_at) IN (
                    SELECT run_id, MAX(captured_at)
                    FROM checkpoints
                    GROUP BY run_id
                )
                ORDER BY captured_at ASC, run_id ASC"#,
            )
            .map_err(|e| format!("list latest checkpoints: {e}"))?;
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let _checkpoint_id: String = row.get(0).map_err(|e| e.to_string())?;
            let run_id: String = row.get(1).map_err(|e| e.to_string())?;
            let captured_at: String = row.get(2).map_err(|e| e.to_string())?;
            let snapshot_json: String = row.get(3).map_err(|e| e.to_string())?;

            let mut checkpoint: kura_runtime::RunCheckpoint = serde_json::from_str(&snapshot_json)
                .map_err(|e| format!("decode checkpoint snapshot: {e}"))?;
            if checkpoint.run.run_id != run_id {
                return Err(format!(
                    "checkpoint run mismatch: row={run_id} snapshot={}",
                    checkpoint.run.run_id
                ));
            }
            checkpoint.captured_at = parse_rfc3339(&captured_at)?;
            items.push(checkpoint);
        }
        Ok(items)
    }
}
