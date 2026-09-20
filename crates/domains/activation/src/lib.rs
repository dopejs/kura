//! Port of `daemon/internal/activation`: first-run activation and
//! quota-enforcement gating domain logic. See `rs/MIGRATION.md` for
//! conventions.
//!
//! The [`Service`] orchestrates personal-tenant resolution, quota baseline
//! projection, readiness reporting, and the activation test chat. Persistence
//! and side effects live behind the [`StateStore`], [`IdentityRepository`],
//! [`BillingProjector`], [`ChatRunner`], and [`AuditSink`] traits; the
//! SQLite-backed adapters (wave 8 parity) live in [`sqlite`];
//! [`Service::with_sqlite`] wires them into the service.

macro_rules! define_string_enum {
    ($(#[$meta:meta])* $name:ident { $( $const_name:ident => $value:literal ),+ $(,)? }) => {
        $(#[$meta])*
        ///
        /// Open string enum (Go `type X string`): known values are exposed as
        /// associated constants, but arbitrary persisted values round-trip
        /// unchanged.
        #[derive(Debug, Clone, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            $( pub const $const_name: &'static str = $value; )+

            #[must_use]
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            #[must_use]
            pub fn is_empty(&self) -> bool {
                self.0.is_empty()
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_string())
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl PartialEq<&str> for $name {
            fn eq(&self, other: &&str) -> bool {
                self.0 == *other
            }
        }

        impl PartialEq<$name> for &str {
            fn eq(&self, other: &$name) -> bool {
                *self == other.0
            }
        }
    };
}

mod audit;
mod diagnostics;
mod error;
mod readiness;
mod service;
mod sqlite;
mod test_chat;
mod types;

#[cfg(test)]
mod testutil;

pub use error::ActivationError;
pub use error::Error;
pub use error::StoreError;
pub use error::reason_code_from_error;
pub use service::ActivateInput;
pub use service::AuditSink;
pub use service::BillingProjector;
pub use service::BoxFuture;
pub use service::ChatRunner;
pub use service::Dependencies;
pub use service::GetInput;
pub use service::IdentityRepository;
pub use service::RunTestChatInput;
pub use service::Service;
pub use service::StateStore;
pub use service::stable_activation_id;
pub use sqlite::BillingProjectorAdapter;
pub use sqlite::ChatRunnerAdapter;
pub use sqlite::SqliteActivationStore;
pub use test_chat::ChatRunFailure;
pub use test_chat::RunTestChatFailure;
pub use test_chat::TestChatInput;
pub use test_chat::TestChatResult;
pub use types::*;
