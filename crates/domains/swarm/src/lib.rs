//! kura-swarm — bounded concurrent sub-agents (Stage 4).
//!
//! A swarm run fans a list of goals out to child turns that execute
//! concurrently. This crate owns the **model**: the run and child records,
//! the bounds, the quota seam, and the state machine. It deliberately owns no
//! execution — a child turn *is* a `chat::Service::query`, driven by the API
//! layer, so every child passes through `chat/turn-start`, `chat/pre-dispatch`
//! and `chat/turn-end` like any other turn and the "model-visible = logged"
//! invariant holds per child without this crate knowing hooks exist.
//!
//! Two decisions carried over from the tool plane:
//! - **No permissive quota default.** A fan-out multiplies spend; the only
//!   always-allow gate is named `ExplicitlyUnboundedChildQuota` so it cannot be
//!   mistaken for one.
//! - **Opt-in, not default-on.** OpenClaw 2026.9.2 enabled Swarm by default;
//!   Kura ships it disabled until `swarm.config.enabled = true`.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const DOC_KIND_SWARM_RUN: &str = "swarm_run";
pub const DEFAULT_MAX_CHILDREN_PER_RUN: usize = 8;
pub const DEFAULT_MAX_CONCURRENT_CHILDREN: usize = 4;
/// Output kept per child in the run record; the full dispatch stays on the
/// dispatch record the child's turn persisted.
pub const OUTPUT_PREVIEW_CHARS: usize = 2_000;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SwarmConfig {
    /// Explicit opt-in. False means every launch is refused.
    pub enabled: bool,
    pub max_children_per_run: usize,
    pub max_concurrent_children: usize,
}

impl SwarmConfig {
    #[must_use]
    pub fn max_children(&self) -> usize {
        if self.max_children_per_run > 0 {
            self.max_children_per_run
        } else {
            DEFAULT_MAX_CHILDREN_PER_RUN
        }
    }
    #[must_use]
    pub fn max_concurrent(&self) -> usize {
        if self.max_concurrent_children > 0 {
            self.max_concurrent_children
        } else {
            DEFAULT_MAX_CONCURRENT_CHILDREN
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildStatus {
    Queued,
    Running,
    Completed,
    Failed,
    QuotaDenied,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Queued,
    Running,
    Completed,
    PartialFailed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwarmChild {
    pub index: usize,
    pub goal: String,
    /// Each child gets its own thread so its continuity cannot bleed into a
    /// sibling's: `swarm:<run>:<index>`.
    pub thread_id: String,
    pub status: ChildStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dispatch_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub output_preview: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwarmRun {
    pub run_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
    pub requested_by: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    pub status: RunStatus,
    pub children: Vec<SwarmChild>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
}

impl SwarmRun {
    /// Structured result: every child's terminal state. The caller reads
    /// this rather than scraping transcripts.
    #[must_use]
    pub fn summary(&self) -> RunSummary {
        let mut s = RunSummary::default();
        for c in &self.children {
            match c.status {
                ChildStatus::Completed => s.completed += 1,
                ChildStatus::Failed => s.failed += 1,
                ChildStatus::QuotaDenied => s.quota_denied += 1,
                ChildStatus::Cancelled => s.cancelled += 1,
                ChildStatus::Queued | ChildStatus::Running => s.pending += 1,
            }
        }
        s
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSummary {
    pub completed: usize,
    pub failed: usize,
    pub quota_denied: usize,
    pub cancelled: usize,
    pub pending: usize,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LaunchInput {
    pub goals: Vec<String>,
    pub provider: String,
    pub model: String,
    pub requested_by: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SwarmError {
    #[error("swarm is not enabled; set swarm.config.enabled = true to opt in")]
    Disabled,
    #[error("at least one non-empty goal is required")]
    GoalsRequired,
    #[error("{requested} goals exceeds the bound of {max} per run")]
    TooManyChildren { requested: usize, max: usize },
    #[error("swarm run not found")]
    NotFound,
}

/// Quota decision for one child, reserved **before** the child is spawned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildQuotaDecision {
    Allowed { operation_key: String },
    Denied { reason_code: String },
}

/// The per-child spend bound. There is no permissive default implementation
/// here on purpose (see the crate docs).
pub trait ChildQuotaGate: Send + Sync {
    fn reserve(&self, tenant_id: &str, run_id: &str, index: usize) -> ChildQuotaDecision;
    fn commit(&self, tenant_id: &str, operation_key: &str);
    fn release(&self, tenant_id: &str, operation_key: &str, reason: &str);
}

/// Allows everything. Named so it cannot be mistaken for a default.
pub struct ExplicitlyUnboundedChildQuota;
impl ChildQuotaGate for ExplicitlyUnboundedChildQuota {
    fn reserve(&self, _t: &str, _r: &str, _i: usize) -> ChildQuotaDecision {
        ChildQuotaDecision::Allowed {
            operation_key: String::new(),
        }
    }
    fn commit(&self, _t: &str, _k: &str) {}
    fn release(&self, _t: &str, _k: &str, _r: &str) {}
}

/// In-memory run registry; persistence is the caller's (persistence
/// inversion, as everywhere in the workspace).
pub struct Manager {
    config: SwarmConfig,
    runs: parking_lot::RwLock<HashMap<String, SwarmRun>>,
    order: parking_lot::RwLock<Vec<String>>,
}

impl Manager {
    #[must_use]
    pub fn new(config: SwarmConfig) -> Self {
        Manager {
            config,
            runs: Default::default(),
            order: Default::default(),
        }
    }
    #[must_use]
    pub fn config(&self) -> &SwarmConfig {
        &self.config
    }

    pub fn restore(&self, runs: Vec<SwarmRun>) {
        let mut map = self.runs.write();
        let mut order = self.order.write();
        map.clear();
        order.clear();
        for mut run in runs {
            // A run interrupted by a restart cannot resume its threads; mark
            // what was still in flight so it is not reported as pending forever.
            if matches!(run.status, RunStatus::Queued | RunStatus::Running) {
                for c in &mut run.children {
                    if matches!(c.status, ChildStatus::Queued | ChildStatus::Running) {
                        c.status = ChildStatus::Cancelled;
                        c.error = "daemon restarted before completion".into();
                    }
                }
                run.status = finalize_status(&run);
                run.completed_at = Some(Utc::now());
            }
            order.push(run.run_id.clone());
            map.insert(run.run_id.clone(), run);
        }
    }

    /// Validates and records a run in `Queued`; execution is the caller's.
    pub fn launch(&self, tenant_id: &str, input: LaunchInput) -> Result<SwarmRun, SwarmError> {
        if !self.config.enabled {
            return Err(SwarmError::Disabled);
        }
        let goals: Vec<String> = input
            .goals
            .iter()
            .map(|g| g.trim().to_string())
            .filter(|g| !g.is_empty())
            .collect();
        if goals.is_empty() {
            return Err(SwarmError::GoalsRequired);
        }
        let max = self.config.max_children();
        if goals.len() > max {
            return Err(SwarmError::TooManyChildren {
                requested: goals.len(),
                max,
            });
        }
        let now = Utc::now();
        let run_id = format!("swm_{}", uuid::Uuid::new_v4().simple());
        let run = SwarmRun {
            run_id: run_id.clone(),
            tenant_id: tenant_id.trim().to_string(),
            requested_by: input.requested_by.trim().to_string(),
            provider: input.provider.trim().to_string(),
            model: input.model.trim().to_string(),
            status: RunStatus::Queued,
            children: goals
                .into_iter()
                .enumerate()
                .map(|(index, goal)| SwarmChild {
                    index,
                    goal,
                    thread_id: format!("swarm:{run_id}:{index}"),
                    status: ChildStatus::Queued,
                    dispatch_id: String::new(),
                    output_preview: String::new(),
                    error: String::new(),
                    started_at: None,
                    completed_at: None,
                })
                .collect(),
            created_at: now,
            updated_at: now,
            completed_at: None,
        };
        self.runs.write().insert(run_id.clone(), run.clone());
        self.order.write().push(run_id);
        Ok(run)
    }

    #[must_use]
    pub fn get(&self, run_id: &str) -> Option<SwarmRun> {
        self.runs.read().get(run_id.trim()).cloned()
    }

    #[must_use]
    pub fn list(&self, tenant_id: &str) -> Vec<SwarmRun> {
        let map = self.runs.read();
        self.order
            .read()
            .iter()
            .filter_map(|id| map.get(id))
            .filter(|r| r.tenant_id == tenant_id.trim())
            .cloned()
            .collect()
    }

    /// Applies a mutation to one child and recomputes the run status.
    pub fn update_child(
        &self,
        run_id: &str,
        index: usize,
        apply: impl FnOnce(&mut SwarmChild),
    ) -> Option<SwarmRun> {
        let mut map = self.runs.write();
        let run = map.get_mut(run_id)?;
        let child = run.children.get_mut(index)?;
        apply(child);
        run.updated_at = Utc::now();
        run.status = if run
            .children
            .iter()
            .any(|c| matches!(c.status, ChildStatus::Queued | ChildStatus::Running))
        {
            RunStatus::Running
        } else {
            let s = finalize_status(run);
            run.completed_at = Some(run.updated_at);
            s
        };
        Some(run.clone())
    }

    pub fn mark_running(&self, run_id: &str) -> Option<SwarmRun> {
        let mut map = self.runs.write();
        let run = map.get_mut(run_id)?;
        run.status = RunStatus::Running;
        run.updated_at = Utc::now();
        Some(run.clone())
    }
}

fn finalize_status(run: &SwarmRun) -> RunStatus {
    let s = run.summary();
    if s.pending > 0 {
        return RunStatus::Running;
    }
    let total = run.children.len();
    if s.cancelled == total {
        RunStatus::Cancelled
    } else if s.completed == total {
        RunStatus::Completed
    } else if s.completed == 0 {
        RunStatus::Failed
    } else {
        RunStatus::PartialFailed
    }
}

/// Truncates an output for the run record without cutting a UTF-8 boundary.
#[must_use]
pub fn preview(output: &str) -> String {
    if output.len() <= OUTPUT_PREVIEW_CHARS {
        return output.to_string();
    }
    let mut end = OUTPUT_PREVIEW_CHARS;
    while !output.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &output[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enabled() -> Manager {
        Manager::new(SwarmConfig {
            enabled: true,
            max_children_per_run: 3,
            max_concurrent_children: 2,
        })
    }

    #[test]
    fn disabled_by_default_refuses_every_launch() {
        let m = Manager::new(SwarmConfig::default());
        assert_eq!(
            m.launch(
                "t",
                LaunchInput {
                    goals: vec!["a".into()],
                    ..Default::default()
                }
            )
            .unwrap_err(),
            SwarmError::Disabled
        );
    }

    #[test]
    fn launch_validates_goals_and_the_bound() {
        let m = enabled();
        assert_eq!(
            m.launch(
                "t",
                LaunchInput {
                    goals: vec![" ".into()],
                    ..Default::default()
                }
            )
            .unwrap_err(),
            SwarmError::GoalsRequired
        );
        let err = m
            .launch(
                "t",
                LaunchInput {
                    goals: vec!["a".into(), "b".into(), "c".into(), "d".into()],
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert_eq!(
            err,
            SwarmError::TooManyChildren {
                requested: 4,
                max: 3
            }
        );
        let run = m
            .launch(
                "t",
                LaunchInput {
                    goals: vec!["a".into(), "b".into()],
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(run.children.len(), 2);
        assert_eq!(run.children[1].thread_id, format!("swarm:{}:1", run.run_id));
        assert!(run.children.iter().all(|c| c.status == ChildStatus::Queued));
    }

    #[test]
    fn run_status_follows_the_children() {
        let m = enabled();
        let run = m
            .launch(
                "t",
                LaunchInput {
                    goals: vec!["a".into(), "b".into()],
                    ..Default::default()
                },
            )
            .unwrap();
        let r = m
            .update_child(&run.run_id, 0, |c| c.status = ChildStatus::Completed)
            .unwrap();
        assert_eq!(r.status, RunStatus::Running, "one still pending");
        let r = m
            .update_child(&run.run_id, 1, |c| c.status = ChildStatus::Failed)
            .unwrap();
        assert_eq!(r.status, RunStatus::PartialFailed);
        assert!(r.completed_at.is_some());
        assert_eq!(
            r.summary(),
            RunSummary {
                completed: 1,
                failed: 1,
                ..Default::default()
            }
        );
    }

    #[test]
    fn restore_cancels_children_a_restart_interrupted() {
        let m = enabled();
        let mut run = m
            .launch(
                "t",
                LaunchInput {
                    goals: vec!["a".into()],
                    ..Default::default()
                },
            )
            .unwrap();
        run.children[0].status = ChildStatus::Running;
        run.status = RunStatus::Running;
        let m2 = enabled();
        m2.restore(vec![run]);
        let r = m2.list("t").pop().unwrap();
        assert_eq!(r.children[0].status, ChildStatus::Cancelled);
        assert_eq!(r.status, RunStatus::Cancelled);
    }

    #[test]
    fn runs_are_listed_per_tenant() {
        let m = enabled();
        m.launch(
            "a",
            LaunchInput {
                goals: vec!["x".into()],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(m.list("a").len(), 1);
        assert!(m.list("b").is_empty());
    }

    #[test]
    fn preview_never_splits_a_char() {
        let s = "é".repeat(OUTPUT_PREVIEW_CHARS);
        let p = preview(&s);
        assert!(p.ends_with('…'));
        assert!(p.chars().count() <= OUTPUT_PREVIEW_CHARS + 1);
    }
}
