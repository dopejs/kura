//! Denial payloads, denial detail projection, and evidence export with
//! redaction (port of `denial.go`).

use chrono::SecondsFormat;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Map;
use serde_json::Value;

use crate::catalog::REASON_QUOTA_STATE_UNAVAILABLE;
use crate::catalog::definition_for;
use crate::error::Result;
use crate::manager::Manager;
use crate::projection::recovery_actions_for_quota_status;
use crate::types::AbuseRestrictionStatus;
use crate::types::AbuseRestrictionSummary;
use crate::types::BillingEvidenceExport;
use crate::types::BillingEvidenceRedaction;
use crate::types::Category;
use crate::types::DenialClassification;
use crate::types::NearLimitReason;
use crate::types::QuotaDenial;
use crate::types::QuotaDenialDetail;
use crate::types::QuotaPeriod;
use crate::types::QuotaStatus;
use crate::types::QuotaStatusItem;
use crate::types::RecoveryAction;
use crate::types::TenantQuotaDashboard;
use crate::types::go_zero_time;
use crate::types::is_zero_i64;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DenialPayload {
    pub code: String,
    pub reason_code: String,
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "Category::is_empty")]
    pub category: Category,
    pub operation_key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub period_start: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub period_end: String,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub requested_amount: i64,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub remaining_amount: i64,
    pub message: String,
}

/// Stable denial payload for an exhausted quota.
#[must_use]
pub fn new_quota_exhausted_denial(
    tenant_id: &str,
    category: &Category,
    operation_key: &str,
    requested: i64,
    remaining: i64,
    period: &QuotaPeriod,
) -> DenialPayload {
    let reason = definition_for(category)
        .map(|definition| definition.denial_reason_code)
        .filter(|reason| !reason.is_empty())
        .unwrap_or_else(|| format!("quota_denied:{category}_exhausted"));
    DenialPayload {
        code: "quota_denied".to_string(),
        reason_code: reason,
        tenant_id: tenant_id.to_string(),
        category: category.clone(),
        operation_key: operation_key.to_string(),
        period_start: period
            .period_start
            .to_rfc3339_opts(SecondsFormat::Secs, true),
        period_end: period.period_end.to_rfc3339_opts(SecondsFormat::Secs, true),
        requested_amount: requested,
        remaining_amount: remaining,
        message: format!("Quota exhausted for {category}."),
    }
}

/// Stable denial payload for fail-closed quota state unavailability.
#[must_use]
pub fn new_quota_state_unavailable_denial(tenant_id: &str, operation_key: &str) -> DenialPayload {
    DenialPayload {
        code: "quota_denied".to_string(),
        reason_code: REASON_QUOTA_STATE_UNAVAILABLE.to_string(),
        tenant_id: tenant_id.to_string(),
        operation_key: operation_key.to_string(),
        message: "Quota state is unavailable; hosted work cannot start.".to_string(),
        ..Default::default()
    }
}

/// Project a stored denial into the operator-facing detail view.
#[must_use]
pub fn project_denial_detail(
    denial: &QuotaDenial,
    restriction: Option<AbuseRestrictionSummary>,
) -> QuotaDenialDetail {
    let classification = classify_denial(denial, restriction.as_ref());
    let status = status_for_denial_classification(&classification);
    let mut detail = QuotaDenialDetail {
        denial_id: denial.denial_id.clone(),
        tenant_id: denial.tenant_id.clone(),
        operation_ref: safe_operation_ref(&denial.operation_key),
        operation_key: denial.operation_key.clone(),
        guarded_entry_point: denial.guarded_entry_point.clone(),
        category: denial.category.clone(),
        reason_code: denial.reason_code.clone(),
        classification: classification.clone(),
        requested_amount: denial.requested_amount,
        remaining_amount: denial.remaining_amount,
        recovery_actions: recovery_actions_for_quota_status(
            &status,
            &NearLimitReason::from(NearLimitReason::NONE),
        ),
        restriction,
        created_at: denial.created_at,
    };
    if classification == DenialClassification::ABUSE_RESTRICTION && detail.restriction.is_none() {
        detail.restriction = Some(AbuseRestrictionSummary {
            status: AbuseRestrictionStatus::from(AbuseRestrictionStatus::ACTIVE),
            affected_category: denial.category.clone(),
            recovery_action: RecoveryAction::from(RecoveryAction::CONTACT_SUPPORT),
            visible_reason_code: denial.reason_code.clone(),
            support_contact_allowed: true,
            ..Default::default()
        });
    }
    detail
}

impl Manager {
    /// Hydrate a stored denial into its detail view, attaching an explicit
    /// abuse restriction record when one covers the denial.
    pub async fn denial_detail(
        &self,
        tenant_id: &str,
        denial_id: &str,
    ) -> Result<Option<QuotaDenialDetail>> {
        let Some(repo) = self.repo() else {
            return Ok(None);
        };
        let Some(denial) = repo.quota_denial_by_id(tenant_id, denial_id).await? else {
            return Ok(None);
        };
        let restriction = self.restriction_for_denial(&denial).await?;
        Ok(Some(project_denial_detail(&denial, restriction)))
    }

    /// Build a redacted evidence export for a denial, including the current
    /// effective limit state and audit references.
    pub async fn evidence_export(
        &self,
        tenant_id: &str,
        denial_id: &str,
        generated_by_principal_id: &str,
        hosted: bool,
    ) -> Result<Option<BillingEvidenceExport>> {
        let Some(detail) = self.denial_detail(tenant_id, denial_id).await? else {
            return Ok(None);
        };
        let dashboard = self.quota_dashboard(tenant_id, hosted).await?;
        let usage_snapshot: Vec<QuotaStatusItem> = dashboard
            .sections
            .iter()
            .flat_map(|section| section.items.clone())
            .collect();
        let state = effective_limit_state_for_denial(&dashboard, &detail);
        let mut export =
            build_evidence_export(generated_by_principal_id, detail, usage_snapshot, state);
        export
            .audit_refs
            .insert(0, format!("quota_denial:{}", export.denial.denial_id));
        if let Some(repo) = self.repo() {
            let refs = repo
                .list_usage_evidence_refs(tenant_id, &export.denial.operation_key, 100)
                .await?;
            export.audit_refs.extend(refs);
        }
        Ok(Some(export))
    }

    async fn restriction_for_denial(
        &self,
        denial: &QuotaDenial,
    ) -> Result<Option<AbuseRestrictionSummary>> {
        let Some(repo) = self.repo() else {
            return Ok(None);
        };
        if denial.category.is_empty()
            || classify_denial(denial, None) != DenialClassification::ABUSE_RESTRICTION
        {
            return Ok(None);
        }
        let at = if denial.created_at == go_zero_time() {
            self.clock_now()
        } else {
            denial.created_at
        };
        let restrictions = repo.list_abuse_restrictions(&denial.tenant_id, at).await?;
        for record in restrictions {
            if record.affected_category != denial.category {
                continue;
            }
            if !record.visible_reason_code.is_empty()
                && !denial.reason_code.is_empty()
                && record.visible_reason_code != denial.reason_code
            {
                continue;
            }
            return Ok(Some(record.summary()));
        }
        Ok(None)
    }
}

/// Classify a denial from its reason code (and an optional explicit
/// restriction) for recovery-action projection.
#[must_use]
pub fn classify_denial(
    denial: &QuotaDenial,
    restriction: Option<&AbuseRestrictionSummary>,
) -> DenialClassification {
    let reason = denial.reason_code.to_lowercase();
    if restriction.is_some() || reason.contains("abuse_restriction") {
        return DenialClassification::from(DenialClassification::ABUSE_RESTRICTION);
    }
    if reason == REASON_QUOTA_STATE_UNAVAILABLE {
        return DenialClassification::from(DenialClassification::QUOTA_STATE_UNAVAILABLE);
    }
    if reason.contains("operator_action_needed") {
        return DenialClassification::from(DenialClassification::OPERATOR_ACTION_NEEDED);
    }
    if reason.contains("unauthorized") {
        return DenialClassification::from(DenialClassification::UNAUTHORIZED);
    }
    DenialClassification::from(DenialClassification::QUOTA_EXHAUSTION)
}

fn status_for_denial_classification(classification: &DenialClassification) -> QuotaStatus {
    match classification.as_str() {
        DenialClassification::ABUSE_RESTRICTION => QuotaStatus::from(QuotaStatus::RESTRICTED),
        DenialClassification::QUOTA_STATE_UNAVAILABLE
        | DenialClassification::OPERATOR_ACTION_NEEDED
        | DenialClassification::UNAUTHORIZED => QuotaStatus::from(QuotaStatus::UNAVAILABLE),
        _ => QuotaStatus::from(QuotaStatus::EXHAUSTED),
    }
}

/// Strip the tenant prefix from an operation key for safe external display.
#[must_use]
pub fn safe_operation_ref(operation_key: &str) -> String {
    if operation_key.is_empty() {
        return "operation:unknown".to_string();
    }
    let parts: Vec<&str> = operation_key.split(':').collect();
    if parts.len() >= 4 && parts[0] == "tenant" {
        return parts[2..].join(":");
    }
    if parts.len() >= 2 {
        return parts[parts.len() - 2..].join(":");
    }
    operation_key.to_string()
}

/// Build a redacted evidence export for a denial detail.
#[must_use]
pub fn build_evidence_export(
    generated_by_principal_id: &str,
    denial: QuotaDenialDetail,
    usage_snapshot: Vec<QuotaStatusItem>,
    effective_limit_state: Map<String, Value>,
) -> BillingEvidenceExport {
    let (clean_state, redactions) =
        redact_evidence_value("$", &Value::Object(effective_limit_state));
    let state = match clean_state {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    let redactions = append_standard_evidence_redactions(redactions);
    let mut audit_refs = Vec::new();
    if let Some(restriction) = &denial.restriction {
        if !restriction.source_audit_ref.is_empty() {
            audit_refs.push(restriction.source_audit_ref.clone());
        }
    }
    BillingEvidenceExport {
        schema_version: "2026-05-07".to_string(),
        export_id: format!("evidence_{}", denial.denial_id),
        tenant_id: denial.tenant_id.clone(),
        generated_at: Utc::now(),
        generated_by_principal_id: generated_by_principal_id.to_string(),
        denial,
        usage_snapshot,
        effective_limit_state: state,
        audit_refs,
        redactions,
    }
}

fn append_standard_evidence_redactions(
    mut redactions: Vec<BillingEvidenceRedaction>,
) -> Vec<BillingEvidenceRedaction> {
    let standard = [
        (
            "$.rawAuditPayload",
            "raw_audit_payload_excluded",
            "[EXCLUDED]",
        ),
        ("$.connectorPayload", "connector_payload", "[REDACTED]"),
        ("$.secrets", "secret", "[REDACTED]"),
        (
            "$.unrelatedRunContent",
            "unrelated_content_excluded",
            "[EXCLUDED]",
        ),
    ];
    let mut seen: std::collections::HashSet<String> =
        redactions.iter().map(|item| item.path.clone()).collect();
    for (path, reason, replacement) in standard {
        if seen.insert(path.to_string()) {
            redactions.push(BillingEvidenceRedaction {
                path: path.to_string(),
                reason: reason.to_string(),
                replacement: replacement.to_string(),
            });
        }
    }
    redactions
}

fn effective_limit_state_for_denial(
    dashboard: &TenantQuotaDashboard,
    denial: &QuotaDenialDetail,
) -> Map<String, Value> {
    let mut plan = Map::new();
    plan.insert(
        "planKey".to_string(),
        Value::from(dashboard.plan.plan_key.clone()),
    );
    plan.insert(
        "enforcementMode".to_string(),
        Value::from(dashboard.plan.enforcement_mode.as_str()),
    );
    plan.insert(
        "status".to_string(),
        Value::from(dashboard.plan.status.as_str()),
    );
    plan.insert(
        "basePlanLabel".to_string(),
        Value::from(dashboard.plan.base_plan_label.clone()),
    );
    plan.insert(
        "checkoutAvailable".to_string(),
        Value::from(dashboard.plan.checkout_available),
    );
    let mut state = Map::new();
    state.insert("plan".to_string(), Value::Object(plan));
    state.insert(
        "denialCategory".to_string(),
        Value::from(denial.category.as_str()),
    );
    for section in &dashboard.sections {
        for item in &section.items {
            if item.category != denial.category {
                continue;
            }
            let mut quota = Map::new();
            quota.insert("category".to_string(), Value::from(item.category.as_str()));
            quota.insert("unit".to_string(), Value::from(item.unit.as_str()));
            quota.insert("status".to_string(), Value::from(item.status.as_str()));
            quota.insert("baseLimit".to_string(), Value::from(item.base_limit));
            quota.insert(
                "effectiveLimit".to_string(),
                Value::from(item.effective_limit),
            );
            quota.insert("limit".to_string(), Value::from(item.limit));
            quota.insert(
                "remainingAmount".to_string(),
                Value::from(item.remaining_amount),
            );
            quota.insert("nearLimit".to_string(), Value::from(item.near_limit));
            quota.insert(
                "nearLimitReason".to_string(),
                Value::from(item.near_limit_reason.as_str()),
            );
            quota.insert(
                "typicalOperationAmount".to_string(),
                Value::from(item.typical_operation_amount),
            );
            quota.insert(
                "currentPeriod".to_string(),
                period_evidence_state(&item.current_period),
            );
            if let Some(previous) = &item.previous_period {
                quota.insert(
                    "previousPeriod".to_string(),
                    period_evidence_state(previous),
                );
            }
            if let Some(override_) = &item.override_ {
                let mut override_state = Map::new();
                override_state.insert("baseLimit".to_string(), Value::from(override_.base_limit));
                override_state.insert(
                    "effectiveLimit".to_string(),
                    Value::from(override_.effective_limit),
                );
                override_state.insert("reason".to_string(), Value::from(override_.reason.clone()));
                override_state.insert(
                    "effectiveAt".to_string(),
                    serde_json::to_value(override_.effective_at).unwrap_or(Value::Null),
                );
                override_state.insert(
                    "expiresAt".to_string(),
                    serde_json::to_value(override_.expires_at).unwrap_or(Value::Null),
                );
                quota.insert("override".to_string(), Value::Object(override_state));
            }
            if let Some(restriction) = item.restriction.as_ref().or(denial.restriction.as_ref()) {
                quota.insert(
                    "restriction".to_string(),
                    Value::Object(abuse_restriction_evidence_state(restriction)),
                );
            }
            let recovery: Vec<Value> = item
                .recovery_actions
                .iter()
                .map(|action| Value::from(action.as_str()))
                .collect();
            quota.insert("recoveryActions".to_string(), Value::Array(recovery));
            state.insert("quota".to_string(), Value::Object(quota));
            return state;
        }
    }
    if let Some(restriction) = &denial.restriction {
        state.insert(
            "restriction".to_string(),
            Value::Object(abuse_restriction_evidence_state(restriction)),
        );
    }
    state
}

fn period_evidence_state(period: &crate::types::UsagePeriodSummary) -> Value {
    let mut map = Map::new();
    map.insert(
        "periodStart".to_string(),
        serde_json::to_value(period.period_start).unwrap_or(Value::Null),
    );
    map.insert(
        "periodEnd".to_string(),
        serde_json::to_value(period.period_end).unwrap_or(Value::Null),
    );
    map.insert(
        "periodAnchor".to_string(),
        Value::from(period.period_anchor.clone()),
    );
    map.insert(
        "consumedAmount".to_string(),
        Value::from(period.consumed_amount),
    );
    map.insert(
        "reservedAmount".to_string(),
        Value::from(period.reserved_amount),
    );
    map.insert(
        "adjustedAmount".to_string(),
        Value::from(period.adjusted_amount),
    );
    map.insert(
        "carryoverApplied".to_string(),
        Value::from(period.carryover_applied),
    );
    map.insert(
        "remainingAmount".to_string(),
        Value::from(period.remaining_amount),
    );
    map.insert("overLimit".to_string(), Value::from(period.over_limit));
    Value::Object(map)
}

fn abuse_restriction_evidence_state(restriction: &AbuseRestrictionSummary) -> Map<String, Value> {
    let mut map = Map::new();
    map.insert(
        "restrictionId".to_string(),
        Value::from(restriction.restriction_id.clone()),
    );
    map.insert(
        "status".to_string(),
        Value::from(restriction.status.as_str()),
    );
    map.insert(
        "affectedCategory".to_string(),
        Value::from(restriction.affected_category.as_str()),
    );
    map.insert(
        "recoveryAction".to_string(),
        Value::from(restriction.recovery_action.as_str()),
    );
    map.insert(
        "visibleReasonCode".to_string(),
        Value::from(restriction.visible_reason_code.clone()),
    );
    map.insert(
        "sourceAuditRef".to_string(),
        Value::from(restriction.source_audit_ref.clone()),
    );
    map.insert(
        "supportContactAllowed".to_string(),
        Value::from(restriction.support_contact_allowed),
    );
    map.insert(
        "startedAt".to_string(),
        serde_json::to_value(restriction.started_at).unwrap_or(Value::Null),
    );
    map.insert(
        "expiresAt".to_string(),
        serde_json::to_value(restriction.expires_at).unwrap_or(Value::Null),
    );
    map
}

fn redact_evidence_value(path: &str, source: &Value) -> (Value, Vec<BillingEvidenceRedaction>) {
    match source {
        Value::Object(map) => {
            let mut out = Map::with_capacity(map.len());
            let mut redactions = Vec::new();
            for (key, value) in map {
                let child_path = format!("{path}.{key}");
                if should_redact_evidence_key(key) {
                    redactions.push(BillingEvidenceRedaction {
                        path: child_path,
                        reason: redaction_reason_for_key(key),
                        replacement: "[REDACTED]".to_string(),
                    });
                    continue;
                }
                let (clean, nested) = redact_evidence_value(&child_path, value);
                out.insert(key.clone(), clean);
                redactions.extend(nested);
            }
            (Value::Object(out), redactions)
        }
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            let mut redactions = Vec::new();
            for (index, value) in items.iter().enumerate() {
                let (clean, nested) = redact_evidence_value(&format!("{path}[{index}]"), value);
                out.push(clean);
                redactions.extend(nested);
            }
            (Value::Array(out), redactions)
        }
        other => (other.clone(), Vec::new()),
    }
}

fn should_redact_evidence_key(key: &str) -> bool {
    let normalized = key.to_lowercase();
    normalized.contains("secret")
        || normalized.contains("token")
        || normalized.contains("credential")
        || normalized.contains("connectorpayload")
        || normalized.contains("payload")
}

fn redaction_reason_for_key(key: &str) -> String {
    if key.to_lowercase().contains("payload") {
        "connector_payload".to_string()
    } else {
        "secret".to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::DateTime;
    use serde_json::json;

    use super::*;
    use crate::fixtures::FixtureRepo;
    use crate::fixtures::TEN_FINITE;
    use crate::types::AbuseRestrictionRecord;
    use crate::types::QuotaOverride;
    use crate::types::UsageCounter;
    use crate::types::UsageEvent;
    use crate::types::UsageEventKind;

    fn exhausted_denial() -> QuotaDenial {
        QuotaDenial {
            denial_id: "denial_quota".to_string(),
            tenant_id: TEN_FINITE.to_string(),
            category: Category::from(Category::RUN_LAUNCHES),
            operation_key: "tenant:ten_a:run:client_1".to_string(),
            reason_code: "quota_denied:run_launches_exhausted".to_string(),
            requested_amount: 1,
            remaining_amount: 0,
            guarded_entry_point: "POST /v1/runs".to_string(),
            created_at: Utc::now(),
            ..Default::default()
        }
    }

    #[test]
    fn stable_quota_denial_payloads() {
        let period = QuotaPeriod {
            quota_period_id: "period_1".to_string(),
            tenant_id: TEN_FINITE.to_string(),
            category: Category::from(Category::RUN_LAUNCHES),
            period_start: DateTime::from_naive_utc_and_offset(
                chrono::NaiveDate::from_ymd_opt(2026, 4, 1)
                    .unwrap()
                    .and_hms_opt(0, 0, 0)
                    .unwrap(),
                Utc,
            ),
            period_end: DateTime::from_naive_utc_and_offset(
                chrono::NaiveDate::from_ymd_opt(2026, 5, 1)
                    .unwrap()
                    .and_hms_opt(0, 0, 0)
                    .unwrap(),
                Utc,
            ),
            ..Default::default()
        };
        let denial = new_quota_exhausted_denial(
            TEN_FINITE,
            &Category::from(Category::RUN_LAUNCHES),
            "op_1",
            1,
            0,
            &period,
        );
        assert_eq!(denial.code, "quota_denied");
        assert_eq!(denial.reason_code, "quota_denied:run_launches_exhausted");
        assert!(!denial.message.is_empty());
        assert!(!denial.period_start.is_empty() && !denial.period_end.is_empty());

        let unavailable = new_quota_state_unavailable_denial(TEN_FINITE, "op_2");
        assert_eq!(unavailable.reason_code, REASON_QUOTA_STATE_UNAVAILABLE);
        assert!(unavailable.category.is_empty());
    }

    #[test]
    fn project_denial_detail_classifies_quota_abuse_unavailable_and_operator_states() {
        let cases: [(&str, QuotaDenial, &str, &str); 4] = [
            (
                "quota exhaustion",
                exhausted_denial(),
                DenialClassification::QUOTA_EXHAUSTION,
                RecoveryAction::WAIT,
            ),
            (
                "abuse restriction",
                QuotaDenial {
                    denial_id: "denial_abuse".to_string(),
                    tenant_id: TEN_FINITE.to_string(),
                    category: Category::from(Category::RUNTIME_TOOL_CALLS),
                    operation_key: "tenant:ten_a:tool_call:1".to_string(),
                    reason_code: "abuse_restriction:temporary".to_string(),
                    requested_amount: 1,
                    remaining_amount: 0,
                    guarded_entry_point: "tool call creation".to_string(),
                    created_at: Utc::now(),
                    ..Default::default()
                },
                DenialClassification::ABUSE_RESTRICTION,
                RecoveryAction::CONTACT_SUPPORT,
            ),
            (
                "quota state unavailable",
                QuotaDenial {
                    denial_id: "denial_unavailable".to_string(),
                    tenant_id: TEN_FINITE.to_string(),
                    operation_key: "tenant:ten_a:run:missing".to_string(),
                    reason_code: REASON_QUOTA_STATE_UNAVAILABLE.to_string(),
                    guarded_entry_point: "POST /v1/runs".to_string(),
                    created_at: Utc::now(),
                    ..Default::default()
                },
                DenialClassification::QUOTA_STATE_UNAVAILABLE,
                RecoveryAction::OPERATOR_RESOLUTION_REQUIRED,
            ),
            (
                "operator action needed",
                QuotaDenial {
                    denial_id: "denial_operator".to_string(),
                    tenant_id: TEN_FINITE.to_string(),
                    category: Category::from(Category::RUN_LAUNCHES),
                    operation_key: "tenant:ten_a:run:pending".to_string(),
                    reason_code: "quota_denied:operator_action_needed".to_string(),
                    requested_amount: 1,
                    remaining_amount: 0,
                    guarded_entry_point: "POST /v1/runs".to_string(),
                    created_at: Utc::now(),
                    ..Default::default()
                },
                DenialClassification::OPERATOR_ACTION_NEEDED,
                RecoveryAction::OPERATOR_RESOLUTION_REQUIRED,
            ),
        ];
        for (name, denial, want_class, want_action) in cases {
            let detail = project_denial_detail(&denial, None);
            assert_eq!(detail.classification, want_class, "{name}: {detail:?}");
            assert!(
                !detail.recovery_actions.is_empty() && detail.recovery_actions[0] == want_action,
                "{name}: expected first recovery action {want_action}, got {:?}",
                detail.recovery_actions
            );
            assert!(
                !detail.operation_ref.is_empty() && !detail.operation_key.is_empty(),
                "{name}"
            );
        }
    }

    #[test]
    fn project_denial_detail_maps_safe_operation_references_for_all_guarded_categories() {
        let cases: [(&str, &str); 7] = [
            (Category::RUN_LAUNCHES, "tenant:ten_a:run:client_1"),
            (
                Category::WORKFLOW_LAUNCHES,
                "tenant:ten_a:workflow:run_1:workflow_1",
            ),
            (
                Category::RUNTIME_TOOL_CALLS,
                "tenant:ten_a:tool_call:run_1:step_1:tool_1",
            ),
            (
                Category::LIVE_VALIDATION_ATTEMPTS,
                "tenant:ten_a:live_validation:validation_1",
            ),
            (
                Category::INTEGRATION_OPERATIONS,
                "tenant:ten_a:integration:calendar:operation_1",
            ),
            (
                Category::ARTIFACT_STORAGE_BYTES,
                "tenant:ten_a:artifact:artifact_1",
            ),
            (
                Category::REPLAY_EVALUATION_ATTEMPTS,
                "tenant:ten_a:evaluation:candidate_1:attempt_1",
            ),
        ];
        for (category, operation_key) in cases {
            let category = Category::from(category);
            let definition = definition_for(&category).unwrap();
            let detail = project_denial_detail(
                &QuotaDenial {
                    denial_id: format!("denial_{category}"),
                    tenant_id: TEN_FINITE.to_string(),
                    category: category.clone(),
                    operation_key: operation_key.to_string(),
                    reason_code: definition.denial_reason_code.clone(),
                    requested_amount: 1,
                    remaining_amount: 0,
                    guarded_entry_point: definition.reservation_rule.clone(),
                    created_at: Utc::now(),
                    ..Default::default()
                },
                None,
            );
            assert!(
                !detail.operation_ref.is_empty()
                    && detail.operation_ref != operation_key
                    && !detail.tenant_id.is_empty(),
                "expected tenant-safe operation ref for {category}: {detail:?}"
            );
            assert_eq!(
                detail.classification,
                DenialClassification::QUOTA_EXHAUSTION,
                "{category}"
            );
        }
    }

    #[test]
    fn build_evidence_export_redacts_sensitive_metadata() {
        let denial = project_denial_detail(&exhausted_denial(), None);
        let state = Map::from_iter([
            ("secret".to_string(), json!("sk-live")),
            ("connectorPayload".to_string(), json!({"token": "raw"})),
            ("events".to_string(), json!([{"accessToken": "raw-token"}])),
            ("safe".to_string(), json!("kept")),
        ]);
        let export = build_evidence_export(
            "prn_support",
            denial.clone(),
            vec![QuotaStatusItem {
                category: Category::from(Category::RUN_LAUNCHES),
                ..Default::default()
            }],
            state,
        );
        assert!(!export.schema_version.is_empty() && !export.export_id.is_empty());
        assert_eq!(export.denial.denial_id, denial.denial_id);
        assert!(
            export.redactions.len() >= 2,
            "expected secret and connector payload redaction records: {:?}",
            export.redactions
        );
        assert!(export.effective_limit_state.contains_key("safe"));
        assert!(
            !export.effective_limit_state.contains_key("secret"),
            "secret field was not redacted"
        );
        if let Some(Value::Array(events)) = export.effective_limit_state.get("events") {
            if let Some(Value::Object(first)) = events.first() {
                assert!(
                    !first.contains_key("accessToken"),
                    "nested token field was not redacted"
                );
            }
        }
    }

    #[test]
    fn build_evidence_export_supports_ordinary_and_abuse_restriction_denials() {
        let ordinary = project_denial_detail(&exhausted_denial(), None);
        let restriction = AbuseRestrictionSummary {
            restriction_id: "restriction_1".to_string(),
            status: AbuseRestrictionStatus::from(AbuseRestrictionStatus::ACTIVE),
            affected_category: Category::from(Category::RUNTIME_TOOL_CALLS),
            recovery_action: RecoveryAction::from(RecoveryAction::CONTACT_SUPPORT),
            visible_reason_code: "abuse_restriction:temporary".to_string(),
            source_audit_ref: "audit_abuse_1".to_string(),
            ..Default::default()
        };
        let abuse = project_denial_detail(
            &QuotaDenial {
                denial_id: "denial_abuse_export".to_string(),
                tenant_id: TEN_FINITE.to_string(),
                category: Category::from(Category::RUNTIME_TOOL_CALLS),
                operation_key: "tenant:ten_a:tool_call:1".to_string(),
                reason_code: "abuse_restriction:temporary".to_string(),
                requested_amount: 1,
                remaining_amount: 0,
                guarded_entry_point: "tool call creation".to_string(),
                created_at: Utc::now(),
                ..Default::default()
            },
            Some(restriction),
        );
        for denial in [ordinary, abuse] {
            let export = build_evidence_export(
                "prn_support",
                denial.clone(),
                vec![QuotaStatusItem {
                    category: denial.category.clone(),
                    ..Default::default()
                }],
                Map::from_iter([("safe".to_string(), json!("value"))]),
            );
            assert_eq!(export.tenant_id, TEN_FINITE);
            assert_eq!(export.denial.denial_id, denial.denial_id);
            assert_eq!(export.usage_snapshot.len(), 1);
            assert!(
                !export.redactions.is_empty(),
                "expected explicit redaction records for {}",
                denial.denial_id
            );
            if denial.classification == DenialClassification::ABUSE_RESTRICTION {
                assert!(
                    !export.audit_refs.is_empty(),
                    "expected abuse restriction audit ref"
                );
            }
        }
    }

    #[tokio::test]
    async fn denial_detail_hydrates_explicit_abuse_restriction_record() {
        let now = crate::projection::utc_ymd_hms(2026, 5, 7, 10, 0, 0);
        let expires_at = now + chrono::Duration::hours(1);
        let repo = Arc::new(FixtureRepo::new(now - chrono::Duration::hours(1)));
        repo.push_denial(QuotaDenial {
            denial_id: "denial_abuse_hydrate".to_string(),
            tenant_id: TEN_FINITE.to_string(),
            category: Category::from(Category::RUNTIME_TOOL_CALLS),
            operation_key: "tenant:ten_finite:tool_call:run_1:step_1:tool_1".to_string(),
            reason_code: "abuse_restriction:temporary".to_string(),
            requested_amount: 1,
            remaining_amount: 0,
            guarded_entry_point: "tool call creation".to_string(),
            created_at: now,
            ..Default::default()
        });
        repo.push_restriction(AbuseRestrictionRecord {
            restriction_id: "restriction_runtime".to_string(),
            tenant_id: TEN_FINITE.to_string(),
            status: AbuseRestrictionStatus::from(AbuseRestrictionStatus::ACTIVE),
            affected_category: Category::from(Category::RUNTIME_TOOL_CALLS),
            recovery_action: RecoveryAction::from(RecoveryAction::CONTACT_SUPPORT),
            visible_reason_code: "abuse_restriction:temporary".to_string(),
            source_audit_ref: "audit_runtime_restriction".to_string(),
            support_contact_allowed: true,
            started_at: now - chrono::Duration::minutes(1),
            expires_at: Some(expires_at),
            document: Some(Map::from_iter([(
                "detectionSignals".to_string(),
                json!("not visible"),
            )])),
        });
        let manager = Manager::with_clock(repo, move || now);

        let detail = manager
            .denial_detail(TEN_FINITE, "denial_abuse_hydrate")
            .await
            .unwrap()
            .expect("denial detail");
        let restriction = detail.restriction.as_ref().expect("hydrated restriction");
        assert_eq!(restriction.restriction_id, "restriction_runtime");
        assert_eq!(restriction.source_audit_ref, "audit_runtime_restriction");
        assert!(restriction.expires_at.is_some());
    }

    #[tokio::test]
    async fn evidence_export_includes_effective_limit_override_and_restriction_state() {
        let now = crate::projection::utc_ymd_hms(2026, 5, 7, 10, 0, 0);
        let expires_at = now + chrono::Duration::hours(1);
        let repo = Arc::new(FixtureRepo::new(now - chrono::Duration::hours(1)));
        let definition = definition_for(&Category::from(Category::RUNTIME_TOOL_CALLS)).unwrap();
        let period = repo.open_period_sync(TEN_FINITE, &definition, now);
        repo.set_override(QuotaOverride {
            quota_override_id: "override_runtime".to_string(),
            tenant_id: TEN_FINITE.to_string(),
            category: Category::from(Category::RUNTIME_TOOL_CALLS),
            limit: Some(3),
            reason: "temporary lowered limit".to_string(),
            effective_at: now - chrono::Duration::minutes(1),
            ..Default::default()
        });
        repo.save_counter(UsageCounter {
            tenant_id: TEN_FINITE.to_string(),
            category: Category::from(Category::RUNTIME_TOOL_CALLS),
            quota_period_id: period.quota_period_id.clone(),
            committed_amount: 3,
            updated_at: now,
            ..Default::default()
        });
        repo.push_denial(QuotaDenial {
            denial_id: "denial_export_state".to_string(),
            tenant_id: TEN_FINITE.to_string(),
            category: Category::from(Category::RUNTIME_TOOL_CALLS),
            quota_period_id: period.quota_period_id.clone(),
            operation_key: "tenant:ten_finite:tool_call:run_1:step_1:tool_1".to_string(),
            reason_code: "abuse_restriction:temporary".to_string(),
            requested_amount: 1,
            remaining_amount: 0,
            guarded_entry_point: "tool call creation".to_string(),
            created_at: now,
            ..Default::default()
        });
        repo.push_restriction(AbuseRestrictionRecord {
            restriction_id: "restriction_runtime".to_string(),
            tenant_id: TEN_FINITE.to_string(),
            status: AbuseRestrictionStatus::from(AbuseRestrictionStatus::ACTIVE),
            affected_category: Category::from(Category::RUNTIME_TOOL_CALLS),
            recovery_action: RecoveryAction::from(RecoveryAction::CONTACT_SUPPORT),
            visible_reason_code: "abuse_restriction:temporary".to_string(),
            source_audit_ref: "audit_runtime_restriction".to_string(),
            support_contact_allowed: true,
            started_at: now - chrono::Duration::minutes(1),
            expires_at: Some(expires_at),
            ..Default::default()
        });
        repo.push_event(UsageEvent {
            usage_event_id: "usage_event_denial".to_string(),
            tenant_id: TEN_FINITE.to_string(),
            category: Category::from(Category::RUNTIME_TOOL_CALLS),
            quota_period_id: period.quota_period_id.clone(),
            operation_key: "tenant:ten_finite:tool_call:run_1:step_1:tool_1".to_string(),
            event_kind: UsageEventKind::from(UsageEventKind::DENIAL),
            reason_code: "abuse_restriction:temporary".to_string(),
            created_at: now,
            ..Default::default()
        });
        let manager = Manager::with_clock(repo, move || now);

        let export = manager
            .evidence_export(TEN_FINITE, "denial_export_state", "prn_support", true)
            .await
            .unwrap()
            .expect("evidence export");
        let quota_state = export
            .effective_limit_state
            .get("quota")
            .and_then(Value::as_object)
            .expect("quota effective limit state");
        assert_eq!(
            quota_state.get("category").and_then(Value::as_str),
            Some(Category::RUNTIME_TOOL_CALLS)
        );
        for key in ["baseLimit", "effectiveLimit", "override", "restriction"] {
            assert!(
                quota_state.get(key).is_some_and(|value| !value.is_null()),
                "missing {key} in quota effective limit state: {quota_state:?}"
            );
        }
        assert!(
            export.audit_refs.len() >= 3,
            "expected denial, restriction, and usage refs, got {:?}",
            export.audit_refs
        );
    }
}
