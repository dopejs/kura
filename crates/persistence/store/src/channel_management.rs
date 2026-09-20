//! SQLite CRUD for the channel-management ledger: route policies (plus their
//! historical snapshots), routing decisions, foreground reply outcomes,
//! background delivery outcomes, and support-evidence bundles. Ported from
//! `daemon/internal/store/channel_management.go` (SaveChannelRoutePolicy,
//! GetChannelRoutePolicy, SaveChannelRoutingDecision, ListChannelRoutingDecisions,
//! SaveChannelForegroundReplyOutcome, ListChannelForegroundReplyOutcomes,
//! SaveChannelBackgroundDeliveryOutcome, ListChannelBackgroundDeliveryOutcomes,
//! SaveChannelSupportEvidence, GetLatestChannelSupportEvidence,
//! ListExpiredChannelSupportEvidence, SaveChannelConnectorEnablementState,
//! GetChannelConnectorEnablementState, SaveChannelRepairAction,
//! ListChannelRepairActions, SaveChannelManagementAuditRecord,
//! ListChannelManagementAuditRecords).
//!
//! The record types and their pure predicates live in `kura-connectors`
//! (management.rs), matching the Go layout where the connectors package owns
//! them and the store only persists them. This module re-exports them so
//! existing `kura_store::channel_management::*` imports keep resolving.

use chrono::{DateTime, Duration, Utc};
use rusqlite::{Row, params};

use crate::SQLiteStore;
use crate::crud::now_rfc3339;

/// The channel-management record types, enums, and pure predicates, defined in
/// `kura-connectors` (Go keeps them in the connectors package) and re-exported
/// here for the store DAOs below and for callers importing them from kura-store.
pub use kura_connectors::{
    BackgroundDeliveryOutcome, ConnectorAuditRecord, EnablementState, ForegroundReplyOutcome,
    ManagementState, RepairAction, RouteDecisionOutcome, RoutePolicy, RoutingDecision,
    SupportEvidenceBundle, contains_route_policy_value, default_route_policy,
    normalize_route_policy, route_policy_allows_conversation, route_policy_allows_sender,
    route_policy_is_valid,
};

fn new_store_id(prefix: &str) -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!("{prefix}_{}", &hex[..16])
}

/// A chrono-defaulted timestamp (UNIX epoch) stands in for Go's zero `time.Time`.
fn is_unset_time(dt: &DateTime<Utc>) -> bool {
    dt.timestamp() == 0 && dt.timestamp_subsec_nanos() == 0
}

/// Go `newStoreID` default retention horizon for channel outcome records.
fn default_retention(occurred_at: DateTime<Utc>) -> DateTime<Utc> {
    occurred_at + Duration::days(90)
}

fn scan_channel_route_policy(row: &Row) -> Result<RoutePolicy, String> {
    let raw: String = row.get(0).map_err(|e| e.to_string())?;
    serde_json::from_str(&raw).map_err(|e| format!("decode channel route policy: {e}"))
}

fn scan_channel_routing_decision(row: &Row) -> Result<RoutingDecision, String> {
    let raw: String = row.get(0).map_err(|e| e.to_string())?;
    serde_json::from_str(&raw).map_err(|e| format!("decode channel routing decision: {e}"))
}

fn scan_channel_reply_outcome(row: &Row) -> Result<ForegroundReplyOutcome, String> {
    let raw: String = row.get(0).map_err(|e| e.to_string())?;
    serde_json::from_str(&raw).map_err(|e| format!("decode channel reply outcome: {e}"))
}

fn scan_channel_delivery_outcome(row: &Row) -> Result<BackgroundDeliveryOutcome, String> {
    let raw: String = row.get(0).map_err(|e| e.to_string())?;
    serde_json::from_str(&raw).map_err(|e| format!("decode channel delivery outcome: {e}"))
}

fn scan_channel_support_evidence(row: &Row) -> Result<SupportEvidenceBundle, String> {
    let raw: String = row.get(0).map_err(|e| e.to_string())?;
    serde_json::from_str(&raw).map_err(|e| format!("decode channel support evidence: {e}"))
}

fn scan_channel_enablement_state(row: &Row) -> Result<kura_connectors::EnablementState, String> {
    let raw: String = row.get(0).map_err(|e| e.to_string())?;
    serde_json::from_str(&raw).map_err(|e| format!("decode channel enablement state: {e}"))
}

fn scan_channel_repair_action(row: &Row) -> Result<kura_connectors::RepairAction, String> {
    let raw: String = row.get(0).map_err(|e| e.to_string())?;
    serde_json::from_str(&raw).map_err(|e| format!("decode channel repair action: {e}"))
}

fn scan_channel_audit_record(row: &Row) -> Result<kura_connectors::ConnectorAuditRecord, String> {
    let raw: String = row.get(0).map_err(|e| e.to_string())?;
    serde_json::from_str(&raw).map_err(|e| format!("decode channel management audit: {e}"))
}

/// Serializes an optional string enum to its wire literal, or NULL when unset.
fn opt_enum_str<T: serde::Serialize>(value: &Option<T>) -> Option<String> {
    value.as_ref().map(crate::crud::enum_str)
}

impl SQLiteStore {
    /// Go `SaveChannelRoutePolicy` — upserts the current policy and appends a
    /// historical snapshot row.
    pub fn save_channel_route_policy(&self, policy: &RoutePolicy) -> Result<(), String> {
        let mut policy = policy.clone();
        if policy.route_policy_id.trim().is_empty() {
            policy.route_policy_id = new_store_id("channel_route_policy");
        }
        if is_unset_time(&policy.validated_at) {
            policy.validated_at = Utc::now();
        }
        let document = serde_json::to_string(&policy)
            .map_err(|e| format!("marshal channel route policy: {e}"))?;

        self.conn
            .execute(
                r#"INSERT INTO channel_route_policies (
                    tenant_id, connector_id, route_policy_id, validation_state, reason_code,
                    background_delivery_eligible, validated_at, audit_event_id, redaction_status, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                ON CONFLICT(tenant_id, connector_id) DO UPDATE SET
                    route_policy_id = excluded.route_policy_id,
                    validation_state = excluded.validation_state,
                    reason_code = excluded.reason_code,
                    background_delivery_eligible = excluded.background_delivery_eligible,
                    validated_at = excluded.validated_at,
                    audit_event_id = excluded.audit_event_id,
                    redaction_status = excluded.redaction_status,
                    document_json = excluded.document_json"#,
                params![
                    policy.tenant_id,
                    policy.connector_id,
                    policy.route_policy_id,
                    policy.validation_state,
                    crate::crud::null_string(&policy.reason_code),
                    i64::from(policy.background_delivery_eligible),
                    now_rfc3339(&policy.validated_at),
                    crate::crud::null_string(&policy.audit_event_id),
                    crate::crud::enum_str(&policy.redaction_status),
                    document.clone(),
                ],
            )
            .map_err(|e| format!("save channel route policy {}/{}: {e}", policy.tenant_id, policy.connector_id))?;

        self.conn
            .execute(
                r#"INSERT INTO channel_route_policy_snapshots (
                    route_policy_id, tenant_id, connector_id, validated_at, audit_event_id, redaction_status, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                ON CONFLICT(route_policy_id) DO UPDATE SET
                    validated_at = excluded.validated_at,
                    audit_event_id = excluded.audit_event_id,
                    redaction_status = excluded.redaction_status,
                    document_json = excluded.document_json"#,
                params![
                    policy.route_policy_id,
                    policy.tenant_id,
                    policy.connector_id,
                    now_rfc3339(&policy.validated_at),
                    crate::crud::null_string(&policy.audit_event_id),
                    crate::crud::enum_str(&policy.redaction_status),
                    document,
                ],
            )
            .map_err(|e| format!("save channel route policy snapshot {}: {e}", policy.route_policy_id))?;
        Ok(())
    }

    /// Go `GetChannelRoutePolicy`.
    pub fn get_channel_route_policy(
        &self,
        tenant_id: &str,
        connector_id: &str,
    ) -> Result<Option<RoutePolicy>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT document_json FROM channel_route_policies WHERE tenant_id = ?1 AND connector_id = ?2",
            )
            .map_err(|e| format!("get channel route policy {tenant_id}/{connector_id}: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id, connector_id])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        scan_channel_route_policy(row).map(Some)
    }

    /// Go `SaveChannelRoutingDecision`.
    pub fn save_channel_routing_decision(&self, decision: &RoutingDecision) -> Result<(), String> {
        let mut decision = decision.clone();
        if decision.routing_decision_id.trim().is_empty() {
            decision.routing_decision_id = new_store_id("channel_routing_decision");
        }
        if is_unset_time(&decision.occurred_at) {
            decision.occurred_at = Utc::now();
        }
        if is_unset_time(&decision.retention_expires_at) {
            decision.retention_expires_at = default_retention(decision.occurred_at);
        }
        let document = serde_json::to_string(&decision)
            .map_err(|e| format!("marshal channel routing decision: {e}"))?;

        self.conn
            .execute(
                r#"INSERT INTO channel_routing_decisions (
                    routing_decision_id, tenant_id, connector_id, connector_kind, outcome, reason_code,
                    occurred_at, retention_expires_at, redaction_status, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                ON CONFLICT(routing_decision_id) DO UPDATE SET
                    outcome = excluded.outcome,
                    reason_code = excluded.reason_code,
                    occurred_at = excluded.occurred_at,
                    retention_expires_at = excluded.retention_expires_at,
                    redaction_status = excluded.redaction_status,
                    document_json = excluded.document_json"#,
                params![
                    decision.routing_decision_id,
                    decision.tenant_id,
                    decision.connector_id,
                    decision.connector_kind,
                    crate::crud::enum_str(&decision.outcome),
                    crate::crud::null_string(&decision.reason_code),
                    now_rfc3339(&decision.occurred_at),
                    now_rfc3339(&decision.retention_expires_at),
                    crate::crud::enum_str(&decision.redaction_status),
                    document,
                ],
            )
            .map_err(|e| format!("save channel routing decision {}: {e}", decision.routing_decision_id))?;
        Ok(())
    }

    /// Go `ListChannelRoutingDecisions` — unexpired decisions, newest first.
    pub fn list_channel_routing_decisions(
        &self,
        tenant_id: &str,
        connector_id: &str,
        now: DateTime<Utc>,
    ) -> Result<Vec<RoutingDecision>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT document_json
                FROM channel_routing_decisions
                WHERE tenant_id = ?1 AND connector_id = ?2 AND retention_expires_at > ?3
                ORDER BY occurred_at DESC, routing_decision_id DESC
                LIMIT 50"#,
            )
            .map_err(|e| {
                format!("list channel routing decisions {tenant_id}/{connector_id}: {e}")
            })?;
        let mut rows = stmt
            .query(params![tenant_id, connector_id, now_rfc3339(&now)])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_channel_routing_decision(row)?);
        }
        Ok(items)
    }

    /// Go `SaveChannelForegroundReplyOutcome`.
    pub fn save_channel_foreground_reply_outcome(
        &self,
        outcome: &ForegroundReplyOutcome,
    ) -> Result<(), String> {
        let mut outcome = outcome.clone();
        if outcome.reply_outcome_id.trim().is_empty() {
            outcome.reply_outcome_id = new_store_id("channel_reply_outcome");
        }
        if is_unset_time(&outcome.occurred_at) {
            outcome.occurred_at = Utc::now();
        }
        if is_unset_time(&outcome.retention_expires_at) {
            outcome.retention_expires_at = default_retention(outcome.occurred_at);
        }
        let document = serde_json::to_string(&outcome)
            .map_err(|e| format!("marshal channel reply outcome: {e}"))?;

        self.conn
            .execute(
                r#"INSERT INTO channel_reply_outcomes (
                    reply_outcome_id, tenant_id, connector_id, routing_decision_id, status, reason_code,
                    occurred_at, retention_expires_at, redaction_status, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                ON CONFLICT(reply_outcome_id) DO UPDATE SET
                    routing_decision_id = excluded.routing_decision_id,
                    status = excluded.status,
                    reason_code = excluded.reason_code,
                    occurred_at = excluded.occurred_at,
                    retention_expires_at = excluded.retention_expires_at,
                    redaction_status = excluded.redaction_status,
                    document_json = excluded.document_json"#,
                params![
                    outcome.reply_outcome_id,
                    outcome.tenant_id,
                    outcome.connector_id,
                    crate::crud::null_string(&outcome.routing_decision_id),
                    outcome.status,
                    crate::crud::null_string(&outcome.reason_code),
                    now_rfc3339(&outcome.occurred_at),
                    now_rfc3339(&outcome.retention_expires_at),
                    crate::crud::enum_str(&outcome.redaction_status),
                    document,
                ],
            )
            .map_err(|e| format!("save channel reply outcome {}: {e}", outcome.reply_outcome_id))?;
        Ok(())
    }

    /// Go `ListChannelForegroundReplyOutcomes`.
    pub fn list_channel_foreground_reply_outcomes(
        &self,
        tenant_id: &str,
        connector_id: &str,
        now: DateTime<Utc>,
    ) -> Result<Vec<ForegroundReplyOutcome>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT document_json
                FROM channel_reply_outcomes
                WHERE tenant_id = ?1 AND connector_id = ?2 AND retention_expires_at > ?3
                ORDER BY occurred_at DESC, reply_outcome_id DESC
                LIMIT 50"#,
            )
            .map_err(|e| format!("list channel reply outcomes {tenant_id}/{connector_id}: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id, connector_id, now_rfc3339(&now)])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_channel_reply_outcome(row)?);
        }
        Ok(items)
    }

    /// Go `SaveChannelBackgroundDeliveryOutcome`.
    pub fn save_channel_background_delivery_outcome(
        &self,
        outcome: &BackgroundDeliveryOutcome,
    ) -> Result<(), String> {
        let mut outcome = outcome.clone();
        if outcome.delivery_outcome_id.trim().is_empty() {
            outcome.delivery_outcome_id = new_store_id("channel_delivery_outcome");
        }
        if is_unset_time(&outcome.occurred_at) {
            outcome.occurred_at = Utc::now();
        }
        if is_unset_time(&outcome.retention_expires_at) {
            outcome.retention_expires_at = default_retention(outcome.occurred_at);
        }
        let document = serde_json::to_string(&outcome)
            .map_err(|e| format!("marshal channel delivery outcome: {e}"))?;

        self.conn
            .execute(
                r#"INSERT INTO channel_delivery_outcomes (
                    delivery_outcome_id, tenant_id, connector_id, delivery_target_id, status, reason_code,
                    occurred_at, retention_expires_at, redaction_status, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                ON CONFLICT(delivery_outcome_id) DO UPDATE SET
                    delivery_target_id = excluded.delivery_target_id,
                    status = excluded.status,
                    reason_code = excluded.reason_code,
                    occurred_at = excluded.occurred_at,
                    retention_expires_at = excluded.retention_expires_at,
                    redaction_status = excluded.redaction_status,
                    document_json = excluded.document_json"#,
                params![
                    outcome.delivery_outcome_id,
                    outcome.tenant_id,
                    outcome.connector_id,
                    crate::crud::null_string(&outcome.delivery_target_id),
                    outcome.status,
                    crate::crud::null_string(&outcome.reason_code),
                    now_rfc3339(&outcome.occurred_at),
                    now_rfc3339(&outcome.retention_expires_at),
                    crate::crud::enum_str(&outcome.redaction_status),
                    document,
                ],
            )
            .map_err(|e| format!("save channel delivery outcome {}: {e}", outcome.delivery_outcome_id))?;
        Ok(())
    }

    /// Go `ListChannelBackgroundDeliveryOutcomes`.
    pub fn list_channel_background_delivery_outcomes(
        &self,
        tenant_id: &str,
        connector_id: &str,
        now: DateTime<Utc>,
    ) -> Result<Vec<BackgroundDeliveryOutcome>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT document_json
                FROM channel_delivery_outcomes
                WHERE tenant_id = ?1 AND connector_id = ?2 AND retention_expires_at > ?3
                ORDER BY occurred_at DESC, delivery_outcome_id DESC
                LIMIT 50"#,
            )
            .map_err(|e| {
                format!("list channel delivery outcomes {tenant_id}/{connector_id}: {e}")
            })?;
        let mut rows = stmt
            .query(params![tenant_id, connector_id, now_rfc3339(&now)])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_channel_delivery_outcome(row)?);
        }
        Ok(items)
    }

    /// Go `SaveChannelSupportEvidence` — the current row plus its historical
    /// bundle document.
    pub fn save_channel_support_evidence(
        &self,
        bundle: &SupportEvidenceBundle,
    ) -> Result<(), String> {
        let mut bundle = bundle.clone();
        if bundle.support_evidence_id.trim().is_empty() {
            bundle.support_evidence_id = new_store_id("channel_support_evidence");
        }
        if is_unset_time(&bundle.generated_at) {
            bundle.generated_at = Utc::now();
        }
        if is_unset_time(&bundle.retention_expires_at) {
            bundle.retention_expires_at = default_retention(bundle.generated_at);
        }
        let document = serde_json::to_string(&bundle)
            .map_err(|e| format!("marshal channel support evidence: {e}"))?;

        self.conn
            .execute(
                r#"INSERT INTO channel_support_evidence (
                    support_evidence_id, tenant_id, connector_id, generated_by_principal_id,
                    generated_at, current_state, retention_expires_at, redaction_status, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                ON CONFLICT(support_evidence_id) DO UPDATE SET
                    document_json = excluded.document_json"#,
                params![
                    bundle.support_evidence_id,
                    bundle.tenant_id,
                    bundle.connector_id,
                    crate::crud::null_string(&bundle.generated_by_principal_id),
                    now_rfc3339(&bundle.generated_at),
                    bundle.current_state.as_str(),
                    now_rfc3339(&bundle.retention_expires_at),
                    crate::crud::enum_str(&bundle.redaction_status),
                    document,
                ],
            )
            .map_err(|e| format!("save channel support evidence {}: {e}", bundle.support_evidence_id))?;
        Ok(())
    }

    /// Go `GetLatestChannelSupportEvidence` — the newest unexpired bundle.
    pub fn get_latest_channel_support_evidence(
        &self,
        tenant_id: &str,
        connector_id: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<SupportEvidenceBundle>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT document_json
                FROM channel_support_evidence
                WHERE tenant_id = ?1 AND connector_id = ?2 AND retention_expires_at > ?3
                ORDER BY generated_at DESC, support_evidence_id DESC
                LIMIT 1"#,
            )
            .map_err(|e| format!("get channel support evidence {tenant_id}/{connector_id}: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id, connector_id, now_rfc3339(&now)])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        scan_channel_support_evidence(row).map(Some)
    }

    /// Go `ListExpiredChannelSupportEvidence` — expired bundles, most recently
    /// expired first.
    pub fn list_expired_channel_support_evidence(
        &self,
        tenant_id: &str,
        connector_id: &str,
        now: DateTime<Utc>,
    ) -> Result<Vec<SupportEvidenceBundle>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT document_json
                FROM channel_support_evidence
                WHERE tenant_id = ?1 AND connector_id = ?2 AND retention_expires_at <= ?3
                ORDER BY retention_expires_at DESC, support_evidence_id DESC
                LIMIT 50"#,
            )
            .map_err(|e| {
                format!("list expired channel support evidence {tenant_id}/{connector_id}: {e}")
            })?;
        let mut rows = stmt
            .query(params![tenant_id, connector_id, now_rfc3339(&now)])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_channel_support_evidence(row)?);
        }
        Ok(items)
    }

    /// Go `SaveChannelConnectorEnablementState` — upserts the per-tenant
    /// enablement state (PK tenant_id + connector_id).
    pub fn save_channel_connector_enablement_state(
        &self,
        state: &kura_connectors::EnablementState,
    ) -> Result<(), String> {
        let mut state = state.clone();
        if is_unset_time(&state.changed_at) {
            state.changed_at = Utc::now();
        }
        let document = serde_json::to_string(&state)
            .map_err(|e| format!("marshal channel connector enablement: {e}"))?;

        self.conn
            .execute(
                r#"INSERT INTO channel_connector_enablement_states (
                    tenant_id, connector_id, state, reason_code, changed_by_principal_id,
                    changed_at, validated_at, audit_event_id, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                ON CONFLICT(tenant_id, connector_id) DO UPDATE SET
                    state = excluded.state,
                    reason_code = excluded.reason_code,
                    changed_by_principal_id = excluded.changed_by_principal_id,
                    changed_at = excluded.changed_at,
                    validated_at = excluded.validated_at,
                    audit_event_id = excluded.audit_event_id,
                    document_json = excluded.document_json"#,
                params![
                    state.tenant_id,
                    state.connector_id,
                    state.state,
                    crate::crud::null_string(&state.reason_code),
                    crate::crud::null_string(&state.changed_by_principal_id),
                    now_rfc3339(&state.changed_at),
                    crate::crud::opt_time_string(&state.validated_at),
                    state.audit_event_id,
                    document,
                ],
            )
            .map_err(|e| {
                format!(
                    "save channel connector enablement {}/{}: {e}",
                    state.tenant_id, state.connector_id
                )
            })?;
        Ok(())
    }

    /// Go `GetChannelConnectorEnablementState`.
    pub fn get_channel_connector_enablement_state(
        &self,
        tenant_id: &str,
        connector_id: &str,
    ) -> Result<Option<kura_connectors::EnablementState>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT document_json FROM channel_connector_enablement_states WHERE tenant_id = ?1 AND connector_id = ?2",
            )
            .map_err(|e| format!("get channel connector enablement {tenant_id}/{connector_id}: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id, connector_id])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        scan_channel_enablement_state(row).map(Some)
    }

    /// Go `SaveChannelRepairAction` — generates a repair_action_id when unset.
    pub fn save_channel_repair_action(
        &self,
        action: &kura_connectors::RepairAction,
    ) -> Result<(), String> {
        let mut action = action.clone();
        if action.repair_action_id.trim().is_empty() {
            action.repair_action_id = new_store_id("channel_repair_action");
        }
        if is_unset_time(&action.started_at) {
            action.started_at = Utc::now();
        }
        // Go defaults an empty RedactionStatus to Redacted; the Rust enum is
        // non-empty by construction (Default is Redacted), so no defaulting is needed.
        let document = serde_json::to_string(&action)
            .map_err(|e| format!("marshal channel repair action: {e}"))?;

        self.conn
            .execute(
                r#"INSERT INTO channel_repair_actions (
                    repair_action_id, tenant_id, connector_id, connector_kind, actor_principal_id,
                    action_kind, source_diagnostic_state_id, setup_session_id, status, retry_safety,
                    remediation_owner, started_at, completed_at, audit_event_id, redaction_status, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
                ON CONFLICT(repair_action_id) DO UPDATE SET
                    status = excluded.status,
                    completed_at = excluded.completed_at,
                    audit_event_id = excluded.audit_event_id,
                    redaction_status = excluded.redaction_status,
                    document_json = excluded.document_json"#,
                params![
                    action.repair_action_id,
                    action.tenant_id,
                    action.connector_id,
                    action.connector_kind,
                    crate::crud::null_string(&action.actor_principal_id),
                    crate::crud::enum_str(&action.action_kind),
                    crate::crud::null_string(&action.source_diagnostic_state_id),
                    crate::crud::null_string(&action.setup_session_id),
                    crate::crud::enum_str(&action.status),
                    opt_enum_str(&action.retry_safety),
                    opt_enum_str(&action.remediation_owner),
                    now_rfc3339(&action.started_at),
                    crate::crud::opt_time_string(&action.completed_at),
                    action.audit_event_id,
                    crate::crud::enum_str(&action.redaction_status),
                    document,
                ],
            )
            .map_err(|e| format!("save channel repair action {}: {e}", action.repair_action_id))?;
        Ok(())
    }

    /// Go `ListChannelRepairActions` — newest started first, limited to 50.
    pub fn list_channel_repair_actions(
        &self,
        tenant_id: &str,
        connector_id: &str,
    ) -> Result<Vec<kura_connectors::RepairAction>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT document_json
                FROM channel_repair_actions
                WHERE tenant_id = ?1 AND connector_id = ?2
                ORDER BY started_at DESC, repair_action_id DESC
                LIMIT 50"#,
            )
            .map_err(|e| format!("list channel repair actions {tenant_id}/{connector_id}: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id, connector_id])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_channel_repair_action(row)?);
        }
        Ok(items)
    }

    /// Go `SaveChannelManagementAuditRecord` — generates an audit_event_id when
    /// unset.
    pub fn save_channel_management_audit_record(
        &self,
        record: &kura_connectors::ConnectorAuditRecord,
    ) -> Result<(), String> {
        let mut record = record.clone();
        if record.audit_event_id.trim().is_empty() {
            record.audit_event_id = new_store_id("connector_management_audit");
        }
        if is_unset_time(&record.created_at) {
            record.created_at = Utc::now();
        }
        // Go defaults an empty RedactionStatus to Redacted; the Rust enum is
        // non-empty by construction (Default is Redacted), so no defaulting is needed.
        let document = serde_json::to_string(&record)
            .map_err(|e| format!("marshal channel management audit: {e}"))?;

        self.conn
            .execute(
                r#"INSERT INTO channel_management_audit_records (
                    audit_event_id, tenant_id, connector_id, principal_id, action, permission_gate,
                    outcome, reason_code, created_at, redaction_status, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                ON CONFLICT(audit_event_id) DO UPDATE SET
                    tenant_id = excluded.tenant_id,
                    connector_id = excluded.connector_id,
                    principal_id = excluded.principal_id,
                    action = excluded.action,
                    permission_gate = excluded.permission_gate,
                    outcome = excluded.outcome,
                    reason_code = excluded.reason_code,
                    created_at = excluded.created_at,
                    redaction_status = excluded.redaction_status,
                    document_json = excluded.document_json"#,
                params![
                    record.audit_event_id,
                    record.tenant_id,
                    record.connector_id,
                    crate::crud::null_string(&record.principal_id),
                    record.action,
                    record.permission_gate,
                    record.outcome,
                    crate::crud::null_string(&record.reason_code),
                    now_rfc3339(&record.created_at),
                    crate::crud::enum_str(&record.redaction_status),
                    document,
                ],
            )
            .map_err(|e| {
                format!(
                    "save channel management audit {}: {e}",
                    record.audit_event_id
                )
            })?;
        Ok(())
    }

    /// Go `ListChannelManagementAuditRecords` — newest created first, limited to 50.
    pub fn list_channel_management_audit_records(
        &self,
        tenant_id: &str,
        connector_id: &str,
    ) -> Result<Vec<kura_connectors::ConnectorAuditRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT document_json
                FROM channel_management_audit_records
                WHERE tenant_id = ?1 AND connector_id = ?2
                ORDER BY created_at DESC, audit_event_id DESC
                LIMIT 50"#,
            )
            .map_err(|e| {
                format!("list channel management audit {tenant_id}/{connector_id}: {e}")
            })?;
        let mut rows = stmt
            .query(params![tenant_id, connector_id])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            items.push(scan_channel_audit_record(row)?);
        }
        Ok(items)
    }
}
