//! Ported from daemon/internal/contracts/matrix_route_delivery_contract_test.go (wave 8 contract parity).
//!
//! Each test mirrors the corresponding Go test function: the same
//! schemaPath -> fixture set is validated through
//! Validator::validate_relative (Go ValidateRelative).

mod common;

use common::{Fixture, schema_root_dir, validate_fixtures};
use kura_contracts::Validator;

#[test]
fn test_matrix_route_and_delivery_contracts_accept_redacted_evidence() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[
        (
            r##"schemas/api/matrix-route-policy-resource.schema.json"##,
            r##"{"tenantId":"ten_matrix","connectorId":"matrix-main","homeserverBindingId":"matrix_hs_1","selectedRooms":[{"conversationId":"!room:example.org","conversationType":"room","roomSelectionState":"selected","validationState":"valid","redactionStatus":"redacted"}],"allowedDirectUsers":["@alice:example.org"],"roomInvocationGate":"bot_mention_or_command_required","configuredCommands":["!kura"],"encryptedRoomPolicy":"unsupported","validationState":"valid","validatedAt":"2026-05-10T10:01:00Z","redactionStatus":"redacted","safeEvidence":{"route":"selected_room_and_direct_allowment"}}"##,
        ),
        (
            r##"schemas/api/matrix-smoke-evidence-resource.schema.json"##,
            r##"{"smokeEvidenceId":"matrix_smoke_1","tenantId":"ten_matrix","connectorId":"matrix-main","homeserverBindingId":"matrix_hs_1","status":"skipped","authorizationMode":"unavailable","owner":"operator","reason":"safe Matrix credentials unavailable","remainingRisk":"No live Matrix smoke was run.","validatedAt":"2026-05-10T10:02:00Z","retentionExpiresAt":"2026-08-08T10:02:00Z","redactionStatus":"redacted","safeEvidence":{"policy":"structured_skip"}}"##,
        ),
        (
            r##"schemas/events/connector-route-outcome-recorded.event.schema.json"##,
            r##"{"eventId":"evt_matrix_route_1","sequence":1,"category":"connector","name":"connector.route_outcome_recorded","occurredAt":"2026-05-10T10:03:00Z","scope":{"connectorId":"matrix-main"},"resource":{"kind":"connector_route_outcome","id":"$event"},"payload":{"tenantId":"ten_matrix","connectorId":"matrix-main","homeserverId":"example.org","conversationId":"!room:example.org","matrixEventId":"$event","outcome":"accepted","reasonCode":"accepted","surface":"room","redactionStatus":"redacted"}}"##,
        ),
    ];
    validate_fixtures(&validator, fixtures);
}
