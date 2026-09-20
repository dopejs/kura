export type EnvironmentScope = "test" | "prod";

/**
 * How the daemon is deployed, as reported by `/v1/config` and system info.
 *
 * Distinct from {@link EnvironmentScope}, which is a data-isolation label: an
 * embedded daemon (supervised by a host application, one instance per
 * workspace) reports `embedded` here while still writing `test`-scoped data.
 */
export type DeploymentEnvironment = EnvironmentScope | "embedded";

/**
 * A modality a model can be routed to.
 *
 * Roles are modalities rather than performance tiers: a tool that needs an
 * image asks the runtime for the `image` role instead of carrying its own
 * provider, key and retry policy.
 */
export type ModelRole = "primary" | "vision" | "image" | "video" | "embed";

export type ModelRoleResource = {
  role: ModelRole;
  providerId: string;
  /** Empty means the provider's own default model applies. */
  model: string;
  /**
   * False when no provider serves the role. Treat this as "capability
   * unavailable" rather than falling back to the default provider.
   */
  routed: boolean;
  /** Where the binding came from. */
  source: "store" | "config" | "unrouted";
};

export type ModelRoleListResponse = {
  items: ModelRoleResource[];
};

export type ChatQueryInput = {
  provider?: string;
  model?: string;
  skills?: string[];
  query: string;
  timeoutMs?: number;
  maxRetries?: number;
  threadId?: string;
  continuity?: {
    mode?: "auto" | "disabled";
  };
};

export type ChatQueryResponse = {
  dispatchId: string;
  provider: string;
  model: string;
  skills: string[];
  query: string;
  status: "queued" | "running" | "completed" | "partial_failed" | "failed" | "cancelled";
  partial: boolean;
  reply: string;
  finishReason?: string;
  usage: {
    inputTokens: number;
    outputTokens: number;
    totalTokens: number;
  };
  errorCode?: string;
  error?: string;
  threadId?: string;
  sessionSegmentId?: string;
  requestTurnId?: string;
  responseTurnId?: string;
  continuityPreviewId?: string;
  continuityApplied?: boolean;
  continuityStatus?: "applied" | "empty" | "disabled" | "blocked" | "partial" | "failed";
  continuityIncludedCount?: number;
  continuityExcludedCount?: number;
};


export type ChatQueryStreamStarted = {
  dispatchId: string;
  provider: string;
  model: string;
  skills: string[];
  query: string;
  threadId?: string;
  sessionSegmentId?: string;
  requestTurnId?: string;
  continuityPreviewId?: string;
  continuityApplied?: boolean;
  continuityStatus?: "applied" | "empty" | "disabled" | "blocked" | "partial" | "failed";
};

export type ChatQueryStreamDelta = {
  dispatchId: string;
  delta: string;
  reply: string;
};

export type StreamHandlers = {
  onStarted?: (payload: ChatQueryStreamStarted) => void;
  onDelta?: (payload: ChatQueryStreamDelta) => void;
  onCompleted?: (payload: ChatQueryResponse) => void;
  onFailed?: (payload: ChatQueryResponse) => void;
  onCancelled?: (payload: ChatQueryResponse) => void;
};

export type OperatorReadinessItem = {
  itemId: string;
  itemKind: string;
  resourceId?: string;
  displayName: string;
  status: "ready" | "blocked" | "degraded" | "missing_configuration" | "optional";
  healthState?: string;
  reason?: string;
  requiredOperatorAction?: string;
  requiredForSelectedAction: boolean;
  detailRoute?: string;
  environmentScope: EnvironmentScope;
  updatedAt: string;
};

export type OperatorFirstUsefulAction = {
  actionId: string;
  actionKind: string;
  displayName: string;
  recommended: boolean;
  available: boolean;
  blockingItemIds?: string[];
  summary?: string;
  invokeRoute: string;
  resultRoute?: string;
};

export type OperatorOnboardingResponse = {
  environmentScope: EnvironmentScope;
  status: "blocked" | "ready_for_action" | "completed";
  currentStepId?: string;
  completedStepIds?: string[];
  blockingItemIds: string[];
  optionalFollowUpItemIds: string[];
  recommendedActionId?: string;
  readinessItems: OperatorReadinessItem[];
  firstUsefulActions: OperatorFirstUsefulAction[];
  lastEvaluatedAt: string;
};

export type OperatorResourceRef = {
  kind: string;
  id: string;
  route?: string;
};

export type OperatorActivityRecord = {
  activityId: string;
  sourceKind: string;
  sourceId: string;
  title: string;
  status: string;
  summary: string;
  attentionLevel: "info" | "warning" | "critical";
  occurredAt: string;
  detailRoute?: string;
  relatedResourceRefs?: OperatorResourceRef[];
  environmentScope: EnvironmentScope;
};

export type OperatorActivityListResponse = {
  environmentScope: EnvironmentScope;
  items: OperatorActivityRecord[];
  generatedAt: string;
};

export type OperatorDiagnosticFinding = {
  findingId: string;
  sourceKind: string;
  sourceId: string;
  plane: "readiness" | "approval" | "execution" | "delivery";
  severity: "warning" | "critical";
  status: string;
  reason: string;
  recommendedAction?: string;
  detailRoute?: string;
  relatedResourceRefs?: OperatorResourceRef[];
  environmentScope: EnvironmentScope;
  capturedAt: string;
};

export type OperatorDiagnosticListResponse = {
  environmentScope: EnvironmentScope;
  items: OperatorDiagnosticFinding[];
  generatedAt: string;
};

export type OperatorActivityQuery = {
  sourceKind?: string;
  attentionOnly?: boolean;
  limit?: number;
};

export type OperatorDiagnosticsQuery = {
  sourceKind?: string;
  plane?: OperatorDiagnosticFinding["plane"];
  severity?: OperatorDiagnosticFinding["severity"];
};

export type ReplaySourceRef = {
  kind: string;
  id: string;
  route?: string;
};

export type EvaluationProductResourceKind =
  | "discovery_policy"
  | "discovery_run"
  | "discovered_candidate"
  | "candidate_evidence"
  | "suppression"
  | "product_fixture"
  | "fixture_revision"
  | "campaign"
  | "campaign_item"
  | "campaign_attempt_group"
  | "dashboard_projection"
  | "tool_call_inspection"
  | "retention_application";

export type EvaluationProductLifecycleStatus =
  | "queued"
  | "running"
  | "completed"
  | "partial"
  | "failed"
  | "cancelled"
  | "draft"
  | "in_review"
  | "approved"
  | "rejected"
  | "published"
  | "archived"
  | "deleted"
  | "expired"
  | "suppressed";

export type EvaluationProductRetentionState = "active" | "expired" | "deleted" | "tombstone";
export type EvaluationProductSuppressionState = "none" | "suppressed" | "expired" | "revoked";
export type EvaluationProductRedactionStatus = "clean" | "redacted" | "failed";
export type EvaluationProductScoreBand = "high" | "medium" | "low";

export type EvaluationProductTenantOptions = {
  tenantId?: string;
  cursor?: string;
  limit?: number;
};

export type EvaluationProductCursorPage = {
  cursor?: string;
  nextCursor?: string;
  limit: number;
};

export type EvaluationProductDenial = {
  code: string;
  message: string;
  reasonCode: string;
  permission?: string;
  tenantId?: string;
  resourceKind?: EvaluationProductResourceKind;
  resourceId?: string;
  checkedAt?: string;
};

export type EvaluationProductRedactionMetadata = {
  status: EvaluationProductRedactionStatus;
  rulesApplied?: string[];
  sensitiveFieldsExcluded?: string[];
  failureReasonCode?: string;
};

export type EvaluationProductRetentionOutcome = {
  applicationId: string;
  tenantId: string;
  resourceKind: EvaluationProductResourceKind;
  resourceId?: string;
  dryRun: boolean;
  outcome: "retained" | "expired" | "deleted" | "tombstoned" | "dry_run" | "failed";
  reasonCode?: string;
  affectedCount?: number;
  appliedAt: string;
};

export type EvaluationDiscoveryPolicyResource = {
  policyId: string;
  tenantId: string;
  enabled: boolean;
  sourceKinds: ReplayCandidateResource["sourceKind"][];
  windowStart: string;
  windowEnd: string;
  maxInspectedRecords: number;
  maxEmittedCandidates: number;
  costBudget: number;
  sensitiveFieldRules?: string[];
  retentionPolicyRef?: string;
  createdBy?: string;
  createdAt: string;
  updatedAt: string;
};

export type EvaluationDiscoveryRunResource = {
  discoveryRunId: string;
  tenantId: string;
  policyId?: string;
  status: EvaluationProductLifecycleStatus;
  cursor?: string;
  sourceKinds: ReplayCandidateResource["sourceKind"][];
  windowStart: string;
  windowEnd: string;
  maxInspectedRecords: number;
  maxEmittedCandidates: number;
  costBudget: number;
  inspectedRecords: number;
  emittedCandidates: number;
  partialReason?: string;
  startedBy?: string;
  startedAt: string;
  completedAt?: string;
  updatedAt: string;
  idempotencyKey?: string;
};

export type EvaluationDiscoveredCandidateResource = {
  discoveredCandidateId: string;
  tenantId: string;
  discoveryRunId: string;
  sourceKind: ReplayCandidateResource["sourceKind"];
  sourceId: string;
  sourceRefs?: ReplaySourceRef[];
  score: number;
  scoreBand: EvaluationProductScoreBand;
  explanationFields?: Record<string, unknown>;
  redactionStatus: EvaluationProductRedactionStatus;
  evidenceRef?: string;
  readinessStatus: ReplayCandidateResource["readinessStatus"];
  suppressionState: EvaluationProductSuppressionState;
  retentionState: EvaluationProductRetentionState;
  createdAt: string;
  updatedAt: string;
  expiresAt?: string;
};

export type EvaluationSuppressionRecord = {
  suppressionId: string;
  tenantId: string;
  targetKind: EvaluationProductResourceKind;
  targetId?: string;
  targetSourceRef?: string;
  reasonCode: string;
  reason?: string;
  createdBy?: string;
  createdAt: string;
  expiresAt?: string;
  active: boolean;
};

export type EvaluationProductListResponse<T> = {
  tenantId: string;
  page: EvaluationProductCursorPage;
  items: T[];
};

export type EvaluationDiscoveryPolicyQuery = {
  enabled?: boolean;
  cursor?: string;
  limit?: number;
};

export type UpsertEvaluationDiscoveryPolicyInput = {
  enabled: boolean;
  sourceKinds: ReplayCandidateResource["sourceKind"][];
  windowStart: string;
  windowEnd: string;
  maxInspectedRecords: number;
  maxEmittedCandidates: number;
  costBudget: number;
  sensitiveFieldRules?: string[];
  retentionPolicyRef?: string;
  idempotencyKey?: string;
};

export type StartEvaluationDiscoveryRunInput = {
  policyId?: string;
  windowStart?: string;
  windowEnd?: string;
  sourceKinds?: ReplayCandidateResource["sourceKind"][];
  maxInspectedRecords?: number;
  maxEmittedCandidates?: number;
  costBudget?: number;
  cursor?: string;
  idempotencyKey?: string;
};

export type EvaluationDiscoveryRunQuery = {
  status?: EvaluationDiscoveryRunResource["status"];
  sourceKind?: ReplayCandidateResource["sourceKind"];
  cursor?: string;
  limit?: number;
};

export type EvaluationDiscoveredCandidateQuery = {
  discoveryRunId?: string;
  sourceKind?: ReplayCandidateResource["sourceKind"];
  readinessStatus?: ReplayCandidateResource["readinessStatus"];
  suppressionState?: EvaluationProductSuppressionState;
  scoreBand?: EvaluationProductScoreBand;
  cursor?: string;
  limit?: number;
};

export type CreateEvaluationSuppressionInput = {
  suppressionId?: string;
  targetKind: EvaluationProductResourceKind;
  targetId?: string;
  targetSourceRef?: string;
  reasonCode: string;
  reason?: string;
  expiresAt?: string;
  idempotencyKey?: string;
};

export type ProductFixtureDomainClass = "schedule" | "integration" | "computer_use";

export type ProductFixtureResource = {
  fixtureId: string;
  tenantId: string;
  displayName: string;
  domainClass: ProductFixtureDomainClass;
  sourceKind: string;
  sourceRefs?: ReplaySourceRef[];
  sourceCandidateId?: string;
  currentRevisionId: string;
  reviewState: EvaluationProductLifecycleStatus;
  suppressionState: EvaluationProductSuppressionState;
  retentionState: EvaluationProductRetentionState;
  createdBy?: string;
  createdAt: string;
  updatedAt: string;
};

export type FixtureRevisionResource = {
  revisionId: string;
  fixtureId: string;
  tenantId: string;
  revisionNumber: number;
  contentSummary?: string;
  fixturePayload?: Record<string, unknown>;
  changeSummary?: string;
  sourceEvidenceRefs?: string[];
  redactionStatus: EvaluationProductRedactionStatus;
  createdBy?: string;
  createdAt: string;
};

export type ProductFixtureMutationResponse = {
  fixture: ProductFixtureResource;
  revision?: FixtureRevisionResource;
};

export type ProductFixtureQuery = {
  domainClass?: ProductFixtureDomainClass;
  reviewState?: EvaluationProductLifecycleStatus;
  suppressionState?: EvaluationProductSuppressionState;
  cursor?: string;
  limit?: number;
};

export type CreateProductFixtureInput = {
  fixtureId?: string;
  displayName: string;
  domainClass: ProductFixtureDomainClass;
  fixturePayload?: Record<string, unknown>;
  changeSummary?: string;
  idempotencyKey?: string;
};

export type CreateFixtureRevisionInput = {
  revisionId?: string;
  fixturePayload?: Record<string, unknown>;
  contentSummary?: string;
  changeSummary?: string;
  sourceEvidenceRefs?: string[];
  idempotencyKey?: string;
};

export type ReviewProductFixtureInput = {
  revisionId: string;
  decision: "approved" | "rejected" | "needs_changes";
  reason?: string;
};

export type EvaluationCampaignResource = {
  campaignId: string;
  tenantId: string;
  displayName: string;
  status: EvaluationProductLifecycleStatus;
  scopeSummary?: string;
  startedBy?: string;
  createdAt: string;
  startedAt?: string;
  completedAt?: string;
  publishedAt?: string;
  retentionState: EvaluationProductRetentionState;
  idempotencyKey?: string;
};

export type EvaluationCampaignItemResource = {
  campaignItemId: string;
  campaignId: string;
  tenantId: string;
  sourceType: EvaluationProductResourceKind;
  sourceId: string;
  sourceSnapshot?: Record<string, unknown>;
  selectionReason?: string;
  suppressionCheckedAt: string;
  createdAt: string;
};

export type EvaluationCampaignAttemptGroupResource = {
  attemptGroupId: string;
  campaignId: string;
  campaignItemId: string;
  tenantId: string;
  replayAttemptIds?: string[];
  comparisonIds?: string[];
  liveValidationIds?: string[];
  status: EvaluationProductLifecycleStatus;
  driftCount: number;
  failureCount: number;
  unsupportedCount: number;
  operatorActionNeededCount: number;
  summary?: string;
  createdAt: string;
  updatedAt: string;
};

export type CreateEvaluationCampaignInput = {
  campaignId?: string;
  displayName: string;
  scopeSummary?: string;
  sourceSelections?: Array<{
    sourceType: EvaluationProductResourceKind;
    sourceId: string;
    sourceSnapshot?: Record<string, unknown>;
    selectionReason?: string;
  }>;
  startImmediately?: boolean;
  idempotencyKey?: string;
};

export type EvaluationDashboardProjectionResource = {
  projectionId: string;
  tenantId: string;
  windowStart: string;
  windowEnd: string;
  campaignStatusCounts?: Record<string, number>;
  driftSummary?: Record<string, number>;
  failureSummary?: Record<string, number>;
  unsupportedSummary?: Record<string, number>;
  operatorActionNeededSummary?: Record<string, number>;
  liveValidationSummary?: Record<string, number>;
  candidateSummary?: Record<string, number>;
  fixtureSummary?: Record<string, number>;
  generatedAt: string;
  cursor?: string;
  retentionState?: EvaluationProductRetentionState;
};

export type EvaluationToolCallInspectionClassification =
  | "matched"
  | "drifted"
  | "failed"
  | "unsupported"
  | "missing_original_evidence"
  | "missing_replay_evidence"
  | "live_validation_denied"
  | "live_validation_aborted"
  | "live_validation_failed"
  | "live_validation_operator_action_needed"
  | "live_validation_completed";

export type EvaluationToolCallInspectionResource = {
  inspectionId: string;
  tenantId: string;
  campaignId: string;
  campaignItemId: string;
  toolCallRef: string;
  originalEvidenceRef?: string;
  nonLiveReplayEvidenceRef?: string;
  liveValidationLedgerRefs?: string[];
  classification: EvaluationToolCallInspectionClassification;
  diffSummary?: string;
  redactionStatus: EvaluationProductRedactionStatus;
  retentionState?: EvaluationProductRetentionState;
  createdAt: string;
  updatedAt: string;
};

export type PlaneSummaries = {
  runtime?: string;
  policy?: string;
  integration?: string;
  delivery?: string;
  evidence?: string;
};

export type ReplayCandidateResource = {
  candidateId: string;
  candidateKind: "curated_work" | "fixture";
  displayName: string;
  description?: string;
  sourceKind: "run" | "workflow" | "schedule" | "integration" | "computer_use" | "fixture";
  sourceId: string;
  sourceRefs: ReplaySourceRef[];
  toolClasses?: string[];
  environmentScope: EnvironmentScope;
  readinessStatus: "fully_replayable" | "partially_replayable" | "blocked" | "unreplayable";
  readinessReasons: string[];
  limitations: string[];
  defaultReplayMode: "non_live";
  fixtureId?: string;
  latestAttemptId?: string;
  latestComparisonId?: string;
  expectedComparisonSummary?: PlaneSummaries;
  capturedEvidenceRefs?: ReplaySourceRef[];
  createdAt: string;
  updatedAt: string;
};

export type ReplayCandidateListResponse = {
  environmentScope: EnvironmentScope;
  items: ReplayCandidateResource[];
};

export type ReplayCandidateQuery = {
  candidateKind?: ReplayCandidateResource["candidateKind"];
  sourceKind?: ReplayCandidateResource["sourceKind"];
  readinessStatus?: ReplayCandidateResource["readinessStatus"];
  limit?: number;
};

export type CreateReplayCandidateInput = Omit<ReplayCandidateResource, "createdAt" | "updatedAt" | "latestAttemptId" | "latestComparisonId"> & {
  createdAt?: string;
  updatedAt?: string;
};

export type CreateReplayAttemptInput = {
  mode?: "non_live" | "live_validation";
  changeWindowLabel?: string;
  baselineAttemptId?: string;
  safetyScope?: {
    mode?: "non_live" | "live_validation";
    description?: string;
  };
};

export type ReplayAttemptResource = {
  attemptId: string;
  candidateId: string;
  sourceRefs: ReplaySourceRef[];
  environmentScope: EnvironmentScope;
  mode: "non_live" | "live_validation";
  status: "queued" | "running" | "completed" | "blocked" | "unreplayable" | "failed" | "cancelled";
  safetyScope: {
    mode?: "non_live" | "live_validation";
    description?: string;
  };
  approvalHandling: "blocked" | "evidence_only" | "fresh_approval_required";
  sideEffectHandling: "blocked" | "evidence_only" | "live";
  launchedBy?: string;
  changeWindowLabel?: string;
  baselineAttemptId?: string;
  resultRunId?: string;
  resultWorkflowId?: string;
  evidenceRefs: ReplaySourceRef[];
  blockedReasons: string[];
  runtimeSummary?: string;
  policySummary?: string;
  integrationSummary?: string;
  deliverySummary?: string;
  evidenceSummary?: string;
  startedAt?: string;
  completedAt?: string;
  createdAt: string;
  updatedAt: string;
};

export type ReplayAttemptListResponse = {
  environmentScope: EnvironmentScope;
  items: ReplayAttemptResource[];
};

export type ReplayAttemptQuery = {
  candidateId?: string;
  status?: ReplayAttemptResource["status"];
  limit?: number;
};

export type DriftFindingResource = {
  findingId: string;
  comparisonId: string;
  plane: "runtime" | "policy" | "integration" | "delivery" | "evidence" | "unknown" | "mixed";
  severity: "info" | "warning" | "critical";
  summary: string;
  baselineValue?: string;
  replayValue?: string;
  evidenceRefs?: ReplaySourceRef[];
  recommendedAction?: string;
  createdAt: string;
};

export type CreateReplayComparisonInput = {
  baselineAttemptId?: string;
  baselineRef?: string;
  changeWindowLabel?: string;
};

export type ReplayComparisonResource = {
  comparisonId: string;
  candidateId: string;
  baselineRef: string;
  attemptId: string;
  environmentScope: EnvironmentScope;
  terminalStatus: "matched" | "drifted" | "blocked" | "unreplayable";
  runtimeSummary: string;
  policySummary: string;
  integrationSummary: string;
  deliverySummary: string;
  evidenceSummary: string;
  confidence: "high" | "medium" | "low";
  limitations: string[];
  driftFindings: DriftFindingResource[];
  changeWindowLabel?: string;
  generatedAt: string;
};

export type ReplayComparisonListResponse = {
  environmentScope: EnvironmentScope;
  items: ReplayComparisonResource[];
};

export type ReplayComparisonQuery = {
  candidateId?: string;
  attemptId?: string;
  terminalStatus?: ReplayComparisonResource["terminalStatus"];
  limit?: number;
};

export type ReplayFixtureResource = {
  fixtureId: string;
  displayName: string;
  domainClass: "schedule" | "integration" | "computer_use";
  manifestPath: string;
  sourceRefs: ReplaySourceRef[];
  capturedEvidenceRefs: ReplaySourceRef[];
  assumptions: string[];
  limitations: string[];
  expectedReplayMode: "non_live" | "live_validation";
  expectedComparisonSummary: PlaneSummaries;
  candidateId?: string;
  environmentScope: EnvironmentScope;
  createdAt: string;
  updatedAt: string;
};

export type ReplayFixtureListResponse = {
  environmentScope: EnvironmentScope;
  items: ReplayFixtureResource[];
};

export type ReplayFixtureQuery = {
  domainClass?: ReplayFixtureResource["domainClass"];
  limit?: number;
};

export type LiveValidationAttemptStatus =
  | "queued"
  | "awaiting_approval"
  | "running"
  | "completed"
  | "blocked"
  | "aborted"
  | "failed"
  | "operator_action_needed";

export type LiveValidationLedgerOutcome =
  | "attempted"
  | "skipped"
  | "completed"
  | "failed"
  | "aborted"
  | "denied"
  | "operator_action_needed";

export type LiveValidationSafetyClass =
  | "read_only"
  | "idempotent_mutation"
  | "non_idempotent_mutation"
  | "unsupported";

export type LiveValidationGateDecision = {
  allowed: boolean;
  reasonCode?: string;
  reference?: string;
  checkedAt: string;
};

export type LiveValidationSideEffectScope = {
  scopeId: string;
  validationId: string;
  includedToolClasses?: string[];
  excludedToolClasses?: string[];
  includedActions?: string[];
  excludedActions?: string[];
  approvalMode: "scope_level" | "per_action" | "mixed";
  declaredBy: string;
  declaredAt: string;
};

export type LiveValidationAttemptResource = {
  validationId: string;
  tenantId?: string;
  candidateId: string;
  sourceAttemptId?: string;
  requestedBy: string;
  environmentScope: EnvironmentScope | string;
  requestedScope: LiveValidationSideEffectScope;
  status: LiveValidationAttemptStatus;
  permissionDecision: LiveValidationGateDecision;
  quotaDecision: LiveValidationGateDecision;
  killSwitchDecision: LiveValidationGateDecision;
  approvalSummary: {
    required: number;
    approved: number;
    denied: number;
    expired: number;
    pending: number;
  };
  ledgerSummary: Partial<Record<LiveValidationLedgerOutcome, number>>;
  comparisonId?: string;
  createdAt: string;
  startedAt?: string;
  completedAt?: string;
  updatedAt: string;
};

export type LiveValidationAttemptListResponse = {
  tenantId?: string;
  environmentScope?: EnvironmentScope | string;
  items: LiveValidationAttemptResource[];
};

export type LiveValidationApprovalEvidence = {
  approvalId: string;
  validationId: string;
  tenantId?: string;
  approvalTarget: "scope" | "action";
  toolClass: string;
  safetyClass: LiveValidationSafetyClass;
  actionRef?: string;
  approvedScope?: string;
  status: "pending" | "approved" | "denied" | "expired";
  requestedBy: string;
  resolvedBy?: string;
  requestedAt: string;
  resolvedAt?: string;
};

export type CreateLiveValidationInput = {
  validationId?: string;
  candidateId: string;
  sourceAttemptId?: string;
  candidateToolClasses?: string[];
  requestedScope: LiveValidationSideEffectScope;
  freshApprovals?: LiveValidationApprovalEvidence[];
  clientKey?: string;
  changeWindowLabel?: string;
};

export type CreateLiveValidationResponse = {
  attempt: LiveValidationAttemptResource;
  denials?: Array<{
    gate: "permission" | "quota" | "kill_switch" | "support_matrix" | "approval" | "scope" | string;
    reasonCode: string;
    message: string;
    reference?: string;
  }>;
};

export type LiveValidationAttemptQuery = {
  candidateId?: string;
  status?: LiveValidationAttemptStatus;
  limit?: number;
};

export type LiveValidationLedgerResource = {
  ledgerEntryId: string;
  validationId: string;
  tenantId?: string;
  candidateId: string;
  sourceRef: string;
  toolClass: string;
  safetyClass: LiveValidationSafetyClass;
  actionRef: string;
  approvalId?: string;
  correlationKey?: string;
  downstreamRef?: string;
  outcome: LiveValidationLedgerOutcome;
  reasonCode?: string;
  attemptedAt?: string;
  completedAt?: string;
  updatedAt: string;
  evidenceRefs?: string[];
  retryCount: number;
  ambiguousCommit: boolean;
  reconciliationId?: string;
};

export type LiveValidationLedgerListResponse = {
  validationId: string;
  tenantId?: string;
  items: LiveValidationLedgerResource[];
};

export type LiveValidationSupportMatrixResource = {
  toolClass: string;
  safetyClass: LiveValidationSafetyClass;
  permission?: TenantPermission | string;
  approval: "not_required" | "scope_level" | "per_action" | "unsupported";
  approvalAction?: string;
  idempotency?: string;
  retryPolicy: "automatic_retry" | "manual_retry" | "no_retry";
  ambiguousCommitBehavior?: string;
  compensation: "not_applicable" | "automatic_compensation" | "manual_confirmation" | "unsupported";
  ledgerEvents: LiveValidationLedgerOutcome[];
  testCase: string;
  version: string;
};

export type LiveValidationSupportMatrixResponse = {
  environmentScope?: EnvironmentScope | string;
  version: string;
  items: LiveValidationSupportMatrixResource[];
};

export type LiveValidationComparisonResource = {
  comparisonId: string;
  validationId: string;
  candidateId: string;
  baselineRef: string;
  terminalStatus: "matched" | "drifted" | "blocked" | "unsupported" | "operator_action_needed";
  ledgerSummary: Partial<Record<LiveValidationLedgerOutcome, number>>;
  unsupportedClasses?: string[];
  denials?: string[];
  ambiguousCommits?: string[];
  driftFindings?: string[];
  generatedAt: string;
};

export type ResolveLiveValidationReconciliationInput = {
  resolution: "confirmed_committed" | "confirmed_not_committed" | "compensated" | "accepted_manual_state" | "unsupported_unresolved";
  reason: string;
  evidenceRefs?: string[];
};

export type LiveValidationReconciliationResource = {
  reconciliationId: string;
  ambiguousCommitId: string;
  tenantId?: string;
  resolvedBy: string;
  resolution: ResolveLiveValidationReconciliationInput["resolution"];
  reason: string;
  evidenceRefs?: string[];
  resolvedAt: string;
};

export type LiveValidationRetentionResource = {
  policyId: string;
  tenantId?: string;
  appliesTo: "attempts" | "ledger_entries" | "reconciliation_decisions" | "comparisons" | "all";
  mode: "indefinite" | "explicit";
  retentionPeriod?: string;
  createdByPrincipalId: string;
  createdAt: string;
  expiresAt?: string;
};

export type UpdateLiveValidationKillSwitchInput = {
  scope: "tenant" | "global";
  tenantId?: string;
  enabled: boolean;
  reason: string;
  expiresAt?: string;
};

export type LiveValidationKillSwitchResource = {
  killSwitchId: string;
  scope: "tenant" | "global";
  tenantId?: string;
  enabled: boolean;
  reason: string;
  changedBy: string;
  changedAt: string;
  expiresAt?: string;
};

export type LiveValidationKillSwitchListResponse = {
  tenantId?: string;
  items: LiveValidationKillSwitchResource[];
};

export type ApprovalStatus = "pending" | "approved" | "rejected";

export type ApprovalResource = {
  approvalId: string;
  action: string;
  resourceKind?: string;
  resourceId?: string;
  reason: string;
  requestedBy?: string;
  status: ApprovalStatus;
  createdAt: string;
  updatedAt: string;
  resolvedAt?: string;
  resolution?: string;
  comment?: string;
  integrationBindings?: Array<Record<string, unknown>>;
  sandbox?: Record<string, unknown>;
};

export type ApprovalListResponse = {
  items: ApprovalResource[];
};

export type DecisionResource = {
  decisionId: string;
  action: string;
  resourceKind?: string;
  resourceId?: string;
  outcome: string;
  reason: string;
  approvalId?: string;
  createdAt: string;
  sandbox?: Record<string, unknown>;
};

export type ApprovalDecisionResponse = {
  approval: ApprovalResource;
  decision: DecisionResource;
};

export type ResolveApprovalInput = {
  resolution: "approved" | "rejected";
  comment?: string;
};

export type SessionRouteRequest = {
  kind?: string;
  channel?: string;
  accountId?: string;
  peerId?: string;
  threadId?: string;
};

export type CreateRunInput = {
  sessionId?: string;
  route?: SessionRouteRequest;
  entrypoint: string;
  goal?: string;
  input?: unknown;
};

export type RunResource = {
  runId: string;
  sessionId?: string;
  scheduleId?: string;
  scheduleAttemptId?: string;
  reminderId?: string;
  reminderOccurrenceId?: string;
  entrypoint: string;
  status: string;
  goal: string;
  activeWorkflowId?: string;
  workflowCount?: number;
  latestDeliveryId?: string;
  latestDeliveryStatus?: string;
  latestDeliveryTargetId?: string;
  activeProfileProjection?: AgentProfileRuntimeProjection;
  createdAt: string;
  updatedAt: string;
};

export type DaemonEventScope = {
  sessionId?: string;
  runId?: string;
  workflowId?: string;
  workflowStepId?: string;
  scheduleId?: string;
  scheduleAttemptId?: string;
  stepId?: string;
  computerUseSessionId?: string;
  computerUseActionId?: string;
  connectorId?: string;
  capabilityId?: string;
};

export type DaemonEvent = {
  eventId: string;
  sequence: number;
  category: string;
  name: string;
  occurredAt: string;
  scope: DaemonEventScope;
  resource: {
    kind: string;
    id: string;
  };
  payload: Record<string, unknown>;
};

export type EventStreamQuery = {
  category?: string;
  runId?: string;
  sessionId?: string;
  scheduleId?: string;
  scheduleAttemptId?: string;
  resourceKind?: string;
  cursor?: number;
};

export type EventStreamHandlers = {
  onEvent?: (event: DaemonEvent) => void;
  onError?: (error: Error) => void;
};

export type EventStreamSubscription = {
  close: () => void;
  completed: Promise<void>;
};

export type TenantRole = "owner" | "admin" | "operator" | "viewer";
export type TenantKind = "personal" | "organization";
export type TenantLifecycleStatus =
  | "invited"
  | "pending"
  | "active"
  | "disabled"
  | "removed"
  | "rejected"
  | "revoked"
  | "expired"
  | "accepted"
  | "rotated";

export type TenantPermission =
  | "tenant.manage"
  | "secrets.manage"
  | "credentials.inspect"
  | "integrations.manage"
  | "integrations.diagnostics.read"
  | "integrations.diagnostics.run"
  | "integrations.diagnostics.smoke"
  | "integrations.diagnostics.smoke_risky"
  | "connectors.manage"
  | "mcp.manage"
  | "runs.execute"
  | "approvals.resolve"
  | "live_validation.execute"
  | "live_validation.reconcile"
  | "evaluation.manage"
  | "billing.view"
  | "billing.manage"
  | "billing.evidence_export"
  | "profiles.inspect"
  | "profiles.manage"
  | "read_only.inspect";

export type IntegrationDiagnosticStatus = "unknown" | "healthy" | "degraded" | "blocked" | "unsupported";
export type IntegrationDiagnosticFreshnessState = "fresh" | "stale";
export type IntegrationDiagnosticRetrySafety = "no_action_needed" | "retryable" | "blocked" | "unsafe_to_retry" | "operator_action_needed";
export type IntegrationDiagnosticRemediationOwner = "product_user" | "tenant_admin" | "operator" | "provider" | "none_required";
export type IntegrationDiagnosticRedactionStatus = "redacted" | "suppressed" | "failed_closed";

export type IntegrationDiagnosticReasonCode =
  | "healthy"
  | "auth_missing"
  | "permission_missing"
  | "blocked_route"
  | "duplicate_inbound"
  | "reply_failed"
  | "unsupported_capability"
  | "unknown_connector_failure"
  | "app_authorization_missing"
  | "bot_authorization_missing"
  | "user_authorization_missing"
  | "tenant_approval_pending"
  | "scope_missing"
  | "token_missing"
  | "token_expired"
  | "token_revoked"
  | "refresh_credentials_missing"
  | "token_refresh_failed"
  | "tenant_mismatch"
  | "rate_limited"
  | "provider_unavailable"
  | "transient_provider_failure"
  | "network_failed"
  | "ambiguous_downstream_commit"
  | "unsafe_to_retry"
  | "operator_action_needed"
  | "limited_diagnostic"
  | "unsupported_diagnostic"
  | "redaction_failed_closed"
  | "unknown_provider_error";

export type DiscordReadinessState = "hosted_ready" | "degraded_needs_repair" | "failed" | "disabled";
export type DiscordCredentialState = "missing" | "submitted" | "valid" | "invalid" | "revoked" | "redaction_suppressed";
export type DiscordRedactionStatus = "redacted" | "suppressed" | "redaction_failed";
export type TelegramTerminalState = "ready" | "degraded" | "unavailable" | "cancelled" | "action-required";
export type TelegramCredentialState = DiscordCredentialState;
export type TelegramRedactionStatus = DiscordRedactionStatus;
export type SlackTerminalState = TelegramTerminalState;
export type SlackRedactionStatus = DiscordRedactionStatus;
export type MatrixTerminalState = TelegramTerminalState;
export type MatrixRedactionStatus = DiscordRedactionStatus;

export type DiscordDestinationValidationResource = {
  tenantId?: string;
  connectorId: string;
  destinationId: string;
  destinationType: "guild" | "channel" | "direct_message";
  providerLabel?: string;
  selected: boolean;
  validationState: "valid" | "invalid" | "missing_permission" | "message_content_missing" | "bot_not_member" | "not_found" | "dm_restricted" | "stale";
  reasonCode?: string;
  validatedAt: string;
  redactionStatus: DiscordRedactionStatus;
  safeEvidence?: Record<string, string>;
};

export type DiscordHostedSetupResource = {
  tenantId?: string;
  connectorId: string;
  connectorKind: "discord";
  displayName: string;
  status: "configured" | "healthy" | "degraded" | "failed" | "permission_blocked" | "rate_limited" | "unsupported_capability" | "disabled";
  readinessState: DiscordReadinessState;
  hostedReady: boolean;
  credentialState: DiscordCredentialState;
  respondInDM: boolean;
  requireMention: boolean;
  deliveryMode: "gateway";
  reasonCode?: string;
  destinations?: DiscordDestinationValidationResource[];
  redactionStatus: DiscordRedactionStatus;
  createdAt: string;
  updatedAt: string;
  validatedAt?: string;
  retentionExpiresAt: string;
};

export type DiscordSmokeEvidenceResource = {
  smokeEvidenceId: string;
  tenantId?: string;
  connectorId: string;
  status: "passed" | "failed" | "skipped";
  credentialMode: "safe_live" | "fake" | "unavailable";
  owner: string;
  reason: string;
  remainingRisk?: string;
  validatedAt: string;
  retentionExpiresAt: string;
  redactionStatus: DiscordRedactionStatus;
  safeEvidence?: Record<string, string>;
};

export type TelegramAllowmentResource = {
  tenantId?: string;
  connectorId: string;
  allowmentId: string;
  telegramScopeType: "user" | "direct_chat" | "group";
  telegramScopeId: string;
  providerLabel?: string;
  enabled: boolean;
  groupGate: "not_applicable" | "mention_or_command_required";
  validationState: "valid" | "invalid" | "blocked" | "stale" | "missing_permission" | "not_found";
  reasonCode?: string;
  validatedAt: string;
  redactionStatus: TelegramRedactionStatus;
  safeEvidence?: Record<string, string>;
};

export type TelegramHostedSetupResource = {
  tenantId?: string;
  connectorId: string;
  connectorKind: "telegram";
  displayName: string;
  status: "configured" | "healthy" | "degraded" | "failed" | "permission_blocked" | "rate_limited" | "unsupported_capability";
  terminalState: TelegramTerminalState;
  hostedReady: boolean;
  credentialState: TelegramCredentialState;
  allowmentState: "none" | "partial" | "valid" | "stale";
  groupBehavior: "disabled" | "mention_or_command_required";
  deliveryEligible: boolean;
  reasonCode?: string;
  allowments?: TelegramAllowmentResource[];
  redactionStatus: TelegramRedactionStatus;
  createdAt: string;
  updatedAt: string;
  validatedAt?: string;
  retentionExpiresAt: string;
};

export type TelegramSmokeEvidenceResource = {
  smokeEvidenceId: string;
  tenantId?: string;
  connectorId: string;
  status: "passed" | "failed" | "skipped";
  credentialMode: "safe_live" | "fake" | "unavailable";
  owner: string;
  reason: string;
  remainingRisk?: string;
  validatedAt: string;
  retentionExpiresAt: string;
  redactionStatus: TelegramRedactionStatus;
  safeEvidence?: Record<string, string>;
};

export type SlackWorkspaceBindingResource = {
  workspaceId: string;
  workspaceLabel?: string;
  installationId: string;
  oauthGrantState: "valid" | "missing" | "revoked" | "scope_missing" | "approval_required" | "workspace_mismatch" | "provider_unavailable" | "network_failed" | "unknown";
  requiredScopeState: "valid" | "missing" | "stale" | "unknown";
  validatedAt: string;
  redactionStatus: SlackRedactionStatus;
  safeEvidence?: Record<string, string>;
};

export type SlackConversationRouteResource = {
  conversationId: string;
  conversationType: "direct_message" | "channel";
  selectedChannelState: "selected" | "not_selected" | "stale" | "archived" | "missing_membership" | "not_applicable";
  validationState: "valid" | "partial" | "stale" | "blocked" | "missing_permission";
  reasonCode?: string;
  redactionStatus?: SlackRedactionStatus;
  safeEvidence?: Record<string, string>;
};

export type SlackRoutePolicyResource = {
  tenantId?: string;
  connectorId: string;
  workspaceBindingId: string;
  selectedChannels: SlackConversationRouteResource[];
  allowedDMUsers: string[];
  allowedDMUserGroups: string[];
  mentionGate: "agent_mention_required";
  threadReplyMode: "channel_mentions_thread_rooted";
  validationState: "valid" | "partial" | "stale" | "blocked" | "missing_permission";
  reasonCode?: string;
  validatedAt: string;
  redactionStatus: SlackRedactionStatus;
  safeEvidence?: Record<string, string>;
};

export type SlackHostedSetupResource = {
  tenantId?: string;
  connectorId: string;
  connectorKind: "slack";
  displayName: string;
  status: "configured" | "healthy" | "degraded" | "failed" | "permission_blocked" | "rate_limited" | "unsupported_capability";
  terminalState: SlackTerminalState;
  oauthState: "not_started" | "started" | "callback_received" | "grant_valid" | "grant_missing" | "scope_missing" | "approval_required" | "revoked" | "redaction_suppressed";
  routePolicyState: "none" | "partial" | "valid" | "stale";
  deliveryEligible: boolean;
  workspaceBindingId: string;
  workspaceBinding?: SlackWorkspaceBindingResource;
  routePolicy?: SlackRoutePolicyResource;
  reasonCode?: string;
  redactionStatus: SlackRedactionStatus;
  createdAt: string;
  updatedAt: string;
  validatedAt?: string;
  retentionExpiresAt: string;
};

export type SlackSmokeEvidenceResource = {
 smokeEvidenceId: string;
 tenantId?: string;
  connectorId: string;
  workspaceBindingId: string;
  status: "passed" | "failed" | "skipped";
  authorizationMode: "safe_live" | "fake_oauth" | "unavailable";
  owner: string;
  reason: string;
  remainingRisk?: string;
  validatedAt: string;
  retentionExpiresAt: string;
  redactionStatus: SlackRedactionStatus;
 safeEvidence?: Record<string, string>;
};

export type MatrixHomeserverBindingResource = {
  homeserverUrl: string;
  homeserverName?: string;
  botUserId: string;
  botDeviceId?: string;
  authorizationState: "valid" | "missing" | "revoked" | "permission_missing" | "ownership_mismatch" | "provider_unavailable" | "network_failed" | "unknown";
  homeserverCapabilityState: "valid" | "unsupported" | "stale" | "rate_limited" | "unknown";
  validatedAt: string;
  redactionStatus: MatrixRedactionStatus;
  safeEvidence?: Record<string, string>;
};

export type MatrixConversationRouteResource = {
  conversationId: string;
  conversationType: "direct_message" | "room";
  roomSelectionState: "selected" | "not_selected" | "stale" | "left" | "banned" | "missing_membership" | "encrypted_unsupported" | "not_applicable";
  validationState: "valid" | "partial" | "stale" | "blocked" | "missing_permission";
  reasonCode?: string;
  redactionStatus?: MatrixRedactionStatus;
  safeEvidence?: Record<string, string>;
};

export type MatrixRoutePolicyResource = {
  tenantId?: string;
  connectorId: string;
  homeserverBindingId: string;
  selectedRooms: MatrixConversationRouteResource[];
  allowedDirectUsers: string[];
  roomInvocationGate: "bot_mention_or_command_required";
  configuredCommands: string[];
  encryptedRoomPolicy: "unsupported";
  validationState: "valid" | "partial" | "stale" | "blocked" | "missing_permission";
  reasonCode?: string;
  validatedAt: string;
  redactionStatus: MatrixRedactionStatus;
  safeEvidence?: Record<string, string>;
};

export type MatrixHostedSetupResource = {
  tenantId?: string;
  connectorId: string;
  connectorKind: "matrix";
  displayName: string;
  status: "configured" | "healthy" | "degraded" | "failed" | "permission_blocked" | "rate_limited" | "unsupported_capability" | "disabled";
  terminalState: MatrixTerminalState;
  botCredentialState: MatrixCredentialState;
  homeserverState: "reachable" | "unreachable" | "unsupported" | "rate_limited" | "federation_failed" | "network_failed" | "unknown";
  routePolicyState: "none" | "partial" | "valid" | "stale" | "blocked";
  deliveryEligible: boolean;
  homeserverBindingId: string;
  homeserverBinding?: MatrixHomeserverBindingResource;
  routePolicy?: MatrixRoutePolicyResource;
  diagnostic?: {
    reasonCode?: string;
    matrixCondition?: string;
    remediationOwner?: string;
    freshnessState?: string;
  };
  reasonCode?: string;
  redactionStatus: MatrixRedactionStatus;
  createdAt: string;
  updatedAt: string;
  validatedAt?: string;
  retentionExpiresAt: string;
};

export type MatrixCredentialState = DiscordCredentialState;

export type MatrixSmokeEvidenceResource = {
  smokeEvidenceId: string;
  tenantId?: string;
  connectorId: string;
  homeserverBindingId: string;
  status: "passed" | "failed" | "skipped";
  authorizationMode: "safe_live" | "fake_matrix" | "unavailable";
  owner: string;
  reason: string;
  remainingRisk?: string;
  validatedAt: string;
  retentionExpiresAt: string;
  redactionStatus: MatrixRedactionStatus;
  safeEvidence?: Record<string, string>;
};

export type ConnectorConformanceResultResource = {
  conformanceResultId: string;
  tenantId?: string;
  connectorKind: string;
  connectorId?: string;
  scenarioId: string;
  area: string;
  result: "pass" | "fail" | "supported" | "limited" | "unsupported";
  reasonCode?: string;
  redactionStatus: DiscordRedactionStatus;
  evidenceTimestamp: string;
  retentionExpiresAt: string;
};

export type DiscordConformanceEvidenceResponse = {
  tenantId: string;
  connectorId: string;
  items: ConnectorConformanceResultResource[];
};
export type TelegramConformanceEvidenceResponse = DiscordConformanceEvidenceResponse;
export type SlackConformanceEvidenceResponse = DiscordConformanceEvidenceResponse;
export type MatrixConformanceEvidenceResponse = DiscordConformanceEvidenceResponse;

export type DiscordHostedReadinessProjection = {
  tenantId?: string;
  connectorId: string;
  displayName: string;
  deliveryMode: "gateway";
  readinessState: DiscordReadinessState;
  hostedReady: boolean;
  localCompatible: boolean;
  reasonCode?: string;
  requireMention: boolean;
  respondInDM: boolean;
  allowedGuildIds?: string[];
  allowedChannelIds?: string[];
  botTokenConfigured: boolean;
  redactionStatus: DiscordRedactionStatus;
};

export type TelegramHostedReadinessProjection = {
  tenantId?: string;
  connectorId: string;
  displayName: string;
  terminalState: TelegramTerminalState;
  hostedReady: boolean;
  localCompatible: boolean;
  reasonCode?: string;
  botTokenConfigured: boolean;
  botUsername?: string;
  allowedUserIds?: string[];
  allowedDirectChatIds?: string[];
  allowedGroupIds?: string[];
  redactionStatus: TelegramRedactionStatus;
};

export type ConfigDiscordConnectorResponse = {
  enabled: boolean;
  configured: boolean;
  connectorId: string;
  displayName: string;
  deliveryMode: "gateway";
  requireMention: boolean;
  respondInDM: boolean;
  allowedGuildIds: string[];
  allowedChannelIds: string[];
  botTokenConfigured: boolean;
  botTokenEnv?: string;
  hostedReadiness: DiscordHostedReadinessProjection;
};

export type ConfigTelegramConnectorResponse = {
  enabled: boolean;
  configured: boolean;
  connectorId: string;
  displayName: string;
  botTokenConfigured: boolean;
  botTokenEnv?: string;
  botUsername?: string;
  allowedUserIds: string[];
  allowedDirectChatIds: string[];
  allowedGroupIds: string[];
  hostedReadiness: TelegramHostedReadinessProjection;
};

export type ConfigSlackConnectorResponse = {
  enabled: boolean;
  configured: boolean;
  connectorId: string;
  displayName: string;
  apiBaseURL?: string;
  botTokenSecretRef?: string;
  workspaceId?: string;
  workspaceBindingId?: string;
  botUserId?: string;
  allowedChannelIds: string[];
  allowedDMUserIds: string[];
  allowedDMUserGroups: string[];
  hostedReadiness: {
    tenantId?: string;
    connectorId: string;
    displayName: string;
    terminalState: SlackTerminalState;
    hostedReady: boolean;
    localCompatible: boolean;
    reasonCode?: string;
    workspaceBindingId?: string;
    workspaceId?: string;
    botUserId?: string;
    allowedChannelIds?: string[];
    allowedDMUserIds?: string[];
    allowedDMUserGroups?: string[];
    redactionStatus: SlackRedactionStatus;
  };
};

export type MatrixHostedReadinessProjection = {
  tenantId?: string;
  connectorId: string;
  displayName: string;
  terminalState: MatrixTerminalState;
  hostedReady: boolean;
  localCompatible: boolean;
  reasonCode?: string;
  homeserverId?: string;
  homeserverUrl?: string;
  botUserId?: string;
  selectedRoomIds?: string[];
  allowedDirectUserIds?: string[];
  configuredCommands?: string[];
  redactionStatus: MatrixRedactionStatus;
};

export type ConfigMatrixConnectorResponse = {
  enabled: boolean;
  configured: boolean;
  connectorId: string;
  displayName: string;
  homeserverUrl?: string;
  homeserverId?: string;
  botUserId?: string;
  botAccessTokenSet: boolean;
  botAccessTokenEnv?: string;
  selectedRoomIds: string[];
  allowedDirectUserIds: string[];
  configuredCommands: string[];
  hostedReadiness: MatrixHostedReadinessProjection;
};

export type ConfigResponse = {
  environment: DeploymentEnvironment;
  bindAddr: string;
  dataDir: string;
  configFilePath: string;
  logLevel: "debug" | "info" | "warn" | "error";
  version: string;
  llm: Record<string, unknown>;
  connectors: {
    discord: ConfigDiscordConnectorResponse;
    telegram: ConfigTelegramConnectorResponse;
    slack: ConfigSlackConnectorResponse;
    matrix: ConfigMatrixConnectorResponse;
  };
  mcp: Record<string, unknown>;
  sandbox: Record<string, unknown>;
  redactedFields: string[];
};

export type IntegrationDiagnosticResultResource = {
  diagnosticResultId: string;
  tenantId: string;
  integrationId: string;
  integrationAccountId?: string;
  domainKind: string;
  providerKind: string;
  capability: string;
  status: IntegrationDiagnosticStatus;
  reasonCode: IntegrationDiagnosticReasonCode;
  remediationOwner: IntegrationDiagnosticRemediationOwner;
  remediationHint?: string;
  retrySafety: IntegrationDiagnosticRetrySafety;
  checkedAt: string;
  staleAfter: string;
  freshnessState: IntegrationDiagnosticFreshnessState;
  runId?: string;
  redactionStatus: IntegrationDiagnosticRedactionStatus;
  evidenceSummary?: string;
  retentionExpiresAt: string;
  smokeReportId?: string;
  artifactRefs?: string[];
};

export type IntegrationDiagnosticRunResource = {
  diagnosticRunId: string;
  tenantId: string;
  integrationId: string;
  integrationAccountId?: string;
  domainKind?: string;
  providerKind?: string;
  requestedBy: string;
  trigger: string;
  status: "queued" | "running" | "completed" | "failed" | "blocked";
  startedAt: string;
  completedAt?: string;
  checkedCapabilities: string[];
  resultIds: string[];
  failureReasonCode?: IntegrationDiagnosticReasonCode;
  redactionStatus: IntegrationDiagnosticRedactionStatus;
  retentionExpiresAt: string;
  idempotencyKey?: string;
};

export type IntegrationDiagnosticReasonCodeResource = {
  reasonCode: IntegrationDiagnosticReasonCode;
  category: string;
  defaultRetrySafety: IntegrationDiagnosticRetrySafety;
  defaultRemediationOwner: IntegrationDiagnosticRemediationOwner;
  userMessageKey: string;
  operatorMessageKey: string;
  supportedDomains?: string[];
};

export type IntegrationDiagnosticListResponse = {
  integrationId?: string;
  tenantId?: string;
  freshnessSummary?: string;
  items: IntegrationDiagnosticResultResource[];
  nextCursor?: string;
};

export type CreateIntegrationDiagnosticRunInput = {
  capabilities?: string[];
  forceRefresh?: boolean;
  clientKey: string;
  reason?: string;
};

export type SmokeProbeResult = "passed" | "failed" | "blocked" | "skipped";
export type SmokeReportStatus = "draft" | "running" | "completed" | "blocked" | "failed" | "published";

export type SmokeProbeOutcomeResource = {
  probeOutcomeId: string;
  tenantId: string;
  smokeReportId: string;
  integrationId: string;
  integrationAccountId?: string;
  domainKind: string;
  providerKind: string;
  probeAction: string;
  result: SmokeProbeResult;
  reasonCode: IntegrationDiagnosticReasonCode;
  remediationHint: string;
  retrySafety: IntegrationDiagnosticRetrySafety;
  blockedOrSkippedReason?: string;
  approvalRefs?: string[];
  artifactRefs?: string[];
  checkedAt: string;
  redactionStatus: IntegrationDiagnosticRedactionStatus;
  retentionExpiresAt: string;
};

export type SmokeMatrixReportResource = {
  smokeReportId: string;
  tenantId: string;
  reportKind: string;
  requestedBy: string;
  status: SmokeReportStatus;
  domainSummary: Record<string, string>;
  startedAt: string;
  completedAt?: string;
  publishedAt?: string;
  artifactRefs: string[];
  retentionExpiresAt: string;
  probeOutcomes?: SmokeProbeOutcomeResource[];
};

export type CreateIntegrationDiagnosticSmokeInput = {
  reportId?: string;
  integrationId: string;
  probes?: Array<{
    integrationId?: string;
    domainKind?: string;
    probeAction: string;
    safeCredentialsAvailable: boolean;
    tenantApprovalAvailable: boolean;
    providerAvailable: boolean;
    supported: boolean;
    readOnlyOrReversible: boolean;
    tenantAdminApproved?: boolean;
    operatorApproved?: boolean;
    operatorDeferred?: boolean;
    reasonCode?: IntegrationDiagnosticReasonCode;
    providerEvidence?: Record<string, unknown>;
    artifactRefs?: string[];
  }>;
};

export type DiagnosticRetentionRecordResource = {
  retentionRecordId: string;
  tenantId: string;
  targetKind: string;
  targetId: string;
  policyRef?: string;
  defaultExpiresAt: string;
  effectiveExpiresAt: string;
  retentionState: "active" | "expired" | "purged";
  appliedAt?: string;
  createdAt: string;
  updatedAt: string;
};

export type TenantResource = {
  tenantId: string;
  tenantKind: TenantKind;
  displayName: string;
  status: TenantLifecycleStatus;
  createdAt: string;
  updatedAt: string;
  createdByPrincipalId?: string;
  defaultOwnerPrincipalId?: string;
  callerMembershipRole?: TenantRole;
  callerMembershipStatus?: TenantLifecycleStatus;
  callerPermissions?: TenantPermission[];
  defaultForCurrentToken?: boolean;
  defaultForCurrentPrincipal?: boolean;
};

export type PrincipalResource = {
  principalId: string;
  principalKind: "local_operator" | "user" | "service_client";
  displayName: string;
  status: TenantLifecycleStatus;
  defaultTenantId: string;
  createdAt: string;
  updatedAt: string;
  disabledAt?: string;
  removedAt?: string;
};

export type MembershipResource = {
  membershipId: string;
  tenantId: string;
  principalId: string;
  role: TenantRole;
  status: TenantLifecycleStatus;
  invitationId?: string;
  createdAt: string;
  updatedAt: string;
  acceptedAt?: string;
  removedAt?: string;
};

export type TokenTenantGrantResource = {
  grantId: string;
  tokenId: string;
  tenantId: string;
  isDefault: boolean;
  status: TenantLifecycleStatus;
  createdAt: string;
  updatedAt: string;
  revokedAt?: string;
  grantedByPrincipalId?: string;
};

export type TenantContextResource = {
  principalId: string;
  tokenId: string;
  tenantId: string;
  tenantSource: string;
  membershipId?: string;
  role?: TenantRole;
  permissions: TenantPermission[];
  resolvedAt: string;
};

export type TenantDenialResource = {
  error: string;
  errorCode: string;
  requestId?: string;
};

export type AuthMeResponse = {
  token: Record<string, unknown>;
  principal: PrincipalResource;
  defaultTenant: TenantResource;
  currentTenant: TenantResource;
  allowedTenants: TenantResource[];
  tokenGrants: TokenTenantGrantResource[];
  permissions: TenantPermission[];
  tenantContext: TenantContextResource;
};

export type TenantListResponse = {
  items: TenantResource[];
};

export type TenantDetailResponse = {
  tenant: TenantResource;
  tenantContext: TenantContextResource;
};

export type MembershipListResponse = {
  items: MembershipResource[];
};

export type UpdateMembershipRoleInput = {
  role: TenantRole;
};

export type TenantRequestOptions = {
  tenantId?: string;
};

function isTenantRequestOptions(value: unknown): value is TenantRequestOptions {
  return typeof value === "object" && value !== null && "tenantId" in value;
}

export type ActivationStatus = "not_started" | "in_progress" | "blocked" | "active" | "first_action_completed";

export type ActivationReasonCode =
  | "activation_denied:principal_disabled"
  | "activation_denied:principal_denied"
  | "activation_denied:tenant_access_revoked"
  | "activation_blocked:quota_baseline_unavailable"
  | "activation_blocked:environment_unavailable"
  | "activation_blocked:test_chat_unavailable"
  | "activation_failed:tenant_resolution"
  | "activation_failed:test_chat"
  | "activation_failed:audit_write"
  | "activation_failed:persistence"
  | "activation_failed:unexpected"
  | string;

export type ActivationRemediationOwner = "product_user" | "operator" | "tenant_admin" | "system" | "none_required";

export type ActivationReadinessItem = {
  itemId: string;
  itemKind: "tenant_access" | "environment" | "quota_baseline" | "test_chat" | string;
  status: "ready" | "blocked" | "degraded" | "missing_configuration" | "optional";
  reasonCode?: ActivationReasonCode;
  displayName?: string;
  requiredForActivation: boolean;
  retryable: boolean;
  remediationOwner: ActivationRemediationOwner;
  updatedAt?: string;
};

export type ActivationQuotaProjection = {
  category?: string;
  unit?: string;
  limit?: number;
  used?: number;
  remaining?: number;
  period?: string;
  metadata?: Record<string, unknown>;
};

export type ActivationQuotaBaseline = {
  tenantId: string;
  planKey: string;
  enforcementMode: BillingEnforcementMode | string;
  status: "available" | "unavailable";
  quotas: ActivationQuotaProjection[];
  projectedAt?: string;
  projectionSource?: string;
  reasonCode?: ActivationReasonCode;
};

export type ActivationFirstAction = {
  actionId: "test_chat" | string;
  actionKind: "test_chat" | string;
  displayName?: string;
  recommended: boolean;
  available: boolean;
  blockingItemIds: string[];
  invokeRoute: string;
  resultRoute: string;
};

export type ActivationTestChatMetadata = {
  activationId?: string;
  tenantId?: string;
  dispatchId?: string;
  status: "completed" | "failed" | "cancelled";
  provider?: string;
  model?: string;
  usage?: Record<string, unknown>;
  finishReason?: string;
  reasonCode?: ActivationReasonCode;
  completedAt?: string;
};

export type ActivationStateResource = {
  activationId: string;
  principalId: string;
  tenantId: string;
  environmentScope: EnvironmentScope;
  status: ActivationStatus;
  currentStepId: string;
  completedStepIds: string[];
  blockingReasonCodes: ActivationReasonCode[];
  readinessItems: ActivationReadinessItem[];
  quotaBaseline?: ActivationQuotaBaseline;
  firstAction: ActivationFirstAction;
  testChat?: ActivationTestChatMetadata;
  failureReason?: {
    reasonCode: ActivationReasonCode;
    stage: ActivationDiagnostic["stage"];
    retryable: boolean;
    remediationOwner: ActivationRemediationOwner;
    message?: string;
  };
  createdAt?: string;
  updatedAt?: string;
  firstActionCompletedAt?: string;
  lastEvaluatedAt: string;
};

export type ActivationResponse = {
  activation: ActivationStateResource;
};

export type RunActivationInput = {
  source?: "signup" | "invite_acceptance" | "returning_user" | "operator_retry" | string;
};

export type RunActivationTestChatInput = {
  message: string;
};

export type ActivationTestChatResponse = ActivationResponse & {
  testChat: ActivationTestChatMetadata;
};

export type ActivationDiagnostic = {
  activationId: string;
  tenantId?: string;
  principalId: string;
  status: ActivationStatus;
  stage: "tenant_resolution" | "eligibility" | "quota_baseline" | "authorization" | "test_chat" | "audit" | "persistence" | "unexpected" | string;
  reasonCode: ActivationReasonCode;
  retryable: boolean;
  remediationOwner: ActivationRemediationOwner;
  lastTransitionAt: string;
  readinessItemIds?: string[];
  quotaBaselineStatus?: "available" | "unavailable";
  testChat?: ActivationTestChatMetadata;
};

export type ActivationDiagnosticListResponse = {
  items: ActivationDiagnostic[];
};

export type ActivationFailurePayload = {
  code?: ActivationReasonCode;
  reasonCode: ActivationReasonCode;
  stage: ActivationDiagnostic["stage"];
  retryable: boolean;
  remediationOwner: ActivationRemediationOwner;
};

export type SetupState = "not_started" | "in_progress" | "ready" | "degraded" | "unavailable" | "cancelled" | "action_required" | "disabled";
export type SetupStyle = "submitted_secret" | "oauth" | "unsupported";
export type SetupTargetKind = "provider" | "integration" | "channel" | "connector";
export type SetupSupportStatus = "supported" | "unsupported" | "action_required";
export type SetupSafeUseMode = "normal" | "limited_safe" | "blocked";
export type SetupRemediationOwner = "product_user" | "tenant_admin" | "operator" | "provider" | "none_required";
export type SetupRedactionStatus = "redacted" | "suppressed" | "failed_closed";
export type SetupRetrySafety = "no_action_needed" | "retryable" | "blocked" | "unsafe_to_retry";
export type SetupOAuthResult = "completed" | "denied" | "abandoned" | "expired" | "replay" | "tenant_mismatch" | "provider_error";

export type SetupResourceRef = {
  kind: string;
  id: string;
  route?: string;
};

export type SetupTargetResource = {
  targetId: string;
  tenantId?: string;
  targetKind: SetupTargetKind;
  setupStyle: SetupStyle;
  displayName: string;
  proofTarget: boolean;
  supportStatus: SetupSupportStatus;
  requiredPermissions?: TenantPermission[];
  limitedSafeCapabilities?: string[];
  currentSessionId?: string;
  currentState?: SetupState;
  diagnosticResultId?: string;
};

export type SetupSessionResource = {
  setupSessionId: string;
  tenantId?: string;
  actorPrincipalId?: string;
  targetId: string;
  targetKind: SetupTargetKind;
  setupStyle: SetupStyle;
  state: SetupState;
  reasonCode?: string;
  retryable: boolean;
  remediationOwner: SetupRemediationOwner;
  safeUseMode: SetupSafeUseMode;
  allowedCapabilities: string[];
  currentAttemptId?: string;
  diagnosticResultId?: string;
  diagnosticRunId?: string;
  diagnosticStage?: string;
  diagnosticSourceKind?: string;
  diagnosticSourceId?: string;
  diagnosticAllowedCapabilities?: string[];
  redactionStatus: SetupRedactionStatus;
  resourceRefs?: SetupResourceRef[];
  redactedEvidence?: Record<string, string>;
  oauthStateRef?: string;
  createdAt: string;
  updatedAt: string;
  lastTransitionAt: string;
  lastTransitionAuditEventId?: string;
  operatorRemediation?: string;
  userRemediation?: string;
  unsupportedReasonCode?: string;
};

export type SetupDiagnosticResource = {
  setupSessionId: string;
  targetId: string;
  diagnosticResultId: string;
  diagnosticRunId?: string;
  diagnosticStage?: string;
  diagnosticSourceKind?: string;
  diagnosticSourceId?: string;
  status: SetupState;
  reasonCode: string;
  retrySafety: SetupRetrySafety;
  remediationOwner: SetupRemediationOwner;
  allowedCapabilities?: string[];
  checkedAt: string;
  staleAfter: string;
  redactionStatus: SetupRedactionStatus;
};

export type SetupTargetListResponse = {
  items: SetupTargetResource[];
};

export type SetupSessionListResponse = {
  items: SetupSessionResource[];
};

export type SetupSessionResponse = {
  session: SetupSessionResource;
};

export type StartSetupInput = {
  targetId: string;
  setupStyle: SetupStyle;
  source?: string;
};

export type SubmitSetupSecretInput = {
  secretRef: string;
  value: string;
  displayName?: string;
  resourceRefs?: SetupResourceRef[];
};

export type StartSetupOAuthInput = {
  redirectRoute?: string;
};

export type SetupOAuthStartResponse = SetupSessionResponse & {
  authorizationUrl: string;
  state: string;
};

export type CompleteSetupOAuthInput = {
  state: string;
  result: SetupOAuthResult;
  accountLabel?: string;
  code?: string;
  redirectUri?: string;
};

export type DisableSetupInput = {
  disabledReason?: string;
};

export type ReplaceSetupInput = Record<string, never>;

export type SetupDiagnosticListResponse = {
  items: SetupDiagnosticResource[];
};

export type BillingEnforcementMode = "enforced" | "unlimited" | "not_measurable";
export type BillingQuotaStatus = "available" | "near_limit" | "exhausted" | "unlimited" | "not_measurable" | "restricted" | "unavailable";
export type BillingNearLimitReason = "percent_threshold" | "below_one_typical_operation";
export type BillingRecoveryAction = "wait" | "reduce_scope" | "request_override" | "contact_support" | "operator_resolution_required" | "retry_later";
export type BillingDenialClassification = "quota_exhaustion" | "abuse_restriction" | "quota_state_unavailable" | "unauthorized" | "operator_action_needed";

export type BillingPlanResource = {
  planId: string;
  tenantId?: string;
  planKey: string;
  status: "active" | "scheduled" | "disabled" | "superseded" | string;
  enforcementMode: BillingEnforcementMode;
  effectiveAt: string;
  supersededAt?: string;
  assignedByPrincipalId?: string;
  assignmentReason?: string;
  document?: Record<string, unknown>;
};

export type BillingQuotaResource = {
  tenantId?: string;
  planKey: string;
  category: string;
  unit: string;
  periodStart: string;
  periodEnd: string;
  periodAnchor?: string;
  limit: number;
  consumedAmount: number;
  reservedAmount: number;
  adjustedAmount: number;
  carryoverApplied: number;
  remainingAmount: number;
  enforcementMode: BillingEnforcementMode;
  denialReasonCode?: string;
  overLimit?: boolean;
};

export type BillingDenialResource = {
  denialId: string;
  tenantId?: string;
  category?: string;
  quotaPeriodId?: string;
  operationKey: string;
  reasonCode: string;
  requestedAmount: number;
  remainingAmount: number;
  guardedEntryPoint: string;
  createdAt: string;
};

export type BillingManualAdjustmentResource = {
  adjustmentId: string;
  tenantId?: string;
  category: string;
  quotaPeriodId: string;
  amountDelta: number;
  reason: string;
  createdByPrincipalId?: string;
  createdAt: string;
};

export type BillingUsageResponse = {
  tenantId?: string;
  planKey: string;
  enforcementMode: BillingEnforcementMode;
  quotas: BillingQuotaResource[];
  manualAdjustments?: BillingManualAdjustmentResource[];
  denials?: BillingDenialResource[];
};

export type BillingUsagePeriodSummary = {
  periodStart: string;
  periodEnd: string;
  periodAnchor: string;
  consumedAmount: number;
  reservedAmount: number;
  adjustedAmount: number;
  carryoverApplied: number;
  remainingAmount: number;
  overLimit?: boolean;
};

export type BillingQuotaOverrideSummary = {
  baseLimit: number;
  effectiveLimit: number;
  reason?: string;
  effectiveAt?: string;
  expiresAt?: string;
};

export type BillingAbuseRestrictionSummary = {
  restrictionId?: string;
  status: "active" | "expired" | string;
  affectedCategory?: string;
  recoveryAction?: BillingRecoveryAction | string;
  visibleReasonCode?: string;
  sourceAuditRef?: string;
  supportContactAllowed?: boolean;
  startedAt?: string;
  expiresAt?: string;
};

export type BillingQuotaStatusItem = {
  category: string;
  unit: "count" | "bytes" | "attempts" | string;
  status: BillingQuotaStatus;
  currentPeriod: BillingUsagePeriodSummary;
  previousPeriod?: BillingUsagePeriodSummary;
  limit: number;
  remainingAmount: number;
  nearLimit: boolean;
  nearLimitReason?: BillingNearLimitReason;
  typicalOperationAmount: number;
  baseLimit?: number;
  effectiveLimit?: number;
  override?: BillingQuotaOverrideSummary;
  restriction?: BillingAbuseRestrictionSummary;
  recoveryActions: BillingRecoveryAction[];
};

export type BillingQuotaSection = {
  sectionKey: string;
  label: string;
  items: BillingQuotaStatusItem[];
};

export type BillingQuotaDashboardResponse = {
  tenantId: string;
  plan: {
    planKey: string;
    enforcementMode: BillingEnforcementMode;
    status?: string;
    effectiveAt: string;
    basePlanLabel?: string;
    checkoutAvailable: boolean;
  };
  sections: BillingQuotaSection[];
  generatedAt: string;
  permission?: { allowed: boolean; reasonCode?: string };
};

export type BillingDenialDetailResponse = {
  denialId: string;
  tenantId: string;
  operationRef: string;
  operationKey: string;
  guardedEntryPoint: string;
  category?: string;
  reasonCode: string;
  classification: BillingDenialClassification;
  requestedAmount: number;
  remainingAmount: number;
  recoveryActions: BillingRecoveryAction[];
  restriction?: BillingAbuseRestrictionSummary;
  createdAt: string;
};

export type BillingEvidenceRedaction = {
  path: string;
  reason: string;
  replacement: string;
};

export type BillingEvidenceExportResponse = {
  schemaVersion: string;
  exportId: string;
  tenantId: string;
  generatedAt: string;
  generatedByPrincipalId: string;
  denial: BillingDenialDetailResponse;
  usageSnapshot: BillingQuotaStatusItem[];
  effectiveLimitState: Record<string, unknown>;
  auditRefs: string[];
  redactions: BillingEvidenceRedaction[];
};

export type BillingPlanAssignmentInput = {
  planKey: string;
  enforcementMode: BillingEnforcementMode;
  reason: string;
};

export type BillingQuotaOverrideInput = {
  category: string;
  limit?: number;
  reason: string;
};

export type BillingManualAdjustmentInput = {
  category: string;
  quotaPeriodId: string;
  amountDelta: number;
  reason: string;
};

export type BillingReservationResource = {
  reservationId: string;
  tenantId?: string;
  category: string;
  quotaPeriodId: string;
  operationKey: string;
  amountReserved: number;
  amountCommitted?: number;
  amountRefunded?: number;
  status: string;
  reservationPoint?: string;
  commitPoint?: string;
  refundPoint?: string;
  createdAt: string;
  updatedAt: string;
  expiresAt?: string;
  recoveryReason?: string;
};

export type BillingReservationResolutionInput = {
  outcome: "committed" | "released" | "refunded" | "operator_action_needed";
  reason: string;
  amount?: number;
};

export type BillingQuotaDenialPayload = {
  code: "quota_denied";
  reasonCode: string;
  tenantId?: string;
  category?: string;
  operationKey?: string;
  requestedAmount?: number;
  remainingAmount?: number;
  periodStart?: string;
  periodEnd?: string;
};

export type SecretStatus = "active" | "disabled" | "pending_remediation";
export type SecretResolutionStatus = "resolved" | "unavailable" | "denied" | "not_applicable";

export type RedactedSecretSummary = {
  secretRef?: string;
  secretVersionId?: string;
  resolution?: SecretResolutionStatus;
  status?: SecretStatus;
  disabledReason?: string;
  redactionRule?: string;
};

export type TenantSecretResource = {
  secretId: string;
  tenantId: string;
  secretRef: string;
  displayName?: string;
  status: SecretStatus;
  activeVersionId?: string;
  disabledReason?: string;
  remediationReason?: string;
  createdAt: string;
  updatedAt: string;
  rotatedAt?: string;
  disabledAt?: string;
  document?: Record<string, unknown>;
  secretRefs?: RedactedSecretSummary[];
};

export type TenantSecretListResponse = {
  items: TenantSecretResource[];
};

export type CreateTenantSecretInput = {
  secretRef: string;
  displayName?: string;
  value: string;
  document?: Record<string, unknown>;
};

export type UpdateTenantSecretInput = {
  displayName?: string;
  document?: Record<string, unknown>;
};

export type RotateTenantSecretInput = {
  value: string;
};

export type DisableTenantSecretInput = {
  disabledReason: string;
};

export type TenantSecretResponse = {
  secret: TenantSecretResource;
};

export type ChannelManagementState = "ready" | "disabled" | "degraded" | "unavailable" | "action-required";
export type ChannelManagementCapabilitySupport = "supported" | "limited" | "unsupported";
export type ChannelManagementActionKind = "repair" | "reconnect" | "credential-rotation" | "route-revalidate" | "diagnostic-rerun" | "disable";

export type ChannelConnectorResource = {
  connectorId: string;
  connectorKind: string;
  displayName: string;
  enablementState: ChannelManagementState;
  setupState: string;
  healthStatus: string;
  diagnosticFreshness: "fresh" | "stale";
  deliveryEligible: boolean;
  nextAction?: {
    actionKind: ChannelManagementActionKind | "re-enable";
    label: string;
    reasonCode?: string;
    remediationOwner?: string;
  };
  capabilities: Record<string, ChannelManagementCapabilitySupport>;
  redactionStatus: "redacted" | "suppressed" | "redaction_failed";
  updatedAt: string;
};

export type ChannelConnectorListResponse = {
  tenantId?: string;
  page: {
    limit: number;
    nextCursor?: string;
    order: "attention_disabled_ready_name_id";
  };
  items: ChannelConnectorResource[];
};

export type ChannelConnectorDetailResource = ChannelConnectorResource & {
  diagnosticSummary?: ConnectorDiagnosticStateResource;
  routePolicy?: ChannelRoutePolicyResource;
  recentRouteDecisions?: unknown[];
  foregroundReplyOutcomes?: unknown[];
  backgroundDeliveryOutcomes?: unknown[];
  repairActions?: ChannelRepairActionResource[];
  supportEvidenceAvailable?: boolean;
  retention?: Record<string, string>;
};

export type ConnectorDiagnosticStateResource = {
  diagnosticStateId: string;
  tenantId?: string;
  connectorId: string;
  status: string;
  reasonCode: string;
  remediationOwner: string;
  retrySafety: string;
  evidenceTimestamp: string;
  freshnessState: "fresh" | "stale";
  redactionStatus: "redacted" | "suppressed" | "redaction_failed";
  retentionExpiresAt: string;
  safeEvidence?: Record<string, string>;
};

export type ChannelRoutePolicyResource = {
  routePolicyId?: string;
  tenantId: string;
  connectorId: string;
  eligibleSenders?: string[];
  eligibleConversations?: string[];
  eligibleRooms?: string[];
  eligibleChannels?: string[];
  invocationGates?: string[];
  backgroundDeliveryEligible: boolean;
  validationState: string;
  reasonCode?: string;
  validatedAt: string;
  auditEventId?: string;
  redactionStatus: "redacted" | "suppressed" | "redaction_failed";
};

export type ChannelManagementActionInput = {
  reasonCode?: string;
  note?: string;
  actionKind?: ChannelManagementActionKind;
  sourceDiagnosticStateId?: string;
  eligibleSenders?: string[];
  eligibleConversations?: string[];
  eligibleRooms?: string[];
  eligibleChannels?: string[];
  invocationGates?: string[];
  backgroundDeliveryEligible?: boolean;
};

export type ChannelEnablementMutationResponse = {
  connectorId: string;
  enablementState: ChannelManagementState;
  deliveryEligible: boolean;
  auditEventId: string;
  changedAt: string;
};

export type ChannelRepairActionResource = {
  repairActionId: string;
  tenantId: string;
  connectorId: string;
  connectorKind: string;
  actionKind: ChannelManagementActionKind;
  sourceDiagnosticStateId?: string;
  setupSessionId?: string;
  status: "ready" | "degraded" | "unavailable" | "disabled" | "cancelled" | "action-required";
  retrySafety?: string;
  remediationOwner?: string;
  startedAt: string;
  completedAt?: string;
  auditEventId: string;
  redactionStatus: "redacted" | "suppressed" | "redaction_failed";
};

export type ChannelManagementSupportEvidence = {
  supportEvidenceId: string;
  tenantId: string;
  connectorId: string;
  generatedByPrincipalId?: string;
  generatedAt: string;
  currentState: ChannelManagementState;
  stateTransitions?: string[];
  diagnosticRefs?: string[];
  repairRefs?: string[];
  routingDecisionRefs?: string[];
  replyOutcomeRefs?: string[];
  deliveryOutcomeRefs?: string[];
  auditRefs?: string[];
  redactions?: string[];
  retentionExpiresAt: string;
  redactionStatus: "redacted" | "suppressed" | "redaction_failed";
  safeEvidence?: Record<string, string>;
};

export type ThreadLifecycleState = "active" | "reset" | "archived" | "reopened";
export type ThreadSourceKind = "chat" | "channel" | "workflow" | "schedule" | "shell" | "legacy";
export type ThreadLifecycleActionKind = "reset" | "archive" | "reopen";
export type ThreadRedactionStatus = "redacted" | "suppressed" | "redaction_failed";

// Thread lifecycle responses are inspection metadata only. They do not model
// assistant memory recall, semantic summaries, context packing, or pruning.
export type ThreadResource = {
  threadId: string;
  tenantId: string;
  lifecycleState: ThreadLifecycleState;
  sourceKind: ThreadSourceKind;
  sourceSummary?: string;
  currentSessionSegmentId?: string;
  currentSessionId?: string;
  lastActivityAt: string;
  availableActions: ThreadLifecycleActionKind[];
  redactionStatus: ThreadRedactionStatus;
  retentionExpiresAt?: string;
  updatedAt: string;
};

export type ThreadListResponse = {
  tenantId: string;
  page: {
    limit: number;
    nextCursor?: string;
    order: string;
  };
  items: ThreadResource[];
};

export type ThreadSourceLinkage = {
  sourceLinkageId: string;
  sourceKind: ThreadSourceKind;
  connectorId?: string;
  connectorKind?: string;
  sourceAccountId?: string;
  sourceConversationId?: string;
  sourceMessageId?: string;
  routingOutcome:
    | "accepted"
    | "ignored"
    | "blocked"
    | "duplicate"
    | "disabled"
    | "unsupported"
    | "failed"
    | "unknown_source"
    | "stale_source"
    | "inaccessible_tenant_binding";
  current: boolean;
  linkedAt?: string;
  retentionExpiresAt?: string;
  redactionStatus: ThreadRedactionStatus;
};

export type ThreadRuntimeProjection = {
  runtimeProjectionId: string;
  resourceKind: "session" | "run" | "workflow" | "approval" | "foreground_reply" | "background_delivery" | "connector_message";
  resourceId: string;
  status: string;
  reasonCode?: string;
  occurredAt: string;
  route?: string;
  safeSummary?: string;
  retentionExpiresAt?: string;
  redactionStatus: ThreadRedactionStatus;
};

export type ThreadContinuityPreview = {
  continuityPreviewId: string;
  tenantId?: string;
  threadId?: string;
  sessionSegmentId?: string;
  continuityApplied: boolean;
  status: "applied" | "empty" | "disabled" | "blocked" | "partial" | "failed";
  includedCount: number;
  excludedCount: number;
  windowPolicyId?: string;
  maxPriorTurns?: number;
  activeWindowDays?: number;
  orderedBy?: string;
  redactionStatus?: ThreadRedactionStatus;
  retentionExpiresAt?: string;
};

export type ThreadContinuityPreviewItem = {
  previewItemId?: string;
  continuityPreviewId?: string;
  tenantId?: string;
  threadId?: string;
  itemKind: "turn" | "artifact_excerpt" | "handoff_source_reference";
  continuityTurnId?: string;
  role?: "user" | "assistant";
  artifactRef?: string;
  artifactExcerptId?: string;
  handoffSourceReferenceId?: string;
  decision: "included" | "excluded";
  reasonCode: string;
  acceptanceSequence?: number;
  sourceTimestamp?: string;
  safeSummary?: string;
  redactionStatus: ThreadRedactionStatus;
  itemOrder?: number;
};

export type ThreadContinuityPreviewDetail = {
  preview: ThreadContinuityPreview;
  items: ThreadContinuityPreviewItem[];
};

export type ThreadConversationShape = {
  conversationShapeId?: string;
  tenantId?: string;
  threadId?: string;
  sessionSegmentId?: string;
  shape: "direct_message" | "group" | "room" | "web" | "unknown" | "unsupported";
  sourceKind?: ThreadSourceKind;
  connectorId?: string;
  connectorKind?: string;
  sourceAccountId?: string;
  sourceConversationId?: string;
  sourceConversationSummary?: string;
  participantSummary?: string;
  shapeEvidenceStatus: "proven" | "partial" | "unsupported" | "failed";
  recordedAt?: string;
  updatedAt?: string;
  retentionExpiresAt?: string;
  redactionStatus: ThreadRedactionStatus;
};

export type ThreadParticipationDecision = {
  participationDecisionId?: string;
  tenantId?: string;
  threadId?: string;
  sessionSegmentId?: string;
  conversationShape: "group" | "room" | "unknown" | "unsupported";
  decision: "accepted" | "ignored" | "blocked" | "denied" | "duplicate" | "unsupported" | "failed";
  reasonCode: string;
  createdAssistantWork: boolean;
  safeSummary?: string;
  redactionStatus: ThreadRedactionStatus;
};

export type ThreadResetEvent = {
  resetEventId?: string;
  tenantId?: string;
  threadId?: string;
  conversationShape: ThreadConversationShape["shape"];
  sourceConversationId?: string;
  actorPrincipalId?: string;
  permissionGate: "connectors.manage";
  priorSessionSegmentId?: string;
  resultingSessionSegmentId?: string;
  status: "succeeded" | "denied" | "failed_closed" | "unsupported";
  reasonCode: string;
  requestedAt?: string;
  completedAt?: string;
  auditEventId?: string;
  retentionExpiresAt?: string;
  redactionStatus: ThreadRedactionStatus;
};

export type ThreadHandoffLink = {
  handoffLinkId?: string;
  sourceThreadId: string;
  destinationThreadId: string;
  sourceConversationShape: ThreadConversationShape["shape"];
  destinationConversationShape: ThreadConversationShape["shape"];
  status: "succeeded" | "denied" | "failed_closed" | "unsupported" | "expired";
  sourceReferenceStatus: "available" | "consumed" | "blocked" | "expired" | "none";
  permissionGate: "connectors.manage";
  redactionStatus: ThreadRedactionStatus;
};

export type ThreadHandoffDestinationInput =
  | { surface: "web"; connectorId?: null; sourceAccountId?: null; sourceConversationId?: null; conversationShape?: "web" }
  | { surface: "channel"; connectorId: string; sourceAccountId: string; sourceConversationId: string; conversationShape: "direct_message" | "group" | "room" };

export type ThreadHandoffInput = {
  destination: ThreadHandoffDestinationInput;
  reasonCode?: string;
};

export type ThreadDetailResponse = {
  thread: ThreadResource;
  sessionSegments: Record<string, unknown>[];
  sourceLinkages: ThreadSourceLinkage[];
  runtimeProjections: ThreadRuntimeProjection[];
  activeProfileProjection?: AgentProfileRuntimeProjection;
  continuityPreviews?: ThreadContinuityPreview[];
  lifecycleActions: Record<string, unknown>[];
  conversationShape?: ThreadConversationShape;
  participationDecisions?: ThreadParticipationDecision[];
  resetEvents?: ThreadResetEvent[];
  handoffLinks?: ThreadHandoffLink[];
  /** Additive runtime binding evidence (FR-013); present only for callers with bindings.inspect. */
  bindingProjection?: BindingRuntimeEvidenceResource;
};

export type ThreadListQuery = {
  limit?: number;
  cursor?: string;
  state?: ThreadLifecycleState;
  sourceKind?: ThreadSourceKind;
};

export type ThreadLifecycleActionInput = {
  reasonCode?: string;
  note?: string;
};

export type ThreadLifecycleActionResponse = {
  threadId: string;
  lifecycleState: ThreadLifecycleState;
  previousSessionSegmentId?: string;
  currentSessionSegmentId?: string;
  auditEventId: string;
  changedAt: string;
  action: ThreadLifecycleActionKind;
  availableActions: ThreadLifecycleActionKind[];
};

export type AgentProfileStatus = "draft" | "active" | "archived" | "disabled";
export type AgentProfileRedactionStatus = "redacted" | "suppressed" | "redaction_failed";
export type AgentProfileOverlayValidationState = "valid" | "partial" | "missing" | "permission_denied" | "out_of_scope" | "too_large" | "unsafe_content" | "redaction_failed";

export type AgentProfileDisplayIdentity = {
  name?: string;
  description?: string;
  safeSummary?: string;
};

export type AgentProfilePersona = {
  tone?: string;
  instructions?: string;
  safeSummary?: string;
};

export type AgentProfileProviderPreference = {
  providerId?: string;
  model?: string;
  reasoningLevel?: string;
  validationState?: AgentProfileOverlayValidationState;
  failureReasonCode?: string;
};

export type AgentProfileSafetyDefaults = {
  approvalPosture?: string;
  riskTolerance?: string;
  validationState?: AgentProfileOverlayValidationState;
  failureReasonCode?: string;
};

export type AgentProfileOverlayReference = {
  overlayReferenceId: string;
  profileId: string;
  profileVersionId?: string;
  tenantId: string;
  referenceKind: string;
  scope: string;
  referenceUri: string;
  safeDisplayLabel: string;
  validationState: AgentProfileOverlayValidationState;
  failureReasonCode?: string;
  lastValidatedAt?: string;
  createdAt: string;
  updatedAt: string;
  redactionStatus: AgentProfileRedactionStatus;
};

export type AgentProfileResource = {
  profileId: string;
  tenantId: string;
  displayName: string;
  displayIdentity: AgentProfileDisplayIdentity;
  persona: AgentProfilePersona;
  defaultProviderPreference: AgentProfileProviderPreference;
  safetyDefaults: AgentProfileSafetyDefaults;
  status: AgentProfileStatus;
  activeVersionId?: string;
  tenantDefault: boolean;
  overlayReferenceCount: number;
  createdAt: string;
  updatedAt: string;
  archivedAt?: string;
  disabledAt?: string;
  createdByPrincipalId?: string;
  updatedByPrincipalId?: string;
  redactionStatus: AgentProfileRedactionStatus;
};

export type AgentProfileVersionResource = {
  profileVersionId: string;
  profileId: string;
  tenantId: string;
  versionNumber: number;
  sourceVersionId?: string;
  changeKind: "created" | "updated" | "activated" | "rolled_back" | "archived" | "disabled" | "validated";
  changeSummary: string;
  snapshot: AgentProfileResource;
  rollbackEligibility: "eligible" | "invalid_provider" | "invalid_overlay" | "policy_blocked" | "profile_archived" | "profile_disabled" | "redaction_failed";
  actorPrincipalId?: string;
  createdAt: string;
  auditEventId?: string;
  redactionStatus: AgentProfileRedactionStatus;
};

export type AgentProfileActiveSelection = {
  selectionId: string;
  tenantId: string;
  profileId: string;
  profileVersionId: string;
  selectionScope: string;
  selectionReason: "default_seeded" | "user_activated" | "rollback_activated" | "system_fallback";
  selectedByPrincipalId?: string;
  selectedAt: string;
  auditEventId?: string;
  redactionStatus: AgentProfileRedactionStatus;
};

export type AgentProfileRuntimeProjection = {
  runtimeProfileProjectionId: string;
  tenantId: string;
  profileId: string;
  profileVersionId: string;
  selectionId: string;
  resourceKind: "thread" | "session" | "run" | "workflow" | "handoff_destination";
  resourceId: string;
  safeDisplayName: string;
  safeSummary: string;
  configurationScope?: string;
  deferredBindingClassification?: string;
  occurredAt: string;
  retentionExpiresAt?: string;
  redactionStatus: AgentProfileRedactionStatus;
};

export type AgentProfileMutationInput = {
  displayName: string;
  displayIdentity?: AgentProfileDisplayIdentity;
  persona?: AgentProfilePersona;
  defaultProviderPreference?: AgentProfileProviderPreference;
  safetyDefaults?: AgentProfileSafetyDefaults;
  overlayReferences?: Array<{ referenceKind?: string; referenceUri: string; scope?: string }>;
  activate?: boolean;
  reasonCode?: string;
};

export type AgentProfileMutationResult = {
  profile: AgentProfileResource;
  version: AgentProfileVersionResource;
  selection?: AgentProfileActiveSelection;
  auditEventId: string;
};

export type AgentProfileDetailResponse = {
  profile: AgentProfileResource;
  versions: AgentProfileVersionResource[];
  overlayReferences: AgentProfileOverlayReference[];
  auditEvents: Array<Record<string, unknown>>;
};

export type AgentProfileListResponse = {
  tenantId: string;
  page: {
    limit: number;
    nextCursor?: string;
    order: string;
  };
  items: AgentProfileResource[];
};

export type KuraClientOptions = {
  baseURL: string;
  accessToken?: string;
  fetchImpl?: typeof fetch;
  defaultTenantId?: string;
};

// --- Roadmap 58: Workspace and capability binding ---

export type WorkspaceStatus = "active" | "archived" | "disabled";
export type BindingRepairStatus =
  | "healthy"
  | "disabled"
  | "invalid"
  | "stale"
  | "unsupported"
  | "needs_repair";
export type BindingScopeKind = "channel" | "integration_account";
export type CapabilityVisibility = "visible" | "hidden" | "disabled" | "default_enabled";
export type CapabilityVisibilityScope = "profile" | "workspace";

export interface WorkspaceResource {
  workspaceId: string;
  tenantId?: string;
  displayName: string;
  status: WorkspaceStatus;
  isDefault: boolean;
  repairStatus: BindingRepairStatus;
  redactionStatus: string;
  createdAt: string;
  updatedAt: string;
}

export interface WorkspaceListResponse {
  tenantId?: string;
  workspaces: WorkspaceResource[];
}

export interface BindingResource {
  bindingId: string;
  tenantId?: string;
  scopeKind: BindingScopeKind;
  scopeLabel: string;
  selectedProfileId?: string;
  selectedProfileVersionId?: string;
  selectedWorkspaceId?: string;
  status: "active" | "disabled";
  repairStatus: BindingRepairStatus;
  validationStatus: "valid" | "invalid";
  resultingSelectionSummary?: string;
  lastMaterialChangeAt: string;
  redactionStatus: string;
}

export interface BindingListResponse {
  tenantId?: string;
  bindings: BindingResource[];
}

export interface CreateBindingInput {
  scopeKind: BindingScopeKind;
  scopeRef: string;
  selectedProfileId?: string;
  selectedWorkspaceId?: string;
  reasonCode?: string;
}

export interface UpdateBindingInput {
  selectedProfileId?: string;
  selectedWorkspaceId?: string;
  disable?: boolean;
  reasonCode?: string;
}

export interface CapabilityVisibilityResource {
  policyId: string;
  tenantId?: string;
  scopeKind: CapabilityVisibilityScope;
  scopeRef: string;
  capabilityId: string;
  visibility: CapabilityVisibility;
  validationStatus: "valid" | "invalid";
}

export interface CapabilityVisibilityListResponse {
  tenantId?: string;
  policies: CapabilityVisibilityResource[];
}

export interface SetCapabilityVisibilityInput {
  scopeKind: CapabilityVisibilityScope;
  scopeRef: string;
  capabilityId: string;
  visibility: CapabilityVisibility;
  reasonCode?: string;
}

export type BindingRuntimeScope = "channel" | "integration_account" | "tenant_default";
export type BindingRuntimeClassification = "applied_binding" | "default_binding" | "legacy_default";

export interface BindingCapabilityDecisionResource {
  capabilityId: string;
  effective: "visible" | "hidden" | "disabled" | "blocked";
  defaultEnabled?: boolean;
  offered: boolean;
  executable: boolean;
  reason: string;
  scope?: string;
}

/** Runtime binding evidence attached to a run/thread (FR-013). Surfaced as the optional,
 * additive `bindingProjection` on thread detail for callers holding `bindings.inspect`. */
export interface BindingRuntimeEvidenceResource {
  projectionId: string;
  resourceKind: string;
  resourceId: string;
  selectedProfileId?: string;
  selectedProfileVersionId?: string;
  selectedWorkspaceId?: string;
  bindingScope: BindingRuntimeScope;
  bindingId?: string;
  classification: BindingRuntimeClassification;
  selectionReason: string;
  capabilityVisibilitySummary?: BindingCapabilityDecisionResource[];
  occurredAt: string;
  redactionStatus: string;
}

// --- Roadmap 65-69 product surface types (operator shell, Roadmap 70) ---

export type TriageClassification = "urgent" | "needs_reply" | "fyi" | "newsletter" | "blocked" | "unsupported";
export type TriageOutcome = "draft_reply" | "reminder" | "delivery_digest" | "no_action";
export type TriageCondition = { field: "sender" | "subject" | "body" | "recipient"; operator: "contains" | "equals" | "not_contains"; value: string };
export type TriageRule = { ruleId?: string; description?: string; conditions?: TriageCondition[]; classification: TriageClassification; outcome?: TriageOutcome };
export type TriagePolicyResource = { policyId: string; environmentScope?: string; name: string; rules: TriageRule[]; defaultClassification: TriageClassification; createdAt: string; updatedAt: string };
export type CreateTriagePolicyInput = { name: string; rules: TriageRule[]; defaultClassification?: TriageClassification };
export type TriageMessage = { messageId: string; threadId?: string; sender?: string; recipients?: string[]; subject?: string; bodyPreview?: string };
export type TriageDecision = { messageId: string; classification: TriageClassification; matchedRuleId?: string; outcome: TriageOutcome; defaultApplied?: boolean; replayCandidate: boolean; decidedAt: string };
export type TriageRunResource = { runId: string; policyId: string; environmentScope?: string; messageCount: number; decisions: TriageDecision[]; createdAt: string };

export type RoutineTrigger = { kind: "cron" | "once"; cronExpr?: string; timezone?: string; fireAt?: string };
export type RoutineWorkflow = { entrypoint?: string; goal: string };
export type RoutineDefinition = { name: string; trigger: RoutineTrigger; workflow: RoutineWorkflow; approvalExpectation?: string; deliveryPreferenceId?: string; maxRetries?: number };
export type RoutineVersion = { version: number; definition: RoutineDefinition; scheduleId?: string; createdAt: string };
export type RoutineResource = { routineId: string; environmentScope?: string; name: string; state: "active" | "paused" | "cancelled"; currentVersion: number; currentScheduleId?: string; definition: RoutineDefinition; versions: RoutineVersion[]; createdAt: string; updatedAt: string };
export type RoutinePreview = { scheduleKind: string; triggerSummary: string; workflowSummary: string; approvalExpectation: string; deliveryPreferenceId?: string; retrySummary: string };

export type WebhookEndpointResource = { webhookId: string; tenantId?: string; environmentScope?: string; name: string; targetKind: "routine" | "workflow" | "run"; targetRef: string; status: "active" | "disabled"; secretFingerprint: string; secretVersion?: number; createdAt: string; updatedAt: string };
export type CreateWebhookInput = { name: string; targetKind: "routine" | "workflow" | "run"; targetRef: string };
export type WebhookCreateResult = { endpoint: WebhookEndpointResource; secret: string };

export type CatalogRequirement = { key: string; description?: string };
export type CatalogVersion = { version: string; source: string; checksum?: string; requirements?: CatalogRequirement[]; publishedAt: string };
export type CatalogItemResource = { itemId: string; kind: "skill" | "mcp_server" | "capability" | "plugin"; name: string; trustTier: "official" | "verified" | "community" | "untrusted"; permissions?: string[]; versions: CatalogVersion[]; createdAt: string; updatedAt: string };
export type CatalogEnablementResource = { tenantId?: string; itemId: string; state: "enabled" | "disabled"; activeVersion?: string; versionStack?: string[]; history: { action: string; version?: string; actor?: string; reason?: string; occurredAt: string }[]; updatedAt: string };
export type CatalogInspection = { item: CatalogItemResource; enablement: CatalogEnablementResource; unmetRequirements?: CatalogRequirement[]; permissionSatisfied: boolean };

export type ExecutionProfileResource = { profile: { profileId: string; name: string; backendKind: "subprocess" | "docker" | "ssh" | "local_shell"; riskTier: "low" | "medium" | "high"; provides?: string[]; requirements?: string[]; description?: string; createdAt: string }; status: { profileId: string; health: "ready" | "degraded" | "unavailable"; reason?: string; unmetRequirements?: string[]; available: boolean } };
export type ExecutionDenialExplanation = { requiredCapabilities: string[]; eligibleProfiles: string[]; missingCapabilities?: Record<string, string[]>; unavailable?: Record<string, string> };

export type EvidenceScopeKind = "run" | "workflow" | "thread" | "connector" | "provider" | "routine" | "quota_denial" | "time_window";
export type EvidenceScope = { kind: EvidenceScopeKind; ref?: string; windowStart?: string; windowEnd?: string };
export type EvidenceSection = { kind: string; resourceRefs?: string[]; summary?: Record<string, string>; links?: string[] };
export type EvidenceBundleResource = { bundleId: string; tenantId?: string; actor?: string; scope: EvidenceScope; sections?: EvidenceSection[]; redactionStatus: "redacted" | "failed_closed"; createdAt: string; retentionExpiresAt: string };
export type GenerateEvidenceBundleInput = { tenantId?: string; actor?: string; scope: EvidenceScope };

export type LaunchGateWorkload = { name: string; status: "pass" | "fail" | "skip"; owner?: string; reason?: string };
export type LaunchGateEvidenceInput = {
  channels?: unknown[];
  providerSmoke?: unknown[];
  workloads?: LaunchGateWorkload[];
  soakDurationMet?: boolean;
  supportBundleValidated?: boolean;
  redactionValidated?: boolean;
};
export type LaunchGateDecision = { result: "ship" | "no_ship"; reasons?: string[]; nonKnowledgeParityComplete: boolean; gateStatement: string };


// ---------------------------------------------------------------------------
// Plugin assembly (agent pluginization, phase 1)
// ---------------------------------------------------------------------------

export interface PluginStatusResource {
  id: string;
  summary: string;
  /** Where the plugin comes from; "builtin" until out-of-process providers ship. */
  source: string;
  enabled: boolean;
  /** Why the plugin is disabled; absent when enabled. */
  reason?: string;
  provides: string[];
  requires: string[];
}

export interface PluginHookRegistration {
  /** Hook point name, e.g. "chat/pre-dispatch". */
  point: string;
  pluginId: string;
}

/** GET /v1/plugins — the boot-time plugin assembly report. */
export interface PluginsReport {
  plugins: PluginStatusResource[];
  warnings: string[];
  /** Hook-bus registrations made during assembly. */
  hooks?: PluginHookRegistration[];
}

export interface PluginProfileEntry {
  /** false disables the plugin; absent/true inherits the default (enabled). */
  enabled?: boolean;
  /** Plugin-scoped configuration, opaque to the kernel. */
  config?: Record<string, unknown>;
}

/** The on-disk plugin profile (<data_dir>/plugins.json). Boot-time input. */
export interface PluginProfile {
  disabled?: string[];
  entries?: Record<string, PluginProfileEntry>;
}

export interface RetrievalHit {
  assetId: string;
  layer: string;
  title?: string;
  content: string;
  rank: number;
  sourceLinks: { kind: string; id: string; excerpt?: string }[];
  memberAssetIds?: string[];
}

export interface RetrievalQueryResponse {
  hits: RetrievalHit[];
}

export interface PluginProfileUpdateResponse {
  profile: PluginProfile;
  /** Always true: the profile takes effect at the next daemon start. */
  restartRequired: boolean;
}

// ---------------------------------------------------------------------------
// Memory plane (Roadmap 78, spec 058)
// ---------------------------------------------------------------------------

export type MemoryAssetKind = "chat_memory" | "skill" | "wiki" | "code_graph";
export type MemoryLayer = "l0_ref" | "l1" | "l2" | "l3";
export type MemoryVisibility = "private" | "team" | "restricted" | "agent";
export type MemoryAssetStatus = "pending" | "ready" | "superseded" | "revoked" | "expired";
export type MemoryAtomType = "fact" | "preference" | "constraint" | "event" | "decision" | "reference";

export interface MemoryActor {
  kind: "operator" | "agent" | "system";
  id: string;
}

export interface MemorySourceLink {
  kind: "thread" | "run" | "event" | "message" | "asset" | "external";
  id: string;
  excerpt?: string;
}

export interface MemoryAssetResource {
  assetId: string;
  kind: MemoryAssetKind;
  layer: MemoryLayer;
  tenantId?: string;
  owner: MemoryActor;
  visibility: MemoryVisibility;
  status: MemoryAssetStatus;
  version: number;
  supersedesAssetId?: string;
  bindings?: string[];
  atomType?: MemoryAtomType;
  title?: string;
  content?: string;
  memberAssetIds?: string[];
  sourceLinks?: MemorySourceLink[];
  retentionClass?: string;
  createdAt: string;
  updatedAt: string;
  readyAt?: string;
  revokedAt?: string;
  expiresAt?: string;
  statusReason?: string;
}

export interface CreateMemoryAssetInput {
  kind?: MemoryAssetKind;
  layer?: MemoryLayer;
  tenantId?: string;
  owner: MemoryActor;
  visibility?: MemoryVisibility;
  atomType?: MemoryAtomType;
  title?: string;
  content?: string;
  memberAssetIds?: string[];
  sourceLinks?: MemorySourceLink[];
  retentionClass?: string;
  bindings?: string[];
  supersedesAssetId?: string;
}

export type MemoryWriteDecision = "accept" | { require_approval: { reason: string } } | { reject: { reason: string } };

export interface MemoryAssetDecision {
  asset: MemoryAssetResource;
  decision: MemoryWriteDecision;
}

/** One `(layer, status)` bucket of a tenant's memory plane. */
export interface MemoryLayerCount {
  layer: MemoryLayer;
  status: MemoryAssetStatus;
  count: number;
}

/** A compact record of something the agent recently started or stopped remembering. */
export interface MemoryChangeEntry {
  assetId: string;
  layer: MemoryLayer;
  status: MemoryAssetStatus;
  title: string;
  updatedAt: string;
}

/**
 * The tenant-level answer to "what does it remember, and what did it forget".
 *
 * `derivedEmbeddings` is the size of the retrieval index for this tenant. A
 * value far below the Ready total means the index is not being written, which
 * otherwise shows up only as unexplained retrieval latency.
 */
export interface MemoryOverview {
  tenantId: string;
  counts: MemoryLayerCount[];
  derivedEmbeddings: number;
  recentlyRemembered: MemoryChangeEntry[];
  recentlyForgotten: MemoryChangeEntry[];
}

export interface MemoryIndexRebuildResult {
  tenantId: string;
  clearedEmbeddings: number;
}

export type ToolCapability = "web.search" | "image.generate" | "video.generate" | "browser.session";
export type ToolAuthMode = "none" | "api_key" | "oauth_device" | "local_cli_bridge";
export type ToolProfileSource = "builtin" | "config" | "managed";
export type ToolReadiness = "unconfigured" | "ready" | "disabled" | "error";
export type ToolCheckErrorClass =
  | "config_error"
  | "auth_error"
  | "transport_error"
  | "upstream_error"
  | "quota_error"
  | "timeout";

export interface ToolLimits {
  timeoutMs: number;
  maxResults: number;
  /** 0 means "inherit the plane default", not "unlimited". */
  maxCallsPerDay: number;
}

/**
 * A configured tool provider.
 *
 * Note what is absent: there is no credential field. The profile carries a
 * `secretRef`; the value lives only in the tenant secret plane and is written
 * through the tenant-secret routes.
 */
export interface ToolProfileResource {
  profileId: string;
  tenantId?: string;
  title: string;
  capability: ToolCapability;
  family: string;
  authMode: ToolAuthMode;
  source: ToolProfileSource;
  enabled: boolean;
  isDefault: boolean;
  baseUrl?: string;
  /** Reference into the tenant secret plane. Never a value. */
  secretRef?: string;
  secretConfigured: boolean;
  /** `mcp_backed` only: the MCP server that serves this capability. */
  mcpServerId?: string;
  /** `mcp_backed` only: the tool on that server. */
  mcpToolName?: string;
  limits: ToolLimits;
  readiness: ToolReadiness;
  issues?: string[];
  createdAt: string;
  updatedAt: string;
}

/** Whether one agent-facing capability is usable here. Unconfigured ones are reported, not omitted. */
export interface ToolCapabilityStatus {
  capability: ToolCapability;
  configured: boolean;
  defaultProfileId?: string;
  profileCount: number;
}

export interface ToolCheckOutcome {
  profileId: string;
  passed: boolean;
  errorClass?: ToolCheckErrorClass;
  detail?: string;
  checkedAt: string;
}

export interface CreateToolProfileInput {
  title: string;
  capability: ToolCapability;
  family: string;
  authMode: ToolAuthMode;
  baseUrl?: string;
  secretRef?: string;
  /** Required for family `mcp_backed`. */
  mcpServerId?: string;
  /** Required for family `mcp_backed`. */
  mcpToolName?: string;
  limits?: ToolLimits;
  isDefault?: boolean;
  enabled?: boolean;
}

export interface UpdateToolProfileInput {
  title?: string;
  baseUrl?: string;
  /** Points the profile at a different secret; the value is written elsewhere. */
  secretRef?: string;
  mcpServerId?: string;
  mcpToolName?: string;
  limits?: ToolLimits;
  enabled?: boolean;
  isDefault?: boolean;
}

/**
 * One recorded pre-dispatch context assembly: what memory was injected, what
 * was excluded and why. Correlated by tenant and thread in time order; an
 * assembly cannot carry a dispatch id because it is recorded before the
 * dispatch exists.
 */
export interface ContextAssembly {
  assemblyId: string;
  tenantId?: string;
  threadId?: string;
  recordedAt: string;
  record: {
    included: Array<{ assetId: string; layer: string; chars: number; source: string }>;
    excluded: Array<{ assetId: string; layer: string; reason: string; source: string }>;
    budgetChars: number;
    usedChars: number;
    retrievalBudgetChars?: number;
    retrievalUsedChars?: number;
  };
}

/** The operator-owned config.json, redacted, with the hash a compare-and-set write must present. */
export interface ConfigFile {
  path: string;
  exists: boolean;
  config: Record<string, unknown>;
  sha256: string;
  /** Settings that take effect without a restart. Empty today; published rather than implied. */
  hotApplySettings: string[];
}

export interface ConfigFileWriteInput {
  config: Record<string, unknown>;
  /** Compare-and-set against the sha256 returned by getConfigFile. */
  expectSha256?: string;
  dryRun?: boolean;
  /** Refuse unknown keys instead of warning about them. */
  strict?: boolean;
}

export interface ConfigFileWriteResult {
  applied: boolean;
  dryRun: boolean;
  sha256: string;
  restartRequired: boolean;
  hotApplied: string[];
  restartRequiredFor: string[];
  warnings: string[];
}

/** What a thread is for, kept as a system message that survives window elision. */
export interface SessionFrame {
  threadId: string;
  tenantId?: string;
  goal: string;
  constraints: string[];
  updatedAt: string;
}

export interface SessionFrameInput {
  goal: string;
  constraints?: string[];
}

export type SwarmRunStatus = "queued" | "running" | "completed" | "partial_failed" | "failed" | "cancelled";
export type SwarmChildStatus = "queued" | "running" | "completed" | "failed" | "quota_denied" | "cancelled";

export interface SwarmChild {
  index: number;
  goal: string;
  threadId: string;
  status: SwarmChildStatus;
  dispatchId?: string;
  outputPreview?: string;
  error?: string;
  startedAt?: string;
  completedAt?: string;
}

/** A bounded fan-out of goals to concurrent child turns, with every child's terminal state. */
export interface SwarmRun {
  runId: string;
  tenantId?: string;
  requestedBy: string;
  provider?: string;
  model?: string;
  status: SwarmRunStatus;
  children: SwarmChild[];
  createdAt: string;
  updatedAt: string;
  completedAt?: string;
}

export interface SwarmLaunchInput {
  goals: string[];
  provider?: string;
  model?: string;
  requestedBy?: string;
}

export interface SkillUsage {
  skillId: string;
  invocations: number;
  helpful: number;
  corrected: number;
  lastUsedAt?: string;
  lastFeedbackAt?: string;
}

export interface SkillSearchHit {
  skillId: string;
  name: string;
  description: string;
  rank: number;
  usage: SkillUsage;
}

export interface SkillDistillInput {
  threadId: string;
  tenantId?: string;
  /** Steering folded into the distillation prompt; reply on the distill thread and call again. */
  guidance?: string;
  provider?: string;
  model?: string;
}

export interface SkillDistillResult {
  distillThreadId: string;
  dispatchId: string;
  proposal?: { asset: MemoryAssetResource; decision: MemoryWriteDecision };
  parseError?: string;
}

export interface MemoryDrilldownNode {
  asset: MemoryAssetResource;
  members?: MemoryDrilldownNode[];
}

export interface MemoryConsolidationRun {
  runId: string;
  tenantId?: string;
  trigger: string;
  extractedL1: number;
  aggregatedL2: number;
  distilledL3: number;
  pendingApproval?: number;
  startedAt: string;
  completedAt: string;
  error?: string;
}

export class KuraClientError extends Error {
  readonly status: number;
  readonly code?: string;
  readonly tenantDenied?: boolean;
  readonly denial?: TenantDenialResource;
  readonly quotaDenial?: BillingQuotaDenialPayload;
  readonly activationFailure?: ActivationFailurePayload;

  constructor(message: string, options: { status: number; code?: string; tenantDenied?: boolean; denial?: TenantDenialResource; quotaDenial?: BillingQuotaDenialPayload; activationFailure?: ActivationFailurePayload }) {
    super(message);
    this.name = "KuraClientError";
    this.status = options.status;
    this.code = options.code;
    this.tenantDenied = options.tenantDenied;
    this.denial = options.denial;
    this.quotaDenial = options.quotaDenial;
    this.activationFailure = options.activationFailure;
  }
}

export class KuraClient {
  private readonly baseURL: string;
  private readonly accessToken?: string;
  private readonly fetchImpl: typeof fetch;
  private readonly defaultTenantId?: string;

  constructor(options: KuraClientOptions) {
    this.baseURL = trimBaseURL(options.baseURL);
    this.accessToken = options.accessToken?.trim() || undefined;
    this.fetchImpl = options.fetchImpl ?? globalThis.fetch.bind(globalThis);
    this.defaultTenantId = options.defaultTenantId?.trim() || undefined;
  }

  async queryChat(input: ChatQueryInput, tenantOptions?: TenantRequestOptions): Promise<ChatQueryResponse> {
    return this.requestJSON<ChatQueryResponse>("/v1/chat/query", {
      method: "POST",
      body: normalizeChatInput(input),
      tenant: tenantOptions
    });
  }

  async streamChatQuery(input: ChatQueryInput, handlers: StreamHandlers = {}, tenantOptions?: TenantRequestOptions): Promise<ChatQueryResponse> {
    const response = await this.fetchImpl(this.buildURL("/v1/chat/query/stream"), {
      method: "POST",
      headers: this.buildHeaders(tenantOptions),
      body: JSON.stringify(normalizeChatInput(input))
    });

    if (!response.ok) {
      throw await toClientError(response);
    }
    if (!response.body) {
      throw new KuraClientError("chat stream response body is missing", { status: 500, code: "stream_body_missing" });
    }

    let terminal: ChatQueryResponse | null = null;
    await readSSE(response.body, (event) => {
      switch (event.event) {
        case "chat.query.started":
          handlers.onStarted?.(JSON.parse(event.data) as ChatQueryStreamStarted);
          break;
        case "chat.query.delta":
          handlers.onDelta?.(JSON.parse(event.data) as ChatQueryStreamDelta);
          break;
        case "chat.query.completed":
          terminal = JSON.parse(event.data) as ChatQueryResponse;
          handlers.onCompleted?.(terminal);
          break;
        case "chat.query.failed":
          terminal = JSON.parse(event.data) as ChatQueryResponse;
          handlers.onFailed?.(terminal);
          break;
        case "chat.query.cancelled":
          terminal = JSON.parse(event.data) as ChatQueryResponse;
          handlers.onCancelled?.(terminal);
          break;
        default:
          break;
      }
    });

    if (!terminal) {
      throw new KuraClientError("chat stream ended without a terminal event", {
        status: 502,
        code: "stream_terminal_event_missing"
      });
    }

    return terminal;
  }

  async getConfig(tenantOptions?: TenantRequestOptions): Promise<ConfigResponse> {
    return this.requestJSON<ConfigResponse>("/v1/config", { tenant: tenantOptions });
  }

  /** Every model role, including unrouted ones. */
  async listModelRoles(tenantOptions?: TenantRequestOptions): Promise<ModelRoleListResponse> {
    return this.requestJSON<ModelRoleListResponse>("/v1/model-roles", { tenant: tenantOptions });
  }

  /** Route a role to a provider. Omit `model` to use the provider's default. */
  async setModelRole(role: ModelRole, input: { providerId: string; model?: string }, tenantOptions?: TenantRequestOptions): Promise<ModelRoleResource> {
    return this.requestJSON<ModelRoleResource>(`/v1/model-roles/${encodePathComponent(role)}`, { method: "PUT", body: input, tenant: tenantOptions });
  }

  /** Unroute a role so the configured default applies again. */
  async clearModelRole(role: ModelRole, tenantOptions?: TenantRequestOptions): Promise<void> {
    await this.requestJSON<void>(`/v1/model-roles/${encodePathComponent(role)}`, { method: "DELETE", tenant: tenantOptions });
  }

  async listChannelConnectors(query: { limit?: number; cursor?: string; state?: string; kind?: string } = {}, tenantOptions?: TenantRequestOptions): Promise<ChannelConnectorListResponse> {
    return this.requestJSON<ChannelConnectorListResponse>("/v1/channel-management/connectors", { query, tenant: tenantOptions });
  }

  async listThreads(query: ThreadListQuery = {}, tenantOptions?: TenantRequestOptions): Promise<ThreadListResponse> {
    return this.requestJSON<ThreadListResponse>("/v1/threads", { query, tenant: tenantOptions });
  }

  async getThread(threadId: string, tenantOptions?: TenantRequestOptions): Promise<ThreadDetailResponse> {
    return this.requestJSON<ThreadDetailResponse>(`/v1/threads/${encodePathComponent(threadId.trim())}`, { tenant: tenantOptions });
  }

  async getThreadContinuityPreview(threadId: string, previewId: string, tenantOptions?: TenantRequestOptions): Promise<ThreadContinuityPreviewDetail> {
    return this.requestJSON<ThreadContinuityPreviewDetail>(`/v1/threads/${encodePathComponent(threadId.trim())}/continuity-previews/${encodePathComponent(previewId.trim())}`, { tenant: tenantOptions });
  }

  async resetThread(threadId: string, input: ThreadLifecycleActionInput = {}, tenantOptions?: TenantRequestOptions): Promise<ThreadLifecycleActionResponse> {
    return this.requestJSON<ThreadLifecycleActionResponse>(`/v1/threads/${encodePathComponent(threadId.trim())}/reset`, { method: "POST", body: input, tenant: tenantOptions });
  }

  async archiveThread(threadId: string, input: ThreadLifecycleActionInput = {}, tenantOptions?: TenantRequestOptions): Promise<ThreadLifecycleActionResponse> {
    return this.requestJSON<ThreadLifecycleActionResponse>(`/v1/threads/${encodePathComponent(threadId.trim())}/archive`, { method: "POST", body: input, tenant: tenantOptions });
  }

  async reopenThread(threadId: string, input: ThreadLifecycleActionInput = {}, tenantOptions?: TenantRequestOptions): Promise<ThreadLifecycleActionResponse> {
    return this.requestJSON<ThreadLifecycleActionResponse>(`/v1/threads/${encodePathComponent(threadId.trim())}/reopen`, { method: "POST", body: input, tenant: tenantOptions });
  }

  async createThreadHandoff(threadId: string, input: ThreadHandoffInput, tenantOptions?: TenantRequestOptions): Promise<ThreadHandoffLink> {
    return this.requestJSON<ThreadHandoffLink>(`/v1/threads/${encodePathComponent(threadId.trim())}/handoffs`, { method: "POST", body: input, tenant: tenantOptions });
  }

  // --- Roadmap 58: Workspace and capability binding ---

  async listWorkspaces(tenantOptions?: TenantRequestOptions): Promise<WorkspaceListResponse> {
    return this.requestJSON<WorkspaceListResponse>("/v1/workspaces", { tenant: tenantOptions });
  }

  async getWorkspace(workspaceId: string, tenantOptions?: TenantRequestOptions): Promise<WorkspaceResource> {
    return this.requestJSON<WorkspaceResource>(`/v1/workspaces/${encodePathComponent(workspaceId.trim())}`, { tenant: tenantOptions });
  }

  async createWorkspace(input: { displayName: string }, tenantOptions?: TenantRequestOptions): Promise<WorkspaceResource> {
    return this.requestJSON<WorkspaceResource>("/v1/workspaces", { method: "POST", body: input, tenant: tenantOptions });
  }

  async updateWorkspace(workspaceId: string, input: { status: WorkspaceStatus; reasonCode?: string }, tenantOptions?: TenantRequestOptions): Promise<WorkspaceResource> {
    return this.requestJSON<WorkspaceResource>(`/v1/workspaces/${encodePathComponent(workspaceId.trim())}`, { method: "PATCH", body: input, tenant: tenantOptions });
  }

  async listBindings(tenantOptions?: TenantRequestOptions): Promise<BindingListResponse> {
    return this.requestJSON<BindingListResponse>("/v1/bindings", { tenant: tenantOptions });
  }

  async getBinding(bindingId: string, tenantOptions?: TenantRequestOptions): Promise<BindingResource> {
    return this.requestJSON<BindingResource>(`/v1/bindings/${encodePathComponent(bindingId.trim())}`, { tenant: tenantOptions });
  }

  async createBinding(input: CreateBindingInput, tenantOptions?: TenantRequestOptions): Promise<BindingResource> {
    return this.requestJSON<BindingResource>("/v1/bindings", { method: "POST", body: input, tenant: tenantOptions });
  }

  async updateBinding(bindingId: string, input: UpdateBindingInput, tenantOptions?: TenantRequestOptions): Promise<BindingResource> {
    return this.requestJSON<BindingResource>(`/v1/bindings/${encodePathComponent(bindingId.trim())}`, { method: "PATCH", body: input, tenant: tenantOptions });
  }

  async removeBinding(bindingId: string, tenantOptions?: TenantRequestOptions): Promise<void> {
    await this.requestJSON<void>(`/v1/bindings/${encodePathComponent(bindingId.trim())}`, { method: "DELETE", tenant: tenantOptions });
  }

  async repairBinding(bindingId: string, tenantOptions?: TenantRequestOptions): Promise<BindingResource> {
    return this.requestJSON<BindingResource>(`/v1/bindings/${encodePathComponent(bindingId.trim())}/repair`, { method: "POST", body: {}, tenant: tenantOptions });
  }

  async listCapabilityVisibility(query: { scopeKind: CapabilityVisibilityScope; scopeRef: string }, tenantOptions?: TenantRequestOptions): Promise<CapabilityVisibilityListResponse> {
    return this.requestJSON<CapabilityVisibilityListResponse>("/v1/capability-visibility", { query, tenant: tenantOptions });
  }

  async setCapabilityVisibility(input: SetCapabilityVisibilityInput, tenantOptions?: TenantRequestOptions): Promise<CapabilityVisibilityResource> {
    return this.requestJSON<CapabilityVisibilityResource>("/v1/capability-visibility", { method: "PUT", body: input, tenant: tenantOptions });
  }

  async getChannelConnector(connectorId: string, tenantOptions?: TenantRequestOptions): Promise<ChannelConnectorDetailResource> {
    return this.requestJSON<ChannelConnectorDetailResource>(`/v1/channel-management/connectors/${encodePathComponent(connectorId.trim())}`, { tenant: tenantOptions });
  }

  async getChannelConnectorDiagnostics(connectorId: string, tenantOptions?: TenantRequestOptions): Promise<{ items: ConnectorDiagnosticStateResource[] }> {
    return this.requestJSON<{ items: ConnectorDiagnosticStateResource[] }>(`/v1/channel-management/connectors/${encodePathComponent(connectorId.trim())}/diagnostics`, { tenant: tenantOptions });
  }

  async disableChannelConnector(connectorId: string, input: ChannelManagementActionInput = {}, tenantOptions?: TenantRequestOptions): Promise<ChannelEnablementMutationResponse> {
    return this.requestJSON<ChannelEnablementMutationResponse>(`/v1/channel-management/connectors/${encodePathComponent(connectorId.trim())}/disable`, {
      method: "POST",
      body: input,
      tenant: tenantOptions
    });
  }

  async reEnableChannelConnector(connectorId: string, input: ChannelManagementActionInput = {}, tenantOptions?: TenantRequestOptions): Promise<ChannelEnablementMutationResponse> {
    return this.requestJSON<ChannelEnablementMutationResponse>(`/v1/channel-management/connectors/${encodePathComponent(connectorId.trim())}/re-enable`, {
      method: "POST",
      body: input,
      tenant: tenantOptions
    });
  }

  async startChannelConnectorRepair(connectorId: string, input: ChannelManagementActionInput = { actionKind: "repair" }, tenantOptions?: TenantRequestOptions): Promise<ChannelRepairActionResource> {
    return this.requestJSON<ChannelRepairActionResource>(`/v1/channel-management/connectors/${encodePathComponent(connectorId.trim())}/repair-actions`, {
      method: "POST",
      body: { ...input, actionKind: input.actionKind ?? "repair" },
      tenant: tenantOptions
    });
  }

  async reconnectChannelConnector(connectorId: string, input: ChannelManagementActionInput = {}, tenantOptions?: TenantRequestOptions): Promise<ChannelRepairActionResource> {
    return this.startChannelConnectorRepair(connectorId, { ...input, actionKind: "reconnect" }, tenantOptions);
  }

  async rotateChannelConnectorCredentials(connectorId: string, input: ChannelManagementActionInput = {}, tenantOptions?: TenantRequestOptions): Promise<ChannelRepairActionResource> {
    return this.startChannelConnectorRepair(connectorId, { ...input, actionKind: "credential-rotation" }, tenantOptions);
  }

  async getChannelRoutePolicy(connectorId: string, tenantOptions?: TenantRequestOptions): Promise<ChannelRoutePolicyResource> {
    return this.requestJSON<ChannelRoutePolicyResource>(`/v1/channel-management/connectors/${encodePathComponent(connectorId.trim())}/route-policy`, { tenant: tenantOptions });
  }

  async updateChannelRoutePolicy(connectorId: string, input: ChannelManagementActionInput, tenantOptions?: TenantRequestOptions): Promise<ChannelRoutePolicyResource> {
    return this.requestJSON<ChannelRoutePolicyResource>(`/v1/channel-management/connectors/${encodePathComponent(connectorId.trim())}/route-policy`, {
      method: "PUT",
      body: input,
      tenant: tenantOptions
    });
  }

  async listChannelReplyOutcomes(connectorId: string, tenantOptions?: TenantRequestOptions): Promise<{ items: unknown[] }> {
    return this.requestJSON<{ items: unknown[] }>(`/v1/channel-management/connectors/${encodePathComponent(connectorId.trim())}/reply-outcomes`, { tenant: tenantOptions });
  }

  async listChannelDeliveryOutcomes(connectorId: string, tenantOptions?: TenantRequestOptions): Promise<{ items: unknown[] }> {
    return this.requestJSON<{ items: unknown[] }>(`/v1/channel-management/connectors/${encodePathComponent(connectorId.trim())}/delivery-outcomes`, { tenant: tenantOptions });
  }

  async getChannelConnectorSupportEvidence(connectorId: string, tenantOptions?: TenantRequestOptions): Promise<ChannelManagementSupportEvidence> {
    return this.requestJSON<ChannelManagementSupportEvidence>(`/v1/channel-management/connectors/${encodePathComponent(connectorId.trim())}/support-evidence`, { tenant: tenantOptions });
  }

  async getDiscordSetup(connectorId: string, tenantOptions?: TenantRequestOptions): Promise<DiscordHostedSetupResource> {
    return this.requestJSON<DiscordHostedSetupResource>(`/v1/connectors/${encodePathComponent(connectorId.trim())}/discord-setup`, { tenant: tenantOptions });
  }

  async getDiscordSmokeEvidence(connectorId: string, tenantOptions?: TenantRequestOptions): Promise<DiscordSmokeEvidenceResource> {
    return this.requestJSON<DiscordSmokeEvidenceResource>(`/v1/connectors/${encodePathComponent(connectorId.trim())}/discord-smoke`, { tenant: tenantOptions });
  }

  async getTelegramSetup(connectorId: string, tenantOptions?: TenantRequestOptions): Promise<TelegramHostedSetupResource> {
    return this.requestJSON<TelegramHostedSetupResource>(`/v1/connectors/${encodePathComponent(connectorId.trim())}/telegram-setup`, { tenant: tenantOptions });
  }

  async getTelegramSmokeEvidence(connectorId: string, tenantOptions?: TenantRequestOptions): Promise<TelegramSmokeEvidenceResource> {
    return this.requestJSON<TelegramSmokeEvidenceResource>(`/v1/connectors/${encodePathComponent(connectorId.trim())}/telegram-smoke`, { tenant: tenantOptions });
  }

  async getSlackSetup(connectorId: string, tenantOptions?: TenantRequestOptions): Promise<SlackHostedSetupResource> {
    return this.requestJSON<SlackHostedSetupResource>(`/v1/connectors/${encodePathComponent(connectorId.trim())}/slack-setup`, { tenant: tenantOptions });
  }

  async getSlackSmokeEvidence(connectorId: string, tenantOptions?: TenantRequestOptions): Promise<SlackSmokeEvidenceResource> {
    return this.requestJSON<SlackSmokeEvidenceResource>(`/v1/connectors/${encodePathComponent(connectorId.trim())}/slack-smoke`, { tenant: tenantOptions });
  }

  async getMatrixSetup(connectorId: string, tenantOptions?: TenantRequestOptions): Promise<MatrixHostedSetupResource> {
    return this.requestJSON<MatrixHostedSetupResource>(`/v1/connectors/${encodePathComponent(connectorId.trim())}/matrix-setup`, { tenant: tenantOptions });
  }

  async getMatrixSmokeEvidence(connectorId: string, tenantOptions?: TenantRequestOptions): Promise<MatrixSmokeEvidenceResource> {
    return this.requestJSON<MatrixSmokeEvidenceResource>(`/v1/connectors/${encodePathComponent(connectorId.trim())}/matrix-smoke`, { tenant: tenantOptions });
  }

  async getSlackLiveValidationSmokeEvidence(connectorId: string, tenantOptions?: TenantRequestOptions): Promise<SlackSmokeEvidenceResource> {
    return this.requestJSON<SlackSmokeEvidenceResource>("/v1/live-validations/slack-smoke", {
      query: { connectorId: connectorId.trim() },
      tenant: tenantOptions
    });
  }

  async getMatrixLiveValidationSmokeEvidence(connectorId: string, tenantOptions?: TenantRequestOptions): Promise<MatrixSmokeEvidenceResource> {
    return this.requestJSON<MatrixSmokeEvidenceResource>("/v1/live-validations/matrix-smoke", {
      query: { connectorId: connectorId.trim() },
      tenant: tenantOptions
    });
  }

  async getDiscordConformanceEvidence(connectorId: string, tenantOptions?: TenantRequestOptions): Promise<DiscordConformanceEvidenceResponse> {
    return this.requestJSON<DiscordConformanceEvidenceResponse>("/v1/live-validations/discord-conformance", {
      query: { connectorId: connectorId.trim() },
      tenant: tenantOptions
    });
  }

  async getTelegramConformanceEvidence(connectorId: string, tenantOptions?: TenantRequestOptions): Promise<TelegramConformanceEvidenceResponse> {
    return this.requestJSON<TelegramConformanceEvidenceResponse>("/v1/live-validations/telegram-conformance", {
      query: { connectorId: connectorId.trim() },
      tenant: tenantOptions
    });
  }

  async getSlackConformanceEvidence(connectorId: string, tenantOptions?: TenantRequestOptions): Promise<SlackConformanceEvidenceResponse> {
    return this.requestJSON<SlackConformanceEvidenceResponse>("/v1/live-validations/slack-conformance", {
      query: { connectorId: connectorId.trim() },
      tenant: tenantOptions
    });
  }

  async getMatrixConformanceEvidence(connectorId: string, tenantOptions?: TenantRequestOptions): Promise<MatrixConformanceEvidenceResponse> {
    return this.requestJSON<MatrixConformanceEvidenceResponse>("/v1/live-validations/matrix-conformance", {
      query: { connectorId: connectorId.trim() },
      tenant: tenantOptions
    });
  }

  async getMe(tenantOptions?: TenantRequestOptions): Promise<AuthMeResponse> {
    return this.requestJSON<AuthMeResponse>("/v1/auth/me", { tenant: tenantOptions });
  }

  async listTenants(query: Record<string, QueryValue> = {}, tenantOptions?: TenantRequestOptions): Promise<TenantListResponse> {
    return this.requestJSON<TenantListResponse>("/v1/tenants", { query, tenant: tenantOptions });
  }

  async getTenant(tenantId: string, tenantOptions?: TenantRequestOptions): Promise<TenantDetailResponse> {
    return this.requestJSON<TenantDetailResponse>(`/v1/tenants/${tenantId.trim()}`, { tenant: tenantOptions });
  }

  async listMemberships(tenantId: string, query: Record<string, QueryValue> = {}, tenantOptions?: TenantRequestOptions): Promise<MembershipListResponse> {
    return this.requestJSON<MembershipListResponse>(`/v1/tenants/${tenantId.trim()}/memberships`, {
      query,
      tenant: tenantOptions
    });
  }

  async updateMembershipRole(
    tenantId: string,
    membershipId: string,
    input: UpdateMembershipRoleInput,
    tenantOptions?: TenantRequestOptions
  ): Promise<{ membership: MembershipResource }> {
    return this.requestJSON<{ membership: MembershipResource }>(`/v1/tenants/${tenantId.trim()}/memberships/${membershipId.trim()}`, {
      method: "PATCH",
      body: { role: input.role },
      tenant: tenantOptions
    });
  }

  async removeMembership(tenantId: string, membershipId: string, tenantOptions?: TenantRequestOptions): Promise<{ membership: MembershipResource }> {
    return this.requestJSON<{ membership: MembershipResource }>(`/v1/tenants/${tenantId.trim()}/memberships/${membershipId.trim()}`, {
      method: "DELETE",
      tenant: tenantOptions
    });
  }

  async listTenantSecrets(tenantOptions?: TenantRequestOptions): Promise<TenantSecretListResponse> {
    return this.requestJSON<TenantSecretListResponse>("/v1/tenant-secrets", { tenant: tenantOptions });
  }

  async getTenantSecret(secretRef: string, tenantOptions?: TenantRequestOptions): Promise<TenantSecretResponse> {
    return this.requestJSON<TenantSecretResponse>(`/v1/tenant-secrets/${encodePathComponent(secretRef)}`, { tenant: tenantOptions });
  }

  async getBillingPlan(tenantOptions?: TenantRequestOptions): Promise<BillingPlanResource> {
    return this.requestJSON<BillingPlanResource>("/v1/billing/plan", { tenant: tenantOptions });
  }

  async getBillingUsage(tenantOptions?: TenantRequestOptions): Promise<BillingUsageResponse> {
    return this.requestJSON<BillingUsageResponse>("/v1/billing/usage", { tenant: tenantOptions });
  }

  async listBillingQuotas(tenantOptions?: TenantRequestOptions): Promise<{ items: BillingQuotaResource[] }> {
    return this.requestJSON<{ items: BillingQuotaResource[] }>("/v1/billing/quotas", { tenant: tenantOptions });
  }

  async listBillingDenials(tenantOptions?: TenantRequestOptions): Promise<{ items: BillingDenialResource[] }> {
    return this.requestJSON<{ items: BillingDenialResource[] }>("/v1/billing/denials", { tenant: tenantOptions });
  }

  async getBillingQuotaDashboard(tenantOptions?: TenantRequestOptions): Promise<BillingQuotaDashboardResponse> {
    return this.requestJSON<BillingQuotaDashboardResponse>("/v1/billing/quota-dashboard", { tenant: tenantOptions });
  }

  async getBillingDenialDetail(denialId: string, tenantOptions?: TenantRequestOptions): Promise<BillingDenialDetailResponse> {
    return this.requestJSON<BillingDenialDetailResponse>(`/v1/billing/denials/${encodePathComponent(denialId.trim())}`, { tenant: tenantOptions });
  }

  async exportBillingDenialEvidence(denialId: string, tenantOptions?: TenantRequestOptions): Promise<BillingEvidenceExportResponse> {
    return this.requestJSON<BillingEvidenceExportResponse>(`/v1/billing/denials/${encodePathComponent(denialId.trim())}/evidence-export`, {
      method: "POST",
      tenant: tenantOptions
    });
  }

  async getActivation(tenantOptions?: TenantRequestOptions): Promise<ActivationResponse> {
    return this.requestJSON<ActivationResponse>("/v1/activation", { tenant: tenantOptions });
  }

  async activate(input: RunActivationInput = {}, tenantOptions?: TenantRequestOptions): Promise<ActivationResponse> {
    return this.requestJSON<ActivationResponse>("/v1/activation", {
      method: "POST",
      body: normalizeActivationInput(input),
      tenant: tenantOptions
    });
  }

  async runActivationTestChat(input: RunActivationTestChatInput, tenantOptions?: TenantRequestOptions): Promise<ActivationTestChatResponse> {
    return this.requestJSON<ActivationTestChatResponse>("/v1/activation/test-chat", {
      method: "POST",
      body: normalizeActivationTestChatInput(input),
      tenant: tenantOptions
    });
  }

  async getActivationDiagnostics(tenantOptions?: TenantRequestOptions): Promise<ActivationDiagnosticListResponse> {
    return this.requestJSON<ActivationDiagnosticListResponse>("/v1/activation/diagnostics", { tenant: tenantOptions });
  }

  async listSetupTargets(tenantOptions?: TenantRequestOptions): Promise<SetupTargetListResponse> {
    return this.requestJSON<SetupTargetListResponse>("/v1/setup/targets", { tenant: tenantOptions });
  }

  async listSetupSessions(tenantOptions?: TenantRequestOptions): Promise<SetupSessionListResponse> {
    return this.requestJSON<SetupSessionListResponse>("/v1/setup/sessions", { tenant: tenantOptions });
  }

  async startSetup(input: StartSetupInput, tenantOptions?: TenantRequestOptions): Promise<SetupSessionResponse> {
    return this.requestJSON<SetupSessionResponse>("/v1/setup/sessions", {
      method: "POST",
      body: normalizeStartSetupInput(input),
      tenant: tenantOptions
    });
  }

  async getSetupSession(setupSessionId: string, tenantOptions?: TenantRequestOptions): Promise<SetupSessionResponse> {
    return this.requestJSON<SetupSessionResponse>(`/v1/setup/sessions/${encodePathComponent(setupSessionId)}`, { tenant: tenantOptions });
  }

  async submitSetupSecret(setupSessionId: string, input: SubmitSetupSecretInput, tenantOptions?: TenantRequestOptions): Promise<SetupSessionResponse> {
    return this.requestJSON<SetupSessionResponse>(`/v1/setup/sessions/${encodePathComponent(setupSessionId)}/submit-secret`, {
      method: "POST",
      body: {
        secretRef: input.secretRef.trim(),
        value: input.value,
        displayName: input.displayName?.trim() || undefined,
        resourceRefs: input.resourceRefs
      },
      tenant: tenantOptions
    });
  }

  async startSetupOAuth(setupSessionId: string, input: StartSetupOAuthInput = {}, tenantOptions?: TenantRequestOptions): Promise<SetupOAuthStartResponse> {
    return this.requestJSON<SetupOAuthStartResponse>(`/v1/setup/sessions/${encodePathComponent(setupSessionId)}/oauth/start`, {
      method: "POST",
      body: { redirectRoute: input.redirectRoute?.trim() || undefined },
      tenant: tenantOptions
    });
  }

  async completeSetupOAuth(setupSessionId: string, input: CompleteSetupOAuthInput, tenantOptions?: TenantRequestOptions): Promise<SetupSessionResponse> {
    return this.requestJSON<SetupSessionResponse>(`/v1/setup/sessions/${encodePathComponent(setupSessionId)}/oauth/callback`, {
      method: "POST",
      body: {
        state: input.state.trim(),
        result: input.result,
        accountLabel: input.accountLabel?.trim() || undefined,
        code: input.code?.trim() || undefined,
        redirectUri: input.redirectUri?.trim() || undefined
      },
      tenant: tenantOptions
    });
  }

  async retrySetup(setupSessionId: string, tenantOptions?: TenantRequestOptions): Promise<SetupSessionResponse> {
    return this.requestJSON<SetupSessionResponse>(`/v1/setup/sessions/${encodePathComponent(setupSessionId)}/retry`, {
      method: "POST",
      tenant: tenantOptions
    });
  }

  async replaceSetup(setupSessionId: string, inputOrTenantOptions?: ReplaceSetupInput | TenantRequestOptions, tenantOptions?: TenantRequestOptions): Promise<SetupSessionResponse> {
    const tenant = tenantOptions ?? (isTenantRequestOptions(inputOrTenantOptions) ? inputOrTenantOptions : undefined);
    return this.requestJSON<SetupSessionResponse>(`/v1/setup/sessions/${encodePathComponent(setupSessionId)}/replace`, {
      method: "POST",
      tenant
    });
  }

  async cancelSetup(setupSessionId: string, tenantOptions?: TenantRequestOptions): Promise<SetupSessionResponse> {
    return this.requestJSON<SetupSessionResponse>(`/v1/setup/sessions/${encodePathComponent(setupSessionId)}/cancel`, {
      method: "POST",
      tenant: tenantOptions
    });
  }

  async disableSetup(setupSessionId: string, input: DisableSetupInput = {}, tenantOptions?: TenantRequestOptions): Promise<SetupSessionResponse> {
    return this.requestJSON<SetupSessionResponse>(`/v1/setup/sessions/${encodePathComponent(setupSessionId)}/disable`, {
      method: "POST",
      body: { disabledReason: input.disabledReason?.trim() || undefined },
      tenant: tenantOptions
    });
  }

  async getSetupDiagnostics(setupSessionId: string, tenantOptions?: TenantRequestOptions): Promise<SetupDiagnosticListResponse> {
    return this.requestJSON<SetupDiagnosticListResponse>(`/v1/setup/sessions/${encodePathComponent(setupSessionId)}/diagnostics`, {
      tenant: tenantOptions
    });
  }

  async assignBillingPlan(tenantId: string, input: BillingPlanAssignmentInput, tenantOptions?: TenantRequestOptions): Promise<BillingPlanResource> {
    return this.requestJSON<BillingPlanResource>(`/v1/admin/billing/tenants/${tenantId.trim()}/plan`, {
      method: "POST",
      body: {
        planKey: input.planKey.trim(),
        enforcementMode: input.enforcementMode,
        reason: input.reason.trim()
      },
      tenant: tenantOptions
    });
  }

  async createBillingQuotaOverride(tenantId: string, input: BillingQuotaOverrideInput, tenantOptions?: TenantRequestOptions): Promise<Record<string, unknown>> {
    return this.requestJSON<Record<string, unknown>>(`/v1/admin/billing/tenants/${tenantId.trim()}/quota-overrides`, {
      method: "POST",
      body: {
        category: input.category,
        limit: input.limit,
        reason: input.reason.trim()
      },
      tenant: tenantOptions
    });
  }

  async createBillingManualAdjustment(tenantId: string, input: BillingManualAdjustmentInput, tenantOptions?: TenantRequestOptions): Promise<BillingManualAdjustmentResource> {
    return this.requestJSON<BillingManualAdjustmentResource>(`/v1/admin/billing/tenants/${tenantId.trim()}/manual-adjustments`, {
      method: "POST",
      body: {
        category: input.category,
        quotaPeriodId: input.quotaPeriodId.trim(),
        amountDelta: input.amountDelta,
        reason: input.reason.trim()
      },
      tenant: tenantOptions
    });
  }

  async resolveBillingReservation(tenantId: string, reservationId: string, input: BillingReservationResolutionInput, tenantOptions?: TenantRequestOptions): Promise<BillingReservationResource> {
    return this.requestJSON<BillingReservationResource>(`/v1/admin/billing/tenants/${tenantId.trim()}/reservations/${reservationId.trim()}/resolve`, {
      method: "POST",
      body: {
        outcome: input.outcome,
        reason: input.reason.trim(),
        amount: input.amount
      },
      tenant: tenantOptions
    });
  }

  async createTenantSecret(input: CreateTenantSecretInput, tenantOptions?: TenantRequestOptions): Promise<TenantSecretResponse> {
    return this.requestJSON<TenantSecretResponse>("/v1/tenant-secrets", {
      method: "POST",
      body: {
        secretRef: input.secretRef.trim(),
        displayName: input.displayName?.trim() || undefined,
        value: input.value,
        document: input.document
      },
      tenant: tenantOptions
    });
  }

  async updateTenantSecret(secretRef: string, input: UpdateTenantSecretInput, tenantOptions?: TenantRequestOptions): Promise<TenantSecretResponse> {
    return this.requestJSON<TenantSecretResponse>(`/v1/tenant-secrets/${encodePathComponent(secretRef)}`, {
      method: "PATCH",
      body: {
        displayName: input.displayName?.trim() || undefined,
        document: input.document
      },
      tenant: tenantOptions
    });
  }

  async rotateTenantSecret(secretRef: string, input: RotateTenantSecretInput, tenantOptions?: TenantRequestOptions): Promise<TenantSecretResponse> {
    return this.requestJSON<TenantSecretResponse>(`/v1/tenant-secrets/${encodePathComponent(secretRef)}/rotate`, {
      method: "POST",
      body: { value: input.value },
      tenant: tenantOptions
    });
  }

  async disableTenantSecret(secretRef: string, input: DisableTenantSecretInput, tenantOptions?: TenantRequestOptions): Promise<TenantSecretResponse> {
    return this.requestJSON<TenantSecretResponse>(`/v1/tenant-secrets/${encodePathComponent(secretRef)}/disable`, {
      method: "POST",
      body: { disabledReason: input.disabledReason.trim() },
      tenant: tenantOptions
    });
  }

  async getOnboarding(tenantOptions?: TenantRequestOptions): Promise<OperatorOnboardingResponse> {
    return this.requestJSON<OperatorOnboardingResponse>("/v1/operator/onboarding", { tenant: tenantOptions });
  }

  async getActivity(query: OperatorActivityQuery = {}, tenantOptions?: TenantRequestOptions): Promise<OperatorActivityListResponse> {
    return this.requestJSON<OperatorActivityListResponse>("/v1/operator/activity", {
      query: {
        sourceKind: query.sourceKind,
        attentionOnly: query.attentionOnly,
        limit: query.limit
      },
      tenant: tenantOptions
    });
  }

  async getDiagnostics(query: OperatorDiagnosticsQuery = {}, tenantOptions?: TenantRequestOptions): Promise<OperatorDiagnosticListResponse> {
    return this.requestJSON<OperatorDiagnosticListResponse>("/v1/operator/diagnostics", {
      query: {
        sourceKind: query.sourceKind,
        plane: query.plane,
        severity: query.severity
      },
      tenant: tenantOptions
    });
  }

  async listReplayCandidates(query: ReplayCandidateQuery = {}, tenantOptions?: TenantRequestOptions): Promise<ReplayCandidateListResponse> {
    return this.requestJSON<ReplayCandidateListResponse>("/v1/evaluation/replay-candidates", { query, tenant: tenantOptions });
  }

  async getReplayCandidate(candidateId: string, tenantOptions?: TenantRequestOptions): Promise<ReplayCandidateResource> {
    return this.requestJSON<ReplayCandidateResource>(`/v1/evaluation/replay-candidates/${candidateId.trim()}`, { tenant: tenantOptions });
  }

  async createReplayCandidate(input: CreateReplayCandidateInput, tenantOptions?: TenantRequestOptions): Promise<ReplayCandidateResource> {
    return this.requestJSON<ReplayCandidateResource>("/v1/evaluation/replay-candidates", {
      method: "POST",
      body: input,
      tenant: tenantOptions
    });
  }

  async createReplayAttempt(candidateId: string, input: CreateReplayAttemptInput = {}, tenantOptions?: TenantRequestOptions): Promise<ReplayAttemptResource> {
    return this.requestJSON<ReplayAttemptResource>(`/v1/evaluation/replay-candidates/${candidateId.trim()}/attempts`, {
      method: "POST",
      body: input,
      tenant: tenantOptions
    });
  }

  async listReplayAttempts(query: ReplayAttemptQuery = {}, tenantOptions?: TenantRequestOptions): Promise<ReplayAttemptListResponse> {
    return this.requestJSON<ReplayAttemptListResponse>("/v1/evaluation/replay-attempts", { query, tenant: tenantOptions });
  }

  async getReplayAttempt(attemptId: string, tenantOptions?: TenantRequestOptions): Promise<ReplayAttemptResource> {
    return this.requestJSON<ReplayAttemptResource>(`/v1/evaluation/replay-attempts/${attemptId.trim()}`, { tenant: tenantOptions });
  }

  async createReplayComparison(attemptId: string, input: CreateReplayComparisonInput = {}, tenantOptions?: TenantRequestOptions): Promise<ReplayComparisonResource> {
    return this.requestJSON<ReplayComparisonResource>(`/v1/evaluation/replay-attempts/${attemptId.trim()}/compare`, {
      method: "POST",
      body: input,
      tenant: tenantOptions
    });
  }

  async listReplayComparisons(query: ReplayComparisonQuery = {}, tenantOptions?: TenantRequestOptions): Promise<ReplayComparisonListResponse> {
    return this.requestJSON<ReplayComparisonListResponse>("/v1/evaluation/comparisons", { query, tenant: tenantOptions });
  }

  async getReplayComparison(comparisonId: string, tenantOptions?: TenantRequestOptions): Promise<ReplayComparisonResource> {
    return this.requestJSON<ReplayComparisonResource>(`/v1/evaluation/comparisons/${comparisonId.trim()}`, { tenant: tenantOptions });
  }

  async listReplayFixtures(query: ReplayFixtureQuery = {}, tenantOptions?: TenantRequestOptions): Promise<ReplayFixtureListResponse> {
    return this.requestJSON<ReplayFixtureListResponse>("/v1/evaluation/fixtures", { query, tenant: tenantOptions });
  }

  async listEvaluationDiscoveryPolicies(query: EvaluationDiscoveryPolicyQuery = {}, tenantOptions?: TenantRequestOptions): Promise<EvaluationProductListResponse<EvaluationDiscoveryPolicyResource>> {
    return this.requestJSON<EvaluationProductListResponse<EvaluationDiscoveryPolicyResource>>("/v1/evaluation/discovery-policies", { query, tenant: tenantOptions });
  }

  async getEvaluationDiscoveryPolicy(policyId: string, tenantOptions?: TenantRequestOptions): Promise<EvaluationDiscoveryPolicyResource> {
    return this.requestJSON<EvaluationDiscoveryPolicyResource>(`/v1/evaluation/discovery-policies/${policyId.trim()}`, { tenant: tenantOptions });
  }

  async upsertEvaluationDiscoveryPolicy(policyId: string, input: UpsertEvaluationDiscoveryPolicyInput, tenantOptions?: TenantRequestOptions): Promise<EvaluationDiscoveryPolicyResource> {
    return this.requestJSON<EvaluationDiscoveryPolicyResource>(`/v1/evaluation/discovery-policies/${policyId.trim()}`, {
      method: "PUT",
      body: input,
      tenant: tenantOptions
    });
  }

  async startEvaluationDiscoveryRun(input: StartEvaluationDiscoveryRunInput = {}, tenantOptions?: TenantRequestOptions): Promise<EvaluationDiscoveryRunResource> {
    return this.requestJSON<EvaluationDiscoveryRunResource>("/v1/evaluation/discovery-runs", {
      method: "POST",
      body: input,
      tenant: tenantOptions
    });
  }

  async listEvaluationDiscoveryRuns(query: EvaluationDiscoveryRunQuery = {}, tenantOptions?: TenantRequestOptions): Promise<EvaluationProductListResponse<EvaluationDiscoveryRunResource>> {
    return this.requestJSON<EvaluationProductListResponse<EvaluationDiscoveryRunResource>>("/v1/evaluation/discovery-runs", { query, tenant: tenantOptions });
  }

  async getEvaluationDiscoveryRun(discoveryRunId: string, tenantOptions?: TenantRequestOptions): Promise<EvaluationDiscoveryRunResource> {
    return this.requestJSON<EvaluationDiscoveryRunResource>(`/v1/evaluation/discovery-runs/${discoveryRunId.trim()}`, { tenant: tenantOptions });
  }

  async listEvaluationDiscoveredCandidates(query: EvaluationDiscoveredCandidateQuery = {}, tenantOptions?: TenantRequestOptions): Promise<EvaluationProductListResponse<EvaluationDiscoveredCandidateResource>> {
    return this.requestJSON<EvaluationProductListResponse<EvaluationDiscoveredCandidateResource>>("/v1/evaluation/discovered-candidates", { query, tenant: tenantOptions });
  }

  async getEvaluationDiscoveredCandidate(discoveredCandidateId: string, tenantOptions?: TenantRequestOptions): Promise<EvaluationDiscoveredCandidateResource> {
    return this.requestJSON<EvaluationDiscoveredCandidateResource>(`/v1/evaluation/discovered-candidates/${discoveredCandidateId.trim()}`, { tenant: tenantOptions });
  }

  async createEvaluationSuppression(input: CreateEvaluationSuppressionInput, tenantOptions?: TenantRequestOptions): Promise<EvaluationSuppressionRecord> {
    return this.requestJSON<EvaluationSuppressionRecord>("/v1/evaluation/suppressions", {
      method: "POST",
      body: input,
      tenant: tenantOptions
    });
  }

  async materializeProductFixture(discoveredCandidateId: string, input: CreateProductFixtureInput, tenantOptions?: TenantRequestOptions): Promise<ProductFixtureMutationResponse> {
    return this.requestJSON<ProductFixtureMutationResponse>(`/v1/evaluation/discovered-candidates/${discoveredCandidateId.trim()}/product-fixtures`, {
      method: "POST",
      body: input,
      tenant: tenantOptions
    });
  }

  async listProductFixtures(query: ProductFixtureQuery = {}, tenantOptions?: TenantRequestOptions): Promise<EvaluationProductListResponse<ProductFixtureResource>> {
    return this.requestJSON<EvaluationProductListResponse<ProductFixtureResource>>("/v1/evaluation/product-fixtures", { query, tenant: tenantOptions });
  }

  async getProductFixture(fixtureId: string, tenantOptions?: TenantRequestOptions): Promise<ProductFixtureResource> {
    return this.requestJSON<ProductFixtureResource>(`/v1/evaluation/product-fixtures/${fixtureId.trim()}`, { tenant: tenantOptions });
  }

  async listProductFixtureRevisions(fixtureId: string, query: EvaluationProductTenantOptions = {}, tenantOptions?: TenantRequestOptions): Promise<EvaluationProductListResponse<FixtureRevisionResource>> {
    return this.requestJSON<EvaluationProductListResponse<FixtureRevisionResource>>(`/v1/evaluation/product-fixtures/${fixtureId.trim()}/revisions`, { query, tenant: tenantOptions });
  }

  async createProductFixtureRevision(fixtureId: string, input: CreateFixtureRevisionInput, tenantOptions?: TenantRequestOptions): Promise<ProductFixtureMutationResponse> {
    return this.requestJSON<ProductFixtureMutationResponse>(`/v1/evaluation/product-fixtures/${fixtureId.trim()}/revisions`, {
      method: "POST",
      body: input,
      tenant: tenantOptions
    });
  }

  async reviewProductFixture(fixtureId: string, input: ReviewProductFixtureInput, tenantOptions?: TenantRequestOptions): Promise<ProductFixtureMutationResponse> {
    return this.requestJSON<ProductFixtureMutationResponse>(`/v1/evaluation/product-fixtures/${fixtureId.trim()}/review`, {
      method: "POST",
      body: input,
      tenant: tenantOptions
    });
  }

  async suppressProductFixture(fixtureId: string, input: Partial<Pick<CreateEvaluationSuppressionInput, "reasonCode" | "reason" | "expiresAt">> = {}, tenantOptions?: TenantRequestOptions): Promise<ProductFixtureMutationResponse> {
    return this.requestJSON<ProductFixtureMutationResponse>(`/v1/evaluation/product-fixtures/${fixtureId.trim()}/suppress`, {
      method: "POST",
      body: input,
      tenant: tenantOptions
    });
  }

  async createEvaluationCampaign(input: CreateEvaluationCampaignInput, tenantOptions?: TenantRequestOptions): Promise<EvaluationCampaignResource> {
    return this.requestJSON<EvaluationCampaignResource>("/v1/evaluation/campaigns", {
      method: "POST",
      body: input,
      tenant: tenantOptions
    });
  }

  async listEvaluationCampaigns(query: EvaluationProductTenantOptions = {}, tenantOptions?: TenantRequestOptions): Promise<EvaluationProductListResponse<EvaluationCampaignResource>> {
    return this.requestJSON<EvaluationProductListResponse<EvaluationCampaignResource>>("/v1/evaluation/campaigns", { query, tenant: tenantOptions });
  }

  async getEvaluationCampaign(campaignId: string, tenantOptions?: TenantRequestOptions): Promise<EvaluationCampaignResource> {
    return this.requestJSON<EvaluationCampaignResource>(`/v1/evaluation/campaigns/${campaignId.trim()}`, { tenant: tenantOptions });
  }

  async startEvaluationCampaign(campaignId: string, tenantOptions?: TenantRequestOptions): Promise<EvaluationCampaignResource> {
    return this.requestJSON<EvaluationCampaignResource>(`/v1/evaluation/campaigns/${campaignId.trim()}/start`, { method: "POST", tenant: tenantOptions });
  }

  async cancelEvaluationCampaign(campaignId: string, tenantOptions?: TenantRequestOptions): Promise<EvaluationCampaignResource> {
    return this.requestJSON<EvaluationCampaignResource>(`/v1/evaluation/campaigns/${campaignId.trim()}/cancel`, { method: "POST", tenant: tenantOptions });
  }

  async publishEvaluationCampaignResults(campaignId: string, tenantOptions?: TenantRequestOptions): Promise<EvaluationCampaignResource> {
    return this.requestJSON<EvaluationCampaignResource>(`/v1/evaluation/campaigns/${campaignId.trim()}/publish-results`, { method: "POST", tenant: tenantOptions });
  }

  async listEvaluationCampaignItems(campaignId: string, query: EvaluationProductTenantOptions = {}, tenantOptions?: TenantRequestOptions): Promise<EvaluationProductListResponse<EvaluationCampaignItemResource>> {
    return this.requestJSON<EvaluationProductListResponse<EvaluationCampaignItemResource>>(`/v1/evaluation/campaigns/${campaignId.trim()}/items`, { query, tenant: tenantOptions });
  }

  async listEvaluationCampaignAttemptGroups(campaignId: string, query: EvaluationProductTenantOptions = {}, tenantOptions?: TenantRequestOptions): Promise<EvaluationProductListResponse<EvaluationCampaignAttemptGroupResource>> {
    return this.requestJSON<EvaluationProductListResponse<EvaluationCampaignAttemptGroupResource>>(`/v1/evaluation/campaigns/${campaignId.trim()}/attempt-groups`, { query, tenant: tenantOptions });
  }

  async listEvaluationDashboard(query: EvaluationProductTenantOptions = {}, tenantOptions?: TenantRequestOptions): Promise<EvaluationProductListResponse<EvaluationDashboardProjectionResource>> {
    return this.requestJSON<EvaluationProductListResponse<EvaluationDashboardProjectionResource>>("/v1/evaluation/dashboard", { query, tenant: tenantOptions });
  }

  async listEvaluationToolCallInspections(campaignId: string, query: EvaluationProductTenantOptions = {}, tenantOptions?: TenantRequestOptions): Promise<EvaluationProductListResponse<EvaluationToolCallInspectionResource>> {
    return this.requestJSON<EvaluationProductListResponse<EvaluationToolCallInspectionResource>>(`/v1/evaluation/campaigns/${campaignId.trim()}/tool-call-inspections`, { query, tenant: tenantOptions });
  }

  async getEvaluationToolCallInspection(inspectionId: string, tenantOptions?: TenantRequestOptions): Promise<EvaluationToolCallInspectionResource> {
    return this.requestJSON<EvaluationToolCallInspectionResource>(`/v1/evaluation/tool-call-inspections/${inspectionId.trim()}`, { tenant: tenantOptions });
  }

  async startLiveValidation(input: CreateLiveValidationInput, tenantOptions?: TenantRequestOptions): Promise<CreateLiveValidationResponse> {
    return this.requestJSON<CreateLiveValidationResponse>("/v1/live-validations", {
      method: "POST",
      body: input,
      tenant: tenantOptions
    });
  }

  async listLiveValidations(query: LiveValidationAttemptQuery = {}, tenantOptions?: TenantRequestOptions): Promise<LiveValidationAttemptListResponse> {
    return this.requestJSON<LiveValidationAttemptListResponse>("/v1/live-validations", { query, tenant: tenantOptions });
  }

  async getLiveValidation(validationId: string, tenantOptions?: TenantRequestOptions): Promise<LiveValidationAttemptResource> {
    return this.requestJSON<LiveValidationAttemptResource>(`/v1/live-validations/${validationId.trim()}`, { tenant: tenantOptions });
  }

  async abortLiveValidation(validationId: string, tenantOptions?: TenantRequestOptions): Promise<LiveValidationAttemptResource> {
    return this.requestJSON<LiveValidationAttemptResource>(`/v1/live-validations/${validationId.trim()}/abort`, {
      method: "POST",
      tenant: tenantOptions
    });
  }

  async listLiveValidationSupportMatrix(tenantOptions?: TenantRequestOptions): Promise<LiveValidationSupportMatrixResponse> {
    return this.requestJSON<LiveValidationSupportMatrixResponse>("/v1/live-validations/support-matrix", { tenant: tenantOptions });
  }

  async listLiveValidationLedger(validationId: string, query: { outcome?: LiveValidationLedgerOutcome; toolClass?: string; limit?: number } = {}, tenantOptions?: TenantRequestOptions): Promise<LiveValidationLedgerListResponse> {
    return this.requestJSON<LiveValidationLedgerListResponse>(`/v1/live-validations/${validationId.trim()}/ledger`, { query, tenant: tenantOptions });
  }

  async resolveLiveValidationReconciliation(validationId: string, ambiguousCommitId: string, input: ResolveLiveValidationReconciliationInput, tenantOptions?: TenantRequestOptions): Promise<LiveValidationReconciliationResource> {
    return this.requestJSON<LiveValidationReconciliationResource>(`/v1/live-validations/${validationId.trim()}/reconciliations/${ambiguousCommitId.trim()}/resolve`, {
      method: "POST",
      body: input,
      tenant: tenantOptions
    });
  }

  async getLiveValidationRetention(validationId: string, tenantOptions?: TenantRequestOptions): Promise<LiveValidationRetentionResource> {
    return this.requestJSON<LiveValidationRetentionResource>(`/v1/live-validations/${validationId.trim()}/retention`, { tenant: tenantOptions });
  }

  async createLiveValidationComparison(validationId: string, tenantOptions?: TenantRequestOptions): Promise<LiveValidationComparisonResource> {
    return this.requestJSON<LiveValidationComparisonResource>(`/v1/live-validations/${validationId.trim()}/compare`, {
      method: "POST",
      tenant: tenantOptions
    });
  }

  async listLiveValidationKillSwitches(query: { tenantId?: string; scope?: "tenant" | "global"; limit?: number } = {}, tenantOptions?: TenantRequestOptions): Promise<LiveValidationKillSwitchListResponse> {
    return this.requestJSON<LiveValidationKillSwitchListResponse>("/v1/live-validations/kill-switches", { query, tenant: tenantOptions });
  }

  async listIntegrationDiagnostics(integrationId: string, query: { capability?: string; includeStale?: boolean; limit?: number; cursor?: string } = {}, tenantOptions?: TenantRequestOptions): Promise<IntegrationDiagnosticListResponse> {
    return this.requestJSON<IntegrationDiagnosticListResponse>(`/v1/integrations/${integrationId.trim()}/diagnostics`, { query, tenant: tenantOptions });
  }

  async startIntegrationDiagnosticRun(integrationId: string, input: CreateIntegrationDiagnosticRunInput, tenantOptions?: TenantRequestOptions): Promise<IntegrationDiagnosticRunResource> {
    return this.requestJSON<IntegrationDiagnosticRunResource>(`/v1/integrations/${integrationId.trim()}/diagnostics/runs`, {
      method: "POST",
      body: input,
      tenant: tenantOptions
    });
  }

  async listIntegrationDiagnosticRuns(query: { integrationId?: string; providerKind?: string; domainKind?: string; status?: string; reasonCode?: IntegrationDiagnosticReasonCode; limit?: number; cursor?: string } = {}, tenantOptions?: TenantRequestOptions): Promise<{ items: IntegrationDiagnosticRunResource[]; nextCursor?: string }> {
    return this.requestJSON<{ items: IntegrationDiagnosticRunResource[]; nextCursor?: string }>("/v1/integration-diagnostics/runs", { query, tenant: tenantOptions });
  }

  async getIntegrationDiagnosticRun(runId: string, tenantOptions?: TenantRequestOptions): Promise<IntegrationDiagnosticRunResource> {
    return this.requestJSON<IntegrationDiagnosticRunResource>(`/v1/integration-diagnostics/runs/${runId.trim()}`, { tenant: tenantOptions });
  }

  async createIntegrationDiagnosticSmoke(input: CreateIntegrationDiagnosticSmokeInput, tenantOptions?: TenantRequestOptions): Promise<SmokeMatrixReportResource> {
    return this.requestJSON<SmokeMatrixReportResource>("/v1/integration-diagnostics/smoke", {
      method: "POST",
      body: input,
      tenant: tenantOptions
    });
  }

  async applyIntegrationDiagnosticRetention(query: { limit?: number } = {}, tenantOptions?: TenantRequestOptions): Promise<{ items: DiagnosticRetentionRecordResource[] }> {
    return this.requestJSON<{ items: DiagnosticRetentionRecordResource[] }>("/v1/integration-diagnostics/retention/apply", {
      method: "POST",
      query,
      tenant: tenantOptions
    });
  }

  async listIntegrationDiagnosticReasonCodes(tenantOptions?: TenantRequestOptions): Promise<{ items: IntegrationDiagnosticReasonCodeResource[] }> {
    return this.requestJSON<{ items: IntegrationDiagnosticReasonCodeResource[] }>("/v1/integration-diagnostics/reason-codes", { tenant: tenantOptions });
  }

  async updateLiveValidationKillSwitch(input: UpdateLiveValidationKillSwitchInput, tenantOptions?: TenantRequestOptions): Promise<LiveValidationKillSwitchResource> {
    return this.requestJSON<LiveValidationKillSwitchResource>("/v1/live-validations/kill-switches", {
      method: "POST",
      body: input,
      tenant: tenantOptions
    });
  }

  async listApprovals(status?: ApprovalStatus, tenantOptions?: TenantRequestOptions): Promise<ApprovalListResponse> {
    return this.requestJSON<ApprovalListResponse>("/v1/policy/approvals", {
      query: { status },
      tenant: tenantOptions
    });
  }

  async getApproval(approvalId: string, tenantOptions?: TenantRequestOptions): Promise<ApprovalResource> {
    return this.requestJSON<ApprovalResource>(`/v1/policy/approvals/${approvalId.trim()}`, { tenant: tenantOptions });
  }

  async resolveApproval(approvalId: string, input: ResolveApprovalInput, tenantOptions?: TenantRequestOptions): Promise<ApprovalDecisionResponse> {
    return this.requestJSON<ApprovalDecisionResponse>(`/v1/policy/approvals/${approvalId.trim()}/resolve`, {
      method: "POST",
      body: {
        resolution: input.resolution,
        comment: input.comment?.trim() || ""
      },
      tenant: tenantOptions
    });
  }

  async createRun(input: CreateRunInput, tenantOptions?: TenantRequestOptions): Promise<RunResource> {
    return this.requestJSON<RunResource>("/v1/runs", {
      method: "POST",
      body: {
        sessionId: input.sessionId?.trim() || undefined,
        route: input.route,
        entrypoint: input.entrypoint.trim(),
        goal: input.goal?.trim() || undefined,
        input: input.input
      },
      tenant: tenantOptions
    });
  }

  async getRun(runId: string, tenantOptions?: TenantRequestOptions): Promise<RunResource> {
    return this.requestJSON<RunResource>(`/v1/runs/${encodePathComponent(runId.trim())}`, { tenant: tenantOptions });
  }

  async fetchRoute<T>(route: string, tenantOptions?: TenantRequestOptions): Promise<T> {
    return this.requestJSON<T>(normalizeRoute(route), { tenant: tenantOptions });
  }

  async listAgentProfiles(query: { limit?: number } = {}, tenantOptions?: TenantRequestOptions): Promise<AgentProfileListResponse> {
    return this.requestJSON<AgentProfileListResponse>("/v1/profiles", { query, tenant: tenantOptions });
  }

  async getAgentProfile(profileId: string, tenantOptions?: TenantRequestOptions): Promise<AgentProfileDetailResponse> {
    return this.requestJSON<AgentProfileDetailResponse>(`/v1/profiles/${encodePathComponent(profileId)}`, { tenant: tenantOptions });
  }

  async createAgentProfile(input: AgentProfileMutationInput, tenantOptions?: TenantRequestOptions): Promise<AgentProfileMutationResult> {
    return this.requestJSON<AgentProfileMutationResult>("/v1/profiles", { method: "POST", body: input, tenant: tenantOptions });
  }

  async updateAgentProfile(profileId: string, input: AgentProfileMutationInput, tenantOptions?: TenantRequestOptions): Promise<AgentProfileMutationResult> {
    return this.requestJSON<AgentProfileMutationResult>(`/v1/profiles/${encodePathComponent(profileId)}`, { method: "PATCH", body: input, tenant: tenantOptions });
  }

  async activateAgentProfile(profileId: string, input: { profileVersionId?: string; reasonCode?: string } = {}, tenantOptions?: TenantRequestOptions): Promise<AgentProfileActiveSelection> {
    return this.requestJSON<AgentProfileActiveSelection>(`/v1/profiles/${encodePathComponent(profileId)}/activate`, { method: "POST", body: input, tenant: tenantOptions });
  }

  async listAgentProfileVersions(profileId: string, query: { limit?: number } = {}, tenantOptions?: TenantRequestOptions): Promise<{ items: AgentProfileVersionResource[] }> {
    return this.requestJSON<{ items: AgentProfileVersionResource[] }>(`/v1/profiles/${encodePathComponent(profileId)}/versions`, { query, tenant: tenantOptions });
  }

  async rollbackAgentProfile(profileId: string, input: { sourceProfileVersionId: string; reasonCode?: string }, tenantOptions?: TenantRequestOptions): Promise<AgentProfileMutationResult> {
    return this.requestJSON<AgentProfileMutationResult>(`/v1/profiles/${encodePathComponent(profileId)}/rollback`, { method: "POST", body: input, tenant: tenantOptions });
  }

  async archiveAgentProfile(profileId: string, input: { reasonCode?: string } = {}, tenantOptions?: TenantRequestOptions): Promise<AgentProfileMutationResult> {
    return this.requestJSON<AgentProfileMutationResult>(`/v1/profiles/${encodePathComponent(profileId)}/archive`, { method: "POST", body: input, tenant: tenantOptions });
  }

  async disableAgentProfile(profileId: string, input: { reasonCode?: string } = {}, tenantOptions?: TenantRequestOptions): Promise<AgentProfileMutationResult> {
    return this.requestJSON<AgentProfileMutationResult>(`/v1/profiles/${encodePathComponent(profileId)}/disable`, { method: "POST", body: input, tenant: tenantOptions });
  }

  // --- Roadmap 65: inbox triage ---

  async listTriagePolicies(tenantOptions?: TenantRequestOptions): Promise<{ items: TriagePolicyResource[] }> {
    return this.requestJSON<{ items: TriagePolicyResource[] }>("/v1/triage/policies", { tenant: tenantOptions });
  }

  async createTriagePolicy(input: CreateTriagePolicyInput, tenantOptions?: TenantRequestOptions): Promise<TriagePolicyResource> {
    return this.requestJSON<TriagePolicyResource>("/v1/triage/policies", { method: "POST", body: input, tenant: tenantOptions });
  }

  async getTriagePolicy(policyId: string, tenantOptions?: TenantRequestOptions): Promise<TriagePolicyResource> {
    return this.requestJSON<TriagePolicyResource>(`/v1/triage/policies/${encodePathComponent(policyId)}`, { tenant: tenantOptions });
  }

  async runTriagePolicy(policyId: string, input: { messages: TriageMessage[] }, tenantOptions?: TenantRequestOptions): Promise<TriageRunResource> {
    return this.requestJSON<TriageRunResource>(`/v1/triage/policies/${encodePathComponent(policyId)}/run`, { method: "POST", body: input, tenant: tenantOptions });
  }

  // --- Roadmap 66: routine builder ---


  // --- Memory plane (Roadmap 78, spec 058) ---

  async listMemoryAssets(
    query?: { layer?: MemoryLayer; status?: MemoryAssetStatus; tenantId?: string },
    tenantOptions?: TenantRequestOptions,
  ): Promise<{ items: MemoryAssetResource[] }> {
    const params = new URLSearchParams();
    if (query?.layer) params.set("layer", query.layer);
    if (query?.status) params.set("status", query.status);
    if (query?.tenantId) params.set("tenantId", query.tenantId);
    const suffix = params.size > 0 ? `?${params.toString()}` : "";
    return this.requestJSON<{ items: MemoryAssetResource[] }>(`/v1/memory/assets${suffix}`, { tenant: tenantOptions });
  }

  async createMemoryAsset(input: CreateMemoryAssetInput, tenantOptions?: TenantRequestOptions): Promise<MemoryAssetDecision> {
    return this.requestJSON<MemoryAssetDecision>("/v1/memory/assets", { method: "POST", body: input, tenant: tenantOptions });
  }

  async getMemoryAsset(assetId: string, tenantOptions?: TenantRequestOptions): Promise<MemoryAssetResource> {
    return this.requestJSON<MemoryAssetResource>(`/v1/memory/assets/${encodeURIComponent(assetId)}`, { tenant: tenantOptions });
  }

  async getMemoryDrilldown(assetId: string, tenantOptions?: TenantRequestOptions): Promise<MemoryDrilldownNode> {
    return this.requestJSON<MemoryDrilldownNode>(`/v1/memory/assets/${encodeURIComponent(assetId)}/drilldown`, { tenant: tenantOptions });
  }

  async approveMemoryAsset(assetId: string, actor: MemoryActor, tenantOptions?: TenantRequestOptions): Promise<MemoryAssetResource> {
    return this.requestJSON<MemoryAssetResource>(`/v1/memory/assets/${encodeURIComponent(assetId)}/approve`, { method: "POST", body: { actor }, tenant: tenantOptions });
  }

  async rejectMemoryAsset(assetId: string, reason: string, tenantOptions?: TenantRequestOptions): Promise<MemoryAssetResource> {
    return this.requestJSON<MemoryAssetResource>(`/v1/memory/assets/${encodeURIComponent(assetId)}/reject`, { method: "POST", body: { reason }, tenant: tenantOptions });
  }

  async revokeMemoryAsset(assetId: string, reason: string, tenantOptions?: TenantRequestOptions): Promise<MemoryAssetResource> {
    return this.requestJSON<MemoryAssetResource>(`/v1/memory/assets/${encodeURIComponent(assetId)}/revoke`, { method: "POST", body: { reason }, tenant: tenantOptions });
  }

  async setMemoryAssetVisibility(assetId: string, visibility: MemoryVisibility, tenantOptions?: TenantRequestOptions): Promise<MemoryAssetDecision> {
    return this.requestJSON<MemoryAssetDecision>(`/v1/memory/assets/${encodeURIComponent(assetId)}/visibility`, { method: "POST", body: { visibility }, tenant: tenantOptions });
  }

  /** Distils a skill proposal from a conversation as a visible, steerable chat turn (Stage 3.1). */
  async distillSkillProposal(input: SkillDistillInput, tenantOptions?: TenantRequestOptions): Promise<SkillDistillResult> {
    return this.requestJSON<SkillDistillResult>("/v1/skills/proposals/distill", { method: "POST", body: input, tenant: tenantOptions });
  }

  /** "Which skill covers this" — fused ranking over the installed registry (Stage 3.2). */
  async searchSkills(q: string, tenantOptions?: TenantRequestOptions): Promise<{ items: SkillSearchHit[] }> {
    return this.requestJSON<{ items: SkillSearchHit[] }>(`/v1/skills/search?q=${encodeURIComponent(q)}`, { tenant: tenantOptions });
  }

  async getSkillUsage(skillId: string, tenantOptions?: TenantRequestOptions): Promise<SkillUsage> {
    return this.requestJSON<SkillUsage>(`/v1/skills/${encodeURIComponent(skillId)}/usage`, { tenant: tenantOptions });
  }

  async recordSkillFeedback(skillId: string, outcome: "helpful" | "corrected", tenantOptions?: TenantRequestOptions): Promise<SkillUsage> {
    return this.requestJSON<SkillUsage>(`/v1/skills/${encodeURIComponent(skillId)}/feedback`, { method: "POST", body: { outcome }, tenant: tenantOptions });
  }

  /** Launches a bounded fan-out (202). Refused until the swarm plugin is opted in. */
  async launchSwarmRun(input: SwarmLaunchInput, tenantOptions?: TenantRequestOptions): Promise<SwarmRun> {
    return this.requestJSON<SwarmRun>("/v1/swarm/runs", { method: "POST", body: input, tenant: tenantOptions });
  }

  async listSwarmRuns(tenantOptions?: TenantRequestOptions): Promise<{ items: SwarmRun[] }> {
    return this.requestJSON<{ items: SwarmRun[] }>("/v1/swarm/runs", { tenant: tenantOptions });
  }

  async getSwarmRun(runId: string, tenantOptions?: TenantRequestOptions): Promise<SwarmRun> {
    return this.requestJSON<SwarmRun>(`/v1/swarm/runs/${encodeURIComponent(runId)}`, { tenant: tenantOptions });
  }

  async getSessionFrame(threadId: string, tenantOptions?: TenantRequestOptions): Promise<SessionFrame> {
    return this.requestJSON<SessionFrame>(`/v1/threads/${encodeURIComponent(threadId)}/frame`, { tenant: tenantOptions });
  }

  async setSessionFrame(threadId: string, input: SessionFrameInput, tenantOptions?: TenantRequestOptions): Promise<SessionFrame> {
    return this.requestJSON<SessionFrame>(`/v1/threads/${encodeURIComponent(threadId)}/frame`, { method: "PUT", body: input, tenant: tenantOptions });
  }

  async clearSessionFrame(threadId: string, tenantOptions?: TenantRequestOptions): Promise<void> {
    await this.requestJSON<void>(`/v1/threads/${encodeURIComponent(threadId)}/frame`, { method: "DELETE", tenant: tenantOptions });
  }

  async getConfigFile(tenantOptions?: TenantRequestOptions): Promise<ConfigFile> {
    return this.requestJSON<ConfigFile>("/v1/config/file", { tenant: tenantOptions });
  }

  /**
   * Writes config.json through the daemon: validated, compare-and-set,
   * atomic. Inline secret values are refused — use the `*Env` indirection or
   * a secret reference. Nothing hot-applies today; the result says so.
   */
  async writeConfigFile(input: ConfigFileWriteInput, tenantOptions?: TenantRequestOptions): Promise<ConfigFileWriteResult> {
    return this.requestJSON<ConfigFileWriteResult>("/v1/config/file", { method: "PUT", body: input, tenant: tenantOptions });
  }

  /** What memory the model actually saw, newest first. The one-call answer during an incident. */
  async listContextAssemblies(
    query?: { tenantId?: string; threadId?: string; limit?: number },
    tenantOptions?: TenantRequestOptions,
  ): Promise<{ items: ContextAssembly[] }> {
    const params = new URLSearchParams();
    if (query?.tenantId) params.set("tenantId", query.tenantId);
    if (query?.threadId) params.set("threadId", query.threadId);
    if (query?.limit) params.set("limit", String(query.limit));
    const suffix = params.size > 0 ? `?${params.toString()}` : "";
    return this.requestJSON<{ items: ContextAssembly[] }>(`/v1/context/assemblies${suffix}`, { tenant: tenantOptions });
  }

  async getMemoryOverview(query?: { tenantId?: string }, tenantOptions?: TenantRequestOptions): Promise<MemoryOverview> {
    const params = new URLSearchParams();
    if (query?.tenantId) params.set("tenantId", query.tenantId);
    const suffix = params.size > 0 ? `?${params.toString()}` : "";
    return this.requestJSON<MemoryOverview>(`/v1/memory/overview${suffix}`, { tenant: tenantOptions });
  }

  /**
   * Drops the tenant's derived retrieval index so it is recomputed from the
   * assets. Conversation truth and the assets themselves are untouched — this
   * is the repair path, not a reset.
   */
  async rebuildMemoryIndexes(query?: { tenantId?: string }, tenantOptions?: TenantRequestOptions): Promise<MemoryIndexRebuildResult> {
    const params = new URLSearchParams();
    if (query?.tenantId) params.set("tenantId", query.tenantId);
    const suffix = params.size > 0 ? `?${params.toString()}` : "";
    return this.requestJSON<MemoryIndexRebuildResult>(`/v1/memory/indexes/rebuild${suffix}`, { method: "POST", tenant: tenantOptions });
  }

  async consolidateMemory(input?: { tenantId?: string; trigger?: string }, tenantOptions?: TenantRequestOptions): Promise<MemoryConsolidationRun> {
    return this.requestJSON<MemoryConsolidationRun>("/v1/memory/consolidate", { method: "POST", body: input ?? {}, tenant: tenantOptions });
  }

  /** What the agent could do in this deployment, configured or not. */
  async listToolCapabilities(query?: { tenantId?: string }, tenantOptions?: TenantRequestOptions): Promise<{ items: ToolCapabilityStatus[] }> {
    const params = new URLSearchParams();
    if (query?.tenantId) params.set("tenantId", query.tenantId);
    const suffix = params.size > 0 ? `?${params.toString()}` : "";
    return this.requestJSON<{ items: ToolCapabilityStatus[] }>(`/v1/tools/capabilities${suffix}`, { tenant: tenantOptions });
  }

  async listToolProfiles(query?: { tenantId?: string; capability?: ToolCapability }, tenantOptions?: TenantRequestOptions): Promise<{ items: ToolProfileResource[] }> {
    const params = new URLSearchParams();
    if (query?.tenantId) params.set("tenantId", query.tenantId);
    if (query?.capability) params.set("capability", query.capability);
    const suffix = params.size > 0 ? `?${params.toString()}` : "";
    return this.requestJSON<{ items: ToolProfileResource[] }>(`/v1/tools/profiles${suffix}`, { tenant: tenantOptions });
  }

  async createToolProfile(input: CreateToolProfileInput, tenantOptions?: TenantRequestOptions): Promise<ToolProfileResource> {
    return this.requestJSON<ToolProfileResource>("/v1/tools/profiles", { method: "POST", body: input, tenant: tenantOptions });
  }

  async updateToolProfile(profileId: string, input: UpdateToolProfileInput, tenantOptions?: TenantRequestOptions): Promise<ToolProfileResource> {
    return this.requestJSON<ToolProfileResource>(`/v1/tools/profiles/${encodeURIComponent(profileId)}`, { method: "PATCH", body: input, tenant: tenantOptions });
  }

  async deleteToolProfile(profileId: string, tenantOptions?: TenantRequestOptions): Promise<void> {
    await this.requestJSON<void>(`/v1/tools/profiles/${encodeURIComponent(profileId)}`, { method: "DELETE", tenant: tenantOptions });
  }

  /**
   * Preflight a profile. Surfaces a mistyped credential where the operator is
   * standing rather than as a failed turn in front of the user.
   */
  async checkToolProfile(profileId: string, tenantOptions?: TenantRequestOptions): Promise<ToolCheckOutcome> {
    return this.requestJSON<ToolCheckOutcome>(`/v1/tools/profiles/${encodeURIComponent(profileId)}/check`, { method: "POST", tenant: tenantOptions });
  }

  async listPlugins(tenantOptions?: TenantRequestOptions): Promise<PluginsReport> {
    return this.requestJSON<PluginsReport>("/v1/plugins", { tenant: tenantOptions });
  }

  async getPluginProfile(tenantOptions?: TenantRequestOptions): Promise<PluginProfile> {
    return this.requestJSON<PluginProfile>("/v1/plugins/profile", { tenant: tenantOptions });
  }

  async updatePluginProfile(profile: PluginProfile, tenantOptions?: TenantRequestOptions): Promise<PluginProfileUpdateResponse> {
    return this.requestJSON<PluginProfileUpdateResponse>("/v1/plugins/profile", { method: "PUT", body: profile, tenant: tenantOptions });
  }

  async queryRetrieval(input: { query: string; tenantId?: string; limit?: number }, tenantOptions?: TenantRequestOptions): Promise<RetrievalQueryResponse> {
    return this.requestJSON<RetrievalQueryResponse>("/v1/retrieval/queries", { method: "POST", body: input, tenant: tenantOptions });
  }

  async listRoutines(tenantOptions?: TenantRequestOptions): Promise<{ items: RoutineResource[] }> {
    return this.requestJSON<{ items: RoutineResource[] }>("/v1/routines", { tenant: tenantOptions });
  }

  async createRoutine(input: { definition: RoutineDefinition }, tenantOptions?: TenantRequestOptions): Promise<RoutineResource> {
    return this.requestJSON<RoutineResource>("/v1/routines", { method: "POST", body: input, tenant: tenantOptions });
  }

  async previewRoutine(input: { definition: RoutineDefinition }, tenantOptions?: TenantRequestOptions): Promise<RoutinePreview> {
    return this.requestJSON<RoutinePreview>("/v1/routines/preview", { method: "POST", body: input, tenant: tenantOptions });
  }

  async getRoutine(routineId: string, tenantOptions?: TenantRequestOptions): Promise<RoutineResource> {
    return this.requestJSON<RoutineResource>(`/v1/routines/${encodePathComponent(routineId)}`, { tenant: tenantOptions });
  }

  async updateRoutine(routineId: string, input: { definition: RoutineDefinition }, tenantOptions?: TenantRequestOptions): Promise<RoutineResource> {
    return this.requestJSON<RoutineResource>(`/v1/routines/${encodePathComponent(routineId)}`, { method: "PUT", body: input, tenant: tenantOptions });
  }

  async routineLifecycle(routineId: string, action: "pause" | "resume" | "cancel" | "repair", tenantOptions?: TenantRequestOptions): Promise<RoutineResource> {
    return this.requestJSON<RoutineResource>(`/v1/routines/${encodePathComponent(routineId)}/${action}`, { method: "POST", tenant: tenantOptions });
  }

  // --- Roadmap 67: webhook trigger plane ---

  async listWebhooks(tenantOptions?: TenantRequestOptions): Promise<{ items: WebhookEndpointResource[] }> {
    return this.requestJSON<{ items: WebhookEndpointResource[] }>("/v1/webhooks", { tenant: tenantOptions });
  }

  async createWebhook(input: CreateWebhookInput, tenantOptions?: TenantRequestOptions): Promise<WebhookCreateResult> {
    return this.requestJSON<WebhookCreateResult>("/v1/webhooks", { method: "POST", body: input, tenant: tenantOptions });
  }

  async getWebhook(webhookId: string, tenantOptions?: TenantRequestOptions): Promise<WebhookEndpointResource> {
    return this.requestJSON<WebhookEndpointResource>(`/v1/webhooks/${encodePathComponent(webhookId)}`, { tenant: tenantOptions });
  }

  async rotateWebhook(webhookId: string, tenantOptions?: TenantRequestOptions): Promise<WebhookCreateResult> {
    return this.requestJSON<WebhookCreateResult>(`/v1/webhooks/${encodePathComponent(webhookId)}/rotate`, { method: "POST", tenant: tenantOptions });
  }

  async disableWebhook(webhookId: string, tenantOptions?: TenantRequestOptions): Promise<WebhookEndpointResource> {
    return this.requestJSON<WebhookEndpointResource>(`/v1/webhooks/${encodePathComponent(webhookId)}/disable`, { method: "POST", tenant: tenantOptions });
  }

  // --- Roadmap 68: operator-managed catalog ---

  async listCatalogItems(tenantOptions?: TenantRequestOptions): Promise<{ items: CatalogItemResource[] }> {
    return this.requestJSON<{ items: CatalogItemResource[] }>("/v1/catalog/items", { tenant: tenantOptions });
  }

  async inspectCatalogItem(itemId: string, tenantOptions?: TenantRequestOptions): Promise<CatalogInspection> {
    return this.requestJSON<CatalogInspection>(`/v1/catalog/items/${encodePathComponent(itemId)}`, { tenant: tenantOptions });
  }

  async catalogItemLifecycle(itemId: string, action: "enable" | "disable" | "rollback", input: { version?: string; actor?: string } = {}, tenantOptions?: TenantRequestOptions): Promise<CatalogEnablementResource> {
    return this.requestJSON<CatalogEnablementResource>(`/v1/catalog/items/${encodePathComponent(itemId)}/${action}`, { method: "POST", body: input, tenant: tenantOptions });
  }

  // --- Roadmap 69: execution backend + sandbox profile UX ---

  async listExecutionProfiles(tenantOptions?: TenantRequestOptions): Promise<{ items: ExecutionProfileResource[] }> {
    return this.requestJSON<{ items: ExecutionProfileResource[] }>("/v1/execution/profiles", { tenant: tenantOptions });
  }

  async getExecutionProfile(profileId: string, tenantOptions?: TenantRequestOptions): Promise<ExecutionProfileResource> {
    return this.requestJSON<ExecutionProfileResource>(`/v1/execution/profiles/${encodePathComponent(profileId)}`, { tenant: tenantOptions });
  }

  async selectExecutionProfile(profileId: string, input: { actor?: string } = {}, tenantOptions?: TenantRequestOptions): Promise<unknown> {
    return this.requestJSON<unknown>(`/v1/execution/profiles/${encodePathComponent(profileId)}/select`, { method: "POST", body: input, tenant: tenantOptions });
  }

  async explainExecution(input: { requiredCapabilities: string[] }, tenantOptions?: TenantRequestOptions): Promise<ExecutionDenialExplanation> {
    return this.requestJSON<ExecutionDenialExplanation>("/v1/execution/explain", { method: "POST", body: input, tenant: tenantOptions });
  }

  // --- Roadmap 71: support diagnostics evidence bundle ---

  async listEvidenceBundles(query: { tenantId?: string; actor?: string } = {}, tenantOptions?: TenantRequestOptions): Promise<{ items: EvidenceBundleResource[] }> {
    return this.requestJSON<{ items: EvidenceBundleResource[] }>("/v1/support/evidence-bundles", { query, tenant: tenantOptions });
  }

  async generateEvidenceBundle(input: GenerateEvidenceBundleInput, tenantOptions?: TenantRequestOptions): Promise<EvidenceBundleResource> {
    return this.requestJSON<EvidenceBundleResource>("/v1/support/evidence-bundles", { method: "POST", body: input, tenant: tenantOptions });
  }

  async getEvidenceBundle(bundleId: string, query: { tenantId?: string; actor?: string } = {}, tenantOptions?: TenantRequestOptions): Promise<EvidenceBundleResource> {
    return this.requestJSON<EvidenceBundleResource>(`/v1/support/evidence-bundles/${encodePathComponent(bundleId)}`, { query, tenant: tenantOptions });
  }

  // --- Roadmap 72: public release launch gate ---

  async validateLaunchGate(evidence: LaunchGateEvidenceInput, tenantOptions?: TenantRequestOptions): Promise<LaunchGateDecision> {
    return this.requestJSON<LaunchGateDecision>("/v1/release/launch-gate", { method: "POST", body: evidence, tenant: tenantOptions });
  }

  streamEvents(query: EventStreamQuery = {}, handlers: EventStreamHandlers = {}, tenantOptions?: TenantRequestOptions): EventStreamSubscription {
    const controller = new AbortController();
    const completed = (async () => {
      const response = await this.fetchImpl(this.buildURL("/v1/events/stream", query), {
        method: "GET",
        headers: this.buildHeaders(tenantOptions),
        signal: controller.signal
      });

      if (!response.ok) {
        throw await toClientError(response);
      }
      if (!response.body) {
        throw new KuraClientError("event stream response body is missing", { status: 500, code: "stream_body_missing" });
      }

      await readSSE(response.body, (event) => {
        handlers.onEvent?.(JSON.parse(event.data) as DaemonEvent);
      });
    })().catch((error: unknown) => {
      if (controller.signal.aborted) {
        return;
      }
      handlers.onError?.(error instanceof Error ? error : new Error(String(error)));
    });

    return {
      close() {
        controller.abort();
      },
      completed
    };
  }

  private async requestJSON<T>(
    route: string,
    options: {
      method?: string;
      query?: Record<string, QueryValue>;
      body?: unknown;
      tenant?: TenantRequestOptions;
    } = {}
  ): Promise<T> {
    const response = await this.fetchImpl(this.buildURL(route, options.query), {
      method: options.method ?? "GET",
      headers: this.buildHeaders(options.tenant),
      body: options.body === undefined ? undefined : JSON.stringify(options.body)
    });

    if (!response.ok) {
      throw await toClientError(response);
    }
    if (response.status === 204) {
      return undefined as T;
    }
    const text = await response.text();
    if (text.length === 0) {
      return undefined as T;
    }
    return JSON.parse(text) as T;
  }

  private buildURL(route: string, query?: Record<string, QueryValue>): string {
    const url = new URL(`${this.baseURL}${normalizeRoute(route)}`);
    if (!query) {
      return url.toString();
    }

    for (const [key, value] of Object.entries(query)) {
      if (value === undefined || value === null || value === "") {
        continue;
      }
      url.searchParams.set(key, String(value));
    }
    return url.toString();
  }

  private buildHeaders(tenantOptions?: TenantRequestOptions): HeadersInit {
    const headers: Record<string, string> = {
      "Content-Type": "application/json"
    };
    if (this.accessToken) {
      headers.Authorization = `Bearer ${this.accessToken}`;
    }
    const tenantId = this.resolveTenantId(tenantOptions);
    if (tenantId) {
      headers["X-Kura-Tenant-ID"] = tenantId;
    }
    return headers;
  }

  private resolveTenantId(tenantOptions?: TenantRequestOptions): string | undefined {
    return tenantOptions?.tenantId?.trim() || this.defaultTenantId;
  }
}

export function createKuraClient(options: KuraClientOptions): KuraClient {
  return new KuraClient(options);
}

type QueryValue = string | number | boolean | undefined | null;

function trimBaseURL(value: string): string {
  const trimmed = value.trim();
  if (!trimmed) {
    throw new Error("baseURL is required");
  }
  return trimmed.replace(/\/+$/, "");
}

function normalizeRoute(route: string): string {
  const trimmed = route.trim();
  if (!trimmed) {
    throw new Error("route is required");
  }
  return trimmed.startsWith("/") ? trimmed : `/${trimmed}`;
}

function encodePathComponent(value: string): string {
  const trimmed = value.trim();
  if (!trimmed) {
    throw new Error("path value is required");
  }
  return encodeURIComponent(trimmed);
}

function normalizeChatInput(input: ChatQueryInput): ChatQueryInput {
  return {
    provider: input.provider?.trim() || undefined,
    model: input.model?.trim() || undefined,
    skills: input.skills?.map((item) => item.trim()).filter(Boolean),
    query: input.query.trim(),
    timeoutMs: input.timeoutMs,
    maxRetries: input.maxRetries,
    threadId: input.threadId?.trim() || undefined,
    continuity: input.continuity?.mode ? { mode: input.continuity.mode } : undefined
  };
}

function normalizeActivationInput(input: RunActivationInput): RunActivationInput {
  return {
    source: input.source?.trim() || undefined
  };
}

function normalizeActivationTestChatInput(input: RunActivationTestChatInput): RunActivationTestChatInput {
  return {
    message: input.message.trim()
  };
}

function normalizeStartSetupInput(input: StartSetupInput): StartSetupInput {
  return {
    targetId: input.targetId.trim(),
    setupStyle: input.setupStyle,
    source: input.source?.trim() || undefined
  };
}

async function toClientError(response: Response): Promise<KuraClientError> {
  let message = `request failed with status ${response.status}`;
  let code: string | undefined;
  let denial: TenantDenialResource | undefined;
  let quotaDenial: BillingQuotaDenialPayload | undefined;
  let activationFailure: ActivationFailurePayload | undefined;

  try {
    const payload = (await response.json()) as {
      error?: string;
      errorCode?: string;
      requestId?: string;
      code?: string;
      reasonCode?: string;
      stage?: string;
      retryable?: boolean;
      remediationOwner?: string;
    } & Partial<BillingQuotaDenialPayload>;
    if (payload.error) {
      message = payload.error;
    }
    if (payload.errorCode || payload.reasonCode || payload.code) {
      code = payload.errorCode ?? payload.reasonCode ?? payload.code;
    }
    if (isTenantDenialCode(payload.errorCode) && payload.errorCode) {
      denial = {
        error: payload.error ?? "tenant access denied",
        errorCode: payload.errorCode,
        requestId: payload.requestId
      };
    }
    if (payload.code === "quota_denied" && payload.reasonCode) {
      quotaDenial = {
        code: "quota_denied",
        reasonCode: payload.reasonCode,
        tenantId: payload.tenantId,
        category: payload.category,
        operationKey: payload.operationKey,
        requestedAmount: payload.requestedAmount,
        remainingAmount: payload.remainingAmount,
        periodStart: payload.periodStart,
        periodEnd: payload.periodEnd
      };
      code = payload.reasonCode;
    }
    if (payload.reasonCode?.startsWith("activation_") && payload.stage && typeof payload.retryable === "boolean" && payload.remediationOwner) {
      activationFailure = {
        code: payload.code as ActivationReasonCode | undefined,
        reasonCode: payload.reasonCode as ActivationReasonCode,
        stage: payload.stage,
        retryable: payload.retryable,
        remediationOwner: payload.remediationOwner as ActivationRemediationOwner
      };
      code = payload.reasonCode;
    }
  } catch {
    // Ignore non-json failure bodies.
  }

  const tenantDenied = Boolean(denial) || isTenantDenialCode(code);
  return new KuraClientError(message, { status: response.status, code, tenantDenied: tenantDenied || undefined, denial, quotaDenial, activationFailure });
}

function isTenantDenialCode(code: string | undefined): boolean {
  return code === "tenant_access_denied" || code === "tenant_permission_denied" || code === "tenant_ownership_denied" || code?.startsWith("credential_denied:") === true || code?.startsWith("setup_denied:") === true;
}

type SSEEvent = {
  event: string;
  data: string;
};

async function readSSE(stream: ReadableStream<Uint8Array>, onEvent: (event: SSEEvent) => void | Promise<void>): Promise<void> {
  const reader = stream.getReader();
  const decoder = new TextDecoder();
  let buffer = "";

  while (true) {
    const { done, value } = await reader.read();
    if (done) {
      break;
    }
    buffer += decoder.decode(value, { stream: true });
    const parts = buffer.split("\n\n");
    buffer = parts.pop() ?? "";
    for (const part of parts) {
      const parsed = parseSSEEvent(part);
      if (parsed) {
        await onEvent(parsed);
      }
    }
  }

  buffer += decoder.decode();
  if (buffer.trim()) {
    const parsed = parseSSEEvent(buffer);
    if (parsed) {
      await onEvent(parsed);
    }
  }
}

function parseSSEEvent(chunk: string): SSEEvent | null {
  const lines = chunk
    .split("\n")
    .map((line) => line.trimEnd())
    .filter((line) => line !== "");

  let event = "";
  const data: string[] = [];

  for (const line of lines) {
    if (line.startsWith(":")) {
      continue;
    }
    if (line.startsWith("event:")) {
      event = line.slice("event:".length).trim();
      continue;
    }
    if (line.startsWith("data:")) {
      data.push(line.slice("data:".length).trim());
    }
  }

  if (!event || data.length === 0) {
    return null;
  }

  return {
    event,
    data: data.join("\n")
  };
}
