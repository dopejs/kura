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

