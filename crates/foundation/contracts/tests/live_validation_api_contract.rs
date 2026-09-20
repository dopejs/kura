//! Ported from daemon/internal/contracts/live_validation_api_contract_test.go (wave 8 contract parity).
//!
//! Each test mirrors the corresponding Go test function: the same
//! schemaPath -> fixture set is validated through
//! Validator::validate_relative (Go ValidateRelative).

mod common;

use common::{Fixture, schema_root_dir, validate_fixtures};
use kura_contracts::Validator;

#[test]
fn test_live_validation_start_response_contract_variants() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[
        (
            r##"schemas/api/create-live-validation.response.schema.json"##,
            r##"{"attempt":{"validationId":"lv_blocked","candidateId":"candidate_1","requestedBy":"prn_1","environmentScope":"test","requestedScope":{"scopeId":"scope_1","validationId":"lv_blocked","approvalMode":"scope_level","declaredBy":"prn_1","declaredAt":"2026-04-29T10:00:00Z"},"status":"blocked","permissionDecision":{"allowed":false,"reasonCode":"permission_missing","checkedAt":"2026-04-29T10:00:00Z"},"quotaDecision":{"allowed":true,"checkedAt":"2026-04-29T10:00:00Z"},"killSwitchDecision":{"allowed":true,"checkedAt":"2026-04-29T10:00:00Z"},"approvalSummary":{"required":0,"approved":0,"denied":0,"expired":0,"pending":0},"ledgerSummary":{},"createdAt":"2026-04-29T10:00:00Z","updatedAt":"2026-04-29T10:00:00Z"},"denials":[{"reasonCode":"permission_missing","gate":"permission","message":"Missing live validation permission."}]}"##,
        ),
        (
            r##"schemas/api/create-live-validation.response.schema.json"##,
            r##"{"attempt":{"validationId":"lv_awaiting","candidateId":"candidate_1","requestedBy":"prn_1","environmentScope":"test","requestedScope":{"scopeId":"scope_1","validationId":"lv_awaiting","approvalMode":"scope_level","declaredBy":"prn_1","declaredAt":"2026-04-29T10:00:00Z"},"status":"awaiting_approval","permissionDecision":{"allowed":true,"checkedAt":"2026-04-29T10:00:00Z"},"quotaDecision":{"allowed":true,"checkedAt":"2026-04-29T10:00:00Z"},"killSwitchDecision":{"allowed":true,"checkedAt":"2026-04-29T10:00:00Z"},"approvalSummary":{"required":1,"approved":0,"denied":0,"expired":0,"pending":1},"ledgerSummary":{},"createdAt":"2026-04-29T10:00:00Z","updatedAt":"2026-04-29T10:00:00Z"}}"##,
        ),
        (
            r##"schemas/api/create-live-validation.response.schema.json"##,
            r##"{"attempt":{"validationId":"lv_running","candidateId":"candidate_1","requestedBy":"prn_1","environmentScope":"test","requestedScope":{"scopeId":"scope_1","validationId":"lv_running","approvalMode":"scope_level","declaredBy":"prn_1","declaredAt":"2026-04-29T10:00:00Z"},"status":"running","permissionDecision":{"allowed":true,"checkedAt":"2026-04-29T10:00:00Z"},"quotaDecision":{"allowed":true,"checkedAt":"2026-04-29T10:00:00Z"},"killSwitchDecision":{"allowed":true,"checkedAt":"2026-04-29T10:00:00Z"},"approvalSummary":{"required":1,"approved":1,"denied":0,"expired":0,"pending":0},"ledgerSummary":{},"createdAt":"2026-04-29T10:00:00Z","startedAt":"2026-04-29T10:00:00Z","updatedAt":"2026-04-29T10:00:00Z"}}"##,
        ),
    ];
    validate_fixtures(&validator, fixtures);
}
