//! Ported from daemon/internal/contracts/evidence_contracts_test.go (wave 8 contract parity).
//!
//! Each test mirrors the corresponding Go test function: the same
//! schemaPath -> fixture set is validated through
//! Validator::validate_relative (Go ValidateRelative).

mod common;

use common::{Fixture, schema_root_dir, validate_fixtures};
use kura_contracts::Validator;

#[test]
fn test_evidence_bundle_schemas_accept_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[
        (
            r##"schemas/api/generate-evidence-bundle.request.schema.json"##,
            r##"{"actor":"support@kura","scope":{"kind":"routine","ref":"routine_1"}}"##,
        ),
        (
            r##"schemas/api/evidence-bundle-resource.schema.json"##,
            r##"{"bundleId":"evidence_bundle_1","tenantId":"ten_a","actor":"support@kura","scope":{"kind":"routine","ref":"routine_1"},"sections":[{"kind":"routine","resourceRefs":["routine_1"],"summary":{"state":"failed","accessToken":"[redacted]"},"links":["/v1/routines/routine_1"]}],"redactionStatus":"redacted","createdAt":"2026-06-30T10:00:00Z","retentionExpiresAt":"2026-07-14T10:00:00Z"}"##,
        ),
    ];
    validate_fixtures(&validator, fixtures);
}
