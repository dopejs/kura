//! Ported Go event-builder behavioral tests plus serde round-trip coverage for
//! the kura-events constructor modules (daemon/internal/events/*.go).

use std::collections::HashMap;

use chrono::{DateTime, TimeZone, Utc};
use kura_events::*;

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 5, 11, 10, 0, 0).unwrap()
}

fn payload_str<'a>(event: &'a Event, key: &str) -> &'a str {
    event
        .payload
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
}

fn payload_i64(event: &Event, key: &str) -> i64 {
    event
        .payload
        .get(key)
        .and_then(|v| v.as_i64())
        .unwrap_or(-1)
}

fn payload_bool(event: &Event, key: &str) -> bool {
    event
        .payload
        .get(key)
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Builders for the kura-threads records (the crate deliberately has no
/// Default impls on these evidence records).
mod builders {
    use super::*;
    use kura_threads::*;

    pub fn continuity_turn(
        id: &str,
        tenant: &str,
        thread: &str,
        segment: &str,
        acceptance: i64,
        redaction: RedactionStatus,
        at: DateTime<Utc>,
    ) -> ContinuityTurn {
        ContinuityTurn {
            continuity_turn_id: id.into(),
            tenant_id: tenant.into(),
            thread_id: thread.into(),
            session_segment_id: segment.into(),
            acceptance_sequence: acceptance,
            role: ContinuityRole::User,
            source_kind: SourceKind::Chat,
            source_linkage_id: String::new(),
            source_message_id: String::new(),
            source_timestamp: None,
            dispatch_id: String::new(),
            response_to_turn_id: String::new(),
            safe_content: String::new(),
            content_redaction_status: redaction,
            artifact_excerpt_refs: Vec::new(),
            recorded_at: at,
            retention_expires_at: None,
            source_event_key: String::new(),
        }
    }

    pub fn continuity_preview(
        id: &str,
        tenant: &str,
        thread: &str,
        segment: &str,
        status: ContinuityStatus,
        at: DateTime<Utc>,
        redaction: RedactionStatus,
    ) -> ContinuityPreview {
        ContinuityPreview {
            continuity_preview_id: id.into(),
            tenant_id: tenant.into(),
            thread_id: thread.into(),
            session_segment_id: segment.into(),
            dispatch_id: String::new(),
            request_turn_id: String::new(),
            response_turn_id: String::new(),
            window_policy_id: String::new(),
            max_prior_turns: 0,
            active_window_days: 0,
            included_count: 0,
            excluded_count: 0,
            continuity_applied: false,
            status,
            failure_class: String::new(),
            assembly_started_at: at,
            assembly_completed_at: at,
            assembly_duration_ms: 0,
            retention_expires_at: at,
            redaction_status: redaction,
        }
    }

    pub fn participation_decision(
        id: &str,
        tenant: &str,
        thread: &str,
        segment: &str,
        connector: &str,
        shape: ConversationShape,
        decision: ParticipationDecisionValue,
        reason: &str,
        at: DateTime<Utc>,
        redaction: RedactionStatus,
    ) -> ParticipationDecision {
        ParticipationDecision {
            participation_decision_id: id.into(),
            tenant_id: tenant.into(),
            thread_id: thread.into(),
            session_segment_id: segment.into(),
            connector_id: connector.into(),
            connector_kind: String::new(),
            source_account_id: String::new(),
            source_conversation_id: String::new(),
            source_message_id: String::new(),
            conversation_shape: shape,
            policy_id: String::new(),
            mention_status: MentionStatus::Qualified,
            allowlist_status: AllowlistStatus::Eligible,
            decision,
            reason_code: reason.into(),
            created_assistant_work: false,
            occurred_at: Some(at),
            retention_expires_at: None,
            redaction_status: redaction,
            safe_summary: String::new(),
        }
    }

    pub fn reset_event(
        id: &str,
        tenant: &str,
        thread: &str,
        shape: ConversationShape,
        permission: &str,
        resulting_segment: &str,
        status: ResetEventStatus,
        reason: &str,
        at: DateTime<Utc>,
        redaction: RedactionStatus,
    ) -> ResetEvent {
        ResetEvent {
            reset_event_id: id.into(),
            tenant_id: tenant.into(),
            thread_id: thread.into(),
            conversation_shape: shape,
            source_conversation_id: String::new(),
            actor_principal_id: String::new(),
            permission_gate: permission.into(),
            prior_session_segment_id: String::new(),
            resulting_session_segment_id: resulting_segment.into(),
            status,
            reason_code: reason.into(),
            requested_at: None,
            completed_at: Some(at),
            audit_event_id: String::new(),
            retention_expires_at: None,
            redaction_status: redaction,
        }
    }

    pub fn handoff_link(
        id: &str,
        tenant: &str,
        source_thread: &str,
        destination_thread: &str,
        source_shape: ConversationShape,
        destination_shape: ConversationShape,
        status: HandoffStatus,
        reason: &str,
        permission: &str,
        source_ref: HandoffSourceReferenceStatus,
        at: DateTime<Utc>,
        redaction: RedactionStatus,
    ) -> HandoffLink {
        HandoffLink {
            handoff_link_id: id.into(),
            tenant_id: tenant.into(),
            source_thread_id: source_thread.into(),
            source_session_segment_id: String::new(),
            destination_thread_id: destination_thread.into(),
            destination_session_segment_id: String::new(),
            source_conversation_shape: source_shape,
            destination_conversation_shape: destination_shape,
            source_kind: None,
            destination_kind: None,
            source_connector_id: String::new(),
            destination_connector_id: String::new(),
            source_conversation_id: String::new(),
            destination_conversation_id: String::new(),
            actor_principal_id: String::new(),
            permission_gate: permission.into(),
            status,
            reason_code: reason.into(),
            first_destination_response_id: String::new(),
            source_reference_status: source_ref,
            active_profile_projection: None,
            created_at: Some(at),
            consumed_at: None,
            retention_expires_at: None,
            redaction_status: redaction,
        }
    }

    pub fn lifecycle_action(
        id: &str,
        tenant: &str,
        thread: &str,
        kind: LifecycleActionKind,
        status: &str,
        audit: &str,
        reason: &str,
        at: DateTime<Utc>,
        redaction: RedactionStatus,
    ) -> LifecycleAction {
        LifecycleAction {
            lifecycle_action_id: id.into(),
            thread_id: thread.into(),
            tenant_id: tenant.into(),
            action_kind: kind,
            actor_principal_id: String::new(),
            prior_state: LifecycleState::Active,
            resulting_state: LifecycleState::Reset,
            prior_session_segment_id: String::new(),
            resulting_session_segment_id: String::new(),
            reason_code: reason.into(),
            requested_at: at,
            completed_at: at,
            status: status.into(),
            audit_event_id: audit.into(),
            retention_expires_at: None,
            redaction_status: redaction,
        }
    }

    pub fn source_linkage(
        id: &str,
        tenant: &str,
        thread: &str,
        outcome: RoutingOutcome,
        at: DateTime<Utc>,
        redaction: RedactionStatus,
    ) -> SourceLinkage {
        SourceLinkage {
            source_linkage_id: id.into(),
            thread_id: thread.into(),
            tenant_id: tenant.into(),
            source_kind: SourceKind::Chat,
            connector_id: String::new(),
            connector_kind: String::new(),
            source_account_id: String::new(),
            source_conversation_id: String::new(),
            source_message_id: String::new(),
            routing_outcome: outcome,
            current: false,
            linked_at: Some(at),
            retention_expires_at: None,
            redaction_status: redaction,
        }
    }

    pub fn runtime_projection(
        id: &str,
        tenant: &str,
        thread: &str,
        kind: RuntimeResourceKind,
        resource_id: &str,
        status: &str,
        at: DateTime<Utc>,
        redaction: RedactionStatus,
    ) -> RuntimeProjection {
        RuntimeProjection {
            runtime_projection_id: id.into(),
            thread_id: thread.into(),
            tenant_id: tenant.into(),
            session_segment_id: String::new(),
            resource_kind: kind,
            resource_id: resource_id.into(),
            status: status.into(),
            reason_code: String::new(),
            occurred_at: at,
            route: String::new(),
            safe_summary: String::new(),
            retention_expires_at: None,
            redaction_status: redaction,
        }
    }
}

// ---- agent_profiles.go tests ----

#[test]
fn agent_profile_events_use_safe_metadata() {
    let lifecycle = agent_profile_lifecycle_event(AgentProfileLifecycleInput {
        tenant_id: "ten_1".into(),
        profile_id: "prof_1".into(),
        event_name: "agent_profile.created".into(),
        outcome: "succeeded".into(),
        reason_code: "user_created_profile".into(),
        safe_summary: "safe".into(),
        ..Default::default()
    });
    assert_eq!(lifecycle.category, "agent_profile");
    assert_eq!(payload_str(&lifecycle, "safeSummary"), "safe");
    assert_eq!(payload_str(&lifecycle, "redactionStatus"), "redacted");

    let projection = agent_profile_runtime_projected_event(kura_profiles::RuntimeProjection {
        runtime_profile_projection_id: "rpp_1".into(),
        tenant_id: "ten_1".into(),
        profile_id: "prof_1".into(),
        profile_version_id: "profv_1".into(),
        selection_id: "sel_1".into(),
        resource_kind: kura_profiles::RuntimeResourceKind::RUN,
        resource_id: "run_1".into(),
        selection_scope: "tenant_default".into(),
        selection_reason: kura_profiles::SelectionReason::DEFAULT_SEEDED,
        safe_display_name: "Agent".into(),
        safe_summary: "safe".into(),
        occurred_at: now(),
        redaction_status: kura_profiles::RedactionStatus::REDACTED,
        ..Default::default()
    });
    assert_eq!(projection.name, "agent_profile.runtime_projected");
    assert_eq!(projection.resource.kind, "run");
    assert_eq!(
        payload_str(&projection, "selectionReason"),
        "default_seeded"
    );
}

#[test]
fn agent_profile_lifecycle_event_names_cover_failure_retirement_and_rollback_outcomes() {
    let cases: Vec<(&str, &str, &str, &str)> = vec![
        (
            "validation failure",
            "agent_profile.validation_failed",
            "denied",
            "profile_validation_failed",
        ),
        (
            "permission denial",
            "agent_profile.permission_denied",
            "denied",
            "permission_denied",
        ),
        (
            "archive",
            "agent_profile.archived",
            "succeeded",
            "operator_retired_profile",
        ),
        (
            "disable",
            "agent_profile.disabled",
            "succeeded",
            "operator_retired_profile",
        ),
        (
            "retirement denial",
            "agent_profile.retirement_denied",
            "denied",
            "profile_not_found",
        ),
        (
            "safe fallback",
            "agent_profile.safe_default_fallback",
            "succeeded",
            "current_default_retired",
        ),
        (
            "rollback requested",
            "agent_profile.rollback_requested",
            "requested",
            "operator_reverted_persona",
        ),
        (
            "rollback succeeded",
            "agent_profile.rolled_back",
            "succeeded",
            "operator_reverted_persona",
        ),
        (
            "rollback denied",
            "agent_profile.rollback_denied",
            "denied",
            "profile_not_activatable",
        ),
        (
            "audit failed closed",
            "agent_profile.audit_failed_closed",
            "failed_closed",
            "audit_write_failed",
        ),
    ];
    for (case_name, event_name, outcome, reason_code) in cases {
        let event = agent_profile_lifecycle_event(AgentProfileLifecycleInput {
            tenant_id: "ten_1".into(),
            profile_id: "prof_1".into(),
            profile_version_id: "profv_1".into(),
            event_name: event_name.into(),
            outcome: outcome.into(),
            reason_code: reason_code.into(),
            permission_gate: "profiles.manage".into(),
            safe_summary: "safe metadata".into(),
            ..Default::default()
        });
        assert_eq!(event.name, event_name, "case {case_name}");
        assert_eq!(payload_str(&event, "outcome"), outcome, "case {case_name}");
        assert_eq!(
            payload_str(&event, "reasonCode"),
            reason_code,
            "case {case_name}"
        );
        assert_eq!(
            payload_str(&event, "safeSummary"),
            "safe metadata",
            "case {case_name}"
        );
        assert_eq!(
            payload_str(&event, "redactionStatus"),
            "redacted",
            "case {case_name}"
        );
    }
}

// ---- billing.go tests ----

#[test]
fn billing_event_projection() {
    let created_at = Utc.with_ymd_and_hms(2026, 4, 28, 10, 0, 0).unwrap();
    let event = billing_usage_event(
        BILLING_USAGE_RESERVED_NAME,
        kura_billing::UsageEvent {
            usage_event_id: "usage_event_1".into(),
            tenant_id: "ten_r38_a".into(),
            category: kura_billing::Category::from(kura_billing::Category::RUN_LAUNCHES),
            quota_period_id: "period_1".into(),
            operation_key: "tenant:ten_r38_a:run:client_1".into(),
            amount: 1,
            reason_code: "usage_reserved".into(),
            outcome: "reserved".into(),
            created_at,
            ..Default::default()
        },
    );
    assert_eq!(event.category, "billing");
    assert_eq!(event.name, BILLING_USAGE_RESERVED_NAME);
    assert_eq!(event.tenant_id, "ten_r38_a");
    assert_eq!(
        payload_str(&event, "operationKey"),
        "tenant:ten_r38_a:run:client_1"
    );
    assert_eq!(payload_i64(&event, "amount"), 1);
}

#[test]
fn billing_recovery_decision_event_projection() {
    let updated_at = Utc.with_ymd_and_hms(2026, 4, 28, 10, 10, 0).unwrap();
    let decision = kura_billing::RecoveryDecision {
        reservation: kura_billing::UsageReservation {
            reservation_id: "reservation_1".into(),
            tenant_id: "ten_r38_a".into(),
            category: kura_billing::Category::from(kura_billing::Category::RUN_LAUNCHES),
            quota_period_id: "period_1".into(),
            operation_key: "tenant:ten_r38_a:run:client_1".into(),
            status: kura_billing::ReservationStatus::from(
                kura_billing::ReservationStatus::OPERATOR_ACTION_NEEDED,
            ),
            updated_at,
            recovery_reason: "restart outcome could not be proven".into(),
            ..Default::default()
        },
        outcome: kura_billing::ReservationStatus::from(
            kura_billing::ReservationStatus::OPERATOR_ACTION_NEEDED,
        ),
        reason: "restart outcome could not be proven".into(),
    };
    let event = billing_recovery_decision_event(decision);
    assert_eq!(event.category, "billing");
    assert_eq!(event.name, BILLING_RESERVATION_RECOVERY_DECIDED_NAME);
    assert_eq!(event.resource.kind, "billing_reservation");
    assert_eq!(payload_str(&event, "outcome"), "operator_action_needed");
    assert!(!payload_str(&event, "reason").is_empty());
}

// ---- channel_management.go tests ----

#[test]
fn connector_management_events_carry_tenant_and_redaction_metadata() {
    let input = ConnectorManagementEventInput {
        tenant_id: "ten_channels".into(),
        connector_id: "matrix-main".into(),
        evidence_id: "support_1".into(),
        reason_code: "support_evidence_generated".into(),
        redaction_status: "redacted".into(),
        occurred_at: now(),
        ..Default::default()
    };
    let events = [
        connector_management_support_evidence_generated(input.clone()),
        connector_management_redaction_failed(input.clone()),
        connector_management_retention_applied(input),
    ];
    for event in &events {
        assert_eq!(event.tenant_id, "ten_channels");
        assert_eq!(event.scope.connector_id, "matrix-main");
        assert_eq!(event.category, "connector");
        assert_eq!(payload_str(event, "redactionStatus"), "redacted");
    }
    // Action/outcome defaults: action falls back to the event name, outcome to "succeeded".
    assert_eq!(
        events[0].name,
        "connector.management_support_evidence_generated"
    );
    assert_eq!(
        payload_str(&events[0], "action"),
        "connector.management_support_evidence_generated"
    );
    assert_eq!(payload_str(&events[0], "outcome"), "succeeded");
}

// ---- connector_delivery.go tests ----

#[test]
fn connector_delivery_events_accept_telegram_connector_evidence() {
    let failed = connector_foreground_reply_failed(ConnectorForegroundReplyFailedInput {
        tenant_id: "ten_telegram".into(),
        connector_id: "telegram-main".into(),
        message_delivery_id: "delivery_reply_1".into(),
        reason_code: "reply_failed".into(),
        retry_safety: "retryable".into(),
        background_delivery_id: "delivery_background_1".into(),
        separation_status: "separate_truths".into(),
    });
    assert_eq!(failed.name, "connector.foreground_reply_failed");
    assert_eq!(payload_str(&failed, "connectorId"), "telegram-main");
    assert_eq!(payload_str(&failed, "redactionStatus"), "redacted");
    assert_eq!(payload_str(&failed, "separationStatus"), "separate_truths");

    let separation = connector_delivery_separation_recorded(ConnectorDeliverySeparationInput {
        tenant_id: "ten_telegram".into(),
        connector_id: "telegram-main".into(),
        boundary_id: "boundary_1".into(),
        foreground_reply_outcome_id: "foreground_reply_1".into(),
        background_delivery_id: "delivery_background_1".into(),
        transport_kind: "telegram".into(),
        separation_status: "separate_truths".into(),
    });
    assert_eq!(separation.name, "connector.delivery_separation_recorded");
    assert_eq!(payload_str(&separation, "transportKind"), "telegram");
}

#[test]
fn connector_delivery_events_accept_slack_connector_evidence() {
    let failed = connector_foreground_reply_failed(ConnectorForegroundReplyFailedInput {
        tenant_id: "ten_slack".into(),
        connector_id: "slack-main".into(),
        message_delivery_id: "delivery_slack_reply_1".into(),
        reason_code: "reply_failed".into(),
        retry_safety: "retryable".into(),
        background_delivery_id: "delivery_slack_background_1".into(),
        separation_status: "separate_truths".into(),
    });
    assert_eq!(failed.name, "connector.foreground_reply_failed");
    assert_eq!(payload_str(&failed, "connectorId"), "slack-main");
    assert_eq!(payload_str(&failed, "redactionStatus"), "redacted");
    assert_eq!(
        payload_str(&failed, "backgroundDeliveryId"),
        "delivery_slack_background_1"
    );

    let separation = connector_delivery_separation_recorded(ConnectorDeliverySeparationInput {
        tenant_id: "ten_slack".into(),
        connector_id: "slack-main".into(),
        boundary_id: "boundary_slack_1".into(),
        foreground_reply_outcome_id: "foreground_slack_reply_1".into(),
        background_delivery_id: "delivery_slack_background_1".into(),
        transport_kind: "slack".into(),
        separation_status: "separate_truths".into(),
    });
    assert_eq!(separation.name, "connector.delivery_separation_recorded");
    assert_eq!(payload_str(&separation, "transportKind"), "slack");
}

// ---- connectors.go / connector_matrix.go tests ----

#[test]
fn matrix_connector_event_names_cover_phase52_evidence() {
    let want = [
        CONNECTOR_EVENT_MATRIX_SETUP_VALIDATED,
        CONNECTOR_EVENT_ROUTE_OUTCOME_RECORDED,
        CONNECTOR_EVENT_INBOUND_DUPLICATE_DETECTED,
        CONNECTOR_EVENT_REPLY_SENT,
        CONNECTOR_EVENT_REPLY_FAILED,
        CONNECTOR_EVENT_FOREGROUND_REPLY_FAILED,
        CONNECTOR_EVENT_DELIVERY_SEPARATION_RECORDED,
        CONNECTOR_EVENT_DIAGNOSTIC_STATE_CHANGED,
        CONNECTOR_EVENT_DIAGNOSTIC_REDACTION_FAILED,
        CONNECTOR_EVENT_MATRIX_SMOKE_EVIDENCE_RECORDED,
    ];
    for name in want {
        assert!(
            MATRIX_CONNECTOR_EVENT_NAMES.contains(&name),
            "MatrixConnectorEventNames missing {name}"
        );
    }
}

#[test]
fn matrix_connector_event_constructors_are_redacted() {
    let route = connector_matrix_route_outcome_recorded(ConnectorMatrixRouteOutcomeRecordedInput {
        tenant_id: "ten_matrix".into(),
        connector_id: "matrix-main".into(),
        homeserver_id: "matrix.example.org".into(),
        conversation_id: "!room:example.org".into(),
        matrix_event_id: "$event_redacted".into(),
        sync_batch_id: "batch_redacted".into(),
        transaction_id: "txn_redacted".into(),
        outcome: "accepted".into(),
        reason_code: "accepted".into(),
        surface: "room".into(),
        redaction_status: "redacted".into(),
    });
    assert_eq!(route.name, CONNECTOR_EVENT_ROUTE_OUTCOME_RECORDED);
    assert_eq!(payload_str(&route, "redactionStatus"), "redacted");

    let smoke =
        connector_matrix_smoke_evidence_recorded(ConnectorMatrixSmokeEvidenceRecordedInput {
            tenant_id: "ten_matrix".into(),
            connector_id: "matrix-main".into(),
            smoke_evidence_id: "matrix_smoke_1".into(),
            homeserver_binding_id: "matrix_hs_1".into(),
            status: "skipped".into(),
            authorization_mode: "unavailable".into(),
            owner: "operator".into(),
            reason: "safe_credentials_unavailable".into(),
            redaction_status: "redacted".into(),
            validated_at: now(),
            retention_expires_at: now() + chrono::Duration::days(90),
        });
    assert_eq!(smoke.name, CONNECTOR_EVENT_MATRIX_SMOKE_EVIDENCE_RECORDED);
    assert_eq!(smoke.resource.kind, "matrix_smoke_evidence");
    assert_eq!(payload_str(&smoke, "redactionStatus"), "redacted");
    assert_eq!(
        payload_str(&smoke, "reason"),
        "safe_credentials_unavailable"
    );
}

// ---- connector_slack.go / connector_telegram.go tests ----

#[test]
fn connector_slack_route_outcome_recorded_uses_shared_connector_contract() {
    let event = connector_slack_route_outcome_recorded(ConnectorSlackRouteOutcomeRecordedInput {
        tenant_id: "ten_slack".into(),
        connector_id: "slack-main".into(),
        workspace_id: "workspace_redacted".into(),
        conversation_id: "channel_redacted".into(),
        message_id: "message_redacted".into(),
        event_id: "event_redacted".into(),
        outcome: "blocked".into(),
        reason_code: "blocked_route".into(),
        surface: "channel".into(),
        redaction_status: "redacted".into(),
    });
    assert_eq!(event.name, "connector.route_outcome_recorded");
    assert_eq!(event.resource.kind, "connector_route_outcome");
    assert_eq!(payload_str(&event, "workspaceId"), "workspace_redacted");
    assert_eq!(payload_str(&event, "reasonCode"), "blocked_route");
}

#[test]
fn connector_telegram_setup_validated_builds_redacted_event() {
    let event = connector_telegram_setup_validated(ConnectorTelegramSetupValidatedInput {
        tenant_id: "ten_telegram".into(),
        connector_id: "telegram-main".into(),
        terminal_state: "ready".into(),
        hosted_ready: true,
        credential_state: "valid".into(),
        allowment_state: "valid".into(),
        reason_code: "healthy".into(),
        redaction_status: "redacted".into(),
        validated_at: Utc.with_ymd_and_hms(2026, 5, 8, 10, 1, 0).unwrap(),
    });
    assert_eq!(event.category, "connector");
    assert_eq!(event.name, "connector.telegram_setup_validated");
    assert_eq!(event.scope.connector_id, "telegram-main");
    assert_eq!(event.resource.kind, "telegram_hosted_setup");
    assert_eq!(payload_str(&event, "tenantId"), "ten_telegram");
    assert_eq!(payload_str(&event, "terminalState"), "ready");
    assert_eq!(payload_str(&event, "redactionStatus"), "redacted");
    assert!(payload_bool(&event, "hostedReady"));
}

// ---- integration_diagnostics.go tests ----

#[test]
fn integration_diagnostic_events() {
    let now = Utc.with_ymd_and_hms(2026, 4, 30, 10, 0, 0).unwrap();
    let run = kura_integrations::DiagnosticRun {
        diagnostic_run_id: "diag_run_1".into(),
        tenant_id: "ten_r42".into(),
        integration_id: "integration_feishu".into(),
        requested_by: "prn_operator".into(),
        status: kura_integrations::DiagnosticRunStatus::Completed,
        started_at: now,
        completed_at: Some(now),
        redaction_status: kura_integrations::RedactionStatus::Redacted,
        ..Default::default()
    };
    let run_event =
        integration_diagnostic_run_event(INTEGRATION_DIAGNOSTIC_RUN_COMPLETED_NAME, run);
    assert_eq!(run_event.name, INTEGRATION_DIAGNOSTIC_RUN_COMPLETED_NAME);
    assert_eq!(payload_str(&run_event, "diagnosticRunId"), "diag_run_1");
    assert_eq!(payload_str(&run_event, "redactionStatus"), "redacted");

    let result = kura_integrations::DiagnosticResult {
        diagnostic_result_id: "diag_result_1".into(),
        tenant_id: "ten_r42".into(),
        integration_id: "integration_feishu".into(),
        status: kura_integrations::DiagnosticStatus::Unknown,
        reason_code: kura_integrations::DiagnosticReasonCode::RedactionFailedClosed,
        redaction_status: kura_integrations::RedactionStatus::FailedClosed,
        checked_at: now,
        ..Default::default()
    };
    let redaction_event = integration_diagnostic_redaction_failed_event(result.clone());
    assert_eq!(
        redaction_event.name,
        INTEGRATION_DIAGNOSTIC_REDACTION_FAILED_NAME
    );
    assert_eq!(
        payload_str(&redaction_event, "redactionStatus"),
        "failed_closed"
    );
    assert_eq!(
        payload_str(&redaction_event, "targetKind"),
        "diagnostic_result"
    );

    let state_event = integration_diagnostic_state_changed_event(
        result,
        kura_integrations::DiagnosticStatus::Healthy,
    );
    assert_eq!(state_event.name, INTEGRATION_DIAGNOSTIC_STATE_CHANGED_NAME);
    assert_eq!(payload_str(&state_event, "previousStatus"), "healthy");
    assert_eq!(payload_str(&state_event, "status"), "unknown");
    assert_eq!(
        payload_str(&state_event, "reasonCode"),
        "redaction_failed_closed"
    );

    let smoke = kura_opsreadiness::SmokeMatrixReport {
        smoke_report_id: "smoke_1".into(),
        tenant_id: "ten_r42".into(),
        status: kura_opsreadiness::SmokeReportStatus::Completed,
        domain_summary: HashMap::from([("feishu".to_string(), "passed".to_string())]),
        artifact_refs: vec!["artifact_1".to_string()],
        completed_at: Some(now),
        ..Default::default()
    };
    let smoke_event = integration_diagnostic_smoke_completed_event(smoke);
    assert_eq!(
        smoke_event.name,
        INTEGRATION_DIAGNOSTIC_SMOKE_COMPLETED_NAME
    );
    assert_eq!(payload_str(&smoke_event, "smokeReportId"), "smoke_1");
    assert_eq!(payload_str(&smoke_event, "status"), "completed");
    assert_eq!(
        smoke_event.payload["domainSummary"],
        serde_json::json!({ "feishu": "passed" })
    );
    assert_eq!(
        smoke_event.payload["artifactRefs"],
        serde_json::json!(["artifact_1"])
    );

    let applied_at = now + chrono::Duration::minutes(1);
    let mut record = kura_integrations::new_diagnostic_retention_record(
        "ten_r42",
        "diagnostic_run",
        "diag_run_1",
        now,
    );
    record.retention_state = kura_integrations::DiagnosticRetentionState::Expired;
    record.applied_at = Some(applied_at);
    let retention_event = integration_diagnostic_retention_applied_event(record);
    assert_eq!(
        retention_event.name,
        INTEGRATION_DIAGNOSTIC_RETENTION_APPLIED_NAME
    );
    assert_eq!(payload_str(&retention_event, "retentionState"), "expired");
    assert_eq!(
        payload_str(&retention_event, "targetKind"),
        "diagnostic_run"
    );
}

// ---- thread_continuity.go tests ----

#[test]
fn thread_continuity_events_are_metadata_only() {
    use kura_threads::{ContinuityStatus, RedactionStatus};
    let now = now();
    let turn_event = thread_continuity_turn_recorded_event(
        builders::continuity_turn(
            "turn_1",
            "ten_1",
            "thr_1",
            "seg_1",
            7,
            RedactionStatus::Redacted,
            now,
        ),
        "",
    );
    assert_eq!(turn_event.name, THREAD_CONTINUITY_TURN_RECORDED_NAME);
    assert_eq!(payload_i64(&turn_event, "acceptanceSequence"), 7);
    assert_eq!(payload_str(&turn_event, "outcome"), "recorded");
    assert_eq!(payload_str(&turn_event, "reasonCode"), "included_recent");
    assert_eq!(turn_event.scope.session_id, "seg_1");
    assert_eq!(payload_str(&turn_event, "redactionStatus"), "redacted");
    assert!(
        turn_event.payload.get("safeContent").is_none(),
        "turn event leaked content"
    );

    let preview_event = thread_continuity_preview_recorded_event(builders::continuity_preview(
        "contprev_1",
        "ten_1",
        "thr_1",
        "seg_1",
        ContinuityStatus::Applied,
        now,
        RedactionStatus::Redacted,
    ));
    assert_eq!(preview_event.name, THREAD_CONTINUITY_PREVIEW_RECORDED_NAME);
    assert_eq!(payload_str(&preview_event, "outcome"), "applied");
    assert_eq!(payload_str(&preview_event, "action"), "preview_recorded");
    assert!(
        preview_event.payload.get("items").is_none(),
        "preview event leaked item detail"
    );
}

// ---- thread_group_room.go tests ----

#[test]
fn group_room_reset_handoff_events_use_safe_metadata() {
    use kura_threads::{
        ConversationShape, GROUP_ROOM_REASON_MISSING_QUALIFYING_MENTION,
        GROUP_ROOM_REASON_SCOPED_RESET_SUCCEEDED, HandoffSourceReferenceStatus, HandoffStatus,
        ParticipationDecisionValue, RedactionStatus, ResetEventStatus,
    };
    let now = now();

    let participation = thread_participation_decision_event(builders::participation_decision(
        "part_1",
        "ten_1",
        "thr_1",
        "seg_1",
        "slack-main",
        ConversationShape::Room,
        ParticipationDecisionValue::Ignored,
        GROUP_ROOM_REASON_MISSING_QUALIFYING_MENTION,
        now,
        RedactionStatus::Redacted,
    ));
    assert_eq!(
        participation.name,
        THREAD_PARTICIPATION_DECISION_RECORDED_NAME
    );
    assert_eq!(payload_str(&participation, "decision"), "ignored");
    assert_eq!(payload_str(&participation, "redactionStatus"), "redacted");
    assert_eq!(payload_str(&participation, "conversationShape"), "room");

    let reset = thread_scoped_reset_evidence_event(builders::reset_event(
        "reset_1",
        "ten_1",
        "thr_1",
        ConversationShape::Room,
        "connectors.manage",
        "seg_2",
        ResetEventStatus::Succeeded,
        GROUP_ROOM_REASON_SCOPED_RESET_SUCCEEDED,
        now,
        RedactionStatus::Redacted,
    ));
    assert_eq!(reset.name, THREAD_RESET_SCOPED_NAME);
    assert_eq!(payload_str(&reset, "conversationShape"), "room");
    assert_eq!(payload_str(&reset, "permissionGate"), "connectors.manage");
    assert_eq!(payload_str(&reset, "status"), "succeeded");

    let handoff = thread_handoff_linked_event(builders::handoff_link(
        "handoff_1",
        "ten_1",
        "thr_source",
        "thr_destination",
        ConversationShape::Room,
        ConversationShape::Web,
        HandoffStatus::Succeeded,
        "user_requested_handoff",
        "connectors.manage",
        HandoffSourceReferenceStatus::Available,
        now,
        RedactionStatus::Redacted,
    ));
    assert_eq!(handoff.name, THREAD_HANDOFF_LINKED_NAME);
    assert_eq!(payload_str(&handoff, "sourceThreadId"), "thr_source");
    assert_eq!(payload_str(&handoff, "sourceReferenceStatus"), "available");
    assert_eq!(payload_str(&handoff, "destinationConversationShape"), "web");
}

// ---- thread_lifecycle.go tests ----

#[test]
fn thread_lifecycle_events() {
    use kura_threads::{LifecycleActionKind, RedactionStatus};
    let now = now();
    let event = thread_lifecycle_event(builders::lifecycle_action(
        "action_1",
        "ten_1",
        "thr_1",
        LifecycleActionKind::Reset,
        "succeeded",
        "audit_1",
        "user_requested_reset",
        now,
        RedactionStatus::Redacted,
    ));
    assert_eq!(event.category, "thread");
    assert_eq!(event.name, "thread.lifecycle_reset");
    assert_eq!(event.tenant_id, "ten_1");
    assert_eq!(payload_str(&event, "auditEventId"), "audit_1");
    assert_eq!(event.resource.id, "thr_1");
    assert_eq!(payload_str(&event, "action"), "reset");

    let archive = thread_lifecycle_event(builders::lifecycle_action(
        "action_2",
        "ten_1",
        "thr_2",
        LifecycleActionKind::Archive,
        "succeeded",
        "",
        "",
        now,
        RedactionStatus::Redacted,
    ));
    assert_eq!(archive.name, "thread.lifecycle_archived");
    let reopen = thread_lifecycle_event(builders::lifecycle_action(
        "action_3",
        "ten_1",
        "thr_2",
        LifecycleActionKind::Reopen,
        "succeeded",
        "",
        "",
        now,
        RedactionStatus::Redacted,
    ));
    assert_eq!(reopen.name, "thread.lifecycle_reopened");
}

#[test]
fn thread_source_runtime_retention_and_failure_events() {
    use kura_threads::{RedactionStatus, RoutingOutcome, RuntimeResourceKind};
    let now = now();

    let source = thread_source_linked_event(builders::source_linkage(
        "src_1",
        "ten_1",
        "thr_1",
        RoutingOutcome::Accepted,
        now,
        RedactionStatus::Redacted,
    ));
    assert_eq!(source.name, THREAD_SOURCE_LINKED_NAME);
    assert_eq!(payload_str(&source, "routingOutcome"), "accepted");

    let runtime = thread_runtime_projection_event(builders::runtime_projection(
        "rtp_1",
        "ten_1",
        "thr_1",
        RuntimeResourceKind::Run,
        "run_1",
        "completed",
        now,
        RedactionStatus::Redacted,
    ));
    assert_eq!(runtime.name, THREAD_RUNTIME_PROJECTION_NAME);
    assert_eq!(payload_str(&runtime, "resourceKind"), "run");

    let retention =
        thread_retention_applied_event("ten_1", "thr_1", now, RedactionStatus::Redacted);
    assert_eq!(retention.name, THREAD_RETENTION_APPLIED_NAME);
    assert!(!payload_str(&retention, "retentionExpiresAt").is_empty());
    assert_eq!(payload_str(&retention, "redactionStatus"), "redacted");

    let redaction = thread_redaction_failed_event("ten_1", "thr_1", "unsafe_provider_detail");
    assert_eq!(redaction.name, THREAD_REDACTION_FAILED_NAME);
    assert_eq!(
        payload_str(&redaction, "reasonCode"),
        "unsafe_provider_detail"
    );
    assert_eq!(payload_str(&redaction, "outcome"), "redaction_failed");
    assert_eq!(
        payload_str(&redaction, "redactionStatus"),
        "redaction_failed"
    );

    let audit = thread_audit_failed_closed_event("ten_1", "thr_1", "audit_unavailable");
    assert_eq!(audit.name, THREAD_AUDIT_FAILED_CLOSED_NAME);
    assert_eq!(payload_str(&audit, "outcome"), "failed_closed");
    assert_eq!(payload_str(&audit, "redactionStatus"), "redacted");

    let recovery = thread_restart_recovery_event("ten_1", 4, 1, 1);
    assert_eq!(recovery.name, THREAD_RESTART_RECOVERED_NAME);
    assert_eq!(payload_i64(&recovery, "partialThreadStates"), 1);
    assert_eq!(payload_i64(&recovery, "checkedThreads"), 4);
    assert_eq!(payload_str(&recovery, "semanticMemoryInteraction"), "none");
    assert_eq!(payload_str(&recovery, "outcome"), "recovered");
}

// ---- live_validation.go tests (no Go test file; behavioral + serde coverage) ----

#[test]
fn live_validation_events_project_attempt_and_ledger_evidence() {
    use kura_livevalidation::{AttemptStatus, ComparisonStatus, ReconciliationResolutionValue};
    let now = now();

    let attempt = kura_livevalidation::Attempt {
        validation_id: "lv_1".into(),
        tenant_id: "ten_1".into(),
        candidate_id: "cand_1".into(),
        environment_scope: "test".into(),
        status: AttemptStatus::from(AttemptStatus::BLOCKED),
        updated_at: now,
        ..Default::default()
    };
    let denial = kura_livevalidation::Denial {
        gate: "permission".into(),
        reason_code: "permission_denied".into(),
        message: "blocked by permission gate".into(),
        reference: String::new(),
    };
    let event = live_validation_attempt_event(LIVE_VALIDATION_BLOCKED_NAME, attempt, &[denial]);
    assert_eq!(event.category, "evaluation");
    assert_eq!(event.name, LIVE_VALIDATION_BLOCKED_NAME);
    assert_eq!(payload_str(&event, "validationId"), "lv_1");
    assert_eq!(payload_str(&event, "status"), "blocked");
    assert_eq!(payload_str(&event, "gate"), "permission");
    assert_eq!(payload_str(&event, "reasonCode"), "permission_denied");
    assert_eq!(
        event.payload["denials"],
        serde_json::json!([{ "gate": "permission", "reasonCode": "permission_denied", "message": "blocked by permission gate" }])
    );

    // LedgerOutcome is not re-exported by kura-livevalidation, so build the
    // entry from JSON (the outcome type is a transparent string newtype).
    let entry: kura_livevalidation::SideEffectLedgerEntry =
        serde_json::from_value(serde_json::json!({
            "ledgerEntryId": "ledger_1",
            "validationId": "lv_1",
            "tenantId": "ten_1",
            "candidateId": "cand_1",
            "sourceRef": "run_1",
            "toolClass": "mcp.tool_call",
            "safetyClass": "read_only",
            "actionRef": "mcp.call",
            "outcome": "completed",
            "ambiguousCommit": false,
            "retryCount": 0,
            "updatedAt": now.to_rfc3339(),
        }))
        .unwrap();
    let ledger = live_validation_ledger_event(LIVE_VALIDATION_SIDE_EFFECT_RECORDED_NAME, entry);
    assert_eq!(ledger.name, LIVE_VALIDATION_SIDE_EFFECT_RECORDED_NAME);
    assert_eq!(payload_str(&ledger, "toolClass"), "mcp.tool_call");
    assert_eq!(payload_str(&ledger, "outcome"), "completed");
    assert!(!payload_bool(&ledger, "ambiguousCommit"));

    let comparison = kura_livevalidation::Comparison {
        comparison_id: "comp_1".into(),
        validation_id: "lv_1".into(),
        terminal_status: ComparisonStatus::from(ComparisonStatus::MATCHED),
        generated_at: now,
        ..Default::default()
    };
    let comp_event = live_validation_comparison_event(comparison);
    assert_eq!(comp_event.name, LIVE_VALIDATION_COMPARISON_COMPLETED_NAME);
    assert_eq!(payload_str(&comp_event, "terminalStatus"), "matched");

    let resolution = kura_livevalidation::ReconciliationResolution {
        reconciliation_id: "recon_1".into(),
        ambiguous_commit_id: "amb_1".into(),
        tenant_id: "ten_1".into(),
        resolved_by: "prn_operator".into(),
        resolution: ReconciliationResolutionValue::from(
            ReconciliationResolutionValue::CONFIRMED_COMMITTED,
        ),
        resolved_at: now,
        ..Default::default()
    };
    let recon = live_validation_reconciliation_event(resolution);
    assert_eq!(recon.name, LIVE_VALIDATION_RECONCILIATION_RESOLVED_NAME);
    assert_eq!(payload_str(&recon, "ambiguousCommitId"), "amb_1");
    assert_eq!(payload_str(&recon, "resolution"), "confirmed_committed");
    assert_eq!(payload_str(&recon, "resolvedBy"), "prn_operator");
}

// ---- workspace_capability_bindings.go tests ----

#[test]
fn binding_lifecycle_and_visibility_events_are_redacted() {
    let lifecycle = binding_lifecycle_event(BindingLifecycleInput {
        tenant_id: "ten_1".into(),
        binding_id: "bind_1".into(),
        workspace_id: "ws_1".into(),
        event_name: "binding.created".into(),
        outcome: "succeeded".into(),
        permission_gate: "bindings.manage".into(),
        safe_summary: "safe metadata".into(),
        previous_selection_summary: "previous".into(),
        resulting_selection_summary: "resulting".into(),
        audit_event_id: "audit_1".into(),
        ..Default::default()
    });
    assert_eq!(lifecycle.category, "binding");
    assert_eq!(lifecycle.name, "binding.created");
    assert_eq!(lifecycle.resource.kind, "workspace_capability_binding");
    assert_eq!(lifecycle.resource.id, "bind_1");
    assert_eq!(payload_str(&lifecycle, "redactionStatus"), "redacted");
    assert_eq!(payload_str(&lifecycle, "safeSummary"), "safe metadata");

    // Empty binding id falls back to the workspace id.
    let fallback = binding_lifecycle_event(BindingLifecycleInput {
        tenant_id: "ten_1".into(),
        workspace_id: "ws_1".into(),
        event_name: "binding.created".into(),
        ..Default::default()
    });
    assert_eq!(fallback.resource.id, "ws_1");

    let visibility = capability_visibility_changed_event(CapabilityVisibilityChangedInput {
        tenant_id: "ten_1".into(),
        actor_principal_id: "prn_1".into(),
        scope_kind: kura_bindings::VisibilityScopeKind::from(
            kura_bindings::VisibilityScopeKind::PROFILE,
        ),
        scope_ref: "profile_1".into(),
        capability_id: "cap_1".into(),
        visibility: kura_bindings::Visibility::from(kura_bindings::Visibility::VISIBLE),
        audit_event_id: "audit_2".into(),
    });
    assert_eq!(visibility.name, "capability_visibility.changed");
    assert_eq!(visibility.resource.kind, "capability_visibility_policy");
    assert_eq!(payload_str(&visibility, "scopeKind"), "profile");
    assert_eq!(payload_str(&visibility, "visibility"), "visible");
    assert_eq!(payload_str(&visibility, "redactionStatus"), "redacted");

    let projected = binding_runtime_projected_event(kura_bindings::RuntimeBindingEvidence {
        projection_id: "proj_1".into(),
        tenant_id: "ten_1".into(),
        resource_kind: "run".into(),
        resource_id: "run_1".into(),
        selected_profile_id: "prof_1".into(),
        selected_profile_version_id: "profv_1".into(),
        selected_workspace_id: "ws_1".into(),
        binding_scope: kura_bindings::BindingRuntimeScope::from(
            kura_bindings::BindingRuntimeScope::TENANT_DEFAULT,
        ),
        binding_id: "bind_1".into(),
        classification: kura_bindings::Classification::from(kura_bindings::Classification::DEFAULT),
        selection_reason: "tenant_default".into(),
        capability_visibility: vec![],
        occurred_at: now(),
        redaction_status: kura_bindings::RedactionStatus::REDACTED,
    });
    assert_eq!(projected.name, "binding.runtime_projected");
    assert_eq!(payload_str(&projected, "bindingScope"), "tenant_default");
    assert_eq!(payload_str(&projected, "classification"), "default_binding");
    assert_eq!(payload_str(&projected, "redactionStatus"), "redacted");
}

// ---- serde round-trip coverage ----

#[test]
fn input_structs_serialize_camel_case() {
    let input = AgentProfileLifecycleInput {
        tenant_id: "ten_1".into(),
        profile_id: "prof_1".into(),
        profile_version_id: "profv_1".into(),
        actor_principal_id: "prn_1".into(),
        event_name: "agent_profile.created".into(),
        outcome: "succeeded".into(),
        reason_code: "user_created_profile".into(),
        permission_gate: "profiles.manage".into(),
        safe_summary: "safe".into(),
        audit_event_id: "audit_1".into(),
        redaction_status: kura_profiles::RedactionStatus::REDACTED,
    };
    let json = serde_json::to_value(&input).unwrap();
    assert_eq!(json["tenantId"], "ten_1");
    assert_eq!(json["profileId"], "prof_1");
    assert_eq!(json["profileVersionId"], "profv_1");
    assert_eq!(json["actorPrincipalId"], "prn_1");
    assert_eq!(json["eventName"], "agent_profile.created");
    assert_eq!(json["permissionGate"], "profiles.manage");
    assert_eq!(json["auditEventId"], "audit_1");
    assert_eq!(json["redactionStatus"], "redacted");

    let management = ConnectorManagementEventInput {
        tenant_id: "ten_1".into(),
        connector_id: "matrix-main".into(),
        evidence_id: "support_1".into(),
        redaction_status: "redacted".into(),
        occurred_at: now(),
        ..Default::default()
    };
    let json = serde_json::to_value(&management).unwrap();
    assert_eq!(json["connectorId"], "matrix-main");
    assert_eq!(json["evidenceId"], "support_1");
    assert_eq!(
        json["occurredAt"],
        now().to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
    );

    let setup = ConnectorDiscordSetupValidatedInput {
        tenant_id: "ten_1".into(),
        connector_id: "discord-main".into(),
        readiness_state: "ready".into(),
        hosted_ready: true,
        credential_state: "valid".into(),
        reason_code: "healthy".into(),
        redaction_status: "redacted".into(),
        validated_at: now(),
    };
    let json = serde_json::to_value(&setup).unwrap();
    assert_eq!(json["readinessState"], "ready");
    assert_eq!(json["hostedReady"], true);
    assert_eq!(
        json["validatedAt"],
        now().to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
    );
}

#[test]
fn event_round_trips_with_camel_case_payload() {
    let event = connector_discord_setup_validated(ConnectorDiscordSetupValidatedInput {
        tenant_id: "ten_1".into(),
        connector_id: "discord-main".into(),
        readiness_state: "ready".into(),
        hosted_ready: true,
        credential_state: "valid".into(),
        reason_code: "healthy".into(),
        redaction_status: "redacted".into(),
        validated_at: now(),
    });
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["category"], "connector");
    assert_eq!(json["name"], "connector.discord_setup_validated");
    assert_eq!(json["scope"]["connectorId"], "discord-main");
    assert_eq!(json["resource"]["kind"], "discord_hosted_setup");
    assert_eq!(json["payload"]["tenantId"], "ten_1");
    assert_eq!(json["payload"]["readinessState"], "ready");
    assert_eq!(json["payload"]["hostedReady"], true);

    let back: Event = serde_json::from_value(json).unwrap();
    assert_eq!(back.category, "connector");
    assert_eq!(back.name, "connector.discord_setup_validated");
    assert_eq!(back.payload["redactionStatus"], "redacted");
}

#[test]
fn connector_route_outcome_resource_id_falls_back_to_connector_id() {
    let with_delivery = connector_route_outcome_recorded(ConnectorRouteOutcomeInput {
        tenant_id: "ten_1".into(),
        connector_id: "slack-main".into(),
        outcome: "accepted".into(),
        message_delivery_id: "delivery_1".into(),
        ..Default::default()
    });
    assert_eq!(with_delivery.resource.id, "delivery_1");

    let without_delivery = connector_route_outcome_recorded(ConnectorRouteOutcomeInput {
        tenant_id: "ten_1".into(),
        connector_id: "slack-main".into(),
        outcome: "accepted".into(),
        ..Default::default()
    });
    assert_eq!(without_delivery.resource.id, "slack-main");
}

#[test]
fn thread_wire_strings_surface_in_event_payloads() {
    // The wire helpers are pub(crate); verify their output through the events.
    let retention = thread_retention_applied_event(
        "ten_1",
        "thr_1",
        now(),
        kura_threads::RedactionStatus::RedactionFailed,
    );
    assert_eq!(
        payload_str(&retention, "redactionStatus"),
        "redaction_failed"
    );
    let redaction = thread_redaction_failed_event("ten_1", "thr_1", "unsafe");
    assert_eq!(
        payload_str(&redaction, "redactionStatus"),
        "redaction_failed"
    );
}
