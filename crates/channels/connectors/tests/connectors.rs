//! Serde round-trip and behavioral tests for the kura-connectors wire types,
//! the connector supervisor manager, and the conformance helpers. JSON
//! assertions check the camelCase field names and snake_case enum values
//! against the Go json tags; the behavioral tests port supervisor_test.go and
//! conformance_test.go.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Duration, Utc};
use kura_connectors::{
    AccountBindingSummary, CONNECTOR_KIND_MATRIX, CapabilityProfile, ConformanceArea,
    ConformanceResult, ConformanceResultStatus, Connector, ConnectorDiagnosticState,
    ConnectorsError, DiagnosticReasonCode, FreshnessState, GROUP_ROOM_SURFACE_ALLOWLIST_EVIDENCE,
    GROUP_ROOM_SURFACE_DELETED_MESSAGE_EVIDENCE, GROUP_ROOM_SURFACE_DUPLICATE_MESSAGE_EVIDENCE,
    GROUP_ROOM_SURFACE_EDITED_MESSAGE_EVIDENCE, GROUP_ROOM_SURFACE_MENTION_EVIDENCE,
    GROUP_ROOM_SURFACE_UNSUPPORTED_SOURCE_EVIDENCE, GroupRoomCapabilities,
    HANDOFF_SURFACE_DESTINATION_SUPPORT, HANDOFF_SURFACE_FIRST_RESPONSE_SOURCE_REFERENCES,
    HANDOFF_SURFACE_SOURCE_SUPPORT, HandoffCapabilities, LifecycleState, MatrixCase,
    RedactionStatus, RegisterInput, RemediationOwner, ReportFailureInput, ReportHealthInput,
    RetrySafety, Status, Supervisor, SurfaceSupport, clean_strings, core_invariant_areas,
    core_reason_code, min_int, run_matrix_case, surface_result, validate_capability_profile,
};
use kura_livevalidation::FakeOutcome;

fn ts(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}

fn passing_core() -> HashMap<ConformanceArea, ConformanceResultStatus> {
    core_invariant_areas()
        .into_iter()
        .map(|area| (area, ConformanceResultStatus::Pass))
        .collect()
}

// --- serde round-trips ----------------------------------------------------

#[test]
fn connector_roundtrips_camel_case_fields_and_enum_values() {
    let connector = Connector {
        tenant_id: "tenant-a".to_string(),
        connector_id: "conn-1".to_string(),
        kind: "matrix".to_string(),
        display_name: "Matrix".to_string(),
        status: Status::Healthy,
        disabled_reason: String::new(),
        secret_refs: vec!["ref/matrix/token".to_string()],
        secret_summary: vec![],
        failure_count: 3,
        restart_count: 1,
        backoff_seconds: 30,
        next_restart_at: None,
        last_restart_at: None,
        last_heartbeat_at: None,
        last_failure_reason: "timeout".to_string(),
        capability_profile: Default::default(),
        diagnostic_state: Default::default(),
        conformance_result: Default::default(),
        account_binding: Default::default(),
        created_at: ts("2025-01-02T03:04:05Z"),
        updated_at: ts("2025-01-02T04:05:06Z"),
    };

    let json = serde_json::to_value(&connector).unwrap();
    let obj = json.as_object().unwrap();

    // camelCase field names per supervisor.go json tags.
    assert!(obj.contains_key("tenantId"));
    assert!(obj.contains_key("connectorId"));
    assert!(obj.contains_key("displayName"));
    assert!(obj.contains_key("secretRefs"));
    assert!(obj.contains_key("failureCount"));
    assert!(obj.contains_key("restartCount"));
    assert!(obj.contains_key("backoffSeconds"));
    assert!(obj.contains_key("lastFailureReason"));
    assert!(obj.contains_key("createdAt"));
    assert!(obj.contains_key("updatedAt"));

    // snake_case enum wire value.
    assert_eq!(obj["status"], serde_json::json!("healthy"));
    // Go's omitempty: empty optional fields are absent.
    assert!(!obj.contains_key("disabledReason"));
    assert!(!obj.contains_key("secretSummary"));
    assert!(!obj.contains_key("nextRestartAt"));
    assert!(!obj.contains_key("lastRestartAt"));
    assert!(!obj.contains_key("lastHeartbeatAt"));
    assert!(!obj.contains_key("capabilityProfile"));
    assert!(!obj.contains_key("diagnosticState"));
    assert!(!obj.contains_key("conformanceResult"));
    assert!(!obj.contains_key("accountBinding"));

    let back: Connector = serde_json::from_value(json).unwrap();
    assert_eq!(back, connector);
}

#[test]
fn conformance_result_roundtrips_camel_case_fields_and_enum_values() {
    let result = ConformanceResult {
        conformance_result_id: "conf_scenario-1_redaction".to_string(),
        tenant_id: "tenant-a".to_string(),
        connector_kind: "matrix".to_string(),
        connector_id: "conn-1".to_string(),
        scenario_id: "scenario-1".to_string(),
        area: "redaction".to_string(),
        result: ConformanceResultStatus::Pass,
        reason_code: String::new(),
        redaction_status: RedactionStatus::Redacted,
        evidence_timestamp: ts("2025-01-02T03:04:05Z"),
        retention_expires_at: ts("2025-04-02T03:04:05Z"),
    };

    let json = serde_json::to_value(&result).unwrap();
    let obj = json.as_object().unwrap();

    assert!(obj.contains_key("conformanceResultId"));
    assert!(obj.contains_key("tenantId"));
    assert!(obj.contains_key("connectorKind"));
    assert!(obj.contains_key("connectorId"));
    assert!(obj.contains_key("scenarioId"));
    assert!(obj.contains_key("area"));
    assert!(obj.contains_key("result"));
    assert!(obj.contains_key("redactionStatus"));
    assert!(obj.contains_key("evidenceTimestamp"));
    assert!(obj.contains_key("retentionExpiresAt"));
    // snake_case enum wire values.
    assert_eq!(obj["result"], serde_json::json!("pass"));
    assert_eq!(obj["redactionStatus"], serde_json::json!("redacted"));
    // omitempty
    assert!(!obj.contains_key("reasonCode"));

    let back: ConformanceResult = serde_json::from_value(json).unwrap();
    assert_eq!(back, result);
}

#[test]
fn connector_diagnostic_state_roundtrips_camel_case_fields_and_enum_values() {
    let state = ConnectorDiagnosticState {
        diagnostic_state_id: "diag_conn-1_rate_limited".to_string(),
        tenant_id: "tenant-a".to_string(),
        connector_id: "conn-1".to_string(),
        connector_account_id: "acct-9".to_string(),
        status: LifecycleState::RateLimited,
        reason_code: DiagnosticReasonCode::RateLimited,
        remediation_owner: RemediationOwner::Provider,
        user_visible_severity: "warning".to_string(),
        retry_safety: RetrySafety::RetryAfter,
        evidence_timestamp: ts("2025-01-02T03:04:05Z"),
        freshness_state: FreshnessState::Fresh,
        redaction_status: RedactionStatus::Redacted,
        retention_expires_at: ts("2025-04-02T03:04:05Z"),
        safe_evidence: HashMap::from([("scope".to_string(), "rate_limited".to_string())]),
        redaction_failure_id: String::new(),
    };

    let json = serde_json::to_value(&state).unwrap();
    let obj = json.as_object().unwrap();

    assert!(obj.contains_key("diagnosticStateId"));
    assert!(obj.contains_key("tenantId"));
    assert!(obj.contains_key("connectorId"));
    assert!(obj.contains_key("connectorAccountId"));
    assert!(obj.contains_key("userVisibleSeverity"));
    assert!(obj.contains_key("evidenceTimestamp"));
    assert!(obj.contains_key("freshnessState"));
    assert!(obj.contains_key("retentionExpiresAt"));
    assert!(obj.contains_key("safeEvidence"));

    // snake_case enum wire values.
    assert_eq!(obj["status"], serde_json::json!("rate_limited"));
    assert_eq!(obj["reasonCode"], serde_json::json!("rate_limited"));
    assert_eq!(obj["remediationOwner"], serde_json::json!("provider"));
    assert_eq!(obj["retrySafety"], serde_json::json!("retry_after"));
    assert_eq!(obj["freshnessState"], serde_json::json!("fresh"));
    assert_eq!(obj["redactionStatus"], serde_json::json!("redacted"));
    // omitempty
    assert!(!obj.contains_key("redactionFailureId"));

    let back: ConnectorDiagnosticState = serde_json::from_value(json).unwrap();
    assert_eq!(back, state);
}

#[test]
fn enum_wire_values_match_go_constants() {
    // Spot-check every enum against the Go string constants.
    assert_eq!(Status::Registered.as_str(), "registered");
    assert_eq!(Status::BackingOff.as_str(), "backing_off");
    assert_eq!(Status::Disabled.as_str(), "disabled");

    assert_eq!(LifecycleState::Configured.as_str(), "configured");
    assert_eq!(
        LifecycleState::PermissionBlocked.as_str(),
        "permission_blocked"
    );
    assert_eq!(
        LifecycleState::UnsupportedCapability.as_str(),
        "unsupported_capability"
    );

    assert_eq!(ConformanceResultStatus::Fail.as_str(), "fail");
    assert_eq!(ConformanceResultStatus::Limited.as_str(), "limited");

    assert_eq!(RedactionStatus::Redacted.as_str(), "redacted");
    assert_eq!(RedactionStatus::Suppressed.as_str(), "suppressed");
    assert_eq!(RedactionStatus::Failed.as_str(), "redaction_failed");

    assert_eq!(DiagnosticReasonCode::AuthMissing.as_str(), "auth_missing");
    assert_eq!(
        DiagnosticReasonCode::UnknownConnectorFailure.as_str(),
        "unknown_connector_failure"
    );

    assert_eq!(RemediationOwner::User.as_str(), "product_user");
    assert_eq!(RemediationOwner::NoneRequired.as_str(), "none_required");

    assert_eq!(RetrySafety::NoActionNeeded.as_str(), "no_action_needed");
    assert_eq!(RetrySafety::Unsafe.as_str(), "unsafe");

    assert_eq!(FreshnessState::Fresh.as_str(), "fresh");
    assert_eq!(FreshnessState::Stale.as_str(), "stale");
}

#[test]
fn capability_profile_roundtrips_camel_case_fields_and_omitempty() {
    let profile = CapabilityProfile {
        profile_id: "profile_scenario-1".to_string(),
        tenant_id: "ten_matrix".to_string(),
        connector_id: "matrix-main".to_string(),
        connector_kind: "matrix".to_string(),
        core_invariant_results: passing_core(),
        provider_surface_results: HashMap::from([(
            "direct_message".to_string(),
            SurfaceSupport::Supported,
        )]),
        group_room_capabilities: GroupRoomCapabilities {
            mention_evidence: Some(SurfaceSupport::Supported),
            ..GroupRoomCapabilities::default()
        },
        handoff_capabilities: HandoffCapabilities::default(),
        equivalent_durable_identity_rule_id: "rule-id".to_string(),
        equivalent_durable_identity_rule: String::new(),
        declared_at: ts("2026-05-11T10:00:00Z"),
    };

    let json = serde_json::to_value(&profile).unwrap();
    let obj = json.as_object().unwrap();

    assert!(obj.contains_key("profileId"));
    assert!(obj.contains_key("tenantId"));
    assert!(obj.contains_key("connectorId"));
    assert!(obj.contains_key("connectorKind"));
    assert!(obj.contains_key("coreInvariantResults"));
    assert!(obj.contains_key("providerSurfaceResults"));
    assert!(obj.contains_key("groupRoomCapabilities"));
    assert!(obj.contains_key("handoffCapabilities"));
    assert!(obj.contains_key("equivalentDurableIdentityRuleId"));
    assert!(obj.contains_key("declaredAt"));
    // omitempty: unset optional fields are absent.
    assert!(!obj.contains_key("equivalentDurableIdentityRule"));
    // enum wire values are snake_case.
    assert_eq!(
        obj["coreInvariantResults"]["redaction"],
        serde_json::json!("pass")
    );
    assert_eq!(
        obj["providerSurfaceResults"]["direct_message"],
        serde_json::json!("supported")
    );
    assert_eq!(
        obj["groupRoomCapabilities"]["mentionEvidence"],
        serde_json::json!("supported")
    );

    let back: CapabilityProfile = serde_json::from_value(json).unwrap();
    assert_eq!(back, profile);
}

#[test]
fn account_binding_summary_roundtrips_camel_case_fields() {
    let binding = AccountBindingSummary {
        tenant_id: "ten_matrix".to_string(),
        connector_id: String::new(),
        connector_account_id: "acct-1".to_string(),
        provider_account_label: "Matrix room".to_string(),
        permission_state: "granted".to_string(),
        redaction_status: RedactionStatus::Redacted,
    };

    let json = serde_json::to_value(&binding).unwrap();
    let obj = json.as_object().unwrap();

    assert!(obj.contains_key("tenantId"));
    assert!(obj.contains_key("connectorAccountId"));
    assert!(obj.contains_key("providerAccountLabel"));
    assert!(obj.contains_key("permissionState"));
    assert!(obj.contains_key("redactionStatus"));
    assert!(!obj.contains_key("connectorId"));
    assert_eq!(obj["redactionStatus"], serde_json::json!("redacted"));

    let back: AccountBindingSummary = serde_json::from_value(json).unwrap();
    assert_eq!(back, binding);
}

// --- supervisor.go behavioral tests ----------------------------------------

#[test]
fn connector_supervisor_lifecycle() {
    let supervisor = Supervisor::new();

    let (connector, created) = supervisor
        .register(RegisterInput {
            connector_id: "slack-main".to_string(),
            kind: "slack".to_string(),
            display_name: "Slack Main".to_string(),
            ..RegisterInput::default()
        })
        .expect("register");
    assert!(created, "expected first register to create connector");
    assert_eq!(connector.status, Status::Registered);

    let connector = supervisor
        .report_health(
            &connector.connector_id,
            ReportHealthInput {
                status: Status::Healthy,
            },
        )
        .expect("report health");
    assert_eq!(connector.status, Status::Healthy);

    let connector = supervisor
        .report_failure(
            &connector.connector_id,
            ReportFailureInput {
                reason: "socket disconnected".to_string(),
            },
        )
        .expect("report failure");
    assert_eq!(connector.status, Status::BackingOff);
    assert!(
        connector.backoff_seconds != 0 && connector.next_restart_at.is_some(),
        "expected backoff state to be set, got {connector:?}"
    );

    let connector = supervisor
        .restart(&connector.connector_id)
        .expect("restart");
    assert_eq!(connector.status, Status::Registered);
    assert_eq!(connector.restart_count, 1);
}

#[test]
fn connector_supervisor_fails_after_repeated_failures() {
    let supervisor = Supervisor::new();
    let (mut connector, _) = supervisor
        .register(RegisterInput {
            connector_id: "discord-main".to_string(),
            kind: "discord".to_string(),
            ..RegisterInput::default()
        })
        .expect("register");

    for _ in 0..5 {
        connector = supervisor
            .report_failure(
                &connector.connector_id,
                ReportFailureInput {
                    reason: "connection failed".to_string(),
                },
            )
            .expect("report failure");
    }

    assert_eq!(connector.status, Status::Failed);
}

#[test]
fn connector_supervisor_tenant_ownership_and_disable() {
    let supervisor = Supervisor::new();
    supervisor
        .register(RegisterInput {
            tenant_id: "ten_a".to_string(),
            connector_id: "discord-shared".to_string(),
            kind: "discord".to_string(),
            secret_refs: Some(vec!["discord/token".to_string()]),
            ..RegisterInput::default()
        })
        .expect("register tenant A");
    supervisor
        .register(RegisterInput {
            tenant_id: "ten_b".to_string(),
            connector_id: "slack-b".to_string(),
            kind: "slack".to_string(),
            ..RegisterInput::default()
        })
        .expect("register tenant B");

    let ten_a = supervisor.list_for_tenant("ten_a");
    assert_eq!(ten_a.len(), 1);
    assert_eq!(ten_a[0].connector_id, "discord-shared");
    assert_eq!(ten_a[0].secret_refs, vec!["discord/token".to_string()]);
    assert!(
        supervisor
            .get_for_tenant("discord-shared", "ten_b")
            .is_none(),
        "tenant B unexpectedly resolved tenant A connector"
    );

    let disabled = supervisor
        .disable("discord-shared", "integration disconnected")
        .expect("disable");
    assert_eq!(disabled.status, Status::Disabled);
    assert!(!disabled.disabled_reason.is_empty());
    assert_eq!(
        supervisor.restart("discord-shared"),
        Err(ConnectorsError::ConnectorDisabled)
    );
}

#[test]
fn connector_supervisor_register_validation() {
    let supervisor = Supervisor::new();
    assert_eq!(
        supervisor.register(RegisterInput::default()),
        Err(ConnectorsError::ConnectorIdRequired)
    );
    let input = RegisterInput {
        connector_id: "c1".to_string(),
        ..RegisterInput::default()
    };
    assert_eq!(
        supervisor.register(input),
        Err(ConnectorsError::ConnectorKindRequired)
    );
}

#[test]
fn connector_supervisor_register_update_keeps_status_tenant_and_refs() {
    let supervisor = Supervisor::new();
    let (connector, created) = supervisor
        .register(RegisterInput {
            tenant_id: "ten_a".to_string(),
            connector_id: "c1".to_string(),
            kind: "slack".to_string(),
            display_name: "Slack".to_string(),
            secret_refs: Some(vec!["a".to_string()]),
        })
        .expect("register");
    assert!(created);
    let _ = supervisor
        .report_health(
            &connector.connector_id,
            ReportHealthInput {
                status: Status::Healthy,
            },
        )
        .expect("report health");

    // Re-register without tenant/refs: kind + display name update, everything
    // else is kept (Go nil secretRefs semantics).
    let (updated, created) = supervisor
        .register(RegisterInput {
            connector_id: "c1".to_string(),
            kind: "slack-2".to_string(),
            display_name: "Slack 2".to_string(),
            ..RegisterInput::default()
        })
        .expect("re-register");
    assert!(!created);
    assert_eq!(updated.kind, "slack-2");
    assert_eq!(updated.display_name, "Slack 2");
    assert_eq!(updated.tenant_id, "ten_a");
    assert_eq!(updated.status, Status::Healthy);
    assert_eq!(updated.secret_refs, vec!["a".to_string()]);

    // Re-register with tenant + refs replaces them (cleanStrings applied).
    let (updated, _) = supervisor
        .register(RegisterInput {
            tenant_id: "ten_b".to_string(),
            connector_id: "c1".to_string(),
            kind: "slack-2".to_string(),
            display_name: "Slack 2".to_string(),
            secret_refs: Some(vec![
                "  b  ".to_string(),
                "".to_string(),
                "a".to_string(),
                "b".to_string(),
            ]),
        })
        .expect("re-register 2");
    assert_eq!(updated.tenant_id, "ten_b");
    assert_eq!(updated.secret_refs, vec!["b".to_string(), "a".to_string()]);
}

#[test]
fn connector_supervisor_list_and_tenant_scoping() {
    let supervisor = Supervisor::new();
    for (tenant_id, connector_id, kind) in [
        ("ten_a", "c1", "slack"),
        ("ten_b", "c2", "discord"),
        ("", "c3", "matrix"),
    ] {
        supervisor
            .register(RegisterInput {
                tenant_id: tenant_id.to_string(),
                connector_id: connector_id.to_string(),
                kind: kind.to_string(),
                ..RegisterInput::default()
            })
            .expect("register");
    }

    let all: Vec<String> = supervisor
        .list()
        .into_iter()
        .map(|c| c.connector_id)
        .collect();
    assert_eq!(all, vec!["c1", "c2", "c3"], "list keeps registration order");

    let ten_a: Vec<String> = supervisor
        .list_for_tenant("ten_a")
        .into_iter()
        .map(|c| c.connector_id)
        .collect();
    assert_eq!(ten_a, vec!["c1"]);
    assert_eq!(
        supervisor.list_for_tenant("").len(),
        3,
        "empty tenant matches all"
    );

    assert_eq!(supervisor.get("c2").unwrap().kind, "discord");
    assert_eq!(supervisor.get("nope"), None);
    assert!(supervisor.get_for_tenant("c1", "ten_a").is_some());
    assert!(supervisor.get_for_tenant("c1", "ten_b").is_none());
    assert!(supervisor.get_for_tenant("c1", "").is_some());
    assert!(supervisor.get_for_tenant("nope", "").is_none());
}

#[test]
fn connector_supervisor_require_inbound_ready() {
    let supervisor = Supervisor::new();
    assert_eq!(
        supervisor.require_inbound_ready("missing", ""),
        Err(ConnectorsError::ConnectorNotFound)
    );
    supervisor
        .register(RegisterInput {
            tenant_id: "ten_a".to_string(),
            connector_id: "c1".to_string(),
            kind: "slack".to_string(),
            ..RegisterInput::default()
        })
        .expect("register");
    assert_eq!(
        supervisor
            .require_inbound_ready("c1", "ten_a")
            .unwrap()
            .connector_id,
        "c1"
    );
    assert_eq!(
        supervisor.require_inbound_ready("c1", "ten_b"),
        Err(ConnectorsError::ConnectorNotFound)
    );
    let _ = supervisor.disable("c1", "maintenance");
    assert_eq!(
        supervisor.require_inbound_ready("c1", ""),
        Err(ConnectorsError::ConnectorDisabled)
    );
}

#[test]
fn connector_supervisor_report_health_validation_and_reset() {
    let supervisor = Supervisor::new();
    let (connector, _) = supervisor
        .register(RegisterInput {
            connector_id: "c1".to_string(),
            kind: "slack".to_string(),
            ..RegisterInput::default()
        })
        .expect("register");

    for status in [Status::Registered, Status::BackingOff, Status::Failed] {
        assert_eq!(
            supervisor.report_health("c1", ReportHealthInput { status }),
            Err(ConnectorsError::InvalidConnectorHealth)
        );
    }
    assert_eq!(
        supervisor.report_health(
            "missing",
            ReportHealthInput {
                status: Status::Healthy
            }
        ),
        Err(ConnectorsError::ConnectorNotFound)
    );

    // A degraded heartbeat resets failure/backoff state.
    let _ = supervisor
        .report_failure(
            &connector.connector_id,
            ReportFailureInput {
                reason: "boom".to_string(),
            },
        )
        .expect("report failure");
    let degraded = supervisor
        .report_health(
            "c1",
            ReportHealthInput {
                status: Status::Degraded,
            },
        )
        .expect("report health");
    assert_eq!(degraded.status, Status::Degraded);
    assert_eq!(degraded.failure_count, 0);
    assert_eq!(degraded.backoff_seconds, 0);
    assert_eq!(degraded.next_restart_at, None);
    assert!(degraded.last_heartbeat_at.is_some());

    let _ = supervisor.disable("c1", "off");
    assert_eq!(
        supervisor.report_health(
            "c1",
            ReportHealthInput {
                status: Status::Healthy
            }
        ),
        Err(ConnectorsError::ConnectorDisabled)
    );
}

#[test]
fn connector_supervisor_report_failure_backoff_and_threshold() {
    let supervisor = Supervisor::new();
    let (connector, _) = supervisor
        .register(RegisterInput {
            connector_id: "c1".to_string(),
            kind: "slack".to_string(),
            ..RegisterInput::default()
        })
        .expect("register");

    assert_eq!(
        supervisor.report_failure(
            "c1",
            ReportFailureInput {
                reason: String::new()
            }
        ),
        Err(ConnectorsError::ConnectorFailureRequired)
    );
    assert_eq!(
        supervisor.report_failure(
            "missing",
            ReportFailureInput {
                reason: "x".to_string()
            }
        ),
        Err(ConnectorsError::ConnectorNotFound)
    );

    let mut connector = connector;
    for (i, backoff) in [5, 10, 20, 40].iter().enumerate() {
        connector = supervisor
            .report_failure(
                &connector.connector_id,
                ReportFailureInput {
                    reason: "connection failed".to_string(),
                },
            )
            .expect("report failure");
        assert_eq!(connector.status, Status::BackingOff);
        assert_eq!(connector.failure_count, (i + 1) as i64);
        assert_eq!(connector.backoff_seconds, *backoff);
        assert!(connector.next_restart_at.is_some());
        assert_eq!(connector.last_failure_reason, "connection failed");
    }

    // 5th failure circuit-breaks to Failed and clears backoff.
    connector = supervisor
        .report_failure(
            &connector.connector_id,
            ReportFailureInput {
                reason: "connection failed".to_string(),
            },
        )
        .expect("report failure");
    assert_eq!(connector.status, Status::Failed);
    assert_eq!(connector.failure_count, 5);
    assert_eq!(connector.backoff_seconds, 0);
    assert_eq!(connector.next_restart_at, None);

    let _ = supervisor.disable("c1", "off");
    assert_eq!(
        supervisor.report_failure(
            "c1",
            ReportFailureInput {
                reason: "x".to_string()
            }
        ),
        Err(ConnectorsError::ConnectorDisabled)
    );
}

#[test]
fn connector_supervisor_restart_resets_state() {
    let supervisor = Supervisor::new();
    assert_eq!(
        supervisor.restart("missing"),
        Err(ConnectorsError::ConnectorNotFound)
    );
    let (connector, _) = supervisor
        .register(RegisterInput {
            connector_id: "c1".to_string(),
            kind: "slack".to_string(),
            ..RegisterInput::default()
        })
        .expect("register");
    let _ = supervisor
        .report_failure(
            &connector.connector_id,
            ReportFailureInput {
                reason: "x".to_string(),
            },
        )
        .expect("report failure");

    let restarted = supervisor.restart("c1").expect("restart");
    assert_eq!(restarted.status, Status::Registered);
    assert_eq!(restarted.restart_count, 1);
    assert_eq!(restarted.backoff_seconds, 0);
    assert_eq!(restarted.next_restart_at, None);
    assert!(restarted.last_restart_at.is_some());

    let _ = supervisor.disable("c1", "off");
    assert_eq!(
        supervisor.restart("c1"),
        Err(ConnectorsError::ConnectorDisabled)
    );
}

#[test]
fn connector_supervisor_disable_and_re_enable() {
    let supervisor = Supervisor::new();
    assert_eq!(
        supervisor.disable("missing", "x"),
        Err(ConnectorsError::ConnectorNotFound)
    );
    let (connector, _) = supervisor
        .register(RegisterInput {
            connector_id: "c1".to_string(),
            kind: "slack".to_string(),
            ..RegisterInput::default()
        })
        .expect("register");
    let _ = supervisor
        .report_failure(
            &connector.connector_id,
            ReportFailureInput {
                reason: "x".to_string(),
            },
        )
        .expect("report failure");

    let disabled = supervisor
        .disable("c1", "integration disconnected")
        .expect("disable");
    assert_eq!(disabled.status, Status::Disabled);
    assert_eq!(disabled.disabled_reason, "integration disconnected");
    assert_eq!(disabled.backoff_seconds, 0);
    assert_eq!(disabled.next_restart_at, None);

    let disabled_again = supervisor
        .disable("c1", "other reason")
        .expect("disable again");
    assert_eq!(disabled_again.disabled_reason, "other reason");

    let enabled = supervisor.re_enable("c1").expect("re-enable");
    assert_eq!(enabled.status, Status::Registered);
    assert_eq!(enabled.disabled_reason, "");
    assert_eq!(
        supervisor.re_enable("missing"),
        Err(ConnectorsError::ConnectorNotFound)
    );
}

#[test]
fn connector_supervisor_restore_replaces_registry() {
    let supervisor = Supervisor::new();
    supervisor
        .register(RegisterInput {
            connector_id: "old".to_string(),
            kind: "slack".to_string(),
            ..RegisterInput::default()
        })
        .expect("register");

    let restored = Connector {
        connector_id: "a".to_string(),
        kind: "discord".to_string(),
        display_name: "A".to_string(),
        status: Status::Healthy,
        created_at: ts("2026-01-01T00:00:00Z"),
        updated_at: ts("2026-01-01T00:00:00Z"),
        ..Connector::default()
    };
    supervisor.restore(vec![restored.clone()]);

    assert_eq!(supervisor.list().len(), 1);
    assert_eq!(supervisor.list()[0].connector_id, "a");
    assert_eq!(supervisor.get("old"), None);
    assert_eq!(supervisor.get("a").unwrap(), restored);
}

#[test]
fn connector_supervisor_clean_strings_and_min_int_helpers() {
    assert_eq!(clean_strings(vec![]), Vec::<String>::new());
    assert_eq!(
        clean_strings(vec![
            "  a  ".to_string(),
            "".to_string(),
            "b".to_string(),
            " a ".to_string(),
            "b".to_string(),
        ]),
        vec!["a".to_string(), "b".to_string()]
    );
    assert_eq!(
        clean_strings(vec!["".to_string(), "   ".to_string()]),
        Vec::<String>::new()
    );
    assert_eq!(min_int(5, 3), 3);
    assert_eq!(min_int(2, 10), 2);
    assert_eq!(min_int(7, 7), 7);
}

#[test]
fn connector_supervisor_with_connector_mutation_serializes_and_propagates() {
    let supervisor = Supervisor::new();
    supervisor
        .register(RegisterInput {
            connector_id: "c1".to_string(),
            kind: "slack".to_string(),
            ..RegisterInput::default()
        })
        .expect("register");

    // The closure may call supervisor methods without deadlocking (the
    // per-connector mutation lock and the registry lock are independent).
    let result = supervisor.with_connector_mutation("c1", || {
        let connector = supervisor.report_health(
            "c1",
            ReportHealthInput {
                status: Status::Healthy,
            },
        )?;
        assert_eq!(connector.status, Status::Healthy);
        Ok(())
    });
    assert_eq!(result, Ok(()));

    let result =
        supervisor.with_connector_mutation("c1", || Err(ConnectorsError::ConnectorDisabled));
    assert_eq!(result, Err(ConnectorsError::ConnectorDisabled));
}

#[test]
fn connector_supervisor_run_live_validation_outcome() {
    let supervisor = Supervisor::new();

    let failed = supervisor.run_live_validation_outcome(FakeOutcome::from(FakeOutcome::FAILED));
    assert_eq!(failed.outcome.as_str(), "failed");
    assert!(
        !failed.automatic_retry_allowed,
        "non-idempotent mutations must not auto-retry"
    );
    assert!(failed.correlation_key_required);
    assert_eq!(failed.reason_code, "live_validation.fake_failed");

    let completed =
        supervisor.run_live_validation_outcome(FakeOutcome::from(FakeOutcome::COMPLETED));
    assert_eq!(completed.outcome.as_str(), "completed");
    assert!(completed.correlation_key_required);
    assert!(!completed.ambiguous_commit);
}

// --- conformance.go behavioral tests ---------------------------------------

#[test]
fn core_invariant_areas_are_ordered_and_wired() {
    let areas = core_invariant_areas();
    let expected = [
        (ConformanceArea::TenantOwnership, "tenant_ownership"),
        (ConformanceArea::PermissionGating, "permission_gating"),
        (ConformanceArea::Redaction, "redaction"),
        (
            ConformanceArea::ActiveTenantAccountBinding,
            "active_tenant_account_binding",
        ),
        (ConformanceArea::InboundIdentity, "inbound_identity"),
        (ConformanceArea::DurableDedupe, "durable_dedupe"),
        (
            ConformanceArea::StableRoutingDecisions,
            "stable_routing_decisions",
        ),
        (
            ConformanceArea::MinimumForegroundReply,
            "minimum_foreground_reply",
        ),
        (ConformanceArea::RequiredDiagnostics, "required_diagnostics"),
        (ConformanceArea::DeliverySeparation, "delivery_separation"),
    ];
    assert_eq!(areas.len(), expected.len());
    for (i, (area, literal)) in expected.iter().enumerate() {
        assert_eq!(areas[i], *area);
        assert_eq!(areas[i].as_str(), *literal);
    }
    assert_eq!(ConformanceArea::default(), ConformanceArea::TenantOwnership);
    assert_eq!(SurfaceSupport::default(), SurfaceSupport::Unsupported);
}

fn passing_profile() -> CapabilityProfile {
    CapabilityProfile {
        profile_id: "profile_scenario-1".to_string(),
        connector_id: "matrix-main".to_string(),
        connector_kind: CONNECTOR_KIND_MATRIX.to_string(),
        core_invariant_results: passing_core(),
        ..CapabilityProfile::default()
    }
}

#[test]
fn validate_capability_profile_accepts_passing_profile() {
    assert_eq!(validate_capability_profile(&passing_profile()), Ok(()));
}

#[test]
fn validate_capability_profile_rejects_missing_id_or_kind() {
    let mut profile = passing_profile();
    profile.connector_id = "   ".to_string();
    assert_eq!(
        validate_capability_profile(&profile),
        Err(ConnectorsError::ConnectorIdRequired)
    );

    let mut profile = passing_profile();
    profile.connector_kind = "".to_string();
    assert_eq!(
        validate_capability_profile(&profile),
        Err(ConnectorsError::ConnectorKindRequired)
    );
}

#[test]
fn validate_capability_profile_rejects_failing_core_invariant() {
    let mut profile = passing_profile();
    profile
        .core_invariant_results
        .insert(ConformanceArea::Redaction, ConformanceResultStatus::Fail);
    assert_eq!(
        validate_capability_profile(&profile),
        Err(ConnectorsError::CoreInvariantFailed)
    );

    // A missing area counts as a failure (Go zero value).
    let mut profile = passing_profile();
    profile
        .core_invariant_results
        .remove(&ConformanceArea::DurableDedupe);
    assert_eq!(
        validate_capability_profile(&profile),
        Err(ConnectorsError::CoreInvariantFailed)
    );
}

#[test]
fn validate_capability_profile_requires_rule_when_identity_id_set() {
    let mut profile = passing_profile();
    profile.equivalent_durable_identity_rule_id = "rule-id".to_string();
    assert_eq!(
        validate_capability_profile(&profile),
        Err(ConnectorsError::EquivalentIdentityRequired)
    );
    profile.equivalent_durable_identity_rule = "rule".to_string();
    assert_eq!(validate_capability_profile(&profile), Ok(()));

    // A rule without an id is accepted (id empty + rule empty is fine too).
    let mut profile = passing_profile();
    profile.equivalent_durable_identity_rule = "rule".to_string();
    assert_eq!(validate_capability_profile(&profile), Ok(()));
}

#[test]
fn run_matrix_case_declares_group_room_evidence_capabilities() {
    let (results, profile) = run_matrix_case(MatrixCase {
        scenario_id: "group_room_evidence".to_string(),
        connector_kind: CONNECTOR_KIND_MATRIX.to_string(),
        connector_id: "matrix-main".to_string(),
        tenant_id: "ten_matrix".to_string(),
        core_invariant_results: passing_core(),
        group_room_capabilities: GroupRoomCapabilities {
            mention_evidence: Some(SurfaceSupport::Supported),
            allowlist_evidence: Some(SurfaceSupport::Supported),
            unsupported_source_evidence: Some(SurfaceSupport::Limited),
            duplicate_message_evidence: Some(SurfaceSupport::Supported),
            edited_message_evidence: Some(SurfaceSupport::Unsupported),
            deleted_message_evidence: Some(SurfaceSupport::Unsupported),
        },
        now: ts("2026-05-11T10:00:00Z"),
        ..MatrixCase::default()
    })
    .expect("run matrix case");

    assert_eq!(
        profile.group_room_capabilities.mention_evidence,
        Some(SurfaceSupport::Supported)
    );
    assert_eq!(
        profile.group_room_capabilities.unsupported_source_evidence,
        Some(SurfaceSupport::Limited)
    );
    assert_eq!(
        profile.group_room_capabilities.edited_message_evidence,
        Some(SurfaceSupport::Unsupported)
    );
    assert_eq!(
        profile.group_room_capabilities.deleted_message_evidence,
        Some(SurfaceSupport::Unsupported)
    );

    let got: HashMap<&str, ConformanceResultStatus> = results
        .iter()
        .map(|r| (r.area.as_str(), r.result))
        .collect();
    let want = [
        (
            GROUP_ROOM_SURFACE_MENTION_EVIDENCE,
            ConformanceResultStatus::Supported,
        ),
        (
            GROUP_ROOM_SURFACE_ALLOWLIST_EVIDENCE,
            ConformanceResultStatus::Supported,
        ),
        (
            GROUP_ROOM_SURFACE_UNSUPPORTED_SOURCE_EVIDENCE,
            ConformanceResultStatus::Limited,
        ),
        (
            GROUP_ROOM_SURFACE_DUPLICATE_MESSAGE_EVIDENCE,
            ConformanceResultStatus::Supported,
        ),
        (
            GROUP_ROOM_SURFACE_EDITED_MESSAGE_EVIDENCE,
            ConformanceResultStatus::Unsupported,
        ),
        (
            GROUP_ROOM_SURFACE_DELETED_MESSAGE_EVIDENCE,
            ConformanceResultStatus::Unsupported,
        ),
    ];
    for (area, status) in want {
        assert_eq!(got[area], status, "result area {area}");
    }
    // Core areas still produce pass results with the conf_<scenario>_<area> id.
    assert_eq!(got["redaction"], ConformanceResultStatus::Pass);
    assert!(
        results
            .iter()
            .any(|r| r.conformance_result_id == "conf_group_room_evidence_redaction")
    );
    assert!(
        results
            .iter()
            .any(|r| r.conformance_result_id
                == "conf_group_room_evidence_group_room_mention_evidence")
    );
}

#[test]
fn run_matrix_case_declares_handoff_capabilities() {
    let (results, profile) = run_matrix_case(MatrixCase {
        scenario_id: "handoff".to_string(),
        connector_kind: CONNECTOR_KIND_MATRIX.to_string(),
        connector_id: "matrix-main".to_string(),
        tenant_id: "ten_matrix".to_string(),
        core_invariant_results: passing_core(),
        handoff_capabilities: HandoffCapabilities {
            source_support: Some(SurfaceSupport::Supported),
            destination_support: Some(SurfaceSupport::Limited),
            first_response_source_references: Some(SurfaceSupport::Supported),
        },
        now: ts("2026-05-11T10:00:00Z"),
        ..MatrixCase::default()
    })
    .expect("run matrix case");

    assert_eq!(
        profile.handoff_capabilities.source_support,
        Some(SurfaceSupport::Supported)
    );
    assert_eq!(
        profile.handoff_capabilities.destination_support,
        Some(SurfaceSupport::Limited)
    );
    assert_eq!(
        profile
            .handoff_capabilities
            .first_response_source_references,
        Some(SurfaceSupport::Supported)
    );

    let got: HashMap<&str, ConformanceResultStatus> = results
        .iter()
        .map(|r| (r.area.as_str(), r.result))
        .collect();
    let want = [
        (
            HANDOFF_SURFACE_SOURCE_SUPPORT,
            ConformanceResultStatus::Supported,
        ),
        (
            HANDOFF_SURFACE_DESTINATION_SUPPORT,
            ConformanceResultStatus::Limited,
        ),
        (
            HANDOFF_SURFACE_FIRST_RESPONSE_SOURCE_REFERENCES,
            ConformanceResultStatus::Supported,
        ),
    ];
    for (area, status) in want {
        assert_eq!(got[area], status, "result area {area}");
    }
}

#[test]
fn run_matrix_case_does_not_infer_memory_from_group_room_or_handoff() {
    let (_, profile) = run_matrix_case(MatrixCase {
        scenario_id: "non_memory_scope".to_string(),
        connector_kind: "slack".to_string(),
        connector_id: "slack-main".to_string(),
        core_invariant_results: passing_core(),
        provider_surface_results: HashMap::from([
            (
                "memory_based_team_context".to_string(),
                SurfaceSupport::Unsupported,
            ),
            (
                "semantic_cross_room_recall".to_string(),
                SurfaceSupport::Unsupported,
            ),
        ]),
        group_room_capabilities: GroupRoomCapabilities {
            mention_evidence: Some(SurfaceSupport::Supported),
            allowlist_evidence: Some(SurfaceSupport::Supported),
            ..GroupRoomCapabilities::default()
        },
        handoff_capabilities: HandoffCapabilities {
            source_support: Some(SurfaceSupport::Supported),
            destination_support: Some(SurfaceSupport::Supported),
            first_response_source_references: Some(SurfaceSupport::Supported),
        },
        now: ts("2026-05-11T10:00:00Z"),
        ..MatrixCase::default()
    })
    .expect("run matrix case");

    assert_eq!(
        profile.provider_surface_results["memory_based_team_context"],
        SurfaceSupport::Unsupported
    );
    assert_eq!(
        profile.provider_surface_results["semantic_cross_room_recall"],
        SurfaceSupport::Unsupported
    );
}

#[test]
fn run_matrix_case_validation_errors() {
    assert_eq!(
        run_matrix_case(MatrixCase::default()),
        Err(ConnectorsError::ConformanceScenarioRequired)
    );

    let mut input = MatrixCase {
        scenario_id: "scenario-1".to_string(),
        ..MatrixCase::default()
    };
    assert_eq!(
        run_matrix_case(input.clone()),
        Err(ConnectorsError::ConformanceKindRequired)
    );

    input.connector_kind = CONNECTOR_KIND_MATRIX.to_string();
    input.equivalent_durable_identity_rule_id = "rule-id".to_string();
    assert_eq!(
        run_matrix_case(input),
        Err(ConnectorsError::EquivalentIdentityRequired)
    );
}

#[test]
fn run_matrix_case_defaults_missing_core_to_fail_and_redaction_to_redacted() {
    let now = ts("2026-05-11T10:00:00Z");
    let (results, profile) = run_matrix_case(MatrixCase {
        scenario_id: "defaults".to_string(),
        connector_kind: "slack".to_string(),
        connector_id: "slack-main".to_string(),
        core_invariant_results: HashMap::new(),
        now,
        ..MatrixCase::default()
    })
    .expect("run matrix case");

    assert_eq!(results.len(), 10);
    assert_eq!(results[0].area, "tenant_ownership");
    for result in &results {
        assert_eq!(result.result, ConformanceResultStatus::Fail);
        assert_eq!(result.reason_code, "core_invariant_failed");
        assert_eq!(result.redaction_status, RedactionStatus::Redacted);
        assert_eq!(result.evidence_timestamp, now);
        assert_eq!(result.retention_expires_at, now + Duration::days(90));
    }
    assert_eq!(profile.core_invariant_results.len(), 10);
    assert_eq!(
        profile.core_invariant_results[&ConformanceArea::Redaction],
        ConformanceResultStatus::Fail
    );
    assert_eq!(profile.declared_at, now);

    // A zero `now` falls back to the current time.
    let (_, profile) = run_matrix_case(MatrixCase {
        scenario_id: "now_default".to_string(),
        connector_kind: "slack".to_string(),
        connector_id: "slack-main".to_string(),
        ..MatrixCase::default()
    })
    .expect("run matrix case");
    let elapsed = (Utc::now() - profile.declared_at).num_seconds().abs();
    assert!(elapsed < 5, "declared_at should default to now");
}

#[test]
fn run_matrix_case_sorts_surface_keys_and_merges_surfaces() {
    let now = ts("2026-05-11T10:00:00Z");
    let (results, profile) = run_matrix_case(MatrixCase {
        scenario_id: "sorted".to_string(),
        connector_kind: "matrix".to_string(),
        connector_id: "matrix-main".to_string(),
        core_invariant_results: passing_core(),
        provider_surface_results: HashMap::from([
            ("zebra_surface".to_string(), SurfaceSupport::Supported),
            ("alpha_surface".to_string(), SurfaceSupport::Limited),
        ]),
        group_room_capabilities: GroupRoomCapabilities {
            mention_evidence: Some(SurfaceSupport::Supported),
            ..GroupRoomCapabilities::default()
        },
        handoff_capabilities: HandoffCapabilities {
            source_support: Some(SurfaceSupport::Limited),
            ..HandoffCapabilities::default()
        },
        unsafe_incremental_update_degraded: true,
        now,
        ..MatrixCase::default()
    })
    .expect("run matrix case");

    let core_areas: HashSet<&str> = core_invariant_areas()
        .iter()
        .copied()
        .map(ConformanceArea::as_str)
        .collect();
    let surface_areas: Vec<&str> = results
        .iter()
        .map(|r| r.area.as_str())
        .filter(|area| !core_areas.contains(area))
        .collect();
    assert_eq!(
        surface_areas,
        vec![
            "alpha_surface",
            "group_room_mention_evidence",
            "handoff_source_support",
            "incremental_visible_updates",
            "zebra_surface",
        ]
    );

    let incremental = results
        .iter()
        .find(|r| r.area == "incremental_visible_updates")
        .expect("incremental_visible_updates result");
    assert_eq!(incremental.result, ConformanceResultStatus::Limited);
    assert_eq!(
        incremental.conformance_result_id,
        "conf_sorted_incremental_visible_updates"
    );
    assert_eq!(
        profile.provider_surface_results["incremental_visible_updates"],
        SurfaceSupport::Limited
    );
}

#[test]
fn surface_result_maps_support_levels() {
    assert_eq!(
        surface_result(SurfaceSupport::Supported),
        ConformanceResultStatus::Supported
    );
    assert_eq!(
        surface_result(SurfaceSupport::Limited),
        ConformanceResultStatus::Limited
    );
    assert_eq!(
        surface_result(SurfaceSupport::Unsupported),
        ConformanceResultStatus::Unsupported
    );
}

#[test]
fn core_reason_code_marks_only_failures() {
    assert_eq!(
        core_reason_code(ConformanceResultStatus::Fail),
        "core_invariant_failed"
    );
    assert_eq!(core_reason_code(ConformanceResultStatus::Pass), "");
    assert_eq!(core_reason_code(ConformanceResultStatus::Supported), "");
    assert_eq!(core_reason_code(ConformanceResultStatus::Limited), "");
    assert_eq!(core_reason_code(ConformanceResultStatus::Unsupported), "");
}
