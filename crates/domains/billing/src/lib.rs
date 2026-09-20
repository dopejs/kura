//! Port of `daemon/internal/billing`: quota/metering domain types, limit
//! checks, and usage accounting logic. See `rs/MIGRATION.md` for conventions.
//!
//! The crate is storage-agnostic: persistence lives behind the [`Repository`]
//! trait (the Go store layer lands in wave 3 and will implement it).

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

mod admin;
mod catalog;
mod denial;
mod error;
mod lifecycle;
mod manager;
mod operation_key;
mod preflight;
mod projection;
mod recovery;
mod types;

#[cfg(test)]
mod fixtures;

pub use admin::ResolveReservationInput;
pub use catalog::{
    CatalogEntry, CatalogExport, PERIOD_ANCHOR_UTC, REASON_QUOTA_STATE_UNAVAILABLE, definition_for,
    export_catalog, initial_catalog, initial_definitions, required_categories,
};
pub use denial::{
    DenialPayload, build_evidence_export, classify_denial, new_quota_exhausted_denial,
    new_quota_state_unavailable_denial, project_denial_detail, safe_operation_ref,
};
pub use error::BillingError;
pub use lifecycle::ResolveInput;
pub use manager::{
    BoxFuture, Manager, Repository, ReserveAllResult, ReserveInput, ReserveResult, development_plan,
};
pub use operation_key::{
    artifact_operation_key, evaluation_operation_key, integration_operation_key,
    live_validation_operation_key, run_operation_key, tool_call_operation_key,
    workflow_operation_key,
};
pub use preflight::{LIVE_VALIDATION_PREFLIGHT_ENTRY_POINT, reserve_live_validation_preflight};
pub use projection::{
    EffectiveQuota, UsageSummary, build_quota_status_item,
    category_defined_typical_operation_amount, group_quota_status_items, is_quota_near_limit,
    near_limit_reason_for_quota, period_for, project_quota, recovery_actions_for_quota_status,
};
pub use recovery::RecoveryDecision;
pub use types::*;
