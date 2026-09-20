//! Ported from daemon/internal/contracts/matrix_schema_test.go (wave 8 contract parity).
//!
//! Each test mirrors the corresponding Go test function: the same
//! schemaPath -> fixture set is validated through
//! Validator::validate_relative (Go ValidateRelative).

mod common;

use common::{Fixture, schema_root_dir, validate_fixtures};
use kura_contracts::Validator;

#[test]
fn test_matrix_schema_fixtures_load_individually() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[
        (
            r##"schemas/api/matrix-hosted-setup-resource.schema.json"##,
            r##"{"tenantId":"ten_matrix","connectorId":"matrix-main","connectorKind":"matrix","displayName":"Matrix Main","status":"degraded","terminalState":"action-required","botCredentialState":"valid","homeserverState":"reachable","routePolicyState":"valid","deliveryEligible":false,"homeserverBindingId":"matrix_hs_1","redactionStatus":"redacted","createdAt":"2026-05-10T10:00:00Z","updatedAt":"2026-05-10T10:01:00Z","validatedAt":"2026-05-10T10:01:00Z","retentionExpiresAt":"2026-08-08T10:01:00Z","homeserverBinding":{"homeserverUrl":"https://matrix.example.org","botUserId":"@bot:example.org","authorizationState":"valid","homeserverCapabilityState":"valid","validatedAt":"2026-05-10T10:01:00Z","redactionStatus":"redacted"},"routePolicy":{"tenantId":"ten_matrix","connectorId":"matrix-main","homeserverBindingId":"matrix_hs_1","selectedRooms":[{"conversationId":"!room:example.org","conversationType":"room","roomSelectionState":"selected","validationState":"valid","redactionStatus":"redacted"}],"allowedDirectUsers":["@alice:example.org"],"roomInvocationGate":"bot_mention_or_command_required","configuredCommands":["!kura"],"encryptedRoomPolicy":"unsupported","validationState":"valid","validatedAt":"2026-05-10T10:01:00Z","redactionStatus":"redacted"},"diagnostic":{"reasonCode":"blocked_route","matrixCondition":"blocked_route","remediationOwner":"tenant_admin","freshnessState":"fresh"}}"##,
        ),
        (
            r##"schemas/api/matrix-route-policy-resource.schema.json"##,
            r##"{"tenantId":"ten_matrix","connectorId":"matrix-main","homeserverBindingId":"matrix_hs_1","selectedRooms":[{"conversationId":"!room:example.org","conversationType":"room","roomSelectionState":"selected","validationState":"valid","redactionStatus":"redacted"}],"allowedDirectUsers":["@alice:example.org"],"roomInvocationGate":"bot_mention_or_command_required","configuredCommands":["!kura"],"encryptedRoomPolicy":"unsupported","validationState":"valid","validatedAt":"2026-05-10T10:01:00Z","redactionStatus":"redacted","safeEvidence":{"route":"selected_room_and_direct_allowment"}}"##,
        ),
        (
            r##"schemas/api/matrix-smoke-evidence-resource.schema.json"##,
            r##"{"smokeEvidenceId":"matrix_smoke_1","tenantId":"ten_matrix","connectorId":"matrix-main","homeserverBindingId":"matrix_hs_1","status":"skipped","authorizationMode":"unavailable","owner":"operator","reason":"safe Matrix credentials unavailable","remainingRisk":"No live Matrix smoke was run.","validatedAt":"2026-05-10T10:02:00Z","retentionExpiresAt":"2026-08-08T10:02:00Z","redactionStatus":"redacted","safeEvidence":{"policy":"structured_skip"}}"##,
        ),
        (
            r##"schemas/events/connector-matrix-setup-validated.event.schema.json"##,
            r##"{"eventId":"evt_matrix_setup_1","sequence":1,"category":"connector","name":"connector.matrix_setup_validated","occurredAt":"2026-05-10T10:01:00Z","scope":{"connectorId":"matrix-main"},"resource":{"kind":"matrix_hosted_setup","id":"matrix-main"},"payload":{"tenantId":"ten_matrix","connectorId":"matrix-main","homeserverBindingId":"matrix_hs_1","terminalState":"action-required","botCredentialState":"valid","routePolicyState":"valid","deliveryEligible":false,"reasonCode":"blocked_route","matrixCondition":"blocked_route","redactionStatus":"redacted","validatedAt":"2026-05-10T10:01:00Z"}}"##,
        ),
    ];
    validate_fixtures(&validator, fixtures);
}
