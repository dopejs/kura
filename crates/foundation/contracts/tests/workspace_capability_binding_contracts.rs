//! Ported from daemon/internal/contracts/workspace_capability_binding_contracts_test.go (wave 8 contract parity).
//!
//! Each test mirrors the corresponding Go test function: the same
//! schemaPath -> fixture set is validated through
//! Validator::validate_relative (Go ValidateRelative).

mod common;

use common::{Fixture, schema_root_dir, validate_fixtures};
use kura_contracts::Validator;

#[test]
fn test_workspace_capability_binding_schemas_accept_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[
        (
            r##"schemas/api/workspace-resource.schema.json"##,
            r##"{"workspaceId":"ws_1","tenantId":"ten_58","displayName":"Personal Workspace","status":"active","isDefault":true,"repairStatus":"healthy","redactionStatus":"not_required","createdAt":"2026-06-03T10:00:00Z","updatedAt":"2026-06-03T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/binding-rule-resource.schema.json"##,
            r##"{"bindingId":"bnd_1","tenantId":"ten_58","scopeKind":"channel","scopeLabel":"discord:c1","selectedProfileId":"prof_1","selectedWorkspaceId":"ws_1","status":"active","repairStatus":"healthy","validationStatus":"valid","resultingSelectionSummary":"profile+workspace","lastMaterialChangeAt":"2026-06-03T10:00:00Z","redactionStatus":"redacted"}"##,
        ),
        (
            r##"schemas/api/capability-visibility-policy.schema.json"##,
            r##"{"policyId":"cvp_1","tenantId":"ten_58","scopeKind":"workspace","scopeRef":"ws_1","capabilityId":"tool.shell","visibility":"hidden","validationStatus":"valid"}"##,
        ),
        (
            r##"schemas/api/binding-runtime-evidence.schema.json"##,
            r##"{"projectionId":"brp_1","resourceKind":"thread","resourceId":"thr_1","selectedProfileId":"prof_1","selectedWorkspaceId":"ws_1","bindingScope":"channel","bindingId":"bnd_1","classification":"applied_binding","selectionReason":"explicit_binding_selection","capabilityVisibilitySummary":[{"capabilityId":"tool.shell","effective":"hidden","defaultEnabled":false,"offered":false,"executable":false,"reason":"hidden_by_policy","scope":"workspace"}],"occurredAt":"2026-06-03T10:00:00Z","redactionStatus":"redacted"}"##,
        ),
        (
            r##"schemas/api/effective-binding-selection.schema.json"##,
            r##"{"outcome":"repair_required","bindingScope":"channel","selectedProfileId":"prof_1","selectedWorkspaceId":"ws_1","repairStatus":"invalid","repairReason":"selected_profile_unavailable"}"##,
        ),
        (
            r##"schemas/api/binding-repair-status.schema.json"##,
            r##""needs_repair""##,
        ),
        (
            r##"schemas/api/workspace-list.response.schema.json"##,
            r##"{"tenantId":"ten_58","workspaces":[{"workspaceId":"ws_1","displayName":"Personal Workspace","status":"active","isDefault":true,"repairStatus":"healthy","redactionStatus":"not_required","createdAt":"2026-06-03T10:00:00Z","updatedAt":"2026-06-03T10:00:00Z"}]}"##,
        ),
        (
            r##"schemas/api/binding-list.response.schema.json"##,
            r##"{"tenantId":"ten_58","bindings":[{"bindingId":"bnd_1","scopeKind":"channel","scopeLabel":"discord:c1","status":"active","repairStatus":"healthy","validationStatus":"valid","lastMaterialChangeAt":"2026-06-03T10:00:00Z","redactionStatus":"redacted"}]}"##,
        ),
        (
            r##"schemas/api/create-binding.request.schema.json"##,
            r##"{"scopeKind":"channel","scopeRef":"discord:c1","selectedProfileId":"prof_1","selectedWorkspaceId":"ws_1"}"##,
        ),
        (
            r##"schemas/api/update-binding.request.schema.json"##,
            r##"{"disable":true,"reasonCode":"operator_disabled"}"##,
        ),
        (
            r##"schemas/api/create-workspace.request.schema.json"##,
            r##"{"displayName":"Project X"}"##,
        ),
        (
            r##"schemas/api/update-workspace.request.schema.json"##,
            r##"{"status":"archived"}"##,
        ),
        (
            r##"schemas/api/update-capability-visibility.request.schema.json"##,
            r##"{"scopeKind":"workspace","scopeRef":"ws_1","capabilityId":"tool.shell","visibility":"hidden"}"##,
        ),
        (
            r##"schemas/events/binding-lifecycle.event.schema.json"##,
            r##"{"eventId":"evt_b1","category":"binding","name":"binding.created","occurredAt":"2026-06-03T10:00:00Z","resource":{"kind":"workspace_capability_binding","id":"bnd_1"},"payload":{"bindingId":"bnd_1","actorPrincipalId":"prn_admin","outcome":"succeeded","reasonCode":"user_created_binding","permissionGate":"bindings.manage","safeSummary":"Binding created","redactionStatus":"redacted"}}"##,
        ),
        (
            r##"schemas/events/capability-visibility-changed.event.schema.json"##,
            r##"{"eventId":"evt_b2","category":"binding","name":"capability_visibility.changed","occurredAt":"2026-06-03T10:00:00Z","resource":{"kind":"capability_visibility_policy","id":"tool.shell"},"payload":{"actorPrincipalId":"prn_admin","scopeKind":"workspace","scopeRef":"ws_1","capabilityId":"tool.shell","visibility":"hidden","redactionStatus":"redacted"}}"##,
        ),
        (
            r##"schemas/events/binding-runtime-projected.event.schema.json"##,
            r##"{"eventId":"evt_b3","category":"binding","name":"binding.runtime_projected","occurredAt":"2026-06-03T10:00:00Z","resource":{"kind":"thread","id":"thr_1"},"payload":{"projectionId":"brp_1","selectedProfileId":"prof_1","selectedWorkspaceId":"ws_1","bindingScope":"channel","bindingId":"bnd_1","classification":"applied_binding","selectionReason":"explicit_binding_selection","redactionStatus":"redacted"}}"##,
        ),
    ];
    validate_fixtures(&validator, fixtures);
}

#[test]
fn test_profile_runtime_projection_accepts_applied_binding_classification() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[(
        r##"schemas/api/agent-profile-runtime-projection.schema.json"##,
        r##"{"runtimeProfileProjectionId":"rpp_1","tenantId":"ten_58","profileId":"prof_1","profileVersionId":"profv_1","selectionId":"sel_1","resourceKind":"thread","resourceId":"thr_1","threadId":"thr_1","selectionScope":"tenant_default","selectionReason":"user_activated","safeDisplayName":"Support Agent","safeSummary":"Direct support persona","configurationScope":"explicit_profile_configuration","deferredBindingClassification":"roadmap_58_applied_binding","occurredAt":"2026-06-03T10:00:00Z","redactionStatus":"redacted"}"##,
    )];
    validate_fixtures(&validator, fixtures);
}
