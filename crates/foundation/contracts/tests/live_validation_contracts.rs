//! Ported from daemon/internal/contracts/live_validation_contracts_test.go (wave 8 contract parity).
//!
//! Each test mirrors the corresponding Go test function: the same
//! schemaPath -> fixture set is validated through
//! Validator::validate_relative (Go ValidateRelative).

mod common;

use common::{schema_root_dir, validate_fixtures};
use kura_contracts::Validator;

#[test]
fn test_live_validation_schemas_accept_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    validate_fixtures(
        &validator,
        &[common::data::live_validation_contract_fixtures()].concat(),
    );
}
