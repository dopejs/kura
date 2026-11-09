//! `kura-cua-eval`: measures how well the agent uses a person's computer
//! through the generic `computer.*` tools (docs/harness/remote-computer-use.md
//! §12, slice 5).
//!
//! A suite ([`suite`]) is a fixed, versioned set of desktop tasks. The
//! [`runner`] drives each one through a running daemon's ordinary chat
//! route, with the person's real `kura-device` connected, and reads the
//! turn's `chat.tool.called` events and approvals while it runs. Success is
//! decided by [`checks`] of the computer's state, never by what the model
//! says it did. [`report`] turns the attempts into a success rate, step
//! counts and a verdict on whether a native computer-use mapping is worth
//! building.
//!
//! The runner is only a client of the daemon's HTTP API; it needs no access
//! to the daemon's data directory.

pub mod checks;
pub mod client;
pub mod desktop;
pub mod report;
pub mod runner;
pub mod suite;
