//! Ported from daemon/internal/contracts/channel_management_contracts_test.go (wave 8 contract parity).
//!
//! Each test mirrors the corresponding Go test function: the same
//! schemaPath -> fixture set is validated through
//! Validator::validate_relative (Go ValidateRelative).

mod common;

use common::{Fixture, schema_root_dir, validate_fixtures};
use kura_contracts::Validator;

#[test]
fn test_channel_management_schemas_accept_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[
        (
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
        ),
        (
            r##"schemas/api/channel-management-support-evidence.schema.json"##,
            r##"{
  "supportEvidenceId": "support_slack_main",
  "tenantId": "ten_channel_management",
  "connectorId": "slack-main",
  "generatedByPrincipalId": "prn_support",
  "generatedAt": "2026-05-10T10:00:00Z",
  "currentState": "action-required",
  "diagnosticRefs": ["diag_slack_permission"],
  "repairRefs": ["repair_slack_reconnect"],
  "auditRefs": ["audit_slack_reconnect"],
  "redactions": ["message_body", "raw_provider_payload", "credentials", "authorization_grants"],
  "retentionExpiresAt": "2026-08-08T10:00:00Z",
  "redactionStatus": "redacted",
  "safeEvidence": {
    "connectorKind": "slack",
    "displayName": "Slack Main"
  }
}
"##,
        ),
        (
            r##"schemas/api/channel-management-action.schema.json"##,
            r##"{"actionKind":"reconnect","reasonCode":"permission_missing","sourceDiagnosticStateId":"diag_slack_permission"}"##,
        ),
        (
            r##"schemas/events/connector-management.event.schema.json"##,
            r##"{"eventId":"evt_channel_management_1","sequence":1,"category":"connector","name":"connector.management_support_evidence_generated","occurredAt":"2026-05-10T10:00:00Z","scope":{"connectorId":"slack-main"},"resource":{"kind":"channel_support_evidence","id":"support_slack_main"},"payload":{"tenantId":"ten_channel_management","connectorId":"slack-main","action":"support_evidence","outcome":"succeeded","evidenceId":"support_slack_main","redactionStatus":"redacted"}}"##,
        ),
    ];
    validate_fixtures(&validator, fixtures);
}
