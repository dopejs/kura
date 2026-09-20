//! Redaction primitives (port of redaction.go): bounded, secret-free labels and
//! reasons for binding inspection, runtime evidence, events, and audit output
//! (FR-028, SC-014).

use crate::policy::contains_unsafe;
use crate::types::RedactionStatus;

/// Bounds any user-facing label surfaced from binding state.
pub const MAX_SAFE_LABEL_LEN: usize = 160;

/// SafeLabel returns a bounded, secret-free version of a label suitable for binding
/// inspection, runtime evidence, events, and audit output (FR-028, SC-014). Unsafe or
/// empty input collapses to a neutral placeholder so nothing sensitive leaks.
pub fn safe_label(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        return "(unnamed)".to_string();
    }
    if contains_unsafe(value) {
        return "(redacted)".to_string();
    }
    // Strip control characters and collapse whitespace.
    let mut out = String::with_capacity(value.len());
    let mut last_space = false;
    for r in value.chars() {
        if (r as u32) < 0x20 || r == '\u{7f}' {
            continue;
        }
        if r == ' ' || r == '\t' {
            if last_space {
                continue;
            }
            last_space = true;
            out.push(' ');
            continue;
        }
        last_space = false;
        out.push(r);
    }
    let out = out.trim();
    if out.is_empty() {
        return "(unnamed)".to_string();
    }
    if out.len() > MAX_SAFE_LABEL_LEN {
        return truncate_bytes(out, MAX_SAFE_LABEL_LEN);
    }
    out.to_string()
}

/// Returns a valid UTF-8 string whose total byte length (including the "…" marker)
/// does not exceed `limit`.
fn truncate_bytes(value: &str, limit: usize) -> String {
    const MARKER: &str = "…";
    if limit <= MARKER.len() {
        return MARKER.to_string();
    }
    let budget = limit - MARKER.len();
    // Walk back to a char boundary; equivalent to Go's byte-at-a-time UTF-8 repair,
    // since only the truncation point can split a multi-byte sequence.
    let mut end = budget.min(value.len());
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = String::with_capacity(end + MARKER.len());
    out.push_str(&value[..end]);
    out.push_str(MARKER);
    out
}

/// SafeReason returns a bounded, secret-free reason code/string for denial evidence.
pub fn safe_reason(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        return "unspecified".to_string();
    }
    if contains_unsafe(value) {
        return "redacted".to_string();
    }
    if value.len() > MAX_SAFE_LABEL_LEN {
        return truncate_bytes(value, MAX_SAFE_LABEL_LEN);
    }
    value.to_string()
}

/// Reports whether redaction was applied to a label. A label that had to be scrubbed
/// reports `RedactionStatus::REDACTED`; otherwise `RedactionStatus::NOT_REQUIRED`.
pub fn redaction_status_for(original: &str) -> RedactionStatus {
    if safe_label(original) != original.trim() {
        return RedactionStatus::REDACTED;
    }
    RedactionStatus::NOT_REQUIRED
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection::{RuntimeBindingEvidenceInput, build_runtime_binding_evidence};
    use crate::types::*;
    use chrono::{DateTime, Utc};

    // Port of TestSafeLabel.
    #[test]
    fn safe_label_behavior() {
        assert_eq!(safe_label("  Personal Workspace  "), "Personal Workspace");
        assert_eq!(safe_label(""), "(unnamed)");
        assert_eq!(safe_label("secret=hunter2"), "(redacted)");
        let got = safe_label("line\nbreak\tand   spaces");
        assert!(
            !got.contains('\n') && !got.contains('\t'),
            "control chars not stripped: {got:?}"
        );
        let long = "a".repeat(500);
        assert!(safe_label(&long).len() <= MAX_SAFE_LABEL_LEN);
    }

    // Port of TestSafeReason.
    #[test]
    fn safe_reason_behavior() {
        assert_eq!(safe_reason("token=abc"), "redacted");
        assert_eq!(safe_reason(""), "unspecified");
    }

    // Port of TestContainsUnsafeMarkers (FR-028/SC-014): the redaction primitive must
    // catch a broad set of credential and raw payload markers.
    #[test]
    fn contains_unsafe_markers() {
        let unsafe_values = [
            "password=hunter2",
            "Authorization: Bearer abc",
            "bearer eyJhbGciOiJIUzI1NiJ9.payload.sig",
            "x-api-key: k",
            "ACCESS_KEY=AKIA",
            "private_key: ...",
            "passwd=root",
        ];
        for v in unsafe_values {
            assert_eq!(safe_label(v), "(redacted)", "SafeLabel should redact {v:?}");
            assert_eq!(safe_reason(v), "redacted", "SafeReason should redact {v:?}");
        }
        // Benign operator text must survive.
        assert_eq!(safe_label("Marketing Workspace"), "Marketing Workspace");
    }

    // Port of TestBuildRuntimeBindingEvidence_AppliedAndRedacted (B14/B20): denial
    // evidence must never carry secrets; the builder routes through redaction.
    #[test]
    fn build_evidence_applied_and_redacted() {
        let sel = EffectiveBindingSelection {
            outcome: ResolutionOutcome::RESOLVED,
            binding_scope: BindingRuntimeScope::CHANNEL,
            binding_id: "bnd_1".into(),
            selected_profile_id: "prof_1".into(),
            selected_workspace_id: "ws_1".into(),
            capability_visibility: vec![CapabilityDecision {
                capability_id: "cap.x".into(),
                effective: EffectiveVisibility::HIDDEN,
                reason: "token=leak".into(),
                scope: "workspace".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let ev = build_runtime_binding_evidence(
            &sel,
            &RuntimeBindingEvidenceInput {
                projection_id: "brp_1".into(),
                tenant_id: "ten_1".into(),
                resource_kind: "thread".into(),
                resource_id: "thr_1".into(),
                occurred_at: DateTime::<Utc>::UNIX_EPOCH,
                legacy_default: false,
            },
        );
        assert_eq!(ev.classification, Classification::APPLIED);
        assert_ne!(
            ev.capability_visibility[0].reason, "token=leak",
            "denial reason leaked secret"
        );
        assert_eq!(ev.redaction_status, RedactionStatus::REDACTED);
    }

    // Port of TestBuildRuntimeBindingEvidence_DefaultAndLegacy.
    #[test]
    fn build_evidence_default_and_legacy() {
        let def = build_runtime_binding_evidence(
            &EffectiveBindingSelection {
                outcome: ResolutionOutcome::DEFAULT,
                ..Default::default()
            },
            &RuntimeBindingEvidenceInput::default(),
        );
        assert_eq!(def.classification, Classification::DEFAULT);
        let legacy = build_runtime_binding_evidence(
            &EffectiveBindingSelection {
                outcome: ResolutionOutcome::DEFAULT,
                ..Default::default()
            },
            &RuntimeBindingEvidenceInput {
                legacy_default: true,
                ..Default::default()
            },
        );
        assert_eq!(legacy.classification, Classification::LEGACY);
    }
}
