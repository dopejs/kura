//! Port of `daemon/internal/threads`. See rs/MIGRATION.md for conventions.
//!
//! Thread/session lifecycle models and transitions: lifecycle state machine,
//! source linkage and continuation keys, runtime projections, continuity
//! windows, group/room participation, handoff links, and redaction-safe
//! evidence summaries.
//!
//! ID fields are plain `String` (not `kura_protocol::ThreadId`/`TenantId`):
//! the Go daemon stores connector-derived thread IDs (Slack message IDs,
//! Matrix conversation IDs, `thread_*` strings) that are not UUIDs, so the
//! UUIDv7 newtypes would reject real persisted data.

mod continuity;
mod error;
mod group_room;
mod handoff;
mod lifecycle;
mod projection;
mod redaction;
mod source;

pub use continuity::*;
pub use error::ThreadsError;
pub use group_room::*;
pub use handoff::*;
pub use lifecycle::*;
pub use projection::*;
pub use redaction::*;
pub use source::*;

pub(crate) fn utc_now_or(
    now: Option<chrono::DateTime<chrono::Utc>>,
) -> chrono::DateTime<chrono::Utc> {
    now.unwrap_or_else(chrono::Utc::now)
}
