//! Ported from daemon/internal/contracts/launch_gate_contracts_test.go (wave 8 contract parity).
//!
//! Each test mirrors the corresponding Go test function: the same
//! schemaPath -> fixture set is validated through
//! Validator::validate_relative (Go ValidateRelative).

mod common;

use common::{Fixture, schema_root_dir, validate_fixtures};
use kura_contracts::Validator;

#[test]
fn test_launch_gate_decision_schema_accepts_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[(
        r##"schemas/api/launch-gate-decision.schema.json"##,
        r##"{"result":"no_ship","reasons":["missing mail provider smoke entry"],"nonKnowledgeParityComplete":false,"gateStatement":"Context, knowledge, and memory work may begin only after non-knowledge parity release evidence passes or residual exceptions are explicitly accepted."}"##,
    )];
    validate_fixtures(&validator, fixtures);
}
