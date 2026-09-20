//! Port of `daemon/internal/evaluation/runtime_recorder.go`: recording a
//! completed non-live replay into the runtime plane — creating a run, driving
//! one evaluation-replay step through planning/execution/completion, and
//! persisting a completed workflow — with quota reservations for the run and
//! workflow launches, and credential-marker redaction of all persisted
//! summaries.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use kura_billing::{
    BillingError, Category, ReserveAllResult, ReserveInput, ReserveResult, ResolveInput,
    UsageReservation, run_operation_key, workflow_operation_key,
};
use kura_identity::tenantctx;

use crate::campaign::is_zero_time;
use crate::error::{BillingReservationError, EvaluationError};
use crate::fixtures::CapturedEvidence;
use crate::types::{ReplayAttempt, ReplayCandidate, SourceKind, SourceRef};
use crate::util::{first_non_empty, new_id};

pub const REPLAY_RUNTIME_ENTRYPOINT: &str = "evaluation.replay";
pub const REPLAY_REDACTED_CREDENTIAL: &str = "[REDACTED]";

pub const REPLAY_CREDENTIAL_LEAK_MARKERS: [&str; 3] =
    ["R37_FAKE_SECRET", "R37_FAKE_TOKEN", "secret-token"];

/// Object-safe boxed future for async trait methods.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Go `RuntimeRecorder` interface.
pub trait RuntimeRecorder: Send + Sync {
    fn record_replay(
        &self,
        input: ReplayRecordInput,
    ) -> BoxFuture<'_, Result<ReplayRecordResult, EvaluationError>>;
}

/// Go `ReplayRecordInput`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReplayRecordInput {
    pub candidate: ReplayCandidate,
    pub attempt: ReplayAttempt,
    pub evidence: CapturedEvidence,
    pub now: DateTime<Utc>,
}

/// Go `ReplayRecordResult`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReplayRecordResult {
    pub run_id: String,
    pub workflow_id: String,
    pub evidence_refs: Vec<SourceRef>,
}

/// Go `ReplayRuntime` interface (sync; the in-memory runtime manager is sync).
pub trait ReplayRuntime: Send + Sync {
    fn create_run(&self, input: kura_runtime::CreateRunInput) -> Result<kura_runtime::Run, String>;
    fn create_step(
        &self,
        run_id: &str,
        input: kura_runtime::CreateStepInput,
    ) -> Result<kura_runtime::Step, String>;
    fn update_step_status_and_reconcile_run(
        &self,
        run_id: &str,
        step_id: &str,
        input: kura_runtime::UpdateStepStatusInput,
    ) -> Result<(kura_runtime::Step, Option<kura_runtime::Run>), String>;
}

impl ReplayRuntime for kura_runtime::Manager {
    fn create_run(&self, input: kura_runtime::CreateRunInput) -> Result<kura_runtime::Run, String> {
        kura_runtime::Manager::create_run(self, input).map_err(|e| e.to_string())
    }

    fn create_step(
        &self,
        run_id: &str,
        input: kura_runtime::CreateStepInput,
    ) -> Result<kura_runtime::Step, String> {
        kura_runtime::Manager::create_step(self, run_id, input).map_err(|e| e.to_string())
    }

    fn update_step_status_and_reconcile_run(
        &self,
        run_id: &str,
        step_id: &str,
        input: kura_runtime::UpdateStepStatusInput,
    ) -> Result<(kura_runtime::Step, Option<kura_runtime::Run>), String> {
        kura_runtime::Manager::update_step_status_and_reconcile_run(self, run_id, step_id, input)
            .map_err(|e| e.to_string())
    }
}

/// Go `ReplayRuntimeStore` interface.
pub trait ReplayRuntimeStore: Send + Sync {
    fn upsert_run(&self, run: kura_runtime::Run) -> Result<(), String>;
    fn upsert_step(&self, step: kura_runtime::Step) -> Result<(), String>;
    fn upsert_workflow(&self, workflow: kura_orchestration::Workflow) -> Result<(), String>;
    fn replace_workflow_steps(
        &self,
        workflow_id: &str,
        steps: Vec<kura_orchestration::WorkflowStep>,
    ) -> Result<(), String>;
}

/// Go `RuntimeReplayRecorder`.
pub struct RuntimeReplayRecorder {
    runtime: Option<Arc<dyn ReplayRuntime>>,
    store: Option<Arc<dyn ReplayRuntimeStore>>,
    billing_manager: Option<Arc<kura_billing::Manager>>,
    hosted_billing: bool,
}

impl RuntimeReplayRecorder {
    /// Go `NewRuntimeReplayRecorder`.
    #[must_use]
    pub fn new(runtime: Arc<dyn ReplayRuntime>, store: Arc<dyn ReplayRuntimeStore>) -> Self {
        RuntimeReplayRecorder {
            runtime: Some(runtime),
            store: Some(store),
            billing_manager: None,
            hosted_billing: false,
        }
    }

    /// Go `ConfigureBilling`.
    pub fn configure_billing(&mut self, manager: Option<Arc<kura_billing::Manager>>, hosted: bool) {
        self.billing_manager = manager;
        self.hosted_billing = hosted;
    }
}

impl RuntimeRecorder for RuntimeReplayRecorder {
    fn record_replay(
        &self,
        input: ReplayRecordInput,
    ) -> BoxFuture<'_, Result<ReplayRecordResult, EvaluationError>> {
        Box::pin(async move { self.record_replay_inner(input).await })
    }
}

impl RuntimeReplayRecorder {
    async fn record_replay_inner(
        &self,
        input: ReplayRecordInput,
    ) -> Result<ReplayRecordResult, EvaluationError> {
        let Some(runtime) = &self.runtime else {
            // Go: nil receiver / nil runtime manager degrades to a no-op result.
            return Ok(ReplayRecordResult::default());
        };
        let input = redact_replay_record_input(input);
        let tenant_id = tenantctx::from_context()
            .map(|tc| tc.tenant_id.trim().to_string())
            .unwrap_or_default();
        let run_id = kura_runtime::new_run_id();
        let workflow_id = new_id("workflow");
        let workflow_step_id = new_id("workflow_step");
        let mut run_reservation = UsageReservation::default();
        let mut workflow_reservation = UsageReservation::default();
        if !tenant_id.is_empty() {
            let run_operation_key = run_operation_key(
                &tenant_id,
                &format!("evaluation:{}", input.attempt.attempt_id),
                "",
            );
            let workflow_operation_key = workflow_operation_key(
                &tenant_id,
                &run_id,
                &workflow_id,
                &format!("evaluation:{}", input.attempt.attempt_id),
            );
            let runtime_reservations = reserve_replay_runtime_quotas(
                &self.billing_manager,
                &tenant_id,
                &run_operation_key,
                &workflow_operation_key,
                self.hosted_billing,
            )
            .await?;
            if let Some(first) = runtime_reservations.results.first() {
                if let Some(reservation) = &first.reservation {
                    run_reservation = reservation.clone();
                }
            }
            if let Some(second) = runtime_reservations.results.get(1) {
                if let Some(reservation) = &second.reservation {
                    workflow_reservation = reservation.clone();
                }
            }
        }
        let run = match runtime.create_run(kura_runtime::CreateRunInput {
            run_id: run_id.clone(),
            entrypoint: REPLAY_RUNTIME_ENTRYPOINT.to_string(),
            goal: replay_run_goal(&input.candidate, &input.attempt),
            ..kura_runtime::CreateRunInput::default()
        }) {
            Ok(run) => run,
            Err(err) => {
                release_replay_runtime_reservation(
                    &self.billing_manager,
                    &workflow_reservation,
                    "evaluation replay runtime run creation failed",
                )
                .await;
                release_replay_runtime_reservation(
                    &self.billing_manager,
                    &run_reservation,
                    "evaluation replay runtime run creation failed",
                )
                .await;
                return Err(EvaluationError::RecordReplay(err));
            }
        };
        if let Err(err) = self.upsert_run(run.clone()) {
            release_replay_runtime_reservation(
                &self.billing_manager,
                &workflow_reservation,
                "evaluation replay runtime run persistence failed",
            )
            .await;
            release_replay_runtime_reservation(
                &self.billing_manager,
                &run_reservation,
                "evaluation replay runtime run persistence failed",
            )
            .await;
            return Err(EvaluationError::RecordReplay(err));
        }
        if let Err(err) = commit_replay_runtime_reservation(
            &self.billing_manager,
            &run_reservation,
            "evaluation replay runtime run persisted",
        )
        .await
        {
            release_replay_runtime_reservation(
                &self.billing_manager,
                &workflow_reservation,
                "evaluation replay runtime run billing commit failed",
            )
            .await;
            return Err(EvaluationError::RecordReplay(err.to_string()));
        }
        let now = if is_zero_time(input.now) {
            Utc::now()
        } else {
            input.now
        };

        let step = match runtime.create_step(
            &run.run_id,
            kura_runtime::CreateStepInput {
                title: "Replay captured evidence".to_string(),
                kind: "evaluation_replay".to_string(),
                workflow_id: workflow_id.clone(),
                workflow_step_id: workflow_step_id.clone(),
                input: Some(serde_json::json!({
                    "candidateId": input.candidate.candidate_id,
                    "attemptId": input.attempt.attempt_id,
                    "mode": input.attempt.mode.as_str(),
                    "changeWindowLabel": input.attempt.change_window_label,
                    "sourceRefs": input.candidate.source_refs,
                    "evidenceRefs": input.attempt.evidence_refs,
                })),
                ..kura_runtime::CreateStepInput::default()
            },
        ) {
            Ok(step) => step,
            Err(err) => {
                release_replay_runtime_reservation(
                    &self.billing_manager,
                    &workflow_reservation,
                    "evaluation replay runtime step creation failed",
                )
                .await;
                return Err(EvaluationError::RecordReplay(err));
            }
        };
        if let Err(err) = self.upsert_step(step.clone()) {
            release_replay_runtime_reservation(
                &self.billing_manager,
                &workflow_reservation,
                "evaluation replay runtime step persistence failed",
            )
            .await;
            return Err(EvaluationError::RecordReplay(err));
        }
        let mut run = run;
        let (step, run_update) = self
            .update_step_status(
                &runtime,
                &run,
                &step,
                kura_runtime::StepStatus::Planning,
                None,
                &workflow_reservation,
                "evaluation replay runtime step planning failed",
            )
            .await?;
        if let Some(updated_run) = run_update {
            run = updated_run;
        }
        let (step, run_update) = self
            .update_step_status(
                &runtime,
                &run,
                &step,
                kura_runtime::StepStatus::ExecutingTool,
                None,
                &workflow_reservation,
                "evaluation replay runtime step execution failed",
            )
            .await?;
        if let Some(updated_run) = run_update {
            run = updated_run;
        }
        let (step, run_update) = self
            .update_step_status(
                &runtime,
                &run,
                &step,
                kura_runtime::StepStatus::Completed,
                Some(serde_json::json!({
                    "runtimeSummary": input.attempt.runtime_summary,
                    "policySummary": input.attempt.policy_summary,
                    "integrationSummary": input.attempt.integration_summary,
                    "deliverySummary": input.attempt.delivery_summary,
                    "evidenceSummary": input.attempt.evidence_summary,
                    "evidence": input.evidence,
                })),
                &workflow_reservation,
                "evaluation replay runtime step completion failed",
            )
            .await?;
        if let Some(updated_run) = run_update {
            run = updated_run;
        }
        let workflow = replay_workflow(
            &input,
            &run.run_id,
            &workflow_id,
            &workflow_step_id,
            &step.step_id,
            now,
        );
        if let Err(err) = self.upsert_workflow(workflow.clone()) {
            release_replay_runtime_reservation(
                &self.billing_manager,
                &workflow_reservation,
                "evaluation replay workflow persistence failed",
            )
            .await;
            return Err(EvaluationError::RecordReplay(err));
        }
        if let Err(err) = self.replace_workflow_steps(&workflow.workflow_id, workflow.steps.clone())
        {
            release_replay_runtime_reservation(
                &self.billing_manager,
                &workflow_reservation,
                "evaluation replay workflow steps persistence failed",
            )
            .await;
            return Err(EvaluationError::RecordReplay(err));
        }
        if let Err(err) = commit_replay_runtime_reservation(
            &self.billing_manager,
            &workflow_reservation,
            "evaluation replay workflow persisted",
        )
        .await
        {
            return Err(EvaluationError::RecordReplay(err.to_string()));
        }
        Ok(ReplayRecordResult {
            run_id: run.run_id.clone(),
            workflow_id: workflow.workflow_id.clone(),
            evidence_refs: vec![
                SourceRef {
                    kind: SourceKind::Run,
                    id: run.run_id.clone(),
                    route: format!("/v1/runs/{}", run.run_id),
                },
                SourceRef {
                    kind: SourceKind::Workflow,
                    id: workflow.workflow_id.clone(),
                    route: format!("/v1/runs/{}/workflows/{}", run.run_id, workflow.workflow_id),
                },
            ],
        })
    }

    /// Drives one step-status transition and reconciles the run, releasing the
    /// workflow reservation on failure. Returns the updated step and the
    /// reconciled run update when the run status changed.
    async fn update_step_status(
        &self,
        runtime: &Arc<dyn ReplayRuntime>,
        run: &kura_runtime::Run,
        step: &kura_runtime::Step,
        status: kura_runtime::StepStatus,
        output: Option<serde_json::Value>,
        workflow_reservation: &UsageReservation,
        failure_reason: &str,
    ) -> Result<(kura_runtime::Step, Option<kura_runtime::Run>), EvaluationError> {
        let (step, run_update) = match runtime.update_step_status_and_reconcile_run(
            &run.run_id,
            &step.step_id,
            kura_runtime::UpdateStepStatusInput { status, output },
        ) {
            Ok(updated) => updated,
            Err(err) => {
                release_replay_runtime_reservation(
                    &self.billing_manager,
                    workflow_reservation,
                    failure_reason,
                )
                .await;
                return Err(EvaluationError::RecordReplay(err));
            }
        };
        if let Err(err) = self.upsert_step(step.clone()) {
            release_replay_runtime_reservation(
                &self.billing_manager,
                workflow_reservation,
                "evaluation replay runtime step persistence failed",
            )
            .await;
            return Err(EvaluationError::RecordReplay(err));
        }
        if let Some(updated_run) = &run_update {
            if let Err(err) = self.upsert_run(updated_run.clone()) {
                return Err(EvaluationError::RecordReplay(err));
            }
        }
        Ok((step, run_update))
    }

    fn upsert_run(&self, run: kura_runtime::Run) -> Result<(), String> {
        let Some(store) = &self.store else {
            return Ok(());
        };
        store
            .upsert_run(run)
            .map_err(|e| format!("upsert run: {e}"))
    }

    fn upsert_step(&self, step: kura_runtime::Step) -> Result<(), String> {
        let Some(store) = &self.store else {
            return Ok(());
        };
        store
            .upsert_step(step)
            .map_err(|e| format!("upsert step: {e}"))
    }

    fn upsert_workflow(&self, workflow: kura_orchestration::Workflow) -> Result<(), String> {
        let Some(store) = &self.store else {
            return Ok(());
        };
        store
            .upsert_workflow(workflow)
            .map_err(|e| format!("upsert workflow: {e}"))
    }

    fn replace_workflow_steps(
        &self,
        workflow_id: &str,
        steps: Vec<kura_orchestration::WorkflowStep>,
    ) -> Result<(), String> {
        let Some(store) = &self.store else {
            return Ok(());
        };
        store
            .replace_workflow_steps(workflow_id, steps)
            .map_err(|e| format!("replace workflow steps {workflow_id}: {e}"))
    }
}

async fn reserve_replay_runtime_quotas(
    manager: &Option<Arc<kura_billing::Manager>>,
    tenant_id: &str,
    run_operation_key: &str,
    workflow_operation_key: &str,
    hosted: bool,
) -> Result<ReserveAllResult, EvaluationError> {
    let Some(manager) = manager else {
        if hosted {
            let denial =
                kura_billing::new_quota_state_unavailable_denial(tenant_id, run_operation_key);
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
        return Ok(ReserveAllResult {
            allowed: true,
            ..Default::default()
        });
    };
    let result = manager
        .reserve_all(vec![
            ReserveInput {
                tenant_id: tenant_id.to_string(),
                category: Category::from(Category::RUN_LAUNCHES),
                amount: 1,
                operation_key: run_operation_key.to_string(),
                reservation_point: "evaluation replay runtime run launch".to_string(),
                guarded_entry_point: "evaluation replay runtime run launch".to_string(),
                hosted,
                ..Default::default()
            },
            ReserveInput {
                tenant_id: tenant_id.to_string(),
                category: Category::from(Category::WORKFLOW_LAUNCHES),
                amount: 1,
                operation_key: workflow_operation_key.to_string(),
                reservation_point: "evaluation replay runtime workflow launch".to_string(),
                guarded_entry_point: "evaluation replay runtime workflow launch".to_string(),
                hosted,
                ..Default::default()
            },
        ])
        .await;
    match result {
        Ok(result) if result.allowed => Ok(result),
        Ok(result) => Err(EvaluationError::BillingReservation(
            BillingReservationError {
                result: result.results.first().cloned().unwrap_or_default(),
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

async fn release_replay_runtime_reservation(
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
            reason_code: "billing.evaluation_replay_runtime_released".to_string(),
            reason: reason.to_string(),
            ..Default::default()
        })
        .await;
}

async fn commit_replay_runtime_reservation(
    manager: &Option<Arc<kura_billing::Manager>>,
    reservation: &UsageReservation,
    reason: &str,
) -> Result<(), BillingError> {
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
            reason_code: "billing.evaluation_replay_runtime_committed".to_string(),
            reason: reason.to_string(),
            ..Default::default()
        })
        .await
        .map(|_| ())
}

/// Go `redactReplayRecordInput`.
#[must_use]
pub fn redact_replay_record_input(mut input: ReplayRecordInput) -> ReplayRecordInput {
    input.attempt.change_window_label =
        redact_replay_credential_string(&input.attempt.change_window_label);
    input.attempt.runtime_summary = redact_replay_credential_string(&input.attempt.runtime_summary);
    input.attempt.policy_summary = redact_replay_credential_string(&input.attempt.policy_summary);
    input.attempt.integration_summary =
        redact_replay_credential_string(&input.attempt.integration_summary);
    input.attempt.delivery_summary =
        redact_replay_credential_string(&input.attempt.delivery_summary);
    input.attempt.evidence_summary =
        redact_replay_credential_string(&input.attempt.evidence_summary);
    input.attempt.blocked_reasons =
        redact_replay_credential_strings(&input.attempt.blocked_reasons);
    input.evidence.runtime_summary =
        redact_replay_credential_string(&input.evidence.runtime_summary);
    input.evidence.policy_summary = redact_replay_credential_string(&input.evidence.policy_summary);
    input.evidence.integration_summary =
        redact_replay_credential_string(&input.evidence.integration_summary);
    input.evidence.delivery_summary =
        redact_replay_credential_string(&input.evidence.delivery_summary);
    input.evidence.evidence_summary =
        redact_replay_credential_string(&input.evidence.evidence_summary);
    input.evidence.blocked_reasons =
        redact_replay_credential_strings(&input.evidence.blocked_reasons);
    input.evidence.limitations = redact_replay_credential_strings(&input.evidence.limitations);
    input
}

/// Go `redactReplayCredentialStrings`.
#[must_use]
pub fn redact_replay_credential_strings(values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|value| redact_replay_credential_string(value))
        .collect()
}

/// Go `redactReplayCredentialString`.
#[must_use]
pub fn redact_replay_credential_string(value: &str) -> String {
    for marker in REPLAY_CREDENTIAL_LEAK_MARKERS {
        if !marker.is_empty() && value.contains(marker) {
            return REPLAY_REDACTED_CREDENTIAL.to_string();
        }
    }
    value.to_string()
}

/// Go `replayRunGoal`.
#[must_use]
pub fn replay_run_goal(candidate: &ReplayCandidate, attempt: &ReplayAttempt) -> String {
    let name = first_non_empty(&[&candidate.display_name, &candidate.candidate_id]);
    if attempt.change_window_label.is_empty() {
        format!("Replay evaluation candidate {name}")
    } else {
        format!(
            "Replay evaluation candidate {name} for {}",
            attempt.change_window_label
        )
    }
}

/// Go `replayWorkflow`.
#[must_use]
pub fn replay_workflow(
    input: &ReplayRecordInput,
    run_id: &str,
    workflow_id: &str,
    workflow_step_id: &str,
    runtime_step_id: &str,
    now: DateTime<Utc>,
) -> kura_orchestration::Workflow {
    let goal = replay_run_goal(&input.candidate, &input.attempt);
    let mut evidence_refs = input.attempt.evidence_refs.clone();
    evidence_refs.extend(input.candidate.captured_evidence_refs.clone());
    let step = kura_orchestration::WorkflowStep {
        workflow_step_id: workflow_step_id.to_string(),
        workflow_id: workflow_id.to_string(),
        title: "Replay captured evidence".to_string(),
        position: 1,
        consumer_kind: "evaluation".to_string(),
        consumer_id: input.candidate.candidate_id.clone(),
        tool_name: REPLAY_RUNTIME_ENTRYPOINT.to_string(),
        input: Some(serde_json::json!({
            "candidateId": input.candidate.candidate_id,
            "attemptId": input.attempt.attempt_id,
            "sourceRefs": input.candidate.source_refs,
            "evidenceRefs": evidence_refs,
        })),
        status: kura_orchestration::StepStatus::Completed,
        runtime_step_id: runtime_step_id.to_string(),
        attempt_count: 1,
        max_attempts: 1,
        output_summary: input.attempt.evidence_summary.clone(),
        created_at: now,
        updated_at: now,
        ..kura_orchestration::WorkflowStep::default()
    };
    kura_orchestration::Workflow {
        workflow_id: workflow_id.to_string(),
        run_id: run_id.to_string(),
        environment_scope: input.candidate.environment_scope.clone(),
        goal,
        status: kura_orchestration::WorkflowStatus::Completed,
        plan_summary: "Replay captured evidence through the evaluation runtime envelope."
            .to_string(),
        created_at: now,
        updated_at: now,
        started_at: Some(now),
        completed_at: Some(now),
        steps: vec![step],
        ..kura_orchestration::Workflow::default()
    }
}
