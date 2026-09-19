use chrono::{TimeZone, Utc};
use kura_integrations::DiagnosticReasonCode;
use kura_opsreadiness::{
    HostedOperationalProfile, SmokeProbeInput, SmokeProbeResult, SmokeReportStatus,
    build_integration_diagnostic_smoke_report, build_smoke_probe_outcome, generate_hosted_run_id,
    validate_hosted_profile, validate_hosted_redaction,
};

#[test]
fn hosted_run_id_generation() {
    let started = Utc
        .with_ymd_and_hms(2026, 4, 23, 16, 0, 0)
        .single()
        .unwrap();
    let id = generate_hosted_run_id("My Profile", started).unwrap();
    assert!(id.starts_with("my_profile_"));
    assert!(generate_hosted_run_id("", started).is_err());
}

#[test]
fn hosted_profile_rejects_production_data_dir() {
    let profile = HostedOperationalProfile {
        profile_id: "p1".to_string(),
        profile_name: "P1".to_string(),
        environment: "test".to_string(),
        host_class: "stable_test_host".to_string(),
        data_directory: "~/.kura".to_string(),
        log_directory: "/logs".to_string(),
        artifact_directory: "/art".to_string(),
        backup_directory: "/bak".to_string(),
        report_directory: "/rep".to_string(),
        temporary_directory: "/tmp".to_string(),
        live_connector_mode: "disabled".to_string(),
        retention_days: 90,
        ..HostedOperationalProfile::default()
    };
    let err = validate_hosted_profile(&profile).unwrap_err();
    assert!(err.contains("production data directory"));
}

#[test]
fn hosted_redaction_detects_raw_credential() {
    #[derive(serde::Serialize)]
    struct Leaky {
        value: String,
    }
    let err = validate_hosted_redaction(
        "test",
        &Leaky {
            value: "access_token abc".to_string(),
        },
    )
    .unwrap_err();
    assert!(err.contains("raw credential material"));
    assert!(
        validate_hosted_redaction(
            "test",
            &Leaky {
                value: "ok".to_string()
            }
        )
        .is_ok()
    );
}

#[test]
fn smoke_probe_blocks_on_missing_credentials() {
    let probe = SmokeProbeInput {
        domain_kind: "calendar".to_string(),
        provider_kind: "feishu_lark".to_string(),
        supported: true,
        read_only_or_reversible: true,
        ..SmokeProbeInput::default()
    };
    let outcome = build_smoke_probe_outcome("r1", 0, &probe, Utc::now());
    assert_eq!(outcome.result, SmokeProbeResult::Blocked);
    assert_eq!(
        outcome.reason_code,
        DiagnosticReasonCode::TokenMissing.as_str()
    );
}

#[test]
fn smoke_report_aggregates_domain_summary() {
    let probes = vec![
        SmokeProbeInput {
            tenant_id: "t1".to_string(),
            domain_kind: "calendar".to_string(),
            supported: true,
            read_only_or_reversible: true,
            safe_credentials_available: true,
            tenant_approval_available: true,
            provider_available: true,
            ..SmokeProbeInput::default()
        },
        SmokeProbeInput {
            tenant_id: "t1".to_string(),
            domain_kind: "mail".to_string(),
            supported: true,
            read_only_or_reversible: true,
            safe_credentials_available: true,
            tenant_approval_available: true,
            provider_available: true,
            ..SmokeProbeInput::default()
        },
    ];
    let report = build_integration_diagnostic_smoke_report("r1", "op", &probes, Utc::now());
    assert_eq!(report.status, SmokeReportStatus::Completed);
    assert_eq!(report.domain_summary.len(), 2);
    assert_eq!(report.probe_outcomes.len(), 2);
}
