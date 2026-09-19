//! SQLite CRUD for the tenant-scoped evaluation product domain. Ported from
//! `daemon/internal/store/evaluation_product.go` (UpsertDiscoveryPolicy,
//! ListDiscoveryPolicies, GetDiscoveryPolicy, SaveDiscoveryRun,
//! ListDiscoveryRuns, GetDiscoveryRun, SaveDiscoveredCandidate,
//! ListDiscoveredCandidates, GetDiscoveredCandidate,
//! GetLatestCandidateEvidence, CreateSuppression, ApplyRetention,
//! UpsertProductFixture, ListProductFixtures, GetProductFixture,
//! SaveFixtureRevision, ListFixtureRevisions, SaveReplayCampaign,
//! ListReplayCampaigns, GetReplayCampaign, SaveCampaignItem,
//! ListCampaignItems, SaveCampaignAttemptGroup, ListCampaignAttemptGroups,
//! SaveDashboardProjection, ListDashboardProjections,
//! GetDashboardProjection, SaveToolCallInspection,
//! ListToolCallInspections, GetToolCallInspection). Rows persist as
//! `document_json` plus denormalized tenant columns; the tenant is resolved
//! through the default-personal binding when the caller does not name one.

use chrono::{DateTime, Utc};
use rusqlite::{params, params_from_iter, types::Value};

use crate::SQLiteStore;
use crate::crud::{now_rfc3339, null_string};

fn is_unset_time(dt: &DateTime<Utc>) -> bool {
    dt.timestamp() == 0 && dt.timestamp_subsec_nanos() == 0
}

fn format_product_time(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
}

fn nullable_time_string(value: &Option<DateTime<Utc>>) -> Option<String> {
    value
        .as_ref()
        .filter(|t| !is_unset_time(t))
        .map(|t| format_product_time(*t))
}

fn bool_to_int(value: bool) -> i64 {
    if value { 1 } else { 0 }
}

/// Go `candidateEvidenceRedactionStatus`.
fn candidate_evidence_redaction_status(
    evidence: &kura_evaluation::CandidateEvidence,
) -> kura_evaluation::RedactionStatus {
    if !evidence.redaction_rules_applied.is_empty()
        || !evidence.sensitive_fields_excluded.is_empty()
    {
        kura_evaluation::RedactionStatus::Redacted
    } else {
        kura_evaluation::RedactionStatus::Clean
    }
}

/// Go `productSourceKindsContain`.
fn product_source_kinds_contain(
    values: &[kura_evaluation::SourceKind],
    target: &kura_evaluation::SourceKind,
) -> bool {
    values.iter().any(|v| v == target)
}

impl SQLiteStore {
    /// Resolves the tenant for a product write: explicit tenant wins, else the
    /// default-personal binding; validates the tenant-scoped request contract.
    pub fn evaluation_product_tenant_id(&self, explicit: &str) -> Result<String, String> {
        let tenant_id = if !explicit.trim().is_empty() {
            explicit.trim().to_string()
        } else {
            self.resolve_default_tenant_binding().unwrap_or_default()
        };
        kura_evaluation::validate_tenant_scoped_product_request(&tenant_id)
            .map_err(|e| e.to_string())?;
        Ok(tenant_id)
    }

    pub fn upsert_discovery_policy(
        &self,
        mut item: kura_evaluation::DiscoveryPolicy,
    ) -> Result<(), String> {
        let tenant_id = self.evaluation_product_tenant_id(&item.tenant_id)?;
        item.tenant_id = tenant_id.clone();
        kura_evaluation::validate_discovery_policy(&item).map_err(|e| e.to_string())?;
        let document_json = serde_json::to_string(&item)
            .map_err(|e| format!("marshal evaluation discovery policy: {e}"))?;
        self.conn
            .execute(
                r#"INSERT INTO evaluation_discovery_policies (
                    policy_id, tenant_id, enabled, window_start, window_end, max_inspected_records,
                    max_emitted_candidates, cost_budget, created_by, created_at, updated_at, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                ON CONFLICT(policy_id) DO UPDATE SET
                    tenant_id = COALESCE(evaluation_discovery_policies.tenant_id, excluded.tenant_id),
                    enabled = excluded.enabled,
                    window_start = excluded.window_start,
                    window_end = excluded.window_end,
                    max_inspected_records = excluded.max_inspected_records,
                    max_emitted_candidates = excluded.max_emitted_candidates,
                    cost_budget = excluded.cost_budget,
                    created_by = excluded.created_by,
                    updated_at = excluded.updated_at,
                    document_json = excluded.document_json"#,
                params![
                    item.policy_id,
                    tenant_id,
                    bool_to_int(item.enabled),
                    format_product_time(item.window_start),
                    format_product_time(item.window_end),
                    item.max_inspected_records,
                    item.max_emitted_candidates,
                    item.cost_budget,
                    null_string(&item.created_by),
                    format_product_time(item.created_at),
                    format_product_time(item.updated_at),
                    document_json,
                ],
            )
            .map_err(|e| format!("upsert evaluation discovery policy {}: {e}", item.policy_id))?;
        Ok(())
    }

    pub fn list_discovery_policies(
        &self,
        filter: &kura_evaluation::DiscoveryPolicyFilter,
    ) -> Result<Vec<kura_evaluation::DiscoveryPolicy>, String> {
        let tenant_id = self.evaluation_product_tenant_id(&filter.base.tenant_id)?;
        let mut query = String::from(
            "SELECT document_json FROM evaluation_discovery_policies WHERE tenant_id = ?",
        );
        let mut args: Vec<Value> = vec![tenant_id.clone().into()];
        if let Some(enabled) = filter.enabled {
            query.push_str(" AND enabled = ?");
            args.push(bool_to_int(enabled).into());
        }
        if !filter.base.cursor.is_empty() {
            query.push_str(" AND policy_id < ?");
            args.push(filter.base.cursor.clone().into());
        }
        query.push_str(" ORDER BY updated_at DESC, policy_id DESC LIMIT ?");
        args.push(kura_evaluation::normalize_product_limit(filter.base.limit).into());
        self.scan_evaluation_product_documents::<kura_evaluation::DiscoveryPolicy>(
            &query,
            &args,
            "discovery policies",
        )
    }

    pub fn get_discovery_policy(
        &self,
        tenant_id: &str,
        policy_id: &str,
    ) -> Result<Option<kura_evaluation::DiscoveryPolicy>, String> {
        self.get_evaluation_product_document::<kura_evaluation::DiscoveryPolicy>(
            "evaluation_discovery_policies",
            "policy_id",
            tenant_id,
            policy_id,
            "discovery policy",
        )
    }

    pub fn save_discovery_run(
        &self,
        mut item: kura_evaluation::DiscoveryRun,
    ) -> Result<(), String> {
        let tenant_id = self.evaluation_product_tenant_id(&item.tenant_id)?;
        item.tenant_id = tenant_id.clone();
        let document_json = serde_json::to_string(&item)
            .map_err(|e| format!("marshal evaluation discovery run: {e}"))?;
        self.conn
            .execute(
                r#"INSERT INTO evaluation_discovery_runs (
                    discovery_run_id, tenant_id, policy_id, status, cursor, window_start, window_end,
                    max_inspected_records, max_emitted_candidates, cost_budget, inspected_records,
                    emitted_candidates, started_by, started_at, completed_at, updated_at,
                    idempotency_key, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
                ON CONFLICT(discovery_run_id) DO UPDATE SET
                    tenant_id = COALESCE(evaluation_discovery_runs.tenant_id, excluded.tenant_id),
                    policy_id = excluded.policy_id,
                    status = excluded.status,
                    cursor = excluded.cursor,
                    inspected_records = excluded.inspected_records,
                    emitted_candidates = excluded.emitted_candidates,
                    completed_at = excluded.completed_at,
                    updated_at = excluded.updated_at,
                    document_json = excluded.document_json"#,
                params![
                    item.discovery_run_id,
                    tenant_id,
                    null_string(&item.policy_id),
                    item.status.as_str(),
                    null_string(&item.cursor),
                    format_product_time(item.window_start),
                    format_product_time(item.window_end),
                    item.max_inspected_records,
                    item.max_emitted_candidates,
                    item.cost_budget,
                    item.inspected_records,
                    item.emitted_candidates,
                    null_string(&item.started_by),
                    format_product_time(item.started_at),
                    nullable_time_string(&item.completed_at),
                    format_product_time(item.updated_at),
                    null_string(&item.idempotency_key),
                    document_json,
                ],
            )
            .map_err(|e| format!("save evaluation discovery run {}: {e}", item.discovery_run_id))?;
        Ok(())
    }

    pub fn list_discovery_runs(
        &self,
        filter: &kura_evaluation::DiscoveryRunFilter,
    ) -> Result<Vec<kura_evaluation::DiscoveryRun>, String> {
        let tenant_id = self.evaluation_product_tenant_id(&filter.base.tenant_id)?;
        let mut query =
            String::from("SELECT document_json FROM evaluation_discovery_runs WHERE tenant_id = ?");
        let mut args: Vec<Value> = vec![tenant_id.clone().into()];
        if !filter.status.as_str().is_empty() {
            query.push_str(" AND status = ?");
            args.push(filter.status.as_str().to_string().into());
        }
        if !filter.base.cursor.is_empty() {
            query.push_str(" AND discovery_run_id < ?");
            args.push(filter.base.cursor.clone().into());
        }
        query.push_str(" ORDER BY updated_at DESC, discovery_run_id DESC LIMIT ?");
        args.push(kura_evaluation::normalize_product_limit(filter.base.limit).into());
        let items = self.scan_evaluation_product_documents::<kura_evaluation::DiscoveryRun>(
            &query,
            &args,
            "discovery runs",
        )?;
        if filter.source_kind.as_str().is_empty() {
            return Ok(items);
        }
        Ok(items
            .into_iter()
            .filter(|item| product_source_kinds_contain(&item.source_kinds, &filter.source_kind))
            .collect())
    }

    pub fn get_discovery_run(
        &self,
        tenant_id: &str,
        discovery_run_id: &str,
    ) -> Result<Option<kura_evaluation::DiscoveryRun>, String> {
        self.get_evaluation_product_document::<kura_evaluation::DiscoveryRun>(
            "evaluation_discovery_runs",
            "discovery_run_id",
            tenant_id,
            discovery_run_id,
            "discovery run",
        )
    }

    /// Go `SaveDiscoveredCandidate`: candidate + optional evidence, transactional.
    pub fn save_discovered_candidate(
        &self,
        mut item: kura_evaluation::DiscoveredCandidate,
        mut evidence: kura_evaluation::CandidateEvidence,
    ) -> Result<(), String> {
        let tenant_id = self.evaluation_product_tenant_id(&item.tenant_id)?;
        item.tenant_id = tenant_id.clone();
        evidence.tenant_id = tenant_id.clone();
        if evidence.discovered_candidate_id.is_empty() {
            evidence.discovered_candidate_id = item.discovered_candidate_id.clone();
        }
        if !evidence.evidence_id.is_empty() && item.evidence_ref.is_empty() {
            item.evidence_ref = evidence.evidence_id.clone();
        }
        let candidate_json = serde_json::to_string(&item)
            .map_err(|e| format!("marshal evaluation discovered candidate: {e}"))?;
        let evidence_json = serde_json::to_string(&evidence)
            .map_err(|e| format!("marshal evaluation candidate evidence: {e}"))?;
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin save evaluation discovered candidate: {e}"))?;
        tx.execute(
            r#"INSERT INTO evaluation_discovered_candidates (
                discovered_candidate_id, tenant_id, discovery_run_id, source_kind, source_id,
                score, score_band, redaction_status, evidence_ref, readiness_status,
                suppression_state, retention_state, created_at, updated_at, expires_at, document_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
            ON CONFLICT(discovered_candidate_id) DO UPDATE SET
                tenant_id = COALESCE(evaluation_discovered_candidates.tenant_id, excluded.tenant_id),
                discovery_run_id = excluded.discovery_run_id,
                source_kind = excluded.source_kind,
                source_id = excluded.source_id,
                score = excluded.score,
                score_band = excluded.score_band,
                redaction_status = excluded.redaction_status,
                evidence_ref = excluded.evidence_ref,
                readiness_status = excluded.readiness_status,
                suppression_state = excluded.suppression_state,
                retention_state = excluded.retention_state,
                updated_at = excluded.updated_at,
                expires_at = excluded.expires_at,
                document_json = excluded.document_json"#,
            params![
                item.discovered_candidate_id,
                tenant_id,
                item.discovery_run_id,
                item.source_kind.as_str(),
                item.source_id,
                item.score,
                item.score_band.as_str(),
                item.redaction_status.as_str(),
                null_string(&item.evidence_ref),
                item.readiness_status.as_str(),
                item.suppression_state.as_str(),
                item.retention_state.as_str(),
                format_product_time(item.created_at),
                format_product_time(item.updated_at),
                nullable_time_string(&item.expires_at),
                candidate_json,
            ],
        )
        .map_err(|e| format!("save evaluation discovered candidate {}: {e}", item.discovered_candidate_id))?;
        if !evidence.evidence_id.is_empty() {
            tx.execute(
                r#"INSERT INTO evaluation_candidate_evidence (
                    evidence_id, tenant_id, discovered_candidate_id, redaction_status,
                    materialization_allowed, retention_state, created_at, expires_at, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                ON CONFLICT(evidence_id) DO UPDATE SET
                    tenant_id = COALESCE(evaluation_candidate_evidence.tenant_id, excluded.tenant_id),
                    discovered_candidate_id = excluded.discovered_candidate_id,
                    redaction_status = excluded.redaction_status,
                    materialization_allowed = excluded.materialization_allowed,
                    retention_state = excluded.retention_state,
                    expires_at = excluded.expires_at,
                    document_json = excluded.document_json"#,
                params![
                    evidence.evidence_id,
                    tenant_id,
                    evidence.discovered_candidate_id,
                    candidate_evidence_redaction_status(&evidence).as_str(),
                    bool_to_int(evidence.materialization_allowed),
                    evidence.retention_state.as_str(),
                    format_product_time(evidence.created_at),
                    nullable_time_string(&evidence.expires_at),
                    evidence_json,
                ],
            )
            .map_err(|e| format!("save evaluation candidate evidence {}: {e}", evidence.evidence_id))?;
        }
        tx.commit().map_err(|e| {
            format!(
                "commit save evaluation discovered candidate {}: {e}",
                item.discovered_candidate_id
            )
        })?;
        Ok(())
    }

    pub fn list_discovered_candidates(
        &self,
        filter: &kura_evaluation::DiscoveredCandidateFilter,
    ) -> Result<Vec<kura_evaluation::DiscoveredCandidate>, String> {
        let tenant_id = self.evaluation_product_tenant_id(&filter.base.tenant_id)?;
        let mut query = String::from(
            "SELECT document_json FROM evaluation_discovered_candidates WHERE tenant_id = ?",
        );
        let mut args: Vec<Value> = vec![tenant_id.clone().into()];
        if !filter.discovery_run_id.is_empty() {
            query.push_str(" AND discovery_run_id = ?");
            args.push(filter.discovery_run_id.clone().into());
        }
        if !filter.source_kind.as_str().is_empty() {
            query.push_str(" AND source_kind = ?");
            args.push(filter.source_kind.as_str().to_string().into());
        }
        if !filter.readiness_status.as_str().is_empty() {
            query.push_str(" AND readiness_status = ?");
            args.push(filter.readiness_status.as_str().to_string().into());
        }
        if !filter.suppression_state.as_str().is_empty() {
            query.push_str(" AND suppression_state = ?");
            args.push(filter.suppression_state.as_str().to_string().into());
        }
        if !filter.score_band.as_str().is_empty() {
            query.push_str(" AND score_band = ?");
            args.push(filter.score_band.as_str().to_string().into());
        }
        if !filter.base.cursor.is_empty() {
            query.push_str(" AND discovered_candidate_id < ?");
            args.push(filter.base.cursor.clone().into());
        }
        query.push_str(" ORDER BY updated_at DESC, discovered_candidate_id DESC LIMIT ?");
        args.push(kura_evaluation::normalize_product_limit(filter.base.limit).into());
        self.scan_evaluation_product_documents::<kura_evaluation::DiscoveredCandidate>(
            &query,
            &args,
            "discovered candidates",
        )
    }

    pub fn get_discovered_candidate(
        &self,
        tenant_id: &str,
        discovered_candidate_id: &str,
    ) -> Result<Option<kura_evaluation::DiscoveredCandidate>, String> {
        self.get_evaluation_product_document::<kura_evaluation::DiscoveredCandidate>(
            "evaluation_discovered_candidates",
            "discovered_candidate_id",
            tenant_id,
            discovered_candidate_id,
            "discovered candidate",
        )
    }

    pub fn get_latest_candidate_evidence(
        &self,
        tenant_id: &str,
        discovered_candidate_id: &str,
    ) -> Result<Option<kura_evaluation::CandidateEvidence>, String> {
        let tenant_id = self.evaluation_product_tenant_id(tenant_id)?;
        let mut stmt = self
            .conn
            .prepare(
                "SELECT document_json FROM evaluation_candidate_evidence
                 WHERE tenant_id = ?1 AND discovered_candidate_id = ?2
                 ORDER BY created_at DESC, evidence_id DESC LIMIT 1",
            )
            .map_err(|e| format!("get latest evaluation candidate evidence: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id, discovered_candidate_id])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let raw: String = row.get(0).map_err(|e| e.to_string())?;
        let item: kura_evaluation::CandidateEvidence = serde_json::from_str(&raw)
            .map_err(|e| format!("decode latest evaluation candidate evidence: {e}"))?;
        Ok(Some(item))
    }

    /// Go `CreateSuppression`.
    pub fn create_suppression(
        &self,
        mut item: kura_evaluation::SuppressionRecord,
    ) -> Result<(), String> {
        let tenant_id = self.evaluation_product_tenant_id(&item.tenant_id)?;
        item.tenant_id = tenant_id.clone();
        let document_json = serde_json::to_string(&item)
            .map_err(|e| format!("marshal evaluation suppression: {e}"))?;
        self.conn
            .execute(
                r#"INSERT INTO evaluation_suppressions (
                    suppression_id, tenant_id, target_kind, target_id, target_source_ref,
                    reason_code, created_by, active, created_at, expires_at, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                ON CONFLICT(suppression_id) DO UPDATE SET
                    tenant_id = COALESCE(evaluation_suppressions.tenant_id, excluded.tenant_id),
                    target_kind = excluded.target_kind,
                    target_id = excluded.target_id,
                    target_source_ref = excluded.target_source_ref,
                    reason_code = excluded.reason_code,
                    created_by = excluded.created_by,
                    active = excluded.active,
                    expires_at = excluded.expires_at,
                    document_json = excluded.document_json"#,
                params![
                    item.suppression_id,
                    tenant_id,
                    item.target_kind.as_str(),
                    null_string(&item.target_id),
                    null_string(&item.target_source_ref),
                    item.reason_code,
                    null_string(&item.created_by),
                    bool_to_int(item.active),
                    format_product_time(item.created_at),
                    nullable_time_string(&item.expires_at),
                    document_json,
                ],
            )
            .map_err(|e| format!("create evaluation suppression {}: {e}", item.suppression_id))?;
        Ok(())
    }

    /// Go `ApplyRetention`: expires matching product rows (dry-run records only
    /// when dry_run is set). Returns the recorded application ids.
    pub fn apply_retention(
        &self,
        filter: &kura_evaluation::RetentionApplicationFilter,
    ) -> Result<Vec<String>, String> {
        let tenant_id = self.evaluation_product_tenant_id(&filter.base.tenant_id)?;
        let kinds: Vec<kura_evaluation::ProductResourceKind> = if filter.resource_kinds.is_empty() {
            vec![
                kura_evaluation::ProductResourceKind::DiscoveredCandidate,
                kura_evaluation::ProductResourceKind::CandidateEvidence,
                kura_evaluation::ProductResourceKind::ProductFixture,
                kura_evaluation::ProductResourceKind::Campaign,
                kura_evaluation::ProductResourceKind::DashboardProjection,
                kura_evaluation::ProductResourceKind::ToolCallInspection,
            ]
        } else {
            filter.resource_kinds.clone()
        };
        let now = Utc::now();
        let mut application_ids = Vec::new();
        for kind in &kinds {
            let application_id = format!(
                "retention_{}_{}",
                now.timestamp_nanos_opt()
                    .unwrap_or_else(|| now.timestamp() * 1_000_000_000),
                kind.as_str().replace('_', ""),
            );
            let outcome = if filter.dry_run {
                "dry_run".to_string()
            } else {
                "expired".to_string()
            };
            if !filter.dry_run {
                self.apply_product_retention_kind(&tenant_id, kind, now)?;
            }
            let document = serde_json::json!({
                "applicationId": application_id,
                "tenantId": tenant_id,
                "resourceKind": kind.as_str(),
                "dryRun": filter.dry_run,
                "outcome": outcome,
                "appliedAt": now_rfc3339(&now),
            })
            .to_string();
            self.conn
                .execute(
                    r#"INSERT INTO evaluation_retention_applications (
                        application_id, tenant_id, resource_kind, resource_id, dry_run,
                        outcome, applied_at, document_json
                    ) VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, ?7)"#,
                    params![
                        application_id,
                        tenant_id,
                        kind.as_str(),
                        bool_to_int(filter.dry_run),
                        outcome,
                        format_product_time(now),
                        document,
                    ],
                )
                .map_err(|e| {
                    format!(
                        "record evaluation retention application {}: {e}",
                        kind.as_str()
                    )
                })?;
            application_ids.push(application_id);
        }
        Ok(application_ids)
    }

    pub fn upsert_product_fixture(
        &self,
        mut item: kura_evaluation::ProductManagedFixture,
    ) -> Result<(), String> {
        let tenant_id = self.evaluation_product_tenant_id(&item.tenant_id)?;
        item.tenant_id = tenant_id.clone();
        let document_json = serde_json::to_string(&item)
            .map_err(|e| format!("marshal evaluation product fixture: {e}"))?;
        self.conn
            .execute(
                r#"INSERT INTO evaluation_product_fixtures (
                    fixture_id, tenant_id, display_name, domain_class, source_kind, source_candidate_id,
                    current_revision_id, review_state, suppression_state, retention_state, created_by,
                    created_at, updated_at, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                ON CONFLICT(fixture_id) DO UPDATE SET
                    tenant_id = COALESCE(evaluation_product_fixtures.tenant_id, excluded.tenant_id),
                    display_name = excluded.display_name,
                    domain_class = excluded.domain_class,
                    source_kind = excluded.source_kind,
                    source_candidate_id = excluded.source_candidate_id,
                    current_revision_id = excluded.current_revision_id,
                    review_state = excluded.review_state,
                    suppression_state = excluded.suppression_state,
                    retention_state = excluded.retention_state,
                    updated_at = excluded.updated_at,
                    document_json = excluded.document_json"#,
                params![
                    item.fixture_id,
                    tenant_id,
                    item.display_name,
                    item.domain_class.as_str(),
                    null_string(&item.source_kind),
                    null_string(&item.source_candidate_id),
                    null_string(&item.current_revision_id),
                    item.review_state.as_str(),
                    item.suppression_state.as_str(),
                    item.retention_state.as_str(),
                    null_string(&item.created_by),
                    format_product_time(item.created_at),
                    format_product_time(item.updated_at),
                    document_json,
                ],
            )
            .map_err(|e| format!("upsert evaluation product fixture {}: {e}", item.fixture_id))?;
        Ok(())
    }

    pub fn list_product_fixtures(
        &self,
        filter: &kura_evaluation::ProductListFilter,
    ) -> Result<Vec<kura_evaluation::ProductManagedFixture>, String> {
        let tenant_id = self.evaluation_product_tenant_id(&filter.tenant_id)?;
        let mut query = String::from(
            "SELECT document_json FROM evaluation_product_fixtures WHERE tenant_id = ?",
        );
        let mut args: Vec<Value> = vec![tenant_id.clone().into()];
        if !filter.cursor.is_empty() {
            query.push_str(" AND fixture_id < ?");
            args.push(filter.cursor.clone().into());
        }
        query.push_str(" ORDER BY updated_at DESC, fixture_id DESC LIMIT ?");
        args.push(kura_evaluation::normalize_product_limit(filter.limit).into());
        self.scan_evaluation_product_documents::<kura_evaluation::ProductManagedFixture>(
            &query,
            &args,
            "product fixtures",
        )
    }

    pub fn get_product_fixture(
        &self,
        tenant_id: &str,
        fixture_id: &str,
    ) -> Result<Option<kura_evaluation::ProductManagedFixture>, String> {
        self.get_evaluation_product_document::<kura_evaluation::ProductManagedFixture>(
            "evaluation_product_fixtures",
            "fixture_id",
            tenant_id,
            fixture_id,
            "product fixture",
        )
    }

    pub fn save_fixture_revision(
        &self,
        mut item: kura_evaluation::FixtureRevision,
    ) -> Result<(), String> {
        let tenant_id = self.evaluation_product_tenant_id(&item.tenant_id)?;
        item.tenant_id = tenant_id.clone();
        let document_json = serde_json::to_string(&item)
            .map_err(|e| format!("marshal evaluation fixture revision: {e}"))?;
        self.conn
            .execute(
                r#"INSERT INTO evaluation_fixture_revisions (
                    revision_id, fixture_id, tenant_id, revision_number, redaction_status,
                    created_by, created_at, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                ON CONFLICT(revision_id) DO UPDATE SET document_json = evaluation_fixture_revisions.document_json"#,
                params![
                    item.revision_id,
                    item.fixture_id,
                    tenant_id,
                    item.revision_number,
                    item.redaction_status.as_str(),
                    null_string(&item.created_by),
                    format_product_time(item.created_at),
                    document_json,
                ],
            )
            .map_err(|e| format!("save evaluation fixture revision {}: {e}", item.revision_id))?;
        Ok(())
    }

    pub fn list_fixture_revisions(
        &self,
        tenant_id: &str,
        fixture_id: &str,
        limit: i64,
    ) -> Result<Vec<kura_evaluation::FixtureRevision>, String> {
        let tenant_id = self.evaluation_product_tenant_id(tenant_id)?;
        let query = String::from(
            "SELECT document_json FROM evaluation_fixture_revisions
             WHERE tenant_id = ? AND fixture_id = ?
             ORDER BY revision_number DESC, revision_id DESC LIMIT ?",
        );
        let args: Vec<Value> = vec![
            tenant_id.into(),
            fixture_id.to_string().into(),
            kura_evaluation::normalize_product_limit(limit).into(),
        ];
        self.scan_evaluation_product_documents::<kura_evaluation::FixtureRevision>(
            &query,
            &args,
            "fixture revisions",
        )
    }

    pub fn save_replay_campaign(
        &self,
        mut item: kura_evaluation::ReplayCampaign,
    ) -> Result<(), String> {
        let tenant_id = self.evaluation_product_tenant_id(&item.tenant_id)?;
        item.tenant_id = tenant_id.clone();
        let document_json = serde_json::to_string(&item)
            .map_err(|e| format!("marshal evaluation replay campaign: {e}"))?;
        self.conn
            .execute(
                r#"INSERT INTO evaluation_campaigns (
                    campaign_id, tenant_id, display_name, status, created_at, started_at,
                    completed_at, published_at, retention_state, idempotency_key, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                ON CONFLICT(campaign_id) DO UPDATE SET
                    tenant_id = COALESCE(evaluation_campaigns.tenant_id, excluded.tenant_id),
                    display_name = excluded.display_name,
                    status = excluded.status,
                    started_at = excluded.started_at,
                    completed_at = excluded.completed_at,
                    published_at = excluded.published_at,
                    retention_state = excluded.retention_state,
                    document_json = excluded.document_json"#,
                params![
                    item.campaign_id,
                    tenant_id,
                    item.display_name,
                    item.status.as_str(),
                    format_product_time(item.created_at),
                    nullable_time_string(&item.started_at),
                    nullable_time_string(&item.completed_at),
                    nullable_time_string(&item.published_at),
                    item.retention_state.as_str(),
                    null_string(&item.idempotency_key),
                    document_json,
                ],
            )
            .map_err(|e| format!("save evaluation replay campaign {}: {e}", item.campaign_id))?;
        Ok(())
    }

    pub fn list_replay_campaigns(
        &self,
        filter: &kura_evaluation::ProductListFilter,
    ) -> Result<Vec<kura_evaluation::ReplayCampaign>, String> {
        let tenant_id = self.evaluation_product_tenant_id(&filter.tenant_id)?;
        let mut query =
            String::from("SELECT document_json FROM evaluation_campaigns WHERE tenant_id = ?");
        let mut args: Vec<Value> = vec![tenant_id.clone().into()];
        if !filter.cursor.is_empty() {
            query.push_str(" AND campaign_id < ?");
            args.push(filter.cursor.clone().into());
        }
        query.push_str(" ORDER BY created_at DESC, campaign_id DESC LIMIT ?");
        args.push(kura_evaluation::normalize_product_limit(filter.limit).into());
        self.scan_evaluation_product_documents::<kura_evaluation::ReplayCampaign>(
            &query,
            &args,
            "replay campaigns",
        )
    }

    pub fn get_replay_campaign(
        &self,
        tenant_id: &str,
        campaign_id: &str,
    ) -> Result<Option<kura_evaluation::ReplayCampaign>, String> {
        self.get_evaluation_product_document::<kura_evaluation::ReplayCampaign>(
            "evaluation_campaigns",
            "campaign_id",
            tenant_id,
            campaign_id,
            "replay campaign",
        )
    }

    pub fn save_campaign_item(
        &self,
        mut item: kura_evaluation::CampaignItem,
    ) -> Result<(), String> {
        let tenant_id = self.evaluation_product_tenant_id(&item.tenant_id)?;
        item.tenant_id = tenant_id.clone();
        let document_json = serde_json::to_string(&item)
            .map_err(|e| format!("marshal evaluation campaign item: {e}"))?;
        self.conn
            .execute(
                r#"INSERT INTO evaluation_campaign_items (
                    campaign_item_id, campaign_id, tenant_id, source_type, source_id,
                    suppression_checked_at, created_at, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                ON CONFLICT(campaign_item_id) DO UPDATE SET document_json = excluded.document_json"#,
                params![
                    item.campaign_item_id,
                    item.campaign_id,
                    tenant_id,
                    item.source_type.as_str(),
                    item.source_id,
                    format_product_time(item.suppression_checked_at),
                    format_product_time(item.created_at),
                    document_json,
                ],
            )
            .map_err(|e| format!("save evaluation campaign item {}: {e}", item.campaign_item_id))?;
        Ok(())
    }

    pub fn list_campaign_items(
        &self,
        filter: &kura_evaluation::ProductListFilter,
        campaign_id: &str,
    ) -> Result<Vec<kura_evaluation::CampaignItem>, String> {
        let tenant_id = self.evaluation_product_tenant_id(&filter.tenant_id)?;
        let mut query =
            String::from("SELECT document_json FROM evaluation_campaign_items WHERE tenant_id = ?");
        let mut args: Vec<Value> = vec![tenant_id.clone().into()];
        if !campaign_id.is_empty() {
            query.push_str(" AND campaign_id = ?");
            args.push(campaign_id.to_string().into());
        }
        if !filter.cursor.is_empty() {
            query.push_str(" AND campaign_item_id < ?");
            args.push(filter.cursor.clone().into());
        }
        query.push_str(" ORDER BY created_at DESC, campaign_item_id DESC LIMIT ?");
        args.push(kura_evaluation::normalize_product_limit(filter.limit).into());
        self.scan_evaluation_product_documents::<kura_evaluation::CampaignItem>(
            &query,
            &args,
            "campaign items",
        )
    }

    pub fn save_campaign_attempt_group(
        &self,
        mut item: kura_evaluation::CampaignAttemptGroup,
    ) -> Result<(), String> {
        let tenant_id = self.evaluation_product_tenant_id(&item.tenant_id)?;
        item.tenant_id = tenant_id.clone();
        let document_json = serde_json::to_string(&item)
            .map_err(|e| format!("marshal evaluation campaign attempt group: {e}"))?;
        self.conn
            .execute(
                r#"INSERT INTO evaluation_campaign_attempt_groups (
                    attempt_group_id, campaign_id, campaign_item_id, tenant_id, status, drift_count,
                    failure_count, unsupported_count, operator_action_needed_count, created_at,
                    updated_at, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                ON CONFLICT(attempt_group_id) DO UPDATE SET
                    status = excluded.status,
                    drift_count = excluded.drift_count,
                    failure_count = excluded.failure_count,
                    unsupported_count = excluded.unsupported_count,
                    operator_action_needed_count = excluded.operator_action_needed_count,
                    updated_at = excluded.updated_at,
                    document_json = excluded.document_json"#,
                params![
                    item.attempt_group_id,
                    item.campaign_id,
                    item.campaign_item_id,
                    tenant_id,
                    item.status.as_str(),
                    item.drift_count,
                    item.failure_count,
                    item.unsupported_count,
                    item.operator_action_needed_count,
                    format_product_time(item.created_at),
                    format_product_time(item.updated_at),
                    document_json,
                ],
            )
            .map_err(|e| {
                format!(
                    "save evaluation campaign attempt group {}: {e}",
                    item.attempt_group_id
                )
            })?;
        Ok(())
    }

    pub fn list_campaign_attempt_groups(
        &self,
        filter: &kura_evaluation::ProductListFilter,
        campaign_id: &str,
    ) -> Result<Vec<kura_evaluation::CampaignAttemptGroup>, String> {
        let tenant_id = self.evaluation_product_tenant_id(&filter.tenant_id)?;
        let mut query = String::from(
            "SELECT document_json FROM evaluation_campaign_attempt_groups WHERE tenant_id = ?",
        );
        let mut args: Vec<Value> = vec![tenant_id.clone().into()];
        if !campaign_id.is_empty() {
            query.push_str(" AND campaign_id = ?");
            args.push(campaign_id.to_string().into());
        }
        if !filter.cursor.is_empty() {
            query.push_str(" AND attempt_group_id < ?");
            args.push(filter.cursor.clone().into());
        }
        query.push_str(" ORDER BY updated_at DESC, attempt_group_id DESC LIMIT ?");
        args.push(kura_evaluation::normalize_product_limit(filter.limit).into());
        self.scan_evaluation_product_documents::<kura_evaluation::CampaignAttemptGroup>(
            &query,
            &args,
            "campaign attempt groups",
        )
    }

    pub fn save_dashboard_projection(
        &self,
        mut item: kura_evaluation::DashboardProjection,
    ) -> Result<(), String> {
        let tenant_id = self.evaluation_product_tenant_id(&item.tenant_id)?;
        item.tenant_id = tenant_id.clone();
        if item.retention_state.as_str().is_empty() {
            item.retention_state = kura_evaluation::RetentionState::Active;
        }
        let document_json = serde_json::to_string(&item)
            .map_err(|e| format!("marshal evaluation dashboard projection: {e}"))?;
        self.conn
            .execute(
                r#"INSERT INTO evaluation_dashboard_projections (
                    projection_id, tenant_id, window_start, window_end, generated_at, cursor,
                    retention_state, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                ON CONFLICT(projection_id) DO UPDATE SET
                    window_start = excluded.window_start,
                    window_end = excluded.window_end,
                    generated_at = excluded.generated_at,
                    cursor = excluded.cursor,
                    retention_state = excluded.retention_state,
                    document_json = excluded.document_json"#,
                params![
                    item.projection_id,
                    tenant_id,
                    format_product_time(item.window_start),
                    format_product_time(item.window_end),
                    format_product_time(item.generated_at),
                    null_string(&item.cursor),
                    item.retention_state.as_str(),
                    document_json,
                ],
            )
            .map_err(|e| {
                format!(
                    "save evaluation dashboard projection {}: {e}",
                    item.projection_id
                )
            })?;
        Ok(())
    }

    pub fn list_dashboard_projections(
        &self,
        filter: &kura_evaluation::ProductListFilter,
    ) -> Result<Vec<kura_evaluation::DashboardProjection>, String> {
        let tenant_id = self.evaluation_product_tenant_id(&filter.tenant_id)?;
        let mut query = String::from(
            "SELECT document_json FROM evaluation_dashboard_projections WHERE tenant_id = ? AND retention_state = ?",
        );
        let mut args: Vec<Value> = vec![
            tenant_id.clone().into(),
            kura_evaluation::RetentionState::Active
                .as_str()
                .to_string()
                .into(),
        ];
        if !filter.cursor.is_empty() {
            query.push_str(" AND projection_id < ?");
            args.push(filter.cursor.clone().into());
        }
        query.push_str(" ORDER BY generated_at DESC, projection_id DESC LIMIT ?");
        args.push(kura_evaluation::normalize_product_limit(filter.limit).into());
        self.scan_evaluation_product_documents::<kura_evaluation::DashboardProjection>(
            &query,
            &args,
            "dashboard projections",
        )
    }

    pub fn get_dashboard_projection(
        &self,
        tenant_id: &str,
        projection_id: &str,
    ) -> Result<Option<kura_evaluation::DashboardProjection>, String> {
        self.get_evaluation_product_document::<kura_evaluation::DashboardProjection>(
            "evaluation_dashboard_projections",
            "projection_id",
            tenant_id,
            projection_id,
            "dashboard projection",
        )
    }

    pub fn save_tool_call_inspection(
        &self,
        mut item: kura_evaluation::ToolCallInspection,
    ) -> Result<(), String> {
        let tenant_id = self.evaluation_product_tenant_id(&item.tenant_id)?;
        item.tenant_id = tenant_id.clone();
        if item.retention_state.as_str().is_empty() {
            item.retention_state = kura_evaluation::RetentionState::Active;
        }
        let document_json = serde_json::to_string(&item)
            .map_err(|e| format!("marshal evaluation tool-call inspection: {e}"))?;
        self.conn
            .execute(
                r#"INSERT INTO evaluation_tool_call_inspections (
                    inspection_id, tenant_id, campaign_id, campaign_item_id, tool_call_ref,
                    classification, redaction_status, retention_state, created_at, updated_at, document_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                ON CONFLICT(inspection_id) DO UPDATE SET
                    classification = excluded.classification,
                    redaction_status = excluded.redaction_status,
                    retention_state = excluded.retention_state,
                    updated_at = excluded.updated_at,
                    document_json = excluded.document_json"#,
                params![
                    item.inspection_id,
                    tenant_id,
                    item.campaign_id,
                    item.campaign_item_id,
                    item.tool_call_ref,
                    item.classification,
                    item.redaction_status.as_str(),
                    item.retention_state.as_str(),
                    format_product_time(item.created_at),
                    format_product_time(item.updated_at),
                    document_json,
                ],
            )
            .map_err(|e| format!("save evaluation tool-call inspection {}: {e}", item.inspection_id))?;
        Ok(())
    }

    pub fn list_tool_call_inspections(
        &self,
        filter: &kura_evaluation::ProductListFilter,
        campaign_id: &str,
    ) -> Result<Vec<kura_evaluation::ToolCallInspection>, String> {
        let tenant_id = self.evaluation_product_tenant_id(&filter.tenant_id)?;
        let mut query = String::from(
            "SELECT document_json FROM evaluation_tool_call_inspections WHERE tenant_id = ? AND retention_state = ?",
        );
        let mut args: Vec<Value> = vec![
            tenant_id.clone().into(),
            kura_evaluation::RetentionState::Active
                .as_str()
                .to_string()
                .into(),
        ];
        if !campaign_id.is_empty() {
            query.push_str(" AND campaign_id = ?");
            args.push(campaign_id.to_string().into());
        }
        if !filter.cursor.is_empty() {
            query.push_str(" AND inspection_id < ?");
            args.push(filter.cursor.clone().into());
        }
        query.push_str(" ORDER BY updated_at DESC, inspection_id DESC LIMIT ?");
        args.push(kura_evaluation::normalize_product_limit(filter.limit).into());
        self.scan_evaluation_product_documents::<kura_evaluation::ToolCallInspection>(
            &query,
            &args,
            "tool-call inspections",
        )
    }

    pub fn get_tool_call_inspection(
        &self,
        tenant_id: &str,
        inspection_id: &str,
    ) -> Result<Option<kura_evaluation::ToolCallInspection>, String> {
        self.get_evaluation_product_document::<kura_evaluation::ToolCallInspection>(
            "evaluation_tool_call_inspections",
            "inspection_id",
            tenant_id,
            inspection_id,
            "tool-call inspection",
        )
    }

    // --- internal helpers ---

    fn apply_product_retention_kind(
        &self,
        tenant_id: &str,
        kind: &kura_evaluation::ProductResourceKind,
        now: DateTime<Utc>,
    ) -> Result<(), String> {
        match kind {
            kura_evaluation::ProductResourceKind::DiscoveredCandidate => {
                self.update_evaluation_product_retention::<kura_evaluation::DiscoveredCandidate>(
                    "evaluation_discovered_candidates",
                    "discovered_candidate_id",
                    tenant_id,
                    now,
                    RetentionColumns {
                        expires_at: true,
                        updated_at: true,
                    },
                    |item| {
                        item.retention_state = kura_evaluation::RetentionState::Expired;
                        item.updated_at = now;
                        item.expires_at = Some(now);
                    },
                )
            }
            kura_evaluation::ProductResourceKind::CandidateEvidence => {
                self.update_evaluation_product_retention::<kura_evaluation::CandidateEvidence>(
                    "evaluation_candidate_evidence",
                    "evidence_id",
                    tenant_id,
                    now,
                    RetentionColumns {
                        expires_at: true,
                        updated_at: false,
                    },
                    |item| {
                        item.retention_state = kura_evaluation::RetentionState::Expired;
                        item.expires_at = Some(now);
                    },
                )
            }
            kura_evaluation::ProductResourceKind::ProductFixture => {
                self.update_evaluation_product_retention::<kura_evaluation::ProductManagedFixture>(
                    "evaluation_product_fixtures",
                    "fixture_id",
                    tenant_id,
                    now,
                    RetentionColumns {
                        expires_at: false,
                        updated_at: true,
                    },
                    |item| {
                        item.retention_state = kura_evaluation::RetentionState::Expired;
                        item.updated_at = now;
                    },
                )
            }
            kura_evaluation::ProductResourceKind::Campaign => self
                .update_evaluation_product_retention::<kura_evaluation::ReplayCampaign>(
                    "evaluation_campaigns",
                    "campaign_id",
                    tenant_id,
                    now,
                    RetentionColumns {
                        expires_at: false,
                        updated_at: false,
                    },
                    |item| {
                        item.retention_state = kura_evaluation::RetentionState::Expired;
                    },
                ),
            kura_evaluation::ProductResourceKind::DashboardProjection => {
                self.update_evaluation_product_retention::<kura_evaluation::DashboardProjection>(
                    "evaluation_dashboard_projections",
                    "projection_id",
                    tenant_id,
                    now,
                    RetentionColumns {
                        expires_at: false,
                        updated_at: false,
                    },
                    |item| {
                        item.retention_state = kura_evaluation::RetentionState::Expired;
                    },
                )
            }
            kura_evaluation::ProductResourceKind::ToolCallInspection => {
                self.update_evaluation_product_retention::<kura_evaluation::ToolCallInspection>(
                    "evaluation_tool_call_inspections",
                    "inspection_id",
                    tenant_id,
                    now,
                    RetentionColumns {
                        expires_at: false,
                        updated_at: true,
                    },
                    |item| {
                        item.retention_state = kura_evaluation::RetentionState::Expired;
                        item.updated_at = now;
                    },
                )
            }
            _ => Ok(()),
        }
    }

    fn update_evaluation_product_retention<T>(
        &self,
        table: &str,
        id_column: &str,
        tenant_id: &str,
        now: DateTime<Utc>,
        columns: RetentionColumns,
        mutate: impl Fn(&mut T),
    ) -> Result<(), String>
    where
        T: serde::de::DeserializeOwned + serde::Serialize,
    {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {id_column}, document_json FROM {table} WHERE tenant_id = ?1"
            ))
            .map_err(|e| format!("load evaluation product retention rows for {table}: {e}"))?;
        let mut rows = stmt.query(params![tenant_id]).map_err(|e| e.to_string())?;
        let mut loaded: Vec<(String, String)> = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let id: String = row.get(0).map_err(|e| e.to_string())?;
            let doc: String = row.get(1).map_err(|e| e.to_string())?;
            loaded.push((id, doc));
        }
        drop(rows);
        for (id, doc) in loaded {
            let mut item: T = serde_json::from_str(&doc)
                .map_err(|e| format!("decode retention row {table}/{id}: {e}"))?;
            mutate(&mut item);
            let encoded = serde_json::to_string(&item)
                .map_err(|e| format!("marshal retention row {table}/{id}: {e}"))?;
            let mut assignments = vec!["retention_state = ?".to_string()];
            let mut args: Vec<Value> = vec![
                kura_evaluation::RetentionState::Expired
                    .as_str()
                    .to_string()
                    .into(),
            ];
            if columns.expires_at {
                assignments.push("expires_at = COALESCE(expires_at, ?)".to_string());
                args.push(format_product_time(now).into());
            }
            if columns.updated_at {
                assignments.push("updated_at = ?".to_string());
                args.push(format_product_time(now).into());
            }
            assignments.push("document_json = ?".to_string());
            args.push(encoded.into());
            args.push(tenant_id.to_string().into());
            args.push(id.clone().into());
            self.conn
                .execute(
                    &format!(
                        "UPDATE {table} SET {} WHERE tenant_id = ? AND {id_column} = ?",
                        assignments.join(", ")
                    ),
                    params_from_iter(args.iter()),
                )
                .map_err(|e| format!("update retention row {table}/{id}: {e}"))?;
        }
        Ok(())
    }

    fn scan_evaluation_product_documents<T>(
        &self,
        query: &str,
        args: &[Value],
        label: &str,
    ) -> Result<Vec<T>, String>
    where
        T: serde::de::DeserializeOwned,
    {
        let mut stmt = self
            .conn
            .prepare(query)
            .map_err(|e| format!("list evaluation product {label}: {e}"))?;
        let mut rows = stmt
            .query(params_from_iter(args.iter()))
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let raw: String = row.get(0).map_err(|e| e.to_string())?;
            let item: T = serde_json::from_str(&raw)
                .map_err(|e| format!("decode evaluation product {label}: {e}"))?;
            items.push(item);
        }
        Ok(items)
    }

    fn get_evaluation_product_document<T>(
        &self,
        table: &str,
        id_column: &str,
        tenant_id: &str,
        id: &str,
        label: &str,
    ) -> Result<Option<T>, String>
    where
        T: serde::de::DeserializeOwned,
    {
        if tenant_id.trim().is_empty() || id.trim().is_empty() {
            return Ok(None);
        }
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT document_json FROM {table} WHERE tenant_id = ?1 AND {id_column} = ?2"
            ))
            .map_err(|e| format!("get evaluation product {label} {id}: {e}"))?;
        let mut rows = stmt
            .query(params![tenant_id, id])
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let raw: String = row.get(0).map_err(|e| e.to_string())?;
        let item: T = serde_json::from_str(&raw)
            .map_err(|e| format!("decode evaluation product {label} {id}: {e}"))?;
        Ok(Some(item))
    }
}

#[derive(Debug, Clone, Copy)]
struct RetentionColumns {
    expires_at: bool,
    updated_at: bool,
}
