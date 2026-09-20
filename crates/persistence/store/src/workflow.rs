//! SQLite CRUD for the workflow orchestration ledger: workflows and their child
//! steps/dependencies/handoffs. Ported from `daemon/internal/store/store.go`
//! (UpsertWorkflow, ReplaceWorkflowSteps, ReplaceWorkflowDependencies,
//! ReplaceWorkflowHandoffs, ListWorkflows, GetWorkflow, GetWorkflowByID,
//! MarkInFlightWorkflowsInterrupted). The workflow document is decoded from
//! `document_json` (the whole serialized `kura-orchestration` value) and the
//! steps/dependencies/handoffs are stored in their own tables, matching the Go
//! read/decode path exactly. The tenant column is written as NULL until the
//! tenancy package is ported.

use chrono::{DateTime, Utc};
use rusqlite::{Row, params};

use crate::SQLiteStore;
use crate::crud::{
    enum_str, now_rfc3339, null_string, opt_time_string, parse_enum, parse_opt_rfc3339,
    parse_rfc3339,
};

/// A workflow ledger row. `document` is the JSON-serialized workflow document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkflowRecord {
    pub workflow_id: String,
    pub run_id: String,
    pub schedule_id: String,
    pub schedule_attempt_id: String,
    pub environment_scope: String,
    pub goal: String,
    pub status: String,
    pub plan_summary: String,
    pub failure_summary: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub interrupted_at: Option<DateTime<Utc>>,
    pub document: String,
}

/// A workflow step ledger row. `document` is the JSON-serialized step document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkflowStepRecord {
    pub workflow_step_id: String,
    pub workflow_id: String,
    pub position: i64,
    pub status: String,
    pub runtime_step_id: String,
    pub active_tool_call_id: String,
    pub attempt_count: i64,
    pub max_attempts: i64,
    pub last_failure_class: String,
    pub blocked_reason: String,
    pub document: String,
}

/// A workflow dependency ledger row. `document` is the JSON-serialized dependency document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkflowDependencyRecord {
    pub dependency_id: String,
    pub workflow_id: String,
    pub document: String,
}

/// A workflow handoff ledger row. `document` is the JSON-serialized handoff document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkflowHandoffRecord {
    pub handoff_id: String,
    pub workflow_id: String,
    pub status: String,
    pub document: String,
}

fn scan_workflow_record(row: &Row) -> Result<WorkflowRecord, String> {
    let workflow_id: String = row.get(0).map_err(|e| e.to_string())?;
    let run_id: String = row.get(1).map_err(|e| e.to_string())?;
    let schedule_id: Option<String> = row.get(2).map_err(|e| e.to_string())?;
    let schedule_attempt_id: Option<String> = row.get(3).map_err(|e| e.to_string())?;
    let environment_scope: String = row.get(4).map_err(|e| e.to_string())?;
    let goal: String = row.get(5).map_err(|e| e.to_string())?;
    let status: String = row.get(6).map_err(|e| e.to_string())?;
    let plan_summary: Option<String> = row.get(7).map_err(|e| e.to_string())?;
    let failure_summary: Option<String> = row.get(8).map_err(|e| e.to_string())?;
    let created_at: String = row.get(9).map_err(|e| e.to_string())?;
    let updated_at: String = row.get(10).map_err(|e| e.to_string())?;
    let started_at: Option<String> = row.get(11).map_err(|e| e.to_string())?;
    let completed_at: Option<String> = row.get(12).map_err(|e| e.to_string())?;
    let interrupted_at: Option<String> = row.get(13).map_err(|e| e.to_string())?;
    let document: String = row.get(14).map_err(|e| e.to_string())?;

    Ok(WorkflowRecord {
        workflow_id,
        run_id,
        schedule_id: schedule_id.unwrap_or_default(),
        schedule_attempt_id: schedule_attempt_id.unwrap_or_default(),
        environment_scope,
        goal,
        status,
        plan_summary: plan_summary.unwrap_or_default(),
        failure_summary: failure_summary.unwrap_or_default(),
        created_at: parse_rfc3339(&created_at)?,
        updated_at: parse_rfc3339(&updated_at)?,
        started_at: parse_opt_rfc3339(started_at)?,
        completed_at: parse_opt_rfc3339(completed_at)?,
        interrupted_at: parse_opt_rfc3339(interrupted_at)?,
        document,
    })
}

fn scan_workflow_step_record(row: &Row) -> Result<WorkflowStepRecord, String> {
    let workflow_step_id: String = row.get(0).map_err(|e| e.to_string())?;
    let workflow_id: String = row.get(1).map_err(|e| e.to_string())?;
    let position: i64 = row.get(2).map_err(|e| e.to_string())?;
    let status: String = row.get(3).map_err(|e| e.to_string())?;
    let runtime_step_id: Option<String> = row.get(4).map_err(|e| e.to_string())?;
    let active_tool_call_id: Option<String> = row.get(5).map_err(|e| e.to_string())?;
    let attempt_count: i64 = row.get(6).map_err(|e| e.to_string())?;
    let max_attempts: i64 = row.get(7).map_err(|e| e.to_string())?;
    let last_failure_class: Option<String> = row.get(8).map_err(|e| e.to_string())?;
    let blocked_reason: Option<String> = row.get(9).map_err(|e| e.to_string())?;
    let document: String = row.get(10).map_err(|e| e.to_string())?;

    Ok(WorkflowStepRecord {
        workflow_step_id,
        workflow_id,
        position,
        status,
        runtime_step_id: runtime_step_id.unwrap_or_default(),
        active_tool_call_id: active_tool_call_id.unwrap_or_default(),
        attempt_count,
        max_attempts,
        last_failure_class: last_failure_class.unwrap_or_default(),
        blocked_reason: blocked_reason.unwrap_or_default(),
        document,
    })
}

fn scan_workflow_dependency_record(row: &Row) -> Result<WorkflowDependencyRecord, String> {
    let dependency_id: String = row.get(0).map_err(|e| e.to_string())?;
    let workflow_id: String = row.get(1).map_err(|e| e.to_string())?;
    let document: String = row.get(2).map_err(|e| e.to_string())?;
    Ok(WorkflowDependencyRecord {
        dependency_id,
        workflow_id,
        document,
    })
}

fn scan_workflow_handoff_record(row: &Row) -> Result<WorkflowHandoffRecord, String> {
    let handoff_id: String = row.get(0).map_err(|e| e.to_string())?;
    let workflow_id: String = row.get(1).map_err(|e| e.to_string())?;
    let status: String = row.get(2).map_err(|e| e.to_string())?;
    let document: String = row.get(3).map_err(|e| e.to_string())?;
    Ok(WorkflowHandoffRecord {
        handoff_id,
        workflow_id,
        status,
        document,
    })
}

impl SQLiteStore {
    pub fn upsert_workflow(&self, workflow: &kura_orchestration::Workflow) -> Result<(), String> {
        let document_json = serde_json::to_string(workflow)
            .map_err(|e| format!("marshal workflow {}: {e}", workflow.workflow_id))?;
        self.conn
            .execute(
                r#"INSERT INTO workflows (
                    workflow_id, run_id, schedule_id, schedule_attempt_id, environment_scope, goal,
                    status, plan_summary, failure_summary, created_at, updated_at, started_at,
                    completed_at, interrupted_at, document_json, tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
                ON CONFLICT(workflow_id) DO UPDATE SET
                    run_id = excluded.run_id,
                    schedule_id = excluded.schedule_id,
                    schedule_attempt_id = excluded.schedule_attempt_id,
                    environment_scope = excluded.environment_scope,
                    goal = excluded.goal,
                    status = excluded.status,
                    plan_summary = excluded.plan_summary,
                    failure_summary = excluded.failure_summary,
                    created_at = excluded.created_at,
                    updated_at = excluded.updated_at,
                    started_at = excluded.started_at,
                    completed_at = excluded.completed_at,
                    interrupted_at = excluded.interrupted_at,
                    document_json = excluded.document_json,
                    tenant_id = COALESCE(workflows.tenant_id, excluded.tenant_id)"#,
                params![
                    workflow.workflow_id,
                    workflow.run_id,
                    null_string(&workflow.schedule_id),
                    null_string(&workflow.schedule_attempt_id),
                    workflow.environment_scope,
                    workflow.goal,
                    enum_str(&workflow.status),
                    null_string(&workflow.plan_summary),
                    null_string(&workflow.failure_summary),
                    now_rfc3339(&workflow.created_at),
                    now_rfc3339(&workflow.updated_at),
                    opt_time_string(&workflow.started_at),
                    opt_time_string(&workflow.completed_at),
                    opt_time_string(&workflow.interrupted_at),
                    document_json,
                    None::<String>,
                ],
            )
            .map_err(|e| format!("upsert workflow {}: {e}", workflow.workflow_id))?;
        Ok(())
    }

    pub fn replace_workflow_steps(
        &self,
        workflow_id: &str,
        steps: &[kura_orchestration::WorkflowStep],
    ) -> Result<(), String> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin replace workflow steps {workflow_id}: {e}"))?;

        tx.execute(
            "DELETE FROM workflow_steps WHERE workflow_id = ?1",
            params![workflow_id],
        )
        .map_err(|e| format!("delete workflow steps {workflow_id}: {e}"))?;

        for step in steps {
            let document_json = serde_json::to_string(step)
                .map_err(|e| format!("marshal workflow step {}: {e}", step.workflow_step_id))?;
            tx.execute(
                r#"INSERT INTO workflow_steps (
                    workflow_step_id, workflow_id, position, status, runtime_step_id,
                    active_tool_call_id, attempt_count, max_attempts, last_failure_class,
                    blocked_reason, document_json, tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"#,
                params![
                    step.workflow_step_id,
                    workflow_id,
                    step.position,
                    enum_str(&step.status),
                    null_string(&step.runtime_step_id),
                    null_string(&step.active_tool_call_id),
                    step.attempt_count,
                    step.max_attempts,
                    null_string(&step.last_failure_class),
                    null_string(&step.blocked_reason),
                    document_json,
                    None::<String>,
                ],
            )
            .map_err(|e| format!("insert workflow step {}: {e}", step.workflow_step_id))?;
        }

        tx.commit()
            .map_err(|e| format!("commit replace workflow steps {workflow_id}: {e}"))
    }

    pub fn replace_workflow_dependencies(
        &self,
        workflow_id: &str,
        items: &[kura_orchestration::Dependency],
    ) -> Result<(), String> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin replace workflow dependencies {workflow_id}: {e}"))?;

        tx.execute(
            "DELETE FROM workflow_dependencies WHERE workflow_id = ?1",
            params![workflow_id],
        )
        .map_err(|e| format!("delete workflow dependencies {workflow_id}: {e}"))?;

        for item in items {
            let document_json = serde_json::to_string(item)
                .map_err(|e| format!("marshal workflow dependency {}: {e}", item.dependency_id))?;
            tx.execute(
                "INSERT INTO workflow_dependencies (dependency_id, workflow_id, document_json, tenant_id) VALUES (?1, ?2, ?3, ?4)",
                params![item.dependency_id, workflow_id, document_json, None::<String>],
            )
            .map_err(|e| format!("insert workflow dependency {}: {e}", item.dependency_id))?;
        }

        tx.commit()
            .map_err(|e| format!("commit replace workflow dependencies {workflow_id}: {e}"))
    }

    pub fn replace_workflow_handoffs(
        &self,
        workflow_id: &str,
        items: &[kura_orchestration::Handoff],
    ) -> Result<(), String> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin replace workflow handoffs {workflow_id}: {e}"))?;

        tx.execute(
            "DELETE FROM workflow_handoffs WHERE workflow_id = ?1",
            params![workflow_id],
        )
        .map_err(|e| format!("delete workflow handoffs {workflow_id}: {e}"))?;

        for item in items {
            let document_json = serde_json::to_string(item)
                .map_err(|e| format!("marshal workflow handoff {}: {e}", item.handoff_id))?;
            tx.execute(
                "INSERT INTO workflow_handoffs (handoff_id, workflow_id, status, document_json, tenant_id) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![item.handoff_id, workflow_id, enum_str(&item.status), document_json, None::<String>],
            )
            .map_err(|e| format!("insert workflow handoff {}: {e}", item.handoff_id))?;
        }

        tx.commit()
            .map_err(|e| format!("commit replace workflow handoffs {workflow_id}: {e}"))
    }

    pub fn list_workflows(
        &self,
        environment_scope: &str,
        run_id: &str,
    ) -> Result<Vec<kura_orchestration::Workflow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT workflow_id, run_id, schedule_id, schedule_attempt_id, environment_scope,
                    goal, status, plan_summary, failure_summary, created_at, updated_at, started_at,
                    completed_at, interrupted_at, document_json
                FROM workflows
                WHERE environment_scope = ?1 AND run_id = ?2
                ORDER BY created_at ASC, workflow_id ASC"#,
            )
            .map_err(|e| format!("list workflows for run {run_id}: {e}"))?;
        let mut rows = stmt
            .query(params![environment_scope.trim(), run_id.trim()])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let record = scan_workflow_record(row)?;
            items.push(self.decode_workflow_record(record)?);
        }
        Ok(items)
    }

    pub fn get_workflow(
        &self,
        environment_scope: &str,
        run_id: &str,
        workflow_id: &str,
    ) -> Result<Option<kura_orchestration::Workflow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT workflow_id, run_id, schedule_id, schedule_attempt_id, environment_scope,
                    goal, status, plan_summary, failure_summary, created_at, updated_at, started_at,
                    completed_at, interrupted_at, document_json
                FROM workflows
                WHERE environment_scope = ?1 AND run_id = ?2 AND workflow_id = ?3"#,
            )
            .map_err(|e| e.to_string())?;
        let mut rows = stmt
            .query(params![
                environment_scope.trim(),
                run_id.trim(),
                workflow_id.trim()
            ])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let record = scan_workflow_record(row)?;
        self.decode_workflow_record(record).map(Some)
    }

    pub fn get_workflow_by_id(
        &self,
        environment_scope: &str,
        workflow_id: &str,
    ) -> Result<Option<kura_orchestration::Workflow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT workflow_id, run_id, schedule_id, schedule_attempt_id, environment_scope,
                    goal, status, plan_summary, failure_summary, created_at, updated_at, started_at,
                    completed_at, interrupted_at, document_json
                FROM workflows
                WHERE environment_scope = ?1 AND workflow_id = ?2"#,
            )
            .map_err(|e| e.to_string())?;
        let mut rows = stmt
            .query(params![environment_scope.trim(), workflow_id.trim()])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let record = scan_workflow_record(row)?;
        self.decode_workflow_record(record).map(Some)
    }

    pub fn mark_in_flight_workflows_interrupted(
        &self,
        environment_scope: &str,
        interrupted_at: DateTime<Utc>,
    ) -> Result<Vec<kura_orchestration::Workflow>, String> {
        let items = self.list_interruptible_workflows(environment_scope)?;
        let mut updated = Vec::new();
        for mut workflow in items {
            let timestamp = interrupted_at.with_timezone(&Utc);
            workflow.status = kura_orchestration::WorkflowStatus::Interrupted;
            workflow.updated_at = timestamp;
            workflow.interrupted_at = Some(timestamp);
            for step in &mut workflow.steps {
                match step.status {
                    kura_orchestration::StepStatus::Running
                    | kura_orchestration::StepStatus::Ready
                    | kura_orchestration::StepStatus::WaitingDependency => {
                        step.status = kura_orchestration::StepStatus::Interrupted;
                        step.updated_at = timestamp;
                    }
                    _ => {}
                }
            }
            for handoff in &mut workflow.handoffs {
                if handoff.status == kura_orchestration::HandoffStatus::Pending {
                    handoff.status = kura_orchestration::HandoffStatus::Invalid;
                    handoff.invalid_reason = "daemon_restart_interrupted_workflow".to_string();
                }
            }
            self.upsert_workflow(&workflow)?;
            self.replace_workflow_steps(&workflow.workflow_id, &workflow.steps)?;
            self.replace_workflow_handoffs(&workflow.workflow_id, &workflow.handoffs)?;
            updated.push(workflow);
        }
        Ok(updated)
    }

    fn list_interruptible_workflows(
        &self,
        environment_scope: &str,
    ) -> Result<Vec<kura_orchestration::Workflow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT workflow_id, run_id, schedule_id, schedule_attempt_id, environment_scope,
                    goal, status, plan_summary, failure_summary, created_at, updated_at, started_at,
                    completed_at, interrupted_at, document_json
                FROM workflows
                WHERE environment_scope = ?1
                  AND status IN (?2, ?3, ?4)
                ORDER BY created_at ASC, workflow_id ASC"#,
            )
            .map_err(|e| format!("list interruptible workflows: {e}"))?;
        let mut rows = stmt
            .query(params![
                environment_scope.trim(),
                kura_orchestration::WorkflowStatus::Planned.as_str(),
                kura_orchestration::WorkflowStatus::Running.as_str(),
                kura_orchestration::WorkflowStatus::Blocked.as_str(),
            ])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let record = scan_workflow_record(row)?;
            items.push(self.decode_workflow_record(record)?);
        }
        Ok(items)
    }

    fn decode_workflow_record(
        &self,
        record: WorkflowRecord,
    ) -> Result<kura_orchestration::Workflow, String> {
        let mut workflow: kura_orchestration::Workflow = if record.document.is_empty() {
            kura_orchestration::Workflow::default()
        } else {
            serde_json::from_str(&record.document)
                .map_err(|e| format!("decode workflow {}: {e}", record.workflow_id))?
        };
        if workflow.workflow_id.is_empty() {
            workflow = kura_orchestration::Workflow {
                workflow_id: record.workflow_id.clone(),
                run_id: record.run_id.clone(),
                schedule_id: record.schedule_id.clone(),
                schedule_attempt_id: record.schedule_attempt_id.clone(),
                environment_scope: record.environment_scope.clone(),
                goal: record.goal.clone(),
                status: parse_enum(&record.status)?,
                plan_summary: record.plan_summary.clone(),
                failure_summary: record.failure_summary.clone(),
                created_at: record.created_at,
                updated_at: record.updated_at,
                started_at: record.started_at,
                completed_at: record.completed_at,
                interrupted_at: record.interrupted_at,
                ..kura_orchestration::Workflow::default()
            };
        }
        workflow.schedule_id = record.schedule_id;
        workflow.schedule_attempt_id = record.schedule_attempt_id;
        workflow.environment_scope = record.environment_scope;
        workflow.status = parse_enum(&record.status)?;
        workflow.plan_summary = record.plan_summary;
        workflow.failure_summary = record.failure_summary;
        workflow.created_at = record.created_at;
        workflow.updated_at = record.updated_at;
        workflow.started_at = record.started_at;
        workflow.completed_at = record.completed_at;
        workflow.interrupted_at = record.interrupted_at;

        workflow.steps = self.list_workflow_steps(&record.workflow_id)?;
        workflow.dependencies = self.list_workflow_dependencies(&record.workflow_id)?;
        workflow.handoffs = self.list_workflow_handoffs(&record.workflow_id)?;
        Ok(workflow)
    }

    fn list_workflow_steps(
        &self,
        workflow_id: &str,
    ) -> Result<Vec<kura_orchestration::WorkflowStep>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT workflow_step_id, workflow_id, position, status, runtime_step_id,
                    active_tool_call_id, attempt_count, max_attempts, last_failure_class,
                    blocked_reason, document_json
                FROM workflow_steps
                WHERE workflow_id = ?1
                ORDER BY position ASC, workflow_step_id ASC"#,
            )
            .map_err(|e| format!("list workflow steps {workflow_id}: {e}"))?;
        let mut rows = stmt
            .query(params![workflow_id])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let record = scan_workflow_step_record(row)?;
            let mut item: kura_orchestration::WorkflowStep = if record.document.is_empty() {
                kura_orchestration::WorkflowStep::default()
            } else {
                serde_json::from_str(&record.document)
                    .map_err(|e| format!("decode workflow step {}: {e}", record.workflow_step_id))?
            };
            item.workflow_step_id = record.workflow_step_id;
            item.workflow_id = record.workflow_id;
            item.position = record.position;
            item.status = parse_enum(&record.status)?;
            item.runtime_step_id = record.runtime_step_id;
            item.active_tool_call_id = record.active_tool_call_id;
            item.attempt_count = record.attempt_count;
            item.max_attempts = record.max_attempts;
            item.last_failure_class = record.last_failure_class;
            item.blocked_reason = record.blocked_reason;
            items.push(item);
        }
        Ok(items)
    }

    fn list_workflow_dependencies(
        &self,
        workflow_id: &str,
    ) -> Result<Vec<kura_orchestration::Dependency>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT dependency_id, workflow_id, document_json
                FROM workflow_dependencies
                WHERE workflow_id = ?1
                ORDER BY dependency_id ASC"#,
            )
            .map_err(|e| format!("list workflow dependencies {workflow_id}: {e}"))?;
        let mut rows = stmt
            .query(params![workflow_id])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let record = scan_workflow_dependency_record(row)?;
            let item: kura_orchestration::Dependency = serde_json::from_str(&record.document)
                .map_err(|e| format!("decode workflow dependency {}: {e}", record.dependency_id))?;
            items.push(item);
        }
        Ok(items)
    }

    fn list_workflow_handoffs(
        &self,
        workflow_id: &str,
    ) -> Result<Vec<kura_orchestration::Handoff>, String> {
        let mut stmt = self
            .conn
            .prepare(
                r#"SELECT handoff_id, workflow_id, status, document_json
                FROM workflow_handoffs
                WHERE workflow_id = ?1
                ORDER BY handoff_id ASC"#,
            )
            .map_err(|e| format!("list workflow handoffs {workflow_id}: {e}"))?;
        let mut rows = stmt
            .query(params![workflow_id])
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let record = scan_workflow_handoff_record(row)?;
            let mut item: kura_orchestration::Handoff = serde_json::from_str(&record.document)
                .map_err(|e| format!("decode workflow handoff {}: {e}", record.handoff_id))?;
            item.status = parse_enum(&record.status)?;
            items.push(item);
        }
        Ok(items)
    }
}
