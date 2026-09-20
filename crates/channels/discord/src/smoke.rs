//! Smoke evidence (port of smoke.go): the structured skip/pass/fail evidence
//! produced when safe credentials are unavailable or a live probe ran.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::redaction_status_redacted;
use kura_connectors::{DiagnosticReasonCode, RedactionStatus};

/// Go `SmokeStatus`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SmokeStatus {
    #[default]
    Passed,
    Failed,
    Skipped,
}

impl SmokeStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            SmokeStatus::Passed => "passed",
            SmokeStatus::Failed => "failed",
            SmokeStatus::Skipped => "skipped",
        }
    }
}

impl std::fmt::Display for SmokeStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Go `CredentialMode`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialMode {
    #[default]
    SafeLive,
    Fake,
    Unavailable,
}

impl CredentialMode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            CredentialMode::SafeLive => "safe_live",
            CredentialMode::Fake => "fake",
            CredentialMode::Unavailable => "unavailable",
        }
    }
}

impl std::fmt::Display for CredentialMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Go `SmokeInput` (no JSON tags).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SmokeInput {
    pub smoke_evidence_id: String,
    pub tenant_id: String,
    pub connector_id: String,
    pub safe_credentials_approved: bool,
    pub passed: bool,
    pub owner: String,
    pub reason: String,
    pub remaining_risk: String,
    pub validated_at: DateTime<Utc>,
    pub safe_evidence: HashMap<String, String>,
}

/// Go `SmokeEvidence`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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

/// Go `BuildSmokeEvidence`: safe credentials unavailable -> structured skip;
/// approved and passed -> passed; approved and failed -> failed.
#[must_use]
pub fn build_smoke_evidence(input: SmokeInput) -> SmokeEvidence {
    let mut now = input.validated_at;
    if now == DateTime::<Utc>::default() {
        now = Utc::now();
    }
    let id = input.smoke_evidence_id.trim().to_string();
    let id = if id.is_empty() {
        format!("discord_smoke_{}", input.connector_id.trim())
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
        reason: first_non_empty(&input.reason, "safe_credentials_unavailable"),
        remaining_risk: first_non_empty(
            &input.remaining_risk,
            "Live Discord hosted smoke was not run.",
        ),
        validated_at: now,
        retention_expires_at: now + Duration::days(90),
        redaction_status: redaction_status_redacted(),
        safe_evidence: input.safe_evidence.clone(),
    };
    if !input.safe_credentials_approved {
        return evidence;
    }
    evidence.credential_mode = CredentialMode::SafeLive;
    if input.passed {
        evidence.status = SmokeStatus::Passed;
        evidence.reason = "healthy".to_string();
        evidence.remaining_risk = String::new();
        return evidence;
    }
    evidence.status = SmokeStatus::Failed;
    evidence.reason = first_non_empty(
        &input.reason,
        DiagnosticReasonCode::UnknownConnectorFailure.as_str(),
    );
    evidence
}

/// Go `firstNonEmpty` (runtime.go helper).
#[must_use]
pub(crate) fn first_non_empty(value: &str, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        fallback.trim().to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-05-07T10:00:00Z")
            .expect("parse")
            .with_timezone(&Utc)
    }

    // Go TestBuildSmokeEvidenceStructuresSkipWhenSafeCredentialsUnavailable
    #[test]
    fn build_smoke_evidence_structures_skip_when_safe_credentials_unavailable() {
        let now = ts();
        let evidence = build_smoke_evidence(SmokeInput {
            smoke_evidence_id: "discord_smoke_1".to_string(),
            tenant_id: "ten_discord".to_string(),
            connector_id: "discord-main".to_string(),
            validated_at: now,
            ..SmokeInput::default()
        });
        assert_eq!(evidence.status, SmokeStatus::Skipped);
        assert_eq!(evidence.credential_mode, CredentialMode::Unavailable);
        assert!(
            !evidence.owner.is_empty(),
            "skip evidence must include owner"
        );
        assert!(
            !evidence.reason.is_empty(),
            "skip evidence must include reason"
        );
        assert!(
            !evidence.remaining_risk.is_empty(),
            "skip evidence must include remaining risk"
        );
        assert_eq!(evidence.retention_expires_at - now, Duration::days(90));
        assert_eq!(evidence.redaction_status, RedactionStatus::Redacted);
    }

    #[test]
    fn build_smoke_evidence_marks_passed_and_failed_with_safe_live_credentials() {
        let now = ts();
        let passed = build_smoke_evidence(SmokeInput {
            smoke_evidence_id: "discord_smoke_2".to_string(),
            tenant_id: "ten_discord".to_string(),
            connector_id: "discord-main".to_string(),
            safe_credentials_approved: true,
            passed: true,
            validated_at: now,
            ..SmokeInput::default()
        });
        assert_eq!(passed.status, SmokeStatus::Passed);
        assert_eq!(passed.credential_mode, CredentialMode::SafeLive);
        assert_eq!(passed.reason, "healthy");
        assert!(passed.remaining_risk.is_empty());

        let failed = build_smoke_evidence(SmokeInput {
            smoke_evidence_id: "discord_smoke_3".to_string(),
            tenant_id: "ten_discord".to_string(),
            connector_id: "discord-main".to_string(),
            safe_credentials_approved: true,
            passed: false,
            validated_at: now,
            ..SmokeInput::default()
        });
        assert_eq!(failed.status, SmokeStatus::Failed);
        assert_eq!(failed.credential_mode, CredentialMode::SafeLive);
        assert_eq!(
            failed.reason,
            DiagnosticReasonCode::UnknownConnectorFailure.as_str()
        );
        assert_eq!(
            failed.remaining_risk,
            "Live Discord hosted smoke was not run."
        );
    }

    #[test]
    fn build_smoke_evidence_applies_defaults() {
        let evidence = build_smoke_evidence(SmokeInput {
            tenant_id: "ten_discord".to_string(),
            connector_id: "discord-main".to_string(),
            ..SmokeInput::default()
        });
        assert_eq!(evidence.smoke_evidence_id, "discord_smoke_discord-main");
        assert_eq!(evidence.owner, "operator");
        assert_eq!(evidence.reason, "safe_credentials_unavailable");
    }
}
