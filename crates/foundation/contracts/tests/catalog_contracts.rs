//! Ported from daemon/internal/contracts/catalog_contracts_test.go (wave 8 contract parity).
//!
//! Each test mirrors the corresponding Go test function: the same
//! schemaPath -> fixture set is validated through
//! Validator::validate_relative (Go ValidateRelative).

mod common;

use common::{Fixture, schema_root_dir, validate_fixtures};
use kura_contracts::Validator;

#[test]
fn test_catalog_schemas_accept_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[
        (
            r##"schemas/api/catalog-item-resource.schema.json"##,
            r##"{"itemId":"catalog_item_1","kind":"skill","name":"pdf-extract","trustTier":"verified","permissions":["skills.manage"],"versions":[{"version":"1.0.0","source":"registry://pdf-extract","publishedAt":"2026-06-30T10:00:00Z"},{"version":"1.1.0","source":"registry://pdf-extract","requirements":[{"key":"sandbox_backend","description":"needs a sandbox"}],"publishedAt":"2026-06-30T10:00:00Z"}],"createdAt":"2026-06-30T10:00:00Z","updatedAt":"2026-06-30T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/catalog-enablement-resource.schema.json"##,
            r##"{"tenantId":"ten_a","itemId":"catalog_item_1","state":"enabled","activeVersion":"1.0.0","versionStack":["1.0.0"],"history":[{"action":"enabled","version":"1.0.0","actor":"op","occurredAt":"2026-06-30T10:00:01Z"}],"updatedAt":"2026-06-30T10:00:01Z"}"##,
        ),
    ];
    validate_fixtures(&validator, fixtures);
}
