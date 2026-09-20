//! Port of `daemon/internal/evaluation`: the evaluation replay ledger model,
//! the product/dashboard/discovery/campaign domain, and the evaluation
//! manager (replay candidate + attempt + comparison CRUD with billing quota
//! reservation and runtime-plane recording). See `rs/MIGRATION.md` for
//! conventions: camelCase serde fields, snake_case enum wire values,
//! `thiserror` error enums, `chrono::DateTime<Utc>` times, and no
//! `unwrap`/`expect` outside tests.
//!
//! Wave 4 completion adds the manager and the product-family modules on top of
//! the replay-ledger types already ported here.

macro_rules! string_enum {
    ($name:ident { $first:ident => $first_s:literal $(, $v:ident => $s:literal)* $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
        pub enum $name {
            #[default]
            #[serde(rename = $first_s)]
            $first,
            $(#[serde(rename = $s)] $v),*
        }
        impl $name {
            #[must_use]
            pub fn as_str(self) -> &'static str {
                match self {
                    $name::$first => $first_s,
                    $( $name::$v => $s ),*
                }
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

pub mod campaign;
pub mod campaign_aggregation;
pub mod campaign_runner;
pub mod comparison;
pub mod dashboard;
pub mod discovery;
pub mod discovery_scoring;
pub mod discovery_sources;
pub mod error;
pub mod fixtures;
pub mod manager;
pub mod product_fixture;
pub mod product_fixture_validation;
pub mod product_redaction;
pub mod product_store;
pub mod product_validation;
pub mod runtime_recorder;
pub mod suppression;
pub mod tool_call_inspection;
pub mod tool_call_inspection_diff;
pub mod types;
pub(crate) mod util;

pub use campaign::{
    CampaignSourceSelection, CampaignTransition, CreateCampaignInput, campaign_idempotency_scope,
    campaign_item_from_selection, create_replay_campaign, transition_replay_campaign,
};
pub use campaign_aggregation::{
    CampaignAttemptAggregationInput, CampaignReplayLaunchPlan, build_campaign_attempt_group,
    build_campaign_replay_launch_plan,
};
pub use campaign_runner::{CampaignRunnerInput, CampaignRunnerPlan, build_campaign_runner_plan};
pub use comparison::compare_attempt;
pub use dashboard::{
    DashboardProjectionInput, build_dashboard_projection, page_dashboard_projections,
};
pub use discovery::{
    DISCOVERY_PARTIAL_REASON_MAX_EMITTED_CANDIDATES,
    DISCOVERY_PARTIAL_REASON_MAX_INSPECTED_RECORDS, DiscoveryProgress, StartDiscoveryRunInput,
    apply_discovery_run_progress, build_discovery_run_from_policy, discovery_idempotency_scope,
};
pub use discovery_scoring::{
    CandidateScoringInput, build_discovered_candidate_from_signals, candidate_discovery_score,
    candidate_explanation_fields, readiness_status_default, redaction_status_default,
    score_band_for,
};
pub use discovery_sources::{
    DiscoverySourceFilter, DiscoverySourceReader, DiscoverySourceRecord,
    collect_discovery_source_refs, discovery_source_route, read_discovery_source_refs,
};
pub use error::{BillingReservationError, EvaluationError};
pub use fixtures::{
    CapturedEvidence, candidate_from_fixture, candidate_id_for_fixture, load_captured_evidence,
    load_regression_fixtures,
};
pub use manager::{Dependencies, Manager, Store};
pub use product_fixture::{
    FixtureReviewDecision, FixtureRevisionInput, ProductFixtureInput,
    apply_product_fixture_retention, create_product_fixture_from_candidate,
    create_product_fixture_revision, ensure_product_fixture_editable, product_fixture_selectable,
    review_product_fixture, suppress_product_fixture,
};
pub use product_fixture_validation::{
    ProductFixturePayloadValidation, reject_repo_managed_fixture_edit,
    validate_product_fixture_payload,
};
pub use product_redaction::{
    CandidateEvidenceInput, RedactedEvidence, RedactionPolicy, candidate_evidence_from_payload,
    failed_closed_redacted_evidence, normalize_sensitive_field, redact_evidence_payload,
};
pub use product_store::{
    DiscoveredCandidateFilter, DiscoveryPolicyFilter, DiscoveryRunFilter, ProductListFilter,
    ProductStore, RetentionApplicationFilter,
};
pub use product_validation::{
    DEFAULT_PRODUCT_PAGE_LIMIT, MAX_PRODUCT_PAGE_LIMIT, normalize_product_limit,
    validate_discovery_policy, validate_tenant_scoped_product_request,
};
pub use runtime_recorder::{
    BoxFuture, REPLAY_CREDENTIAL_LEAK_MARKERS, REPLAY_REDACTED_CREDENTIAL,
    REPLAY_RUNTIME_ENTRYPOINT, ReplayRecordInput, ReplayRecordResult, ReplayRuntime,
    ReplayRuntimeStore, RuntimeRecorder, RuntimeReplayRecorder, redact_replay_credential_string,
    redact_replay_credential_strings, redact_replay_record_input, replay_run_goal, replay_workflow,
};
pub use suppression::{
    CreateSuppressionInput, candidate_source_ref, filter_suppressed_candidates,
    find_active_suppression, new_suppression_record, revoke_suppression_record,
    suppression_applies,
};
pub use tool_call_inspection::{
    INSPECTION_DRIFTED, INSPECTION_FAILED, INSPECTION_LIVE_VALIDATION_ABORTED,
    INSPECTION_LIVE_VALIDATION_COMPLETED, INSPECTION_LIVE_VALIDATION_DENIED,
    INSPECTION_LIVE_VALIDATION_FAILED, INSPECTION_LIVE_VALIDATION_OPERATOR_ACTION,
    INSPECTION_MATCHED, INSPECTION_MISSING_ORIGINAL_EVIDENCE, INSPECTION_MISSING_REPLAY_EVIDENCE,
    INSPECTION_UNSUPPORTED, ToolCallInspectionInput, build_tool_call_inspection,
    classify_tool_call_inspection,
};
pub use tool_call_inspection_diff::{ToolCallDiffInput, redacted_tool_call_diff};
pub use types::*;
