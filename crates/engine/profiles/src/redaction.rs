//! Redaction ported from daemon/internal/profiles/redaction.go.
//!
//! Raw persona text (descriptions, instructions) never leaves the domain:
//! redacted profiles keep only safe summaries, and overlay references record
//! sanitized display labels plus validation evidence.

use crate::policy::contains_unsafe;
use crate::types::{
    AgentProfile, OverlayReference, OverlayReferenceInput, OverlayValidationState, RedactionStatus,
};

/// Go: `RedactProfile` — strips raw text and backfills safe summaries.
pub fn redact_profile(mut profile: AgentProfile) -> AgentProfile {
    profile.redaction_status = RedactionStatus::REDACTED;
    profile.display_identity.description.clear();
    if profile.display_identity.safe_summary.is_empty() {
        let summary = safe_summary(&profile.display_identity.name, &profile.display_name);
        profile.display_identity.safe_summary = summary;
    }
    profile.persona.instructions.clear();
    if profile.persona.safe_summary.is_empty() {
        let summary = safe_summary(&profile.persona.tone, "structured profile");
        profile.persona.safe_summary = summary;
    }
    profile
}

/// Go: `NormalizeOverlay` — converts user overlay input into a sanitized
/// reference with validation evidence; never fails, mirroring the Go
/// signature whose error return is always nil.
pub fn normalize_overlay(input: &OverlayReferenceInput) -> OverlayReference {
    let uri = input.reference_uri.trim();
    if uri.is_empty() {
        return OverlayReference {
            validation_state: OverlayValidationState::MISSING,
            failure_reason_code: "overlay_reference_missing".to_string(),
            ..OverlayReference::default()
        };
    }
    let mut state = OverlayValidationState::VALID;
    let mut reason = String::new();
    if uri.starts_with('/') || uri.contains("..") {
        state = OverlayValidationState::OUT_OF_SCOPE;
        reason = "overlay_out_of_scope".to_string();
    }
    if contains_unsafe(uri) {
        state = OverlayValidationState::UNSAFE_CONTENT;
        reason = "overlay_unsafe_content".to_string();
    }
    let mut label = unix_basename(uri).to_string();
    if contains_unsafe(&label) {
        label = "overlay reference suppressed".to_string();
    }
    let scope = {
        let scope = input.scope.trim();
        if scope.is_empty() { "profile" } else { scope }
    };
    OverlayReference {
        reference_kind: default_string(input.reference_kind.trim(), "prompt_file"),
        scope: scope.to_string(),
        reference_uri: uri.to_string(),
        safe_display_label: label,
        validation_state: state,
        failure_reason_code: reason,
        redaction_status: RedactionStatus::REDACTED,
        ..OverlayReference::default()
    }
}

/// Go: `SafeProfileSummary` — the safest available human-readable summary.
pub fn safe_profile_summary(profile: &AgentProfile) -> String {
    if !profile.persona.safe_summary.is_empty() {
        return profile.persona.safe_summary.clone();
    }
    if !profile.display_identity.safe_summary.is_empty() {
        return profile.display_identity.safe_summary.clone();
    }
    safe_summary(&profile.display_name, "profile")
}

/// Go: `safeSummary` — trimmed preferred value, byte-truncated to 160, else fallback.
fn safe_summary(preferred: &str, fallback: &str) -> String {
    let preferred = preferred.trim();
    if preferred.is_empty() {
        return fallback.to_string();
    }
    if preferred.len() > 160 {
        // Go slices `preferred[:160]` by bytes; floor to a char boundary so we
        // never panic or emit invalid UTF-8.
        let boundary = preferred
            .char_indices()
            .map(|(i, _)| i)
            .take_while(|&i| i <= 160)
            .last()
            .unwrap_or(0);
        return preferred[..boundary].to_string();
    }
    preferred.to_string()
}

/// Go: `defaultString`.
fn default_string(value: &str, fallback: &str) -> String {
    if value.trim().is_empty() {
        fallback.to_string()
    } else {
        value.trim().to_string()
    }
}

/// Port of Go's `filepath.Base` under Unix semantics (the daemon's target
/// platforms): strip trailing slashes, keep the segment after the last slash.
fn unix_basename(path: &str) -> &str {
    if path.is_empty() {
        return ".";
    }
    let stripped = path.trim_end_matches('/');
    let base = match stripped.rfind('/') {
        Some(i) => &stripped[i + 1..],
        None => stripped,
    };
    if base.is_empty() { "/" } else { base }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{ProfilesError, validate_mutation};
    use crate::types::{DisplayIdentity, MutationInput, Persona};

    #[test]
    fn validation_rejects_unsafe_and_deferred_overlay_inputs() {
        let unsafe_persona = MutationInput {
            display_name: "bad".to_string(),
            persona: Persona {
                instructions: "token=secret".to_string(),
                ..Persona::default()
            },
            ..MutationInput::default()
        };
        let err = validate_mutation(&unsafe_persona).expect_err("unsafe persona must be rejected");
        assert!(
            matches!(err, ProfilesError::InvalidProfile(_)),
            "expected ErrInvalidProfile equivalent, got {err}"
        );

        let deferred = MutationInput {
            display_name: "bad".to_string(),
            overlay_references: vec![OverlayReferenceInput {
                reference_uri: "profiles/base.md".to_string(),
                scope: "thread".to_string(),
                ..OverlayReferenceInput::default()
            }],
            ..MutationInput::default()
        };
        let err = validate_mutation(&deferred).expect_err("scoped binding must be deferred");
        assert!(
            matches!(err, ProfilesError::ScopedBindingDeferred),
            "expected ErrScopedBindingDeferred equivalent, got {err}"
        );
    }

    #[test]
    fn redact_profile_suppresses_raw_persona_and_keeps_safe_summary() {
        let profile = redact_profile(AgentProfile {
            display_name: "Support".to_string(),
            display_identity: DisplayIdentity {
                name: "Support".to_string(),
                description: "raw description".to_string(),
                ..DisplayIdentity::default()
            },
            persona: Persona {
                tone: "direct".to_string(),
                instructions: "raw instructions".to_string(),
                ..Persona::default()
            },
            ..AgentProfile::default()
        });
        assert!(profile.display_identity.description.is_empty());
        assert!(profile.persona.instructions.is_empty());
        assert!(!profile.display_identity.safe_summary.is_empty());
        assert!(!profile.persona.safe_summary.is_empty());
        assert_eq!(profile.redaction_status, RedactionStatus::REDACTED);
    }

    #[test]
    fn normalize_overlay_records_unsafe_out_of_scope_evidence() {
        let overlay = normalize_overlay(&OverlayReferenceInput {
            reference_uri: "../secret-token=abc".to_string(),
            reference_kind: "prompt_file".to_string(),
            ..OverlayReferenceInput::default()
        });
        assert_eq!(
            overlay.validation_state,
            OverlayValidationState::UNSAFE_CONTENT
        );
        assert_eq!(overlay.redaction_status, RedactionStatus::REDACTED);
        assert!(!overlay.safe_display_label.is_empty());
    }

    #[test]
    fn profile_and_overlay_summaries_do_not_expose_secrets() {
        let profile = redact_profile(AgentProfile {
            display_name: "Support".to_string(),
            display_identity: DisplayIdentity {
                name: "Support".to_string(),
                description: "api_key=hidden".to_string(),
                ..DisplayIdentity::default()
            },
            persona: Persona {
                tone: "direct".to_string(),
                instructions: "token=hidden".to_string(),
                ..Persona::default()
            },
            ..AgentProfile::default()
        });
        assert!(profile.display_identity.description.is_empty());
        assert!(profile.persona.instructions.is_empty());
        assert!(!contains_unsafe(&safe_profile_summary(&profile)));

        let overlay = normalize_overlay(&OverlayReferenceInput {
            reference_kind: "prompt".to_string(),
            reference_uri: "prompt://token=hidden".to_string(),
            ..OverlayReferenceInput::default()
        });
        assert_eq!(
            overlay.validation_state,
            OverlayValidationState::UNSAFE_CONTENT
        );
        assert!(!contains_unsafe(&overlay.safe_display_label));
    }

    #[test]
    fn unix_basename_matches_go_filepath_base() {
        assert_eq!(unix_basename(""), ".");
        assert_eq!(unix_basename("/"), "/");
        assert_eq!(unix_basename("a/b/c"), "c");
        assert_eq!(unix_basename("a/b/"), "b");
        assert_eq!(unix_basename("prompt://profile/support"), "support");
    }
}
