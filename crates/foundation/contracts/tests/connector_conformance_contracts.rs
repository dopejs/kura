//! Ported from daemon/internal/contracts/connector_conformance_contracts_test.go (wave 8 contract parity).
//!
//! Each test mirrors the corresponding Go test function: the same
//! schemaPath -> fixture set is validated through
//! Validator::validate_relative (Go ValidateRelative).

mod common;

use common::{Fixture, schema_root_dir, validate_fixtures};
use kura_contracts::Validator;

#[test]
fn test_connector_conformance_schemas_accept_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    validate_fixtures(
        &validator,
        &[common::data::connector_conformance_contract_fixtures()].concat(),
    );
}

#[test]
fn test_connector_conformance_schemas_accept_telegram_fixtures() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[
        (
            r##"schemas/api/connector-capability-profile.schema.json"##,
            r##"{"connectorKind":"telegram","connectorId":"telegram-main","lifecycleState":"healthy","surfaces":{"direct_message":"supported","group_message":"supported","mention_gating":"supported","command_gating":"supported","attachments":"unsupported","voice":"unsupported","payments":"unsupported","mini_apps":"unsupported","background_delivery":"supported"},"identityRules":{"durableIdentity":"tenant_id + connector_account_id + telegram_chat_id + telegram_message_id","equivalentIdentity":"telegram_update_id retained only as redacted evidence"},"coreInvariants":{"tenant_scoped_identity":"pass","dedupe":"pass","foreground_background_separation":"pass","redacted_diagnostics":"pass"},"generatedAt":"2026-05-08T10:00:00Z","retentionExpiresAt":"2026-08-06T10:00:00Z","redactionStatus":"redacted"}"##,
        ),
        (
            r##"schemas/api/connector-conformance-result.schema.json"##,
            r##"{"conformanceResultId":"conformance_telegram_1","tenantId":"ten_telegram","connectorKind":"telegram","connectorId":"telegram-main","scenarioId":"telegram.group.command.accepted","area":"command_gating","result":"pass","reasonCode":"matched","evidenceTimestamp":"2026-05-08T10:00:00Z","redactionStatus":"redacted","retentionExpiresAt":"2026-08-06T10:00:00Z","evidenceSummary":"Allowed Telegram group command gating accepted one redacted text update."}"##,
        ),
        (
            r##"schemas/api/connector-resource.schema.json"##,
            r##"{"tenantId":"ten_telegram","connectorId":"telegram-main","kind":"telegram","displayName":"Telegram Main","status":"healthy","failureCount":0,"restartCount":0,"backoffSeconds":0,"createdAt":"2026-05-08T09:00:00Z","updatedAt":"2026-05-08T10:00:00Z","capabilityProfile":{"connectorKind":"telegram","connectorId":"telegram-main","lifecycleState":"healthy","surfaces":{"direct_message":"supported","group_message":"supported","attachments":"unsupported"},"identityRules":{"durableIdentity":"tenant_id + connector_account_id + telegram_chat_id + telegram_message_id"},"coreInvariants":{"tenant_scoped_identity":"pass"},"generatedAt":"2026-05-08T10:00:00Z","retentionExpiresAt":"2026-08-06T10:00:00Z","redactionStatus":"redacted"},"conformanceResult":{"conformanceResultId":"conformance_telegram_1","tenantId":"ten_telegram","connectorKind":"telegram","connectorId":"telegram-main","scenarioId":"telegram.direct.pass","area":"direct_message","result":"pass","evidenceTimestamp":"2026-05-08T10:00:00Z","redactionStatus":"redacted","retentionExpiresAt":"2026-08-06T10:00:00Z"}}"##,
        ),
    ];
    validate_fixtures(&validator, fixtures);
}

#[test]
fn test_connector_conformance_schemas_accept_slack_fixtures() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[
        (
            r##"schemas/api/connector-capability-profile.schema.json"##,
            r##"{"connectorKind":"slack","connectorId":"slack-main","lifecycleState":"healthy","surfaces":{"hosted_oauth_setup":"supported","submitted_token_setup":"unsupported","workspace_binding":"supported","direct_message":"supported","selected_channel_mention":"supported","channel_thread_reply":"supported","final_only_foreground_reply":"supported","connector_backed_delivery":"supported","marketplace_publication":"unsupported","enterprise_grid_administration":"unsupported","memory_based_team_context":"unsupported","files":"unsupported","voice_huddles":"unsupported","canvases":"unsupported","workflow_buttons":"unsupported","interactive_blocks":"unsupported","rich_media":"unsupported","thinking_visibility":"unsupported","incremental_visible_updates":"unsupported"},"identityRules":{"durableIdentity":"tenant_id + connector_id + workspace_id + conversation_id + slack_message_id","equivalentIdentity":"slack event_id retained only as redacted delivery evidence"},"coreInvariants":{"tenant_scoped_identity":"pass","dedupe":"pass","foreground_background_separation":"pass","redacted_diagnostics":"pass"},"generatedAt":"2026-05-08T10:00:00Z","retentionExpiresAt":"2026-08-06T10:00:00Z","redactionStatus":"redacted"}"##,
        ),
        (
            r##"schemas/api/connector-conformance-result.schema.json"##,
            r##"{"conformanceResultId":"conformance_slack_1","tenantId":"ten_slack","connectorKind":"slack","connectorId":"slack-main","scenarioId":"slack.channel_mention.thread_reply","area":"thread_reply","result":"pass","reasonCode":"matched","evidenceTimestamp":"2026-05-08T10:00:00Z","redactionStatus":"redacted","retentionExpiresAt":"2026-08-06T10:00:00Z","evidenceSummary":"Selected Slack channel mention produced one final-only thread reply with redacted identity evidence."}"##,
        ),
        (
            r##"schemas/api/connector-resource.schema.json"##,
            r##"{"tenantId":"ten_slack","connectorId":"slack-main","kind":"slack","displayName":"Slack Main","status":"healthy","failureCount":0,"restartCount":0,"backoffSeconds":0,"createdAt":"2026-05-08T09:00:00Z","updatedAt":"2026-05-08T10:00:00Z","capabilityProfile":{"connectorKind":"slack","connectorId":"slack-main","lifecycleState":"healthy","surfaces":{"hosted_oauth_setup":"supported","direct_message":"supported","selected_channel_mention":"supported","files":"unsupported"},"identityRules":{"durableIdentity":"tenant_id + connector_id + workspace_id + conversation_id + slack_message_id"},"coreInvariants":{"tenant_scoped_identity":"pass"},"generatedAt":"2026-05-08T10:00:00Z","retentionExpiresAt":"2026-08-06T10:00:00Z","redactionStatus":"redacted"},"conformanceResult":{"conformanceResultId":"conformance_slack_1","tenantId":"ten_slack","connectorKind":"slack","connectorId":"slack-main","scenarioId":"slack.direct.pass","area":"direct_message","result":"pass","evidenceTimestamp":"2026-05-08T10:00:00Z","redactionStatus":"redacted","retentionExpiresAt":"2026-08-06T10:00:00Z"}}"##,
        ),
    ];
    validate_fixtures(&validator, fixtures);
}
