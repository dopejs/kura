//! Ported from daemon/internal/contracts/channel_management_list_contracts_test.go (wave 8 contract parity).
//!
//! Each test mirrors the corresponding Go test function: the same
//! schemaPath -> fixture set is validated through
//! Validator::validate_relative (Go ValidateRelative).

mod common;

use common::{Fixture, schema_root_dir, validate_fixtures};
use kura_contracts::Validator;

#[test]
fn test_channel_management_list_schema_accepts_redacted_fixture() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[(
        r##"schemas/api/channel-management-connector-list.response.schema.json"##,
        r##"{
  "tenantId": "ten_channel_management",
  "page": {
    "limit": 20,
    "order": "attention_disabled_ready_name_id"
  },
  "items": [
    {
      "connectorId": "slack-main",
      "connectorKind": "slack",
      "displayName": "Slack Main",
      "enablementState": "action-required",
      "setupState": "action-required",
      "healthStatus": "permission_blocked",
      "diagnosticFreshness": "fresh",
      "deliveryEligible": false,
      "nextAction": {
        "actionKind": "reconnect",
        "label": "Reconnect authorization",
        "reasonCode": "permission_missing",
        "remediationOwner": "tenant_admin"
      },
      "capabilities": {
        "disable": "supported",
        "re-enable": "supported",
        "repair": "supported",
        "reconnect": "supported",
        "credential-rotation": "limited",
        "route-edit": "supported",
        "foreground-reply-status": "supported",
        "background-delivery-status": "supported",
        "support-evidence": "supported"
      },
      "redactionStatus": "redacted",
      "updatedAt": "2026-05-10T10:00:00Z"
    }
  ]
}
"##,
    )];
    validate_fixtures(&validator, fixtures);
}
