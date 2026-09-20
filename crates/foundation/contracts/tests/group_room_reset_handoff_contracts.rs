//! Ported from daemon/internal/contracts/group_room_reset_handoff_contracts_test.go (wave 8 contract parity).
//!
//! Each test mirrors the corresponding Go test function: the same
//! schemaPath -> fixture set is validated through
//! Validator::validate_relative (Go ValidateRelative).

mod common;

use common::{Fixture, schema_root_dir, validate_fixtures};
use kura_contracts::Validator;

#[test]
fn test_group_room_reset_handoff_contracts() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[
        (
            r##"schemas/api/thread-conversation-shape.schema.json"##,
            r##"{"shape":"room","sourceKind":"channel","connectorId":"slack-main","connectorKind":"slack","sourceAccountId":"workspace_redacted","sourceConversationId":"channel_redacted","sourceConversationSummary":"Slack Main / #support","shapeEvidenceStatus":"proven","redactionStatus":"redacted"}"##,
        ),
        (
            r##"schemas/api/thread-participation-decision.schema.json"##,
            r##"{"participationDecisionId":"part_123","threadId":"thr_123","sessionSegmentId":"seg_123","conversationShape":"room","decision":"ignored","reasonCode":"missing_qualifying_mention","mentionStatus":"missing","allowlistStatus":"eligible","createdAssistantWork":false,"safeSummary":"Room message ignored by participation policy","occurredAt":"2026-05-11T10:00:00Z","redactionStatus":"redacted"}"##,
        ),
        (
            r##"schemas/api/thread-reset-event.schema.json"##,
            r##"{"resetEventId":"reset_123","threadId":"thr_group","conversationShape":"group","permissionGate":"connectors.manage","priorSessionSegmentId":"seg_old","resultingSessionSegmentId":"seg_new","status":"succeeded","reasonCode":"scoped_reset_succeeded","requestedAt":"2026-05-11T10:00:00Z","completedAt":"2026-05-11T10:00:00Z","redactionStatus":"redacted"}"##,
        ),
        (
            r##"schemas/api/thread-handoff-link.schema.json"##,
            r##"{"handoffLinkId":"handoff_123","sourceThreadId":"thr_source","destinationThreadId":"thr_destination","sourceSessionSegmentId":"seg_source","destinationSessionSegmentId":"seg_destination","sourceConversationShape":"room","destinationConversationShape":"web","status":"succeeded","sourceReferenceStatus":"available","permissionGate":"connectors.manage","createdAt":"2026-05-11T10:00:00Z","redactionStatus":"redacted"}"##,
        ),
        (
            r##"schemas/api/thread-handoff.request.schema.json"##,
            r##"{"destination":{"surface":"channel","connectorId":"slack-main","sourceAccountId":"workspace_redacted","sourceConversationId":"channel_redacted","conversationShape":"room"},"reasonCode":"user_requested_handoff"}"##,
        ),
        (
            r##"schemas/api/thread-handoff.response.schema.json"##,
            r##"{"handoffLinkId":"handoff_123","sourceThreadId":"thr_source","destinationThreadId":"thr_destination","sourceSessionSegmentId":"seg_source","destinationSessionSegmentId":"seg_destination","sourceConversationShape":"room","destinationConversationShape":"web","status":"succeeded","sourceReferenceStatus":"available","permissionGate":"connectors.manage","createdAt":"2026-05-11T10:00:00Z","redactionStatus":"redacted"}"##,
        ),
        (
            r##"schemas/events/thread-participation-decision.event.schema.json"##,
            r##"{"tenantId":"ten_1","threadId":"thr_123","sessionSegmentId":"seg_123","eventKind":"thread.participation_decision_recorded","resourceId":"part_123","status":"ignored","reasonCode":"missing_qualifying_mention","redactionStatus":"redacted","occurredAt":"2026-05-11T10:00:00Z"}"##,
        ),
        (
            r##"schemas/events/thread-reset-scoped.event.schema.json"##,
            r##"{"tenantId":"ten_1","threadId":"thr_group","sessionSegmentId":"seg_new","eventKind":"thread.reset_scoped","resourceId":"reset_123","status":"succeeded","permissionGate":"connectors.manage","redactionStatus":"redacted","occurredAt":"2026-05-11T10:00:00Z"}"##,
        ),
        (
            r##"schemas/events/thread-handoff-linked.event.schema.json"##,
            r##"{"tenantId":"ten_1","threadId":"thr_source","sessionSegmentId":"seg_destination","eventKind":"thread.handoff_linked","resourceId":"handoff_123","status":"succeeded","reasonCode":"user_requested_handoff","permissionGate":"connectors.manage","redactionStatus":"redacted","occurredAt":"2026-05-11T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/thread-detail.response.schema.json"##,
            r##"{"thread":{"threadId":"thr_1","tenantId":"ten_1","lifecycleState":"active","sourceKind":"channel","sourceSummary":"Slack Main / #support","currentSessionSegmentId":"seg_1","lastActivityAt":"2026-05-11T10:00:00Z","availableActions":["reset"],"redactionStatus":"redacted","retentionExpiresAt":"2026-08-09T10:00:00Z","updatedAt":"2026-05-11T10:00:00Z"},"sessionSegments":[],"sourceLinkages":[],"runtimeProjections":[],"lifecycleActions":[],"conversationShape":{"shape":"room","shapeEvidenceStatus":"proven","redactionStatus":"redacted"},"participationDecisions":[{"conversationShape":"room","decision":"ignored","reasonCode":"missing_qualifying_mention","createdAssistantWork":false,"redactionStatus":"redacted"}],"resetEvents":[{"conversationShape":"room","permissionGate":"connectors.manage","status":"succeeded","reasonCode":"scoped_reset_succeeded","redactionStatus":"redacted"}],"handoffLinks":[{"sourceThreadId":"thr_source","destinationThreadId":"thr_destination","sourceConversationShape":"room","destinationConversationShape":"web","status":"succeeded","sourceReferenceStatus":"consumed","permissionGate":"connectors.manage","redactionStatus":"redacted"}]}"##,
        ),
    ];
    validate_fixtures(&validator, fixtures);
}
