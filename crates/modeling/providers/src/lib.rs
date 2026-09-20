//! Port of `daemon/internal/providers`: the LLM provider registry, profile
//! projection, managed-auth lifecycle, and dispatch resolution (Roadmap 9/10).

mod manager;
mod types;

pub use manager::{
    Check, CheckInput, Manager, ResolvedDispatch, SyncResult, new_check_id, new_manager,
};
pub use types::*;
