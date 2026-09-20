//! Ported from daemon/internal/contracts/execprofile_contracts_test.go (wave 8 contract parity).
//!
//! Each test mirrors the corresponding Go test function: the same
//! schemaPath -> fixture set is validated through
//! Validator::validate_relative (Go ValidateRelative).

mod common;

use common::{Fixture, schema_root_dir, validate_fixtures};
use kura_contracts::Validator;

#[test]
fn test_execution_profile_schemas_accept_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[
        (
            r##"schemas/api/execution-profile-resource.schema.json"##,
            r##"{"profile":{"profileId":"subprocess","name":"Subprocess Sandbox","backendKind":"subprocess","riskTier":"low","provides":["local_fs"],"description":"repo-owned subprocess sandbox","createdAt":"2026-06-30T10:00:00Z"},"status":{"profileId":"subprocess","health":"ready","available":true}}"##,
        ),
        (
            r##"schemas/api/execution-denial-explanation.schema.json"##,
            r##"{"requiredCapabilities":["docker"],"eligibleProfiles":[],"missingCapabilities":{"subprocess":["docker"]},"unavailable":{"docker":"backend unavailable"}}"##,
        ),
    ];
    validate_fixtures(&validator, fixtures);
}
