//! SQLite CRUD for thread handoff links and source references. Ported from
//! `daemon/internal/store/thread_handoff.go` (SaveHandoffLink,
//! SaveHandoffSourceReferences, ListHandoffLinksForThread, GetHandoffLink,
//! ListHandoffSourceReferencesForLink, MarkHandoffSourceReferencesConsumed).
//! Rows persist as `document_json` plus the denormalized tenant columns; the
//! retention expiry defaults through the tenant thread-retention policy.

use chrono::{DateTime, Utc};
use rusqlite::params;

use crate::SQLiteStore;
use crate::crud::{enum_str, null_string};

fn new_store_id(prefix: &str) -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!("{prefix}_{}", &hex[..16])
}

fn is_unset_time(dt: &DateTime<Utc>) -> bool {
    dt.timestamp() == 0 && dt.timestamp_subsec_nanos() == 0
}

fn nullable_time(value: &Option<DateTime<Utc>>) -> Option<String> {
    value
        .as_ref()
        .filter(|t| !is_unset_time(t))
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true))
}

fn scan_handoff_link(raw: &str) -> Result<kura_threads::HandoffLink, String> {
    serde_json::from_str(raw).map_err(|e| format!("decode handoff link document: {e}"))
}

fn scan_handoff_source_reference(
    raw: &str,
) -> Result<kura_threads::HandoffSourceReference, String> {
    serde_json::from_str(raw).map_err(|e| format!("decode handoff source reference document: {e}"))
}

impl SQLiteStore {
    /// Go `SaveHandoffLink` (upsert on handoff_link_id).
    pub fn save_handoff_link(
        &self,
        mut link: kura_threads::HandoffLink,
    ) -> Result<kura_threads::HandoffLink, String> {
        if link.handoff_link_id.is_empty() {
            link.handoff_link_id = new_store_id("handoff");
        }
        if link.permission_gate.is_empty() {
            link.permission_gate = "connectors.manage".to_string();
        }
        if link.created_at.is_none() || link.created_at.is_some_and(|t| is_unset_time(&t)) {
            link.created_at = Some(Utc::now());
        }
        if link.retention_expires_at.is_none()
            || link.retention_expires_at.is_some_and(|t| is_unset_time(&t))
        {
            let created_at = link.created_at.unwrap_or_else(Utc::now);
            link.retention_expires_at =
                Some(self.thread_retention_expiry(&link.tenant_id, created_at)?);
        }
        let document_json = serde_json::to_string(&link)
            .map_err(|e| format!("marshal handoff link {}: {e}", link.handoff_link_id))?;
        self.conn
            .execute(
                r#"INSERT INTO thread_handoff_links (
                    handoff_link_id, tenant_id, source_thread_id, source_session_segment_id,
                    destination_thread_id, destination_session_segment_id,
                    source_conversation_shape, destination_conversation_shape, source_kind,
                    destination_kind, source_connector_id, destination_connector_id,
                    source_conversation_id, destination_conversation_id, actor_principal_id,
                    permission_gate, status, reason_code, first_destination_response_id,
                    source_reference_status, created_at, consumed_at, retention_expires_at,
                    redaction_status, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)
                ON CONFLICT(handoff_link_id) DO UPDATE SET
                    status = excluded.status,
                    reason_code = excluded.reason_code,
                    first_destination_response_id = excluded.first_destination_response_id,
                    source_reference_status = excluded.source_reference_status,
                    consumed_at = excluded.consumed_at,
                    redaction_status = excluded.redaction_status,
                    document_json = excluded.document_json"#,
                params![
                    link.handoff_link_id,
                    link.tenant_id,
                    link.source_thread_id,
                    null_string(&link.source_session_segment_id),
                    link.destination_thread_id,
                    null_string(&link.destination_session_segment_id),
                    enum_str(&link.source_conversation_shape),
                    enum_str(&link.destination_conversation_shape),
                    link.source_kind.as_ref().map(enum_str),
                    link.destination_kind.as_ref().map(enum_str),
                    null_string(&link.source_connector_id),
                    null_string(&link.destination_connector_id),
                    null_string(&link.source_conversation_id),
                    null_string(&link.destination_conversation_id),
                    null_string(&link.actor_principal_id),
                    link.permission_gate,
                    enum_str(&link.status),
                    null_string(&link.reason_code),
                    null_string(&link.first_destination_response_id),
                    enum_str(&link.source_reference_status),
                    link.created_at.map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)),
                    nullable_time(&link.consumed_at),
                    link.retention_expires_at.map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)),
                    enum_str(&link.redaction_status),
                    document_json,
                ],
            )
            .map_err(|e| format!("save handoff link {}: {e}", link.handoff_link_id))?;
        Ok(link)
    }

    /// Go `SaveHandoffSourceReferences`: batch insert of source references.
    pub fn save_handoff_source_references(
        &self,
        refs: &mut [kura_threads::HandoffSourceReference],
    ) -> Result<(), String> {
        for index in 0..refs.len() {
            let mut reference = refs[index].clone();
            if reference.handoff_source_reference_id.is_empty() {
                reference.handoff_source_reference_id = new_store_id("href");
            }
            if reference.created_at.is_none()
                || reference.created_at.is_some_and(|t| is_unset_time(&t))
            {
                reference.created_at = Some(Utc::now());
            }
            if reference.retention_expires_at.is_none()
                || reference
                    .retention_expires_at
                    .is_some_and(|t| is_unset_time(&t))
            {
                let created_at = reference.created_at.unwrap_or_else(Utc::now);
                reference.retention_expires_at =
                    Some(self.thread_retention_expiry(&reference.tenant_id, created_at)?);
            }
            let document_json = serde_json::to_string(&reference).map_err(|e| {
                format!(
                    "marshal handoff source reference {}: {e}",
                    reference.handoff_source_reference_id
                )
            })?;
            self.conn
                .execute(
                    r#"INSERT INTO thread_handoff_source_references (
                        handoff_source_reference_id, handoff_link_id, tenant_id, source_thread_id,
                        source_session_segment_id, destination_thread_id,
                        destination_session_segment_id, continuity_turn_id, artifact_excerpt_ref,
                        eligibility_status, decision, safe_summary, redaction_status, created_at,
                        consumed_at, retention_expires_at, document_json
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)"#,
                    params![
                        reference.handoff_source_reference_id,
                        reference.handoff_link_id,
                        reference.tenant_id,
                        reference.source_thread_id,
                        null_string(&reference.source_session_segment_id),
                        reference.destination_thread_id,
                        null_string(&reference.destination_session_segment_id),
                        null_string(&reference.continuity_turn_id),
                        null_string(&reference.artifact_excerpt_ref),
                        enum_str(&reference.eligibility_status),
                        enum_str(&reference.decision),
                        null_string(&reference.safe_summary),
                        enum_str(&reference.redaction_status),
                        reference.created_at.map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)),
                        nullable_time(&reference.consumed_at),
                        reference.retention_expires_at.map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)),
                        document_json,
                    ],
                )
                .map_err(|e| format!("save handoff source reference {}: {e}", reference.handoff_source_reference_id))?;
            refs[index] = reference;
        }
        Ok(())
    }

    /// Go `ListHandoffLinksForThread`: links where the thread is source or destination.
    pub fn list_handoff_links(
        &self,
        tenant_id: &str,
        thread_id: &str,
        limit: i64,
    ) -> Result<Vec<kura_threads::HandoffLink>, String> {
        let limit = if limit <= 0 { 20 } else { limit };
        let mut stmt = self
            .conn
            .prepare(
                "SELECT document_json FROM thread_handoff_links
                 WHERE tenant_id = ?1 AND (source_thread_id = ?2 OR destination_thread_id = ?2)
                 ORDER BY created_at DESC, handoff_link_id DESC LIMIT ?3",
            )
            .map_err(|e| format!("list handoff links {tenant_id}/{thread_id}: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id, thread_id, limit])
            .map_err(|e| e.to_string())?;
        let mut links = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let raw: String = row.get(0).map_err(|e| e.to_string())?;
            links.push(scan_handoff_link(&raw)?);
        }
        Ok(links)
    }

    /// Go `GetHandoffLink`.
    pub fn get_handoff_link(
        &self,
        tenant_id: &str,
        handoff_link_id: &str,
    ) -> Result<Option<kura_threads::HandoffLink>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT document_json FROM thread_handoff_links WHERE tenant_id = ?1 AND handoff_link_id = ?2")
            .map_err(|e| format!("get handoff link {handoff_link_id}: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id, handoff_link_id])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let raw: String = row.get(0).map_err(|e| e.to_string())?;
        Ok(Some(scan_handoff_link(&raw)?))
    }

    /// Go `ListHandoffSourceReferencesForLink`.
    pub fn list_handoff_source_references_for_link(
        &self,
        tenant_id: &str,
        handoff_link_id: &str,
    ) -> Result<Vec<kura_threads::HandoffSourceReference>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT document_json FROM thread_handoff_source_references
                 WHERE tenant_id = ?1 AND handoff_link_id = ?2
                 ORDER BY created_at ASC, handoff_source_reference_id ASC",
            )
            .map_err(|e| {
                format!("list handoff source references {tenant_id}/{handoff_link_id}: {e}")
            })?;
        let mut rows = stmt
            .query(params![tenant_id, handoff_link_id])
            .map_err(|e| e.to_string())?;
        let mut refs = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let raw: String = row.get(0).map_err(|e| e.to_string())?;
            refs.push(scan_handoff_source_reference(&raw)?);
        }
        Ok(refs)
    }

    /// Go `MarkHandoffSourceReferencesConsumed`: marks the link consumed and
    /// flips `Referenced` decisions to `Consumed`.
    pub fn mark_handoff_source_references_consumed(
        &self,
        tenant_id: &str,
        handoff_link_id: &str,
        first_destination_response_id: &str,
        consumed_at: Option<DateTime<Utc>>,
    ) -> Result<(), String> {
        let consumed_at = consumed_at.unwrap_or_else(Utc::now);
        let mut link = self
            .get_handoff_link(tenant_id, handoff_link_id)?
            .ok_or_else(|| format!("handoff link {handoff_link_id} not found"))?;
        link.source_reference_status = kura_threads::HandoffSourceReferenceStatus::Consumed;
        link.first_destination_response_id = first_destination_response_id.to_string();
        link.consumed_at = Some(consumed_at);
        self.save_handoff_link(link)?;

        let refs = self.list_handoff_source_references_for_link(tenant_id, handoff_link_id)?;
        for mut reference in refs {
            reference.consumed_at = Some(consumed_at);
            if reference.decision == kura_threads::HandoffSourceReferenceDecision::Referenced {
                reference.decision = kura_threads::HandoffSourceReferenceDecision::Consumed;
            }
            let document_json = serde_json::to_string(&reference).map_err(|e| {
                format!(
                    "marshal handoff source reference {}: {e}",
                    reference.handoff_source_reference_id
                )
            })?;
            self.conn
                .execute(
                    "UPDATE thread_handoff_source_references
                     SET decision = ?1, consumed_at = ?2, document_json = ?3
                     WHERE tenant_id = ?4 AND handoff_source_reference_id = ?5",
                    params![
                        enum_str(&reference.decision),
                        nullable_time(&reference.consumed_at),
                        document_json,
                        tenant_id,
                        reference.handoff_source_reference_id,
                    ],
                )
                .map_err(|e| {
                    format!(
                        "consume handoff source reference {}: {e}",
                        reference.handoff_source_reference_id
                    )
                })?;
        }
        Ok(())
    }
}
