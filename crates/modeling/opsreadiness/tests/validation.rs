use kura_opsreadiness::{
    BackupArtifact, CalendarSmokeInput, LaunchGateEvidence, REQUIRED_LAUNCH_WORKLOADS,
    RESULT_NO_SHIP, RESULT_SHIP, RealAccountSmokeStatus, RestartEvent, WorkloadEvidence,
    calendar_real_account_smoke, contains_raw_credential_material, validate_backup_artifact,
    validate_launch_gate, validate_restart_recovery,
};

fn passing_smoke(domain: &str) -> RealAccountSmokeStatus {
    RealAccountSmokeStatus {
        domain: domain.to_string(),
        safe_credentials_available: true,
        enabled: true,
        result: "pass".to_string(),
        fake_backend_coverage_passing: true,
        ..RealAccountSmokeStatus::default()
    }
}

#[test]
fn raw_credential_detection() {
    assert!(contains_raw_credential_material(
        "exposed access_token value"
    ));
    assert!(!contains_raw_credential_material("all good here"));
}

#[test]
fn launch_gate_ships_with_full_evidence() {
    let evidence = LaunchGateEvidence {
        channels: vec![
            passing_smoke("discord"),
            passing_smoke("slack"),
            passing_smoke("web"),
        ],
        provider_smoke: vec![passing_smoke("calendar"), passing_smoke("mail")],
        workloads: REQUIRED_LAUNCH_WORKLOADS
            .iter()
            .map(|name| WorkloadEvidence {
                name: name.to_string(),
                status: "pass".to_string(),
                ..WorkloadEvidence::default()
            })
            .collect(),
        soak_duration_met: true,
        support_bundle_validated: true,
        redaction_validated: true,
        ..LaunchGateEvidence::default()
    };
    let decision = validate_launch_gate(&evidence);
    assert_eq!(decision.result, RESULT_SHIP);
    assert!(decision.reasons.is_empty());
}

#[test]
fn launch_gate_blocks_on_missing_workload() {
    let evidence = LaunchGateEvidence {
        channels: vec![
            passing_smoke("discord"),
            passing_smoke("slack"),
            passing_smoke("web"),
        ],
        provider_smoke: vec![passing_smoke("calendar"), passing_smoke("mail")],
        workloads: vec![WorkloadEvidence {
            name: "activation".to_string(),
            status: "pass".to_string(),
            ..WorkloadEvidence::default()
        }],
        soak_duration_met: true,
        support_bundle_validated: true,
        redaction_validated: true,
        ..LaunchGateEvidence::default()
    };
    let decision = validate_launch_gate(&evidence);
    assert_eq!(decision.result, RESULT_NO_SHIP);
    assert!(!decision.reasons.is_empty());
    assert!(
        decision
            .reasons
            .iter()
            .any(|r| r.contains("missing required workload evidence"))
    );
}

#[test]
fn backup_artifact_requires_identity() {
    let err = validate_backup_artifact(&BackupArtifact::default()).unwrap_err();
    assert!(err.contains("artifact id is required"));
}

#[test]
fn restart_recovery_requires_three_events() {
    let err = validate_restart_recovery(&[RestartEvent::default()]).unwrap_err();
    assert!(err.contains("at least 3 daemon restarts"));
}

#[test]
fn calendar_smoke_passes_when_exercised() {
    let status = calendar_real_account_smoke(&CalendarSmokeInput {
        safe_credentials_available: true,
        enabled: true,
        create_update_cancel_exercised: true,
        fake_backend_coverage_passing: true,
        ..CalendarSmokeInput::default()
    });
    assert_eq!(status.result, "pass");

    let skipped = calendar_real_account_smoke(&CalendarSmokeInput::default());
    assert_eq!(skipped.result, "skip");
    assert!(!skipped.skip_reason.is_empty());
}
