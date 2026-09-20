use chrono::{TimeZone, Utc};
use kura_opsreadiness::{
    BackupArtifact, HostedDeploymentManifest, REQUIRED_FAULT_TYPES, REQUIRED_WORKLOAD_AREAS,
    TenantStateSummary,
};

#[test]
fn required_lists_are_populated() {
    assert_eq!(REQUIRED_WORKLOAD_AREAS.len(), 8);
    assert_eq!(REQUIRED_FAULT_TYPES.len(), 6);
    assert_eq!(REQUIRED_WORKLOAD_AREAS[0], "runtime");
}

#[test]
fn tenant_state_summary_roundtrips_camel_case() {
    let summary = TenantStateSummary {
        tenant_id: "ten_1".to_string(),
        credential_refs: vec!["sec_1".to_string()],
        quota_state: "ok".to_string(),
        work_state: "idle".to_string(),
        reconnect_required: false,
        operator_action_needed: true,
        ..TenantStateSummary::default()
    };
    let json = serde_json::to_string(&summary).unwrap();
    assert!(json.contains("tenantId"));
    assert!(json.contains("operatorActionNeeded"));
    let back: TenantStateSummary = serde_json::from_str(&json).unwrap();
    assert_eq!(back.tenant_id, "ten_1");
    assert!(back.operator_action_needed);
}

#[test]
fn backup_artifact_roundtrips() {
    let artifact = BackupArtifact {
        artifact_id: "bak_1".to_string(),
        created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).single().unwrap(),
        source_version: "1.2.3".to_string(),
        source_environment: "test".to_string(),
        tenant_count: 2,
        tenant_state_summary: vec![TenantStateSummary {
            tenant_id: "t1".to_string(),
            ..TenantStateSummary::default()
        }],
        ..BackupArtifact::default()
    };
    let json = serde_json::to_string(&artifact).unwrap();
    assert!(json.contains("artifactId"));
    let back: BackupArtifact = serde_json::from_str(&json).unwrap();
    assert_eq!(back.tenant_count, 2);
    assert_eq!(back.tenant_state_summary.len(), 1);
}

#[test]
fn hosted_manifest_roundtrips() {
    let manifest = HostedDeploymentManifest {
        manifest_id: "m1".to_string(),
        run_id: "r1".to_string(),
        redaction_status: "passed".to_string(),
        ..HostedDeploymentManifest::default()
    };
    let json = serde_json::to_string(&manifest).unwrap();
    assert!(json.contains("manifestId"));
    let back: HostedDeploymentManifest = serde_json::from_str(&json).unwrap();
    assert_eq!(back.run_id, "r1");
    assert_eq!(back.redaction_status, "passed");
}
