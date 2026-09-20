//! Ported Go event-builder tests for the evaluation product events
//! (daemon/internal/events/evaluation_product_test.go) plus serde coverage.

use chrono::{TimeZone, Utc};
use kura_events::*;

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 4, 29, 10, 0, 0).unwrap()
}

fn payload_str<'a>(event: &'a Event, key: &str) -> &'a str {
    event
        .payload
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
}

// ---- evaluation_product_test.go ports ----

#[test]
fn evaluation_product_audit_event_construction() {
    let now = now();
    let event = evaluation_product_audit_event(
        EVALUATION_PRODUCT_RETENTION_APPLIED_NAME,
        EvaluationProductAuditPayload {
            tenant_id: "ten_eval".into(),
            actor_id: "prn_eval".into(),
            action: "retention.apply".into(),
            target_kind: kura_evaluation::ProductResourceKind::DiscoveredCandidate,
            target_id: "candidate_1".into(),
            outcome: "retention_applied".into(),
            reason_code: "evaluation.retention_applied".into(),
            retention_app_id: "retention_1".into(),
            occurred_at: now,
            ..Default::default()
        },
    );
    assert_eq!(event.category, "evaluation");
    assert_eq!(event.name, EVALUATION_PRODUCT_RETENTION_APPLIED_NAME);
    assert_eq!(event.tenant_id, "ten_eval");
    assert_eq!(event.resource.kind, "discovered_candidate");
    assert_eq!(event.resource.id, "candidate_1");
    assert_eq!(payload_str(&event, "retentionApplicationId"), "retention_1");
    assert_eq!(payload_str(&event, "targetKind"), "discovered_candidate");
    assert_eq!(
        payload_str(&event, "createdAt"),
        now.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
    );
}

#[test]
fn evaluation_discovery_event_construction() {
    let now = now();
    let event = evaluation_discovery_event(
        EVALUATION_DISCOVERY_CANDIDATE_NAME,
        EvaluationDiscoveryPayload {
            tenant_id: "ten_eval".into(),
            policy_id: "policy_1".into(),
            discovery_run_id: "discovery_run_1".into(),
            discovered_candidate_id: "candidate_1".into(),
            status: kura_evaluation::ProductLifecycleStatus::Partial,
            reason_code: "max_inspected_records".into(),
            redaction_status: Some(kura_evaluation::RedactionStatus::Redacted),
            occurred_at: now,
            ..Default::default()
        },
    );
    assert_eq!(event.category, "evaluation");
    assert_eq!(event.name, EVALUATION_DISCOVERY_CANDIDATE_NAME);
    assert_eq!(event.tenant_id, "ten_eval");
    assert_eq!(event.resource.kind, "discovered_candidate");
    assert_eq!(event.resource.id, "candidate_1");
    assert_eq!(payload_str(&event, "discoveryRunId"), "discovery_run_1");
    assert_eq!(payload_str(&event, "policyId"), "policy_1");
    assert_eq!(payload_str(&event, "status"), "partial");
    assert_eq!(payload_str(&event, "redactionStatus"), "redacted");
}

#[test]
fn evaluation_fixture_event_construction() {
    let now = now();
    let event = evaluation_fixture_event(
        EVALUATION_FIXTURE_CREATED_NAME,
        EvaluationFixturePayload {
            tenant_id: "ten_eval".into(),
            actor_id: "prn_eval".into(),
            fixture_id: "product_fixture_1".into(),
            revision_id: "revision_1".into(),
            source_candidate_id: "candidate_1".into(),
            source_evidence_refs: vec!["evidence_1".into()],
            review_state: Some(kura_evaluation::ProductLifecycleStatus::Draft),
            redaction_status: Some(kura_evaluation::RedactionStatus::Redacted),
            outcome: "created".into(),
            occurred_at: now,
            ..Default::default()
        },
    );
    assert_eq!(event.category, "evaluation");
    assert_eq!(event.name, EVALUATION_FIXTURE_CREATED_NAME);
    assert_eq!(event.resource.id, "product_fixture_1");
    assert_eq!(event.resource.kind, "product_fixture");
    assert_eq!(payload_str(&event, "revisionId"), "revision_1");
    assert_eq!(payload_str(&event, "reviewState"), "draft");
    assert_eq!(payload_str(&event, "redactionStatus"), "redacted");
    assert_eq!(
        event.payload["sourceEvidenceRefs"],
        serde_json::json!(["evidence_1"])
    );
}

#[test]
fn evaluation_campaign_dashboard_and_inspection_event_construction() {
    let now = now();

    let campaign = evaluation_campaign_event(
        EVALUATION_CAMPAIGN_RESULTS_PUBLISHED_NAME,
        EvaluationCampaignPayload {
            tenant_id: "ten_eval".into(),
            actor_id: "prn_eval".into(),
            campaign_id: "campaign_1".into(),
            campaign_item_id: "campaign_item_1".into(),
            attempt_group_id: "attempt_group_1".into(),
            status: kura_evaluation::ProductLifecycleStatus::Published,
            outcome: "published".into(),
            redaction_status: Some(kura_evaluation::RedactionStatus::Redacted),
            occurred_at: now,
            ..Default::default()
        },
    );
    assert_eq!(campaign.name, EVALUATION_CAMPAIGN_RESULTS_PUBLISHED_NAME);
    assert_eq!(campaign.resource.kind, "campaign");
    assert_eq!(payload_str(&campaign, "status"), "published");
    assert_eq!(payload_str(&campaign, "campaignItemId"), "campaign_item_1");
    assert_eq!(payload_str(&campaign, "attemptGroupId"), "attempt_group_1");

    let dashboard = evaluation_dashboard_event(
        EVALUATION_DASHBOARD_PROJECTION_GENERATED_NAME,
        EvaluationDashboardPayload {
            tenant_id: "ten_eval".into(),
            projection_id: "projection_1".into(),
            window_start: now - chrono::Duration::hours(1),
            window_end: now,
            generated_at: now,
            outcome: "generated".into(),
            occurred_at: now,
            ..Default::default()
        },
    );
    assert_eq!(
        dashboard.name,
        EVALUATION_DASHBOARD_PROJECTION_GENERATED_NAME
    );
    assert_eq!(dashboard.resource.kind, "dashboard_projection");
    assert_eq!(payload_str(&dashboard, "projectionId"), "projection_1");
    assert_eq!(
        dashboard.payload["windowStart"],
        (now - chrono::Duration::hours(1)).to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
    );
    assert_eq!(
        dashboard.payload["generatedAt"],
        now.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
    );

    let inspection = evaluation_tool_call_inspection_event(
        EVALUATION_TOOL_CALL_INSPECTION_GENERATED_NAME,
        EvaluationToolCallInspectionPayload {
            tenant_id: "ten_eval".into(),
            inspection_id: "inspection_1".into(),
            campaign_id: "campaign_1".into(),
            campaign_item_id: "campaign_item_1".into(),
            classification: kura_evaluation::INSPECTION_MATCHED.to_string(),
            redaction_status: Some(kura_evaluation::RedactionStatus::Clean),
            outcome: "generated".into(),
            occurred_at: now,
            ..Default::default()
        },
    );
    assert_eq!(
        inspection.name,
        EVALUATION_TOOL_CALL_INSPECTION_GENERATED_NAME
    );
    assert_eq!(inspection.resource.kind, "tool_call_inspection");
    assert_eq!(
        payload_str(&inspection, "classification"),
        kura_evaluation::INSPECTION_MATCHED
    );
    assert_eq!(payload_str(&inspection, "redactionStatus"), "clean");
    assert_eq!(
        payload_str(&inspection, "campaignItemId"),
        "campaign_item_1"
    );
}

// ---- additional behavioral coverage ----

#[test]
fn evaluation_discovery_resource_resolves_by_most_specific_evidence() {
    let now = now();

    // Policy-only: resource is the discovery policy.
    let policy = evaluation_discovery_event(
        EVALUATION_DISCOVERY_POLICY_CHANGED_NAME,
        EvaluationDiscoveryPayload {
            tenant_id: "ten_eval".into(),
            policy_id: "policy_1".into(),
            status: kura_evaluation::ProductLifecycleStatus::Running,
            occurred_at: now,
            ..Default::default()
        },
    );
    assert_eq!(policy.resource.kind, "discovery_policy");
    assert_eq!(policy.resource.id, "policy_1");

    // Suppression beats a policy reference.
    let suppressed = evaluation_discovery_event(
        EVALUATION_DISCOVERY_SUPPRESSED_NAME,
        EvaluationDiscoveryPayload {
            tenant_id: "ten_eval".into(),
            policy_id: "policy_1".into(),
            suppression_id: "suppression_1".into(),
            status: kura_evaluation::ProductLifecycleStatus::Suppressed,
            occurred_at: now,
            ..Default::default()
        },
    );
    assert_eq!(suppressed.resource.kind, "suppression");
    assert_eq!(suppressed.resource.id, "suppression_1");
    assert_eq!(payload_str(&suppressed, "suppressionId"), "suppression_1");

    // Discovery-run default when no candidate/suppression and a run id exists.
    let run = evaluation_discovery_event(
        EVALUATION_DISCOVERY_COMPLETED_NAME,
        EvaluationDiscoveryPayload {
            tenant_id: "ten_eval".into(),
            policy_id: "policy_1".into(),
            discovery_run_id: "discovery_run_1".into(),
            status: kura_evaluation::ProductLifecycleStatus::Completed,
            occurred_at: now,
            ..Default::default()
        },
    );
    assert_eq!(run.resource.kind, "discovery_run");
    assert_eq!(run.resource.id, "discovery_run_1");
}

#[test]
fn evaluation_events_omit_empty_optional_payload_fields() {
    let now = now();

    // Audit event with only the mandatory envelope fields.
    let audit = evaluation_product_audit_event(
        EVALUATION_PRODUCT_AUDIT_RECORDED_NAME,
        EvaluationProductAuditPayload {
            tenant_id: "ten_eval".into(),
            actor_id: "prn_eval".into(),
            action: "audit".into(),
            target_kind: kura_evaluation::ProductResourceKind::ProductFixture,
            outcome: "recorded".into(),
            occurred_at: now,
            ..Default::default()
        },
    );
    assert!(audit.payload.get("targetId").is_none());
    assert!(audit.payload.get("reasonCode").is_none());
    assert!(audit.payload.get("evidenceRefs").is_none());
    assert!(audit.payload.get("retentionApplicationId").is_none());

    // Fixture event with no review state / redaction status emits neither key.
    let fixture = evaluation_fixture_event(
        EVALUATION_FIXTURE_CREATED_NAME,
        EvaluationFixturePayload {
            tenant_id: "ten_eval".into(),
            fixture_id: "product_fixture_1".into(),
            outcome: "created".into(),
            occurred_at: now,
            ..Default::default()
        },
    );
    assert!(fixture.payload.get("reviewState").is_none());
    assert!(fixture.payload.get("redactionStatus").is_none());

    // Dashboard with unset window bounds omits them; generated_at falls back.
    let dashboard = evaluation_dashboard_event(
        EVALUATION_DASHBOARD_PROJECTION_GENERATED_NAME,
        EvaluationDashboardPayload {
            tenant_id: "ten_eval".into(),
            projection_id: "projection_1".into(),
            occurred_at: now,
            ..Default::default()
        },
    );
    assert!(dashboard.payload.get("windowStart").is_none());
    assert!(dashboard.payload.get("windowEnd").is_none());
    assert_eq!(
        dashboard.payload["generatedAt"],
        now.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
    );
}

// ---- serde round-trip coverage ----

#[test]
fn evaluation_input_structs_serialize_camel_case() {
    let audit = EvaluationProductAuditPayload {
        tenant_id: "ten_eval".into(),
        actor_id: "prn_eval".into(),
        action: "retention.apply".into(),
        target_kind: kura_evaluation::ProductResourceKind::DiscoveredCandidate,
        target_id: "candidate_1".into(),
        outcome: "retention_applied".into(),
        reason_code: "evaluation.retention_applied".into(),
        evidence_refs: vec!["evidence_1".into()],
        retention_app_id: "retention_1".into(),
        occurred_at: now(),
    };
    let json = serde_json::to_value(&audit).unwrap();
    assert_eq!(json["tenantId"], "ten_eval");
    assert_eq!(json["actorId"], "prn_eval");
    assert_eq!(json["targetKind"], "discovered_candidate");
    assert_eq!(json["targetId"], "candidate_1");
    assert_eq!(json["evidenceRefs"], serde_json::json!(["evidence_1"]));
    assert_eq!(json["retentionAppId"], "retention_1");

    let discovery = EvaluationDiscoveryPayload {
        tenant_id: "ten_eval".into(),
        status: kura_evaluation::ProductLifecycleStatus::Partial,
        redaction_status: Some(kura_evaluation::RedactionStatus::Redacted),
        occurred_at: now(),
        ..Default::default()
    };
    let json = serde_json::to_value(&discovery).unwrap();
    assert_eq!(json["discoveryRunId"], "");
    assert_eq!(json["redactionStatus"], "redacted");
    assert_eq!(json["status"], "partial");
}

#[test]
fn evaluation_event_round_trips_with_camel_case_payload() {
    let event = evaluation_campaign_event(
        EVALUATION_CAMPAIGN_COMPLETED_NAME,
        EvaluationCampaignPayload {
            tenant_id: "ten_eval".into(),
            campaign_id: "campaign_1".into(),
            status: kura_evaluation::ProductLifecycleStatus::Completed,
            occurred_at: now(),
            ..Default::default()
        },
    );
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["category"], "evaluation");
    assert_eq!(json["name"], "evaluation.campaign.completed");
    assert_eq!(json["resource"]["kind"], "campaign");
    assert_eq!(json["payload"]["campaignId"], "campaign_1");
    assert_eq!(json["payload"]["status"], "completed");

    let back: Event = serde_json::from_value(json).unwrap();
    assert_eq!(back.name, "evaluation.campaign.completed");
    assert_eq!(back.payload["status"], "completed");
}
