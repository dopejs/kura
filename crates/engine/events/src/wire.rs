//! Wire-string mapping for the `kura-threads` enums used in event payloads.
//!
//! The threads crate models Go's `type X string` vocabularies as closed Rust
//! enums (serde `snake_case`) without `as_str`/`Display` impls. These helpers
//! reproduce exactly the string `serde` would emit for each variant, so event
//! payloads stay byte-identical to the Go builders. A test in
//! `tests/constructors.rs` locks each mapping against `serde_json` output.

use kura_threads::{
    ContinuityStatus, ConversationShape, HandoffSourceReferenceStatus, HandoffStatus,
    LifecycleActionKind, ParticipationDecisionValue, RedactionStatus, ResetEventStatus,
    RoutingOutcome, RuntimeResourceKind, ShapeEvidenceStatus,
};

macro_rules! wire_string {
    ($fn_name:ident: $ty:ty { $($variant:ident => $wire:literal),+ $(,)? }) => {
        pub(crate) fn $fn_name(value: &$ty) -> &'static str {
            match value {
                $(<$ty>::$variant => $wire,)+
            }
        }
    };
}

wire_string!(redaction_status: RedactionStatus {
    Redacted => "redacted",
    Suppressed => "suppressed",
    RedactionFailed => "redaction_failed",
});

wire_string!(continuity_status: ContinuityStatus {
    Applied => "applied",
    Empty => "empty",
    Disabled => "disabled",
    Blocked => "blocked",
    Partial => "partial",
    Failed => "failed",
});

wire_string!(conversation_shape: ConversationShape {
    DirectMessage => "direct_message",
    Group => "group",
    Room => "room",
    Web => "web",
    Unknown => "unknown",
    Unsupported => "unsupported",
});

wire_string!(shape_evidence_status: ShapeEvidenceStatus {
    Proven => "proven",
    Partial => "partial",
    Unsupported => "unsupported",
    Failed => "failed",
});

wire_string!(participation_decision_value: ParticipationDecisionValue {
    Accepted => "accepted",
    Ignored => "ignored",
    Blocked => "blocked",
    Denied => "denied",
    Duplicate => "duplicate",
    Unsupported => "unsupported",
    Failed => "failed",
});

wire_string!(reset_event_status: ResetEventStatus {
    Succeeded => "succeeded",
    Denied => "denied",
    FailedClosed => "failed_closed",
    Unsupported => "unsupported",
});

wire_string!(handoff_status: HandoffStatus {
    Succeeded => "succeeded",
    Denied => "denied",
    FailedClosed => "failed_closed",
    Unsupported => "unsupported",
    Expired => "expired",
});

wire_string!(handoff_source_reference_status: HandoffSourceReferenceStatus {
    Available => "available",
    Consumed => "consumed",
    Blocked => "blocked",
    Expired => "expired",
    None => "none",
});

wire_string!(routing_outcome: RoutingOutcome {
    Accepted => "accepted",
    Ignored => "ignored",
    Blocked => "blocked",
    Duplicate => "duplicate",
    Disabled => "disabled",
    Unsupported => "unsupported",
    Failed => "failed",
    UnknownSource => "unknown_source",
    StaleSource => "stale_source",
    InaccessibleTenantBinding => "inaccessible_tenant_binding",
});

wire_string!(lifecycle_action_kind: LifecycleActionKind {
    Reset => "reset",
    Archive => "archive",
    Reopen => "reopen",
});

wire_string!(runtime_resource_kind: RuntimeResourceKind {
    Session => "session",
    Run => "run",
    Workflow => "workflow",
    Approval => "approval",
    ForegroundReply => "foreground_reply",
    BackgroundDelivery => "background_delivery",
    ConnectorMessage => "connector_message",
});

#[cfg(test)]
mod tests {
    use super::*;

    /// Every wire-string mapping must equal exactly what serde would emit for
    /// the variant (snake_case), so event payloads stay byte-identical to Go.
    fn assert_wire<T: serde::Serialize>(wire: &str, value: &T) {
        assert_eq!(
            serde_json::Value::String(wire.to_string()),
            serde_json::to_value(value).unwrap(),
            "wire string diverged from the serde form of {wire}"
        );
    }

    #[test]
    fn wire_strings_match_serde_snake_case() {
        use kura_threads::{ContinuityRole, SourceKind};

        assert_wire(
            redaction_status(&RedactionStatus::Redacted),
            &RedactionStatus::Redacted,
        );
        assert_wire(
            redaction_status(&RedactionStatus::Suppressed),
            &RedactionStatus::Suppressed,
        );
        assert_wire(
            redaction_status(&RedactionStatus::RedactionFailed),
            &RedactionStatus::RedactionFailed,
        );

        assert_wire(
            continuity_status(&ContinuityStatus::Applied),
            &ContinuityStatus::Applied,
        );
        assert_wire(
            continuity_status(&ContinuityStatus::Empty),
            &ContinuityStatus::Empty,
        );
        assert_wire(
            continuity_status(&ContinuityStatus::Disabled),
            &ContinuityStatus::Disabled,
        );
        assert_wire(
            continuity_status(&ContinuityStatus::Blocked),
            &ContinuityStatus::Blocked,
        );
        assert_wire(
            continuity_status(&ContinuityStatus::Partial),
            &ContinuityStatus::Partial,
        );
        assert_wire(
            continuity_status(&ContinuityStatus::Failed),
            &ContinuityStatus::Failed,
        );

        assert_wire(
            conversation_shape(&ConversationShape::DirectMessage),
            &ConversationShape::DirectMessage,
        );
        assert_wire(
            conversation_shape(&ConversationShape::Group),
            &ConversationShape::Group,
        );
        assert_wire(
            conversation_shape(&ConversationShape::Room),
            &ConversationShape::Room,
        );
        assert_wire(
            conversation_shape(&ConversationShape::Web),
            &ConversationShape::Web,
        );
        assert_wire(
            conversation_shape(&ConversationShape::Unknown),
            &ConversationShape::Unknown,
        );
        assert_wire(
            conversation_shape(&ConversationShape::Unsupported),
            &ConversationShape::Unsupported,
        );

        assert_wire(
            shape_evidence_status(&ShapeEvidenceStatus::Proven),
            &ShapeEvidenceStatus::Proven,
        );
        assert_wire(
            shape_evidence_status(&ShapeEvidenceStatus::Partial),
            &ShapeEvidenceStatus::Partial,
        );
        assert_wire(
            shape_evidence_status(&ShapeEvidenceStatus::Unsupported),
            &ShapeEvidenceStatus::Unsupported,
        );
        assert_wire(
            shape_evidence_status(&ShapeEvidenceStatus::Failed),
            &ShapeEvidenceStatus::Failed,
        );

        assert_wire(
            participation_decision_value(&ParticipationDecisionValue::Accepted),
            &ParticipationDecisionValue::Accepted,
        );
        assert_wire(
            participation_decision_value(&ParticipationDecisionValue::Ignored),
            &ParticipationDecisionValue::Ignored,
        );
        assert_wire(
            participation_decision_value(&ParticipationDecisionValue::Blocked),
            &ParticipationDecisionValue::Blocked,
        );
        assert_wire(
            participation_decision_value(&ParticipationDecisionValue::Denied),
            &ParticipationDecisionValue::Denied,
        );
        assert_wire(
            participation_decision_value(&ParticipationDecisionValue::Duplicate),
            &ParticipationDecisionValue::Duplicate,
        );
        assert_wire(
            participation_decision_value(&ParticipationDecisionValue::Unsupported),
            &ParticipationDecisionValue::Unsupported,
        );
        assert_wire(
            participation_decision_value(&ParticipationDecisionValue::Failed),
            &ParticipationDecisionValue::Failed,
        );

        assert_wire(
            reset_event_status(&ResetEventStatus::Succeeded),
            &ResetEventStatus::Succeeded,
        );
        assert_wire(
            reset_event_status(&ResetEventStatus::Denied),
            &ResetEventStatus::Denied,
        );
        assert_wire(
            reset_event_status(&ResetEventStatus::FailedClosed),
            &ResetEventStatus::FailedClosed,
        );
        assert_wire(
            reset_event_status(&ResetEventStatus::Unsupported),
            &ResetEventStatus::Unsupported,
        );

        assert_wire(
            handoff_status(&HandoffStatus::Succeeded),
            &HandoffStatus::Succeeded,
        );
        assert_wire(
            handoff_status(&HandoffStatus::Denied),
            &HandoffStatus::Denied,
        );
        assert_wire(
            handoff_status(&HandoffStatus::FailedClosed),
            &HandoffStatus::FailedClosed,
        );
        assert_wire(
            handoff_status(&HandoffStatus::Unsupported),
            &HandoffStatus::Unsupported,
        );
        assert_wire(
            handoff_status(&HandoffStatus::Expired),
            &HandoffStatus::Expired,
        );

        assert_wire(
            handoff_source_reference_status(&HandoffSourceReferenceStatus::Available),
            &HandoffSourceReferenceStatus::Available,
        );
        assert_wire(
            handoff_source_reference_status(&HandoffSourceReferenceStatus::Consumed),
            &HandoffSourceReferenceStatus::Consumed,
        );
        assert_wire(
            handoff_source_reference_status(&HandoffSourceReferenceStatus::Blocked),
            &HandoffSourceReferenceStatus::Blocked,
        );
        assert_wire(
            handoff_source_reference_status(&HandoffSourceReferenceStatus::Expired),
            &HandoffSourceReferenceStatus::Expired,
        );
        assert_wire(
            handoff_source_reference_status(&HandoffSourceReferenceStatus::None),
            &HandoffSourceReferenceStatus::None,
        );

        assert_wire(
            routing_outcome(&RoutingOutcome::Accepted),
            &RoutingOutcome::Accepted,
        );
        assert_wire(
            routing_outcome(&RoutingOutcome::Ignored),
            &RoutingOutcome::Ignored,
        );
        assert_wire(
            routing_outcome(&RoutingOutcome::Blocked),
            &RoutingOutcome::Blocked,
        );
        assert_wire(
            routing_outcome(&RoutingOutcome::Duplicate),
            &RoutingOutcome::Duplicate,
        );
        assert_wire(
            routing_outcome(&RoutingOutcome::Disabled),
            &RoutingOutcome::Disabled,
        );
        assert_wire(
            routing_outcome(&RoutingOutcome::Unsupported),
            &RoutingOutcome::Unsupported,
        );
        assert_wire(
            routing_outcome(&RoutingOutcome::Failed),
            &RoutingOutcome::Failed,
        );
        assert_wire(
            routing_outcome(&RoutingOutcome::UnknownSource),
            &RoutingOutcome::UnknownSource,
        );
        assert_wire(
            routing_outcome(&RoutingOutcome::StaleSource),
            &RoutingOutcome::StaleSource,
        );
        assert_wire(
            routing_outcome(&RoutingOutcome::InaccessibleTenantBinding),
            &RoutingOutcome::InaccessibleTenantBinding,
        );

        assert_wire(
            lifecycle_action_kind(&LifecycleActionKind::Reset),
            &LifecycleActionKind::Reset,
        );
        assert_wire(
            lifecycle_action_kind(&LifecycleActionKind::Archive),
            &LifecycleActionKind::Archive,
        );
        assert_wire(
            lifecycle_action_kind(&LifecycleActionKind::Reopen),
            &LifecycleActionKind::Reopen,
        );

        assert_wire(
            runtime_resource_kind(&RuntimeResourceKind::Session),
            &RuntimeResourceKind::Session,
        );
        assert_wire(
            runtime_resource_kind(&RuntimeResourceKind::Run),
            &RuntimeResourceKind::Run,
        );
        assert_wire(
            runtime_resource_kind(&RuntimeResourceKind::Workflow),
            &RuntimeResourceKind::Workflow,
        );
        assert_wire(
            runtime_resource_kind(&RuntimeResourceKind::Approval),
            &RuntimeResourceKind::Approval,
        );
        assert_wire(
            runtime_resource_kind(&RuntimeResourceKind::ForegroundReply),
            &RuntimeResourceKind::ForegroundReply,
        );
        assert_wire(
            runtime_resource_kind(&RuntimeResourceKind::BackgroundDelivery),
            &RuntimeResourceKind::BackgroundDelivery,
        );
        assert_wire(
            runtime_resource_kind(&RuntimeResourceKind::ConnectorMessage),
            &RuntimeResourceKind::ConnectorMessage,
        );

        // Sanity: non-wire-mapped thread enums still serialize snake_case.
        assert_eq!(
            serde_json::to_value(ContinuityRole::User).unwrap(),
            serde_json::json!("user")
        );
        assert_eq!(
            serde_json::to_value(SourceKind::Chat).unwrap(),
            serde_json::json!("chat")
        );
    }
}
