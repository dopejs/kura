//! Roadmap 41 evaluation-product fixture (port of
//! daemon/internal/store/migrationfixture/r41_evaluation_product.go): seeds two
//! tenants with a full discovery → evidence → suppression → product-fixture →
//! campaign → inspection → retention chain in every Roadmap 41 table.

use std::collections::HashMap;

use rusqlite::params;

use kura_store::SQLiteStore;

use crate::FIXTURE_TIMESTAMP;
use crate::seeds::exec_insert;

/// Table names expected from the Roadmap 41 storage migration (migration v38).
pub static R41_EVALUATION_PRODUCT_TABLE_NAMES: [&str; 13] = [
    "evaluation_discovery_policies",
    "evaluation_discovery_runs",
    "evaluation_discovered_candidates",
    "evaluation_candidate_evidence",
    "evaluation_suppressions",
    "evaluation_product_fixtures",
    "evaluation_fixture_revisions",
    "evaluation_campaigns",
    "evaluation_campaign_items",
    "evaluation_campaign_attempt_groups",
    "evaluation_dashboard_projections",
    "evaluation_tool_call_inspections",
    "evaluation_retention_applications",
];

/// Minimum migration fixture coverage expected when Roadmap 41 tables are
/// implemented (Go R41EvaluationProductFixtureNotes).
pub static R41_EVALUATION_PRODUCT_FIXTURE_NOTES: [&str; 6] = [
    "seed at least two tenants to prove tenant-scoped indexes and accessors",
    "seed discovery policy, run, candidate, evidence, and suppression rows",
    "seed product fixture head plus immutable revisions",
    "seed campaigns with immutable source snapshots and attempt groups",
    "seed dashboard and inspection rows with retention-state variants",
    "seed redaction-failed and retention-applied audit/event examples",
];

/// Expected per-table row counts after seeding (Go R41EvaluationProductFixture).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct R41EvaluationProductFixture {
    pub tenant_ids: Vec<String>,
    pub expected_row_count: HashMap<String, i64>,
}

#[must_use]
pub fn build_r41_evaluation_product_fixture() -> R41EvaluationProductFixture {
    let mut counts = HashMap::new();
    for table in R41_EVALUATION_PRODUCT_TABLE_NAMES {
        counts.insert(table.to_string(), 2);
    }
    R41EvaluationProductFixture {
        tenant_ids: vec!["ten_eval_alpha".to_string(), "ten_eval_beta".to_string()],
        expected_row_count: counts,
    }
}

/// Seeds two tenants × every r41 table. Requires the store at head schema (v38+).
pub fn seed_r41_evaluation_product_rows(
    store: &SQLiteStore,
) -> Result<R41EvaluationProductFixture, String> {
    let fixture = build_r41_evaluation_product_fixture();
    let conn = crate::open_fixture_connection(store.db_path())?;
    let ts = FIXTURE_TIMESTAMP;

    for (index, tenant_id) in fixture.tenant_ids.iter().enumerate() {
        let suffix = (index + 1).to_string();
        let policy_id = format!("r41_policy_{suffix}");
        let run_id = format!("r41_discovery_run_{suffix}");
        let candidate_id = format!("r41_discovered_candidate_{suffix}");
        let evidence_id = format!("r41_evidence_{suffix}");
        let fixture_id = format!("r41_fixture_{suffix}");
        let revision_id = format!("r41_revision_{suffix}");
        let campaign_id = format!("r41_campaign_{suffix}");
        let item_id = format!("r41_campaign_item_{suffix}");
        let group_id = format!("r41_attempt_group_{suffix}");
        let projection_id = format!("r41_projection_{suffix}");
        let inspection_id = format!("r41_inspection_{suffix}");
        let retention_id = format!("r41_retention_{suffix}");

        exec_insert(
            &conn,
            "INSERT INTO evaluation_discovery_policies (policy_id, tenant_id, enabled, window_start, window_end, max_inspected_records, max_emitted_candidates, cost_budget, created_by, created_at, updated_at, document_json) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                policy_id,
                tenant_id,
                1i64,
                ts,
                ts,
                50i64,
                10i64,
                100i64,
                format!("prn_{suffix}"),
                ts,
                ts,
                "{\"redactionStatus\":\"clean\"}"
            ],
        )?;
        exec_insert(
            &conn,
            "INSERT INTO evaluation_discovery_runs (discovery_run_id, tenant_id, policy_id, status, cursor, window_start, window_end, max_inspected_records, max_emitted_candidates, cost_budget, inspected_records, emitted_candidates, started_by, started_at, completed_at, updated_at, idempotency_key, document_json) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                run_id,
                tenant_id,
                policy_id,
                "completed",
                format!("cursor_{suffix}"),
                ts,
                ts,
                50i64,
                10i64,
                100i64,
                12i64,
                1i64,
                format!("prn_{suffix}"),
                ts,
                ts,
                ts,
                format!("idem_{suffix}"),
                "{\"status\":\"completed\"}"
            ],
        )?;
        exec_insert(
            &conn,
            "INSERT INTO evaluation_discovered_candidates (discovered_candidate_id, tenant_id, discovery_run_id, source_kind, source_id, score, score_band, redaction_status, evidence_ref, readiness_status, suppression_state, retention_state, created_at, updated_at, expires_at, document_json) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                candidate_id,
                tenant_id,
                run_id,
                "run",
                "run_seed",
                0.92f64,
                "high",
                "redacted",
                evidence_id,
                "ready",
                "none",
                "active",
                ts,
                ts,
                None::<String>,
                "{\"evidence\":\"redacted\"}"
            ],
        )?;
        exec_insert(
            &conn,
            "INSERT INTO evaluation_candidate_evidence (evidence_id, tenant_id, discovered_candidate_id, redaction_status, materialization_allowed, retention_state, created_at, expires_at, document_json) VALUES (?,?,?,?,?,?,?,?,?)",
            params![
                evidence_id,
                tenant_id,
                candidate_id,
                "redacted",
                1i64,
                "active",
                ts,
                None::<String>,
                "{\"redactedPayload\":{\"token\":\"[REDACTED]\"}}"
            ],
        )?;
        exec_insert(
            &conn,
            "INSERT INTO evaluation_suppressions (suppression_id, tenant_id, target_kind, target_id, target_source_ref, reason_code, created_by, active, created_at, expires_at, document_json) VALUES (?,?,?,?,?,?,?,?,?,?,?)",
            params![
                format!("r41_suppression_{suffix}"),
                tenant_id,
                "discovered_candidate",
                candidate_id,
                None::<String>,
                "operator_hidden",
                format!("prn_{suffix}"),
                (index % 2) as i64,
                ts,
                None::<String>,
                "{\"active\":true}"
            ],
        )?;
        exec_insert(
            &conn,
            "INSERT INTO evaluation_product_fixtures (fixture_id, tenant_id, display_name, domain_class, source_kind, source_candidate_id, current_revision_id, review_state, suppression_state, retention_state, created_by, created_at, updated_at, document_json) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                fixture_id,
                tenant_id,
                format!("R41 fixture {suffix}"),
                "runtime",
                "run",
                candidate_id,
                revision_id,
                "approved",
                "none",
                "active",
                format!("prn_{suffix}"),
                ts,
                ts,
                "{\"displayName\":\"R41 fixture\"}"
            ],
        )?;
        exec_insert(
            &conn,
            "INSERT INTO evaluation_fixture_revisions (revision_id, fixture_id, tenant_id, revision_number, redaction_status, created_by, created_at, document_json) VALUES (?,?,?,?,?,?,?,?)",
            params![
                revision_id,
                fixture_id,
                tenant_id,
                1i64,
                "redacted",
                format!("prn_{suffix}"),
                ts,
                "{\"payload\":{\"secret\":\"[REDACTED]\"}}"
            ],
        )?;
        exec_insert(
            &conn,
            "INSERT INTO evaluation_campaigns (campaign_id, tenant_id, display_name, status, created_at, started_at, completed_at, published_at, retention_state, idempotency_key, document_json) VALUES (?,?,?,?,?,?,?,?,?,?,?)",
            params![
                campaign_id,
                tenant_id,
                format!("R41 campaign {suffix}"),
                "completed",
                ts,
                ts,
                ts,
                None::<String>,
                "active",
                format!("campaign_idem_{suffix}"),
                "{\"status\":\"completed\"}"
            ],
        )?;
        exec_insert(
            &conn,
            "INSERT INTO evaluation_campaign_items (campaign_item_id, campaign_id, tenant_id, source_type, source_id, suppression_checked_at, created_at, document_json) VALUES (?,?,?,?,?,?,?,?)",
            params![
                item_id,
                campaign_id,
                tenant_id,
                "product_fixture",
                fixture_id,
                ts,
                ts,
                "{\"sourceSnapshot\":{\"fixtureId\":\"redacted\"}}"
            ],
        )?;
        exec_insert(
            &conn,
            "INSERT INTO evaluation_campaign_attempt_groups (attempt_group_id, campaign_id, campaign_item_id, tenant_id, status, drift_count, failure_count, unsupported_count, operator_action_needed_count, created_at, updated_at, document_json) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                group_id,
                campaign_id,
                item_id,
                tenant_id,
                "completed",
                index as i64,
                0i64,
                0i64,
                0i64,
                ts,
                ts,
                "{\"summary\":\"ok\"}"
            ],
        )?;
        exec_insert(
            &conn,
            "INSERT INTO evaluation_dashboard_projections (projection_id, tenant_id, window_start, window_end, generated_at, cursor, document_json) VALUES (?,?,?,?,?,?,?)",
            params![
                projection_id,
                tenant_id,
                ts,
                ts,
                ts,
                format!("cursor_{suffix}"),
                "{\"campaignStatusCounts\":{\"completed\":1}}"
            ],
        )?;
        exec_insert(
            &conn,
            "INSERT INTO evaluation_tool_call_inspections (inspection_id, tenant_id, campaign_id, campaign_item_id, tool_call_ref, classification, redaction_status, created_at, updated_at, document_json) VALUES (?,?,?,?,?,?,?,?,?,?)",
            params![
                inspection_id,
                tenant_id,
                campaign_id,
                item_id,
                format!("tool_call_{suffix}"),
                "matched",
                "redacted",
                ts,
                ts,
                "{\"diffSummary\":\"redacted\"}"
            ],
        )?;
        exec_insert(
            &conn,
            "INSERT INTO evaluation_retention_applications (application_id, tenant_id, resource_kind, resource_id, dry_run, outcome, applied_at, document_json) VALUES (?,?,?,?,?,?,?,?)",
            params![
                retention_id,
                tenant_id,
                "campaign",
                campaign_id,
                (index % 2) as i64,
                "retained",
                ts,
                "{\"outcome\":\"retained\"}"
            ],
        )?;
    }
    Ok(fixture)
}

/// Counts rows per r41 table (Go CountR41EvaluationProductRows).
pub fn count_r41_evaluation_product_rows(
    store: &SQLiteStore,
) -> Result<HashMap<String, i64>, String> {
    let conn = crate::open_fixture_connection(store.db_path())?;
    let mut counts = HashMap::new();
    for table in R41_EVALUATION_PRODUCT_TABLE_NAMES {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .map_err(|e| format!("count {table}: {e}"))?;
        counts.insert(table.to_string(), count);
    }
    Ok(counts)
}
