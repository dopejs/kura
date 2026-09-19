//! SQLite CRUD for the thread/session lifecycle domain (threads, session segments,
//! source linkages, runtime projections, group/room conversation shapes and
//! participation decisions, reset events) and the per-tenant retention policy.
//! Ported from `daemon/internal/store/thread_lifecycle.go` (UpsertThread,
//! UpsertThreadSessionSegment, GetThreadForTenant, GetCurrentThreadForSource,
//! SetThreadRetentionPolicy, ThreadRetentionExpiry, SaveThreadSourceLinkage,
//! SaveThreadRuntimeProjection, ListThreadsForTenant) and
//! `daemon/internal/store/thread_group_room.go` (SaveConversationShapeEvidence,
//! GetConversationShapeForThread, SaveParticipationDecision,
//! GetParticipationDecisionBySourceMessage, ListParticipationDecisionsForThread,
//! SaveResetEvent, ListResetEventsForThread).
//!
//! Documents are stored as the full camelCase JSON snapshot (the Go daemon
//! round-trips `document_json` on read); the indexed columns carry the fields
//! the retention/tenant/source queries filter on.

use chrono::{DateTime, Duration, Utc};
use rusqlite::{Row, params};

use crate::SQLiteStore;
use crate::crud::{enum_str, now_rfc3339, null_string, opt_time_string, parse_rfc3339};

/// A chrono-defaulted timestamp (UNIX epoch) stands in for Go's zero `time.Time`.
fn is_unset_time(dt: &DateTime<Utc>) -> bool {
    dt.timestamp() == 0 && dt.timestamp_subsec_nanos() == 0
}

/// Go `newStoreID`.
fn new_store_id(prefix: &str) -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!("{prefix}_{}", &hex[..16])
}

/// Go `isUniqueConstraintError`.
fn is_unique_constraint_error(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(e, _) if e.code == rusqlite::ErrorCode::ConstraintViolation
    )
}

/// Decodes a `document_json` column back into a domain record.
fn decode_document<T: serde::de::DeserializeOwned>(raw: &str, what: &str) -> Result<T, String> {
    serde_json::from_str(raw).map_err(|e| format!("decode {what}: {e}"))
}

/// Scan a `threads` row by its document column (Go reads `document_json` only).
fn scan_thread(row: &Row) -> Result<kura_threads::Thread, String> {
    let raw: String = row.get(0).map_err(|e| e.to_string())?;
    decode_document(&raw, "thread")
}

fn scan_session_segment(row: &Row) -> Result<kura_threads::SessionSegment, String> {
    let raw: String = row.get(0).map_err(|e| e.to_string())?;
    decode_document(&raw, "thread session segment")
}

fn scan_source_linkage(row: &Row) -> Result<kura_threads::SourceLinkage, String> {
    let raw: String = row.get(0).map_err(|e| e.to_string())?;
    decode_document(&raw, "thread source linkage")
}

fn scan_runtime_projection(row: &Row) -> Result<kura_threads::RuntimeProjection, String> {
    let raw: String = row.get(0).map_err(|e| e.to_string())?;
    decode_document(&raw, "thread runtime projection")
}

fn scan_conversation_shape(row: &Row) -> Result<kura_threads::ConversationShapeEvidence, String> {
    let raw: String = row.get(0).map_err(|e| e.to_string())?;
    decode_document(&raw, "conversation shape evidence")
}

fn scan_participation_decision(row: &Row) -> Result<kura_threads::ParticipationDecision, String> {
    let raw: String = row.get(0).map_err(|e| e.to_string())?;
    decode_document(&raw, "participation decision")
}

fn scan_reset_event(row: &Row) -> Result<kura_threads::ResetEvent, String> {
    let raw: String = row.get(0).map_err(|e| e.to_string())?;
    decode_document(&raw, "reset event")
}

/// Go `ThreadListQuery`.
#[derive(Debug, Clone, Default)]
pub struct ThreadListQuery {
    pub tenant_id: String,
    pub limit: i64,
    pub cursor: String,
    pub state_filter: String,
    pub source_filter: String,
}

impl SQLiteStore {
    /// Go `UpsertThread` — defaults created/updated timestamps and the redaction
    /// status, then upserts on `thread_id`.
    pub fn upsert_thread(&self, thread: &kura_threads::Thread) -> Result<(), String> {
        let mut thread = thread.clone();
        let now = Utc::now();
        if is_unset_time(&thread.created_at) {
            thread.created_at = now;
        }
        if is_unset_time(&thread.updated_at) {
            thread.updated_at = now;
        }
        if is_unset_time(&thread.last_activity_at) {
            thread.last_activity_at = thread.updated_at;
        }
        let document = serde_json::to_string(&thread)
            .map_err(|e| format!("marshal thread {}: {e}", thread.thread_id))?;

        self.conn
            .execute(
                r#"INSERT INTO threads (
                    thread_id, tenant_id, lifecycle_state, current_session_segment_id, source_kind,
                    source_summary, last_activity_at, created_at, updated_at, retention_expires_at,
                    redaction_status, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                ON CONFLICT(thread_id) DO UPDATE SET
                    tenant_id = excluded.tenant_id,
                    lifecycle_state = excluded.lifecycle_state,
                    current_session_segment_id = excluded.current_session_segment_id,
                    source_kind = excluded.source_kind,
                    source_summary = excluded.source_summary,
                    last_activity_at = excluded.last_activity_at,
                    updated_at = excluded.updated_at,
                    retention_expires_at = excluded.retention_expires_at,
                    redaction_status = excluded.redaction_status,
                    document_json = excluded.document_json"#,
                params![
                    thread.thread_id,
                    thread.tenant_id,
                    enum_str(&thread.lifecycle_state),
                    thread.current_session_segment_id,
                    enum_str(&thread.source_kind),
                    thread.source_summary,
                    now_rfc3339(&thread.last_activity_at),
                    now_rfc3339(&thread.created_at),
                    now_rfc3339(&thread.updated_at),
                    opt_time_string(&thread.retention_expires_at),
                    enum_str(&thread.redaction_status),
                    document,
                ],
            )
            .map_err(|e| format!("upsert thread {}: {e}", thread.thread_id))?;
        Ok(())
    }

    /// Go `UpsertThreadSessionSegment`.
    pub fn upsert_thread_session_segment(
        &self,
        segment: &kura_threads::SessionSegment,
    ) -> Result<(), String> {
        let mut segment = segment.clone();
        let now = Utc::now();
        if is_unset_time(&segment.started_at) {
            segment.started_at = now;
        }
        if is_unset_time(&segment.last_active_at) {
            segment.last_active_at = segment.started_at;
        }
        if segment.state.trim().is_empty() {
            segment.state = "active".to_string();
        }
        let document = serde_json::to_string(&segment).map_err(|e| {
            format!(
                "marshal thread session segment {}: {e}",
                segment.session_segment_id
            )
        })?;

        self.conn
            .execute(
                r#"INSERT INTO thread_session_segments (
                    session_segment_id, thread_id, tenant_id, session_id, generation, state,
                    started_at, ended_at, last_active_at, reset_from_session_segment_id,
                    partial_evidence, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                ON CONFLICT(session_segment_id) DO UPDATE SET
                    thread_id = excluded.thread_id,
                    tenant_id = excluded.tenant_id,
                    session_id = excluded.session_id,
                    generation = excluded.generation,
                    state = excluded.state,
                    started_at = excluded.started_at,
                    ended_at = excluded.ended_at,
                    last_active_at = excluded.last_active_at,
                    reset_from_session_segment_id = excluded.reset_from_session_segment_id,
                    partial_evidence = excluded.partial_evidence,
                    document_json = excluded.document_json"#,
                params![
                    segment.session_segment_id,
                    segment.thread_id,
                    segment.tenant_id,
                    null_string(&segment.session_id),
                    i64::from(segment.generation),
                    segment.state,
                    now_rfc3339(&segment.started_at),
                    opt_time_string(&segment.ended_at),
                    now_rfc3339(&segment.last_active_at),
                    null_string(&segment.reset_from_session_segment_id),
                    i64::from(segment.partial_evidence),
                    document,
                ],
            )
            .map_err(|e| {
                format!(
                    "upsert thread session segment {}: {e}",
                    segment.session_segment_id
                )
            })?;
        Ok(())
    }

    /// Go `GetThreadForTenant`.
    pub fn get_thread_for_tenant(
        &self,
        tenant_id: &str,
        thread_id: &str,
    ) -> Result<Option<kura_threads::Thread>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT document_json FROM threads WHERE tenant_id = ?1 AND thread_id = ?2")
            .map_err(|e| format!("get thread {tenant_id}/{thread_id}: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id, thread_id])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        scan_thread(row).map(Some)
    }

    /// Go `GetCurrentThreadForSource` — normalizes the continuation key (trim +
    /// lowercase, rejects missing parts) and joins the current source linkage to
    /// its thread.
    pub fn get_current_thread_for_source(
        &self,
        key: &kura_threads::SourceContinuationKey,
    ) -> Result<Option<kura_threads::Thread>, String> {
        let normalized = kura_threads::normalize_source_continuation_key(key)
            .map_err(|e| format!("normalize source continuation key: {e}"))?;
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT t.document_json
                FROM thread_source_links AS l
                JOIN threads AS t ON t.thread_id = l.thread_id AND t.tenant_id = l.tenant_id
                WHERE l.tenant_id = ?1 AND l.connector_id = ?2 AND l.source_account_id = ?3
                    AND l.source_conversation_id = ?4 AND l.current_flag = 1"#,
            )
            .map_err(|e| format!("get current thread for source {normalized}: {e}"))?;
        let mut rows = stmt
            .query(params![
                normalized.tenant_id,
                normalized.connector_id,
                normalized.source_account_id,
                normalized.source_conversation_id,
            ])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        scan_thread(row).map(Some)
    }

    /// Go `SetThreadRetentionPolicy`.
    pub fn set_thread_retention_policy(
        &self,
        tenant_id: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<(), String> {
        self.conn
            .execute(
                r#"INSERT INTO thread_retention_policies (tenant_id, retention_expires_at)
                VALUES (?1, ?2)
                ON CONFLICT(tenant_id) DO UPDATE SET retention_expires_at = excluded.retention_expires_at"#,
                params![tenant_id, now_rfc3339(&expires_at)],
            )
            .map_err(|e| format!("set thread retention policy {tenant_id}: {e}"))?;
        Ok(())
    }

    /// Go `ThreadRetentionExpiry` — 90 days from `now`, or the tenant override
    /// when it is later.
    pub fn thread_retention_expiry(
        &self,
        tenant_id: &str,
        now: DateTime<Utc>,
    ) -> Result<DateTime<Utc>, String> {
        let now = if is_unset_time(&now) { Utc::now() } else { now };
        let default_expiry = now + Duration::days(90);
        // Go returns the default expiry on any lookup error (missing row or otherwise).
        let raw: Option<String> = match self.conn.query_row(
            "SELECT retention_expires_at FROM thread_retention_policies WHERE tenant_id = ?1",
            params![tenant_id],
            |row| row.get(0),
        ) {
            Ok(raw) => raw,
            Err(_) => return Ok(default_expiry),
        };
        match raw {
            Some(raw) => match parse_rfc3339(&raw) {
                Ok(parsed) if parsed >= default_expiry => Ok(parsed),
                _ => Ok(default_expiry),
            },
            None => Ok(default_expiry),
        }
    }

    /// Go `SaveThreadSourceLinkage` — when the linkage is current, first clears
    /// any existing current linkage for the same source, then upserts within a
    /// transaction.
    pub fn save_thread_source_linkage(
        &self,
        linkage: &kura_threads::SourceLinkage,
    ) -> Result<(), String> {
        let mut linkage = linkage.clone();
        if linkage.linked_at.is_none() || is_unset_time(&linkage.linked_at.unwrap_or_default()) {
            linkage.linked_at = Some(Utc::now());
        }
        let linked_at = linkage.linked_at.unwrap_or_default();
        if linkage.retention_expires_at.is_none() {
            linkage.retention_expires_at =
                Some(self.thread_retention_expiry(&linkage.tenant_id, linked_at)?);
        }
        let document = serde_json::to_string(&linkage).map_err(|e| {
            format!(
                "marshal thread source linkage {}: {e}",
                linkage.source_linkage_id
            )
        })?;

        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin thread source linkage: {e}"))?;
        if linkage.current {
            tx.execute(
                r#"UPDATE thread_source_links
                SET current_flag = 0
                WHERE tenant_id = ?1 AND connector_id = ?2 AND source_account_id = ?3
                    AND source_conversation_id = ?4 AND current_flag = 1"#,
                params![
                    linkage.tenant_id,
                    linkage.connector_id,
                    linkage.source_account_id,
                    linkage.source_conversation_id,
                ],
            )
            .map_err(|e| format!("clear current source linkage: {e}"))?;
        }
        tx.execute(
            r#"INSERT INTO thread_source_links (
                source_linkage_id, thread_id, tenant_id, source_kind, connector_id,
                connector_kind, source_account_id, source_conversation_id, source_message_id,
                routing_outcome, current_flag, linked_at, retention_expires_at,
                redaction_status, document_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
            ON CONFLICT(source_linkage_id) DO UPDATE SET
                thread_id = excluded.thread_id,
                tenant_id = excluded.tenant_id,
                source_kind = excluded.source_kind,
                connector_id = excluded.connector_id,
                connector_kind = excluded.connector_kind,
                source_account_id = excluded.source_account_id,
                source_conversation_id = excluded.source_conversation_id,
                source_message_id = excluded.source_message_id,
                routing_outcome = excluded.routing_outcome,
                current_flag = excluded.current_flag,
                linked_at = excluded.linked_at,
                retention_expires_at = excluded.retention_expires_at,
                redaction_status = excluded.redaction_status,
                document_json = excluded.document_json"#,
            params![
                linkage.source_linkage_id,
                linkage.thread_id,
                linkage.tenant_id,
                enum_str(&linkage.source_kind),
                null_string(&linkage.connector_id),
                null_string(&linkage.connector_kind),
                null_string(&linkage.source_account_id),
                null_string(&linkage.source_conversation_id),
                null_string(&linkage.source_message_id),
                enum_str(&linkage.routing_outcome),
                i64::from(linkage.current),
                now_rfc3339(&linked_at),
                opt_time_string(&linkage.retention_expires_at),
                enum_str(&linkage.redaction_status),
                document,
            ],
        )
        .map_err(|e| {
            format!(
                "save thread source linkage {}: {e}",
                linkage.source_linkage_id
            )
        })?;
        tx.commit()
            .map_err(|e| format!("commit thread source linkage: {e}"))?;
        Ok(())
    }

    /// Go `SaveThreadRuntimeProjection`.
    pub fn save_thread_runtime_projection(
        &self,
        projection: &kura_threads::RuntimeProjection,
    ) -> Result<(), String> {
        let mut projection = projection.clone();
        let occurred_at = if is_unset_time(&projection.occurred_at) {
            let now = Utc::now();
            projection.occurred_at = now;
            now
        } else {
            projection.occurred_at
        };
        if projection.retention_expires_at.is_none() {
            projection.retention_expires_at =
                Some(self.thread_retention_expiry(&projection.tenant_id, occurred_at)?);
        }
        let document = serde_json::to_string(&projection).map_err(|e| {
            format!(
                "marshal thread runtime projection {}: {e}",
                projection.runtime_projection_id
            )
        })?;

        self.conn
            .execute(
                r#"INSERT INTO thread_runtime_projections (
                    runtime_projection_id, thread_id, tenant_id, session_segment_id,
                    resource_kind, resource_id, status, reason_code, occurred_at,
                    route, safe_summary, retention_expires_at, redaction_status, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                ON CONFLICT(runtime_projection_id) DO UPDATE SET
                    thread_id = excluded.thread_id,
                    tenant_id = excluded.tenant_id,
                    session_segment_id = excluded.session_segment_id,
                    resource_kind = excluded.resource_kind,
                    resource_id = excluded.resource_id,
                    status = excluded.status,
                    reason_code = excluded.reason_code,
                    occurred_at = excluded.occurred_at,
                    route = excluded.route,
                    safe_summary = excluded.safe_summary,
                    retention_expires_at = excluded.retention_expires_at,
                    redaction_status = excluded.redaction_status,
                    document_json = excluded.document_json"#,
                params![
                    projection.runtime_projection_id,
                    projection.thread_id,
                    projection.tenant_id,
                    null_string(&projection.session_segment_id),
                    enum_str(&projection.resource_kind),
                    projection.resource_id,
                    projection.status,
                    null_string(&projection.reason_code),
                    now_rfc3339(&projection.occurred_at),
                    null_string(&projection.route),
                    null_string(&projection.safe_summary),
                    opt_time_string(&projection.retention_expires_at),
                    enum_str(&projection.redaction_status),
                    document,
                ],
            )
            .map_err(|e| {
                format!(
                    "save thread runtime projection {}: {e}",
                    projection.runtime_projection_id
                )
            })?;
        Ok(())
    }

    /// Go `ListThreadsForTenant` — archived threads sort last, then recency,
    /// then id; fetches limit+1 rows to derive the next cursor.
    pub fn list_threads_for_tenant(
        &self,
        query: &ThreadListQuery,
    ) -> Result<kura_threads::ThreadListResponse, String> {
        let limit = if query.limit <= 0 { 20 } else { query.limit };
        let offset = match query.cursor.parse::<i64>() {
            Ok(parsed) if parsed > 0 => parsed,
            _ => 0,
        };
        let mut sql = String::from("SELECT document_json FROM threads WHERE tenant_id = ?1");
        let mut args: Vec<Box<dyn rusqlite::types::ToSql>> =
            vec![Box::new(query.tenant_id.clone())];
        if !query.state_filter.trim().is_empty() {
            sql.push_str(" AND lifecycle_state = ?");
            args.push(Box::new(query.state_filter.trim().to_string()));
        }
        if !query.source_filter.trim().is_empty() {
            sql.push_str(" AND source_kind = ?");
            args.push(Box::new(query.source_filter.trim().to_string()));
        }
        sql.push_str(
            " ORDER BY CASE lifecycle_state WHEN 'archived' THEN 1 ELSE 0 END ASC,
                last_activity_at DESC,
                thread_id ASC
            LIMIT ? OFFSET ?",
        );
        args.push(Box::new(limit + 1));
        args.push(Box::new(offset));
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("list threads for tenant {}: {e}", query.tenant_id))?;
        let mut rows = stmt
            .query(rusqlite::params_from_iter(args.iter()))
            .map_err(|e| e.to_string())?;

        let mut items = Vec::new();
        let mut count: i64 = 0;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            count += 1;
            if count > limit {
                continue;
            }
            let thread = scan_thread(row)?;
            items.push(kura_threads::build_thread_resource(&thread, ""));
        }
        let next_cursor = if count > limit {
            (offset + limit).to_string()
        } else {
            String::new()
        };
        Ok(kura_threads::ThreadListResponse {
            tenant_id: query.tenant_id.clone(),
            page: kura_threads::ThreadPage {
                limit: limit as i32,
                next_cursor,
                order: "active_recent_archived_id".to_string(),
            },
            items,
        })
    }

    /// Go `threadSegments` — all segments for a thread in generation order.
    pub fn list_thread_session_segments(
        &self,
        tenant_id: &str,
        thread_id: &str,
    ) -> Result<Vec<kura_threads::SessionSegment>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT document_json
                FROM thread_session_segments
                WHERE tenant_id = ?1 AND thread_id = ?2
                ORDER BY generation ASC, session_segment_id ASC"#,
            )
            .map_err(|e| format!("list thread session segments {tenant_id}/{thread_id}: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id, thread_id])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_session_segment(row)?);
        }
        Ok(items)
    }

    /// Go `threadSourceLinkages` — current + unexpired linkages, newest first.
    pub fn list_thread_source_linkages(
        &self,
        tenant_id: &str,
        thread_id: &str,
        now: DateTime<Utc>,
    ) -> Result<Vec<kura_threads::SourceLinkage>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT document_json
                FROM thread_source_links
                WHERE tenant_id = ?1 AND thread_id = ?2
                  AND (retention_expires_at IS NULL OR retention_expires_at = '' OR retention_expires_at > ?3)
                ORDER BY linked_at DESC, source_linkage_id DESC"#,
            )
            .map_err(|e| format!("list thread source linkages {tenant_id}/{thread_id}: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id, thread_id, now_rfc3339(&now)])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_source_linkage(row)?);
        }
        Ok(items)
    }

    /// Go `threadRuntimeProjections` — unexpired projections, newest first.
    pub fn list_thread_runtime_projections(
        &self,
        tenant_id: &str,
        thread_id: &str,
        now: DateTime<Utc>,
    ) -> Result<Vec<kura_threads::RuntimeProjection>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT document_json
                FROM thread_runtime_projections
                WHERE tenant_id = ?1 AND thread_id = ?2
                  AND (retention_expires_at IS NULL OR retention_expires_at = '' OR retention_expires_at > ?3)
                ORDER BY occurred_at DESC, runtime_projection_id DESC"#,
            )
            .map_err(|e| format!("list thread runtime projections {tenant_id}/{thread_id}: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id, thread_id, now_rfc3339(&now)])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_runtime_projection(row)?);
        }
        Ok(items)
    }

    /// Go `SaveConversationShapeEvidence` — allocates an id, defaults the
    /// evidence timestamps and retention, then upserts.
    pub fn save_conversation_shape_evidence(
        &self,
        evidence: &kura_threads::ConversationShapeEvidence,
    ) -> Result<(), String> {
        let mut evidence = evidence.clone();
        if evidence.conversation_shape_id.trim().is_empty() {
            evidence.conversation_shape_id = new_store_id("shape");
        }
        let now = Utc::now();
        let recorded_at = evidence.recorded_at.unwrap_or(now);
        let updated_at = evidence.updated_at.unwrap_or(recorded_at);
        evidence.recorded_at = Some(recorded_at);
        evidence.updated_at = Some(updated_at);
        if evidence.retention_expires_at.is_none() {
            evidence.retention_expires_at =
                Some(self.thread_retention_expiry(&evidence.tenant_id, updated_at)?);
        }
        let document = serde_json::to_string(&evidence).map_err(|e| {
            format!(
                "marshal conversation shape evidence {}: {e}",
                evidence.conversation_shape_id
            )
        })?;

        self.conn
            .execute(
                r#"INSERT INTO thread_conversation_shapes (
                    conversation_shape_id, tenant_id, thread_id, session_segment_id, shape,
                    source_kind, connector_id, connector_kind, source_account_id, source_conversation_id,
                    shape_evidence_status, recorded_at, updated_at, retention_expires_at, redaction_status, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
                ON CONFLICT(conversation_shape_id) DO UPDATE SET
                    thread_id = excluded.thread_id,
                    session_segment_id = excluded.session_segment_id,
                    shape = excluded.shape,
                    source_kind = excluded.source_kind,
                    connector_id = excluded.connector_id,
                    connector_kind = excluded.connector_kind,
                    source_account_id = excluded.source_account_id,
                    source_conversation_id = excluded.source_conversation_id,
                    shape_evidence_status = excluded.shape_evidence_status,
                    updated_at = excluded.updated_at,
                    retention_expires_at = excluded.retention_expires_at,
                    redaction_status = excluded.redaction_status,
                    document_json = excluded.document_json"#,
                params![
                    evidence.conversation_shape_id,
                    evidence.tenant_id,
                    evidence.thread_id,
                    null_string(&evidence.session_segment_id),
                    enum_str(&evidence.shape),
                    evidence.source_kind.map(|kind| enum_str(&kind)),
                    null_string(&evidence.connector_id),
                    null_string(&evidence.connector_kind),
                    null_string(&evidence.source_account_id),
                    null_string(&evidence.source_conversation_id),
                    enum_str(&evidence.shape_evidence_status),
                    now_rfc3339(&recorded_at),
                    now_rfc3339(&updated_at),
                    opt_time_string(&evidence.retention_expires_at),
                    enum_str(&evidence.redaction_status),
                    document,
                ],
            )
            .map_err(|e| format!("save conversation shape evidence {}: {e}", evidence.conversation_shape_id))?;
        Ok(())
    }

    /// Go `GetConversationShapeForThread` — the most recently updated shape.
    pub fn get_conversation_shape_for_thread(
        &self,
        tenant_id: &str,
        thread_id: &str,
    ) -> Result<Option<kura_threads::ConversationShapeEvidence>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT document_json
                FROM thread_conversation_shapes
                WHERE tenant_id = ?1 AND thread_id = ?2
                ORDER BY updated_at DESC, conversation_shape_id DESC
                LIMIT 1"#,
            )
            .map_err(|e| format!("get conversation shape {tenant_id}/{thread_id}: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id, thread_id])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        scan_conversation_shape(row).map(Some)
    }

    /// Go `SaveParticipationDecision` — plain insert; a duplicate source-message
    /// identity (unique index) resolves to the existing row instead of failing.
    pub fn save_participation_decision(
        &self,
        decision: &kura_threads::ParticipationDecision,
    ) -> Result<(), String> {
        let mut decision = decision.clone();
        if decision.participation_decision_id.trim().is_empty() {
            decision.participation_decision_id = new_store_id("part");
        }
        let occurred_at = decision.occurred_at.unwrap_or_else(Utc::now);
        decision.occurred_at = Some(occurred_at);
        if decision.retention_expires_at.is_none() {
            decision.retention_expires_at =
                Some(self.thread_retention_expiry(&decision.tenant_id, occurred_at)?);
        }
        let document = serde_json::to_string(&decision).map_err(|e| {
            format!(
                "marshal participation decision {}: {e}",
                decision.participation_decision_id
            )
        })?;

        let insert = self.conn.execute(
            r#"INSERT INTO thread_participation_decisions (
                participation_decision_id, tenant_id, thread_id, session_segment_id,
                connector_id, source_account_id, source_conversation_id, source_message_id,
                conversation_shape, decision, reason_code, created_assistant_work,
                occurred_at, retention_expires_at, redaction_status, document_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)"#,
            params![
                decision.participation_decision_id,
                decision.tenant_id,
                decision.thread_id,
                null_string(&decision.session_segment_id),
                null_string(&decision.connector_id),
                null_string(&decision.source_account_id),
                null_string(&decision.source_conversation_id),
                null_string(&decision.source_message_id),
                enum_str(&decision.conversation_shape),
                enum_str(&decision.decision),
                decision.reason_code,
                i64::from(decision.created_assistant_work),
                now_rfc3339(&occurred_at),
                opt_time_string(&decision.retention_expires_at),
                enum_str(&decision.redaction_status),
                document,
            ],
        );
        if let Err(err) = insert {
            if !decision.source_message_id.trim().is_empty() && is_unique_constraint_error(&err) {
                if let Some(existing) = self.get_participation_decision_by_source_message(
                    &decision.tenant_id,
                    &decision.connector_id,
                    &decision.source_account_id,
                    &decision.source_conversation_id,
                    &decision.source_message_id,
                )? {
                    let _ = existing;
                    return Ok(());
                }
            }
            return Err(format!(
                "save participation decision {}: {err}",
                decision.participation_decision_id
            ));
        }
        Ok(())
    }

    /// Go `GetParticipationDecisionBySourceMessage`.
    pub fn get_participation_decision_by_source_message(
        &self,
        tenant_id: &str,
        connector_id: &str,
        account_id: &str,
        conversation_id: &str,
        message_id: &str,
    ) -> Result<Option<kura_threads::ParticipationDecision>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT document_json
                FROM thread_participation_decisions
                WHERE tenant_id = ?1 AND connector_id = ?2 AND source_account_id = ?3
                    AND source_conversation_id = ?4 AND source_message_id = ?5
                LIMIT 1"#,
            )
            .map_err(|e| format!("get participation decision by source message: {e}"))?;
        let mut rows = stmt
            .query(params![
                tenant_id,
                connector_id,
                account_id,
                conversation_id,
                message_id
            ])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        scan_participation_decision(row).map(Some)
    }

    /// Go `ListParticipationDecisionsForThread`.
    pub fn list_participation_decisions_for_thread(
        &self,
        tenant_id: &str,
        thread_id: &str,
        limit: i64,
    ) -> Result<Vec<kura_threads::ParticipationDecision>, String> {
        let limit = if limit <= 0 { 20 } else { limit };
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT document_json
                FROM thread_participation_decisions
                WHERE tenant_id = ?1 AND thread_id = ?2
                ORDER BY occurred_at DESC, participation_decision_id DESC
                LIMIT ?3"#,
            )
            .map_err(|e| format!("list participation decisions {tenant_id}/{thread_id}: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id, thread_id, limit])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_participation_decision(row)?);
        }
        Ok(items)
    }

    /// Go `SaveResetEvent`.
    pub fn save_reset_event(&self, event: &kura_threads::ResetEvent) -> Result<(), String> {
        let mut event = event.clone();
        if event.reset_event_id.trim().is_empty() {
            event.reset_event_id = new_store_id("reset");
        }
        let requested_at = event.requested_at.unwrap_or_else(Utc::now);
        let completed_at = event.completed_at.unwrap_or(requested_at);
        event.requested_at = Some(requested_at);
        event.completed_at = Some(completed_at);
        if event.retention_expires_at.is_none() {
            event.retention_expires_at =
                Some(self.thread_retention_expiry(&event.tenant_id, completed_at)?);
        }
        if event.permission_gate.trim().is_empty() {
            event.permission_gate = "connectors.manage".to_string();
        }
        if event.reason_code.trim().is_empty() {
            event.reason_code = kura_threads::GROUP_ROOM_REASON_SCOPED_RESET_SUCCEEDED.to_string();
        }
        let document = serde_json::to_string(&event)
            .map_err(|e| format!("marshal reset event {}: {e}", event.reset_event_id))?;

        self.conn
            .execute(
                r#"INSERT INTO thread_reset_events (
                    reset_event_id, tenant_id, thread_id, conversation_shape, source_conversation_id,
                    actor_principal_id, permission_gate, prior_session_segment_id, resulting_session_segment_id,
                    status, reason_code, requested_at, completed_at, audit_event_id,
                    retention_expires_at, redaction_status, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
                ON CONFLICT(reset_event_id) DO UPDATE SET
                    conversation_shape = excluded.conversation_shape,
                    source_conversation_id = excluded.source_conversation_id,
                    status = excluded.status,
                    reason_code = excluded.reason_code,
                    completed_at = excluded.completed_at,
                    retention_expires_at = excluded.retention_expires_at,
                    redaction_status = excluded.redaction_status,
                    document_json = excluded.document_json"#,
                params![
                    event.reset_event_id,
                    event.tenant_id,
                    event.thread_id,
                    enum_str(&event.conversation_shape),
                    null_string(&event.source_conversation_id),
                    null_string(&event.actor_principal_id),
                    event.permission_gate,
                    null_string(&event.prior_session_segment_id),
                    null_string(&event.resulting_session_segment_id),
                    enum_str(&event.status),
                    event.reason_code,
                    now_rfc3339(&requested_at),
                    now_rfc3339(&completed_at),
                    null_string(&event.audit_event_id),
                    opt_time_string(&event.retention_expires_at),
                    enum_str(&event.redaction_status),
                    document,
                ],
            )
            .map_err(|e| format!("save reset event {}: {e}", event.reset_event_id))?;
        Ok(())
    }

    /// Go `ListResetEventsForThread`.
    pub fn list_reset_events_for_thread(
        &self,
        tenant_id: &str,
        thread_id: &str,
        limit: i64,
    ) -> Result<Vec<kura_threads::ResetEvent>, String> {
        let limit = if limit <= 0 { 20 } else { limit };
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT document_json
                FROM thread_reset_events
                WHERE tenant_id = ?1 AND thread_id = ?2
                ORDER BY completed_at DESC, reset_event_id DESC
                LIMIT ?3"#,
            )
            .map_err(|e| format!("list reset events {tenant_id}/{thread_id}: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id, thread_id, limit])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_reset_event(row)?);
        }
        Ok(items)
    }
}

/// Go `store.ThreadLifecycleMutationResult`.
#[derive(Debug, Clone)]
pub struct ThreadLifecycleMutationResult {
    pub thread: kura_threads::Thread,
    pub action: kura_threads::LifecycleAction,
    pub segment: Option<kura_threads::SessionSegment>,
}

impl SQLiteStore {
    /// Go `ApplyThreadLifecycleAction` — applies a reset/archive/reopen
    /// mutation inside a transaction with optimistic concurrency control on the
    /// prior thread row, recording the lifecycle action and (for resets) the
    /// scoped reset event. Returns `Ok(None)` when the thread does not exist.
    pub fn apply_thread_lifecycle_action(
        &self,
        tenant_id: &str,
        thread_id: &str,
        kind: kura_threads::LifecycleActionKind,
        input: &kura_threads::LifecycleMutationInput,
    ) -> Result<Option<ThreadLifecycleMutationResult>, String> {
        if input.audit_event_id.trim().is_empty() {
            return Err(kura_threads::ThreadsError::AuditEvidenceRequired.to_string());
        }
        let now = input.now.unwrap_or_else(Utc::now);
        let retention_expires_at = self.thread_retention_expiry(tenant_id, now)?;

        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin thread lifecycle action: {e}"))?;
        let thread = {
            let mut stmt = tx
                .prepare(
                    "SELECT document_json FROM threads WHERE tenant_id = ?1 AND thread_id = ?2",
                )
                .map_err(|e| format!("get thread {tenant_id}/{thread_id}: {e}"))?;
            let mut rows = stmt
                .query(params![tenant_id, thread_id])
                .map_err(|e| e.to_string())?;
            let Some(row) = rows.next().map_err(|e| e.to_string())? else {
                return Ok(None);
            };
            scan_thread(row)?
        };

        let mut input = input.clone();
        input.now = Some(now);
        let (updated, mut action, segment) = match kind {
            kura_threads::LifecycleActionKind::Reset => {
                if input.new_segment_id.trim().is_empty() {
                    input.new_segment_id = format!(
                        "seg_{}_{}",
                        thread_id,
                        now.timestamp_nanos_opt().unwrap_or_default()
                    );
                }
                let (next, lifecycle_action, mut new_segment) =
                    kura_threads::reset_thread(&thread, &input).map_err(|e| e.to_string())?;
                new_segment.generation =
                    next_thread_segment_generation_tx(&tx, tenant_id, thread_id)?;
                (next, lifecycle_action, Some(new_segment))
            }
            kura_threads::LifecycleActionKind::Archive => {
                let (next, lifecycle_action) =
                    kura_threads::archive_thread(&thread, &input).map_err(|e| e.to_string())?;
                (next, lifecycle_action, None)
            }
            kura_threads::LifecycleActionKind::Reopen => {
                if !thread_reopen_eligible_tx(&tx, &thread)? {
                    return Err(kura_threads::ThreadsError::LifecycleReopenNotEligible.to_string());
                }
                let (next, lifecycle_action) =
                    kura_threads::reopen_thread(&thread, &input).map_err(|e| e.to_string())?;
                (next, lifecycle_action, None)
            }
        };
        action.lifecycle_action_id = input.audit_event_id.clone();
        action.retention_expires_at = Some(retention_expires_at);
        update_thread_lifecycle_tx(&tx, &thread, &updated)?;
        if let Some(segment) = &segment {
            upsert_thread_session_segment_tx(&tx, segment)?;
        }
        insert_thread_lifecycle_action_tx(&tx, &action)?;
        if kind == kura_threads::LifecycleActionKind::Reset {
            let shape = {
                let mut stmt = tx
                    .prepare(
                        r#"SELECT document_json
                        FROM thread_conversation_shapes
                        WHERE tenant_id = ?1 AND thread_id = ?2
                        ORDER BY updated_at DESC, conversation_shape_id DESC
                        LIMIT 1"#,
                    )
                    .map_err(|e| {
                        format!("get reset conversation shape {tenant_id}/{thread_id}: {e}")
                    })?;
                let mut rows = stmt
                    .query(params![tenant_id, thread_id])
                    .map_err(|e| e.to_string())?;
                match rows.next().map_err(|e| e.to_string())? {
                    Some(row) => scan_conversation_shape(row)?,
                    None => kura_threads::ConversationShapeEvidence {
                        conversation_shape_id: String::new(),
                        tenant_id: tenant_id.to_string(),
                        thread_id: thread_id.to_string(),
                        session_segment_id: String::new(),
                        shape: kura_threads::ConversationShape::Unknown,
                        source_kind: None,
                        connector_id: String::new(),
                        connector_kind: String::new(),
                        source_account_id: String::new(),
                        source_conversation_id: String::new(),
                        source_conversation_summary: String::new(),
                        participant_summary: String::new(),
                        shape_evidence_status: kura_threads::ShapeEvidenceStatus::Proven,
                        recorded_at: None,
                        updated_at: None,
                        retention_expires_at: None,
                        redaction_status: kura_threads::RedactionStatus::Redacted,
                    },
                }
            };
            let mut reset_event = kura_threads::build_scoped_reset_event(&action, &shape);
            reset_event.reset_event_id = format!("reset_{}", action.audit_event_id);
            reset_event.retention_expires_at = Some(retention_expires_at);
            insert_thread_reset_event_tx(&tx, &reset_event)?;
        }
        tx.commit()
            .map_err(|e| format!("commit thread lifecycle action: {e}"))?;
        Ok(Some(ThreadLifecycleMutationResult {
            thread: updated,
            action,
            segment,
        }))
    }

    /// Go `GetThreadDetailForTenant` — the full operator detail view for a
    /// thread. The active-profile projection, continuity previews, and handoff
    /// links are surfaced by store DAOs not yet ported to kura-store, so those
    /// fields are left empty (see the wave-8 surface plan).
    pub fn get_thread_detail_for_tenant(
        &self,
        tenant_id: &str,
        thread_id: &str,
    ) -> Result<Option<kura_threads::ThreadDetailResponse>, String> {
        let Some(thread) = self.get_thread_for_tenant(tenant_id, thread_id)? else {
            return Ok(None);
        };
        let now = Utc::now();
        let segments = self.list_thread_session_segments(tenant_id, thread_id)?;
        let actions = self.list_thread_lifecycle_actions(tenant_id, thread_id, now)?;
        let source_linkages = self.list_thread_source_linkages(tenant_id, thread_id, now)?;
        let runtime_projections =
            self.list_thread_runtime_projections(tenant_id, thread_id, now)?;
        let conversation_shape = self.get_conversation_shape_for_thread(tenant_id, thread_id)?;
        let participation_decisions =
            self.list_participation_decisions_for_thread(tenant_id, thread_id, 20)?;
        let reset_events = self.list_reset_events_for_thread(tenant_id, thread_id, 20)?;

        let current_session_id = segments
            .iter()
            .find(|segment| segment.session_segment_id == thread.current_session_segment_id)
            .map(|segment| segment.session_id.clone())
            .unwrap_or_default();

        let mut response = kura_threads::ThreadDetailResponse {
            thread: kura_threads::build_thread_resource(&thread, &current_session_id),
            session_segments: segments,
            source_linkages,
            runtime_projections,
            lifecycle_actions: actions,
            continuity_previews: Vec::new(),
            participation_decisions,
            reset_events,
            handoff_links: Vec::new(),
            active_profile_projection: None,
            conversation_shape: None,
        };
        if let Some(shape) = conversation_shape {
            response.conversation_shape = Some(shape);
        }
        Ok(Some(response))
    }

    /// Go `threadLifecycleActions` — lifecycle audit rows for a thread, with
    /// unexpired retention, newest first.
    pub fn list_thread_lifecycle_actions(
        &self,
        tenant_id: &str,
        thread_id: &str,
        now: DateTime<Utc>,
    ) -> Result<Vec<kura_threads::LifecycleAction>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT document_json
                FROM thread_lifecycle_events
                WHERE tenant_id = ?1 AND thread_id = ?2
                ORDER BY occurred_at DESC, lifecycle_event_id DESC"#,
            )
            .map_err(|e| format!("list thread lifecycle actions {tenant_id}/{thread_id}: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id, thread_id])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let raw: String = row.get(0).map_err(|e| e.to_string())?;
            let mut action: kura_threads::LifecycleAction = serde_json::from_str(&raw)
                .map_err(|e| format!("decode thread lifecycle action: {e}"))?;
            let expires_at = action.retention_expires_at.unwrap_or_else(|| {
                self.thread_retention_expiry(tenant_id, action.completed_at)
                    .unwrap_or(action.completed_at + chrono::Duration::days(90))
            });
            if expires_at <= now {
                continue;
            }
            action.retention_expires_at = Some(expires_at);
            items.push(action);
        }
        Ok(items)
    }
}

/// Go `upsertThreadSessionSegmentTx` — the transaction variant of
/// `UpsertThreadSessionSegment` (used by lifecycle mutations).
fn upsert_thread_session_segment_tx(
    tx: &rusqlite::Transaction<'_>,
    segment: &kura_threads::SessionSegment,
) -> Result<(), String> {
    let mut segment = segment.clone();
    let now = Utc::now();
    if is_unset_time(&segment.started_at) {
        segment.started_at = now;
    }
    if is_unset_time(&segment.last_active_at) {
        segment.last_active_at = segment.started_at;
    }
    if segment.state.trim().is_empty() {
        segment.state = "active".to_string();
    }
    let document = serde_json::to_string(&segment).map_err(|e| {
        format!(
            "marshal thread session segment {}: {e}",
            segment.session_segment_id
        )
    })?;
    tx.execute(
        r#"INSERT INTO thread_session_segments (
            session_segment_id, thread_id, tenant_id, session_id, generation, state,
            started_at, ended_at, last_active_at, reset_from_session_segment_id,
            partial_evidence, document_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
        ON CONFLICT(session_segment_id) DO UPDATE SET
            thread_id = excluded.thread_id,
            tenant_id = excluded.tenant_id,
            session_id = excluded.session_id,
            generation = excluded.generation,
            state = excluded.state,
            started_at = excluded.started_at,
            ended_at = excluded.ended_at,
            last_active_at = excluded.last_active_at,
            reset_from_session_segment_id = excluded.reset_from_session_segment_id,
            partial_evidence = excluded.partial_evidence,
            document_json = excluded.document_json"#,
        params![
            segment.session_segment_id,
            segment.thread_id,
            segment.tenant_id,
            null_string(&segment.session_id),
            i64::from(segment.generation),
            segment.state,
            now_rfc3339(&segment.started_at),
            opt_time_string(&segment.ended_at),
            now_rfc3339(&segment.last_active_at),
            null_string(&segment.reset_from_session_segment_id),
            i64::from(segment.partial_evidence),
            document,
        ],
    )
    .map_err(|e| {
        format!(
            "upsert thread session segment {}: {e}",
            segment.session_segment_id
        )
    })?;
    Ok(())
}

/// Go `updateThreadLifecycleTx` — optimistic concurrency update of the thread
/// row keyed on the prior state; a concurrent mutation fails the write.
fn update_thread_lifecycle_tx(
    tx: &rusqlite::Transaction<'_>,
    prior: &kura_threads::Thread,
    updated: &kura_threads::Thread,
) -> Result<(), String> {
    let document = serde_json::to_string(updated)
        .map_err(|e| format!("marshal thread {}: {e}", updated.thread_id))?;
    let affected = tx
        .execute(
            r#"UPDATE threads
            SET lifecycle_state = ?1,
                current_session_segment_id = ?2,
                source_kind = ?3,
                source_summary = ?4,
                last_activity_at = ?5,
                updated_at = ?6,
                retention_expires_at = ?7,
                redaction_status = ?8,
                document_json = ?9
            WHERE tenant_id = ?10
              AND thread_id = ?11
              AND lifecycle_state = ?12
              AND current_session_segment_id = ?13
              AND updated_at = ?14"#,
            params![
                enum_str(&updated.lifecycle_state),
                updated.current_session_segment_id,
                enum_str(&updated.source_kind),
                updated.source_summary,
                now_rfc3339(&updated.last_activity_at),
                now_rfc3339(&updated.updated_at),
                opt_time_string(&updated.retention_expires_at),
                enum_str(&updated.redaction_status),
                document,
                prior.tenant_id,
                prior.thread_id,
                enum_str(&prior.lifecycle_state),
                prior.current_session_segment_id,
                now_rfc3339(&prior.updated_at),
            ],
        )
        .map_err(|e| format!("update thread lifecycle {}: {e}", updated.thread_id))?;
    if affected != 1 {
        return Err(kura_threads::ThreadsError::LifecycleMutationConflict.to_string());
    }
    Ok(())
}

/// Go `insertThreadLifecycleActionTx`.
fn insert_thread_lifecycle_action_tx(
    tx: &rusqlite::Transaction<'_>,
    action: &kura_threads::LifecycleAction,
) -> Result<(), String> {
    let document = serde_json::to_string(action).map_err(|e| {
        format!(
            "marshal thread lifecycle action {}: {e}",
            action.lifecycle_action_id
        )
    })?;
    tx.execute(
        r#"INSERT INTO thread_lifecycle_events (
            lifecycle_event_id, thread_id, tenant_id, action, outcome, audit_event_id,
            occurred_at, redaction_status, document_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"#,
        params![
            action.lifecycle_action_id,
            action.thread_id,
            action.tenant_id,
            enum_str(&action.action_kind),
            action.status,
            action.audit_event_id,
            now_rfc3339(&action.completed_at),
            enum_str(&action.redaction_status),
            document,
        ],
    )
    .map_err(|e| {
        format!(
            "insert thread lifecycle action {}: {e}",
            action.lifecycle_action_id
        )
    })?;
    Ok(())
}

/// Go `insertThreadResetEventTx`.
fn insert_thread_reset_event_tx(
    tx: &rusqlite::Transaction<'_>,
    event: &kura_threads::ResetEvent,
) -> Result<(), String> {
    let mut event = event.clone();
    if event.reset_event_id.trim().is_empty() {
        event.reset_event_id = new_store_id("reset");
    }
    if event.permission_gate.trim().is_empty() {
        event.permission_gate = "connectors.manage".to_string();
    }
    if event.reason_code.trim().is_empty() {
        event.reason_code = kura_threads::GROUP_ROOM_REASON_SCOPED_RESET_SUCCEEDED.to_string();
    }
    let document = serde_json::to_string(&event)
        .map_err(|e| format!("marshal reset event {}: {e}", event.reset_event_id))?;
    tx.execute(
        r#"INSERT INTO thread_reset_events (
            reset_event_id, tenant_id, thread_id, conversation_shape, source_conversation_id,
            actor_principal_id, permission_gate, prior_session_segment_id, resulting_session_segment_id,
            status, reason_code, requested_at, completed_at, audit_event_id,
            retention_expires_at, redaction_status, document_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
        ON CONFLICT(reset_event_id) DO UPDATE SET
            conversation_shape = excluded.conversation_shape,
            source_conversation_id = excluded.source_conversation_id,
            status = excluded.status,
            reason_code = excluded.reason_code,
            completed_at = excluded.completed_at,
            retention_expires_at = excluded.retention_expires_at,
            redaction_status = excluded.redaction_status,
            document_json = excluded.document_json"#,
        params![
            event.reset_event_id,
            event.tenant_id,
            event.thread_id,
            enum_str(&event.conversation_shape),
            null_string(&event.source_conversation_id),
            null_string(&event.actor_principal_id),
            event.permission_gate,
            null_string(&event.prior_session_segment_id),
            null_string(&event.resulting_session_segment_id),
            enum_str(&event.status),
            event.reason_code,
            now_rfc3339(&event.requested_at.unwrap_or_default()),
            now_rfc3339(&event.completed_at.unwrap_or_default()),
            null_string(&event.audit_event_id),
            opt_time_string(&event.retention_expires_at),
            enum_str(&event.redaction_status),
            document,
        ],
    )
    .map_err(|e| format!("insert reset event {}: {e}", event.reset_event_id))?;
    Ok(())
}

/// Go `nextThreadSegmentGenerationTx`.
fn next_thread_segment_generation_tx(
    tx: &rusqlite::Transaction<'_>,
    tenant_id: &str,
    thread_id: &str,
) -> Result<i32, String> {
    let max: Option<i64> = tx
        .query_row(
            r#"SELECT MAX(generation)
            FROM thread_session_segments
            WHERE tenant_id = ?1 AND thread_id = ?2"#,
            params![tenant_id, thread_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("next thread segment generation {tenant_id}/{thread_id}: {e}"))?;
    Ok(match max {
        Some(value) => value as i32 + 1,
        None => 1,
    })
}

/// Go `threadReopenEligibleTx`.
fn thread_reopen_eligible_tx(
    tx: &rusqlite::Transaction<'_>,
    thread: &kura_threads::Thread,
) -> Result<bool, String> {
    if thread.current_session_segment_id.trim().is_empty() {
        return Ok(false);
    }
    let segment_count: i64 = tx
        .query_row(
            r#"SELECT COUNT(*)
            FROM thread_session_segments
            WHERE tenant_id = ?1 AND thread_id = ?2 AND session_segment_id = ?3"#,
            params![
                thread.tenant_id,
                thread.thread_id,
                thread.current_session_segment_id
            ],
            |row| row.get(0),
        )
        .map_err(|e| format!("check reopen session eligibility {}: {e}", thread.thread_id))?;
    if segment_count != 1 {
        return Ok(false);
    }
    if thread.source_kind != kura_threads::SourceKind::Channel {
        return Ok(true);
    }
    let source_count: i64 = tx
        .query_row(
            r#"SELECT COUNT(*)
            FROM thread_source_links
            WHERE tenant_id = ?1
              AND thread_id = ?2
              AND current_flag = 1
              AND COALESCE(connector_id, '') != ''
              AND COALESCE(source_account_id, '') != ''
              AND COALESCE(source_conversation_id, '') != ''"#,
            params![thread.tenant_id, thread.thread_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("check reopen source eligibility {}: {e}", thread.thread_id))?;
    Ok(source_count == 1)
}
