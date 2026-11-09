//! kura-workitems — long-lived work (docs/harness/work-items.md).
//!
//! A work item is work that can outlive one turn: it holds an immutable
//! mandate (why the agent may do it), runs in rounds, and each round is an
//! ordinary chat turn whose final answer is a structured report. The stage
//! an item is in is derived from its stored rows on every read.
//!
//! Slice 1: items, the mandate and the API. Slice 2: the `work_item.open`
//! tool, owner subscriptions to IM conversations, the IM hook that wakes an
//! item on its requester's message (steering a running round, or running
//! the next one), resume of an ended item after its mandate is revalidated,
//! and stop. Slice 3: the outbox, which sends an item's replies to a
//! conversation in order and retries them across restarts. Restart
//! recovery, attention answers, rules and webhooks arrive in the later
//! slices. Off unless `entries.work-items.config.enabled` is true.

mod driver;
mod im;
mod manager;
mod mandate;
mod outbox;
mod report;
mod stage;
mod tool;
mod types;
mod wake;

pub use driver::{Drive, DriveStop, DrivenRound};
pub use im::{ImHook, round_reply};
pub use manager::{
    MAX_LIST_LIMIT, Manager, OpenRequest, RoundOutcome, RoundTurn, WorkItemError, check_open,
    round_request_id,
};
pub use outbox::{
    Flushed, OUTBOX_FIRST_BACKOFF_SECS, OUTBOX_MAX_BACKOFF_SECS, ReplyBinding, outbox_backoff,
};
pub use report::{
    QuotedInput, REPORT_FORMAT_NAME, frame_input, read_report, report_schema, round_instructions,
    round_query, unreadable_report,
};
pub use stage::{StageFacts, derive_stage};
pub use tool::{OPEN_TOOL, OpenTool, is_round_thread};
pub use types::{
    Attention, Basis, EndReason, Input, InputKind, InputState, ItemState, Mandate, MandateSource,
    MandateSourceKind, MatchedRule, OutboxDropReason, OutboxRow, OutboxState, Report, ReportReason,
    ReportStatus, RevokeReason, Round, RoundState, Sender, Stage, Strength, Subscription,
    SubscriptionRole, WorkItem, WorkItemsConfig,
};
pub use wake::InputPath;
