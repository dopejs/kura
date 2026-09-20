//! Port of daemon/internal/connectors/matrix/redaction.go: evidence redaction
//! rules that strip secrets and raw provider payloads from safe evidence.

use std::collections::HashMap;

use kura_connectors::RedactionStatus;

use crate::types::RedactionResult;

/// Go `RedactEvidence`: every evidence key whose lowercased name contains a
/// sensitive marker (`token`, `secret`, `rawProviderPayload`,
/// `payload`, `authorization`) is dropped and the result status becomes
/// `suppressed`; everything else is retained as safe evidence.
#[must_use]
pub fn redact_evidence(evidence: &HashMap<String, String>) -> RedactionResult {
    let mut safe = HashMap::new();
    let mut status = RedactionStatus::Redacted;
    for (key, value) in evidence {
        let lower = key.to_lowercase();
        if lower.contains("token")
            || lower.contains("secret")
            || lower.contains("rawproviderpayload")
            || lower.contains("payload")
            || lower.contains("authorization")
        {
            status = RedactionStatus::Suppressed;
            continue;
        }
        safe.insert(key.clone(), value.clone());
    }
    RedactionResult {
        status,
        safe_evidence: safe,
    }
}
