use chrono::Utc;
use kura_integrations::{
    BackendBinding, BackendKind, CreateInput, DiagnosticInspectionInput, DiagnosticManager,
    DiagnosticReasonCode, DiagnosticRetentionState, FeishuLarkDiagnosticBackend, FreshnessState,
    IntegrationError, Manager, ProbeKind, ProviderDiagnosticEvidence, ReadinessStatus,
    RedactionStatus, Resource, UpdateReadinessInput, backend_kind_supports_domain,
    classify_provider_evidence, diagnostic_failure_for_operation_failure,
    new_diagnostic_retention_record, redact_diagnostic_summary,
};
use serde_json::Map;

fn manager() -> Manager {
    Manager::new("test")
}

fn create_input(integration_id: &str, domain_kind: &str) -> CreateInput {
    CreateInput {
        tenant_id: "ten_1".to_string(),
        integration_id: integration_id.to_string(),
        domain_kind: domain_kind.to_string(),
        display_name: "Test".to_string(),
        backend_binding: BackendBinding {
            backend_kind: BackendKind::FakeLocal,
            supports_probe_read: true,
            ..BackendBinding::default()
        },
        ..CreateInput::default()
    }
}

#[test]
fn create_sets_not_configured() {
    let manager = manager();
    let resource = manager
        .create(create_input("cal-1", "calendar"))
        .expect("create");
    assert_eq!(resource.readiness_status, ReadinessStatus::NotConfigured);
    assert_eq!(resource.integration_id, "cal-1");
    assert_eq!(resource.domain_kind, "calendar");
}

#[test]
fn update_readiness_to_healthy() {
    let manager = manager();
    let _ = manager
        .create(create_input("cal-1", "calendar"))
        .expect("create");
    let resource = manager
        .update_readiness(
            "cal-1",
            UpdateReadinessInput {
                readiness_status: ReadinessStatus::Healthy,
                ..UpdateReadinessInput::default()
            },
        )
        .expect("update");
    assert_eq!(resource.readiness_status, ReadinessStatus::Healthy);
    assert!(resource.last_ready_at.is_some());
}

#[test]
fn create_rejects_missing_fields() {
    let manager = manager();
    assert!(manager.create(create_input("", "calendar")).is_err());
    assert!(manager.create(create_input("cal-1", "")).is_err());
}

#[test]
fn fake_backend_supports_calendar_and_mail() {
    assert!(backend_kind_supports_domain(
        BackendKind::FakeLocal,
        "calendar"
    ));
    assert!(backend_kind_supports_domain(BackendKind::FakeLocal, "mail"));
    assert!(!backend_kind_supports_domain(
        BackendKind::FakeLocal,
        "reminders"
    ));
    assert!(!backend_kind_supports_domain(
        BackendKind::AdapterRpc,
        "calendar"
    ));
}

#[test]
fn run_probe_returns_completed() {
    let manager = manager();
    let _ = manager
        .create(create_input("cal-1", "calendar"))
        .expect("create");
    let input = Map::new();
    let (_resource, result, summary) = manager
        .run_probe("cal-1", ProbeKind::Inspect, &input)
        .expect("probe");
    assert_eq!(result.status, "completed");
    assert_eq!(summary.integration_id, "cal-1");
}

#[test]
fn classify_provider_evidence_maps_token_expired() {
    let evidence = ProviderDiagnosticEvidence {
        provider_error_class: "token_expired".to_string(),
        ..ProviderDiagnosticEvidence::default()
    };
    let classification = classify_provider_evidence(&evidence);
    assert_eq!(
        classification.reason_code,
        DiagnosticReasonCode::TokenExpired
    );
    assert!(!classification.ambiguous);
}

#[test]
fn classify_provider_evidence_feishu_scope() {
    let evidence = ProviderDiagnosticEvidence {
        provider_kind: "feishu_lark".to_string(),
        provider_error_class: "99991669".to_string(),
        ..ProviderDiagnosticEvidence::default()
    };
    let classification = classify_provider_evidence(&evidence);
    assert_eq!(
        classification.reason_code,
        DiagnosticReasonCode::ScopeMissing
    );
}

#[test]
fn operation_failure_projection_is_fresh() {
    let projection = diagnostic_failure_for_operation_failure(
        "calendar",
        "feishu_lark",
        "cal-1",
        "create_event",
        "scope_not_granted",
        "missing scope",
        true,
        Utc::now(),
    );
    assert_eq!(projection.reason_code, DiagnosticReasonCode::ScopeMissing);
    assert_eq!(projection.freshness_state, FreshnessState::Fresh);
}

#[test]
fn redact_diagnostic_summary_suppresses_secrets() {
    let result = redact_diagnostic_summary("Authorization: Bearer abc123def");
    assert_eq!(result.status, RedactionStatus::Suppressed);
    assert_eq!(result.summary, "diagnostic detail suppressed");
}

#[test]
fn redact_diagnostic_summary_passes_clean_text() {
    let result = redact_diagnostic_summary("integration is healthy");
    assert_eq!(result.status, RedactionStatus::Redacted);
    assert_eq!(result.summary, "integration is healthy");
}

#[test]
fn diagnostic_manager_inspect_limited_diagnostic() {
    let manager = DiagnosticManager::new();
    let resource = Resource {
        domain_kind: "calendar".to_string(),
        readiness_status: ReadinessStatus::Healthy,
        backend_binding: BackendBinding {
            backend_kind: BackendKind::FakeLocal,
            supports_probe_read: true,
            ..BackendBinding::default()
        },
        ..Resource::default()
    };
    let result = manager.inspect(DiagnosticInspectionInput {
        resource,
        ..DiagnosticInspectionInput::default()
    });
    assert_eq!(result.reason_code, DiagnosticReasonCode::LimitedDiagnostic);
    assert_eq!(result.capability, "integration.readiness");
}

#[test]
fn feishu_lark_backend_run_probe_healthy() {
    let backend = FeishuLarkDiagnosticBackend::new();
    let resource = Resource {
        domain_kind: "calendar".to_string(),
        readiness_status: ReadinessStatus::Healthy,
        backend_binding: BackendBinding {
            backend_kind: BackendKind::FeishuLark,
            ..BackendBinding::default()
        },
        ..Resource::default()
    };
    let result = backend
        .run_probe(&resource, ProbeKind::Inspect, &serde_json::Map::new())
        .unwrap();
    assert_eq!(result.status, "completed");
}

#[test]
fn feishu_lark_backend_rejects_unsupported_domain() {
    let backend = FeishuLarkDiagnosticBackend::new();
    let resource = Resource {
        domain_kind: "chat".to_string(),
        backend_binding: BackendBinding {
            backend_kind: BackendKind::FeishuLark,
            ..BackendBinding::default()
        },
        ..Resource::default()
    };
    let err = backend
        .run_probe(&resource, ProbeKind::Inspect, &serde_json::Map::new())
        .unwrap_err();
    assert!(matches!(err, IntegrationError::ProbeUnsupported));
}

#[test]
fn retention_record_is_active_by_default() {
    let record = new_diagnostic_retention_record("ten_1", "integration", "cal-1", Utc::now());
    assert_eq!(record.retention_state, DiagnosticRetentionState::Active);
    assert!(record.retention_record_id.starts_with("diag_retention_"));
    assert_eq!(record.default_expires_at, record.effective_expires_at);
}
