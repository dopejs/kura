//! Ported from daemon/internal/contracts/contracts_test.go (wave 8 contract parity).
//!
//! Each test mirrors the corresponding Go test function: the same
//! schemaPath -> fixture set is validated through
//! Validator::validate_relative (Go ValidateRelative).

mod common;

use common::{Fixture, schema_root_dir, validate_fixtures};
use kura_contracts::Validator;

#[test]
fn test_supplemental_computer_use_schemas_accept_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    validate_fixtures(
        &validator,
        &[common::data::computer_use_contract_fixtures()].concat(),
    );
}

#[test]
fn test_request_schemas_accept_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[
        (
            r##"schemas/api/create-run.request.schema.json"##,
            r##"{"entrypoint":"chat","route":{"kind":"direct","channel":"telegram","accountId":"bot-main","peerId":"dm-1"}}"##,
        ),
        (
            r##"schemas/api/create-run.request.schema.json"##,
            r##"{"entrypoint":"chat","goal":"ship daemon"}"##,
        ),
        (
            r##"schemas/api/connector-ingress-message.request.schema.json"##,
            r##"{"route":{"kind":"group","accountId":"bot-main","peerId":"channel-1","threadId":"thread-1"},"message":{"messageId":"msg_1","text":"hello"},"run":{"entrypoint":"connector.message","goal":"handle inbound"}}"##,
        ),
        (
            r##"schemas/api/create-step.request.schema.json"##,
            r##"{"title":"plan","kind":"task","input":{"phase":"draft"}}"##,
        ),
        (
            r##"schemas/api/update-step-status.request.schema.json"##,
            r##"{"status":"planning","output":{"phase":"ok"}}"##,
        ),
        (
            r##"schemas/api/create-tool-call.request.schema.json"##,
            r##"{"capabilityId":"docs","toolName":"lookup","approvalId":"approval_1","input":{"query":"hello"}}"##,
        ),
        (
            r##"schemas/api/complete-tool-call.request.schema.json"##,
            r##"{"output":{"ok":true}}"##,
        ),
        (
            r##"schemas/api/fail-tool-call.request.schema.json"##,
            r##"{"error":"tool failed"}"##,
        ),
        (
            r##"schemas/api/create-connector.request.schema.json"##,
            r##"{"connectorId":"telegram-main","kind":"telegram","displayName":"Telegram Main"}"##,
        ),
        (
            r##"schemas/api/report-connector-health.request.schema.json"##,
            r##"{"status":"healthy"}"##,
        ),
        (
            r##"schemas/api/report-connector-failure.request.schema.json"##,
            r##"{"reason":"socket dropped"}"##,
        ),
        (
            r##"schemas/api/create-capability.request.schema.json"##,
            r##"{"capabilityId":"docs","kind":"docs","displayName":"Docs"}"##,
        ),
        (
            r##"schemas/api/report-capability-health.request.schema.json"##,
            r##"{"status":"degraded"}"##,
        ),
        (
            r##"schemas/api/report-capability-failure.request.schema.json"##,
            r##"{"reason":"worker exited"}"##,
        ),
        (
            r##"schemas/api/create-llm-dispatch.request.schema.json"##,
            r##"{"provider":"echo","model":"echo-v1","messages":[{"role":"user","content":"hello"}],"timeoutMs":1000,"maxRetries":1}"##,
        ),
        (
            r##"schemas/api/chat-query.request.schema.json"##,
            r##"{"provider":"echo","model":"echo-v1","skills":["shared"],"query":"hello","timeoutMs":1000,"maxRetries":1}"##,
        ),
        (
            r##"schemas/api/run-provider-check.request.schema.json"##,
            r##"{"model":"echo-v1","prompt":"hello"}"##,
        ),
        (
            r##"schemas/api/provider-default-model.request.schema.json"##,
            r##"{"model":"gpt-5.4"}"##,
        ),
        (
            r##"schemas/api/create-schedule.request.schema.json"##,
            r##"{"trigger":{"kind":"once","fireAt":"2026-04-22T10:01:00Z"},"target":{"kind":"workflow","workflow":{"entrypoint":"operator","runGoal":"dispatch calendar workflow","workflowGoal":"create calendar event","calendarAction":{"operationClass":"create_event","integrationId":"calendar-a","title":"Calendar workflow","startsAt":"2026-04-23T17:00:00Z","endsAt":"2026-04-23T17:30:00Z"}}},"retryPolicy":{"maxRetries":1,"backoffKind":"fixed","baseDelaySeconds":5,"maxDelaySeconds":5}}"##,
        ),
        (
            r##"schemas/api/request-approval.request.schema.json"##,
            r##"{"action":"tool_call.execute","resourceKind":"capability","resourceId":"browser","reason":"needs approval","requestedBy":"web-ui"}"##,
        ),
        (
            r##"schemas/api/resolve-approval.request.schema.json"##,
            r##"{"resolution":"approved","comment":"allowed"}"##,
        ),
        (
            r##"schemas/api/create-integration.request.schema.json"##,
            r##"{"integrationId":"calendar-a","domainKind":"calendar","displayName":"Calendar A","backendKind":"fake_local","accountBinding":{"accountKey":"acct_calendar"},"canonicalDefault":true}"##,
        ),
        (
            r##"schemas/api/report-integration-readiness.request.schema.json"##,
            r##"{"readinessStatus":"healthy","authState":"authorized","healthState":"healthy","reason":"probe passed","requiredOperatorAction":"none","secretResolution":"resolved","accountBinding":{"accountKey":"acct_calendar"}}"##,
        ),
        (
            r##"schemas/api/set-integration-default.request.schema.json"##,
            r##"{}"##,
        ),
        (
            r##"schemas/api/create-integration-probe.request.schema.json"##,
            r##"{"probeKind":"mutate","approvalId":"approval_1","input":{"mode":"write"}}"##,
        ),
        (
            r##"schemas/api/create-calendar-availability-query.request.schema.json"##,
            r##"{"integrationId":"calendar-a","windowStart":"2026-04-23T16:00:00Z","windowEnd":"2026-04-23T18:00:00Z","source":{"runId":"run_1","stepId":"step_1","toolCallId":"tool_call_1","workflowId":"wf_1","workflowStepId":"wfstep_1","scheduleId":"sched_1","scheduleAttemptId":"sched_attempt_1","deliveryId":"delivery_1"}}"##,
        ),
        (
            r##"schemas/api/create-calendar-event.request.schema.json"##,
            r##"{"integrationId":"calendar-a","title":"Design review","startsAt":"2026-04-23T17:00:00Z","endsAt":"2026-04-23T17:30:00Z","timezone":"America/Los_Angeles"}"##,
        ),
        (
            r##"schemas/api/update-calendar-event.request.schema.json"##,
            r##"{"title":"Moved review","startsAt":"2026-04-23T18:00:00Z","endsAt":"2026-04-23T18:30:00Z"}"##,
        ),
        (
            r##"schemas/api/cancel-calendar-event.request.schema.json"##,
            r##"{"reason":"cancelled","source":{"runId":"run_1"}}"##,
        ),
        (
            r##"schemas/api/create-mail-draft.request.schema.json"##,
            r##"{"integrationId":"mail-a","composeMode":"new_message","to":["carol@example.com"],"subject":"Phase 30 draft","body":"Hello","source":{"runId":"run_1","workflowId":"wf_1","allowSendSideEffects":false}}"##,
        ),
        (
            r##"schemas/api/update-mail-draft.request.schema.json"##,
            r##"{"integrationId":"mail-a","subject":"Updated draft","attachmentRefs":[{"attachmentRefId":"attachment_1","displayName":"brief.pdf","mediaType":"application/pdf","sizeBytes":1024}],"source":{"runId":"run_1"}}"##,
        ),
        (
            r##"schemas/api/send-mail-message.request.schema.json"##,
            r##"{"integrationId":"mail-a","to":["carol@example.com"],"subject":"Phase 30 send","body":"Hello","source":{"workflowId":"wf_1","allowSendSideEffects":true}}"##,
        ),
        (
            r##"schemas/api/send-mail-draft.request.schema.json"##,
            r##"{"integrationId":"mail-a","source":{"scheduleId":"sched_1","scheduleAttemptId":"sched_attempt_1","allowSendSideEffects":true}}"##,
        ),
        (
            r##"schemas/api/reply-mail-message.request.schema.json"##,
            r##"{"integrationId":"mail-a","resultMode":"draft","body":"Reply later","source":{"runId":"run_1"}}"##,
        ),
        (
            r##"schemas/api/forward-mail-message.request.schema.json"##,
            r##"{"integrationId":"mail-a","resultMode":"send","to":["dave@example.com"],"body":"FYI","source":{"workflowId":"wf_1","allowSendSideEffects":true}}"##,
        ),
        (
            r##"schemas/api/sandbox-execution.request.schema.json"##,
            r##"{"profileId":"subprocess_default","command":"echo","args":["hello"],"cwd":"/tmp/kura","timeoutMs":1000,"requestedBy":"web-ui","resourceKind":"skill","resourceId":"shared","scope":"chat","reason":"inspect profile","metadata":{"ticket":"sandbox-16"},"access":{"readRoots":["/tmp/kura"],"writeRoots":["/tmp/kura"],"networkMode":"allow_list","allowedHosts":["localhost"],"allowedPorts":[80],"allowLoopback":true}}"##,
        ),
        (
            r##"schemas/api/sandbox-explain.request.schema.json"##,
            r##"{"profileId":"subprocess_default","command":"echo","args":["hello"],"cwd":"/tmp/kura","access":{"readRoots":["/tmp/kura"],"writeRoots":["/tmp/kura"],"allowedHosts":[],"allowedPorts":[]}}"##,
        ),
        (
            r##"schemas/api/mcp-server-create.request.schema.json"##,
            r##"{"serverId":"mcp-test","displayName":"MCP Test","enabled":true,"sandboxProfileId":"subprocess_default","declarationId":"mcp_server:mcp-test:lifecycle.start","transportKind":"stdio","command":"/tmp/mcp-helper","args":["--stdio"],"workingDir":"/tmp/kura","secretRefs":["MCP_TEST_TOKEN"],"autoRestart":true}"##,
        ),
        (
            r##"schemas/api/mcp-server-update.request.schema.json"##,
            r##"{"displayName":"Updated MCP","enabled":false,"autoRestart":false}"##,
        ),
        (
            r##"schemas/api/mcp-catalog-install-request.schema.json"##,
            r##"{"serverId":"filesystem-test","displayName":"Filesystem Test","workingDir":"/tmp/kura"}"##,
        ),
        (
            r##"schemas/api/mcp-tool-exposure-update.request.schema.json"##,
            r##"{"runtimeSurface":"chat","exposureMode":"approval_required","active":true,"reason":"needs approval"}"##,
        ),
        (
            r##"schemas/api/mcp-tool-authorization.request.schema.json"##,
            r##"{"runtimeSurface":"chat","approvalId":"approval_1","requestedBy":"web-ui"}"##,
        ),
        (
            r##"schemas/api/start-pairing.request.schema.json"##,
            r##"{"mode":"local","label":"web-ui","ttlSeconds":120}"##,
        ),
        (
            r##"schemas/api/complete-pairing.request.schema.json"##,
            r##"{"code":"123456"}"##,
        ),
    ];
    validate_fixtures(&validator, fixtures);
}

#[test]
fn test_schedule_schemas_accept_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    validate_fixtures(
        &validator,
        &[common::data::schedule_contract_fixtures()].concat(),
    );
}

#[test]
fn test_integration_schemas_accept_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    validate_fixtures(
        &validator,
        &[common::data::integration_contract_fixtures()].concat(),
    );
}

#[test]
fn test_integration_adapter_schemas_accept_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[
        (
            r##"schemas/capability/integration-adapter/request.json"##,
            r##"{"requestId":"req-1","contractVersion":"1","domain":"calendar","operation":"ProjectAccount","deadlineMs":30000,"resource":{}}"##,
        ),
        (
            r##"schemas/capability/integration-adapter/response.json"##,
            r##"{"requestId":"req-1","contractVersion":"1","status":"ok","payload":{}}"##,
        ),
        (
            r##"schemas/events/integrations/adapter-health.json"##,
            r##"{"capabilityId":"cap-1","domain":"calendar","status":"healthy","readiness":"ready"}"##,
        ),
    ];
    validate_fixtures(&validator, fixtures);
}

#[test]
fn test_delivery_schemas_accept_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    validate_fixtures(
        &validator,
        &[common::data::delivery_contract_fixtures()].concat(),
    );
}

#[test]
fn test_calendar_schemas_accept_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    validate_fixtures(
        &validator,
        &[common::data::calendar_contract_fixtures()].concat(),
    );
}

#[test]
fn test_mail_schemas_accept_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    validate_fixtures(
        &validator,
        &[common::data::mail_contract_fixtures()].concat(),
    );
}

#[test]
fn test_reminder_schemas_accept_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    validate_fixtures(
        &validator,
        &[common::data::reminder_contract_fixtures()].concat(),
    );
}

#[test]
fn test_validator_rejects_invalid_request_fixture() {
    let validator = Validator::new(schema_root_dir());
    let err = validator.validate_relative(
        r##"schemas/api/create-run.request.schema.json"##,
        r##"{"goal":"missing entrypoint"}"##.as_bytes(),
    );
    assert!(err.is_err(), "expected fixture to fail schema validation");
}

#[test]
fn test_chat_stream_schemas_accept_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[
        (
            r##"schemas/api/chat-query-stream-started.event.schema.json"##,
            r##"{"dispatchId":"dispatch_1","provider":"openai_compatible","model":"gpt-test","skills":["shared"],"skillContracts":[{"declaration":{"declarationId":"skill:shared:selection","consumerKind":"skill","consumerId":"shared","operationKind":"skill_selection","profileId":"subprocess_default","executionMode":"declaration_only","allowedBackendKinds":["subprocess"],"readRoots":["/tmp/kura/skills/shared"],"writeRoots":[],"networkMode":"deny","secretRefs":[],"approvalMode":"allow","requiredEnforcementStrength":"declared_only","active":true,"source":"builtin"}}],"query":"hello"}"##,
        ),
        (
            r##"schemas/api/chat-query-stream-delta.event.schema.json"##,
            r##"{"dispatchId":"dispatch_1","delta":"hello","reply":"hello"}"##,
        ),
        (
            r##"schemas/api/chat-query.response.schema.json"##,
            r##"{"dispatchId":"dispatch_1","provider":"openai_compatible","model":"gpt-test","skills":["shared"],"skillContracts":[{"declaration":{"declarationId":"skill:shared:selection","consumerKind":"skill","consumerId":"shared","operationKind":"skill_selection","profileId":"subprocess_default","executionMode":"declaration_only","allowedBackendKinds":["subprocess"],"readRoots":["/tmp/kura/skills/shared"],"writeRoots":[],"networkMode":"deny","secretRefs":[],"approvalMode":"allow","requiredEnforcementStrength":"declared_only","active":true,"source":"builtin"}}],"query":"hello","status":"completed","partial":false,"reply":"hello world","finishReason":"stop","usage":{"inputTokens":2,"outputTokens":3,"totalTokens":5}}"##,
        ),
        (
            r##"schemas/api/chat-query.response.schema.json"##,
            r##"{"dispatchId":"dispatch_2","provider":"openai_compatible","model":"gpt-test","skills":[],"query":"what did I decide about caching?","status":"completed","partial":false,"reply":"You chose write-through.","finishReason":"stop","usage":{"inputTokens":20,"outputTokens":9,"totalTokens":29},"toolTrace":[{"round":0,"dispatchId":"dispatch_1","callId":"call_1","name":"memory.lookup","arguments":"{\"query\":\"caching decision\"}","output":"[1] (l1) caching: write-through chosen","isError":false,"durationMs":3}]}"##,
        ),
    ];
    validate_fixtures(&validator, fixtures);
}

#[test]
fn test_policy_schemas_accept_sandbox_fixtures() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[
        (
            r##"schemas/api/approval-resource.schema.json"##,
            r##"{"approvalId":"approval_1","action":"tool_call.execute","resourceKind":"capability","resourceId":"shell","reason":"need approval","status":"pending","createdAt":"2026-04-18T12:00:00Z","updatedAt":"2026-04-18T12:00:00Z","sandbox":{"declaration":{"declarationId":"local_tool:shell:tool_call.execute","consumerKind":"local_tool","consumerId":"shell","operationKind":"tool_call.execute","profileId":"subprocess_default","executionMode":"access_only","allowedBackendKinds":["subprocess"],"networkMode":"deny","secretRefs":[],"approvalMode":"ask","requiredEnforcementStrength":"declared_only","active":true,"source":"builtin"},"policyRecord":{"policyRecordId":"policy_local_tool_shell_1","consumerKind":"local_tool","consumerId":"shell","operationKind":"tool_call.execute","declarationId":"local_tool:shell:tool_call.execute","requestedBy":"web-ui","approvalId":"approval_1","decisionId":"decision_1","decision":"ask","approvalStatus":"pending","secretResolution":"not_applicable","enforcementStrength":"declared_only","startedAt":"2026-04-18T12:00:00Z","status":"approval_pending"}}}"##,
        ),
        (
            r##"schemas/api/decision-resource.schema.json"##,
            r##"{"decisionId":"decision_1","action":"tool_call.execute","resourceKind":"capability","resourceId":"shell","outcome":"requires_approval","reason":"need approval","approvalId":"approval_1","createdAt":"2026-04-18T12:00:00Z","sandbox":{"declaration":{"declarationId":"local_tool:shell:tool_call.execute","consumerKind":"local_tool","consumerId":"shell","operationKind":"tool_call.execute","profileId":"subprocess_default","executionMode":"access_only","allowedBackendKinds":["subprocess"],"networkMode":"deny","secretRefs":[],"approvalMode":"ask","requiredEnforcementStrength":"declared_only","active":true,"source":"builtin"},"policyRecord":{"policyRecordId":"policy_local_tool_shell_1","consumerKind":"local_tool","consumerId":"shell","operationKind":"tool_call.execute","declarationId":"local_tool:shell:tool_call.execute","requestedBy":"web-ui","approvalId":"approval_1","decisionId":"decision_1","decision":"ask","approvalStatus":"pending","secretResolution":"not_applicable","enforcementStrength":"declared_only","startedAt":"2026-04-18T12:00:00Z","status":"approval_pending"}}}"##,
        ),
        (
            r##"schemas/events/policy-approval-requested.event.schema.json"##,
            r##"{"eventId":"evt_policy_1","sequence":12,"category":"policy","name":"policy.approval_requested","occurredAt":"2026-04-18T12:00:01Z","scope":{},"resource":{"kind":"approval","id":"approval_1"},"payload":{"action":"tool_call.execute","resourceKind":"capability","resourceId":"shell","status":"pending","sandbox":{"declaration":{"declarationId":"local_tool:shell:tool_call.execute","consumerKind":"local_tool","consumerId":"shell","operationKind":"tool_call.execute","profileId":"subprocess_default","executionMode":"access_only","allowedBackendKinds":["subprocess"],"networkMode":"deny","secretRefs":[],"approvalMode":"ask","requiredEnforcementStrength":"declared_only","active":true,"source":"builtin"},"policyRecord":{"policyRecordId":"policy_local_tool_shell_1","consumerKind":"local_tool","consumerId":"shell","operationKind":"tool_call.execute","declarationId":"local_tool:shell:tool_call.execute","requestedBy":"web-ui","approvalId":"approval_1","decisionId":"decision_1","decision":"ask","approvalStatus":"pending","secretResolution":"not_applicable","enforcementStrength":"declared_only","startedAt":"2026-04-18T12:00:00Z","status":"approval_pending"}}}}"##,
        ),
        (
            r##"schemas/events/policy-approval-resolved.event.schema.json"##,
            r##"{"eventId":"evt_policy_2","sequence":13,"category":"policy","name":"policy.approval_resolved","occurredAt":"2026-04-18T12:00:02Z","scope":{},"resource":{"kind":"approval","id":"approval_1"},"payload":{"action":"tool_call.execute","resourceKind":"capability","resourceId":"shell","status":"approved","resolution":"approved","sandbox":{"declaration":{"declarationId":"local_tool:shell:tool_call.execute","consumerKind":"local_tool","consumerId":"shell","operationKind":"tool_call.execute","profileId":"subprocess_default","executionMode":"access_only","allowedBackendKinds":["subprocess"],"networkMode":"deny","secretRefs":[],"approvalMode":"ask","requiredEnforcementStrength":"declared_only","active":true,"source":"builtin"},"policyRecord":{"policyRecordId":"policy_local_tool_shell_1","consumerKind":"local_tool","consumerId":"shell","operationKind":"tool_call.execute","declarationId":"local_tool:shell:tool_call.execute","requestedBy":"web-ui","approvalId":"approval_1","decisionId":"decision_2","decision":"allow","approvalStatus":"approved","secretResolution":"not_applicable","enforcementStrength":"declared_only","startedAt":"2026-04-18T12:00:00Z","completedAt":"2026-04-18T12:00:02Z","status":"preflight_allowed"}}}}"##,
        ),
        (
            r##"schemas/events/policy-decision-recorded.event.schema.json"##,
            r##"{"eventId":"evt_policy_3","sequence":14,"category":"policy","name":"policy.decision_recorded","occurredAt":"2026-04-18T12:00:02Z","scope":{},"resource":{"kind":"decision","id":"decision_2"},"payload":{"action":"tool_call.execute","resourceKind":"capability","resourceId":"shell","outcome":"approved","approvalId":"approval_1","sandbox":{"declaration":{"declarationId":"local_tool:shell:tool_call.execute","consumerKind":"local_tool","consumerId":"shell","operationKind":"tool_call.execute","profileId":"subprocess_default","executionMode":"access_only","allowedBackendKinds":["subprocess"],"networkMode":"deny","secretRefs":[],"approvalMode":"ask","requiredEnforcementStrength":"declared_only","active":true,"source":"builtin"},"policyRecord":{"policyRecordId":"policy_local_tool_shell_1","consumerKind":"local_tool","consumerId":"shell","operationKind":"tool_call.execute","declarationId":"local_tool:shell:tool_call.execute","requestedBy":"web-ui","approvalId":"approval_1","decisionId":"decision_2","decision":"allow","approvalStatus":"approved","secretResolution":"not_applicable","enforcementStrength":"declared_only","startedAt":"2026-04-18T12:00:00Z","completedAt":"2026-04-18T12:00:02Z","status":"preflight_allowed"}}}}"##,
        ),
    ];
    validate_fixtures(&validator, fixtures);
}

#[test]
fn test_provider_schemas_accept_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[
        (
            r##"schemas/api/provider-resource.schema.json"##,
            r##"{"providerId":"echo","title":"Echo","family":"builtin_echo","authMode":"none","source":"builtin","modelSelectionMode":"fixed","knownModels":["echo-v1"],"registered":true,"configured":true,"ready":true,"default":true,"defaultModel":"echo-v1","effectiveModel":"echo-v1","effectiveTimeoutMs":30000,"effectiveMaxRetries":0,"secretConfigured":false,"capabilities":{"chat":true,"stream":true}}"##,
        ),
        (
            r##"schemas/api/provider-check-resource.schema.json"##,
            r##"{"checkId":"provider_check_1","providerId":"echo","family":"builtin_echo","authMode":"none","status":"passed","model":"echo-v1","usage":{"inputTokens":1,"outputTokens":1,"totalTokens":2},"createdAt":"2026-04-18T12:00:00Z","completedAt":"2026-04-18T12:00:01Z"}"##,
        ),
        (
            r##"schemas/api/provider-auth-state.response.schema.json"##,
            r##"{"auth":{"providerId":"codex_managed","family":"codex_cli","authMode":"local_cli_bridge","status":"authenticated","cliAvailable":true,"accountLabel":"user@example.com","accountId":"acct_1","plan":"pro","authMethod":"chatgpt","loginCommand":["codex","login"],"logoutCommand":["codex","logout"],"lastCheckedAt":"2026-04-18T12:00:00Z","lastAuthenticatedAt":"2026-04-18T11:59:00Z","metadata":{"source":"contract","managedProviderId":"codex_managed","managedProviderAction":"auth_status","sandboxProfileId":"managed_provider_codex","sandboxDecision":"allow","enforcementStrength":"declared_only"}}}"##,
        ),
        (
            r##"schemas/api/provider-model.schema.json"##,
            r##"{"providerId":"codex_managed","modelId":"gpt-5.4","displayName":"GPT-5.4","description":"Primary coding model","default":true,"available":true,"source":"cache","chat":true,"stream":true,"coding":true,"toolUse":false,"reasoningLevels":["medium","high"]}"##,
        ),
        (
            r##"schemas/api/provider-model-list.response.schema.json"##,
            r##"{"items":[{"providerId":"codex_managed","modelId":"gpt-5.4","displayName":"GPT-5.4","default":true,"available":true,"source":"cache","chat":true,"stream":true,"coding":true,"toolUse":false}]}"##,
        ),
        (
            r##"schemas/api/provider-default-model.response.schema.json"##,
            r##"{"providerId":"codex_managed","defaultModel":"gpt-5.4","updatedAt":"2026-04-18T12:00:00Z"}"##,
        ),
        (
            r##"schemas/events/provider-check-completed.event.schema.json"##,
            r##"{"eventId":"evt_1","sequence":1,"category":"provider","name":"provider.check_completed","occurredAt":"2026-04-18T12:00:01Z","scope":{},"resource":{"kind":"provider_check","id":"provider_check_1"},"payload":{"providerId":"echo","family":"builtin_echo","authMode":"none","status":"passed","model":"echo-v1","endpoint":"","usage":{"inputTokens":1,"outputTokens":1,"totalTokens":2}}}"##,
        ),
        (
            r##"schemas/events/provider-check-failed.event.schema.json"##,
            r##"{"eventId":"evt_2","sequence":2,"category":"provider","name":"provider.check_failed","occurredAt":"2026-04-18T12:00:01Z","scope":{},"resource":{"kind":"provider_check","id":"provider_check_2"},"payload":{"providerId":"openai_compatible","family":"openai_compatible","authMode":"api_key","status":"failed","model":"gpt-5.4","errorClass":"auth_error","errorCode":"upstream_auth_failed","errorMessage":"unauthorized","usage":{"inputTokens":0,"outputTokens":0,"totalTokens":0}}}"##,
        ),
        (
            r##"schemas/events/provider-auth-started.event.schema.json"##,
            r##"{"eventId":"evt_3","sequence":3,"category":"provider","name":"provider.auth_started","occurredAt":"2026-04-18T12:00:01Z","scope":{},"resource":{"kind":"provider_auth","id":"codex_managed"},"payload":{"providerId":"codex_managed","family":"codex_cli","authMode":"local_cli_bridge","status":"pending_login","cliAvailable":true,"accountLabel":"","accountId":"","plan":"","authMethod":"","lastError":"","metadata":{"source":"contract","managedProviderId":"codex_managed","managedProviderAction":"auth_status","sandboxProfileId":"managed_provider_codex","sandboxDecision":"allow","enforcementStrength":"declared_only"}}}"##,
        ),
        (
            r##"schemas/events/provider-auth-completed.event.schema.json"##,
            r##"{"eventId":"evt_4","sequence":4,"category":"provider","name":"provider.auth_completed","occurredAt":"2026-04-18T12:00:01Z","scope":{},"resource":{"kind":"provider_auth","id":"codex_managed"},"payload":{"providerId":"codex_managed","family":"codex_cli","authMode":"local_cli_bridge","status":"authenticated","cliAvailable":true,"accountLabel":"user@example.com","accountId":"acct_1","plan":"pro","authMethod":"chatgpt","lastError":"","metadata":{"source":"contract","managedProviderId":"codex_managed","managedProviderAction":"auth_status","sandboxProfileId":"managed_provider_codex","sandboxDecision":"allow","enforcementStrength":"declared_only"}}}"##,
        ),
        (
            r##"schemas/events/provider-auth-refreshed.event.schema.json"##,
            r##"{"eventId":"evt_5","sequence":5,"category":"provider","name":"provider.auth_refreshed","occurredAt":"2026-04-18T12:00:01Z","scope":{},"resource":{"kind":"provider_auth","id":"codex_managed"},"payload":{"providerId":"codex_managed","family":"codex_cli","authMode":"local_cli_bridge","status":"authenticated","cliAvailable":true,"accountLabel":"user@example.com","accountId":"acct_1","plan":"pro","authMethod":"chatgpt","lastError":"","metadata":{"source":"contract","managedProviderId":"codex_managed","managedProviderAction":"auth_status","sandboxProfileId":"managed_provider_codex","sandboxDecision":"allow","enforcementStrength":"declared_only"}}}"##,
        ),
        (
            r##"schemas/events/provider-auth-revoked.event.schema.json"##,
            r##"{"eventId":"evt_6","sequence":6,"category":"provider","name":"provider.auth_revoked","occurredAt":"2026-04-18T12:00:01Z","scope":{},"resource":{"kind":"provider_auth","id":"codex_managed"},"payload":{"providerId":"codex_managed","family":"codex_cli","authMode":"local_cli_bridge","status":"revoked","cliAvailable":true,"accountLabel":"","accountId":"","plan":"","authMethod":"","lastError":"","metadata":{"source":"contract","managedProviderId":"codex_managed","managedProviderAction":"logout","sandboxProfileId":"managed_provider_codex","sandboxDecision":"allow","enforcementStrength":"declared_only"}}}"##,
        ),
        (
            r##"schemas/events/provider-default-model-updated.event.schema.json"##,
            r##"{"eventId":"evt_7","sequence":7,"category":"provider","name":"provider.default_model_updated","occurredAt":"2026-04-18T12:00:01Z","scope":{},"resource":{"kind":"provider","id":"codex_managed"},"payload":{"providerId":"codex_managed","defaultModel":"gpt-5.4","updatedAt":"2026-04-18T12:00:01Z"}}"##,
        ),
    ];
    validate_fixtures(&validator, fixtures);
}

#[test]
fn test_m_c_p_schemas_accept_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[
        (
            r##"schemas/api/mcp-server-resource.schema.json"##,
            r##"{"serverId":"mcp-test","displayName":"MCP Test","source":"api","originKind":"catalog","catalogEntryId":"filesystem","installMethod":"api","environmentScope":"test","catalogManagement":{"sourceKind":"bundled","installedRevision":"sha256:installed","currentRevision":"sha256:current","driftStatus":"catalog_updated","driftReason":"installed server no longer matches the current catalog revision","installedAt":"2026-04-18T12:00:00Z","lastMaintainedAt":"2026-04-18T12:05:00Z","lastActionAt":"2026-04-18T12:06:00Z","lastAction":"revalidate","lastActionStatus":"completed","lastActionReason":"installed server no longer matches the current catalog revision","installInputSnapshot":{"serverId":"mcp-test","displayName":"MCP Test","enabled":true,"sandboxProfileId":"subprocess_default","secretRefs":["MCP_TEST_TOKEN"],"installMethod":"api"},"lastRevalidation":{"checkedAt":"2026-04-18T12:06:00Z","status":"ready","classification":"catalog_drift","reason":"installed server no longer matches the current catalog revision","issues":[{"kind":"catalog","name":"filesystem","status":"warning","reason":"installed server no longer matches the current catalog revision","environmentScope":"test"}]}},"enabled":true,"sandboxProfileId":"subprocess_default","declarationId":"mcp_server:mcp-test:lifecycle.start","declaration":{"executionMode":"subprocess","allowedBackendKinds":["subprocess"],"networkMode":"deny","approvalMode":"allow","requiredEnforcementStrength":"declared_only","active":true},"transportKind":"stdio","command":"/tmp/mcp-helper","args":["--stdio"],"workingDir":"/tmp/kura","secretRefs":["MCP_TEST_TOKEN"],"autoRestart":true,"createdAt":"2026-04-18T12:00:00Z","updatedAt":"2026-04-18T12:00:00Z","state":{"serverId":"mcp-test","status":"healthy","failureCount":0,"restartCount":0,"lastStartedAt":"2026-04-18T12:00:01Z","lastHeartbeatAt":"2026-04-18T12:00:02Z","lastExecutionId":"sandbox_exec_1","lastPolicyRecordId":"policy_mcp_1","updatedAt":"2026-04-18T12:00:02Z"},"secretSummary":[{"consumerId":"mcp-test","secretRef":"MCP_TEST_TOKEN","environmentScope":"test","defaultRuleId":"mcp_server:mcp-test","resolution":"resolved","deliveryKind":"environment_variable","redactionRule":"value_redacted"}],"toolCount":1,"tools":[{"serverId":"mcp-test","toolName":"lookup","title":"Lookup","description":"Lookup tool","schemaFingerprint":"abc123","discoveryStatus":"discovered","lastDiscoveredAt":"2026-04-18T12:00:02Z","updatedAt":"2026-04-18T12:00:02Z","effectiveAvailability":"blocked","approvalRequired":false}]}"##,
        ),
        (
            r##"schemas/api/mcp-server-list.response.schema.json"##,
            r##"{"items":[{"serverId":"mcp-test","displayName":"MCP Test","source":"api","originKind":"catalog","catalogEntryId":"filesystem","installMethod":"api","environmentScope":"test","catalogManagement":{"sourceKind":"bundled","installedRevision":"sha256:installed","currentRevision":"sha256:current","driftStatus":"catalog_updated","driftReason":"installed server no longer matches the current catalog revision","installedAt":"2026-04-18T12:00:00Z","lastMaintainedAt":"2026-04-18T12:05:00Z","lastActionAt":"2026-04-18T12:06:00Z","lastAction":"revalidate","lastActionStatus":"completed","lastActionReason":"installed server no longer matches the current catalog revision","installInputSnapshot":{"serverId":"mcp-test","displayName":"MCP Test","enabled":true,"sandboxProfileId":"subprocess_default","secretRefs":["MCP_TEST_TOKEN"],"installMethod":"api"},"lastRevalidation":{"checkedAt":"2026-04-18T12:06:00Z","status":"ready","classification":"catalog_drift","reason":"installed server no longer matches the current catalog revision","issues":[{"kind":"catalog","name":"filesystem","status":"warning","reason":"installed server no longer matches the current catalog revision","environmentScope":"test"}]}},"enabled":true,"sandboxProfileId":"subprocess_default","declarationId":"mcp_server:mcp-test:lifecycle.start","declaration":{"executionMode":"subprocess","allowedBackendKinds":["subprocess"],"networkMode":"deny","approvalMode":"allow","requiredEnforcementStrength":"declared_only","active":true},"transportKind":"stdio","command":"/tmp/mcp-helper","args":["--stdio"],"workingDir":"/tmp/kura","secretRefs":["MCP_TEST_TOKEN"],"autoRestart":true,"createdAt":"2026-04-18T12:00:00Z","updatedAt":"2026-04-18T12:00:00Z","state":{"serverId":"mcp-test","status":"healthy","failureCount":0,"restartCount":0,"updatedAt":"2026-04-18T12:00:02Z"},"toolCount":1,"tools":[{"serverId":"mcp-test","toolName":"lookup","discoveryStatus":"discovered","updatedAt":"2026-04-18T12:00:02Z","effectiveAvailability":"blocked","approvalRequired":false}]}]}"##,
        ),
        (
            r##"schemas/api/mcp-server-lifecycle.response.schema.json"##,
            r##"{"action":"start","server":{"serverId":"mcp-test","displayName":"MCP Test","source":"api","enabled":true,"sandboxProfileId":"subprocess_default","declarationId":"mcp_server:mcp-test:lifecycle.start","declaration":{"executionMode":"subprocess","allowedBackendKinds":["subprocess"],"networkMode":"deny","approvalMode":"allow","requiredEnforcementStrength":"declared_only","active":true},"transportKind":"stdio","command":"/tmp/mcp-helper","args":["--stdio"],"workingDir":"/tmp/kura","secretRefs":["MCP_TEST_TOKEN"],"autoRestart":true,"createdAt":"2026-04-18T12:00:00Z","updatedAt":"2026-04-18T12:00:00Z","state":{"serverId":"mcp-test","status":"healthy","failureCount":0,"restartCount":0,"updatedAt":"2026-04-18T12:00:02Z"},"toolCount":1,"tools":[{"serverId":"mcp-test","toolName":"lookup","discoveryStatus":"discovered","updatedAt":"2026-04-18T12:00:02Z","effectiveAvailability":"blocked","approvalRequired":false}]},"idempotent":false,"executionId":"sandbox_exec_1","blocked":false,"preflightMs":42}"##,
        ),
        (
            r##"schemas/api/mcp-transport-capability.schema.json"##,
            r##"{"transportKind":"websocket","availabilityStatus":"ready","healthStatus":"degraded","reason":"one or more servers are recovering","prerequisites":["websocket endpoint must be configured per server","authenticated endpoints require secret-ref-backed header auth"],"environmentScope":"test","supportedAuthKinds":["bearer_header","header"],"daemonManagedReconnect":true,"recoverySummary":"daemon manages bounded websocket reconnect and restore history"}"##,
        ),
        (
            r##"schemas/api/mcp-transport-capability-list.response.schema.json"##,
            r##"{"items":[{"transportKind":"stdio","availabilityStatus":"ready","healthStatus":"healthy","environmentScope":"test","daemonManagedReconnect":false},{"transportKind":"websocket","availabilityStatus":"ready","healthStatus":"degraded","reason":"one or more servers are recovering","prerequisites":["websocket endpoint must be configured per server"],"environmentScope":"test","supportedAuthKinds":["bearer_header"],"daemonManagedReconnect":true,"recoverySummary":"daemon manages bounded websocket reconnect and restore history"}]}"##,
        ),
        (
            r##"schemas/api/mcp-tool-resource.schema.json"##,
            r##"{"serverId":"mcp-test","toolName":"lookup","title":"Lookup","description":"Lookup tool","schemaFingerprint":"abc123","discoveryStatus":"discovered","lastDiscoveredAt":"2026-04-18T12:00:02Z","updatedAt":"2026-04-18T12:00:02Z","exposure":[{"serverId":"mcp-test","toolName":"lookup","runtimeSurface":"chat","exposureMode":"approval_required","active":true,"reason":"needs approval","updatedAt":"2026-04-18T12:00:03Z"}],"effectiveAvailability":"available","approvalRequired":true}"##,
        ),
        (
            r##"schemas/api/mcp-tool-list.response.schema.json"##,
            r##"{"items":[{"serverId":"mcp-test","toolName":"lookup","discoveryStatus":"discovered","updatedAt":"2026-04-18T12:00:02Z","effectiveAvailability":"blocked","approvalRequired":false}]}"##,
        ),
        (
            r##"schemas/api/mcp-tool-authorization.response.schema.json"##,
            r##"{"status":"pending","tool":{"serverId":"mcp-test","toolName":"lookup","discoveryStatus":"discovered","updatedAt":"2026-04-18T12:00:02Z","effectiveAvailability":"available","approvalRequired":true},"message":"tool use requires approval","approval":{"approvalId":"approval_1","action":"tool_call.execute","resourceKind":"mcp_tool","resourceId":"mcp-test:lookup:chat","reason":"MCP tool execution requires approval","requestedBy":"web-ui","status":"pending","createdAt":"2026-04-18T12:00:00Z","updatedAt":"2026-04-18T12:00:00Z","sandbox":{"declaration":{"declarationId":"mcp_server:mcp-test:lifecycle.start:tool:chat:lookup","consumerKind":"mcp_server","consumerId":"mcp-test","operationKind":"tool_call.execute","profileId":"subprocess_default","executionMode":"subprocess","allowedBackendKinds":["subprocess"],"networkMode":"deny","secretRefs":["MCP_TEST_TOKEN"],"approvalMode":"ask","requiredEnforcementStrength":"declared_only","active":true,"source":"builtin"},"policyRecord":{"policyRecordId":"policy_mcp_mcp-test_tool_call_execute_1","consumerKind":"mcp_server","consumerId":"mcp-test","operationKind":"tool_call.execute","declarationId":"mcp_server:mcp-test:lifecycle.start:tool:chat:lookup","requestedBy":"web-ui","approvalId":"approval_1","decisionId":"decision_1","decision":"ask","approvalStatus":"pending","secretResolution":"resolved","enforcementStrength":"declared_only","startedAt":"2026-04-18T12:00:00Z","status":"approval_pending"}}},"decision":{"decisionId":"decision_1","action":"tool_call.execute","resourceKind":"mcp_tool","resourceId":"mcp-test:lookup:chat","outcome":"requires_approval","reason":"MCP tool execution requires approval","approvalId":"approval_1","createdAt":"2026-04-18T12:00:00Z","sandbox":{"declaration":{"declarationId":"mcp_server:mcp-test:lifecycle.start:tool:chat:lookup","consumerKind":"mcp_server","consumerId":"mcp-test","operationKind":"tool_call.execute","profileId":"subprocess_default","executionMode":"subprocess","allowedBackendKinds":["subprocess"],"networkMode":"deny","secretRefs":["MCP_TEST_TOKEN"],"approvalMode":"ask","requiredEnforcementStrength":"declared_only","active":true,"source":"builtin"},"policyRecord":{"policyRecordId":"policy_mcp_mcp-test_tool_call_execute_1","consumerKind":"mcp_server","consumerId":"mcp-test","operationKind":"tool_call.execute","declarationId":"mcp_server:mcp-test:lifecycle.start:tool:chat:lookup","requestedBy":"web-ui","approvalId":"approval_1","decisionId":"decision_1","decision":"ask","approvalStatus":"pending","secretResolution":"resolved","enforcementStrength":"declared_only","startedAt":"2026-04-18T12:00:00Z","status":"approval_pending"}}},"sandbox":{"declaration":{"declarationId":"mcp_server:mcp-test:lifecycle.start:tool:chat:lookup","consumerKind":"mcp_server","consumerId":"mcp-test","operationKind":"tool_call.execute","profileId":"subprocess_default","executionMode":"subprocess","allowedBackendKinds":["subprocess"],"networkMode":"deny","secretRefs":["MCP_TEST_TOKEN"],"approvalMode":"ask","requiredEnforcementStrength":"declared_only","active":true,"source":"builtin"},"policyRecord":{"policyRecordId":"policy_mcp_mcp-test_tool_call_execute_1","consumerKind":"mcp_server","consumerId":"mcp-test","operationKind":"tool_call.execute","declarationId":"mcp_server:mcp-test:lifecycle.start:tool:chat:lookup","requestedBy":"web-ui","approvalId":"approval_1","decisionId":"decision_1","decision":"ask","approvalStatus":"pending","secretResolution":"resolved","enforcementStrength":"declared_only","startedAt":"2026-04-18T12:00:00Z","status":"approval_pending"}}}"##,
        ),
        (
            r##"schemas/api/mcp-catalog-entry.schema.json"##,
            r##"{"id":"filesystem","displayName":"Filesystem","description":"Local project filesystem access.","transportKind":"stdio","sourceKind":"bundled","tags":["local","filesystem"],"immediateUse":false,"prerequisites":[{"kind":"binary","name":"npx","required":true,"description":"Node.js with npx available on PATH"}],"environmentEligibility":["test","prod"],"availabilityStatus":"unavailable","availabilityReason":"default bundled stdio command requires a local command override because sandbox network is denied","installSupport":{"scriptSupported":true,"scriptArgs":["filesystem"]},"defaultInstallSpec":{"displayName":"Filesystem","originKind":"catalog","catalogEntryId":"filesystem","installMethod":"api","environmentScope":"test","enabled":true,"sandboxProfileId":"subprocess_default","declarationId":"mcp_server:filesystem:lifecycle.start","declaration":{"executionMode":"subprocess","allowedBackendKinds":["subprocess"],"networkMode":"deny","approvalMode":"allow","requiredEnforcementStrength":"declared_only","active":true},"transportKind":"stdio","command":"npx","args":["-y","@modelcontextprotocol/server-filesystem","/tmp/kura"],"workingDir":"/tmp/kura","autoRestart":true}}"##,
        ),
        (
            r##"schemas/api/mcp-catalog-list.response.schema.json"##,
            r##"{"items":[{"id":"filesystem","displayName":"Filesystem","description":"Local project filesystem access.","transportKind":"stdio","sourceKind":"bundled","tags":["local","filesystem"],"immediateUse":false,"availabilityStatus":"unavailable","availabilityReason":"default bundled stdio command requires a local command override because sandbox network is denied","installSupport":{"scriptSupported":true,"scriptArgs":["filesystem"]},"defaultInstallSpec":{"displayName":"Filesystem","originKind":"catalog","catalogEntryId":"filesystem","installMethod":"api","environmentScope":"test","enabled":true,"sandboxProfileId":"subprocess_default","declarationId":"mcp_server:filesystem:lifecycle.start","declaration":{"executionMode":"subprocess","allowedBackendKinds":["subprocess"],"networkMode":"deny","approvalMode":"allow","requiredEnforcementStrength":"declared_only","active":true},"transportKind":"stdio","command":"npx","args":["-y","@modelcontextprotocol/server-filesystem","/tmp/kura"],"workingDir":"/tmp/kura","autoRestart":true}}]}"##,
        ),
        (
            r##"schemas/api/mcp-catalog-detail.response.schema.json"##,
            r##"{"id":"filesystem","displayName":"Filesystem","description":"Local project filesystem access.","transportKind":"stdio","sourceKind":"bundled","tags":["local","filesystem"],"immediateUse":false,"availabilityStatus":"unavailable","availabilityReason":"default bundled stdio command requires a local command override because sandbox network is denied","installSupport":{"scriptSupported":true,"scriptArgs":["filesystem"]},"defaultInstallSpec":{"displayName":"Filesystem","originKind":"catalog","catalogEntryId":"filesystem","installMethod":"api","environmentScope":"test","enabled":true,"sandboxProfileId":"subprocess_default","declarationId":"mcp_server:filesystem:lifecycle.start","declaration":{"executionMode":"subprocess","allowedBackendKinds":["subprocess"],"networkMode":"deny","approvalMode":"allow","requiredEnforcementStrength":"declared_only","active":true},"transportKind":"stdio","command":"npx","args":["-y","@modelcontextprotocol/server-filesystem","/tmp/kura"],"workingDir":"/tmp/kura","autoRestart":true}}"##,
        ),
        (
            r##"schemas/api/mcp-catalog-install-result.schema.json"##,
            r##"{"installId":"mcp_install_1","status":"installed","catalogEntryId":"filesystem","serverId":"filesystem-test","availabilityStatus":"ready","auditEventIds":["evt_install_1","evt_install_2"],"server":{"serverId":"filesystem-test","displayName":"Filesystem","source":"api","originKind":"catalog","catalogEntryId":"filesystem","installMethod":"api","environmentScope":"test","catalogManagement":{"sourceKind":"bundled","installedRevision":"sha256:installed","currentRevision":"sha256:installed","driftStatus":"in_sync","installedAt":"2026-04-18T12:00:00Z","lastActionAt":"2026-04-18T12:00:00Z","lastAction":"install","lastActionStatus":"completed","installInputSnapshot":{"serverId":"filesystem-test","displayName":"Filesystem","enabled":true,"sandboxProfileId":"subprocess_default","installMethod":"api"}},"enabled":true,"sandboxProfileId":"subprocess_default","declarationId":"mcp_server:filesystem:lifecycle.start","declaration":{"executionMode":"subprocess","allowedBackendKinds":["subprocess"],"networkMode":"deny","approvalMode":"allow","requiredEnforcementStrength":"declared_only","active":true},"transportKind":"stdio","command":"/tmp/mcp-helper","args":["--stdio"],"workingDir":"/tmp/kura","autoRestart":true,"createdAt":"2026-04-18T12:00:00Z","updatedAt":"2026-04-18T12:00:00Z","state":{"serverId":"filesystem-test","status":"stopped","failureCount":0,"restartCount":0,"updatedAt":"2026-04-18T12:00:00Z"},"transportConfigSummary":"/tmp/mcp-helper --stdio","availabilityStatus":"ready","toolCount":0}}"##,
        ),
        (
            r##"schemas/api/mcp-catalog-lifecycle-result.schema.json"##,
            r##"{"actionId":"mcp_catalog_refresh_1","action":"refresh","status":"completed","serverId":"filesystem-test","catalogEntryId":"filesystem","auditEventIds":["evt_maintenance_1","evt_maintenance_2"],"server":{"serverId":"filesystem-test","displayName":"Filesystem","source":"api","originKind":"catalog","catalogEntryId":"filesystem","installMethod":"api","environmentScope":"test","catalogManagement":{"sourceKind":"bundled","installedRevision":"sha256:installed","currentRevision":"sha256:installed","driftStatus":"in_sync","installedAt":"2026-04-18T12:00:00Z","lastMaintainedAt":"2026-04-18T12:05:00Z","lastActionAt":"2026-04-18T12:05:00Z","lastAction":"refresh","lastActionStatus":"completed","installInputSnapshot":{"serverId":"filesystem-test","displayName":"Filesystem","enabled":true,"sandboxProfileId":"subprocess_default","installMethod":"api"}},"enabled":true,"sandboxProfileId":"subprocess_default","declarationId":"mcp_server:filesystem:lifecycle.start","declaration":{"executionMode":"subprocess","allowedBackendKinds":["subprocess"],"networkMode":"deny","approvalMode":"allow","requiredEnforcementStrength":"declared_only","active":true},"transportKind":"stdio","command":"/tmp/mcp-helper","args":["--stdio"],"workingDir":"/tmp/kura","autoRestart":true,"createdAt":"2026-04-18T12:00:00Z","updatedAt":"2026-04-18T12:05:00Z","state":{"serverId":"filesystem-test","status":"stopped","failureCount":0,"restartCount":0,"updatedAt":"2026-04-18T12:05:00Z"},"availabilityStatus":"ready","toolCount":0},"preflightMs":42}"##,
        ),
        (
            r##"schemas/api/mcp-catalog-revalidation-result.schema.json"##,
            r##"{"actionId":"mcp_revalidate_1","action":"revalidate","serverId":"filesystem-test","catalogEntryId":"filesystem","status":"blocked","classification":"prerequisite_lost","reason":"MCP_TEST_TOKEN is required","issues":[{"kind":"secret","name":"MCP_TEST_TOKEN","status":"blocked","reason":"MCP_TEST_TOKEN is required","environmentScope":"test"}],"auditEventIds":["evt_revalidate_1","evt_revalidate_2"],"server":{"serverId":"filesystem-test","displayName":"Filesystem","source":"api","originKind":"catalog","catalogEntryId":"filesystem","installMethod":"api","environmentScope":"test","catalogManagement":{"sourceKind":"bundled","installedRevision":"sha256:installed","currentRevision":"sha256:installed","driftStatus":"in_sync","installedAt":"2026-04-18T12:00:00Z","lastActionAt":"2026-04-18T12:06:00Z","lastAction":"revalidate","lastActionStatus":"completed","lastActionReason":"MCP_TEST_TOKEN is required","installInputSnapshot":{"serverId":"filesystem-test","displayName":"Filesystem","enabled":true,"sandboxProfileId":"subprocess_default","secretRefs":["MCP_TEST_TOKEN"],"installMethod":"api"},"lastRevalidation":{"checkedAt":"2026-04-18T12:06:00Z","status":"blocked","classification":"prerequisite_lost","reason":"MCP_TEST_TOKEN is required","issues":[{"kind":"secret","name":"MCP_TEST_TOKEN","status":"blocked","reason":"MCP_TEST_TOKEN is required","environmentScope":"test"}]}},"enabled":true,"sandboxProfileId":"subprocess_default","declarationId":"mcp_server:filesystem:lifecycle.start","declaration":{"executionMode":"subprocess","allowedBackendKinds":["subprocess"],"networkMode":"deny","approvalMode":"allow","requiredEnforcementStrength":"declared_only","active":true},"transportKind":"stdio","command":"/tmp/mcp-helper","args":["--stdio"],"workingDir":"/tmp/kura","secretRefs":["MCP_TEST_TOKEN"],"autoRestart":true,"createdAt":"2026-04-18T12:00:00Z","updatedAt":"2026-04-18T12:00:00Z","state":{"serverId":"filesystem-test","status":"stopped","failureCount":0,"restartCount":0,"updatedAt":"2026-04-18T12:00:00Z"},"availabilityStatus":"blocked","availabilityReason":"MCP_TEST_TOKEN is required","toolCount":0},"preflightMs":25}"##,
        ),
        (
            r##"schemas/api/tool-call-resource.schema.json"##,
            r##"{"toolCallId":"tool_call_mcp_1","runId":"run_1","stepId":"step_1","invocationKind":"mcp_tool","mcpServerId":"filesystem-test","mcpServerName":"Filesystem","mcpToolName":"lookup","mcpTransportKind":"stdio","mcpSessionId":"session_1","authorizationResult":"allowed","toolName":"lookup","status":"completed","createdAt":"2026-04-18T12:00:00Z","updatedAt":"2026-04-18T12:00:01Z","output":{"result":{"content":[{"type":"text","text":"[REDACTED]"}]}},"sandbox":{"declaration":{"declarationId":"mcp_server:filesystem-test:lifecycle.start:tool:chat:lookup","consumerKind":"mcp_server","consumerId":"filesystem-test","operationKind":"tool_call.execute","profileId":"subprocess_default","executionMode":"subprocess","allowedBackendKinds":["subprocess"],"networkMode":"deny","secretRefs":["MCP_TEST_TOKEN"],"approvalMode":"allow","requiredEnforcementStrength":"declared_only","active":true,"source":"builtin"},"policyRecord":{"policyRecordId":"policy_mcp_1","consumerKind":"mcp_server","consumerId":"filesystem-test","operationKind":"tool_call.execute","declarationId":"mcp_server:filesystem-test:lifecycle.start:tool:chat:lookup","requestedBy":"web-ui","decision":"allow","approvalStatus":"approved","secretResolution":"resolved","enforcementStrength":"declared_only","toolCallId":"tool_call_mcp_1","startedAt":"2026-04-18T12:00:00Z","completedAt":"2026-04-18T12:00:01Z","status":"completed"}}}"##,
        ),
        (
            r##"schemas/events/mcp-server-registered.event.schema.json"##,
            r##"{"eventId":"evt_mcp_1","sequence":20,"category":"mcp","name":"mcp.server_registered","occurredAt":"2026-04-18T12:00:01Z","scope":{},"resource":{"kind":"mcp_server","id":"mcp-test"},"payload":{"serverId":"mcp-test","displayName":"MCP Test","originKind":"catalog","catalogEntryId":"filesystem","installMethod":"api","enabled":true,"sandboxProfileId":"subprocess_default","declarationId":"mcp_server:mcp-test:lifecycle.start","transportKind":"stdio","availabilityStatus":"ready","availabilityReason":"","catalogManagement":{"sourceKind":"bundled","installedRevision":"sha256:installed","currentRevision":"sha256:installed","driftStatus":"in_sync"},"created":true}}"##,
        ),
        (
            r##"schemas/events/mcp-server-updated.event.schema.json"##,
            r##"{"eventId":"evt_mcp_2","sequence":21,"category":"mcp","name":"mcp.server_updated","occurredAt":"2026-04-18T12:00:02Z","scope":{},"resource":{"kind":"mcp_server","id":"mcp-test"},"payload":{"serverId":"mcp-test","displayName":"MCP Test","originKind":"catalog","catalogEntryId":"filesystem","installMethod":"api","enabled":false,"sandboxProfileId":"subprocess_default","declarationId":"mcp_server:mcp-test:lifecycle.start","transportKind":"stdio","availabilityStatus":"unavailable","availabilityReason":"server is not healthy","catalogManagement":{"sourceKind":"bundled","installedRevision":"sha256:installed","currentRevision":"sha256:current","driftStatus":"catalog_updated","driftReason":"installed server no longer matches the current catalog revision"},"created":false}}"##,
        ),
        (
            r##"schemas/events/mcp-server-started.event.schema.json"##,
            r##"{"eventId":"evt_mcp_3","sequence":22,"category":"mcp","name":"mcp.server_started","occurredAt":"2026-04-18T12:00:03Z","scope":{},"resource":{"kind":"mcp_server","id":"mcp-test"},"payload":{"serverId":"mcp-test","status":"healthy","executionId":"sandbox_exec_1","toolCount":1,"transportKind":"stdio"}}"##,
        ),
        (
            r##"schemas/events/mcp-server-stopped.event.schema.json"##,
            r##"{"eventId":"evt_mcp_4","sequence":23,"category":"mcp","name":"mcp.server_stopped","occurredAt":"2026-04-18T12:00:04Z","scope":{},"resource":{"kind":"mcp_server","id":"mcp-test"},"payload":{"serverId":"mcp-test","status":"stopping","executionId":"sandbox_exec_1","cancelled":false}}"##,
        ),
        (
            r##"schemas/events/mcp-server-failed.event.schema.json"##,
            r##"{"eventId":"evt_mcp_5","sequence":24,"category":"mcp","name":"mcp.server_failed","occurredAt":"2026-04-18T12:00:05Z","scope":{},"resource":{"kind":"mcp_server","id":"mcp-test"},"payload":{"serverId":"mcp-test","status":"failed","reason":"transport failed","failureClass":"transport_runtime_failure"}}"##,
        ),
        (
            r##"schemas/events/mcp-server-health-changed.event.schema.json"##,
            r##"{"eventId":"evt_mcp_6","sequence":25,"category":"mcp","name":"mcp.server_health_changed","occurredAt":"2026-04-18T12:00:06Z","scope":{},"resource":{"kind":"mcp_server","id":"mcp-test"},"payload":{"serverId":"mcp-test","status":"degraded","reason":"heartbeat stale","availabilityStatus":"unavailable","availabilityReason":"heartbeat stale","catalogManagement":{"sourceKind":"bundled","installedRevision":"sha256:installed","currentRevision":"sha256:current","driftStatus":"catalog_updated","driftReason":"installed server no longer matches the current catalog revision"}}}"##,
        ),
        (
            r##"schemas/events/mcp-server-reconnect-scheduled.event.schema.json"##,
            r##"{"eventId":"evt_mcp_6a","sequence":25,"category":"mcp","name":"mcp.server_reconnect_scheduled","occurredAt":"2026-04-18T12:00:06Z","scope":{},"resource":{"kind":"mcp_server","id":"mcp-test"},"payload":{"serverId":"mcp-test","transportKind":"websocket","attempt":1,"reason":"websocket disconnected","nextRetryAt":"2026-04-18T12:00:11Z"}}"##,
        ),
        (
            r##"schemas/events/mcp-server-reconnect-completed.event.schema.json"##,
            r##"{"eventId":"evt_mcp_6b","sequence":26,"category":"mcp","name":"mcp.server_reconnect_completed","occurredAt":"2026-04-18T12:00:07Z","scope":{},"resource":{"kind":"mcp_server","id":"mcp-test"},"payload":{"serverId":"mcp-test","transportKind":"websocket","attempt":1,"sessionId":"session_ws_1"}}"##,
        ),
        (
            r##"schemas/events/mcp-server-reconnect-failed.event.schema.json"##,
            r##"{"eventId":"evt_mcp_6c","sequence":27,"category":"mcp","name":"mcp.server_reconnect_failed","occurredAt":"2026-04-18T12:00:08Z","scope":{},"resource":{"kind":"mcp_server","id":"mcp-test"},"payload":{"serverId":"mcp-test","transportKind":"websocket","attempt":3,"reason":"reconnect exhausted","failureClass":"reconnect_exhausted"}}"##,
        ),
        (
            r##"schemas/events/mcp-server-restore-completed.event.schema.json"##,
            r##"{"eventId":"evt_mcp_6d","sequence":28,"category":"mcp","name":"mcp.server_restore_completed","occurredAt":"2026-04-18T12:00:09Z","scope":{},"resource":{"kind":"mcp_server","id":"mcp-test"},"payload":{"serverId":"mcp-test","transportKind":"websocket","sessionId":"session_ws_2","toolCount":1}}"##,
        ),
        (
            r##"schemas/events/mcp-server-restore-failed.event.schema.json"##,
            r##"{"eventId":"evt_mcp_6e","sequence":29,"category":"mcp","name":"mcp.server_restore_failed","occurredAt":"2026-04-18T12:00:10Z","scope":{},"resource":{"kind":"mcp_server","id":"mcp-test"},"payload":{"serverId":"mcp-test","transportKind":"websocket","reason":"dial failed","failureClass":"transport_runtime_failure"}}"##,
        ),
        (
            r##"schemas/events/mcp-catalog-lifecycle-requested.event.schema.json"##,
            r##"{"eventId":"evt_mcp_11","sequence":30,"category":"mcp","name":"mcp.catalog_lifecycle_requested","occurredAt":"2026-04-18T12:00:11Z","scope":{},"resource":{"kind":"mcp_server","id":"filesystem-test"},"payload":{"actionId":"mcp_catalog_refresh_1","action":"refresh","serverId":"filesystem-test","catalogEntryId":"filesystem","environment":"test"}}"##,
        ),
        (
            r##"schemas/events/mcp-catalog-lifecycle-completed.event.schema.json"##,
            r##"{"eventId":"evt_mcp_12","sequence":31,"category":"mcp","name":"mcp.catalog_lifecycle_completed","occurredAt":"2026-04-18T12:00:12Z","scope":{},"resource":{"kind":"mcp_server","id":"filesystem-test"},"payload":{"actionId":"mcp_catalog_refresh_1","action":"refresh","serverId":"filesystem-test","catalogEntryId":"filesystem","status":"completed","removed":false,"environment":"test"}}"##,
        ),
        (
            r##"schemas/events/mcp-catalog-lifecycle-failed.event.schema.json"##,
            r##"{"eventId":"evt_mcp_13","sequence":32,"category":"mcp","name":"mcp.catalog_lifecycle_failed","occurredAt":"2026-04-18T12:00:13Z","scope":{},"resource":{"kind":"mcp_server","id":"filesystem-test"},"payload":{"actionId":"mcp_catalog_refresh_2","action":"refresh","serverId":"filesystem-test","catalogEntryId":"filesystem","status":"blocked","failureClass":"conflict","reason":"server has local operator modifications","environment":"test"}}"##,
        ),
        (
            r##"schemas/events/mcp-catalog-revalidation-completed.event.schema.json"##,
            r##"{"eventId":"evt_mcp_14","sequence":33,"category":"mcp","name":"mcp.catalog_revalidation_completed","occurredAt":"2026-04-18T12:00:14Z","scope":{},"resource":{"kind":"mcp_server","id":"filesystem-test"},"payload":{"actionId":"mcp_revalidate_1","action":"revalidate","serverId":"filesystem-test","catalogEntryId":"filesystem","status":"blocked","classification":"prerequisite_lost","reason":"MCP_TEST_TOKEN is required","issues":[{"kind":"secret","name":"MCP_TEST_TOKEN","status":"blocked","reason":"MCP_TEST_TOKEN is required","environmentScope":"test"}],"environment":"test"}}"##,
        ),
        (
            r##"schemas/events/mcp-tool-exposure-updated.event.schema.json"##,
            r##"{"eventId":"evt_mcp_7","sequence":26,"category":"mcp","name":"mcp.tool_exposure_updated","occurredAt":"2026-04-18T12:00:07Z","scope":{},"resource":{"kind":"mcp_tool","id":"mcp-test:lookup"},"payload":{"serverId":"mcp-test","toolName":"lookup","runtimeSurface":"chat","exposureMode":"approval_required","active":true,"reason":"needs approval"}}"##,
        ),
        (
            r##"schemas/events/mcp-catalog-install-requested.event.schema.json"##,
            r##"{"eventId":"evt_mcp_8","sequence":27,"category":"mcp","name":"mcp.catalog_install_requested","occurredAt":"2026-04-18T12:00:08Z","scope":{},"resource":{"kind":"mcp_catalog_install","id":"mcp_install_1"},"payload":{"installId":"mcp_install_1","catalogEntryId":"filesystem","method":"api","environment":"test"}}"##,
        ),
        (
            r##"schemas/events/mcp-catalog-install-completed.event.schema.json"##,
            r##"{"eventId":"evt_mcp_9","sequence":28,"category":"mcp","name":"mcp.catalog_install_completed","occurredAt":"2026-04-18T12:00:09Z","scope":{},"resource":{"kind":"mcp_catalog_install","id":"mcp_install_1"},"payload":{"installId":"mcp_install_1","catalogEntryId":"filesystem","serverId":"filesystem-test","method":"api","status":"installed","availabilityStatus":"ready"}}"##,
        ),
        (
            r##"schemas/events/mcp-catalog-install-failed.event.schema.json"##,
            r##"{"eventId":"evt_mcp_10","sequence":29,"category":"mcp","name":"mcp.catalog_install_failed","occurredAt":"2026-04-18T12:00:10Z","scope":{},"resource":{"kind":"mcp_catalog_install","id":"mcp_install_2"},"payload":{"installId":"mcp_install_2","catalogEntryId":"github","method":"api","status":"blocked","availabilityStatus":"blocked","availabilityReason":"GitHub personal access token"}}"##,
        ),
    ];
    validate_fixtures(&validator, fixtures);
}

#[test]
fn test_skill_schemas_accept_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[
        (
            r##"schemas/api/skill-file.schema.json"##,
            r##"{"path":"assets/guide.md","sizeBytes":42}"##,
        ),
        (
            r##"schemas/api/skill-summary.schema.json"##,
            r##"{"skillId":"exec-skill","name":"exec-skill","description":"executable skill","source":"data_dir","rootPath":"/tmp/kura/skills","skillPath":"/tmp/kura/skills/exec-skill","instructionPath":"/tmp/kura/skills/exec-skill/SKILL.md","files":[{"path":"assets/guide.md","sizeBytes":42}],"frontmatter":{"name":"exec-skill","description":"executable skill"},"executionManifest":{"entrypoint":"/tmp/kura/skills/exec-skill/scripts/run.sh","args":["alpha","beta"],"workingDir":"/tmp/kura/skills/exec-skill","profileId":"subprocess_default","backendKind":"subprocess","readRoots":["/tmp/kura/skills/exec-skill"],"writeRoots":["/tmp/kura/skills/exec-skill"],"networkMode":"deny","secretRefs":["EXEC_SKILL_TOKEN"],"approvalMode":"ask","timeoutMs":1000,"requiredEnforcementStrength":"declared_only"},"availabilityStatus":"available","sandbox":{"declaration":{"declarationId":"skill:exec-skill:tool_call.execute","consumerKind":"skill","consumerId":"exec-skill","operationKind":"tool_call.execute","profileId":"subprocess_default","executionMode":"subprocess","allowedBackendKinds":["subprocess"],"readRoots":["/tmp/kura/skills/exec-skill"],"writeRoots":["/tmp/kura/skills/exec-skill"],"networkMode":"deny","secretRefs":["EXEC_SKILL_TOKEN"],"approvalMode":"ask","requiredEnforcementStrength":"declared_only","active":true,"source":"builtin"},"secretScope":[{"consumerKind":"skill","consumerId":"exec-skill","secretRef":"EXEC_SKILL_TOKEN","environmentScope":"test","defaultSource":"kind_default","defaultRuleId":"skill:exec-skill","deliveryKind":"environment_variable","redactionRule":"value_redacted","resolution":"resolved"}]}}"##,
        ),
        (
            r##"schemas/api/skill-detail.response.schema.json"##,
            r##"{"skillId":"broken-skill","name":"broken-skill","description":"invalid executable skill","source":"data_dir","rootPath":"/tmp/kura/skills","skillPath":"/tmp/kura/skills/broken-skill","instructionPath":"/tmp/kura/skills/broken-skill/SKILL.md","files":[{"path":"assets/guide.md","sizeBytes":42}],"frontmatter":{"name":"broken-skill","description":"invalid executable skill"},"frontmatterRaw":"name: broken-skill","body":"data instructions","executionManifest":{"entrypoint":"/tmp/kura/skills/broken-skill/scripts/run.sh","profileId":"subprocess_default","backendKind":"subprocess","approvalMode":"ask"},"availabilityStatus":"unavailable","availabilityReason":"executable skill secret ref EXEC_SKILL_TOKEN is unavailable for test environment"}"##,
        ),
        (
            r##"schemas/api/skill-overlay.schema.json"##,
            r##"{"overlayId":"data_dir_agents","source":"data_dir","path":"/tmp/kura/AGENTS.md","sizeBytes":12,"modifiedAt":"2026-04-18T12:00:00Z"}"##,
        ),
        (
            r##"schemas/api/skill-registry.response.schema.json"##,
            r##"{"loadedAt":"2026-04-18T12:00:00Z","items":[{"skillId":"exec-skill","name":"exec-skill","description":"executable skill","source":"data_dir","rootPath":"/tmp/kura/skills","skillPath":"/tmp/kura/skills/exec-skill","instructionPath":"/tmp/kura/skills/exec-skill/SKILL.md","files":[{"path":"assets/guide.md","sizeBytes":42}],"frontmatter":{"name":"exec-skill","description":"executable skill"},"executionManifest":{"entrypoint":"/tmp/kura/skills/exec-skill/scripts/run.sh","profileId":"subprocess_default","backendKind":"subprocess","approvalMode":"ask"},"availabilityStatus":"available"},{"skillId":"broken-skill","name":"broken-skill","description":"invalid executable skill","source":"data_dir","rootPath":"/tmp/kura/skills","skillPath":"/tmp/kura/skills/broken-skill","instructionPath":"/tmp/kura/skills/broken-skill/SKILL.md","files":[{"path":"assets/guide.md","sizeBytes":42}],"frontmatter":{"name":"broken-skill","description":"invalid executable skill"},"executionManifest":{"entrypoint":"/tmp/kura/skills/broken-skill/scripts/run.sh","profileId":"subprocess_default","backendKind":"subprocess","approvalMode":"ask"},"availabilityStatus":"unavailable","availabilityReason":"executable skill secret ref EXEC_SKILL_TOKEN is unavailable for test environment"}],"overlays":[{"overlayId":"home_agents","source":"home","path":"/tmp/home/.agents/AGENTS.md","sizeBytes":11,"modifiedAt":"2026-04-18T12:00:00Z"}]}"##,
        ),
        (
            r##"schemas/api/sandbox-backend-capability.schema.json"##,
            r##"{"backendKind":"docker","displayName":"Docker","filesystemEnforcement":"container_mount_scoped","networkEnforcement":"container_network_mode","envInjectionMode":"container_env_injection","approvalBehavior":"profile_and_command_policy","restartBehavior":"interrupted_execution_recovers_as_cancelled","hostPrerequisites":["docker CLI available on PATH"],"availabilityStatus":"unavailable","availabilityReason":"docker CLI is not available on PATH"}"##,
        ),
        (
            r##"schemas/api/sandbox-profile.schema.json"##,
            r##"{"profileId":"subprocess_default","title":"Default Subprocess Sandbox","description":"Conservative local subprocess execution for the harness control plane.","backendKind":"subprocess","backendCapability":{"backendKind":"subprocess","displayName":"Subprocess","filesystemEnforcement":"declared_scoped","networkEnforcement":"declared_only","envInjectionMode":"filtered_host_env","approvalBehavior":"profile_and_command_policy","restartBehavior":"interrupted_execution_recovers_as_cancelled","availabilityStatus":"available"},"defaultWorkDir":"/tmp/kura-data","filesystemPolicy":{"mode":"scoped","readRoots":["/tmp/kura-data"],"writeRoots":["/tmp/kura-data"],"tempRoots":["/tmp"],"allowDataDir":true,"allowUserAgentsDir":true,"allowHomeRead":false,"allowHomeWrite":false},"networkPolicy":{"mode":"deny","allowedHosts":[],"allowedPorts":[],"allowLoopback":false,"enforcementMode":"declared_only"},"envPolicy":{"mode":"inherit_safe","allowedVars":["PATH"],"injectedVars":{"KURA_DATA_DIR":"/tmp/kura-data"},"redactedVars":[]},"approvalPolicy":{"mode":"ask","requiredForCommands":["curl"],"requiredForWritesOutsideRoots":true,"requiredForNetwork":true,"requiredForUnknownBackends":true},"processPolicy":{"timeoutMs":30000,"maxTimeoutMs":300000,"killGraceMs":1000,"captureStdout":true,"captureStderr":true,"maxOutputBytes":65536,"allowStreaming":false,"restartOnFailure":false},"defaultTimeoutMs":30000,"maxTimeoutMs":300000,"restartable":false,"source":"builtin","active":true}"##,
        ),
        (
            r##"schemas/api/sandbox-profile-list.response.schema.json"##,
            r##"{"items":[{"profileId":"subprocess_default","title":"Default Subprocess Sandbox","description":"Conservative local subprocess execution for the harness control plane.","backendKind":"subprocess","backendCapability":{"backendKind":"subprocess","displayName":"Subprocess","filesystemEnforcement":"declared_scoped","networkEnforcement":"declared_only","envInjectionMode":"filtered_host_env","approvalBehavior":"profile_and_command_policy","restartBehavior":"interrupted_execution_recovers_as_cancelled","availabilityStatus":"available"},"defaultWorkDir":"/tmp/kura-data","filesystemPolicy":{"mode":"scoped","readRoots":["/tmp/kura-data"],"writeRoots":["/tmp/kura-data"],"tempRoots":["/tmp"],"allowDataDir":true,"allowUserAgentsDir":true,"allowHomeRead":false,"allowHomeWrite":false},"networkPolicy":{"mode":"deny","allowedHosts":[],"allowedPorts":[],"allowLoopback":false,"enforcementMode":"declared_only"},"envPolicy":{"mode":"inherit_safe","allowedVars":["PATH"],"injectedVars":{"KURA_DATA_DIR":"/tmp/kura-data"},"redactedVars":[]},"approvalPolicy":{"mode":"ask","requiredForCommands":["curl"],"requiredForWritesOutsideRoots":true,"requiredForNetwork":true,"requiredForUnknownBackends":true},"processPolicy":{"timeoutMs":30000,"maxTimeoutMs":300000,"killGraceMs":1000,"captureStdout":true,"captureStderr":true,"maxOutputBytes":65536,"allowStreaming":false,"restartOnFailure":false},"defaultTimeoutMs":30000,"maxTimeoutMs":300000,"restartable":false,"source":"builtin","active":true}]}"##,
        ),
        (
            r##"schemas/api/sandbox-decision.schema.json"##,
            r##"{"decisionId":"sandbox_decision_1","executionId":"sandbox_exec_1","resolution":"ask","selectionOutcome":"selected","matchedRules":["profile:subprocess_default","network:approval_required"],"approvalRequired":true,"approvalStatus":"pending","effectiveProfileId":"subprocess_default","effectiveBackendKind":"subprocess","requiredBackendKind":"subprocess","hostStatus":"ready","explanation":"sandbox execution requires approval"}"##,
        ),
        (
            r##"schemas/api/sandbox-result.schema.json"##,
            r##"{"executionId":"sandbox_exec_1","status":"denied","outputTruncated":false,"partial":false,"errorClass":"approval_required","errorCode":"sandbox_approval_required","error":"sandbox execution requires approval","backendMetadata":{"managedProviderId":"codex_managed","managedProviderAction":"prompt_execution","managedProviderOperationId":"managed_provider_op_1","enforcementStrength":"declared_only","sensitiveStateClasses":["config_file","temp_output"]}}"##,
        ),
        (
            r##"schemas/api/sandbox-execution.resource.schema.json"##,
            r##"{"executionId":"sandbox_exec_1","profileId":"subprocess_default","backendKind":"subprocess","command":"echo","args":["hello"],"cwd":"/tmp/kura","envKeys":["HOME"],"stdinProvided":false,"timeoutMs":1000,"requestedBy":"web-ui","resourceKind":"skill","resourceId":"shared","scope":"chat","approvalId":"approval_1","reason":"inspect profile","metadata":{"ticket":"sandbox-16","managedProviderId":"codex_managed","managedProviderAction":"prompt_execution","managedProviderOperationId":"managed_provider_op_1","sandboxProfileId":"managed_provider_codex","sandboxDecision":"ask","enforcementStrength":"declared_only","sensitiveStateClasses":"config_file,temp_output"},"access":{"readRoots":["/tmp/kura"],"writeRoots":["/tmp/kura"],"networkMode":"allow_list","allowedHosts":["localhost"],"allowedPorts":[80],"allowLoopback":true},"status":"denied","decision":{"decisionId":"sandbox_decision_1","executionId":"sandbox_exec_1","resolution":"ask","selectionOutcome":"selected","matchedRules":["profile:subprocess_default","network:approval_required"],"approvalRequired":true,"approvalStatus":"pending","effectiveProfileId":"subprocess_default","effectiveBackendKind":"subprocess","requiredBackendKind":"subprocess","hostStatus":"ready","explanation":"sandbox execution requires approval"},"result":{"executionId":"sandbox_exec_1","status":"denied","outputTruncated":false,"partial":false,"errorClass":"approval_required","errorCode":"sandbox_approval_required","error":"sandbox execution requires approval","backendMetadata":{"managedProviderId":"codex_managed","managedProviderAction":"prompt_execution","managedProviderOperationId":"managed_provider_op_1","enforcementStrength":"declared_only","sensitiveStateClasses":["config_file","temp_output"]}},"requestedAt":"2026-04-18T12:00:00Z","updatedAt":"2026-04-18T12:00:00Z"}"##,
        ),
        (
            r##"schemas/api/sandbox-execution-list.response.schema.json"##,
            r##"{"items":[{"executionId":"sandbox_exec_1","profileId":"subprocess_default","backendKind":"subprocess","command":"echo","args":["hello"],"cwd":"/tmp/kura","envKeys":["HOME"],"stdinProvided":false,"timeoutMs":1000,"access":{"readRoots":["/tmp/kura"],"writeRoots":["/tmp/kura"],"networkMode":"allow_list","allowedHosts":["localhost"],"allowedPorts":[80],"allowLoopback":true},"status":"denied","decision":{"decisionId":"sandbox_decision_1","executionId":"sandbox_exec_1","resolution":"ask","selectionOutcome":"selected","matchedRules":["profile:subprocess_default","network:approval_required"],"approvalRequired":true,"approvalStatus":"pending","effectiveProfileId":"subprocess_default","effectiveBackendKind":"subprocess","requiredBackendKind":"subprocess","hostStatus":"ready","explanation":"sandbox execution requires approval"},"result":{"executionId":"sandbox_exec_1","status":"denied","outputTruncated":false,"partial":false,"errorClass":"approval_required","errorCode":"sandbox_approval_required","error":"sandbox execution requires approval"},"requestedAt":"2026-04-18T12:00:00Z","updatedAt":"2026-04-18T12:00:00Z"}]}"##,
        ),
        (
            r##"schemas/api/sandbox-explain.response.schema.json"##,
            r##"{"decision":{"decisionId":"sandbox_decision_1","resolution":"ask","selectionOutcome":"selected","matchedRules":["profile:subprocess_default","network:approval_required"],"approvalRequired":true,"approvalStatus":"pending","effectiveProfileId":"subprocess_default","effectiveBackendKind":"subprocess","requiredBackendKind":"subprocess","hostStatus":"ready","explanation":"sandbox execution requires approval"}}"##,
        ),
    ];
    validate_fixtures(&validator, fixtures);
}

#[test]
fn test_skill_backed_execution_schemas_accept_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[
        (
            r##"schemas/api/tool-call-resource.schema.json"##,
            r##"{"toolCallId":"tool_call_skill_1","runId":"run_1","stepId":"step_1","invocationKind":"skill","skillId":"exec-skill","toolName":"exec-skill","status":"completed","sandboxExecutionId":"sandbox_exec_skill_1","createdAt":"2026-04-18T12:00:00Z","updatedAt":"2026-04-18T12:00:02Z","output":{"stdout":"[REDACTED]"},"sandbox":{"declaration":{"declarationId":"skill:exec-skill:tool_call.execute","consumerKind":"skill","consumerId":"exec-skill","operationKind":"tool_call.execute","profileId":"subprocess_default","executionMode":"subprocess","allowedBackendKinds":["subprocess"],"readRoots":["/tmp/kura/skills/exec-skill"],"writeRoots":["/tmp/kura/skills/exec-skill"],"networkMode":"deny","secretRefs":["EXEC_SKILL_TOKEN"],"approvalMode":"ask","requiredEnforcementStrength":"declared_only","active":true,"source":"builtin"},"secretScope":[{"consumerKind":"skill","consumerId":"exec-skill","secretRef":"EXEC_SKILL_TOKEN","environmentScope":"test","defaultSource":"kind_default","defaultRuleId":"skill:exec-skill","deliveryKind":"environment_variable","redactionRule":"value_redacted","resolution":"resolved"}],"policyRecord":{"policyRecordId":"policy_skill_1","consumerKind":"skill","consumerId":"exec-skill","operationKind":"tool_call.execute","declarationId":"skill:exec-skill:tool_call.execute","requestedBy":"web-ui","decision":"allow","approvalStatus":"approved","secretResolution":"resolved","enforcementStrength":"declared_only","sandboxExecutionId":"sandbox_exec_skill_1","toolCallId":"tool_call_skill_1","startedAt":"2026-04-18T12:00:00Z","completedAt":"2026-04-18T12:00:02Z","status":"completed"}}}"##,
        ),
        (
            r##"schemas/api/tool-call-list.response.schema.json"##,
            r##"{"items":[{"toolCallId":"tool_call_skill_1","runId":"run_1","stepId":"step_1","invocationKind":"skill","skillId":"exec-skill","toolName":"exec-skill","status":"completed","sandboxExecutionId":"sandbox_exec_skill_1","createdAt":"2026-04-18T12:00:00Z","updatedAt":"2026-04-18T12:00:02Z","sandbox":{"declaration":{"declarationId":"skill:exec-skill:tool_call.execute","consumerKind":"skill","consumerId":"exec-skill","operationKind":"tool_call.execute","profileId":"subprocess_default","executionMode":"subprocess","allowedBackendKinds":["subprocess"],"readRoots":["/tmp/kura/skills/exec-skill"],"writeRoots":["/tmp/kura/skills/exec-skill"],"networkMode":"deny","secretRefs":["EXEC_SKILL_TOKEN"],"approvalMode":"ask","requiredEnforcementStrength":"declared_only","active":true,"source":"builtin"},"policyRecord":{"policyRecordId":"policy_skill_1","consumerKind":"skill","consumerId":"exec-skill","operationKind":"tool_call.execute","declarationId":"skill:exec-skill:tool_call.execute","requestedBy":"web-ui","decision":"allow","approvalStatus":"approved","secretResolution":"resolved","enforcementStrength":"declared_only","sandboxExecutionId":"sandbox_exec_skill_1","toolCallId":"tool_call_skill_1","startedAt":"2026-04-18T12:00:00Z","completedAt":"2026-04-18T12:00:02Z","status":"completed"}}}]}"##,
        ),
        (
            r##"schemas/api/approval-resource.schema.json"##,
            r##"{"approvalId":"approval_skill_1","action":"tool_call.execute","resourceKind":"skill","resourceId":"exec-skill","reason":"needs approval","status":"pending","createdAt":"2026-04-18T12:00:00Z","updatedAt":"2026-04-18T12:00:00Z","sandbox":{"declaration":{"declarationId":"skill:exec-skill:tool_call.execute","consumerKind":"skill","consumerId":"exec-skill","operationKind":"tool_call.execute","profileId":"subprocess_default","executionMode":"subprocess","allowedBackendKinds":["subprocess"],"readRoots":["/tmp/kura/skills/exec-skill"],"writeRoots":["/tmp/kura/skills/exec-skill"],"networkMode":"deny","secretRefs":["EXEC_SKILL_TOKEN"],"approvalMode":"ask","requiredEnforcementStrength":"declared_only","active":true,"source":"builtin"},"secretScope":[{"consumerKind":"skill","consumerId":"exec-skill","secretRef":"EXEC_SKILL_TOKEN","environmentScope":"test","defaultSource":"kind_default","defaultRuleId":"skill:exec-skill","deliveryKind":"environment_variable","redactionRule":"value_redacted","resolution":"resolved"}],"policyRecord":{"policyRecordId":"policy_skill_approval_1","consumerKind":"skill","consumerId":"exec-skill","operationKind":"tool_call.execute","declarationId":"skill:exec-skill:tool_call.execute","requestedBy":"web-ui","approvalId":"approval_skill_1","decisionId":"decision_skill_1","decision":"ask","approvalStatus":"pending","secretResolution":"resolved","enforcementStrength":"declared_only","startedAt":"2026-04-18T12:00:00Z","status":"approval_pending"}}}"##,
        ),
        (
            r##"schemas/api/decision-resource.schema.json"##,
            r##"{"decisionId":"decision_skill_1","action":"tool_call.execute","resourceKind":"skill","resourceId":"exec-skill","outcome":"requires_approval","reason":"needs approval","approvalId":"approval_skill_1","createdAt":"2026-04-18T12:00:00Z","sandbox":{"declaration":{"declarationId":"skill:exec-skill:tool_call.execute","consumerKind":"skill","consumerId":"exec-skill","operationKind":"tool_call.execute","profileId":"subprocess_default","executionMode":"subprocess","allowedBackendKinds":["subprocess"],"readRoots":["/tmp/kura/skills/exec-skill"],"writeRoots":["/tmp/kura/skills/exec-skill"],"networkMode":"deny","secretRefs":["EXEC_SKILL_TOKEN"],"approvalMode":"ask","requiredEnforcementStrength":"declared_only","active":true,"source":"builtin"},"secretScope":[{"consumerKind":"skill","consumerId":"exec-skill","secretRef":"EXEC_SKILL_TOKEN","environmentScope":"test","defaultSource":"kind_default","defaultRuleId":"skill:exec-skill","deliveryKind":"environment_variable","redactionRule":"value_redacted","resolution":"resolved"}],"policyRecord":{"policyRecordId":"policy_skill_approval_1","consumerKind":"skill","consumerId":"exec-skill","operationKind":"tool_call.execute","declarationId":"skill:exec-skill:tool_call.execute","requestedBy":"web-ui","approvalId":"approval_skill_1","decisionId":"decision_skill_1","decision":"ask","approvalStatus":"pending","secretResolution":"resolved","enforcementStrength":"declared_only","startedAt":"2026-04-18T12:00:00Z","status":"approval_pending"}}}"##,
        ),
        (
            r##"schemas/api/sandbox-execution.resource.schema.json"##,
            r##"{"executionId":"sandbox_exec_skill_1","profileId":"subprocess_default","backendKind":"subprocess","command":"/tmp/kura/skills/exec-skill/scripts/run.sh","args":["alpha"],"cwd":"/tmp/kura/skills/exec-skill","envKeys":["EXEC_SKILL_TOKEN"],"stdinProvided":false,"timeoutMs":1000,"requestedBy":"web-ui","resourceKind":"skill","resourceId":"exec-skill","scope":"tool_call","approvalId":"approval_skill_1","access":{"readRoots":["/tmp/kura/skills/exec-skill"],"writeRoots":["/tmp/kura/skills/exec-skill"],"networkMode":"deny","allowedHosts":[],"allowedPorts":[]},"status":"completed","decision":{"decisionId":"sandbox_decision_skill_1","executionId":"sandbox_exec_skill_1","resolution":"allow","selectionOutcome":"selected","matchedRules":["profile:subprocess_default"],"approvalRequired":false,"approvalStatus":"approved","effectiveProfileId":"subprocess_default","effectiveBackendKind":"subprocess","requiredBackendKind":"subprocess","hostStatus":"ready","explanation":"sandbox execution allowed"},"result":{"executionId":"sandbox_exec_skill_1","status":"completed","stdout":"[REDACTED]","stderr":"","exitCode":0,"outputTruncated":false,"partial":false},"requestedAt":"2026-04-18T12:00:00Z","updatedAt":"2026-04-18T12:00:02Z","startedAt":"2026-04-18T12:00:00Z","completedAt":"2026-04-18T12:00:02Z","consumer":{"declaration":{"declarationId":"skill:exec-skill:tool_call.execute","consumerKind":"skill","consumerId":"exec-skill","operationKind":"tool_call.execute","profileId":"subprocess_default","executionMode":"subprocess","allowedBackendKinds":["subprocess"],"readRoots":["/tmp/kura/skills/exec-skill"],"writeRoots":["/tmp/kura/skills/exec-skill"],"networkMode":"deny","secretRefs":["EXEC_SKILL_TOKEN"],"approvalMode":"ask","requiredEnforcementStrength":"declared_only","active":true,"source":"builtin"},"secretScope":[{"consumerKind":"skill","consumerId":"exec-skill","secretRef":"EXEC_SKILL_TOKEN","environmentScope":"test","defaultSource":"kind_default","defaultRuleId":"skill:exec-skill","deliveryKind":"environment_variable","redactionRule":"value_redacted","resolution":"resolved"}],"policyRecord":{"policyRecordId":"policy_skill_1","consumerKind":"skill","consumerId":"exec-skill","operationKind":"tool_call.execute","declarationId":"skill:exec-skill:tool_call.execute","requestedBy":"web-ui","approvalId":"approval_skill_1","decisionId":"decision_skill_2","decision":"allow","approvalStatus":"approved","secretResolution":"resolved","enforcementStrength":"declared_only","sandboxExecutionId":"sandbox_exec_skill_1","toolCallId":"tool_call_skill_1","startedAt":"2026-04-18T12:00:00Z","completedAt":"2026-04-18T12:00:02Z","status":"completed"}}}"##,
        ),
    ];
    validate_fixtures(&validator, fixtures);
}

#[test]
fn test_sandbox_result_schema_accepts_managed_provider_failure_class() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[(
        r##"schemas/api/sandbox-result.schema.json"##,
        r##"{"executionId":"sandbox_exec_provider_1","status":"failed","outputTruncated":false,"partial":false,"errorClass":"provider_auth_failed","errorCode":"upstream_auth_failed","error":"not logged in","backendMetadata":{"managedProviderId":"claude_managed","managedProviderAction":"prompt_execution","managedProviderOperationId":"managed_provider_op_1","enforcementStrength":"declared_only","sensitiveStateClasses":["settings_file"]}}"##,
    )];
    validate_fixtures(&validator, fixtures);
}

#[test]
fn test_streaming_timeout_schemas_accept_canonical_fixtures() {
    let validator = Validator::new(schema_root_dir());
    let fixtures: &[Fixture] = &[
        (
            r##"schemas/events/llm-dispatch-partial-failed.event.schema.json"##,
            r##"{"eventId":"evt_partial_1","sequence":8,"category":"llm","name":"llm.dispatch.partial_failed","occurredAt":"2026-04-18T12:00:01Z","scope":{},"resource":{"kind":"llm_dispatch","id":"dispatch_1"},"payload":{"provider":"openai_compatible","model":"gpt-5.4","status":"partial_failed","partial":true,"attemptCount":1,"finishReason":"","usage":{"inputTokens":1,"outputTokens":2,"totalTokens":3},"errorCode":"idle_timeout","error":"stream stalled","skills":["shared"],"skillContracts":[{"declaration":{"declarationId":"skill:shared:selection","consumerKind":"skill","consumerId":"shared","operationKind":"skill_selection","profileId":"subprocess_default","executionMode":"declaration_only","allowedBackendKinds":["subprocess"],"readRoots":["/tmp/kura/skills/shared"],"writeRoots":[],"networkMode":"deny","secretRefs":[],"approvalMode":"allow","requiredEnforcementStrength":"declared_only","active":true,"source":"builtin"}}]}}"##,
        ),
        (
            r##"schemas/events/connector-reply-partial.event.schema.json"##,
            r##"{"eventId":"evt_partial_2","sequence":9,"category":"connector","name":"connector.reply_partial","occurredAt":"2026-04-18T12:00:01Z","scope":{"runId":"run_1","stepId":"step_1","connectorId":"discord-main"},"resource":{"kind":"connector","id":"discord-main"},"payload":{"messageId":"msg_1","replyMessageId":"reply_1","replyMessageIds":["reply_1"],"partCount":1,"contentLength":128,"error":"stream stalled","errorClass":""}}"##,
        ),
        (
            r##"schemas/events/sandbox-execution-requested.event.schema.json"##,
            r##"{"eventId":"evt_sandbox_1","sequence":10,"category":"sandbox","name":"sandbox.execution_requested","occurredAt":"2026-04-18T12:00:01Z","scope":{},"resource":{"kind":"sandbox_execution","id":"sandbox_exec_1"},"payload":{"profileId":"subprocess_default","backendKind":"subprocess","command":"echo","args":["hello"],"cwd":"/tmp/kura","requestedBy":"web-ui","resourceKind":"skill","resourceId":"shared","scope":"chat","status":"denied"}}"##,
        ),
        (
            r##"schemas/events/sandbox-decision-recorded.event.schema.json"##,
            r##"{"eventId":"evt_sandbox_2","sequence":11,"category":"sandbox","name":"sandbox.decision_recorded","occurredAt":"2026-04-18T12:00:01Z","scope":{},"resource":{"kind":"sandbox_execution","id":"sandbox_exec_1"},"payload":{"decisionId":"sandbox_decision_1","resolution":"ask","selectionOutcome":"selected","matchedRules":["profile:subprocess_default"],"approvalRequired":true,"approvalStatus":"pending","effectiveProfileId":"subprocess_default","effectiveBackendKind":"subprocess","requiredBackendKind":"subprocess","hostStatus":"ready","explanation":"sandbox execution requires approval"}}"##,
        ),
        (
            r##"schemas/events/sandbox-execution-started.event.schema.json"##,
            r##"{"eventId":"evt_sandbox_3","sequence":12,"category":"sandbox","name":"sandbox.execution_started","occurredAt":"2026-04-18T12:00:01Z","scope":{},"resource":{"kind":"sandbox_execution","id":"sandbox_exec_2"},"payload":{"profileId":"subprocess_default","backendKind":"subprocess","status":"running","startedAt":"2026-04-18T12:00:01Z"}}"##,
        ),
        (
            r##"schemas/events/sandbox-execution-completed.event.schema.json"##,
            r##"{"eventId":"evt_sandbox_4","sequence":13,"category":"sandbox","name":"sandbox.execution_completed","occurredAt":"2026-04-18T12:00:02Z","scope":{},"resource":{"kind":"sandbox_execution","id":"sandbox_exec_2"},"payload":{"profileId":"subprocess_default","backendKind":"subprocess","status":"completed","exitCode":0,"completedAt":"2026-04-18T12:00:02Z","outputTruncated":false,"partial":false}}"##,
        ),
        (
            r##"schemas/events/sandbox-execution-failed.event.schema.json"##,
            r##"{"eventId":"evt_sandbox_5","sequence":14,"category":"sandbox","name":"sandbox.execution_failed","occurredAt":"2026-04-18T12:00:02Z","scope":{},"resource":{"kind":"sandbox_execution","id":"sandbox_exec_3"},"payload":{"profileId":"subprocess_default","backendKind":"subprocess","status":"failed","completedAt":"2026-04-18T12:00:02Z","outputTruncated":false,"partial":false,"errorClass":"process_failed","errorCode":"sandbox_process_failed","error":"exit status 1"}}"##,
        ),
        (
            r##"schemas/events/sandbox-execution-cancelled.event.schema.json"##,
            r##"{"eventId":"evt_sandbox_6","sequence":15,"category":"sandbox","name":"sandbox.execution_cancelled","occurredAt":"2026-04-18T12:00:02Z","scope":{},"resource":{"kind":"sandbox_execution","id":"sandbox_exec_4"},"payload":{"profileId":"subprocess_default","backendKind":"subprocess","status":"cancelled","completedAt":"2026-04-18T12:00:02Z","outputTruncated":false,"partial":false,"errorClass":"cancelled","errorCode":"sandbox_cancelled","error":"execution was cancelled"}}"##,
        ),
        (
            r##"schemas/events/sandbox-execution-denied.event.schema.json"##,
            r##"{"eventId":"evt_sandbox_7","sequence":16,"category":"sandbox","name":"sandbox.execution_denied","occurredAt":"2026-04-18T12:00:02Z","scope":{},"resource":{"kind":"sandbox_execution","id":"sandbox_exec_1"},"payload":{"profileId":"subprocess_default","backendKind":"subprocess","status":"denied","completedAt":"2026-04-18T12:00:02Z","outputTruncated":false,"partial":false,"errorClass":"approval_required","errorCode":"sandbox_approval_required","error":"sandbox execution requires approval"}}"##,
        ),
    ];
    validate_fixtures(&validator, fixtures);
}
