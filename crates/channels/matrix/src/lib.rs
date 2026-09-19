//! Port of the daemon/internal/connectors/matrix package: the hosted-safe
//! Matrix channel connector boundary (Phase 52).
//!
//! Supports tenant-provided Matrix bot accounts on tenant-selected homeservers,
//! unencrypted text direct messages, selected unencrypted rooms with bot
//! mention or configured command gates, durable homeserver/conversation/event
//! dedupe, final-only foreground replies, connector-backed delivery evidence,
//! and redacted diagnostics. It does not operate Matrix homeservers, provision
//! Matrix accounts, support encrypted rooms, or implement WhatsApp fallback
//! behavior.
//!
//! Ported from the Go package with the rs/MIGRATION.md conventions: snake_case
//! methods, serde camelCase wire names, thiserror error enums, chrono
//! timestamps, and no unwrap/expect outside tests. The Go tenant context is
//! replaced by an explicit tenant id passed to
//! [Runtime::start] / [crate::new_runtime].

pub mod conformance;
pub mod dedupe;
pub mod diagnostics;
pub mod provider_decision;
pub mod readiness;
pub mod redaction;
pub mod reply;
pub mod routes;
pub mod runtime;
pub mod setup;
pub mod smoke;
pub mod transport;
pub mod transport_client;
pub mod types;
pub mod unsupported;

pub use conformance::{conformance_profile, support_flag};
pub use dedupe::{DedupeCache, dedupe_key, new_dedupe_cache};
pub use diagnostics::{DiagnosticInput, DiagnosticState, MatrixCondition, map_condition};
pub use provider_decision::{
    ProviderDecision, ProviderDecisionError, phase52_provider_decision, validate_provider_decision,
};
pub use readiness::{
    ERR_HOMESERVER_BINDING_INVALID, homeserver_state, normalize_homeserver_binding,
    validate_homeserver_binding,
};
pub use redaction::redact_evidence;
pub use reply::send_final_reply;
pub use routes::{decide_route, has_ready_route_policy, normalize_route_policy};
pub use runtime::{Runtime, matrix_event_identity_key, new_runtime, normalize_inbound_event};
pub use setup::{evaluate_hosted_setup, reason_for_bot_credential};
pub use smoke::{
    SafeLiveSmokeInput, SmokeAuthorizationMode, SmokeEvidence, SmokeStatus, SmokeTransport,
    execute_safe_live_smoke, structured_skip_smoke_evidence,
};
pub use transport::{FakeTransport, Transport};
pub use transport_client::{
    AccessTokenProvider, ClientApiError, ClientError, ClientTransport, ClientTransportConfig,
    new_client_transport,
};
pub use types::*;
pub use unsupported::unsupported_message_kind;

/// String enum with explicit per-variant wire literals: every variant's serde
/// representation is exactly the literal, and `as_str`/`Display` agree with
/// it. The first variant carries `#[default]`, mirroring the normalized Go
/// zero value where the package defaults empty strings to that state.
macro_rules! string_enum {
    ($name:ident { $first:ident => $first_s:literal $(, $v:ident => $s:literal)* $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
        pub enum $name {
            #[default]
            #[serde(rename = $first_s)]
            $first,
            $(
                #[serde(rename = $s)]
                $v
            ),*
        }
        impl $name {
            #[must_use]
            pub fn as_str(self) -> &'static str {
                match self {
                    $name::$first => $first_s,
                    $( $name::$v => $s ),*
                }
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}
pub(crate) use string_enum;

/// Go `time.Time.IsZero()` ported with the workspace epoch convention
/// (1970-01-01T00:00:00Z is the unset timestamp).
#[must_use]
pub(crate) fn is_unset_time(dt: &chrono::DateTime<chrono::Utc>) -> bool {
    dt.timestamp() == 0 && dt.timestamp_subsec_nanos() == 0
}
