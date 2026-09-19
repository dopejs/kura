// Operator shell information architecture (Roadmap 70). This module is the single source of
// truth for how the web shell organizes all public, non-knowledge daemon surfaces into coherent
// navigation, what each surface expects (permission/approval/quota/side-effect), and the
// standard view states (empty/error/denied/unsupported). The shell remains a pure client of
// daemon APIs — this module owns no runtime truth.

export type SurfaceCriticalExpectation = {
  permission?: string;
  approval?: "none" | "scope" | "per_action";
  quota?: boolean;
  sideEffect?: boolean;
};

export type ShellSurface = {
  id: string;
  label: string;
  route: string;
  /** Whether the surface requires an active tenant selection. */
  requiresTenant: boolean;
  /** Expectations shown before a critical action runs (FR: critical actions show these). */
  critical?: SurfaceCriticalExpectation;
};

export type ShellSection = {
  id: string;
  label: string;
  surfaces: ShellSurface[];
};

// OPERATOR_SHELL_SECTIONS organizes setup, channels, sessions, profiles, routines, providers,
// quota, diagnostics, evaluation, and support into coherent navigation (FR-001).
export const OPERATOR_SHELL_SECTIONS: ShellSection[] = [
  {
    id: "plugins",
    label: "Plugins",
    surfaces: [
      { id: "plugin-assembly", label: "Plugin assembly", route: "/plugins", requiresTenant: false },
      { id: "plugin-profile", label: "Plugin profile", route: "/plugins/profile", requiresTenant: false, critical: { sideEffect: true } },
    ],
  },
  {
    id: "memory",
    label: "Memory",
    surfaces: [
      { id: "memory-overview", label: "What is remembered", route: "/memory/overview", requiresTenant: true },
      { id: "memory-assets", label: "Memory assets", route: "/memory", requiresTenant: true },
      { id: "memory-review", label: "Pending review", route: "/memory/review", requiresTenant: true, critical: { approval: "per_action", sideEffect: true } },
    ],
  },
  {
    id: "setup",
    label: "Setup",
    surfaces: [
      { id: "first-run", label: "First-run setup", route: "/setup", requiresTenant: true },
      { id: "credentials", label: "Credentials & OAuth", route: "/setup/credentials", requiresTenant: true, critical: { permission: "integrations.manage", sideEffect: true } },
    ],
  },
  {
    id: "channels",
    label: "Channels",
    surfaces: [
      { id: "channel-list", label: "Channels", route: "/channels", requiresTenant: true },
      { id: "channel-repair", label: "Channel repair", route: "/channels/repair", requiresTenant: true, critical: { permission: "channels.manage", approval: "scope", sideEffect: true } },
    ],
  },
  {
    id: "sessions",
    label: "Sessions",
    surfaces: [
      { id: "threads", label: "Sessions & threads", route: "/sessions", requiresTenant: true },
      { id: "thread-reset", label: "Reset session", route: "/sessions/reset", requiresTenant: true, critical: { permission: "threads.manage", approval: "per_action", sideEffect: true } },
    ],
  },
  {
    id: "profiles",
    label: "Profiles",
    surfaces: [
      { id: "agent-profiles", label: "Agent profiles", route: "/profiles", requiresTenant: true },
      { id: "workspace-bindings", label: "Workspace & capability bindings", route: "/profiles/bindings", requiresTenant: true, critical: { permission: "profiles.manage" } },
    ],
  },
  {
    id: "routines",
    label: "Routines",
    surfaces: [
      { id: "routine-list", label: "Routines", route: "/routines", requiresTenant: true },
      { id: "routine-create", label: "Create routine", route: "/routines/new", requiresTenant: true, critical: { permission: "routines.manage", approval: "scope", quota: true, sideEffect: true } },
      { id: "webhooks", label: "Webhook triggers", route: "/routines/webhooks", requiresTenant: true, critical: { permission: "webhooks.manage", sideEffect: true } },
    ],
  },
  {
    id: "providers",
    label: "Providers & capabilities",
    surfaces: [
      { id: "providers", label: "Providers", route: "/providers", requiresTenant: true },
      { id: "catalog", label: "Skill & capability catalog", route: "/providers/catalog", requiresTenant: true, critical: { permission: "catalog.manage", sideEffect: true } },
      { id: "execution-profiles", label: "Execution profiles", route: "/providers/execution", requiresTenant: true },
    ],
  },
  {
    id: "quota",
    label: "Quota & billing",
    surfaces: [{ id: "quota", label: "Quota & usage", route: "/quota", requiresTenant: true }],
  },
  {
    id: "diagnostics",
    label: "Diagnostics & support",
    surfaces: [
      { id: "diagnostics", label: "Integration diagnostics", route: "/diagnostics", requiresTenant: true },
      { id: "evaluation", label: "Evaluation & replay", route: "/diagnostics/evaluation", requiresTenant: true },
      { id: "support-evidence", label: "Support evidence", route: "/diagnostics/support", requiresTenant: true },
    ],
  },
];

export type ViewState = "loading" | "ready" | "empty" | "error" | "denied" | "unsupported";

export type ViewStateInput = {
  loading?: boolean;
  tenantSelected: boolean;
  requiresTenant: boolean;
  errorCode?: string;
  itemCount?: number;
};

// resolveViewState maps a surface's load result to one of the standard states so every view —
// including empty/error/denied/unsupported — renders a stable, explainable state (FR-001/FR-004).
export function resolveViewState(input: ViewStateInput): ViewState {
  if (input.requiresTenant && !input.tenantSelected) {
    return "denied";
  }
  if (input.loading) {
    return "loading";
  }
  switch (input.errorCode) {
    case undefined:
    case "":
      break;
    case "permission_denied":
    case "forbidden":
      return "denied";
    case "unsupported":
    case "not_implemented":
      return "unsupported";
    default:
      return "error";
  }
  if ((input.itemCount ?? 0) === 0) {
    return "empty";
  }
  return "ready";
}

// allSurfaceRoutes returns every surface route, used to assert navigation coverage in tests.
export function allSurfaceRoutes(): string[] {
  return OPERATOR_SHELL_SECTIONS.flatMap((section) => section.surfaces.map((s) => s.route));
}

// criticalSurfaces returns surfaces that perform critical actions (must show expectations).
export function criticalSurfaces(): ShellSurface[] {
  return OPERATOR_SHELL_SECTIONS.flatMap((section) => section.surfaces).filter((s) => Boolean(s.critical));
}
