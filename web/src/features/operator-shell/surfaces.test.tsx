import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import type {
  CatalogItemResource,
  EvidenceBundleResource,
  ExecutionProfileResource,
  RoutineResource,
  TriagePolicyResource,
  WebhookEndpointResource,
} from "@kura/client";

import {
  CatalogView,
  ExecutionProfilesView,
  RoutinesView,
  SupportEvidenceView,
  SurfacePanel,
  TriagePoliciesView,
  WebhooksView,
} from "./surfaces";

describe("operator shell product surface views (Roadmap 70)", () => {
  afterEach(() => cleanup());

  it("renders the denied state with a reason", () => {
    render(<SurfacePanel title="Routines" state="denied" reason="select a tenant" />);
    expect(screen.getByRole("status").textContent).toContain("Select a tenant");
    expect(screen.getByText("select a tenant")).toBeTruthy();
  });

  it("renders empty and unsupported states", () => {
    render(<SurfacePanel title="A" state="empty" />);
    expect(screen.getByRole("status").textContent).toContain("Nothing here yet");
  });

  it("renders routines with state + version", () => {
    const routines: RoutineResource[] = [{ routineId: "r1", name: "Daily", state: "active", currentVersion: 2, definition: { name: "Daily", trigger: { kind: "cron", cronExpr: "0 8 * * *" }, workflow: { goal: "summarize" } }, versions: [], createdAt: "t", updatedAt: "t" }];
    render(<RoutinesView routines={routines} state="ready" />);
    expect(screen.getByText("Daily")).toBeTruthy();
    expect(screen.getByText("v2")).toBeTruthy();
  });

  it("renders webhooks showing only the redacted secret fingerprint", () => {
    const endpoints: WebhookEndpointResource[] = [{ webhookId: "w1", name: "deploy", targetKind: "routine", targetRef: "r1", status: "active", secretFingerprint: "sha256:abc", createdAt: "t", updatedAt: "t" }];
    render(<WebhooksView endpoints={endpoints} state="ready" />);
    expect(screen.getByText("deploy")).toBeTruthy();
    expect(screen.getByText("sha256:abc")).toBeTruthy();
  });

  it("renders catalog items with trust tier", () => {
    const items: CatalogItemResource[] = [{ itemId: "c1", kind: "skill", name: "pdf-extract", trustTier: "verified", versions: [{ version: "1.0.0", source: "s", publishedAt: "t" }], createdAt: "t", updatedAt: "t" }];
    render(<CatalogView items={items} state="ready" />);
    expect(screen.getByText("pdf-extract")).toBeTruthy();
    expect(screen.getByText("verified")).toBeTruthy();
  });

  it("renders execution profiles with health + denial reason", () => {
    const profiles: ExecutionProfileResource[] = [{ profile: { profileId: "p1", name: "Docker", backendKind: "docker", riskTier: "medium", createdAt: "t" }, status: { profileId: "p1", health: "unavailable", reason: "docker daemon not running", available: false } }];
    render(<ExecutionProfilesView profiles={profiles} state="ready" />);
    expect(screen.getByText("Docker")).toBeTruthy();
    expect(screen.getByText("docker daemon not running")).toBeTruthy();
  });

  it("renders triage policies", () => {
    const policies: TriagePolicyResource[] = [{ policyId: "tp1", name: "inbox", rules: [], defaultClassification: "fyi", createdAt: "t", updatedAt: "t" }];
    render(<TriagePoliciesView policies={policies} state="ready" />);
    expect(screen.getByText("inbox")).toBeTruthy();
  });

  it("renders support evidence bundles with redaction status", () => {
    const bundles: EvidenceBundleResource[] = [{ bundleId: "b1", scope: { kind: "routine", ref: "r1" }, redactionStatus: "redacted", createdAt: "t", retentionExpiresAt: "t" }];
    render(<SupportEvidenceView bundles={bundles} state="ready" />);
    expect(screen.getByText("routine:r1")).toBeTruthy();
    expect(screen.getByText("redacted")).toBeTruthy();
  });
});

describe("plugin assembly surface (pluginization program)", () => {
  afterEach(() => cleanup());

  it("renders plugins with enablement, source, reason, hooks and warnings", async () => {
    const { PluginsView } = await import("./surfaces");
    const plugins = [
      { id: "memory", summary: "Layered memory plane", source: "builtin", enabled: true, provides: ["memory.manager"], requires: ["llm"] },
      { id: "webhooks", summary: "Webhook ingress", source: "builtin", enabled: false, reason: "requires disabled plugin `billing`", provides: [], requires: ["billing"] },
    ];
    const toggles: Array<[string, boolean]> = [];
    render(
      <PluginsView
        plugins={plugins}
        hooks={[{ point: "chat/turn-end", pluginId: "memory" }]}
        warnings={["profile disables unknown plugin `ghost`"]}
        state="ready"
        restartRequired
        onToggle={(id, enable) => toggles.push([id, enable])}
      />,
    );
    expect(screen.getAllByText("memory")).toHaveLength(2); // plugin row + hook registration
    expect(screen.getByText("requires disabled plugin `billing`")).toBeTruthy();
    expect(screen.getByText("chat/turn-end")).toBeTruthy();
    expect(screen.getByText("profile disables unknown plugin `ghost`")).toBeTruthy();
    expect(screen.getByRole("note").textContent).toContain("restart");

    screen.getAllByRole("button", { name: "Disable" })[0].click();
    screen.getByRole("button", { name: "Enable" }).click();
    expect(toggles).toEqual([
      ["memory", false],
      ["webhooks", true],
    ]);
  });

  it("MemoryOverviewView reports what is remembered, what was forgotten, and index lag", async () => {
    const { MemoryOverviewView } = await import("./surfaces");
    const overview = {
      tenantId: "ten_1",
      counts: [
        { layer: "l1" as const, status: "ready" as const, count: 10 },
        { layer: "l1" as const, status: "revoked" as const, count: 2 },
      ],
      // Deliberately below the Ready total: the shortfall is the signal that
      // the derived index has stopped being written.
      derivedEmbeddings: 7,
      recentlyRemembered: [
        { assetId: "mem_1", layer: "l1" as const, status: "ready" as const, title: "reply language", updatedAt: "t1" },
      ],
      recentlyForgotten: [
        { assetId: "mem_2", layer: "l1" as const, status: "revoked" as const, title: "stale fact", updatedAt: "t2" },
      ],
    };
    let rebuilt = 0;
    render(<MemoryOverviewView overview={overview} state="ready" onRebuildIndexes={() => (rebuilt += 1)} />);

    // "10" also appears in the per-layer count row, so assert against the
    // summary list specifically rather than the whole panel.
    expect(screen.getAllByText("10").length).toBeGreaterThan(0);
    expect(screen.getByText("Remembered")).toBeTruthy();
    expect(screen.getByText("Retrieval index")).toBeTruthy();
    expect(screen.getByText("(3 not yet indexed)")).toBeTruthy();
    expect(screen.getByText("reply language")).toBeTruthy();
    expect(screen.getByText("stale fact")).toBeTruthy();

    screen.getByRole("button", { name: "Rebuild retrieval index" }).click();
    expect(rebuilt).toBe(1);
  });
});
