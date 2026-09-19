//! Ported from daemon/internal/contracts/webhook_contracts_test.go (wave 8 contract parity).
//!
//! Each test mirrors the corresponding Go test function: the same
//! schemaPath -> fixture set is validated through
//! Validator::validate_relative (Go ValidateRelative).

mod common;

use common::{Fixture, schema_root_dir, validate_fixtures};
use kura_contracts::Validator;

#[test]
fn test_webhook_schemas_accept_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[
        (
            r##"schemas/api/create-webhook.request.schema.json"##,
            r##"{"name":"deploy hook","targetKind":"routine","targetRef":"routine_1"}"##,
        ),
        (
            r##"schemas/api/webhook-endpoint-resource.schema.json"##,
            r##"{"webhookId":"webhook_1","tenantId":"ten_a","environmentScope":"test","name":"deploy hook","targetKind":"routine","targetRef":"routine_1","status":"active","secretFingerprint":"sha256:abcdef012345","secretVersion":1,"createdAt":"2026-06-30T10:00:00Z","updatedAt":"2026-06-30T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/webhook-trigger-record.schema.json"##,
            r##"{"triggerId":"webhook_trigger_1","webhookId":"webhook_1","tenantId":"ten_a","environmentScope":"test","idempotencyKey":"evt-1","status":"fired","payloadBytes":42,"executionRef":"run_1","createdAt":"2026-06-30T10:00:01Z"}"##,
        ),
    ];
    validate_fixtures(&validator, fixtures);
}
