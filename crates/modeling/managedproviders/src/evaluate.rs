//! Managed-provider operation planning and preflight evaluation, ported from
//! `bridges.go`. The Go package propagates the operation plan through
//! `context.Context` (`withManagedProviderOperation` /
//! `managedProviderOperationFromContext`); the Rust port passes the plan
//! explicitly (see `Runner::run`) because the port is synchronous and has no
//! context object.

use std::collections::HashMap;

use chrono::Utc;
use kura_sandbox::{
    AccessRequest, ApprovalMode, BackendKind, ConsumerContractView, ConsumerKind,
    ConsumerPolicyRecord, ConsumerRequirementDeclaration, DecisionApprovalStatus,
    DecisionResolution, ExecutionFinalization, ExecutionStatus, ManagedProviderActionKind,
    ManagedProviderOperation, ManagedProviderOperationStatus,
    ManagedProviderRequirementDeclaration, PolicyRecordStatus, SecretDefaultSource,
    SecretEnvironmentScope, SecretResolution, SecretScopeOutcome, SensitiveLocalStateAccessSummary,
    Source,
};

use crate::bridge::{RunResult, SandboxManager};
use crate::error::{DeniedEvaluation, Error};
use crate::helpers::{
    clone_ints, clone_roots, clone_string_map, clone_strings, first_non_empty,
    first_non_empty_roots, paths_within_declared, redacted_path_summary,
};

// Metadata keys and constants (Go `bridges.go`).
pub const METADATA_PROVIDER_ID: &str = "managedProviderId";
pub const METADATA_ACTION: &str = "managedProviderAction";
pub const METADATA_OPERATION_ID: &str = "managedProviderOperationId";
pub const METADATA_PROFILE_ID: &str = "sandboxProfileId";
pub const METADATA_DECISION: &str = "sandboxDecision";
pub const METADATA_FAILURE_CLASS: &str = "failureClass";
pub const METADATA_STRENGTH: &str = "enforcementStrength";
pub const METADATA_SENSITIVE_STATES: &str = "sensitiveStateClasses";
pub const METADATA_ACCESS_SUMMARY: &str = "localStateAccesses";
pub const REQUESTED_BY_PREFIX: &str = "managed_provider:";
pub const REDACTION_RULE: &str = "class_summary_only";

/// Go `managedProviderOperationPlan`.
#[derive(Debug, Clone, Default)]
pub struct ManagedProviderOperationPlan {
    pub operation_id: String,
    pub provider_id: String,
    pub action: ManagedProviderActionKind,
    pub profile_id: String,
    pub requested_by: String,
    pub reason: String,
    pub declared_read: Vec<String>,
    pub declared_write: Vec<String>,
    pub access: AccessRequest,
    pub local_state: Vec<SensitiveLocalStateAccessSummary>,
    pub sensitive_kinds: Vec<String>,
}

/// Go `managedProviderOperationEvaluation`.
#[derive(Debug, Clone, Default)]
pub struct ManagedProviderOperationEvaluation {
    pub declaration: ManagedProviderRequirementDeclaration,
    pub operation: ManagedProviderOperation,
    pub metadata: HashMap<String, String>,
    pub consumer: Option<ConsumerContractView>,
}

/// Go `evaluateManagedProviderOperation`: builds the requirement declaration
/// and operation record for a managed-provider action, runs it through the
/// sandbox manager's access evaluation when one is attached, fails closed when
/// the requested access escapes the declared roots, and finalizes the
/// consumer contract view + metadata.
pub fn evaluate_managed_provider_operation(
    manager: Option<&dyn SandboxManager>,
    operation: &ManagedProviderOperationPlan,
) -> Result<ManagedProviderOperationEvaluation, Error> {
    let now = Utc::now();
    let mut evaluation = ManagedProviderOperationEvaluation {
        declaration: ManagedProviderRequirementDeclaration {
            provider_id: operation.provider_id.trim().to_string(),
            action_kind: operation.action,
            profile_id: operation.profile_id.trim().to_string(),
            backend_kind: BackendKind::Subprocess,
            read_roots: clone_roots(&first_non_empty_roots(
                &operation.declared_read,
                &operation.access.read_roots,
            )),
            write_roots: clone_roots(&first_non_empty_roots(
                &operation.declared_write,
                &operation.access.write_roots,
            )),
            network_mode: operation
                .access
                .network_mode
                .unwrap_or(kura_sandbox::NetworkMode::Deny),
            allowed_hosts: clone_strings(&operation.access.allowed_hosts),
            allowed_ports: clone_ints(&operation.access.allowed_ports),
            approval_mode: ApprovalMode::Allow,
            sensitive_state_classes: clone_strings(&operation.sensitive_kinds),
            enforcement_strength: "declared_only".to_string(),
            active: true,
        },
        operation: ManagedProviderOperation {
            operation_id: new_managed_provider_operation_id(),
            provider_id: operation.provider_id.trim().to_string(),
            action_kind: operation.action,
            requested_by: first_non_empty(&[
                &operation.requested_by,
                &format!("{REQUESTED_BY_PREFIX}{}", operation.provider_id.trim()),
            ]),
            requirement_profile_id: operation.profile_id.trim().to_string(),
            decision: DecisionResolution::Allow,
            approval_status: DecisionApprovalStatus::NotApplicable,
            enforcement_strength: "declared_only".to_string(),
            sensitive_state_classes: clone_strings(&operation.sensitive_kinds),
            started_at: now,
            status: ManagedProviderOperationStatus::LocalStateInspection,
            local_state_access_summaries: clone_local_state_summaries(&operation.local_state),
            ..ManagedProviderOperation::default()
        },
        ..ManagedProviderOperationEvaluation::default()
    };
    evaluation.consumer = Some(build_managed_provider_consumer_view(
        operation,
        Some(&evaluation),
    ));

    if let Some(manager) = manager {
        let decision = manager
            .evaluate_access(&operation.profile_id, "", &operation.access)
            .map_err(Error::Other)?;
        evaluation.operation.decision = decision.resolution;
        evaluation.operation.approval_status = decision.approval_status;
        if decision.resolution == DecisionResolution::Deny {
            evaluation.operation.status = ManagedProviderOperationStatus::Denied;
            evaluation.operation.failure_class =
                kura_sandbox::ErrorClass::PolicyDenied.as_str().to_string();
        }
        if decision.resolution == DecisionResolution::Ask {
            evaluation.operation.status = ManagedProviderOperationStatus::Denied;
            evaluation.operation.failure_class = kura_sandbox::ErrorClass::ApprovalRequired
                .as_str()
                .to_string();
        }
        if let Some(profile) = manager.get_profile(&operation.profile_id) {
            evaluation.declaration.backend_kind = profile.backend_kind;
            evaluation.declaration.approval_mode = profile.approval_policy.mode;
            evaluation.declaration.enforcement_strength =
                first_non_empty(&[&profile.network_policy.enforcement_mode, "declared_only"]);
            evaluation.operation.enforcement_strength =
                evaluation.declaration.enforcement_strength.clone();
        }
    }

    if evaluation.operation.decision == DecisionResolution::Allow {
        let reads_within = paths_within_declared(
            &operation.access.read_roots,
            &evaluation.declaration.read_roots,
        );
        let writes_within = paths_within_declared(
            &operation.access.write_roots,
            &evaluation.declaration.write_roots,
        );
        if !reads_within || !writes_within {
            evaluation.operation.decision = DecisionResolution::Deny;
            evaluation.operation.status = ManagedProviderOperationStatus::Denied;
            evaluation.operation.failure_class =
                kura_sandbox::ErrorClass::PolicyDenied.as_str().to_string();
        }
    }

    evaluation.metadata = operation_metadata(&evaluation.operation);
    if let Some(consumer) = evaluation.consumer.as_mut() {
        consumer.policy_record.as_mut().map(|record| {
            record.decision = evaluation.operation.decision;
            record.approval_status = evaluation.operation.approval_status;
            if evaluation.operation.status == ManagedProviderOperationStatus::Denied {
                record.status = PolicyRecordStatus::Denied;
                record.failure_class = evaluation.operation.failure_class.clone();
                record.completed_at = Some(Utc::now());
            }
        });
    }
    if let Some(manager) = manager {
        if let Some(consumer) = evaluation.consumer.as_ref() {
            let _ = manager.persist_consumer_view(consumer);
        }
    }
    Ok(evaluation)
}

/// Go `operationMetadata`.
#[must_use]
pub fn operation_metadata(operation: &ManagedProviderOperation) -> HashMap<String, String> {
    let mut metadata = HashMap::new();
    metadata.insert(
        METADATA_PROVIDER_ID.to_string(),
        operation.provider_id.trim().to_string(),
    );
    metadata.insert(
        METADATA_ACTION.to_string(),
        operation.action_kind.as_str().to_string(),
    );
    metadata.insert(
        METADATA_OPERATION_ID.to_string(),
        operation.operation_id.trim().to_string(),
    );
    metadata.insert(
        METADATA_PROFILE_ID.to_string(),
        operation.requirement_profile_id.trim().to_string(),
    );
    metadata.insert(
        METADATA_DECISION.to_string(),
        operation.decision.as_str().to_string(),
    );
    metadata.insert(
        METADATA_STRENGTH.to_string(),
        operation.enforcement_strength.trim().to_string(),
    );
    if !operation.sensitive_state_classes.is_empty() {
        metadata.insert(
            METADATA_SENSITIVE_STATES.to_string(),
            operation.sensitive_state_classes.join(","),
        );
    }
    if !operation.failure_class.trim().is_empty() {
        metadata.insert(
            METADATA_FAILURE_CLASS.to_string(),
            operation.failure_class.trim().to_string(),
        );
    }
    if !operation.local_state_access_summaries.is_empty() {
        if let Ok(encoded) = serde_json::to_string(&operation.local_state_access_summaries) {
            metadata.insert(METADATA_ACCESS_SUMMARY.to_string(), encoded);
        }
    }
    metadata
}

/// Go `operationMetadataFromPlan`.
#[must_use]
pub fn operation_metadata_from_plan(
    plan: &ManagedProviderOperationPlan,
) -> HashMap<String, String> {
    let operation = ManagedProviderOperation {
        operation_id: first_non_empty(&[&plan.operation_id, &new_managed_provider_operation_id()]),
        provider_id: plan.provider_id.clone(),
        action_kind: plan.action,
        requested_by: plan.requested_by.clone(),
        requirement_profile_id: plan.profile_id.clone(),
        decision: DecisionResolution::Allow,
        approval_status: DecisionApprovalStatus::NotApplicable,
        enforcement_strength: "declared_only".to_string(),
        sensitive_state_classes: clone_strings(&plan.sensitive_kinds),
        started_at: Utc::now(),
        status: ManagedProviderOperationStatus::Running,
        local_state_access_summaries: clone_local_state_summaries(&plan.local_state),
        ..ManagedProviderOperation::default()
    };
    operation_metadata(&operation)
}

/// Go `buildManagedProviderConsumerView`.
#[must_use]
pub fn build_managed_provider_consumer_view(
    operation: &ManagedProviderOperationPlan,
    evaluation: Option<&ManagedProviderOperationEvaluation>,
) -> ConsumerContractView {
    let consumer_id = operation.provider_id.trim().to_string();
    let operation_kind = operation.action.as_str().to_string();
    let declaration_id = format!("managed_provider:{consumer_id}:{operation_kind}");
    let read_roots = clone_roots(&first_non_empty_roots(
        &operation.declared_read,
        &operation.access.read_roots,
    ));
    let write_roots = clone_roots(&first_non_empty_roots(
        &operation.declared_write,
        &operation.access.write_roots,
    ));

    let mut secret_scope = Vec::new();
    for item in &operation.local_state {
        if !item.sensitive {
            continue;
        }
        secret_scope.push(SecretScopeOutcome {
            consumer_kind: ConsumerKind::ManagedProvider,
            consumer_id: consumer_id.clone(),
            secret_ref: item.state_class.trim().to_string(),
            environment_scope: SecretEnvironmentScope::Both,
            default_source: Some(SecretDefaultSource::InstanceOverride),
            default_rule_id: format!("managed_provider:{consumer_id}"),
            delivery_kind: "local_state_access".to_string(),
            redaction_rule: item.redaction_rule.clone(),
            resolution: SecretResolution::Resolved,
        });
    }

    let mut approval_mode = ApprovalMode::Allow;
    let mut required_strength = "declared_only".to_string();
    if let Some(evaluation) = evaluation {
        approval_mode = evaluation.declaration.approval_mode;
        required_strength = first_non_empty(&[
            &evaluation.declaration.enforcement_strength,
            &required_strength,
        ]);
    }

    let mut policy_record = ConsumerPolicyRecord {
        policy_record_id: format!(
            "policy_{}",
            first_non_empty(&[
                &operation.operation_id,
                &new_managed_provider_operation_id()
            ])
        ),
        consumer_kind: ConsumerKind::ManagedProvider,
        consumer_id: consumer_id.clone(),
        operation_kind: operation_kind.clone(),
        declaration_id: declaration_id.clone(),
        requested_by: first_non_empty(&[
            &operation.requested_by,
            &format!("{REQUESTED_BY_PREFIX}{consumer_id}"),
        ]),
        decision: DecisionResolution::Allow,
        approval_status: DecisionApprovalStatus::NotApplicable,
        secret_resolution: secret_resolution_from_local_state(&secret_scope),
        enforcement_strength: required_strength.clone(),
        provider_operation_id: operation.operation_id.trim().to_string(),
        started_at: Utc::now(),
        status: PolicyRecordStatus::PreflightAllowed,
        ..ConsumerPolicyRecord::default()
    };
    if let Some(evaluation) = evaluation {
        policy_record.decision = evaluation.operation.decision;
        policy_record.approval_status = evaluation.operation.approval_status;
        policy_record.failure_class = evaluation.operation.failure_class.clone();
    }

    ConsumerContractView {
        declaration: Some(ConsumerRequirementDeclaration {
            declaration_id,
            consumer_kind: ConsumerKind::ManagedProvider,
            consumer_id,
            operation_kind,
            profile_id: operation.profile_id.trim().to_string(),
            execution_mode: kura_sandbox::ExecutionMode::Subprocess,
            allowed_backend_kinds: vec![BackendKind::Subprocess],
            read_roots,
            write_roots,
            network_mode: operation.access.network_mode,
            allowed_hosts: clone_strings(&operation.access.allowed_hosts),
            allowed_ports: clone_ints(&operation.access.allowed_ports),
            allow_loopback: operation.access.allow_loopback,
            secret_refs: local_state_class_list(&operation.local_state),
            approval_mode: Some(approval_mode),
            required_enforcement_strength: required_strength,
            active: true,
            source: Source::Builtin,
        }),
        secret_scope,
        policy_record: Some(policy_record),
    }
}

/// Go `secretResolutionFromLocalState`.
#[must_use]
pub fn secret_resolution_from_local_state(items: &[SecretScopeOutcome]) -> SecretResolution {
    if items.is_empty() {
        SecretResolution::NotApplicable
    } else {
        SecretResolution::Resolved
    }
}

/// Go `consumerViewJSON`: the consumer contract view as a JSON value (Go
/// marshals/unmarshals to a `map[string]any`).
#[must_use]
pub fn consumer_view_json(view: Option<&ConsumerContractView>) -> Option<serde_json::Value> {
    let view = view?;
    serde_json::to_value(view).ok()
}

/// Go `finalizeManagedProviderMetadata`.
#[must_use]
pub fn finalize_managed_provider_metadata(
    metadata: &HashMap<String, String>,
    failure_class: &str,
) -> HashMap<String, String> {
    let mut updated = clone_string_map(metadata);
    if failure_class.trim().is_empty() {
        updated.remove(METADATA_FAILURE_CLASS);
    } else {
        updated.insert(
            METADATA_FAILURE_CLASS.to_string(),
            failure_class.trim().to_string(),
        );
    }
    updated
}

/// Go `finalizeManagedProviderExecutionSuccess`.
pub fn finalize_managed_provider_execution_success(
    manager: Option<&dyn SandboxManager>,
    result: &RunResult,
) {
    let Some(manager) = manager else { return };
    if result.execution_id.trim().is_empty() {
        return;
    }
    let _ = manager.finalize_execution(
        &result.execution_id,
        ExecutionFinalization {
            status: Some(ExecutionStatus::Completed),
            ..ExecutionFinalization::default()
        },
    );
}

/// Go `finalizeManagedProviderExecutionFailure`.
pub fn finalize_managed_provider_execution_failure(
    manager: Option<&dyn SandboxManager>,
    result: &RunResult,
    err: &kura_llm::ProviderError,
) {
    let Some(manager) = manager else { return };
    if result.execution_id.trim().is_empty() {
        return;
    }
    let mut finalization = ExecutionFinalization {
        status: Some(ExecutionStatus::Failed),
        error_class: kura_sandbox::ErrorClass::ProviderFailed
            .as_str()
            .to_string(),
        error_code: "provider_error".to_string(),
        error: err.to_string().trim().to_string(),
    };
    let code = err.code().trim().to_string();
    if !code.is_empty() {
        finalization.error_code = first_non_empty(&[&code, &finalization.error_code]);
    }
    if code == "upstream_auth_failed" {
        finalization.error_class = kura_sandbox::ErrorClass::ProviderAuth.as_str().to_string();
    }
    let _ = manager.finalize_execution(&result.execution_id, finalization);
}

/// Go `newManagedProviderOperationID`: UTC timestamp with nanoseconds, no
/// separators.
#[must_use]
pub fn new_managed_provider_operation_id() -> String {
    let stamp = Utc::now()
        .format("%Y%m%d%H%M%S%.9f")
        .to_string()
        .replace('.', "");
    format!("managed_provider_op_{stamp}")
}

/// Go `cloneLocalStateSummaries`.
#[must_use]
pub fn clone_local_state_summaries(
    items: &[SensitiveLocalStateAccessSummary],
) -> Vec<SensitiveLocalStateAccessSummary> {
    items.to_vec()
}

/// Go `localStateSummary`.
#[must_use]
pub fn local_state_summary(
    provider_id: &str,
    action: ManagedProviderActionKind,
    state_class: &str,
    access_mode: kura_sandbox::LocalStateAccessMode,
    path: &str,
    sensitive: bool,
) -> SensitiveLocalStateAccessSummary {
    SensitiveLocalStateAccessSummary {
        provider_id: provider_id.trim().to_string(),
        action_kind: action,
        state_class: state_class.trim().to_string(),
        access_mode,
        path_summary: redacted_path_summary(path),
        declared: true,
        sensitive,
        redaction_rule: REDACTION_RULE.to_string(),
    }
}

/// Go `localStateClassList`: deduped, trimmed state classes.
#[must_use]
pub fn local_state_class_list(items: &[SensitiveLocalStateAccessSummary]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut values = Vec::with_capacity(items.len());
    for item in items {
        let key = item.state_class.trim();
        if key.is_empty() || !seen.insert(key.to_string()) {
            continue;
        }
        values.push(key.to_string());
    }
    values
}

/// Go `cloneAccessRequest`.
#[must_use]
pub fn clone_access_request(access: &AccessRequest) -> AccessRequest {
    AccessRequest {
        read_roots: clone_roots(&access.read_roots),
        write_roots: clone_roots(&access.write_roots),
        network_mode: access.network_mode,
        allowed_hosts: access.allowed_hosts.clone(),
        allowed_ports: access.allowed_ports.clone(),
        allow_loopback: access.allow_loopback,
    }
}

/// Go `withManagedProviderOperation` clone semantics applied to a plan before
/// it is handed to a runner (deep-copies access, local state, sensitive kinds).
#[must_use]
pub fn clone_operation_plan(
    operation: &ManagedProviderOperationPlan,
) -> ManagedProviderOperationPlan {
    ManagedProviderOperationPlan {
        operation_id: operation.operation_id.clone(),
        provider_id: operation.provider_id.clone(),
        action: operation.action,
        profile_id: operation.profile_id.clone(),
        requested_by: operation.requested_by.clone(),
        reason: operation.reason.clone(),
        declared_read: clone_strings(&operation.declared_read),
        declared_write: clone_strings(&operation.declared_write),
        access: clone_access_request(&operation.access),
        local_state: clone_local_state_summaries(&operation.local_state),
        sensitive_kinds: clone_strings(&operation.sensitive_kinds),
    }
}

/// Builds the denial error used by the per-bridge evaluation helpers
/// (codex/claude settings evaluation) when the sandbox denies the request.
#[must_use]
pub fn denied_evaluation(evaluation: ManagedProviderOperationEvaluation) -> Error {
    Error::Denied(DeniedEvaluation {
        message: "sandbox denied managed provider local state access".to_string(),
        evaluation: ManagedProviderOperationEvaluation {
            metadata: finalize_managed_provider_metadata(
                &evaluation.metadata,
                kura_sandbox::ErrorClass::PolicyDenied.as_str(),
            ),
            ..evaluation
        },
    })
}
