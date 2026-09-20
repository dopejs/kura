//! Ported from daemon/internal/contracts/activation_contract_test.go (wave 8 contract parity).
//!
//! Each test mirrors the corresponding Go test function: the same
//! schemaPath -> fixture set is validated through
//! Validator::validate_relative (Go ValidateRelative).

mod common;

use common::{Fixture, schema_root_dir, validate_fixtures};
use kura_contracts::Validator;

#[test]
fn test_activation_schemas_accept_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    validate_fixtures(
        &validator,
        &[common::data::activation_contract_fixtures()].concat(),
    );
}

#[test]
fn test_activation_contract_accepts_blocked_quota_readiness_response() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[(
        r##"schemas/api/activation.response.schema.json"##,
        r##"{"activation":{"activationId":"act_1","principalId":"prn_1","tenantId":"ten_personal","environmentScope":"prod","status":"blocked","currentStepId":"quota_baseline","completedStepIds":["tenant_resolved"],"blockingReasonCodes":["activation_blocked:quota_baseline_unavailable"],"readinessItems":[{"itemId":"tenant-access","itemKind":"tenant_access","status":"ready","displayName":"Tenant access","requiredForActivation":true,"retryable":false,"remediationOwner":"none_required","updatedAt":"2026-05-06T00:00:00Z"},{"itemId":"environment","itemKind":"environment","status":"ready","displayName":"Hosted environment","requiredForActivation":true,"retryable":false,"remediationOwner":"none_required","updatedAt":"2026-05-06T00:00:00Z"},{"itemId":"quota-baseline","itemKind":"quota_baseline","status":"blocked","reasonCode":"activation_blocked:quota_baseline_unavailable","displayName":"Quota baseline","requiredForActivation":true,"retryable":true,"remediationOwner":"operator","updatedAt":"2026-05-06T00:00:00Z"}],"quotaBaseline":{"tenantId":"ten_personal","planKey":"unknown","enforcementMode":"not_measurable","status":"unavailable","reasonCode":"activation_blocked:quota_baseline_unavailable","quotas":[]},"firstAction":{"actionId":"test_chat","actionKind":"test_chat","recommended":true,"available":false,"blockingItemIds":["quota-baseline"],"invokeRoute":"/v1/activation/test-chat","resultRoute":"/v1/activation"},"failureReason":{"reasonCode":"activation_blocked:quota_baseline_unavailable","stage":"quota_baseline","retryable":true,"remediationOwner":"operator"},"lastEvaluatedAt":"2026-05-06T00:00:00Z"}}"##,
    )];
    validate_fixtures(&validator, fixtures);
}
