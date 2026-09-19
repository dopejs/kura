//! Ported from daemon/internal/contracts/thread_lifecycle_contracts_test.go (wave 8 contract parity).
//!
//! Each test mirrors the corresponding Go test function: the same
//! schemaPath -> fixture set is validated through
//! Validator::validate_relative (Go ValidateRelative).

mod common;

use common::{Fixture, schema_root_dir, validate_fixtures};
use kura_contracts::Validator;

#[test]
fn test_thread_lifecycle_a_p_i_and_event_contracts() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[
        (
            r##"schemas/api/thread-list.response.schema.json"##,
            r##"{"tenantId":"ten_1","page":{"limit":20,"order":"active_recent_archived_id"},"items":[{"threadId":"thr_1","tenantId":"ten_1","lifecycleState":"active","sourceKind":"channel","sourceSummary":"Slack Main / #support","currentSessionSegmentId":"seg_1","currentSessionId":"sess_1","lastActivityAt":"2026-05-11T10:00:00Z","availableActions":["reset","archive"],"redactionStatus":"redacted","retentionExpiresAt":"2026-08-09T10:00:00Z","updatedAt":"2026-05-11T10:00:00Z"}]}"##,
        ),
        (
            r##"schemas/api/thread-detail.response.schema.json"##,
            r##"{"thread":{"threadId":"thr_1","tenantId":"ten_1","lifecycleState":"active","sourceKind":"channel","sourceSummary":"Slack Main / #support","currentSessionSegmentId":"seg_1","currentSessionId":"sess_1","lastActivityAt":"2026-05-11T10:00:00Z","availableActions":["reset"],"redactionStatus":"redacted","retentionExpiresAt":"2026-08-09T10:00:00Z","updatedAt":"2026-05-11T10:00:00Z"},"sessionSegments":[{"sessionSegmentId":"seg_1","partialEvidence":false}],"sourceLinkages":[{"sourceLinkageId":"src_1","sourceKind":"channel","routingOutcome":"accepted","current":true,"linkedAt":"2026-05-11T10:00:00Z","retentionExpiresAt":"2026-08-09T10:00:00Z","redactionStatus":"redacted"}],"runtimeProjections":[{"runtimeProjectionId":"rtp_1","resourceKind":"run","resourceId":"run_1","status":"completed","occurredAt":"2026-05-11T10:00:00Z","retentionExpiresAt":"2026-08-09T10:00:00Z","redactionStatus":"redacted"}],"lifecycleActions":[{"lifecycleActionId":"act_1"}]}"##,
        ),
        (
            r##"schemas/api/thread-lifecycle-action.response.schema.json"##,
            r##"{"threadId":"thr_1","lifecycleState":"reset","previousSessionSegmentId":"seg_old","currentSessionSegmentId":"seg_new","auditEventId":"audit_1","changedAt":"2026-05-11T10:00:00Z","action":"reset","availableActions":["reset","archive"]}"##,
        ),
        (
            r##"schemas/events/thread-lifecycle.event.schema.json"##,
            r##"{"tenantId":"ten_1","threadId":"thr_1","sessionSegmentId":"seg_1","action":"reset","outcome":"succeeded","auditEventId":"audit_1","reasonCode":"user_requested_reset","redactionStatus":"redacted"}"##,
        ),
        (
            r##"schemas/events/thread-source-linked.event.schema.json"##,
            r##"{"tenantId":"ten_1","threadId":"thr_1","sessionSegmentId":"seg_1","sourceLinkageId":"src_1","routingOutcome":"accepted","redactionStatus":"redacted"}"##,
        ),
        (
            r##"schemas/events/thread-retention-applied.event.schema.json"##,
            r##"{"tenantId":"ten_1","threadId":"thr_1","retentionExpiresAt":"2026-08-09T10:00:00Z","policySource":"default","redactionStatus":"redacted"}"##,
        ),
    ];
    validate_fixtures(&validator, fixtures);
}
