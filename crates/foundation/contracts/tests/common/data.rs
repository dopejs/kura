#![allow(dead_code)]
//! Fixture data shared across the ported contract tests.
//!
//! Each function mirrors a func xxxFixtures() map[string]string helper in
//! daemon/internal/contracts and is referenced by the test files exactly as
//! the Go tests call their helpers.

use super::Fixture;

pub fn activation_contract_fixtures() -> &'static [Fixture] {
    &[
        (
            r##"schemas/api/activation-state-resource.schema.json"##,
            r##"{"activationId":"act_1","principalId":"prn_1","tenantId":"ten_personal","environmentScope":"test","status":"active","currentStepId":"test_chat","completedStepIds":["tenant_resolved","quota_baseline_ready"],"blockingReasonCodes":[],"readinessItems":[],"quotaBaseline":{"tenantId":"ten_personal","planKey":"free","enforcementMode":"enforced","status":"available","quotas":[]},"firstAction":{"actionId":"test_chat","actionKind":"test_chat","recommended":true,"available":true,"blockingItemIds":[],"invokeRoute":"/v1/activation/test-chat","resultRoute":"/v1/activation"},"lastEvaluatedAt":"2026-05-06T00:00:00Z"}"##,
        ),
        (
            r##"schemas/api/activation.response.schema.json"##,
            r##"{"activation":{"activationId":"act_1","principalId":"prn_1","tenantId":"ten_personal","environmentScope":"test","status":"active","currentStepId":"test_chat","completedStepIds":["tenant_resolved","quota_baseline_ready"],"blockingReasonCodes":[],"readinessItems":[],"quotaBaseline":{"tenantId":"ten_personal","planKey":"free","enforcementMode":"enforced","status":"available","quotas":[]},"firstAction":{"actionId":"test_chat","actionKind":"test_chat","recommended":true,"available":true,"blockingItemIds":[],"invokeRoute":"/v1/activation/test-chat","resultRoute":"/v1/activation"},"lastEvaluatedAt":"2026-05-06T00:00:00Z"}}"##,
        ),
        (
            r##"schemas/api/activation-test-chat.request.schema.json"##,
            r##"{"message":"Run a safe hosted activation test."}"##,
        ),
        (
            r##"schemas/api/activation-test-chat.response.schema.json"##,
            r##"{"activation":{"activationId":"act_1","principalId":"prn_1","tenantId":"ten_personal","environmentScope":"test","status":"first_action_completed","currentStepId":"completed","completedStepIds":["tenant_resolved","quota_baseline_ready","test_chat_completed"],"blockingReasonCodes":[],"readinessItems":[],"quotaBaseline":{"tenantId":"ten_personal","planKey":"free","enforcementMode":"enforced","status":"available","quotas":[]},"firstAction":{"actionId":"test_chat","actionKind":"test_chat","recommended":true,"available":true,"blockingItemIds":[],"invokeRoute":"/v1/activation/test-chat","resultRoute":"/v1/activation"},"firstActionCompletedAt":"2026-05-06T00:00:00Z","lastEvaluatedAt":"2026-05-06T00:00:00Z"},"testChat":{"dispatchId":"dispatch_1","status":"completed","provider":"test","model":"test-chat","finishReason":"stop","usage":{},"completedAt":"2026-05-06T00:00:00Z"}}"##,
        ),
        (
            r##"schemas/api/activation-diagnostic-list.response.schema.json"##,
            r##"{"items":[{"activationId":"act_1","tenantId":"ten_personal","principalId":"prn_1","status":"blocked","stage":"quota_baseline","reasonCode":"activation_blocked:quota_baseline_unavailable","retryable":true,"remediationOwner":"operator","lastTransitionAt":"2026-05-06T00:00:00Z","readinessItemIds":["quota-baseline"],"quotaBaselineStatus":"unavailable"}]}"##,
        ),
    ]
}

pub fn billing_contract_fixtures() -> &'static [Fixture] {
    &[
        (
            r##"schemas/api/billing-plan.response.schema.json"##,
            r##"{"planId":"plan_1","tenantId":"ten_1","planKey":"finite","status":"active","enforcementMode":"enforced","effectiveAt":"2026-04-28T10:00:00Z","assignedByPrincipalId":"prn_admin","assignmentReason":"test assignment"}"##,
        ),
        (
            r##"schemas/api/billing-quota-resource.schema.json"##,
            r##"{"tenantId":"ten_1","planKey":"finite","category":"run_launches","unit":"count","periodStart":"2026-04-01T00:00:00Z","periodEnd":"2026-05-01T00:00:00Z","periodAnchor":"UTC","limit":10,"consumedAmount":5,"reservedAmount":1,"adjustedAmount":0,"carryoverApplied":0,"remainingAmount":4,"enforcementMode":"enforced"}"##,
        ),
        (
            r##"schemas/api/billing-usage.response.schema.json"##,
            r##"{"tenantId":"ten_1","planKey":"finite","enforcementMode":"enforced","quotas":[{"tenantId":"ten_1","planKey":"finite","category":"run_launches","unit":"count","periodStart":"2026-04-01T00:00:00Z","periodEnd":"2026-05-01T00:00:00Z","limit":10,"consumedAmount":5,"reservedAmount":1,"adjustedAmount":0,"carryoverApplied":0,"remainingAmount":4,"enforcementMode":"enforced"}],"manualAdjustments":[],"denials":[]}"##,
        ),
        (
            r##"schemas/api/billing-quota-dashboard.response.schema.json"##,
            r##"{"tenantId":"ten_1","plan":{"planKey":"finite","enforcementMode":"enforced","status":"active","effectiveAt":"2026-04-28T10:00:00Z","basePlanLabel":"finite","checkoutAvailable":false},"sections":[{"sectionKey":"launches","label":"Launches","items":[{"category":"run_launches","unit":"count","status":"near_limit","currentPeriod":{"periodStart":"2026-04-01T00:00:00Z","periodEnd":"2026-05-01T00:00:00Z","periodAnchor":"UTC","consumedAmount":8,"reservedAmount":0,"adjustedAmount":0,"carryoverApplied":0,"remainingAmount":2,"overLimit":false},"limit":10,"remainingAmount":2,"nearLimit":true,"nearLimitReason":"percent_threshold","typicalOperationAmount":1,"baseLimit":10,"effectiveLimit":10,"recoveryActions":["wait","reduce_scope"]}]}],"generatedAt":"2026-04-28T10:00:00Z","permission":{"allowed":true}}"##,
        ),
        (
            r##"schemas/api/billing-denial-detail.response.schema.json"##,
            r##"{"denialId":"denial_1","tenantId":"ten_1","operationRef":"run:client_1","operationKey":"tenant:ten_1:run:client_1","guardedEntryPoint":"POST /v1/runs","category":"run_launches","reasonCode":"quota_denied:run_launches_exhausted","classification":"quota_exhaustion","requestedAmount":1,"remainingAmount":0,"recoveryActions":["wait","reduce_scope"],"createdAt":"2026-04-28T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/billing-evidence-export.response.schema.json"##,
            r##"{"schemaVersion":"2026-05-07","exportId":"evidence_denial_1","tenantId":"ten_1","generatedAt":"2026-04-28T10:00:00Z","generatedByPrincipalId":"prn_support","denial":{"denialId":"denial_1","tenantId":"ten_1","operationRef":"run:client_1","operationKey":"tenant:ten_1:run:client_1","guardedEntryPoint":"POST /v1/runs","category":"run_launches","reasonCode":"quota_denied:run_launches_exhausted","classification":"quota_exhaustion","requestedAmount":1,"remainingAmount":0,"recoveryActions":["wait","reduce_scope"],"createdAt":"2026-04-28T10:00:00Z"},"usageSnapshot":[],"effectiveLimitState":{},"auditRefs":["audit_1"],"redactions":[{"path":"$.secret","reason":"secret","replacement":"[REDACTED]"}]}"##,
        ),
        (
            r##"schemas/api/billing-denial-resource.schema.json"##,
            r##"{"denialId":"denial_1","tenantId":"ten_1","category":"run_launches","quotaPeriodId":"period_1","operationKey":"tenant:ten_1:run:client_1","reasonCode":"quota_denied:run_launches_exhausted","requestedAmount":1,"remainingAmount":0,"guardedEntryPoint":"POST /v1/runs","createdAt":"2026-04-28T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/billing-manual-adjustment-resource.schema.json"##,
            r##"{"adjustmentId":"adjustment_1","tenantId":"ten_1","category":"run_launches","quotaPeriodId":"period_1","amountDelta":-1,"reason":"operator correction","createdByPrincipalId":"prn_admin","createdAt":"2026-04-28T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/billing-manual-adjustment.request.schema.json"##,
            r##"{"category":"run_launches","quotaPeriodId":"period_1","amountDelta":-1,"reason":"operator correction"}"##,
        ),
        (
            r##"schemas/api/billing-plan-assignment.request.schema.json"##,
            r##"{"planKey":"finite","enforcementMode":"enforced","reason":"customer plan assignment"}"##,
        ),
        (
            r##"schemas/api/billing-quota-override.request.schema.json"##,
            r##"{"category":"run_launches","limit":10,"reason":"temporary increase"}"##,
        ),
        (
            r##"schemas/api/billing-reservation-resource.schema.json"##,
            r##"{"reservationId":"reservation_1","tenantId":"ten_1","category":"run_launches","quotaPeriodId":"period_1","operationKey":"tenant:ten_1:run:client_1","amountReserved":1,"amountCommitted":0,"amountRefunded":0,"status":"operator_action_needed","reservationPoint":"POST /v1/runs before runtime.CreateRun","recoveryReason":"restart outcome could not be proven","createdAt":"2026-04-28T10:00:00Z","updatedAt":"2026-04-28T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/billing-reservation-resolution.request.schema.json"##,
            r##"{"outcome":"released","reason":"work did not start","amount":1}"##,
        ),
        (
            r##"schemas/api/error-response.schema.json"##,
            r##"{"code":"quota_denied","reasonCode":"quota_denied:run_launches_exhausted","tenantId":"ten_1","category":"run_launches","operationKey":"tenant:ten_1:run:client_2","periodStart":"2026-04-01T00:00:00Z","periodEnd":"2026-05-01T00:00:00Z","requestedAmount":1,"remainingAmount":0,"message":"Quota exhausted for run launches."}"##,
        ),
        (
            r##"schemas/events/billing-usage-reserved.event.schema.json"##,
            r##"{"eventId":"evt_1","sequence":1,"category":"billing","name":"billing.usage_reserved","occurredAt":"2026-04-28T10:00:00Z","scope":{},"resource":{"kind":"billing_reservation","id":"reservation_1"},"payload":{"tenantId":"ten_1","category":"run_launches","operationKey":"tenant:ten_1:run:client_1","amount":1}}"##,
        ),
        (
            r##"schemas/events/billing-usage-committed.event.schema.json"##,
            r##"{"eventId":"evt_2","sequence":2,"category":"billing","name":"billing.usage_committed","occurredAt":"2026-04-28T10:00:01Z","scope":{},"resource":{"kind":"billing_reservation","id":"reservation_1"},"payload":{"tenantId":"ten_1","category":"run_launches","operationKey":"tenant:ten_1:run:client_1","amount":1}}"##,
        ),
        (
            r##"schemas/events/billing-usage-refunded.event.schema.json"##,
            r##"{"eventId":"evt_3","sequence":3,"category":"billing","name":"billing.usage_refunded","occurredAt":"2026-04-28T10:00:02Z","scope":{},"resource":{"kind":"billing_reservation","id":"reservation_1"},"payload":{"tenantId":"ten_1","category":"run_launches","operationKey":"tenant:ten_1:run:client_1","amount":1}}"##,
        ),
        (
            r##"schemas/events/billing-quota-denied.event.schema.json"##,
            r##"{"eventId":"evt_4","sequence":4,"category":"billing","name":"billing.quota_denied","occurredAt":"2026-04-28T10:00:03Z","scope":{},"resource":{"kind":"billing_denial","id":"denial_1"},"payload":{"tenantId":"ten_1","category":"run_launches","operationKey":"tenant:ten_1:run:client_2","reasonCode":"quota_denied:run_launches_exhausted","requestedAmount":1,"remainingAmount":0}}"##,
        ),
        (
            r##"schemas/events/billing-manual-adjustment-created.event.schema.json"##,
            r##"{"eventId":"evt_5","sequence":5,"category":"billing","name":"billing.manual_adjustment_created","occurredAt":"2026-04-28T10:00:04Z","scope":{},"resource":{"kind":"billing_manual_adjustment","id":"adjustment_1"},"payload":{"tenantId":"ten_1","category":"run_launches","quotaPeriodId":"period_1","amountDelta":-1,"reason":"operator correction"}}"##,
        ),
        (
            r##"schemas/events/billing-reservation-recovery-decided.event.schema.json"##,
            r##"{"eventId":"evt_6","sequence":6,"category":"billing","name":"billing.reservation_recovery_decided","occurredAt":"2026-04-28T10:00:05Z","scope":{},"resource":{"kind":"billing_reservation","id":"reservation_1"},"payload":{"tenantId":"ten_1","category":"run_launches","operationKey":"tenant:ten_1:run:client_1","outcome":"operator_action_needed","reason":"restart outcome could not be proven"}}"##,
        ),
    ]
}

pub fn calendar_contract_fixtures() -> &'static [Fixture] {
    &[
        (
            r##"schemas/api/calendar-operation-summary.schema.json"##,
            r##"{"operationId":"calendar_op_1","operationClass":"list_events","integrationId":"calendar-a","externalEventId":"evt_ext_1","status":"completed","timezoneUsed":"America/Los_Angeles","capturedAt":"2026-04-23T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/calendar-account-resource.schema.json"##,
            r##"{"calendarAccountId":"acct_calendar-a","integrationId":"calendar-a","domainKind":"calendar","environmentScope":"test","accountKey":"acct_calendar","accountLabel":"Primary Calendar","readinessStatus":"healthy","canonicalDefault":true,"selectionMode":"canonical_default","primaryCalendarRef":"primary","primaryCalendarLabel":"Primary Calendar","primaryTimezone":"America/Los_Angeles","supportsEventInspection":true,"supportsBusyFree":true,"supportsTimedMutation":true,"lastSyncedAt":"2026-04-23T10:00:00Z","updatedAt":"2026-04-23T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/calendar-account-list.response.schema.json"##,
            r##"{"items":[{"calendarAccountId":"acct_calendar-a","integrationId":"calendar-a","domainKind":"calendar","environmentScope":"test","readinessStatus":"healthy","canonicalDefault":true,"primaryCalendarRef":"primary","primaryCalendarLabel":"Primary Calendar","primaryTimezone":"America/Los_Angeles","supportsEventInspection":true,"supportsBusyFree":true,"supportsTimedMutation":true,"lastSyncedAt":"2026-04-23T10:00:00Z","updatedAt":"2026-04-23T10:00:00Z"}]}"##,
        ),
        (
            r##"schemas/api/calendar-event-resource.schema.json"##,
            r##"{"externalEventId":"evt_ext_1","integrationId":"calendar-a","calendarAccountId":"acct_calendar-a","calendarRef":"primary","title":"Design review","startsAt":"2026-04-23T17:00:00Z","endsAt":"2026-04-23T17:30:00Z","timezone":"America/Los_Angeles","mutationEligibleInPhase":true,"lifecycleState":"active","createdAt":"2026-04-23T10:00:00Z","updatedAt":"2026-04-23T10:05:00Z"}"##,
        ),
        (
            r##"schemas/api/calendar-availability-query-resource.schema.json"##,
            r##"{"queryId":"calendar_op_2","operationId":"calendar_op_2","integrationId":"calendar-a","calendarAccountId":"acct_calendar-a","windowStart":"2026-04-23T16:00:00Z","windowEnd":"2026-04-23T18:00:00Z","timezone":"America/Los_Angeles","busyIntervals":[{"startsAt":"2026-04-23T17:00:00Z","endsAt":"2026-04-23T17:30:00Z"}],"conflictCount":1,"resultSummary":"1 busy interval(s)","createdAt":"2026-04-23T10:01:00Z"}"##,
        ),
        (
            r##"schemas/api/calendar-event-artifact.schema.json"##,
            r##"{"artifactId":"calendar_artifact_1","operationId":"calendar_op_1","kind":"event_snapshot","integrationId":"calendar-a","environmentScope":"test","externalEventId":"evt_ext_1","calendarRef":"primary","title":"Design review","startsAt":"2026-04-23T17:00:00Z","endsAt":"2026-04-23T17:30:00Z","timezone":"America/Los_Angeles","mutationEligibleInPhase":true,"lifecycleState":"active","createdAt":"2026-04-23T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/calendar-operation-resource.schema.json"##,
            r##"{"operationId":"calendar_op_1","operationClass":"list_events","status":"completed","integrationId":"calendar-a","calendarAccountId":"acct_calendar-a","environmentScope":"test","calendarRef":"primary","selectionMode":"explicit","timezoneUsed":"America/Los_Angeles","requestSummary":"2026-04-23T16:00:00Z/2026-04-23T18:00:00Z","externalEventId":"evt_ext_1","runId":"run_1","stepId":"step_1","toolCallId":"tool_call_1","workflowId":"wf_1","workflowStepId":"wfstep_1","scheduleId":"sched_1","scheduleAttemptId":"sched_attempt_1","deliveryId":"delivery_1","artifactIds":["calendar_artifact_1"],"createdAt":"2026-04-23T10:00:00Z","completedAt":"2026-04-23T10:00:01Z","updatedAt":"2026-04-23T10:00:01Z"}"##,
        ),
        (
            r##"schemas/api/calendar-operation-list.response.schema.json"##,
            r##"{"items":[{"operationId":"calendar_op_1","operationClass":"list_events","status":"completed","integrationId":"calendar-a","calendarAccountId":"acct_calendar-a","environmentScope":"test","createdAt":"2026-04-23T10:00:00Z","updatedAt":"2026-04-23T10:00:01Z"}]}"##,
        ),
        (
            r##"schemas/api/calendar-event-list.response.schema.json"##,
            r##"{"account":{"calendarAccountId":"acct_calendar-a","integrationId":"calendar-a","domainKind":"calendar","environmentScope":"test","readinessStatus":"healthy","canonicalDefault":true,"primaryCalendarRef":"primary","primaryCalendarLabel":"Primary Calendar","primaryTimezone":"America/Los_Angeles","supportsEventInspection":true,"supportsBusyFree":true,"supportsTimedMutation":true,"lastSyncedAt":"2026-04-23T10:00:00Z","updatedAt":"2026-04-23T10:00:00Z"},"items":[{"externalEventId":"evt_ext_1","integrationId":"calendar-a","calendarAccountId":"acct_calendar-a","calendarRef":"primary","title":"Design review","startsAt":"2026-04-23T17:00:00Z","endsAt":"2026-04-23T17:30:00Z","timezone":"America/Los_Angeles","mutationEligibleInPhase":true,"lifecycleState":"active","createdAt":"2026-04-23T10:00:00Z","updatedAt":"2026-04-23T10:05:00Z"}],"operation":{"operationId":"calendar_op_1","operationClass":"list_events","status":"completed","integrationId":"calendar-a","calendarAccountId":"acct_calendar-a","environmentScope":"test","createdAt":"2026-04-23T10:00:00Z","updatedAt":"2026-04-23T10:00:01Z"},"artifacts":[{"artifactId":"calendar_artifact_1","operationId":"calendar_op_1","kind":"event_snapshot","integrationId":"calendar-a","environmentScope":"test","createdAt":"2026-04-23T10:00:00Z"}]}"##,
        ),
        (
            r##"schemas/events/calendar-account-projected.event.schema.json"##,
            r##"{"eventId":"evt_calendar_1","sequence":1,"category":"calendar","name":"calendar.account_projected","occurredAt":"2026-04-23T10:00:00Z","scope":{},"resource":{"kind":"calendar_account","id":"acct_calendar-a"},"payload":{"integrationId":"calendar-a","accountKey":"acct_calendar","primaryCalendarRef":"primary","primaryTimezone":"America/Los_Angeles","readinessStatus":"healthy","canonicalDefault":true}}"##,
        ),
        (
            r##"schemas/events/calendar-operation-requested.event.schema.json"##,
            r##"{"eventId":"evt_calendar_2","sequence":2,"category":"calendar","name":"calendar.operation_requested","occurredAt":"2026-04-23T10:00:00Z","scope":{"runId":"run_1","workflowId":"wf_1","scheduleId":"sched_1"},"resource":{"kind":"calendar_operation","id":"calendar_op_1"},"payload":{"operationId":"calendar_op_1","operationClass":"list_events","integrationId":"calendar-a","runId":"run_1","workflowId":"wf_1","scheduleId":"sched_1","deliveryId":"delivery_1","status":"completed","timezoneUsed":"America/Los_Angeles","externalEventId":"evt_ext_1","failureClass":""}}"##,
        ),
        (
            r##"schemas/events/calendar-operation-completed.event.schema.json"##,
            r##"{"eventId":"evt_calendar_3","sequence":3,"category":"calendar","name":"calendar.operation_completed","occurredAt":"2026-04-23T10:00:01Z","scope":{"runId":"run_1","workflowId":"wf_1","scheduleId":"sched_1"},"resource":{"kind":"calendar_operation","id":"calendar_op_1"},"payload":{"operationId":"calendar_op_1","operationClass":"list_events","integrationId":"calendar-a","runId":"run_1","workflowId":"wf_1","scheduleId":"sched_1","status":"completed","timezoneUsed":"America/Los_Angeles","externalEventId":"evt_ext_1","failureClass":""}}"##,
        ),
        (
            r##"schemas/events/calendar-operation-failed.event.schema.json"##,
            r##"{"eventId":"evt_calendar_4","sequence":4,"category":"calendar","name":"calendar.operation_failed","occurredAt":"2026-04-23T10:00:02Z","scope":{"runId":"run_1"},"resource":{"kind":"calendar_operation","id":"calendar_op_2"},"payload":{"operationId":"calendar_op_2","operationClass":"create_event","integrationId":"calendar-a","runId":"run_1","status":"failed","timezoneUsed":"America/Los_Angeles","failureClass":"backend_error"}}"##,
        ),
        (
            r##"schemas/events/calendar-artifact-recorded.event.schema.json"##,
            r##"{"eventId":"evt_calendar_5","sequence":5,"category":"calendar","name":"calendar.artifact_recorded","occurredAt":"2026-04-23T10:00:03Z","scope":{"runId":"run_1"},"resource":{"kind":"calendar_artifact","id":"calendar_artifact_1"},"payload":{"artifactId":"calendar_artifact_1","operationId":"calendar_op_1","externalEventId":"evt_ext_1","calendarRef":"primary","lifecycleState":"active"}}"##,
        ),
    ]
}

pub fn computer_use_contract_fixtures() -> &'static [Fixture] {
    &[
        (
            r##"schemas/api/create-computer-use-session.request.schema.json"##,
            r##"{"driverKind":"browser","initialUrl":"https://example.test"}"##,
        ),
        (
            r##"schemas/api/computer-use-session-resource.schema.json"##,
            r##"{"computerUseSessionId":"cusess_1","environmentScope":"test","runId":"run_1","status":"active","driverKind":"browser","currentPage":{"url":"https://example.test","title":"example.test"},"startedAt":"2026-04-22T10:00:00Z","updatedAt":"2026-04-22T10:00:01Z","actions":[]}"##,
        ),
        (
            r##"schemas/api/computer-use-session-list.response.schema.json"##,
            r##"{"items":[{"computerUseSessionId":"cusess_1","environmentScope":"test","runId":"run_1","status":"active","driverKind":"browser","startedAt":"2026-04-22T10:00:00Z","updatedAt":"2026-04-22T10:00:01Z"}]}"##,
        ),
        (
            r##"schemas/api/create-computer-use-action.request.schema.json"##,
            r##"{"actionKind":"input","pageTarget":"active_page","value":"Phase 26","targetMatchContext":{"matchStrategy":"dom_selector","expectedSelector":"#name"}}"##,
        ),
        (
            r##"schemas/api/computer-use-action-resource.schema.json"##,
            r##"{"computerUseActionId":"cuact_1","environmentScope":"test","computerUseSessionId":"cusess_1","runId":"run_1","actionKind":"input","status":"failed","riskLevel":"high","failureClass":"target_mismatch","failureReason":"approved target no longer matches current page","requestedAt":"2026-04-22T10:00:02Z","updatedAt":"2026-04-22T10:00:03Z","completedAt":"2026-04-22T10:00:03Z","artifacts":[{"artifactId":"cuart_1","environmentScope":"test","computerUseSessionId":"cusess_1","computerUseActionId":"cuact_1","runId":"run_1","kind":"page_snapshot","status":"available","mimeType":"application/json","fileName":"page-snapshot.json","byteSize":10,"storageKey":"computer-use/cusess_1/cuart_1","sha256":"abc","createdAt":"2026-04-22T10:00:03Z"}]}"##,
        ),
        (
            r##"schemas/api/computer-use-action-list.response.schema.json"##,
            r##"{"items":[{"computerUseActionId":"cuact_1","environmentScope":"test","computerUseSessionId":"cusess_1","runId":"run_1","actionKind":"navigate","status":"completed","riskLevel":"low","requestedAt":"2026-04-22T10:00:02Z","updatedAt":"2026-04-22T10:00:03Z"}]}"##,
        ),
        (
            r##"schemas/api/computer-use-artifact-resource.schema.json"##,
            r##"{"artifactId":"cuart_1","environmentScope":"test","computerUseSessionId":"cusess_1","computerUseActionId":"cuact_1","runId":"run_1","kind":"page_snapshot","status":"available","mimeType":"application/json","fileName":"page-snapshot.json","byteSize":10,"storageKey":"computer-use/cusess_1/cuart_1","sha256":"abc","createdAt":"2026-04-22T10:00:03Z"}"##,
        ),
        (
            r##"schemas/api/computer-use-artifact-content.response.schema.json"##,
            r##"{"artifactId":"cuart_1","mimeType":"application/json","fileName":"page-snapshot.json","status":"available","content":"ZXhhbXBsZQ=="}"##,
        ),
        (
            r##"schemas/api/workflow-step-resource.schema.json"##,
            r##"{"workflowStepId":"wfstep_browser_1","workflowId":"wf_1","title":"Inspect deterministic browser fixture","position":1,"consumerKind":"computer_use","consumerId":"browser","toolName":"browser","status":"completed","runtimeStepId":"step_1","activeToolCallId":"toolcall_1","attemptCount":1,"maxAttempts":1,"computerUseSessionId":"cusess_1","computerUseActionIds":["cuact_nav_1","cuact_snap_1"],"computerUseArtifacts":[{"artifactId":"cuart_1","environmentScope":"test","computerUseSessionId":"cusess_1","computerUseActionId":"cuact_snap_1","runId":"run_1","kind":"page_snapshot","status":"available","mimeType":"application/json","fileName":"page-snapshot.json","byteSize":10,"storageKey":"computer-use/cusess_1/cuart_1","sha256":"abc","createdAt":"2026-04-22T10:00:03Z"}],"createdAt":"2026-04-22T10:00:00Z","updatedAt":"2026-04-22T10:00:03Z"}"##,
        ),
        (
            r##"schemas/api/tool-call-resource.schema.json"##,
            r##"{"toolCallId":"toolcall_1","runId":"run_1","stepId":"step_1","workflowId":"wf_1","workflowStepId":"wfstep_browser_1","computerUseSessionId":"cusess_1","computerUseActionId":"cuact_snap_1","invocationKind":"local_tool","capabilityId":"browser","toolName":"snapshot","status":"completed","createdAt":"2026-04-22T10:00:02Z","updatedAt":"2026-04-22T10:00:03Z","output":{"computerUseSessionId":"cusess_1","computerUseActionId":"cuact_snap_1"}}"##,
        ),
        (
            r##"schemas/api/tool-call-list.response.schema.json"##,
            r##"{"items":[{"toolCallId":"toolcall_1","runId":"run_1","stepId":"step_1","workflowId":"wf_1","workflowStepId":"wfstep_browser_1","computerUseSessionId":"cusess_1","computerUseActionId":"cuact_snap_1","invocationKind":"local_tool","capabilityId":"browser","toolName":"snapshot","status":"completed","createdAt":"2026-04-22T10:00:02Z","updatedAt":"2026-04-22T10:00:03Z"}]}"##,
        ),
        (
            r##"schemas/events/runtime-event.schema.json"##,
            r##"{"eventId":"evt_runtime_1","sequence":1,"category":"capability","name":"computer_use.action_status_changed","occurredAt":"2026-04-22T10:00:03Z","scope":{"runId":"run_1","stepId":"step_1","computerUseSessionId":"cusess_1","computerUseActionId":"cuact_1"},"resource":{"kind":"computer_use_action","id":"cuact_1"},"payload":{"status":"failed","failureClass":"target_mismatch"}}"##,
        ),
        (
            r##"schemas/events/computer-use-session-created.event.schema.json"##,
            r##"{"eventId":"evt_1","sequence":1,"category":"capability","name":"computer_use.session_created","occurredAt":"2026-04-22T10:00:00Z","scope":{"runId":"run_1","computerUseSessionId":"cusess_1"},"resource":{"kind":"computer_use_session","id":"cusess_1"},"payload":{"status":"active","computerUseSessionId":"cusess_1"}}"##,
        ),
        (
            r##"schemas/events/computer-use-session-status-changed.event.schema.json"##,
            r##"{"eventId":"evt_2","sequence":2,"category":"capability","name":"computer_use.session_status_changed","occurredAt":"2026-04-22T10:00:01Z","scope":{"runId":"run_1","computerUseSessionId":"cusess_1"},"resource":{"kind":"computer_use_session","id":"cusess_1"},"payload":{"status":"closed","computerUseSessionId":"cusess_1"}}"##,
        ),
        (
            r##"schemas/events/computer-use-action-requested.event.schema.json"##,
            r##"{"eventId":"evt_3","sequence":3,"category":"capability","name":"computer_use.action_requested","occurredAt":"2026-04-22T10:00:02Z","scope":{"runId":"run_1","stepId":"step_1","computerUseSessionId":"cusess_1","computerUseActionId":"cuact_1"},"resource":{"kind":"computer_use_action","id":"cuact_1"},"payload":{"status":"waiting_approval","actionKind":"input","computerUseSessionId":"cusess_1","computerUseActionId":"cuact_1"}}"##,
        ),
        (
            r##"schemas/events/computer-use-action-status-changed.event.schema.json"##,
            r##"{"eventId":"evt_4","sequence":4,"category":"capability","name":"computer_use.action_status_changed","occurredAt":"2026-04-22T10:00:03Z","scope":{"runId":"run_1","stepId":"step_1","computerUseSessionId":"cusess_1","computerUseActionId":"cuact_1"},"resource":{"kind":"computer_use_action","id":"cuact_1"},"payload":{"status":"completed","computerUseSessionId":"cusess_1","computerUseActionId":"cuact_1"}}"##,
        ),
        (
            r##"schemas/events/computer-use-action-target-mismatch.event.schema.json"##,
            r##"{"eventId":"evt_5","sequence":5,"category":"capability","name":"computer_use.action_target_mismatch","occurredAt":"2026-04-22T10:00:04Z","scope":{"runId":"run_1","stepId":"step_1","computerUseSessionId":"cusess_1","computerUseActionId":"cuact_1"},"resource":{"kind":"computer_use_action","id":"cuact_1"},"payload":{"status":"failed","failureClass":"target_mismatch","computerUseSessionId":"cusess_1","computerUseActionId":"cuact_1"}}"##,
        ),
        (
            r##"schemas/events/computer-use-artifact-recorded.event.schema.json"##,
            r##"{"eventId":"evt_6","sequence":6,"category":"capability","name":"computer_use.artifact_recorded","occurredAt":"2026-04-22T10:00:05Z","scope":{"runId":"run_1","computerUseSessionId":"cusess_1","computerUseActionId":"cuact_1"},"resource":{"kind":"computer_use_artifact","id":"cuart_1"},"payload":{"artifactId":"cuart_1","artifactKind":"page_snapshot","captureStatus":"available"}}"##,
        ),
    ]
}

pub fn connector_conformance_contract_fixtures() -> &'static [Fixture] {
    &[
        (
            r##"schemas/api/connector-capability-profile.schema.json"##,
            r##"{"connectorKind":"discord","connectorId":"connector_discord_test","lifecycleState":"healthy","surfaces":{"direct_message":"supported","group_channel":"supported","thread_reply":"limited","incremental_update":"unsupported","background_delivery":"supported"},"identityRules":{"durableIdentity":"tenant_id + connector_account_id + channel_or_conversation_id + provider_message_id","equivalentIdentity":"provider_message_id aliases webhook delivery id when provider supplies both"},"coreInvariants":{"tenant_scoped_identity":"pass","dedupe":"pass","foreground_background_separation":"pass","redacted_diagnostics":"pass"},"generatedAt":"2026-05-07T10:00:00Z","retentionExpiresAt":"2026-08-05T10:00:00Z","redactionStatus":"redacted"}"##,
        ),
        (
            r##"schemas/api/connector-conformance-result.schema.json"##,
            r##"{"conformanceResultId":"conformance_result_1","tenantId":"ten_033","connectorKind":"discord","connectorId":"connector_discord_test","scenarioId":"discord.thread_reply.limited","area":"thread_reply","result":"limited","reasonCode":"unsupported_capability","evidenceTimestamp":"2026-05-07T10:00:00Z","redactionStatus":"redacted","retentionExpiresAt":"2026-08-05T10:00:00Z","evidenceSummary":"Thread reply is recorded as limited while direct messages pass core invariants."}"##,
        ),
        (
            r##"schemas/api/connector-diagnostic-state.schema.json"##,
            r##"{"diagnosticStateId":"diagnostic_state_1","tenantId":"ten_033","connectorId":"connector_discord_test","connectorAccountId":"acct_redacted","status":"degraded","reasonCode":"rate_limited","remediationOwner":"provider","userVisibleSeverity":"warning","retrySafety":"retry_after","freshnessState":"fresh","evidenceTimestamp":"2026-05-07T10:00:00Z","staleAfter":"2026-05-07T10:15:00Z","redactionStatus":"redacted","retentionExpiresAt":"2026-08-05T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/connector-account-binding-summary.schema.json"##,
            r##"{"tenantId":"ten_033","connectorId":"connector_discord_test","connectorAccountId":"acct_redacted","displayName":"Discord workspace","providerAccountHint":"discord:guild_redacted","redactionStatus":"redacted","updatedAt":"2026-05-07T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/connector-resource.schema.json"##,
            r##"{"tenantId":"ten_033","connectorId":"connector_discord_test","kind":"discord","displayName":"Discord","status":"healthy","failureCount":0,"restartCount":1,"backoffSeconds":0,"createdAt":"2026-05-07T09:00:00Z","updatedAt":"2026-05-07T10:00:00Z","capabilityProfile":{"connectorKind":"discord","connectorId":"connector_discord_test","lifecycleState":"healthy","surfaces":{"direct_message":"supported"},"identityRules":{"durableIdentity":"tenant_id + connector_account_id + channel_or_conversation_id + provider_message_id"},"coreInvariants":{"tenant_scoped_identity":"pass"},"generatedAt":"2026-05-07T10:00:00Z","retentionExpiresAt":"2026-08-05T10:00:00Z","redactionStatus":"redacted"},"diagnosticState":{"diagnosticStateId":"diagnostic_state_1","tenantId":"ten_033","connectorId":"connector_discord_test","status":"healthy","reasonCode":"healthy","remediationOwner":"none_required","userVisibleSeverity":"info","retrySafety":"no_action_needed","freshnessState":"fresh","evidenceTimestamp":"2026-05-07T10:00:00Z","redactionStatus":"redacted","retentionExpiresAt":"2026-08-05T10:00:00Z"},"conformanceResult":{"conformanceResultId":"conformance_result_1","tenantId":"ten_033","connectorKind":"discord","connectorId":"connector_discord_test","scenarioId":"discord.direct.pass","area":"direct_message","result":"pass","evidenceTimestamp":"2026-05-07T10:00:00Z","redactionStatus":"redacted","retentionExpiresAt":"2026-08-05T10:00:00Z"},"accountBinding":{"tenantId":"ten_033","connectorId":"connector_discord_test","connectorAccountId":"acct_redacted","redactionStatus":"redacted","updatedAt":"2026-05-07T10:00:00Z"}}"##,
        ),
        (
            r##"schemas/events/connector-conformance-result-recorded.event.schema.json"##,
            r##"{"eventId":"evt_connector_conformance_1","sequence":1,"category":"connector","name":"connector.conformance_result_recorded","occurredAt":"2026-05-07T10:00:00Z","scope":{"connectorId":"connector_discord_test"},"resource":{"kind":"connector_conformance_result","id":"conformance_result_1"},"payload":{"tenantId":"ten_033","conformanceResultId":"conformance_result_1","connectorKind":"discord","connectorId":"connector_discord_test","scenarioId":"discord.direct.pass","area":"direct_message","result":"pass","redactionStatus":"redacted","retentionExpiresAt":"2026-08-05T10:00:00Z"}}"##,
        ),
        (
            r##"schemas/events/connector-diagnostic-state-changed.event.schema.json"##,
            r##"{"eventId":"evt_connector_diagnostic_1","sequence":2,"category":"connector","name":"connector.diagnostic_state_changed","occurredAt":"2026-05-07T10:01:00Z","scope":{"connectorId":"connector_discord_test"},"resource":{"kind":"connector_diagnostic_state","id":"diagnostic_state_1"},"payload":{"tenantId":"ten_033","diagnosticStateId":"diagnostic_state_1","connectorId":"connector_discord_test","previousStatus":"healthy","status":"degraded","reasonCode":"rate_limited","remediationOwner":"provider","retrySafety":"retry_after","freshnessState":"fresh","redactionStatus":"redacted"}}"##,
        ),
        (
            r##"schemas/events/connector-diagnostic-redaction-failed.event.schema.json"##,
            r##"{"eventId":"evt_connector_diagnostic_2","sequence":3,"category":"connector","name":"connector.diagnostic_redaction_failed","occurredAt":"2026-05-07T10:02:00Z","scope":{"connectorId":"connector_discord_test"},"resource":{"kind":"connector_diagnostic_redaction_failure","id":"redaction_failure_1"},"payload":{"tenantId":"ten_033","connectorId":"connector_discord_test","targetKind":"diagnostic_state","targetId":"diagnostic_state_1","reasonCode":"redaction_failed_closed","redactionStatus":"suppressed","retentionExpiresAt":"2026-08-05T10:02:00Z"}}"##,
        ),
        (
            r##"schemas/events/connector-inbound-duplicate-detected.event.schema.json"##,
            r##"{"eventId":"evt_connector_duplicate_1","sequence":4,"category":"connector","name":"connector.inbound_duplicate_detected","occurredAt":"2026-05-07T10:03:00Z","scope":{"connectorId":"connector_discord_test"},"resource":{"kind":"connector_message","id":"msg_duplicate_1"},"payload":{"tenantId":"ten_033","connectorId":"connector_discord_test","connectorAccountId":"acct_redacted","channelOrConversationId":"chan_redacted","providerMessageId":"provider_msg_1","equivalentRuleId":"discord_message_id","existingDeliveryId":"delivery_1","redactionStatus":"redacted"}}"##,
        ),
        (
            r##"schemas/events/connector-route-outcome-recorded.event.schema.json"##,
            r##"{"eventId":"evt_connector_route_1","sequence":5,"category":"connector","name":"connector.route_outcome_recorded","occurredAt":"2026-05-07T10:04:00Z","scope":{"connectorId":"connector_discord_test"},"resource":{"kind":"connector_route_outcome","id":"route_outcome_1"},"payload":{"tenantId":"ten_033","connectorId":"connector_discord_test","outcome":"blocked","reasonCode":"blocked_route","surface":"group_channel","messageDeliveryId":"delivery_1","redactionStatus":"redacted"}}"##,
        ),
        (
            r##"schemas/events/connector-foreground-reply-failed.event.schema.json"##,
            r##"{"eventId":"evt_connector_reply_failed_1","sequence":6,"category":"connector","name":"connector.foreground_reply_failed","occurredAt":"2026-05-07T10:05:00Z","scope":{"connectorId":"connector_discord_test","sessionId":"session_1","runId":"run_1"},"resource":{"kind":"connector_foreground_reply","id":"foreground_reply_1"},"payload":{"tenantId":"ten_033","connectorId":"connector_discord_test","messageDeliveryId":"delivery_reply_1","status":"failed","reasonCode":"transport_failed","retrySafety":"retryable","backgroundDeliveryId":"delivery_background_1","separationStatus":"separate_truths","redactionStatus":"redacted"}}"##,
        ),
        (
            r##"schemas/events/connector-delivery-separation-recorded.event.schema.json"##,
            r##"{"eventId":"evt_connector_delivery_boundary_1","sequence":7,"category":"connector","name":"connector.delivery_separation_recorded","occurredAt":"2026-05-07T10:06:00Z","scope":{"connectorId":"connector_discord_test"},"resource":{"kind":"connector_delivery_boundary","id":"boundary_1"},"payload":{"tenantId":"ten_033","connectorId":"connector_discord_test","foregroundReplyOutcomeId":"foreground_reply_1","backgroundDeliveryId":"delivery_background_1","transportKind":"discord","separationStatus":"separate_truths","redactionStatus":"redacted"}}"##,
        ),
    ]
}

pub fn delivery_contract_fixtures() -> &'static [Fixture] {
    &[
        (
            r##"schemas/api/create-delivery-target.request.schema.json"##,
            r##"{"targetId":"test-sink-default","displayName":"Test Sink","targetKind":"test_sink","addressSummary":"repo-owned sink"}"##,
        ),
        (
            r##"schemas/api/delivery-target-resource.schema.json"##,
            r##"{"targetId":"test-sink-default","displayName":"Test Sink","environmentScope":"test","targetKind":"test_sink","status":"active","addressSummary":"repo-owned sink","supportsImmediate":true,"supportsDigest":true,"createdAt":"2026-04-22T10:00:00Z","updatedAt":"2026-04-22T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/delivery-target-list.response.schema.json"##,
            r##"{"items":[{"targetId":"test-sink-default","displayName":"Test Sink","environmentScope":"test","targetKind":"test_sink","status":"active","addressSummary":"repo-owned sink","supportsImmediate":true,"supportsDigest":true,"createdAt":"2026-04-22T10:00:00Z","updatedAt":"2026-04-22T10:00:00Z"}]}"##,
        ),
        (
            r##"schemas/api/update-delivery-target-status.request.schema.json"##,
            r##"{}"##,
        ),
        (
            r##"schemas/api/upsert-delivery-preference.request.schema.json"##,
            r##"{"preferenceId":"pref-default","scopeKind":"user_default","preferredTargetsByClass":{"routine_success":"test-sink-default","urgent":"test-sink-default","failure":"test-sink-default"},"summaryPolicy":{"routineSuccessMode":"digest","windowMinutes":15},"suppressionPolicy":{"suppressFailure":false}}"##,
        ),
        (
            r##"schemas/api/delivery-preference-resource.schema.json"##,
            r##"{"preferenceId":"pref-default","environmentScope":"test","scopeKind":"user_default","preferredTargetsByClass":{"routine_success":"test-sink-default","urgent":"test-sink-default","failure":"test-sink-default"},"summaryPolicy":{"routineSuccessMode":"digest","windowMinutes":15},"suppressionPolicy":{"suppressFailure":false},"active":true,"createdAt":"2026-04-22T10:00:00Z","updatedAt":"2026-04-22T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/delivery-preference-list.response.schema.json"##,
            r##"{"items":[{"preferenceId":"pref-default","environmentScope":"test","scopeKind":"user_default","preferredTargetsByClass":{"routine_success":"test-sink-default","urgent":"test-sink-default","failure":"test-sink-default"},"summaryPolicy":{"routineSuccessMode":"digest","windowMinutes":15},"suppressionPolicy":{"suppressFailure":false},"active":true,"createdAt":"2026-04-22T10:00:00Z","updatedAt":"2026-04-22T10:00:00Z"}]}"##,
        ),
        (
            r##"schemas/api/delivery-outcome-resource.schema.json"##,
            r##"{"deliveryId":"delivery_1","environmentScope":"test","sourceKind":"run","sourceId":"run_1","runId":"run_1","scheduleId":"sched_1","scheduleAttemptId":"sched_attempt_1","resultClass":"routine_success","mode":"immediate","status":"delivered","chosenTargetId":"test-sink-default","preferenceId":"pref-default","payloadPreview":"background result","attempts":[{"attemptId":"delivery_attempt_1","deliveryId":"delivery_1","attemptNumber":1,"targetId":"test-sink-default","transportKind":"test_sink","status":"delivered","transportReceiptSummary":"stored in repo-owned test sink","startedAt":"2026-04-22T10:00:01Z","completedAt":"2026-04-22T10:00:01Z"}],"createdAt":"2026-04-22T10:00:00Z","updatedAt":"2026-04-22T10:00:01Z","finalizedAt":"2026-04-22T10:00:01Z"}"##,
        ),
        (
            r##"schemas/api/delivery-outcome-list.response.schema.json"##,
            r##"{"items":[{"deliveryId":"delivery_1","environmentScope":"test","sourceKind":"run","sourceId":"run_1","runId":"run_1","scheduleId":"sched_1","scheduleAttemptId":"sched_attempt_1","resultClass":"routine_success","mode":"immediate","status":"delivered","chosenTargetId":"test-sink-default","preferenceId":"pref-default","payloadPreview":"background result","createdAt":"2026-04-22T10:00:00Z","updatedAt":"2026-04-22T10:00:01Z","finalizedAt":"2026-04-22T10:00:01Z"}]}"##,
        ),
        (
            r##"schemas/api/delivery-attempt-resource.schema.json"##,
            r##"{"attemptId":"delivery_attempt_1","deliveryId":"delivery_1","attemptNumber":1,"targetId":"test-sink-default","transportKind":"test_sink","status":"delivered","transportReceiptSummary":"stored in repo-owned test sink","startedAt":"2026-04-22T10:00:01Z","completedAt":"2026-04-22T10:00:01Z"}"##,
        ),
        (
            r##"schemas/api/delivery-summary-window.resource.schema.json"##,
            r##"{"summaryWindowId":"summary_window_1","environmentScope":"test","targetId":"test-sink-default","preferenceId":"pref-default","status":"delivered","windowStartedAt":"2026-04-22T10:00:00Z","windowEndsAt":"2026-04-22T10:15:00Z","resultCount":2,"emittedDeliveryId":"delivery_digest_1","createdAt":"2026-04-22T10:00:00Z","updatedAt":"2026-04-22T10:15:01Z"}"##,
        ),
        (
            r##"schemas/api/delivery-summary-window-list.response.schema.json"##,
            r##"{"items":[{"summaryWindowId":"summary_window_1","environmentScope":"test","targetId":"test-sink-default","preferenceId":"pref-default","status":"delivered","windowStartedAt":"2026-04-22T10:00:00Z","windowEndsAt":"2026-04-22T10:15:00Z","resultCount":2,"emittedDeliveryId":"delivery_digest_1","createdAt":"2026-04-22T10:00:00Z","updatedAt":"2026-04-22T10:15:01Z"}]}"##,
        ),
        (
            r##"schemas/events/delivery-target-registered.event.schema.json"##,
            r##"{"eventId":"evt_delivery_1","sequence":1,"category":"delivery","name":"delivery.target_registered","occurredAt":"2026-04-22T10:00:00Z","scope":{},"resource":{"kind":"delivery_target","id":"test-sink-default"},"payload":{"targetId":"test-sink-default","targetKind":"test_sink","environmentScope":"test","status":"active"}}"##,
        ),
        (
            r##"schemas/events/delivery-target-status-changed.event.schema.json"##,
            r##"{"eventId":"evt_delivery_2","sequence":2,"category":"delivery","name":"delivery.target_status_changed","occurredAt":"2026-04-22T10:00:01Z","scope":{},"resource":{"kind":"delivery_target","id":"test-sink-default"},"payload":{"targetId":"test-sink-default","targetKind":"test_sink","environmentScope":"test","status":"disabled"}}"##,
        ),
        (
            r##"schemas/events/delivery-preference-updated.event.schema.json"##,
            r##"{"eventId":"evt_delivery_3","sequence":3,"category":"delivery","name":"delivery.preference_updated","occurredAt":"2026-04-22T10:00:02Z","scope":{},"resource":{"kind":"delivery_preference","id":"pref-default"},"payload":{"preferenceId":"pref-default","environmentScope":"test","scopeKind":"user_default"}}"##,
        ),
        (
            r##"schemas/events/delivery-outcome-created.event.schema.json"##,
            r##"{"eventId":"evt_delivery_4","sequence":4,"category":"delivery","name":"delivery.outcome_created","occurredAt":"2026-04-22T10:00:03Z","scope":{"runId":"run_1","scheduleId":"sched_1","scheduleAttemptId":"sched_attempt_1"},"resource":{"kind":"delivery","id":"delivery_1"},"payload":{"deliveryId":"delivery_1","sourceKind":"run","sourceId":"run_1","runId":"run_1","scheduleId":"sched_1","scheduleAttemptId":"sched_attempt_1","resultClass":"routine_success","mode":"immediate","status":"dispatching","chosenTargetId":"test-sink-default"}}"##,
        ),
        (
            r##"schemas/events/delivery-attempt-recorded.event.schema.json"##,
            r##"{"eventId":"evt_delivery_5","sequence":5,"category":"delivery","name":"delivery.attempt_recorded","occurredAt":"2026-04-22T10:00:04Z","scope":{"runId":"run_1","scheduleId":"sched_1","scheduleAttemptId":"sched_attempt_1"},"resource":{"kind":"delivery","id":"delivery_1"},"payload":{"sourceKind":"run","sourceId":"run_1","runId":"run_1","scheduleId":"sched_1","scheduleAttemptId":"sched_attempt_1","deliveryId":"delivery_1","attemptId":"delivery_attempt_1","attemptNumber":1,"transportKind":"test_sink","status":"delivered"}}"##,
        ),
        (
            r##"schemas/events/delivery-outcome-status-changed.event.schema.json"##,
            r##"{"eventId":"evt_delivery_6","sequence":6,"category":"delivery","name":"delivery.outcome_status_changed","occurredAt":"2026-04-22T10:00:05Z","scope":{"runId":"run_1","scheduleId":"sched_1","scheduleAttemptId":"sched_attempt_1"},"resource":{"kind":"delivery","id":"delivery_1"},"payload":{"sourceKind":"run","sourceId":"run_1","runId":"run_1","scheduleId":"sched_1","scheduleAttemptId":"sched_attempt_1","deliveryId":"delivery_1","resultClass":"routine_success","mode":"immediate","status":"delivered","chosenTargetId":"test-sink-default"}}"##,
        ),
        (
            r##"schemas/events/delivery-summary-emitted.event.schema.json"##,
            r##"{"eventId":"evt_delivery_7","sequence":7,"category":"delivery","name":"delivery.summary_emitted","occurredAt":"2026-04-22T10:15:01Z","scope":{},"resource":{"kind":"delivery_summary_window","id":"summary_window_1"},"payload":{"summaryWindowId":"summary_window_1","resultCount":2,"emittedDeliveryId":"delivery_digest_1"}}"##,
        ),
    ]
}

pub fn discord_hardening_contract_fixtures() -> &'static [Fixture] {
    &[
        (
            r##"schemas/api/discord-hosted-setup-resource.schema.json"##,
            r##"{"tenantId":"ten_discord","connectorId":"discord-main","connectorKind":"discord","displayName":"Discord Main","status":"degraded","readinessState":"degraded_needs_repair","hostedReady":false,"credentialState":"valid","respondInDM":true,"requireMention":true,"deliveryMode":"gateway","reasonCode":"destination_validation_failed","redactionStatus":"redacted","createdAt":"2026-05-07T10:00:00Z","updatedAt":"2026-05-07T10:01:00Z","validatedAt":"2026-05-07T10:01:00Z","retentionExpiresAt":"2026-08-05T10:01:00Z","destinations":[{"tenantId":"ten_discord","connectorId":"discord-main","destinationId":"channel_redacted","destinationType":"channel","selected":true,"validationState":"missing_permission","reasonCode":"permission_missing","validatedAt":"2026-05-07T10:01:00Z","redactionStatus":"redacted","safeEvidence":{"permission":"send_messages"}}]}"##,
        ),
        (
            r##"schemas/api/discord-destination-validation-resource.schema.json"##,
            r##"{"tenantId":"ten_discord","connectorId":"discord-main","destinationId":"guild_redacted","destinationType":"guild","providerLabel":"Guild redacted","selected":true,"validationState":"valid","reasonCode":"healthy","validatedAt":"2026-05-07T10:01:00Z","redactionStatus":"redacted","safeEvidence":{"validation":"bot_member"}}"##,
        ),
        (
            r##"schemas/api/discord-smoke-evidence-resource.schema.json"##,
            r##"{"smokeEvidenceId":"discord_smoke_1","tenantId":"ten_discord","connectorId":"discord-main","status":"skipped","credentialMode":"unavailable","owner":"operator","reason":"safe_credentials_unavailable","remainingRisk":"No live Discord hosted smoke was run in this release validation.","validatedAt":"2026-05-07T10:02:00Z","retentionExpiresAt":"2026-08-05T10:02:00Z","redactionStatus":"redacted","safeEvidence":{"policy":"structured_skip"}}"##,
        ),
        (
            r##"schemas/events/connector-discord-setup-validated.event.schema.json"##,
            r##"{"eventId":"evt_discord_setup_1","sequence":1,"category":"connector","name":"connector.discord_setup_validated","occurredAt":"2026-05-07T10:01:00Z","scope":{"connectorId":"discord-main"},"resource":{"kind":"discord_hosted_setup","id":"discord-main"},"payload":{"tenantId":"ten_discord","connectorId":"discord-main","readinessState":"degraded_needs_repair","hostedReady":false,"credentialState":"valid","reasonCode":"destination_validation_failed","redactionStatus":"redacted","validatedAt":"2026-05-07T10:01:00Z"}}"##,
        ),
        (
            r##"schemas/events/connector-reply-failed.event.schema.json"##,
            r##"{"eventId":"evt_discord_reply_failed_1","sequence":2,"category":"connector","name":"connector.reply_failed","occurredAt":"2026-05-07T10:03:00Z","scope":{"connectorId":"discord-main","runId":"run_1"},"resource":{"kind":"connector","id":"discord-main"},"payload":{"tenantId":"ten_discord","connectorId":"discord-main","messageId":"discord_msg_1","replyMessageId":"discord_reply_1","assistantExecutionOutcome":"succeeded","discordDeliveryOutcome":"failed","errorClass":"network_failed","reasonCode":"reply_failed","redactionStatus":"redacted"}}"##,
        ),
    ]
}

pub fn evaluation_contract_fixtures() -> &'static [Fixture] {
    &[
        (
            r##"schemas/api/replay-candidate-resource.schema.json"##,
            r##"{"candidateId":"candidate_fixture_schedule","candidateKind":"fixture","displayName":"Schedule Fixture","sourceKind":"fixture","sourceId":"fixture_schedule","sourceRefs":[{"kind":"schedule","id":"sched_1","route":"/v1/schedules/sched_1"}],"toolClasses":["calendar.event.create"],"environmentScope":"test","readinessStatus":"fully_replayable","readinessReasons":["fixture has evidence"],"limitations":[],"defaultReplayMode":"non_live","fixtureId":"fixture_schedule","latestAttemptId":"attempt_1","latestComparisonId":"comparison_1","capturedEvidenceRefs":[{"kind":"fixture_evidence","id":"schedule/evidence.json"}],"createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/replay-candidate-list.response.schema.json"##,
            r##"{"environmentScope":"test","items":[{"candidateId":"candidate_fixture_schedule","candidateKind":"fixture","displayName":"Schedule Fixture","sourceKind":"fixture","sourceId":"fixture_schedule","sourceRefs":[{"kind":"schedule","id":"sched_1","route":"/v1/schedules/sched_1"}],"environmentScope":"test","readinessStatus":"fully_replayable","readinessReasons":["fixture has evidence"],"limitations":[],"defaultReplayMode":"non_live","fixtureId":"fixture_schedule","createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z"}]}"##,
        ),
        (
            r##"schemas/api/create-replay-candidate.request.schema.json"##,
            r##"{"candidateId":"candidate_curated_run","candidateKind":"curated_work","displayName":"Curated Run","sourceKind":"run","sourceId":"run_curated","sourceRefs":[{"kind":"run","id":"run_curated","route":"/v1/runs/run_curated"}],"toolClasses":["daemon.inspection.read"],"environmentScope":"test","readinessStatus":"partially_replayable","readinessReasons":["curated run has captured summaries"],"limitations":["evidence-only replay"],"defaultReplayMode":"non_live","expectedComparisonSummary":{"runtime":"runtime captured","policy":"policy captured","evidence":"evidence captured"}}"##,
        ),
        (
            r##"schemas/api/create-replay-attempt.request.schema.json"##,
            r##"{"mode":"non_live","changeWindowLabel":"phase-33","baselineAttemptId":"attempt_base","safetyScope":{"mode":"non_live","description":"captured evidence only"}}"##,
        ),
        (
            r##"schemas/api/replay-attempt-resource.schema.json"##,
            r##"{"attemptId":"attempt_1","candidateId":"candidate_fixture_schedule","sourceRefs":[{"kind":"schedule","id":"sched_1","route":"/v1/schedules/sched_1"}],"environmentScope":"test","mode":"non_live","status":"completed","safetyScope":{"mode":"non_live"},"approvalHandling":"evidence_only","sideEffectHandling":"evidence_only","changeWindowLabel":"phase-33","evidenceRefs":[{"kind":"fixture_evidence","id":"schedule/evidence.json"}],"blockedReasons":[],"runtimeSummary":"runtime matched","policySummary":"policy matched","integrationSummary":"integration matched","deliverySummary":"delivery matched","evidenceSummary":"evidence matched","startedAt":"2026-04-24T10:00:00Z","completedAt":"2026-04-24T10:00:00Z","createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/replay-attempt-list.response.schema.json"##,
            r##"{"environmentScope":"test","items":[{"attemptId":"attempt_1","candidateId":"candidate_fixture_schedule","sourceRefs":[],"environmentScope":"test","mode":"non_live","status":"completed","safetyScope":{"mode":"non_live"},"approvalHandling":"evidence_only","sideEffectHandling":"evidence_only","evidenceRefs":[],"blockedReasons":[],"createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z"}]}"##,
        ),
        (
            r##"schemas/api/create-replay-comparison.request.schema.json"##,
            r##"{"baselineAttemptId":"attempt_base","changeWindowLabel":"phase-33"}"##,
        ),
        (
            r##"schemas/api/replay-drift-finding.schema.json"##,
            r##"{"findingId":"finding_1","comparisonId":"comparison_1","plane":"runtime","severity":"warning","summary":"runtime changed","baselineValue":"old","replayValue":"new","evidenceRefs":[{"kind":"fixture_evidence","id":"schedule/evidence.json"}],"recommendedAction":"Inspect evidence.","createdAt":"2026-04-24T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/replay-comparison-resource.schema.json"##,
            r##"{"comparisonId":"comparison_1","candidateId":"candidate_fixture_schedule","baselineRef":"attempt_base","attemptId":"attempt_1","environmentScope":"test","terminalStatus":"drifted","runtimeSummary":"runtime changed","policySummary":"policy matched","integrationSummary":"integration matched","deliverySummary":"delivery matched","evidenceSummary":"evidence matched","confidence":"medium","limitations":[],"driftFindings":[{"findingId":"finding_1","comparisonId":"comparison_1","plane":"runtime","severity":"warning","summary":"runtime changed","createdAt":"2026-04-24T10:00:00Z"}],"changeWindowLabel":"phase-33","generatedAt":"2026-04-24T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/replay-comparison-list.response.schema.json"##,
            r##"{"environmentScope":"test","items":[{"comparisonId":"comparison_1","candidateId":"candidate_fixture_schedule","baselineRef":"attempt_base","attemptId":"attempt_1","environmentScope":"test","terminalStatus":"matched","runtimeSummary":"runtime matched","policySummary":"policy matched","integrationSummary":"integration matched","deliverySummary":"delivery matched","evidenceSummary":"evidence matched","confidence":"high","limitations":[],"driftFindings":[],"generatedAt":"2026-04-24T10:00:00Z"}]}"##,
        ),
        (
            r##"schemas/api/replay-fixture-resource.schema.json"##,
            r##"{"fixtureId":"fixture_schedule","displayName":"Schedule Fixture","domainClass":"schedule","manifestPath":"daemon/internal/evaluation/testdata/fixtures/schedule-basic/manifest.json","sourceRefs":[{"kind":"schedule","id":"sched_1"}],"capturedEvidenceRefs":[{"kind":"fixture_evidence","id":"schedule/evidence.json"}],"assumptions":["captured evidence"],"limitations":[],"expectedReplayMode":"non_live","expectedComparisonSummary":{"runtime":"runtime matched","policy":"policy matched","integration":"integration matched","delivery":"delivery matched","evidence":"evidence matched"},"candidateId":"candidate_fixture_schedule","environmentScope":"test","createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/replay-fixture-list.response.schema.json"##,
            r##"{"environmentScope":"test","items":[{"fixtureId":"fixture_schedule","displayName":"Schedule Fixture","domainClass":"schedule","manifestPath":"daemon/internal/evaluation/testdata/fixtures/schedule-basic/manifest.json","sourceRefs":[],"capturedEvidenceRefs":[],"assumptions":[],"limitations":[],"expectedReplayMode":"non_live","expectedComparisonSummary":{"runtime":"runtime matched","evidence":"evidence matched"},"environmentScope":"test","createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z"}]}"##,
        ),
        (
            r##"schemas/events/evaluation-replay-started.event.schema.json"##,
            r##"{"eventId":"evt_1","sequence":1,"category":"evaluation","name":"evaluation.replay_started","occurredAt":"2026-04-24T10:00:00Z","scope":{},"resource":{"kind":"replay_attempt","id":"attempt_1"},"payload":{"candidateId":"candidate_1","attemptId":"attempt_1","mode":"non_live","environmentScope":"test"}}"##,
        ),
        (
            r##"schemas/events/evaluation-replay-completed.event.schema.json"##,
            r##"{"eventId":"evt_2","sequence":2,"category":"evaluation","name":"evaluation.replay_completed","occurredAt":"2026-04-24T10:00:00Z","scope":{},"resource":{"kind":"replay_attempt","id":"attempt_1"},"payload":{"candidateId":"candidate_1","attemptId":"attempt_1","mode":"non_live","status":"completed","environmentScope":"test"}}"##,
        ),
        (
            r##"schemas/events/evaluation-replay-blocked.event.schema.json"##,
            r##"{"eventId":"evt_3","sequence":3,"category":"evaluation","name":"evaluation.replay_blocked","occurredAt":"2026-04-24T10:00:00Z","scope":{},"resource":{"kind":"replay_attempt","id":"attempt_1"},"payload":{"candidateId":"candidate_1","attemptId":"attempt_1","mode":"non_live","status":"blocked","blockedReasons":["approval required"],"environmentScope":"test"}}"##,
        ),
        (
            r##"schemas/events/evaluation-replay-unreplayable.event.schema.json"##,
            r##"{"eventId":"evt_4","sequence":4,"category":"evaluation","name":"evaluation.replay_unreplayable","occurredAt":"2026-04-24T10:00:00Z","scope":{},"resource":{"kind":"replay_attempt","id":"attempt_1"},"payload":{"candidateId":"candidate_1","attemptId":"attempt_1","mode":"non_live","status":"unreplayable","limitations":["evidence expired"],"environmentScope":"test"}}"##,
        ),
        (
            r##"schemas/events/evaluation-replay-failed.event.schema.json"##,
            r##"{"eventId":"evt_5","sequence":5,"category":"evaluation","name":"evaluation.replay_failed","occurredAt":"2026-04-24T10:00:00Z","scope":{},"resource":{"kind":"replay_attempt","id":"attempt_1"},"payload":{"candidateId":"candidate_1","attemptId":"attempt_1","mode":"non_live","status":"failed","failureReason":"processing failed","environmentScope":"test"}}"##,
        ),
        (
            r##"schemas/events/evaluation-comparison-completed.event.schema.json"##,
            r##"{"eventId":"evt_6","sequence":6,"category":"evaluation","name":"evaluation.comparison_completed","occurredAt":"2026-04-24T10:00:00Z","scope":{},"resource":{"kind":"replay_comparison","id":"comparison_1"},"payload":{"candidateId":"candidate_1","attemptId":"attempt_1","comparisonId":"comparison_1","terminalStatus":"matched","driftPlanes":[],"environmentScope":"test"}}"##,
        ),
    ]
}

pub fn evaluation_product_foundation_fixtures() -> &'static [Fixture] {
    &[
        (
            r##"schemas/api/evaluation-product-pagination.schema.json"##,
            r##"{"applicationId":"retention_1","tenantId":"ten_eval","resourceKind":"discovered_candidate","resourceId":"candidate_1","dryRun":false,"outcome":"expired","affectedCount":1,"appliedAt":"2026-04-29T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/evaluation-discovery-policy-resource.schema.json"##,
            r##"{"policyId":"policy_1","tenantId":"ten_eval","enabled":true,"sourceKinds":["run"],"windowStart":"2026-04-29T09:00:00Z","windowEnd":"2026-04-29T10:00:00Z","maxInspectedRecords":10,"maxEmittedCandidates":2,"costBudget":5,"createdAt":"2026-04-29T10:00:00Z","updatedAt":"2026-04-29T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/evaluation-discovery-run-resource.schema.json"##,
            r##"{"discoveryRunId":"discovery_run_1","tenantId":"ten_eval","policyId":"policy_1","status":"partial","cursor":"cur_1","sourceKinds":["run"],"windowStart":"2026-04-29T09:00:00Z","windowEnd":"2026-04-29T10:00:00Z","maxInspectedRecords":10,"maxEmittedCandidates":2,"costBudget":5,"inspectedRecords":10,"emittedCandidates":1,"partialReason":"max_inspected_records","startedAt":"2026-04-29T10:00:00Z","updatedAt":"2026-04-29T10:01:00Z"}"##,
        ),
        (
            r##"schemas/api/evaluation-discovered-candidate-resource.schema.json"##,
            r##"{"discoveredCandidateId":"candidate_1","tenantId":"ten_eval","discoveryRunId":"discovery_run_1","sourceKind":"run","sourceId":"run_1","sourceRefs":[{"kind":"run","id":"run_1","route":"/v1/runs/run_1"}],"score":0.9,"scoreBand":"high","redactionStatus":"redacted","evidenceRef":"evidence_1","readinessStatus":"fully_replayable","suppressionState":"none","retentionState":"active","createdAt":"2026-04-29T10:00:00Z","updatedAt":"2026-04-29T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/evaluation-product-fixture-resource.schema.json"##,
            r##"{"fixture":{"fixtureId":"product_fixture_1","tenantId":"ten_eval","displayName":"Schedule Product Fixture","domainClass":"schedule","sourceKind":"discovered_candidate","sourceCandidateId":"candidate_1","currentRevisionId":"revision_1","reviewState":"draft","suppressionState":"none","retentionState":"active","createdBy":"prn_eval","createdAt":"2026-04-29T10:00:00Z","updatedAt":"2026-04-29T10:00:00Z"},"revision":{"revisionId":"revision_1","fixtureId":"product_fixture_1","tenantId":"ten_eval","revisionNumber":1,"fixturePayload":{"goal":"safe"},"sourceEvidenceRefs":["evidence_1"],"redactionStatus":"redacted","createdBy":"prn_eval","createdAt":"2026-04-29T10:00:00Z"}}"##,
        ),
        (
            r##"schemas/api/evaluation-campaign-resource.schema.json"##,
            r##"{"attemptGroupId":"attempt_group_1","campaignId":"campaign_1","campaignItemId":"campaign_item_1","tenantId":"ten_eval","replayAttemptIds":["attempt_1"],"comparisonIds":["comparison_1"],"liveValidationIds":["ledger_1"],"status":"completed","driftCount":1,"failureCount":0,"unsupportedCount":0,"operatorActionNeededCount":1,"summary":"1 drift","createdAt":"2026-04-29T10:00:00Z","updatedAt":"2026-04-29T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/evaluation-dashboard-resource.schema.json"##,
            r##"{"projectionId":"projection_1","tenantId":"ten_eval","windowStart":"2026-04-29T09:00:00Z","windowEnd":"2026-04-29T10:00:00Z","campaignStatusCounts":{"completed":1},"driftSummary":{"total":1},"failureSummary":{"total":0},"unsupportedSummary":{"total":0},"operatorActionNeededSummary":{"total":1},"liveValidationSummary":{"linked":1},"generatedAt":"2026-04-29T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/evaluation-tool-call-inspection-resource.schema.json"##,
            r##"{"inspectionId":"inspection_1","tenantId":"ten_eval","campaignId":"campaign_1","campaignItemId":"campaign_item_1","toolCallRef":"tool_call_1","originalEvidenceRef":"original_1","nonLiveReplayEvidenceRef":"replay_1","liveValidationLedgerRefs":["ledger_1"],"classification":"live_validation_completed","diffSummary":"redacted matched","redactionStatus":"redacted","createdAt":"2026-04-29T10:00:00Z","updatedAt":"2026-04-29T10:00:00Z"}"##,
        ),
        (
            r##"schemas/events/evaluation-product-audit-recorded.event.schema.json"##,
            r##"{"eventId":"evt_eval_product_1","sequence":1,"category":"evaluation","name":"evaluation.product_retention_applied","occurredAt":"2026-04-29T10:00:00Z","scope":{},"resource":{"kind":"retention_application","id":"retention_1"},"payload":{"tenantId":"ten_eval","actorId":"prn_eval","action":"retention.apply","targetKind":"discovered_candidate","targetId":"candidate_1","outcome":"retention_applied","retentionApplicationId":"retention_1","createdAt":"2026-04-29T10:00:00Z"}}"##,
        ),
        (
            r##"schemas/events/evaluation-discovery-started.event.schema.json"##,
            r##"{"eventId":"evt_eval_discovery_1","sequence":1,"category":"evaluation","name":"evaluation.discovery_started","occurredAt":"2026-04-29T10:00:00Z","scope":{},"resource":{"kind":"discovery_run","id":"discovery_run_1"},"payload":{"tenantId":"ten_eval","policyId":"policy_1","discoveryRunId":"discovery_run_1","status":"queued"}}"##,
        ),
        (
            r##"schemas/events/evaluation-fixture-created.event.schema.json"##,
            r##"{"eventId":"evt_eval_fixture_1","sequence":1,"category":"evaluation","name":"evaluation.fixture.created","occurredAt":"2026-04-29T10:00:00Z","scope":{},"resource":{"kind":"product_fixture","id":"product_fixture_1"},"payload":{"tenantId":"ten_eval","actorId":"prn_eval","fixtureId":"product_fixture_1","revisionId":"revision_1","sourceCandidateId":"candidate_1","sourceEvidenceRefs":["evidence_1"],"outcome":"created"}}"##,
        ),
        (
            r##"schemas/events/evaluation-campaign-created.event.schema.json"##,
            r##"{"eventId":"evt_eval_campaign_1","sequence":1,"category":"evaluation","name":"evaluation.campaign.created","occurredAt":"2026-04-29T10:00:00Z","scope":{},"resource":{"kind":"campaign","id":"campaign_1"},"payload":{"tenantId":"ten_eval","actorId":"prn_eval","campaignId":"campaign_1","status":"queued","outcome":"created"}}"##,
        ),
        (
            r##"schemas/events/evaluation-dashboard-projection-generated.event.schema.json"##,
            r##"{"eventId":"evt_eval_dashboard_1","sequence":1,"category":"evaluation","name":"evaluation.dashboard.projection_generated","occurredAt":"2026-04-29T10:00:00Z","scope":{},"resource":{"kind":"dashboard_projection","id":"projection_1"},"payload":{"tenantId":"ten_eval","projectionId":"projection_1","windowStart":"2026-04-29T09:00:00Z","windowEnd":"2026-04-29T10:00:00Z","generatedAt":"2026-04-29T10:00:00Z","outcome":"generated"}}"##,
        ),
        (
            r##"schemas/events/evaluation-tool-call-inspection-generated.event.schema.json"##,
            r##"{"eventId":"evt_eval_inspection_1","sequence":1,"category":"evaluation","name":"evaluation.tool_call_inspection.generated","occurredAt":"2026-04-29T10:00:00Z","scope":{},"resource":{"kind":"tool_call_inspection","id":"inspection_1"},"payload":{"tenantId":"ten_eval","inspectionId":"inspection_1","campaignId":"campaign_1","campaignItemId":"campaign_item_1","classification":"matched","redactionStatus":"clean","outcome":"generated"}}"##,
        ),
    ]
}

pub fn hosted_credential_contract_fixtures() -> &'static [Fixture] {
    &[
        (
            r##"schemas/api/tenant-secret-resource.schema.json"##,
            r##"{"secretId":"sec_1","tenantId":"ten_1","secretRef":"provider/api-key","displayName":"Provider API key","status":"active","activeVersionId":"secver_1","createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z","rotatedAt":"2026-04-24T10:00:00Z","document":{"owner":"ops"},"secretRefs":[{"secretRef":"provider/api-key","resolution":"unavailable","redactionRule":"secret_ref_only"}]}"##,
        ),
        (
            r##"schemas/api/tenant-secret-list.response.schema.json"##,
            r##"{"items":[{"secretId":"sec_1","tenantId":"ten_1","secretRef":"provider/api-key","status":"active","createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z"}]}"##,
        ),
        (
            r##"schemas/api/connector-resource.schema.json"##,
            r##"{"tenantId":"ten_1","connectorId":"discord","kind":"discord","displayName":"Discord","status":"healthy","secretRefs":["discord/token"],"secretSummary":[{"secretRef":"discord/token","resolution":"unavailable","redactionRule":"secret_ref_only"}],"failureCount":0,"restartCount":0,"backoffSeconds":0,"createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/mcp-server-resource.schema.json"##,
            r##"{"tenantId":"ten_1","serverId":"srv_1","displayName":"GitHub","source":"api","enabled":true,"sandboxProfileId":"default","declarationId":"decl_1","declaration":{"executionMode":"subprocess","active":true},"transportKind":"stdio","command":"node","args":["server.js"],"secretRefs":["github/token"],"autoRestart":true,"createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z","state":{"serverId":"srv_1","status":"healthy","failureCount":0,"restartCount":0,"updatedAt":"2026-04-24T10:00:00Z"},"transportConfigSummary":"stdio:node","secretSummary":[{"consumerId":"srv_1","secretRef":"github/token","environmentScope":"test","resolution":"unavailable","redactionRule":"secret_ref_only"}],"toolCount":0}"##,
        ),
        (
            r##"schemas/api/create-tenant-secret.request.schema.json"##,
            r##"{"secretRef":"provider/api-key","displayName":"Provider API key","value":"fake-secret","document":{"owner":"ops"}}"##,
        ),
        (
            r##"schemas/api/rotate-tenant-secret.request.schema.json"##,
            r##"{"value":"fake-new-secret"}"##,
        ),
        (
            r##"schemas/api/rotate-tenant-secret.response.schema.json"##,
            r##"{"secret":{"secretId":"sec_1","tenantId":"ten_1","secretRef":"provider/api-key","status":"active","activeVersionId":"secver_2","createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:01:00Z","rotatedAt":"2026-04-24T10:01:00Z"}}"##,
        ),
        (
            r##"schemas/api/provider-auth-state.response.schema.json"##,
            r##"{"auth":{"tenantId":"ten_1","providerId":"codex","family":"codex_cli","authMode":"local_cli_bridge","status":"authenticated","cliAvailable":true,"accountLabel":"operator","accountId":"acct_1","lastCheckedAt":"2026-04-24T10:00:00Z","secretRefs":[{"secretRef":"provider/codex","resolution":"unavailable","redactionRule":"secret_ref_only"}]}}"##,
        ),
        (
            r##"schemas/events/credential-audit-recorded.event.schema.json"##,
            r##"{"eventId":"evt_credential_1","sequence":1,"category":"tenant","name":"credential.audit_recorded","occurredAt":"2026-04-24T10:00:00Z","scope":{},"resource":{"kind":"tenant_secret","id":"sec_1"},"payload":{"tenantId":"ten_1","principalId":"prn_1","resourceKind":"tenant_secret","resourceId":"sec_1","action":"secret.rotate","outcome":"succeeded","reasonCode":"credential_rotated","secretRef":"provider/api-key","secretVersionId":"secver_2","secretRefCount":1,"secretRefs":[{"secretRef":"provider/api-key","resolution":"unavailable","redactionRule":"secret_ref_only"}]}}"##,
        ),
        (
            r##"schemas/api/tenant-audit-event-resource.schema.json"##,
            r##"{"auditEventId":"audit_credential_1","eventKind":"credential.audit_recorded","tenantId":"ten_1","principalId":"prn_1","outcome":"succeeded","reasonCode":"credential_rotated","createdAt":"2026-04-24T10:00:00Z","document":{"resourceKind":"tenant_secret","resourceId":"sec_1","action":"secret.rotate","secretRef":"provider/api-key","secretVersionId":"secver_2","secretRefCount":1,"secretRefs":[{"secretRef":"provider/api-key","resolution":"unavailable","redactionRule":"secret_ref_only"}]}}"##,
        ),
    ]
}

pub fn integration_contract_fixtures() -> &'static [Fixture] {
    &[
        (
            r##"schemas/api/integration-binding-summary.schema.json"##,
            r##"{"integrationId":"calendar-a","domainKind":"calendar","displayName":"Calendar A","accountKey":"acct_calendar","canonicalDefault":true,"readinessAtInvocation":"degraded","backendKind":"fake_local","secretResolution":"resolved","environmentScope":"test","capturedAt":"2026-04-22T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/integration-resource.schema.json"##,
            r##"{"integrationId":"calendar-a","domainKind":"calendar","displayName":"Calendar A","environmentScope":"test","readinessStatus":"healthy","authState":"authorized","healthState":"healthy","readinessReason":"probe passed","requiredOperatorAction":"none","canonicalDefault":true,"accountBinding":{"accountKey":"acct_calendar","accountLabel":"Primary Calendar"},"backendBinding":{"backendKind":"fake_local","backendRefId":"calendar-fake","backendDisplayName":"Calendar Fake","sourceKind":"fake_local","supportsProbeRead":true,"supportsProbeMutation":true},"provenance":{"secretResolution":"resolved","secretMaterialPresent":true,"environmentScope":"test","backedBy":"fake_local"},"createdAt":"2026-04-22T10:00:00Z","updatedAt":"2026-04-22T10:01:00Z","lastReadyAt":"2026-04-22T10:01:00Z","lastTransitionAt":"2026-04-22T10:01:00Z"}"##,
        ),
        (
            r##"schemas/api/integration-list.response.schema.json"##,
            r##"{"items":[{"integrationId":"calendar-a","domainKind":"calendar","displayName":"Calendar A","environmentScope":"test","readinessStatus":"healthy","canonicalDefault":true,"backendBinding":{"backendKind":"fake_local"},"createdAt":"2026-04-22T10:00:00Z","updatedAt":"2026-04-22T10:01:00Z","lastTransitionAt":"2026-04-22T10:01:00Z"}]}"##,
        ),
        (
            r##"schemas/api/integration-probe.response.schema.json"##,
            r##"{"runId":"run_1","stepId":"step_1","toolCallId":"tool_call_1","status":"completed","integrationBindings":[{"integrationId":"calendar-a","domainKind":"calendar","displayName":"Calendar A","accountKey":"acct_calendar","canonicalDefault":true,"readinessAtInvocation":"degraded","backendKind":"fake_local","secretResolution":"resolved","environmentScope":"test","capturedAt":"2026-04-22T10:00:00Z"}],"approval":{"approvalId":"approval_1","action":"integration.probe.mutate","resourceKind":"integration","resourceId":"calendar-a","reason":"mutation probe","status":"pending","createdAt":"2026-04-22T10:00:00Z","updatedAt":"2026-04-22T10:00:00Z","integrationBindings":[{"integrationId":"calendar-a","domainKind":"calendar","displayName":"Calendar A","accountKey":"acct_calendar","canonicalDefault":true,"readinessAtInvocation":"degraded","backendKind":"fake_local","secretResolution":"resolved","environmentScope":"test","capturedAt":"2026-04-22T10:00:00Z"}]}}"##,
        ),
        (
            r##"schemas/events/integration-registered.event.schema.json"##,
            r##"{"eventId":"evt_integration_1","sequence":1,"category":"integration","name":"integration.registered","occurredAt":"2026-04-22T10:00:00Z","scope":{},"resource":{"kind":"integration","id":"calendar-a"},"payload":{"integrationId":"calendar-a","domainKind":"calendar","displayName":"Calendar A","environmentScope":"test","readinessStatus":"not_configured","canonicalDefault":true,"backendKind":"fake_local","accountKey":"acct_calendar"}}"##,
        ),
        (
            r##"schemas/events/integration-updated.event.schema.json"##,
            r##"{"eventId":"evt_integration_2","sequence":2,"category":"integration","name":"integration.updated","occurredAt":"2026-04-22T10:01:00Z","scope":{},"resource":{"kind":"integration","id":"calendar-a"},"payload":{"integrationId":"calendar-a","domainKind":"calendar","displayName":"Calendar A","environmentScope":"test","readinessStatus":"healthy","canonicalDefault":true,"backendKind":"fake_local","accountKey":"acct_calendar"}}"##,
        ),
        (
            r##"schemas/events/integration-readiness-changed.event.schema.json"##,
            r##"{"eventId":"evt_integration_3","sequence":3,"category":"integration","name":"integration.readiness_changed","occurredAt":"2026-04-22T10:01:00Z","scope":{},"resource":{"kind":"integration","id":"calendar-a"},"payload":{"integrationId":"calendar-a","readinessStatus":"healthy","authState":"authorized","healthState":"healthy","reason":"probe passed","requiredOperatorAction":"none","accountKey":"acct_calendar","backendKind":"fake_local"}}"##,
        ),
        (
            r##"schemas/events/integration-default-changed.event.schema.json"##,
            r##"{"eventId":"evt_integration_4","sequence":4,"category":"integration","name":"integration.default_changed","occurredAt":"2026-04-22T10:02:00Z","scope":{},"resource":{"kind":"integration","id":"calendar-b"},"payload":{"integrationId":"calendar-b","domainKind":"calendar","environmentScope":"test","accountKey":"acct_calendar","canonicalDefault":true}}"##,
        ),
    ]
}

pub fn integration_diagnostic_contract_fixtures() -> &'static [Fixture] {
    &[
        (
            r##"schemas/api/integration-diagnostic-reason-code.schema.json"##,
            r##"{"reasonCode":"healthy","category":"healthy","defaultRetrySafety":"no_action_needed","defaultRemediationOwner":"none_required","userMessageKey":"integration.diagnostic.healthy","operatorMessageKey":"integration.diagnostic.healthy","supportedDomains":["calendar"]}"##,
        ),
        (
            r##"schemas/api/integration-diagnostic-result.schema.json"##,
            r##"{"diagnosticResultId":"diag_result_1","tenantId":"ten_r42","integrationId":"integration_calendar_feishu","integrationAccountId":"acct_redacted","domainKind":"calendar","providerKind":"feishu_lark","capability":"calendar.read","status":"healthy","reasonCode":"healthy","remediationOwner":"none_required","remediationHint":"No action needed.","retrySafety":"no_action_needed","checkedAt":"2026-04-30T10:00:00Z","staleAfter":"2026-04-30T10:15:00Z","freshnessState":"fresh","runId":"diag_run_1","redactionStatus":"redacted","evidenceSummary":"provider ready","retentionExpiresAt":"2026-07-29T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/integration-diagnostic-run.schema.json"##,
            r##"{"diagnosticRunId":"diag_run_1","tenantId":"ten_r42","integrationId":"integration_calendar_feishu","integrationAccountId":"acct_redacted","domainKind":"calendar","providerKind":"feishu_lark","status":"completed","trigger":"operator_inspection","requestedBy":"operator_r42","startedAt":"2026-04-30T10:00:00Z","completedAt":"2026-04-30T10:01:00Z","checkedCapabilities":["calendar.read"],"resultIds":["diag_result_1"],"redactionStatus":"redacted","retentionExpiresAt":"2026-07-29T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/integration-diagnostic-list.response.schema.json"##,
            r##"{"integrationId":"integration_calendar_feishu","tenantId":"ten_r42","freshnessSummary":"latest diagnostic state","items":[{"diagnosticResultId":"diag_result_1","tenantId":"ten_r42","integrationId":"integration_calendar_feishu","domainKind":"calendar","providerKind":"feishu_lark","capability":"calendar.read","status":"healthy","reasonCode":"healthy","remediationOwner":"none_required","retrySafety":"no_action_needed","checkedAt":"2026-04-30T10:00:00Z","staleAfter":"2026-04-30T10:15:00Z","freshnessState":"fresh","redactionStatus":"redacted","retentionExpiresAt":"2026-07-29T10:00:00Z"}]}"##,
        ),
        (
            r##"schemas/api/create-integration-diagnostic-run.request.schema.json"##,
            r##"{"capabilities":["calendar.read"],"forceRefresh":true,"clientKey":"client_r42","reason":"operator inspection"}"##,
        ),
        (
            r##"schemas/api/create-integration-diagnostic-smoke.request.schema.json"##,
            r##"{"reportId":"smoke_feishu_probe","integrationId":"integration_calendar_feishu","probes":[{"domainKind":"calendar","probeAction":"calendar.read","safeCredentialsAvailable":true,"tenantApprovalAvailable":true,"providerAvailable":true,"supported":true,"readOnlyOrReversible":true,"providerEvidence":{"code":"scope_not_granted"}}]}"##,
        ),
        (
            r##"schemas/api/integration-diagnostic-failure-projection.schema.json"##,
            r##"{"reasonCode":"tenant_approval_pending","remediationOwner":"tenant_admin","remediationHint":"Ask a tenant administrator to approve the provider application.","retrySafety":"blocked","freshnessState":"fresh","checkedAt":"2026-04-30T10:00:00Z","redactionStatus":"redacted"}"##,
        ),
        (
            r##"schemas/api/smoke-probe-outcome.schema.json"##,
            r##"{"probeOutcomeId":"probe_1","smokeReportId":"smoke_1","tenantId":"ten_r42","integrationId":"integration_calendar_feishu","integrationAccountId":"acct_redacted","domainKind":"calendar","providerKind":"feishu_lark","probeAction":"calendar.read","result":"passed","reasonCode":"healthy","remediationHint":"No action needed.","retrySafety":"no_action_needed","artifactRefs":["artifact_1"],"checkedAt":"2026-04-30T10:01:00Z","redactionStatus":"redacted","retentionExpiresAt":"2026-07-29T10:01:00Z"}"##,
        ),
        (
            r##"schemas/api/smoke-matrix-report.schema.json"##,
            r##"{"smokeReportId":"smoke_1","tenantId":"ten_r42","reportKind":"diagnostic","requestedBy":"operator_r42","status":"completed","startedAt":"2026-04-30T10:00:00Z","completedAt":"2026-04-30T10:03:00Z","domainSummary":{"calendar":"passed","mail":"blocked"},"artifactRefs":["artifact_1"],"retentionExpiresAt":"2026-07-29T10:03:00Z"}"##,
        ),
        (
            r##"schemas/events/integration-diagnostic-run-started.event.schema.json"##,
            r##"{"eventId":"evt_diag_1","sequence":1,"category":"integration","name":"integration_diagnostic.run_started","occurredAt":"2026-04-30T10:00:00Z","scope":{},"resource":{"kind":"integration_diagnostic_run","id":"diag_run_1"},"payload":{"tenantId":"ten_r42","diagnosticRunId":"diag_run_1","integrationId":"integration_calendar_feishu","requestedBy":"operator_r42","status":"running"}}"##,
        ),
        (
            r##"schemas/events/integration-diagnostic-run-completed.event.schema.json"##,
            r##"{"eventId":"evt_diag_2","sequence":2,"category":"integration","name":"integration_diagnostic.run_completed","occurredAt":"2026-04-30T10:01:00Z","scope":{},"resource":{"kind":"integration_diagnostic_run","id":"diag_run_1"},"payload":{"tenantId":"ten_r42","diagnosticRunId":"diag_run_1","integrationId":"integration_calendar_feishu","status":"completed","resultIds":["diag_result_1"],"redactionStatus":"redacted"}}"##,
        ),
        (
            r##"schemas/events/integration-diagnostic-state-changed.event.schema.json"##,
            r##"{"eventId":"evt_diag_3","sequence":3,"category":"integration","name":"integration_diagnostic.state_changed","occurredAt":"2026-04-30T10:01:00Z","scope":{},"resource":{"kind":"integration_diagnostic_result","id":"diag_result_1"},"payload":{"tenantId":"ten_r42","diagnosticResultId":"diag_result_1","integrationId":"integration_calendar_feishu","previousStatus":"unknown","status":"healthy","reasonCode":"healthy","remediationOwner":"none_required"}}"##,
        ),
        (
            r##"schemas/events/integration-diagnostic-redaction-failed.event.schema.json"##,
            r##"{"eventId":"evt_diag_4","sequence":4,"category":"integration","name":"integration_diagnostic.redaction_failed_closed","occurredAt":"2026-04-30T10:02:00Z","scope":{},"resource":{"kind":"integration_diagnostic_result","id":"diag_result_2"},"payload":{"tenantId":"ten_r42","targetKind":"diagnostic_result","targetId":"diag_result_2","reasonCode":"redaction_failed_closed","redactionStatus":"failed_closed"}}"##,
        ),
        (
            r##"schemas/events/integration-diagnostic-smoke-completed.event.schema.json"##,
            r##"{"eventId":"evt_diag_5","sequence":5,"category":"integration","name":"integration_diagnostic.smoke_completed","occurredAt":"2026-04-30T10:03:00Z","scope":{},"resource":{"kind":"integration_diagnostic_smoke_report","id":"smoke_1"},"payload":{"tenantId":"ten_r42","smokeReportId":"smoke_1","status":"completed","domainSummary":{"calendar":"passed"},"artifactRefs":["artifact_1"]}}"##,
        ),
        (
            r##"schemas/events/integration-diagnostic-retention-applied.event.schema.json"##,
            r##"{"eventId":"evt_diag_6","sequence":6,"category":"integration","name":"integration_diagnostic.retention_applied","occurredAt":"2026-04-30T10:04:00Z","scope":{},"resource":{"kind":"integration_diagnostic_retention","id":"retention_1"},"payload":{"tenantId":"ten_r42","targetKind":"diagnostic_run","targetId":"diag_run_1","retentionState":"active","effectiveExpiresAt":"2026-07-29T10:00:00Z"}}"##,
        ),
    ]
}

pub fn live_validation_contract_fixtures() -> &'static [Fixture] {
    &[
        (
            r##"schemas/api/live-validation-attempt-resource.schema.json"##,
            r##"{"validationId":"lv_1","tenantId":"ten_1","candidateId":"candidate_1","requestedBy":"prn_1","environmentScope":"test","requestedScope":{"scopeId":"scope_1","validationId":"lv_1","includedToolClasses":["daemon.inspection.read"],"approvalMode":"scope_level","declaredBy":"prn_1","declaredAt":"2026-04-29T10:00:00Z"},"status":"queued","permissionDecision":{"allowed":true,"checkedAt":"2026-04-29T10:00:00Z"},"quotaDecision":{"allowed":true,"checkedAt":"2026-04-29T10:00:00Z"},"killSwitchDecision":{"allowed":true,"checkedAt":"2026-04-29T10:00:00Z"},"approvalSummary":{"required":1,"approved":1,"denied":0,"expired":0,"pending":0},"ledgerSummary":{"attempted":1,"completed":1},"createdAt":"2026-04-29T10:00:00Z","updatedAt":"2026-04-29T10:00:01Z"}"##,
        ),
        (
            r##"schemas/api/live-validation-attempt-list.response.schema.json"##,
            r##"{"tenantId":"ten_1","environmentScope":"test","items":[{"validationId":"lv_1","tenantId":"ten_1","candidateId":"candidate_1","requestedBy":"prn_1","environmentScope":"test","requestedScope":{"scopeId":"scope_1","validationId":"lv_1","approvalMode":"scope_level","declaredBy":"prn_1","declaredAt":"2026-04-29T10:00:00Z"},"status":"queued","permissionDecision":{"allowed":true,"checkedAt":"2026-04-29T10:00:00Z"},"quotaDecision":{"allowed":true,"checkedAt":"2026-04-29T10:00:00Z"},"killSwitchDecision":{"allowed":true,"checkedAt":"2026-04-29T10:00:00Z"},"approvalSummary":{"required":1,"approved":1,"denied":0,"expired":0,"pending":0},"ledgerSummary":{},"createdAt":"2026-04-29T10:00:00Z","updatedAt":"2026-04-29T10:00:01Z"}]}"##,
        ),
        (
            r##"schemas/api/live-validation-denial.schema.json"##,
            r##"{"reasonCode":"live_validation.permission_denied","gate":"permission","message":"Missing live validation permission.","validationId":"lv_1","checkedAt":"2026-04-29T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/live-validation-ledger-resource.schema.json"##,
            r##"{"ledgerEntryId":"ledger_1","validationId":"lv_1","tenantId":"ten_1","candidateId":"candidate_1","sourceRef":"tool_call_1","toolClass":"daemon.inspection.read","safetyClass":"read_only","actionRef":"action_1","outcome":"completed","attemptedAt":"2026-04-29T10:00:00Z","completedAt":"2026-04-29T10:00:01Z","updatedAt":"2026-04-29T10:00:01Z","evidenceRefs":["event_1"],"retryCount":0,"ambiguousCommit":false}"##,
        ),
        (
            r##"schemas/api/live-validation-ledger-list.response.schema.json"##,
            r##"{"validationId":"lv_1","tenantId":"ten_1","items":[{"ledgerEntryId":"ledger_1","validationId":"lv_1","tenantId":"ten_1","candidateId":"candidate_1","sourceRef":"tool_call_1","toolClass":"daemon.inspection.read","safetyClass":"read_only","actionRef":"action_1","outcome":"completed","updatedAt":"2026-04-29T10:00:01Z","retryCount":0,"ambiguousCommit":false}]}"##,
        ),
        (
            r##"schemas/api/live-validation-support-matrix-resource.schema.json"##,
            r##"{"toolClass":"mail.send","safetyClass":"non_idempotent_mutation","permission":"live_validation.execute","approval":"per_action","approvalAction":"live_validation.approve","idempotency":"message idempotency key where supported","retryPolicy":"no_retry","ambiguousCommitBehavior":"operator_action_needed","compensation":"manual_confirmation","ledgerEvents":["attempted","completed","failed","aborted","denied","operator_action_needed"],"testCase":"fake mail send ambiguous commit test","version":"v1"}"##,
        ),
        (
            r##"schemas/api/live-validation-support-matrix.response.schema.json"##,
            r##"{"environmentScope":"test","version":"v1","items":[{"toolClass":"mcp.tool_call","safetyClass":"unsupported","approval":"unsupported","idempotency":"not available","retryPolicy":"no_retry","ambiguousCommitBehavior":"unsupported validation state","compensation":"unsupported","ledgerEvents":["skipped","denied"],"testCase":"MCP unsupported completeness test","version":"v1"}]}"##,
        ),
        (
            r##"schemas/api/live-validation-kill-switch-resource.schema.json"##,
            r##"{"killSwitchId":"kill_1","scope":"tenant","tenantId":"ten_1","enabled":true,"reason":"operator containment","changedBy":"prn_owner","changedAt":"2026-04-29T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/live-validation-reconciliation-resource.schema.json"##,
            r##"{"reconciliationId":"rec_1","ambiguousCommitId":"amb_1","tenantId":"ten_1","resolvedBy":"prn_owner","resolution":"confirmed_committed","reason":"provider state verified","evidenceRefs":["provider:event_1"],"resolvedAt":"2026-04-29T10:10:00Z"}"##,
        ),
        (
            r##"schemas/api/live-validation-retention-resource.schema.json"##,
            r##"{"policyId":"ret_1","tenantId":"ten_1","appliesTo":"all","mode":"indefinite","createdByPrincipalId":"prn_owner","createdAt":"2026-04-29T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/live-validation-comparison-resource.schema.json"##,
            r##"{"comparisonId":"cmp_1","validationId":"lv_1","candidateId":"candidate_1","baselineRef":"attempt_1","terminalStatus":"matched","ledgerSummary":{"completed":1},"unsupportedClasses":[],"denials":[],"ambiguousCommits":[],"driftFindings":[],"generatedAt":"2026-04-29T10:01:00Z"}"##,
        ),
        (
            r##"schemas/api/resolve-live-validation-reconciliation.request.schema.json"##,
            r##"{"resolution":"confirmed_committed","reason":"provider state verified","evidenceRefs":["provider:event_1"]}"##,
        ),
        (
            r##"schemas/api/create-live-validation-comparison.response.schema.json"##,
            r##"{"comparisonId":"cmp_1","validationId":"lv_1","candidateId":"candidate_1","baselineRef":"attempt_1","terminalStatus":"matched","ledgerSummary":{"completed":1},"generatedAt":"2026-04-29T10:01:00Z"}"##,
        ),
        (
            r##"schemas/api/update-live-validation-kill-switch.request.schema.json"##,
            r##"{"scope":"tenant","tenantId":"ten_1","enabled":true,"reason":"containment"}"##,
        ),
        (
            r##"schemas/api/create-live-validation.request.schema.json"##,
            r##"{"validationId":"lv_1","candidateId":"candidate_1","candidateToolClasses":["daemon.inspection.read"],"requestedScope":{"scopeId":"scope_1","validationId":"lv_1","includedToolClasses":["daemon.inspection.read"],"approvalMode":"scope_level","declaredBy":"prn_1","declaredAt":"2026-04-29T10:00:00Z"}}"##,
        ),
        (
            r##"schemas/api/create-live-validation.response.schema.json"##,
            r##"{"attempt":{"validationId":"lv_1","candidateId":"candidate_1","requestedBy":"prn_1","environmentScope":"test","requestedScope":{"scopeId":"scope_1","validationId":"lv_1","approvalMode":"scope_level","declaredBy":"prn_1","declaredAt":"2026-04-29T10:00:00Z"},"status":"awaiting_approval","permissionDecision":{"allowed":true,"checkedAt":"2026-04-29T10:00:00Z"},"quotaDecision":{"allowed":true,"checkedAt":"2026-04-29T10:00:00Z"},"killSwitchDecision":{"allowed":true,"checkedAt":"2026-04-29T10:00:00Z"},"approvalSummary":{"required":1,"approved":0,"denied":0,"expired":0,"pending":1},"ledgerSummary":{},"createdAt":"2026-04-29T10:00:00Z","updatedAt":"2026-04-29T10:00:00Z"}}"##,
        ),
        (
            r##"schemas/events/live-validation-started.event.schema.json"##,
            r##"{"eventId":"evt_lv_1","sequence":1,"category":"evaluation","name":"live_validation.started","occurredAt":"2026-04-29T10:00:00Z","scope":{},"resource":{"kind":"live_validation","id":"lv_1"},"payload":{"validationId":"lv_1","tenantId":"ten_1","candidateId":"candidate_1","environmentScope":"test","status":"queued"}}"##,
        ),
        (
            r##"schemas/events/live-validation-blocked.event.schema.json"##,
            r##"{"eventId":"evt_lv_2","sequence":2,"category":"evaluation","name":"live_validation.blocked","occurredAt":"2026-04-29T10:00:00Z","scope":{},"resource":{"kind":"live_validation","id":"lv_1"},"payload":{"validationId":"lv_1","tenantId":"ten_1","candidateId":"candidate_1","gate":"permission","reasonCode":"live_validation.permission_denied","denials":[{"reasonCode":"live_validation.permission_denied","gate":"permission","message":"Missing live validation permission."}]}}"##,
        ),
        (
            r##"schemas/events/live-validation-awaiting-approval.event.schema.json"##,
            r##"{"eventId":"evt_lv_awaiting","sequence":3,"category":"evaluation","name":"live_validation.awaiting_approval","occurredAt":"2026-04-29T10:00:00Z","scope":{},"resource":{"kind":"live_validation","id":"lv_1"},"payload":{"validationId":"lv_1","tenantId":"ten_1","candidateId":"candidate_1","environmentScope":"test","status":"awaiting_approval"}}"##,
        ),
        (
            r##"schemas/events/live-validation-side-effect-recorded.event.schema.json"##,
            r##"{"eventId":"evt_lv_3","sequence":3,"category":"evaluation","name":"live_validation.side_effect_recorded","occurredAt":"2026-04-29T10:00:01Z","scope":{},"resource":{"kind":"live_validation_ledger_entry","id":"ledger_1"},"payload":{"validationId":"lv_1","tenantId":"ten_1","ledgerEntryId":"ledger_1","toolClass":"daemon.inspection.read","actionRef":"action_1","outcome":"completed","ambiguousCommit":false}}"##,
        ),
        (
            r##"schemas/events/live-validation-reconciliation-resolved.event.schema.json"##,
            r##"{"eventId":"evt_lv_4","sequence":4,"category":"evaluation","name":"live_validation.reconciliation_resolved","occurredAt":"2026-04-29T10:10:00Z","scope":{},"resource":{"kind":"live_validation_reconciliation","id":"rec_1"},"payload":{"validationId":"lv_1","tenantId":"ten_1","ambiguousCommitId":"amb_1","reconciliationId":"rec_1","resolution":"confirmed_committed","resolvedBy":"prn_owner"}}"##,
        ),
        (
            r##"schemas/events/live-validation-operator-action-needed.event.schema.json"##,
            r##"{"eventId":"evt_lv_5","sequence":5,"category":"evaluation","name":"live_validation.operator_action_needed","occurredAt":"2026-04-29T10:10:00Z","scope":{},"resource":{"kind":"live_validation_ledger_entry","id":"ledger_1"},"payload":{"validationId":"lv_1","tenantId":"ten_1","ledgerEntryId":"ledger_1","reasonCode":"live_validation.ambiguous_commit"}}"##,
        ),
        (
            r##"schemas/events/live-validation-completed.event.schema.json"##,
            r##"{"eventId":"evt_lv_6","sequence":6,"category":"evaluation","name":"live_validation.completed","occurredAt":"2026-04-29T10:10:00Z","scope":{},"resource":{"kind":"live_validation","id":"lv_1"},"payload":{"validationId":"lv_1","tenantId":"ten_1","status":"completed"}}"##,
        ),
        (
            r##"schemas/events/live-validation-comparison-completed.event.schema.json"##,
            r##"{"eventId":"evt_lv_7","sequence":7,"category":"evaluation","name":"live_validation.comparison_completed","occurredAt":"2026-04-29T10:10:00Z","scope":{},"resource":{"kind":"live_validation_comparison","id":"cmp_1"},"payload":{"validationId":"lv_1","comparisonId":"cmp_1","terminalStatus":"matched"}}"##,
        ),
        (
            r##"schemas/events/live-validation-aborted.event.schema.json"##,
            r##"{"eventId":"evt_lv_8","sequence":8,"category":"evaluation","name":"live_validation.aborted","occurredAt":"2026-04-29T10:10:00Z","scope":{},"resource":{"kind":"live_validation","id":"lv_1"},"payload":{"validationId":"lv_1","tenantId":"ten_1","status":"aborted","reasonCode":"live_validation.kill_switch_aborted"}}"##,
        ),
    ]
}

pub fn mail_contract_fixtures() -> &'static [Fixture] {
    &[
        (
            r##"schemas/api/mail-account-resource.schema.json"##,
            r##"{"mailAccountId":"mail_acct_mail-a","integrationId":"mail-a","domainKind":"mail","environmentScope":"test","accountKey":"alice@example.com","accountLabel":"Alice Mailbox","readinessStatus":"healthy","canonicalDefault":true,"selectionMode":"explicit","mailboxAddress":"alice@example.com","mailboxLabel":"Alice Mailbox","supportsThreadInspection":true,"supportsDrafts":true,"supportsDirectSend":true,"supportsReply":true,"supportsForward":true,"lastSyncedAt":"2026-04-23T10:00:00Z","updatedAt":"2026-04-23T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/mail-account-list.response.schema.json"##,
            r##"{"items":[{"mailAccountId":"mail_acct_mail-a","integrationId":"mail-a","domainKind":"mail","environmentScope":"test","readinessStatus":"healthy","canonicalDefault":true,"mailboxAddress":"alice@example.com","mailboxLabel":"Alice Mailbox","supportsThreadInspection":true,"supportsDrafts":true,"supportsDirectSend":true,"supportsReply":true,"supportsForward":true,"lastSyncedAt":"2026-04-23T10:00:00Z","updatedAt":"2026-04-23T10:00:00Z"}]}"##,
        ),
        (
            r##"schemas/api/mail-thread-resource.schema.json"##,
            r##"{"threadId":"thread_seed","operationId":"mail_op_1","integrationId":"mail-a","mailAccountId":"mail_acct_mail-a","subject":"Seed message thread","participantSummary":["alice@example.com","bob@example.com"],"messageIds":["msg_seed"],"draftIds":["draft_seed"],"latestMessageAt":"2026-04-23T16:00:00Z","messageCount":1,"draftCount":1,"createdAt":"2026-04-23T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/mail-thread-list.response.schema.json"##,
            r##"{"account":{"mailAccountId":"mail_acct_mail-a","integrationId":"mail-a","domainKind":"mail","environmentScope":"test","readinessStatus":"healthy","canonicalDefault":true,"mailboxAddress":"alice@example.com","mailboxLabel":"Alice Mailbox","supportsThreadInspection":true,"supportsDrafts":true,"supportsDirectSend":true,"supportsReply":true,"supportsForward":true,"lastSyncedAt":"2026-04-23T10:00:00Z","updatedAt":"2026-04-23T10:00:00Z"},"items":[{"threadId":"thread_seed","operationId":"mail_op_1","integrationId":"mail-a","mailAccountId":"mail_acct_mail-a","subject":"Seed message thread","participantSummary":["alice@example.com","bob@example.com"],"messageIds":["msg_seed"],"draftIds":["draft_seed"],"latestMessageAt":"2026-04-23T16:00:00Z","messageCount":1,"draftCount":1,"createdAt":"2026-04-23T10:00:00Z"}],"operation":{"operationId":"mail_op_1","operationClass":"list_threads","status":"completed","resultMode":"inspection","integrationId":"mail-a","mailAccountId":"mail_acct_mail-a","environmentScope":"test","selectionMode":"explicit","threadId":"thread_seed","artifactIds":["mail_artifact_thread"],"createdAt":"2026-04-23T10:00:00Z","completedAt":"2026-04-23T10:00:01Z","updatedAt":"2026-04-23T10:00:01Z"},"artifacts":[{"artifactId":"mail_artifact_thread","operationId":"mail_op_1","kind":"thread_snapshot","integrationId":"mail-a","environmentScope":"test","threadId":"thread_seed","thread":{"threadId":"thread_seed","operationId":"mail_op_1","integrationId":"mail-a","mailAccountId":"mail_acct_mail-a","subject":"Seed message thread","participantSummary":["alice@example.com","bob@example.com"],"messageIds":["msg_seed"],"draftIds":["draft_seed"],"latestMessageAt":"2026-04-23T16:00:00Z","messageCount":1,"draftCount":1,"createdAt":"2026-04-23T10:00:00Z"},"createdAt":"2026-04-23T10:00:00Z"}]}"##,
        ),
        (
            r##"schemas/api/mail-message-resource.schema.json"##,
            r##"{"messageId":"msg_seed","threadId":"thread_seed","operationId":"mail_op_2","integrationId":"mail-a","mailAccountId":"mail_acct_mail-a","direction":"inbound","senderSummary":"bob@example.com","recipientSummary":["alice@example.com"],"subject":"Seed message thread","bodyPreview":"Can you review phase 30 today?","deliveryState":"received","receivedAt":"2026-04-23T16:00:00Z","createdAt":"2026-04-23T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/mail-draft-resource.schema.json"##,
            r##"{"draftId":"draft_seed","threadId":"thread_seed","operationId":"mail_op_3","integrationId":"mail-a","mailAccountId":"mail_acct_mail-a","composeMode":"reply","sourceMessageId":"msg_seed","recipientSummary":["bob@example.com"],"subject":"Re: Seed message thread","bodyPreview":"Draft response body.","draftStatus":"draft","createdAt":"2026-04-23T10:00:00Z","updatedAt":"2026-04-23T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/mail-draft-list.response.schema.json"##,
            r##"{"account":{"mailAccountId":"mail_acct_mail-a","integrationId":"mail-a","domainKind":"mail","environmentScope":"test","readinessStatus":"healthy","canonicalDefault":true,"mailboxAddress":"alice@example.com","mailboxLabel":"Alice Mailbox","supportsThreadInspection":true,"supportsDrafts":true,"supportsDirectSend":true,"supportsReply":true,"supportsForward":true,"lastSyncedAt":"2026-04-23T10:00:00Z","updatedAt":"2026-04-23T10:00:00Z"},"items":[{"draftId":"draft_seed","threadId":"thread_seed","operationId":"mail_op_3","integrationId":"mail-a","mailAccountId":"mail_acct_mail-a","composeMode":"reply","sourceMessageId":"msg_seed","recipientSummary":["bob@example.com"],"subject":"Re: Seed message thread","bodyPreview":"Draft response body.","draftStatus":"draft","createdAt":"2026-04-23T10:00:00Z","updatedAt":"2026-04-23T10:00:00Z"}],"operation":{"operationId":"mail_op_3","operationClass":"list_drafts","status":"completed","resultMode":"inspection","integrationId":"mail-a","mailAccountId":"mail_acct_mail-a","environmentScope":"test","selectionMode":"explicit","draftId":"draft_seed","artifactIds":["mail_artifact_draft"],"createdAt":"2026-04-23T10:00:00Z","completedAt":"2026-04-23T10:00:01Z","updatedAt":"2026-04-23T10:00:01Z"},"artifacts":[{"artifactId":"mail_artifact_draft","operationId":"mail_op_3","kind":"draft_snapshot","integrationId":"mail-a","environmentScope":"test","draftId":"draft_seed","draft":{"draftId":"draft_seed","threadId":"thread_seed","operationId":"mail_op_3","integrationId":"mail-a","mailAccountId":"mail_acct_mail-a","composeMode":"reply","sourceMessageId":"msg_seed","recipientSummary":["bob@example.com"],"subject":"Re: Seed message thread","bodyPreview":"Draft response body.","draftStatus":"draft","createdAt":"2026-04-23T10:00:00Z","updatedAt":"2026-04-23T10:00:00Z"},"createdAt":"2026-04-23T10:00:00Z"}]}"##,
        ),
        (
            r##"schemas/api/mail-artifact-resource.schema.json"##,
            r##"{"artifactId":"mail_artifact_attachment","operationId":"mail_op_4","kind":"attachment_reference","integrationId":"mail-a","environmentScope":"test","attachmentRefId":"attachment_1","attachment":{"attachmentRefId":"attachment_1","operationId":"mail_op_4","integrationId":"mail-a","parentKind":"message","parentId":"msg_sent","displayName":"brief.pdf","mediaType":"application/pdf","sizeBytes":1024,"resolutionStatus":"resolved","createdAt":"2026-04-23T10:00:00Z"},"createdAt":"2026-04-23T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/mail-operation-resource.schema.json"##,
            r##"{"operationId":"mail_op_4","operationClass":"send_message","status":"completed","resultMode":"sent","sendPath":"direct","integrationId":"mail-a","mailAccountId":"mail_acct_mail-a","environmentScope":"test","selectionMode":"explicit","threadId":"thread_sent","messageId":"msg_sent","requestSummary":"Phase 30 -> carol@example.com","backgroundSendPermitted":false,"runId":"run_1","stepId":"step_1","toolCallId":"tool_call_1","workflowId":"wf_1","workflowStepId":"wfstep_1","scheduleId":"sched_1","scheduleAttemptId":"sched_attempt_1","deliveryId":"delivery_1","artifactIds":["mail_artifact_message"],"createdAt":"2026-04-23T10:00:00Z","completedAt":"2026-04-23T10:00:01Z","updatedAt":"2026-04-23T10:00:01Z"}"##,
        ),
        (
            r##"schemas/api/mail-operation-list.response.schema.json"##,
            r##"{"items":[{"operationId":"mail_op_4","operationClass":"send_message","status":"completed","resultMode":"sent","sendPath":"direct","integrationId":"mail-a","mailAccountId":"mail_acct_mail-a","environmentScope":"test","createdAt":"2026-04-23T10:00:00Z","updatedAt":"2026-04-23T10:00:01Z"}]}"##,
        ),
        (
            r##"schemas/api/mail-operation-summary.schema.json"##,
            r##"{"operationId":"mail_op_4","operationClass":"send_message","integrationId":"mail-a","threadId":"thread_sent","messageId":"msg_sent","resultMode":"sent","sendPath":"direct","status":"completed","capturedAt":"2026-04-23T10:00:01Z"}"##,
        ),
        (
            r##"schemas/api/mail-workflow-action.schema.json"##,
            r##"{"operationClass":"send_message","integrationId":"mail-a","to":["carol@example.com"],"subject":"Workflow mail","body":"hello","allowSendSideEffects":true}"##,
        ),
        (
            r##"schemas/api/create-workflow.request.schema.json"##,
            r##"{"goal":"Send mail from workflow.","mailAction":{"operationClass":"send_message","integrationId":"mail-a","to":["carol@example.com"],"subject":"Workflow mail","body":"hello","allowSendSideEffects":true}}"##,
        ),
        (
            r##"schemas/api/create-schedule.request.schema.json"##,
            r##"{"trigger":{"kind":"once","fireAt":"2026-04-22T10:01:00Z"},"target":{"kind":"workflow","workflow":{"entrypoint":"operator","runGoal":"dispatch mail workflow","workflowGoal":"send mail","mailAction":{"operationClass":"send_message","integrationId":"mail-a","to":["carol@example.com"],"subject":"Workflow mail","body":"hello","allowSendSideEffects":true}}},"retryPolicy":{"maxRetries":1,"backoffKind":"fixed","baseDelaySeconds":5,"maxDelaySeconds":5}}"##,
        ),
        (
            r##"schemas/api/tool-call-resource.schema.json"##,
            r##"{"toolCallId":"tool_call_mail_1","runId":"run_1","stepId":"step_1","workflowId":"wf_1","workflowStepId":"wfstep_1","invocationKind":"domain_tool","domainKind":"mail","toolName":"mail.send_message","status":"completed","mailOperationSummaries":[{"operationId":"mail_op_4","operationClass":"send_message","integrationId":"mail-a","threadId":"thread_sent","messageId":"msg_sent","resultMode":"sent","sendPath":"direct","status":"completed","capturedAt":"2026-04-23T10:00:01Z"}],"createdAt":"2026-04-23T10:00:00Z","updatedAt":"2026-04-23T10:00:01Z","output":{"result":"sent"}}"##,
        ),
        (
            r##"schemas/api/workflow-step-resource.schema.json"##,
            r##"{"workflowStepId":"wfstep_mail_1","workflowId":"wf_1","title":"Send workflow mail","position":1,"consumerKind":"mail","consumerId":"mail-a","toolName":"mail.send_message","input":{"operationClass":"send_message","to":["carol@example.com"]},"status":"completed","selectionRationale":"Selected the configured mail integration for outbound action.","approvalModeExpected":"allow","attemptCount":1,"maxAttempts":1,"mailOperationSummaries":[{"operationId":"mail_op_4","operationClass":"send_message","integrationId":"mail-a","threadId":"thread_sent","messageId":"msg_sent","resultMode":"sent","sendPath":"direct","status":"completed","capturedAt":"2026-04-23T10:00:01Z"}],"createdAt":"2026-04-23T10:00:00Z","updatedAt":"2026-04-23T10:00:01Z"}"##,
        ),
        (
            r##"schemas/api/schedule-attempt-resource.schema.json"##,
            r##"{"scheduleAttemptId":"sched_attempt_1","scheduleId":"sched_1","dueAt":"2026-04-22T10:01:00Z","triggerSource":"normal","dispatchStatus":"dispatched","retryCount":0,"retryBudget":1,"resolvedTargetRevision":1,"runId":"run_1","workflowId":"wf_1","downstreamStatus":"completed","latestDeliveryId":"delivery_1","latestDeliveryStatus":"delivered","latestDeliveryTargetId":"test-sink-default","mailOperationSummaries":[{"operationId":"mail_op_4","operationClass":"send_message","integrationId":"mail-a","threadId":"thread_sent","messageId":"msg_sent","resultMode":"sent","sendPath":"direct","status":"completed","capturedAt":"2026-04-23T10:00:01Z"}],"createdAt":"2026-04-22T10:01:01Z","updatedAt":"2026-04-22T10:01:01Z"}"##,
        ),
        (
            r##"schemas/api/delivery-outcome-resource.schema.json"##,
            r##"{"deliveryId":"delivery_1","environmentScope":"test","sourceKind":"workflow","sourceId":"wf_1","runId":"run_1","workflowId":"wf_1","scheduleId":"sched_1","scheduleAttemptId":"sched_attempt_1","resultClass":"routine_success","mode":"immediate","status":"delivered","chosenTargetId":"test-sink-default","preferenceId":"pref-default","payloadPreview":"workflow mail sent","mailOperationIds":["mail_op_4"],"mailOperationSummaries":[{"operationId":"mail_op_4","operationClass":"send_message","integrationId":"mail-a","threadId":"thread_sent","messageId":"msg_sent","resultMode":"sent","sendPath":"direct","status":"completed","capturedAt":"2026-04-23T10:00:01Z"}],"attempts":[{"attemptId":"delivery_attempt_1","deliveryId":"delivery_1","attemptNumber":1,"targetId":"test-sink-default","transportKind":"test_sink","status":"delivered","transportReceiptSummary":"stored in repo-owned test sink","startedAt":"2026-04-22T10:00:01Z","completedAt":"2026-04-22T10:00:01Z"}],"createdAt":"2026-04-22T10:00:00Z","updatedAt":"2026-04-22T10:00:01Z","finalizedAt":"2026-04-22T10:00:01Z"}"##,
        ),
        (
            r##"schemas/events/mail-account-projected.event.schema.json"##,
            r##"{"eventId":"evt_mail_1","sequence":1,"category":"mail","name":"mail.account_projected","occurredAt":"2026-04-23T10:00:00Z","scope":{},"resource":{"kind":"mail_account","id":"mail_acct_mail-a"},"payload":{"integrationId":"mail-a","accountKey":"alice@example.com","mailboxAddress":"alice@example.com","readinessStatus":"healthy","canonicalDefault":true}}"##,
        ),
        (
            r##"schemas/events/mail-operation-requested.event.schema.json"##,
            r##"{"eventId":"evt_mail_2","sequence":2,"category":"mail","name":"mail.operation_requested","occurredAt":"2026-04-23T10:00:00Z","scope":{"runId":"run_1","workflowId":"wf_1","scheduleId":"sched_1"},"resource":{"kind":"mail_operation","id":"mail_op_4"},"payload":{"operationId":"mail_op_4","operationClass":"send_message","integrationId":"mail-a","runId":"run_1","workflowId":"wf_1","scheduleId":"sched_1","resultMode":"sent","sendPath":"direct","threadId":"thread_sent","messageId":"msg_sent","failureClass":""}}"##,
        ),
        (
            r##"schemas/events/mail-operation-completed.event.schema.json"##,
            r##"{"eventId":"evt_mail_3","sequence":3,"category":"mail","name":"mail.operation_completed","occurredAt":"2026-04-23T10:00:01Z","scope":{"runId":"run_1","workflowId":"wf_1","scheduleId":"sched_1"},"resource":{"kind":"mail_operation","id":"mail_op_4"},"payload":{"operationId":"mail_op_4","operationClass":"send_message","integrationId":"mail-a","runId":"run_1","workflowId":"wf_1","scheduleId":"sched_1","resultMode":"sent","sendPath":"direct","threadId":"thread_sent","messageId":"msg_sent","failureClass":""}}"##,
        ),
        (
            r##"schemas/events/mail-operation-failed.event.schema.json"##,
            r##"{"eventId":"evt_mail_4","sequence":4,"category":"mail","name":"mail.operation_failed","occurredAt":"2026-04-23T10:00:02Z","scope":{"runId":"run_1"},"resource":{"kind":"mail_operation","id":"mail_op_5"},"payload":{"operationId":"mail_op_5","operationClass":"send_message","integrationId":"mail-a","runId":"run_1","resultMode":"blocked","sendPath":"","failureClass":"attachment_unresolved"}}"##,
        ),
        (
            r##"schemas/events/mail-artifact-recorded.event.schema.json"##,
            r##"{"eventId":"evt_mail_5","sequence":5,"category":"mail","name":"mail.artifact_recorded","occurredAt":"2026-04-23T10:00:03Z","scope":{"runId":"run_1"},"resource":{"kind":"mail_artifact","id":"mail_artifact_message"},"payload":{"artifactId":"mail_artifact_message","operationId":"mail_op_4","threadId":"thread_sent","messageId":"msg_sent","draftId":""}}"##,
        ),
    ]
}

pub fn matrix_connector_contract_fixtures() -> &'static [Fixture] {
    &[
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
    ]
}

pub fn operator_contract_fixtures() -> &'static [Fixture] {
    &[
        (
            r##"schemas/api/operator-readiness-item.schema.json"##,
            r##"{"itemId":"integration-calendar-a","itemKind":"integration","resourceId":"calendar-a","displayName":"Calendar A","status":"degraded","healthState":"degraded","reason":"token refresh is required","requiredOperatorAction":"Refresh the calendar integration.","requiredForSelectedAction":false,"detailRoute":"/v1/integrations/calendar-a","environmentScope":"test","updatedAt":"2026-04-24T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/operator-first-use-action.schema.json"##,
            r##"{"actionId":"test_run","actionKind":"test_run","displayName":"Launch test run","recommended":true,"available":true,"blockingItemIds":[],"summary":"Persist a bounded shell smoke action.","invokeRoute":"/v1/runs","resultRoute":"/v1/runs"}"##,
        ),
        (
            r##"schemas/api/operator-onboarding.response.schema.json"##,
            r##"{"environmentScope":"test","status":"ready_for_action","currentStepId":"run-first-action","completedStepIds":["auth-ready"],"blockingItemIds":[],"optionalFollowUpItemIds":["integration-calendar-a"],"recommendedActionId":"test_run","readinessItems":[{"itemId":"auth-token","itemKind":"auth","resourceId":"token_1","displayName":"Operator access token","status":"ready","reason":"Authenticated shell session is active.","requiredForSelectedAction":true,"detailRoute":"/v1/auth/me","environmentScope":"test","updatedAt":"2026-04-24T10:00:00Z"},{"itemId":"integration-calendar-a","itemKind":"integration","resourceId":"calendar-a","displayName":"Calendar A","status":"degraded","healthState":"degraded","reason":"token refresh is required","requiredOperatorAction":"Refresh the calendar integration.","requiredForSelectedAction":false,"detailRoute":"/v1/integrations/calendar-a","environmentScope":"test","updatedAt":"2026-04-24T10:00:00Z"}],"firstUsefulActions":[{"actionId":"test_run","actionKind":"test_run","displayName":"Launch test run","recommended":true,"available":true,"blockingItemIds":[],"summary":"Persist a bounded shell smoke action.","invokeRoute":"/v1/runs","resultRoute":"/v1/runs"}],"lastEvaluatedAt":"2026-04-24T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/operator-activity-record.schema.json"##,
            r##"{"activityId":"delivery-delivery_1","sourceKind":"delivery","sourceId":"delivery_1","title":"Delivery failure","status":"failed","summary":"Source workflow | target transport failed","attentionLevel":"critical","occurredAt":"2026-04-24T10:05:00Z","detailRoute":"/v1/deliveries/delivery_1","relatedResourceRefs":[{"kind":"run","id":"run_1","route":"/v1/runs/run_1"},{"kind":"workflow","id":"wf_1","route":"/v1/runs/run_1/workflows/wf_1"}],"environmentScope":"test"}"##,
        ),
        (
            r##"schemas/api/operator-activity-list.response.schema.json"##,
            r##"{"environmentScope":"test","items":[{"activityId":"delivery-delivery_1","sourceKind":"delivery","sourceId":"delivery_1","title":"Delivery failure","status":"failed","summary":"Source workflow | target transport failed","attentionLevel":"critical","occurredAt":"2026-04-24T10:05:00Z","detailRoute":"/v1/deliveries/delivery_1","relatedResourceRefs":[{"kind":"run","id":"run_1","route":"/v1/runs/run_1"},{"kind":"workflow","id":"wf_1","route":"/v1/runs/run_1/workflows/wf_1"}],"environmentScope":"test"}],"generatedAt":"2026-04-24T10:05:00Z"}"##,
        ),
        (
            r##"schemas/api/operator-diagnostic-finding.schema.json"##,
            r##"{"findingId":"delivery-delivery_1","sourceKind":"delivery","sourceId":"delivery_1","plane":"delivery","severity":"critical","status":"failed","reason":"Delivery transport exhausted retries.","recommendedAction":"Inspect delivery attempts and target state.","detailRoute":"/v1/deliveries/delivery_1","relatedResourceRefs":[{"kind":"run","id":"run_1","route":"/v1/runs/run_1"},{"kind":"workflow","id":"wf_1","route":"/v1/runs/run_1/workflows/wf_1"}],"environmentScope":"test","capturedAt":"2026-04-24T10:06:00Z"}"##,
        ),
        (
            r##"schemas/api/operator-diagnostic-list.response.schema.json"##,
            r##"{"environmentScope":"test","items":[{"findingId":"delivery-delivery_1","sourceKind":"delivery","sourceId":"delivery_1","plane":"delivery","severity":"critical","status":"failed","reason":"Delivery transport exhausted retries.","recommendedAction":"Inspect delivery attempts and target state.","detailRoute":"/v1/deliveries/delivery_1","relatedResourceRefs":[{"kind":"run","id":"run_1","route":"/v1/runs/run_1"},{"kind":"workflow","id":"wf_1","route":"/v1/runs/run_1/workflows/wf_1"}],"environmentScope":"test","capturedAt":"2026-04-24T10:06:00Z"}],"generatedAt":"2026-04-24T10:06:00Z"}"##,
        ),
    ]
}

pub fn reminder_contract_fixtures() -> &'static [Fixture] {
    &[
        (
            r##"schemas/api/create-reminder.request.schema.json"##,
            r##"{"title":"Review follow-up","details":"check workflow outcome","behaviorMode":"launch_workflow","trigger":{"kind":"once","fireAt":"2026-04-23T12:00:00Z"},"workflowLaunchConfig":{"entrypoint":"operator","runGoal":"launch reminder workflow","workflowGoal":"follow up","calendarAction":{"operationClass":"create_event","integrationId":"calendar-a","title":"Reminder workflow","startsAt":"2026-04-23T17:00:00Z","endsAt":"2026-04-23T17:30:00Z"}},"followUpLink":{"linkKind":"workflow","sourceId":"wf_1","environmentScope":"test","sourceSummary":"Existing workflow","sourceDisplayState":"running"}}"##,
        ),
        (
            r##"schemas/api/reminder-trigger-resource.schema.json"##,
            r##"{"kind":"once","fireAt":"2026-04-23T12:00:00Z"}"##,
        ),
        (
            r##"schemas/api/reminder-workflow-launch.schema.json"##,
            r##"{"entrypoint":"operator","runGoal":"launch reminder workflow","workflowGoal":"follow up","mailAction":{"operationClass":"send_message","integrationId":"mail-a","to":["carol@example.com"],"subject":"Reminder workflow","body":"hello","allowSendSideEffects":true}}"##,
        ),
        (
            r##"schemas/api/reminder-follow-up-link.schema.json"##,
            r##"{"linkKind":"workflow","sourceId":"wf_1","environmentScope":"test","sourceSummary":"Existing workflow","sourceDisplayState":"running","stale":false,"lastCheckedAt":"2026-04-23T12:01:00Z"}"##,
        ),
        (
            r##"schemas/api/reminder-resource.schema.json"##,
            r##"{"reminderId":"rem_1","environmentScope":"test","title":"Review follow-up","details":"check workflow outcome","behaviorMode":"launch_workflow","trigger":{"kind":"once","fireAt":"2026-04-23T12:00:00Z"},"currentState":"acknowledged","activeOccurrenceId":"rem_occ_1","workflowLaunchConfig":{"entrypoint":"operator","workflowGoal":"follow up"},"followUpLink":{"linkKind":"workflow","sourceId":"wf_1","environmentScope":"test","sourceSummary":"Existing workflow","sourceDisplayState":"running","stale":false,"lastCheckedAt":"2026-04-23T12:01:00Z"},"createdAt":"2026-04-23T11:59:00Z","updatedAt":"2026-04-23T12:01:00Z"}"##,
        ),
        (
            r##"schemas/api/reminder-list.response.schema.json"##,
            r##"{"items":[{"reminderId":"rem_1","environmentScope":"test","title":"Review follow-up","behaviorMode":"notify_only","trigger":{"kind":"once","fireAt":"2026-04-23T12:00:00Z"},"currentState":"pending","createdAt":"2026-04-23T11:59:00Z","updatedAt":"2026-04-23T11:59:00Z"}]}"##,
        ),
        (
            r##"schemas/api/acknowledge-reminder.request.schema.json"##,
            r##"{"occurrenceId":"rem_occ_1","reason":"saw it","actorKind":"user"}"##,
        ),
        (
            r##"schemas/api/snooze-reminder.request.schema.json"##,
            r##"{"occurrenceId":"rem_occ_1","reason":"later","actorKind":"user","snoozedUntil":"2026-04-23T12:15:00Z"}"##,
        ),
        (
            r##"schemas/api/complete-reminder.request.schema.json"##,
            r##"{"occurrenceId":"rem_occ_1","reason":"done","actorKind":"user"}"##,
        ),
        (
            r##"schemas/api/dismiss-reminder.request.schema.json"##,
            r##"{"occurrenceId":"rem_occ_1","reason":"ignore","actorKind":"user"}"##,
        ),
        (
            r##"schemas/api/reschedule-reminder.request.schema.json"##,
            r##"{"occurrenceId":"rem_occ_1","reason":"move it","actorKind":"user","trigger":{"kind":"once","fireAt":"2026-04-23T12:30:00Z"}}"##,
        ),
        (
            r##"schemas/api/cancel-reminder.request.schema.json"##,
            r##"{"reason":"not needed","actorKind":"user"}"##,
        ),
        (
            r##"schemas/api/reminder-occurrence-resource.schema.json"##,
            r##"{"occurrenceId":"rem_occ_1","reminderId":"rem_1","environmentScope":"test","state":"acknowledged","scheduledFor":"2026-04-23T12:00:00Z","becameDueAt":"2026-04-23T12:00:00Z","acknowledgedAt":"2026-04-23T12:00:02Z","runId":"run_1","workflowId":"wf_1","latestDeliveryId":"delivery_1","latestDeliveryStatus":"delivered","latestDeliveryTargetId":"test-sink-default","createdAt":"2026-04-23T12:00:00Z","updatedAt":"2026-04-23T12:00:02Z"}"##,
        ),
        (
            r##"schemas/api/reminder-occurrence-list.response.schema.json"##,
            r##"{"items":[{"occurrenceId":"rem_occ_1","reminderId":"rem_1","environmentScope":"test","state":"due","scheduledFor":"2026-04-23T12:00:00Z","createdAt":"2026-04-23T12:00:00Z","updatedAt":"2026-04-23T12:00:01Z"}]}"##,
        ),
        (
            r##"schemas/api/reminder-action-resource.schema.json"##,
            r##"{"actionId":"rem_act_1","reminderId":"rem_1","occurrenceId":"rem_occ_1","actionKind":"workflow_started","actorKind":"system","previousState":"due","newState":"acknowledged","reason":"launched","runId":"run_1","workflowId":"wf_1","deliveryId":"delivery_1","createdAt":"2026-04-23T12:00:02Z"}"##,
        ),
        (
            r##"schemas/api/reminder-action-list.response.schema.json"##,
            r##"{"items":[{"actionId":"rem_act_2","reminderId":"rem_1","occurrenceId":"rem_occ_1","actionKind":"delivery_linked","actorKind":"system","newState":"due","deliveryId":"delivery_1","createdAt":"2026-04-23T12:00:01Z"}]}"##,
        ),
        (
            r##"schemas/api/run-resource.schema.json"##,
            r##"{"runId":"run_1","sessionId":"session_1","reminderId":"rem_1","reminderOccurrenceId":"rem_occ_1","entrypoint":"operator","status":"running","goal":"launch reminder workflow","activeWorkflowId":"wf_1","workflowCount":1,"latestDeliveryId":"delivery_1","latestDeliveryStatus":"delivered","latestDeliveryTargetId":"test-sink-default","createdAt":"2026-04-23T11:59:59Z","updatedAt":"2026-04-23T12:00:02Z"}"##,
        ),
        (
            r##"schemas/api/workflow-resource.schema.json"##,
            r##"{"workflowId":"wf_1","runId":"run_1","reminderId":"rem_1","reminderOccurrenceId":"rem_occ_1","environmentScope":"test","goal":"follow up","status":"planned","createdAt":"2026-04-23T12:00:00Z","updatedAt":"2026-04-23T12:00:00Z","steps":[],"dependencies":[],"handoffs":[]}"##,
        ),
        (
            r##"schemas/api/delivery-outcome-resource.schema.json"##,
            r##"{"deliveryId":"delivery_1","environmentScope":"test","sourceKind":"reminder_occurrence","sourceId":"rem_occ_1","runId":"run_1","workflowId":"wf_1","resultClass":"routine_success","mode":"immediate","status":"delivered","chosenTargetId":"test-sink-default","preferenceId":"pref-default","payloadPreview":"review follow-up","attempts":[{"attemptId":"delivery_attempt_1","deliveryId":"delivery_1","attemptNumber":1,"targetId":"test-sink-default","transportKind":"test_sink","status":"delivered","transportReceiptSummary":"stored in repo-owned test sink","startedAt":"2026-04-23T12:00:01Z","completedAt":"2026-04-23T12:00:01Z"}],"createdAt":"2026-04-23T12:00:01Z","updatedAt":"2026-04-23T12:00:01Z","finalizedAt":"2026-04-23T12:00:01Z"}"##,
        ),
        (
            r##"schemas/events/reminder-created.event.schema.json"##,
            r##"{"eventId":"evt_reminder_1","sequence":1,"category":"reminder","name":"reminder.created","occurredAt":"2026-04-23T11:59:00Z","scope":{},"resource":{"kind":"reminder","id":"rem_1"},"payload":{"reminderId":"rem_1","behaviorMode":"notify_only","currentState":"pending"}}"##,
        ),
        (
            r##"schemas/events/reminder-updated.event.schema.json"##,
            r##"{"eventId":"evt_reminder_2","sequence":2,"category":"reminder","name":"reminder.updated","occurredAt":"2026-04-23T12:05:00Z","scope":{},"resource":{"kind":"reminder","id":"rem_1"},"payload":{"reminderId":"rem_1","currentState":"cancelled"}}"##,
        ),
        (
            r##"schemas/events/reminder-occurrence-created.event.schema.json"##,
            r##"{"eventId":"evt_reminder_3","sequence":3,"category":"reminder","name":"reminder.occurrence_created","occurredAt":"2026-04-23T12:00:00Z","scope":{},"resource":{"kind":"reminder","id":"rem_1"},"payload":{"reminderId":"rem_1","occurrenceId":"rem_occ_1","state":"due","scheduledFor":"2026-04-23T12:00:00Z"}}"##,
        ),
        (
            r##"schemas/events/reminder-occurrence-transitioned.event.schema.json"##,
            r##"{"eventId":"evt_reminder_4","sequence":4,"category":"reminder","name":"reminder.occurrence_transitioned","occurredAt":"2026-04-23T12:00:02Z","scope":{"runId":"run_1","workflowId":"wf_1"},"resource":{"kind":"reminder","id":"rem_1"},"payload":{"reminderId":"rem_1","occurrenceId":"rem_occ_1","state":"acknowledged","actionKind":"workflow_started"}}"##,
        ),
        (
            r##"schemas/events/reminder-workflow-launch-started.event.schema.json"##,
            r##"{"eventId":"evt_reminder_5","sequence":5,"category":"reminder","name":"reminder.workflow_launch_started","occurredAt":"2026-04-23T12:00:02Z","scope":{"runId":"run_1","workflowId":"wf_1"},"resource":{"kind":"reminder","id":"rem_1"},"payload":{"reminderId":"rem_1","occurrenceId":"rem_occ_1","actionKind":"workflow_started"}}"##,
        ),
        (
            r##"schemas/events/reminder-workflow-launch-failed.event.schema.json"##,
            r##"{"eventId":"evt_reminder_6","sequence":6,"category":"reminder","name":"reminder.workflow_launch_failed","occurredAt":"2026-04-23T12:00:02Z","scope":{},"resource":{"kind":"reminder","id":"rem_1"},"payload":{"reminderId":"rem_1","occurrenceId":"rem_occ_1","actionKind":"workflow_start_failed","reason":"launcher unavailable"}}"##,
        ),
        (
            r##"schemas/events/reminder-delivery-linked.event.schema.json"##,
            r##"{"eventId":"evt_reminder_7","sequence":7,"category":"reminder","name":"reminder.delivery_linked","occurredAt":"2026-04-23T12:00:01Z","scope":{},"resource":{"kind":"reminder","id":"rem_1"},"payload":{"reminderId":"rem_1","occurrenceId":"rem_occ_1","actionKind":"delivery_linked","deliveryId":"delivery_1"}}"##,
        ),
    ]
}

pub fn schedule_contract_fixtures() -> &'static [Fixture] {
    &[
        (
            r##"schemas/api/schedule-resource.schema.json"##,
            r##"{"scheduleId":"sched_1","environmentScope":"test","kind":"one_time","status":"completed","targetRefId":"sched_target_1","trigger":{"kind":"once","fireAt":"2026-04-22T10:01:00Z","nextDueAt":"2026-04-22T10:01:00Z"},"target":{"kind":"run","revision":1,"active":true,"run":{"entrypoint":"operator","goal":"dispatch once"},"summary":"dispatch once","updatedAt":"2026-04-22T10:00:00Z"},"retryPolicy":{"maxRetries":1,"backoffKind":"fixed","baseDelaySeconds":5,"maxDelaySeconds":5},"nextDueAt":"2026-04-22T10:01:00Z","lastAttemptAt":"2026-04-22T10:01:01Z","lastOutcome":"dispatched","createdAt":"2026-04-22T10:00:00Z","updatedAt":"2026-04-22T10:01:01Z","completedAt":"2026-04-22T10:01:01Z","attempts":[{"scheduleAttemptId":"sched_attempt_1","scheduleId":"sched_1","dueAt":"2026-04-22T10:01:00Z","triggerSource":"normal","dispatchStatus":"dispatched","retryCount":0,"retryBudget":1,"resolvedTargetRevision":1,"runId":"run_1","downstreamStatus":"running","createdAt":"2026-04-22T10:01:01Z","updatedAt":"2026-04-22T10:01:01Z"}]}"##,
        ),
        (
            r##"schemas/api/schedule-list.response.schema.json"##,
            r##"{"items":[{"scheduleId":"sched_1","environmentScope":"test","kind":"recurring","status":"active","targetRefId":"sched_target_1","trigger":{"kind":"cron","cronExpr":"*/1 * * * *","timezone":"UTC","nextDueAt":"2026-04-22T10:02:00Z"},"target":{"kind":"run","revision":1,"active":true,"run":{"entrypoint":"operator","goal":"dispatch once"},"summary":"dispatch once","updatedAt":"2026-04-22T10:00:00Z"},"retryPolicy":{"maxRetries":1,"backoffKind":"fixed","baseDelaySeconds":5,"maxDelaySeconds":5},"nextDueAt":"2026-04-22T10:02:00Z","createdAt":"2026-04-22T10:00:00Z","updatedAt":"2026-04-22T10:01:01Z","attempts":[]}]}"##,
        ),
        (
            r##"schemas/events/schedule-created.event.schema.json"##,
            r##"{"eventId":"evt_schedule_1","sequence":1,"category":"schedule","name":"schedule.created","occurredAt":"2026-04-22T10:00:00Z","scope":{"scheduleId":"sched_1"},"resource":{"kind":"schedule","id":"sched_1"},"payload":{"scheduleId":"sched_1","status":"scheduled","targetKind":"run","targetRefId":"sched_target_1"}}"##,
        ),
        (
            r##"schemas/events/schedule-status-changed.event.schema.json"##,
            r##"{"eventId":"evt_schedule_2","sequence":2,"category":"schedule","name":"schedule.status_changed","occurredAt":"2026-04-22T10:00:01Z","scope":{"scheduleId":"sched_1"},"resource":{"kind":"schedule","id":"sched_1"},"payload":{"scheduleId":"sched_1","status":"paused"}}"##,
        ),
        (
            r##"schemas/events/schedule-dispatch-attempted.event.schema.json"##,
            r##"{"eventId":"evt_schedule_3","sequence":3,"category":"schedule","name":"schedule.dispatch_attempted","occurredAt":"2026-04-22T10:01:00Z","scope":{"scheduleId":"sched_1","scheduleAttemptId":"sched_attempt_1"},"resource":{"kind":"schedule","id":"sched_1"},"payload":{"scheduleId":"sched_1","scheduleAttemptId":"sched_attempt_1","dispatchStatus":"dispatching","dueAt":"2026-04-22T10:01:00Z","triggerSource":"normal"}}"##,
        ),
        (
            r##"schemas/events/schedule-dispatch-recorded.event.schema.json"##,
            r##"{"eventId":"evt_schedule_4","sequence":4,"category":"schedule","name":"schedule.dispatch_recorded","occurredAt":"2026-04-22T10:01:01Z","scope":{"scheduleId":"sched_1","scheduleAttemptId":"sched_attempt_1","runId":"run_1"},"resource":{"kind":"schedule","id":"sched_1"},"payload":{"scheduleId":"sched_1","scheduleAttemptId":"sched_attempt_1","dispatchStatus":"dispatched","dueAt":"2026-04-22T10:01:00Z","triggerSource":"normal","runId":"run_1"}}"##,
        ),
        (
            r##"schemas/events/schedule-retry-scheduled.event.schema.json"##,
            r##"{"eventId":"evt_schedule_5","sequence":5,"category":"schedule","name":"schedule.retry_scheduled","occurredAt":"2026-04-22T10:01:05Z","scope":{"scheduleId":"sched_1","scheduleAttemptId":"sched_attempt_2"},"resource":{"kind":"schedule","id":"sched_1"},"payload":{"scheduleId":"sched_1","scheduleAttemptId":"sched_attempt_2","dispatchStatus":"failed","retryCount":1,"nextRetryAt":"2026-04-22T10:01:10Z"}}"##,
        ),
    ]
}

pub fn setup_wizard_contract_fixtures() -> &'static [Fixture] {
    &[
        (
            r##"schemas/api/setup-target-list.response.schema.json"##,
            r##"{"items":[{"targetId":"integration.feishu_lark","tenantId":"ten_personal","targetKind":"integration","setupStyle":"oauth","displayName":"Feishu/Lark OAuth","proofTarget":true,"supportStatus":"supported","requiredPermissions":["secrets.manage","integrations.manage"],"limitedSafeCapabilities":["metadata_read"],"currentSessionId":"setup_ten_personal_integration_feishu_lark","currentState":"action_required","diagnosticResultId":"diag_lark_setup"},{"targetId":"provider.openai_compatible","tenantId":"ten_personal","targetKind":"provider","setupStyle":"submitted_secret","displayName":"OpenAI-compatible provider","proofTarget":true,"supportStatus":"supported","requiredPermissions":["secrets.manage","integrations.manage"],"limitedSafeCapabilities":["metadata_read"],"currentSessionId":"setup_ten_personal_provider_openai_compatible","currentState":"ready","diagnosticResultId":"diag_openai_setup"}]}"##,
        ),
        (
            r##"schemas/api/setup-session-resource.schema.json"##,
            r##"{"setupSessionId":"setup_ten_personal_provider_openai_compatible","tenantId":"ten_personal","actorPrincipalId":"prn_1","targetId":"provider.openai_compatible","targetKind":"provider","setupStyle":"submitted_secret","state":"ready","reasonCode":"healthy","retryable":false,"remediationOwner":"none_required","safeUseMode":"normal","allowedCapabilities":[],"currentAttemptId":"attempt_1","diagnosticResultId":"diag_openai_setup","diagnosticRunId":"diag_run_openai_setup","diagnosticStage":"credential_probe","diagnosticSourceKind":"provider_check","diagnosticSourceId":"provider.openai_compatible","redactionStatus":"redacted","resourceRefs":[{"kind":"tenant_secret","id":"provider/openai-compatible","route":"/v1/tenant-secrets/provider%2Fopenai-compatible"}],"redactedEvidence":{"secretRef":"provider/openai-compatible","secretVersionId":"secver_1"},"createdAt":"2026-05-06T00:00:00Z","updatedAt":"2026-05-06T00:01:00Z","lastTransitionAt":"2026-05-06T00:01:00Z","lastTransitionAuditEventId":"audit_setup_1"}"##,
        ),
        (
            r##"schemas/api/setup-session.response.schema.json"##,
            r##"{"session":{"setupSessionId":"setup_ten_personal_provider_openai_compatible","tenantId":"ten_personal","actorPrincipalId":"prn_1","targetId":"provider.openai_compatible","targetKind":"provider","setupStyle":"submitted_secret","state":"ready","reasonCode":"healthy","retryable":false,"remediationOwner":"none_required","safeUseMode":"normal","allowedCapabilities":[],"currentAttemptId":"attempt_1","diagnosticResultId":"diag_openai_setup","diagnosticRunId":"diag_run_openai_setup","diagnosticStage":"credential_probe","diagnosticSourceKind":"provider_check","diagnosticSourceId":"provider.openai_compatible","redactionStatus":"redacted","resourceRefs":[{"kind":"tenant_secret","id":"provider/openai-compatible","route":"/v1/tenant-secrets/provider%2Fopenai-compatible"}],"redactedEvidence":{"secretRef":"provider/openai-compatible","secretVersionId":"secver_1"},"createdAt":"2026-05-06T00:00:00Z","updatedAt":"2026-05-06T00:01:00Z","lastTransitionAt":"2026-05-06T00:01:00Z","lastTransitionAuditEventId":"audit_setup_1"}}"##,
        ),
        (
            r##"schemas/api/setup-session-list.response.schema.json"##,
            r##"{"items":[{"setupSessionId":"setup_ten_personal_provider_openai_compatible","tenantId":"ten_personal","actorPrincipalId":"prn_1","targetId":"provider.openai_compatible","targetKind":"provider","setupStyle":"submitted_secret","state":"ready","reasonCode":"healthy","retryable":false,"remediationOwner":"none_required","safeUseMode":"normal","allowedCapabilities":[],"currentAttemptId":"attempt_1","diagnosticResultId":"diag_openai_setup","diagnosticRunId":"diag_run_openai_setup","diagnosticStage":"credential_probe","diagnosticSourceKind":"provider_check","diagnosticSourceId":"provider.openai_compatible","redactionStatus":"redacted","resourceRefs":[{"kind":"tenant_secret","id":"provider/openai-compatible","route":"/v1/tenant-secrets/provider%2Fopenai-compatible"}],"redactedEvidence":{"secretRef":"provider/openai-compatible","secretVersionId":"secver_1"},"createdAt":"2026-05-06T00:00:00Z","updatedAt":"2026-05-06T00:01:00Z","lastTransitionAt":"2026-05-06T00:01:00Z","lastTransitionAuditEventId":"audit_setup_1"}]}"##,
        ),
        (
            r##"schemas/api/setup-secret-submit.request.schema.json"##,
            r##"{"secretRef":"provider/openai-compatible","value":"R46_FAKE_OPENAI_COMPATIBLE_KEY_DO_NOT_LEAK","displayName":"OpenAI-compatible API key"}"##,
        ),
        (
            r##"schemas/api/setup-oauth-start.request.schema.json"##,
            r##"{"redirectRoute":"/setup/oauth/feishu-lark/callback"}"##,
        ),
        (
            r##"schemas/api/setup-oauth-callback.request.schema.json"##,
            r##"{"state":"oauth_state_ref_1","result":"denied","accountLabel":"tenant workspace"}"##,
        ),
        (
            r##"schemas/api/setup-diagnostic-list.response.schema.json"##,
            r##"{"items":[{"setupSessionId":"setup_ten_personal_integration_feishu_lark","targetId":"integration.feishu_lark","diagnosticResultId":"diag_lark_scope","diagnosticRunId":"diag_run_1","diagnosticStage":"oauth_probe","diagnosticSourceKind":"integration_diagnostic","diagnosticSourceId":"integration.feishu_lark","status":"action_required","reasonCode":"scope_missing","retrySafety":"retryable","remediationOwner":"tenant_admin","allowedCapabilities":[],"checkedAt":"2026-05-06T00:02:00Z","staleAfter":"2026-05-06T00:12:00Z","redactionStatus":"redacted"}]}"##,
        ),
        (
            r##"schemas/api/setup-error.response.schema.json"##,
            r##"{"error":"setup permission denied","code":"setup_denied:missing_permission","reasonCode":"setup_denied:missing_permission","stage":"permission","retryable":false,"remediationOwner":"tenant_admin"}"##,
        ),
    ]
}

pub fn slack_connector_contract_fixtures() -> &'static [Fixture] {
    &[
        (
            r##"schemas/api/slack-hosted-setup-resource.schema.json"##,
            r##"{"tenantId":"ten_slack","connectorId":"slack-main","connectorKind":"slack","displayName":"Slack Main","status":"degraded","terminalState":"action-required","oauthState":"scope_missing","routePolicyState":"valid","deliveryEligible":false,"workspaceBindingId":"slack_workspace_binding_1","redactionStatus":"redacted","createdAt":"2026-05-08T10:00:00Z","updatedAt":"2026-05-08T10:01:00Z","validatedAt":"2026-05-08T10:01:00Z","retentionExpiresAt":"2026-08-06T10:01:00Z","workspaceBinding":{"workspaceId":"workspace_redacted","workspaceLabel":"Workspace redacted","installationId":"installation_redacted","oauthGrantState":"scope_missing","requiredScopeState":"missing","validatedAt":"2026-05-08T10:01:00Z","redactionStatus":"redacted"},"routePolicy":{"tenantId":"ten_slack","connectorId":"slack-main","workspaceBindingId":"slack_workspace_binding_1","selectedChannels":[{"conversationId":"channel_redacted","conversationType":"channel","selectedChannelState":"selected","validationState":"valid","redactionStatus":"redacted"}],"allowedDMUsers":["user_hash_1"],"allowedDMUserGroups":["group_hash_1"],"mentionGate":"agent_mention_required","threadReplyMode":"channel_mentions_thread_rooted","validationState":"valid","validatedAt":"2026-05-08T10:01:00Z","redactionStatus":"redacted"},"diagnostic":{"reasonCode":"permission_missing","slackCondition":"missing_scope","remediationOwner":"tenant_admin","freshnessState":"fresh"}}"##,
        ),
        (
            r##"schemas/api/slack-route-policy-resource.schema.json"##,
            r##"{"tenantId":"ten_slack","connectorId":"slack-main","workspaceBindingId":"slack_workspace_binding_1","selectedChannels":[{"conversationId":"channel_redacted","conversationType":"channel","selectedChannelState":"selected","validationState":"valid","redactionStatus":"redacted"}],"allowedDMUsers":["user_hash_1"],"allowedDMUserGroups":["group_hash_1"],"mentionGate":"agent_mention_required","threadReplyMode":"channel_mentions_thread_rooted","validationState":"valid","validatedAt":"2026-05-08T10:01:00Z","redactionStatus":"redacted","safeEvidence":{"route":"selected_channel_and_dm_allowment"}}"##,
        ),
        (
            r##"schemas/api/slack-smoke-evidence-resource.schema.json"##,
            r##"{"smokeEvidenceId":"slack_smoke_1","tenantId":"ten_slack","connectorId":"slack-main","workspaceBindingId":"slack_workspace_binding_1","status":"skipped","authorizationMode":"unavailable","owner":"operator","reason":"safe_slack_authorization_unavailable","remainingRisk":"No live Slack hosted smoke was run in this release validation.","validatedAt":"2026-05-08T10:02:00Z","retentionExpiresAt":"2026-08-06T10:02:00Z","redactionStatus":"redacted","safeEvidence":{"policy":"structured_skip"}}"##,
        ),
        (
            r##"schemas/events/connector-slack-setup-validated.event.schema.json"##,
            r##"{"eventId":"evt_slack_setup_1","sequence":1,"category":"connector","name":"connector.slack_setup_validated","occurredAt":"2026-05-08T10:01:00Z","scope":{"connectorId":"slack-main"},"resource":{"kind":"slack_hosted_setup","id":"slack-main"},"payload":{"tenantId":"ten_slack","connectorId":"slack-main","workspaceBindingId":"slack_workspace_binding_1","terminalState":"action-required","oauthState":"scope_missing","routePolicyState":"valid","deliveryEligible":false,"reasonCode":"permission_missing","slackCondition":"missing_scope","redactionStatus":"redacted","validatedAt":"2026-05-08T10:01:00Z"}}"##,
        ),
    ]
}

pub fn telegram_connector_contract_fixtures() -> &'static [Fixture] {
    &[
        (
            r##"schemas/api/telegram-hosted-setup-resource.schema.json"##,
            r##"{"tenantId":"ten_telegram","connectorId":"telegram-main","connectorKind":"telegram","displayName":"Telegram Main","status":"healthy","terminalState":"ready","hostedReady":true,"credentialState":"valid","allowmentState":"valid","groupBehavior":"mention_or_command_required","deliveryEligible":true,"reasonCode":"healthy","redactionStatus":"redacted","createdAt":"2026-05-08T10:00:00Z","updatedAt":"2026-05-08T10:01:00Z","validatedAt":"2026-05-08T10:01:00Z","retentionExpiresAt":"2026-08-06T10:01:00Z","allowments":[{"tenantId":"ten_telegram","connectorId":"telegram-main","allowmentId":"allow_dm","telegramScopeType":"direct_chat","telegramScopeId":"chat_redacted","enabled":true,"groupGate":"not_applicable","validationState":"valid","reasonCode":"healthy","validatedAt":"2026-05-08T10:01:00Z","redactionStatus":"redacted","safeEvidence":{"scope":"direct_chat"}}]}"##,
        ),
        (
            r##"schemas/api/telegram-allowment-resource.schema.json"##,
            r##"{"tenantId":"ten_telegram","connectorId":"telegram-main","allowmentId":"allow_group","telegramScopeType":"group","telegramScopeId":"group_redacted","providerLabel":"Telegram group","enabled":true,"groupGate":"mention_or_command_required","validationState":"valid","reasonCode":"healthy","validatedAt":"2026-05-08T10:01:00Z","redactionStatus":"redacted","safeEvidence":{"gate":"mention_or_command_required"}}"##,
        ),
        (
            r##"schemas/api/telegram-smoke-evidence-resource.schema.json"##,
            r##"{"smokeEvidenceId":"telegram_smoke_1","tenantId":"ten_telegram","connectorId":"telegram-main","status":"passed","credentialMode":"fake","owner":"operator","reason":"healthy","remainingRisk":"live provider not exercised","validatedAt":"2026-05-08T10:02:00Z","retentionExpiresAt":"2026-08-06T10:02:00Z","redactionStatus":"redacted","safeEvidence":{"transport":"fake"}}"##,
        ),
        (
            r##"schemas/api/record-telegram-smoke.request.schema.json"##,
            r##"{"connectorId":"telegram-main","status":"skipped","credentialMode":"unavailable","owner":"operator","reason":"safe_credentials_unavailable","remainingRisk":"No safe live Telegram credential was available for this release validation.","safeEvidence":{"policy":"structured_skip"}}"##,
        ),
        (
            r##"schemas/events/connector-telegram-setup-validated.event.schema.json"##,
            r##"{"eventId":"evt_telegram_setup_1","sequence":1,"category":"connector","name":"connector.telegram_setup_validated","occurredAt":"2026-05-08T10:01:00Z","scope":{"connectorId":"telegram-main"},"resource":{"kind":"telegram_hosted_setup","id":"telegram-main"},"payload":{"tenantId":"ten_telegram","connectorId":"telegram-main","terminalState":"ready","hostedReady":true,"credentialState":"valid","allowmentState":"valid","reasonCode":"healthy","redactionStatus":"redacted","validatedAt":"2026-05-08T10:01:00Z"}}"##,
        ),
    ]
}

pub fn tenant_identity_contract_fixtures() -> &'static [Fixture] {
    &[
        (
            r##"schemas/api/auth-access-token-resource.schema.json"##,
            r##"{"tokenId":"tok_1","principalId":"prn_1","label":"web","mode":"local","tokenPreview":"kura_preview","status":"active","defaultTenantId":"ten_1","createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z","lastUsedAt":"2026-04-24T10:01:00Z"}"##,
        ),
        (
            r##"schemas/api/tenant-resource.schema.json"##,
            r##"{"tenantId":"ten_1","tenantKind":"personal","displayName":"Personal tenant","status":"active","createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z","createdByPrincipalId":"prn_1","defaultOwnerPrincipalId":"prn_1","callerMembershipRole":"owner","callerMembershipStatus":"active","callerPermissions":["tenant.manage","read_only.inspect"],"defaultForCurrentToken":true,"defaultForCurrentPrincipal":true}"##,
        ),
        (
            r##"schemas/api/principal-resource.schema.json"##,
            r##"{"principalId":"prn_1","principalKind":"local_operator","displayName":"Local operator","status":"active","defaultTenantId":"ten_1","createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/tenant-context-resource.schema.json"##,
            r##"{"principalId":"prn_1","tokenId":"tok_1","tenantId":"ten_1","tenantSource":"default","membershipId":"mem_1","role":"owner","permissions":["tenant.manage","read_only.inspect"],"resolvedAt":"2026-04-24T10:01:00Z"}"##,
        ),
        (
            r##"schemas/api/tenant-permission-resource.schema.json"##,
            r##"{"permission":"tenant.manage","allowed":true}"##,
        ),
        (
            r##"schemas/api/token-tenant-grant-resource.schema.json"##,
            r##"{"grantId":"grant_1","tokenId":"tok_1","tenantId":"ten_1","isDefault":true,"status":"active","createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z","grantedByPrincipalId":"prn_1"}"##,
        ),
        (
            r##"schemas/api/create-tenant.request.schema.json"##,
            r##"{"displayName":"Acme","tenantKind":"organization"}"##,
        ),
        (
            r##"schemas/api/create-tenant.response.schema.json"##,
            r##"{"tenant":{"tenantId":"ten_org","tenantKind":"organization","displayName":"Acme","status":"active","createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z"},"membership":{"membershipId":"mem_owner","tenantId":"ten_org","principalId":"prn_1","role":"owner","status":"active","createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z"}}"##,
        ),
        (
            r##"schemas/api/membership-resource.schema.json"##,
            r##"{"membershipId":"mem_1","tenantId":"ten_1","principalId":"prn_1","role":"owner","status":"active","createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z","acceptedAt":"2026-04-24T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/membership-list.response.schema.json"##,
            r##"{"items":[{"membershipId":"mem_1","tenantId":"ten_1","principalId":"prn_1","role":"owner","status":"active","createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z"}]}"##,
        ),
        (
            r##"schemas/api/create-tenant-invitation.request.schema.json"##,
            r##"{"invitedPrincipalId":"prn_2","role":"operator"}"##,
        ),
        (
            r##"schemas/api/tenant-invitation-resource.schema.json"##,
            r##"{"invitationId":"inv_1","tenantId":"ten_1","invitedPrincipalId":"prn_2","invitedByPrincipalId":"prn_1","role":"operator","status":"invited","createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/create-tenant-invitation.response.schema.json"##,
            r##"{"invitation":{"invitationId":"inv_1","tenantId":"ten_1","invitedPrincipalId":"prn_2","invitedByPrincipalId":"prn_1","role":"operator","status":"invited","createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z"}}"##,
        ),
        (
            r##"schemas/api/tenant-invitation-list.response.schema.json"##,
            r##"{"items":[{"invitationId":"inv_1","tenantId":"ten_1","invitedPrincipalId":"prn_2","invitedByPrincipalId":"prn_1","role":"operator","status":"invited","createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z"}]}"##,
        ),
        (
            r##"schemas/api/accept-tenant-invitation.request.schema.json"##,
            r##"{}"##,
        ),
        (
            r##"schemas/api/reject-tenant-invitation.request.schema.json"##,
            r##"{}"##,
        ),
        (
            r##"schemas/api/tenant-invitation-decision.response.schema.json"##,
            r##"{"membership":{"membershipId":"mem_2","tenantId":"ten_1","principalId":"prn_2","role":"operator","status":"active","invitationId":"inv_1","createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z"}}"##,
        ),
        (
            r##"schemas/api/update-membership.request.schema.json"##,
            r##"{"role":"viewer"}"##,
        ),
        (
            r##"schemas/api/principal-list.response.schema.json"##,
            r##"{"items":[{"principalId":"prn_1","principalKind":"local_operator","displayName":"Local operator","status":"active","defaultTenantId":"ten_1","createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z"}]}"##,
        ),
        (
            r##"schemas/api/update-principal.request.schema.json"##,
            r##"{"status":"disabled"}"##,
        ),
        (
            r##"schemas/api/update-principal.response.schema.json"##,
            r##"{"principal":{"principalId":"prn_1","principalKind":"local_operator","displayName":"Local operator","status":"disabled","defaultTenantId":"ten_1","createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z"}}"##,
        ),
        (
            r##"schemas/api/tenant-audit-event-resource.schema.json"##,
            r##"{"auditEventId":"audit_1","eventKind":"tenant.permission_denied","tenantId":"ten_1","principalId":"prn_1","tokenId":"tok_1","outcome":"denied","reasonCode":"permission_denied:tenant.manage","createdAt":"2026-04-24T10:00:00Z"}"##,
        ),
        (
            r##"schemas/api/tenant-audit-event-list.response.schema.json"##,
            r##"{"items":[{"auditEventId":"audit_1","eventKind":"tenant.permission_denied","tenantId":"ten_1","principalId":"prn_1","tokenId":"tok_1","outcome":"denied","reasonCode":"permission_denied:tenant.manage","createdAt":"2026-04-24T10:00:00Z"}]}"##,
        ),
        (
            r##"schemas/api/auth-token-list.response.schema.json"##,
            r##"{"items":[{"tokenId":"tok_1","principalId":"prn_1","label":"web","mode":"local","tokenPreview":"kura_preview","status":"active","defaultTenantId":"ten_1","createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z"}]}"##,
        ),
        (
            r##"schemas/api/create-auth-token.request.schema.json"##,
            r##"{"label":"automation","defaultTenantId":"ten_1","allowedTenantIds":["ten_1"]}"##,
        ),
        (
            r##"schemas/api/create-auth-token.response.schema.json"##,
            r##"{"token":{"tokenId":"tok_2","principalId":"prn_1","label":"automation","mode":"token","tokenPreview":"kura_preview","status":"active","defaultTenantId":"ten_1","createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z"},"accessToken":"kura_secret","grants":[{"grantId":"grant_2","tokenId":"tok_2","tenantId":"ten_1","isDefault":true,"status":"active","createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z"}]}"##,
        ),
        (
            r##"schemas/api/rotate-auth-token.request.schema.json"##,
            r##"{"reason":"scheduled"}"##,
        ),
        (
            r##"schemas/api/rotate-auth-token.response.schema.json"##,
            r##"{"oldToken":{"tokenId":"tok_1","principalId":"prn_1","label":"web","mode":"local","tokenPreview":"kura_preview","status":"rotated","defaultTenantId":"ten_1","createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z","rotatedToTokenId":"tok_2"},"newToken":{"tokenId":"tok_2","principalId":"prn_1","label":"web","mode":"local","tokenPreview":"kura_new","status":"active","defaultTenantId":"ten_1","createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z","rotatedFromTokenId":"tok_1"},"accessToken":"kura_secret","grants":[{"grantId":"grant_2","tokenId":"tok_2","tenantId":"ten_1","isDefault":true,"status":"active","createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z"}]}"##,
        ),
        (
            r##"schemas/api/revoke-auth-token.request.schema.json"##,
            r##"{"reason":"manual"}"##,
        ),
        (
            r##"schemas/api/update-token-tenant-grants.request.schema.json"##,
            r##"{"defaultTenantId":"ten_1","allowedTenantIds":["ten_1"],"reason":"least_privilege"}"##,
        ),
        (
            r##"schemas/api/update-token-tenant-grants.response.schema.json"##,
            r##"{"tokenId":"tok_1","grants":[{"grantId":"grant_1","tokenId":"tok_1","tenantId":"ten_1","isDefault":true,"status":"active","createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z"}]}"##,
        ),
        (
            r##"schemas/api/tenant-list.response.schema.json"##,
            r##"{"items":[{"tenantId":"ten_1","tenantKind":"personal","displayName":"Personal tenant","status":"active","createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z","callerMembershipRole":"owner","callerMembershipStatus":"active","callerPermissions":["tenant.manage"],"defaultForCurrentToken":true,"defaultForCurrentPrincipal":true}]}"##,
        ),
        (
            r##"schemas/api/tenant-detail.response.schema.json"##,
            r##"{"tenant":{"tenantId":"ten_1","tenantKind":"personal","displayName":"Personal tenant","status":"active","createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z","callerMembershipRole":"owner","callerMembershipStatus":"active","callerPermissions":["tenant.manage"]},"tenantContext":{"principalId":"prn_1","tokenId":"tok_1","tenantId":"ten_1","tenantSource":"explicit_header","membershipId":"mem_1","role":"owner","permissions":["tenant.manage"],"resolvedAt":"2026-04-24T10:01:00Z"}}"##,
        ),
        (
            r##"schemas/api/auth-me.response.schema.json"##,
            r##"{"token":{"tokenId":"tok_1","principalId":"prn_1","label":"web","mode":"local","tokenPreview":"kura_preview","status":"active","defaultTenantId":"ten_1","createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z"},"principal":{"principalId":"prn_1","principalKind":"local_operator","displayName":"Local operator","status":"active","defaultTenantId":"ten_1","createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z"},"defaultTenant":{"tenantId":"ten_1","tenantKind":"personal","displayName":"Personal tenant","status":"active","createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z","defaultForCurrentToken":true,"defaultForCurrentPrincipal":true},"currentTenant":{"tenantId":"ten_1","tenantKind":"personal","displayName":"Personal tenant","status":"active","createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z","callerMembershipRole":"owner","callerMembershipStatus":"active","callerPermissions":["tenant.manage"]},"allowedTenants":[{"tenantId":"ten_1","tenantKind":"personal","displayName":"Personal tenant","status":"active","createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z","defaultForCurrentToken":true,"defaultForCurrentPrincipal":true}],"tokenGrants":[{"grantId":"grant_1","tokenId":"tok_1","tenantId":"ten_1","isDefault":true,"status":"active","createdAt":"2026-04-24T10:00:00Z","updatedAt":"2026-04-24T10:00:00Z"}],"permissions":["tenant.manage"],"tenantContext":{"principalId":"prn_1","tokenId":"tok_1","tenantId":"ten_1","tenantSource":"default","membershipId":"mem_1","role":"owner","permissions":["tenant.manage"],"resolvedAt":"2026-04-24T10:01:00Z"}}"##,
        ),
        (
            r##"schemas/api/error-response.schema.json"##,
            r##"{"error":"tenant access denied","errorCode":"tenant_access_denied"}"##,
        ),
        (
            r##"schemas/events/tenant-access-denied.event.schema.json"##,
            r##"{"eventId":"evt_tenant_1","sequence":1,"category":"tenant","name":"tenant.access_denied","occurredAt":"2026-04-24T10:01:00Z","scope":{},"resource":{"kind":"tenant","id":"ten_hidden"},"payload":{"principalId":"prn_1","tokenId":"tok_1","tenantId":"ten_hidden","reasonCode":"tenant_resolution_denied"}}"##,
        ),
        (
            r##"schemas/events/tenant-context-resolved.event.schema.json"##,
            r##"{"eventId":"evt_tenant_2","sequence":2,"category":"tenant","name":"tenant.context_resolved","occurredAt":"2026-04-24T10:01:00Z","scope":{},"resource":{"kind":"tenant","id":"ten_1"},"payload":{"principalId":"prn_1","tokenId":"tok_1","tenantId":"ten_1","tenantSource":"default","membershipId":"mem_1","role":"owner"}}"##,
        ),
        (
            r##"schemas/events/tenant-membership-changed.event.schema.json"##,
            r##"{"eventId":"evt_tenant_3","sequence":3,"category":"tenant","name":"tenant.membership_changed","occurredAt":"2026-04-24T10:01:00Z","scope":{},"resource":{"kind":"tenant","id":"ten_1"},"payload":{"tenantId":"ten_1","principalId":"prn_2","membershipId":"mem_2","role":"viewer","status":"active","reasonCode":"membership_role_updated"}}"##,
        ),
        (
            r##"schemas/events/tenant-invitation-created.event.schema.json"##,
            r##"{"eventId":"evt_tenant_4","sequence":4,"category":"tenant","name":"tenant.invitation_created","occurredAt":"2026-04-24T10:01:00Z","scope":{},"resource":{"kind":"tenant","id":"ten_1"},"payload":{"tenantId":"ten_1","invitationId":"inv_1","invitedPrincipalId":"prn_2","role":"operator"}}"##,
        ),
        (
            r##"schemas/events/tenant-invitation-accepted.event.schema.json"##,
            r##"{"eventId":"evt_tenant_5","sequence":5,"category":"tenant","name":"tenant.invitation_accepted","occurredAt":"2026-04-24T10:01:00Z","scope":{},"resource":{"kind":"tenant","id":"ten_1"},"payload":{"tenantId":"ten_1","invitationId":"inv_1","invitedPrincipalId":"prn_2","role":"operator"}}"##,
        ),
        (
            r##"schemas/events/tenant-invitation-rejected.event.schema.json"##,
            r##"{"eventId":"evt_tenant_6","sequence":6,"category":"tenant","name":"tenant.invitation_rejected","occurredAt":"2026-04-24T10:01:00Z","scope":{},"resource":{"kind":"tenant","id":"ten_1"},"payload":{"tenantId":"ten_1","invitationId":"inv_1","invitedPrincipalId":"prn_2","role":"operator"}}"##,
        ),
        (
            r##"schemas/events/tenant-invitation-revoked.event.schema.json"##,
            r##"{"eventId":"evt_tenant_7","sequence":7,"category":"tenant","name":"tenant.invitation_revoked","occurredAt":"2026-04-24T10:01:00Z","scope":{},"resource":{"kind":"tenant","id":"ten_1"},"payload":{"tenantId":"ten_1","invitationId":"inv_1","invitedPrincipalId":"prn_2","role":"operator"}}"##,
        ),
        (
            r##"schemas/events/tenant-invitation-expired.event.schema.json"##,
            r##"{"eventId":"evt_tenant_8","sequence":8,"category":"tenant","name":"tenant.invitation_expired","occurredAt":"2026-04-24T10:01:00Z","scope":{},"resource":{"kind":"tenant","id":"ten_1"},"payload":{"tenantId":"ten_1","invitationId":"inv_1","invitedPrincipalId":"prn_2","role":"operator"}}"##,
        ),
        (
            r##"schemas/events/tenant-audit-failed-closed.event.schema.json"##,
            r##"{"eventId":"evt_tenant_9","sequence":9,"category":"tenant","name":"tenant.audit_failed_closed","occurredAt":"2026-04-24T10:01:00Z","scope":{},"resource":{"kind":"tenant","id":"ten_1"},"payload":{"tenantId":"ten_1","principalId":"prn_1","tokenId":"tok_1","reasonCode":"audit_write_failed"}}"##,
        ),
        (
            r##"schemas/events/tenant-token-issued.event.schema.json"##,
            r##"{"eventId":"evt_tenant_10","sequence":10,"category":"tenant","name":"tenant.token_issued","occurredAt":"2026-04-24T10:01:00Z","scope":{},"resource":{"kind":"token","id":"tok_2"},"payload":{"principalId":"prn_1","tokenId":"tok_2","defaultTenantId":"ten_1","allowedTenantIds":["ten_1"],"reasonCode":"token_issued"}}"##,
        ),
        (
            r##"schemas/events/tenant-token-rotated.event.schema.json"##,
            r##"{"eventId":"evt_tenant_11","sequence":11,"category":"tenant","name":"tenant.token_rotated","occurredAt":"2026-04-24T10:01:00Z","scope":{},"resource":{"kind":"token","id":"tok_1"},"payload":{"principalId":"prn_1","oldTokenId":"tok_1","newTokenId":"tok_2","reasonCode":"scheduled"}}"##,
        ),
        (
            r##"schemas/events/tenant-token-revoked.event.schema.json"##,
            r##"{"eventId":"evt_tenant_12","sequence":12,"category":"tenant","name":"tenant.token_revoked","occurredAt":"2026-04-24T10:01:00Z","scope":{},"resource":{"kind":"token","id":"tok_1"},"payload":{"principalId":"prn_1","tokenId":"tok_1","defaultTenantId":"ten_1","allowedTenantIds":["ten_1"],"reasonCode":"manual"}}"##,
        ),
        (
            r##"schemas/events/tenant-token-expiry-denied.event.schema.json"##,
            r##"{"eventId":"evt_tenant_13","sequence":13,"category":"tenant","name":"tenant.token_expiry_denied","occurredAt":"2026-04-24T10:01:00Z","scope":{},"resource":{"kind":"token","id":"tok_1"},"payload":{"principalId":"prn_1","tokenId":"tok_1","defaultTenantId":"ten_1","allowedTenantIds":["ten_1"],"reasonCode":"token_expired"}}"##,
        ),
        (
            r##"schemas/events/tenant-token-grants-changed.event.schema.json"##,
            r##"{"eventId":"evt_tenant_14","sequence":14,"category":"tenant","name":"tenant.token_grants_changed","occurredAt":"2026-04-24T10:01:00Z","scope":{},"resource":{"kind":"token","id":"tok_1"},"payload":{"principalId":"prn_1","tokenId":"tok_1","defaultTenantId":"ten_1","allowedTenantIds":["ten_1"],"reasonCode":"token_grants_changed"}}"##,
        ),
    ]
}

pub fn workflow_contract_fixtures() -> &'static [Fixture] {
    &[
        (
            r##"schemas/api/calendar-workflow-action.schema.json"##,
            r##"{"operationClass":"create_event","integrationId":"calendar-a","title":"Calendar workflow","startsAt":"2026-04-23T17:00:00Z","endsAt":"2026-04-23T17:30:00Z"}"##,
        ),
        (
            r##"schemas/api/create-workflow.request.schema.json"##,
            r##"{"goal":"Create a deterministic calendar workflow.","calendarAction":{"operationClass":"create_event","integrationId":"calendar-a","title":"Calendar workflow","startsAt":"2026-04-23T17:00:00Z","endsAt":"2026-04-23T17:30:00Z"}}"##,
        ),
        (
            r##"schemas/api/workflow-resource.schema.json"##,
            r##"{"workflowId":"wf_1","runId":"run_1","environmentScope":"test","goal":"Use MCP and a skill to complete a deterministic workflow.","status":"planned","planSummary":"Plan one MCP step followed by one executable skill handoff.","createdAt":"2026-04-21T10:00:00Z","updatedAt":"2026-04-21T10:00:00Z","steps":[{"workflowStepId":"wfstep_1","workflowId":"wf_1","title":"Use MCP tool lookup","position":1,"consumerKind":"mcp_tool","consumerId":"filesystem-test","toolName":"lookup","input":{"query":"deterministic workflow"},"status":"planned","selectionRationale":"Selected the first available MCP tool to satisfy the goal through the existing MCP runtime plane.","approvalModeExpected":"allow","attemptCount":0,"maxAttempts":1,"createdAt":"2026-04-21T10:00:00Z","updatedAt":"2026-04-21T10:00:00Z"},{"workflowStepId":"wfstep_2","workflowId":"wf_1","title":"Run executable skill exec-skill","position":2,"consumerKind":"skill","consumerId":"exec-skill","toolName":"exec-skill","input":{"args":["deterministic workflow"]},"status":"planned","selectionRationale":"Selected the first available executable skill to continue the workflow without a new execution boundary.","approvalModeExpected":"allow","dependencyIds":["wfdep_1"],"attemptCount":0,"maxAttempts":2,"createdAt":"2026-04-21T10:00:00Z","updatedAt":"2026-04-21T10:00:00Z"}],"dependencies":[{"dependencyId":"wfdep_1","workflowId":"wf_1","fromWorkflowStepId":"wfstep_1","toWorkflowStepId":"wfstep_2","dependencyType":"success","reason":"workflow consumes MCP output before local continuation"}],"handoffs":[{"handoffId":"wfhandoff_1","workflowId":"wf_1","fromWorkflowStepId":"wfstep_1","toWorkflowStepId":"wfstep_2","status":"pending","payloadSummary":"MCP lookup result summary","sourcePath":"step.output.result"}]}"##,
        ),
        (
            r##"schemas/api/workflow-list.response.schema.json"##,
            r##"{"items":[{"workflowId":"wf_1","runId":"run_1","environmentScope":"test","goal":"Use MCP and a skill to complete a deterministic workflow.","status":"planned","createdAt":"2026-04-21T10:00:00Z","updatedAt":"2026-04-21T10:00:00Z"}]}"##,
        ),
        (
            r##"schemas/events/workflow-planned.event.schema.json"##,
            r##"{"eventId":"evt_workflow_1","sequence":1,"category":"workflow","name":"workflow.planned","occurredAt":"2026-04-21T10:00:00Z","scope":{"runId":"run_1","workflowId":"wf_1"},"resource":{"kind":"workflow","id":"wf_1"},"payload":{"workflowId":"wf_1","runId":"run_1","status":"planned"}}"##,
        ),
        (
            r##"schemas/events/workflow-started.event.schema.json"##,
            r##"{"eventId":"evt_workflow_2","sequence":2,"category":"workflow","name":"workflow.started","occurredAt":"2026-04-21T10:00:01Z","scope":{"runId":"run_1","workflowId":"wf_1"},"resource":{"kind":"workflow","id":"wf_1"},"payload":{"workflowId":"wf_1","runId":"run_1","status":"running"}}"##,
        ),
        (
            r##"schemas/events/workflow-status-changed.event.schema.json"##,
            r##"{"eventId":"evt_workflow_3","sequence":3,"category":"workflow","name":"workflow.status_changed","occurredAt":"2026-04-21T10:00:02Z","scope":{"runId":"run_1","workflowId":"wf_1"},"resource":{"kind":"workflow","id":"wf_1"},"payload":{"workflowId":"wf_1","runId":"run_1","status":"completed"}}"##,
        ),
        (
            r##"schemas/events/workflow-step-status-changed.event.schema.json"##,
            r##"{"eventId":"evt_workflow_4","sequence":4,"category":"workflow","name":"workflow.step_status_changed","occurredAt":"2026-04-21T10:00:03Z","scope":{"runId":"run_1","workflowId":"wf_1","workflowStepId":"wfstep_2"},"resource":{"kind":"workflow","id":"wf_1"},"payload":{"workflowId":"wf_1","runId":"run_1","workflowStepId":"wfstep_2","status":"completed","runtimeStepId":"step_2","toolCallId":"tool_call_2","attempt":1,"toolName":"exec-skill","invocationKind":"skill","consumerId":"exec-skill","consumerKind":"skill"}}"##,
        ),
    ]
}
