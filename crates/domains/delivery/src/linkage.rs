//! Port of `daemon/internal/delivery/linkage.go`: latest-delivery summaries surfaced on the
//! runtime/run and workflow/schedule surfaces.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::manager::{DeliveryError, ManagerInner};
use crate::{DeliveryOutcome, OutcomeFilter};

/// Latest delivery summary for a run/workflow/schedule-attempt surface (port of
/// `LatestSummary`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LatestSummary {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub latest_delivery_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub latest_delivery_status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub latest_delivery_target_id: String,
}

impl ManagerInner {
    /// Port of `LatestSummaryForRun`: the most recently updated outcome for the run source.
    pub fn latest_summary_for_run(
        &self,
        run_id: &str,
    ) -> Result<(LatestSummary, bool), DeliveryError> {
        let items = self.list_outcomes(&OutcomeFilter {
            source_kind: "run".to_string(),
            run_id: run_id.trim().to_string(),
            ..OutcomeFilter::default()
        })?;
        match items.first() {
            Some(first) => Ok((latest_summary_from_outcome(first), true)),
            None => Ok((LatestSummary::default(), false)),
        }
    }

    /// Port of `LatestSummaryForWorkflow`.
    pub fn latest_summary_for_workflow(
        &self,
        workflow_id: &str,
    ) -> Result<(LatestSummary, bool), DeliveryError> {
        let items = self.list_outcomes(&OutcomeFilter {
            source_kind: "workflow".to_string(),
            workflow_id: workflow_id.trim().to_string(),
            ..OutcomeFilter::default()
        })?;
        match items.first() {
            Some(first) => Ok((latest_summary_from_outcome(first), true)),
            None => Ok((LatestSummary::default(), false)),
        }
    }

    /// Port of `LatestSummariesForScheduleAttempts`: first outcome per schedule attempt.
    pub fn latest_summaries_for_schedule_attempts(
        &self,
        schedule_id: &str,
    ) -> Result<HashMap<String, LatestSummary>, DeliveryError> {
        let items = self.list_outcomes(&OutcomeFilter {
            schedule_id: schedule_id.trim().to_string(),
            ..OutcomeFilter::default()
        })?;
        let mut summaries = HashMap::new();
        for item in items {
            if item.schedule_attempt_id.trim().is_empty() {
                continue;
            }
            if summaries.contains_key(&item.schedule_attempt_id) {
                continue;
            }
            summaries.insert(
                item.schedule_attempt_id.clone(),
                latest_summary_from_outcome(&item),
            );
        }
        Ok(summaries)
    }
}

/// Port of `latestSummaryFromOutcome`.
#[must_use]
pub fn latest_summary_from_outcome(outcome: &DeliveryOutcome) -> LatestSummary {
    LatestSummary {
        latest_delivery_id: outcome.delivery_id.clone(),
        latest_delivery_status: outcome.status.as_str().to_string(),
        latest_delivery_target_id: outcome.chosen_target_id.clone(),
    }
}

impl crate::Manager {
    /// Port of `LatestSummaryForRun`.
    pub fn latest_summary_for_run(
        &self,
        run_id: &str,
    ) -> Result<(LatestSummary, bool), DeliveryError> {
        self.inner.latest_summary_for_run(run_id)
    }

    /// Port of `LatestSummaryForWorkflow`.
    pub fn latest_summary_for_workflow(
        &self,
        workflow_id: &str,
    ) -> Result<(LatestSummary, bool), DeliveryError> {
        self.inner.latest_summary_for_workflow(workflow_id)
    }

    /// Port of `LatestSummariesForScheduleAttempts`.
    pub fn latest_summaries_for_schedule_attempts(
        &self,
        schedule_id: &str,
    ) -> Result<HashMap<String, LatestSummary>, DeliveryError> {
        self.inner
            .latest_summaries_for_schedule_attempts(schedule_id)
    }
}
