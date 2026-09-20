//! Domain types ported from daemon/internal/profiles/profile.go.
//!
//! Field order and serde attributes reproduce the Go JSON tags exactly,
//! including Go's `omitempty` semantics (empty string / empty slice / nil
//! pointer omitted). Container-level `#[serde(default)]` reproduces Go's
//! missing-field → zero-value decode.

use std::borrow::Cow;
use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Defines a Go `type X string` mirror: a newtype over `Cow<'static, str>`
/// with associated constants for the known values. Unknown values survive
/// JSON round-trips so validation layers can reject them with reason codes.
macro_rules! string_kind {
    (
        $(#[$meta:meta])*
        $name:ident {
            $($(#[$cmeta:meta])* $const:ident = $value:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
        pub struct $name(Cow<'static, str>);

        impl $name {
            $($(#[$cmeta])*
            pub const $const: $name = $name(Cow::Borrowed($value));)+

            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// True when the kind holds Go's zero value ("").
            pub fn is_empty(&self) -> bool {
                self.0.is_empty()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<&'static str> for $name {
            fn from(value: &'static str) -> Self {
                Self(Cow::Borrowed(value))
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(Cow::Owned(value))
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl std::borrow::Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
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

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                Ok(Self(Cow::Owned(String::deserialize(deserializer)?)))
            }
        }
    };
}

string_kind! {
    /// Lifecycle status of an agent profile.
    Status {
        DRAFT = "draft",
        ACTIVE = "active",
        ARCHIVED = "archived",
        DISABLED = "disabled",
    }
}

string_kind! {
    /// Kind of change recorded by a profile version.
    ChangeKind {
        CREATED = "created",
        UPDATED = "updated",
        ACTIVATED = "activated",
        ROLLED_BACK = "rolled_back",
        ARCHIVED = "archived",
        DISABLED = "disabled",
        VALIDATED = "validated",
    }
}

string_kind! {
    /// Whether a profile version may be rolled back to, and if not, why.
    RollbackEligibility {
        ELIGIBLE = "eligible",
        INVALID_PROVIDER = "invalid_provider",
        INVALID_OVERLAY = "invalid_overlay",
        POLICY_BLOCKED = "policy_blocked",
        PROFILE_ARCHIVED = "profile_archived",
        PROFILE_DISABLED = "profile_disabled",
        REDACTION_FAILED = "redaction_failed",
    }
}

string_kind! {
    /// Redaction outcome for a record crossing a persistence/API boundary.
    RedactionStatus {
        REDACTED = "redacted",
        SUPPRESSED = "suppressed",
        FAILED = "redaction_failed",
    }
}

string_kind! {
    /// Validation state of an overlay or overlay-derived preference.
    OverlayValidationState {
        VALID = "valid",
        PARTIAL = "partial",
        MISSING = "missing",
        PERMISSION_DENIED = "permission_denied",
        OUT_OF_SCOPE = "out_of_scope",
        TOO_LARGE = "too_large",
        UNSAFE_CONTENT = "unsafe_content",
        REDACTION_FAILED = "redaction_failed",
    }
}

string_kind! {
    /// Runtime resource a profile projection is anchored to.
    RuntimeResourceKind {
        THREAD = "thread",
        SESSION = "session",
        RUN = "run",
        WORKFLOW = "workflow",
        HANDOFF_DESTINATION = "handoff_destination",
    }
}

string_kind! {
    /// Why an active selection came to be.
    SelectionReason {
        DEFAULT_SEEDED = "default_seeded",
        USER_ACTIVATED = "user_activated",
        ROLLBACK_ACTIVATED = "rollback_activated",
        SYSTEM_FALLBACK = "system_fallback",
    }
}

/// Selection scope recorded for the tenant-default profile.
pub const SELECTION_SCOPE_TENANT_DEFAULT: &str = "tenant_default";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DisplayIdentity {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub safe_summary: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Persona {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tone: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub instructions: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub safe_summary: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DefaultProviderPreference {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub provider_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reasoning_level: String,
    #[serde(skip_serializing_if = "OverlayValidationState::is_empty")]
    pub validation_state: OverlayValidationState,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub failure_reason_code: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SafetyDefaults {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub approval_posture: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub risk_tolerance: String,
    #[serde(skip_serializing_if = "OverlayValidationState::is_empty")]
    pub validation_state: OverlayValidationState,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub failure_reason_code: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OverlayReference {
    pub overlay_reference_id: String,
    pub profile_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub profile_version_id: String,
    pub tenant_id: String,
    pub reference_kind: String,
    pub scope: String,
    pub reference_uri: String,
    pub safe_display_label: String,
    pub validation_state: OverlayValidationState,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub failure_reason_code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_validated_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub redaction_status: RedactionStatus,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentProfile {
    pub profile_id: String,
    pub tenant_id: String,
    pub display_name: String,
    pub display_identity: DisplayIdentity,
    pub persona: Persona,
    pub default_provider_preference: DefaultProviderPreference,
    pub safety_defaults: SafetyDefaults,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub legacy_mapping_evidence: Vec<LegacyMappingEvidence>,
    pub status: Status,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub active_version_id: String,
    pub tenant_default: bool,
    pub overlay_reference_count: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub created_by_principal_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub updated_by_principal_id: String,
    pub redaction_status: RedactionStatus,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LegacyMappingEvidence {
    pub source_kind: String,
    pub mapping_state: OverlayValidationState,
    pub reason_code: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub safe_summary: String,
    pub redaction_status: RedactionStatus,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProfileVersion {
    pub profile_version_id: String,
    pub profile_id: String,
    pub tenant_id: String,
    pub version_number: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub source_version_id: String,
    pub change_kind: ChangeKind,
    pub change_summary: String,
    pub snapshot: AgentProfile,
    pub rollback_eligibility: RollbackEligibility,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub actor_principal_id: String,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub audit_event_id: String,
    pub redaction_status: RedactionStatus,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ActiveSelection {
    pub selection_id: String,
    pub tenant_id: String,
    pub profile_id: String,
    pub profile_version_id: String,
    pub selection_scope: String,
    pub selection_reason: SelectionReason,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub selected_by_principal_id: String,
    pub selected_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub audit_event_id: String,
    pub redaction_status: RedactionStatus,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RuntimeProjection {
    pub runtime_profile_projection_id: String,
    pub tenant_id: String,
    pub profile_id: String,
    pub profile_version_id: String,
    pub selection_id: String,
    pub resource_kind: RuntimeResourceKind,
    pub resource_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub thread_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub session_segment_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub run_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub workflow_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub handoff_link_id: String,
    pub selection_scope: String,
    pub selection_reason: SelectionReason,
    pub safe_display_name: String,
    pub safe_summary: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub configuration_scope: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub deferred_binding_classification: String,
    pub occurred_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retention_expires_at: Option<DateTime<Utc>>,
    pub redaction_status: RedactionStatus,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AuditEvent {
    pub audit_event_id: String,
    pub tenant_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub profile_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub profile_version_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub actor_principal_id: String,
    pub event_kind: String,
    pub outcome: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub permission_gate: String,
    pub reason_code: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub safe_summary: String,
    pub occurred_at: DateTime<Utc>,
    pub redaction_status: RedactionStatus,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProfileDetail {
    pub profile: AgentProfile,
    pub versions: Vec<ProfileVersion>,
    pub overlay_references: Vec<OverlayReference>,
    pub audit_events: Vec<AuditEvent>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListResponse {
    pub tenant_id: String,
    pub page: Page,
    pub items: Vec<AgentProfile>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Page {
    pub limit: i64,
    pub next_cursor: String,
    pub order: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MutationInput {
    pub display_name: String,
    pub display_identity: DisplayIdentity,
    pub persona: Persona,
    pub default_provider_preference: DefaultProviderPreference,
    pub safety_defaults: SafetyDefaults,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub legacy_mapping_evidence: Vec<LegacyMappingEvidence>,
    pub overlay_references: Vec<OverlayReferenceInput>,
    pub activate: bool,
    pub reason_code: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OverlayReferenceInput {
    pub reference_kind: String,
    pub reference_uri: String,
    pub scope: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ActivationInput {
    pub profile_version_id: String,
    pub reason_code: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RollbackInput {
    pub source_profile_version_id: String,
    pub reason_code: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RetirementInput {
    pub reason_code: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MutationResult {
    pub profile: AgentProfile,
    pub version: ProfileVersion,
    // Go tag is `json:"selection,omitempty"`, but encoding/json never omits a
    // struct value: "selection" is always serialized. Keep it unconditional.
    pub selection: ActiveSelection,
    pub audit_event_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_constants_cover_runtime_and_rollback_states() {
        assert_eq!(Status::ACTIVE, "active");
        assert_eq!(ChangeKind::ROLLED_BACK, "rolled_back");
        assert_eq!(RollbackEligibility::INVALID_OVERLAY, "invalid_overlay");
        assert_eq!(
            OverlayValidationState::PERMISSION_DENIED,
            "permission_denied"
        );
        assert_eq!(
            RuntimeResourceKind::HANDOFF_DESTINATION,
            "handoff_destination"
        );
    }

    #[test]
    fn string_kinds_round_trip_unknown_values() {
        let state: OverlayValidationState =
            serde_json::from_str("\"future_state\"").expect("unknown state must decode");
        assert_eq!(state.as_str(), "future_state");
        assert_eq!(
            serde_json::to_string(&state).expect("encode"),
            "\"future_state\""
        );
        assert!(OverlayValidationState::default().is_empty());
    }
}
