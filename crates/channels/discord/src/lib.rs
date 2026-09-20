//! Discord channel connector (port of daemon/internal/connectors/discord).
//!
//! The crate provides the Discord Config, the transport traits and the
//! GatewayTransport (REST via ureq + a tokio-tungstenite gateway receive
//! loop), the hosted-setup readiness evaluation, capability conformance
//! declarations, diagnostics classification, smoke evidence, and the
//! Runtime that wires a transport into the shared kura-im message loop,
//! the connector supervisor, the SQLite store, and the event bus.
//!
//! Pure logic — message normalization, route gating, destination validation,
//! conformance, diagnostics, and readiness — is fully unit-tested; the
//! runtime integration behavior is covered in tests/runtime.rs with a fake
//! transport. See rs/MIGRATION.md for the porting conventions.

pub use kura_connectors::RedactionStatus;

/// The Redacted default for connector evidence records (Go's zero value).
pub(crate) fn redaction_status_redacted() -> RedactionStatus {
    RedactionStatus::Redacted
}

mod config;
mod conformance;
mod destinations;
mod diagnostics;
mod gateway;
mod readiness;
mod runtime;
mod smoke;
mod transport;

pub use config::Config;
pub use conformance::{conformance_profile, conformance_profile_for_setup, support_flag};
pub use destinations::{
    DestinationType, DestinationValidation, DestinationValidationState,
    has_explicit_hosted_destination, selected_destinations_valid,
};
pub use diagnostics::{
    build_diagnostic_state, classify_discord_error, classify_discord_error_message,
    diagnostic_reason_for_error, diagnostic_reason_for_error_message,
};
pub use readiness::{
    CredentialState, HostedSetup, HostedSetupInput, ReadinessState, evaluate_hosted_setup,
};
pub use runtime::{Runtime, new_runtime};
pub use smoke::{CredentialMode, SmokeEvidence, SmokeInput, SmokeStatus, build_smoke_evidence};
pub use transport::{
    DestinationValidator, DiscordError, GatewayTransport, LifecycleObservableTransport, Transport,
    TransportLifecycleEvent, redact_discord_route, redacted_discord_label, strip_bot_mention,
    wrap_discord_error,
};
