//! Port of daemon/internal/evaluation/runtime_recorder_test.go: credential
//! redaction in the runtime replay artifacts and manager integration that
//! records a completed non-live replay into the runtime plane.

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use kura_evaluation::{
    CandidateKind, CreateReplayAttemptInput, Dependencies, Manager, PlaneSummaries,
    ReadinessStatus, ReplayAttempt, ReplayCandidate, ReplayMode, ReplayRecordInput, ReplayRuntime,
    ReplayRuntimeStore, RuntimeRecorder, RuntimeReplayRecorder, SourceKind, SourceRef,
    redact_replay_credential_string, redact_replay_credential_strings,
};
use kura_runtime::Manager as RuntimeManager;

use common::fixed_now;

#[derive(Debug, Default)]
struct MemoryRuntimeStore {
    runs: parking_lot::Mutex<HashMap<String, kura_runtime::Run>>,
    steps: parking_lot::Mutex<HashMap<String, kura_runtime::Step>>,
    workflows: parking_lot::Mutex<HashMap<String, kura_orchestration::Workflow>>,
}

impl ReplayRuntimeStore for MemoryRuntimeStore {
    fn upsert_run(&self, run: kura_runtime::Run) -> Result<(), String> {
        self.runs.lock().insert(run.run_id.clone(), run);
        Ok(())
    }
    fn upsert_step(&self, step: kura_runtime::Step) -> Result<(), String> {
        self.steps.lock().insert(step.step_id.clone(), step);
        Ok(())
    }
    fn upsert_workflow(&self, workflow: kura_orchestration::Workflow) -> Result<(), String> {
        self.workflows
            .lock()
            .insert(workflow.workflow_id.clone(), workflow);
        Ok(())
    }
    fn replace_workflow_steps(
        &self,
        workflow_id: &str,
        steps: Vec<kura_orchestration::WorkflowStep>,
    ) -> Result<(), String> {
        let mut workflows = self.workflows.lock();
        let mut workflow = workflows
            .get(workflow_id)
            .cloned()
            .ok_or_else(|| format!("workflow {workflow_id} not found"))?;
        workflow.steps = steps;
        workflows.insert(workflow_id.to_string(), workflow);
        Ok(())
    }
}

impl MemoryRuntimeStore {
    fn snapshot_json(&self) -> String {
        let runs = self.runs.lock();
        let steps = self.steps.lock();
        let workflows = self.workflows.lock();
        serde_json::json!({
            "runs": *runs,
            "steps": *steps,
            "workflows": *workflows,
        })
        .to_string()
    }
}

#[tokio::test]
async fn runtime_replay_recorder_preserves_redacted_credential_evidence() {
    let runtime: Arc<dyn ReplayRuntime> = Arc::new(RuntimeManager::new());
    let store = Arc::new(MemoryRuntimeStore::default());
    let recorder = RuntimeReplayRecorder::new(runtime, store.clone());
    recorder
        .record_replay(ReplayRecordInput {
            candidate: ReplayCandidate {
                candidate_id: "candidate_redacted_runtime".to_string(),
                source_kind: SourceKind::Run,
                source_id: "run_redacted_source".to_string(),
                source_refs: vec![SourceRef {
                    kind: SourceKind::Run,
                    id: "run_redacted_source".to_string(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            attempt: ReplayAttempt {
                attempt_id: "attempt_redacted_runtime".to_string(),
                mode: ReplayMode::NonLive,
                runtime_summary: "credential-backed runtime completed with token=R37_FAKE_SECRET_TENANT_A_DO_NOT_LEAK"
                    .to_string(),
                policy_summary: "policy blocked secret-token".to_string(),
                evidence_summary: "captured output was already redacted".to_string(),
                blocked_reasons: vec!["never persist R37_FAKE_TOKEN_TENANT_B_DO_NOT_LEAK".to_string()],
                evidence_refs: vec![SourceRef {
                    kind: SourceKind::Run,
                    id: "run_redacted_source".to_string(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            evidence: kura_evaluation::CapturedEvidence {
                runtime_summary: "credential-backed runtime completed with token=R37_FAKE_SECRET_TENANT_A_DO_NOT_LEAK"
                    .to_string(),
                policy_summary: "policy blocked secret-token".to_string(),
                integration_summary: "integration used R37_FAKE_TOKEN_TENANT_B_DO_NOT_LEAK".to_string(),
                evidence_summary: "captured output was already redacted".to_string(),
                limitations: vec!["do not persist R37_FAKE_SECRET_TENANT_B_DO_NOT_LEAK".to_string()],
                ..Default::default()
            },
            now: fixed_now(),
        })
        .await
        .expect("RecordReplay returned error");

    let data = store.snapshot_json();
    assert!(
        !data.contains("secret-token") && !data.contains("R37_FAKE_SECRET"),
        "runtime replay artifacts leaked credential material: {data}"
    );
    assert!(
        !data.contains("R37_FAKE_TOKEN"),
        "runtime replay artifacts leaked token material: {data}"
    );
    assert!(
        data.contains("[REDACTED]"),
        "expected redacted credential marker to be preserved: {data}"
    );
}

#[tokio::test]
async fn manager_records_completed_replay_in_runtime_plane() {
    let store = common::MemoryStore::new();
    let runtime: Arc<dyn ReplayRuntime> = Arc::new(RuntimeManager::new());
    let runtime_store = Arc::new(MemoryRuntimeStore::default());
    let recorder = Arc::new(RuntimeReplayRecorder::new(runtime, runtime_store.clone()));
    let manager = Manager::new(Dependencies {
        environment_scope: "test".to_string(),
        store: Some(Arc::new(store)),
        runtime_recorder: Some(recorder),
        clock: Some(Arc::new(fixed_now)),
        ..Default::default()
    });
    let candidate = ReplayCandidate {
        candidate_id: "candidate_runtime_record".to_string(),
        candidate_kind: CandidateKind::CuratedWork,
        display_name: "Runtime record candidate".to_string(),
        source_kind: SourceKind::Run,
        source_id: "run_source".to_string(),
        environment_scope: "test".to_string(),
        readiness_status: ReadinessStatus::FullyReplayable,
        default_replay_mode: ReplayMode::NonLive,
        source_refs: vec![SourceRef {
            kind: SourceKind::Run,
            id: "run_source".to_string(),
            route: "/v1/runs/run_source".to_string(),
        }],
        expected_comparison: Some(PlaneSummaries {
            runtime: "expected runtime".to_string(),
            policy: "expected policy".to_string(),
            evidence: "expected evidence".to_string(),
            ..Default::default()
        }),
        ..Default::default()
    };
    manager
        .upsert_replay_candidate(candidate)
        .expect("UpsertReplayCandidate returned error");

    let attempt = manager
        .create_replay_attempt(
            "candidate_runtime_record",
            CreateReplayAttemptInput::default(),
        )
        .await
        .expect("CreateReplayAttempt returned error");
    assert!(
        !attempt.result_run_id.is_empty(),
        "expected replay attempt to link a runtime run"
    );
    assert!(
        !attempt.result_workflow_id.is_empty(),
        "expected replay attempt to link a workflow"
    );
    let runs = runtime_store.runs.lock();
    assert!(
        runs.contains_key(&attempt.result_run_id),
        "expected runtime store to persist replay run {}",
        attempt.result_run_id
    );
    drop(runs);
    let workflows = runtime_store.workflows.lock();
    let workflow = workflows
        .get(&attempt.result_workflow_id)
        .expect("runtime store must persist replay workflow");
    assert_eq!(workflow.run_id, attempt.result_run_id);
    assert_eq!(
        workflow.status,
        kura_orchestration::WorkflowStatus::Completed,
        "expected completed replay workflow linked to run"
    );
    drop(workflows);
    let steps = runtime_store.steps.lock();
    assert_eq!(steps.len(), 1, "expected one persisted replay step");
    drop(steps);
    assert!(
        attempt.evidence_refs.len() >= 2,
        "expected attempt evidence refs to include replay run and workflow"
    );
    let n = attempt.evidence_refs.len();
    assert_eq!(attempt.evidence_refs[n - 2].id, attempt.result_run_id);
    assert_eq!(attempt.evidence_refs[n - 1].id, attempt.result_workflow_id);
}

#[test]
fn redact_replay_credential_strings_redacts_markers() {
    assert_eq!(
        redact_replay_credential_string("ok token=R37_FAKE_SECRET leak"),
        "[REDACTED]"
    );
    assert_eq!(
        redact_replay_credential_strings(&["secret-token here".to_string(), "fine".to_string()]),
        vec!["[REDACTED]".to_string(), "fine".to_string()]
    );
    assert_eq!(redact_replay_credential_string("no marker"), "no marker");
}
