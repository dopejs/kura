//! Port of daemon/internal/evaluation/manager.go: the evaluation manager —
//! replay candidate/fixture CRUD, live-validation handoff preparation, replay
//! attempt creation with billing quota reservation, and plane-level comparison
//! generation. The store stays behind the [Store] trait (implemented by
//! kura-store's evaluation DAOs in the store crate / tests); billing goes
//! through the kura-billing manager; the optional runtime recorder persists
//! completed non-live replays into the runtime plane.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use kura_billing::{
    BillingError, ReserveInput, ReserveResult, ResolveInput, UsageReservation,
    evaluation_operation_key, new_quota_state_unavailable_denial,
};
use kura_identity::tenantctx;

use crate::comparison::compare_attempt;
use crate::error::{BillingReservationError, EvaluationError};
use crate::fixtures::{
    CapturedEvidence, candidate_from_fixture, candidate_id_for_fixture, load_captured_evidence,
    load_regression_fixtures,
};
use crate::runtime_recorder::{ReplayRecordInput, RuntimeRecorder};
use crate::types::*;
use crate::util::{
    append_reasons, first_non_empty, new_id, replay_mode_default, zero_time_default,
};

/// Go [Store] interface. Implementations map onto kura-store's evaluation
/// DAO methods (see rs/store/src/evaluation.rs).
pub trait Store: Send + Sync {
    fn upsert_replay_candidate(&self, item: ReplayCandidate) -> Result<(), EvaluationError>;
    fn list_replay_candidates(
        &self,
        filter: &CandidateFilter,
    ) -> Result<Vec<ReplayCandidate>, EvaluationError>;
    fn get_replay_candidate(
        &self,
        environment_scope: &str,
        candidate_id: &str,
    ) -> Result<Option<ReplayCandidate>, EvaluationError>;
    fn upsert_replay_attempt(&self, item: ReplayAttempt) -> Result<(), EvaluationError>;
    fn list_replay_attempts(
        &self,
        filter: &AttemptFilter,
    ) -> Result<Vec<ReplayAttempt>, EvaluationError>;
    fn get_replay_attempt(
        &self,
        environment_scope: &str,
        attempt_id: &str,
    ) -> Result<Option<ReplayAttempt>, EvaluationError>;
    fn upsert_comparison_result(&self, item: ComparisonResult) -> Result<(), EvaluationError>;
    fn list_comparison_results(
        &self,
        filter: &ComparisonFilter,
    ) -> Result<Vec<ComparisonResult>, EvaluationError>;
    fn get_comparison_result(
        &self,
        environment_scope: &str,
        comparison_id: &str,
    ) -> Result<Option<ComparisonResult>, EvaluationError>;
    fn upsert_regression_fixture(&self, item: RegressionFixture) -> Result<(), EvaluationError>;
    fn list_regression_fixtures(
        &self,
        filter: &FixtureFilter,
    ) -> Result<Vec<RegressionFixture>, EvaluationError>;
}

/// Go [Dependencies].
#[derive(Default)]
pub struct Dependencies {
    pub environment_scope: String,
    pub store: Option<Arc<dyn Store>>,
    pub fixtures_dir: String,
    pub runtime_recorder: Option<Arc<dyn RuntimeRecorder>>,
    pub billing: Option<Arc<kura_billing::Manager>>,
    pub hosted_billing: bool,
    pub clock: Option<Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>>,
}

/// Go [Manager].
pub struct Manager {
    environment_scope: String,
    store: Option<Arc<dyn Store>>,
    fixtures_dir: String,
    runtime_recorder: Option<Arc<dyn RuntimeRecorder>>,
    billing_manager: Option<Arc<kura_billing::Manager>>,
    hosted_billing: bool,
    clock: Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>,
}

impl Manager {
    /// Go [NewManager].
    #[must_use]
    pub fn new(deps: Dependencies) -> Self {
        let clock = deps.clock.unwrap_or_else(|| Arc::new(Utc::now));
        Manager {
            environment_scope: first_non_empty(&[&deps.environment_scope, "test"]),
            store: deps.store,
            fixtures_dir: deps.fixtures_dir,
            runtime_recorder: deps.runtime_recorder,
            billing_manager: deps.billing,
            hosted_billing: deps.hosted_billing,
            clock,
        }
    }

    fn store(&self) -> Result<Arc<dyn Store>, EvaluationError> {
        self.store
            .clone()
            .ok_or(EvaluationError::StoreNotConfigured)
    }

    fn clock_now(&self) -> DateTime<Utc> {
        (self.clock)()
    }

    /// Go [LoadFixtures].
    pub fn load_fixtures(&self) -> Result<(), EvaluationError> {
        if self.fixtures_dir.trim().is_empty() {
            return Ok(());
        }
        let store = self.store()?;
        let fixtures = load_regression_fixtures(&self.fixtures_dir, &self.environment_scope)?;
        for mut fixture in fixtures {
            let now = self.clock_now();
            fixture.created_at = zero_time_default(fixture.created_at, now);
            fixture.updated_at = zero_time_default(fixture.updated_at, now);
            fixture.candidate_id = candidate_id_for_fixture(&fixture.fixture_id);
            store.upsert_regression_fixture(fixture.clone())?;
            let candidate = candidate_from_fixture(fixture, now);
            store.upsert_replay_candidate(candidate)?;
        }
        Ok(())
    }

    /// Go [UpsertReplayCandidate].
    pub fn upsert_replay_candidate(
        &self,
        mut candidate: ReplayCandidate,
    ) -> Result<(), EvaluationError> {
        let now = self.clock_now();
        candidate.environment_scope =
            first_non_empty(&[&candidate.environment_scope, &self.environment_scope]);
        candidate.default_replay_mode = replay_mode_default(Some(candidate.default_replay_mode));
        candidate.created_at = zero_time_default(candidate.created_at, now);
        candidate.updated_at = zero_time_default(candidate.updated_at, now);
        if candidate.readiness_status.as_str().is_empty() {
            candidate.readiness_status = ReadinessStatus::FullyReplayable;
        }
        if candidate.candidate_id.is_empty() {
            candidate.candidate_id = new_id("replay_candidate");
        }
        if candidate.candidate_kind.as_str().is_empty() {
            candidate.candidate_kind = CandidateKind::CuratedWork;
        }
        validate_replay_candidate(&candidate)?;
        let candidate = normalize_replay_candidate(candidate);
        self.store()?.upsert_replay_candidate(candidate)
    }

    /// Go [ListReplayCandidates].
    pub fn list_replay_candidates(
        &self,
        filter: &CandidateFilter,
    ) -> Result<Vec<ReplayCandidate>, EvaluationError> {
        let store = self.store()?;
        let mut filter = filter.clone();
        filter.environment_scope =
            first_non_empty(&[&filter.environment_scope, &self.environment_scope]);
        let mut items = store.list_replay_candidates(&filter)?;
        items.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.candidate_id.cmp(&b.candidate_id))
        });
        Ok(normalize_replay_candidates(limit_candidates(
            items,
            filter.limit,
        )))
    }

    /// Go [GetReplayCandidate].
    pub fn get_replay_candidate(
        &self,
        candidate_id: &str,
    ) -> Result<Option<ReplayCandidate>, EvaluationError> {
        let store = self.store()?;
        let item = store.get_replay_candidate(&self.environment_scope, candidate_id)?;
        Ok(item.map(normalize_replay_candidate))
    }

    /// Go [PrepareLiveValidationHandoff].
    pub fn prepare_live_validation_handoff(
        &self,
        candidate_id: &str,
    ) -> Result<ReplayCandidate, EvaluationError> {
        let candidate = self
            .get_replay_candidate(candidate_id)?
            .ok_or_else(|| EvaluationError::CandidateNotFound(candidate_id.to_string()))?;
        Ok(candidate)
    }

    /// Go [CreateReplayAttempt]. Async because billing reservation and the
    /// runtime recorder are async.
    pub async fn create_replay_attempt(
        &self,
        candidate_id: &str,
        input: CreateReplayAttemptInput,
    ) -> Result<ReplayAttempt, EvaluationError> {
        let store = self.store()?;
        let candidate = store
            .get_replay_candidate(&self.environment_scope, candidate_id)?
            .ok_or_else(|| EvaluationError::CandidateNotFound(candidate_id.to_string()))?;
        let candidate = normalize_replay_candidate(candidate);
        let now = self.clock_now();
        let mode = replay_mode_default(input.mode);
        let attempt_id = new_id("replay_attempt");
        let mut reservation = UsageReservation::default();
        if let Some(tenant_context) = tenantctx::from_context() {
            if !tenant_context.tenant_id.is_empty() {
                let result = reserve_evaluation_attempt_quota(
                    &self.billing_manager,
                    &tenant_context.tenant_id,
                    &candidate.candidate_id,
                    &attempt_id,
                    self.hosted_billing,
                )
                .await?;
                if let Some(res) = result.reservation {
                    reservation = res;
                }
            }
        }
        let (evidence, evidence_error) = match self.captured_evidence_for_candidate(&candidate) {
            Ok(evidence) => (evidence, None),
            Err(err) => (CapturedEvidence::default(), Some(err)),
        };
        let expected = candidate.expected_comparison.clone().unwrap_or_default();
        let mut attempt = ReplayAttempt {
            attempt_id: attempt_id.clone(),
            candidate_id: candidate.candidate_id.clone(),
            source_refs: candidate.source_refs.clone(),
            environment_scope: candidate.environment_scope.clone(),
            mode,
            status: ReplayAttemptStatus::Completed,
            safety_scope: input.safety_scope.clone().unwrap_or_default(),
            approval_handling: ApprovalHandling::EvidenceOnly,
            side_effect_handling: SideEffectHandling::EvidenceOnly,
            launched_by: input.launched_by.clone(),
            change_window_label: input.change_window_label.clone(),
            baseline_attempt_id: input.baseline_attempt_id.clone(),
            evidence_refs: candidate.captured_evidence_refs.clone(),
            runtime_summary: first_non_empty(&[
                &evidence.runtime_summary,
                &expected.runtime,
                "replay evidence completed",
            ]),
            policy_summary: first_non_empty(&[
                &evidence.policy_summary,
                &expected.policy,
                "policy evidence preserved",
            ]),
            integration_summary: first_non_empty(&[
                &evidence.integration_summary,
                &expected.integration,
                "integration evidence preserved",
            ]),
            delivery_summary: first_non_empty(&[
                &evidence.delivery_summary,
                &expected.delivery,
                "delivery evidence preserved",
            ]),
            evidence_summary: first_non_empty(&[
                &evidence.evidence_summary,
                &expected.evidence,
                "replay evidence captured",
            ]),
            result_run_id: evidence.result_run_id.clone(),
            result_workflow_id: evidence.result_workflow_id.clone(),
            started_at: Some(now),
            completed_at: Some(now),
            created_at: now,
            updated_at: now,
            ..ReplayAttempt::default()
        };
        if evidence.terminal_status != ReplayAttemptStatus::default() {
            attempt.status = evidence.terminal_status;
        }
        if let Some(evidence_err) = &evidence_error {
            if candidate.candidate_kind == CandidateKind::Fixture {
                attempt.status = ReplayAttemptStatus::Unreplayable;
                attempt.approval_handling = ApprovalHandling::Blocked;
                attempt.side_effect_handling = SideEffectHandling::Blocked;
                attempt.blocked_reasons = vec![evidence_err.to_string()];
            }
        }
        attempt
            .blocked_reasons
            .extend(evidence.blocked_reasons.clone());
        attempt.blocked_reasons.extend(evidence.limitations.clone());
        if mode == ReplayMode::LiveValidation {
            attempt.status = ReplayAttemptStatus::Blocked;
            attempt.approval_handling = ApprovalHandling::Blocked;
            attempt.side_effect_handling = SideEffectHandling::Blocked;
            attempt.blocked_reasons.push(
                "live validation requires an explicit live side-effect executor and approval flow before side effects can run"
                    .to_string(),
            );
        }
        if candidate.readiness_status == ReadinessStatus::Blocked {
            attempt.status = ReplayAttemptStatus::Blocked;
            attempt.approval_handling = ApprovalHandling::Blocked;
            attempt.side_effect_handling = SideEffectHandling::Blocked;
            attempt.blocked_reasons =
                append_reasons(&candidate.readiness_reasons, &candidate.limitations);
        }
        if candidate.readiness_status == ReadinessStatus::Unreplayable {
            attempt.status = ReplayAttemptStatus::Unreplayable;
            attempt.approval_handling = ApprovalHandling::Blocked;
            attempt.side_effect_handling = SideEffectHandling::Blocked;
            attempt.blocked_reasons =
                append_reasons(&candidate.readiness_reasons, &candidate.limitations);
        }
        if attempt.safety_scope.mode.as_str().is_empty() {
            attempt.safety_scope.mode = mode;
        }
        if attempt.status == ReplayAttemptStatus::Completed && attempt.mode == ReplayMode::NonLive {
            if let Some(recorder) = &self.runtime_recorder {
                let record = recorder
                    .record_replay(ReplayRecordInput {
                        candidate: candidate.clone(),
                        attempt: attempt.clone(),
                        evidence: evidence.clone(),
                        now,
                    })
                    .await;
                match record {
                    Ok(record) => {
                        attempt.result_run_id =
                            first_non_empty(&[&record.run_id, &attempt.result_run_id]);
                        attempt.result_workflow_id =
                            first_non_empty(&[&record.workflow_id, &attempt.result_workflow_id]);
                        attempt.evidence_refs.extend(record.evidence_refs);
                    }
                    Err(err) => {
                        attempt.status = ReplayAttemptStatus::Failed;
                        attempt.completed_at = Some(self.clock_now());
                        attempt.updated_at =
                            attempt.completed_at.unwrap_or_else(|| self.clock_now());
                        attempt
                            .blocked_reasons
                            .push(format!("record replay runtime run: {err}"));
                    }
                }
            }
        }
        let attempt = normalize_replay_attempt(attempt);
        if let Err(err) = store.upsert_replay_attempt(attempt.clone()) {
            release_evaluation_attempt_reservation(
                &self.billing_manager,
                &reservation,
                "replay attempt persistence failed before accepted attempt",
            )
            .await;
            return Err(err);
        }
        if let Err(err) = commit_evaluation_attempt_reservation(
            &self.billing_manager,
            &reservation,
            "replay attempt persisted",
        )
        .await
        {
            return Err(err);
        }
        let mut candidate = candidate;
        candidate.latest_attempt_id = attempt.attempt_id.clone();
        candidate.updated_at = now;
        store.upsert_replay_candidate(candidate)?;
        Ok(attempt)
    }

    /// Go [ListReplayAttempts].
    pub fn list_replay_attempts(
        &self,
        filter: &AttemptFilter,
    ) -> Result<Vec<ReplayAttempt>, EvaluationError> {
        let store = self.store()?;
        let mut filter = filter.clone();
        filter.environment_scope =
            first_non_empty(&[&filter.environment_scope, &self.environment_scope]);
        let mut items = store.list_replay_attempts(&filter)?;
        items.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.attempt_id.cmp(&b.attempt_id))
        });
        Ok(normalize_replay_attempts(limit_attempts(
            items,
            filter.limit,
        )))
    }

    /// Go [GetReplayAttempt].
    pub fn get_replay_attempt(
        &self,
        attempt_id: &str,
    ) -> Result<Option<ReplayAttempt>, EvaluationError> {
        let store = self.store()?;
        let item = store.get_replay_attempt(&self.environment_scope, attempt_id)?;
        Ok(item.map(normalize_replay_attempt))
    }

    /// Go [CreateComparison].
    pub fn create_comparison(
        &self,
        attempt_id: &str,
        input: CreateComparisonInput,
    ) -> Result<ComparisonResult, EvaluationError> {
        let store = self.store()?;
        let attempt = store
            .get_replay_attempt(&self.environment_scope, attempt_id)?
            .ok_or_else(|| EvaluationError::AttemptNotFound(attempt_id.to_string()))?;
        let attempt = normalize_replay_attempt(attempt);
        let candidate = store
            .get_replay_candidate(&self.environment_scope, &attempt.candidate_id)?
            .ok_or_else(|| EvaluationError::CandidateNotFound(attempt.candidate_id.clone()))?;
        let candidate = normalize_replay_candidate(candidate);
        let baseline = if input.baseline_attempt_id.trim().is_empty() {
            None
        } else {
            let item = store
                .get_replay_attempt(&self.environment_scope, &input.baseline_attempt_id)?
                .ok_or_else(|| {
                    EvaluationError::BaselineAttemptNotFound(input.baseline_attempt_id.clone())
                })?;
            Some(normalize_replay_attempt(item))
        };
        let comparison = normalize_comparison(compare_attempt(
            &candidate,
            baseline.as_ref(),
            &attempt,
            &input,
            self.clock_now(),
        ));
        store.upsert_comparison_result(comparison.clone())?;
        let mut candidate = candidate;
        candidate.latest_comparison_id = comparison.comparison_id.clone();
        candidate.updated_at = comparison.generated_at;
        store.upsert_replay_candidate(candidate)?;
        Ok(comparison)
    }

    /// Go [capturedEvidenceForCandidate] (manager.go).
    fn captured_evidence_for_candidate(
        &self,
        candidate: &ReplayCandidate,
    ) -> Result<CapturedEvidence, EvaluationError> {
        if candidate.fixture_id.trim().is_empty() {
            return Ok(CapturedEvidence::default());
        }
        let store = self.store()?;
        let fixtures = store.list_regression_fixtures(&FixtureFilter {
            environment_scope: candidate.environment_scope.clone(),
            ..FixtureFilter::default()
        })?;
        for fixture in fixtures {
            if fixture.fixture_id == candidate.fixture_id {
                return load_captured_evidence(&fixture);
            }
        }
        Err(EvaluationError::FixtureNotFound(
            candidate.fixture_id.clone(),
            candidate.candidate_id.clone(),
        ))
    }

    /// Go [ListComparisons].
    pub fn list_comparisons(
        &self,
        filter: &ComparisonFilter,
    ) -> Result<Vec<ComparisonResult>, EvaluationError> {
        let store = self.store()?;
        let mut filter = filter.clone();
        filter.environment_scope =
            first_non_empty(&[&filter.environment_scope, &self.environment_scope]);
        let mut items = store.list_comparison_results(&filter)?;
        items.sort_by(|a, b| {
            a.generated_at
                .cmp(&b.generated_at)
                .then_with(|| a.comparison_id.cmp(&b.comparison_id))
        });
        Ok(normalize_comparisons(limit_comparisons(
            items,
            filter.limit,
        )))
    }

    /// Go [GetComparison].
    pub fn get_comparison(
        &self,
        comparison_id: &str,
    ) -> Result<Option<ComparisonResult>, EvaluationError> {
        let store = self.store()?;
        let item = store.get_comparison_result(&self.environment_scope, comparison_id)?;
        Ok(item.map(normalize_comparison))
    }

    /// Go [ListFixtures].
    pub fn list_fixtures(
        &self,
        filter: &FixtureFilter,
    ) -> Result<Vec<RegressionFixture>, EvaluationError> {
        let store = self.store()?;
        let mut filter = filter.clone();
        filter.environment_scope =
            first_non_empty(&[&filter.environment_scope, &self.environment_scope]);
        let mut items = store.list_regression_fixtures(&filter)?;
        items.sort_by(|a, b| {
            a.domain_class
                .cmp(&b.domain_class)
                .then_with(|| a.fixture_id.cmp(&b.fixture_id))
        });
        Ok(normalize_fixtures(limit_fixtures(items, filter.limit)))
    }
}

/// Go [reserveEvaluationAttemptQuota] (manager.go).
async fn reserve_evaluation_attempt_quota(
    manager: &Option<Arc<kura_billing::Manager>>,
    tenant_id: &str,
    candidate_id: &str,
    attempt_id: &str,
    hosted: bool,
) -> Result<ReserveResult, EvaluationError> {
    let operation_key = evaluation_operation_key(tenant_id, candidate_id, attempt_id, "");
    let Some(manager) = manager else {
        if hosted {
            let denial = new_quota_state_unavailable_denial(tenant_id, &operation_key);
            return Err(EvaluationError::BillingReservation(
                BillingReservationError {
                    result: ReserveResult {
                        allowed: false,
                        denial: Some(denial),
                        failure: Some(BillingError::QuotaStateUnavailable),
                        ..Default::default()
                    },
                    error: BillingError::QuotaStateUnavailable,
                },
            ));
        }
        return Ok(ReserveResult {
            allowed: true,
            ..Default::default()
        });
    };
    let result = manager
        .reserve(ReserveInput {
            tenant_id: tenant_id.to_string(),
            category: kura_billing::Category::from(
                kura_billing::Category::REPLAY_EVALUATION_ATTEMPTS,
            ),
            amount: 1,
            operation_key,
            reservation_point: "replay/evaluation attempt creation before work starts".to_string(),
            guarded_entry_point: "POST /v1/evaluation/replay-candidates/{candidateId}/attempts"
                .to_string(),
            hosted,
            ..Default::default()
        })
        .await;
    match result {
        Ok(result) if result.failure.is_none() => Ok(result),
        Ok(result) => Err(EvaluationError::BillingReservation(
            BillingReservationError {
                result: result.clone(),
                error: result.failure.clone().unwrap_or(BillingError::QuotaDenied),
            },
        )),
        Err(err) => Err(EvaluationError::BillingReservation(
            BillingReservationError {
                result: ReserveResult::default(),
                error: err,
            },
        )),
    }
}

/// Go [releaseEvaluationAttemptReservation] (manager.go).
async fn release_evaluation_attempt_reservation(
    manager: &Option<Arc<kura_billing::Manager>>,
    reservation: &UsageReservation,
    reason: &str,
) {
    let Some(manager) = manager else {
        return;
    };
    if reservation.reservation_id.trim().is_empty() {
        return;
    }
    let _ = manager
        .release(ResolveInput {
            tenant_id: reservation.tenant_id.clone(),
            category: reservation.category.clone(),
            operation_key: reservation.operation_key.clone(),
            amount: reservation.amount_reserved,
            reason_code: "billing.replay_evaluation_attempt_released".to_string(),
            reason: reason.to_string(),
            ..Default::default()
        })
        .await;
}

/// Go [commitEvaluationAttemptReservation] (manager.go).
async fn commit_evaluation_attempt_reservation(
    manager: &Option<Arc<kura_billing::Manager>>,
    reservation: &UsageReservation,
    reason: &str,
) -> Result<(), EvaluationError> {
    let Some(manager) = manager else {
        return Ok(());
    };
    if reservation.reservation_id.trim().is_empty() {
        return Ok(());
    }
    manager
        .commit(ResolveInput {
            tenant_id: reservation.tenant_id.clone(),
            category: reservation.category.clone(),
            operation_key: reservation.operation_key.clone(),
            amount: reservation.amount_reserved,
            reason_code: "billing.replay_evaluation_attempt_committed".to_string(),
            reason: reason.to_string(),
            ..Default::default()
        })
        .await
        .map(|_| ())
        .map_err(EvaluationError::BillingLifecycle)
}

fn normalize_replay_candidates(items: Vec<ReplayCandidate>) -> Vec<ReplayCandidate> {
    items.into_iter().map(normalize_replay_candidate).collect()
}

fn normalize_replay_candidate(mut item: ReplayCandidate) -> ReplayCandidate {
    if item.source_refs.is_empty() {
        item.source_refs = Vec::new();
    }
    if item.tool_classes.is_empty() {
        item.tool_classes = Vec::new();
    }
    if item.readiness_reasons.is_empty() {
        item.readiness_reasons = Vec::new();
    }
    if item.limitations.is_empty() {
        item.limitations = Vec::new();
    }
    if item.captured_evidence_refs.is_empty() {
        item.captured_evidence_refs = Vec::new();
    }
    item
}

fn normalize_replay_attempts(items: Vec<ReplayAttempt>) -> Vec<ReplayAttempt> {
    items.into_iter().map(normalize_replay_attempt).collect()
}

fn normalize_replay_attempt(mut item: ReplayAttempt) -> ReplayAttempt {
    if item.source_refs.is_empty() {
        item.source_refs = Vec::new();
    }
    if item.evidence_refs.is_empty() {
        item.evidence_refs = Vec::new();
    }
    if item.blocked_reasons.is_empty() {
        item.blocked_reasons = Vec::new();
    }
    item
}

fn normalize_comparisons(items: Vec<ComparisonResult>) -> Vec<ComparisonResult> {
    items.into_iter().map(normalize_comparison).collect()
}

fn normalize_comparison(mut item: ComparisonResult) -> ComparisonResult {
    if item.limitations.is_empty() {
        item.limitations = Vec::new();
    }
    if item.drift_findings.is_empty() {
        item.drift_findings = Vec::new();
    }
    for finding in &mut item.drift_findings {
        if finding.evidence_refs.is_empty() {
            finding.evidence_refs = Vec::new();
        }
    }
    item
}

fn normalize_fixtures(items: Vec<RegressionFixture>) -> Vec<RegressionFixture> {
    items
        .into_iter()
        .map(|mut item| {
            if item.source_refs.is_empty() {
                item.source_refs = Vec::new();
            }
            if item.captured_evidence_refs.is_empty() {
                item.captured_evidence_refs = Vec::new();
            }
            if item.assumptions.is_empty() {
                item.assumptions = Vec::new();
            }
            if item.limitations.is_empty() {
                item.limitations = Vec::new();
            }
            item
        })
        .collect()
}

/// Go [validateReplayCandidate] (manager.go).
fn validate_replay_candidate(candidate: &ReplayCandidate) -> Result<(), EvaluationError> {
    if candidate.candidate_id.trim().is_empty() {
        return Err(EvaluationError::CandidateIdRequired);
    }
    if candidate.candidate_kind != CandidateKind::CuratedWork
        && candidate.candidate_kind != CandidateKind::Fixture
    {
        return Err(EvaluationError::UnsupportedCandidateKind(
            candidate.candidate_kind.as_str().to_string(),
        ));
    }
    if candidate.display_name.trim().is_empty() {
        return Err(EvaluationError::DisplayNameRequired);
    }
    if candidate.source_kind.as_str().is_empty() {
        return Err(EvaluationError::SourceKindRequired);
    }
    if candidate.source_id.trim().is_empty() {
        return Err(EvaluationError::SourceIdRequired);
    }
    if candidate.source_refs.is_empty() {
        return Err(EvaluationError::SourceRefsRequired);
    }
    for (idx, reference) in candidate.source_refs.iter().enumerate() {
        if reference.kind.as_str().is_empty() || reference.id.trim().is_empty() {
            return Err(EvaluationError::SourceRefInvalid(idx));
        }
    }
    if candidate.readiness_status == ReadinessStatus::Blocked
        || candidate.readiness_status == ReadinessStatus::Unreplayable
    {
        if candidate.readiness_reasons.is_empty() && candidate.limitations.is_empty() {
            return Err(EvaluationError::BlockedCandidateRequiresReasons);
        }
    }
    if candidate.default_replay_mode != ReplayMode::NonLive {
        return Err(EvaluationError::DefaultReplayModeInvalid);
    }
    Ok(())
}

fn limit_candidates(mut items: Vec<ReplayCandidate>, limit: i64) -> Vec<ReplayCandidate> {
    if limit > 0 && items.len() > limit as usize {
        items.truncate(limit as usize);
    }
    items
}

fn limit_attempts(mut items: Vec<ReplayAttempt>, limit: i64) -> Vec<ReplayAttempt> {
    if limit > 0 && items.len() > limit as usize {
        items.truncate(limit as usize);
    }
    items
}

fn limit_comparisons(mut items: Vec<ComparisonResult>, limit: i64) -> Vec<ComparisonResult> {
    if limit > 0 && items.len() > limit as usize {
        items.truncate(limit as usize);
    }
    items
}

fn limit_fixtures(mut items: Vec<RegressionFixture>, limit: i64) -> Vec<RegressionFixture> {
    if limit > 0 && items.len() > limit as usize {
        items.truncate(limit as usize);
    }
    items
}
