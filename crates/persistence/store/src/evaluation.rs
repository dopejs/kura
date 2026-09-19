//! SQLite CRUD for the evaluation replay ledger: replay candidates, replay attempts,
//! comparison results, and regression fixtures. Ported from
//! `daemon/internal/store/store.go` (UpsertReplayCandidate, ListReplayCandidates,
//! GetReplayCandidate, UpsertReplayAttempt, ListReplayAttempts, GetReplayAttempt,
//! UpsertComparisonResult, ListComparisonResults, GetComparisonResult,
//! UpsertRegressionFixture, ListRegressionFixtures, GetRegressionFixture). The Go
//! daemon reads these tables back via `document_json` alone (no per-column scan
//! helpers), so this module does the same. The tenant column is written as NULL until
//! the tenancy package is ported; `document_json` holds the whole serialized domain
//! value, matching Go.

use rusqlite::{params, params_from_iter, types::Value};

use crate::SQLiteStore;
use crate::crud::{enum_str, now_rfc3339, null_string, opt_time_string};

impl SQLiteStore {
    pub fn upsert_replay_candidate(
        &self,
        item: &kura_evaluation::ReplayCandidate,
    ) -> Result<(), String> {
        let document_json =
            serde_json::to_string(item).map_err(|e| format!("marshal replay candidate: {e}"))?;
        self.conn
            .execute(
                r#"INSERT INTO evaluation_replay_candidates (
                    candidate_id, environment_scope, candidate_kind, source_kind, source_id,
                    readiness_status, latest_attempt_id, latest_comparison_id, created_at,
                    updated_at, document_json, tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                ON CONFLICT(candidate_id) DO UPDATE SET
                    environment_scope = excluded.environment_scope,
                    candidate_kind = excluded.candidate_kind,
                    source_kind = excluded.source_kind,
                    source_id = excluded.source_id,
                    readiness_status = excluded.readiness_status,
                    latest_attempt_id = excluded.latest_attempt_id,
                    latest_comparison_id = excluded.latest_comparison_id,
                    updated_at = excluded.updated_at,
                    document_json = excluded.document_json,
                    tenant_id = COALESCE(evaluation_replay_candidates.tenant_id, excluded.tenant_id)"#,
                params![
                    item.candidate_id,
                    item.environment_scope,
                    enum_str(&item.candidate_kind),
                    enum_str(&item.source_kind),
                    item.source_id,
                    enum_str(&item.readiness_status),
                    null_string(&item.latest_attempt_id),
                    null_string(&item.latest_comparison_id),
                    now_rfc3339(&item.created_at),
                    now_rfc3339(&item.updated_at),
                    document_json,
                    None::<String>,
                ],
            )
            .map_err(|e| format!("upsert replay candidate {}: {e}", item.candidate_id))?;
        Ok(())
    }

    pub fn list_replay_candidates(
        &self,
        filter: &kura_evaluation::CandidateFilter,
    ) -> Result<Vec<kura_evaluation::ReplayCandidate>, String> {
        let mut sql = String::from(
            r#"SELECT document_json
            FROM evaluation_replay_candidates
            WHERE 1 = 1"#,
        );
        let mut args: Vec<Value> = Vec::new();
        if !filter.environment_scope.trim().is_empty() {
            sql.push_str(" AND environment_scope = ?");
            args.push(Value::Text(filter.environment_scope.trim().to_string()));
        }
        if filter.candidate_kind != kura_evaluation::CandidateKind::default() {
            sql.push_str(" AND candidate_kind = ?");
            args.push(Value::Text(enum_str(&filter.candidate_kind)));
        }
        if filter.source_kind != kura_evaluation::SourceKind::default() {
            sql.push_str(" AND source_kind = ?");
            args.push(Value::Text(enum_str(&filter.source_kind)));
        }
        if filter.readiness_status != kura_evaluation::ReadinessStatus::default() {
            sql.push_str(" AND readiness_status = ?");
            args.push(Value::Text(enum_str(&filter.readiness_status)));
        }
        sql.push_str(" ORDER BY updated_at DESC, candidate_id DESC");
        if filter.limit > 0 {
            sql.push_str(" LIMIT ?");
            args.push(Value::Integer(filter.limit));
        }

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("list replay candidates: {e}"))?;
        let mut rows = stmt
            .query(params_from_iter(&args))
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let raw: String = row.get(0).map_err(|e| e.to_string())?;
            let item =
                serde_json::from_str(&raw).map_err(|e| format!("decode replay candidate: {e}"))?;
            items.push(item);
        }
        Ok(items)
    }

    pub fn get_replay_candidate(
        &self,
        environment_scope: &str,
        candidate_id: &str,
    ) -> Result<Option<kura_evaluation::ReplayCandidate>, String> {
        let mut sql = String::from(
            "SELECT document_json FROM evaluation_replay_candidates WHERE candidate_id = ?",
        );
        let mut args: Vec<Value> = vec![Value::Text(candidate_id.trim().to_string())];
        if !environment_scope.trim().is_empty() {
            sql.push_str(" AND environment_scope = ?");
            args.push(Value::Text(environment_scope.trim().to_string()));
        }
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("get replay candidate {candidate_id}: {e}"))?;
        let mut rows = stmt
            .query(params_from_iter(&args))
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let raw: String = row.get(0).map_err(|e| e.to_string())?;
        let item = serde_json::from_str(&raw)
            .map_err(|e| format!("decode replay candidate {candidate_id}: {e}"))?;
        Ok(Some(item))
    }

    pub fn upsert_replay_attempt(
        &self,
        item: &kura_evaluation::ReplayAttempt,
    ) -> Result<(), String> {
        let document_json =
            serde_json::to_string(item).map_err(|e| format!("marshal replay attempt: {e}"))?;
        self.conn
            .execute(
                r#"INSERT INTO evaluation_replay_attempts (
                    attempt_id, candidate_id, environment_scope, mode, status, change_window_label,
                    baseline_attempt_id, started_at, completed_at, created_at, updated_at,
                    document_json, tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                ON CONFLICT(attempt_id) DO UPDATE SET
                    candidate_id = excluded.candidate_id,
                    environment_scope = excluded.environment_scope,
                    mode = excluded.mode,
                    status = excluded.status,
                    change_window_label = excluded.change_window_label,
                    baseline_attempt_id = excluded.baseline_attempt_id,
                    started_at = excluded.started_at,
                    completed_at = excluded.completed_at,
                    updated_at = excluded.updated_at,
                    document_json = excluded.document_json,
                    tenant_id = COALESCE(evaluation_replay_attempts.tenant_id, excluded.tenant_id)"#,
                params![
                    item.attempt_id,
                    item.candidate_id,
                    item.environment_scope,
                    enum_str(&item.mode),
                    enum_str(&item.status),
                    null_string(&item.change_window_label),
                    null_string(&item.baseline_attempt_id),
                    opt_time_string(&item.started_at),
                    opt_time_string(&item.completed_at),
                    now_rfc3339(&item.created_at),
                    now_rfc3339(&item.updated_at),
                    document_json,
                    None::<String>,
                ],
            )
            .map_err(|e| format!("upsert replay attempt {}: {e}", item.attempt_id))?;
        Ok(())
    }

    pub fn list_replay_attempts(
        &self,
        filter: &kura_evaluation::AttemptFilter,
    ) -> Result<Vec<kura_evaluation::ReplayAttempt>, String> {
        let mut sql = String::from(
            r#"SELECT document_json
            FROM evaluation_replay_attempts
            WHERE 1 = 1"#,
        );
        let mut args: Vec<Value> = Vec::new();
        if !filter.environment_scope.trim().is_empty() {
            sql.push_str(" AND environment_scope = ?");
            args.push(Value::Text(filter.environment_scope.trim().to_string()));
        }
        if !filter.candidate_id.trim().is_empty() {
            sql.push_str(" AND candidate_id = ?");
            args.push(Value::Text(filter.candidate_id.trim().to_string()));
        }
        if filter.status != kura_evaluation::ReplayAttemptStatus::default() {
            sql.push_str(" AND status = ?");
            args.push(Value::Text(enum_str(&filter.status)));
        }
        sql.push_str(" ORDER BY created_at DESC, attempt_id DESC");
        if filter.limit > 0 {
            sql.push_str(" LIMIT ?");
            args.push(Value::Integer(filter.limit));
        }

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("list replay attempts: {e}"))?;
        let mut rows = stmt
            .query(params_from_iter(&args))
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let raw: String = row.get(0).map_err(|e| e.to_string())?;
            let item =
                serde_json::from_str(&raw).map_err(|e| format!("decode replay attempt: {e}"))?;
            items.push(item);
        }
        Ok(items)
    }

    pub fn get_replay_attempt(
        &self,
        environment_scope: &str,
        attempt_id: &str,
    ) -> Result<Option<kura_evaluation::ReplayAttempt>, String> {
        let mut sql = String::from(
            "SELECT document_json FROM evaluation_replay_attempts WHERE attempt_id = ?",
        );
        let mut args: Vec<Value> = vec![Value::Text(attempt_id.trim().to_string())];
        if !environment_scope.trim().is_empty() {
            sql.push_str(" AND environment_scope = ?");
            args.push(Value::Text(environment_scope.trim().to_string()));
        }
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("get replay attempt {attempt_id}: {e}"))?;
        let mut rows = stmt
            .query(params_from_iter(&args))
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let raw: String = row.get(0).map_err(|e| e.to_string())?;
        let item = serde_json::from_str(&raw)
            .map_err(|e| format!("decode replay attempt {attempt_id}: {e}"))?;
        Ok(Some(item))
    }

    pub fn upsert_comparison_result(
        &self,
        item: &kura_evaluation::ComparisonResult,
    ) -> Result<(), String> {
        let document_json =
            serde_json::to_string(item).map_err(|e| format!("marshal comparison result: {e}"))?;
        self.conn
            .execute(
                r#"INSERT INTO evaluation_comparisons (
                    comparison_id, candidate_id, attempt_id, environment_scope, terminal_status,
                    change_window_label, generated_at, document_json, tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                ON CONFLICT(comparison_id) DO UPDATE SET
                    candidate_id = excluded.candidate_id,
                    attempt_id = excluded.attempt_id,
                    environment_scope = excluded.environment_scope,
                    terminal_status = excluded.terminal_status,
                    change_window_label = excluded.change_window_label,
                    generated_at = excluded.generated_at,
                    document_json = excluded.document_json,
                    tenant_id = COALESCE(evaluation_comparisons.tenant_id, excluded.tenant_id)"#,
                params![
                    item.comparison_id,
                    item.candidate_id,
                    item.attempt_id,
                    item.environment_scope,
                    enum_str(&item.terminal_status),
                    null_string(&item.change_window_label),
                    now_rfc3339(&item.generated_at),
                    document_json,
                    None::<String>,
                ],
            )
            .map_err(|e| format!("upsert comparison {}: {e}", item.comparison_id))?;
        Ok(())
    }

    pub fn list_comparison_results(
        &self,
        filter: &kura_evaluation::ComparisonFilter,
    ) -> Result<Vec<kura_evaluation::ComparisonResult>, String> {
        let mut sql = String::from(
            r#"SELECT document_json
            FROM evaluation_comparisons
            WHERE 1 = 1"#,
        );
        let mut args: Vec<Value> = Vec::new();
        if !filter.environment_scope.trim().is_empty() {
            sql.push_str(" AND environment_scope = ?");
            args.push(Value::Text(filter.environment_scope.trim().to_string()));
        }
        if !filter.candidate_id.trim().is_empty() {
            sql.push_str(" AND candidate_id = ?");
            args.push(Value::Text(filter.candidate_id.trim().to_string()));
        }
        if !filter.attempt_id.trim().is_empty() {
            sql.push_str(" AND attempt_id = ?");
            args.push(Value::Text(filter.attempt_id.trim().to_string()));
        }
        if filter.terminal_status != kura_evaluation::ComparisonTerminalStatus::default() {
            sql.push_str(" AND terminal_status = ?");
            args.push(Value::Text(enum_str(&filter.terminal_status)));
        }
        sql.push_str(" ORDER BY generated_at DESC, comparison_id DESC");
        if filter.limit > 0 {
            sql.push_str(" LIMIT ?");
            args.push(Value::Integer(filter.limit));
        }

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("list comparisons: {e}"))?;
        let mut rows = stmt
            .query(params_from_iter(&args))
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let raw: String = row.get(0).map_err(|e| e.to_string())?;
            let item = serde_json::from_str(&raw).map_err(|e| format!("decode comparison: {e}"))?;
            items.push(item);
        }
        Ok(items)
    }

    pub fn get_comparison_result(
        &self,
        environment_scope: &str,
        comparison_id: &str,
    ) -> Result<Option<kura_evaluation::ComparisonResult>, String> {
        let mut sql = String::from(
            "SELECT document_json FROM evaluation_comparisons WHERE comparison_id = ?",
        );
        let mut args: Vec<Value> = vec![Value::Text(comparison_id.trim().to_string())];
        if !environment_scope.trim().is_empty() {
            sql.push_str(" AND environment_scope = ?");
            args.push(Value::Text(environment_scope.trim().to_string()));
        }
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("get comparison {comparison_id}: {e}"))?;
        let mut rows = stmt
            .query(params_from_iter(&args))
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let raw: String = row.get(0).map_err(|e| e.to_string())?;
        let item = serde_json::from_str(&raw)
            .map_err(|e| format!("decode comparison {comparison_id}: {e}"))?;
        Ok(Some(item))
    }

    pub fn upsert_regression_fixture(
        &self,
        item: &kura_evaluation::RegressionFixture,
    ) -> Result<(), String> {
        let document_json =
            serde_json::to_string(item).map_err(|e| format!("marshal regression fixture: {e}"))?;
        self.conn
            .execute(
                r#"INSERT INTO evaluation_regression_fixtures (
                    fixture_id, environment_scope, domain_class, candidate_id, manifest_path,
                    created_at, updated_at, document_json, tenant_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                ON CONFLICT(fixture_id) DO UPDATE SET
                    environment_scope = excluded.environment_scope,
                    domain_class = excluded.domain_class,
                    candidate_id = excluded.candidate_id,
                    manifest_path = excluded.manifest_path,
                    updated_at = excluded.updated_at,
                    document_json = excluded.document_json,
                    tenant_id = COALESCE(evaluation_regression_fixtures.tenant_id, excluded.tenant_id)"#,
                params![
                    item.fixture_id,
                    item.environment_scope,
                    enum_str(&item.domain_class),
                    null_string(&item.candidate_id),
                    item.manifest_path,
                    now_rfc3339(&item.created_at),
                    now_rfc3339(&item.updated_at),
                    document_json,
                    None::<String>,
                ],
            )
            .map_err(|e| format!("upsert regression fixture {}: {e}", item.fixture_id))?;
        Ok(())
    }

    pub fn list_regression_fixtures(
        &self,
        filter: &kura_evaluation::FixtureFilter,
    ) -> Result<Vec<kura_evaluation::RegressionFixture>, String> {
        let mut sql = String::from(
            r#"SELECT document_json
            FROM evaluation_regression_fixtures
            WHERE 1 = 1"#,
        );
        let mut args: Vec<Value> = Vec::new();
        if !filter.environment_scope.trim().is_empty() {
            sql.push_str(" AND environment_scope = ?");
            args.push(Value::Text(filter.environment_scope.trim().to_string()));
        }
        if filter.domain_class != kura_evaluation::FixtureDomainClass::default() {
            sql.push_str(" AND domain_class = ?");
            args.push(Value::Text(enum_str(&filter.domain_class)));
        }
        sql.push_str(" ORDER BY domain_class ASC, fixture_id ASC");
        if filter.limit > 0 {
            sql.push_str(" LIMIT ?");
            args.push(Value::Integer(filter.limit));
        }

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("list regression fixtures: {e}"))?;
        let mut rows = stmt
            .query(params_from_iter(&args))
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let raw: String = row.get(0).map_err(|e| e.to_string())?;
            let item = serde_json::from_str(&raw)
                .map_err(|e| format!("decode regression fixture: {e}"))?;
            items.push(item);
        }
        Ok(items)
    }

    pub fn get_regression_fixture(
        &self,
        environment_scope: &str,
        fixture_id: &str,
    ) -> Result<Option<kura_evaluation::RegressionFixture>, String> {
        let mut sql = String::from(
            "SELECT document_json FROM evaluation_regression_fixtures WHERE fixture_id = ?",
        );
        let mut args: Vec<Value> = vec![Value::Text(fixture_id.trim().to_string())];
        if !environment_scope.trim().is_empty() {
            sql.push_str(" AND environment_scope = ?");
            args.push(Value::Text(environment_scope.trim().to_string()));
        }
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("get regression fixture {fixture_id}: {e}"))?;
        let mut rows = stmt
            .query(params_from_iter(&args))
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let raw: String = row.get(0).map_err(|e| e.to_string())?;
        let item = serde_json::from_str(&raw)
            .map_err(|e| format!("decode regression fixture {fixture_id}: {e}"))?;
        Ok(Some(item))
    }
}

// --- kura_evaluation::Store trait impl (sync wrapper over the DAOs) ---
//
// rusqlite's Connection is Send but not Sync, so SQLiteStore cannot be the
// trait's `Send + Sync` self type directly. The workspace convention shares
// the store as `Arc<parking_lot::Mutex<SQLiteStore>>`; because the orphan
// rule forbids implementing an external trait for a foreign type, the mutex
// is wrapped in the local `EvaluationStoreHandle` newtype (see
// SecretStoreHandle / ComputerUseStoreHandle for the same pattern).

/// Send + Sync handle over the SQLite store implementing
/// [`kura_evaluation::Store`]. Construct from a fresh store and share as
/// `Arc<EvaluationStoreHandle>` with the evaluation manager.
pub struct EvaluationStoreHandle(pub parking_lot::Mutex<SQLiteStore>);

impl EvaluationStoreHandle {
    pub fn new(store: SQLiteStore) -> Self {
        Self(parking_lot::Mutex::new(store))
    }
}

impl kura_evaluation::Store for EvaluationStoreHandle {
    fn upsert_replay_candidate(
        &self,
        item: kura_evaluation::ReplayCandidate,
    ) -> Result<(), kura_evaluation::EvaluationError> {
        let store = self.0.lock();
        store
            .upsert_replay_candidate(&item)
            .map_err(kura_evaluation::EvaluationError::Store)
    }

    fn list_replay_candidates(
        &self,
        filter: &kura_evaluation::CandidateFilter,
    ) -> Result<Vec<kura_evaluation::ReplayCandidate>, kura_evaluation::EvaluationError> {
        let store = self.0.lock();
        store
            .list_replay_candidates(filter)
            .map_err(kura_evaluation::EvaluationError::Store)
    }

    fn get_replay_candidate(
        &self,
        environment_scope: &str,
        candidate_id: &str,
    ) -> Result<Option<kura_evaluation::ReplayCandidate>, kura_evaluation::EvaluationError> {
        let store = self.0.lock();
        store
            .get_replay_candidate(environment_scope, candidate_id)
            .map_err(kura_evaluation::EvaluationError::Store)
    }

    fn upsert_replay_attempt(
        &self,
        item: kura_evaluation::ReplayAttempt,
    ) -> Result<(), kura_evaluation::EvaluationError> {
        let store = self.0.lock();
        store
            .upsert_replay_attempt(&item)
            .map_err(kura_evaluation::EvaluationError::Store)
    }

    fn list_replay_attempts(
        &self,
        filter: &kura_evaluation::AttemptFilter,
    ) -> Result<Vec<kura_evaluation::ReplayAttempt>, kura_evaluation::EvaluationError> {
        let store = self.0.lock();
        store
            .list_replay_attempts(filter)
            .map_err(kura_evaluation::EvaluationError::Store)
    }

    fn get_replay_attempt(
        &self,
        environment_scope: &str,
        attempt_id: &str,
    ) -> Result<Option<kura_evaluation::ReplayAttempt>, kura_evaluation::EvaluationError> {
        let store = self.0.lock();
        store
            .get_replay_attempt(environment_scope, attempt_id)
            .map_err(kura_evaluation::EvaluationError::Store)
    }

    fn upsert_comparison_result(
        &self,
        item: kura_evaluation::ComparisonResult,
    ) -> Result<(), kura_evaluation::EvaluationError> {
        let store = self.0.lock();
        store
            .upsert_comparison_result(&item)
            .map_err(kura_evaluation::EvaluationError::Store)
    }

    fn list_comparison_results(
        &self,
        filter: &kura_evaluation::ComparisonFilter,
    ) -> Result<Vec<kura_evaluation::ComparisonResult>, kura_evaluation::EvaluationError> {
        let store = self.0.lock();
        store
            .list_comparison_results(filter)
            .map_err(kura_evaluation::EvaluationError::Store)
    }

    fn get_comparison_result(
        &self,
        environment_scope: &str,
        comparison_id: &str,
    ) -> Result<Option<kura_evaluation::ComparisonResult>, kura_evaluation::EvaluationError> {
        let store = self.0.lock();
        store
            .get_comparison_result(environment_scope, comparison_id)
            .map_err(kura_evaluation::EvaluationError::Store)
    }

    fn upsert_regression_fixture(
        &self,
        item: kura_evaluation::RegressionFixture,
    ) -> Result<(), kura_evaluation::EvaluationError> {
        let store = self.0.lock();
        store
            .upsert_regression_fixture(&item)
            .map_err(kura_evaluation::EvaluationError::Store)
    }

    fn list_regression_fixtures(
        &self,
        filter: &kura_evaluation::FixtureFilter,
    ) -> Result<Vec<kura_evaluation::RegressionFixture>, kura_evaluation::EvaluationError> {
        let store = self.0.lock();
        store
            .list_regression_fixtures(filter)
            .map_err(kura_evaluation::EvaluationError::Store)
    }
}
