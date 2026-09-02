//! Wire and domain types shared between the agent core and its clients.
//!
//! Pure types only: no I/O, no async, no provider specifics. Layering mirrors
//! `codex-rs/protocol`: everything here must stay cheap to serialize and safe
//! to persist as runtime evidence.

mod ids;
mod protocol;

pub use ids::{ProfileId, RunId, TenantId, ThreadId};
pub use protocol::{Event, EventMsg, Op, ResponseItem, Role, ToolSpec};
