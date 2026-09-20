//! SQLite CRUD for thread continuity turns and previews. Ported from
//! `daemon/internal/store/thread_continuity.go` (SaveContinuityTurn,
//! ListContinuityTurns, ListContinuityTurnsOutsideSessionSegment,
//! SaveContinuityPreview, ListContinuityPreviewSummaries,
//! GetContinuityPreviewDetail). Turns persist as `document_json` plus the
//! denormalized columns; acceptance sequences are allocated transactionally
//! and source-event-key duplicates resolve to the existing turn.

use chrono::{DateTime, Utc};
use rusqlite::{Transaction, params};

use crate::SQLiteStore;
use crate::crud::{enum_str, null_string};

fn new_store_id(prefix: &str) -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!("{prefix}_{}", &hex[..16])
}

fn is_unset_time(dt: &DateTime<Utc>) -> bool {
    dt.timestamp() == 0 && dt.timestamp_subsec_nanos() == 0
}

fn is_unique_constraint_error(err: &str) -> bool {
    err.to_uppercase().contains("UNIQUE")
}

fn scan_continuity_turn(raw: &str) -> Result<kura_threads::ContinuityTurn, String> {
    serde_json::from_str(raw).map_err(|e| format!("decode continuity turn document: {e}"))
}

fn scan_continuity_preview(raw: &str) -> Result<kura_threads::ContinuityPreview, String> {
    serde_json::from_str(raw).map_err(|e| format!("decode continuity preview document: {e}"))
}

fn scan_continuity_preview_item(raw: &str) -> Result<kura_threads::ContinuityPreviewItem, String> {
    serde_json::from_str(raw).map_err(|e| format!("decode continuity preview item document: {e}"))
}

/// Go `ContinuityLookupQuery`.
#[derive(Debug, Clone, Default)]
pub struct ContinuityLookupQuery {
    pub tenant_id: String,
    pub thread_id: String,
    pub session_segment_id: String,
    pub limit: i64,
    pub now: Option<DateTime<Utc>>,
}

impl SQLiteStore {
    /// Go `SaveContinuityTurn` with the retry loop for unique/busy errors.
    pub fn save_continuity_turn(
        &self,
        turn: &kura_threads::ContinuityTurn,
    ) -> Result<kura_threads::ContinuityTurn, String> {
        let mut last_err: Option<String> = None;
        for attempt in 0..5 {
            match self.save_continuity_turn_once(turn) {
                Ok(saved) => return Ok(saved),
                Err(err) => {
                    if !err_retryable(&err) {
                        return Err(err);
                    }
                    last_err = Some(err);
                    std::thread::sleep(std::time::Duration::from_millis((attempt + 1) * 5));
                }
            }
        }
        if let Some(err) = last_err {
            return Err(err);
        }
        self.save_continuity_turn_once(turn)
    }

    fn save_continuity_turn_once(
        &self,
        turn: &kura_threads::ContinuityTurn,
    ) -> Result<kura_threads::ContinuityTurn, String> {
        let mut turn = turn.clone();
        if is_unset_time(&turn.recorded_at) {
            turn.recorded_at = Utc::now();
        }
        if turn.retention_expires_at.is_none()
            || turn.retention_expires_at.is_some_and(|t| is_unset_time(&t))
        {
            turn.retention_expires_at =
                Some(self.thread_retention_expiry(&turn.tenant_id, turn.recorded_at)?);
        }
        if turn.continuity_turn_id.is_empty() {
            turn.continuity_turn_id = new_store_id("turn");
        }
        kura_threads::validate_continuity_turn(&turn)
            .map_err(|e| format!("validate continuity turn: {e}"))?;
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin continuity turn: {e}"))?;
        if turn.acceptance_sequence == 0 {
            let next: i64 = tx
                .query_row(
                    "SELECT COALESCE(MAX(acceptance_sequence), 0) + 1
                     FROM thread_continuity_turns
                     WHERE tenant_id = ?1 AND thread_id = ?2",
                    params![turn.tenant_id, turn.thread_id],
                    |row| row.get(0),
                )
                .map_err(|e| format!("allocate continuity sequence: {e}"))?;
            turn.acceptance_sequence = next;
        }
        if let Err(err) = insert_continuity_turn_tx(&tx, &turn) {
            if is_unique_constraint_error(&err) && !turn.source_event_key.trim().is_empty() {
                let existing = self.get_continuity_turn_by_source_event_key(
                    &turn.tenant_id,
                    &turn.source_event_key,
                )?;
                if let Some(existing) = existing {
                    return Ok(existing);
                }
                return Err(format!(
                    "insert continuity turn {}: {err}",
                    turn.continuity_turn_id
                ));
            }
            return Err(format!(
                "insert continuity turn {}: {err}",
                turn.continuity_turn_id
            ));
        }
        tx.commit()
            .map_err(|e| format!("commit continuity turn: {e}"))?;
        Ok(turn)
    }

    /// Go `ListContinuityTurns`: current-session turns with full evidence.
    pub fn list_continuity_turns(
        &self,
        query: &ContinuityLookupQuery,
    ) -> Result<Vec<kura_threads::ContinuityTurn>, String> {
        let now = query.now.unwrap_or_else(Utc::now);
        let limit = if query.limit <= 0 {
            kura_threads::DEFAULT_CONTINUITY_MAX_PRIOR_TURNS as i64 + 64
        } else {
            query.limit
        };
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT c.document_json
                FROM thread_continuity_turns AS c
                JOIN thread_session_segments AS s
                  ON s.tenant_id = c.tenant_id
                 AND s.thread_id = c.thread_id
                 AND s.session_segment_id = c.session_segment_id
                WHERE c.tenant_id = ?1
                  AND c.thread_id = ?2
                  AND c.session_segment_id = ?3
                  AND c.retention_expires_at >= ?4
                  AND s.partial_evidence = 0
                ORDER BY c.acceptance_sequence DESC
                LIMIT ?5"#,
            )
            .map_err(|e| format!("list continuity turns: {e}"))?;
        let mut rows = stmt
            .query(params![
                query.tenant_id,
                query.thread_id,
                query.session_segment_id,
                fmt_time(now),
                limit
            ])
            .map_err(|e| e.to_string())?;
        let mut turns = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let raw: String = row.get(0).map_err(|e| e.to_string())?;
            turns.push(scan_continuity_turn(&raw)?);
        }
        Ok(turns)
    }

    /// Go `ListContinuityTurnsOutsideSessionSegment`: reset-boundary turns.
    pub fn list_continuity_turns_outside_session_segment(
        &self,
        query: &ContinuityLookupQuery,
    ) -> Result<Vec<kura_threads::ContinuityTurn>, String> {
        let now = query.now.unwrap_or_else(Utc::now);
        let limit = if query.limit <= 0 {
            kura_threads::DEFAULT_CONTINUITY_MAX_PRIOR_TURNS as i64 + 64
        } else {
            query.limit
        };
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT document_json
                FROM thread_continuity_turns
                WHERE tenant_id = ?1
                  AND thread_id = ?2
                  AND session_segment_id <> ?3
                  AND retention_expires_at >= ?4
                ORDER BY acceptance_sequence DESC
                LIMIT ?5"#,
            )
            .map_err(|e| format!("list reset-boundary continuity turns: {e}"))?;
        let mut rows = stmt
            .query(params![
                query.tenant_id,
                query.thread_id,
                query.session_segment_id,
                fmt_time(now),
                limit
            ])
            .map_err(|e| e.to_string())?;
        let mut turns = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let raw: String = row.get(0).map_err(|e| e.to_string())?;
            turns.push(scan_continuity_turn(&raw)?);
        }
        Ok(turns)
    }

    /// Go `SaveContinuityPreview`: persists the preview plus its items in one
    /// transaction, filling Go's defaults.
    pub fn save_continuity_preview(
        &self,
        mut preview: kura_threads::ContinuityPreview,
        items: &mut [kura_threads::ContinuityPreviewItem],
    ) -> Result<kura_threads::ContinuityPreview, String> {
        let now = Utc::now();
        if preview.continuity_preview_id.is_empty() {
            preview.continuity_preview_id = new_store_id("contprev");
        }
        if preview.window_policy_id.is_empty() {
            let policy = kura_threads::default_continuity_policy();
            preview.window_policy_id = policy.window_policy_id;
            preview.max_prior_turns = policy.max_prior_turns;
            preview.active_window_days = policy.active_window_days;
        }
        if preview.max_prior_turns == 0 {
            preview.max_prior_turns = kura_threads::DEFAULT_CONTINUITY_MAX_PRIOR_TURNS;
        }
        if preview.active_window_days == 0 {
            preview.active_window_days = kura_threads::DEFAULT_CONTINUITY_ACTIVE_DAYS;
        }
        if is_unset_time(&preview.assembly_started_at) {
            preview.assembly_started_at = now;
        }
        if is_unset_time(&preview.assembly_completed_at) {
            preview.assembly_completed_at = now;
        }
        if preview.assembly_duration_ms == 0 {
            preview.assembly_duration_ms =
                (preview.assembly_completed_at - preview.assembly_started_at).num_milliseconds();
        }
        if is_unset_time(&preview.retention_expires_at) {
            preview.retention_expires_at =
                self.thread_retention_expiry(&preview.tenant_id, preview.assembly_completed_at)?;
        }
        // Go's empty-status default (Applied/Empty) is unrepresentable in the
        // closed Rust enum; callers construct the preview with an explicit status.
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin continuity preview: {e}"))?;
        insert_continuity_preview_tx(&tx, &preview)?;
        for (index, item) in items.iter_mut().enumerate() {
            if item.preview_item_id.is_empty() {
                item.preview_item_id = new_store_id("contitem");
            }
            item.continuity_preview_id = preview.continuity_preview_id.clone();
            item.tenant_id = preview.tenant_id.clone();
            item.thread_id = preview.thread_id.clone();
            if item.item_order == 0 {
                item.item_order = index as i32;
            }
            insert_continuity_preview_item_tx(&tx, item)?;
        }
        tx.commit()
            .map_err(|e| format!("commit continuity preview: {e}"))?;
        Ok(preview)
    }

    /// Go `ListContinuityPreviewSummaries`.
    pub fn list_continuity_preview_summaries(
        &self,
        tenant_id: &str,
        thread_id: &str,
        limit: i64,
    ) -> Result<Vec<kura_threads::ContinuityPreview>, String> {
        let limit = if limit <= 0 { 10 } else { limit };
        let mut stmt = self
            .conn
            .prepare(
                "SELECT document_json FROM thread_continuity_previews
                 WHERE tenant_id = ?1 AND thread_id = ?2 AND retention_expires_at >= ?3
                 ORDER BY assembly_completed_at DESC, continuity_preview_id DESC LIMIT ?4",
            )
            .map_err(|e| format!("list continuity previews: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id, thread_id, fmt_time(Utc::now()), limit])
            .map_err(|e| e.to_string())?;
        let mut previews = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let raw: String = row.get(0).map_err(|e| e.to_string())?;
            previews.push(scan_continuity_preview(&raw)?);
        }
        Ok(previews)
    }

    /// Go `GetContinuityPreviewDetail`: preview plus its ordered items.
    pub fn get_continuity_preview_detail(
        &self,
        tenant_id: &str,
        thread_id: &str,
        preview_id: &str,
    ) -> Result<Option<kura_threads::ContinuityPreviewDetail>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT document_json FROM thread_continuity_previews
                 WHERE tenant_id = ?1 AND thread_id = ?2 AND continuity_preview_id = ?3
                   AND retention_expires_at >= ?4",
            )
            .map_err(|e| format!("get continuity preview {preview_id}: {e}"))?;
        let mut rows = stmt
            .query(params![
                tenant_id,
                thread_id,
                preview_id,
                fmt_time(Utc::now())
            ])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let raw: String = row.get(0).map_err(|e| e.to_string())?;
        let preview = scan_continuity_preview(&raw)?;
        let mut stmt = self
            .conn
            .prepare(
                "SELECT document_json FROM thread_continuity_preview_items
                 WHERE continuity_preview_id = ?1 AND tenant_id = ?2 AND thread_id = ?3
                 ORDER BY item_order ASC, preview_item_id ASC",
            )
            .map_err(|e| format!("list continuity preview items: {e}"))?;
        let mut rows = stmt
            .query(params![preview_id, tenant_id, thread_id])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let item_raw: String = row.get(0).map_err(|e| e.to_string())?;
            items.push(scan_continuity_preview_item(&item_raw)?);
        }
        Ok(Some(kura_threads::ContinuityPreviewDetail {
            preview,
            items,
        }))
    }

    pub fn get_continuity_turn_by_source_event_key(
        &self,
        tenant_id: &str,
        source_event_key: &str,
    ) -> Result<Option<kura_threads::ContinuityTurn>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT document_json FROM thread_continuity_turns
                 WHERE tenant_id = ?1 AND source_event_key = ?2",
            )
            .map_err(|e| format!("get continuity turn by source event key: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id, source_event_key])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let raw: String = row.get(0).map_err(|e| e.to_string())?;
        Ok(Some(scan_continuity_turn(&raw)?))
    }
}

fn err_retryable(err: &str) -> bool {
    let lower = err.to_lowercase();
    lower.contains("unique constraint")
        || lower.contains("database is locked")
        || lower.contains("busy")
}

fn fmt_time(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
}

fn insert_continuity_turn_tx(
    tx: &Transaction,
    turn: &kura_threads::ContinuityTurn,
) -> Result<(), String> {
    let document_json =
        serde_json::to_string(turn).map_err(|e| format!("marshal continuity turn: {e}"))?;
    let source_timestamp = turn
        .source_timestamp
        .filter(|t| !is_unset_time(t))
        .map(|t| fmt_time(t));
    tx.execute(
        r#"INSERT INTO thread_continuity_turns (
            continuity_turn_id, tenant_id, thread_id, session_segment_id, acceptance_sequence,
            role, source_kind, source_linkage_id, source_message_id, source_timestamp,
            dispatch_id, response_to_turn_id, safe_content, content_redaction_status,
            recorded_at, retention_expires_at, source_event_key, document_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)"#,
        params![
            turn.continuity_turn_id,
            turn.tenant_id,
            turn.thread_id,
            turn.session_segment_id,
            turn.acceptance_sequence,
            enum_str(&turn.role),
            enum_str(&turn.source_kind),
            null_string(&turn.source_linkage_id),
            null_string(&turn.source_message_id),
            source_timestamp,
            null_string(&turn.dispatch_id),
            null_string(&turn.response_to_turn_id),
            turn.safe_content,
            enum_str(&turn.content_redaction_status),
            fmt_time(turn.recorded_at),
            turn.retention_expires_at.map(fmt_time),
            null_string(&turn.source_event_key),
            document_json,
        ],
    )
    .map_err(|e| format!("insert continuity turn {}: {e}", turn.continuity_turn_id))?;
    Ok(())
}

fn insert_continuity_preview_tx(
    tx: &Transaction,
    preview: &kura_threads::ContinuityPreview,
) -> Result<(), String> {
    let document_json =
        serde_json::to_string(preview).map_err(|e| format!("marshal continuity preview: {e}"))?;
    tx.execute(
        r#"INSERT INTO thread_continuity_previews (
            continuity_preview_id, tenant_id, thread_id, session_segment_id, dispatch_id,
            request_turn_id, response_turn_id, window_policy_id, max_prior_turns,
            active_window_days, included_count, excluded_count, continuity_applied,
            status, failure_class, assembly_started_at, assembly_completed_at,
            assembly_duration_ms, retention_expires_at, redaction_status, document_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)"#,
        params![
            preview.continuity_preview_id,
            preview.tenant_id,
            preview.thread_id,
            preview.session_segment_id,
            null_string(&preview.dispatch_id),
            null_string(&preview.request_turn_id),
            null_string(&preview.response_turn_id),
            preview.window_policy_id,
            preview.max_prior_turns,
            preview.active_window_days,
            preview.included_count,
            preview.excluded_count,
            bool_to_int(preview.continuity_applied),
            enum_str(&preview.status),
            null_string(&preview.failure_class),
            fmt_time(preview.assembly_started_at),
            fmt_time(preview.assembly_completed_at),
            preview.assembly_duration_ms,
            fmt_time(preview.retention_expires_at),
            enum_str(&preview.redaction_status),
            document_json,
        ],
    )
    .map_err(|e| format!("insert continuity preview {}: {e}", preview.continuity_preview_id))?;
    Ok(())
}

fn insert_continuity_preview_item_tx(
    tx: &Transaction,
    item: &kura_threads::ContinuityPreviewItem,
) -> Result<(), String> {
    let document_json =
        serde_json::to_string(item).map_err(|e| format!("marshal continuity preview item: {e}"))?;
    let source_timestamp = item
        .source_timestamp
        .filter(|t| !is_unset_time(t))
        .map(|t| fmt_time(t));
    let sequence: Option<i64> = if item.acceptance_sequence > 0 {
        Some(item.acceptance_sequence)
    } else {
        None
    };
    tx.execute(
        r#"INSERT INTO thread_continuity_preview_items (
            preview_item_id, continuity_preview_id, tenant_id, thread_id, item_kind,
            continuity_turn_id, artifact_ref, artifact_excerpt_id, decision, reason_code,
            acceptance_sequence, source_timestamp, safe_summary, redaction_status,
            item_order, document_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)"#,
        params![
            item.preview_item_id,
            item.continuity_preview_id,
            item.tenant_id,
            item.thread_id,
            enum_str(&item.item_kind),
            null_string(&item.continuity_turn_id),
            null_string(&item.artifact_ref),
            null_string(&item.artifact_excerpt_id),
            enum_str(&item.decision),
            enum_str(&item.reason_code),
            sequence,
            source_timestamp,
            null_string(&item.safe_summary),
            enum_str(&item.redaction_status),
            item.item_order,
            document_json,
        ],
    )
    .map_err(|e| {
        format!(
            "insert continuity preview item {}: {e}",
            item.preview_item_id
        )
    })?;
    Ok(())
}

fn bool_to_int(value: bool) -> i64 {
    if value { 1 } else { 0 }
}
