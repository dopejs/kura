//! Runs a suite: for every task (and repeat) set up its work directory, ask
//! the agent through the daemon's ordinary chat route, watch the turn's
//! tool calls and approvals while it runs, then check the computer's state
//! and tear down.
//!
//! Attempts run one at a time: a device has one session, and the steps of
//! an attempt are the `chat.tool.called` events between its start and end.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::checks::{CheckResult, run_checks};
use crate::client::{Client, Reply, ToolCall};
use crate::desktop::Desktop;
use crate::suite::{Placeholders, SetupStep, Suite, Task, TeardownStep};

/// What to do with an approval the agent raises (a risky chord). The
/// suite's tasks never need one, so the default turns it down at once
/// rather than leave the turn waiting three minutes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    Reject,
    /// Leave it for a person to answer in the UI.
    Manual,
}

#[derive(Debug, Clone)]
pub struct RunConfig {
    pub run_id: String,
    /// The run's root; every work directory is inside it, and its path
    /// (which contains `run_id`) is what teardown matches on.
    pub work_root: PathBuf,
    pub provider: String,
    pub model: String,
    pub repeats: u32,
    /// Only these task ids, when not empty.
    pub only: Vec<String>,
    pub approvals: ApprovalMode,
    /// How often the turn's tool calls and approvals are read.
    pub poll: Duration,
    /// Pause after the turn before checking, for apps to finish writing.
    pub settle: Duration,
    /// How long a cancelled turn may take to stop before the run gives up.
    pub cancel_grace: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The check held and the steps stayed within budget.
    Passed,
    /// The turn ended and the check did not hold.
    CheckFailed,
    /// More `computer.act` calls than the budget; the turn was cancelled.
    OverBudget,
    /// The time limit passed; the turn was cancelled.
    TimedOut,
    /// The chat turn failed (provider error, round limit, HTTP error).
    TurnFailed,
    /// The agent asked for control and did not get it: denied, or the
    /// device was offline. Not counted for the verdict.
    NoControl,
    /// The harness could not prepare the task. Not counted.
    SetupFailed,
    /// A precondition did not hold (an app was already open). Not counted.
    Skipped,
}

impl Outcome {
    /// Whether the attempt says something about the agent and its tools,
    /// rather than about the harness or the person at the computer.
    #[must_use]
    pub fn counted(self) -> bool {
        !matches!(self, Self::NoControl | Self::SetupFailed | Self::Skipped)
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::CheckFailed => "check_failed",
            Self::OverBudget => "over_budget",
            Self::TimedOut => "timed_out",
            Self::TurnFailed => "turn_failed",
            Self::NoControl => "no_control",
            Self::SetupFailed => "setup_failed",
            Self::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalSeen {
    pub approval_id: String,
    pub reason: String,
    /// `rejected_by_runner`, `left_for_person`, or the final status.
    pub handling: String,
    pub final_status: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnInfo {
    pub http_status: u16,
    pub status: String,
    pub finish_reason: String,
    pub error_code: String,
    pub error: String,
    pub turn_id: String,
    /// The model's last words, clipped; for reading, never for scoring.
    pub reply_excerpt: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

/// One attempt at one task.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Attempt {
    pub task_id: String,
    pub category: String,
    pub repeat: u32,
    pub outcome: Outcome,
    pub check_passed: bool,
    pub checks: Vec<CheckResult>,
    /// `computer.act` calls, failed ones included.
    pub steps: u32,
    pub step_budget: u32,
    pub reference_steps: u32,
    /// `computer.act` calls by `op`.
    pub ops: BTreeMap<String, u32>,
    pub start_calls: u32,
    pub control_granted: bool,
    /// Other tools the model called in the turn.
    pub other_tool_calls: u32,
    /// Failed `computer.*` calls by the error code their output starts with.
    pub errors: BTreeMap<String, u32>,
    pub approvals: Vec<ApprovalSeen>,
    /// Screenshot bytes the daemon received in the attempt, from
    /// `/metrics`; absent when that is not readable.
    pub screenshot_bytes: Option<u64>,
    pub wall_ms: u64,
    pub turn: TurnInfo,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cancelled: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub setup_error: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub teardown_errors: Vec<String>,
    pub work_dir: String,
}

/// Every attempt, and why the run stopped early if it did.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunResult {
    pub attempts: Vec<Attempt>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub aborted: String,
}

/// Device-side codes that mean the agent never got the computer, through
/// no fault of its own.
const NO_CONTROL_CODES: &[&str] = &["session_denied", "device_offline"];

pub struct Runner {
    pub client: Client,
    pub desktop: Arc<dyn Desktop>,
    pub config: RunConfig,
}

impl Runner {
    /// Runs every selected task `repeats` times, in suite order within
    /// each repeat. Stops early, keeping what was recorded, when going on
    /// would be unsafe or pointless (no device, a turn that would not stop).
    pub async fn run(&self, suite: &Suite) -> RunResult {
        let tasks: Vec<&Task> = suite
            .tasks
            .iter()
            .filter(|task| self.config.only.is_empty() || self.config.only.contains(&task.id))
            .collect();
        let mut result = RunResult {
            attempts: Vec::new(),
            aborted: String::new(),
        };
        let mut cursor = match self.client.tool_calls_after(0).await {
            Ok((_, head)) => head,
            Err(err) => {
                result.aborted = format!("cannot read events: {err}");
                return result;
            }
        };
        for repeat in 1..=self.config.repeats.max(1) {
            for task in &tasks {
                match self.attempt(suite, task, repeat, &mut cursor).await {
                    Ok(attempt) => {
                        eprintln!(
                            "[cua-eval] {} r{}: {} steps={}/{} wall={}ms{}",
                            task.id,
                            repeat,
                            attempt.outcome.as_str(),
                            attempt.steps,
                            attempt.step_budget,
                            attempt.wall_ms,
                            if attempt.errors.is_empty() {
                                String::new()
                            } else {
                                format!(" errors={:?}", attempt.errors)
                            }
                        );
                        result.attempts.push(attempt);
                    }
                    Err((attempt, reason)) => {
                        eprintln!("[cua-eval] {} r{}: run stopped: {reason}", task.id, repeat);
                        if let Some(attempt) = attempt {
                            result.attempts.push(*attempt);
                        }
                        result.aborted = reason;
                        return result;
                    }
                }
            }
        }
        result
    }

    async fn attempt(
        &self,
        suite: &Suite,
        task: &Task,
        repeat: u32,
        cursor: &mut i64,
    ) -> Result<Attempt, (Option<Box<Attempt>>, String)> {
        let work = self.config.work_root.join(format!("{}-r{repeat}", task.id));
        let placeholders = Placeholders { work: &work };
        let mut attempt = Attempt {
            task_id: task.id.clone(),
            category: task.category.clone(),
            repeat,
            outcome: Outcome::SetupFailed,
            check_passed: false,
            checks: Vec::new(),
            steps: 0,
            step_budget: task.budget(&suite.defaults),
            reference_steps: task.reference_steps,
            ops: BTreeMap::new(),
            start_calls: 0,
            control_granted: false,
            other_tool_calls: 0,
            errors: BTreeMap::new(),
            approvals: Vec::new(),
            screenshot_bytes: None,
            wall_ms: 0,
            turn: TurnInfo::default(),
            cancelled: String::new(),
            setup_error: String::new(),
            teardown_errors: Vec::new(),
            work_dir: work.to_string_lossy().into_owned(),
        };

        // Preconditions, then setup. Neither touches anything outside the
        // work directory except opening the task's own file.
        for app in &task.requires.apps_not_running {
            match self
                .blocking({
                    let app = app.clone();
                    move |desktop| desktop.app_running(&app)
                })
                .await
            {
                Ok(false) => {}
                Ok(true) => {
                    attempt.outcome = Outcome::Skipped;
                    attempt.setup_error =
                        format!("{app} is already running; quit it to run this task");
                    return Ok(attempt);
                }
                Err(err) => {
                    attempt.setup_error = err;
                    return Ok(attempt);
                }
            }
        }
        if let Err(err) = self.setup(task, &work, &placeholders).await {
            attempt.setup_error = err;
            attempt.teardown_errors = self.teardown(task).await;
            return Ok(attempt);
        }

        match self.client.devices().await {
            Ok(devices) if devices.iter().any(|d| d.online && d.session.is_none()) => {}
            Ok(devices) => {
                let reason = if devices.iter().any(|d| d.online) {
                    "the device is already in a session; finish or stop it first"
                } else {
                    "no device is online; start kura-device run"
                };
                attempt.outcome = Outcome::NoControl;
                attempt.setup_error = reason.into();
                attempt.teardown_errors = self.teardown(task).await;
                return Err((Some(Box::new(attempt)), reason.into()));
            }
            Err(err) => {
                attempt.setup_error = err.clone();
                attempt.teardown_errors = self.teardown(task).await;
                return Err((Some(Box::new(attempt)), err));
            }
        }

        let known: BTreeSet<String> = match self.client.approvals(false).await {
            Ok(list) => list.into_iter().map(|a| a.approval_id).collect(),
            Err(err) => {
                attempt.setup_error = err.clone();
                attempt.teardown_errors = self.teardown(task).await;
                return Err((Some(Box::new(attempt)), err));
            }
        };
        let shots_before = self.client.screenshot_bytes_total().await;

        let query = compose_query(&placeholders.apply(&task.instruction), &suite.preamble);
        let request_id = format!("cuaeval-{}-{}-r{repeat}", self.config.run_id, task.id);
        let budget = attempt.step_budget;
        let time_limit = Duration::from_secs(task.time_limit(&suite.defaults));
        let started = Instant::now();

        let chat = self.client.chat(
            &request_id,
            &query,
            &self.config.provider,
            &self.config.model,
            time_limit + self.config.cancel_grace + Duration::from_secs(30),
        );
        tokio::pin!(chat);
        let mut calls: Vec<ToolCall> = Vec::new();
        let mut seen = known.clone();
        let mut cancelled_at: Option<Instant> = None;
        let mut timed_out = false;
        let mut abandoned = false;
        let mut tick = tokio::time::interval(self.config.poll);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let reply: Result<Reply, String> = loop {
            tokio::select! {
                reply = &mut chat => break reply,
                _ = tick.tick() => {
                    self.drain(cursor, &mut calls).await;
                    self.handle_approvals(&mut seen, &mut attempt.approvals).await;
                    let steps = count_steps(&calls);
                    if cancelled_at.is_none() {
                        let why = if steps > budget {
                            Some(format!("{steps} steps, over the budget of {budget}"))
                        } else if started.elapsed() > time_limit {
                            timed_out = true;
                            Some(format!("over the time limit of {}s", time_limit.as_secs()))
                        } else {
                            None
                        };
                        if let Some(why) = why {
                            if let Err(err) = self.client.cancel(&request_id).await {
                                attempt.cancelled = format!("{why}; cancel failed: {err}");
                            } else {
                                attempt.cancelled = why;
                            }
                            cancelled_at = Some(Instant::now());
                        }
                    } else if cancelled_at.is_some_and(|at| at.elapsed() > self.config.cancel_grace) {
                        abandoned = true;
                        break Err("the turn did not stop after it was cancelled".into());
                    }
                }
            }
        };
        attempt.wall_ms = started.elapsed().as_millis() as u64;
        self.drain(cursor, &mut calls).await;
        self.handle_approvals(&mut seen, &mut attempt.approvals)
            .await;
        self.finish_approvals(&mut attempt.approvals).await;
        tally(&calls, &mut attempt);
        match reply {
            Ok(reply) => attempt.turn = turn_info(&reply),
            Err(err) => attempt.turn.error = err,
        }

        // The session belongs to the turn and ends with it; the next
        // attempt must not start while something still holds the device.
        if let Err(reason) = self.wait_for_idle_device(abandoned).await {
            attempt.teardown_errors = self.teardown(task).await;
            attempt.outcome = classify(&attempt, timed_out);
            return Err((Some(Box::new(attempt)), reason));
        }

        tokio::time::sleep(self.config.settle).await;
        attempt.checks = self
            .blocking({
                let checks = task.check.clone();
                let work = work.clone();
                move |desktop| Ok(run_checks(&checks, &work, desktop))
            })
            .await
            .unwrap_or_default();
        attempt.check_passed =
            !attempt.checks.is_empty() && attempt.checks.iter().all(|c| c.passed);
        let shots_after = self.client.screenshot_bytes_total().await;
        attempt.screenshot_bytes = match (shots_before, shots_after) {
            (Some(before), Some(after)) => Some(after.saturating_sub(before)),
            _ => None,
        };
        attempt.teardown_errors = self.teardown(task).await;
        attempt.outcome = classify(&attempt, timed_out);
        Ok(attempt)
    }

    async fn setup(
        &self,
        task: &Task,
        work: &Path,
        placeholders: &Placeholders<'_>,
    ) -> Result<(), String> {
        std::fs::create_dir_all(work).map_err(|err| format!("{}: {err}", work.display()))?;
        for step in &task.setup {
            match step {
                SetupStep::Write { path, content } => {
                    let full = work.join(path);
                    if let Some(parent) = full.parent() {
                        std::fs::create_dir_all(parent)
                            .map_err(|err| format!("{}: {err}", parent.display()))?;
                    }
                    std::fs::write(&full, placeholders.apply(content))
                        .map_err(|err| format!("{}: {err}", full.display()))?;
                }
                SetupStep::Mkdir(path) => {
                    let full = work.join(path);
                    std::fs::create_dir_all(&full)
                        .map_err(|err| format!("{}: {err}", full.display()))?;
                }
                SetupStep::Open { path, app } => {
                    let full = work.join(path);
                    let app = app.clone();
                    self.blocking(move |desktop| desktop.open(&full, app.as_deref()))
                        .await?;
                }
                SetupStep::Wait { ms } => tokio::time::sleep(Duration::from_millis(*ms)).await,
            }
        }
        Ok(())
    }

    async fn teardown(&self, task: &Task) -> Vec<String> {
        let marker = self.config.work_root.to_string_lossy().into_owned();
        let mut errors = Vec::new();
        for step in &task.teardown {
            let step = step.clone();
            let marker = marker.clone();
            let done = self
                .blocking(move |desktop| match &step {
                    TeardownStep::CloseDocuments(app) => desktop.close_documents(app, &marker),
                    TeardownStep::CloseFinderWindows => desktop.close_finder_windows(&marker),
                    TeardownStep::QuitApp(app) => desktop.quit_app(app),
                })
                .await;
            if let Err(err) = done {
                errors.push(err);
            }
        }
        errors
    }

    async fn drain(&self, cursor: &mut i64, calls: &mut Vec<ToolCall>) {
        match self.client.tool_calls_after(*cursor).await {
            Ok((more, next)) => {
                calls.extend(more);
                *cursor = next;
            }
            Err(err) => eprintln!("[cua-eval] reading tool calls failed (will retry): {err}"),
        }
    }

    /// Records every new computer approval and, in `Reject` mode, turns
    /// it down.
    async fn handle_approvals(
        &self,
        seen: &mut BTreeSet<String>,
        approvals: &mut Vec<ApprovalSeen>,
    ) {
        let pending = match self.client.approvals(true).await {
            Ok(pending) => pending,
            Err(err) => {
                eprintln!("[cua-eval] reading approvals failed (will retry): {err}");
                return;
            }
        };
        for approval in pending {
            if approval.action != "computer.action.execute"
                || !seen.insert(approval.approval_id.clone())
            {
                continue;
            }
            let handling = match self.config.approvals {
                ApprovalMode::Reject => match self
                    .client
                    .resolve_approval(&approval.approval_id, "rejected")
                    .await
                {
                    Ok(()) => "rejected_by_runner".to_string(),
                    Err(err) => format!("reject failed: {err}"),
                },
                ApprovalMode::Manual => "left_for_person".to_string(),
            };
            approvals.push(ApprovalSeen {
                approval_id: approval.approval_id,
                reason: approval.reason,
                handling,
                final_status: approval.status,
            });
        }
    }

    async fn finish_approvals(&self, approvals: &mut [ApprovalSeen]) {
        if approvals.is_empty() {
            return;
        }
        if let Ok(all) = self.client.approvals(false).await {
            for seen in approvals.iter_mut() {
                if let Some(found) = all.iter().find(|a| a.approval_id == seen.approval_id) {
                    seen.final_status = found.status.clone();
                }
            }
        }
    }

    async fn wait_for_idle_device(&self, abandoned: bool) -> Result<(), String> {
        let wait = if abandoned {
            Duration::from_secs(60)
        } else {
            Duration::from_secs(10)
        };
        let deadline = Instant::now() + wait;
        loop {
            match self.client.devices().await {
                Ok(devices) if devices.iter().all(|d| d.session.is_none()) => return Ok(()),
                Ok(_) | Err(_) if Instant::now() >= deadline => {
                    return Err(format!(
                        "a session on the device was still live {}s after the turn; stopping \
                         so the next task does not share it",
                        wait.as_secs()
                    ));
                }
                _ => tokio::time::sleep(Duration::from_millis(250)).await,
            }
        }
    }

    async fn blocking<T: Send + 'static>(
        &self,
        work: impl FnOnce(&dyn Desktop) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let desktop = Arc::clone(&self.desktop);
        tokio::task::spawn_blocking(move || work(desktop.as_ref()))
            .await
            .map_err(|err| format!("desktop call did not finish: {err}"))?
    }
}

#[must_use]
pub fn compose_query(instruction: &str, preamble: &str) -> String {
    if preamble.trim().is_empty() {
        instruction.trim().to_string()
    } else {
        format!("{}\n\n{}", instruction.trim(), preamble.trim())
    }
}

/// `computer_act` on the OpenAI wire is `computer.act` in Kura.
fn tool_name(name: &str) -> String {
    match name {
        "computer_start" => "computer.start".into(),
        "computer_act" => "computer.act".into(),
        "computer_stop" => "computer.stop".into(),
        other => other.to_string(),
    }
}

fn count_steps(calls: &[ToolCall]) -> u32 {
    calls
        .iter()
        .filter(|call| tool_name(&call.name) == "computer.act")
        .count() as u32
}

/// The stable code a failed `computer.*` output starts with
/// (`session_denied: ...`), or `other`.
#[must_use]
pub fn error_code(output: &str) -> String {
    let head = output.trim_start().split(':').next().unwrap_or("");
    let plain = !head.is_empty()
        && head.len() <= 40
        && head
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if plain {
        head.to_string()
    } else {
        "other".into()
    }
}

/// Counts a turn's tool calls into the attempt.
pub fn tally(calls: &[ToolCall], attempt: &mut Attempt) {
    for call in calls {
        let name = tool_name(&call.name);
        match name.as_str() {
            "computer.act" => {
                attempt.steps += 1;
                let op = serde_json::from_str::<serde_json::Value>(&call.arguments)
                    .ok()
                    .and_then(|args| args["op"].as_str().map(str::to_string))
                    .unwrap_or_else(|| "unreadable".into());
                *attempt.ops.entry(op).or_default() += 1;
            }
            "computer.start" => {
                attempt.start_calls += 1;
                if !call.is_error {
                    attempt.control_granted = true;
                }
            }
            "computer.stop" => {}
            _ => attempt.other_tool_calls += 1,
        }
        if call.is_error && name.starts_with("computer.") {
            *attempt.errors.entry(error_code(&call.output)).or_default() += 1;
        }
    }
}

/// The outcome, from what was recorded. The computer's state decides
/// success; the step budget overrides it; then the failures say why.
#[must_use]
pub fn classify(attempt: &Attempt, timed_out: bool) -> Outcome {
    if !attempt.setup_error.is_empty() && attempt.checks.is_empty() && attempt.steps == 0 {
        return Outcome::SetupFailed;
    }
    if attempt.steps > attempt.step_budget {
        return Outcome::OverBudget;
    }
    if attempt.check_passed {
        return Outcome::Passed;
    }
    if timed_out {
        return Outcome::TimedOut;
    }
    let refused = NO_CONTROL_CODES
        .iter()
        .any(|code| attempt.errors.contains_key(*code));
    if attempt.start_calls > 0 && !attempt.control_granted && refused {
        return Outcome::NoControl;
    }
    let turn = &attempt.turn;
    if turn.http_status != 200 || !matches!(turn.status.as_str(), "completed" | "") {
        return Outcome::TurnFailed;
    }
    Outcome::CheckFailed
}

fn turn_info(reply: &Reply) -> TurnInfo {
    TurnInfo {
        http_status: reply.http_status,
        status: reply.status.clone(),
        finish_reason: reply.finish_reason.clone(),
        error_code: reply.error_code.clone(),
        error: reply.error.chars().take(500).collect(),
        turn_id: reply.turn_id.clone(),
        reply_excerpt: reply.reply.chars().take(500).collect(),
        input_tokens: reply.input_tokens,
        output_tokens: reply.output_tokens,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, arguments: &str, output: &str, is_error: bool) -> ToolCall {
        ToolCall {
            sequence: 0,
            name: name.into(),
            arguments: arguments.into(),
            output: output.into(),
            is_error,
            duration_ms: 1,
        }
    }

    fn blank() -> Attempt {
        Attempt {
            task_id: "t".into(),
            category: "edit".into(),
            repeat: 1,
            outcome: Outcome::SetupFailed,
            check_passed: false,
            checks: Vec::new(),
            steps: 0,
            step_budget: 5,
            reference_steps: 2,
            ops: BTreeMap::new(),
            start_calls: 0,
            control_granted: false,
            other_tool_calls: 0,
            errors: BTreeMap::new(),
            approvals: Vec::new(),
            screenshot_bytes: None,
            wall_ms: 0,
            turn: TurnInfo {
                http_status: 200,
                status: "completed".into(),
                ..TurnInfo::default()
            },
            cancelled: String::new(),
            setup_error: String::new(),
            teardown_errors: Vec::new(),
            work_dir: String::new(),
        }
    }

    #[test]
    fn error_codes_are_the_stable_prefix() {
        assert_eq!(
            error_code("session_denied: the person said no"),
            "session_denied"
        );
        assert_eq!(
            error_code("outcome_unknown_timeout: take a screenshot"),
            "outcome_unknown_timeout"
        );
        assert_eq!(error_code("Something broke"), "other");
        assert_eq!(error_code(""), "other");
    }

    #[test]
    fn tally_counts_acts_by_op_and_failures_by_code() {
        let mut attempt = blank();
        tally(
            &[
                call("computer.start", "{}", "session_denied: no", true),
                call("computer_start", "{}", "The person allowed control", false),
                call(
                    "computer.act",
                    r#"{"op":"click","x":1,"y":2}"#,
                    "click: done",
                    false,
                ),
                call(
                    "computer_act",
                    r#"{"op":"key","keys":"cmd+q"}"#,
                    "approval_rejected: no",
                    true,
                ),
                call("computer.act", "not json", "invalid: x", true),
                call("computer.stop", "{}", "Control is handed back.", false),
                call("memory.lookup", "{}", "", false),
            ],
            &mut attempt,
        );
        assert_eq!(attempt.steps, 3);
        assert_eq!(attempt.start_calls, 2);
        assert!(attempt.control_granted);
        assert_eq!(attempt.other_tool_calls, 1);
        assert_eq!(
            attempt.ops,
            BTreeMap::from([
                ("click".into(), 1),
                ("key".into(), 1),
                ("unreadable".into(), 1)
            ])
        );
        assert_eq!(
            attempt.errors,
            BTreeMap::from([
                ("approval_rejected".into(), 1),
                ("invalid".into(), 1),
                ("session_denied".into(), 1)
            ])
        );
    }

    #[test]
    fn the_budget_overrides_the_check_and_the_check_overrides_the_turn() {
        let mut attempt = blank();
        attempt.check_passed = true;
        attempt.steps = 6;
        assert_eq!(classify(&attempt, false), Outcome::OverBudget);
        attempt.steps = 5;
        attempt.turn.status = "failed".into();
        assert_eq!(
            classify(&attempt, true),
            Outcome::Passed,
            "the state is what counts"
        );
        attempt.check_passed = false;
        assert_eq!(classify(&attempt, true), Outcome::TimedOut);
        assert_eq!(classify(&attempt, false), Outcome::TurnFailed);
        attempt.turn.status = "completed".into();
        assert_eq!(classify(&attempt, false), Outcome::CheckFailed);
    }

    #[test]
    fn a_denied_session_is_not_the_agents_failure() {
        let mut attempt = blank();
        attempt.start_calls = 1;
        attempt.errors.insert("session_denied".into(), 1);
        assert_eq!(classify(&attempt, false), Outcome::NoControl);
        assert!(!Outcome::NoControl.counted());
        // A start the model got wrong is its own failure.
        let mut attempt = blank();
        attempt.start_calls = 1;
        attempt.errors.insert("invalid".into(), 1);
        assert_eq!(classify(&attempt, false), Outcome::CheckFailed);
    }

    #[test]
    fn the_query_is_the_instruction_then_the_preamble() {
        assert_eq!(compose_query(" do x ", ""), "do x");
        assert_eq!(
            compose_query("do x", "Use my computer."),
            "do x\n\nUse my computer."
        );
    }
}
