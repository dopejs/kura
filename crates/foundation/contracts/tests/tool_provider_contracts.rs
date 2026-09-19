//! Tool provider contract fixtures (Stage 9.1b,
//! `docs/providers/tool-provider-architecture.md`).

mod common;

use common::{Fixture, schema_root_dir, validate_fixtures};
use kura_contracts::Validator;

#[test]
fn test_tool_provider_schemas_accept_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[
        (
            r##"schemas/api/tool-profile-resource.schema.json"##,
            r##"{"profileId":"tlp_1","tenantId":"ten_a","title":"web search","capability":"web.search","family":"builtin_stub","authMode":"api_key","source":"managed","enabled":true,"isDefault":true,"secretRef":"secret://tools/search-key","secretConfigured":true,"limits":{"timeoutMs":20000,"maxResults":10,"maxCallsPerDay":0},"readiness":"ready","createdAt":"2026-09-18T10:00:00Z","updatedAt":"2026-09-18T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/tool-profile-resource.schema.json"##,
            r##"{"profileId":"tlp_2","tenantId":"ten_a","title":"my search (MCP)","capability":"web.search","family":"mcp_backed","authMode":"none","source":"managed","enabled":true,"isDefault":true,"secretConfigured":false,"mcpServerId":"mcp_search","mcpToolName":"search","limits":{"timeoutMs":20000,"maxResults":10,"maxCallsPerDay":0},"readiness":"ready","createdAt":"2026-09-19T10:00:00Z","updatedAt":"2026-09-19T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/tool-capability-status.schema.json"##,
            r##"{"capability":"image.generate","configured":false,"profileCount":0}"##,
        ),
        (
            r##"schemas/api/tool-check-outcome.schema.json"##,
            r##"{"profileId":"tlp_1","passed":false,"errorClass":"auth_error","detail":"the credential reference did not resolve to a value","checkedAt":"2026-09-18T10:00:00Z"}"##,
        ),
    ];
    validate_fixtures(&validator, fixtures);
}

/// The contract must make a credential field unrepresentable, not merely
/// absent from the current fixture: `additionalProperties: false` plus no
/// value-shaped property is what enforces the design's first hard rule.
#[test]
fn a_tool_profile_carrying_a_credential_is_rejected_by_the_contract() {
    let validator = Validator::new(schema_root_dir());
    let with_credential = r##"{"profileId":"tlp_1","title":"web search","capability":"web.search","family":"builtin_stub","authMode":"api_key","source":"managed","enabled":true,"isDefault":true,"secretRef":"secret://k","secretConfigured":true,"apiKey":"sk-live-leaked","limits":{"timeoutMs":20000,"maxResults":10,"maxCallsPerDay":0},"readiness":"ready","createdAt":"2026-09-18T10:00:00Z","updatedAt":"2026-09-18T10:00:00Z"}"##;
    let result = validator.validate_relative(
        "schemas/api/tool-profile-resource.schema.json",
        with_credential.as_bytes(),
    );
    assert!(
        result.is_err(),
        "the schema must reject a profile carrying credential material"
    );
}
