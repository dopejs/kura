//! Computer-use manager (port of manager.go): session/action orchestration over the
//! runtime/policy/store seams. Billing quota classification is surfaced via error messages.

use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::{
    Action, ActionKind, ActionRequestResult, ActionStatus, Artifact, ArtifactCaptureRequest,
    CreateActionInput, CreateSessionInput, Driver, FailureClass, MatchResult, MemoryDriver,
    PageSummary, PageTarget, RiskLevel, Session, SessionStatus, TargetMatchContext,
    first_non_empty,
};

pub const ERR_SESSION_NOT_FOUND: &str = "computer-use session not found";
pub const ERR_ACTION_NOT_FOUND: &str = "computer-use action not found";
pub const ERR_UNSUPPORTED_MODE: &str =
    "computer-use request is outside the browser-first phase 26 scope";

pub trait Store: Send + Sync {
    fn upsert_computer_use_session(&self, session: &Session) -> Result<(), String>;
    fn list_computer_use_sessions(
        &self,
        environment: &str,
        run_id: &str,
    ) -> Result<Vec<Session>, String>;
    fn get_computer_use_session(
        &self,
        environment: &str,
        run_id: &str,
        session_id: &str,
    ) -> Result<Option<Session>, String>;
    fn upsert_computer_use_action(&self, action: &Action) -> Result<(), String>;
    fn list_computer_use_actions(
        &self,
        environment: &str,
        run_id: &str,
        session_id: &str,
    ) -> Result<Vec<Action>, String>;
    fn get_computer_use_action(
        &self,
        environment: &str,
        run_id: &str,
        session_id: &str,
        action_id: &str,
    ) -> Result<Option<Action>, String>;
    fn find_pending_computer_use_action_by_approval(
        &self,
        environment: &str,
        approval_id: &str,
    ) -> Result<Option<Action>, String>;
    fn upsert_computer_use_artifact(&self, artifact: &Artifact) -> Result<(), String>;
    fn list_computer_use_artifacts_for_action(
        &self,
        environment: &str,
        run_id: &str,
        action_id: &str,
    ) -> Result<Vec<Artifact>, String>;
    fn get_computer_use_artifact(
        &self,
        environment: &str,
        artifact_id: &str,
    ) -> Result<Option<Artifact>, String>;
    fn mark_in_flight_computer_use_interrupted(
        &self,
        environment: &str,
        now: DateTime<Utc>,
    ) -> Result<(Vec<Session>, Vec<Action>), String>;
}

pub trait ArtifactRecorder: Send + Sync {
    fn save_computer_use_artifact(&self, input: ArtifactCaptureRequest)
    -> Result<Artifact, String>;
    fn read_computer_use_artifact_content(&self, storage_key: &str) -> Result<Vec<u8>, String>;
}

pub struct Manager {
    environment: String,
    runtime: Option<Arc<kura_runtime::Manager>>,
    policy: Option<Arc<kura_policy::Engine>>,
    store: Arc<dyn Store>,
    driver: Arc<dyn Driver>,
    artifacts: Option<Arc<dyn ArtifactRecorder>>,
}

pub struct Dependencies {
    pub environment_scope: String,
    pub runtime: Option<Arc<kura_runtime::Manager>>,
    pub policy: Option<Arc<kura_policy::Engine>>,
    pub store: Arc<dyn Store>,
    pub driver: Option<Arc<dyn Driver>>,
    pub artifacts: Option<Arc<dyn ArtifactRecorder>>,
}

impl Manager {
    pub fn new(deps: Dependencies) -> Self {
        let driver = deps.driver.unwrap_or_else(|| Arc::new(MemoryDriver::new()));
        Manager {
            environment: deps.environment_scope.trim().to_string(),
            runtime: deps.runtime,
            policy: deps.policy,
            store: deps.store,
            driver,
            artifacts: deps.artifacts,
        }
    }

    pub fn acquire_session(
        &self,
        run_id: &str,
        input: CreateSessionInput,
    ) -> Result<(Session, bool), String> {
        if !input.workflow_id.trim().is_empty() {
            let sessions = self
                .store
                .list_computer_use_sessions(&self.environment, run_id)?;
            for session in sessions {
                if session.workflow_id != input.workflow_id.trim() {
                    continue;
                }
                match session.status {
                    SessionStatus::Starting | SessionStatus::Active | SessionStatus::Blocked => {
                        let enriched = self.enrich_session(&session)?;
                        return Ok((enriched, true));
                    }
                    _ => {}
                }
            }
        }
        let session = self.create_session(run_id, &input)?;
        Ok((session, false))
    }

    pub fn create_session(
        &self,
        run_id: &str,
        input: &CreateSessionInput,
    ) -> Result<Session, String> {
        let runtime = self
            .runtime
            .as_ref()
            .ok_or("runtime manager is not configured")?;
        if runtime.get_run(run_id).is_none() {
            return Err(kura_runtime::RuntimeError::RunNotFound.to_string());
        }
        let driver_kind = first_non_empty(&[&input.driver_kind, "browser"]);
        if driver_kind != "browser" {
            return Err(format!(
                "{ERR_UNSUPPORTED_MODE}: phase 26 is browser-first and does not support driver kind {:?}",
                input.driver_kind
            ));
        }
        let now = Utc::now();
        let session = Session {
            computer_use_session_id: new_computer_use_id("cusess"),
            environment_scope: self.environment.clone(),
            run_id: run_id.trim().to_string(),
            workflow_id: input.workflow_id.trim().to_string(),
            workflow_step_id: input.workflow_step_id.trim().to_string(),
            status: SessionStatus::Starting,
            driver_kind: driver_kind,
            started_at: now,
            updated_at: now,
            ..Session::default()
        };
        let started = self.driver.start_session(session, input.clone())?;
        self.store.upsert_computer_use_session(&started)?;
        self.enrich_session(&started)
    }

    pub fn list_sessions(&self, run_id: &str) -> Result<Vec<Session>, String> {
        let sessions = self
            .store
            .list_computer_use_sessions(&self.environment, run_id)?;
        sessions.iter().map(|s| self.enrich_session(s)).collect()
    }

    pub fn get_session(&self, run_id: &str, session_id: &str) -> Result<Option<Session>, String> {
        let session = self
            .store
            .get_computer_use_session(&self.environment, run_id, session_id)?;
        match session {
            Some(s) => Ok(Some(self.enrich_session(&s)?)),
            None => Ok(None),
        }
    }

    pub fn close_session(&self, run_id: &str, session_id: &str) -> Result<Session, String> {
        let session = self
            .store
            .get_computer_use_session(&self.environment, run_id, session_id)?;
        let Some(session) = session else {
            return Err(ERR_SESSION_NOT_FOUND.to_string());
        };
        let closed = self.driver.close_session(session)?;
        self.store.upsert_computer_use_session(&closed)?;
        self.enrich_session(&closed)
    }

    pub fn create_action(
        &self,
        run_id: &str,
        session_id: &str,
        requested_by: &str,
        input: CreateActionInput,
    ) -> Result<
        (
            ActionRequestResult,
            Option<kura_policy::Approval>,
            Option<kura_policy::Decision>,
        ),
        String,
    > {
        let session = self
            .store
            .get_computer_use_session(&self.environment, run_id, session_id)?;
        let Some(session) = session else {
            return Err(ERR_SESSION_NOT_FOUND.to_string());
        };
        validate_create_action_input(&input)?;
        let (step, tool_call) = self.create_runtime_tracking(&session, &input)?;
        let page_target = first_non_empty(&[&input.page_target, PageTarget::ActivePage.as_str()]);
        let mut action = Action {
            computer_use_action_id: new_computer_use_id("cuact"),
            environment_scope: self.environment.clone(),
            computer_use_session_id: session.computer_use_session_id.clone(),
            run_id: session.run_id.clone(),
            step_id: step.step_id.clone(),
            tool_call_id: tool_call.tool_call_id.clone(),
            workflow_id: session.workflow_id.clone(),
            workflow_step_id: session.workflow_step_id.clone(),
            action_kind: input.action_kind,
            status: ActionStatus::Requested,
            risk_level: classify_risk(&session, &input),
            target_match_context: clone_target_match(input.target_match_context.as_ref()),
            page_before: session.current_page.clone(),
            requested_at: Utc::now(),
            updated_at: Utc::now(),
            ..Action::default()
        };
        action.input = serde_json::json!({
            "url": input.url.trim(),
            "value": input.value,
            "selectedValue": input.selected_value,
            "waitMs": input.wait_ms,
            "pageTarget": page_target,
            "rationale": input.rationale.trim(),
        })
        .as_object()
        .cloned()
        .unwrap_or_default();

        if action.risk_level == RiskLevel::High {
            if let Some(policy) = self.policy.as_ref() {
                let (approval, decision) = policy
                    .request_approval(kura_policy::RequestApprovalInput {
                        action: "computer_use.action.execute".to_string(),
                        resource_kind: "computer_use_action".to_string(),
                        resource_id: action.computer_use_action_id.clone(),
                        reason: "high-risk computer-use action requires approval".to_string(),
                        requested_by: requested_by.to_string(),
                        ..Default::default()
                    })
                    .map_err(|e| e.to_string())?;
                action.approval_id = approval.approval_id.clone();
                action.status = ActionStatus::WaitingApproval;
                let runtime = self
                    .runtime
                    .as_ref()
                    .ok_or("runtime manager is not configured")?;
                runtime
                    .update_step_status_and_reconcile_run(
                        &step.run_id,
                        &step.step_id,
                        kura_runtime::UpdateStepStatusInput {
                            status: kura_runtime::StepStatus::Blocked,
                            output: None,
                        },
                    )
                    .map_err(|e| e.to_string())?;
                self.store.upsert_computer_use_action(&action)?;
                let mut session = session;
                session.status = SessionStatus::Blocked;
                session.last_action_id = action.computer_use_action_id.clone();
                session.updated_at = Utc::now();
                self.store.upsert_computer_use_session(&session)?;
                let enriched = self.enrich_action(&action)?;
                return Ok((
                    ActionRequestResult {
                        action: enriched,
                        pending: true,
                        approved: false,
                    },
                    Some(approval),
                    Some(decision),
                ));
            }
        }
        let enriched = self.execute_action(&session, action)?;
        Ok((
            ActionRequestResult {
                action: enriched,
                pending: false,
                approved: true,
            },
            None,
            None,
        ))
    }

    pub fn resume_pending_action(&self, approval_id: &str) -> Result<(Action, bool), String> {
        if approval_id.trim().is_empty() {
            return Ok((Action::default(), false));
        }
        let policy = self.policy.as_ref();
        let Some(policy) = policy else {
            return Ok((Action::default(), false));
        };
        let Some(approval) = policy.get_approval(approval_id.trim()) else {
            return Ok((Action::default(), false));
        };
        let Some(action) = self
            .store
            .find_pending_computer_use_action_by_approval(&self.environment, approval_id)?
        else {
            return Ok((Action::default(), false));
        };
        let session = self.store.get_computer_use_session(
            &self.environment,
            &action.run_id,
            &action.computer_use_session_id,
        )?;
        let Some(session) = session else {
            return Ok((Action::default(), false));
        };
        match approval.status {
            kura_policy::ApprovalStatus::Rejected => {
                let now = Utc::now();
                let mut action = action;
                action.status = ActionStatus::Denied;
                action.failure_class = FailureClass::PolicyDenied.as_str().to_string();
                action.failure_reason = "approval was rejected".to_string();
                action.updated_at = now;
                action.completed_at = Some(now);
                let runtime = self
                    .runtime
                    .as_ref()
                    .ok_or("runtime manager is not configured")?;
                runtime
                    .deny_tool_call(
                        &action.run_id,
                        &action.step_id,
                        &action.tool_call_id,
                        kura_runtime::DenyToolCallInput {
                            output: Some(serde_json::json!({"approvalId": approval_id})),
                            error: "approval was rejected".to_string(),
                            failure_class: FailureClass::PolicyDenied.as_str().to_string(),
                            ..Default::default()
                        },
                    )
                    .map_err(|e| e.to_string())?;
                runtime
                    .update_step_status_and_reconcile_run(
                        &action.run_id,
                        &action.step_id,
                        kura_runtime::UpdateStepStatusInput {
                            status: kura_runtime::StepStatus::Blocked,
                            output: Some(serde_json::json!({"approvalId": approval_id})),
                        },
                    )
                    .map_err(|e| e.to_string())?;
                let mut session = session;
                session.status = SessionStatus::Active;
                session.updated_at = now;
                self.store.upsert_computer_use_action(&action)?;
                self.store.upsert_computer_use_session(&session)?;
                let enriched = self.enrich_action(&action)?;
                Ok((enriched, true))
            }
            kura_policy::ApprovalStatus::Approved => {
                let enriched = self.execute_action(&session, action)?;
                Ok((enriched, true))
            }
            _ => Ok((Action::default(), true)),
        }
    }

    pub fn get_artifact(&self, artifact_id: &str) -> Result<Option<Artifact>, String> {
        self.store
            .get_computer_use_artifact(&self.environment, artifact_id)
    }

    pub fn read_artifact_content(
        &self,
        artifact_id: &str,
    ) -> Result<(Artifact, Vec<u8>, bool), String> {
        let artifact = self.get_artifact(artifact_id)?;
        let Some(artifact) = artifact else {
            return Ok((Artifact::default(), Vec::new(), false));
        };
        let Some(recorder) = self.artifacts.as_ref() else {
            return Ok((artifact, Vec::new(), true));
        };
        let content = recorder.read_computer_use_artifact_content(&artifact.storage_key)?;
        Ok((artifact, content, true))
    }

    fn execute_action(&self, session: &Session, mut action: Action) -> Result<Action, String> {
        let now = Utc::now();
        let history = self.store.list_computer_use_actions(
            &self.environment,
            &action.run_id,
            &action.computer_use_session_id,
        )?;
        let mut session = session.clone();
        session.actions = history;
        if let Some(runtime) = self.runtime.as_ref() {
            if let Some(step) = runtime.get_step(&action.run_id, &action.step_id) {
                match step.status {
                    kura_runtime::StepStatus::Blocked => {
                        runtime
                            .update_step_status_and_reconcile_run(
                                &action.run_id,
                                &action.step_id,
                                kura_runtime::UpdateStepStatusInput {
                                    status: kura_runtime::StepStatus::Planning,
                                    output: None,
                                },
                            )
                            .map_err(|e| e.to_string())?;
                        runtime
                            .update_step_status_and_reconcile_run(
                                &action.run_id,
                                &action.step_id,
                                kura_runtime::UpdateStepStatusInput {
                                    status: kura_runtime::StepStatus::ExecutingTool,
                                    output: None,
                                },
                            )
                            .map_err(|e| e.to_string())?;
                    }
                    kura_runtime::StepStatus::Planning => {
                        runtime
                            .update_step_status_and_reconcile_run(
                                &action.run_id,
                                &action.step_id,
                                kura_runtime::UpdateStepStatusInput {
                                    status: kura_runtime::StepStatus::ExecutingTool,
                                    output: None,
                                },
                            )
                            .map_err(|e| e.to_string())?;
                    }
                    _ => {}
                }
            }
        }
        if action.target_match_context.is_some() {
            if let Some(ctx) = action.target_match_context.as_mut() {
                ctx.evaluated_at = Some(now);
                ctx.match_result = Some(MatchResult::Matched);
                ctx.observed_page_url = first_page_field(
                    session.current_page.as_ref(),
                    session.current_page.as_ref(),
                    "url",
                );
            }
        }
        if let Some(captures) = evaluate_target_match(&session, &mut action) {
            action.status = ActionStatus::Failed;
            action.failure_class = FailureClass::TargetMismatch.as_str().to_string();
            action.failure_reason = "approved target no longer matches current page".to_string();
            action.updated_at = now;
            action.completed_at = Some(now);
            self.store.upsert_computer_use_action(&action)?;
            for capture in captures {
                if let Some(recorder) = self.artifacts.as_ref() {
                    match recorder.save_computer_use_artifact(capture) {
                        Ok(mut artifact) => {
                            artifact.environment_scope = self.environment.clone();
                            action.artifacts.push(artifact.clone());
                            self.store.upsert_computer_use_artifact(&artifact)?;
                        }
                        Err(e) if is_quota_artifact_capture_error(&e) => return Err(e),
                        Err(_) => {}
                    }
                }
            }
            self.store.upsert_computer_use_action(&action)?;
            self.store.upsert_computer_use_session(&session)?;
            if let Some(runtime) = self.runtime.as_ref() {
                runtime.fail_tool_call(&action.run_id, &action.step_id, &action.tool_call_id, kura_runtime::FailToolCallInput {
                    output: Some(serde_json::json!({"computerUseSessionId": action.computer_use_session_id, "computerUseActionId": action.computer_use_action_id})),
                    error: action.failure_reason.clone(),
                    failure_class: action.failure_class.clone(),
                    ..Default::default()
                }).map_err(|e| e.to_string())?;
                runtime.update_step_status_and_reconcile_run(&action.run_id, &action.step_id, kura_runtime::UpdateStepStatusInput {
                    status: kura_runtime::StepStatus::Failed,
                    output: Some(serde_json::json!({"computerUseActionId": action.computer_use_action_id, "failureClass": action.failure_class})),
                }).map_err(|e| e.to_string())?;
            }
            return self.enrich_action(&action);
        }
        if let Some(runtime) = self.runtime.as_ref() {
            runtime
                .mark_tool_call_running(
                    &action.run_id,
                    &action.step_id,
                    &action.tool_call_id,
                    "",
                    serde_json::Map::new(),
                )
                .map_err(|e| e.to_string())?;
        }
        let (running_session, executed_action, captures) = self
            .driver
            .execute_action(session.clone(), action.clone())?;
        let mut action = executed_action;
        let session = running_session;
        self.store.upsert_computer_use_action(&action)?;
        for capture in captures {
            if let Some(recorder) = self.artifacts.as_ref() {
                match recorder.save_computer_use_artifact(capture) {
                    Ok(mut artifact) => {
                        artifact.environment_scope = self.environment.clone();
                        action.artifacts.push(artifact.clone());
                        self.store.upsert_computer_use_artifact(&artifact)?;
                    }
                    Err(e) if is_quota_artifact_capture_error(&e) => return Err(e),
                    Err(_) => {}
                }
            }
        }
        self.store.upsert_computer_use_action(&action)?;
        self.store.upsert_computer_use_session(&session)?;
        match action.status {
            ActionStatus::Completed => {
                if let Some(runtime) = self.runtime.as_ref() {
                    runtime.complete_tool_call(&action.run_id, &action.step_id, &action.tool_call_id, kura_runtime::CompleteToolCallInput {
                        output: Some(serde_json::json!({"computerUseSessionId": action.computer_use_session_id, "computerUseActionId": action.computer_use_action_id})),
                        ..Default::default()
                    }).map_err(|e| e.to_string())?;
                    runtime.update_step_status_and_reconcile_run(&action.run_id, &action.step_id, kura_runtime::UpdateStepStatusInput {
                        status: kura_runtime::StepStatus::Completed,
                        output: Some(serde_json::json!({"computerUseActionId": action.computer_use_action_id})),
                    }).map_err(|e| e.to_string())?;
                }
            }
            _ => {
                if let Some(runtime) = self.runtime.as_ref() {
                    runtime.fail_tool_call(&action.run_id, &action.step_id, &action.tool_call_id, kura_runtime::FailToolCallInput {
                        output: Some(serde_json::json!({"computerUseSessionId": action.computer_use_session_id, "computerUseActionId": action.computer_use_action_id})),
                        error: action.failure_reason.clone(),
                        failure_class: action.failure_class.clone(),
                        ..Default::default()
                    }).map_err(|e| e.to_string())?;
                    runtime.update_step_status_and_reconcile_run(&action.run_id, &action.step_id, kura_runtime::UpdateStepStatusInput {
                        status: kura_runtime::StepStatus::Failed,
                        output: Some(serde_json::json!({"computerUseActionId": action.computer_use_action_id, "failureClass": action.failure_class})),
                    }).map_err(|e| e.to_string())?;
                }
            }
        }
        self.enrich_action(&action)
    }

    fn create_runtime_tracking(
        &self,
        session: &Session,
        input: &CreateActionInput,
    ) -> Result<(kura_runtime::Step, kura_runtime::ToolCall), String> {
        let runtime = self
            .runtime
            .as_ref()
            .ok_or("runtime manager is not configured")?;
        let step = runtime
            .create_step(&session.run_id, kura_runtime::CreateStepInput {
                title: format!("Computer-use {}", input.action_kind.as_str()),
                kind: "computer_use".to_string(),
                workflow_id: session.workflow_id.clone(),
                workflow_step_id: session.workflow_step_id.clone(),
                input: Some(serde_json::json!({"actionKind": input.action_kind.as_str(), "url": input.url.trim()})),
                ..Default::default()
            })
            .map_err(|e| e.to_string())?;
        runtime
            .update_step_status_and_reconcile_run(
                &session.run_id,
                &step.step_id,
                kura_runtime::UpdateStepStatusInput {
                    status: kura_runtime::StepStatus::Planning,
                    output: None,
                },
            )
            .map_err(|e| e.to_string())?;
        runtime
            .update_step_status_and_reconcile_run(
                &session.run_id,
                &step.step_id,
                kura_runtime::UpdateStepStatusInput {
                    status: kura_runtime::StepStatus::ExecutingTool,
                    output: None,
                },
            )
            .map_err(|e| e.to_string())?;
        let tool_call = runtime
            .create_tool_call(&session.run_id, &step.step_id, kura_runtime::CreateToolCallInput {
                workflow_id: session.workflow_id.clone(),
                workflow_step_id: session.workflow_step_id.clone(),
                invocation_kind: kura_runtime::ToolCallInvocationKind::LocalTool.as_str().to_string(),
                capability_id: "browser".to_string(),
                tool_name: input.action_kind.as_str().to_string(),
                input: Some(serde_json::json!({"actionKind": input.action_kind.as_str(), "url": input.url.trim()})),
                computer_use_session_id: session.computer_use_session_id.clone(),
                ..Default::default()
            })
            .map_err(|e| e.to_string())?;
        Ok((step, tool_call))
    }

    fn enrich_session(&self, session: &Session) -> Result<Session, String> {
        let actions = self.store.list_computer_use_actions(
            &self.environment,
            &session.run_id,
            &session.computer_use_session_id,
        )?;
        let mut session = session.clone();
        session.actions = actions
            .iter()
            .map(|a| self.enrich_action(a))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(session)
    }

    fn enrich_action(&self, action: &Action) -> Result<Action, String> {
        let artifacts = self.store.list_computer_use_artifacts_for_action(
            &self.environment,
            &action.run_id,
            &action.computer_use_action_id,
        )?;
        let mut action = action.clone();
        action.artifacts = artifacts;
        Ok(action)
    }
}

fn classify_risk(session: &Session, input: &CreateActionInput) -> RiskLevel {
    match input.action_kind {
        ActionKind::Click | ActionKind::Input | ActionKind::Select | ActionKind::Download => {
            RiskLevel::High
        }
        ActionKind::Navigate => {
            if session.trusted_page_scope.is_none() || input.url.trim().is_empty() {
                return RiskLevel::Low;
            }
            let origin = origin_from_url(&input.url);
            if !origin.is_empty()
                && Some(origin.as_str())
                    != session
                        .trusted_page_scope
                        .as_ref()
                        .map(|s| s.origin.as_str())
            {
                return RiskLevel::High;
            }
            RiskLevel::Low
        }
        _ => RiskLevel::Low,
    }
}

fn validate_create_action_input(input: &CreateActionInput) -> Result<(), String> {
    if !is_supported_action_kind(input.action_kind) {
        return Err(format!(
            "{ERR_UNSUPPORTED_MODE}: unsupported action kind {:?}",
            input.action_kind.as_str()
        ));
    }
    match first_non_empty(&[&input.page_target, PageTarget::ActivePage.as_str()]).as_str() {
        "active_page" => Ok(()),
        "new_tab" | "new_window" => Err(format!(
            "{ERR_UNSUPPORTED_MODE}: phase 26 supports only a single active page and rejects {:?} requests",
            input.page_target
        )),
        _ => Err(format!(
            "{ERR_UNSUPPORTED_MODE}: unsupported page target {:?}",
            input.page_target
        )),
    }
}

fn is_supported_action_kind(kind: ActionKind) -> bool {
    matches!(
        kind,
        ActionKind::Navigate
            | ActionKind::Back
            | ActionKind::Forward
            | ActionKind::Wait
            | ActionKind::Screenshot
            | ActionKind::Snapshot
            | ActionKind::Click
            | ActionKind::Input
            | ActionKind::Select
            | ActionKind::Download
            | ActionKind::CloseSession
    )
}

fn evaluate_target_match(
    session: &Session,
    action: &mut Action,
) -> Option<Vec<ArtifactCaptureRequest>> {
    if action.target_match_context.is_none() {
        return None;
    }
    let now = Utc::now();
    if let Some(ctx) = action.target_match_context.as_mut() {
        ctx.evaluated_at = Some(now);
    }
    if !mismatched(action.target_match_context.as_ref()) {
        if let Some(ctx) = action.target_match_context.as_mut() {
            ctx.match_result = Some(MatchResult::Matched);
        }
        return None;
    }
    if let Some(ctx) = action.target_match_context.as_mut() {
        ctx.match_result = Some(MatchResult::Mismatched);
    }
    Some(vec![crate::build_page_evidence_capture(
        session,
        action,
        ActionKind::Snapshot,
    )])
}

fn is_quota_artifact_capture_error(err: &str) -> bool {
    err.contains("quota") || err.contains("operator action")
}

#[allow(dead_code)]
fn failure_class_for_driver_error(kind: ActionKind) -> FailureClass {
    if kind == ActionKind::Navigate {
        FailureClass::NavigationFailure
    } else {
        FailureClass::UnavailableConsumer
    }
}

fn clone_target_match(input: Option<&TargetMatchContext>) -> Option<TargetMatchContext> {
    input.cloned()
}

fn new_computer_use_id(prefix: &str) -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!("{prefix}_{}", &hex[..16])
}

fn origin_from_url(raw: &str) -> String {
    match url::Url::parse(raw.trim()) {
        Ok(parsed) => {
            let scheme = parsed.scheme();
            if let Some(host) = parsed.host_str() {
                if !scheme.is_empty() && !host.is_empty() {
                    return format!("{scheme}://{host}");
                }
            }
            String::new()
        }
        Err(_) => String::new(),
    }
}

fn mismatched(target: Option<&TargetMatchContext>) -> bool {
    let Some(target) = target else { return false };
    let expected_selector = target.expected_selector.trim().to_lowercase();
    let expected_text = target.expected_text.trim().to_lowercase();
    let expected_url = target.expected_page_url.trim().to_lowercase();
    expected_selector.contains("missing")
        || expected_text.contains("missing")
        || expected_url.contains("missing")
}

fn first_page_field(
    after: Option<&PageSummary>,
    before: Option<&PageSummary>,
    field: &str,
) -> String {
    match field {
        "url" => {
            if let Some(a) = after {
                if !a.url.is_empty() {
                    return a.url.clone();
                }
            }
            if let Some(b) = before {
                return b.url.clone();
            }
        }
        "title" => {
            if let Some(a) = after {
                if !a.title.is_empty() {
                    return a.title.clone();
                }
            }
            if let Some(b) = before {
                return b.title.clone();
            }
        }
        _ => {}
    }
    String::new()
}
