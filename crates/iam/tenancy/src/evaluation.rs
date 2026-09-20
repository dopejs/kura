//! Tenant-aware accessor for the evaluation + regression-harness family:
//! evaluation_replay_candidates, evaluation_replay_attempts, evaluation_comparisons,
//! evaluation_regression_fixtures. Port of daemon/internal/store/tenancy/evaluation.go
//! (the evaluation-family portion).
//!
//! The live-validation family (UpsertLiveValidation*ForTenant &c.) and the
//! evaluation-product family (evaluation_product.go: discovery policies, campaigns,
//! product fixtures, dashboard projections) are NOT ported yet: they depend on
//! kura-store live-validation / evaluation-product CRUD methods that have not landed.
//! They should be wired here when the underlying store methods exist.

use crate::{TenancyError, emit_denial, require};

/// Tenant-aware accessor for the evaluation family.
pub struct Evaluation {
    store: crate::SQLiteStore,
    emitter: Option<kura_audit::Emitter>,
}

impl Evaluation {
    #[must_use]
    pub fn new(store: crate::SQLiteStore, emitter: Option<kura_audit::Emitter>) -> Self {
        Evaluation { store, emitter }
    }

    fn emit(&self, surface: &str, resource_kind: &str) {
        emit_denial(&self.emitter, surface, resource_kind);
    }

    fn bind_row(
        &self,
        table: &str,
        pk_column: &str,
        pk: &str,
        tenant_id: &str,
        surface: &str,
        resource_kind: &str,
    ) -> Result<(), TenancyError> {
        match self.store.bind_row_tenant(table, pk_column, pk, tenant_id) {
            Err(e) if crate::SQLiteStore::is_cross_tenant_row(&e) => {
                self.emit(surface, resource_kind);
                Err(TenancyError::CrossTenantWrite)
            }
            other => other.map_err(TenancyError::from),
        }
    }

    pub fn upsert_replay_candidate_for_tenant(
        &self,
        item: &kura_evaluation::ReplayCandidate,
    ) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        self.store
            .upsert_replay_candidate(item)
            .map_err(TenancyError::from)?;
        self.bind_row(
            "evaluation_replay_candidates",
            "candidate_id",
            &item.candidate_id,
            &tenant_id,
            "store:UpsertReplayCandidateForTenant",
            "evaluation_replay_candidate",
        )
    }

    pub fn upsert_replay_attempt_for_tenant(
        &self,
        item: &kura_evaluation::ReplayAttempt,
    ) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        self.store
            .upsert_replay_attempt(item)
            .map_err(TenancyError::from)?;
        self.bind_row(
            "evaluation_replay_attempts",
            "attempt_id",
            &item.attempt_id,
            &tenant_id,
            "store:UpsertReplayAttemptForTenant",
            "evaluation_replay_attempt",
        )
    }

    pub fn upsert_comparison_result_for_tenant(
        &self,
        item: &kura_evaluation::ComparisonResult,
    ) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        self.store
            .upsert_comparison_result(item)
            .map_err(TenancyError::from)?;
        self.bind_row(
            "evaluation_comparisons",
            "comparison_id",
            &item.comparison_id,
            &tenant_id,
            "store:UpsertComparisonResultForTenant",
            "evaluation_comparison",
        )
    }

    pub fn upsert_regression_fixture_for_tenant(
        &self,
        item: &kura_evaluation::RegressionFixture,
    ) -> Result<(), TenancyError> {
        let tenant_id = require()?;
        self.store
            .upsert_regression_fixture(item)
            .map_err(TenancyError::from)?;
        self.bind_row(
            "evaluation_regression_fixtures",
            "fixture_id",
            &item.fixture_id,
            &tenant_id,
            "store:UpsertRegressionFixtureForTenant",
            "evaluation_regression_fixture",
        )
    }
}
