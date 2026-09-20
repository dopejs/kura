//! Telegram smoke evidence building (port of smoke.go).

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use kura_connectors::{DiagnosticReasonCode, RedactionStatus};
use serde::{Deserialize, Serialize};

use crate::diagnostics::{contains_unsafe_evidence, safe_evidence};
use crate::readiness::first_non_empty;
use crate::wire_enum;

wire_enum!(SmokeStatus, default Skipped; Skipped => "skipped", Passed => "passed", Failed => "failed");

wire_enum!(CredentialMode, default Unavailable; Unavailable => "unavailable", SafeLive => "safe_live", Fake => "fake");

/// Input to [`build_smoke_evidence`] (Go `SmokeInput`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SmokeInput {
    pub smoke_evidence_id: String,
    pub tenant_id: String,
    pub connector_id: String,
    pub safe_credential: bool,
    pub fake_safe_pass: bool,
    pub passed: bool,
    pub owner: String,
    pub reason: String,
    pub remaining_risk: String,
    pub validated_at: DateTime<Utc>,
    pub safe_evidence: HashMap<String, String>,
}

/// Structured smoke evidence (Go `SmokeEvidence`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmokeEvidence {
    pub smoke_evidence_id: String,
    pub tenant_id: String,
    pub connector_id: String,
    pub status: SmokeStatus,
    pub credential_mode: CredentialMode,
    pub owner: String,
    pub reason: String,
    pub remaining_risk: String,
    pub validated_at: DateTime<Utc>,
    pub retention_expires_at: DateTime<Utc>,
    pub redaction_status: RedactionStatus,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub safe_evidence: HashMap<String, String>,
}

/// Go `BuildSmokeEvidence`: builds a structured skip/pass/fail evidence
/// record, suppressing evidence that carries secret material.
#[must_use]
pub fn build_smoke_evidence(input: SmokeInput) -> SmokeEvidence {
    let now = if crate::is_unset_time(&input.validated_at) {
        Utc::now()
    } else {
        input.validated_at
    };
    let id = input.smoke_evidence_id.trim().to_string();
    let id = if id.is_empty() {
        format!("telegram_smoke_{}", input.connector_id.trim())
    } else {
        id
    };
    let owner = input.owner.trim().to_string();
    let owner = if owner.is_empty() {
        "operator".to_string()
    } else {
        owner
    };
    let mut evidence = SmokeEvidence {
        smoke_evidence_id: id,
        tenant_id: input.tenant_id.trim().to_string(),
        connector_id: input.connector_id.trim().to_string(),
        status: SmokeStatus::Skipped,
        credential_mode: CredentialMode::Unavailable,
        owner,
        reason: first_non_empty(&[input.reason.trim(), "safe_credentials_unavailable"]),
        remaining_risk: first_non_empty(&[
            input.remaining_risk.trim(),
            "Live Telegram hosted smoke was not run.",
        ]),
        validated_at: now,
        retention_expires_at: now + chrono::Duration::days(90),
        redaction_status: RedactionStatus::Redacted,
        safe_evidence: safe_evidence(&input.safe_evidence),
    };
    if contains_unsafe_evidence(&input.safe_evidence) {
        evidence.redaction_status = RedactionStatus::Suppressed;
    }
    if input.fake_safe_pass {
        evidence.credential_mode = CredentialMode::Fake;
    } else if input.safe_credential {
        evidence.credential_mode = CredentialMode::SafeLive;
    } else {
        return evidence;
    }
    if input.passed {
        evidence.status = SmokeStatus::Passed;
        evidence.reason = "healthy".to_string();
        return evidence;
    }
    evidence.status = SmokeStatus::Failed;
    evidence.reason = first_non_empty(&[
        input.reason.trim(),
        DiagnosticReasonCode::UnknownConnectorFailure.as_str(),
    ]);
    evidence
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, s)
            .single()
            .expect("valid timestamp")
    }

    // Go TestBuildSmokeEvidenceStructuredSkipAndFakePass.
    #[test]
    fn build_smoke_evidence_structured_skip_and_fake_pass() {
        let now = ts(2026, 5, 8, 10, 0, 0);
        let skip = build_smoke_evidence(SmokeInput {
            tenant_id: "ten_telegram".to_string(),
            connector_id: "telegram-main".to_string(),
            validated_at: now,
            ..SmokeInput::default()
        });
        assert_eq!(skip.status, SmokeStatus::Skipped);
        assert_eq!(skip.credential_mode, CredentialMode::Unavailable);
        assert!(!skip.remaining_risk.is_empty());

        let fake_pass = build_smoke_evidence(SmokeInput {
            tenant_id: "ten_telegram".to_string(),
            connector_id: "telegram-main".to_string(),
            fake_safe_pass: true,
            passed: true,
            validated_at: now,
            safe_evidence: HashMap::from([("transport".to_string(), "fake".to_string())]),
            remaining_risk: "live provider not exercised".to_string(),
            safe_credential: true,
            ..SmokeInput::default()
        });
        assert_eq!(fake_pass.status, SmokeStatus::Passed);
        assert_eq!(fake_pass.credential_mode, CredentialMode::Fake);
        assert_eq!(fake_pass.reason, "healthy");
    }

    // Go TestBuildSmokeEvidenceSuppressesUnsafeEvidence.
    #[test]
    fn build_smoke_evidence_suppresses_unsafe_evidence() {
        let smoke = build_smoke_evidence(SmokeInput {
            tenant_id: "ten_telegram".to_string(),
            connector_id: "telegram-main".to_string(),
            safe_credential: true,
            passed: false,
            safe_evidence: HashMap::from([("token".to_string(), "123:SECRET".to_string())]),
            ..SmokeInput::default()
        });
        assert_eq!(smoke.redaction_status, RedactionStatus::Suppressed);
        assert!(smoke.safe_evidence.is_empty());
    }
}
