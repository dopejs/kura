import { describe, expect, it, vi } from "vitest";

import { KuraClientError, createKuraClient, type MembershipResource, type SetupSessionResource, type TenantResource, type TenantSecretResource } from "./index.js";

function mockJSONResponse(status: number, payload: unknown): Response {
  return new Response(JSON.stringify(payload), {
    status,
    headers: {
      "Content-Type": "application/json"
    }
  });
}

function productFixtureMutationFixture(overrides: Record<string, unknown> = {}) {
  const revisionId = String(overrides.revisionId ?? "revision_1");
  const reviewState = String(overrides.reviewState ?? "draft");
  const suppressionState = String(overrides.suppressionState ?? "none");
  const revisionNumber = Number(overrides.revisionNumber ?? 1);
  return {
    fixture: {
      fixtureId: "product_fixture_1",
      tenantId: "ten_eval",
      displayName: "Product Fixture",
      domainClass: "schedule",
      sourceKind: "discovered_candidate",
      sourceCandidateId: "candidate_1",
      currentRevisionId: revisionId,
      reviewState,
      suppressionState,
      retentionState: "active",
      createdAt: "2026-04-29T10:00:00Z",
      updatedAt: "2026-04-29T10:00:00Z"
    },
    revision: {
      revisionId,
      fixtureId: "product_fixture_1",
      tenantId: "ten_eval",
      revisionNumber,
      fixturePayload: { goal: "safe" },
      redactionStatus: "redacted",
      createdAt: "2026-04-29T10:00:00Z"
    }
  };
}

function campaignFixture(overrides: Record<string, unknown> = {}) {
  return {
    campaignId: "campaign_1",
    tenantId: "ten_eval",
    displayName: "Campaign",
    status: overrides.status ?? "queued",
    scopeSummary: "release gate",
    startedBy: "prn_eval",
    createdAt: "2026-04-29T10:00:00Z",
    retentionState: "active",
    ...overrides
  };
}

function tenantResource(overrides: Partial<TenantResource> = {}): TenantResource {
  return {
    tenantId: "ten_personal",
    tenantKind: "personal",
    displayName: "Personal Tenant",
    status: "active",
    createdAt: "2026-04-24T10:00:00Z",
    updatedAt: "2026-04-24T10:00:00Z",
    callerMembershipRole: "owner",
    callerMembershipStatus: "active",
    callerPermissions: ["tenant.manage"],
    defaultForCurrentToken: true,
    defaultForCurrentPrincipal: true,
    ...overrides
  };
}

function membershipResource(overrides: Partial<MembershipResource> = {}): MembershipResource {
  return {
    membershipId: "mem_1",
    tenantId: "ten_personal",
    principalId: "prn_1",
    role: "owner",
    status: "active",
    createdAt: "2026-04-24T10:00:00Z",
    updatedAt: "2026-04-24T10:00:00Z",
    ...overrides
  };
}

function tenantSecretResource(overrides: Partial<TenantSecretResource> = {}): TenantSecretResource {
  return {
    secretId: "sec_1",
    tenantId: "ten_personal",
    secretRef: "provider/api-key",
    displayName: "Provider API key",
    status: "active",
    activeVersionId: "secver_1",
    createdAt: "2026-04-24T10:00:00Z",
    updatedAt: "2026-04-24T10:00:00Z",
    rotatedAt: "2026-04-24T10:00:00Z",
    secretRefs: [{ secretRef: "provider/api-key", resolution: "unavailable", redactionRule: "secret_ref_only" }],
    ...overrides
  };
}

function setupSessionResource(overrides: Partial<SetupSessionResource> = {}): SetupSessionResource {
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

function activationResponseFixture(overrides: Record<string, unknown> = {}) {
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
      basePlanLabel: "finite",
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

function billingDenialDetailFixture(overrides: Record<string, unknown> = {}) {
  return {
    denialId: "denial_1",
    tenantId: "ten_personal",
    operationRef: "run:client_1",
    operationKey: "tenant:ten_personal:run:client_1",
    guardedEntryPoint: "POST /v1/runs",
    category: "run_launches",
    reasonCode: "quota_denied:run_launches_exhausted",
    classification: "quota_exhaustion",
    requestedAmount: 1,
    remainingAmount: 0,
    recoveryActions: ["wait", "reduce_scope"],
    createdAt: "2026-05-07T10:00:00Z",
    ...overrides
  };
}

function billingEvidenceExportFixture(overrides: Record<string, unknown> = {}) {
  const denial = billingDenialDetailFixture();
  return {
    schemaVersion: "2026-05-07",
    exportId: "evidence_denial_1",
    tenantId: "ten_personal",
    generatedAt: "2026-05-07T10:01:00Z",
    generatedByPrincipalId: "prn_support",
    denial,
    usageSnapshot: [],
    effectiveLimitState: {},
    auditRefs: ["audit_1"],
    redactions: [{ path: "$.secret", reason: "secret", replacement: "[REDACTED]" }],
    ...overrides
  };
}

describe("KuraClient", () => {
  it("sends a non-stream chat request", async () => {
    let url = "";
    let authorization = "";

    const client = createKuraClient({
      baseURL: "http://127.0.0.1:19192/",
      accessToken: "token",
      fetchImpl: async (input: string | URL | Request, init?: RequestInit) => {
        url = String(input);
        authorization = String((init?.headers as Record<string, string>).Authorization);
        return mockJSONResponse(200, {
          dispatchId: "dispatch_1",
          provider: "openai_compatible",
          model: "gpt-test",
          skills: ["shared"],
          query: "hello",
          status: "completed",
          partial: false,
          reply: "world",
          usage: { inputTokens: 1, outputTokens: 1, totalTokens: 2 }
        });
      }
    });

    const response = await client.queryChat({ query: "hello", skills: [" shared ", ""] });
    expect(url).toBe("http://127.0.0.1:19192/v1/chat/query");
    expect(authorization).toBe("Bearer token");
    expect(response.reply).toBe("world");
    expect(response.skills).toEqual(["shared"]);
  });

  it("sends a response format and returns the structured output", async () => {
    let body: Record<string, unknown> = {};
    const client = createKuraClient({
      baseURL: "http://127.0.0.1:19192",
      fetchImpl: async (_input: string | URL | Request, init?: RequestInit) => {
        body = JSON.parse(String(init?.body));
        return mockJSONResponse(200, {
          dispatchId: "dispatch_1",
          provider: "openai_compatible",
          model: "gpt-test",
          skills: [],
          query: "who?",
          status: "completed",
          partial: false,
          reply: "{\"name\":\"Ada\"}",
          usage: { inputTokens: 1, outputTokens: 1, totalTokens: 2 },
          structuredOutput: { name: "Ada" }
        });
      }
    });

    const schema = { type: "object", properties: { name: { type: "string" } }, required: ["name"] };
    const response = await client.queryChat({
      query: "who?",
      responseFormat: { type: "json_schema", name: " person ", schema }
    });
    expect(body.responseFormat).toEqual({ type: "json_schema", name: "person", schema });
    expect(response.structuredOutput).toEqual({ name: "Ada" });
  });

  it("sends a query with a request id, then a message and a cancel to that turn", async () => {
    const calls: Array<{ url: string; method?: string; requestId?: string | null; body?: unknown }> = [];
    const client = createKuraClient({
      baseURL: "http://127.0.0.1:19192",
      fetchImpl: async (input: string | URL | Request, init?: RequestInit) => {
        const headers = new Headers(init?.headers);
        calls.push({
          url: String(input),
          method: init?.method,
          requestId: headers.get("X-Request-Id"),
          body: init?.body === undefined ? undefined : JSON.parse(String(init.body))
        });
        if (String(input).endsWith("/messages")) {
          return mockJSONResponse(202, { requestId: "req 1", pending: 1 });
        }
        if (String(input).endsWith("/cancel")) {
          return mockJSONResponse(202, { requestId: "req 1" });
        }
        return mockJSONResponse(200, {
          dispatchId: "dispatch_1",
          provider: "echo",
          model: "m",
          skills: [],
          query: "q",
          status: "completed",
          partial: false,
          reply: "r",
          usage: { inputTokens: 1, outputTokens: 1, totalTokens: 2 }
        });
      }
    });

    await client.queryChat({ query: "q" }, { requestId: " req 1 " });
    const accepted = await client.sendChatTurnMessage("req 1", " only PDFs ");
    const cancelled = await client.cancelChatTurn("req 1");
    await client.queryChat({ query: "q" });

    expect(calls[0].requestId).toBe("req 1");
    expect(calls[1]).toEqual({
      url: "http://127.0.0.1:19192/v1/chat/turns/req%201/messages",
      method: "POST",
      requestId: null,
      body: { message: "only PDFs" }
    });
    expect(accepted.pending).toBe(1);
    expect(calls[2].url).toBe("http://127.0.0.1:19192/v1/chat/turns/req%201/cancel");
    expect(cancelled.requestId).toBe("req 1");
    expect(calls[3].requestId).toBeNull();
  });

  it("loads operator surfaces, approvals, details, and run creation with normalized URLs", async () => {
    const fetchImpl = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(mockJSONResponse(200, {
        environmentScope: "test",
        status: "ready_for_action",
        blockingItemIds: [],
        optionalFollowUpItemIds: [],
        readinessItems: [],
        firstUsefulActions: [],
        lastEvaluatedAt: "2026-04-24T10:00:00Z"
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        environmentScope: "test",
        items: [],
        generatedAt: "2026-04-24T10:00:00Z"
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        environmentScope: "test",
        items: [],
        generatedAt: "2026-04-24T10:00:00Z"
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        items: [{ approvalId: "approval_1", action: "workflow.launch", reason: "review", status: "pending", createdAt: "2026-04-24T10:00:00Z", updatedAt: "2026-04-24T10:00:00Z" }]
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        runId: "run_1",
        entrypoint: "operator.shell.test",
        status: "queued",
        goal: "shell smoke",
        createdAt: "2026-04-24T10:00:00Z",
        updatedAt: "2026-04-24T10:00:00Z"
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        approval: {
          approvalId: "approval_1",
          action: "workflow.launch",
          reason: "review",
          status: "approved",
          createdAt: "2026-04-24T10:00:00Z",
          updatedAt: "2026-04-24T10:01:00Z",
          resolvedAt: "2026-04-24T10:01:00Z",
          resolution: "approved"
        },
        decision: {
          decisionId: "decision_1",
          action: "workflow.launch",
          outcome: "approved",
          reason: "review",
          approvalId: "approval_1",
          createdAt: "2026-04-24T10:01:00Z"
        }
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        runId: "run_1",
        entrypoint: "operator.shell.test",
        status: "completed",
        goal: "shell smoke",
        createdAt: "2026-04-24T10:00:00Z",
        updatedAt: "2026-04-24T10:02:00Z"
      }));

    const client = createKuraClient({
      baseURL: "http://127.0.0.1:19192/",
      accessToken: "token",
      fetchImpl
    });

    await client.getOnboarding();
    await client.getActivity({ attentionOnly: true, limit: 5 });
    await client.getDiagnostics({ plane: "delivery", severity: "critical" });
    await client.listApprovals("pending");
    await client.createRun({ entrypoint: "operator.shell.test", goal: "shell smoke" });
    await client.resolveApproval("approval_1", { resolution: "approved", comment: "ok" });
    const detail = await client.fetchRoute<{ runId: string }>("v1/runs/run_1");

    expect(fetchImpl).toHaveBeenNthCalledWith(1, "http://127.0.0.1:19192/v1/operator/onboarding", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(2, "http://127.0.0.1:19192/v1/operator/activity?attentionOnly=true&limit=5", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(3, "http://127.0.0.1:19192/v1/operator/diagnostics?plane=delivery&severity=critical", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(4, "http://127.0.0.1:19192/v1/policy/approvals?status=pending", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(5, "http://127.0.0.1:19192/v1/runs", expect.objectContaining({ method: "POST" }));
    expect(fetchImpl).toHaveBeenNthCalledWith(6, "http://127.0.0.1:19192/v1/policy/approvals/approval_1/resolve", expect.objectContaining({ method: "POST" }));
    expect(fetchImpl).toHaveBeenNthCalledWith(7, "http://127.0.0.1:19192/v1/runs/run_1", expect.anything());
    expect(detail.runId).toBe("run_1");
  });

  it("lists, routes, and clears model roles", async () => {
    const fetchImpl = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(mockJSONResponse(200, {
        items: [
          { role: "primary", providerId: "anthropic", model: "claude-sonnet", routed: true, source: "config" },
          { role: "image", providerId: "", model: "", routed: false, source: "unrouted" },
        ],
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        role: "image", providerId: "studio", model: "sd3-medium", routed: true, source: "store",
      }))
      .mockResolvedValueOnce(new Response(null, { status: 204 }));
    const client = createKuraClient({ baseURL: "http://127.0.0.1:19192", fetchImpl });

    const listed = await client.listModelRoles();
    // An unrouted role must be reported, not omitted: callers need to see that
    // the capability is unavailable.
    expect(listed.items.map((item) => item.role)).toEqual(["primary", "image"]);
    expect(listed.items[1].routed).toBe(false);

    const routed = await client.setModelRole("image", { providerId: "studio", model: "sd3-medium" });
    expect(routed.routed).toBe(true);
    expect(routed.source).toBe("store");

    await client.clearModelRole("image");

    expect(fetchImpl.mock.calls[0][0]).toContain("/v1/model-roles");
    expect(fetchImpl.mock.calls[1][1]?.method).toBe("PUT");
    expect(fetchImpl.mock.calls[2][1]?.method).toBe("DELETE");
  });

  it("accepts an embedded deployment environment from getConfig", async () => {
    // Deployment shape is distinct from the data-isolation scope: a
    // host-supervised daemon reports `embedded` while still writing
    // `test`-scoped data, so ConfigResponse must not be narrowed to
    // EnvironmentScope.
    const fetchImpl = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(mockJSONResponse(200, {
        environment: "embedded",
        bindAddr: "127.0.0.1:54321",
        dataDir: "/tmp/workspace/.loopforge/agent/data",
        configFilePath: "/tmp/workspace/.loopforge/agent/data/config.json",
        logLevel: "info",
        version: "dev",
        llm: {},
        connectors: {},
      }));
    const client = createKuraClient({ baseURL: "http://127.0.0.1:19192", fetchImpl });

    const config = await client.getConfig();

    expect(config.environment).toBe("embedded");
    expect(config.dataDir).toBe("/tmp/workspace/.loopforge/agent/data");
  });

  it("calls Discord hosted setup and config routes", async () => {
    const fetchImpl = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(mockJSONResponse(200, {
        environment: "test",
        bindAddr: "127.0.0.1:19192",
        dataDir: "/tmp/kura",
        configFilePath: "/tmp/kura/config.json",
        logLevel: "info",
        version: "dev",
        llm: {},
        connectors: {
          discord: {
            enabled: true,
            configured: true,
            connectorId: "discord-main",
            displayName: "Discord Main",
            deliveryMode: "gateway",
            requireMention: true,
            respondInDM: true,
            allowedGuildIds: ["guild_redacted"],
            allowedChannelIds: ["channel_redacted"],
            botTokenConfigured: true,
            hostedReadiness: {
              connectorId: "discord-main",
              displayName: "Discord Main",
              deliveryMode: "gateway",
              readinessState: "degraded_needs_repair",
              hostedReady: false,
              localCompatible: true,
              reasonCode: "destination_validation_required",
              requireMention: true,
              respondInDM: true,
              allowedGuildIds: ["guild_redacted"],
              allowedChannelIds: ["channel_redacted"],
              botTokenConfigured: true,
              redactionStatus: "redacted"
            }
          }
        },
        mcp: {},
        sandbox: {},
        redactedFields: ["connectors.discord.botToken"]
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        tenantId: "ten_discord",
        connectorId: "discord-main",
        connectorKind: "discord",
        displayName: "Discord Main",
        status: "degraded",
        readinessState: "degraded_needs_repair",
        hostedReady: false,
        credentialState: "valid",
        respondInDM: true,
        requireMention: true,
        deliveryMode: "gateway",
        reasonCode: "destination_validation_failed",
        redactionStatus: "redacted",
        createdAt: "2026-05-07T10:00:00Z",
        updatedAt: "2026-05-07T10:01:00Z",
        validatedAt: "2026-05-07T10:01:00Z",
        retentionExpiresAt: "2026-08-05T10:01:00Z",
        destinations: [{
          tenantId: "ten_discord",
          connectorId: "discord-main",
          destinationId: "channel_redacted",
          destinationType: "channel",
          selected: true,
          validationState: "missing_permission",
          reasonCode: "permission_missing",
          validatedAt: "2026-05-07T10:01:00Z",
          redactionStatus: "redacted",
          safeEvidence: { permission: "send_messages" }
        }]
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        smokeEvidenceId: "discord_smoke_1",
        tenantId: "ten_discord",
        connectorId: "discord-main",
        status: "skipped",
        credentialMode: "unavailable",
        owner: "operator",
        reason: "safe_credentials_unavailable",
        remainingRisk: "No live Discord hosted smoke was run in this release validation.",
        validatedAt: "2026-05-07T10:02:00Z",
        retentionExpiresAt: "2026-08-05T10:02:00Z",
        redactionStatus: "redacted",
        safeEvidence: { policy: "structured_skip" }
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        tenantId: "ten_discord",
        connectorId: "discord-main",
        items: [{
          conformanceResultId: "conf_1",
          tenantId: "ten_discord",
          connectorKind: "discord",
          connectorId: "discord-main",
          scenarioId: "discord_hosted_setup_discord-main",
          area: "tenant_ownership",
          result: "pass",
          reasonCode: "matched",
          redactionStatus: "redacted",
          evidenceTimestamp: "2026-05-07T10:02:00Z",
          retentionExpiresAt: "2026-08-05T10:02:00Z"
        }]
      }));

    const client = createKuraClient({
      baseURL: "http://127.0.0.1:19192",
      accessToken: "token",
      defaultTenantId: "ten_discord",
      fetchImpl
    });

    const config = await client.getConfig();
    const setup = await client.getDiscordSetup(" discord-main ");
    const smoke = await client.getDiscordSmokeEvidence("discord-main");
    const conformance = await client.getDiscordConformanceEvidence("discord-main");

    expect(config.connectors.discord.hostedReadiness.hostedReady).toBe(false);
    expect(config.connectors.discord.hostedReadiness.reasonCode).toBe("destination_validation_required");
    expect(setup.destinations?.[0]?.reasonCode).toBe("permission_missing");
    expect(smoke.reason).toBe("safe_credentials_unavailable");
    expect(conformance.items[0]?.area).toBe("tenant_ownership");
    expect(fetchImpl).toHaveBeenNthCalledWith(1, "http://127.0.0.1:19192/v1/config", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(2, "http://127.0.0.1:19192/v1/connectors/discord-main/discord-setup", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(3, "http://127.0.0.1:19192/v1/connectors/discord-main/discord-smoke", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(4, "http://127.0.0.1:19192/v1/live-validations/discord-conformance?connectorId=discord-main", expect.anything());
  });

  it("calls Telegram hosted setup and live validation routes", async () => {
    const fetchImpl = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(mockJSONResponse(200, {
        tenantId: "ten_telegram",
        connectorId: "telegram-main",
        connectorKind: "telegram",
        displayName: "Telegram Main",
        status: "healthy",
        terminalState: "ready",
        hostedReady: true,
        credentialState: "valid",
        allowmentState: "valid",
        groupBehavior: "mention_or_command_required",
        deliveryEligible: true,
        reasonCode: "healthy",
        redactionStatus: "redacted",
        createdAt: "2026-05-08T10:00:00Z",
        updatedAt: "2026-05-08T10:01:00Z",
        validatedAt: "2026-05-08T10:01:00Z",
        retentionExpiresAt: "2026-08-06T10:01:00Z",
        allowments: [{
          tenantId: "ten_telegram",
          connectorId: "telegram-main",
          allowmentId: "allow_dm",
          telegramScopeType: "direct_chat",
          telegramScopeId: "chat_redacted",
          enabled: true,
          groupGate: "not_applicable",
          validationState: "valid",
          reasonCode: "healthy",
          validatedAt: "2026-05-08T10:01:00Z",
          redactionStatus: "redacted",
          safeEvidence: { scope: "direct_chat" }
        }]
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        smokeEvidenceId: "telegram_smoke_1",
        tenantId: "ten_telegram",
        connectorId: "telegram-main",
        status: "skipped",
        credentialMode: "unavailable",
        owner: "operator",
        reason: "safe_credentials_unavailable",
        remainingRisk: "No live Telegram hosted smoke was run in this release validation.",
        validatedAt: "2026-05-08T10:02:00Z",
        retentionExpiresAt: "2026-08-06T10:02:00Z",
        redactionStatus: "redacted",
        safeEvidence: { policy: "structured_skip" }
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        tenantId: "ten_telegram",
        connectorId: "telegram-main",
        items: [{
          conformanceResultId: "conf_telegram_1",
          tenantId: "ten_telegram",
          connectorKind: "telegram",
          connectorId: "telegram-main",
          scenarioId: "telegram_hosted_setup",
          area: "tenant_ownership",
          result: "pass",
          reasonCode: "matched",
          redactionStatus: "redacted",
          evidenceTimestamp: "2026-05-08T10:02:00Z",
          retentionExpiresAt: "2026-08-06T10:02:00Z"
        }]
      }));

    const client = createKuraClient({
      baseURL: "http://127.0.0.1:19192",
      accessToken: "token",
      defaultTenantId: "ten_telegram",
      fetchImpl
    });

    const setup = await client.getTelegramSetup(" telegram-main ");
    const smoke = await client.getTelegramSmokeEvidence("telegram-main");
    const conformance = await client.getTelegramConformanceEvidence("telegram-main");

    expect(setup.terminalState).toBe("ready");
    expect(setup.allowments?.[0]?.telegramScopeType).toBe("direct_chat");
    expect(smoke.reason).toBe("safe_credentials_unavailable");
    expect(conformance.items[0]?.connectorKind).toBe("telegram");
    expect(fetchImpl).toHaveBeenNthCalledWith(1, "http://127.0.0.1:19192/v1/connectors/telegram-main/telegram-setup", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(2, "http://127.0.0.1:19192/v1/connectors/telegram-main/telegram-smoke", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(3, "http://127.0.0.1:19192/v1/live-validations/telegram-conformance?connectorId=telegram-main", expect.anything());
  });

  it("calls Slack hosted setup, smoke, and conformance routes", async () => {
    const fetchImpl = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(mockJSONResponse(200, {
        tenantId: "ten_slack",
        connectorId: "slack-main",
        connectorKind: "slack",
        displayName: "Slack Main",
        status: "healthy",
        terminalState: "ready",
        oauthState: "grant_valid",
        routePolicyState: "valid",
        deliveryEligible: true,
        workspaceBindingId: "slack_workspace_binding_1",
        redactionStatus: "redacted",
        createdAt: "2026-05-08T10:00:00Z",
        updatedAt: "2026-05-08T10:01:00Z",
        validatedAt: "2026-05-08T10:01:00Z",
        retentionExpiresAt: "2026-08-06T10:01:00Z",
        routePolicy: {
          connectorId: "slack-main",
          workspaceBindingId: "slack_workspace_binding_1",
          selectedChannels: [{ conversationId: "channel_redacted", conversationType: "channel", selectedChannelState: "selected", validationState: "valid" }],
          allowedDMUsers: ["user_hash_1"],
          allowedDMUserGroups: ["group_hash_1"],
          mentionGate: "agent_mention_required",
          threadReplyMode: "channel_mentions_thread_rooted",
          validationState: "valid",
          validatedAt: "2026-05-08T10:01:00Z",
          redactionStatus: "redacted"
        }
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        smokeEvidenceId: "slack_smoke_1",
        tenantId: "ten_slack",
        connectorId: "slack-main",
        workspaceBindingId: "slack_workspace_binding_1",
        status: "skipped",
        authorizationMode: "unavailable",
        owner: "operator",
        reason: "safe_slack_authorization_unavailable",
        remainingRisk: "No live Slack hosted smoke was run in this release validation.",
        validatedAt: "2026-05-08T10:02:00Z",
        retentionExpiresAt: "2026-08-06T10:02:00Z",
        redactionStatus: "redacted",
        safeEvidence: { policy: "structured_skip" }
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        smokeEvidenceId: "slack_smoke_live_1",
        tenantId: "ten_slack",
        connectorId: "slack-main",
        workspaceBindingId: "slack_workspace_binding_1",
        status: "passed",
        authorizationMode: "fake_oauth",
        owner: "operator",
        reason: "healthy",
        remainingRisk: "",
        validatedAt: "2026-05-08T10:02:30Z",
        retentionExpiresAt: "2026-08-06T10:02:30Z",
        redactionStatus: "redacted",
        safeEvidence: { mode: "fake" }
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        tenantId: "ten_slack",
        connectorId: "slack-main",
        items: [{
          conformanceResultId: "conf_slack_1",
          tenantId: "ten_slack",
          connectorKind: "slack",
          connectorId: "slack-main",
          scenarioId: "slack_hosted_setup",
          area: "tenant_ownership",
          result: "pass",
          redactionStatus: "redacted",
          evidenceTimestamp: "2026-05-08T10:02:00Z",
          retentionExpiresAt: "2026-08-06T10:02:00Z"
        }]
      }));

    const client = createKuraClient({
      baseURL: "http://127.0.0.1:19192",
      accessToken: "token",
      defaultTenantId: "ten_slack",
      fetchImpl
    });

    const setup = await client.getSlackSetup(" slack-main ");
    const smoke = await client.getSlackSmokeEvidence("slack-main");
    const liveSmoke = await client.getSlackLiveValidationSmokeEvidence("slack-main");
    const conformance = await client.getSlackConformanceEvidence("slack-main");

    expect(setup.routePolicy?.selectedChannels[0]?.conversationType).toBe("channel");
    expect(smoke.authorizationMode).toBe("unavailable");
    expect(liveSmoke.authorizationMode).toBe("fake_oauth");
    expect(conformance.items[0]?.connectorKind).toBe("slack");
    expect(fetchImpl).toHaveBeenNthCalledWith(1, "http://127.0.0.1:19192/v1/connectors/slack-main/slack-setup", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(2, "http://127.0.0.1:19192/v1/connectors/slack-main/slack-smoke", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(3, "http://127.0.0.1:19192/v1/live-validations/slack-smoke?connectorId=slack-main", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(4, "http://127.0.0.1:19192/v1/live-validations/slack-conformance?connectorId=slack-main", expect.anything());
  });

  it("calls Matrix hosted setup, smoke, live smoke, and conformance routes", async () => {
    const fetchImpl = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(mockJSONResponse(200, {
        tenantId: "ten_matrix",
        connectorId: "matrix-main",
        connectorKind: "matrix",
        displayName: "Matrix Main",
        status: "degraded",
        terminalState: "action-required",
        botCredentialState: "valid",
        homeserverState: "reachable",
        routePolicyState: "valid",
        deliveryEligible: false,
        homeserverBindingId: "matrix_hs_1",
        reasonCode: "blocked_route",
        redactionStatus: "redacted",
        homeserverBinding: {
          homeserverUrl: "https://matrix.example.org",
          botUserId: "@bot:example.org",
          authorizationState: "valid",
          homeserverCapabilityState: "valid",
          validatedAt: "2026-05-10T10:01:00Z",
          redactionStatus: "redacted"
        },
        routePolicy: {
          connectorId: "matrix-main",
          homeserverBindingId: "matrix_hs_1",
          selectedRooms: [{ conversationId: "!room:example.org", conversationType: "room", roomSelectionState: "selected", validationState: "valid", redactionStatus: "redacted" }],
          allowedDirectUsers: ["@alice:example.org"],
          roomInvocationGate: "bot_mention_or_command_required",
          configuredCommands: ["!kura"],
          encryptedRoomPolicy: "unsupported",
          validationState: "valid",
          validatedAt: "2026-05-10T10:01:00Z",
          redactionStatus: "redacted"
        }
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        smokeEvidenceId: "matrix_smoke_1",
        tenantId: "ten_matrix",
        connectorId: "matrix-main",
        homeserverBindingId: "matrix_hs_1",
        status: "skipped",
        authorizationMode: "unavailable",
        owner: "operator",
        reason: "safe_matrix_authorization_unavailable",
        remainingRisk: "No live Matrix hosted smoke was run in this release validation.",
        validatedAt: "2026-05-10T10:02:00Z",
        retentionExpiresAt: "2026-08-08T10:02:00Z",
        redactionStatus: "redacted",
        safeEvidence: { policy: "structured_skip" }
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        smokeEvidenceId: "matrix_smoke_live_1",
        tenantId: "ten_matrix",
        connectorId: "matrix-main",
        homeserverBindingId: "matrix_hs_1",
        status: "passed",
        authorizationMode: "fake_matrix",
        owner: "operator",
        reason: "healthy",
        validatedAt: "2026-05-10T10:03:00Z",
        retentionExpiresAt: "2026-08-08T10:03:00Z",
        redactionStatus: "redacted",
        safeEvidence: { mode: "fake" }
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        tenantId: "ten_matrix",
        connectorId: "matrix-main",
        items: [{
          conformanceResultId: "conf_matrix_1",
          tenantId: "ten_matrix",
          connectorKind: "matrix",
          connectorId: "matrix-main",
          scenarioId: "matrix_hosted_setup",
          area: "tenant_ownership",
          result: "pass",
          redactionStatus: "redacted",
          evidenceTimestamp: "2026-05-10T10:02:00Z",
          retentionExpiresAt: "2026-08-08T10:02:00Z"
        }]
      }));

    const client = createKuraClient({
      baseURL: "http://127.0.0.1:19192",
      accessToken: "token",
      defaultTenantId: "ten_matrix",
      fetchImpl
    });

    const setup = await client.getMatrixSetup(" matrix-main ");
    const smoke = await client.getMatrixSmokeEvidence("matrix-main");
    const liveSmoke = await client.getMatrixLiveValidationSmokeEvidence("matrix-main");
    const conformance = await client.getMatrixConformanceEvidence("matrix-main");

    expect(setup.connectorKind).toBe("matrix");
    expect(setup.routePolicy?.encryptedRoomPolicy).toBe("unsupported");
    expect(smoke.authorizationMode).toBe("unavailable");
    expect(liveSmoke.authorizationMode).toBe("fake_matrix");
    expect(conformance.items[0]?.connectorKind).toBe("matrix");
    expect(fetchImpl).toHaveBeenNthCalledWith(1, "http://127.0.0.1:19192/v1/connectors/matrix-main/matrix-setup", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(2, "http://127.0.0.1:19192/v1/connectors/matrix-main/matrix-smoke", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(3, "http://127.0.0.1:19192/v1/live-validations/matrix-smoke?connectorId=matrix-main", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(4, "http://127.0.0.1:19192/v1/live-validations/matrix-conformance?connectorId=matrix-main", expect.anything());
  });

  it("calls integration diagnostic inspection routes", async () => {
    const fetchImpl = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(mockJSONResponse(200, {
        integrationId: "integration_feishu",
        tenantId: "ten_diag",
        freshnessSummary: "latest diagnostic state",
        items: [{
          diagnosticResultId: "diag_result_1",
          tenantId: "ten_diag",
          integrationId: "integration_feishu",
          domainKind: "calendar",
          providerKind: "feishu_lark",
          capability: "calendar.read",
          status: "blocked",
          reasonCode: "scope_missing",
          remediationOwner: "tenant_admin",
          retrySafety: "blocked",
          checkedAt: "2026-04-30T10:00:00Z",
          staleAfter: "2026-04-30T10:15:00Z",
          freshnessState: "fresh",
          redactionStatus: "redacted",
          retentionExpiresAt: "2026-07-29T10:00:00Z"
        }]
      }))
      .mockResolvedValueOnce(mockJSONResponse(201, {
        diagnosticRunId: "diag_run_client_key",
        tenantId: "ten_diag",
        integrationId: "integration_feishu",
        domainKind: "calendar",
        providerKind: "feishu_lark",
        requestedBy: "prn_operator",
        trigger: "operator_inspection",
        status: "completed",
        startedAt: "2026-04-30T10:00:00Z",
        completedAt: "2026-04-30T10:00:01Z",
        checkedCapabilities: ["calendar.read"],
        resultIds: ["diag_result_1"],
        redactionStatus: "redacted",
        retentionExpiresAt: "2026-07-29T10:00:00Z",
        idempotencyKey: "client-key"
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        items: [{
          diagnosticRunId: "diag_run_client_key",
          tenantId: "ten_diag",
          integrationId: "integration_feishu",
          requestedBy: "prn_operator",
          trigger: "operator_inspection",
          status: "completed",
          startedAt: "2026-04-30T10:00:00Z",
          checkedCapabilities: ["calendar.read"],
          resultIds: ["diag_result_1"],
          redactionStatus: "redacted",
          retentionExpiresAt: "2026-07-29T10:00:00Z"
        }]
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        diagnosticRunId: "diag_run_client_key",
        tenantId: "ten_diag",
        integrationId: "integration_feishu",
        requestedBy: "prn_operator",
        trigger: "operator_inspection",
        status: "completed",
        startedAt: "2026-04-30T10:00:00Z",
        checkedCapabilities: ["calendar.read"],
        resultIds: ["diag_result_1"],
        redactionStatus: "redacted",
        retentionExpiresAt: "2026-07-29T10:00:00Z"
      }))
      .mockResolvedValueOnce(mockJSONResponse(201, {
        smokeReportId: "smoke_feishu_probe",
        tenantId: "ten_diag",
        reportKind: "diagnostic",
        requestedBy: "prn_operator",
        status: "failed",
        domainSummary: { calendar: "failed" },
        startedAt: "2026-04-30T10:00:00Z",
        completedAt: "2026-04-30T10:01:00Z",
        publishedAt: "2026-04-30T10:01:00Z",
        artifactRefs: ["probe:integration_feishu:inspect"],
        retentionExpiresAt: "2026-07-29T10:00:00Z",
        probeOutcomes: [{
          probeOutcomeId: "probe_1",
          tenantId: "ten_diag",
          smokeReportId: "smoke_feishu_probe",
          integrationId: "integration_feishu",
          domainKind: "calendar",
          providerKind: "feishu_lark",
          probeAction: "calendar.read",
          result: "failed",
          reasonCode: "scope_missing",
          remediationHint: "Ask a tenant administrator to grant the missing provider scope.",
          retrySafety: "blocked",
          checkedAt: "2026-04-30T10:00:00Z",
          redactionStatus: "redacted",
          retentionExpiresAt: "2026-07-29T10:00:00Z"
        }]
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        items: [{
          retentionRecordId: "retention_1",
          tenantId: "ten_diag",
          targetKind: "diagnostic_run",
          targetId: "diag_run_client_key",
          defaultExpiresAt: "2026-07-29T10:00:00Z",
          effectiveExpiresAt: "2026-07-29T10:00:00Z",
          retentionState: "expired",
          appliedAt: "2026-07-30T10:00:00Z",
          createdAt: "2026-04-30T10:00:00Z",
          updatedAt: "2026-07-30T10:00:00Z"
        }]
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        items: [{
          reasonCode: "scope_missing",
          category: "scope",
          defaultRetrySafety: "blocked",
          defaultRemediationOwner: "tenant_admin",
          userMessageKey: "integration.diagnostic.scope_missing",
          operatorMessageKey: "integration.diagnostic.scope_missing"
        }]
      }));

    const client = createKuraClient({
      baseURL: "http://127.0.0.1:19192",
      accessToken: "token",
      defaultTenantId: "ten_diag",
      fetchImpl
    });

    const diagnostics = await client.listIntegrationDiagnostics(" integration_feishu ", { limit: 1 });
    const run = await client.startIntegrationDiagnosticRun("integration_feishu", { clientKey: "client-key", capabilities: ["calendar.read"] });
    const runs = await client.listIntegrationDiagnosticRuns({ integrationId: "integration_feishu" });
    const runDetail = await client.getIntegrationDiagnosticRun("diag_run_client_key");
    const smoke = await client.createIntegrationDiagnosticSmoke({
      reportId: "smoke_feishu_probe",
      integrationId: "integration_feishu",
      probes: [{
        domainKind: "calendar",
        probeAction: "calendar.read",
        safeCredentialsAvailable: true,
        tenantApprovalAvailable: true,
        providerAvailable: true,
        supported: true,
        readOnlyOrReversible: true,
        providerEvidence: { code: "scope_not_granted" }
      }]
    });
    const retention = await client.applyIntegrationDiagnosticRetention({ limit: 10 });
    const reasonCodes = await client.listIntegrationDiagnosticReasonCodes();

    expect(diagnostics.items[0].reasonCode).toBe("scope_missing");
    expect(run.status).toBe("completed");
    expect(runs.items).toHaveLength(1);
    expect(runDetail.diagnosticRunId).toBe("diag_run_client_key");
    expect(smoke.probeOutcomes?.[0]?.reasonCode).toBe("scope_missing");
    expect(retention.items[0].retentionState).toBe("expired");
    expect(reasonCodes.items[0].defaultRemediationOwner).toBe("tenant_admin");
    expect(fetchImpl).toHaveBeenNthCalledWith(1, "http://127.0.0.1:19192/v1/integrations/integration_feishu/diagnostics?limit=1", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(2, "http://127.0.0.1:19192/v1/integrations/integration_feishu/diagnostics/runs", expect.objectContaining({ method: "POST" }));
    expect(fetchImpl).toHaveBeenNthCalledWith(3, "http://127.0.0.1:19192/v1/integration-diagnostics/runs?integrationId=integration_feishu", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(4, "http://127.0.0.1:19192/v1/integration-diagnostics/runs/diag_run_client_key", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(5, "http://127.0.0.1:19192/v1/integration-diagnostics/smoke", expect.objectContaining({ method: "POST" }));
    expect(fetchImpl).toHaveBeenNthCalledWith(6, "http://127.0.0.1:19192/v1/integration-diagnostics/retention/apply?limit=10", expect.objectContaining({ method: "POST" }));
    expect(fetchImpl).toHaveBeenNthCalledWith(7, "http://127.0.0.1:19192/v1/integration-diagnostics/reason-codes", expect.anything());
  });

  it("calls evaluation replay and comparison surfaces", async () => {
    const fetchImpl = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(mockJSONResponse(200, {
        environmentScope: "test",
        items: [{ candidateId: "candidate_1", candidateKind: "fixture", displayName: "Fixture", sourceKind: "fixture", sourceId: "fixture_1", sourceRefs: [], toolClasses: ["daemon.inspection.read"], environmentScope: "test", readinessStatus: "fully_replayable", readinessReasons: [], limitations: [], defaultReplayMode: "non_live", createdAt: "2026-04-24T10:00:00Z", updatedAt: "2026-04-24T10:00:00Z" }]
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        candidateId: "candidate_1", candidateKind: "fixture", displayName: "Fixture", sourceKind: "fixture", sourceId: "fixture_1", sourceRefs: [], toolClasses: ["daemon.inspection.read"], environmentScope: "test", readinessStatus: "fully_replayable", readinessReasons: [], limitations: [], defaultReplayMode: "non_live", createdAt: "2026-04-24T10:00:00Z", updatedAt: "2026-04-24T10:00:00Z"
      }))
      .mockResolvedValueOnce(mockJSONResponse(201, {
        candidateId: "candidate_curated", candidateKind: "curated_work", displayName: "Curated", sourceKind: "run", sourceId: "run_1", sourceRefs: [], toolClasses: ["daemon.inspection.read"], environmentScope: "test", readinessStatus: "partially_replayable", readinessReasons: ["curated"], limitations: ["evidence-only"], defaultReplayMode: "non_live", createdAt: "2026-04-24T10:00:00Z", updatedAt: "2026-04-24T10:00:00Z"
      }))
      .mockResolvedValueOnce(mockJSONResponse(202, {
        attemptId: "attempt_1", candidateId: "candidate_1", sourceRefs: [], environmentScope: "test", mode: "non_live", status: "completed", safetyScope: { mode: "non_live" }, approvalHandling: "evidence_only", sideEffectHandling: "evidence_only", evidenceRefs: [], blockedReasons: [], createdAt: "2026-04-24T10:00:00Z", updatedAt: "2026-04-24T10:00:00Z"
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        environmentScope: "test",
        items: []
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        attemptId: "attempt_1", candidateId: "candidate_1", sourceRefs: [], environmentScope: "test", mode: "non_live", status: "completed", safetyScope: { mode: "non_live" }, approvalHandling: "evidence_only", sideEffectHandling: "evidence_only", evidenceRefs: [], blockedReasons: [], createdAt: "2026-04-24T10:00:00Z", updatedAt: "2026-04-24T10:00:00Z"
      }))
      .mockResolvedValueOnce(mockJSONResponse(201, {
        comparisonId: "comparison_1", candidateId: "candidate_1", baselineRef: "fixture_1", attemptId: "attempt_1", environmentScope: "test", terminalStatus: "matched", runtimeSummary: "runtime", policySummary: "policy", integrationSummary: "integration", deliverySummary: "delivery", evidenceSummary: "evidence", confidence: "high", limitations: [], driftFindings: [], generatedAt: "2026-04-24T10:00:00Z"
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        environmentScope: "test",
        items: []
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        comparisonId: "comparison_1", candidateId: "candidate_1", baselineRef: "fixture_1", attemptId: "attempt_1", environmentScope: "test", terminalStatus: "matched", runtimeSummary: "runtime", policySummary: "policy", integrationSummary: "integration", deliverySummary: "delivery", evidenceSummary: "evidence", confidence: "high", limitations: [], driftFindings: [], generatedAt: "2026-04-24T10:00:00Z"
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        environmentScope: "test",
        items: []
      }));

    const client = createKuraClient({
      baseURL: "http://127.0.0.1:19192/",
      accessToken: "token",
      fetchImpl
    });

    await client.listReplayCandidates({ readinessStatus: "fully_replayable", limit: 5 });
    await client.getReplayCandidate("candidate_1");
    await client.createReplayCandidate({
      candidateId: "candidate_curated",
      candidateKind: "curated_work",
      displayName: "Curated",
      sourceKind: "run",
      sourceId: "run_1",
      sourceRefs: [],
      toolClasses: ["daemon.inspection.read"],
      environmentScope: "test",
      readinessStatus: "partially_replayable",
      readinessReasons: ["curated"],
      limitations: ["evidence-only"],
      defaultReplayMode: "non_live"
    });
    await client.createReplayAttempt("candidate_1", { changeWindowLabel: "phase-33" });
    await client.listReplayAttempts({ candidateId: "candidate_1" });
    await client.getReplayAttempt("attempt_1");
    await client.createReplayComparison("attempt_1", { changeWindowLabel: "phase-33" });
    await client.listReplayComparisons({ terminalStatus: "matched" });
    await client.getReplayComparison("comparison_1");
    await client.listReplayFixtures({ domainClass: "schedule" });

    expect(fetchImpl).toHaveBeenNthCalledWith(1, "http://127.0.0.1:19192/v1/evaluation/replay-candidates?readinessStatus=fully_replayable&limit=5", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(2, "http://127.0.0.1:19192/v1/evaluation/replay-candidates/candidate_1", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(3, "http://127.0.0.1:19192/v1/evaluation/replay-candidates", expect.objectContaining({ method: "POST" }));
    expect(fetchImpl).toHaveBeenNthCalledWith(4, "http://127.0.0.1:19192/v1/evaluation/replay-candidates/candidate_1/attempts", expect.objectContaining({ method: "POST" }));
    expect(fetchImpl).toHaveBeenNthCalledWith(7, "http://127.0.0.1:19192/v1/evaluation/replay-attempts/attempt_1/compare", expect.objectContaining({ method: "POST" }));
    expect(fetchImpl).toHaveBeenNthCalledWith(10, "http://127.0.0.1:19192/v1/evaluation/fixtures?domainClass=schedule", expect.anything());
  });

  it("calls evaluation product discovery surfaces", async () => {
    const fetchImpl = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(mockJSONResponse(200, {
        tenantId: "ten_eval",
        page: { limit: 5 },
        items: [{ policyId: "policy_1", tenantId: "ten_eval", enabled: true, sourceKinds: ["run"], windowStart: "2026-04-29T09:00:00Z", windowEnd: "2026-04-29T10:00:00Z", maxInspectedRecords: 10, maxEmittedCandidates: 2, costBudget: 5, createdAt: "2026-04-29T10:00:00Z", updatedAt: "2026-04-29T10:00:00Z" }]
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        policyId: "policy_1", tenantId: "ten_eval", enabled: true, sourceKinds: ["run"], windowStart: "2026-04-29T09:00:00Z", windowEnd: "2026-04-29T10:00:00Z", maxInspectedRecords: 10, maxEmittedCandidates: 2, costBudget: 5, createdAt: "2026-04-29T10:00:00Z", updatedAt: "2026-04-29T10:00:00Z"
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        policyId: "policy_1", tenantId: "ten_eval", enabled: true, sourceKinds: ["run"], windowStart: "2026-04-29T09:00:00Z", windowEnd: "2026-04-29T10:00:00Z", maxInspectedRecords: 10, maxEmittedCandidates: 2, costBudget: 5, createdAt: "2026-04-29T10:00:00Z", updatedAt: "2026-04-29T10:00:00Z"
      }))
      .mockResolvedValueOnce(mockJSONResponse(202, {
        discoveryRunId: "discovery_run_1", tenantId: "ten_eval", policyId: "policy_1", status: "queued", sourceKinds: ["run"], windowStart: "2026-04-29T09:00:00Z", windowEnd: "2026-04-29T10:00:00Z", maxInspectedRecords: 10, maxEmittedCandidates: 2, costBudget: 5, inspectedRecords: 0, emittedCandidates: 0, startedAt: "2026-04-29T10:00:00Z", updatedAt: "2026-04-29T10:00:00Z", idempotencyKey: "idem_1"
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, { tenantId: "ten_eval", page: { limit: 5 }, items: [] }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        discoveryRunId: "discovery_run_1", tenantId: "ten_eval", policyId: "policy_1", status: "queued", sourceKinds: ["run"], windowStart: "2026-04-29T09:00:00Z", windowEnd: "2026-04-29T10:00:00Z", maxInspectedRecords: 10, maxEmittedCandidates: 2, costBudget: 5, inspectedRecords: 0, emittedCandidates: 0, startedAt: "2026-04-29T10:00:00Z", updatedAt: "2026-04-29T10:00:00Z"
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, { tenantId: "ten_eval", page: { limit: 5 }, items: [] }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        discoveredCandidateId: "candidate_1", tenantId: "ten_eval", discoveryRunId: "discovery_run_1", sourceKind: "run", sourceId: "run_1", score: 0.9, scoreBand: "high", redactionStatus: "redacted", readinessStatus: "fully_replayable", suppressionState: "none", retentionState: "active", createdAt: "2026-04-29T10:00:00Z", updatedAt: "2026-04-29T10:00:00Z"
      }))
      .mockResolvedValueOnce(mockJSONResponse(201, {
        suppressionId: "suppression_1", tenantId: "ten_eval", targetKind: "discovered_candidate", targetId: "candidate_1", reasonCode: "operator_hidden", createdAt: "2026-04-29T10:00:00Z", active: true
      }))
      .mockResolvedValueOnce(mockJSONResponse(201, productFixtureMutationFixture()))
      .mockResolvedValueOnce(mockJSONResponse(200, { tenantId: "ten_eval", page: { limit: 5 }, items: [productFixtureMutationFixture().fixture] }))
      .mockResolvedValueOnce(mockJSONResponse(200, productFixtureMutationFixture().fixture))
      .mockResolvedValueOnce(mockJSONResponse(200, { tenantId: "ten_eval", page: { limit: 5 }, items: [productFixtureMutationFixture().revision] }))
      .mockResolvedValueOnce(mockJSONResponse(201, productFixtureMutationFixture({ revisionId: "revision_2", revisionNumber: 2 })))
      .mockResolvedValueOnce(mockJSONResponse(200, productFixtureMutationFixture({ reviewState: "approved" })))
      .mockResolvedValueOnce(mockJSONResponse(200, productFixtureMutationFixture({ suppressionState: "suppressed" })));

    const client = createKuraClient({
      baseURL: "http://127.0.0.1:19192/",
      accessToken: "token",
      defaultTenantId: "ten_eval",
      fetchImpl
    });

    await client.listEvaluationDiscoveryPolicies({ enabled: true, limit: 5 });
    await client.getEvaluationDiscoveryPolicy("policy_1");
    await client.upsertEvaluationDiscoveryPolicy("policy_1", { enabled: true, sourceKinds: ["run"], windowStart: "2026-04-29T09:00:00Z", windowEnd: "2026-04-29T10:00:00Z", maxInspectedRecords: 10, maxEmittedCandidates: 2, costBudget: 5 });
    await client.startEvaluationDiscoveryRun({ policyId: "policy_1", idempotencyKey: "idem_1" });
    await client.listEvaluationDiscoveryRuns({ status: "queued", limit: 5 });
    await client.getEvaluationDiscoveryRun("discovery_run_1");
    await client.listEvaluationDiscoveredCandidates({ discoveryRunId: "discovery_run_1", scoreBand: "high" });
    await client.getEvaluationDiscoveredCandidate("candidate_1");
    await client.createEvaluationSuppression({ suppressionId: "suppression_1", targetKind: "discovered_candidate", targetId: "candidate_1", reasonCode: "operator_hidden" });
    await client.materializeProductFixture("candidate_1", { fixtureId: "product_fixture_1", displayName: "Product Fixture", domainClass: "schedule", fixturePayload: { goal: "safe" } });
    await client.listProductFixtures({ reviewState: "draft", limit: 5 });
    await client.getProductFixture("product_fixture_1");
    await client.listProductFixtureRevisions("product_fixture_1", { limit: 5 });
    await client.createProductFixtureRevision("product_fixture_1", { fixturePayload: { goal: "updated" } });
    await client.reviewProductFixture("product_fixture_1", { revisionId: "revision_2", decision: "approved" });
    await client.suppressProductFixture("product_fixture_1", { reasonCode: "operator_hidden" });

    expect(fetchImpl).toHaveBeenNthCalledWith(1, "http://127.0.0.1:19192/v1/evaluation/discovery-policies?enabled=true&limit=5", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(3, "http://127.0.0.1:19192/v1/evaluation/discovery-policies/policy_1", expect.objectContaining({ method: "PUT" }));
    expect(fetchImpl).toHaveBeenNthCalledWith(4, "http://127.0.0.1:19192/v1/evaluation/discovery-runs", expect.objectContaining({ method: "POST" }));
    expect(fetchImpl).toHaveBeenNthCalledWith(7, "http://127.0.0.1:19192/v1/evaluation/discovered-candidates?discoveryRunId=discovery_run_1&scoreBand=high", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(9, "http://127.0.0.1:19192/v1/evaluation/suppressions", expect.objectContaining({ method: "POST" }));
    expect(fetchImpl).toHaveBeenNthCalledWith(10, "http://127.0.0.1:19192/v1/evaluation/discovered-candidates/candidate_1/product-fixtures", expect.objectContaining({ method: "POST" }));
    expect(fetchImpl).toHaveBeenNthCalledWith(11, "http://127.0.0.1:19192/v1/evaluation/product-fixtures?reviewState=draft&limit=5", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(14, "http://127.0.0.1:19192/v1/evaluation/product-fixtures/product_fixture_1/revisions", expect.objectContaining({ method: "POST" }));
    expect(fetchImpl).toHaveBeenNthCalledWith(15, "http://127.0.0.1:19192/v1/evaluation/product-fixtures/product_fixture_1/review", expect.objectContaining({ method: "POST" }));
    expect(fetchImpl).toHaveBeenNthCalledWith(16, "http://127.0.0.1:19192/v1/evaluation/product-fixtures/product_fixture_1/suppress", expect.objectContaining({ method: "POST" }));
  });

  it("calls evaluation campaign dashboard and tool-call inspection surfaces", async () => {
    const fetchImpl = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(mockJSONResponse(201, campaignFixture()))
      .mockResolvedValueOnce(mockJSONResponse(200, { tenantId: "ten_eval", page: { limit: 5 }, items: [campaignFixture()] }))
      .mockResolvedValueOnce(mockJSONResponse(200, campaignFixture()))
      .mockResolvedValueOnce(mockJSONResponse(200, campaignFixture({ status: "running" })))
      .mockResolvedValueOnce(mockJSONResponse(200, campaignFixture({ status: "cancelled" })))
      .mockResolvedValueOnce(mockJSONResponse(200, campaignFixture({ status: "published" })))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        tenantId: "ten_eval",
        page: { limit: 5 },
        items: [{ campaignItemId: "campaign_item_1", campaignId: "campaign_1", tenantId: "ten_eval", sourceType: "product_fixture", sourceId: "product_fixture_1", sourceSnapshot: { currentRevisionId: "revision_1" }, suppressionCheckedAt: "2026-04-29T10:00:00Z", createdAt: "2026-04-29T10:00:00Z" }]
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        tenantId: "ten_eval",
        page: { limit: 5 },
        items: [{ attemptGroupId: "attempt_group_1", campaignId: "campaign_1", campaignItemId: "campaign_item_1", tenantId: "ten_eval", replayAttemptIds: ["attempt_1"], comparisonIds: ["comparison_1"], liveValidationIds: ["ledger_1"], status: "completed", driftCount: 1, failureCount: 0, unsupportedCount: 0, operatorActionNeededCount: 1, createdAt: "2026-04-29T10:00:00Z", updatedAt: "2026-04-29T10:00:00Z" }]
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        tenantId: "ten_eval",
        page: { limit: 5 },
        items: [{ projectionId: "projection_1", tenantId: "ten_eval", windowStart: "2026-04-29T09:00:00Z", windowEnd: "2026-04-29T10:00:00Z", campaignStatusCounts: { completed: 1 }, driftSummary: { total: 1 }, generatedAt: "2026-04-29T10:00:00Z" }]
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        tenantId: "ten_eval",
        page: { limit: 5 },
        items: [{ inspectionId: "inspection_1", tenantId: "ten_eval", campaignId: "campaign_1", campaignItemId: "campaign_item_1", toolCallRef: "tool_call_1", originalEvidenceRef: "original_1", nonLiveReplayEvidenceRef: "replay_1", liveValidationLedgerRefs: ["ledger_1"], classification: "live_validation_completed", redactionStatus: "redacted", createdAt: "2026-04-29T10:00:00Z", updatedAt: "2026-04-29T10:00:00Z" }]
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, { inspectionId: "inspection_1", tenantId: "ten_eval", campaignId: "campaign_1", campaignItemId: "campaign_item_1", toolCallRef: "tool_call_1", classification: "live_validation_completed", redactionStatus: "redacted", createdAt: "2026-04-29T10:00:00Z", updatedAt: "2026-04-29T10:00:00Z" }));

    const client = createKuraClient({
      baseURL: "http://127.0.0.1:19192/",
      accessToken: "token",
      defaultTenantId: "ten_eval",
      fetchImpl
    });

    await client.createEvaluationCampaign({ campaignId: "campaign_1", displayName: "Campaign", sourceSelections: [{ sourceType: "product_fixture", sourceId: "product_fixture_1" }], startImmediately: true });
    await client.listEvaluationCampaigns({ limit: 5 });
    await client.getEvaluationCampaign("campaign_1");
    await client.startEvaluationCampaign("campaign_1");
    await client.cancelEvaluationCampaign("campaign_1");
    await client.publishEvaluationCampaignResults("campaign_1");
    await client.listEvaluationCampaignItems("campaign_1", { limit: 5 });
    await client.listEvaluationCampaignAttemptGroups("campaign_1", { limit: 5 });
    await client.listEvaluationDashboard({ limit: 5 });
    await client.listEvaluationToolCallInspections("campaign_1", { limit: 5 });
    await client.getEvaluationToolCallInspection("inspection_1");

    expect(fetchImpl).toHaveBeenNthCalledWith(1, "http://127.0.0.1:19192/v1/evaluation/campaigns", expect.objectContaining({ method: "POST" }));
    expect(fetchImpl).toHaveBeenNthCalledWith(2, "http://127.0.0.1:19192/v1/evaluation/campaigns?limit=5", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(4, "http://127.0.0.1:19192/v1/evaluation/campaigns/campaign_1/start", expect.objectContaining({ method: "POST" }));
    expect(fetchImpl).toHaveBeenNthCalledWith(6, "http://127.0.0.1:19192/v1/evaluation/campaigns/campaign_1/publish-results", expect.objectContaining({ method: "POST" }));
    expect(fetchImpl).toHaveBeenNthCalledWith(7, "http://127.0.0.1:19192/v1/evaluation/campaigns/campaign_1/items?limit=5", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(8, "http://127.0.0.1:19192/v1/evaluation/campaigns/campaign_1/attempt-groups?limit=5", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(9, "http://127.0.0.1:19192/v1/evaluation/dashboard?limit=5", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(10, "http://127.0.0.1:19192/v1/evaluation/campaigns/campaign_1/tool-call-inspections?limit=5", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(11, "http://127.0.0.1:19192/v1/evaluation/tool-call-inspections/inspection_1", expect.anything());
  });

  it("calls live validation surfaces", async () => {
    const attempt = {
      validationId: "lv_1",
      candidateId: "candidate_1",
      requestedBy: "prn_1",
      environmentScope: "test",
      requestedScope: {
        scopeId: "scope_1",
        validationId: "lv_1",
        includedToolClasses: ["daemon.inspection.read"],
        approvalMode: "scope_level",
        declaredBy: "prn_1",
        declaredAt: "2026-04-29T10:00:00Z"
      },
      status: "awaiting_approval",
      permissionDecision: { allowed: true, checkedAt: "2026-04-29T10:00:00Z" },
      quotaDecision: { allowed: true, checkedAt: "2026-04-29T10:00:00Z" },
      killSwitchDecision: { allowed: true, checkedAt: "2026-04-29T10:00:00Z" },
      approvalSummary: { required: 1, approved: 0, denied: 0, expired: 0, pending: 1 },
      ledgerSummary: {},
      createdAt: "2026-04-29T10:00:00Z",
      updatedAt: "2026-04-29T10:00:00Z"
    };
    const fetchImpl = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(mockJSONResponse(202, { attempt }))
      .mockResolvedValueOnce(mockJSONResponse(200, { items: [attempt], environmentScope: "test" }))
      .mockResolvedValueOnce(mockJSONResponse(200, attempt))
      .mockResolvedValueOnce(mockJSONResponse(200, { ...attempt, status: "aborted" }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
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
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, { validationId: "lv_1", items: [{ ledgerEntryId: "ledger_1", validationId: "lv_1", candidateId: "candidate_1", sourceRef: "tool_1", toolClass: "mail.send", safetyClass: "non_idempotent_mutation", actionRef: "send_1", outcome: "operator_action_needed", updatedAt: "2026-04-29T10:00:00Z", retryCount: 0, ambiguousCommit: true }] }))
      .mockResolvedValueOnce(mockJSONResponse(200, { reconciliationId: "rec_1", ambiguousCommitId: "amb_1", resolvedBy: "prn_admin", resolution: "confirmed_committed", reason: "checked", resolvedAt: "2026-04-29T10:01:00Z" }))
      .mockResolvedValueOnce(mockJSONResponse(200, { policyId: "ret_1", appliesTo: "all", mode: "indefinite", createdByPrincipalId: "prn_admin", createdAt: "2026-04-29T10:00:00Z" }))
      .mockResolvedValueOnce(mockJSONResponse(202, { comparisonId: "cmp_1", validationId: "lv_1", candidateId: "candidate_1", baselineRef: "attempt_1", terminalStatus: "operator_action_needed", ledgerSummary: { operator_action_needed: 1 }, generatedAt: "2026-04-29T10:02:00Z" }))
      .mockResolvedValueOnce(mockJSONResponse(200, { items: [{ killSwitchId: "kill_1", scope: "tenant", tenantId: "ten_1", enabled: true, reason: "containment", changedBy: "prn_admin", changedAt: "2026-04-29T10:00:00Z" }] }))
      .mockResolvedValueOnce(mockJSONResponse(200, { killSwitchId: "kill_1", scope: "tenant", tenantId: "ten_1", enabled: true, reason: "containment", changedBy: "prn_admin", changedAt: "2026-04-29T10:00:00Z" }));
    const client = createKuraClient({
      baseURL: "http://127.0.0.1:19192/",
      accessToken: "token",
      fetchImpl
    });

    await client.startLiveValidation({
      candidateId: "candidate_1",
      candidateToolClasses: ["daemon.inspection.read"],
      requestedScope: {
        scopeId: "scope_1",
        validationId: "lv_1",
        includedToolClasses: ["daemon.inspection.read"],
        approvalMode: "scope_level",
        declaredBy: "prn_1",
        declaredAt: "2026-04-29T10:00:00Z"
      }
    });
    await client.listLiveValidations({ status: "awaiting_approval", limit: 5 });
    await client.getLiveValidation("lv_1");
    await client.abortLiveValidation("lv_1");
    const matrix = await client.listLiveValidationSupportMatrix();
    await client.listLiveValidationLedger("lv_1", { outcome: "operator_action_needed" });
    await client.resolveLiveValidationReconciliation("lv_1", "amb_1", { resolution: "confirmed_committed", reason: "checked" });
    await client.getLiveValidationRetention("lv_1");
    await client.createLiveValidationComparison("lv_1");
    await client.listLiveValidationKillSwitches({ scope: "tenant" });
    await client.updateLiveValidationKillSwitch({ scope: "tenant", enabled: true, reason: "containment" });

    expect(fetchImpl).toHaveBeenNthCalledWith(1, "http://127.0.0.1:19192/v1/live-validations", expect.objectContaining({ method: "POST" }));
    expect(JSON.parse(String(fetchImpl.mock.calls[0]?.[1]?.body))).toMatchObject({ candidateToolClasses: ["daemon.inspection.read"] });
    expect(fetchImpl).toHaveBeenNthCalledWith(2, "http://127.0.0.1:19192/v1/live-validations?status=awaiting_approval&limit=5", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(3, "http://127.0.0.1:19192/v1/live-validations/lv_1", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(4, "http://127.0.0.1:19192/v1/live-validations/lv_1/abort", expect.objectContaining({ method: "POST" }));
    expect(fetchImpl).toHaveBeenNthCalledWith(5, "http://127.0.0.1:19192/v1/live-validations/support-matrix", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(6, "http://127.0.0.1:19192/v1/live-validations/lv_1/ledger?outcome=operator_action_needed", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(7, "http://127.0.0.1:19192/v1/live-validations/lv_1/reconciliations/amb_1/resolve", expect.objectContaining({ method: "POST" }));
    expect(fetchImpl).toHaveBeenNthCalledWith(8, "http://127.0.0.1:19192/v1/live-validations/lv_1/retention", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(9, "http://127.0.0.1:19192/v1/live-validations/lv_1/compare", expect.objectContaining({ method: "POST" }));
    expect(fetchImpl).toHaveBeenNthCalledWith(10, "http://127.0.0.1:19192/v1/live-validations/kill-switches?scope=tenant", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(11, "http://127.0.0.1:19192/v1/live-validations/kill-switches", expect.objectContaining({ method: "POST" }));
    expect(matrix.items[0].safetyClass).toBe("unsupported");
  });

  it("streams chat events until terminal response", async () => {
    const encoder = new TextEncoder();
    const body = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(encoder.encode("event: chat.query.started\ndata: {\"dispatchId\":\"dispatch_1\",\"provider\":\"openai_compatible\",\"model\":\"gpt-test\",\"skills\":[\"shared\"],\"query\":\"hello\"}\n\n"));
        controller.enqueue(encoder.encode("event: chat.query.delta\ndata: {\"dispatchId\":\"dispatch_1\",\"delta\":\"hel\",\"reply\":\"hel\"}\n\n"));
        controller.enqueue(encoder.encode("event: chat.query.delta\ndata: {\"dispatchId\":\"dispatch_1\",\"delta\":\"lo\",\"reply\":\"hello\"}\n\n"));
        controller.enqueue(encoder.encode("event: chat.query.completed\ndata: {\"dispatchId\":\"dispatch_1\",\"provider\":\"openai_compatible\",\"model\":\"gpt-test\",\"skills\":[\"shared\"],\"query\":\"hello\",\"status\":\"completed\",\"partial\":false,\"reply\":\"hello\",\"usage\":{\"inputTokens\":1,\"outputTokens\":1,\"totalTokens\":2}}\n\n"));
        controller.close();
      }
    });

    const deltas: string[] = [];
    const client = createKuraClient({
      baseURL: "http://127.0.0.1:19192",
      fetchImpl: async () =>
        new Response(body, {
          status: 200,
          headers: { "Content-Type": "text/event-stream" }
        })
    });

    const response = await client.streamChatQuery({ query: "hello" }, {
      onDelta(payload: { reply: string }) {
        deltas.push(payload.reply);
      }
    });

    expect(deltas).toEqual(["hel", "hello"]);
    expect(response.reply).toBe("hello");
  });

  it("streams daemon events and surfaces them to handlers", async () => {
    const encoder = new TextEncoder();
    const body = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(encoder.encode(": stream-open\n\n"));
        controller.enqueue(encoder.encode("id: 12\nevent: policy.approval_requested\ndata: {\"eventId\":\"evt_1\",\"sequence\":12,\"category\":\"policy\",\"name\":\"policy.approval_requested\",\"occurredAt\":\"2026-04-24T10:00:00Z\",\"scope\":{\"runId\":\"run_1\"},\"resource\":{\"kind\":\"approval\",\"id\":\"approval_1\"},\"payload\":{\"status\":\"pending\"}}\n\n"));
        controller.close();
      }
    });

    const seen: string[] = [];
    const client = createKuraClient({
      baseURL: "http://127.0.0.1:19192",
      accessToken: "token",
      fetchImpl: async () =>
        new Response(body, {
          status: 200,
          headers: { "Content-Type": "text/event-stream" }
        })
    });

    const subscription = client.streamEvents({ category: "policy", cursor: 10 }, {
      onEvent(event) {
        seen.push(`${event.name}:${event.sequence}`);
      }
    });

    await subscription.completed;
    expect(seen).toEqual(["policy.approval_requested:12"]);
  });

  it("maps error responses into KuraClientError", async () => {
    const client = createKuraClient({
      baseURL: "http://127.0.0.1:19192",
      fetchImpl: async () => mockJSONResponse(502, { error: "bad key", errorCode: "upstream_auth_failed" })
    });

    await expect(client.queryChat({ query: "hello" })).rejects.toMatchObject({
      name: "KuraClientError",
      status: 502,
      code: "upstream_auth_failed",
      message: "bad key"
    });
  });

  it("binds the default fetch implementation to the browser global", async () => {
    const originalFetch = globalThis.fetch;
    let observedThis: unknown;

    globalThis.fetch = function (this: unknown, input: string | URL | Request, init?: RequestInit): Promise<Response> {
      observedThis = this;
      return Promise.resolve(mockJSONResponse(200, {
        dispatchId: "dispatch_1",
        provider: "openai_compatible",
        model: "gpt-test",
        skills: [],
        query: "hello",
        status: "completed",
        partial: false,
        reply: "world",
        usage: { inputTokens: 1, outputTokens: 1, totalTokens: 2 }
      }));
    } as typeof fetch;

    try {
      const client = createKuraClient({
        baseURL: "http://127.0.0.1:19192"
      });

      await client.queryChat({ query: "hello" });
      expect(observedThis).toBe(globalThis);
    } finally {
      globalThis.fetch = originalFetch;
    }
  });

  it("propagates default tenant, per-request override, omitted tenant, and stream tenant headers", async () => {
    const observedHeaders: Array<Record<string, string>> = [];
    const encoder = new TextEncoder();
    const streamBody = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(encoder.encode("event: chat.query.completed\ndata: {\"dispatchId\":\"dispatch_1\",\"provider\":\"echo\",\"model\":\"echo\",\"skills\":[],\"query\":\"hello\",\"status\":\"completed\",\"partial\":false,\"reply\":\"ok\",\"usage\":{\"inputTokens\":1,\"outputTokens\":1,\"totalTokens\":2}}\n\n"));
        controller.close();
      }
    });

    const client = createKuraClient({
      baseURL: "http://127.0.0.1:19192",
      defaultTenantId: "ten_default",
      fetchImpl: async (_input, init) => {
        observedHeaders.push(init?.headers as Record<string, string>);
        if (observedHeaders.length === 3) {
          return new Response(streamBody, { status: 200, headers: { "Content-Type": "text/event-stream" } });
        }
        return mockJSONResponse(200, {
          environmentScope: "test",
          items: [],
          generatedAt: "2026-04-24T10:00:00Z"
        });
      }
    });

    await client.getActivity();
    await client.getActivity({}, { tenantId: "ten_override" });
    await client.streamChatQuery({ query: "hello" });

    const tenantlessClient = createKuraClient({
      baseURL: "http://127.0.0.1:19192",
      fetchImpl: async (_input, init) => {
        observedHeaders.push(init?.headers as Record<string, string>);
        return mockJSONResponse(200, { environmentScope: "test", items: [], generatedAt: "2026-04-24T10:00:00Z" });
      }
    });
    await tenantlessClient.getActivity();

    expect(observedHeaders[0]["X-Kura-Tenant-ID"]).toBe("ten_default");
    expect(observedHeaders[1]["X-Kura-Tenant-ID"]).toBe("ten_override");
    expect(observedHeaders[2]["X-Kura-Tenant-ID"]).toBe("ten_default");
    expect(observedHeaders[3]["X-Kura-Tenant-ID"]).toBeUndefined();
  });

  it("exports tenant helpers and maps stable tenant denial metadata", async () => {
    const tenant = tenantResource();
    const fetchImpl = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(mockJSONResponse(200, {
        token: { tokenId: "tok_1" },
        principal: {
          principalId: "prn_1",
          principalKind: "local_operator",
          displayName: "Local",
          status: "active",
          defaultTenantId: tenant.tenantId,
          createdAt: "2026-04-24T10:00:00Z",
          updatedAt: "2026-04-24T10:00:00Z"
        },
        defaultTenant: tenant,
        currentTenant: tenant,
        allowedTenants: [tenant],
        tokenGrants: [],
        permissions: ["tenant.manage"],
        tenantContext: {
          principalId: "prn_1",
          tokenId: "tok_1",
          tenantId: tenant.tenantId,
          tenantSource: "default",
          permissions: ["tenant.manage"],
          resolvedAt: "2026-04-24T10:00:00Z"
        }
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, { items: [tenant] }))
      .mockResolvedValueOnce(mockJSONResponse(200, { tenant, tenantContext: { principalId: "prn_1", tokenId: "tok_1", tenantId: tenant.tenantId, tenantSource: "explicit_header", permissions: ["tenant.manage"], resolvedAt: "2026-04-24T10:00:00Z" } }))
      .mockResolvedValueOnce(mockJSONResponse(403, { error: "tenant access denied", errorCode: "tenant_access_denied" }));

    const client = createKuraClient({
      baseURL: "http://127.0.0.1:19192",
      fetchImpl
    });

    await expect(client.getMe()).resolves.toMatchObject({ currentTenant: { tenantId: "ten_personal" } });
    await expect(client.listTenants()).resolves.toMatchObject({ items: [{ tenantId: "ten_personal" }] });
    await expect(client.getTenant("ten_personal", { tenantId: "ten_personal" })).resolves.toMatchObject({ tenant: { tenantId: "ten_personal" } });
    await expect(client.getTenant("ten_denied", { tenantId: "ten_denied" })).rejects.toMatchObject({
      name: "KuraClientError",
      status: 403,
      code: "tenant_access_denied",
      tenantDenied: true,
      denial: { errorCode: "tenant_access_denied" }
    });
  });

  it("calls membership helper routes with active tenant intent", async () => {
    const membership = membershipResource();
    const fetchImpl = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(mockJSONResponse(200, { items: [membership] }))
      .mockResolvedValueOnce(mockJSONResponse(200, { membership: { ...membership, role: "admin" } }))
      .mockResolvedValueOnce(mockJSONResponse(200, { membership: { ...membership, status: "removed" } }))
      .mockResolvedValueOnce(mockJSONResponse(200, { items: [membership] }));

    const client = createKuraClient({
      baseURL: "http://127.0.0.1:19192",
      fetchImpl
    });

    await client.listMemberships("ten_personal", {}, { tenantId: "ten_personal" });
    await client.updateMembershipRole("ten_personal", "mem_1", { role: "admin" }, { tenantId: "ten_personal" });
    await client.removeMembership("ten_personal", "mem_1", { tenantId: "ten_personal" });
    await client.listMemberships("ten_personal");

    expect(fetchImpl).toHaveBeenNthCalledWith(1, "http://127.0.0.1:19192/v1/tenants/ten_personal/memberships", expect.objectContaining({
      headers: expect.objectContaining({ "X-Kura-Tenant-ID": "ten_personal" })
    }));
    expect(fetchImpl).toHaveBeenNthCalledWith(2, "http://127.0.0.1:19192/v1/tenants/ten_personal/memberships/mem_1", expect.objectContaining({
      method: "PATCH",
      headers: expect.objectContaining({ "X-Kura-Tenant-ID": "ten_personal" })
    }));
    expect(fetchImpl).toHaveBeenNthCalledWith(3, "http://127.0.0.1:19192/v1/tenants/ten_personal/memberships/mem_1", expect.objectContaining({
      method: "DELETE",
      headers: expect.objectContaining({ "X-Kura-Tenant-ID": "ten_personal" })
    }));
    const fourthCall = fetchImpl.mock.calls[3][1];
    expect((fourthCall?.headers as Record<string, string>)["X-Kura-Tenant-ID"]).toBeUndefined();
  });

  it("calls tenant secret helper routes with redacted resource types", async () => {
    const secret = tenantSecretResource();
    const fetchImpl = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(mockJSONResponse(200, { items: [secret] }))
      .mockResolvedValueOnce(mockJSONResponse(200, { secret }))
      .mockResolvedValueOnce(mockJSONResponse(201, { secret }))
      .mockResolvedValueOnce(mockJSONResponse(200, { secret: { ...secret, displayName: "Updated Key" } }))
      .mockResolvedValueOnce(mockJSONResponse(200, { secret: { ...secret, activeVersionId: "secver_2" } }))
      .mockResolvedValueOnce(mockJSONResponse(200, { secret: { ...secret, status: "disabled", disabledReason: "operator_request" } }));

    const client = createKuraClient({
      baseURL: "http://127.0.0.1:19192",
      fetchImpl,
      defaultTenantId: "ten_personal"
    });

    await expect(client.listTenantSecrets()).resolves.toMatchObject({ items: [{ secretRef: "provider/api-key" }] });
    await expect(client.getTenantSecret("provider/api-key")).resolves.toMatchObject({ secret: { secretRef: "provider/api-key" } });
    await client.createTenantSecret({ secretRef: "provider/api-key", displayName: " Provider API key ", value: "raw-secret" });
    await client.updateTenantSecret("provider/api-key", { displayName: " Updated Key " });
    await client.rotateTenantSecret("provider/api-key", { value: "new-raw-secret" });
    await client.disableTenantSecret("provider/api-key", { disabledReason: " operator_request " });

    expect(fetchImpl).toHaveBeenNthCalledWith(2, "http://127.0.0.1:19192/v1/tenant-secrets/provider%2Fapi-key", expect.objectContaining({
      headers: expect.objectContaining({ "X-Kura-Tenant-ID": "ten_personal" })
    }));
    expect(fetchImpl).toHaveBeenNthCalledWith(3, "http://127.0.0.1:19192/v1/tenant-secrets", expect.objectContaining({
      method: "POST",
      body: JSON.stringify({ secretRef: "provider/api-key", displayName: "Provider API key", value: "raw-secret" })
    }));
    expect(fetchImpl).toHaveBeenNthCalledWith(5, "http://127.0.0.1:19192/v1/tenant-secrets/provider%2Fapi-key/rotate", expect.objectContaining({
      method: "POST",
      body: JSON.stringify({ value: "new-raw-secret" })
    }));
    expect(fetchImpl).toHaveBeenNthCalledWith(6, "http://127.0.0.1:19192/v1/tenant-secrets/provider%2Fapi-key/disable", expect.objectContaining({
      method: "POST",
      body: JSON.stringify({ disabledReason: "operator_request" })
    }));
  });

  it("maps hosted credential stable denials", async () => {
    const fetchImpl = vi.fn<typeof fetch>().mockResolvedValueOnce(mockJSONResponse(403, {
      error: "credential_access_denied",
      reasonCode: "credential_denied:missing_permission"
    }));
    const client = createKuraClient({
      baseURL: "http://127.0.0.1:19192",
      fetchImpl
    });

    await expect(client.listTenantSecrets({ tenantId: "ten_personal" })).rejects.toMatchObject({
      name: "KuraClientError",
      status: 403,
      code: "credential_denied:missing_permission",
      tenantDenied: true
    });
  });

  it("calls setup wizard helper routes with redacted response resources and tenant intent", async () => {
    const readySession = setupSessionResource({
      state: "ready",
      retryable: false,
      remediationOwner: "none_required",
      safeUseMode: "normal",
      reasonCode: "healthy",
      diagnosticResultId: "diag_openai_setup",
      resourceRefs: [{ kind: "tenant_secret", id: "provider/openai-compatible" }],
      redactedEvidence: { secretRef: "provider/openai-compatible", secretVersionId: "secver_1" }
    });
    const oauthSession = setupSessionResource({
      setupSessionId: "setup_oauth_1",
      targetId: "integration.feishu_lark",
      targetKind: "integration",
      setupStyle: "oauth",
      oauthStateRef: "oauth_state_ref_1"
    });
    const fetchImpl = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(mockJSONResponse(200, { items: [{ targetId: "provider.openai_compatible", targetKind: "provider", setupStyle: "submitted_secret", displayName: "OpenAI-compatible provider", proofTarget: true, supportStatus: "supported" }] }))
      .mockResolvedValueOnce(mockJSONResponse(200, { items: [readySession] }))
      .mockResolvedValueOnce(mockJSONResponse(201, { session: setupSessionResource() }))
      .mockResolvedValueOnce(mockJSONResponse(200, { session: readySession }))
      .mockResolvedValueOnce(mockJSONResponse(200, { session: readySession }))
      .mockResolvedValueOnce(mockJSONResponse(200, { session: oauthSession, authorizationUrl: "https://oauth.example.test/authorize?state=opaque", state: "oauth_state_ref_1" }))
      .mockResolvedValueOnce(mockJSONResponse(200, { session: { ...oauthSession, state: "action_required", reasonCode: "oauth_denied" } }))
      .mockResolvedValueOnce(mockJSONResponse(200, { session: setupSessionResource({ state: "in_progress" }) }))
      .mockResolvedValueOnce(mockJSONResponse(200, { session: setupSessionResource({ state: "in_progress" }) }))
      .mockResolvedValueOnce(mockJSONResponse(200, { session: setupSessionResource({ state: "cancelled", reasonCode: "user_cancelled" }) }))
      .mockResolvedValueOnce(mockJSONResponse(200, { session: setupSessionResource({ state: "disabled", reasonCode: "disabled_by_user" }) }))
      .mockResolvedValueOnce(mockJSONResponse(200, { items: [{ setupSessionId: "setup_1", targetId: "provider.openai_compatible", diagnosticResultId: "diag_openai_setup", diagnosticRunId: "diag_run_openai_setup", diagnosticStage: "credential_probe", diagnosticSourceKind: "provider_check", diagnosticSourceId: "provider.openai_compatible", status: "ready", reasonCode: "healthy", retrySafety: "no_action_needed", remediationOwner: "none_required", checkedAt: "2026-05-06T00:01:00Z", staleAfter: "2026-05-06T00:11:00Z", redactionStatus: "redacted" }] }));

    const client = createKuraClient({
      baseURL: "http://127.0.0.1:19192",
      fetchImpl,
      defaultTenantId: "ten_personal"
    });

    await expect(client.listSetupTargets()).resolves.toMatchObject({ items: [{ targetId: "provider.openai_compatible" }] });
    await expect(client.listSetupSessions()).resolves.toMatchObject({ items: [{ setupSessionId: "setup_1", redactedEvidence: { secretVersionId: "secver_1" } }] });
    await client.startSetup({ targetId: " provider.openai_compatible ", setupStyle: "submitted_secret", source: " operator_shell " });
    await client.getSetupSession(" setup_1 ");
    await client.submitSetupSecret("setup_1", {
      secretRef: " provider/openai-compatible ",
      displayName: " Provider key ",
      value: "R46_FAKE_OPENAI_COMPATIBLE_KEY_DO_NOT_LEAK",
      resourceRefs: [{ kind: "telegram_allowment_validation", id: "direct_chat:chat_redacted" }]
    });
    await client.startSetupOAuth("setup_oauth_1", { redirectRoute: " /setup/oauth/feishu-lark/callback " });
    await client.completeSetupOAuth("setup_oauth_1", { state: " oauth_state_ref_1 ", result: "denied", accountLabel: " Workspace ", code: " code_1 ", redirectUri: " https://kura.test/callback " });
    await client.retrySetup("setup_1");
    await client.replaceSetup("setup_1", {}, { tenantId: "ten_personal" });
    await client.cancelSetup("setup_1");
    await client.disableSetup("setup_1", { disabledReason: " operator request " });
    await client.getSetupDiagnostics("setup_1");

    expect(fetchImpl).toHaveBeenNthCalledWith(3, "http://127.0.0.1:19192/v1/setup/sessions", expect.objectContaining({
      method: "POST",
      headers: expect.objectContaining({ "X-Kura-Tenant-ID": "ten_personal" }),
      body: JSON.stringify({ targetId: "provider.openai_compatible", setupStyle: "submitted_secret", source: "operator_shell" })
    }));
    expect(fetchImpl).toHaveBeenNthCalledWith(5, "http://127.0.0.1:19192/v1/setup/sessions/setup_1/submit-secret", expect.objectContaining({
      method: "POST",
      body: JSON.stringify({ secretRef: "provider/openai-compatible", value: "R46_FAKE_OPENAI_COMPATIBLE_KEY_DO_NOT_LEAK", displayName: "Provider key", resourceRefs: [{ kind: "telegram_allowment_validation", id: "direct_chat:chat_redacted" }] })
    }));
    expect(fetchImpl).toHaveBeenNthCalledWith(6, "http://127.0.0.1:19192/v1/setup/sessions/setup_oauth_1/oauth/start", expect.objectContaining({
      method: "POST",
      body: JSON.stringify({ redirectRoute: "/setup/oauth/feishu-lark/callback" })
    }));
    expect(fetchImpl).toHaveBeenNthCalledWith(7, "http://127.0.0.1:19192/v1/setup/sessions/setup_oauth_1/oauth/callback", expect.objectContaining({
      method: "POST",
      body: JSON.stringify({ state: "oauth_state_ref_1", result: "denied", accountLabel: "Workspace", code: "code_1", redirectUri: "https://kura.test/callback" })
    }));
    expect(fetchImpl).toHaveBeenNthCalledWith(11, "http://127.0.0.1:19192/v1/setup/sessions/setup_1/disable", expect.objectContaining({
      method: "POST",
      body: JSON.stringify({ disabledReason: "operator request" })
    }));
  });

  it("maps setup inspection denials without disclosing tenant credential existence", async () => {
    const fetchImpl = vi.fn<typeof fetch>().mockResolvedValueOnce(mockJSONResponse(403, {
      error: "setup permission denied",
      code: "setup_denied:missing_permission",
      reasonCode: "setup_denied:missing_permission",
      stage: "permission",
      retryable: false,
      remediationOwner: "tenant_admin"
    }));
    const client = createKuraClient({
      baseURL: "http://127.0.0.1:19192",
      fetchImpl
    });

    await expect(client.listSetupTargets({ tenantId: "ten_personal" })).rejects.toMatchObject({
      name: "KuraClientError",
      status: 403,
      code: "setup_denied:missing_permission",
      tenantDenied: true
    });
  });

  it("calls billing inspection and admin routes with tenant intent", async () => {
    const fetchImpl = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(mockJSONResponse(200, {
        planId: "plan_1",
        tenantId: "ten_personal",
        planKey: "finite",
        status: "active",
        enforcementMode: "enforced",
        effectiveAt: "2026-04-28T10:00:00Z"
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        tenantId: "ten_personal",
        planKey: "finite",
        enforcementMode: "enforced",
        quotas: []
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, { items: [] }))
      .mockResolvedValueOnce(mockJSONResponse(200, { items: [] }))
      .mockResolvedValueOnce(mockJSONResponse(200, billingQuotaDashboardFixture()))
      .mockResolvedValueOnce(mockJSONResponse(200, billingDenialDetailFixture({ classification: "abuse_restriction" })))
      .mockResolvedValueOnce(mockJSONResponse(200, billingEvidenceExportFixture()))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        planId: "plan_2",
        tenantId: "ten_personal",
        planKey: "unlimited",
        status: "active",
        enforcementMode: "unlimited",
        effectiveAt: "2026-04-28T10:01:00Z"
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        quotaOverrideId: "override_1",
        tenantId: "ten_personal",
        category: "run_launches",
        limit: 10,
        reason: "temporary increase"
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        adjustmentId: "adjustment_1",
        tenantId: "ten_personal",
        category: "run_launches",
        quotaPeriodId: "period_1",
        amountDelta: -1,
        reason: "operator correction",
        createdAt: "2026-04-28T10:02:00Z"
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        reservationId: "reservation_1",
        tenantId: "ten_personal",
        category: "run_launches",
        quotaPeriodId: "period_1",
        operationKey: "tenant:ten_personal:run:client_1",
        amountReserved: 1,
        amountCommitted: 0,
        amountRefunded: 1,
        status: "released",
        createdAt: "2026-04-28T10:00:00Z",
        updatedAt: "2026-04-28T10:03:00Z"
      }));

    const client = createKuraClient({
      baseURL: "http://127.0.0.1:19192/",
      defaultTenantId: "ten_personal",
      fetchImpl
    });

    await client.getBillingPlan();
    await client.getBillingUsage();
    await client.listBillingQuotas();
    await client.listBillingDenials();
    const dashboard = await client.getBillingQuotaDashboard({ tenantId: "ten_support" });
    const denial = await client.getBillingDenialDetail(" denial_1 ", { tenantId: "ten_support" });
    await client.exportBillingDenialEvidence(" denial_1 ", { tenantId: "ten_support" });
    await client.assignBillingPlan("ten_personal", { planKey: " unlimited ", enforcementMode: "unlimited", reason: " operator request " });
    await client.createBillingQuotaOverride("ten_personal", { category: "run_launches", limit: 10, reason: " temporary increase " });
    await client.createBillingManualAdjustment("ten_personal", { category: "run_launches", quotaPeriodId: " period_1 ", amountDelta: -1, reason: " operator correction " });
    await client.resolveBillingReservation("ten_personal", " reservation_1 ", { outcome: "released", reason: " operator verified no work started ", amount: 1 });
    expect(dashboard.sections[0]?.items[0]?.nearLimitReason).toBe("percent_threshold");
    expect(dashboard.sections[0]?.items[0]?.override?.effectiveLimit).toBe(8);
    expect(dashboard.sections[0]?.items[0]?.restriction?.visibleReasonCode).toBe("abuse_restriction:temporary");
    expect(denial.classification).toBe("abuse_restriction");

    expect(fetchImpl).toHaveBeenNthCalledWith(1, "http://127.0.0.1:19192/v1/billing/plan", expect.objectContaining({ headers: expect.objectContaining({ "X-Kura-Tenant-ID": "ten_personal" }) }));
    expect(fetchImpl).toHaveBeenNthCalledWith(2, "http://127.0.0.1:19192/v1/billing/usage", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(3, "http://127.0.0.1:19192/v1/billing/quotas", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(4, "http://127.0.0.1:19192/v1/billing/denials", expect.anything());
    expect(fetchImpl).toHaveBeenNthCalledWith(5, "http://127.0.0.1:19192/v1/billing/quota-dashboard", expect.objectContaining({ headers: expect.objectContaining({ "X-Kura-Tenant-ID": "ten_support" }) }));
    expect(fetchImpl).toHaveBeenNthCalledWith(6, "http://127.0.0.1:19192/v1/billing/denials/denial_1", expect.objectContaining({ headers: expect.objectContaining({ "X-Kura-Tenant-ID": "ten_support" }) }));
    expect(fetchImpl).toHaveBeenNthCalledWith(7, "http://127.0.0.1:19192/v1/billing/denials/denial_1/evidence-export", expect.objectContaining({ method: "POST", headers: expect.objectContaining({ "X-Kura-Tenant-ID": "ten_support" }) }));
    expect(fetchImpl).toHaveBeenNthCalledWith(8, "http://127.0.0.1:19192/v1/admin/billing/tenants/ten_personal/plan", expect.objectContaining({ method: "POST" }));
    expect(fetchImpl).toHaveBeenNthCalledWith(9, "http://127.0.0.1:19192/v1/admin/billing/tenants/ten_personal/quota-overrides", expect.objectContaining({ method: "POST" }));
    expect(fetchImpl).toHaveBeenNthCalledWith(10, "http://127.0.0.1:19192/v1/admin/billing/tenants/ten_personal/manual-adjustments", expect.objectContaining({ method: "POST" }));
    expect(fetchImpl).toHaveBeenNthCalledWith(11, "http://127.0.0.1:19192/v1/admin/billing/tenants/ten_personal/reservations/reservation_1/resolve", expect.objectContaining({ method: "POST" }));
  });

  it("calls activation routes with tenant headers and metadata-only responses", async () => {
    const fetchImpl = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(mockJSONResponse(200, activationResponseFixture()))
      .mockResolvedValueOnce(mockJSONResponse(200, activationResponseFixture({ status: "in_progress" })))
      .mockResolvedValueOnce(mockJSONResponse(200, {
        ...activationResponseFixture({ status: "first_action_completed", currentStepId: "completed" }),
        testChat: {
          dispatchId: "dispatch_1",
          status: "completed",
          provider: "test",
          model: "test-chat",
          finishReason: "stop",
          usage: {},
          completedAt: "2026-05-06T00:00:00Z"
        }
      }))
      .mockResolvedValueOnce(mockJSONResponse(200, {
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
      }));

    const client = createKuraClient({
      baseURL: "http://127.0.0.1:19192/",
      accessToken: "token",
      defaultTenantId: "ten_personal",
      fetchImpl
    });

    const current = await client.getActivation();
    const started = await client.activate({ source: " signup " });
    const testChat = await client.runActivationTestChat({ message: " Run a safe hosted activation test. " });
    const diagnostics = await client.getActivationDiagnostics();

    expect(fetchImpl).toHaveBeenNthCalledWith(1, "http://127.0.0.1:19192/v1/activation", expect.objectContaining({
      headers: expect.objectContaining({ Authorization: "Bearer token", "X-Kura-Tenant-ID": "ten_personal" })
    }));
    expect(fetchImpl).toHaveBeenNthCalledWith(2, "http://127.0.0.1:19192/v1/activation", expect.objectContaining({
      method: "POST",
      body: JSON.stringify({ source: "signup" })
    }));
    expect(fetchImpl).toHaveBeenNthCalledWith(3, "http://127.0.0.1:19192/v1/activation/test-chat", expect.objectContaining({
      method: "POST",
      body: JSON.stringify({ message: "Run a safe hosted activation test." })
    }));
    expect(fetchImpl).toHaveBeenNthCalledWith(4, "http://127.0.0.1:19192/v1/activation/diagnostics", expect.anything());
    expect(current.activation.status).toBe("active");
    expect(current.activation.quotaBaseline?.quotas[0].remaining).toBe(8);
    expect(started.activation.status).toBe("in_progress");
    expect(testChat.testChat.status).toBe("completed");
    expect(JSON.stringify(testChat)).not.toContain("Run a safe hosted activation test.");
    expect(diagnostics.items[0].reasonCode).toBe("activation_blocked:quota_baseline_unavailable");
  });

  it("maps activation blocked payloads into KuraClientError metadata", async () => {
    const fetchImpl = vi.fn<typeof fetch>().mockResolvedValueOnce(mockJSONResponse(403, {
      error: "quota baseline is unavailable",
      code: "activation_blocked:quota_baseline_unavailable",
      reasonCode: "activation_blocked:quota_baseline_unavailable",
      stage: "quota_baseline",
      retryable: true,
      remediationOwner: "operator"
    }));
    const client = createKuraClient({
      baseURL: "http://127.0.0.1:19192/",
      accessToken: "token",
      defaultTenantId: "ten_personal",
      fetchImpl
    });

    await expect(client.runActivationTestChat({ message: "safe test" })).rejects.toMatchObject({
      status: 403,
      code: "activation_blocked:quota_baseline_unavailable",
      activationFailure: {
        reasonCode: "activation_blocked:quota_baseline_unavailable",
        stage: "quota_baseline",
        retryable: true,
        remediationOwner: "operator"
      }
    });
  });

  it("maps quota denial payloads into KuraClientError", async () => {
    const fetchImpl = vi.fn<typeof fetch>().mockResolvedValueOnce(mockJSONResponse(429, {
      error: "quota exhausted",
      code: "quota_denied",
      reasonCode: "quota_denied:run_launches_exhausted",
      tenantId: "ten_personal",
      category: "run_launches",
      operationKey: "tenant:ten_personal:run:client_1",
      requestedAmount: 1,
      remainingAmount: 0,
      periodStart: "2026-04-01T00:00:00Z",
      periodEnd: "2026-05-01T00:00:00Z"
    }));
    const client = createKuraClient({
      baseURL: "http://127.0.0.1:19192",
      fetchImpl
    });

    await expect(client.createRun({ entrypoint: "operator.shell.test" })).rejects.toMatchObject({
      name: "KuraClientError",
      status: 429,
      code: "quota_denied:run_launches_exhausted",
      quotaDenial: {
        code: "quota_denied",
        reasonCode: "quota_denied:run_launches_exhausted",
        category: "run_launches",
        remainingAmount: 0
      }
    });
  });
});
