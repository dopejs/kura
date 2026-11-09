//! kura-devices — the user's own computers, for remote computer use
//! (docs/harness/remote-computer-use.md).
//!
//! - [`registry`]: pairing codes, device tokens, revocation (stored).
//! - [`hub`]: live connections, control sessions and actions (in memory).
//! - [`connection`]: one device connection's loop, transport-agnostic.
//! - [`rules`]: which tenants may use it, and which actions need approval.

pub mod connection;
pub mod hub;
pub mod registry;
pub mod rules;

pub use connection::{Closed, ConnectionConfig, serve};
pub use hub::{DeviceError, Hub, HubConfig, OnlineDevice};
pub use registry::{Paired, PairingCode, RegistryError};
pub use rules::RemoteGate;
