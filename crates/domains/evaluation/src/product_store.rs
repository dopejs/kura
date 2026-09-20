//! Port of `daemon/internal/evaluation/product_store.go`: store interfaces
//! and filters for the tenant-scoped evaluation product resources.

use crate::types::{
    CandidateEvidence, DiscoveredCandidate, DiscoveryPolicy, DiscoveryRun, ProductLifecycleStatus,
    ProductResourceKind, ReadinessStatus, ScoreBand, SourceKind, SuppressionRecord,
    SuppressionState,
};

/// Go `ProductListFilter`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProductListFilter {
    pub tenant_id: String,
    pub cursor: String,
    pub limit: i64,
}

/// Go `DiscoveryPolicyFilter`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiscoveryPolicyFilter {
    pub base: ProductListFilter,
    pub enabled: Option<bool>,
}

/// Go `DiscoveryRunFilter`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiscoveryRunFilter {
    pub base: ProductListFilter,
    pub status: ProductLifecycleStatus,
    pub source_kind: SourceKind,
}

/// Go `DiscoveredCandidateFilter`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiscoveredCandidateFilter {
    pub base: ProductListFilter,
    pub discovery_run_id: String,
    pub source_kind: SourceKind,
    pub readiness_status: ReadinessStatus,
    pub suppression_state: SuppressionState,
    pub score_band: ScoreBand,
}

/// Go `RetentionApplicationFilter`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RetentionApplicationFilter {
    pub base: ProductListFilter,
    pub resource_kinds: Vec<ProductResourceKind>,
    pub dry_run: bool,
}

/// Go `ProductStore` interface.
pub trait ProductStore: Send + Sync {
    fn upsert_discovery_policy(&self, policy: DiscoveryPolicy) -> Result<(), String>;
    fn list_discovery_policies(
        &self,
        filter: &DiscoveryPolicyFilter,
    ) -> Result<Vec<DiscoveryPolicy>, String>;
    fn save_discovery_run(&self, run: DiscoveryRun) -> Result<(), String>;
    fn list_discovery_runs(&self, filter: &DiscoveryRunFilter)
    -> Result<Vec<DiscoveryRun>, String>;
    fn save_discovered_candidate(
        &self,
        candidate: DiscoveredCandidate,
        evidence: CandidateEvidence,
    ) -> Result<(), String>;
    fn list_discovered_candidates(
        &self,
        filter: &DiscoveredCandidateFilter,
    ) -> Result<Vec<DiscoveredCandidate>, String>;
    fn create_suppression(&self, record: SuppressionRecord) -> Result<(), String>;
    fn apply_retention(&self, filter: &RetentionApplicationFilter) -> Result<(), String>;
}
