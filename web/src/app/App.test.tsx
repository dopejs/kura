import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { App } from "./App";

const mockClient = {
  getMe: vi.fn(),
  listTenants: vi.fn(),
  getMemoryOverview: vi.fn(),
  rebuildMemoryIndexes: vi.fn(),
  getOnboarding: vi.fn(),
  getActivation: vi.fn(),
  getBillingQuotaDashboard: vi.fn(),
  listBillingDenials: vi.fn(),
  getBillingDenialDetail: vi.fn(),
  exportBillingDenialEvidence: vi.fn(),
  activate: vi.fn(),
  runActivationTestChat: vi.fn(),
  getActivationDiagnostics: vi.fn(),
  listSetupTargets: vi.fn(),
  listSetupSessions: vi.fn(),
  startSetup: vi.fn(),
  getSetupSession: vi.fn(),
  submitSetupSecret: vi.fn(),
  startSetupOAuth: vi.fn(),
  completeSetupOAuth: vi.fn(),
  retrySetup: vi.fn(),
  replaceSetup: vi.fn(),
  cancelSetup: vi.fn(),
  disableSetup: vi.fn(),
  getSetupDiagnostics: vi.fn(),
  listApprovals: vi.fn(),
  getActivity: vi.fn(),
  getDiagnostics: vi.fn(),
  resolveApproval: vi.fn(),
  fetchRoute: vi.fn(),
  createRun: vi.fn(),
  queryChat: vi.fn(),
  listReplayCandidates: vi.fn(),
  listEvaluationDiscoveryPolicies: vi.fn(),
  listEvaluationDiscoveryRuns: vi.fn(),
  listEvaluationDiscoveredCandidates: vi.fn(),
  getEvaluationDiscoveredCandidate: vi.fn(),
  createEvaluationSuppression: vi.fn(),
  materializeProductFixture: vi.fn(),
  listProductFixtures: vi.fn(),
  createProductFixtureRevision: vi.fn(),
  reviewProductFixture: vi.fn(),
  suppressProductFixture: vi.fn(),
  createEvaluationCampaign: vi.fn(),
  listEvaluationCampaigns: vi.fn(),
  getEvaluationCampaign: vi.fn(),
  startEvaluationCampaign: vi.fn(),
  cancelEvaluationCampaign: vi.fn(),
  publishEvaluationCampaignResults: vi.fn(),
  listEvaluationCampaignItems: vi.fn(),
  listEvaluationCampaignAttemptGroups: vi.fn(),
  listEvaluationDashboard: vi.fn(),
  listEvaluationToolCallInspections: vi.fn(),
  getEvaluationToolCallInspection: vi.fn(),
  getReplayCandidate: vi.fn(),
  createReplayAttempt: vi.fn(),
  listReplayAttempts: vi.fn(),
  getReplayAttempt: vi.fn(),
  createReplayComparison: vi.fn(),
  listReplayComparisons: vi.fn(),
  getReplayComparison: vi.fn(),
  listReplayFixtures: vi.fn(),
  startLiveValidation: vi.fn(),
  listLiveValidations: vi.fn(),
  listLiveValidationSupportMatrix: vi.fn(),
  listLiveValidationLedger: vi.fn(),
  getLiveValidationRetention: vi.fn(),
  listLiveValidationKillSwitches: vi.fn(),
  updateLiveValidationKillSwitch: vi.fn(),
  getLiveValidation: vi.fn(),
  abortLiveValidation: vi.fn(),
  listMemberships: vi.fn(),
  updateMembershipRole: vi.fn(),
  listChannelConnectors: vi.fn(),
  listThreads: vi.fn(),
  getChannelConnector: vi.fn(),
  getChannelConnectorSupportEvidence: vi.fn(),
  disableChannelConnector: vi.fn(),
  reEnableChannelConnector: vi.fn(),
  startChannelConnectorRepair: vi.fn(),
  updateChannelRoutePolicy: vi.fn(),
  streamEvents: vi.fn()
};

const createdClientOptions: unknown[] = [];

vi.mock("@kura/client", () => ({
  createKuraClient: (options: unknown) => {
    createdClientOptions.push(options);
    return mockClient;
  }
}));

const emptySubscription = {
  close: vi.fn(),
  completed: Promise.resolve()
};

function onboardingFixture(overrides: Partial<ReturnType<typeof onboardingFixtureBase>> = {}) {
  return {
    ...onboardingFixtureBase(),
    ...overrides
  };
}

function tenantFixture(overrides: Partial<ReturnType<typeof tenantFixtureBase>> = {}) {
  return {
    ...tenantFixtureBase(),
    ...overrides
  };
}

function tenantFixtureBase() {
  return {
    tenantId: "ten_personal",
    tenantKind: "personal",
    displayName: "Personal Tenant",
    status: "active",
    createdAt: "2026-04-24T10:00:00Z",
    updatedAt: "2026-04-24T10:00:00Z",
    callerMembershipRole: "owner",
    callerMembershipStatus: "active",
    callerPermissions: ["tenant.manage", "runs.execute", "approvals.resolve", "evaluation.manage", "live_validation.execute"],
    defaultForCurrentToken: true,
    defaultForCurrentPrincipal: true
  };
}

function authMeFixture(tenants = [tenantFixture()]) {
  return {
    token: { tokenId: "tok_1" },
    principal: {
      principalId: "prn_1",
      principalKind: "local_operator",
      displayName: "Local Operator",
      status: "active",
      defaultTenantId: tenants[0].tenantId,
      createdAt: "2026-04-24T10:00:00Z",
      updatedAt: "2026-04-24T10:00:00Z"
    },
    defaultTenant: tenants[0],
    currentTenant: tenants[0],
    allowedTenants: tenants,
    tokenGrants: [],
    permissions: tenants[0].callerPermissions,
    tenantContext: {
      principalId: "prn_1",
      tokenId: "tok_1",
      tenantId: tenants[0].tenantId,
      tenantSource: "default",
      membershipId: "mem_1",
      role: tenants[0].callerMembershipRole,
      permissions: tenants[0].callerPermissions,
      resolvedAt: "2026-04-24T10:00:00Z"
    }
  };
}

function authMeWithoutTenantFixture() {
  return {
    token: { tokenId: "tok_1" },
    principal: {
      principalId: "prn_1",
      principalKind: "local_operator",
      displayName: "Local Operator",
      status: "active",
      defaultTenantId: "",
      createdAt: "2026-04-24T10:00:00Z",
      updatedAt: "2026-04-24T10:00:00Z"
    },
    defaultTenant: null,
    currentTenant: null,
    allowedTenants: [],
    tokenGrants: [],
    permissions: [],
    tenantContext: null
  };
}

function activationFixture(overrides: Record<string, unknown> = {}) {
  return {
    activation: {
      activationId: "act_1",
      principalId: "prn_1",
      tenantId: "ten_personal",
      environmentScope: "test",
      status: "active",
      currentStepId: "test_chat",
      completedStepIds: ["tenant_resolved", "quota_baseline_ready"],
      blockingReasonCodes: [],
      readinessItems: [],
      quotaBaseline: {
        tenantId: "ten_personal",
        planKey: "free",
        enforcementMode: "enforced",
        status: "available",
        quotas: [{
          category: "run_launches",
          unit: "count",
          limit: 10,
          used: 2,
          remaining: 8,
          period: "2026-05-01T00:00:00Z/2026-06-01T00:00:00Z"
        }]
      },
      firstAction: {
        actionId: "test_chat",
        actionKind: "test_chat",
        displayName: "Test chat",
        recommended: true,
        available: true,
        blockingItemIds: [],
        invokeRoute: "/v1/activation/test-chat",
        resultRoute: "/v1/activation"
      },
      lastEvaluatedAt: "2026-05-06T00:00:00Z",
      ...overrides
    }
  };
}

function billingQuotaDashboardFixture(overrides: Record<string, unknown> = {}) {
  return {
    tenantId: "ten_personal",
    plan: {
      planKey: "finite",
      enforcementMode: "enforced",
      status: "active",
      effectiveAt: "2026-05-07T10:00:00Z",
      basePlanLabel: "Finite",
      checkoutAvailable: false
    },
    sections: [{
      sectionKey: "launches",
      label: "Launches",
      items: [{
        category: "run_launches",
        unit: "count",
        status: "near_limit",
        currentPeriod: {
          periodStart: "2026-05-01T00:00:00Z",
          periodEnd: "2026-06-01T00:00:00Z",
          periodAnchor: "UTC",
          consumedAmount: 8,
          reservedAmount: 0,
          adjustedAmount: 0,
          carryoverApplied: 0,
          remainingAmount: 2,
          overLimit: false
        },
        previousPeriod: {
          periodStart: "2026-04-01T00:00:00Z",
          periodEnd: "2026-05-01T00:00:00Z",
          periodAnchor: "UTC",
          consumedAmount: 5,
          reservedAmount: 0,
          adjustedAmount: 0,
          carryoverApplied: 0,
          remainingAmount: 5,
          overLimit: false
        },
        limit: 10,
        remainingAmount: 2,
        nearLimit: true,
        nearLimitReason: "percent_threshold",
        typicalOperationAmount: 1,
        override: {
          baseLimit: 10,
          effectiveLimit: 8,
          reason: "support override",
          effectiveAt: "2026-05-07T09:00:00Z"
        },
        restriction: {
          restrictionId: "restriction_1",
          status: "active",
          affectedCategory: "run_launches",
          recoveryAction: "contact_support",
          visibleReasonCode: "abuse_restriction:temporary",
          supportContactAllowed: true
        },
        recoveryActions: ["wait", "reduce_scope"]
      }]
    }],
    generatedAt: "2026-05-07T10:00:00Z",
    ...overrides
  };
}

function billingDenialFixture(overrides: Record<string, unknown> = {}) {
  return {
    denialId: "denial_1",
    tenantId: "ten_personal",
    category: "run_launches",
    quotaPeriodId: "period_1",
    operationKey: "tenant:ten_personal:run:client_1",
    reasonCode: "quota_denied:run_launches_exhausted",
    requestedAmount: 1,
    remainingAmount: 0,
    guardedEntryPoint: "POST /v1/runs",
    createdAt: "2026-05-07T10:00:00Z",
    ...overrides
  };
}

function setupTargetFixture(overrides: Record<string, unknown> = {}) {
  return {
    targetId: "provider.openai_compatible",
    tenantId: "ten_personal",
    targetKind: "provider",
    setupStyle: "submitted_secret",
    displayName: "OpenAI-compatible provider",
    proofTarget: true,
    supportStatus: "supported",
    requiredPermissions: ["secrets.manage", "integrations.manage"],
    limitedSafeCapabilities: ["metadata_read"],
    ...overrides
  };
}

function setupSessionFixture(overrides: Record<string, unknown> = {}) {
  return {
    setupSessionId: "setup_1",
    tenantId: "ten_personal",
    actorPrincipalId: "prn_1",
    targetId: "provider.openai_compatible",
    targetKind: "provider",
    setupStyle: "submitted_secret",
    state: "in_progress",
    retryable: true,
    remediationOwner: "product_user",
    safeUseMode: "blocked",
    allowedCapabilities: [],
    redactionStatus: "redacted",
    createdAt: "2026-05-06T00:00:00Z",
    updatedAt: "2026-05-06T00:00:00Z",
    lastTransitionAt: "2026-05-06T00:00:00Z",
    ...overrides
  };
}

function channelConnectorFixture(overrides: Record<string, unknown> = {}) {
  return {
    connectorId: "slack-main",
    connectorKind: "slack",
    displayName: "Slack Main",
    enablementState: "ready",
    setupState: "ready",
    healthStatus: "healthy",
    diagnosticFreshness: "fresh",
    deliveryEligible: true,
    capabilities: {
      disable: "supported",
      "re-enable": "supported",
      repair: "supported",
      "route-edit": "supported"
    },
    redactionStatus: "redacted",
    updatedAt: "2026-05-10T10:00:00Z",
    nextAction: { actionKind: "repair", label: "Repair" },
    ...overrides
  };
}

function channelConnectorDetailFixture(overrides: Record<string, unknown> = {}) {
  return {
    ...channelConnectorFixture(),
    routePolicy: {
      tenantId: "ten_personal",
      connectorId: "slack-main",
      eligibleRooms: [],
      eligibleChannels: ["C123"],
      eligibleConversations: [],
      eligibleSenders: [],
      backgroundDeliveryEligible: true,
      validationState: "valid",
      validatedAt: "2026-05-10T10:00:00Z",
      redactionStatus: "redacted"
    },
    recentRouteDecisions: [],
    foregroundReplyOutcomes: [],
    backgroundDeliveryOutcomes: [],
    repairActions: [],
    supportEvidenceAvailable: true,
    ...overrides
  };
}

function channelSupportEvidenceFixture(overrides: Record<string, unknown> = {}) {
  return {
    supportEvidenceId: "support_slack_main",
    tenantId: "ten_personal",
    connectorId: "slack-main",
    generatedAt: "2026-05-10T10:00:00Z",
    currentState: "ready",
    retentionExpiresAt: "2026-08-08T10:00:00Z",
    redactionStatus: "redacted",
    diagnosticRefs: [],
    repairRefs: [],
    routingDecisionRefs: [],
    replyOutcomeRefs: [],
    deliveryOutcomeRefs: [],
    auditRefs: [],
    ...overrides
  };
}

function membershipFixture(overrides: Partial<ReturnType<typeof membershipFixtureBase>> = {}) {
  return {
    ...membershipFixtureBase(),
    ...overrides
  };
}

function membershipFixtureBase() {
  return {
    membershipId: "mem_1",
    tenantId: "ten_personal",
    principalId: "prn_1",
    role: "owner",
    status: "active",
    createdAt: "2026-04-24T10:00:00Z",
    updatedAt: "2026-04-24T10:00:00Z"
  };
}

function productFixtureMutationFixture(overrides: Record<string, unknown> = {}) {
  const reviewState = String(overrides.reviewState ?? "draft");
  const suppressionState = String(overrides.suppressionState ?? "none");
  const revisionId = String(overrides.revisionId ?? "revision_product_fixture_candidate_product_1_1");
  return {
    fixture: {
      fixtureId: "product_fixture_candidate_product_1",
      tenantId: "ten_personal",
      displayName: "run:run_source_1",
      domainClass: "schedule",
      sourceKind: "discovered_candidate",
      sourceCandidateId: "candidate_product_1",
      currentRevisionId: revisionId,
      reviewState,
      suppressionState,
      retentionState: "active",
      createdAt: "2026-04-29T10:00:00Z",
      updatedAt: "2026-04-29T10:00:00Z"
    },
    revision: {
      revisionId,
      fixtureId: "product_fixture_candidate_product_1",
      tenantId: "ten_personal",
      revisionNumber: 1,
      fixturePayload: { sourceKind: "run", sourceId: "run_source_1" },
      redactionStatus: "redacted",
      createdAt: "2026-04-29T10:00:00Z"
    }
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

async function loadShell(user: ReturnType<typeof userEvent.setup>) {
  await user.type(screen.getByLabelText(/access token/i), "token");
  await user.click(screen.getByRole("button", { name: /load shell/i }));
}

function onboardingFixtureBase() {
  return {
    environmentScope: "test",
    status: "ready_for_action",
    blockingItemIds: [],
    optionalFollowUpItemIds: ["integration-calendar-a"],
    recommendedActionId: "test_run",
    readinessItems: [
      {
        itemId: "auth-token",
        itemKind: "auth",
        displayName: "Operator access token",
        status: "ready",
        requiredForSelectedAction: true,
        environmentScope: "test",
        updatedAt: "2026-04-24T10:00:00Z"
      },
      {
        itemId: "integration-calendar-a",
        itemKind: "integration",
        displayName: "Calendar A",
        status: "degraded",
        reason: "reauth required",
        requiredForSelectedAction: false,
        detailRoute: "/v1/integrations/calendar-a",
        environmentScope: "test",
        updatedAt: "2026-04-24T10:00:00Z"
      }
    ],
    firstUsefulActions: [
      {
        actionId: "test_run",
        actionKind: "test_run",
        displayName: "Launch test run",
        recommended: true,
        available: true,
        invokeRoute: "/v1/runs",
        resultRoute: "/v1/runs"
      },
      {
        actionId: "test_query",
        actionKind: "test_query",
        displayName: "Run test query",
        recommended: false,
        available: true,
        invokeRoute: "/v1/chat/query",
        resultRoute: "/v1/chat/query"
      }
    ],
    lastEvaluatedAt: "2026-04-24T10:00:00Z"
  };
}

describe("App", () => {
  beforeEach(() => {
    createdClientOptions.length = 0;
    window.localStorage.clear();
    const tenants = [tenantFixture()];
    mockClient.getMe.mockReset().mockResolvedValue(authMeFixture(tenants));
    mockClient.listTenants.mockReset().mockResolvedValue({ items: tenants });
    mockClient.getOnboarding.mockReset();
    mockClient.getActivation.mockReset().mockResolvedValue(activationFixture());
    mockClient.getBillingQuotaDashboard.mockReset().mockResolvedValue(billingQuotaDashboardFixture());
    mockClient.listBillingDenials.mockReset().mockResolvedValue({ items: [] });
    mockClient.getBillingDenialDetail.mockReset().mockResolvedValue({ ...billingDenialFixture(), operationRef: "run:client_1", classification: "quota_exhaustion", recoveryActions: ["wait"] });
    mockClient.exportBillingDenialEvidence.mockReset().mockResolvedValue({ schemaVersion: "2026-05-07", exportId: "evidence_denial_1", tenantId: "ten_personal", generatedAt: "2026-05-07T10:00:01Z", generatedByPrincipalId: "prn_support", denial: {}, usageSnapshot: [], effectiveLimitState: {}, auditRefs: [], redactions: [] });
    mockClient.activate.mockReset().mockResolvedValue(activationFixture({ status: "in_progress" }));
    mockClient.runActivationTestChat.mockReset().mockResolvedValue({
      ...activationFixture({ status: "first_action_completed", currentStepId: "completed" }),
      testChat: { status: "completed", dispatchId: "dispatch_1", provider: "test", model: "test-chat" }
    });
    mockClient.getActivationDiagnostics.mockReset().mockResolvedValue({ items: [] });
    mockClient.listSetupTargets.mockReset().mockResolvedValue({ items: [] });
    mockClient.listSetupSessions.mockReset().mockResolvedValue({ items: [] });
    mockClient.startSetup.mockReset().mockResolvedValue({ session: setupSessionFixture() });
    mockClient.getSetupSession.mockReset().mockResolvedValue({ session: setupSessionFixture() });
    mockClient.submitSetupSecret.mockReset().mockResolvedValue({ session: setupSessionFixture({ state: "ready", reasonCode: "healthy", retryable: false, remediationOwner: "none_required", safeUseMode: "normal", redactedEvidence: { secretRef: "provider/openai-compatible", secretVersionId: "secver_1" } }) });
    mockClient.startSetupOAuth.mockReset().mockResolvedValue({ session: setupSessionFixture({ setupSessionId: "setup_oauth_1", targetId: "integration.feishu_lark", targetKind: "integration", setupStyle: "oauth", oauthStateRef: "oauth_state_ref_1" }), authorizationUrl: "https://oauth.example.test/authorize", state: "oauth_state_ref_1" });
    mockClient.completeSetupOAuth.mockReset().mockResolvedValue({ session: setupSessionFixture({ setupSessionId: "setup_oauth_1", targetId: "integration.feishu_lark", targetKind: "integration", setupStyle: "oauth", state: "ready", retryable: false, remediationOwner: "none_required", safeUseMode: "normal" }) });
    mockClient.retrySetup.mockReset().mockResolvedValue({ session: setupSessionFixture({ state: "in_progress" }) });
    mockClient.replaceSetup.mockReset().mockResolvedValue({ session: setupSessionFixture({ state: "in_progress" }) });
    mockClient.cancelSetup.mockReset().mockResolvedValue({ session: setupSessionFixture({ state: "cancelled", reasonCode: "user_cancelled" }) });
    mockClient.disableSetup.mockReset().mockResolvedValue({ session: setupSessionFixture({ state: "disabled", reasonCode: "disabled_by_user" }) });
    mockClient.getSetupDiagnostics.mockReset().mockResolvedValue({ items: [] });
    mockClient.listApprovals.mockReset();
    mockClient.getActivity.mockReset();
    mockClient.getDiagnostics.mockReset();
    mockClient.resolveApproval.mockReset();
    mockClient.fetchRoute.mockReset();
    mockClient.createRun.mockReset();
    mockClient.queryChat.mockReset();
    mockClient.listReplayCandidates.mockReset().mockResolvedValue({ environmentScope: "test", items: [] });
    mockClient.listEvaluationDiscoveryPolicies.mockReset().mockResolvedValue({ tenantId: "ten_personal", page: { limit: 20 }, items: [] });
    mockClient.listEvaluationDiscoveryRuns.mockReset().mockResolvedValue({ tenantId: "ten_personal", page: { limit: 20 }, items: [] });
    mockClient.listEvaluationDiscoveredCandidates.mockReset().mockResolvedValue({ tenantId: "ten_personal", page: { limit: 20 }, items: [] });
    mockClient.getEvaluationDiscoveredCandidate.mockReset();
    mockClient.createEvaluationSuppression.mockReset().mockResolvedValue({ suppressionId: "suppression_1", tenantId: "ten_personal", targetKind: "discovered_candidate", targetId: "candidate_1", reasonCode: "operator_hidden", createdAt: "2026-04-29T10:00:00Z", active: true });
    mockClient.materializeProductFixture.mockReset().mockResolvedValue(productFixtureMutationFixture());
    mockClient.listProductFixtures.mockReset().mockResolvedValue({ tenantId: "ten_personal", page: { limit: 20 }, items: [] });
    mockClient.createProductFixtureRevision.mockReset().mockResolvedValue(productFixtureMutationFixture({ revisionId: "revision_product_fixture_candidate_product_1_2" }));
    mockClient.reviewProductFixture.mockReset().mockResolvedValue(productFixtureMutationFixture({ reviewState: "approved" }));
    mockClient.suppressProductFixture.mockReset().mockResolvedValue(productFixtureMutationFixture({ suppressionState: "suppressed" }));
    mockClient.createEvaluationCampaign.mockReset().mockResolvedValue({
      campaignId: "campaign_1",
      tenantId: "ten_personal",
      displayName: "Campaign",
      status: "queued",
      createdAt: "2026-04-29T10:00:00Z",
      retentionState: "active"
    });
    mockClient.listEvaluationCampaigns.mockReset().mockResolvedValue({ tenantId: "ten_personal", page: { limit: 20 }, items: [] });
    mockClient.getEvaluationCampaign.mockReset();
    mockClient.startEvaluationCampaign.mockReset().mockResolvedValue({
      campaignId: "campaign_1",
      tenantId: "ten_personal",
      displayName: "Campaign",
      status: "running",
      createdAt: "2026-04-29T10:00:00Z",
      retentionState: "active"
    });
    mockClient.cancelEvaluationCampaign.mockReset();
    mockClient.publishEvaluationCampaignResults.mockReset().mockResolvedValue({
      campaignId: "campaign_1",
      tenantId: "ten_personal",
      displayName: "Campaign",
      status: "published",
      createdAt: "2026-04-29T10:00:00Z",
      retentionState: "active"
    });
    mockClient.listEvaluationCampaignItems.mockReset().mockResolvedValue({ tenantId: "ten_personal", page: { limit: 20 }, items: [] });
    mockClient.listEvaluationCampaignAttemptGroups.mockReset().mockResolvedValue({ tenantId: "ten_personal", page: { limit: 20 }, items: [] });
    mockClient.listEvaluationDashboard.mockReset().mockResolvedValue({ tenantId: "ten_personal", page: { limit: 20 }, items: [] });
    mockClient.listEvaluationToolCallInspections.mockReset().mockResolvedValue({ tenantId: "ten_personal", page: { limit: 20 }, items: [] });
    mockClient.getEvaluationToolCallInspection.mockReset();
    mockClient.getReplayCandidate.mockReset();
    mockClient.createReplayAttempt.mockReset();
    mockClient.listReplayAttempts.mockReset().mockResolvedValue({ environmentScope: "test", items: [] });
    mockClient.getReplayAttempt.mockReset();
    mockClient.createReplayComparison.mockReset();
    mockClient.listReplayComparisons.mockReset().mockResolvedValue({ environmentScope: "test", items: [] });
    mockClient.getReplayComparison.mockReset();
    mockClient.listReplayFixtures.mockReset().mockResolvedValue({ environmentScope: "test", items: [] });
    mockClient.startLiveValidation.mockReset();
    mockClient.listLiveValidations.mockReset().mockResolvedValue({ environmentScope: "test", items: [] });
    mockClient.listLiveValidationSupportMatrix.mockReset().mockResolvedValue({ environmentScope: "test", version: "v1", items: [] });
    mockClient.listLiveValidationLedger.mockReset().mockResolvedValue({ validationId: "lv_1", items: [] });
    mockClient.getLiveValidationRetention.mockReset().mockResolvedValue({ policyId: "ret_1", appliesTo: "all", mode: "indefinite", createdByPrincipalId: "prn_1", createdAt: "2026-04-29T10:00:00Z" });
    mockClient.listLiveValidationKillSwitches.mockReset().mockResolvedValue({ items: [] });
    mockClient.updateLiveValidationKillSwitch.mockReset();
    mockClient.getLiveValidation.mockReset();
    mockClient.abortLiveValidation.mockReset();
    mockClient.listMemberships.mockReset().mockResolvedValue({ items: [membershipFixture()] });
    mockClient.updateMembershipRole.mockReset();
    mockClient.listChannelConnectors.mockReset().mockResolvedValue({
      tenantId: "ten_personal",
      page: { limit: 20, order: "attention_disabled_ready_name_id" },
      items: []
    });
    mockClient.listThreads.mockReset().mockResolvedValue({
      tenantId: "ten_personal",
      page: { limit: 20, order: "active_recent_archived_id" },
      items: []
    });
    mockClient.getChannelConnector.mockReset().mockResolvedValue(channelConnectorDetailFixture());
    mockClient.getChannelConnectorSupportEvidence.mockReset().mockResolvedValue(channelSupportEvidenceFixture());
    mockClient.disableChannelConnector.mockReset().mockResolvedValue({
      connectorId: "slack-main",
      enablementState: "disabled",
      deliveryEligible: false,
      auditEventId: "audit_disable_1",
      changedAt: "2026-05-10T10:01:00Z"
    });
    mockClient.reEnableChannelConnector.mockReset().mockResolvedValue({
      connectorId: "slack-main",
      enablementState: "ready",
      deliveryEligible: true,
      auditEventId: "audit_enable_1",
      changedAt: "2026-05-10T10:02:00Z"
    });
    mockClient.startChannelConnectorRepair.mockReset().mockResolvedValue({
      repairActionId: "repair_slack_main",
      tenantId: "ten_personal",
      connectorId: "slack-main",
      connectorKind: "slack",
      actionKind: "repair",
      status: "ready",
      startedAt: "2026-05-10T10:03:00Z",
      auditEventId: "audit_repair_1",
      redactionStatus: "redacted"
    });
    mockClient.updateChannelRoutePolicy.mockReset().mockResolvedValue({
      tenantId: "ten_personal",
      connectorId: "slack-main",
      eligibleRooms: [],
      eligibleChannels: ["C123"],
      eligibleConversations: [],
      eligibleSenders: [],
      backgroundDeliveryEligible: true,
      validationState: "valid",
      validatedAt: "2026-05-10T10:04:00Z",
      redactionStatus: "redacted"
    });
    emptySubscription.close.mockClear();
    mockClient.streamEvents.mockReset().mockReturnValue(emptySubscription);
  });

  afterEach(() => {
    cleanup();
  });

  it("loads onboarding state and launches the recommended test run", async () => {
    mockClient.getOnboarding
      .mockResolvedValueOnce(onboardingFixture())
      .mockResolvedValueOnce(onboardingFixture({
        status: "completed"
      }));
    mockClient.listApprovals.mockResolvedValue({ items: [] });
    mockClient.getActivity.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-04-24T10:00:00Z" });
    mockClient.getDiagnostics.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-04-24T10:00:00Z" });
    mockClient.createRun.mockResolvedValue({
      runId: "run_1",
      entrypoint: "operator.shell.test",
      status: "queued",
      goal: "Run an operator shell smoke check.",
      createdAt: "2026-04-24T10:00:00Z",
      updatedAt: "2026-04-24T10:00:00Z"
    });

    render(<App />);
    const user = userEvent.setup();

    await user.type(screen.getByLabelText(/access token/i), "token");
    await user.click(screen.getByRole("button", { name: /load shell/i }));

    await waitFor(() => {
      expect(screen.getByText("Single control surface for onboarding, approvals, activity, and diagnostics.")).not.toBeNull();
      expect(screen.getAllByText("Launch test run").length).toBeGreaterThan(0);
    });

    await user.click(screen.getAllByRole("button", { name: "Launch test run" })[0]);

    await waitFor(() => {
      expect(mockClient.createRun).toHaveBeenCalledWith({
        entrypoint: "operator.shell.test",
        goal: "Run an operator shell smoke check."
      }, { tenantId: "ten_personal" });
      expect(screen.getByText(/Created test run run_1/i)).not.toBeNull();
      expect(screen.getAllByText("completed").length).toBeGreaterThan(0);
    });
  });

  it("renders quota dashboard, denial detail, and support evidence export states", async () => {
    const billingTenant = tenantFixture({
      callerPermissions: ["tenant.manage", "runs.execute", "billing.view", "billing.evidence_export"]
    });
    mockClient.getMe.mockResolvedValue(authMeFixture([billingTenant]));
    mockClient.listTenants.mockResolvedValue({ items: [billingTenant] });
    mockClient.getOnboarding.mockResolvedValue(onboardingFixture());
    mockClient.listApprovals.mockResolvedValue({ items: [] });
    mockClient.getActivity.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-05-07T10:00:00Z" });
    mockClient.getDiagnostics.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-05-07T10:00:00Z" });
    mockClient.getBillingQuotaDashboard.mockResolvedValue(billingQuotaDashboardFixture());
    mockClient.listBillingDenials.mockResolvedValue({ items: [billingDenialFixture()] });
    mockClient.getBillingDenialDetail.mockResolvedValue({
      ...billingDenialFixture(),
      operationRef: "run:client_1",
      classification: "quota_exhaustion",
      recoveryActions: ["wait", "reduce_scope"]
    });
    mockClient.exportBillingDenialEvidence.mockResolvedValue({
      schemaVersion: "2026-05-07",
      exportId: "evidence_denial_1",
      tenantId: "ten_personal",
      generatedAt: "2026-05-07T10:01:00Z",
      generatedByPrincipalId: "prn_support",
      denial: {},
      usageSnapshot: [],
      effectiveLimitState: {},
      auditRefs: ["audit_1"],
      redactions: [{ path: "$.secret", reason: "secret", replacement: "[REDACTED]" }]
    });

    render(<App />);
    const user = userEvent.setup();
    await loadShell(user);

    expect(await screen.findByRole("heading", { name: "Quota Dashboard" })).not.toBeNull();
    expect(screen.getAllByText("Run Launches").length).toBeGreaterThan(0);
    expect(screen.getByText("Previous: 5 used, 5 remaining")).not.toBeNull();
    expect(screen.getByText("Override: 10 to 8, support override")).not.toBeNull();
    expect(screen.getByText("Restriction: abuse_restriction:temporary")).not.toBeNull();
    expect(screen.getByText("Actions: wait, reduce_scope")).not.toBeNull();

    await user.click(screen.getByRole("button", { name: "Detail" }));
    await waitFor(() => {
      expect(mockClient.getBillingDenialDetail).toHaveBeenCalledWith("denial_1", { tenantId: "ten_personal" });
    });

    await user.click(screen.getByRole("button", { name: "Export" }));
    await waitFor(() => {
      expect(mockClient.exportBillingDenialEvidence).toHaveBeenCalledWith("denial_1", { tenantId: "ten_personal" });
    });
    expect(await screen.findByText("Exported redacted billing evidence evidence_denial_1.")).not.toBeNull();
  });

  it("loads channel detail and wires management actions", async () => {
    const channelTenant = tenantFixture({
      callerPermissions: ["tenant.manage", "runs.execute", "credentials.inspect", "connectors.manage"]
    });
    mockClient.getMe.mockResolvedValue(authMeFixture([channelTenant]));
    mockClient.listTenants.mockResolvedValue({ items: [channelTenant] });
    mockClient.getOnboarding.mockResolvedValue(onboardingFixture());
    mockClient.listApprovals.mockResolvedValue({ items: [] });
    mockClient.getActivity.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-05-10T10:00:00Z" });
    mockClient.getDiagnostics.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-05-10T10:00:00Z" });
    mockClient.listChannelConnectors.mockResolvedValue({
      tenantId: "ten_personal",
      page: { limit: 20, order: "attention_disabled_ready_name_id" },
      items: [channelConnectorFixture()]
    });
    mockClient.listThreads.mockResolvedValue({
      tenantId: "ten_personal",
      page: { limit: 20, order: "active_recent_archived_id" },
      items: [{
        threadId: "thr_slack",
        tenantId: "ten_personal",
        lifecycleState: "active",
        sourceKind: "channel",
        sourceSummary: "Slack Main / #support",
        lastActivityAt: "2026-05-11T10:00:00Z",
        availableActions: ["reset", "archive"],
        redactionStatus: "redacted",
        updatedAt: "2026-05-11T10:00:00Z"
      }]
    });
    mockClient.getChannelConnector.mockResolvedValue(channelConnectorDetailFixture());
    mockClient.getChannelConnectorSupportEvidence.mockResolvedValue(channelSupportEvidenceFixture());

    render(<App />);
    const user = userEvent.setup();
    await loadShell(user);

    expect((await screen.findAllByRole("heading", { name: "Slack Main" })).length).toBeGreaterThan(0);
    expect(await screen.findByText("thr_slack")).not.toBeNull();
    expect(screen.getByText("Slack Main / #support")).not.toBeNull();
    expect(mockClient.listThreads).toHaveBeenCalledWith({ limit: 20 }, { tenantId: "ten_personal" });
    expect(mockClient.getChannelConnector).toHaveBeenCalledWith("slack-main", { tenantId: "ten_personal" });
    expect(mockClient.getChannelConnectorSupportEvidence).toHaveBeenCalledWith("slack-main", { tenantId: "ten_personal" });

    await user.click(screen.getByRole("button", { name: "Disable" }));
    await waitFor(() => {
      expect(mockClient.disableChannelConnector).toHaveBeenCalledWith("slack-main", {}, { tenantId: "ten_personal" });
      expect(screen.getByText("Channel connector slack-main disable completed.")).not.toBeNull();
    });

    await user.click(screen.getByRole("button", { name: "Repair" }));
    await waitFor(() => {
      expect(mockClient.startChannelConnectorRepair).toHaveBeenCalledWith("slack-main", { actionKind: "repair" }, { tenantId: "ten_personal" });
    });

    await user.clear(screen.getByLabelText("Eligible channels"));
    await user.type(screen.getByLabelText("Eligible channels"), "C999");
    await user.click(screen.getByRole("button", { name: "Save route policy" }));
    await waitFor(() => {
      expect(mockClient.updateChannelRoutePolicy).toHaveBeenCalledWith("slack-main", {
        eligibleRooms: [],
        eligibleChannels: ["C999"],
        eligibleConversations: [],
        eligibleSenders: [],
        backgroundDeliveryEligible: true
      }, { tenantId: "ten_personal" });
    });
  });

  it("renders setup proof targets, submits secrets without rendering raw values, and drives OAuth fixtures", async () => {
    mockClient.getOnboarding.mockResolvedValue(onboardingFixture());
    mockClient.listApprovals.mockResolvedValue({ items: [] });
    mockClient.getActivity.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-05-06T00:00:00Z" });
    mockClient.getDiagnostics.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-05-06T00:00:00Z" });
    mockClient.listSetupTargets.mockResolvedValue({
      items: [
        setupTargetFixture(),
        setupTargetFixture({
          targetId: "integration.feishu_lark",
          targetKind: "integration",
          setupStyle: "oauth",
          displayName: "Feishu/Lark OAuth"
        })
      ]
    });
    mockClient.listSetupSessions.mockResolvedValue({ items: [] });

    render(<App />);
    const user = userEvent.setup();
    await loadShell(user);

    await waitFor(() => {
      expect(screen.getByText("OpenAI-compatible provider")).not.toBeNull();
      expect(screen.getByText("Feishu/Lark OAuth")).not.toBeNull();
    });

    await user.type(screen.getByLabelText(/secret value/i), "R46_FAKE_OPENAI_COMPATIBLE_KEY_DO_NOT_LEAK");
    await user.click(screen.getByRole("button", { name: /submit secret/i }));

    await waitFor(() => {
      expect(mockClient.submitSetupSecret).toHaveBeenCalledWith("setup_1", {
        secretRef: "provider/openai-compatible",
        displayName: "OpenAI-compatible provider",
        value: "R46_FAKE_OPENAI_COMPATIBLE_KEY_DO_NOT_LEAK"
      }, { tenantId: "ten_personal" });
      expect(document.body.textContent).not.toContain("R46_FAKE_OPENAI_COMPATIBLE_KEY_DO_NOT_LEAK");
    });

    await user.click(screen.getByRole("button", { name: /start oauth/i }));

    await waitFor(() => {
      expect(mockClient.startSetupOAuth).toHaveBeenCalledWith("setup_1", { redirectRoute: "/setup/oauth/feishu-lark/callback" }, { tenantId: "ten_personal" });
      expect(screen.getByText(/OAuth fixture started/i)).not.toBeNull();
    });
  });

  it("starts Slack OAuth setup with the Slack callback route", async () => {
    mockClient.getOnboarding.mockResolvedValue(onboardingFixture());
    mockClient.listApprovals.mockResolvedValue({ items: [] });
    mockClient.getActivity.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-05-08T00:00:00Z" });
    mockClient.getDiagnostics.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-05-08T00:00:00Z" });
    mockClient.listSetupTargets.mockResolvedValue({
      items: [
        setupTargetFixture({
          targetId: "connector.slack",
          targetKind: "connector",
          setupStyle: "oauth",
          displayName: "Slack OAuth"
        })
      ]
    });
    mockClient.listSetupSessions.mockResolvedValue({ items: [] });
    mockClient.startSetupOAuth.mockResolvedValue({
      session: setupSessionFixture({
        setupSessionId: "setup_slack_oauth_1",
        targetId: "connector.slack",
        targetKind: "connector",
        setupStyle: "oauth",
        oauthStateRef: "oauth_state_ref_slack"
      }),
      authorizationUrl: "https://slack.example.test/oauth",
      state: "oauth_state_ref_slack"
    });

    render(<App />);
    const user = userEvent.setup();
    await loadShell(user);

    expect(await screen.findByText("Slack OAuth")).not.toBeNull();
    await user.click(screen.getByRole("button", { name: /start oauth/i }));

    await waitFor(() => {
      expect(mockClient.startSetup).toHaveBeenCalledWith({
        targetId: "connector.slack",
        setupStyle: "oauth",
        source: "operator_shell"
      }, { tenantId: "ten_personal" });
      expect(mockClient.startSetupOAuth).toHaveBeenCalledWith("setup_1", { redirectRoute: "/setup/oauth/slack/callback" }, { tenantId: "ten_personal" });
    });
  });

  it("loads activation state with the tenant batch and shows stale activation placeholders while refreshing", async () => {
    const refreshActivation = deferred<ReturnType<typeof activationFixture>>();
    mockClient.getActivation
      .mockResolvedValueOnce(activationFixture())
      .mockReturnValueOnce(refreshActivation.promise);
    mockClient.getOnboarding.mockResolvedValue(onboardingFixture());
    mockClient.listApprovals.mockResolvedValue({ items: [] });
    mockClient.getActivity.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-05-06T00:00:00Z" });
    mockClient.getDiagnostics.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-05-06T00:00:00Z" });

    render(<App />);
    const user = userEvent.setup();
    await loadShell(user);

    await waitFor(() => {
      expect(mockClient.getActivation).toHaveBeenCalledWith({ tenantId: "ten_personal" });
      expect(mockClient.getActivationDiagnostics).toHaveBeenCalledWith({ tenantId: "ten_personal" });
      expect(screen.getAllByText("Activation").length).toBeGreaterThan(0);
      expect(screen.getAllByText("active").length).toBeGreaterThan(0);
      expect(screen.getByText(/Quota baseline free/i)).not.toBeNull();
    });

    act(() => {
      mockClient.streamEvents.mock.calls[0][1].onEvent({ name: "tenant.activation_started", sequence: 2 });
    });

    await waitFor(() => {
      expect(screen.getByText(/Activation state is refreshing/i)).not.toBeNull();
    });
  });

  it("renders activation readiness, quota baseline, environment, and the test chat action", async () => {
    mockClient.getActivation.mockResolvedValue(activationFixture({
      quotaBaseline: {
        tenantId: "ten_personal",
        planKey: "hosted-free",
        enforcementMode: "enforced",
        status: "available",
        quotas: [{
          category: "run_launches",
          unit: "count",
          limit: 10,
          used: 2,
          remaining: 8,
          period: "2026-05-01T00:00:00Z/2026-06-01T00:00:00Z"
        }]
      },
      readinessItems: [
        {
          itemId: "environment",
          itemKind: "environment",
          status: "ready",
          displayName: "Hosted environment",
          requiredForActivation: true,
          retryable: false,
          remediationOwner: "none_required",
          updatedAt: "2026-05-06T00:00:00Z"
        },
        {
          itemId: "quota-baseline",
          itemKind: "quota_baseline",
          status: "ready",
          displayName: "Quota baseline",
          requiredForActivation: true,
          retryable: false,
          remediationOwner: "none_required",
          updatedAt: "2026-05-06T00:00:00Z"
        }
      ]
    }));
    mockClient.getOnboarding.mockResolvedValue(onboardingFixture());
    mockClient.listApprovals.mockResolvedValue({ items: [] });
    mockClient.getActivity.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-05-06T00:00:00Z" });
    mockClient.getDiagnostics.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-05-06T00:00:00Z" });

    render(<App />);
    const user = userEvent.setup();
    await loadShell(user);

    await waitFor(() => {
      expect(screen.getByText(/Personal Tenant \(personal\)/)).not.toBeNull();
      expect(screen.getByText(/Quota baseline hosted-free/i)).not.toBeNull();
      expect(screen.getByText(/run_launches/i)).not.toBeNull();
      expect(screen.getByText(/8 remaining/i)).not.toBeNull();
      expect(screen.getByText("Hosted environment")).not.toBeNull();
      expect((screen.getByRole("button", { name: /test chat/i }) as HTMLButtonElement).disabled).toBe(false);
    });
  });

  it("bootstraps activation when signup has no tenant yet", async () => {
    const tenants = [tenantFixture()];
    mockClient.getMe
      .mockResolvedValueOnce(authMeWithoutTenantFixture())
      .mockResolvedValueOnce(authMeFixture(tenants));
    mockClient.listTenants
      .mockResolvedValueOnce({ items: [] })
      .mockResolvedValueOnce({ items: tenants });
    mockClient.activate.mockResolvedValue(activationFixture());
    mockClient.getOnboarding.mockResolvedValue(onboardingFixture());
    mockClient.listApprovals.mockResolvedValue({ items: [] });
    mockClient.getActivity.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-05-06T00:00:00Z" });
    mockClient.getDiagnostics.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-05-06T00:00:00Z" });

    render(<App />);
    const user = userEvent.setup();
    await loadShell(user);

    await waitFor(() => {
      expect(mockClient.activate).toHaveBeenCalledWith({ source: "signup" });
      expect(screen.getByText(/Personal Tenant \(personal\)/)).not.toBeNull();
      expect(screen.getByText(/Quota baseline free/i)).not.toBeNull();
    });
  });

  it("renders quota-blocked activation with disabled first action and diagnostics", async () => {
    mockClient.getActivation.mockResolvedValue(activationFixture({
      status: "blocked",
      currentStepId: "quota_baseline",
      completedStepIds: ["tenant_resolved"],
      blockingReasonCodes: ["activation_blocked:quota_baseline_unavailable"],
      readinessItems: [{
        itemId: "quota-baseline",
        itemKind: "quota_baseline",
        status: "blocked",
        reasonCode: "activation_blocked:quota_baseline_unavailable",
        displayName: "Quota baseline",
        requiredForActivation: true,
        retryable: true,
        remediationOwner: "operator",
        updatedAt: "2026-05-06T00:00:00Z"
      }],
      quotaBaseline: {
        tenantId: "ten_personal",
        planKey: "unknown",
        enforcementMode: "not_measurable",
        status: "unavailable",
        reasonCode: "activation_blocked:quota_baseline_unavailable",
        quotas: []
      },
      firstAction: {
        actionId: "test_chat",
        actionKind: "test_chat",
        displayName: "Test chat",
        recommended: true,
        available: false,
        blockingItemIds: ["quota-baseline"],
        invokeRoute: "/v1/activation/test-chat",
        resultRoute: "/v1/activation"
      }
    }));
    mockClient.getActivationDiagnostics.mockResolvedValue({
      items: [{
        activationId: "act_1",
        tenantId: "ten_personal",
        principalId: "prn_1",
        status: "blocked",
        stage: "quota_baseline",
        reasonCode: "activation_blocked:quota_baseline_unavailable",
        retryable: true,
        remediationOwner: "operator",
        lastTransitionAt: "2026-05-06T00:00:00Z",
        readinessItemIds: ["quota-baseline"],
        quotaBaselineStatus: "unavailable"
      }]
    });
    mockClient.getOnboarding.mockResolvedValue(onboardingFixture());
    mockClient.listApprovals.mockResolvedValue({ items: [] });
    mockClient.getActivity.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-05-06T00:00:00Z" });
    mockClient.getDiagnostics.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-05-06T00:00:00Z" });

    render(<App />);
    const user = userEvent.setup();
    await loadShell(user);

    await waitFor(() => {
      expect(screen.getByText(/Blocked by activation_blocked:quota_baseline_unavailable/i)).not.toBeNull();
      expect(screen.getAllByText("activation_blocked:quota_baseline_unavailable").length).toBeGreaterThan(0);
      expect(screen.getAllByText("quota_baseline").length).toBeGreaterThan(0);
      expect(screen.getByText(/Retryable - operator/i)).not.toBeNull();
      expect((screen.getByRole("button", { name: /test chat/i }) as HTMLButtonElement).disabled).toBe(true);
      expect(screen.getAllByText(/quota-baseline/i).length).toBeGreaterThan(0);
    });
  });

  it("runs activation test chat and refreshes completion state without rendering message content", async () => {
    mockClient.getActivation
      .mockResolvedValueOnce(activationFixture())
      .mockResolvedValueOnce(activationFixture({ status: "first_action_completed", currentStepId: "completed" }));
    mockClient.runActivationTestChat.mockResolvedValue({
      ...activationFixture({ status: "first_action_completed", currentStepId: "completed" }),
      testChat: {
        status: "completed",
        dispatchId: "dispatch_activation",
        provider: "test",
        model: "test-chat",
        usage: { totalTokens: 2 }
      }
    });
    mockClient.getOnboarding.mockResolvedValue(onboardingFixture());
    mockClient.listApprovals.mockResolvedValue({ items: [] });
    mockClient.getActivity.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-05-06T00:00:00Z" });
    mockClient.getDiagnostics.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-05-06T00:00:00Z" });

    render(<App />);
    const user = userEvent.setup();
    await loadShell(user);

    await user.click(await screen.findByRole("button", { name: /test chat/i }));

    await waitFor(() => {
      expect(mockClient.runActivationTestChat).toHaveBeenCalledWith({ message: "Run a safe hosted activation test." }, { tenantId: "ten_personal" });
      expect(screen.getByText(/Activation test chat completed/i)).not.toBeNull();
      expect(screen.getAllByText("first_action_completed").length).toBeGreaterThan(0);
      expect(screen.queryByText("Run a safe hosted activation test.")).toBeNull();
    });
  });

  it("resolves approvals and inspects linked activity detail in the shell", async () => {
    mockClient.getOnboarding.mockResolvedValue(onboardingFixture());
    mockClient.listApprovals.mockResolvedValue({
      items: [
        {
          approvalId: "approval_1",
          action: "workflow.launch",
          reason: "needs review",
          status: "pending",
          createdAt: "2026-04-24T10:00:00Z",
          updatedAt: "2026-04-24T10:00:00Z"
        }
      ]
    });
    mockClient.getActivity.mockResolvedValue({
      environmentScope: "test",
      generatedAt: "2026-04-24T10:00:00Z",
      items: [
        {
          activityId: "workflow-wf_1",
          sourceKind: "workflow",
          sourceId: "wf_1",
          title: "Workflow wf_1",
          status: "failed",
          summary: "workflow failed",
          attentionLevel: "critical",
          occurredAt: "2026-04-24T10:00:00Z",
          detailRoute: "/v1/runs/run_1/workflows/wf_1",
          relatedResourceRefs: [{ kind: "run", id: "run_1", route: "/v1/runs/run_1" }],
          environmentScope: "test"
        }
      ]
    });
    mockClient.getDiagnostics.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-04-24T10:00:00Z" });
    mockClient.resolveApproval.mockResolvedValue({
      approval: {
        approvalId: "approval_1",
        action: "workflow.launch",
        reason: "needs review",
        status: "approved",
        createdAt: "2026-04-24T10:00:00Z",
        updatedAt: "2026-04-24T10:01:00Z"
      },
      decision: {
        decisionId: "decision_1",
        action: "workflow.launch",
        outcome: "approved",
        reason: "needs review",
        approvalId: "approval_1",
        createdAt: "2026-04-24T10:01:00Z"
      }
    });
    mockClient.fetchRoute.mockResolvedValue({ workflowId: "wf_1", status: "failed" });

    render(<App />);
    const user = userEvent.setup();

    await user.type(screen.getByLabelText(/access token/i), "token");
    await user.click(screen.getByRole("button", { name: /load shell/i }));

    await waitFor(() => {
      expect(screen.getByText("Approval Inbox")).not.toBeNull();
      expect(screen.getByText("Workflow wf_1")).not.toBeNull();
    });

    await user.click(screen.getByRole("button", { name: "Approve" }));

    await waitFor(() => {
      expect(mockClient.resolveApproval).toHaveBeenCalledWith("approval_1", {
        resolution: "approved",
        comment: "Resolved in operator shell."
      }, { tenantId: "ten_personal" });
      expect(screen.getByText(/workflow.launch approved/i)).not.toBeNull();
    });

    await user.click(screen.getAllByRole("button", { name: "Inspect" }).find((button) => button.closest(".activity-panel")) ?? screen.getAllByRole("button", { name: "Inspect" })[0]);

    await waitFor(() => {
      expect(mockClient.fetchRoute).toHaveBeenCalledWith("/v1/runs/run_1/workflows/wf_1", { tenantId: "ten_personal" });
      expect(screen.getByText(/Authoritative Detail/i)).not.toBeNull();
      expect(screen.getByText(/"workflowId": "wf_1"/i)).not.toBeNull();
    });
  });

  it("applies diagnostics filters and runs a bounded test query", async () => {
    mockClient.getOnboarding.mockResolvedValue(onboardingFixture());
    mockClient.listApprovals.mockResolvedValue({ items: [] });
    mockClient.getActivity.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-04-24T10:00:00Z" });
    mockClient.getDiagnostics
      .mockResolvedValueOnce({
        environmentScope: "test",
        generatedAt: "2026-04-24T10:00:00Z",
        items: [
          {
            findingId: "approval-1",
            sourceKind: "approval",
            sourceId: "approval_1",
            plane: "approval",
            severity: "warning",
            status: "pending",
            reason: "waiting approval",
            environmentScope: "test",
            capturedAt: "2026-04-24T10:00:00Z"
          }
        ]
      })
      .mockResolvedValueOnce({
        environmentScope: "test",
        generatedAt: "2026-04-24T10:01:00Z",
        items: [
          {
            findingId: "delivery-1",
            sourceKind: "delivery",
            sourceId: "delivery_1",
            plane: "delivery",
            severity: "critical",
            status: "failed",
            reason: "delivery failed",
            detailRoute: "/v1/deliveries/delivery_1",
            environmentScope: "test",
            capturedAt: "2026-04-24T10:01:00Z"
          }
        ]
      })
      .mockResolvedValue({
        environmentScope: "test",
        generatedAt: "2026-04-24T10:01:00Z",
        items: []
      });
    mockClient.queryChat.mockResolvedValue({
      dispatchId: "dispatch_1",
      provider: "echo",
      model: "echo-v1",
      skills: [],
      query: "Return one bounded readiness confirmation.",
      status: "completed",
      partial: false,
      reply: "ready",
      usage: { inputTokens: 1, outputTokens: 1, totalTokens: 2 }
    });

    render(<App />);
    const user = userEvent.setup();

    await user.type(screen.getByLabelText(/access token/i), "token");
    await user.click(screen.getByRole("button", { name: /load shell/i }));

    await waitFor(() => {
      expect(screen.getByText("Diagnostics")).not.toBeNull();
    });

    await user.selectOptions(screen.getByLabelText(/plane filter/i), "delivery");
    await user.selectOptions(screen.getByLabelText(/severity filter/i), "critical");
    await user.click(screen.getByRole("button", { name: /apply filters/i }));
    await user.type(screen.getByLabelText(/thread id/i), "thr_web");

    await waitFor(() => {
      expect(mockClient.getDiagnostics).toHaveBeenLastCalledWith({
        plane: "delivery",
        severity: "critical"
      }, { tenantId: "ten_personal" });
    });

    await user.click(screen.getAllByRole("button", { name: "Run test query" })[0]);

    await waitFor(() => {
      expect(mockClient.queryChat).toHaveBeenCalledWith({
        query: "Return one bounded readiness confirmation.",
        threadId: "thr_web"
      }, { tenantId: "ten_personal" });
      expect(screen.getByText(/Test query completed with 2 total tokens/i)).not.toBeNull();
    });
  });

  it("loads evaluation candidates, launches replay, compares, and exposes fixtures without editing controls", async () => {
    mockClient.getOnboarding.mockResolvedValue(onboardingFixture());
    mockClient.listApprovals.mockResolvedValue({ items: [] });
    mockClient.getActivity.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-04-24T10:00:00Z" });
    mockClient.getDiagnostics.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-04-24T10:00:00Z" });
    mockClient.listReplayCandidates.mockResolvedValue({
      environmentScope: "test",
      items: [
        {
          candidateId: "candidate_1",
          candidateKind: "fixture",
          displayName: "Schedule Fixture",
          sourceKind: "fixture",
          sourceId: "fixture_schedule",
          sourceRefs: [{ kind: "schedule", id: "sched_1", route: "/v1/schedules/sched_1" }],
          environmentScope: "test",
          readinessStatus: "fully_replayable",
          readinessReasons: ["fixture has evidence"],
          limitations: [],
          defaultReplayMode: "non_live",
          latestAttemptId: "attempt_1",
          createdAt: "2026-04-24T10:00:00Z",
          updatedAt: "2026-04-24T10:00:00Z"
        }
      ]
    });
    mockClient.listReplayAttempts.mockResolvedValue({ environmentScope: "test", items: [] });
    mockClient.listReplayComparisons.mockResolvedValue({
      environmentScope: "test",
      items: [
        {
          comparisonId: "comparison_existing",
          candidateId: "candidate_1",
          baselineRef: "fixture_schedule",
          attemptId: "attempt_existing",
          environmentScope: "test",
          terminalStatus: "drifted",
          runtimeSummary: "runtime changed",
          policySummary: "policy matched",
          integrationSummary: "integration matched",
          deliverySummary: "delivery matched",
          evidenceSummary: "evidence matched",
          confidence: "medium",
          limitations: ["captured evidence only"],
          driftFindings: [
            {
              findingId: "finding_1",
              comparisonId: "comparison_existing",
              plane: "runtime",
              severity: "warning",
              summary: "runtime summary changed",
              baselineValue: "old",
              replayValue: "new",
              createdAt: "2026-04-24T10:00:00Z"
            }
          ],
          generatedAt: "2026-04-24T10:00:00Z"
        }
      ]
    });
    mockClient.listReplayFixtures.mockResolvedValue({
      environmentScope: "test",
      items: [
        {
          fixtureId: "fixture_schedule",
          displayName: "Schedule Fixture",
          domainClass: "schedule",
          manifestPath: "daemon/internal/evaluation/testdata/fixtures/schedule-basic/manifest.json",
          sourceRefs: [],
          capturedEvidenceRefs: [],
          assumptions: ["captured evidence"],
          limitations: [],
          expectedReplayMode: "non_live",
          expectedComparisonSummary: { runtime: "runtime", evidence: "evidence" },
          candidateId: "candidate_1",
          environmentScope: "test",
          createdAt: "2026-04-24T10:00:00Z",
          updatedAt: "2026-04-24T10:00:00Z"
        }
      ]
    });
    mockClient.createReplayAttempt.mockResolvedValue({
      attemptId: "attempt_1",
      candidateId: "candidate_1",
      sourceRefs: [],
      environmentScope: "test",
      mode: "non_live",
      status: "completed",
      safetyScope: { mode: "non_live" },
      approvalHandling: "evidence_only",
      sideEffectHandling: "evidence_only",
      evidenceRefs: [],
      blockedReasons: [],
      createdAt: "2026-04-24T10:00:00Z",
      updatedAt: "2026-04-24T10:00:00Z"
    });
    mockClient.createReplayComparison.mockResolvedValue({
      comparisonId: "comparison_1",
      candidateId: "candidate_1",
      baselineRef: "fixture_schedule",
      attemptId: "attempt_1",
      environmentScope: "test",
      terminalStatus: "matched",
      runtimeSummary: "runtime matched",
      policySummary: "policy matched",
      integrationSummary: "integration matched",
      deliverySummary: "delivery matched",
      evidenceSummary: "evidence matched",
      confidence: "high",
      limitations: [],
      driftFindings: [],
      generatedAt: "2026-04-24T10:00:00Z"
    });

    render(<App />);
    const user = userEvent.setup();

    await user.type(screen.getByLabelText(/access token/i), "token");
    await user.click(screen.getByRole("button", { name: /load shell/i }));

    await waitFor(() => {
      expect(screen.getByText("Evaluation Replay")).not.toBeNull();
      expect(screen.getAllByText("Schedule Fixture").length).toBeGreaterThan(0);
      expect(screen.getByText(/Fixtures are engineer-managed and repo-backed/i)).not.toBeNull();
      expect(screen.getByText("Replay History")).not.toBeNull();
      expect(screen.getByText(/runtime changed/i)).not.toBeNull();
      expect(screen.getByText(/runtime summary changed/i)).not.toBeNull();
    });

    expect(screen.queryByRole("button", { name: /edit fixture/i })).toBeNull();

    await user.click(screen.getByRole("button", { name: "Launch Replay" }));

    await waitFor(() => {
      expect(mockClient.createReplayAttempt).toHaveBeenCalledWith("candidate_1", { mode: "non_live" }, { tenantId: "ten_personal" });
      expect(screen.getByText(/Replay attempt attempt_1 completed/i)).not.toBeNull();
    });

    await user.click(screen.getByRole("button", { name: "Compare Latest" }));

    await waitFor(() => {
      expect(mockClient.createReplayComparison).toHaveBeenCalledWith("attempt_1", {}, { tenantId: "ten_personal" });
      expect(screen.getByText(/Comparison comparison_1 matched/i)).not.toBeNull();
    });
  });

  it("loads candidate discovery review state and suppresses discovered candidates", async () => {
    mockClient.getOnboarding.mockResolvedValue(onboardingFixture());
    mockClient.listApprovals.mockResolvedValue({ items: [] });
    mockClient.getActivity.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-04-29T10:00:00Z" });
    mockClient.getDiagnostics.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-04-29T10:00:00Z" });
    mockClient.listEvaluationDiscoveryPolicies.mockResolvedValue({
      tenantId: "ten_personal",
      page: { limit: 20 },
      items: [{
        policyId: "policy_1",
        tenantId: "ten_personal",
        enabled: true,
        sourceKinds: ["run"],
        windowStart: "2026-04-29T09:00:00Z",
        windowEnd: "2026-04-29T10:00:00Z",
        maxInspectedRecords: 20,
        maxEmittedCandidates: 3,
        costBudget: 5,
        createdAt: "2026-04-29T10:00:00Z",
        updatedAt: "2026-04-29T10:00:00Z"
      }]
    });
    mockClient.listEvaluationDiscoveryRuns.mockResolvedValue({
      tenantId: "ten_personal",
      page: { limit: 20 },
      items: [{
        discoveryRunId: "discovery_run_1",
        tenantId: "ten_personal",
        policyId: "policy_1",
        status: "partial",
        sourceKinds: ["run"],
        windowStart: "2026-04-29T09:00:00Z",
        windowEnd: "2026-04-29T10:00:00Z",
        maxInspectedRecords: 20,
        maxEmittedCandidates: 3,
        costBudget: 5,
        inspectedRecords: 20,
        emittedCandidates: 1,
        partialReason: "max_inspected_records",
        startedAt: "2026-04-29T10:00:00Z",
        updatedAt: "2026-04-29T10:00:00Z"
      }]
    });
    mockClient.listEvaluationDiscoveredCandidates
      .mockResolvedValueOnce({
        tenantId: "ten_personal",
        page: { limit: 20 },
        items: [{
          discoveredCandidateId: "candidate_product_1",
          tenantId: "ten_personal",
          discoveryRunId: "discovery_run_1",
          sourceKind: "run",
          sourceId: "run_source_1",
          score: 0.92,
          scoreBand: "high",
          redactionStatus: "redacted",
          readinessStatus: "fully_replayable",
          suppressionState: "none",
          retentionState: "active",
          createdAt: "2026-04-29T10:00:00Z",
          updatedAt: "2026-04-29T10:00:00Z"
        }]
      })
      .mockResolvedValue({
        tenantId: "ten_personal",
        page: { limit: 20 },
        items: [{
          discoveredCandidateId: "candidate_product_1",
          tenantId: "ten_personal",
          discoveryRunId: "discovery_run_1",
          sourceKind: "run",
          sourceId: "run_source_1",
          score: 0.92,
          scoreBand: "high",
          redactionStatus: "redacted",
          readinessStatus: "fully_replayable",
          suppressionState: "suppressed",
          retentionState: "active",
          createdAt: "2026-04-29T10:00:00Z",
          updatedAt: "2026-04-29T10:01:00Z"
        }]
      });

    render(<App />);
    const user = userEvent.setup();
    await loadShell(user);

    await waitFor(() => {
      expect(screen.getByText("Candidate Discovery")).not.toBeNull();
      expect(screen.getByText("Product Candidates")).not.toBeNull();
      expect(screen.getByText("policy_1")).not.toBeNull();
      expect(screen.getByText("run:run_source_1")).not.toBeNull();
      expect(screen.getByText(/redacted evidence .* fully_replayable .* none/i)).not.toBeNull();
      expect(screen.getByText(/1 bounded discovery runs recorded/i)).not.toBeNull();
    });

    await user.click(screen.getByRole("button", { name: "Suppress" }));

    await waitFor(() => {
      expect(mockClient.createEvaluationSuppression).toHaveBeenCalledWith({
        targetKind: "discovered_candidate",
        targetId: "candidate_product_1",
        reasonCode: "operator_hidden",
        reason: "Suppressed from evaluation product review."
      }, { tenantId: "ten_personal" });
      expect(screen.getByText(/Suppressed candidate_product_1/i)).not.toBeNull();
      expect(screen.getByText(/redacted evidence .* fully_replayable .* suppressed/i)).not.toBeNull();
    });
  });

  it("creates, reviews, and suppresses product fixtures from discovered candidates", async () => {
    mockClient.getOnboarding.mockResolvedValue(onboardingFixture());
    mockClient.listApprovals.mockResolvedValue({ items: [] });
    mockClient.getActivity.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-04-29T10:00:00Z" });
    mockClient.getDiagnostics.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-04-29T10:00:00Z" });
    mockClient.listEvaluationDiscoveryPolicies.mockResolvedValue({ tenantId: "ten_personal", page: { limit: 20 }, items: [] });
    mockClient.listEvaluationDiscoveryRuns.mockResolvedValue({ tenantId: "ten_personal", page: { limit: 20 }, items: [] });
    mockClient.listEvaluationDiscoveredCandidates.mockResolvedValue({
      tenantId: "ten_personal",
      page: { limit: 20 },
      items: [{
        discoveredCandidateId: "candidate_product_1",
        tenantId: "ten_personal",
        discoveryRunId: "discovery_run_1",
        sourceKind: "run",
        sourceId: "run_source_1",
        score: 0.92,
        scoreBand: "high",
        redactionStatus: "redacted",
        readinessStatus: "fully_replayable",
        suppressionState: "none",
        retentionState: "active",
        createdAt: "2026-04-29T10:00:00Z",
        updatedAt: "2026-04-29T10:00:00Z"
      }]
    });
    mockClient.listProductFixtures
      .mockResolvedValueOnce({ tenantId: "ten_personal", page: { limit: 20 }, items: [] })
      .mockResolvedValueOnce({ tenantId: "ten_personal", page: { limit: 20 }, items: [productFixtureMutationFixture().fixture] })
      .mockResolvedValueOnce({ tenantId: "ten_personal", page: { limit: 20 }, items: [productFixtureMutationFixture({ revisionId: "revision_product_fixture_candidate_product_1_2" }).fixture] })
      .mockResolvedValueOnce({ tenantId: "ten_personal", page: { limit: 20 }, items: [productFixtureMutationFixture({ reviewState: "approved", revisionId: "revision_product_fixture_candidate_product_1_2" }).fixture] })
      .mockResolvedValue({ tenantId: "ten_personal", page: { limit: 20 }, items: [productFixtureMutationFixture({ reviewState: "approved", suppressionState: "suppressed", revisionId: "revision_product_fixture_candidate_product_1_2" }).fixture] });

    render(<App />);
    const user = userEvent.setup();
    await loadShell(user);

    await screen.findByRole("button", { name: "Create Fixture" });
    await user.click(screen.getByRole("button", { name: "Create Fixture" }));

    await waitFor(() => {
      expect(mockClient.materializeProductFixture).toHaveBeenCalledWith("candidate_product_1", expect.objectContaining({
        fixtureId: "product_fixture_candidate_product_1",
        displayName: "run:run_source_1",
        domainClass: "schedule"
      }), { tenantId: "ten_personal" });
      expect(screen.getByText(/Created product fixture product_fixture_candidate_product_1/i)).not.toBeNull();
      expect(screen.getAllByText(/product_fixture_candidate_product_1/i).length).toBeGreaterThan(0);
    });

    await user.click(screen.getByRole("button", { name: "Revise Fixture" }));
    await waitFor(() => {
      expect(mockClient.createProductFixtureRevision).toHaveBeenCalledWith("product_fixture_candidate_product_1", expect.objectContaining({
        contentSummary: "run:run_source_1",
        changeSummary: "Revised from operator shell."
      }), { tenantId: "ten_personal" });
      expect(screen.getByText(/Created product fixture revision revision_product_fixture_candidate_product_1_2/i)).not.toBeNull();
    });

    await user.click(screen.getByRole("button", { name: "Approve Fixture" }));
    await waitFor(() => {
      expect(mockClient.reviewProductFixture).toHaveBeenCalledWith("product_fixture_candidate_product_1", {
        revisionId: "revision_product_fixture_candidate_product_1_2",
        decision: "approved",
        reason: "Approved from operator shell."
      }, { tenantId: "ten_personal" });
      expect(screen.getByText(/Approved product fixture product_fixture_candidate_product_1/i)).not.toBeNull();
    });

    await user.click(screen.getByRole("button", { name: "Suppress Fixture" }));
    await waitFor(() => {
      expect(mockClient.suppressProductFixture).toHaveBeenCalledWith("product_fixture_candidate_product_1", {
        reasonCode: "operator_hidden",
        reason: "Suppressed from operator shell."
      }, { tenantId: "ten_personal" });
      expect(screen.getByText(/Suppressed product fixture product_fixture_candidate_product_1/i)).not.toBeNull();
    });
  });

  it("loads campaigns dashboard and tool-call inspections", async () => {
    mockClient.getOnboarding.mockResolvedValue(onboardingFixture());
    mockClient.listApprovals.mockResolvedValue({ items: [] });
    mockClient.getActivity.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-04-29T10:00:00Z" });
    mockClient.getDiagnostics.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-04-29T10:00:00Z" });
    mockClient.listProductFixtures.mockResolvedValue({ tenantId: "ten_personal", page: { limit: 20 }, items: [productFixtureMutationFixture({ reviewState: "approved" }).fixture] });
    mockClient.listEvaluationCampaigns
      .mockResolvedValueOnce({
        tenantId: "ten_personal",
        page: { limit: 20 },
        items: [{
          campaignId: "campaign_product_1",
          tenantId: "ten_personal",
          displayName: "Campaign Product",
          status: "completed",
          scopeSummary: "release gate",
          createdAt: "2026-04-29T10:00:00Z",
          retentionState: "active"
        }]
      })
      .mockResolvedValue({
        tenantId: "ten_personal",
        page: { limit: 20 },
        items: [{
          campaignId: "campaign_product_1",
          tenantId: "ten_personal",
          displayName: "Campaign Product",
          status: "published",
          scopeSummary: "release gate",
          createdAt: "2026-04-29T10:00:00Z",
          retentionState: "active"
        }]
      });
    mockClient.listEvaluationCampaignItems.mockResolvedValue({
      tenantId: "ten_personal",
      page: { limit: 20 },
      items: [{
        campaignItemId: "campaign_item_1",
        campaignId: "campaign_product_1",
        tenantId: "ten_personal",
        sourceType: "product_fixture",
        sourceId: "product_fixture_candidate_product_1",
        sourceSnapshot: { currentRevisionId: "revision_1" },
        suppressionCheckedAt: "2026-04-29T10:00:00Z",
        createdAt: "2026-04-29T10:00:00Z"
      }]
    });
    mockClient.listEvaluationCampaignAttemptGroups.mockResolvedValue({
      tenantId: "ten_personal",
      page: { limit: 20 },
      items: [{
        attemptGroupId: "attempt_group_1",
        campaignId: "campaign_product_1",
        campaignItemId: "campaign_item_1",
        tenantId: "ten_personal",
        replayAttemptIds: ["attempt_1"],
        comparisonIds: ["comparison_1"],
        liveValidationIds: ["ledger_1"],
        status: "completed",
        driftCount: 2,
        failureCount: 1,
        unsupportedCount: 0,
        operatorActionNeededCount: 1,
        createdAt: "2026-04-29T10:00:00Z",
        updatedAt: "2026-04-29T10:00:00Z"
      }]
    });
    mockClient.listEvaluationDashboard.mockResolvedValue({
      tenantId: "ten_personal",
      page: { limit: 20 },
      items: [{
        projectionId: "projection_1",
        tenantId: "ten_personal",
        windowStart: "2026-04-29T09:00:00Z",
        windowEnd: "2026-04-29T10:00:00Z",
        driftSummary: { total: 2 },
        failureSummary: { total: 1 },
        liveValidationSummary: { linked: 1 },
        generatedAt: "2026-04-29T10:00:00Z"
      }]
    });
    mockClient.listEvaluationToolCallInspections.mockResolvedValue({
      tenantId: "ten_personal",
      page: { limit: 20 },
      items: [{
        inspectionId: "inspection_1",
        tenantId: "ten_personal",
        campaignId: "campaign_product_1",
        campaignItemId: "campaign_item_1",
        toolCallRef: "tool_call_1",
        liveValidationLedgerRefs: ["ledger_1"],
        classification: "live_validation_completed",
        diffSummary: "redacted matched",
        redactionStatus: "redacted",
        createdAt: "2026-04-29T10:00:00Z",
        updatedAt: "2026-04-29T10:00:00Z"
      }]
    });

    render(<App />);
    const user = userEvent.setup();
    await loadShell(user);

    await waitFor(() => {
      expect(screen.getByText("Replay Campaigns")).not.toBeNull();
      expect(screen.getByText("Campaign Product")).not.toBeNull();
      expect(screen.getByText(/2 drift .* 1 failures .* 0 unsupported .* 1 action/i)).not.toBeNull();
      expect(screen.getByText("Evaluation Signals")).not.toBeNull();
      expect(screen.getAllByText("2").length).toBeGreaterThan(0);
      expect(screen.getByText("tool_call_1")).not.toBeNull();
      expect(screen.getByText(/redacted matched/i)).not.toBeNull();
    });

    await user.click(screen.getByRole("button", { name: "Publish Results" }));
    await waitFor(() => {
      expect(mockClient.publishEvaluationCampaignResults).toHaveBeenCalledWith("campaign_product_1", { tenantId: "ten_personal" });
      expect(screen.getByText(/Published campaign campaign_1/i)).not.toBeNull();
    });

    await user.click(screen.getByRole("button", { name: "Create Campaign" }));
    await waitFor(() => {
      expect(mockClient.createEvaluationCampaign).toHaveBeenCalledWith(expect.objectContaining({
        displayName: "Evaluation Campaign 2",
        sourceSelections: [expect.objectContaining({ sourceType: "product_fixture", sourceId: "product_fixture_candidate_product_1" })],
        startImmediately: true
      }), { tenantId: "ten_personal" });
    });
  });

  it("loads live validation gate state and starts with explicit scope selection", async () => {
    mockClient.getOnboarding.mockResolvedValue(onboardingFixture());
    mockClient.listApprovals.mockResolvedValue({ items: [] });
    mockClient.getActivity.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-04-24T10:00:00Z" });
    mockClient.getDiagnostics.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-04-24T10:00:00Z" });
    mockClient.listReplayCandidates.mockResolvedValue({
      environmentScope: "test",
      items: [
        {
          candidateId: "candidate_live",
          candidateKind: "fixture",
          displayName: "Live Schedule Fixture",
	          sourceKind: "fixture",
	          sourceId: "fixture_live",
	          sourceRefs: [],
	          toolClasses: ["read_only", "idempotent_mutation", "mcp.tool_call"],
	          environmentScope: "test",
          readinessStatus: "fully_replayable",
          readinessReasons: ["fixture has evidence"],
          limitations: [],
          defaultReplayMode: "non_live",
          createdAt: "2026-04-24T10:00:00Z",
          updatedAt: "2026-04-24T10:00:00Z"
        }
      ]
    });
    const blockedAttempt = {
      validationId: "lv_existing",
      tenantId: "ten_personal",
      candidateId: "candidate_live",
      requestedBy: "prn_1",
      environmentScope: "test",
      requestedScope: {
        scopeId: "scope_existing",
        validationId: "lv_existing",
        includedToolClasses: ["read_only"],
        approvalMode: "scope_level",
        declaredBy: "prn_1",
        declaredAt: "2026-04-24T10:00:00Z"
      },
      status: "blocked",
      permissionDecision: { allowed: true, checkedAt: "2026-04-24T10:00:00Z" },
      quotaDecision: { allowed: false, reasonCode: "quota_state_unavailable", checkedAt: "2026-04-24T10:00:00Z" },
      killSwitchDecision: { allowed: true, checkedAt: "2026-04-24T10:00:00Z" },
      approvalSummary: { required: 1, approved: 0, denied: 0, expired: 0, pending: 1 },
      ledgerSummary: {},
      createdAt: "2026-04-24T10:00:00Z",
      updatedAt: "2026-04-24T10:00:00Z"
    };
    mockClient.listLiveValidations.mockResolvedValue({ environmentScope: "test", items: [blockedAttempt] });
    mockClient.listLiveValidationLedger.mockResolvedValue({
      validationId: "lv_existing",
      items: [{
        ledgerEntryId: "ledger_1",
        validationId: "lv_existing",
        candidateId: "candidate_live",
        sourceRef: "tool_1",
        toolClass: "mail.send",
        safetyClass: "non_idempotent_mutation",
        actionRef: "send_1",
        outcome: "operator_action_needed",
        reasonCode: "live_validation.ambiguous_commit",
        updatedAt: "2026-04-29T10:00:00Z",
        retryCount: 0,
        ambiguousCommit: true
      }]
    });
    mockClient.listLiveValidationKillSwitches.mockResolvedValue({ items: [{ killSwitchId: "kill_1", scope: "tenant", tenantId: "ten_personal", enabled: true, reason: "containment", changedBy: "prn_owner", changedAt: "2026-04-29T10:00:00Z" }] });
    mockClient.updateLiveValidationKillSwitch.mockResolvedValue({ killSwitchId: "kill_1", scope: "tenant", tenantId: "ten_personal", enabled: true, reason: "containment", changedBy: "prn_owner", changedAt: "2026-04-29T10:00:00Z" });
    mockClient.listLiveValidationSupportMatrix.mockResolvedValue({
      environmentScope: "test",
      version: "v1",
      items: [{
        toolClass: "mcp.tool_call",
        safetyClass: "unsupported",
        approval: "unsupported",
        retryPolicy: "no_retry",
        compensation: "unsupported",
        ledgerEvents: ["skipped", "denied"],
        testCase: "MCP unsupported completeness test",
        version: "v1"
      }]
    });
    mockClient.startLiveValidation.mockResolvedValue({
      attempt: {
        ...blockedAttempt,
        validationId: "lv_new",
        requestedScope: { ...blockedAttempt.requestedScope, validationId: "lv_new", scopeId: "lv_new_scope", includedToolClasses: ["read_only", "idempotent_mutation"] }
      },
      denials: [{ gate: "quota", reasonCode: "quota_state_unavailable", message: "quota unavailable" }]
    });

    render(<App />);
    const user = userEvent.setup();
    await loadShell(user);

    await waitFor(() => {
      expect(mockClient.listLiveValidations).toHaveBeenCalledWith({ limit: 20 }, { tenantId: "ten_personal" });
      expect(mockClient.listLiveValidationSupportMatrix).toHaveBeenCalledWith({ tenantId: "ten_personal" });
      expect(screen.getByText("Live Validation Scope")).not.toBeNull();
      expect(screen.getByText(/quota: quota_state_unavailable/i)).not.toBeNull();
      expect(screen.getByText(/1 classes .* 1 unsupported/i)).not.toBeNull();
      expect(screen.getByText("mcp.tool_call")).not.toBeNull();
      expect(screen.getByText(/1 entries .* retention indefinite/i)).not.toBeNull();
      expect(screen.getByText("mail.send")).not.toBeNull();
      expect(screen.getByText(/1 active/i)).not.toBeNull();
    });

    await user.clear(screen.getByLabelText("Included tool classes"));
    await user.type(screen.getByLabelText("Included tool classes"), "read_only, idempotent_mutation");
    await user.selectOptions(screen.getByLabelText("Live validation approval mode"), "mixed");
    await user.click(screen.getByRole("button", { name: "Start Live Validation" }));

    await waitFor(() => {
	      expect(mockClient.startLiveValidation).toHaveBeenCalledWith(expect.objectContaining({
	        candidateId: "candidate_live",
	        candidateToolClasses: ["read_only", "idempotent_mutation", "mcp.tool_call"],
	        requestedScope: expect.objectContaining({
          includedToolClasses: ["read_only", "idempotent_mutation"],
          approvalMode: "mixed",
          declaredBy: "prn_1"
        })
      }), { tenantId: "ten_personal" });
      expect(screen.getByText(/Live validation lv_new blocked with 1 denial/i)).not.toBeNull();
    });

    await user.click(screen.getByRole("button", { name: "Enable Tenant Kill Switch" }));
    await waitFor(() => {
      expect(mockClient.updateLiveValidationKillSwitch).toHaveBeenCalledWith({
        scope: "tenant",
        enabled: true,
        reason: "Enabled from operator shell."
      }, { tenantId: "ten_personal" });
    });
  });

  it("keeps the shell rendered when evaluation collection fields arrive as null", async () => {
    mockClient.getOnboarding.mockResolvedValue(onboardingFixture());
    mockClient.listApprovals.mockResolvedValue({ items: [] });
    mockClient.getActivity.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-04-24T10:00:00Z" });
    mockClient.getDiagnostics.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-04-24T10:00:00Z" });
    mockClient.listReplayCandidates.mockResolvedValue({
      environmentScope: "test",
      items: [
        {
          candidateId: "candidate_legacy",
          candidateKind: "fixture",
          displayName: "Legacy Fixture",
          sourceKind: "fixture",
          sourceId: "fixture_legacy",
          sourceRefs: [],
          environmentScope: "test",
          readinessStatus: "fully_replayable",
          readinessReasons: null,
          limitations: null,
          defaultReplayMode: "non_live",
          createdAt: "2026-04-24T10:00:00Z",
          updatedAt: "2026-04-24T10:00:00Z"
        }
      ]
    });
    mockClient.listReplayComparisons.mockResolvedValue({
      environmentScope: "test",
      items: [
        {
          comparisonId: "comparison_legacy",
          candidateId: "candidate_legacy",
          baselineRef: "attempt_base",
          attemptId: "attempt_legacy",
          environmentScope: "test",
          terminalStatus: "matched",
          runtimeSummary: "runtime matched",
          policySummary: "policy matched",
          integrationSummary: "integration matched",
          deliverySummary: "delivery matched",
          evidenceSummary: "evidence matched",
          confidence: "high",
          limitations: null,
          driftFindings: null,
          generatedAt: "2026-04-24T10:00:00Z"
        }
      ]
    });
    mockClient.listReplayFixtures.mockResolvedValue({
      environmentScope: "test",
      items: [
        {
          fixtureId: "fixture_legacy",
          displayName: "Legacy Fixture",
          domainClass: "schedule",
          manifestPath: "manifest.json",
          sourceRefs: [],
          capturedEvidenceRefs: [],
          assumptions: null,
          limitations: null,
          expectedReplayMode: "non_live",
          expectedComparisonSummary: {},
          environmentScope: "test",
          createdAt: "2026-04-24T10:00:00Z",
          updatedAt: "2026-04-24T10:00:00Z"
        }
      ]
    });

    render(<App />);
    const user = userEvent.setup();
    await loadShell(user);

    await waitFor(() => {
      expect(screen.getAllByText("Legacy Fixture").length).toBeGreaterThan(0);
    });
    expect(screen.getByText(/Replay readiness is available/i)).not.toBeNull();
    expect(screen.getByText(/No fixture assumptions recorded/i)).not.toBeNull();
    expect(screen.getByText("comparison_legacy")).not.toBeNull();
  });

  it("loads identity and allowed tenants before tenant-scoped projections", async () => {
    const me = deferred<ReturnType<typeof authMeFixture>>();
    mockClient.getMe.mockReturnValueOnce(me.promise);
    mockClient.listTenants.mockResolvedValue({ items: [tenantFixture()] });
    mockClient.getOnboarding.mockResolvedValue(onboardingFixture());
    mockClient.listApprovals.mockResolvedValue({ items: [] });
    mockClient.getActivity.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-04-24T10:00:00Z" });
    mockClient.getDiagnostics.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-04-24T10:00:00Z" });

    render(<App />);
    const user = userEvent.setup();
    await loadShell(user);

    expect(screen.queryByRole("button", { name: "Launch test run" })).toBeNull();
    expect(mockClient.getOnboarding).not.toHaveBeenCalled();

    me.resolve(authMeFixture());

    await waitFor(() => {
      expect(mockClient.getOnboarding).toHaveBeenCalledWith({ tenantId: "ten_personal" });
      expect((screen.getByRole("button", { name: "Launch test run" }) as HTMLButtonElement).disabled).toBe(false);
    });
  });

  it("switches among allowed tenants, clears old rows, persists selection, and refreshes projections as one tenant batch", async () => {
    const personal = tenantFixture();
    const org = tenantFixture({
      tenantId: "ten_org",
      tenantKind: "organization",
      displayName: "Org Tenant",
      defaultForCurrentToken: false,
      defaultForCurrentPrincipal: false
    });
    mockClient.getMe.mockResolvedValue(authMeFixture([personal, org]));
    mockClient.listTenants.mockResolvedValue({ items: [personal, org] });
    mockClient.getOnboarding.mockResolvedValue(onboardingFixture());
    mockClient.listApprovals.mockResolvedValue({ items: [] });
    mockClient.getActivity
      .mockResolvedValueOnce({
        environmentScope: "test",
        generatedAt: "2026-04-24T10:00:00Z",
        items: [{
          activityId: "activity_a",
          sourceKind: "run",
          sourceId: "run_a",
          title: "Tenant A row",
          status: "completed",
          summary: "personal row",
          attentionLevel: "info",
          occurredAt: "2026-04-24T10:00:00Z",
          environmentScope: "test"
        }]
      })
      .mockResolvedValueOnce({
        environmentScope: "test",
        generatedAt: "2026-04-24T10:01:00Z",
        items: [{
          activityId: "activity_b",
          sourceKind: "run",
          sourceId: "run_b",
          title: "Tenant B row",
          status: "completed",
          summary: "org row",
          attentionLevel: "info",
          occurredAt: "2026-04-24T10:01:00Z",
          environmentScope: "test"
        }]
      });
    mockClient.getDiagnostics.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-04-24T10:00:00Z" });

    render(<App />);
    const user = userEvent.setup();
    await loadShell(user);

    await screen.findByText("Tenant A row");
    await user.selectOptions(screen.getByLabelText("Active Tenant"), "ten_org");

    expect(screen.queryByText("Tenant A row")).toBeNull();
    await screen.findByText("Tenant B row");
    expect(mockClient.getOnboarding).toHaveBeenLastCalledWith({ tenantId: "ten_org" });
    expect(mockClient.listApprovals).toHaveBeenLastCalledWith("pending", { tenantId: "ten_org" });
    expect(mockClient.getActivity).toHaveBeenLastCalledWith({ attentionOnly: true, limit: 20 }, { tenantId: "ten_org" });
    expect(mockClient.getDiagnostics).toHaveBeenLastCalledWith({ plane: undefined, severity: undefined }, { tenantId: "ten_org" });
    expect(mockClient.streamEvents).toHaveBeenLastCalledWith({}, expect.anything(), { tenantId: "ten_org" });
    expect(emptySubscription.close).toHaveBeenCalled();
    expect(window.localStorage.getItem("kura.activeTenant.http://127.0.0.1:19192.prn_1")).toBe("ten_org");

    cleanup();
    mockClient.getActivity.mockResolvedValue({
      environmentScope: "test",
      generatedAt: "2026-04-24T10:02:00Z",
      items: []
    });
    render(<App />);
    const restoreUser = userEvent.setup();
    await loadShell(restoreUser);
    await screen.findByText("Org Tenant (organization)");
  });

  it("shows stable denied state and does not retain previous tenant rows after active tenant revocation", async () => {
    const personal = tenantFixture();
    const org = tenantFixture({ tenantId: "ten_org", tenantKind: "organization", displayName: "Org Tenant" });
    const denial = Object.assign(new Error("tenant access denied"), {
      tenantDenied: true,
      status: 403,
      code: "tenant_access_denied"
    });
    mockClient.getMe.mockResolvedValue(authMeFixture([personal, org]));
    mockClient.listTenants.mockResolvedValue({ items: [personal, org] });
    mockClient.getOnboarding
      .mockResolvedValueOnce(onboardingFixture())
      .mockRejectedValueOnce(denial);
    mockClient.listApprovals.mockResolvedValue({ items: [] });
    mockClient.getActivity.mockResolvedValue({
      environmentScope: "test",
      generatedAt: "2026-04-24T10:00:00Z",
      items: [{
        activityId: "activity_a",
        sourceKind: "run",
        sourceId: "run_a",
        title: "Tenant A row",
        status: "completed",
        summary: "personal row",
        attentionLevel: "info",
        occurredAt: "2026-04-24T10:00:00Z",
        environmentScope: "test"
      }]
    });
    mockClient.getDiagnostics.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-04-24T10:00:00Z" });

    render(<App />);
    const user = userEvent.setup();
    await loadShell(user);
    await screen.findByText("Tenant A row");

    await user.selectOptions(screen.getByLabelText("Active Tenant"), "ten_org");

    await screen.findByText(/Tenant access was denied/i);
    expect(screen.queryByText("Tenant A row")).toBeNull();
    expect(screen.getAllByText("denied").length).toBeGreaterThan(0);
  });

  it("clears tenant projections when the active event stream reports tenant denial", async () => {
    const denial = Object.assign(new Error("tenant access denied"), {
      tenantDenied: true,
      status: 403,
      code: "tenant_access_denied"
    });
    let streamHandlers: { onError?: (error: Error) => void } | undefined;
    mockClient.streamEvents.mockImplementation((_query, handlers) => {
      streamHandlers = handlers as { onError?: (error: Error) => void };
      return emptySubscription;
    });
    mockClient.getOnboarding.mockResolvedValue(onboardingFixture());
    mockClient.listApprovals.mockResolvedValue({ items: [] });
    mockClient.getActivity.mockResolvedValue({
      environmentScope: "test",
      generatedAt: "2026-04-24T10:00:00Z",
      items: [{
        activityId: "activity_a",
        sourceKind: "run",
        sourceId: "run_a",
        title: "Tenant A row",
        status: "completed",
        summary: "personal row",
        attentionLevel: "info",
        occurredAt: "2026-04-24T10:00:00Z",
        environmentScope: "test"
      }]
    });
    mockClient.getDiagnostics.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-04-24T10:00:00Z" });

    render(<App />);
    const user = userEvent.setup();
    await loadShell(user);
    await screen.findByText("Tenant A row");

    await act(async () => {
      streamHandlers?.onError?.(denial);
    });

    await screen.findByText(/Tenant access was denied/i);
    expect(screen.queryByText("Tenant A row")).toBeNull();
    expect(screen.getAllByText("denied").length).toBeGreaterThan(0);
  });

  it("ignores stale in-flight action responses after switching tenants", async () => {
    const personal = tenantFixture();
    const org = tenantFixture({
      tenantId: "ten_org",
      tenantKind: "organization",
      displayName: "Org Tenant",
      defaultForCurrentToken: false,
      defaultForCurrentPrincipal: false
    });
    const run = deferred<{
      runId: string;
      entrypoint: string;
      status: string;
      goal: string;
      createdAt: string;
      updatedAt: string;
    }>();
    mockClient.getMe.mockResolvedValue(authMeFixture([personal, org]));
    mockClient.listTenants.mockResolvedValue({ items: [personal, org] });
    mockClient.getOnboarding.mockResolvedValue(onboardingFixture());
    mockClient.listApprovals.mockResolvedValue({ items: [] });
    mockClient.getActivity.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-04-24T10:00:00Z" });
    mockClient.getDiagnostics.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-04-24T10:00:00Z" });
    mockClient.createRun.mockReturnValueOnce(run.promise);

    render(<App />);
    const user = userEvent.setup();
    await loadShell(user);
    await screen.findByText("Personal Tenant (personal)");

    await user.click(screen.getByRole("button", { name: "Launch test run" }));
    await user.selectOptions(screen.getByLabelText("Active Tenant"), "ten_org");
    await screen.findByText("Org Tenant (organization)");

    run.resolve({
      runId: "run_old",
      entrypoint: "operator.shell.test",
      status: "queued",
      goal: "Run an operator shell smoke check.",
      createdAt: "2026-04-24T10:00:00Z",
      updatedAt: "2026-04-24T10:00:00Z"
    });

    await waitFor(() => {
      expect(mockClient.createRun).toHaveBeenCalledWith(expect.anything(), { tenantId: "ten_personal" });
      expect(screen.queryByText(/Created test run run_old/i)).toBeNull();
    });
  });

  it("ignores stale in-flight action and membership failures after switching tenants", async () => {
    const personal = tenantFixture();
    const org = tenantFixture({
      tenantId: "ten_org",
      tenantKind: "organization",
      displayName: "Org Tenant",
      defaultForCurrentToken: false,
      defaultForCurrentPrincipal: false
    });
    const run = deferred<{
      runId: string;
      entrypoint: string;
      status: string;
      goal: string;
      createdAt: string;
      updatedAt: string;
    }>();
    const member = membershipFixture({ membershipId: "mem_member", principalId: "prn_member", role: "viewer" });
    const membershipUpdate = deferred<{ membership: ReturnType<typeof membershipFixture> }>();
    mockClient.getMe.mockResolvedValue(authMeFixture([personal, org]));
    mockClient.listTenants.mockResolvedValue({ items: [personal, org] });
    mockClient.getOnboarding.mockResolvedValue(onboardingFixture());
    mockClient.listApprovals.mockResolvedValue({ items: [] });
    mockClient.getActivity.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-04-24T10:00:00Z" });
    mockClient.getDiagnostics.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-04-24T10:00:00Z" });
    mockClient.listMemberships.mockResolvedValue({ items: [membershipFixture(), member] });
    mockClient.createRun.mockReturnValueOnce(run.promise);
    mockClient.updateMembershipRole.mockReturnValueOnce(membershipUpdate.promise);

    render(<App />);
    const user = userEvent.setup();
    await loadShell(user);
    await screen.findByText("Personal Tenant (personal)");

    await user.click(screen.getByRole("button", { name: "Launch test run" }));
    await user.selectOptions(await screen.findByLabelText("Role for prn_member"), "admin");
    await user.selectOptions(screen.getByLabelText("Active Tenant"), "ten_org");
    await screen.findByText("Org Tenant (organization)");

    await act(async () => {
      run.reject(new Error("old tenant action failed"));
      membershipUpdate.reject(new Error("old tenant membership failed"));
      await Promise.resolve();
    });

    expect(screen.queryByText(/old tenant action failed/i)).toBeNull();
    expect(screen.queryByText(/old tenant membership failed/i)).toBeNull();
  });

  it("hides membership role controls without tenant.manage and updates authorized roles from daemon-confirmed state", async () => {
    const viewerTenant = tenantFixture({
      callerMembershipRole: "viewer",
      callerPermissions: ["read_only.inspect"]
    });
    mockClient.getMe.mockResolvedValue(authMeFixture([viewerTenant]));
    mockClient.listTenants.mockResolvedValue({ items: [viewerTenant] });
    mockClient.getOnboarding.mockResolvedValue(onboardingFixture());
    mockClient.listApprovals.mockResolvedValue({ items: [] });
    mockClient.getActivity.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-04-24T10:00:00Z" });
    mockClient.getDiagnostics.mockResolvedValue({ environmentScope: "test", items: [], generatedAt: "2026-04-24T10:00:00Z" });

    render(<App />);
    const user = userEvent.setup();
    await loadShell(user);

    await screen.findByText(/Membership role controls are unavailable/i);
    expect(screen.queryByLabelText(/Role for/i)).toBeNull();

    cleanup();

    const adminTenant = tenantFixture();
    const member = membershipFixture({ membershipId: "mem_member", principalId: "prn_member", role: "viewer" });
    mockClient.getMe.mockResolvedValue(authMeFixture([adminTenant]));
    mockClient.listTenants.mockResolvedValue({ items: [adminTenant] });
    mockClient.listMemberships.mockResolvedValue({ items: [membershipFixture(), member] });
    mockClient.updateMembershipRole.mockResolvedValue({ membership: { ...member, role: "admin" } });

    render(<App />);
    const adminUser = userEvent.setup();
    await loadShell(adminUser);

    const roleSelect = await screen.findByLabelText("Role for prn_member");
    await adminUser.selectOptions(roleSelect, "admin");

    await waitFor(() => {
      expect(mockClient.updateMembershipRole).toHaveBeenCalledWith("ten_personal", "mem_member", { role: "admin" }, { tenantId: "ten_personal" });
      expect(screen.getByText(/Updated prn_member to admin/i)).not.toBeNull();
    });

    mockClient.updateMembershipRole.mockRejectedValueOnce(new Error("last owner would be removed"));
    await adminUser.selectOptions(screen.getByLabelText("Role for prn_member"), "viewer");

    await waitFor(() => {
      expect(screen.getAllByText("last owner would be removed").length).toBeGreaterThan(0);
      expect((screen.getByLabelText("Role for prn_member") as HTMLSelectElement).value).toBe("admin");
    });
  });
});
