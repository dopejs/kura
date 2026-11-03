//! kura-workitems — long-lived work (docs/harness/work-items.md).
//!
//! A work item is work that can outlive one turn: it holds an immutable
//! mandate (why the agent may do it), runs in rounds, and each round is an
//! ordinary chat turn whose final answer is a structured report. The stage
//! an item is in is derived from its stored rows on every read.
//!
//! Slice 1: items, the mandate and the API, no wake. An item is opened
//! through `POST /v1/work-items` and runs one round; subscriptions, the IM
//! hook, the outbox, restart recovery and attention answers arrive in the
//! later slices. Off unless `entries.work-items.config.enabled` is true.

mod manager;
mod report;
mod stage;
mod types;

pub use manager::{
    MAX_LIST_LIMIT, Manager, OpenRequest, RoundOutcome, RoundTurn, WorkItemError, round_request_id,
};
pub use report::{REPORT_FORMAT_NAME, read_report, report_schema, round_query, unreadable_report};
pub use stage::{StageFacts, derive_stage};
pub use types::{
    Attention, Basis, EndReason, Input, InputKind, InputState, ItemState, Mandate, MandateSource,
    MandateSourceKind, MatchedRule, Report, ReportReason, ReportStatus, Round, RoundState, Sender,
    Stage, Strength, WorkItem, WorkItemsConfig,
};
