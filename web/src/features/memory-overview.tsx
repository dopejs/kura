// "What is remembered" panel (Stage 2.1). Mounts MemoryOverviewView — which
// until 2026-09-19 existed only as an unmounted presentational component —
// into the operator shell with the tenant-scoped data flow every other panel
// uses. Data fetching stays in App via the SDK; this component owns no
// runtime truth.

import type { MemoryOverview } from "@kura/client";

import { MemoryOverviewView } from "./operator-shell/surfaces";
import type { ViewState } from "./operator-shell/navigation";

type MemoryOverviewPanelProps = {
  overview: MemoryOverview | null;
  loading: boolean;
  denied: boolean;
  error: string;
  onRefresh: () => void;
  onRebuildIndexes: () => void;
};

function viewState({ loading, denied, error, overview }: Omit<MemoryOverviewPanelProps, "onRefresh" | "onRebuildIndexes">): ViewState {
  if (denied) return "denied";
  if (error) return "error";
  if (loading && !overview) return "loading";
  if (!overview) return "empty";
  return "ready";
}

export function MemoryOverviewPanel(props: MemoryOverviewPanelProps) {
  const state = viewState(props);
  return (
    <section className="panel" aria-label="Memory overview panel">
      <div className="panel-header">
        <div>
          <h2>What is remembered</h2>
          <p>{props.overview?.tenantId ? `Tenant ${props.overview.tenantId}` : "Tenant-scoped memory inventory"}</p>
        </div>
        <button type="button" onClick={props.onRefresh} disabled={props.loading}>
          Refresh
        </button>
      </div>
      <MemoryOverviewView
        overview={props.overview ?? undefined}
        state={state}
        reason={props.error || undefined}
        onRebuildIndexes={state === "ready" ? props.onRebuildIndexes : undefined}
      />
    </section>
  );
}
