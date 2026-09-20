//! Ported from daemon/internal/contracts/thread_continuity_contracts_test.go (wave 8 contract parity).
//!
//! Each test mirrors the corresponding Go test function: the same
//! schemaPath -> fixture set is validated through
//! Validator::validate_relative (Go ValidateRelative).

mod common;

use common::{Fixture, schema_root_dir, validate_fixtures};
use kura_contracts::Validator;

#[test]
fn test_thread_continuity_preview_and_event_contracts() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[
        (
            r##"schemas/api/thread-continuity-preview.schema.json"##,
            r##"{"continuityPreviewId":"contprev_1","tenantId":"ten_1","threadId":"thr_1","sessionSegmentId":"seg_1","dispatchId":"dispatch_1","requestTurnId":"turn_user_1","responseTurnId":"turn_assistant_1","windowPolicyId":"default_recent_12_30d","maxPriorTurns":12,"activeWindowDays":30,"orderedBy":"daemon_acceptance_sequence","continuityApplied":true,"status":"applied","includedCount":1,"excludedCount":1,"assemblyStartedAt":"2026-05-11T10:00:00Z","assemblyCompletedAt":"2026-05-11T10:00:00.037Z","assemblyDurationMs":37,"retentionExpiresAt":"2026-08-09T10:00:00Z","redactionStatus":"redacted"}"##,
        ),
        (
            r##"schemas/api/thread-continuity-preview-item.schema.json"##,
            r##"{"previewItemId":"contitem_1","continuityPreviewId":"contprev_1","tenantId":"ten_1","threadId":"thr_1","itemKind":"turn","continuityTurnId":"turn_1","role":"user","decision":"included","reasonCode":"included_recent","acceptanceSequence":8,"safeSummary":"User asked about option A","redactionStatus":"redacted","itemOrder":0}"##,
        ),
        (
            r##"schemas/api/thread-continuity-preview.response.schema.json"##,
            r##"{"preview":{"continuityPreviewId":"contprev_1","tenantId":"ten_1","threadId":"thr_1","sessionSegmentId":"seg_1","dispatchId":"dispatch_1","requestTurnId":"turn_user_1","windowPolicyId":"default_recent_12_30d","maxPriorTurns":12,"activeWindowDays":30,"orderedBy":"daemon_acceptance_sequence","continuityApplied":true,"status":"applied","includedCount":1,"excludedCount":1,"assemblyDurationMs":37,"redactionStatus":"redacted"},"items":[{"previewItemId":"contitem_1","itemKind":"turn","decision":"included","reasonCode":"included_recent","continuityTurnId":"turn_1","role":"user","acceptanceSequence":8,"safeSummary":"User asked about option A","redactionStatus":"redacted","itemOrder":0}]}"##,
        ),
        (
            r##"schemas/api/chat-query.request.schema.json"##,
            r##"{"provider":"openai_compatible","model":"gpt-test","query":"follow up","threadId":"thr_1","continuity":{"mode":"auto"}}"##,
        ),
        (
            r##"schemas/api/chat-query.response.schema.json"##,
            r##"{"dispatchId":"dispatch_1","provider":"openai_compatible","model":"gpt-test","skills":[],"query":"follow up","status":"completed","partial":false,"reply":"answer","usage":{"inputTokens":2,"outputTokens":2,"totalTokens":4},"threadId":"thr_1","sessionSegmentId":"seg_1","requestTurnId":"turn_user_1","responseTurnId":"turn_assistant_1","continuityPreviewId":"contprev_1","continuityApplied":true,"continuityStatus":"applied","continuityIncludedCount":1,"continuityExcludedCount":0}"##,
        ),
        (
            r##"schemas/events/thread-continuity-turn-recorded.event.schema.json"##,
            r##"{"tenantId":"ten_1","threadId":"thr_1","sessionSegmentId":"seg_1","continuityTurnId":"turn_1","dispatchId":"dispatch_1","action":"turn_recorded","outcome":"recorded","reasonCode":"included_recent","acceptanceSequence":8,"redactionStatus":"redacted"}"##,
        ),
        (
            r##"schemas/events/thread-continuity-preview-recorded.event.schema.json"##,
            r##"{"tenantId":"ten_1","threadId":"thr_1","sessionSegmentId":"seg_1","continuityPreviewId":"contprev_1","dispatchId":"dispatch_1","action":"preview_recorded","outcome":"applied","reasonCode":"included_recent","redactionStatus":"redacted"}"##,
        ),
    ];
    validate_fixtures(&validator, fixtures);
}
