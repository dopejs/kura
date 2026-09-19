//! Port of `daemon/internal/evaluation/tool_call_inspection.go`: building
//! tool-call inspections and classifying them from evidence coordinates.

use chrono::{DateTime, Utc};

use crate::campaign::is_zero_time;
use crate::discovery_scoring::redaction_status_default;
use crate::error::EvaluationError;
use crate::product_validation::validate_tenant_scoped_product_request;
use crate::types::{RedactionStatus, RetentionState, ToolCallInspection};

pub const INSPECTION_MATCHED: &str = "matched";
pub const INSPECTION_DRIFTED: &str = "drifted";
pub const INSPECTION_FAILED: &str = "failed";
pub const INSPECTION_UNSUPPORTED: &str = "unsupported";
pub const INSPECTION_MISSING_ORIGINAL_EVIDENCE: &str = "missing_original_evidence";
pub const INSPECTION_MISSING_REPLAY_EVIDENCE: &str = "missing_replay_evidence";
pub const INSPECTION_LIVE_VALIDATION_DENIED: &str = "live_validation_denied";
pub const INSPECTION_LIVE_VALIDATION_ABORTED: &str = "live_validation_aborted";
pub const INSPECTION_LIVE_VALIDATION_FAILED: &str = "live_validation_failed";
pub const INSPECTION_LIVE_VALIDATION_OPERATOR_ACTION: &str =
    "live_validation_operator_action_needed";
pub const INSPECTION_LIVE_VALIDATION_COMPLETED: &str = "live_validation_completed";

/// Go `ToolCallInspectionInput`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolCallInspectionInput {
    pub inspection_id: String,
    pub tenant_id: String,
    pub campaign_id: String,
    pub campaign_item_id: String,
    pub tool_call_ref: String,
    pub original_evidence_ref: String,
    pub non_live_replay_evidence_ref: String,
    pub live_validation_ledger_refs: Vec<String>,
    pub unsupported: bool,
    pub failed: bool,
    pub drifted: bool,
    pub live_validation_outcome: String,
    pub diff_summary: String,
    pub redaction_status: RedactionStatus,
}

/// Go `BuildToolCallInspection`.
pub fn build_tool_call_inspection(
    input: ToolCallInspectionInput,
    now: DateTime<Utc>,
) -> Result<ToolCallInspection, EvaluationError> {
    validate_tenant_scoped_product_request(&input.tenant_id)?;
    if input.campaign_id.is_empty()
        || input.campaign_item_id.is_empty()
        || input.tool_call_ref.is_empty()
    {
        return Err(EvaluationError::ToolCallInspectionEvidenceRequired);
    }
    let now = if is_zero_time(now) { Utc::now() } else { now };
    let inspection_id = {
        let trimmed = input.inspection_id.trim().to_string();
        if trimmed.is_empty() {
            format!(
                "inspection_{}",
                input.tool_call_ref.replace(':', "_").replace('/', "_")
            )
        } else {
            trimmed
        }
    };
    let classification = classify_tool_call_inspection(&input);
    Ok(ToolCallInspection {
        inspection_id,
        tenant_id: input.tenant_id,
        campaign_id: input.campaign_id,
        campaign_item_id: input.campaign_item_id,
        tool_call_ref: input.tool_call_ref,
        original_evidence_ref: input.original_evidence_ref,
        non_live_replay_evidence_ref: input.non_live_replay_evidence_ref,
        live_validation_ledger_refs: input.live_validation_ledger_refs.clone(),
        classification,
        diff_summary: input.diff_summary,
        redaction_status: redaction_status_default(input.redaction_status),
        retention_state: RetentionState::Active,
        created_at: now,
        updated_at: now,
        ..ToolCallInspection::default()
    })
}

/// Go `ClassifyToolCallInspection`.
#[must_use]
pub fn classify_tool_call_inspection(input: &ToolCallInspectionInput) -> String {
    if input.unsupported {
        return INSPECTION_UNSUPPORTED.to_string();
    }
    if input.original_evidence_ref.is_empty() {
        return INSPECTION_MISSING_ORIGINAL_EVIDENCE.to_string();
    }
    if input.non_live_replay_evidence_ref.is_empty() {
        return INSPECTION_MISSING_REPLAY_EVIDENCE.to_string();
    }
    match input.live_validation_outcome.as_str() {
        "denied" => return INSPECTION_LIVE_VALIDATION_DENIED.to_string(),
        "aborted" => return INSPECTION_LIVE_VALIDATION_ABORTED.to_string(),
        "failed" => return INSPECTION_LIVE_VALIDATION_FAILED.to_string(),
        "operator_action_needed" => return INSPECTION_LIVE_VALIDATION_OPERATOR_ACTION.to_string(),
        "completed" => return INSPECTION_LIVE_VALIDATION_COMPLETED.to_string(),
        _ => {}
    }
    if input.failed {
        return INSPECTION_FAILED.to_string();
    }
    if input.drifted {
        return INSPECTION_DRIFTED.to_string();
    }
    INSPECTION_MATCHED.to_string()
}
