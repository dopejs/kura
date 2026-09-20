// Operator-shell product surface views (Roadmap 70). Presentational components for the Roadmap
// 65-71 surfaces, consuming @kura/client resource types. Each renders through SurfacePanel so the
// standard empty/error/denied/unsupported/loading states are consistent and explainable. Data
// fetching is done by the host shell via the SDK (the shell never bypasses daemon APIs).

import type {
  CatalogItemResource,
  EvidenceBundleResource,
  ExecutionProfileResource,
  PluginHookRegistration,
  PluginStatusResource,
  RoutineResource,
  TriagePolicyResource,
  WebhookEndpointResource,
  MemoryAssetResource,
  MemoryOverview,
} from "@kura/client";

import type { ViewState } from "./navigation";

const STATE_MESSAGES: Partial<Record<ViewState, string>> = {
  loading: "Loading…",
  empty: "Nothing here yet.",
  error: "Something went wrong. Check the reason and retry.",
  denied: "Select a tenant or request access to view this surface.",
  unsupported: "This capability is not supported in the current configuration.",
};

type SurfacePanelProps = {
  title: string;
  state: ViewState;
  reason?: string;
  children?: React.ReactNode;
};

// SurfacePanel renders the standard non-ready states, or the surface body when ready.
export function SurfacePanel({ title, state, reason, children }: SurfacePanelProps) {
  return (
    <section className={`surface surface-${state}`} aria-label={title}>
      <h3 className="surface__title">{title}</h3>
      {state === "ready" ? (
        children
      ) : (
        <div className={`surface__state surface__state-${state}`} role="status">
          <p>{STATE_MESSAGES[state] ?? "Unavailable."}</p>
          {reason ? <p className="surface__reason">{reason}</p> : null}
        </div>
      )}
    </section>
  );
}

export function RoutinesView({ routines, state, reason }: { routines: RoutineResource[]; state: ViewState; reason?: string }) {
  return (
    <SurfacePanel title="Routines" state={state} reason={reason}>
      <ul className="surface__list">
        {routines.map((r) => (
          <li key={r.routineId} className="surface__row">
            <span className="surface__name">{r.name}</span>
            <span className={`status-chip status-${r.state}`}>{r.state}</span>
            <span className="surface__meta">v{r.currentVersion}</span>
          </li>
        ))}
      </ul>
    </SurfacePanel>
  );
}

export function WebhooksView({ endpoints, state, reason }: { endpoints: WebhookEndpointResource[]; state: ViewState; reason?: string }) {
  return (
    <SurfacePanel title="Webhook triggers" state={state} reason={reason}>
      <ul className="surface__list">
        {endpoints.map((e) => (
          <li key={e.webhookId} className="surface__row">
            <span className="surface__name">{e.name}</span>
            <span className={`status-chip status-${e.status}`}>{e.status}</span>
            <span className="surface__meta">{e.targetKind}:{e.targetRef}</span>
            <span className="surface__meta surface__redacted">{e.secretFingerprint}</span>
          </li>
        ))}
      </ul>
    </SurfacePanel>
  );
}

export function CatalogView({ items, state, reason }: { items: CatalogItemResource[]; state: ViewState; reason?: string }) {
  return (
    <SurfacePanel title="Skill & capability catalog" state={state} reason={reason}>
      <ul className="surface__list">
        {items.map((item) => (
          <li key={item.itemId} className="surface__row">
            <span className="surface__name">{item.name}</span>
            <span className={`trust-chip trust-${item.trustTier}`}>{item.trustTier}</span>
            <span className="surface__meta">{item.kind}</span>
            <span className="surface__meta">{item.versions.length} version(s)</span>
          </li>
        ))}
      </ul>
    </SurfacePanel>
  );
}

export function ExecutionProfilesView({ profiles, state, reason }: { profiles: ExecutionProfileResource[]; state: ViewState; reason?: string }) {
  return (
    <SurfacePanel title="Execution profiles" state={state} reason={reason}>
      <ul className="surface__list">
        {profiles.map(({ profile, status }) => (
          <li key={profile.profileId} className="surface__row">
            <span className="surface__name">{profile.name}</span>
            <span className={`status-chip status-${status.health}`}>{status.available ? "available" : status.health}</span>
            <span className={`risk-chip risk-${profile.riskTier}`}>{profile.riskTier}</span>
            {status.reason ? <span className="surface__reason">{status.reason}</span> : null}
          </li>
        ))}
      </ul>
    </SurfacePanel>
  );
}

export function TriagePoliciesView({ policies, state, reason }: { policies: TriagePolicyResource[]; state: ViewState; reason?: string }) {
  return (
    <SurfacePanel title="Inbox triage policies" state={state} reason={reason}>
      <ul className="surface__list">
        {policies.map((p) => (
          <li key={p.policyId} className="surface__row">
            <span className="surface__name">{p.name}</span>
            <span className="surface__meta">{p.rules.length} rule(s)</span>
            <span className="surface__meta">default: {p.defaultClassification}</span>
          </li>
        ))}
      </ul>
    </SurfacePanel>
  );
}

export function SupportEvidenceView({ bundles, state, reason }: { bundles: EvidenceBundleResource[]; state: ViewState; reason?: string }) {
  return (
    <SurfacePanel title="Support evidence" state={state} reason={reason}>
      <ul className="surface__list">
        {bundles.map((b) => (
          <li key={b.bundleId} className="surface__row">
            <span className="surface__name">{b.scope.kind}{b.scope.ref ? `:${b.scope.ref}` : ""}</span>
            <span className={`status-chip status-${b.redactionStatus}`}>{b.redactionStatus}</span>
            <span className="surface__meta">{(b.sections ?? []).length} section(s)</span>
          </li>
        ))}
      </ul>
    </SurfacePanel>
  );
}

export function PluginsView({
  plugins,
  hooks = [],
  warnings = [],
  state,
  reason,
  restartRequired = false,
  onToggle,
}: {
  plugins: PluginStatusResource[];
  hooks?: PluginHookRegistration[];
  warnings?: string[];
  state: ViewState;
  reason?: string;
  /** True after a profile write in this session: changes apply on restart. */
  restartRequired?: boolean;
  onToggle?: (pluginId: string, enable: boolean) => void;
}) {
  return (
    <SurfacePanel title="Plugin assembly" state={state} reason={reason}>
      {restartRequired ? (
        <p className="surface__notice" role="note">
          Profile saved. Changes take effect after the daemon restarts.
        </p>
      ) : null}
      <ul className="surface__list">
        {plugins.map((p) => (
          <li key={p.id} className="surface__row">
            <span className="surface__name">{p.id}</span>
            <span className={`status-chip status-${p.enabled ? "enabled" : "disabled"}`}>
              {p.enabled ? "enabled" : "disabled"}
            </span>
            <span className="surface__meta">{p.source}</span>
            <span className="surface__meta">{p.summary}</span>
            {p.reason ? <span className="surface__reason">{p.reason}</span> : null}
            <button type="button" onClick={() => onToggle?.(p.id, !p.enabled)}>
              {p.enabled ? "Disable" : "Enable"}
            </button>
          </li>
        ))}
      </ul>
      {hooks.length > 0 ? (
        <ul className="surface__list surface__hooks" aria-label="Hook registrations">
          {hooks.map((h, i) => (
            <li key={`${h.point}-${h.pluginId}-${i}`} className="surface__row">
              <span className="surface__meta">{h.point}</span>
              <span className="surface__name">{h.pluginId}</span>
            </li>
          ))}
        </ul>
      ) : null}
      {warnings.length > 0 ? (
        <ul className="surface__list surface__warnings" aria-label="Profile warnings">
          {warnings.map((w) => (
            <li key={w} className="surface__reason">{w}</li>
          ))}
        </ul>
      ) : null}
    </SurfacePanel>
  );
}

export function MemoryAssetsView({ assets, state, reason }: { assets: MemoryAssetResource[]; state: ViewState; reason?: string }) {
  return (
    <SurfacePanel title="Memory assets" state={state} reason={reason}>
      <ul className="surface__list">
        {assets.map((a) => (
          <li key={a.assetId} className="surface__row">
            <span className="surface__name">{a.title || a.assetId}</span>
            <span className="surface__meta">{a.layer}</span>
            <span className="surface__meta">{a.status}</span>
            <span className="surface__meta">{a.visibility}</span>
            <span className="surface__meta">v{a.version}</span>
          </li>
        ))}
      </ul>
    </SurfacePanel>
  );
}

export function MemoryReviewView({
  pending,
  state,
  reason,
  onApprove,
  onReject,
}: {
  pending: MemoryAssetResource[];
  state: ViewState;
  reason?: string;
  onApprove?: (assetId: string) => void;
  onReject?: (assetId: string) => void;
}) {
  return (
    <SurfacePanel title="Memory pending review" state={state} reason={reason}>
      <ul className="surface__list">
        {pending.map((a) => (
          <li key={a.assetId} className="surface__row">
            <span className="surface__name">{a.title || a.assetId}</span>
            <span className="surface__meta">{a.owner.kind}:{a.owner.id}</span>
            <span className="surface__meta">{a.statusReason ?? ""}</span>
            <button type="button" onClick={() => onApprove?.(a.assetId)}>Approve</button>
            <button type="button" onClick={() => onReject?.(a.assetId)}>Reject</button>
          </li>
        ))}
      </ul>
    </SurfacePanel>
  );
}

/**
 * "What is remembered" — the tenant-level inventory (counts per layer and
 * status, the derived-index size, and the two recency lists). The asset list
 * answers the question only for someone who already knows what to look for;
 * this answers it for someone who does not.
 *
 * `onRebuildIndexes` is the repair path: it drops the derived retrieval index
 * so it is recomputed. It deletes no memory, which is why it is offered here
 * rather than behind a warning.
 */
export function MemoryOverviewView({
  overview,
  state,
  reason,
  onRebuildIndexes,
}: {
  overview?: MemoryOverview;
  state: ViewState;
  reason?: string;
  onRebuildIndexes?: () => void;
}) {
  const readyCount = (overview?.counts ?? [])
    .filter((c) => c.status === "ready")
    .reduce((sum, c) => sum + c.count, 0);
  const indexed = overview?.derivedEmbeddings ?? 0;
  // Surfacing the shortfall rather than only the two numbers: an index that
  // has silently stopped being written looks identical to a healthy one until
  // someone subtracts.
  const indexLag = readyCount - indexed;

  return (
    <SurfacePanel title="What is remembered" state={state} reason={reason}>
      <dl className="surface__stats">
        <dt>Remembered</dt>
        <dd>{readyCount}</dd>
        <dt>Retrieval index</dt>
        <dd>
          {indexed}
          {indexLag > 0 ? <span className="surface__meta"> ({indexLag} not yet indexed)</span> : null}
        </dd>
      </dl>
      <ul className="surface__list">
        {(overview?.counts ?? []).map((c) => (
          <li key={`${c.layer}-${c.status}`} className="surface__row">
            <span className="surface__name">{c.layer}</span>
            <span className={`status-chip status-${c.status}`}>{c.status}</span>
            <span className="surface__meta">{c.count}</span>
          </li>
        ))}
      </ul>
      <h4 className="surface__subtitle">Recently remembered</h4>
      <ul className="surface__list">
        {(overview?.recentlyRemembered ?? []).map((e) => (
          <li key={e.assetId} className="surface__row">
            <span className="surface__name">{e.title || e.assetId}</span>
            <span className="surface__meta">{e.layer}</span>
            <span className="surface__meta">{e.updatedAt}</span>
          </li>
        ))}
      </ul>
      <h4 className="surface__subtitle">Recently forgotten</h4>
      <ul className="surface__list">
        {(overview?.recentlyForgotten ?? []).map((e) => (
          <li key={e.assetId} className="surface__row">
            <span className="surface__name">{e.title || e.assetId}</span>
            <span className={`status-chip status-${e.status}`}>{e.status}</span>
            <span className="surface__meta">{e.updatedAt}</span>
          </li>
        ))}
      </ul>
      {onRebuildIndexes ? (
        <button type="button" onClick={onRebuildIndexes}>
          Rebuild retrieval index
        </button>
      ) : null}
    </SurfacePanel>
  );
}
