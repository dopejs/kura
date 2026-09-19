//! Ported from daemon/internal/contracts/triage_contracts_test.go (wave 8 contract parity).
//!
//! Each test mirrors the corresponding Go test function: the same
//! schemaPath -> fixture set is validated through
//! Validator::validate_relative (Go ValidateRelative).

mod common;

use common::{Fixture, schema_root_dir, validate_fixtures};
use kura_contracts::Validator;

#[test]
fn test_triage_schemas_accept_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[
        (
            r##"schemas/api/create-triage-policy.request.schema.json"##,
            r##"{"name":"inbox","rules":[{"description":"newsletters","conditions":[{"field":"sender","operator":"contains","value":"newsletter"}],"classification":"newsletter","outcome":"delivery_digest"}],"defaultClassification":"fyi"}"##,
        ),
        (
            r##"schemas/api/triage-policy-resource.schema.json"##,
            r##"{"policyId":"triage_policy_1","environmentScope":"test","name":"inbox","rules":[{"ruleId":"triage_rule_1","description":"urgent from boss","conditions":[{"field":"sender","operator":"contains","value":"boss@"},{"field":"subject","operator":"contains","value":"urgent"}],"classification":"urgent","outcome":"reminder"}],"defaultClassification":"fyi","createdAt":"2026-06-30T10:00:00Z","updatedAt":"2026-06-30T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/triage-run-resource.schema.json"##,
            r##"{"runId":"triage_run_1","policyId":"triage_policy_1","environmentScope":"test","messageCount":2,"decisions":[{"messageId":"m1","classification":"urgent","matchedRuleId":"triage_rule_1","matchedEvidence":[{"field":"sender","operator":"contains","value":"boss@"}],"outcome":"reminder","replayCandidate":true,"decidedAt":"2026-06-30T10:00:01Z"},{"messageId":"m2","classification":"fyi","outcome":"no_action","defaultApplied":true,"replayCandidate":true,"decidedAt":"2026-06-30T10:00:01Z"}],"createdAt":"2026-06-30T10:00:01Z"}"##,
        ),
    ];
    validate_fixtures(&validator, fixtures);
}
