import { describe, expect, it, vi } from "vitest";

import { createKuraClient } from "./index";

function jsonResponse(payload: unknown): Response {
  return new Response(JSON.stringify(payload), {
    status: 200,
    headers: { "Content-Type": "application/json" }
  });
}

function errorResponse(status: number, payload: unknown): Response {
  return new Response(JSON.stringify(payload), {
    status,
    headers: { "Content-Type": "application/json" }
  });
}

describe("thread lifecycle SDK skeleton", () => {
  it("keeps the client factory importable", () => {
    expect(createKuraClient).toBeTypeOf("function");
  });

  it("routes list and get thread through tenant-scoped endpoints", async () => {
    const fetchImpl = vi.fn<typeof fetch>()
      .mockResolvedValueOnce(jsonResponse({ tenantId: "ten_threads", page: { limit: 2, order: "active_recent_archived_id" }, items: [] }))
      .mockResolvedValueOnce(jsonResponse({
        thread: { threadId: "thr_1", tenantId: "ten_threads", lifecycleState: "active", sourceKind: "channel", lastActivityAt: "2026-05-11T10:00:00Z", availableActions: ["reset"], redactionStatus: "redacted", retentionExpiresAt: "2026-08-09T10:00:00Z", updatedAt: "2026-05-11T10:00:00Z" },
        sessionSegments: [],
        sourceLinkages: [{ sourceLinkageId: "src_1", sourceKind: "channel", routingOutcome: "accepted", current: true, linkedAt: "2026-05-11T10:00:00Z", retentionExpiresAt: "2026-08-09T10:00:00Z", redactionStatus: "redacted" }],
        runtimeProjections: [{ runtimeProjectionId: "rtp_1", resourceKind: "run", resourceId: "run_1", status: "completed", occurredAt: "2026-05-11T10:00:00Z", safeSummary: "metadata only", retentionExpiresAt: "2026-08-09T10:00:00Z", redactionStatus: "redacted" }],
        conversationShape: { shape: "room", shapeEvidenceStatus: "proven", sourceConversationSummary: "Slack Main / #support", redactionStatus: "redacted" },
        lifecycleActions: []
      }));
    const client = createKuraClient({ baseURL: "http://127.0.0.1:19192", accessToken: "token", defaultTenantId: "ten_threads", fetchImpl });

    await client.listThreads({ limit: 2, state: "active", sourceKind: "channel" });
    const detail = await client.getThread(" thr_1 ");

    expect(fetchImpl.mock.calls[0]?.[0]).toBe("http://127.0.0.1:19192/v1/threads?limit=2&state=active&sourceKind=channel");
    expect(fetchImpl.mock.calls[1]?.[0]).toBe("http://127.0.0.1:19192/v1/threads/thr_1");
    expect(fetchImpl.mock.calls[0]?.[1]?.headers).toMatchObject({ Authorization: "Bearer token", "X-Kura-Tenant-ID": "ten_threads" });
    expect(detail.thread.retentionExpiresAt).toBe("2026-08-09T10:00:00Z");
    expect(detail.conversationShape?.shape).toBe("room");
    expect(detail.sourceLinkages[0]?.redactionStatus).toBe("redacted");
    expect(JSON.stringify(detail)).not.toMatch(/semanticSummary|recalledMemory|contextPacking|autonomousPruning/);
  });

  it("surfaces thread lifecycle permission denials", async () => {
    const fetchImpl = vi.fn<typeof fetch>().mockResolvedValue(errorResponse(403, { error: "permission_missing" }));
    const client = createKuraClient({ baseURL: "http://127.0.0.1:19192", accessToken: "token", defaultTenantId: "ten_threads", fetchImpl });

    await expect(client.listThreads()).rejects.toMatchObject({ status: 403 });
  });

  it("routes reset archive and reopen mutations with tenant headers", async () => {
    const mutation = { threadId: "thr_1", lifecycleState: "archived", auditEventId: "audit_1", changedAt: "2026-05-11T10:00:00Z", action: "archive", availableActions: ["reopen"] };
    const fetchImpl = vi.fn<typeof fetch>()
      .mockResolvedValueOnce(jsonResponse(mutation))
      .mockResolvedValueOnce(jsonResponse({ ...mutation, lifecycleState: "reset", action: "reset" }))
      .mockResolvedValueOnce(jsonResponse({ ...mutation, lifecycleState: "reopened", action: "reopen" }));
    const client = createKuraClient({ baseURL: "http://127.0.0.1:19192", accessToken: "token", defaultTenantId: "ten_threads", fetchImpl });

    await client.archiveThread("thr_1", { reasonCode: "operator_archive" });
    await client.resetThread("thr_1");
    await client.reopenThread("thr_1");

    expect(fetchImpl.mock.calls[0]?.[0]).toBe("http://127.0.0.1:19192/v1/threads/thr_1/archive");
    expect(fetchImpl.mock.calls[1]?.[0]).toBe("http://127.0.0.1:19192/v1/threads/thr_1/reset");
    expect(fetchImpl.mock.calls[2]?.[0]).toBe("http://127.0.0.1:19192/v1/threads/thr_1/reopen");
    expect(fetchImpl.mock.calls[0]?.[1]?.method).toBe("POST");
    expect(fetchImpl.mock.calls[0]?.[1]?.headers).toMatchObject({ Authorization: "Bearer token", "X-Kura-Tenant-ID": "ten_threads" });
  });

  it("sets and reads a session frame (Stage 5.1)", async () => {
    const frame = { threadId: "thr_1", goal: "ship it", constraints: ["cite"], updatedAt: "t" };
    const fetchImpl = vi.fn().mockResolvedValueOnce(jsonResponse(frame)).mockResolvedValueOnce(jsonResponse(frame));
    const client = createKuraClient({ baseURL: "https://daemon.test", fetchImpl });
    const set = await client.setSessionFrame("thr_1", { goal: "ship it", constraints: ["cite"] });
    expect(set.goal).toBe("ship it");
    expect(fetchImpl.mock.calls[0][1].method).toBe("PUT");
    expect(String(fetchImpl.mock.calls[0][0])).toContain("/v1/threads/thr_1/frame");
    const got = await client.getSessionFrame("thr_1");
    expect(got.constraints).toEqual(["cite"]);
  });
});

