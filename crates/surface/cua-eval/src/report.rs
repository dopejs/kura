//! The numbers a run is judged by, as JSON (every attempt) and a short
//! markdown summary, and the verdict the suite's rule gives them.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::runner::{Attempt, Outcome, RunResult};
use crate::suite::{Suite, VerdictRule};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub run_id: String,
    pub suite: String,
    pub suite_version: u32,
    pub provider: String,
    pub model: String,
    pub started_at: String,
    pub finished_at: String,
    pub summary: Summary,
    pub tasks: Vec<TaskSummary>,
    pub attempts: Vec<Attempt>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub aborted: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub attempts: u32,
    /// Attempts that say something about the agent and its tools: all but
    /// `no_control`, `setup_failed` and `skipped`.
    pub counted: u32,
    pub passed: u32,
    /// `passed / counted`; absent when nothing was counted.
    pub success_rate: Option<f64>,
    /// Median `computer.act` calls of passed attempts.
    pub median_steps_passed: Option<f64>,
    /// Median of counted attempts, failures included.
    pub median_steps_counted: Option<f64>,
    /// Median of steps / referenceSteps over passed attempts: 1.0 is a
    /// person's path; more is detours, misclicks and retries.
    pub median_step_ratio: Option<f64>,
    pub median_wall_ms: Option<f64>,
    pub outcomes: BTreeMap<String, u32>,
    pub errors: BTreeMap<String, u32>,
    pub approvals: u32,
    pub screenshot_bytes: Option<u64>,
    /// Screenshot bytes per screenshot-returning call (start + acts that
    /// succeeded), a proxy for the image cost per step.
    pub mean_screenshot_bytes: Option<f64>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub verdict: Verdict,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSummary {
    pub task_id: String,
    pub category: String,
    pub counted: u32,
    pub passed: u32,
    pub success_rate: Option<f64>,
    pub median_steps_passed: Option<f64>,
    pub reference_steps: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Verdict {
    /// `good_enough`, `native_mapping_worth_measuring`, `inconclusive`, or
    /// `insufficient_data`.
    pub decision: String,
    pub reason: String,
}

#[must_use]
pub fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let mid = values.len() / 2;
    Some(if values.len() % 2 == 0 {
        f64::midpoint(values[mid - 1], values[mid])
    } else {
        values[mid]
    })
}

pub struct RunInfo<'a> {
    pub run_id: &'a str,
    pub provider: &'a str,
    pub model: &'a str,
    pub started_at: String,
    pub finished_at: String,
}

#[must_use]
pub fn build(suite: &Suite, info: RunInfo<'_>, run: RunResult) -> Report {
    let attempts = run.attempts;
    let counted: Vec<&Attempt> = attempts.iter().filter(|a| a.outcome.counted()).collect();
    let passed: Vec<&Attempt> = counted
        .iter()
        .copied()
        .filter(|a| a.outcome == Outcome::Passed)
        .collect();
    let mut outcomes = BTreeMap::new();
    let mut errors = BTreeMap::new();
    for attempt in &attempts {
        *outcomes
            .entry(attempt.outcome.as_str().to_string())
            .or_insert(0) += 1;
        for (code, n) in &attempt.errors {
            *errors.entry(code.clone()).or_insert(0) += n;
        }
    }
    // Attempts that never reached a turn (skipped, setup failed) have no
    // reading; `None` overall means `/metrics` was never readable.
    let measured: Vec<&Attempt> = attempts
        .iter()
        .filter(|a| a.screenshot_bytes.is_some())
        .collect();
    let screenshot_bytes = (!measured.is_empty()).then(|| {
        measured
            .iter()
            .filter_map(|a| a.screenshot_bytes)
            .sum::<u64>()
    });
    let shots: u32 = measured
        .iter()
        .map(|a| {
            let failed: u32 = a.errors.values().sum();
            (a.steps + a.start_calls).saturating_sub(failed)
        })
        .sum();
    let rate =
        |passed: usize, counted: usize| (counted > 0).then(|| passed as f64 / counted as f64);
    let tasks: Vec<TaskSummary> = suite
        .tasks
        .iter()
        .filter(|task| attempts.iter().any(|a| a.task_id == task.id))
        .map(|task| {
            let mine: Vec<&&Attempt> = counted.iter().filter(|a| a.task_id == task.id).collect();
            let won: Vec<&&&Attempt> = mine
                .iter()
                .filter(|a| a.outcome == Outcome::Passed)
                .collect();
            TaskSummary {
                task_id: task.id.clone(),
                category: task.category.clone(),
                counted: mine.len() as u32,
                passed: won.len() as u32,
                success_rate: rate(won.len(), mine.len()),
                median_steps_passed: median(
                    &mut won.iter().map(|a| f64::from(a.steps)).collect::<Vec<_>>(),
                ),
                reference_steps: task.reference_steps,
            }
        })
        .collect();
    let success_rate = rate(passed.len(), counted.len());
    let median_step_ratio = median(
        &mut passed
            .iter()
            .map(|a| f64::from(a.steps) / f64::from(a.reference_steps.max(1)))
            .collect::<Vec<_>>(),
    );
    let verdict = decide(
        &suite.verdict,
        &tasks,
        success_rate,
        median_step_ratio,
        &run.aborted,
    );
    let summary = Summary {
        attempts: attempts.len() as u32,
        counted: counted.len() as u32,
        passed: passed.len() as u32,
        success_rate,
        median_steps_passed: median(
            &mut passed
                .iter()
                .map(|a| f64::from(a.steps))
                .collect::<Vec<_>>(),
        ),
        median_steps_counted: median(
            &mut counted
                .iter()
                .map(|a| f64::from(a.steps))
                .collect::<Vec<_>>(),
        ),
        median_step_ratio,
        median_wall_ms: median(&mut counted.iter().map(|a| a.wall_ms as f64).collect::<Vec<_>>()),
        outcomes,
        errors,
        approvals: attempts.iter().map(|a| a.approvals.len() as u32).sum(),
        screenshot_bytes,
        mean_screenshot_bytes: screenshot_bytes
            .filter(|_| shots > 0)
            .map(|bytes| bytes as f64 / f64::from(shots)),
        input_tokens: attempts.iter().map(|a| a.turn.input_tokens).sum(),
        output_tokens: attempts.iter().map(|a| a.turn.output_tokens).sum(),
        verdict,
    };
    Report {
        run_id: info.run_id.to_string(),
        suite: suite.suite.clone(),
        suite_version: suite.version,
        provider: info.provider.to_string(),
        model: info.model.to_string(),
        started_at: info.started_at,
        finished_at: info.finished_at,
        summary,
        tasks,
        attempts,
        aborted: run.aborted,
    }
}

/// The suite's rule applied to the run. A run that stopped early, or a
/// task with fewer counted attempts than the rule asks for, gives no
/// verdict: a decision to build a native mapping should not rest on one
/// lucky or unlucky attempt.
fn decide(
    rule: &VerdictRule,
    tasks: &[TaskSummary],
    success_rate: Option<f64>,
    step_ratio: Option<f64>,
    aborted: &str,
) -> Verdict {
    let verdict = |decision: &str, reason: String| Verdict {
        decision: decision.into(),
        reason,
    };
    if !aborted.is_empty() {
        return verdict(
            "insufficient_data",
            format!("the run stopped early: {aborted}"),
        );
    }
    let thin: Vec<&str> = tasks
        .iter()
        .filter(|t| t.counted < rule.min_runs_per_task)
        .map(|t| t.task_id.as_str())
        .collect();
    if tasks.is_empty() || !thin.is_empty() {
        return verdict(
            "insufficient_data",
            format!(
                "every task needs {} counted attempts; short: {}",
                rule.min_runs_per_task,
                if thin.is_empty() {
                    "all".into()
                } else {
                    thin.join(", ")
                }
            ),
        );
    }
    let Some(rate) = success_rate else {
        return verdict("insufficient_data", "nothing was counted".into());
    };
    let ratio = step_ratio.unwrap_or(f64::INFINITY);
    let shown = |r: f64| {
        if r.is_finite() {
            format!("{r:.2}")
        } else {
            "n/a".into()
        }
    };
    if rate < rule.native_success_rate || ratio > rule.native_step_ratio {
        return verdict(
            "native_mapping_worth_measuring",
            format!(
                "success {:.0}% (native threshold < {:.0}%) or step ratio {} (> {:.2})",
                rate * 100.0,
                rule.native_success_rate * 100.0,
                shown(ratio),
                rule.native_step_ratio
            ),
        );
    }
    if rate >= rule.good_success_rate && ratio <= rule.good_max_step_ratio {
        return verdict(
            "good_enough",
            format!(
                "success {:.0}% (>= {:.0}%) and step ratio {} (<= {:.2})",
                rate * 100.0,
                rule.good_success_rate * 100.0,
                shown(ratio),
                rule.good_max_step_ratio
            ),
        );
    }
    verdict(
        "inconclusive",
        format!(
            "success {:.0}%, step ratio {}: between the thresholds; read the failures",
            rate * 100.0,
            shown(ratio)
        ),
    )
}

fn pct(rate: Option<f64>) -> String {
    rate.map_or_else(|| "n/a".into(), |r| format!("{:.0}%", r * 100.0))
}

fn num(value: Option<f64>) -> String {
    value.map_or_else(|| "n/a".into(), |v| format!("{v:.1}"))
}

/// The short summary a person reads first.
#[must_use]
pub fn markdown(report: &Report) -> String {
    let s = &report.summary;
    let mut out = String::new();
    out.push_str(&format!(
        "# CUA evaluation {}\n\nSuite `{}` v{} · provider `{}` · model `{}` · {} → {}\n\n",
        report.run_id,
        report.suite,
        report.suite_version,
        if report.provider.is_empty() {
            "default"
        } else {
            &report.provider
        },
        if report.model.is_empty() {
            "default"
        } else {
            &report.model
        },
        report.started_at,
        report.finished_at
    ));
    if !report.aborted.is_empty() {
        out.push_str(&format!("**Run stopped early:** {}\n\n", report.aborted));
    }
    out.push_str(&format!(
        "**Verdict: {}** — {}\n\n",
        s.verdict.decision, s.verdict.reason
    ));
    out.push_str("| Metric | Value |\n|---|---|\n");
    out.push_str(&format!(
        "| Success rate | {} ({} of {} counted; {} attempts) |\n",
        pct(s.success_rate),
        s.passed,
        s.counted,
        s.attempts
    ));
    out.push_str(&format!(
        "| Median steps (passed) | {} |\n",
        num(s.median_steps_passed)
    ));
    out.push_str(&format!(
        "| Median steps (counted) | {} |\n",
        num(s.median_steps_counted)
    ));
    out.push_str(&format!(
        "| Median step ratio (passed) | {} |\n",
        num(s.median_step_ratio)
    ));
    out.push_str(&format!(
        "| Median wall time | {} |\n",
        s.median_wall_ms
            .map_or_else(|| "n/a".into(), |ms| format!("{:.1} s", ms / 1000.0))
    ));
    out.push_str(&format!("| Approvals raised | {} |\n", s.approvals));
    out.push_str(&format!(
        "| Screenshot bytes | {} |\n",
        match (s.screenshot_bytes, s.mean_screenshot_bytes) {
            (Some(total), Some(mean)) => format!("{total} (mean {mean:.0} per screenshot)"),
            (Some(total), None) => total.to_string(),
            _ => "n/a (/metrics not readable)".into(),
        }
    ));
    out.push_str(&format!(
        "| Tokens in / out | {} / {} |\n\n",
        s.input_tokens, s.output_tokens
    ));
    out.push_str("## Outcomes\n\n");
    for (outcome, n) in &s.outcomes {
        out.push_str(&format!("- `{outcome}`: {n}\n"));
    }
    if !s.errors.is_empty() {
        out.push_str("\n## Tool errors by code\n\n");
        for (code, n) in &s.errors {
            out.push_str(&format!("- `{code}`: {n}\n"));
        }
    }
    out.push_str("\n## Tasks\n\n| Task | Category | Passed | Success | Median steps | Reference |\n|---|---|---|---|---|---|\n");
    for task in &report.tasks {
        out.push_str(&format!(
            "| {} | {} | {}/{} | {} | {} | {} |\n",
            task.task_id,
            task.category,
            task.passed,
            task.counted,
            pct(task.success_rate),
            num(task.median_steps_passed),
            task.reference_steps
        ));
    }
    let failed: Vec<&Attempt> = report
        .attempts
        .iter()
        .filter(|a| a.outcome != Outcome::Passed)
        .collect();
    if !failed.is_empty() {
        out.push_str("\n## Attempts that did not pass\n\n");
        for a in failed {
            let why = a
                .checks
                .iter()
                .filter(|c| !c.passed)
                .map(|c| c.detail.as_str())
                .chain((!a.cancelled.is_empty()).then_some(a.cancelled.as_str()))
                .chain((!a.setup_error.is_empty()).then_some(a.setup_error.as_str()))
                .chain((!a.turn.error.is_empty()).then_some(a.turn.error.as_str()))
                .collect::<Vec<_>>()
                .join("; ");
            out.push_str(&format!(
                "- {} r{}: `{}`, {} steps{}\n",
                a.task_id,
                a.repeat,
                a.outcome.as_str(),
                a.steps,
                if why.is_empty() {
                    String::new()
                } else {
                    format!(" — {why}")
                }
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_of_odd_even_and_none() {
        assert_eq!(median(&mut []), None);
        assert_eq!(median(&mut [3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&mut [4.0, 1.0, 2.0, 3.0]), Some(2.5));
    }

    fn rule() -> VerdictRule {
        VerdictRule {
            min_runs_per_task: 3,
            good_success_rate: 0.8,
            good_max_step_ratio: 2.0,
            native_success_rate: 0.6,
            native_step_ratio: 3.0,
        }
    }

    fn task(counted: u32) -> TaskSummary {
        TaskSummary {
            task_id: "a".into(),
            category: "edit".into(),
            counted,
            passed: 0,
            success_rate: None,
            median_steps_passed: None,
            reference_steps: 2,
        }
    }

    #[test]
    fn the_verdict_follows_the_rule() {
        let tasks = [task(3)];
        let d = |rate, ratio| decide(&rule(), &tasks, Some(rate), ratio, "").decision;
        assert_eq!(d(0.9, Some(1.5)), "good_enough");
        assert_eq!(d(0.8, Some(2.0)), "good_enough");
        assert_eq!(d(0.7, Some(1.5)), "inconclusive");
        assert_eq!(d(0.9, Some(2.5)), "inconclusive");
        assert_eq!(d(0.5, Some(1.0)), "native_mapping_worth_measuring");
        assert_eq!(d(0.9, Some(3.5)), "native_mapping_worth_measuring");
        assert_eq!(d(0.0, None), "native_mapping_worth_measuring");
        assert_eq!(
            decide(&rule(), &[task(2)], Some(1.0), Some(1.0), "").decision,
            "insufficient_data"
        );
        assert_eq!(
            decide(&rule(), &tasks, Some(1.0), Some(1.0), "no device").decision,
            "insufficient_data"
        );
    }
}
