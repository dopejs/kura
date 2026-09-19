//! Port of `daemon/internal/evaluation/fixtures.go`: loading repo-managed
//! regression fixtures from disk, reading their captured evidence, and
//! deriving the fixture-backed replay candidates.

use std::path::Path;

use chrono::{DateTime, Utc};

use crate::error::EvaluationError;
use crate::types::{
    CandidateKind, FixtureDomainClass, ReadinessStatus, RegressionFixture, ReplayAttemptStatus,
    ReplayCandidate, ReplayMode, SourceKind,
};
use crate::util::{first_non_empty, replay_mode_default, zero_time_default};

/// Go `CapturedEvidence` (fixtures.go). Field names carry explicit renames:
/// the Go JSON tags for the summary planes are single words ("runtime",
/// "policy", ...), not the camelCase of the Go field names.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapturedEvidence {
    pub terminal_status: ReplayAttemptStatus,
    #[serde(default, skip_serializing_if = "String::is_empty", rename = "runtime")]
    pub runtime_summary: String,
    #[serde(default, skip_serializing_if = "String::is_empty", rename = "policy")]
    pub policy_summary: String,
    #[serde(
        default,
        skip_serializing_if = "String::is_empty",
        rename = "integration"
    )]
    pub integration_summary: String,
    #[serde(default, skip_serializing_if = "String::is_empty", rename = "delivery")]
    pub delivery_summary: String,
    #[serde(default, skip_serializing_if = "String::is_empty", rename = "evidence")]
    pub evidence_summary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub result_run_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub result_workflow_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_reasons: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limitations: Vec<String>,
}

/// Go `LoadRegressionFixtures`.
pub fn load_regression_fixtures(
    root_dir: &str,
    environment_scope: &str,
) -> Result<Vec<RegressionFixture>, EvaluationError> {
    let entries =
        std::fs::read_dir(root_dir).map_err(|e| EvaluationError::ReadFixturesDir(e.to_string()))?;
    let now = Utc::now();
    let mut fixtures = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| EvaluationError::ReadFixturesDir(e.to_string()))?;
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let manifest_path = Path::new(root_dir)
            .join(entry.file_name())
            .join("manifest.json");
        let manifest_path = manifest_path.to_string_lossy().to_string();
        let raw = std::fs::read_to_string(&manifest_path).map_err(|e| {
            EvaluationError::ReadFixtureManifest(manifest_path.clone(), e.to_string())
        })?;
        let mut fixture: RegressionFixture = serde_json::from_str(&raw).map_err(|e| {
            EvaluationError::DecodeFixtureManifest(manifest_path.clone(), e.to_string())
        })?;
        fixture.manifest_path = manifest_path.clone();
        fixture.environment_scope =
            first_non_empty(&[&fixture.environment_scope, environment_scope]);
        fixture.expected_replay_mode = replay_mode_default(Some(fixture.expected_replay_mode));
        fixture.created_at = zero_time_default(fixture.created_at, now);
        fixture.updated_at = zero_time_default(fixture.updated_at, now);
        fixture.candidate_id = candidate_id_for_fixture(&fixture.fixture_id);
        validate_fixture(&fixture)
            .map_err(|e| EvaluationError::ValidateFixture(manifest_path.clone(), e))?;
        fixtures.push(fixture);
    }
    fixtures.sort_by(|a, b| {
        a.domain_class
            .cmp(&b.domain_class)
            .then_with(|| a.fixture_id.cmp(&b.fixture_id))
    });
    Ok(fixtures)
}

/// Go `LoadCapturedEvidence`.
pub fn load_captured_evidence(
    fixture: &RegressionFixture,
) -> Result<CapturedEvidence, EvaluationError> {
    if fixture.captured_evidence_refs.is_empty() {
        return Err(EvaluationError::CapturedEvidenceMissing(
            fixture.fixture_id.clone(),
        ));
    }
    let evidence_ref = &fixture.captured_evidence_refs[0];
    let mut evidence_path = if evidence_ref.route.is_empty() {
        evidence_ref.id.clone()
    } else {
        evidence_ref.route.clone()
    };
    let evidence_path_abs = Path::new(&evidence_path).is_absolute();
    if !evidence_path_abs {
        let manifest_dir = Path::new(&fixture.manifest_path)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default();
        let candidate = manifest_dir.join(&evidence_path);
        if candidate.exists() {
            evidence_path = candidate.to_string_lossy().to_string();
        } else if !evidence_ref.id.is_empty() {
            let id_path = manifest_dir.join(&evidence_ref.id);
            if id_path.exists() {
                evidence_path = id_path.to_string_lossy().to_string();
            } else {
                let base = Path::new(&evidence_ref.id)
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_default();
                if base != "." && !base.is_empty() {
                    let base_path = manifest_dir.join(&base);
                    if base_path.exists() {
                        evidence_path = base_path.to_string_lossy().to_string();
                    }
                }
            }
        }
    }
    let raw = std::fs::read_to_string(&evidence_path)
        .map_err(|e| EvaluationError::ReadCapturedEvidence(evidence_path.clone(), e.to_string()))?;
    let mut evidence: CapturedEvidence = serde_json::from_str(&raw).map_err(|e| {
        EvaluationError::DecodeCapturedEvidence(evidence_path.clone(), e.to_string())
    })?;
    if evidence.terminal_status.as_str().is_empty() {
        evidence.terminal_status = ReplayAttemptStatus::Completed;
    }
    Ok(evidence)
}

/// Go `CandidateFromFixture`.
#[must_use]
pub fn candidate_from_fixture(fixture: RegressionFixture, now: DateTime<Utc>) -> ReplayCandidate {
    let mut readiness = ReadinessStatus::FullyReplayable;
    let mut reasons =
        vec!["fixture has captured evidence and expected comparison summaries".to_string()];
    if !fixture.limitations.is_empty() {
        readiness = ReadinessStatus::PartiallyReplayable;
        reasons.extend(fixture.limitations.iter().cloned());
    }
    ReplayCandidate {
        candidate_id: candidate_id_for_fixture(&fixture.fixture_id),
        candidate_kind: CandidateKind::Fixture,
        display_name: fixture.display_name,
        source_kind: SourceKind::Fixture,
        source_id: fixture.fixture_id.clone(),
        source_refs: fixture.source_refs.clone(),
        environment_scope: fixture.environment_scope.clone(),
        readiness_status: readiness,
        readiness_reasons: reasons,
        limitations: fixture.limitations.clone(),
        default_replay_mode: ReplayMode::NonLive,
        fixture_id: fixture.fixture_id.clone(),
        expected_comparison: Some(fixture.expected_comparison_summary),
        captured_evidence_refs: fixture.captured_evidence_refs.clone(),
        created_at: zero_time_default(fixture.created_at, now),
        updated_at: zero_time_default(fixture.updated_at, now),
        ..ReplayCandidate::default()
    }
}

fn validate_fixture(fixture: &RegressionFixture) -> Result<(), String> {
    if fixture.fixture_id.is_empty() {
        return Err("fixtureId is required".to_string());
    }
    if fixture.display_name.is_empty() {
        return Err("displayName is required".to_string());
    }
    if !matches!(
        fixture.domain_class,
        FixtureDomainClass::Schedule
            | FixtureDomainClass::Integration
            | FixtureDomainClass::ComputerUse
    ) {
        return Err(format!(
            "unsupported domainClass {:?}",
            fixture.domain_class.as_str()
        ));
    }
    if fixture.source_refs.is_empty() {
        return Err("sourceRefs are required".to_string());
    }
    if fixture.captured_evidence_refs.is_empty() {
        return Err("capturedEvidenceRefs are required".to_string());
    }
    if fixture.expected_comparison_summary.runtime.is_empty()
        || fixture.expected_comparison_summary.evidence.is_empty()
    {
        return Err("expectedComparisonSummary runtime and evidence are required".to_string());
    }
    Ok(())
}

/// Go `candidateIDForFixture`.
#[must_use]
pub fn candidate_id_for_fixture(fixture_id: &str) -> String {
    format!("candidate_{fixture_id}")
}
