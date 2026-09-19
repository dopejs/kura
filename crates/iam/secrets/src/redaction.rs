//! Redaction guarantees: secret values are never logged or serialized into
//! output. Summaries carry only refs, version IDs, statuses, and the rule
//! that produced them.

use serde::{Deserialize, Serialize};

use crate::error::{Result, SecretsError};
use crate::types::{ResolutionStatus, SecretStatus};

/// Placeholder substituted for any non-empty secret value.
pub const REDACTED_VALUE: &str = "[REDACTED]";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactedSecretSummary {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub secret_ref: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub secret_version_id: String,
    /// Go `omitempty` on a string-typed status: `None` is the empty value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<ResolutionStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<SecretStatus>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub disabled_reason: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub redaction_rule: String,
}

/// Redacts a secret value. Empty stays empty (Go parity), anything else
/// becomes `[REDACTED]`.
pub fn redact_secret_value(value: &str) -> &str {
    if value.is_empty() { "" } else { REDACTED_VALUE }
}

/// Builds ref-only summaries for secret references; never touches values.
pub fn redact_secret_refs(secret_refs: &[String]) -> Vec<RedactedSecretSummary> {
    let mut items = Vec::with_capacity(secret_refs.len());
    for secret_ref in secret_refs {
        let trimmed = secret_ref.trim();
        if trimmed.is_empty() {
            continue;
        }
        items.push(RedactedSecretSummary {
            secret_ref: trimmed.to_string(),
            resolution: Some(ResolutionStatus::Unavailable),
            redaction_rule: "secret_ref_only".to_string(),
            ..RedactedSecretSummary::default()
        });
    }
    items
}

/// Reports whether `value` contains any leak sentinel (Go
/// `ContainsAnyLeakSentinel`). Empty sentinels are ignored.
pub fn contains_any_leak_sentinel(value: &str, sentinels: &[String]) -> bool {
    sentinels
        .iter()
        .any(|sentinel| !sentinel.is_empty() && value.contains(sentinel.as_str()))
}

/// Marshals `value` to JSON, then applies [`contains_any_leak_sentinel`]
/// (Go `JSONContainsAnyLeakSentinel`).
pub fn json_contains_any_leak_sentinel<T: Serialize>(
    value: &T,
    sentinels: &[String],
) -> Result<bool> {
    let data = serde_json::to_string(value)
        .map_err(|err| SecretsError::Store(format!("marshal for leak check: {err}")))?;
    Ok(contains_any_leak_sentinel(&data, sentinels))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redact_secret_value_preserves_empty() {
        assert_eq!(redact_secret_value(""), "");
        assert_eq!(redact_secret_value("super-secret"), REDACTED_VALUE);
    }

    #[test]
    fn redact_secret_refs_trims_and_skips_empty() {
        let summaries = redact_secret_refs(&[
            "  a/ref ".to_string(),
            "".to_string(),
            "   ".to_string(),
            "b/ref".to_string(),
        ]);
        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].secret_ref, "a/ref");
        assert_eq!(summaries[0].resolution, Some(ResolutionStatus::Unavailable));
        assert_eq!(summaries[0].redaction_rule, "secret_ref_only");
        assert_eq!(summaries[1].secret_ref, "b/ref");
    }

    #[test]
    fn redacted_summary_json_carries_no_secret_material() {
        let summaries = redact_secret_refs(&["svc/token".to_string()]);
        let payload = serde_json::to_value(&summaries[0]).expect("serialize summary");
        assert_eq!(
            payload,
            json!({
                "secretRef": "svc/token",
                "resolution": "unavailable",
                "redactionRule": "secret_ref_only"
            })
        );
    }

    #[test]
    fn leak_sentinels_match_substrings_and_ignore_empty() {
        let sentinels = vec!["DO_NOT_LEAK".to_string(), "".to_string()];
        assert!(contains_any_leak_sentinel(
            "prefix DO_NOT_LEAK suffix",
            &sentinels
        ));
        assert!(!contains_any_leak_sentinel("clean value", &sentinels));
        assert!(!contains_any_leak_sentinel("anything", &[]));
    }

    #[test]
    fn json_leak_sentinel_scans_marshaled_payload() {
        let sentinels = vec!["DO_NOT_LEAK".to_string()];
        let leaked = json!({"nested": ["x", "DO_NOT_LEAK"]});
        let clean = json!({"nested": ["x", "y"]});
        assert!(json_contains_any_leak_sentinel(&leaked, &sentinels).expect("leak check"));
        assert!(!json_contains_any_leak_sentinel(&clean, &sentinels).expect("leak check"));
    }
}
