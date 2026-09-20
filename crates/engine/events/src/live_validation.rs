//! Live-validation attempt / ledger / comparison / reconciliation events
//! (port of `live_validation.go`).

use crate::util::payload;
use crate::{Event, Resource};
use kura_livevalidation::{
    Attempt, Comparison, Denial, ReconciliationResolution, SideEffectLedgerEntry,
};

pub const LIVE_VALIDATION_STARTED_NAME: &str = "live_validation.started";
pub const LIVE_VALIDATION_BLOCKED_NAME: &str = "live_validation.blocked";
pub const LIVE_VALIDATION_AWAITING_APPROVAL_NAME: &str = "live_validation.awaiting_approval";
pub const LIVE_VALIDATION_SIDE_EFFECT_RECORDED_NAME: &str = "live_validation.side_effect_recorded";
pub const LIVE_VALIDATION_OPERATOR_ACTION_NEEDED_NAME: &str =
    "live_validation.operator_action_needed";
pub const LIVE_VALIDATION_COMPLETED_NAME: &str = "live_validation.completed";
pub const LIVE_VALIDATION_COMPARISON_COMPLETED_NAME: &str = "live_validation.comparison_completed";
pub const LIVE_VALIDATION_RECONCILIATION_RESOLVED_NAME: &str =
    "live_validation.reconciliation_resolved";
pub const LIVE_VALIDATION_KILL_SWITCH_CHANGED_NAME: &str = "live_validation.kill_switch_changed";
pub const LIVE_VALIDATION_ABORTED_NAME: &str = "live_validation.aborted";

/// Go: `LiveValidationAttemptEvent` — the first denial's gate and reason code
/// are surfaced when the attempt was blocked.
#[must_use]
pub fn live_validation_attempt_event(name: &str, attempt: Attempt, denials: &[Denial]) -> Event {
    let mut map = payload![
        "validationId" => attempt.validation_id,
        "tenantId" => attempt.tenant_id,
        "candidateId" => attempt.candidate_id,
        "environmentScope" => attempt.environment_scope,
        "status" => attempt.status.as_str(),
    ];
    if !denials.is_empty() {
        map.insert(
            "denials".to_string(),
            serde_json::to_value(denials).unwrap_or(serde_json::Value::Null),
        );
        map.insert(
            "gate".to_string(),
            serde_json::to_value(&denials[0].gate).unwrap_or(serde_json::Value::Null),
        );
        map.insert(
            "reasonCode".to_string(),
            serde_json::to_value(&denials[0].reason_code).unwrap_or(serde_json::Value::Null),
        );
    }
    Event {
        tenant_id: attempt.tenant_id.clone(),
        category: "evaluation".to_string(),
        name: name.to_string(),
        occurred_at: attempt.updated_at,
        resource: Resource {
            kind: "live_validation".to_string(),
            id: attempt.validation_id.clone(),
        },
        payload: map,
        ..Event::default()
    }
}

/// Go: `LiveValidationLedgerEvent`.
#[must_use]
pub fn live_validation_ledger_event(name: &str, entry: SideEffectLedgerEntry) -> Event {
    Event {
        tenant_id: entry.tenant_id.clone(),
        category: "evaluation".to_string(),
        name: name.to_string(),
        occurred_at: entry.updated_at,
        resource: Resource {
            kind: "live_validation_ledger_entry".to_string(),
            id: entry.ledger_entry_id.clone(),
        },
        payload: payload![
            "validationId" => entry.validation_id,
            "tenantId" => entry.tenant_id,
            "ledgerEntryId" => entry.ledger_entry_id,
            "toolClass" => entry.tool_class.as_str(),
            "actionRef" => entry.action_ref,
            "outcome" => entry.outcome.as_str(),
            "ambiguousCommit" => entry.ambiguous_commit,
        ],
        ..Event::default()
    }
}

/// Go: `LiveValidationComparisonEvent`.
#[must_use]
pub fn live_validation_comparison_event(comparison: Comparison) -> Event {
    Event {
        category: "evaluation".to_string(),
        name: LIVE_VALIDATION_COMPARISON_COMPLETED_NAME.to_string(),
        occurred_at: comparison.generated_at,
        resource: Resource {
            kind: "live_validation_comparison".to_string(),
            id: comparison.comparison_id.clone(),
        },
        payload: payload![
            "validationId" => comparison.validation_id,
            "comparisonId" => comparison.comparison_id,
            "terminalStatus" => comparison.terminal_status.as_str(),
        ],
        ..Event::default()
    }
}

/// Go: `LiveValidationReconciliationEvent`.
#[must_use]
pub fn live_validation_reconciliation_event(resolution: ReconciliationResolution) -> Event {
    Event {
        tenant_id: resolution.tenant_id.clone(),
        category: "evaluation".to_string(),
        name: LIVE_VALIDATION_RECONCILIATION_RESOLVED_NAME.to_string(),
        occurred_at: resolution.resolved_at,
        resource: Resource {
            kind: "live_validation_reconciliation".to_string(),
            id: resolution.reconciliation_id.clone(),
        },
        payload: payload![
            "tenantId" => resolution.tenant_id,
            "ambiguousCommitId" => resolution.ambiguous_commit_id,
            "reconciliationId" => resolution.reconciliation_id,
            "resolution" => resolution.resolution.as_str(),
            "resolvedBy" => resolution.resolved_by,
        ],
        ..Event::default()
    }
}
