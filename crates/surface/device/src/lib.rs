//! `kura-device`: lets a Kura agent use this computer
//! (docs/harness/remote-computer-use.md).
//!
//! The device dials out to its cell and keeps one WebSocket open; nothing
//! listens on this computer. The agent can do nothing until the person
//! grants a session here, and the rules that protect the person live in
//! [`agent`], on this side of the wire.

pub mod agent;
pub mod config;
pub use kura_device_protocol::keys;
pub mod link;
pub mod log;
pub mod pair;
pub mod platform;
pub mod shot;
