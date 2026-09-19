//! Behavioral tests for the kura-connectors management layer (port of
//! daemon/internal/connectors/management_routes_test.go,
//! management_repair_test.go, and management_enablement_test.go): projection
//! building, management-state classification, capability profiles, sort
//! ordering, repair terminal state / retry safety, route-policy predicates,
//! support-evidence bundles, and page pagination.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use kura_connectors::{
    CapabilitySupport, ChannelConnectorProjection, Connector, ConnectorDiagnosticState,
    DiagnosticFreshness, DiagnosticInput, DiagnosticReasonCode, LifecycleState,
    ManagementActionKind, ManagementState, ManagementTerminalState, ProjectionInput,
    RedactionStatus, RemediationOwner, RetrySafety, RoutePolicy, Status, SupportEvidenceBundle,
    build_connector_page, build_connector_projection, build_support_evidence_bundle,
    capability_profile_for_kind, classify_diagnostic, contains_route_policy_value,
    default_route_policy, latest_diagnostic, management_state_for_connector,
    next_action_for_diagnostic, next_action_label_for_diagnostic, normalize_route_policy,
    parse_cursor_offset, retry_safety_for_repair_action, route_policy_allows_conversation,
    route_policy_allows_sender, route_policy_is_valid, sort_connector_projections,
    terminal_state_for_repair_action,
};

fn ts(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}

fn connector(id: &str, kind: &str, status: Status, now: DateTime<Utc>) -> Connector {
    Connector {
        connector_id: id.to_string(),
        kind: kind.to_string(),
        display_name: id.to_string(),
        status,
        updated_at: now,
        ..Connector::default()
    }
}

/// A classified diagnostic with an explicit evidence timestamp (Go tests build
/// these through ClassifyDiagnostic; AuthMissing maps to LifecycleState::Failed).
fn diagnostic(reason: DiagnosticReasonCode, at: DateTime<Utc>) -> ConnectorDiagnosticState {
    classify_diagnostic(DiagnosticInput {
        connector_id: "c1".to_string(),
        reason_code: Some(reason),
        evidence_timestamp: Some(at),
        redaction_reliable: true,
        ..DiagnosticInput::default()
    })
    .expect("classify diagnostic")
}

fn projection(
    id: &str,
    name: &str,
    state: ManagementState,
    now: DateTime<Utc>,
) -> ChannelConnectorProjection {
    ChannelConnectorProjection {
        connector_id: id.to_string(),
        display_name: name.to_string(),
        enablement_state: state,
        updated_at: now,
        ..ChannelConnectorProjection::default()
    }
}

// --- management_routes_test.go --------------------------------------------

#[test]
fn default_and_normalized_route_policy_are_redacted_and_future_eligible() {
    let now = ts("2026-05-10T10:00:00Z");
    let policy = default_route_policy("ten_channels", "slack-main", now);
    assert!(policy.background_delivery_eligible);
    assert_eq!(policy.validation_state, "valid");
    assert_eq!(policy.redaction_status, RedactionStatus::Redacted);
    assert_eq!(policy.validated_at, now);

    let normalized = normalize_route_policy(
        RoutePolicy {
            tenant_id: "ten_channels".to_string(),
            connector_id: "slack-main".to_string(),
            ..RoutePolicy::default()
        },
        now,
    );
    assert_eq!(
        normalized.validated_at, now,
        "normalize fills the validated-at timestamp"
    );
    assert_eq!(normalized.validation_state, "valid");
    assert_eq!(normalized.redaction_status, RedactionStatus::Redacted);
}

#[test]
fn route_policy_predicates_gate_senders_and_conversations() {
    let now = ts("2026-05-10T10:00:00Z");
    let valid = default_route_policy("ten_channels", "slack-main", now);
    assert!(route_policy_is_valid(&valid));
    // A default policy has no eligible conversation lists, so nothing routes.
    assert!(!route_policy_allows_conversation(&valid, "conv-1"));
    // An empty eligible-sender list allows every sender.
    assert!(route_policy_allows_sender(&valid, "user-1"));
    assert!(!route_policy_allows_conversation(&valid, "  "));

    // An invalid policy blocks both conversation and sender routing.
    let invalid = RoutePolicy {
        validation_state: "invalid".to_string(),
        ..valid.clone()
    };
    assert!(!route_policy_is_valid(&invalid));
    assert!(!route_policy_allows_conversation(&invalid, "conv-1"));
    assert!(!route_policy_allows_sender(&invalid, "user-1"));

    let policy = RoutePolicy {
        eligible_senders: vec![" alice ".to_string(), "bob".to_string()],
        eligible_conversations: vec!["conv-1".to_string()],
        eligible_rooms: vec!["room-2".to_string()],
        eligible_channels: vec!["channel-3".to_string()],
        ..valid
    };
    // The conversation may match any of the three lists, trimmed.
    assert!(route_policy_allows_conversation(&policy, " conv-1 "));
    assert!(route_policy_allows_conversation(&policy, "room-2"));
    assert!(route_policy_allows_conversation(&policy, "channel-3"));
    assert!(!route_policy_allows_conversation(&policy, "conv-4"));
    assert!(!route_policy_allows_conversation(&policy, ""));
    // Senders must match the allowlist, trimmed.
    assert!(route_policy_allows_sender(&policy, "alice"));
    assert!(route_policy_allows_sender(&policy, "bob"));
    assert!(!route_policy_allows_sender(&policy, "carol"));
    assert!(!route_policy_allows_sender(&policy, ""));

    // contains_route_policy_value trims both sides and rejects empties.
    assert!(contains_route_policy_value(&[" x ".to_string()], "x"));
    assert!(!contains_route_policy_value(&["x".to_string()], ""));
    assert!(!contains_route_policy_value(&[], "x"));
}

// --- management_repair_test.go ---------------------------------------------

#[test]
fn repair_terminal_state_does_not_re_enable_disabled_connector() {
    assert_eq!(
        terminal_state_for_repair_action(ManagementActionKind::Reconnect, true),
        ManagementTerminalState::Disabled
    );
    assert_eq!(
        retry_safety_for_repair_action(ManagementActionKind::CredentialRotation),
        RetrySafety::Blocked
    );
}

#[test]
fn repair_terminal_state_and_retry_safety_table() {
    let actions = [
        ManagementActionKind::Repair,
        ManagementActionKind::Reconnect,
        ManagementActionKind::CredentialRotation,
        ManagementActionKind::RouteRevalidate,
        ManagementActionKind::DiagnosticRerun,
        ManagementActionKind::Disable,
        ManagementActionKind::ReEnable,
    ];
    for action in actions {
        assert_eq!(
            terminal_state_for_repair_action(action, true),
            ManagementTerminalState::Disabled,
            "a disabled connector always terminates disabled for {action}",
        );
    }
    assert_eq!(
        terminal_state_for_repair_action(ManagementActionKind::Disable, false),
        ManagementTerminalState::Disabled
    );
    assert_eq!(
        terminal_state_for_repair_action(ManagementActionKind::DiagnosticRerun, false),
        ManagementTerminalState::Degraded
    );
    assert_eq!(
        terminal_state_for_repair_action(ManagementActionKind::RouteRevalidate, false),
        ManagementTerminalState::Degraded
    );
    for action in [
        ManagementActionKind::Repair,
        ManagementActionKind::Reconnect,
        ManagementActionKind::CredentialRotation,
        ManagementActionKind::ReEnable,
    ] {
        assert_eq!(
            terminal_state_for_repair_action(action, false),
            ManagementTerminalState::ActionRequired,
            "action {action}",
        );
    }

    assert_eq!(
        retry_safety_for_repair_action(ManagementActionKind::Reconnect),
        RetrySafety::Blocked
    );
    assert_eq!(
        retry_safety_for_repair_action(ManagementActionKind::CredentialRotation),
        RetrySafety::Blocked
    );
    for action in [
        ManagementActionKind::Repair,
        ManagementActionKind::RouteRevalidate,
        ManagementActionKind::DiagnosticRerun,
        ManagementActionKind::Disable,
        ManagementActionKind::ReEnable,
    ] {
        assert_eq!(
            retry_safety_for_repair_action(action),
            RetrySafety::Retryable,
            "action {action}",
        );
    }
}

// --- management_enablement_test.go -----------------------------------------

#[test]
fn management_projection_disables_delivery_for_disabled_connector() {
    let now = ts("2026-05-10T10:00:00Z");
    let projection = build_connector_projection(
        Connector {
            connector_id: "discord-main".to_string(),
            kind: "discord".to_string(),
            display_name: "Discord Main".to_string(),
            status: Status::Disabled,
            disabled_reason: "maintenance".to_string(),
            updated_at: now,
            ..Connector::default()
        },
        None,
        now,
    );

    assert_eq!(projection.enablement_state, ManagementState::Disabled);
    assert!(!projection.delivery_eligible);
    assert_eq!(projection.setup_state, "disabled");
    assert_eq!(projection.health_status, "disabled");
    assert_eq!(projection.diagnostic_freshness, DiagnosticFreshness::Stale);
    assert_eq!(projection.redaction_status, RedactionStatus::Redacted);

    let next = projection.next_action.expect("re-enable next action");
    assert_eq!(next.action_kind, ManagementActionKind::ReEnable);
    assert_eq!(next.label, "Re-enable connector");
    assert_eq!(next.reason_code, None);
    assert_eq!(next.remediation_owner, None);
}

#[test]
fn management_state_classification_matches_go_table() {
    let now = ts("2026-05-10T10:00:00Z");

    let status_cases = [
        (Status::Healthy, ManagementState::Ready),
        (Status::Registered, ManagementState::Ready),
        (Status::Degraded, ManagementState::Degraded),
        (Status::BackingOff, ManagementState::Degraded),
        (Status::Failed, ManagementState::ActionRequired),
        (Status::Disabled, ManagementState::Disabled),
    ];
    for (status, want) in status_cases {
        let connector = connector("c1", "slack", status, now);
        assert_eq!(
            management_state_for_connector(&connector, None),
            want,
            "connector status {status}",
        );
    }

    // The diagnostic status overrides the (healthy) connector status.
    let healthy = connector("c1", "slack", Status::Healthy, now);
    let diagnostic_cases = [
        (
            LifecycleState::PermissionBlocked,
            ManagementState::ActionRequired,
        ),
        (LifecycleState::Failed, ManagementState::ActionRequired),
        (LifecycleState::RateLimited, ManagementState::Unavailable),
        (LifecycleState::Degraded, ManagementState::Degraded),
        (
            LifecycleState::UnsupportedCapability,
            ManagementState::Degraded,
        ),
        // Statuses outside the diagnostic switch fall through to the connector.
        (LifecycleState::Healthy, ManagementState::Ready),
        (LifecycleState::Configured, ManagementState::Ready),
    ];
    for (status, want) in diagnostic_cases {
        let d = ConnectorDiagnosticState {
            status,
            ..diagnostic(DiagnosticReasonCode::ReplyFailed, now)
        };
        assert_eq!(
            management_state_for_connector(&healthy, Some(&d)),
            want,
            "diagnostic status {status}",
        );
    }

    // A disabled connector stays disabled regardless of the diagnostic.
    let disabled = connector("c1", "slack", Status::Disabled, now);
    let d = ConnectorDiagnosticState {
        status: LifecycleState::Failed,
        ..diagnostic(DiagnosticReasonCode::ReplyFailed, now)
    };
    assert_eq!(
        management_state_for_connector(&disabled, Some(&d)),
        ManagementState::Disabled
    );
}

#[test]
fn capability_profile_for_kind_supports_builtin_kinds_and_downgrades_unknown() {
    for kind in ["discord", "telegram", "slack", "matrix", "DISCORD"] {
        let profile = capability_profile_for_kind(kind);
        assert_eq!(profile.len(), 9, "kind {kind}");
        assert_eq!(profile.get("disable"), Some(&CapabilitySupport::Supported));
        assert_eq!(
            profile.get("re-enable"),
            Some(&CapabilitySupport::Supported)
        );
        assert_eq!(profile.get("repair"), Some(&CapabilitySupport::Supported));
        assert_eq!(
            profile.get("reconnect"),
            Some(&CapabilitySupport::Supported)
        );
        assert_eq!(
            profile.get("credential-rotation"),
            Some(&CapabilitySupport::Limited)
        );
        assert_eq!(
            profile.get("route-edit"),
            Some(&CapabilitySupport::Supported)
        );
        assert_eq!(
            profile.get("foreground-reply-status"),
            Some(&CapabilitySupport::Supported)
        );
        assert_eq!(
            profile.get("background-delivery-status"),
            Some(&CapabilitySupport::Supported)
        );
        assert_eq!(
            profile.get("support-evidence"),
            Some(&CapabilitySupport::Supported)
        );
    }

    let profile = capability_profile_for_kind("custom");
    assert_eq!(
        profile.get("reconnect"),
        Some(&CapabilitySupport::Unsupported)
    );
    assert_eq!(
        profile.get("credential-rotation"),
        Some(&CapabilitySupport::Unsupported)
    );
    assert_eq!(
        profile.get("route-edit"),
        Some(&CapabilitySupport::Unsupported)
    );
    assert_eq!(profile.get("disable"), Some(&CapabilitySupport::Supported));
    assert_eq!(
        profile.get("support-evidence"),
        Some(&CapabilitySupport::Supported)
    );
}

#[test]
fn sort_connector_projections_orders_by_attention_disabled_ready_name_id() {
    let now = ts("2026-05-10T10:00:00Z");
    let mut items = vec![
        projection("c-ready", "Slack", ManagementState::Ready, now),
        projection("c-disabled", "Alpha", ManagementState::Disabled, now),
        projection("c-action", "Zebra", ManagementState::ActionRequired, now),
        projection("c-degraded", "Beta", ManagementState::Degraded, now),
        projection("c-unavailable", "Gamma", ManagementState::Unavailable, now),
    ];
    sort_connector_projections(&mut items);
    let ids: Vec<&str> = items.iter().map(|i| i.connector_id.as_str()).collect();
    // Rank 0 attention states order by display name, then disabled, then ready.
    assert_eq!(
        ids,
        vec![
            "c-degraded",
            "c-unavailable",
            "c-action",
            "c-disabled",
            "c-ready"
        ]
    );

    // A display-name tie breaks by connector id.
    let mut tie = vec![
        projection("b", "Same", ManagementState::Ready, now),
        projection("a", "Same", ManagementState::Ready, now),
    ];
    sort_connector_projections(&mut tie);
    assert_eq!(tie[0].connector_id, "a");
    assert_eq!(tie[1].connector_id, "b");
}

#[test]
fn next_action_and_label_map_diagnostic_reasons() {
    let now = ts("2026-05-10T10:00:00Z");
    let cases = [
        (
            DiagnosticReasonCode::AuthMissing,
            ManagementActionKind::Reconnect,
            "Reconnect authorization",
        ),
        (
            DiagnosticReasonCode::PermissionMissing,
            ManagementActionKind::Reconnect,
            "Reconnect authorization",
        ),
        (
            DiagnosticReasonCode::BlockedRoute,
            ManagementActionKind::RouteRevalidate,
            "Review route policy",
        ),
        (
            DiagnosticReasonCode::UnsupportedCapability,
            ManagementActionKind::Disable,
            "Disable unsupported connector",
        ),
        (
            DiagnosticReasonCode::RateLimited,
            ManagementActionKind::Repair,
            "Repair connector",
        ),
        (
            DiagnosticReasonCode::ProviderUnavailable,
            ManagementActionKind::Repair,
            "Repair connector",
        ),
        (
            DiagnosticReasonCode::NetworkFailed,
            ManagementActionKind::Repair,
            "Repair connector",
        ),
        (
            DiagnosticReasonCode::ReplyFailed,
            ManagementActionKind::Repair,
            "Repair connector",
        ),
    ];
    for (reason, want_action, want_label) in cases {
        let d = diagnostic(reason, now);
        assert_eq!(
            next_action_for_diagnostic(&d),
            want_action,
            "reason {reason}"
        );
        assert_eq!(
            next_action_label_for_diagnostic(&d),
            want_label,
            "reason {reason}"
        );
    }
}

#[test]
fn projection_reflects_diagnostic_freshness_health_and_next_action() {
    let now = ts("2026-05-10T10:00:00Z");
    let connector = Connector {
        connector_id: "discord-main".to_string(),
        kind: "discord".to_string(),
        display_name: "Discord Main".to_string(),
        status: Status::Healthy,
        updated_at: now,
        ..Connector::default()
    };

    // A fresh failing diagnostic drives action-required + reconnect.
    let fresh = diagnostic(
        DiagnosticReasonCode::AuthMissing,
        now - Duration::minutes(5),
    );
    let projection = build_connector_projection(connector.clone(), Some(&fresh), now);
    assert_eq!(projection.enablement_state, ManagementState::ActionRequired);
    assert_eq!(projection.diagnostic_freshness, DiagnosticFreshness::Fresh);
    assert_eq!(projection.health_status, "failed");
    assert_eq!(projection.setup_state, "action-required");
    assert!(!projection.delivery_eligible);
    let next = projection.next_action.expect("diagnostic next action");
    assert_eq!(next.action_kind, ManagementActionKind::Reconnect);
    assert_eq!(next.label, "Reconnect authorization");
    assert_eq!(next.reason_code, Some(DiagnosticReasonCode::AuthMissing));
    assert_eq!(next.remediation_owner, Some(RemediationOwner::User));

    // Evidence older than 15 minutes marks the projection stale.
    let stale = diagnostic(
        DiagnosticReasonCode::RateLimited,
        now - Duration::minutes(20),
    );
    let projection = build_connector_projection(connector.clone(), Some(&stale), now);
    assert_eq!(projection.diagnostic_freshness, DiagnosticFreshness::Stale);
    assert_eq!(projection.enablement_state, ManagementState::Unavailable);

    // No diagnostic: stale freshness, health from the connector status, ready.
    let projection = build_connector_projection(connector.clone(), None, now);
    assert_eq!(projection.diagnostic_freshness, DiagnosticFreshness::Stale);
    assert_eq!(projection.health_status, "healthy");
    assert_eq!(projection.enablement_state, ManagementState::Ready);
    assert!(projection.delivery_eligible);
    assert!(projection.next_action.is_none());
}

#[test]
fn projection_serializes_camel_case_with_next_action() {
    let now = ts("2026-05-10T10:00:00Z");
    let connector = Connector {
        connector_id: "discord-main".to_string(),
        kind: "discord".to_string(),
        display_name: "Discord Main".to_string(),
        status: Status::Failed,
        updated_at: now,
        ..Connector::default()
    };
    let d = diagnostic(DiagnosticReasonCode::AuthMissing, now);
    let projection = build_connector_projection(connector, Some(&d), now);
    let json = serde_json::to_value(&projection).unwrap();
    let obj = json.as_object().unwrap();

    assert!(obj.contains_key("connectorId"));
    assert!(obj.contains_key("connectorKind"));
    assert!(obj.contains_key("displayName"));
    assert!(obj.contains_key("enablementState"));
    assert_eq!(obj["enablementState"], serde_json::json!("action-required"));
    assert_eq!(obj["setupState"], serde_json::json!("action-required"));
    assert_eq!(obj["healthStatus"], serde_json::json!("failed"));
    assert_eq!(obj["diagnosticFreshness"], serde_json::json!("fresh"));
    assert_eq!(obj["deliveryEligible"], serde_json::json!(false));
    assert!(obj.contains_key("nextAction"));
    assert_eq!(
        obj["nextAction"]["actionKind"],
        serde_json::json!("reconnect")
    );
    assert_eq!(
        obj["nextAction"]["label"],
        serde_json::json!("Reconnect authorization")
    );
    assert_eq!(
        obj["nextAction"]["reasonCode"],
        serde_json::json!("auth_missing")
    );
    assert_eq!(
        obj["nextAction"]["remediationOwner"],
        serde_json::json!("product_user")
    );
    assert_eq!(
        obj["capabilities"]["disable"],
        serde_json::json!("supported")
    );
    assert_eq!(obj["redactionStatus"], serde_json::json!("redacted"));

    let back: ChannelConnectorProjection = serde_json::from_value(json).unwrap();
    assert_eq!(back, projection);
}

#[test]
fn latest_diagnostic_picks_newest_evidence() {
    let now = ts("2026-05-10T10:00:00Z");
    assert_eq!(latest_diagnostic(&[]), None);

    let older = diagnostic(DiagnosticReasonCode::RateLimited, now - Duration::hours(2));
    let middle = diagnostic(
        DiagnosticReasonCode::NetworkFailed,
        now - Duration::hours(1),
    );
    let newest = diagnostic(
        DiagnosticReasonCode::AuthMissing,
        now - Duration::minutes(30),
    );
    let items = [older.clone(), middle.clone(), newest.clone()];
    let latest = latest_diagnostic(&items).unwrap();
    assert_eq!(latest.diagnostic_state_id, newest.diagnostic_state_id);
}

// --- management_support.go -------------------------------------------------

#[test]
fn support_evidence_bundle_redacts_and_tracks_projection_state() {
    let now = ts("2026-05-10T10:00:00Z");
    let connector = Connector {
        connector_id: "discord-main".to_string(),
        kind: "discord".to_string(),
        display_name: "Discord Main".to_string(),
        status: Status::Healthy,
        updated_at: now,
        ..Connector::default()
    };
    let input = ProjectionInput {
        tenant_id: "ten_channels".to_string(),
        connectors: vec![connector.clone()],
        now,
        ..ProjectionInput::default()
    };
    let bundle = build_support_evidence_bundle(&input, &connector, "principal-1", now);

    assert_eq!(bundle.tenant_id, "ten_channels");
    assert_eq!(bundle.connector_id, "discord-main");
    assert_eq!(bundle.generated_by_principal_id, "principal-1");
    assert_eq!(bundle.generated_at, now);
    assert_eq!(bundle.current_state, ManagementState::Ready);
    assert_eq!(
        bundle.redactions,
        vec![
            "message_body",
            "raw_provider_payload",
            "credentials",
            "authorization_grants"
        ]
    );
    assert_eq!(bundle.retention_expires_at, now + Duration::days(90));
    assert_eq!(bundle.redaction_status, RedactionStatus::Redacted);
    assert_eq!(
        bundle.safe_evidence.get("connectorKind"),
        Some(&"discord".to_string())
    );
    assert_eq!(
        bundle.safe_evidence.get("displayName"),
        Some(&"Discord Main".to_string())
    );
    assert!(bundle.support_evidence_id.is_empty());

    // The bundle carries the projection state even for attention connectors.
    let failing = diagnostic(DiagnosticReasonCode::AuthMissing, now);
    let input = ProjectionInput {
        tenant_id: "ten_channels".to_string(),
        connectors: vec![connector.clone()],
        diagnostics: HashMap::from([("discord-main".to_string(), vec![failing.clone()])]),
        now,
        ..ProjectionInput::default()
    };
    let bundle = build_support_evidence_bundle(&input, &connector, "principal-1", now);
    assert_eq!(bundle.current_state, ManagementState::ActionRequired);
}

// --- management.go page builder --------------------------------------------

#[test]
fn build_connector_page_paginates_filters_and_orders() {
    let now = ts("2026-05-10T10:00:00Z");
    let connectors = vec![
        Connector {
            tenant_id: "ten_channels".to_string(),
            connector_id: "slack-main".to_string(),
            kind: "slack".to_string(),
            display_name: "Slack Main".to_string(),
            status: Status::Healthy,
            updated_at: now,
            ..Connector::default()
        },
        Connector {
            tenant_id: "ten_channels".to_string(),
            connector_id: "discord-main".to_string(),
            kind: "discord".to_string(),
            display_name: "Discord Main".to_string(),
            status: Status::Disabled,
            disabled_reason: "maintenance".to_string(),
            updated_at: now,
            ..Connector::default()
        },
        // Belongs to a different tenant: filtered out.
        Connector {
            tenant_id: "ten_other".to_string(),
            connector_id: "matrix-other".to_string(),
            kind: "matrix".to_string(),
            display_name: "Matrix Other".to_string(),
            status: Status::Healthy,
            updated_at: now,
            ..Connector::default()
        },
    ];
    let input = ProjectionInput {
        tenant_id: "ten_channels".to_string(),
        connectors,
        now,
        limit: 1,
        ..ProjectionInput::default()
    };

    // Page 1: disabled ranks before ready.
    let page = build_connector_page(&input);
    assert_eq!(page.tenant_id, "ten_channels");
    assert_eq!(page.page.limit, 1);
    assert_eq!(page.page.order, "attention_disabled_ready_name_id");
    assert_eq!(page.page.next_cursor, "1");
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].connector_id, "discord-main");

    // Page 2 via the returned cursor.
    let next = ProjectionInput {
        cursor: "1".to_string(),
        ..input.clone()
    };
    let page = build_connector_page(&next);
    assert_eq!(page.page.next_cursor, "");
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].connector_id, "slack-main");

    // A cursor beyond the list yields an empty page.
    let beyond = ProjectionInput {
        cursor: "99".to_string(),
        ..input.clone()
    };
    let page = build_connector_page(&beyond);
    assert!(page.items.is_empty());
    assert_eq!(page.page.next_cursor, "");

    // Kind filter excludes the other tenant's matrix connector.
    let kind = ProjectionInput {
        kind_filter: "matrix".to_string(),
        ..input.clone()
    };
    assert!(build_connector_page(&kind).items.is_empty());

    // State filter matches the enablement-state literal.
    let state = ProjectionInput {
        state_filter: "disabled".to_string(),
        ..input.clone()
    };
    let page = build_connector_page(&state);
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].connector_id, "discord-main");
}

#[test]
fn build_connector_page_defaults_limit_and_zero_now() {
    let now = ts("2026-05-10T10:00:00Z");
    let connectors: Vec<Connector> = (0..25)
        .map(|i| Connector {
            connector_id: format!("c{i}"),
            kind: "slack".to_string(),
            display_name: format!("C{i}"),
            status: Status::Healthy,
            updated_at: now,
            ..Connector::default()
        })
        .collect();
    let input = ProjectionInput {
        connectors,
        now,
        ..ProjectionInput::default()
    };
    let page = build_connector_page(&input);
    assert_eq!(page.page.limit, 20);
    assert_eq!(page.page.next_cursor, "20");
    assert_eq!(page.items.len(), 20);

    // A zero now (default input) falls back to the current time.
    let page = build_connector_page(&ProjectionInput::default());
    assert!(page.items.is_empty());
    assert_eq!(page.page.limit, 20);
}

#[test]
fn parse_cursor_offset_rejects_invalid_and_negative() {
    assert_eq!(parse_cursor_offset(""), 0);
    assert_eq!(parse_cursor_offset("   "), 0);
    assert_eq!(parse_cursor_offset("0"), 0);
    assert_eq!(parse_cursor_offset("5"), 5);
    assert_eq!(parse_cursor_offset(" 7 "), 7);
    assert_eq!(parse_cursor_offset("-1"), 0);
    assert_eq!(parse_cursor_offset("abc"), 0);
}

// --- management.go wire records --------------------------------------------

#[test]
fn wire_records_round_trip_camel_case() {
    let now = ts("2026-05-10T10:00:00Z");
    let bundle = SupportEvidenceBundle {
        support_evidence_id: "support-1".to_string(),
        tenant_id: "ten_channels".to_string(),
        connector_id: "discord-main".to_string(),
        generated_by_principal_id: "principal-1".to_string(),
        generated_at: now,
        current_state: ManagementState::Degraded,
        state_transitions: vec!["ready".to_string(), "degraded".to_string()],
        diagnostic_refs: vec!["diag-1".to_string()],
        repair_refs: Vec::new(),
        routing_decision_refs: Vec::new(),
        reply_outcome_refs: Vec::new(),
        delivery_outcome_refs: Vec::new(),
        audit_refs: vec!["audit-1".to_string()],
        redactions: vec!["credentials".to_string()],
        retention_expires_at: now + Duration::days(90),
        redaction_status: RedactionStatus::Redacted,
        safe_evidence: HashMap::from([("connectorKind".to_string(), "discord".to_string())]),
    };
    let json = serde_json::to_value(&bundle).unwrap();
    let obj = json.as_object().unwrap();
    assert!(obj.contains_key("supportEvidenceId"));
    assert!(obj.contains_key("generatedByPrincipalId"));
    assert_eq!(obj["currentState"], serde_json::json!("degraded"));
    assert_eq!(
        obj["stateTransitions"],
        serde_json::json!(["ready", "degraded"])
    );
    // Go omitempty omits empty slice fields entirely.
    assert!(
        !obj.contains_key("routingDecisionRefs"),
        "empty omitempty list is absent"
    );
    assert!(
        !obj.contains_key("repairRefs"),
        "empty omitempty list is absent"
    );
    let back: SupportEvidenceBundle = serde_json::from_value(json).unwrap();
    assert_eq!(back, bundle);
}
