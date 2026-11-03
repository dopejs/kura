//! Stage derivation (work-items.md §4.1).
//!
//! The stage is computed from stored facts on every read, so a restart
//! keeps it and nothing can leave it stale. The first matching row wins.

use crate::types::{ReportStatus, Stage};

/// The stored facts a stage is derived from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StageFacts {
    pub ended: bool,
    /// The last round ended without a report (the turn failed or was
    /// refused), or an outbound message failed 3 times in a row (slice 3).
    pub delivery_problem: bool,
    /// A pending approval is linked as attention (slice 5).
    pub approval_pending: bool,
    pub attention: bool,
    pub last_report: Option<ReportStatus>,
    /// A person's input arrived after the last report.
    pub person_input_since_report: bool,
    pub pending_inputs: bool,
    pub round_running: bool,
}

/// The item's stage; `None` once it has ended.
#[must_use]
pub fn derive_stage(facts: &StageFacts) -> Option<Stage> {
    if facts.ended {
        return None;
    }
    let held_by_report = !facts.person_input_since_report;
    Some(if facts.delivery_problem {
        Stage::PersonDelivery
    } else if facts.approval_pending {
        Stage::PersonApproval
    } else if facts.attention {
        Stage::PersonAnswer
    } else if facts.last_report == Some(ReportStatus::Blocked) && held_by_report {
        Stage::PersonBlocked
    } else if facts.last_report == Some(ReportStatus::Failed) && held_by_report {
        Stage::PersonFailed
    } else if facts.pending_inputs && !facts.round_running {
        Stage::Queued
    } else if facts.round_running {
        Stage::Working
    } else {
        // The last report is `waiting`, or `progress` with nothing pending,
        // or nothing above holds: the item waits for an event.
        Stage::Idle
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> StageFacts {
        StageFacts::default()
    }

    /// Every row of the §4.1 table, in order, each with every later row's
    /// condition also true: the first match must win.
    #[test]
    fn the_first_matching_row_wins() {
        let everything = StageFacts {
            ended: false,
            delivery_problem: true,
            approval_pending: true,
            attention: true,
            last_report: Some(ReportStatus::Blocked),
            person_input_since_report: false,
            pending_inputs: true,
            round_running: true,
        };
        let mut f = everything;
        assert_eq!(derive_stage(&f), Some(Stage::PersonDelivery));
        f.delivery_problem = false;
        assert_eq!(derive_stage(&f), Some(Stage::PersonApproval));
        f.approval_pending = false;
        assert_eq!(derive_stage(&f), Some(Stage::PersonAnswer));
        f.attention = false;
        assert_eq!(derive_stage(&f), Some(Stage::PersonBlocked));
        f.last_report = Some(ReportStatus::Failed);
        assert_eq!(derive_stage(&f), Some(Stage::PersonFailed));
        // A person's input since the report releases the hold.
        f.person_input_since_report = true;
        f.round_running = false;
        assert_eq!(derive_stage(&f), Some(Stage::Queued));
        f.round_running = true;
        assert_eq!(derive_stage(&f), Some(Stage::Working));
        f.pending_inputs = false;
        assert_eq!(derive_stage(&f), Some(Stage::Working));
        f.round_running = false;
        assert_eq!(derive_stage(&f), Some(Stage::Idle));
    }

    #[test]
    fn an_ended_item_has_no_stage_whatever_else_holds() {
        let f = StageFacts {
            ended: true,
            delivery_problem: true,
            attention: true,
            round_running: true,
            ..facts()
        };
        assert_eq!(derive_stage(&f), None);
    }

    #[test]
    fn waiting_and_progress_reports_idle_until_something_arrives() {
        for status in [ReportStatus::Waiting, ReportStatus::Progress] {
            let idle = StageFacts {
                last_report: Some(status),
                ..facts()
            };
            assert_eq!(derive_stage(&idle), Some(Stage::Idle), "{status:?}");
            let woken = StageFacts {
                pending_inputs: true,
                ..idle
            };
            assert_eq!(derive_stage(&woken), Some(Stage::Queued), "{status:?}");
        }
    }

    #[test]
    fn a_blocked_report_holds_until_a_person_speaks_not_until_any_input() {
        let blocked = StageFacts {
            last_report: Some(ReportStatus::Blocked),
            pending_inputs: true,
            ..facts()
        };
        // A bot's or a webhook's input does not release a person hold.
        assert_eq!(derive_stage(&blocked), Some(Stage::PersonBlocked));
        let answered = StageFacts {
            person_input_since_report: true,
            ..blocked
        };
        assert_eq!(derive_stage(&answered), Some(Stage::Queued));
    }

    #[test]
    fn a_first_round_running_is_working() {
        let f = StageFacts {
            round_running: true,
            ..facts()
        };
        assert_eq!(derive_stage(&f), Some(Stage::Working));
    }
}
