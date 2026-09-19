//! Ported from daemon/internal/contracts/matrix_setup_contract_test.go (wave 8 contract parity).
//!
//! Each test mirrors the corresponding Go test function: the same
//! schemaPath -> fixture set is validated through
//! Validator::validate_relative (Go ValidateRelative).

mod common;

use common::{Fixture, schema_root_dir, validate_fixtures};
use kura_contracts::Validator;

#[test]
fn test_matrix_setup_contract_accepts_redacted_terminal_states() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[(
        r##"schemas/api/matrix-hosted-setup-resource.schema.json"##,
        r##"{"connectorId":"matrix-main","connectorKind":"matrix","displayName":"Matrix Main","status":"degraded","terminalState":"action-required","botCredentialState":"invalid","homeserverState":"reachable","routePolicyState":"blocked","deliveryEligible":false,"homeserverBindingId":"matrix_hs_1","reasonCode":"bot_auth_invalid","redactionStatus":"redacted","createdAt":"2026-05-10T10:00:00Z","updatedAt":"2026-05-10T10:01:00Z","retentionExpiresAt":"2026-08-08T10:01:00Z","diagnostic":{"reasonCode":"auth_missing","matrixCondition":"bot_auth_invalid","remediationOwner":"tenant_admin","freshnessState":"fresh"}}"##,
    )];
    validate_fixtures(&validator, fixtures);
}
