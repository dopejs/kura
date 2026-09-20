//! SQLite CRUD for the connector supervisor domain (connectors, connector messages, delivery
//! boundaries, conformance results, diagnostic states). Ported from
//! `daemon/internal/store/store.go` (UpsertConnector, ListConnectors,
//! CreateConnectorMessageIfAbsent, UpsertConnectorMessage, SaveConnectorDeliveryBoundary,
//! GetConnectorMessageByExternalID{,ForTenant}, GetConnectorMessageByStandardIdentity,
//! SaveConnectorConformanceResult, ListConnectorConformanceResults,
//! SaveConnectorDiagnosticState, ListConnectorDiagnosticStates). The tenant-binding
//! resolution (ResolveActiveTenantBinding) is not ported: tenant_id is written as-is
//! (empty string when unbound) so the tenant-scoped unique indexes and lookups behave
//! exactly like the Go unbound-context paths.

use chrono::{DateTime, Utc};
use rusqlite::{Row, params};

use crate::SQLiteStore;
use crate::crud::{
    enum_str, now_rfc3339, null_string, opt_time_string, parse_enum, parse_opt_rfc3339,
    parse_rfc3339,
};

/// A connector delivery-boundary ledger row. `document` is the JSON-serialized boundary
/// snapshot (Go `Document []byte`, tagged `json:"-"`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConnectorDeliveryBoundaryRecord {
    pub boundary_id: String,
    pub tenant_id: String,
    pub connector_id: String,
    pub foreground_reply_outcome_id: String,
    pub background_delivery_id: String,
    pub transport_kind: String,
    pub separation_status: String,
    pub created_at: DateTime<Utc>,
    pub document: String,
}

fn new_store_id(prefix: &str) -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!("{prefix}_{}", &hex[..16])
}

/// A chrono-defaulted timestamp (UNIX epoch) stands in for Go's zero `time.Time`.
fn is_unset_time(dt: &DateTime<Utc>) -> bool {
    dt.timestamp() == 0 && dt.timestamp_subsec_nanos() == 0
}

/// Go `isUniqueConstraintError`: SQLITE_CONSTRAINT and friends.
fn is_unique_constraint_error(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(e, _) if e.code == rusqlite::ErrorCode::ConstraintViolation
    )
}

/// Go `connectorMessageEquivalentRuleID`: default the standard provider-message rule id
/// whenever a provider message id is present.
fn connector_message_equivalent_rule_id(message: &kura_imtypes::MessageRecord) -> String {
    if message.provider_message_id.trim().is_empty() {
        String::new()
    } else if message.equivalent_rule_id.trim().is_empty() {
        "standard_provider_message_id".to_string()
    } else {
        message.equivalent_rule_id.clone()
    }
}

fn scan_connector(row: &Row) -> Result<kura_connectors::Connector, String> {
    let connector_id: String = row.get(0).map_err(|e| e.to_string())?;
    let tenant_id: Option<String> = row.get(1).map_err(|e| e.to_string())?;
    let kind: String = row.get(2).map_err(|e| e.to_string())?;
    let display_name: String = row.get(3).map_err(|e| e.to_string())?;
    let status: String = row.get(4).map_err(|e| e.to_string())?;
    let disabled_reason: Option<String> = row.get(5).map_err(|e| e.to_string())?;
    let secret_refs_raw: Option<String> = row.get(6).map_err(|e| e.to_string())?;
    let failure_count: i64 = row.get(7).map_err(|e| e.to_string())?;
    let restart_count: i64 = row.get(8).map_err(|e| e.to_string())?;
    let backoff_seconds: i64 = row.get(9).map_err(|e| e.to_string())?;
    let next_restart_at: Option<String> = row.get(10).map_err(|e| e.to_string())?;
    let last_restart_at: Option<String> = row.get(11).map_err(|e| e.to_string())?;
    let last_heartbeat_at: Option<String> = row.get(12).map_err(|e| e.to_string())?;
    let last_failure_reason: Option<String> = row.get(13).map_err(|e| e.to_string())?;
    let created_at: String = row.get(14).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(15).map_err(|e| e.to_string())?;

    let secret_refs = match secret_refs_raw {
        Some(raw) if !raw.is_empty() => {
            serde_json::from_str(&raw).map_err(|e| format!("parse connector secret refs: {e}"))?
        }
        _ => Vec::new(),
    };

    Ok(kura_connectors::Connector {
        connector_id,
        tenant_id: tenant_id.unwrap_or_default(),
        kind,
        display_name,
        status: parse_enum(&status)?,
        disabled_reason: disabled_reason.unwrap_or_default(),
        secret_refs,
        failure_count,
        restart_count,
        backoff_seconds,
        next_restart_at: parse_opt_rfc3339(next_restart_at)?,
        last_restart_at: parse_opt_rfc3339(last_restart_at)?,
        last_heartbeat_at: parse_opt_rfc3339(last_heartbeat_at)?,
        last_failure_reason: last_failure_reason.unwrap_or_default(),
        created_at: parse_rfc3339(&created_at)?,
        updated_at: parse_rfc3339(&updated_at)?,
        ..kura_connectors::Connector::default()
    })
}

fn scan_connector_message(row: &Row) -> Result<kura_imtypes::MessageRecord, String> {
    let delivery_id: String = row.get(0).map_err(|e| e.to_string())?;
    let tenant_id: Option<String> = row.get(1).map_err(|e| e.to_string())?;
    let connector_id: String = row.get(2).map_err(|e| e.to_string())?;
    let direction: String = row.get(3).map_err(|e| e.to_string())?;
    let external_message_id: Option<String> = row.get(4).map_err(|e| e.to_string())?;
    let connector_account_id: Option<String> = row.get(5).map_err(|e| e.to_string())?;
    let channel_or_conversation_id: Option<String> = row.get(6).map_err(|e| e.to_string())?;
    let provider_message_id: Option<String> = row.get(7).map_err(|e| e.to_string())?;
    let equivalent_rule_id: Option<String> = row.get(8).map_err(|e| e.to_string())?;
    let session_id: Option<String> = row.get(9).map_err(|e| e.to_string())?;
    let thread_session_segment_id: Option<String> = row.get(10).map_err(|e| e.to_string())?;
    let run_id: Option<String> = row.get(11).map_err(|e| e.to_string())?;
    let channel_id: String = row.get(12).map_err(|e| e.to_string())?;
    let peer_id: Option<String> = row.get(13).map_err(|e| e.to_string())?;
    let thread_id: Option<String> = row.get(14).map_err(|e| e.to_string())?;
    let author_id: Option<String> = row.get(15).map_err(|e| e.to_string())?;
    let content: String = row.get(16).map_err(|e| e.to_string())?;
    let status: String = row.get(17).map_err(|e| e.to_string())?;
    let error_text: Option<String> = row.get(18).map_err(|e| e.to_string())?;
    let reply_to_external_message_id: Option<String> = row.get(19).map_err(|e| e.to_string())?;
    let response_to_delivery_id: Option<String> = row.get(20).map_err(|e| e.to_string())?;
    let foreground_outcome_status: Option<String> = row.get(21).map_err(|e| e.to_string())?;
    let background_delivery_id: Option<String> = row.get(22).map_err(|e| e.to_string())?;
    let delivery_boundary_kind: Option<String> = row.get(23).map_err(|e| e.to_string())?;
    let created_at: String = row.get(24).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(25).map_err(|e| e.to_string())?;

    Ok(kura_imtypes::MessageRecord {
        delivery_id,
        tenant_id: tenant_id.unwrap_or_default(),
        connector_id,
        direction: parse_enum(&direction)?,
        external_message_id: external_message_id.unwrap_or_default(),
        connector_account_id: connector_account_id.unwrap_or_default(),
        channel_or_conversation_id: channel_or_conversation_id.unwrap_or_default(),
        provider_message_id: provider_message_id.unwrap_or_default(),
        equivalent_rule_id: equivalent_rule_id.unwrap_or_default(),
        session_id: session_id.unwrap_or_default(),
        thread_session_segment_id: thread_session_segment_id.unwrap_or_default(),
        run_id: run_id.unwrap_or_default(),
        channel_id,
        peer_id: peer_id.unwrap_or_default(),
        thread_id: thread_id.unwrap_or_default(),
        author_id: author_id.unwrap_or_default(),
        content,
        status: parse_enum(&status)?,
        error: error_text.unwrap_or_default(),
        reply_to_external_message_id: reply_to_external_message_id.unwrap_or_default(),
        response_to_delivery_id: response_to_delivery_id.unwrap_or_default(),
        foreground_outcome_status: foreground_outcome_status.unwrap_or_default(),
        background_delivery_id: background_delivery_id.unwrap_or_default(),
        delivery_boundary_kind: delivery_boundary_kind.unwrap_or_default(),
        created_at: parse_rfc3339(&created_at)?,
        updated_at: parse_rfc3339(&updated_at)?,
    })
}

fn scan_connector_conformance_result(
    row: &Row,
) -> Result<kura_connectors::ConformanceResult, String> {
    let conformance_result_id: String = row.get(0).map_err(|e| e.to_string())?;
    let tenant_id: String = row.get(1).map_err(|e| e.to_string())?;
    let connector_kind: String = row.get(2).map_err(|e| e.to_string())?;
    let connector_id: Option<String> = row.get(3).map_err(|e| e.to_string())?;
    let scenario_id: String = row.get(4).map_err(|e| e.to_string())?;
    let area: String = row.get(5).map_err(|e| e.to_string())?;
    let result: String = row.get(6).map_err(|e| e.to_string())?;
    let reason_code: Option<String> = row.get(7).map_err(|e| e.to_string())?;
    let redaction_status: String = row.get(8).map_err(|e| e.to_string())?;
    let evidence_timestamp: String = row.get(9).map_err(|e| e.to_string())?;
    let retention_expires_at: String = row.get(10).map_err(|e| e.to_string())?;

    Ok(kura_connectors::ConformanceResult {
        conformance_result_id,
        tenant_id,
        connector_kind,
        connector_id: connector_id.unwrap_or_default(),
        scenario_id,
        area,
        result: parse_enum(&result)?,
        reason_code: reason_code.unwrap_or_default(),
        redaction_status: parse_enum(&redaction_status)?,
        evidence_timestamp: parse_rfc3339(&evidence_timestamp)?,
        retention_expires_at: parse_rfc3339(&retention_expires_at)?,
    })
}

fn scan_connector_diagnostic_state(
    row: &Row,
    now: DateTime<Utc>,
) -> Result<kura_connectors::ConnectorDiagnosticState, String> {
    let diagnostic_state_id: String = row.get(0).map_err(|e| e.to_string())?;
    let tenant_id: String = row.get(1).map_err(|e| e.to_string())?;
    let connector_id: String = row.get(2).map_err(|e| e.to_string())?;
    let connector_account_id: Option<String> = row.get(3).map_err(|e| e.to_string())?;
    let status: String = row.get(4).map_err(|e| e.to_string())?;
    let reason_code: String = row.get(5).map_err(|e| e.to_string())?;
    let remediation_owner: String = row.get(6).map_err(|e| e.to_string())?;
    let user_visible_severity: String = row.get(7).map_err(|e| e.to_string())?;
    let retry_safety: String = row.get(8).map_err(|e| e.to_string())?;
    let evidence_timestamp: String = row.get(9).map_err(|e| e.to_string())?;
    let redaction_status: String = row.get(11).map_err(|e| e.to_string())?;
    let retention_expires_at: String = row.get(12).map_err(|e| e.to_string())?;
    let redaction_failure_id: Option<String> = row.get(13).map_err(|e| e.to_string())?;

    let evidence = parse_rfc3339(&evidence_timestamp)?;
    // Go recomputes freshness from the evidence vs. now on read (FreshnessAt).
    let freshness_state = if now.signed_duration_since(evidence) > chrono::Duration::minutes(15) {
        kura_connectors::FreshnessState::Stale
    } else {
        kura_connectors::FreshnessState::Fresh
    };

    Ok(kura_connectors::ConnectorDiagnosticState {
        diagnostic_state_id,
        tenant_id,
        connector_id,
        connector_account_id: connector_account_id.unwrap_or_default(),
        status: parse_enum(&status)?,
        reason_code: parse_enum(&reason_code)?,
        remediation_owner: parse_enum(&remediation_owner)?,
        user_visible_severity,
        retry_safety: parse_enum(&retry_safety)?,
        evidence_timestamp: evidence,
        freshness_state,
        redaction_status: parse_enum(&redaction_status)?,
        retention_expires_at: parse_rfc3339(&retention_expires_at)?,
        redaction_failure_id: redaction_failure_id.unwrap_or_default(),
        ..kura_connectors::ConnectorDiagnosticState::default()
    })
}

impl SQLiteStore {
    pub fn upsert_connector(&self, connector: &kura_connectors::Connector) -> Result<(), String> {
        let secret_refs_json = serde_json::to_string(&connector.secret_refs).map_err(|e| {
            format!(
                "marshal connector secret refs {}: {e}",
                connector.connector_id
            )
        })?;

        self.conn
            .execute(
                r#"INSERT INTO connectors (
                    connector_id, tenant_id, kind, display_name, status, disabled_reason,
                    secret_refs_json, failure_count, restart_count, backoff_seconds,
                    next_restart_at, last_restart_at, last_heartbeat_at, last_failure_reason,
                    created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
                ON CONFLICT(connector_id) DO UPDATE SET
                    tenant_id = COALESCE(connectors.tenant_id, excluded.tenant_id),
                    kind = excluded.kind,
                    display_name = excluded.display_name,
                    status = excluded.status,
                    disabled_reason = excluded.disabled_reason,
                    secret_refs_json = excluded.secret_refs_json,
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
                    connector.connector_id,
                    null_string(&connector.tenant_id),
                    connector.kind,
                    connector.display_name,
                    enum_str(&connector.status),
                    null_string(&connector.disabled_reason),
                    secret_refs_json,
                    connector.failure_count,
                    connector.restart_count,
                    connector.backoff_seconds,
                    opt_time_string(&connector.next_restart_at),
                    opt_time_string(&connector.last_restart_at),
                    opt_time_string(&connector.last_heartbeat_at),
                    null_string(&connector.last_failure_reason),
                    now_rfc3339(&connector.created_at),
                    now_rfc3339(&connector.updated_at),
                ],
            )
            .map_err(|e| format!("upsert connector {}: {e}", connector.connector_id))?;
        Ok(())
    }

    pub fn list_connectors(&self) -> Result<Vec<kura_connectors::Connector>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT connector_id, tenant_id, kind, display_name, status, disabled_reason,
                    secret_refs_json, failure_count, restart_count, backoff_seconds,
                    next_restart_at, last_restart_at, last_heartbeat_at, last_failure_reason,
                    created_at, updated_at
                FROM connectors
                ORDER BY created_at ASC, connector_id ASC"#,
            )
            .map_err(|e| format!("list connectors: {e}"))?;
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_connector(row)?);
        }
        Ok(items)
    }

    /// Go `CreateConnectorMessageIfAbsent`: insert unless a standard-identity or
    /// external-id row already exists; returns the resulting row and whether this call
    /// created it (the bool is the created flag, mirroring the Go signature).
    pub fn create_connector_message_if_absent(
        &self,
        message: &kura_imtypes::MessageRecord,
    ) -> Result<(kura_imtypes::MessageRecord, bool), String> {
        let equivalent_rule_id = connector_message_equivalent_rule_id(message);

        let insert = self.conn.execute(
            r#"INSERT INTO connector_messages (
                delivery_id, tenant_id, connector_id, direction, external_message_id,
                connector_account_id, channel_or_conversation_id, provider_message_id,
                equivalent_rule_id, session_id, thread_session_segment_id, run_id, channel_id,
                peer_id, thread_id, author_id, content, status, error_text,
                reply_to_external_message_id, response_to_delivery_id, foreground_outcome_status,
                background_delivery_id, delivery_boundary_kind, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26)"#,
            params![
                message.delivery_id,
                message.tenant_id,
                message.connector_id,
                enum_str(&message.direction),
                null_string(&message.external_message_id),
                null_string(&message.connector_account_id),
                null_string(&message.channel_or_conversation_id),
                null_string(&message.provider_message_id),
                null_string(&equivalent_rule_id),
                null_string(&message.session_id),
                null_string(&message.thread_session_segment_id),
                null_string(&message.run_id),
                message.channel_id,
                null_string(&message.peer_id),
                null_string(&message.thread_id),
                null_string(&message.author_id),
                message.content,
                enum_str(&message.status),
                null_string(&message.error),
                null_string(&message.reply_to_external_message_id),
                null_string(&message.response_to_delivery_id),
                null_string(&message.foreground_outcome_status),
                null_string(&message.background_delivery_id),
                null_string(&message.delivery_boundary_kind),
                now_rfc3339(&message.created_at),
                now_rfc3339(&message.updated_at),
            ],
        );

        if let Err(err) = insert {
            if !message.provider_message_id.trim().is_empty() && is_unique_constraint_error(&err) {
                if let Some(existing) = self.get_connector_message_by_standard_identity(
                    &message.tenant_id,
                    &message.connector_account_id,
                    &message.channel_or_conversation_id,
                    &message.provider_message_id,
                    message.direction,
                    &equivalent_rule_id,
                )? {
                    return Ok((existing, false));
                }
            }
            if !message.external_message_id.trim().is_empty() && is_unique_constraint_error(&err) {
                if let Some(existing) = self.get_connector_message_by_external_id_for_tenant(
                    &message.tenant_id,
                    &message.connector_id,
                    message.direction,
                    &message.external_message_id,
                )? {
                    return Ok((existing, false));
                }
            }
            return Err(format!(
                "insert connector message {}: {err}",
                message.delivery_id
            ));
        }

        if !message.provider_message_id.trim().is_empty() {
            if let Some(existing) = self.get_connector_message_by_standard_identity(
                &message.tenant_id,
                &message.connector_account_id,
                &message.channel_or_conversation_id,
                &message.provider_message_id,
                message.direction,
                &equivalent_rule_id,
            )? {
                let created = existing.delivery_id == message.delivery_id;
                return Ok((existing, created));
            }
        }
        if let Some(existing) = self.get_connector_message_by_external_id_for_tenant(
            &message.tenant_id,
            &message.connector_id,
            message.direction,
            &message.external_message_id,
        )? {
            let created = existing.delivery_id == message.delivery_id;
            return Ok((existing, created));
        }

        Err(format!(
            "load connector message {} after insert",
            message.delivery_id
        ))
    }

    pub fn upsert_connector_message(
        &self,
        message: &kura_imtypes::MessageRecord,
    ) -> Result<(), String> {
        let equivalent_rule_id = connector_message_equivalent_rule_id(message);

        self.conn
            .execute(
                r#"INSERT INTO connector_messages (
                    delivery_id, connector_id, direction, external_message_id,
                    connector_account_id, channel_or_conversation_id, provider_message_id,
                    equivalent_rule_id, session_id, thread_session_segment_id, run_id, channel_id,
                    peer_id, thread_id, author_id, content, status, error_text,
                    reply_to_external_message_id, response_to_delivery_id, foreground_outcome_status,
                    background_delivery_id, delivery_boundary_kind, created_at, updated_at, tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26)
                ON CONFLICT(delivery_id) DO UPDATE SET
                    connector_id = excluded.connector_id,
                    direction = excluded.direction,
                    external_message_id = excluded.external_message_id,
                    connector_account_id = excluded.connector_account_id,
                    channel_or_conversation_id = excluded.channel_or_conversation_id,
                    provider_message_id = excluded.provider_message_id,
                    equivalent_rule_id = excluded.equivalent_rule_id,
                    session_id = excluded.session_id,
                    thread_session_segment_id = excluded.thread_session_segment_id,
                    run_id = excluded.run_id,
                    channel_id = excluded.channel_id,
                    peer_id = excluded.peer_id,
                    thread_id = excluded.thread_id,
                    author_id = excluded.author_id,
                    content = excluded.content,
                    status = excluded.status,
                    error_text = excluded.error_text,
                    reply_to_external_message_id = excluded.reply_to_external_message_id,
                    response_to_delivery_id = excluded.response_to_delivery_id,
                    foreground_outcome_status = excluded.foreground_outcome_status,
                    background_delivery_id = excluded.background_delivery_id,
                    delivery_boundary_kind = excluded.delivery_boundary_kind,
                    created_at = excluded.created_at,
                    updated_at = excluded.updated_at,
                    tenant_id = COALESCE(connector_messages.tenant_id, excluded.tenant_id)"#,
                params![
                    message.delivery_id,
                    message.connector_id,
                    enum_str(&message.direction),
                    null_string(&message.external_message_id),
                    null_string(&message.connector_account_id),
                    null_string(&message.channel_or_conversation_id),
                    null_string(&message.provider_message_id),
                    null_string(&equivalent_rule_id),
                    null_string(&message.session_id),
                    null_string(&message.thread_session_segment_id),
                    null_string(&message.run_id),
                    message.channel_id,
                    null_string(&message.peer_id),
                    null_string(&message.thread_id),
                    null_string(&message.author_id),
                    message.content,
                    enum_str(&message.status),
                    null_string(&message.error),
                    null_string(&message.reply_to_external_message_id),
                    null_string(&message.response_to_delivery_id),
                    null_string(&message.foreground_outcome_status),
                    null_string(&message.background_delivery_id),
                    null_string(&message.delivery_boundary_kind),
                    now_rfc3339(&message.created_at),
                    now_rfc3339(&message.updated_at),
                    message.tenant_id,
                ],
            )
            .map_err(|e| format!("upsert connector message {}: {e}", message.delivery_id))?;
        Ok(())
    }

    pub fn save_connector_delivery_boundary(
        &self,
        record: &ConnectorDeliveryBoundaryRecord,
    ) -> Result<(), String> {
        let mut record = record.clone();
        if record.boundary_id.trim().is_empty() {
            record.boundary_id = new_store_id("connector_delivery_boundary");
        }
        if is_unset_time(&record.created_at) {
            record.created_at = Utc::now();
        }
        if record.separation_status.trim().is_empty() {
            record.separation_status = "separate_truths".to_string();
        }
        // Go marshals the record when no document was supplied (Document is json:"-").
        let document = if record.document.is_empty() {
            serde_json::to_string(&serde_json::json!({
                "boundaryId": record.boundary_id,
                "tenantId": record.tenant_id,
                "connectorId": record.connector_id,
                "foregroundReplyOutcomeId": record.foreground_reply_outcome_id,
                "backgroundDeliveryId": record.background_delivery_id,
                "transportKind": record.transport_kind,
                "separationStatus": record.separation_status,
                "createdAt": now_rfc3339(&record.created_at),
            }))
            .map_err(|e| format!("marshal connector delivery boundary: {e}"))?
        } else {
            record.document.clone()
        };

        self.conn
            .execute(
                r#"INSERT INTO connector_delivery_boundaries (
                    boundary_id, tenant_id, connector_id, foreground_reply_outcome_id,
                    background_delivery_id, transport_kind, separation_status, created_at,
                    document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                ON CONFLICT(boundary_id) DO UPDATE SET
                    tenant_id = COALESCE(connector_delivery_boundaries.tenant_id, excluded.tenant_id),
                    connector_id = excluded.connector_id,
                    foreground_reply_outcome_id = excluded.foreground_reply_outcome_id,
                    background_delivery_id = excluded.background_delivery_id,
                    transport_kind = excluded.transport_kind,
                    separation_status = excluded.separation_status,
                    created_at = excluded.created_at,
                    document_json = excluded.document_json"#,
                params![
                    record.boundary_id,
                    record.tenant_id,
                    record.connector_id,
                    null_string(&record.foreground_reply_outcome_id),
                    null_string(&record.background_delivery_id),
                    record.transport_kind,
                    record.separation_status,
                    now_rfc3339(&record.created_at),
                    document,
                ],
            )
            .map_err(|e| format!("save connector delivery boundary {}: {e}", record.boundary_id))?;
        Ok(())
    }

    /// Go `GetConnectorMessageByExternalID` resolves the tenant from the context; the
    /// unbound-context path passes an empty tenant.
    pub fn get_connector_message_by_external_id(
        &self,
        connector_id: &str,
        direction: kura_imtypes::DeliveryDirection,
        external_message_id: &str,
    ) -> Result<Option<kura_imtypes::MessageRecord>, String> {
        self.get_connector_message_by_external_id_for_tenant(
            "",
            connector_id,
            direction,
            external_message_id,
        )
    }

    pub fn get_connector_message_by_external_id_for_tenant(
        &self,
        tenant_id: &str,
        connector_id: &str,
        direction: kura_imtypes::DeliveryDirection,
        external_message_id: &str,
    ) -> Result<Option<kura_imtypes::MessageRecord>, String> {
        if external_message_id.trim().is_empty() {
            return Ok(None);
        }
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT delivery_id, tenant_id, connector_id, direction, external_message_id,
                    connector_account_id, channel_or_conversation_id, provider_message_id,
                    equivalent_rule_id, session_id, thread_session_segment_id, run_id, channel_id,
                    peer_id, thread_id, author_id, content, status, error_text,
                    reply_to_external_message_id, response_to_delivery_id, foreground_outcome_status,
                    background_delivery_id, delivery_boundary_kind, created_at, updated_at
                FROM connector_messages
                WHERE tenant_id = ?1 AND connector_id = ?2 AND direction = ?3 AND external_message_id = ?4"#,
            )
            .map_err(|e| e.to_string())?;
        let mut rows = stmt
            .query(params![
                tenant_id,
                connector_id,
                enum_str(&direction),
                external_message_id
            ])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        scan_connector_message(row).map(Some)
    }

    pub fn get_connector_message_by_standard_identity(
        &self,
        tenant_id: &str,
        connector_account_id: &str,
        channel_or_conversation_id: &str,
        provider_message_id: &str,
        direction: kura_imtypes::DeliveryDirection,
        equivalent_rule_id: &str,
    ) -> Result<Option<kura_imtypes::MessageRecord>, String> {
        let equivalent_rule_id = if equivalent_rule_id.trim().is_empty() {
            "standard_provider_message_id"
        } else {
            equivalent_rule_id
        };
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT delivery_id, tenant_id, connector_id, direction, external_message_id,
                    connector_account_id, channel_or_conversation_id, provider_message_id,
                    equivalent_rule_id, session_id, thread_session_segment_id, run_id, channel_id,
                    peer_id, thread_id, author_id, content, status, error_text,
                    reply_to_external_message_id, response_to_delivery_id, foreground_outcome_status,
                    background_delivery_id, delivery_boundary_kind, created_at, updated_at
                FROM connector_messages
                WHERE tenant_id = ?1 AND connector_account_id = ?2 AND channel_or_conversation_id = ?3
                    AND provider_message_id = ?4 AND direction = ?5 AND equivalent_rule_id = ?6"#,
            )
            .map_err(|e| e.to_string())?;
        let mut rows = stmt
            .query(params![
                tenant_id,
                connector_account_id,
                channel_or_conversation_id,
                provider_message_id,
                enum_str(&direction),
                equivalent_rule_id,
            ])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        scan_connector_message(row).map(Some)
    }

    pub fn save_connector_conformance_result(
        &self,
        result: &kura_connectors::ConformanceResult,
    ) -> Result<(), String> {
        let mut result = result.clone();
        if result.conformance_result_id.trim().is_empty() {
            result.conformance_result_id = new_store_id("conformance_result");
        }
        if is_unset_time(&result.evidence_timestamp) {
            result.evidence_timestamp = Utc::now();
        }
        if is_unset_time(&result.retention_expires_at) {
            result.retention_expires_at = result.evidence_timestamp + chrono::Duration::days(90);
        }
        // Go defaults an empty RedactionStatus to Redacted; the Rust enum is non-empty by
        // construction (Default is Redacted), so no defaulting is needed here.
        let document = serde_json::to_string(&result)
            .map_err(|e| format!("marshal connector conformance result: {e}"))?;

        self.conn
            .execute(
                r#"INSERT INTO connector_conformance_results (
                    conformance_result_id, tenant_id, connector_kind, connector_id, scenario_id,
                    area, result, reason_code, redaction_status, evidence_timestamp,
                    retention_expires_at, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                ON CONFLICT(conformance_result_id) DO UPDATE SET
                    tenant_id = COALESCE(connector_conformance_results.tenant_id, excluded.tenant_id),
                    connector_kind = excluded.connector_kind,
                    connector_id = excluded.connector_id,
                    scenario_id = excluded.scenario_id,
                    area = excluded.area,
                    result = excluded.result,
                    reason_code = excluded.reason_code,
                    redaction_status = excluded.redaction_status,
                    evidence_timestamp = excluded.evidence_timestamp,
                    retention_expires_at = excluded.retention_expires_at,
                    document_json = excluded.document_json"#,
                params![
                    result.conformance_result_id,
                    result.tenant_id,
                    result.connector_kind,
                    null_string(&result.connector_id),
                    result.scenario_id,
                    result.area,
                    enum_str(&result.result),
                    null_string(&result.reason_code),
                    enum_str(&result.redaction_status),
                    now_rfc3339(&result.evidence_timestamp),
                    now_rfc3339(&result.retention_expires_at),
                    document,
                ],
            )
            .map_err(|e| format!("save connector conformance result: {e}"))?;
        Ok(())
    }

    pub fn list_connector_conformance_results(
        &self,
        tenant_id: &str,
        connector_id: &str,
        now: DateTime<Utc>,
    ) -> Result<Vec<kura_connectors::ConformanceResult>, String> {
        let now = if is_unset_time(&now) { Utc::now() } else { now };
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT conformance_result_id, tenant_id, connector_kind, COALESCE(connector_id, ''),
                    scenario_id, area, result, COALESCE(reason_code, ''), redaction_status,
                    evidence_timestamp, retention_expires_at
                FROM connector_conformance_results
                WHERE tenant_id = ?1 AND connector_id = ?2 AND retention_expires_at > ?3
                ORDER BY evidence_timestamp DESC, conformance_result_id DESC"#,
            )
            .map_err(|e| format!("list connector conformance results: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id, connector_id, now_rfc3339(&now)])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_connector_conformance_result(row)?);
        }
        Ok(items)
    }

    pub fn save_connector_diagnostic_state(
        &self,
        state: &kura_connectors::ConnectorDiagnosticState,
    ) -> Result<(), String> {
        let mut state = state.clone();
        if state.diagnostic_state_id.trim().is_empty() {
            state.diagnostic_state_id = new_store_id("connector_diagnostic");
        }
        if is_unset_time(&state.evidence_timestamp) {
            state.evidence_timestamp = Utc::now();
        }
        if is_unset_time(&state.retention_expires_at) {
            state.retention_expires_at = state.evidence_timestamp + chrono::Duration::days(90);
        }
        // Go defaults an empty RedactionStatus to Redacted; the Rust enum is non-empty by
        // construction (Default is Redacted), so no defaulting is needed here.
        let redaction_failure_id = if (state.redaction_status
            == kura_connectors::RedactionStatus::Suppressed
            || state.redaction_status == kura_connectors::RedactionStatus::Failed)
            && state.redaction_failure_id.trim().is_empty()
        {
            new_store_id("connector_diagnostic_redaction_failure")
        } else {
            state.redaction_failure_id.clone()
        };
        let document = serde_json::to_string(&state)
            .map_err(|e| format!("marshal connector diagnostic state: {e}"))?;
        let stale_after = state.evidence_timestamp + chrono::Duration::minutes(15);

        self.conn
            .execute(
                r#"INSERT INTO connector_diagnostic_states (
                    diagnostic_state_id, tenant_id, connector_id, connector_account_id, status,
                    reason_code, remediation_owner, user_visible_severity, retry_safety,
                    evidence_timestamp, stale_after, freshness_state, redaction_status,
                    retention_expires_at, redaction_failure_id, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
                ON CONFLICT(diagnostic_state_id) DO UPDATE SET
                    tenant_id = COALESCE(connector_diagnostic_states.tenant_id, excluded.tenant_id),
                    connector_id = excluded.connector_id,
                    connector_account_id = excluded.connector_account_id,
                    status = excluded.status,
                    reason_code = excluded.reason_code,
                    remediation_owner = excluded.remediation_owner,
                    user_visible_severity = excluded.user_visible_severity,
                    retry_safety = excluded.retry_safety,
                    evidence_timestamp = excluded.evidence_timestamp,
                    stale_after = excluded.stale_after,
                    freshness_state = excluded.freshness_state,
                    redaction_status = excluded.redaction_status,
                    retention_expires_at = excluded.retention_expires_at,
                    redaction_failure_id = excluded.redaction_failure_id,
                    document_json = excluded.document_json"#,
                params![
                    state.diagnostic_state_id,
                    state.tenant_id,
                    state.connector_id,
                    null_string(&state.connector_account_id),
                    enum_str(&state.status),
                    enum_str(&state.reason_code),
                    enum_str(&state.remediation_owner),
                    state.user_visible_severity,
                    enum_str(&state.retry_safety),
                    now_rfc3339(&state.evidence_timestamp),
                    opt_time_string(&Some(stale_after)),
                    enum_str(&state.freshness_state),
                    enum_str(&state.redaction_status),
                    now_rfc3339(&state.retention_expires_at),
                    null_string(&redaction_failure_id),
                    document.clone(),
                ],
            )
            .map_err(|e| format!("save connector diagnostic state: {e}"))?;

        if !redaction_failure_id.trim().is_empty() {
            self.conn
                .execute(
                    r#"INSERT INTO connector_diagnostic_redaction_failures (
                        redaction_failure_id, tenant_id, connector_id, diagnostic_state_id,
                        reason_code, occurred_at, retention_expires_at, document_json
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                    ON CONFLICT(redaction_failure_id) DO UPDATE SET
                        tenant_id = COALESCE(connector_diagnostic_redaction_failures.tenant_id, excluded.tenant_id),
                        connector_id = excluded.connector_id,
                        diagnostic_state_id = excluded.diagnostic_state_id,
                        reason_code = excluded.reason_code,
                        occurred_at = excluded.occurred_at,
                        retention_expires_at = excluded.retention_expires_at,
                        document_json = excluded.document_json"#,
                    params![
                        redaction_failure_id,
                        state.tenant_id,
                        state.connector_id,
                        null_string(&state.diagnostic_state_id),
                        enum_str(&state.reason_code),
                        now_rfc3339(&state.evidence_timestamp),
                        now_rfc3339(&state.retention_expires_at),
                        document,
                    ],
                )
                .map_err(|e| format!("save connector diagnostic redaction failure {}: {e}", redaction_failure_id))?;
        }
        Ok(())
    }

    pub fn list_connector_diagnostic_states(
        &self,
        tenant_id: &str,
        connector_id: &str,
        now: DateTime<Utc>,
    ) -> Result<Vec<kura_connectors::ConnectorDiagnosticState>, String> {
        let now = if is_unset_time(&now) { Utc::now() } else { now };
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT diagnostic_state_id, tenant_id, connector_id, COALESCE(connector_account_id, ''),
                    status, reason_code, remediation_owner, user_visible_severity, retry_safety,
                    evidence_timestamp, freshness_state, redaction_status, retention_expires_at,
                    COALESCE(redaction_failure_id, '')
                FROM connector_diagnostic_states
                WHERE tenant_id = ?1 AND connector_id = ?2 AND retention_expires_at > ?3
                ORDER BY evidence_timestamp DESC, diagnostic_state_id DESC"#,
            )
            .map_err(|e| format!("list connector diagnostic states: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id, connector_id, now_rfc3339(&now)])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_connector_diagnostic_state(row, now)?);
        }
        Ok(items)
    }
}
