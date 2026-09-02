//! SQLite CRUD for the registry-style tables: sessions (router), capabilities (supervisor),
//! LLM dispatches, provider records, and policy approvals/decisions. Ported from
//! `daemon/internal/store/store.go` tenantless write paths.

use rusqlite::{params, Row};

use crate::crud::{
    enum_str, now_rfc3339, null_string, opt_time_string, parse_enum, parse_opt_rfc3339,
    parse_rfc3339,
};
use crate::SQLiteStore;

fn scan_session(row: &Row) -> Result<kura_router::Session, String> {
    let session_id: String = row.get(0).map_err(|e| e.to_string())?;
    let kind: String = row.get(1).map_err(|e| e.to_string())?;
    let status: String = row.get(2).map_err(|e| e.to_string())?;
    let channel: String = row.get(3).map_err(|e| e.to_string())?;
    let account_id: Option<String> = row.get(4).map_err(|e| e.to_string())?;
    let peer_id: String = row.get(5).map_err(|e| e.to_string())?;
    let thread_id: Option<String> = row.get(6).map_err(|e| e.to_string())?;
    let routing_key: String = row.get(7).map_err(|e| e.to_string())?;
    let generation: i64 = row.get(8).map_err(|e| e.to_string())?;
    let created_at: String = row.get(9).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(10).map_err(|e| e.to_string())?;
    let last_active_at: String = row.get(11).map_err(|e| e.to_string())?;
    let last_reset_at: Option<String> = row.get(12).map_err(|e| e.to_string())?;

    Ok(kura_router::Session {
        session_id,
        kind: parse_enum(&kind)?,
        status: parse_enum(&status)?,
        channel,
        account_id: account_id.unwrap_or_default(),
        peer_id,
        thread_id: thread_id.unwrap_or_default(),
        routing_key,
        generation,
        created_at: parse_rfc3339(&created_at)?,
        updated_at: parse_rfc3339(&updated_at)?,
        last_active_at: parse_rfc3339(&last_active_at)?,
        last_reset_at: parse_opt_rfc3339(last_reset_at)?,
        active_profile_projection: None,
    })
}

fn scan_capability(row: &Row) -> Result<kura_capabilities::Capability, String> {
    let capability_id: String = row.get(0).map_err(|e| e.to_string())?;
    let kind: String = row.get(1).map_err(|e| e.to_string())?;
    let display_name: String = row.get(2).map_err(|e| e.to_string())?;
    let status: String = row.get(3).map_err(|e| e.to_string())?;
    let failure_count: i64 = row.get(4).map_err(|e| e.to_string())?;
    let restart_count: i64 = row.get(5).map_err(|e| e.to_string())?;
    let backoff_seconds: i64 = row.get(6).map_err(|e| e.to_string())?;
    let next_restart_at: Option<String> = row.get(7).map_err(|e| e.to_string())?;
    let last_restart_at: Option<String> = row.get(8).map_err(|e| e.to_string())?;
    let last_heartbeat_at: Option<String> = row.get(9).map_err(|e| e.to_string())?;
    let last_failure_reason: Option<String> = row.get(10).map_err(|e| e.to_string())?;
    let created_at: String = row.get(11).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(12).map_err(|e| e.to_string())?;

    Ok(kura_capabilities::Capability {
        capability_id,
        kind,
        display_name,
        status: parse_enum(&status)?,
        failure_count,
        restart_count,
        backoff_seconds,
        next_restart_at: parse_opt_rfc3339(next_restart_at)?,
        last_restart_at: parse_opt_rfc3339(last_restart_at)?,
        last_heartbeat_at: parse_opt_rfc3339(last_heartbeat_at)?,
        last_failure_reason: last_failure_reason.unwrap_or_default(),
        created_at: parse_rfc3339(&created_at)?,
        updated_at: parse_rfc3339(&updated_at)?,
    })
}

fn scan_llm_dispatch(row: &Row) -> Result<kura_llm::Dispatch, String> {
    let dispatch_id: String = row.get(0).map_err(|e| e.to_string())?;
    let provider: String = row.get(1).map_err(|e| e.to_string())?;
    let model: String = row.get(2).map_err(|e| e.to_string())?;
    let messages_raw: String = row.get(3).map_err(|e| e.to_string())?;
    let stream: bool = row.get(4).map_err(|e| e.to_string())?;
    let status: String = row.get(5).map_err(|e| e.to_string())?;
    let output: String = row.get(6).map_err(|e| e.to_string())?;
    let finish_reason: Option<String> = row.get(7).map_err(|e| e.to_string())?;
    let usage_raw: String = row.get(8).map_err(|e| e.to_string())?;
    let error_code: Option<String> = row.get(9).map_err(|e| e.to_string())?;
    let error_text: Option<String> = row.get(10).map_err(|e| e.to_string())?;
    let timeout_ms: i64 = row.get(11).map_err(|e| e.to_string())?;
    let max_retries: i64 = row.get(12).map_err(|e| e.to_string())?;
    let attempt_count: i64 = row.get(13).map_err(|e| e.to_string())?;
    let created_at: String = row.get(14).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(15).map_err(|e| e.to_string())?;
    let started_at: Option<String> = row.get(16).map_err(|e| e.to_string())?;
    let completed_at: Option<String> = row.get(17).map_err(|e| e.to_string())?;
    let tools_raw: Option<String> = row.get(18).map_err(|e| e.to_string())?;
    let tool_calls_raw: Option<String> = row.get(19).map_err(|e| e.to_string())?;

    let status: kura_llm::DispatchStatus = parse_enum(&status)?;
    let messages: Vec<kura_llm::Message> =
        crate::crud::decode_json_field(&messages_raw).map_err(|e| format!("decode llm dispatch messages: {e}"))?;
    // Null for every dispatch written before these columns existed, and for
    // any plain chat request since.
    let tools: Vec<kura_llm::ToolSpec> = match tools_raw {
        Some(raw) if !raw.is_empty() => crate::crud::decode_json_field(&raw)
            .map_err(|e| format!("decode llm dispatch tools: {e}"))?,
        _ => Vec::new(),
    };
    let tool_calls: Vec<kura_llm::ToolCall> = match tool_calls_raw {
        Some(raw) if !raw.is_empty() => crate::crud::decode_json_field(&raw)
            .map_err(|e| format!("decode llm dispatch tool calls: {e}"))?,
        _ => Vec::new(),
    };
    let usage: kura_llm::Usage =
        crate::crud::decode_json_field(&usage_raw).map_err(|e| format!("decode llm dispatch usage: {e}"))?;
    let partial = status == kura_llm::DispatchStatus::PartialFailed;

    Ok(kura_llm::Dispatch {
        dispatch_id,
        provider,
        model,
        messages,
        tools,
        tool_calls,
        stream,
        status,
        output,
        finish_reason: finish_reason.unwrap_or_default(),
        usage,
        error_code: error_code.unwrap_or_default(),
        error: error_text.unwrap_or_default(),
        timeout_ms,
        partial,
        max_retries,
        attempt_count,
        created_at: parse_rfc3339(&created_at)?,
        updated_at: parse_rfc3339(&updated_at)?,
        started_at: parse_opt_rfc3339(started_at)?,
        completed_at: parse_opt_rfc3339(completed_at)?,
    })
}

impl SQLiteStore {
    pub fn upsert_session(&self, session: &kura_router::Session) -> Result<(), String> {
        self.conn
            .execute(
                r#"INSERT INTO sessions (
                    session_id, kind, status, channel, account_id, peer_id, thread_id, routing_key,
                    generation, created_at, updated_at, last_active_at, last_reset_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                ON CONFLICT(session_id) DO UPDATE SET
                    kind = excluded.kind,
                    status = excluded.status,
                    channel = excluded.channel,
                    account_id = excluded.account_id,
                    peer_id = excluded.peer_id,
                    thread_id = excluded.thread_id,
                    routing_key = excluded.routing_key,
                    generation = excluded.generation,
                    created_at = excluded.created_at,
                    updated_at = excluded.updated_at,
                    last_active_at = excluded.last_active_at,
                    last_reset_at = excluded.last_reset_at"#,
                params![
                    session.session_id,
                    enum_str(&session.kind),
                    enum_str(&session.status),
                    session.channel,
                    null_string(&session.account_id),
                    session.peer_id,
                    null_string(&session.thread_id),
                    session.routing_key,
                    session.generation,
                    now_rfc3339(&session.created_at),
                    now_rfc3339(&session.updated_at),
                    now_rfc3339(&session.last_active_at),
                    opt_time_string(&session.last_reset_at),
                ],
            )
            .map_err(|e| format!("upsert session {}: {e}", session.session_id))?;
        Ok(())
    }

    pub fn list_sessions(&self) -> Result<Vec<kura_router::Session>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT session_id, kind, status, channel, account_id, peer_id, thread_id, routing_key,
                    generation, created_at, updated_at, last_active_at, last_reset_at
                FROM sessions
                ORDER BY created_at ASC, session_id ASC"#,
            )
            .map_err(|e| format!("list sessions: {e}"))?;
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_session(row)?);
        }
        Ok(items)
    }

    pub fn upsert_capability(&self, capability: &kura_capabilities::Capability) -> Result<(), String> {
        self.conn
            .execute(
                r#"INSERT INTO capabilities (
                    capability_id, kind, display_name, status, failure_count, restart_count,
                    backoff_seconds, next_restart_at, last_restart_at, last_heartbeat_at,
                    last_failure_reason, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                ON CONFLICT(capability_id) DO UPDATE SET
                    kind = excluded.kind,
                    display_name = excluded.display_name,
                    status = excluded.status,
                    failure_count = excluded.failure_count,
                    restart_count = excluded.restart_count,
                    backoff_seconds = excluded.backoff_seconds,
                    next_restart_at = excluded.next_restart_at,
                    last_restart_at = excluded.last_restart_at,
                    last_heartbeat_at = excluded.last_heartbeat_at,
                    last_failure_reason = excluded.last_failure_reason,
                    created_at = excluded.created_at,
                    updated_at = excluded.updated_at"#,
                params![
                    capability.capability_id,
                    capability.kind,
                    capability.display_name,
                    enum_str(&capability.status),
                    capability.failure_count,
                    capability.restart_count,
                    capability.backoff_seconds,
                    opt_time_string(&capability.next_restart_at),
                    opt_time_string(&capability.last_restart_at),
                    opt_time_string(&capability.last_heartbeat_at),
                    null_string(&capability.last_failure_reason),
                    now_rfc3339(&capability.created_at),
                    now_rfc3339(&capability.updated_at),
                ],
            )
            .map_err(|e| format!("upsert capability {}: {e}", capability.capability_id))?;
        Ok(())
    }

    pub fn list_capabilities(&self) -> Result<Vec<kura_capabilities::Capability>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT capability_id, kind, display_name, status, failure_count, restart_count,
                    backoff_seconds, next_restart_at, last_restart_at, last_heartbeat_at,
                    last_failure_reason, created_at, updated_at
                FROM capabilities
                ORDER BY created_at ASC, capability_id ASC"#,
            )
            .map_err(|e| format!("list capabilities: {e}"))?;
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_capability(row)?);
        }
        Ok(items)
    }

    pub fn upsert_llm_dispatch(&self, dispatch: &kura_llm::Dispatch) -> Result<(), String> {
        let messages_json =
            serde_json::to_string(&dispatch.messages).map_err(|e| format!("marshal llm dispatch messages: {e}"))?;
        let usage_json =
            serde_json::to_string(&dispatch.usage).map_err(|e| format!("marshal llm dispatch usage: {e}"))?;

        self.conn
            .execute(
                r#"INSERT INTO llm_dispatches (
                    dispatch_id, provider, model, messages_json, stream, status, output_text,
                    finish_reason, usage_json, error_code, error_text, timeout_ms, max_retries,
                    attempt_count, created_at, updated_at, started_at, completed_at,
                    tools_json, tool_calls_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)
                ON CONFLICT(dispatch_id) DO UPDATE SET
                    provider = excluded.provider,
                    model = excluded.model,
                    messages_json = excluded.messages_json,
                    stream = excluded.stream,
                    status = excluded.status,
                    output_text = excluded.output_text,
                    finish_reason = excluded.finish_reason,
                    usage_json = excluded.usage_json,
                    error_code = excluded.error_code,
                    error_text = excluded.error_text,
                    timeout_ms = excluded.timeout_ms,
                    max_retries = excluded.max_retries,
                    attempt_count = excluded.attempt_count,
                    created_at = excluded.created_at,
                    updated_at = excluded.updated_at,
                    started_at = excluded.started_at,
                    completed_at = excluded.completed_at,
                    tools_json = excluded.tools_json,
                    tool_calls_json = excluded.tool_calls_json"#,
                params![
                    dispatch.dispatch_id,
                    dispatch.provider,
                    dispatch.model,
                    messages_json,
                    dispatch.stream,
                    enum_str(&dispatch.status),
                    dispatch.output,
                    null_string(&dispatch.finish_reason),
                    usage_json,
                    null_string(&dispatch.error_code),
                    null_string(&dispatch.error),
                    dispatch.timeout_ms,
                    dispatch.max_retries,
                    dispatch.attempt_count,
                    now_rfc3339(&dispatch.created_at),
                    now_rfc3339(&dispatch.updated_at),
                    opt_time_string(&dispatch.started_at),
                    opt_time_string(&dispatch.completed_at),
                    null_json(&dispatch.tools, "tools")?,
                    null_json(&dispatch.tool_calls, "tool calls")?,
                ],
            )
            .map_err(|e| format!("upsert llm dispatch {}: {e}", dispatch.dispatch_id))?;
        Ok(())
    }

    pub fn list_llm_dispatches(&self) -> Result<Vec<kura_llm::Dispatch>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT dispatch_id, provider, model, messages_json, stream, status, output_text,
                    finish_reason, usage_json, error_code, error_text, timeout_ms, max_retries,
                    attempt_count, created_at, updated_at, started_at, completed_at, tools_json, tool_calls_json
                FROM llm_dispatches
                ORDER BY created_at ASC, dispatch_id ASC"#,
            )
            .map_err(|e| format!("list llm dispatches: {e}"))?;
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_llm_dispatch(row)?);
        }
        Ok(items)
    }

    pub fn get_llm_dispatch(&self, dispatch_id: &str) -> Result<Option<kura_llm::Dispatch>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT dispatch_id, provider, model, messages_json, stream, status, output_text,
                    finish_reason, usage_json, error_code, error_text, timeout_ms, max_retries,
                    attempt_count, created_at, updated_at, started_at, completed_at, tools_json, tool_calls_json
                FROM llm_dispatches
                WHERE dispatch_id = ?1"#,
            )
            .map_err(|e| e.to_string())?;
        let mut rows = stmt.query(params![dispatch_id]).map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        scan_llm_dispatch(row).map(Some)
    }
}

/// A JSON column that stays null when there is nothing to record.
///
/// Every dispatch predating these columns has neither, and a plain chat
/// request still has neither -- writing `[]` for all of them would make an
/// empty list indistinguishable from a row written before the column existed.
fn null_json<T: serde::Serialize>(values: &[T], label: &str) -> Result<Option<String>, String> {
    if values.is_empty() {
        return Ok(None);
    }
    serde_json::to_string(values)
        .map(Some)
        .map_err(|e| format!("marshal llm dispatch {label}: {e}"))
}
