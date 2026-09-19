//! Ported from daemon/internal/contracts/routine_contracts_test.go (wave 8 contract parity).
//!
//! Each test mirrors the corresponding Go test function: the same
//! schemaPath -> fixture set is validated through
//! Validator::validate_relative (Go ValidateRelative).

mod common;

use common::{Fixture, schema_root_dir, validate_fixtures};
use kura_contracts::Validator;

#[test]
fn test_routine_schemas_accept_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[
        (
            r##"schemas/api/create-routine.request.schema.json"##,
            r##"{"definition":{"name":"Daily summary","trigger":{"kind":"cron","cronExpr":"0 8 * * *","timezone":"UTC"},"workflow":{"goal":"summarize my day"},"approvalExpectation":"ask","maxRetries":1}}"##,
        ),
        (
            r##"schemas/api/routine-resource.schema.json"##,
            r##"{"routineId":"routine_1","environmentScope":"test","name":"Daily summary","state":"active","currentVersion":1,"currentScheduleId":"sched_1","definition":{"name":"Daily summary","trigger":{"kind":"cron","cronExpr":"0 8 * * *","timezone":"UTC"},"workflow":{"entrypoint":"operator","goal":"summarize my day"},"approvalExpectation":"ask","maxRetries":1},"versions":[{"version":1,"definition":{"name":"Daily summary","trigger":{"kind":"cron","cronExpr":"0 8 * * *"},"workflow":{"goal":"summarize my day"}},"scheduleId":"sched_1","createdAt":"2026-06-30T10:00:00Z"}],"createdAt":"2026-06-30T10:00:00Z","updatedAt":"2026-06-30T10:00:00Z"}"##,
        ),
    ];
    validate_fixtures(&validator, fixtures);
}
