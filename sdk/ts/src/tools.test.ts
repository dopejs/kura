import { describe, expect, it, vi } from "vitest";

import { createKuraClient } from "./index";

function jsonResponse(payload: unknown, status = 200): Response {
  return new Response(JSON.stringify(payload), { status, headers: { "Content-Type": "application/json" } });
}

const profile = {
  profileId: "tlp_1",
  tenantId: "ten_a",
  title: "web search",
  capability: "web.search",
  family: "builtin_stub",
  authMode: "api_key",
  source: "managed",
  enabled: true,
  isDefault: true,
  secretRef: "secret://tools/search-key",
  secretConfigured: true,
  limits: { timeoutMs: 20000, maxResults: 10, maxCallsPerDay: 0 },
  readiness: "ready",
  createdAt: "t",
  updatedAt: "t",
};

describe("tool provider SDK methods (Stage 9.1b)", () => {
  it("lists capabilities, creates a profile, and preflights it", async () => {
    const fetchImpl = vi
      .fn()
      .mockResolvedValueOnce(
        jsonResponse({
          items: [
            { capability: "web.search", configured: true, defaultProfileId: "tlp_1", profileCount: 1 },
            { capability: "image.generate", configured: false, profileCount: 0 },
          ],
        }),
      )
      .mockResolvedValueOnce(jsonResponse(profile, 201))
      .mockResolvedValueOnce(
        jsonResponse({ profileId: "tlp_1", passed: false, errorClass: "auth_error", checkedAt: "t" }),
      );
    const client = createKuraClient({ baseURL: "https://daemon.test", fetchImpl });

    const caps = await client.listToolCapabilities();
    // Unconfigured capabilities are reported, not omitted.
    expect(caps.items).toHaveLength(2);
    expect(caps.items[1].configured).toBe(false);

    const created = await client.createToolProfile({
      title: "web search",
      capability: "web.search",
      family: "builtin_stub",
      authMode: "api_key",
      secretRef: "secret://tools/search-key",
    });
    expect(created.secretConfigured).toBe(true);
    // The projection carries the reference, never a value.
    expect(created.secretRef).toBe("secret://tools/search-key");
    expect(Object.keys(created)).not.toContain("apiKey");

    const checked = await client.checkToolProfile("tlp_1");
    expect(checked.passed).toBe(false);
    expect(checked.errorClass).toBe("auth_error");
    expect(String(fetchImpl.mock.calls[2][0])).toContain("/v1/tools/profiles/tlp_1/check");
  });

  it("launches and reads a swarm run (Stage 4)", async () => {
    const run = { runId: "swm_1", requestedBy: "prn_1", status: "queued", children: [{ index: 0, goal: "a", threadId: "swarm:swm_1:0", status: "queued" }], createdAt: "t", updatedAt: "t" };
    const fetchImpl = vi.fn().mockResolvedValueOnce(jsonResponse(run, 202)).mockResolvedValueOnce(jsonResponse({ ...run, status: "completed" }));
    const client = createKuraClient({ baseURL: "https://daemon.test", fetchImpl });
    const launched = await client.launchSwarmRun({ goals: ["a"] });
    expect(launched.children[0].threadId).toBe("swarm:swm_1:0");
    expect(fetchImpl.mock.calls[0][1].method).toBe("POST");
    const got = await client.getSwarmRun("swm_1");
    expect(got.status).toBe("completed");
  });

  it("opens, lists and reads a work item (work items slice 1)", async () => {
    const item = {
      itemId: "wi_1",
      tenantId: "ten_a",
      title: "Fix CI",
      state: "open",
      stage: "working",
      threadId: "thr_wi_1",
      seq: 1,
      mandate: { basis: "api", strength: "explicit", policyHash: "", matchedRules: [], sender: {}, source: { kind: "api" }, scopeText: "s", createdAt: "t" },
      createdAt: "t",
      updatedAt: "t",
    };
    const fetchImpl = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse(item, 201))
      .mockResolvedValueOnce(jsonResponse({ items: [item] }))
      .mockResolvedValueOnce(jsonResponse({ ...item, stage: "person/blocked", lastReport: { status: "blocked", summary: "x", reason: "report_unreadable" } }));
    const client = createKuraClient({ baseURL: "https://daemon.test", fetchImpl });
    const opened = await client.createWorkItem({ title: "Fix CI", scope: "s" });
    expect(opened.stage).toBe("working");
    expect(fetchImpl.mock.calls[0][1].method).toBe("POST");
    await client.listWorkItems({ state: "open", limit: 10 });
    expect(String(fetchImpl.mock.calls[1][0])).toContain("/v1/work-items?state=open&limit=10");
    const got = await client.getWorkItem("wi_1");
    expect(got.lastReport?.reason).toBe("report_unreadable");
  });

  it("stops a work item and reads its subscriptions (work items slice 2)", async () => {
    const stopped = {
      itemId: "wi_1",
      tenantId: "ten_a",
      title: "Fix CI",
      state: "ended",
      endReason: "stopped",
      threadId: "thr_wi_1",
      seq: 1,
      mandate: { basis: "im_mention", strength: "explicit", policyHash: "", matchedRules: [], sender: { connectorId: "slack-main", authorId: "U1" }, source: { kind: "im", ref: "d1", conversation: "C1" }, scopeText: "s", createdAt: "t" },
      createdAt: "t",
      updatedAt: "t",
      subscriptions: [{ subscriptionId: "wis_1", sourceKind: "im", sourceKey: "thr_src_1", role: "owner", createdAt: "t", lastEventAt: "t", droppedAt: "t", dropReason: "stopped" }],
    };
    const fetchImpl = vi.fn().mockResolvedValueOnce(jsonResponse(stopped));
    const client = createKuraClient({ baseURL: "https://daemon.test", fetchImpl });
    const got = await client.stopWorkItem("wi_1");
    expect(String(fetchImpl.mock.calls[0][0])).toContain("/v1/work-items/wi_1/stop");
    expect(fetchImpl.mock.calls[0][1].method).toBe("POST");
    expect(got.endReason).toBe("stopped");
    expect(got.subscriptions?.[0].dropReason).toBe("stopped");
  });

  it("withdraws a queued reply from a work item's outbox (work items slice 3)", async () => {
    const item = {
      itemId: "wi_1",
      tenantId: "ten_a",
      title: "Fix CI",
      state: "open",
      stage: "idle",
      threadId: "thr_wi_1",
      seq: 1,
      mandate: { basis: "api", strength: "explicit", policyHash: "", matchedRules: [], sender: {}, source: { kind: "api" }, scopeText: "s", createdAt: "t" },
      createdAt: "t",
      updatedAt: "t",
      outbox: [{ seq: 2, conversationKey: "thr_src_1", connectorId: "slack-main", body: "b", state: "dropped", attempts: 0, dropReason: "withdrawn", createdAt: "t", droppedAt: "t" }],
    };
    const fetchImpl = vi.fn().mockResolvedValueOnce(jsonResponse(item));
    const client = createKuraClient({ baseURL: "https://daemon.test", fetchImpl });
    const got = await client.withdrawWorkItemReply("wi_1", 2);
    expect(String(fetchImpl.mock.calls[0][0])).toContain("/v1/work-items/wi_1/outbox/2/withdraw");
    expect(fetchImpl.mock.calls[0][1].method).toBe("POST");
    expect(got.outbox?.[0].dropReason).toBe("withdrawn");
  });

  it("lists the attention queue and answers a question (work items slice 5)", async () => {
    const queue = {
      items: [
        { itemId: "wi_1", title: "Deploy", stage: "person/approval", basis: "api", attention: { kind: "approval", question: "run advance", approvalId: "approval_1" }, updatedAt: "t" },
        { itemId: "wi_2", title: "Fix CI", stage: "person/answer", basis: "im_mention", attention: { kind: "question", question: "Pin or patch?", options: ["pin", "patch"] }, updatedAt: "t" },
      ],
    };
    const answered = {
      itemId: "wi_2",
      tenantId: "ten_a",
      title: "Fix CI",
      state: "open",
      stage: "queued",
      threadId: "thr_wi_2",
      seq: 1,
      mandate: { basis: "api", strength: "explicit", policyHash: "", matchedRules: [], sender: {}, source: { kind: "api" }, scopeText: "s", createdAt: "t" },
      createdAt: "t",
      updatedAt: "t",
    };
    const fetchImpl = vi.fn().mockResolvedValueOnce(jsonResponse(queue)).mockResolvedValueOnce(jsonResponse(answered));
    const client = createKuraClient({ baseURL: "https://daemon.test", fetchImpl });
    const listed = await client.listWorkItemAttention();
    expect(String(fetchImpl.mock.calls[0][0])).toContain("/v1/work-items/attention");
    expect(listed.items[0].attention?.approvalId).toBe("approval_1");
    const got = await client.answerWorkItem("wi_2", "pin");
    expect(String(fetchImpl.mock.calls[1][0])).toContain("/v1/work-items/wi_2/answer");
    expect(fetchImpl.mock.calls[1][1].method).toBe("POST");
    expect(JSON.parse(String(fetchImpl.mock.calls[1][1].body))).toEqual({ text: "pin" });
    expect(got.stage).toBe("queued");
  });

  it("searches skills and records feedback (Stage 3)", async () => {
    const usage = { skillId: "deploy-check", invocations: 2, helpful: 1, corrected: 0 };
    const fetchImpl = vi.fn()
      .mockResolvedValueOnce(jsonResponse({ items: [{ skillId: "deploy-check", name: "deploy-check", description: "d", rank: 1, usage }] }))
      .mockResolvedValueOnce(jsonResponse({ ...usage, corrected: 1 }));
    const client = createKuraClient({ baseURL: "https://daemon.test", fetchImpl });
    const hits = await client.searchSkills("deploy to production");
    expect(hits.items[0].rank).toBe(1);
    expect(String(fetchImpl.mock.calls[0][0])).toContain("/v1/skills/search?q=deploy%20to%20production");
    const fb = await client.recordSkillFeedback("deploy-check", "corrected");
    expect(fb.corrected).toBe(1);
  });
});

