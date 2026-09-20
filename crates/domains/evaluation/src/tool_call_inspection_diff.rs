//! Port of `daemon/internal/evaluation/tool_call_inspection_diff.go`:
//! redacted comparison of original vs replay tool-call evidence.

use crate::product_redaction::{RedactionPolicy, redact_evidence_payload};
use crate::types::RedactionStatus;

/// Go `ToolCallDiffInput`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolCallDiffInput {
    pub original: serde_json::Map<String, serde_json::Value>,
    pub replay: serde_json::Map<String, serde_json::Value>,
    pub policy: RedactionPolicy,
}

/// Go `RedactedToolCallDiff`.
#[must_use]
pub fn redacted_tool_call_diff(input: ToolCallDiffInput) -> (String, RedactionStatus) {
    let original = redact_evidence_payload(&input.original, &input.policy);
    let replay = redact_evidence_payload(&input.replay, &input.policy);
    let mut status = RedactionStatus::Clean;
    if original.status == RedactionStatus::Redacted || replay.status == RedactionStatus::Redacted {
        status = RedactionStatus::Redacted;
    }
    if serde_json::Value::Object(original.payload) == serde_json::Value::Object(replay.payload) {
        (
            "tool call evidence matched after redaction".to_string(),
            status,
        )
    } else {
        (
            "tool call evidence drifted after redaction".to_string(),
            status,
        )
    }
}
