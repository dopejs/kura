use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;
use serde::Serialize;

/// Redaction outcome recorded on every thread-scoped evidence record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedactionStatus {
    Redacted,
    Suppressed,
    RedactionFailed,
}

/// A redaction-safe summary plus the status under which it was produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafeEvidenceSummary {
    pub text: String,
    pub status: RedactionStatus,
}

/// Mirrors Go `SafeSummary`: disallowed content collapses to "suppressed".
pub fn safe_summary(text: &str, allowed: bool) -> SafeEvidenceSummary {
    if !allowed {
        return suppressed_summary();
    }
    SafeEvidenceSummary {
        text: text.trim().to_string(),
        status: RedactionStatus::Redacted,
    }
}

fn suppressed_summary() -> SafeEvidenceSummary {
    SafeEvidenceSummary {
        text: "suppressed".to_string(),
        status: RedactionStatus::Suppressed,
    }
}

const RAW_UNSAFE_PATTERNS: &[&str] = &[
    r"(?i)\bauthorization\s*:\s*bearer\s+\S+",
    r"(?i)\b(api[_-]?key|password|secret|access[_-]?token|refresh[_-]?token)\s*[:=]\s*\S+",
    r"\bsk-[A-Za-z0-9][A-Za-z0-9_-]{12,}\b",
];

/// Compiled unsafe-content patterns. `None` only if a compile-time constant
/// pattern fails to compile, in which case callers fail closed (suppress).
static UNSAFE_PATTERNS: LazyLock<Option<Vec<Regex>>> = LazyLock::new(|| {
    RAW_UNSAFE_PATTERNS
        .iter()
        .map(|raw| Regex::new(raw))
        .collect::<Result<Vec<_>, _>>()
        .ok()
});

/// Shared body of Go `SafeContinuityContent` / `SafeGroupRoomEvidenceSummary`.
fn suppress_unsafe_content(text: &str) -> SafeEvidenceSummary {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return SafeEvidenceSummary {
            text: String::new(),
            status: RedactionStatus::Redacted,
        };
    }
    if looks_like_raw_provider_payload(trimmed) {
        return suppressed_summary();
    }
    match UNSAFE_PATTERNS.as_ref() {
        // Fail closed: without the pattern set nothing is provably safe.
        None => suppressed_summary(),
        Some(patterns) => {
            if patterns.iter().any(|pattern| pattern.is_match(trimmed)) {
                suppressed_summary()
            } else {
                safe_summary(trimmed, true)
            }
        }
    }
}

/// Redaction filter for continuity turn and artifact excerpt content.
pub fn safe_continuity_content(text: &str) -> SafeEvidenceSummary {
    suppress_unsafe_content(text)
}

/// Redaction filter for group/room evidence summaries.
pub fn safe_group_room_evidence_summary(text: &str) -> SafeEvidenceSummary {
    suppress_unsafe_content(text)
}

/// Detects pasted raw provider payloads (chat-completion JSON) so they are
/// never surfaced as safe evidence.
fn looks_like_raw_provider_payload(text: &str) -> bool {
    let trimmed = text.trim();
    if !(trimmed.starts_with('{') || trimmed.starts_with('[')) {
        return false;
    }
    let lower = trimmed.to_lowercase();
    (lower.contains("\"choices\"") && lower.contains("\"usage\""))
        || (lower.contains("\"messages\"") && lower.contains("\"model\""))
        || lower.contains("\"raw_provider_payload\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::Duration;
    use chrono::TimeZone;
    use chrono::Utc;

    use crate::continuity::ContinuityDecision;
    use crate::continuity::ContinuityReason;
    use crate::continuity::ContinuityRole;
    use crate::continuity::ContinuityTurn;
    use crate::continuity::RuntimeArtifactExcerpt;
    use crate::continuity::preview_item_for_turn;
    use crate::source::SourceKind;

    // Port of TestContinuityPreviewSuppressesUnsafeTurnAndArtifactEvidence.
    #[test]
    fn continuity_preview_suppresses_unsafe_turn_and_artifact_evidence() {
        let now = Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap();
        let turn = ContinuityTurn {
            continuity_turn_id: "turn_unsafe".to_string(),
            tenant_id: "ten_1".to_string(),
            thread_id: "thr_1".to_string(),
            session_segment_id: "seg_1".to_string(),
            acceptance_sequence: 1,
            role: ContinuityRole::User,
            source_kind: SourceKind::Chat,
            source_linkage_id: String::new(),
            source_message_id: String::new(),
            source_timestamp: None,
            dispatch_id: String::new(),
            response_to_turn_id: String::new(),
            safe_content: "raw unsafe content".to_string(),
            content_redaction_status: RedactionStatus::RedactionFailed,
            artifact_excerpt_refs: Vec::new(),
            recorded_at: now,
            retention_expires_at: Some(now + Duration::days(90)),
            source_event_key: String::new(),
        };
        let item = preview_item_for_turn(
            &turn,
            ContinuityDecision::Excluded,
            ContinuityReason::RedactionFailed,
            0,
        );
        assert_eq!(item.safe_summary, "suppressed");
        assert_eq!(item.redaction_status, RedactionStatus::Suppressed);

        let suppressed = safe_summary("unsafe artifact", false);
        let artifact = RuntimeArtifactExcerpt {
            artifact_excerpt_id: "artex_1".to_string(),
            tenant_id: String::new(),
            thread_id: String::new(),
            session_segment_id: String::new(),
            continuity_turn_id: String::new(),
            resource_kind: "run".to_string(),
            resource_id: "run_1".to_string(),
            excerpt_text: suppressed.text.clone(),
            excerpt_source: String::new(),
            created_at: now,
            retention_expires_at: None,
            redaction_status: suppressed.status,
        };
        assert_eq!(artifact.excerpt_text, "suppressed");
        assert_eq!(artifact.redaction_status, RedactionStatus::Suppressed);
    }

    // Port of TestSafeContinuityContentSuppressesSecretsAndProviderPayloads.
    #[test]
    fn safe_continuity_content_suppresses_secrets_and_provider_payloads() {
        for input in [
            "Authorization: Bearer token_redacted",
            "api_key=sk-secretsecretsecret",
            r#"{"choices":[{"message":{"content":"raw"}}],"usage":{"total_tokens":1}}"#,
        ] {
            let summary = safe_continuity_content(input);
            assert_eq!(summary.text, "suppressed", "input: {input}");
            assert_eq!(
                summary.status,
                RedactionStatus::Suppressed,
                "input: {input}"
            );
        }
        let summary = safe_continuity_content("ordinary follow-up text");
        assert_eq!(summary.text, "ordinary follow-up text");
        assert_eq!(summary.status, RedactionStatus::Redacted);
    }
}
