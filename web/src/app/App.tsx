import { useEffect, useRef, useState } from "react";

import {
  createKuraClient,
  type ActivationDiagnosticListResponse,
  type ActivationStateResource,
  type AgentProfileDetailResponse,
  type AgentProfileListResponse,
  type AgentProfileMutationInput,
  type ApprovalResource,
  type AuthMeResponse,
  type MemoryOverview,
  type BillingDenialResource,
  type BillingQuotaDashboardResponse,
  type BillingQuotaStatusItem,
  type ChannelConnectorDetailResource,
  type EventStreamSubscription,
  type EvaluationCampaignAttemptGroupResource,
  type EvaluationCampaignItemResource,
  type EvaluationCampaignResource,
  type EvaluationDashboardProjectionResource,
  type EvaluationDiscoveredCandidateResource,
  type EvaluationDiscoveryPolicyResource,
  type EvaluationDiscoveryRunResource,
  type EvaluationToolCallInspectionResource,
  type LiveValidationAttemptResource,
  type LiveValidationLedgerResource,
  type LiveValidationRetentionResource,
  type LiveValidationKillSwitchResource,
  type LiveValidationSupportMatrixResource,
  type MembershipResource,
  type OperatorActivityListResponse,
  type OperatorActivityRecord,
  type OperatorDiagnosticFinding,
  type OperatorDiagnosticListResponse,
  type OperatorFirstUsefulAction,
  type OperatorOnboardingResponse,
  type ProductFixtureResource,
  type ReplayAttemptResource,
  type ReplayCandidateResource,
  type ReplayComparisonResource,
  type ReplayFixtureResource,
  type SetupDiagnosticResource,
  type SetupSessionResource,
  type SetupTargetResource,
  type TenantPermission,
  type TenantRequestOptions,
  type TenantResource,
  type TenantRole,
  type ThreadListResponse,
  type ChannelConnectorListResponse,
  type ChannelManagementActionInput,
  type ChannelManagementSupportEvidence
} from "@kura/client";
import { ChannelManagementView } from "../features/channel-management";
import { AgentProfileEditor } from "../features/agent-profiles/AgentProfileEditor";
import { AgentProfileHistory } from "../features/agent-profiles/AgentProfileHistory";
import { ThreadLifecycleView } from "../features/thread-lifecycle";
import { MemoryOverviewPanel } from "../features/memory-overview";
import { isGatewayMode } from "./gatewayMode";

const DEFAULT_DAEMON_URL = "http://127.0.0.1:19192";
// Behind kura-gateway the page and the API share an origin, and the gateway
// authenticates the user and attaches the daemon token itself.
const GATEWAY_MODE = isGatewayMode();
const DEFAULT_RUN_GOAL = "Run an operator shell smoke check.";
const DEFAULT_TEST_QUERY = "Return one bounded readiness confirmation.";
const DEFAULT_ACTIVATION_TEST_CHAT = "Run a safe hosted activation test.";

type ShellStatus = "idle" | "loading" | "ready" | "error";
type EventStatus = "disconnected" | "connected" | "error";
type ActiveTenantStatus = "unresolved" | "resolving" | "active" | "stale" | "denied";
type MembershipStatus = "hidden" | "loading" | "ready" | "empty" | "denied" | "error";

type ShellSnapshot = {
  onboarding: OperatorOnboardingResponse | null;
  activation: ActivationStateResource | null;
  activationDiagnostics: ActivationDiagnosticListResponse | null;
  setupTargets: SetupTargetResource[];
  setupSessions: SetupSessionResource[];
  setupDiagnostics: SetupDiagnosticResource[];
  approvals: ApprovalResource[];
  activity: OperatorActivityListResponse | null;
  diagnostics: OperatorDiagnosticListResponse | null;
  replayCandidates: ReplayCandidateResource[];
  discoveryPolicies: EvaluationDiscoveryPolicyResource[];
  discoveryRuns: EvaluationDiscoveryRunResource[];
  discoveredCandidates: EvaluationDiscoveredCandidateResource[];
  campaigns: EvaluationCampaignResource[];
  campaignItems: EvaluationCampaignItemResource[];
  campaignAttemptGroups: EvaluationCampaignAttemptGroupResource[];
  dashboardProjections: EvaluationDashboardProjectionResource[];
  toolCallInspections: EvaluationToolCallInspectionResource[];
  replayAttempts: ReplayAttemptResource[];
  replayComparisons: ReplayComparisonResource[];
  replayFixtures: ReplayFixtureResource[];
  productFixtures: ProductFixtureResource[];
  liveValidations: LiveValidationAttemptResource[];
  supportMatrix: LiveValidationSupportMatrixResource[];
  liveValidationLedger: LiveValidationLedgerResource[];
  liveValidationRetention: LiveValidationRetentionResource | null;
  liveValidationKillSwitches: LiveValidationKillSwitchResource[];
  billingDashboard: BillingQuotaDashboardResponse | null;
  billingDenials: BillingDenialResource[];
  channelConnectors: ChannelConnectorListResponse | null;
  selectedChannelConnector: ChannelConnectorDetailResource | null;
  channelSupportEvidence: ChannelManagementSupportEvidence | null;
  threads: ThreadListResponse | null;
  profiles: AgentProfileListResponse | null;
  memoryOverview: MemoryOverview | null;
  selectedProfile: AgentProfileDetailResponse | null;
};

type DetailView = {
  title: string;
  route?: string;
  tenantId: string;
  generation: number;
  payload: unknown;
};

type MembershipPanelState = {
  status: MembershipStatus;
  members: MembershipResource[];
  error: string;
  pendingMembershipId: string;
};

const EMPTY_SHELL: ShellSnapshot = {
  onboarding: null,
  activation: null,
  activationDiagnostics: null,
  setupTargets: [],
  setupSessions: [],
  setupDiagnostics: [],
  approvals: [],
  activity: null,
  diagnostics: null,
  replayCandidates: [],
  discoveryPolicies: [],
  discoveryRuns: [],
  discoveredCandidates: [],
  campaigns: [],
  campaignItems: [],
  campaignAttemptGroups: [],
  dashboardProjections: [],
  toolCallInspections: [],
  replayAttempts: [],
  replayComparisons: [],
  replayFixtures: [],
  productFixtures: [],
  liveValidations: [],
  supportMatrix: [],
  liveValidationLedger: [],
  liveValidationRetention: null,
  liveValidationKillSwitches: [],
  billingDashboard: null,
  billingDenials: [],
  channelConnectors: null,
  selectedChannelConnector: null,
  channelSupportEvidence: null,
  threads: null,
  memoryOverview: null,
  profiles: null,
  selectedProfile: null
};

const ROLE_OPTIONS: TenantRole[] = ["owner", "admin", "operator", "viewer"];

export function App() {
  const [daemonURL, setDaemonURL] = useState(GATEWAY_MODE ? window.location.origin : DEFAULT_DAEMON_URL);
  const [accessToken, setAccessToken] = useState("");
  const [status, setStatus] = useState<ShellStatus>("idle");
  const [eventStatus, setEventStatus] = useState<EventStatus>("disconnected");
  const [error, setError] = useState("");
  const [actionMessage, setActionMessage] = useState("");
  const [lastEvent, setLastEvent] = useState("");
  const [detail, setDetail] = useState<DetailView | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);
  const [activeActionId, setActiveActionId] = useState("");
  const [runGoal, setRunGoal] = useState(DEFAULT_RUN_GOAL);
  const [testQuery, setTestQuery] = useState(DEFAULT_TEST_QUERY);
  const [testThreadId, setTestThreadId] = useState("");
  const [setupSecretRef, setSetupSecretRef] = useState("provider/openai-compatible");
  const [setupSecretValue, setSetupSecretValue] = useState("");
  const [liveValidationCandidateId, setLiveValidationCandidateId] = useState("");
  const [liveValidationToolClasses, setLiveValidationToolClasses] = useState("read_only");
  const [liveValidationApprovalMode, setLiveValidationApprovalMode] = useState<"scope_level" | "per_action" | "mixed">("scope_level");
  const [diagnosticPlane, setDiagnosticPlane] = useState("");
  const [diagnosticSeverity, setDiagnosticSeverity] = useState("");
  const [authMe, setAuthMe] = useState<AuthMeResponse | null>(null);
  const [allowedTenants, setAllowedTenants] = useState<TenantResource[]>([]);
  const [activeTenantId, setActiveTenantId] = useState("");
  const [activeTenantStatus, setActiveTenantStatus] = useState<ActiveTenantStatus>("unresolved");
  const [tenantMessage, setTenantMessage] = useState("Tenant context has not been resolved.");
  const [shell, setShell] = useState<ShellSnapshot>(EMPTY_SHELL);
  const [memberships, setMemberships] = useState<MembershipPanelState>({
    status: "hidden",
    members: [],
    error: "",
    pendingMembershipId: ""
  });

  const generationRef = useRef(0);
  const activeTenantRef = useRef("");

  const activeTenant = allowedTenants.find((tenant) => tenant.tenantId === activeTenantId) ?? null;
  const tenantOptions = activeTenantId ? { tenantId: activeTenantId } : undefined;
  const canUseTenantActions = status === "ready" && activeTenantStatus === "active" && Boolean(activeTenantId);
  const canManageMemberships = hasPermission(activeTenant, "tenant.manage");
  const canViewBilling = hasPermission(activeTenant, "billing.view");
  const canExportBillingEvidence = hasPermission(activeTenant, "billing.evidence_export");

  function buildClient(defaultTenantId = activeTenantId) {
    return createKuraClient({
      baseURL: daemonURL,
      accessToken: accessToken.trim() || undefined,
      defaultTenantId: defaultTenantId || undefined
    });
  }

  function markActiveTenantDenied(message: string) {
    activeTenantRef.current = "";
    setActiveTenantStatus("denied");
    setTenantMessage(message);
    setShell(EMPTY_SHELL);
    setDetail(null);
    setDetailLoading(false);
    setMemberships({ status: "denied", members: [], error: message, pendingMembershipId: "" });
    setStatus("ready");
  }

  function isCurrentTenantWork(generation: number, tenantId: string): boolean {
    return generation === generationRef.current && activeTenantRef.current === tenantId;
  }

  async function refreshShell(options: { soft?: boolean; tenantId?: string; explicitSelection?: boolean } = {}) {
    if (!GATEWAY_MODE && !accessToken.trim()) {
      setStatus("error");
      setActiveTenantStatus("denied");
      setError("Access token is required to load the operator shell.");
      return;
    }

    const generation = generationRef.current + 1;
    generationRef.current = generation;
    activeTenantRef.current = "";
    setError("");
    if (!options.soft) {
      setActionMessage("");
      setDetail(null);
      setDetailLoading(false);
      setActiveActionId("");
    }
    setMemberships({ status: "loading", members: [], error: "", pendingMembershipId: "" });
    setActiveTenantStatus(options.soft ? "stale" : "resolving");
    setTenantMessage(options.soft ? "Refreshing tenant-scoped views." : "Resolving tenant context.");
    if (!options.soft) {
      setShell(EMPTY_SHELL);
      setStatus("loading");
    }

    try {
      const bootstrapClient = buildClient("");
      let me = await bootstrapClient.getMe();
      let tenantList = await bootstrapClient.listTenants();
      if (generation !== generationRef.current) {
        return;
      }

      let allowed = tenantList.items.length ? tenantList.items : me.allowedTenants;
      if (allowed.length === 0 && !options.explicitSelection) {
        await bootstrapClient.activate({ source: "signup" });
        if (generation !== generationRef.current) {
          return;
        }
        me = await bootstrapClient.getMe();
        tenantList = await bootstrapClient.listTenants();
        if (generation !== generationRef.current) {
          return;
        }
        allowed = tenantList.items.length ? tenantList.items : me.allowedTenants;
      }
      setAuthMe(me);
      setAllowedTenants(allowed);

      const selected = resolveActiveTenant({
        allowed,
        me,
        requestedTenantId: options.tenantId,
        explicitSelection: options.explicitSelection,
        savedTenantId: readTenantPreference(daemonURL, me.principal.principalId)
      });

      if (!selected.tenant) {
        setActiveTenantId("");
        markActiveTenantDenied(selected.message);
        return;
      }

      const tenant = selected.tenant;
      activeTenantRef.current = tenant.tenantId;
      setActiveTenantId(tenant.tenantId);
      setActiveTenantStatus("active");
      setTenantMessage(selected.message);
      if (options.explicitSelection) {
        writeTenantPreference(daemonURL, me.principal.principalId, tenant.tenantId);
      }

      const scopedClient = buildClient(tenant.tenantId);
      const scopedOptions: TenantRequestOptions = { tenantId: tenant.tenantId };
      const membershipPromise = hasPermission(tenant, "tenant.manage")
        ? scopedClient.listMemberships(tenant.tenantId, {}, scopedOptions).then((response) => response.items)
        : Promise.resolve<MembershipResource[]>([]);
      const billingDashboardPromise = hasPermission(tenant, "billing.view")
        ? scopedClient.getBillingQuotaDashboard(scopedOptions).catch(() => null)
        : Promise.resolve(null);
      const billingDenialsPromise = hasPermission(tenant, "billing.view")
        ? scopedClient.listBillingDenials(scopedOptions).then((response) => response.items).catch(() => [])
        : Promise.resolve<BillingDenialResource[]>([]);
      const channelConnectorsPromise = hasPermission(tenant, "credentials.inspect")
        ? scopedClient.listChannelConnectors({ limit: 20 }, scopedOptions).catch(() => null)
        : Promise.resolve(null);
      const threadsPromise = hasPermission(tenant, "credentials.inspect")
        ? scopedClient.listThreads({ limit: 20 }, scopedOptions).catch(() => null)
        : Promise.resolve(null);
      const profilesPromise = hasPermission(tenant, "profiles.inspect")
        ? scopedClient.listAgentProfiles({ limit: 20 }, scopedOptions).catch(() => null)
        : Promise.resolve(null);

      const [
        onboarding,
        activation,
        activationDiagnostics,
        setupTargets,
        setupSessions,
        approvals,
        activity,
        diagnostics,
        replayCandidates,
        discoveryPolicies,
        discoveryRuns,
        discoveredCandidates,
        campaigns,
        dashboardProjections,
        replayAttempts,
        replayComparisons,
        replayFixtures,
        productFixtures,
        liveValidations,
        supportMatrix,
        killSwitches,
        billingDashboard,
        billingDenials,
        membershipItems,
        channelConnectors,
        threads,
        profiles
      ] = await Promise.all([
        scopedClient.getOnboarding(scopedOptions),
        scopedClient.getActivation(scopedOptions).then((response) => response.activation).catch(() => null),
        scopedClient.getActivationDiagnostics(scopedOptions).catch(() => ({ items: [] })),
        scopedClient.listSetupTargets(scopedOptions).catch(() => ({ items: [] })),
        scopedClient.listSetupSessions(scopedOptions).catch(() => ({ items: [] })),
        scopedClient.listApprovals("pending", scopedOptions),
        scopedClient.getActivity({ attentionOnly: true, limit: 20 }, scopedOptions),
        scopedClient.getDiagnostics({
          plane: diagnosticPlane ? (diagnosticPlane as OperatorDiagnosticFinding["plane"]) : undefined,
          severity: diagnosticSeverity ? (diagnosticSeverity as OperatorDiagnosticFinding["severity"]) : undefined
        }, scopedOptions),
        scopedClient.listReplayCandidates({ limit: 20 }, scopedOptions),
        scopedClient.listEvaluationDiscoveryPolicies({ limit: 20 }, scopedOptions),
        scopedClient.listEvaluationDiscoveryRuns({ limit: 20 }, scopedOptions),
        scopedClient.listEvaluationDiscoveredCandidates({ limit: 20 }, scopedOptions),
        scopedClient.listEvaluationCampaigns({ limit: 20 }, scopedOptions),
        scopedClient.listEvaluationDashboard({ limit: 20 }, scopedOptions),
        scopedClient.listReplayAttempts({ limit: 20 }, scopedOptions),
        scopedClient.listReplayComparisons({ limit: 20 }, scopedOptions),
        scopedClient.listReplayFixtures({}, scopedOptions),
        scopedClient.listProductFixtures({ limit: 20 }, scopedOptions),
        scopedClient.listLiveValidations({ limit: 20 }, scopedOptions),
        scopedClient.listLiveValidationSupportMatrix(scopedOptions),
        scopedClient.listLiveValidationKillSwitches({}, scopedOptions),
        billingDashboardPromise,
        billingDenialsPromise,
        membershipPromise,
        channelConnectorsPromise,
        threadsPromise,
        profilesPromise
      ]);
      const latestValidation = liveValidations.items[0] ?? null;
      const latestCampaign = campaigns.items[0] ?? null;
      const firstSetupSession = setupSessions.items[0] ?? null;
      const firstChannelConnectorID = channelConnectors?.items[0]?.connectorId ?? "";
      const firstProfileID = profiles?.items[0]?.profileId ?? "";
      const channelConnectorDetailPromise = firstChannelConnectorID
        ? scopedClient.getChannelConnector(firstChannelConnectorID, scopedOptions).catch(() => null)
        : Promise.resolve(null);
      const channelSupportEvidencePromise = firstChannelConnectorID
        ? scopedClient.getChannelConnectorSupportEvidence(firstChannelConnectorID, scopedOptions).catch(() => null)
        : Promise.resolve(null);
      const selectedProfilePromise = firstProfileID
        ? scopedClient.getAgentProfile(firstProfileID, scopedOptions).catch(() => null)
        : Promise.resolve(null);
      const [setupDiagnostics, liveValidationLedger, liveValidationRetention, campaignItems, campaignAttemptGroups, toolCallInspections, selectedChannelConnector, channelSupportEvidence, selectedProfile] = await Promise.all([
        firstSetupSession ? scopedClient.getSetupDiagnostics(firstSetupSession.setupSessionId, scopedOptions).then((response) => response.items).catch(() => []) : Promise.resolve([]),
        latestValidation ? scopedClient.listLiveValidationLedger(latestValidation.validationId, { limit: 20 }, scopedOptions).then((response) => response.items) : Promise.resolve([]),
        latestValidation ? scopedClient.getLiveValidationRetention(latestValidation.validationId, scopedOptions) : Promise.resolve(null),
        latestCampaign ? scopedClient.listEvaluationCampaignItems(latestCampaign.campaignId, { limit: 20 }, scopedOptions).then((response) => response.items) : Promise.resolve([]),
        latestCampaign ? scopedClient.listEvaluationCampaignAttemptGroups(latestCampaign.campaignId, { limit: 20 }, scopedOptions).then((response) => response.items) : Promise.resolve([]),
        latestCampaign ? scopedClient.listEvaluationToolCallInspections(latestCampaign.campaignId, { limit: 20 }, scopedOptions).then((response) => response.items) : Promise.resolve([]),
        channelConnectorDetailPromise,
        channelSupportEvidencePromise,
        selectedProfilePromise
      ]);

      // The memory inventory is best-effort: a daemon without the memory
      // plugin enabled answers 500 here, and that must not take the whole
      // shell down with it.
      const memoryOverview: MemoryOverview | null = await Promise.resolve()
        .then(() => scopedClient.getMemoryOverview(undefined, scopedOptions))
        .then((value) => value ?? null)
        .catch(() => null);

      if (generation !== generationRef.current || activeTenantRef.current !== tenant.tenantId) {
        return;
      }

      setShell({
        onboarding,
        memoryOverview,
        activation,
        activationDiagnostics,
        setupTargets: setupTargets.items,
        setupSessions: setupSessions.items,
        setupDiagnostics,
        approvals: approvals.items,
        activity,
        diagnostics,
        replayCandidates: replayCandidates.items,
        discoveryPolicies: discoveryPolicies.items,
        discoveryRuns: discoveryRuns.items,
        discoveredCandidates: discoveredCandidates.items,
        campaigns: campaigns.items,
        campaignItems,
        campaignAttemptGroups,
        dashboardProjections: dashboardProjections.items,
        toolCallInspections,
        replayAttempts: replayAttempts.items,
        replayComparisons: replayComparisons.items,
        replayFixtures: replayFixtures.items,
        productFixtures: productFixtures.items,
        liveValidations: liveValidations.items,
        supportMatrix: supportMatrix.items,
        liveValidationLedger,
        liveValidationRetention,
        liveValidationKillSwitches: killSwitches.items,
        billingDashboard,
        billingDenials,
        channelConnectors,
        selectedChannelConnector,
        channelSupportEvidence,
        threads,
        profiles,
        selectedProfile
      });
      setMemberships({
        status: hasPermission(tenant, "tenant.manage") ? membershipStatusFor(membershipItems) : "hidden",
        members: membershipItems,
        error: "",
        pendingMembershipId: ""
      });
      setStatus("ready");
    } catch (caught) {
      const message = errorMessage(caught);
      if (generation !== generationRef.current) {
        return;
      }
      if (isTenantDenied(caught)) {
        markActiveTenantDenied("Tenant access was denied. Choose another allowed tenant before continuing.");
        setMemberships({ status: "denied", members: [], error: message, pendingMembershipId: "" });
        return;
      }
      setStatus("error");
      setActiveTenantStatus("denied");
      setError(message);
      setMemberships({ status: "error", members: [], error: message, pendingMembershipId: "" });
    }
  }

  useEffect(() => {
    if (GATEWAY_MODE) {
      void refreshShell();
    }
    // Load once on mount; later loads are user- or event-driven.
  }, []);

  useEffect(() => {
    if (!canUseTenantActions || (!GATEWAY_MODE && !accessToken.trim()) || !tenantOptions) {
      setEventStatus("disconnected");
      return;
    }

    const streamTenantId = activeTenantId;
    const client = buildClient(streamTenantId);
    const subscription: EventStreamSubscription = client.streamEvents({}, {
      onEvent(event) {
        if (activeTenantRef.current !== streamTenantId) {
          return;
        }
        setLastEvent(`${event.name} #${event.sequence}`);
        void refreshShell({ soft: true, tenantId: streamTenantId });
      },
      onError(streamError) {
        if (activeTenantRef.current !== streamTenantId) {
          return;
        }
        if (isTenantDenied(streamError)) {
          markActiveTenantDenied("Tenant access was denied. Choose another allowed tenant before continuing.");
          return;
        }
        setEventStatus("error");
        setError(streamError.message);
      }
    }, tenantOptions);
    setEventStatus("connected");

    return () => {
      subscription.close();
      setEventStatus("disconnected");
    };
  }, [canUseTenantActions, activeTenantId, daemonURL, accessToken, diagnosticPlane, diagnosticSeverity]);

  async function inspectRoute(route: string, title: string) {
    const scoped = currentTenantOptions();
    if (!scoped) {
      setError("Resolve an active tenant before inspecting tenant-scoped details.");
      return;
    }
    const generation = generationRef.current;
    const tenantId = scoped.tenantId!;
    setDetailLoading(true);
    setError("");
    try {
      const payload = await buildClient(tenantId).fetchRoute<unknown>(route, scoped);
      if (generation !== generationRef.current || activeTenantRef.current !== tenantId) {
        return;
      }
      setDetail({ title, route, tenantId, generation, payload });
    } catch (caught) {
      if (!isCurrentTenantWork(generation, tenantId)) {
        return;
      }
      const message = errorMessage(caught);
      if (isTenantDenied(caught)) {
        markActiveTenantDenied("Tenant access was denied. Choose another allowed tenant before continuing.");
        return;
      }
      setError(message);
    } finally {
      if (activeTenantRef.current === tenantId) {
        setDetailLoading(false);
      }
    }
  }

  async function handleApprovalDecision(approval: ApprovalResource, resolution: "approved" | "rejected") {
    const scoped = currentTenantOptions();
    if (!scoped) {
      return;
    }
    const tenantId = scoped.tenantId!;
    const generation = generationRef.current;
    setActiveActionId(approval.approvalId);
    setError("");
    try {
      const response = await buildClient(tenantId).resolveApproval(approval.approvalId, {
        resolution,
        comment: "Resolved in operator shell."
      }, scoped);
      if (generation !== generationRef.current || activeTenantRef.current !== tenantId) {
        return;
      }
      setActionMessage(`${approval.action} ${resolution}.`);
      setDetail({
        title: `Approval ${approval.approvalId}`,
        route: `/v1/policy/approvals/${approval.approvalId}`,
        tenantId,
        generation,
        payload: response
      });
      setActiveActionId("");
      await refreshShell({ soft: true, tenantId });
    } catch (caught) {
      if (!isCurrentTenantWork(generation, tenantId)) {
        return;
      }
      if (isTenantDenied(caught)) {
        markActiveTenantDenied("Tenant access was denied. Choose another allowed tenant before continuing.");
        return;
      }
      setError(errorMessage(caught));
    } finally {
      if (activeTenantRef.current === tenantId) {
        setActiveActionId("");
      }
    }
  }

  async function handleFirstUsefulAction(action: OperatorFirstUsefulAction) {
    const scoped = currentTenantOptions();
    if (!scoped) {
      return;
    }
    const tenantId = scoped.tenantId!;
    const generation = generationRef.current;
    setActiveActionId(action.actionId);
    setError("");
    try {
      if (action.actionKind === "test_run") {
        const run = await buildClient(tenantId).createRun({
          entrypoint: "operator.shell.test",
          goal: runGoal.trim() || DEFAULT_RUN_GOAL
        }, scoped);
        if (generation !== generationRef.current || activeTenantRef.current !== tenantId) {
          return;
        }
        setActionMessage(`Created test run ${run.runId}.`);
        setDetail({ title: "Latest Test Run", route: `/v1/runs/${run.runId}`, tenantId, generation, payload: run });
      } else if (action.actionKind === "test_query") {
        const queryPayload = {
          query: testQuery.trim() || DEFAULT_TEST_QUERY,
          ...(testThreadId.trim() ? { threadId: testThreadId.trim() } : {})
        };
        const response = await buildClient(tenantId).queryChat(queryPayload, scoped);
        if (generation !== generationRef.current || activeTenantRef.current !== tenantId) {
          return;
        }
        setActionMessage(`Test query completed with ${response.usage.totalTokens} total tokens.`);
        setDetail({ title: "Test Query Result", route: action.resultRoute, tenantId, generation, payload: response });
      }
      setActiveActionId("");
      await refreshShell({ soft: true, tenantId });
    } catch (caught) {
      if (!isCurrentTenantWork(generation, tenantId)) {
        return;
      }
      if (isTenantDenied(caught)) {
        markActiveTenantDenied("Tenant access was denied. Choose another allowed tenant before continuing.");
        return;
      }
      setError(errorMessage(caught));
    } finally {
      if (activeTenantRef.current === tenantId) {
        setActiveActionId("");
      }
    }
  }

  async function handleActivationFirstAction() {
    const activation = shell.activation;
    if (!activation) {
      return;
    }
    const scoped = currentTenantOptions();
    if (!scoped) {
      return;
    }
    const tenantId = scoped.tenantId!;
    const generation = generationRef.current;
    setActiveActionId(activation.firstAction.actionId);
    setError("");
    try {
      const response = await buildClient(tenantId).runActivationTestChat({ message: DEFAULT_ACTIVATION_TEST_CHAT }, scoped);
      if (generation !== generationRef.current || activeTenantRef.current !== tenantId) {
        return;
      }
      setActionMessage(`Activation test chat ${response.testChat.status}.`);
      setDetail({ title: "Activation Test Chat", route: activation.firstAction.resultRoute, tenantId, generation, payload: response });
      setActiveActionId("");
      await refreshShell({ soft: true, tenantId });
    } catch (caught) {
      if (!isCurrentTenantWork(generation, tenantId)) {
        return;
      }
      if (isTenantDenied(caught)) {
        markActiveTenantDenied("Tenant access was denied. Choose another allowed tenant before continuing.");
        return;
      }
      setError(errorMessage(caught));
    } finally {
      if (activeTenantRef.current === tenantId) {
        setActiveActionId("");
      }
    }
  }

  async function handleLaunchReplay(candidate: ReplayCandidateResource) {
    const scoped = currentTenantOptions();
    if (!scoped) {
      return;
    }
    const tenantId = scoped.tenantId!;
    const generation = generationRef.current;
    setActiveActionId(candidate.candidateId);
    setError("");
    try {
      const attempt = await buildClient(tenantId).createReplayAttempt(candidate.candidateId, { mode: "non_live" }, scoped);
      if (generation !== generationRef.current || activeTenantRef.current !== tenantId) {
        return;
      }
      setActionMessage(`Replay attempt ${attempt.attemptId} ${attempt.status}.`);
      setDetail({ title: `Replay Attempt ${attempt.attemptId}`, route: `/v1/evaluation/replay-attempts/${attempt.attemptId}`, tenantId, generation, payload: attempt });
      setActiveActionId("");
      await refreshShell({ soft: true, tenantId });
    } catch (caught) {
      if (!isCurrentTenantWork(generation, tenantId)) {
        return;
      }
      if (isTenantDenied(caught)) {
        markActiveTenantDenied("Tenant access was denied. Choose another allowed tenant before continuing.");
        return;
      }
      setError(errorMessage(caught));
    } finally {
      if (activeTenantRef.current === tenantId) {
        setActiveActionId("");
      }
    }
  }

  async function handleCompareLatest(candidate: ReplayCandidateResource) {
    const attemptId = candidate.latestAttemptId || shell.replayAttempts.find((attempt) => attempt.candidateId === candidate.candidateId)?.attemptId;
    if (!attemptId) {
      setError("No replay attempt is available to compare for this candidate.");
      return;
    }
    const scoped = currentTenantOptions();
    if (!scoped) {
      return;
    }
    const tenantId = scoped.tenantId!;
    const generation = generationRef.current;
    setActiveActionId(`compare-${candidate.candidateId}`);
    setError("");
    try {
      const comparison = await buildClient(tenantId).createReplayComparison(attemptId, {}, scoped);
      if (generation !== generationRef.current || activeTenantRef.current !== tenantId) {
        return;
      }
      setActionMessage(`Comparison ${comparison.comparisonId} ${comparison.terminalStatus}.`);
      setDetail({ title: `Comparison ${comparison.comparisonId}`, route: `/v1/evaluation/comparisons/${comparison.comparisonId}`, tenantId, generation, payload: comparison });
      setActiveActionId("");
      await refreshShell({ soft: true, tenantId });
    } catch (caught) {
      if (!isCurrentTenantWork(generation, tenantId)) {
        return;
      }
      if (isTenantDenied(caught)) {
        markActiveTenantDenied("Tenant access was denied. Choose another allowed tenant before continuing.");
        return;
      }
      setError(errorMessage(caught));
    } finally {
      if (activeTenantRef.current === tenantId) {
        setActiveActionId("");
      }
    }
  }

  async function handleSuppressCandidate(candidate: EvaluationDiscoveredCandidateResource) {
    if (!activeTenantId) {
      setError("Tenant context is required to suppress a discovered candidate.");
      return;
    }
    const tenantId = activeTenantId;
    const generation = generationRef.current;
    setActiveActionId(`suppress-${candidate.discoveredCandidateId}`);
    setError("");
    try {
      const client = buildClient(tenantId);
      await client.createEvaluationSuppression({
        targetKind: "discovered_candidate",
        targetId: candidate.discoveredCandidateId,
        reasonCode: "operator_hidden",
        reason: "Suppressed from evaluation product review."
      }, { tenantId });
      if (!isCurrentTenantWork(generation, tenantId)) {
        return;
      }
      setActionMessage(`Suppressed ${candidate.discoveredCandidateId}.`);
      await refreshShell({ soft: true, tenantId });
    } catch (err) {
      if (isCurrentTenantWork(generation, tenantId)) {
        setError(err instanceof Error ? err.message : String(err));
      }
    } finally {
      if (isCurrentTenantWork(generation, tenantId)) {
        setActiveActionId("");
      }
    }
  }

  async function handleCreateProductFixture(candidate: EvaluationDiscoveredCandidateResource) {
    if (!activeTenantId) {
      setError("Tenant context is required to create a product fixture.");
      return;
    }
    const tenantId = activeTenantId;
    const generation = generationRef.current;
    setActiveActionId(`fixture-${candidate.discoveredCandidateId}`);
    setError("");
    try {
      const response = await buildClient(tenantId).materializeProductFixture(candidate.discoveredCandidateId, {
        fixtureId: `product_fixture_${candidate.discoveredCandidateId}`,
        displayName: `${candidate.sourceKind}:${candidate.sourceId}`,
        domainClass: "schedule",
        fixturePayload: {
          sourceKind: candidate.sourceKind,
          sourceId: candidate.sourceId
        },
        changeSummary: "Created from candidate discovery review."
      }, { tenantId });
      if (!isCurrentTenantWork(generation, tenantId)) {
        return;
      }
      setActionMessage(`Created product fixture ${response.fixture.fixtureId}.`);
      await refreshShell({ soft: true, tenantId });
    } catch (err) {
      if (isCurrentTenantWork(generation, tenantId)) {
        setError(err instanceof Error ? err.message : String(err));
      }
    } finally {
      if (isCurrentTenantWork(generation, tenantId)) {
        setActiveActionId("");
      }
    }
  }

  async function handleReviewProductFixture(fixture: ProductFixtureResource) {
    if (!activeTenantId) {
      setError("Tenant context is required to review a product fixture.");
      return;
    }
    const tenantId = activeTenantId;
    const generation = generationRef.current;
    setActiveActionId(`review-${fixture.fixtureId}`);
    setError("");
    try {
      await buildClient(tenantId).reviewProductFixture(fixture.fixtureId, {
        revisionId: fixture.currentRevisionId,
        decision: "approved",
        reason: "Approved from operator shell."
      }, { tenantId });
      if (!isCurrentTenantWork(generation, tenantId)) {
        return;
      }
      setActionMessage(`Approved product fixture ${fixture.fixtureId}.`);
      await refreshShell({ soft: true, tenantId });
    } catch (err) {
      if (isCurrentTenantWork(generation, tenantId)) {
        setError(err instanceof Error ? err.message : String(err));
      }
    } finally {
      if (isCurrentTenantWork(generation, tenantId)) {
        setActiveActionId("");
      }
    }
  }

  async function handleReviseProductFixture(fixture: ProductFixtureResource) {
    if (!activeTenantId) {
      setError("Tenant context is required to revise a product fixture.");
      return;
    }
    const tenantId = activeTenantId;
    const generation = generationRef.current;
    setActiveActionId(`revise-${fixture.fixtureId}`);
    setError("");
    try {
      const response = await buildClient(tenantId).createProductFixtureRevision(fixture.fixtureId, {
        fixturePayload: {
          fixtureId: fixture.fixtureId,
          sourceKind: fixture.sourceKind,
          previousRevisionId: fixture.currentRevisionId
        },
        contentSummary: fixture.displayName,
        changeSummary: "Revised from operator shell."
      }, { tenantId });
      if (!isCurrentTenantWork(generation, tenantId)) {
        return;
      }
      setActionMessage(`Created product fixture revision ${response.revision?.revisionId ?? fixture.fixtureId}.`);
      await refreshShell({ soft: true, tenantId });
    } catch (err) {
      if (isCurrentTenantWork(generation, tenantId)) {
        setError(err instanceof Error ? err.message : String(err));
      }
    } finally {
      if (isCurrentTenantWork(generation, tenantId)) {
        setActiveActionId("");
      }
    }
  }

  async function handleSuppressProductFixture(fixture: ProductFixtureResource) {
    if (!activeTenantId) {
      setError("Tenant context is required to suppress a product fixture.");
      return;
    }
    const tenantId = activeTenantId;
    const generation = generationRef.current;
    setActiveActionId(`fixture-suppress-${fixture.fixtureId}`);
    setError("");
    try {
      await buildClient(tenantId).suppressProductFixture(fixture.fixtureId, {
        reasonCode: "operator_hidden",
        reason: "Suppressed from operator shell."
      }, { tenantId });
      if (!isCurrentTenantWork(generation, tenantId)) {
        return;
      }
      setActionMessage(`Suppressed product fixture ${fixture.fixtureId}.`);
      await refreshShell({ soft: true, tenantId });
    } catch (err) {
      if (isCurrentTenantWork(generation, tenantId)) {
        setError(err instanceof Error ? err.message : String(err));
      }
    } finally {
      if (isCurrentTenantWork(generation, tenantId)) {
        setActiveActionId("");
      }
    }
  }

  async function handleCreateCampaign() {
    const scoped = currentTenantOptions();
    if (!scoped) {
      return;
    }
    const source = shell.productFixtures.find((fixture) => fixture.reviewState === "approved" && fixture.suppressionState !== "suppressed" && fixture.retentionState === "active") ?? shell.discoveredCandidates.find((candidate) => candidate.suppressionState !== "suppressed" && candidate.retentionState === "active");
    if (!source) {
      setError("Create or approve a selectable fixture or candidate before starting a campaign.");
      return;
    }
    const tenantId = scoped.tenantId!;
    const generation = generationRef.current;
    setActiveActionId("campaign-create");
    setError("");
    try {
      const sourceType = "fixtureId" in source ? "product_fixture" : "discovered_candidate";
      const sourceId = "fixtureId" in source ? source.fixtureId : source.discoveredCandidateId;
      const campaign = await buildClient(tenantId).createEvaluationCampaign({
        campaignId: `campaign_${Date.now()}`,
        displayName: `Evaluation Campaign ${shell.campaigns.length + 1}`,
        scopeSummary: "Operator shell campaign",
        sourceSelections: [{ sourceType, sourceId, selectionReason: "Selected from operator shell." }],
        startImmediately: true
      }, scoped);
      if (!isCurrentTenantWork(generation, tenantId)) {
        return;
      }
      setActionMessage(`Created campaign ${campaign.campaignId}.`);
      await refreshShell({ soft: true, tenantId });
    } catch (err) {
      if (isCurrentTenantWork(generation, tenantId)) {
        setError(err instanceof Error ? err.message : String(err));
      }
    } finally {
      if (isCurrentTenantWork(generation, tenantId)) {
        setActiveActionId("");
      }
    }
  }

  async function handleStartCampaign(campaign: EvaluationCampaignResource) {
    const scoped = currentTenantOptions();
    if (!scoped) {
      return;
    }
    const tenantId = scoped.tenantId!;
    const generation = generationRef.current;
    setActiveActionId(`campaign-start-${campaign.campaignId}`);
    setError("");
    try {
      const updated = await buildClient(tenantId).startEvaluationCampaign(campaign.campaignId, scoped);
      if (!isCurrentTenantWork(generation, tenantId)) {
        return;
      }
      setActionMessage(`Started campaign ${updated.campaignId}.`);
      await refreshShell({ soft: true, tenantId });
    } catch (err) {
      if (isCurrentTenantWork(generation, tenantId)) {
        setError(err instanceof Error ? err.message : String(err));
      }
    } finally {
      if (isCurrentTenantWork(generation, tenantId)) {
        setActiveActionId("");
      }
    }
  }

  async function handlePublishCampaign(campaign: EvaluationCampaignResource) {
    const scoped = currentTenantOptions();
    if (!scoped) {
      return;
    }
    const tenantId = scoped.tenantId!;
    const generation = generationRef.current;
    setActiveActionId(`campaign-publish-${campaign.campaignId}`);
    setError("");
    try {
      const updated = await buildClient(tenantId).publishEvaluationCampaignResults(campaign.campaignId, scoped);
      if (!isCurrentTenantWork(generation, tenantId)) {
        return;
      }
      setActionMessage(`Published campaign ${updated.campaignId}.`);
      await refreshShell({ soft: true, tenantId });
    } catch (err) {
      if (isCurrentTenantWork(generation, tenantId)) {
        setError(err instanceof Error ? err.message : String(err));
      }
    } finally {
      if (isCurrentTenantWork(generation, tenantId)) {
        setActiveActionId("");
      }
    }
  }

  async function handleStartLiveValidation() {
    const scoped = currentTenantOptions();
    if (!scoped) {
      return;
    }
    const candidateId = liveValidationCandidateId.trim() || shell.replayCandidates[0]?.candidateId || "";
    if (!candidateId) {
      setError("Select a replay candidate before starting live validation.");
      return;
    }
    const tenantId = scoped.tenantId!;
    const generation = generationRef.current;
    const validationId = `lv_${Date.now()}`;
    const includedToolClasses = splitCSV(liveValidationToolClasses);
    const candidate = shell.replayCandidates.find((item) => item.candidateId === candidateId);
    const candidateToolClasses = candidate?.toolClasses?.length ? candidate.toolClasses : includedToolClasses;
    setActiveActionId("live-validation-start");
    setError("");
    try {
      const response = await buildClient(tenantId).startLiveValidation({
        validationId,
        candidateId,
        candidateToolClasses,
        requestedScope: {
          scopeId: `${validationId}_scope`,
          validationId,
          includedToolClasses,
          approvalMode: liveValidationApprovalMode,
          declaredBy: authMe?.principal.principalId || "operator-shell",
          declaredAt: new Date().toISOString()
        }
      }, scoped);
      if (generation !== generationRef.current || activeTenantRef.current !== tenantId) {
        return;
      }
      const denialSuffix = response.denials?.length ? ` with ${response.denials.length} denial(s)` : "";
      setActionMessage(`Live validation ${response.attempt.validationId} ${response.attempt.status}${denialSuffix}.`);
      setDetail({
        title: `Live Validation ${response.attempt.validationId}`,
        route: `/v1/live-validations/${response.attempt.validationId}`,
        tenantId,
        generation,
        payload: response
      });
      setActiveActionId("");
      await refreshShell({ soft: true, tenantId });
    } catch (caught) {
      if (!isCurrentTenantWork(generation, tenantId)) {
        return;
      }
      if (isTenantDenied(caught)) {
        markActiveTenantDenied("Tenant access was denied. Choose another allowed tenant before continuing.");
        return;
      }
      setError(errorMessage(caught));
    } finally {
      if (activeTenantRef.current === tenantId) {
        setActiveActionId("");
      }
    }
  }

  async function handleEnableTenantKillSwitch() {
    const scoped = currentTenantOptions();
    if (!scoped) {
      return;
    }
    const tenantId = scoped.tenantId!;
    const generation = generationRef.current;
    setActiveActionId("live-validation-kill-switch");
    setError("");
    try {
      const item = await buildClient(tenantId).updateLiveValidationKillSwitch({
        scope: "tenant",
        enabled: true,
        reason: "Enabled from operator shell."
      }, scoped);
      if (!isCurrentTenantWork(generation, tenantId)) {
        return;
      }
      setActionMessage(`Live validation kill switch ${item.enabled ? "enabled" : "disabled"}.`);
      await refreshShell({ soft: true, tenantId });
    } catch (caught) {
      if (!isCurrentTenantWork(generation, tenantId)) {
        return;
      }
      setError(errorMessage(caught));
    } finally {
      if (activeTenantRef.current === tenantId) {
        setActiveActionId("");
      }
    }
  }

  async function handleMembershipRoleChange(membership: MembershipResource, role: TenantRole) {
    const scoped = currentTenantOptions();
    if (!scoped) {
      return;
    }
    const tenantId = scoped.tenantId!;
    const generation = generationRef.current;
    setMemberships((current) => ({ ...current, pendingMembershipId: membership.membershipId, error: "" }));
    setError("");
    try {
      const response = await buildClient(tenantId).updateMembershipRole(tenantId, membership.membershipId, { role }, scoped);
      if (!isCurrentTenantWork(generation, tenantId)) {
        return;
      }
      setMemberships((current) => {
        const nextMembers = current.members.map((item) => item.membershipId === response.membership.membershipId ? response.membership : item);
        return {
          status: membershipStatusFor(nextMembers),
          members: nextMembers,
          error: "",
          pendingMembershipId: ""
        };
      });
      setActionMessage(`Updated ${membership.principalId} to ${response.membership.role}.`);
    } catch (caught) {
      if (!isCurrentTenantWork(generation, tenantId)) {
        return;
      }
      if (isTenantDenied(caught)) {
        markActiveTenantDenied("Tenant access was denied. Choose another allowed tenant before continuing.");
        return;
      }
      const message = errorMessage(caught);
      setMemberships((current) => ({ ...current, status: "error", error: message, pendingMembershipId: "" }));
      setError(message);
    }
  }

  async function handleStartSetup(target: SetupTargetResource) {
    const scoped = currentTenantOptions();
    if (!scoped) {
      return;
    }
    const tenantId = scoped.tenantId!;
    const generation = generationRef.current;
    const actionId = `setup-start-${target.targetId}`;
    setActiveActionId(actionId);
    setError("");
    try {
      const response = await buildClient(tenantId).startSetup({ targetId: target.targetId, setupStyle: target.setupStyle, source: "operator_shell" }, scoped);
      if (!isCurrentTenantWork(generation, tenantId)) {
        return;
      }
      setActionMessage(`Setup started for ${target.displayName}.`);
      setDetail({ title: `Setup ${target.displayName}`, route: `/v1/setup/sessions/${response.session.setupSessionId}`, tenantId, generation, payload: response });
      await refreshShell({ soft: true, tenantId });
    } catch (caught) {
      if (isCurrentTenantWork(generation, tenantId)) {
        setError(errorMessage(caught));
      }
    } finally {
      if (activeTenantRef.current === tenantId) {
        setActiveActionId("");
      }
    }
  }

  async function handleSubmitSetupSecret(target: SetupTargetResource, session: SetupSessionResource | null) {
    const scoped = currentTenantOptions();
    if (!scoped) {
      return;
    }
    const tenantId = scoped.tenantId!;
    const generation = generationRef.current;
    const actionId = `setup-secret-${target.targetId}`;
    setActiveActionId(actionId);
    setError("");
    try {
      const client = buildClient(tenantId);
      const activeSession = session ?? (await client.startSetup({ targetId: target.targetId, setupStyle: target.setupStyle, source: "operator_shell" }, scoped)).session;
      const response = await client.submitSetupSecret(activeSession.setupSessionId, {
        secretRef: setupSecretRef.trim() || "provider/openai-compatible",
        displayName: target.displayName,
        value: setupSecretValue
      }, scoped);
      if (!isCurrentTenantWork(generation, tenantId)) {
        return;
      }
      setSetupSecretValue("");
      setActionMessage(`Submitted redacted credential metadata for ${target.displayName}.`);
      setDetail({ title: `Setup ${target.displayName}`, route: `/v1/setup/sessions/${response.session.setupSessionId}`, tenantId, generation, payload: response });
      await refreshShell({ soft: true, tenantId });
    } catch (caught) {
      if (isCurrentTenantWork(generation, tenantId)) {
        setError(errorMessage(caught));
      }
    } finally {
      if (activeTenantRef.current === tenantId) {
        setActiveActionId("");
      }
    }
  }

  async function handleStartSetupOAuth(target: SetupTargetResource, session: SetupSessionResource | null) {
    const scoped = currentTenantOptions();
    if (!scoped) {
      return;
    }
    const tenantId = scoped.tenantId!;
    const generation = generationRef.current;
    const actionId = `setup-oauth-start-${target.targetId}`;
    setActiveActionId(actionId);
    setError("");
    try {
      const client = buildClient(tenantId);
      const activeSession = session ?? (await client.startSetup({ targetId: target.targetId, setupStyle: target.setupStyle, source: "operator_shell" }, scoped)).session;
      const response = await client.startSetupOAuth(activeSession.setupSessionId, { redirectRoute: setupOAuthRedirectRoute(target) }, scoped);
      if (!isCurrentTenantWork(generation, tenantId)) {
        return;
      }
      setActionMessage(`OAuth fixture started for ${target.displayName}.`);
      setDetail({ title: `OAuth ${target.displayName}`, route: `/v1/setup/sessions/${response.session.setupSessionId}`, tenantId, generation, payload: response });
      await refreshShell({ soft: true, tenantId });
    } catch (caught) {
      if (isCurrentTenantWork(generation, tenantId)) {
        setError(errorMessage(caught));
      }
    } finally {
      if (activeTenantRef.current === tenantId) {
        setActiveActionId("");
      }
    }
  }

  async function handleCompleteSetupOAuth(target: SetupTargetResource, session: SetupSessionResource | null) {
    const scoped = currentTenantOptions();
    if (!scoped || !session) {
      return;
    }
    const tenantId = scoped.tenantId!;
    const generation = generationRef.current;
    const actionId = `setup-oauth-complete-${target.targetId}`;
    setActiveActionId(actionId);
    setError("");
    try {
      const response = await buildClient(tenantId).completeSetupOAuth(session.setupSessionId, {
        state: session.oauthStateRef || "oauth_state_ref_1",
        result: "completed",
        accountLabel: activeTenant?.displayName
      }, scoped);
      if (!isCurrentTenantWork(generation, tenantId)) {
        return;
      }
      setActionMessage(`OAuth fixture completed for ${target.displayName}.`);
      setDetail({ title: `OAuth ${target.displayName}`, route: `/v1/setup/sessions/${response.session.setupSessionId}`, tenantId, generation, payload: response });
      await refreshShell({ soft: true, tenantId });
    } catch (caught) {
      if (isCurrentTenantWork(generation, tenantId)) {
        setError(errorMessage(caught));
      }
    } finally {
      if (activeTenantRef.current === tenantId) {
        setActiveActionId("");
      }
    }
  }

  async function handleSetupTransition(session: SetupSessionResource, action: "retry" | "replace" | "cancel" | "disable") {
    const scoped = currentTenantOptions();
    if (!scoped) {
      return;
    }
    const tenantId = scoped.tenantId!;
    const generation = generationRef.current;
    const actionId = `setup-${action}-${session.setupSessionId}`;
    setActiveActionId(actionId);
    setError("");
    try {
      const client = buildClient(tenantId);
      const response = action === "retry"
        ? await client.retrySetup(session.setupSessionId, scoped)
        : action === "replace"
          ? await client.replaceSetup(session.setupSessionId, scoped)
          : action === "cancel"
            ? await client.cancelSetup(session.setupSessionId, scoped)
            : await client.disableSetup(session.setupSessionId, { disabledReason: "operator_shell" }, scoped);
      if (!isCurrentTenantWork(generation, tenantId)) {
        return;
      }
      setActionMessage(`Setup ${action} moved ${session.targetId} to ${response.session.state}.`);
      setDetail({ title: `Setup ${session.targetId}`, route: `/v1/setup/sessions/${response.session.setupSessionId}`, tenantId, generation, payload: response });
      await refreshShell({ soft: true, tenantId });
    } catch (caught) {
      if (isCurrentTenantWork(generation, tenantId)) {
        setError(errorMessage(caught));
      }
    } finally {
      if (activeTenantRef.current === tenantId) {
        setActiveActionId("");
      }
    }
  }

  async function handleSetupDiagnostics(session: SetupSessionResource) {
    const scoped = currentTenantOptions();
    if (!scoped) {
      return;
    }
    const tenantId = scoped.tenantId!;
    const generation = generationRef.current;
    setDetailLoading(true);
    setError("");
    try {
      const payload = await buildClient(tenantId).getSetupDiagnostics(session.setupSessionId, scoped);
      if (!isCurrentTenantWork(generation, tenantId)) {
        return;
      }
      setDetail({ title: `Setup Diagnostics ${session.targetId}`, route: `/v1/setup/sessions/${session.setupSessionId}/diagnostics`, tenantId, generation, payload });
    } catch (caught) {
      if (isCurrentTenantWork(generation, tenantId)) {
        setError(errorMessage(caught));
      }
    } finally {
      if (activeTenantRef.current === tenantId) {
        setDetailLoading(false);
      }
    }
  }

  async function handleBillingDenialDetail(denial: BillingDenialResource) {
    const scoped = currentTenantOptions();
    if (!scoped) {
      return;
    }
    const tenantId = scoped.tenantId!;
    const generation = generationRef.current;
    setDetailLoading(true);
    setError("");
    try {
      const payload = await buildClient(tenantId).getBillingDenialDetail(denial.denialId, scoped);
      if (!isCurrentTenantWork(generation, tenantId)) {
        return;
      }
      setDetail({ title: `Billing Denial ${denial.denialId}`, route: `/v1/billing/denials/${denial.denialId}`, tenantId, generation, payload });
    } catch (caught) {
      if (isCurrentTenantWork(generation, tenantId)) {
        setError(errorMessage(caught));
      }
    } finally {
      if (activeTenantRef.current === tenantId) {
        setDetailLoading(false);
      }
    }
  }

  async function handleBillingEvidenceExport(denial: BillingDenialResource) {
    const scoped = currentTenantOptions();
    if (!scoped) {
      return;
    }
    if (!canExportBillingEvidence) {
      setError("billing.evidence_export is required to export denial evidence.");
      return;
    }
    const tenantId = scoped.tenantId!;
    const generation = generationRef.current;
    setActiveActionId(`billing-export-${denial.denialId}`);
    setError("");
    try {
      const payload = await buildClient(tenantId).exportBillingDenialEvidence(denial.denialId, scoped);
      if (!isCurrentTenantWork(generation, tenantId)) {
        return;
      }
      setActionMessage(`Exported redacted billing evidence ${payload.exportId}.`);
      setDetail({ title: `Billing Evidence ${denial.denialId}`, route: `/v1/billing/denials/${denial.denialId}/evidence-export`, tenantId, generation, payload });
    } catch (caught) {
      if (isCurrentTenantWork(generation, tenantId)) {
        setError(errorMessage(caught));
      }
    } finally {
      if (activeTenantRef.current === tenantId) {
        setActiveActionId("");
      }
    }
  }

  function currentTenantOptions(): TenantRequestOptions | null {
    if (!canUseTenantActions || !activeTenantId) {
      setError("Resolve an active tenant before performing tenant-scoped work.");
      return null;
    }
    return { tenantId: activeTenantId };
  }

  async function refreshChannelConnectorDetail(connectorId: string, tenantId: string, scoped: TenantRequestOptions) {
    const client = buildClient(tenantId);
    const [channelConnectors, selectedChannelConnector, channelSupportEvidence] = await Promise.all([
      client.listChannelConnectors({ limit: 20 }, scoped).catch(() => shell.channelConnectors),
      client.getChannelConnector(connectorId, scoped),
      client.getChannelConnectorSupportEvidence(connectorId, scoped).catch(() => null)
    ]);
    setShell((previous) => ({
      ...previous,
      channelConnectors,
      selectedChannelConnector,
      channelSupportEvidence
    }));
  }

  async function handleChannelConnectorSelect(connectorId: string) {
    const scoped = currentTenantOptions();
    if (!scoped) {
      return;
    }
    const tenantId = scoped.tenantId ?? activeTenantId;
    setError("");
    setActionMessage("");
    setActiveActionId(`channel-select-${connectorId}`);
    try {
      await refreshChannelConnectorDetail(connectorId, tenantId, scoped);
    } catch (caught) {
      setError(errorMessage(caught));
    } finally {
      setActiveActionId("");
    }
  }

  async function runChannelConnectorAction(connectorId: string, actionKind: string, action: (client: ReturnType<typeof buildClient>, scoped: TenantRequestOptions) => Promise<unknown>) {
    const scoped = currentTenantOptions();
    if (!scoped) {
      return;
    }
    const tenantId = scoped.tenantId ?? activeTenantId;
    setError("");
    setActionMessage("");
    setActiveActionId(`channel-${actionKind}-${connectorId}`);
    try {
      const client = buildClient(tenantId);
      await action(client, scoped);
      await refreshChannelConnectorDetail(connectorId, tenantId, scoped);
      setActionMessage(`Channel connector ${connectorId} ${actionKind} completed.`);
    } catch (caught) {
      setError(errorMessage(caught));
    } finally {
      setActiveActionId("");
    }
  }

  function handleChannelConnectorDisable(connectorId: string) {
    void runChannelConnectorAction(connectorId, "disable", (client, scoped) => client.disableChannelConnector(connectorId, {}, scoped));
  }

  function handleChannelConnectorReEnable(connectorId: string) {
    void runChannelConnectorAction(connectorId, "re-enable", (client, scoped) => client.reEnableChannelConnector(connectorId, {}, scoped));
  }

  function handleChannelConnectorRepair(connectorId: string) {
    void runChannelConnectorAction(connectorId, "repair", (client, scoped) => client.startChannelConnectorRepair(connectorId, { actionKind: "repair" }, scoped));
  }

  function handleChannelRoutePolicyUpdate(connectorId: string, input: ChannelManagementActionInput) {
    void runChannelConnectorAction(connectorId, "route-policy", (client, scoped) => client.updateChannelRoutePolicy(connectorId, input, scoped));
  }

  async function refreshProfiles(selectedProfileId?: string) {
    const scoped = currentTenantOptions();
    if (!scoped) {
      return;
    }
    const tenantId = scoped.tenantId ?? activeTenantId;
    const client = buildClient(tenantId);
    const profiles = await client.listAgentProfiles({ limit: 20 }, scoped);
    const profileId = selectedProfileId || profiles.items[0]?.profileId || "";
    const selectedProfile = profileId ? await client.getAgentProfile(profileId, scoped).catch(() => null) : null;
    setShell((previous) => ({ ...previous, profiles, selectedProfile }));
  }

  function handleProfileSelect(profileId: string) {
    const scoped = currentTenantOptions();
    if (!scoped) {
      return;
    }
    const tenantId = scoped.tenantId ?? activeTenantId;
    const client = buildClient(tenantId);
    void client.getAgentProfile(profileId, scoped)
      .then((selectedProfile) => setShell((previous) => ({ ...previous, selectedProfile })))
      .catch((caught) => setError(errorMessage(caught)));
  }

  function runProfileAction(profileId: string, actionName: string, action: (client: ReturnType<typeof buildClient>, scoped: TenantRequestOptions) => Promise<unknown>) {
    const scoped = currentTenantOptions();
    if (!scoped) {
      return;
    }
    const tenantId = scoped.tenantId ?? activeTenantId;
    setError("");
    setActionMessage("");
    setActiveActionId(`profile-${actionName}-${profileId}`);
    const client = buildClient(tenantId);
    void action(client, scoped)
      .then(() => refreshProfiles(profileId))
      .then(() => setActionMessage(`Profile ${profileId} ${actionName} completed.`))
      .catch((caught) => setError(errorMessage(caught)))
      .finally(() => setActiveActionId(""));
  }

  function handleProfileActivate(profileId: string) {
    runProfileAction(profileId, "activate", (client, scoped) => client.activateAgentProfile(profileId, {}, scoped));
  }

  function handleProfileArchive(profileId: string) {
    runProfileAction(profileId, "archive", (client, scoped) => client.archiveAgentProfile(profileId, { reasonCode: "web_profile_archive" }, scoped));
  }

  function handleProfileDisable(profileId: string) {
    runProfileAction(profileId, "disable", (client, scoped) => client.disableAgentProfile(profileId, { reasonCode: "web_profile_disable" }, scoped));
  }

  function runProfileMutation(actionName: string, action: (client: ReturnType<typeof buildClient>, scoped: TenantRequestOptions) => Promise<{ profile?: { profileId: string } }>) {
    const scoped = currentTenantOptions();
    if (!scoped) {
      return;
    }
    const tenantId = scoped.tenantId ?? activeTenantId;
    setError("");
    setActionMessage("");
    setActiveActionId(`profile-${actionName}`);
    const client = buildClient(tenantId);
    void action(client, scoped)
      .then((result) => refreshProfiles(result.profile?.profileId))
      .then(() => setActionMessage(`Profile ${actionName} completed.`))
      .catch((caught) => setError(errorMessage(caught)))
      .finally(() => setActiveActionId(""));
  }

  function handleProfileSave(input: AgentProfileMutationInput) {
    const profileId = shell.selectedProfile?.profile.profileId || shell.profiles?.items[0]?.profileId || "";
    if (!profileId) {
      handleProfileCreate(input);
      return;
    }
    runProfileMutation("update", (client, scoped) => client.updateAgentProfile(profileId, { ...input, reasonCode: "web_profile_update" }, scoped));
  }

  function handleProfileCreate(input: AgentProfileMutationInput) {
    runProfileMutation("create", (client, scoped) => client.createAgentProfile({ ...input, reasonCode: "web_profile_create" }, scoped));
  }

  function handleProfileRollback(profileVersionId: string) {
    const profileId = shell.selectedProfile?.profile.profileId || "";
    if (!profileId) {
      return;
    }
    runProfileMutation("rollback", (client, scoped) => client.rollbackAgentProfile(profileId, { sourceProfileVersionId: profileVersionId, reasonCode: "web_profile_rollback" }, scoped));
  }

  const onboarding = shell.onboarding;
  const activation = shell.activation;
  const activationDiagnosticItems = shell.activationDiagnostics?.items ?? [];
  const setupDiagnostics = shell.setupDiagnostics;
  const activationStale = activeTenantStatus === "stale" && Boolean(activation);
  const activityItems = shell.activity?.items ?? [];
  const diagnosticItems = shell.diagnostics?.items ?? [];
  const selectedLiveValidationCandidateId = liveValidationCandidateId || shell.replayCandidates[0]?.candidateId || "";
  const latestLiveValidation = shell.liveValidations[0] ?? null;
  const unsupportedMatrixRows = shell.supportMatrix.filter((row) => row.safetyClass === "unsupported");
  const billingDashboard = shell.billingDashboard;
  const latestBillingDenials = shell.billingDenials.slice(0, 4);

  return (
    <main className="operator-shell">
      <section className="hero-panel">
        <div className="product-brand" aria-label="Kura">
          <img src="/kura-mark.svg" alt="" />
          <span>Kura</span>
        </div>
        <div>
          <p className="eyebrow">Operator Shell</p>
          <h1>Single control surface for onboarding, approvals, activity, and diagnostics.</h1>
        </div>
        <p className="hero-copy">
          This web shell projects daemon-owned readiness, approvals, workflows, deliveries, and failures without dropping the
          operator into raw route tabs.
        </p>
      </section>

      <section className="config-panel">
        {GATEWAY_MODE ? (
          <p>
            <a href="/gw/settings">Model settings</a>
          </p>
        ) : (
          <>
            <label>
              <span>Daemon URL</span>
              <input value={daemonURL} onChange={(event) => setDaemonURL(event.target.value)} placeholder={DEFAULT_DAEMON_URL} />
            </label>
            <label>
              <span>Access Token</span>
              <input value={accessToken} onChange={(event) => setAccessToken(event.target.value)} placeholder="Bearer token" type="password" />
            </label>
          </>
        )}
        <label>
          <span>Active Tenant</span>
          <select
            aria-label="Active Tenant"
            disabled={!allowedTenants.length || activeTenantStatus === "resolving" || activeTenantStatus === "stale"}
            value={activeTenantId}
            onChange={(event) => {
              void refreshShell({ tenantId: event.target.value, explicitSelection: true });
            }}
          >
            <option value="">Resolve tenant</option>
            {allowedTenants.map((tenant) => (
              <option key={tenant.tenantId} value={tenant.tenantId}>{tenant.displayName}</option>
            ))}
          </select>
        </label>
        <div className="config-actions">
          <button className="secondary" disabled={status === "loading"} type="button" onClick={() => {
            setError("");
            setActionMessage("");
            setDetail(null);
            void refreshShell();
          }}>
            {status === "loading" ? "Loading..." : status === "ready" ? "Refresh Shell" : "Load Shell"}
          </button>
        </div>
      </section>

      <section className="tenant-strip" aria-label="tenant status">
        <div>
          <span className="banner-label">Active Tenant</span>
          <strong>{activeTenant ? `${activeTenant.displayName} (${activeTenant.tenantKind})` : "unresolved"}</strong>
        </div>
        <span className={`status-chip status-${activeTenantStatus}`}>{activeTenantStatus}</span>
        <p>{tenantMessage}</p>
      </section>

      <section className="banner-strip" aria-label="shell status">
        <div className="banner-card">
          <span className="banner-label">Environment</span>
          <strong>{onboarding?.environmentScope ?? "not loaded"}</strong>
        </div>
        <div className="banner-card">
          <span className="banner-label">Onboarding</span>
          <strong>{onboarding?.status ?? status}</strong>
        </div>
        <div className="banner-card">
          <span className="banner-label">Activation</span>
          <strong>{activation?.status ?? "not loaded"}</strong>
        </div>
        <div className="banner-card">
          <span className="banner-label">Event Stream</span>
          <strong>{eventStatus}</strong>
        </div>
        <div className="banner-card">
          <span className="banner-label">Last Event</span>
          <strong>{lastEvent || "waiting"}</strong>
        </div>
      </section>

      {error ? <div className="error-box" role="alert">{error}</div> : null}
      {actionMessage ? <div className="message-box">{actionMessage}</div> : null}

      <ChannelManagementView
        connectors={shell.channelConnectors}
        selected={shell.selectedChannelConnector}
        supportEvidence={shell.channelSupportEvidence}
        loading={status === "loading"}
        error={activeTenantStatus === "denied" ? "Channel management unavailable until tenant access is restored." : ""}
        onSelectConnector={handleChannelConnectorSelect}
        onDisableConnector={handleChannelConnectorDisable}
        onReEnableConnector={handleChannelConnectorReEnable}
        onStartRepair={handleChannelConnectorRepair}
        onUpdateRoutePolicy={handleChannelRoutePolicyUpdate}
      />

      <ThreadLifecycleView
        threads={shell.threads}
        loading={status === "loading"}
        denied={activeTenantStatus === "denied"}
        stalePermission={activeTenantStatus === "stale"}
        error={activeTenantStatus === "denied" ? "Thread inspection unavailable until tenant access is restored." : ""}
        onRefresh={() => {
          void refreshShell({ tenantId: activeTenantId });
        }}
      />

      <MemoryOverviewPanel
        overview={shell.memoryOverview}
        loading={status === "loading"}
        denied={activeTenantStatus === "denied"}
        error={activeTenantStatus === "denied" ? "Memory inventory unavailable until tenant access is restored." : ""}
        onRefresh={() => {
          void refreshShell({ soft: true, tenantId: activeTenantId });
        }}
        onRebuildIndexes={() => {
          void (async () => {
            try {
              const result = await buildClient(activeTenantId).rebuildMemoryIndexes(undefined, tenantOptions);
              setActionMessage(`Retrieval index rebuilt: ${result.clearedEmbeddings} vector(s) cleared; they are recomputed on the next retrieval.`);
              await refreshShell({ soft: true, tenantId: activeTenantId });
            } catch (err) {
              setActionMessage(`Rebuild failed: ${err instanceof Error ? err.message : String(err)}`);
            }
          })();
        }}
      />

      <AgentProfileEditor
        profiles={shell.profiles}
        detail={shell.selectedProfile}
        loading={status === "loading"}
        denied={activeTenantStatus === "denied"}
        error={activeTenantStatus === "denied" ? "Profile inspection unavailable until tenant access is restored." : ""}
        onRefresh={() => {
          void refreshProfiles();
        }}
        onSelectProfile={handleProfileSelect}
        onCreate={handleProfileCreate}
        onSave={handleProfileSave}
        onActivate={handleProfileActivate}
        onArchive={handleProfileArchive}
        onDisable={handleProfileDisable}
      />
      <AgentProfileHistory versions={shell.selectedProfile?.versions ?? []} onRollback={handleProfileRollback} />

      <section className={`dashboard-grid tenant-${activeTenantStatus}`}>
        <section className="panel onboarding-panel">
          <div className="panel-head">
            <div>
              <p className="section-kicker">First Run</p>
              <h2>Onboarding</h2>
            </div>
            {onboarding ? <span className={`status-chip status-${onboarding.status}`}>{onboarding.status}</span> : null}
          </div>

          {onboarding ? (
            <>
              <div className="action-inputs">
                <label>
                  <span>Test Run Goal</span>
                  <input value={runGoal} onChange={(event) => setRunGoal(event.target.value)} />
                </label>
                <label>
                  <span>Test Query</span>
                  <textarea value={testQuery} onChange={(event) => setTestQuery(event.target.value)} rows={4} />
                </label>
                <label>
                  <span>Thread ID</span>
                  <input value={testThreadId} onChange={(event) => setTestThreadId(event.target.value)} />
                </label>
              </div>

              <div className="action-grid">
                {onboarding.firstUsefulActions.map((action) => (
                  <article className="action-card" key={action.actionId}>
                    <div className="action-title-row">
                      <strong>{action.displayName}</strong>
                      {action.recommended ? <span className="tag">Recommended</span> : null}
                    </div>
                    <p>{action.summary || "No action summary."}</p>
                    <button
                      className="primary"
                      disabled={!canUseTenantActions || !action.available || activeActionId === action.actionId}
                      type="button"
                      onClick={() => {
                        void handleFirstUsefulAction(action);
                      }}
                    >
                      {activeActionId === action.actionId ? "Running..." : action.displayName}
                    </button>
                    {action.blockingItemIds?.length ? <small>Blocked by {action.blockingItemIds.join(", ")}</small> : null}
                  </article>
                ))}
              </div>

              <div className="readiness-list">
                {onboarding.readinessItems.map((item) => (
                  <article className="readiness-card" key={item.itemId}>
                    <div className="readiness-head">
                      <strong>{item.displayName}</strong>
                      <span className={`status-chip status-${item.status}`}>{item.status}</span>
                    </div>
                    <p>{item.reason || "No readiness note."}</p>
                    <small>{item.requiredForSelectedAction ? "Required for selected action." : "Optional follow-up."}</small>
                    <div className="inline-actions">
                      {item.detailRoute ? (
                        <button disabled={!canUseTenantActions} type="button" onClick={() => {
                          void inspectRoute(item.detailRoute!, item.displayName);
                        }}>
                          Inspect
                        </button>
                      ) : null}
                    </div>
                  </article>
                ))}
              </div>
            </>
          ) : (
            <div className="empty-state">{activeTenantStatus === "denied" ? "Tenant access is denied. Choose another allowed tenant." : "Load the shell to project readiness and bounded first-use actions."}</div>
          )}
        </section>

        <section className="panel quota-panel">
          <div className="panel-head">
            <div>
              <p className="section-kicker">Hosted Billing</p>
              <h2>Quota Dashboard</h2>
            </div>
            {billingDashboard ? <span className={`status-chip status-${billingDashboard.plan.enforcementMode}`}>{billingDashboard.plan.enforcementMode}</span> : null}
          </div>

          {canViewBilling && billingDashboard ? (
            <div className="quota-dashboard">
              <div className="quota-plan-row">
                <div>
                  <span className="banner-label">Plan</span>
                  <strong>{billingDashboard.plan.basePlanLabel || billingDashboard.plan.planKey}</strong>
                </div>
                <div>
                  <span className="banner-label">Generated</span>
                  <strong>{formatDateTime(billingDashboard.generatedAt)}</strong>
                </div>
              </div>
              {billingDashboard.sections.map((section) => (
                <div className="quota-section" key={section.sectionKey}>
                  <div className="stack-head">
                    <strong>{section.label}</strong>
                    <span className="count-chip">{section.items.length}</span>
                  </div>
                  <div className="quota-item-list">
                    {section.items.map((item) => (
                      <article className="quota-item" key={item.category}>
                        <div className="stack-head">
                          <strong>{formatLabel(item.category)}</strong>
                          <span className={`status-chip status-${item.status}`}>{item.status}</span>
                        </div>
                        <div className="quota-meter" aria-label={`${item.category} usage`}>
                          <span style={{ width: `${quotaPercent(item)}%` }} />
                        </div>
                        <div className="quota-meta">
                          <span>{item.currentPeriod.consumedAmount + item.currentPeriod.reservedAmount} used</span>
                          <span>{item.remainingAmount} remaining</span>
                          <span>{item.limit} limit</span>
                        </div>
                        {item.previousPeriod ? <small>Previous: {item.previousPeriod.consumedAmount} used, {item.previousPeriod.remainingAmount} remaining</small> : null}
                        {item.override ? <small>Override: {item.override.baseLimit} to {item.override.effectiveLimit}{item.override.reason ? `, ${item.override.reason}` : ""}</small> : null}
                        {item.restriction ? <small>Restriction: {item.restriction.visibleReasonCode || item.restriction.status}</small> : null}
                        {item.recoveryActions.length ? <small>Actions: {item.recoveryActions.join(", ")}</small> : null}
                      </article>
                    ))}
                  </div>
                </div>
              ))}
              <div className="quota-denials">
                <div className="stack-head">
                  <strong>Recent Denials</strong>
                  <span className="count-chip">{latestBillingDenials.length}</span>
                </div>
                {latestBillingDenials.length ? latestBillingDenials.map((denial) => (
                  <article className="quota-denial" key={denial.denialId}>
                    <div>
                      <strong>{formatLabel(denial.category || "quota_denial")}</strong>
                      <small>{denial.reasonCode}</small>
                    </div>
                    <div className="inline-actions">
                      <button disabled={!canUseTenantActions || detailLoading} type="button" onClick={() => {
                        void handleBillingDenialDetail(denial);
                      }}>
                        Detail
                      </button>
                      <button disabled={!canUseTenantActions || !canExportBillingEvidence || activeActionId === `billing-export-${denial.denialId}`} type="button" onClick={() => {
                        void handleBillingEvidenceExport(denial);
                      }}>
                        {activeActionId === `billing-export-${denial.denialId}` ? "Exporting..." : "Export"}
                      </button>
                    </div>
                  </article>
                )) : <div className="empty-state">No recent quota denials.</div>}
              </div>
            </div>
          ) : (
            <div className="empty-state">{canViewBilling ? "Quota dashboard is unavailable." : "Billing visibility is not granted for this tenant."}</div>
          )}
        </section>

        <section className={`panel activation-panel${activationStale ? " is-stale" : ""}`}>
          <div className="panel-head">
            <div>
              <p className="section-kicker">Hosted Tenant</p>
              <h2>Activation</h2>
            </div>
            {activation ? <span className={`status-chip status-${activation.status}`}>{activation.status}</span> : null}
          </div>

          {activation ? (
            <div className="activation-summary">
              {activationStale ? <div className="empty-state">Activation state is refreshing for the active tenant.</div> : null}
              <div className="activation-metrics">
                <div>
                  <span className="banner-label">Step</span>
                  <strong>{activation.currentStepId}</strong>
                </div>
                <div>
                  <span className="banner-label">Environment</span>
                  <strong>{activation.environmentScope}</strong>
                </div>
                <div>
                  <span className="banner-label">Quota</span>
                  <strong>
                    {activation.quotaBaseline
                      ? `Quota baseline ${activation.quotaBaseline.planKey}`
                      : "Quota baseline unavailable"}
                  </strong>
                </div>
                <div>
                  <span className="banner-label">Action</span>
                  <strong>{activation.firstAction.displayName || activation.firstAction.actionKind}</strong>
                </div>
              </div>
              <div className="activation-action-row">
                <button
                  className="primary"
                  disabled={!canUseTenantActions || activationStale || !activation.firstAction.available || activeActionId === activation.firstAction.actionId}
                  type="button"
                  onClick={() => {
                    void handleActivationFirstAction();
                  }}
                >
                  {activeActionId === activation.firstAction.actionId ? "Running..." : activation.firstAction.displayName || activation.firstAction.actionKind}
                </button>
                {activation.firstAction.blockingItemIds.length ? <small>Blocked by {activation.firstAction.blockingItemIds.join(", ")}</small> : null}
              </div>
              {activation.quotaBaseline?.quotas.length ? (
                <div className="activation-quota-list">
                  {activation.quotaBaseline.quotas.map((quota, index) => (
                    <div key={`${quota.category ?? "quota"}-${quota.period ?? index}`}>
                      <strong>{quota.category ?? "quota"}</strong>
                      <span>{formatActivationQuota(quota.remaining, quota.unit)} remaining</span>
                    </div>
                  ))}
                </div>
              ) : null}
              {activation.readinessItems.length ? (
                <div className="activation-readiness-list">
                  {activation.readinessItems.map((item) => (
                    <article className="readiness-card" key={item.itemId}>
                      <div className="readiness-head">
                        <strong>{item.displayName || item.itemId}</strong>
                        <span className={`status-chip status-${item.status}`}>{item.status}</span>
                      </div>
                      <p>{item.reasonCode || `${item.remediationOwner} remediation`}</p>
                      <small>{item.retryable ? "Retryable readiness check." : "Required readiness check."}</small>
                    </article>
                  ))}
                </div>
              ) : null}
              {activation.blockingReasonCodes.length ? (
                <p className="muted-line">Blocked by {activation.blockingReasonCodes.join(", ")}</p>
              ) : null}
              {activationDiagnosticItems.length ? (
                <div className="activation-diagnostic-list">
                  {activationDiagnosticItems.map((item) => (
                    <article className="readiness-card" key={`${item.activationId}-${item.reasonCode}`}>
                      <div className="readiness-head">
                        <strong>{item.reasonCode}</strong>
                        <span className={`status-chip status-${item.status}`}>{item.stage}</span>
                      </div>
                      <p>{item.retryable ? "Retryable" : "Not retryable"} - {item.remediationOwner}</p>
                      {item.readinessItemIds?.length ? <small>{item.readinessItemIds.join(", ")}</small> : null}
                    </article>
                  ))}
                </div>
              ) : null}
            </div>
          ) : (
            <div className="empty-state">
              {activeTenantStatus === "stale" ? "Activation state is refreshing for the active tenant." : "Activation state has not loaded for the active tenant."}
            </div>
          )}
        </section>

        <section className="panel setup-panel">
          <div className="panel-head">
            <div>
              <p className="section-kicker">Hosted Setup</p>
              <h2>Credentials</h2>
            </div>
            <span className="count-chip">{shell.setupTargets.length}</span>
          </div>

          {shell.setupTargets.length ? (
            <div className="setup-list">
              {shell.setupTargets.map((target) => {
                const session = shell.setupSessions.find((item) => item.targetId === target.targetId && item.setupStyle === target.setupStyle) ?? null;
                const state = session?.state ?? target.currentState ?? "not_started";
                const actionBase = session?.setupSessionId ?? target.targetId;
                return (
                  <article className="setup-row" key={`${target.targetId}-${target.setupStyle}`}>
                    <div className="setup-row-main">
                      <div>
                        <strong>{target.displayName}</strong>
                        <p>{target.targetId}</p>
                      </div>
                      <span className={`status-chip status-${state}`}>{state}</span>
                    </div>
                    <div className="setup-meta">
                      <span>{target.setupStyle}</span>
                      <span>{session?.reasonCode ?? target.supportStatus}</span>
                      <span>{session?.redactionStatus ?? "redacted"}</span>
                      {session?.safeUseMode === "limited_safe" ? <span>{session.allowedCapabilities.join(", ") || "limited safe use"}</span> : null}
                    </div>
                    {target.setupStyle === "submitted_secret" ? (
                      <div className="setup-secret-form">
                        <label>
                          <span>Secret Ref</span>
                          <input value={setupSecretRef} onChange={(event) => setSetupSecretRef(event.target.value)} />
                        </label>
                        <label>
                          <span>Secret Value</span>
                          <input value={setupSecretValue} onChange={(event) => setSetupSecretValue(event.target.value)} type="password" />
                        </label>
                        <button
                          className="primary"
                          disabled={!canUseTenantActions || !setupSecretValue || activeActionId === `setup-secret-${target.targetId}`}
                          type="button"
                          onClick={() => {
                            void handleSubmitSetupSecret(target, session);
                          }}
                        >
                          {activeActionId === `setup-secret-${target.targetId}` ? "Submitting..." : "Submit Secret"}
                        </button>
                      </div>
                    ) : null}
                    <div className="inline-actions">
                      {target.setupStyle === "oauth" ? (
                        <>
                          <button disabled={!canUseTenantActions || activeActionId === `setup-oauth-start-${target.targetId}`} type="button" onClick={() => {
                            void handleStartSetupOAuth(target, session);
                          }}>
                            Start OAuth
                          </button>
                          <button className="primary" disabled={!canUseTenantActions || !session || activeActionId === `setup-oauth-complete-${target.targetId}`} type="button" onClick={() => {
                            void handleCompleteSetupOAuth(target, session);
                          }}>
                            Complete Fixture
                          </button>
                        </>
                      ) : (
                        <button disabled={!canUseTenantActions || Boolean(session) || activeActionId === `setup-start-${target.targetId}`} type="button" onClick={() => {
                          void handleStartSetup(target);
                        }}>
                          Start Setup
                        </button>
                      )}
                      {session ? (
                        <>
                          <button disabled={!canUseTenantActions || activeActionId === `setup-retry-${actionBase}`} type="button" onClick={() => {
                            void handleSetupTransition(session, "retry");
                          }}>
                            Retry
                          </button>
                          <button disabled={!canUseTenantActions || activeActionId === `setup-replace-${actionBase}`} type="button" onClick={() => {
                            void handleSetupTransition(session, "replace");
                          }}>
                            Replace
                          </button>
                          <button disabled={!canUseTenantActions || activeActionId === `setup-cancel-${actionBase}`} type="button" onClick={() => {
                            void handleSetupTransition(session, "cancel");
                          }}>
                            Cancel
                          </button>
                          <button disabled={!canUseTenantActions || activeActionId === `setup-disable-${actionBase}`} type="button" onClick={() => {
                            void handleSetupTransition(session, "disable");
                          }}>
                            Disable
                          </button>
                          <button disabled={!canUseTenantActions || detailLoading} type="button" onClick={() => {
                            void handleSetupDiagnostics(session);
                          }}>
                            Diagnostics
                          </button>
                        </>
                      ) : null}
                    </div>
                  </article>
                );
              })}
              {setupDiagnostics.length ? (
                <div className="setup-diagnostics">
                  {setupDiagnostics.map((item) => (
                    <article className="readiness-card" key={`${item.setupSessionId}-${item.diagnosticResultId}`}>
                      <div className="readiness-head">
                        <strong>{item.reasonCode}</strong>
                        <span className={`status-chip status-${item.status}`}>{item.retrySafety}</span>
                      </div>
                      <p>{item.remediationOwner} remediation · {item.redactionStatus}</p>
                      {item.allowedCapabilities?.length ? <small>{item.allowedCapabilities.join(", ")}</small> : null}
                    </article>
                  ))}
                </div>
              ) : null}
            </div>
          ) : (
            <div className="empty-state">No credential setup targets are available for the active tenant.</div>
          )}
        </section>

        <section className="panel approvals-panel">
          <div className="panel-head">
            <div>
              <p className="section-kicker">Human Gate</p>
              <h2>Approval Inbox</h2>
            </div>
            <span className="count-chip">{shell.approvals.length}</span>
          </div>

          {shell.approvals.length ? (
            <div className="stack-list">
              {shell.approvals.map((approval) => (
                <article className="stack-card" key={approval.approvalId}>
                  <div className="stack-head">
                    <strong>{approval.action}</strong>
                    <span className={`status-chip status-${approval.status}`}>{approval.status}</span>
                  </div>
                  <p>{approval.reason}</p>
                  <div className="inline-actions">
                    <button className="primary" disabled={!canUseTenantActions || activeActionId === approval.approvalId} type="button" onClick={() => {
                      void handleApprovalDecision(approval, "approved");
                    }}>
                      Approve
                    </button>
                    <button disabled={!canUseTenantActions || activeActionId === approval.approvalId} type="button" onClick={() => {
                      void handleApprovalDecision(approval, "rejected");
                    }}>
                      Reject
                    </button>
                    <button disabled={!canUseTenantActions} type="button" onClick={() => {
                      void inspectRoute(`/v1/policy/approvals/${approval.approvalId}`, `Approval ${approval.approvalId}`);
                    }}>
                      Inspect
                    </button>
                  </div>
                </article>
              ))}
            </div>
          ) : (
            <div className="empty-state">No pending approvals.</div>
          )}
        </section>

        <section className="panel activity-panel">
          <div className="panel-head">
            <div>
              <p className="section-kicker">Recent Work</p>
              <h2>Activity</h2>
            </div>
            <span className="count-chip">{activityItems.length}</span>
          </div>

          {activityItems.length ? (
            <div className="stack-list">
              {activityItems.map((item) => <ActivityCard key={item.activityId} disabled={!canUseTenantActions} item={item} onInspect={inspectRoute} />)}
            </div>
          ) : (
            <div className="empty-state">No recent operator activity projected yet.</div>
          )}
        </section>

        <section className="panel diagnostics-panel">
          <div className="panel-head">
            <div>
              <p className="section-kicker">Failure Planes</p>
              <h2>Diagnostics</h2>
            </div>
            <span className="count-chip">{diagnosticItems.length}</span>
          </div>

          <div className="filter-row">
            <label>
              <span>Plane Filter</span>
              <select value={diagnosticPlane} onChange={(event) => setDiagnosticPlane(event.target.value)}>
                <option value="">All planes</option>
                <option value="readiness">Readiness</option>
                <option value="approval">Approval</option>
                <option value="execution">Execution</option>
                <option value="delivery">Delivery</option>
              </select>
            </label>
            <label>
              <span>Severity Filter</span>
              <select value={diagnosticSeverity} onChange={(event) => setDiagnosticSeverity(event.target.value)}>
                <option value="">All severities</option>
                <option value="warning">Warning</option>
                <option value="critical">Critical</option>
              </select>
            </label>
            <button className="secondary" disabled={!canUseTenantActions} type="button" onClick={() => {
              void refreshShell({ tenantId: activeTenantId });
            }}>
              Apply Filters
            </button>
          </div>

          {diagnosticItems.length ? (
            <div className="stack-list">
              {diagnosticItems.map((item) => (
                <article className="stack-card" key={item.findingId}>
                  <div className="stack-head">
                    <strong>{item.sourceKind}</strong>
                    <span className={`status-chip status-${item.severity}`}>{item.severity}</span>
                  </div>
                  <p>{item.reason}</p>
                  <small>{item.plane} plane · {item.status}</small>
                  <div className="inline-actions">
                    {item.detailRoute ? (
                      <button disabled={!canUseTenantActions} type="button" onClick={() => {
                        void inspectRoute(item.detailRoute!, `${item.sourceKind} ${item.sourceId}`);
                      }}>
                        Inspect
                      </button>
                    ) : null}
                  </div>
                </article>
              ))}
            </div>
          ) : (
            <div className="empty-state">No matching diagnostics in the current filter window.</div>
          )}
        </section>

        <section className="panel membership-panel">
          <div className="panel-head">
            <div>
              <p className="section-kicker">Tenant Access</p>
              <h2>Memberships</h2>
            </div>
            <span className={`status-chip status-${memberships.status}`}>{memberships.status}</span>
          </div>
          <MembershipPanel
            canManage={canManageMemberships}
            memberships={memberships}
            onRoleChange={handleMembershipRoleChange}
          />
        </section>

        <section className="panel evaluation-panel">
          <div className="panel-head">
            <div>
              <p className="section-kicker">Deterministic Harness</p>
              <h2>Evaluation Replay</h2>
            </div>
            <span className="count-chip">{shell.replayCandidates.length}</span>
          </div>

          <div className="discovery-box" aria-label="candidate discovery review">
            <div className="stack-head">
              <div>
                <p className="section-kicker">Candidate Discovery</p>
                <strong>Product Candidates</strong>
              </div>
              <span className="count-chip">{shell.discoveredCandidates.length}</span>
            </div>
            {shell.discoveryPolicies.length ? (
              <div className="mini-grid">
                {shell.discoveryPolicies.map((policy) => (
                  <div className="mini-metric" key={policy.policyId}>
                    <span>{policy.enabled ? "enabled" : "disabled"}</span>
                    <strong>{policy.maxInspectedRecords}/{policy.maxEmittedCandidates}</strong>
                    <small>{policy.policyId}</small>
                  </div>
                ))}
              </div>
            ) : (
              <div className="empty-state">No discovery policy is configured for this tenant.</div>
            )}
            {shell.discoveredCandidates.length ? (
              <div className="stack-list">
                {shell.discoveredCandidates.map((candidate) => (
                  <article className="stack-card" key={candidate.discoveredCandidateId}>
                    <div className="stack-head">
                      <strong>{candidate.sourceKind}:{candidate.sourceId}</strong>
                      <span className={`status-chip status-${candidate.scoreBand}`}>{candidate.scoreBand}</span>
                    </div>
                    <p>{candidate.redactionStatus} evidence · {candidate.readinessStatus} · {candidate.suppressionState}</p>
                    <small>{candidate.discoveryRunId} · {candidate.retentionState}</small>
                    <div className="inline-actions">
                      <button disabled={!canUseTenantActions} type="button" onClick={() => {
                        void inspectRoute(`/v1/evaluation/discovered-candidates/${candidate.discoveredCandidateId}`, candidate.discoveredCandidateId);
                      }}>
                        Inspect
                      </button>
                      <button disabled={!canUseTenantActions || candidate.suppressionState === "suppressed" || activeActionId === `suppress-${candidate.discoveredCandidateId}`} type="button" onClick={() => {
                        void handleSuppressCandidate(candidate);
                      }}>
                        Suppress
                      </button>
                      <button disabled={!canUseTenantActions || candidate.suppressionState === "suppressed" || activeActionId === `fixture-${candidate.discoveredCandidateId}`} type="button" onClick={() => {
                        void handleCreateProductFixture(candidate);
                      }}>
                        Create Fixture
                      </button>
                    </div>
                  </article>
                ))}
              </div>
            ) : (
              <div className="empty-state">No discovered candidates are awaiting review.</div>
            )}
            {shell.discoveryRuns.length ? <p className="muted-line">{shell.discoveryRuns.length} bounded discovery runs recorded.</p> : null}
          </div>

          {shell.replayCandidates.length ? (
            <div className="stack-list">
              {shell.replayCandidates.map((candidate) => (
                <article className="stack-card" key={candidate.candidateId}>
                  <div className="stack-head">
                    <strong>{candidate.displayName}</strong>
                    <span className={`status-chip status-${candidate.readinessStatus}`}>{candidate.readinessStatus}</span>
                  </div>
                  <p>{(candidate.readinessReasons ?? []).join(" ") || "Replay readiness is available."}</p>
                  <small>{candidate.candidateKind} · {candidate.sourceKind} · {candidate.defaultReplayMode}</small>
                  {(candidate.limitations ?? []).length ? <p className="muted-line">{(candidate.limitations ?? []).join(" ")}</p> : null}
                  <div className="inline-actions">
                    <button className="primary" disabled={!canUseTenantActions || activeActionId === candidate.candidateId} type="button" onClick={() => {
                      void handleLaunchReplay(candidate);
                    }}>
                      Launch Replay
                    </button>
                    <button disabled={!canUseTenantActions || activeActionId === `compare-${candidate.candidateId}`} type="button" onClick={() => {
                      void handleCompareLatest(candidate);
                    }}>
                      Compare Latest
                    </button>
                    <button disabled={!canUseTenantActions} type="button" onClick={() => {
                      void inspectRoute(`/v1/evaluation/replay-candidates/${candidate.candidateId}`, candidate.displayName);
                    }}>
                      Inspect
                    </button>
                  </div>
                </article>
              ))}
            </div>
          ) : (
            <div className="empty-state">No curated replay candidates or fixtures are available in this environment.</div>
          )}

          <div className="campaign-box" aria-label="evaluation campaigns">
            <div className="stack-head">
              <div>
                <p className="section-kicker">Campaigns</p>
                <strong>Replay Campaigns</strong>
              </div>
              <span className="count-chip">{shell.campaigns.length}</span>
            </div>
            <div className="inline-actions">
              <button className="primary" disabled={!canUseTenantActions || activeActionId === "campaign-create"} type="button" onClick={() => {
                void handleCreateCampaign();
              }}>
                Create Campaign
              </button>
            </div>
            {shell.campaigns.length ? (
              <div className="stack-list">
                {shell.campaigns.map((campaign) => (
                  <article className="stack-card" key={campaign.campaignId}>
                    <div className="stack-head">
                      <strong>{campaign.displayName}</strong>
                      <span className={`status-chip status-${campaign.status}`}>{campaign.status}</span>
                    </div>
                    <p>{campaign.scopeSummary || "No campaign scope summary."}</p>
                    <small>{campaign.campaignId} · {campaign.retentionState}</small>
                    <div className="inline-actions">
                      <button disabled={!canUseTenantActions || !["draft", "queued"].includes(campaign.status) || activeActionId === `campaign-start-${campaign.campaignId}`} type="button" onClick={() => {
                        void handleStartCampaign(campaign);
                      }}>
                        Start Campaign
                      </button>
                      <button disabled={!canUseTenantActions || campaign.status !== "completed" || activeActionId === `campaign-publish-${campaign.campaignId}`} type="button" onClick={() => {
                        void handlePublishCampaign(campaign);
                      }}>
                        Publish Results
                      </button>
                      <button disabled={!canUseTenantActions} type="button" onClick={() => {
                        void inspectRoute(`/v1/evaluation/campaigns/${campaign.campaignId}`, campaign.displayName);
                      }}>
                        Inspect Campaign
                      </button>
                    </div>
                  </article>
                ))}
              </div>
            ) : (
              <div className="empty-state">No replay campaigns have been started for this tenant.</div>
            )}
            {shell.campaignAttemptGroups.length ? (
              <div className="mini-card-grid">
                {shell.campaignAttemptGroups.map((group) => (
                  <article className="mini-card" key={group.attemptGroupId}>
                    <strong>{group.attemptGroupId}</strong>
                    <small>{group.status}</small>
                    <p>{group.driftCount} drift · {group.failureCount} failures · {group.unsupportedCount} unsupported · {group.operatorActionNeededCount} action</p>
                  </article>
                ))}
              </div>
            ) : null}
          </div>

          <div className="dashboard-box" aria-label="evaluation dashboard">
            <div className="stack-head">
              <div>
                <p className="section-kicker">Dashboard</p>
                <strong>Evaluation Signals</strong>
              </div>
              <span className="count-chip">{shell.dashboardProjections.length}</span>
            </div>
            {shell.dashboardProjections[0] ? (
              <div className="mini-grid">
                <div className="mini-metric">
                  <span>drift</span>
                  <strong>{shell.dashboardProjections[0].driftSummary?.total ?? 0}</strong>
                  <small>latest projection</small>
                </div>
                <div className="mini-metric">
                  <span>failures</span>
                  <strong>{shell.dashboardProjections[0].failureSummary?.total ?? 0}</strong>
                  <small>campaign evidence</small>
                </div>
                <div className="mini-metric">
                  <span>live links</span>
                  <strong>{shell.dashboardProjections[0].liveValidationSummary?.linked ?? 0}</strong>
                  <small>ledger references</small>
                </div>
              </div>
            ) : (
              <div className="empty-state">No dashboard projection is available yet.</div>
            )}
          </div>

          <div className="inspection-box" aria-label="tool-call inspections">
            <div className="stack-head">
              <div>
                <p className="section-kicker">Inspection</p>
                <strong>Tool Calls</strong>
              </div>
              <span className="count-chip">{shell.toolCallInspections.length}</span>
            </div>
            {shell.toolCallInspections.length ? (
              <div className="stack-list">
                {shell.toolCallInspections.map((inspection) => (
                  <article className="stack-card" key={inspection.inspectionId}>
                    <div className="stack-head">
                      <strong>{inspection.toolCallRef}</strong>
                      <span className={`status-chip status-${inspection.classification}`}>{inspection.classification}</span>
                    </div>
                    <p>{inspection.diffSummary || "No diff summary recorded."}</p>
                    <small>{inspection.redactionStatus} · {(inspection.liveValidationLedgerRefs ?? []).join(", ") || "no live ledger"}</small>
                    <div className="inline-actions">
                      <button disabled={!canUseTenantActions} type="button" onClick={() => {
                        void inspectRoute(`/v1/evaluation/tool-call-inspections/${inspection.inspectionId}`, inspection.inspectionId);
                      }}>
                        Inspect Tool Call
                      </button>
                    </div>
                  </article>
                ))}
              </div>
            ) : (
              <div className="empty-state">No tool-call inspections are linked to the latest campaign.</div>
            )}
          </div>

          <div className="live-validation-box" aria-label="live validation controls">
            <div className="stack-head">
              <div>
                <p className="section-kicker">Live Gate</p>
                <strong>Live Validation Scope</strong>
              </div>
              <span className="count-chip">{shell.liveValidations.length}</span>
            </div>
            <div className="live-validation-form">
              <label>
                <span>Candidate</span>
                <select
                  aria-label="Live validation candidate"
                  disabled={!canUseTenantActions || !shell.replayCandidates.length}
                  value={selectedLiveValidationCandidateId}
                  onChange={(event) => setLiveValidationCandidateId(event.target.value)}
                >
                  {shell.replayCandidates.length ? null : <option value="">No candidate</option>}
                  {shell.replayCandidates.map((candidate) => (
                    <option key={candidate.candidateId} value={candidate.candidateId}>{candidate.displayName}</option>
                  ))}
                </select>
              </label>
              <label>
                <span>Included Tool Classes</span>
                <input
                  aria-label="Included tool classes"
                  disabled={!canUseTenantActions}
                  value={liveValidationToolClasses}
                  onChange={(event) => setLiveValidationToolClasses(event.target.value)}
                  placeholder="read_only, idempotent_mutation"
                />
              </label>
              <label>
                <span>Approval Mode</span>
                <select
                  aria-label="Live validation approval mode"
                  disabled={!canUseTenantActions}
                  value={liveValidationApprovalMode}
                  onChange={(event) => setLiveValidationApprovalMode(event.target.value as "scope_level" | "per_action" | "mixed")}
                >
                  <option value="scope_level">scope_level</option>
                  <option value="per_action">per_action</option>
                  <option value="mixed">mixed</option>
                </select>
              </label>
              <button
                className="primary"
                disabled={!canUseTenantActions || !selectedLiveValidationCandidateId || activeActionId === "live-validation-start"}
                type="button"
                onClick={() => {
                  void handleStartLiveValidation();
                }}
              >
                {activeActionId === "live-validation-start" ? "Starting..." : "Start Live Validation"}
              </button>
            </div>
            {latestLiveValidation ? <LiveValidationGateSummary attempt={latestLiveValidation} onInspect={inspectRoute} disabled={!canUseTenantActions} /> : (
              <div className="empty-state">No live validation attempts have been recorded yet.</div>
            )}
            <div className="support-matrix-strip">
              <strong>Ledger</strong>
              <span>{shell.liveValidationLedger.length} entries · retention {shell.liveValidationRetention?.mode ?? "not loaded"}</span>
            </div>
            {shell.liveValidationLedger.length ? (
              <div className="mini-card-grid">
                {shell.liveValidationLedger.map((entry) => (
                  <article className="mini-card" key={entry.ledgerEntryId}>
                    <strong>{entry.toolClass}</strong>
                    <small>{entry.outcome}</small>
                    <p>{entry.reasonCode || entry.actionRef || "No ledger note."}</p>
                  </article>
                ))}
              </div>
            ) : null}
            <div className="support-matrix-strip">
              <strong>Kill Switches</strong>
              <span>{shell.liveValidationKillSwitches.filter((item) => item.enabled).length} active</span>
            </div>
            <div className="inline-actions">
              <button disabled={!canUseTenantActions || activeActionId === "live-validation-kill-switch"} type="button" onClick={() => {
                void handleEnableTenantKillSwitch();
              }}>
                Enable Tenant Kill Switch
              </button>
            </div>
            <div className="support-matrix-strip">
              <strong>Support Matrix</strong>
              <span>{shell.supportMatrix.length} classes · {unsupportedMatrixRows.length} unsupported</span>
            </div>
            {unsupportedMatrixRows.length ? (
              <div className="mini-card-grid">
                {unsupportedMatrixRows.map((row) => (
                  <article className="mini-card" key={row.toolClass}>
                    <strong>{row.toolClass}</strong>
                    <small>{row.safetyClass}</small>
                    <p>{row.testCase}</p>
                  </article>
                ))}
              </div>
            ) : null}
          </div>

          <div className="fixture-strip">
            <strong>Fixtures</strong>
            <span>Fixtures are engineer-managed and repo-backed; this shell intentionally does not expose fixture editing controls.</span>
          </div>
          {shell.productFixtures.length ? (
            <div className="stack-list">
              {shell.productFixtures.map((fixture) => (
                <article className="stack-card" key={fixture.fixtureId}>
                  <div className="stack-head">
                    <strong>{fixture.displayName}</strong>
                    <span className={`status-chip status-${fixture.reviewState}`}>{fixture.reviewState}</span>
                  </div>
                  <p>{fixture.sourceKind} · {fixture.suppressionState} · {fixture.retentionState}</p>
                  <small>{fixture.fixtureId} · {fixture.currentRevisionId}</small>
                  <div className="inline-actions">
                    <button disabled={!canUseTenantActions || fixture.reviewState === "approved" || activeActionId === `review-${fixture.fixtureId}`} type="button" onClick={() => {
                      void handleReviewProductFixture(fixture);
                    }}>
                      Approve Fixture
                    </button>
                    <button disabled={!canUseTenantActions || activeActionId === `revise-${fixture.fixtureId}`} type="button" onClick={() => {
                      void handleReviseProductFixture(fixture);
                    }}>
                      Revise Fixture
                    </button>
                    <button disabled={!canUseTenantActions || fixture.suppressionState === "suppressed" || activeActionId === `fixture-suppress-${fixture.fixtureId}`} type="button" onClick={() => {
                      void handleSuppressProductFixture(fixture);
                    }}>
                      Suppress Fixture
                    </button>
                    <button disabled={!canUseTenantActions} type="button" onClick={() => {
                      void inspectRoute(`/v1/evaluation/product-fixtures/${fixture.fixtureId}`, fixture.displayName);
                    }}>
                      Inspect Fixture
                    </button>
                  </div>
                </article>
              ))}
            </div>
          ) : (
            <div className="empty-state">No product-managed fixtures have been created yet.</div>
          )}
          {shell.replayFixtures.length ? (
            <div className="mini-card-grid">
              {shell.replayFixtures.map((fixture) => (
                <article className="mini-card" key={fixture.fixtureId}>
                  <strong>{fixture.displayName}</strong>
                  <small>{fixture.domainClass}</small>
                  <p>{(fixture.assumptions ?? []).join(" ") || "No fixture assumptions recorded."}</p>
                </article>
              ))}
            </div>
          ) : null}

          <div className="fixture-strip">
            <strong>Replay History</strong>
            <span>Attempts and comparisons are daemon-owned records; inspect detail for the authoritative payload.</span>
          </div>
          {shell.replayAttempts.length || shell.replayComparisons.length ? (
            <div className="stack-list">
              {shell.replayAttempts.map((attempt) => (
                <article className="stack-card" key={attempt.attemptId}>
                  <div className="stack-head">
                    <strong>{attempt.attemptId}</strong>
                    <span className={`status-chip status-${attempt.status}`}>{attempt.status}</span>
                  </div>
                  <p>{attempt.runtimeSummary || attempt.evidenceSummary || "Replay attempt has no summary yet."}</p>
                  <small>{attempt.mode} · {attempt.sideEffectHandling} · {attempt.candidateId}</small>
                  <div className="inline-actions">
                    <button disabled={!canUseTenantActions} type="button" onClick={() => {
                      void inspectRoute(`/v1/evaluation/replay-attempts/${attempt.attemptId}`, attempt.attemptId);
                    }}>
                      Inspect Attempt
                    </button>
                  </div>
                </article>
              ))}
              {shell.replayComparisons.map((comparison) => (
                <article className="stack-card" key={comparison.comparisonId}>
                  <div className="stack-head">
                    <strong>{comparison.comparisonId}</strong>
                    <span className={`status-chip status-${comparison.terminalStatus}`}>{comparison.terminalStatus}</span>
                  </div>
                  <p>{comparison.runtimeSummary}</p>
                  <small>policy: {comparison.policySummary || "n/a"} · integration: {comparison.integrationSummary || "n/a"} · delivery: {comparison.deliverySummary || "n/a"}</small>
                  {(comparison.driftFindings ?? []).length ? (
                    <div className="drift-list">
                      {(comparison.driftFindings ?? []).map((finding) => <p key={finding.findingId}><strong>{finding.plane}</strong>: {finding.summary}</p>)}
                    </div>
                  ) : null}
                  <div className="inline-actions">
                    <button disabled={!canUseTenantActions} type="button" onClick={() => {
                      void inspectRoute(`/v1/evaluation/comparisons/${comparison.comparisonId}`, comparison.comparisonId);
                    }}>
                      Inspect Comparison
                    </button>
                  </div>
                </article>
              ))}
            </div>
          ) : (
            <div className="empty-state">No replay attempts or comparisons have been recorded yet.</div>
          )}
        </section>

        <section className="panel detail-panel">
          <div className="panel-head">
            <div>
              <p className="section-kicker">Authoritative Detail</p>
              <h2>Same-Shell Inspection</h2>
            </div>
            {detail?.route ? <span className="route-chip">{detail.route}</span> : null}
          </div>

          {detailLoading ? <div className="empty-state">Loading detail...</div> : null}
          {!detailLoading && detail ? (
            <>
              <h3>{detail.title}</h3>
              <small>Loaded under tenant {detail.tenantId}</small>
              <pre>{JSON.stringify(detail.payload, null, 2)}</pre>
            </>
          ) : null}
          {!detailLoading && !detail ? <div className="empty-state">Inspect an approval, activity item, readiness item, or diagnostic to view daemon detail here.</div> : null}
        </section>
      </section>
    </main>
  );
}

function LiveValidationGateSummary(props: {
  attempt: LiveValidationAttemptResource;
  disabled: boolean;
  onInspect: (route: string, title: string) => Promise<void>;
}) {
  const { attempt, disabled, onInspect } = props;
  const gateDecisions = [
    ["permission", attempt.permissionDecision],
    ["quota", attempt.quotaDecision],
    ["kill switch", attempt.killSwitchDecision]
  ] as const;

  return (
    <article className="live-validation-gates">
      <div className="stack-head">
        <strong>{attempt.validationId}</strong>
        <span className={`status-chip status-${attempt.status}`}>{attempt.status}</span>
      </div>
      <p>{attempt.candidateId} · approvals {attempt.approvalSummary.approved}/{attempt.approvalSummary.required} · pending {attempt.approvalSummary.pending}</p>
      <div className="gate-grid">
        {gateDecisions.map(([label, decision]) => (
          <span className={`gate-chip gate-${decision.allowed ? "allowed" : "denied"}`} key={label}>
            {label}: {decision.allowed ? "allowed" : decision.reasonCode || "denied"}
          </span>
        ))}
      </div>
      <small>{(attempt.requestedScope.includedToolClasses ?? []).join(", ") || "all declared classes"} · {attempt.requestedScope.approvalMode}</small>
      <div className="inline-actions">
        <button disabled={disabled} type="button" onClick={() => {
          void onInspect(`/v1/live-validations/${attempt.validationId}`, attempt.validationId);
        }}>
          Inspect Live Validation
        </button>
      </div>
    </article>
  );
}

function ActivityCard(props: {
  item: OperatorActivityRecord;
  disabled: boolean;
  onInspect: (route: string, title: string) => Promise<void>;
}) {
  const { item, disabled, onInspect } = props;

  return (
    <article className="stack-card">
      <div className="stack-head">
        <strong>{item.title}</strong>
        <span className={`status-chip status-${item.attentionLevel}`}>{item.attentionLevel}</span>
      </div>
      <p>{item.summary}</p>
      <small>{item.sourceKind} · {item.status}</small>
      <div className="inline-actions">
        {item.detailRoute ? (
          <button disabled={disabled} type="button" onClick={() => {
            void onInspect(item.detailRoute!, item.title);
          }}>
            Inspect
          </button>
        ) : null}
        {item.relatedResourceRefs?.map((ref) => (
          <button disabled={disabled || !ref.route} key={`${item.activityId}-${ref.kind}-${ref.id}`} type="button" onClick={() => {
            if (ref.route) {
              void onInspect(ref.route, `${ref.kind} ${ref.id}`);
            }
          }}>
            {ref.kind}
          </button>
        ))}
      </div>
    </article>
  );
}

function MembershipPanel(props: {
  canManage: boolean;
  memberships: MembershipPanelState;
  onRoleChange: (membership: MembershipResource, role: TenantRole) => Promise<void>;
}) {
  const { canManage, memberships, onRoleChange } = props;

  if (!canManage) {
    return <div className="empty-state">Membership role controls are unavailable for the active tenant.</div>;
  }
  if (memberships.status === "loading") {
    return <div className="empty-state">Loading active tenant memberships.</div>;
  }
  if (memberships.status === "denied") {
    return <div className="empty-state">Membership access was denied for the active tenant.</div>;
  }
  if (memberships.status === "empty") {
    return <div className="empty-state">Only the owner is active for this organization tenant.</div>;
  }

  return (
    <>
      {memberships.error ? <div className="error-box" role="alert">{memberships.error}</div> : null}
      <div className="membership-table">
        {memberships.members.map((membership) => (
          <article className="membership-row" key={membership.membershipId}>
            <div>
              <strong>{membership.principalId}</strong>
              <small>{membership.status}</small>
            </div>
            <select
              aria-label={`Role for ${membership.principalId}`}
              disabled={memberships.pendingMembershipId === membership.membershipId}
              value={membership.role}
              onChange={(event) => {
                void onRoleChange(membership, event.target.value as TenantRole);
              }}
            >
              {ROLE_OPTIONS.map((role) => <option key={role} value={role}>{role}</option>)}
            </select>
          </article>
        ))}
      </div>
    </>
  );
}

function resolveActiveTenant(input: {
  allowed: TenantResource[];
  me: AuthMeResponse;
  requestedTenantId?: string;
  explicitSelection?: boolean;
  savedTenantId?: string;
}): { tenant: TenantResource | null; message: string } {
  const requested = input.requestedTenantId?.trim();
  if (requested) {
    const tenant = input.allowed.find((item) => item.tenantId === requested) ?? null;
    if (tenant) {
      return { tenant, message: `Tenant ${tenant.displayName} is active.` };
    }
    if (input.explicitSelection) {
      return { tenant: null, message: "Selected tenant is no longer allowed for this principal." };
    }
  }

  const saved = input.savedTenantId?.trim();
  if (saved) {
    const tenant = input.allowed.find((item) => item.tenantId === saved);
    if (tenant) {
      return { tenant, message: `Restored tenant ${tenant.displayName} after revalidation.` };
    }
  }

  const preferred =
    input.allowed.find((item) => item.defaultForCurrentToken) ??
    input.allowed.find((item) => item.defaultForCurrentPrincipal) ??
    input.allowed.find((item) => item.tenantId === input.me.currentTenant.tenantId) ??
    input.allowed.find((item) => item.tenantId === input.me.defaultTenant.tenantId) ??
    input.allowed[0] ??
    null;

  if (!preferred) {
    return { tenant: null, message: "No allowed tenant is available for this principal." };
  }
  return { tenant: preferred, message: `Tenant ${preferred.displayName} is active.` };
}

function hasPermission(tenant: TenantResource | null, permission: TenantPermission): boolean {
  return Boolean(tenant?.callerPermissions?.includes(permission));
}

function membershipStatusFor(items: MembershipResource[]): MembershipStatus {
  const activeItems = items.filter((item) => item.status === "active");
  if (activeItems.length <= 1 && activeItems.every((item) => item.role === "owner")) {
    return "empty";
  }
  return "ready";
}

function preferenceKey(daemonURL: string, principalId: string): string {
  return `kura.activeTenant.${daemonURL.trim()}.${principalId}`;
}

function readTenantPreference(daemonURL: string, principalId: string): string | undefined {
  try {
    return window.localStorage.getItem(preferenceKey(daemonURL, principalId)) ?? undefined;
  } catch {
    return undefined;
  }
}

function writeTenantPreference(daemonURL: string, principalId: string, tenantId: string) {
  try {
    window.localStorage.setItem(preferenceKey(daemonURL, principalId), tenantId);
  } catch {
    // Browser storage is continuity only; failing closed to no preference is acceptable.
  }
}

function setupOAuthRedirectRoute(target: SetupTargetResource): string {
  const normalized = `${target.targetId} ${target.displayName}`.toLowerCase();
  if (normalized.includes("slack")) {
    return "/setup/oauth/slack/callback";
  }
  if (normalized.includes("feishu") || normalized.includes("lark")) {
    return "/setup/oauth/feishu-lark/callback";
  }
  return "/setup/oauth/callback";
}

function splitCSV(value: string): string[] {
  return value.split(",").map((item) => item.trim()).filter(Boolean);
}

function formatActivationQuota(value: number | undefined, _unit?: string): string {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    return "unknown";
  }
  return new Intl.NumberFormat("en-US").format(value);
}

function formatDateTime(value: string | undefined): string {
  if (!value) {
    return "unknown";
  }
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }
  return date.toLocaleString();
}

function formatLabel(value: string): string {
  return value.replace(/[_:.-]+/g, " ").replace(/\b\w/g, (letter) => letter.toUpperCase());
}

function quotaPercent(item: BillingQuotaStatusItem): number {
  if (!Number.isFinite(item.limit) || item.limit <= 0 || item.status === "unlimited" || item.status === "not_measurable") {
    return 0;
  }
  const used = item.currentPeriod.consumedAmount + item.currentPeriod.reservedAmount + item.currentPeriod.adjustedAmount - item.currentPeriod.carryoverApplied;
  return Math.max(0, Math.min(100, Math.round((used / item.limit) * 100)));
}

function errorMessage(caught: unknown): string {
  return caught instanceof Error ? caught.message : String(caught);
}

function isTenantDenied(caught: unknown): boolean {
  return typeof caught === "object" && caught !== null && "tenantDenied" in caught && Boolean((caught as { tenantDenied?: boolean }).tenantDenied);
}
