# Daemon Roadmaps — Remaining Work (Post-R72, Rust Era)

> **Supersession note (2026-08-16):** this document replaces the pre-Rust-era
> roadmap document. The full per-roadmap execution record for Roadmaps 1-72
> (goals, tasks, definitions of done) lives in this file's git history
> (revisions before 2026-08-16). That version carried stale statuses
> (`[ ] proposed`) for Roadmaps 45-72; the ledger below is the corrected
> record. Statuses here are authoritative.

## Purpose

This document defines the execution structure for the remaining daemon work.

The delivery unit is a **roadmap**, not an isolated task. Each roadmap:

- forms a closed vertical slice
- contains multiple tasks
- has a roadmap-level definition of done
- is only complete when every in-scope task meets its own completion standard

Each implementation round should finish **one whole roadmap**.

## Execution Standard

- A roadmap is the planning and delivery boundary; a task is an auditable work
  item inside it.
- A task is `[x]` only when its full definition of done is satisfied. A roadmap
  is `[x]` only when every required task is satisfied and the roadmap-level
  definition of done is met.
- Partial, provisional, or narrow implementations stay `[ ]` with a note like
  `(partial)` or `(blocked)`. Demo-grade shortcuts do not count as completion.
- A roadmap too large for one round must be re-cut before implementation
  starts, not partially completed and counted as done.
- **Implementation status and release-evidence status are tracked separately**
  (the Roadmap 44 standard). Release-closure claims must go through
  [`release-truth-checklist.md`](release-truth-checklist.md).

## Current State (2026-08)

- **Roadmaps 1-72 are all implemented and merged to `main`** (one
  `Complete phase N` / `Complete roadmap N` commit per slice; the final
  non-knowledge parity slice landed 2026-06-30, commits `9d542ea..52a2580`
  plus the MVP-gap closure through `02fd047`).
- **The Go daemon was then fully ported to the Rust workspace** (`crates/`,
  8 waves, see [`crates/MIGRATION.md`](../../crates/MIGRATION.md)). The Go
  `daemon/` tree and the TypeScript TUI are removed; the control plane is the
  Rust `kura-cli` package (user binary `kura`), and the terminal client package
  is `kura-tui` (user binary `kura-tui`).
- Verification commands: `cargo test --workspace` (or `make daemon-test`) and
  `make daemon-contract-test`. Go-era `go test ./...` references in archived
  documents map to these.
- **What is not closed** is captured below: Rust API surface parity (four
  route families and auth-middleware attachment still open, Roadmap 74),
  release evidence (all soak/launch evidence predates the Rust port),
  deferred permissive hooks, and the gated context/knowledge/memory program.

## Completed Roadmap Ledger (1-72)

Implementation status: all `[x]`. Spec-number → roadmap-number mapping lives in
[`../specs/README.md`](../specs/README.md); the detailed execution record is in
this file's git history (pre-2026-08-16 revisions) and in `specs/<NNN>-.../`.

| Roadmaps | Program | Notes |
|---|---|---|
| 1-13 | Daemon core: runtime, supervision, LLM dispatch, trust, contracts, conversation, clients, ingress, providers, IM loop, streaming | |
| 14-24 | Test env, skills, sandbox plane, MCP execution/catalog/transports, tool-call orchestration | R20 follow-ons (stronger backends, VM-grade isolation) deferred, see below |
| 25-33 | Personal-agent surfaces: scheduling, computer use, integrations, delivery, calendar, mail, reminders, operator shell, evaluation harness | |
| 34-44 | Tenancy, hosted secrets, billing/quotas, production ops, live validation, evaluation expansion, diagnostics, hosted profile, release truth | R37/39/42/43 carry open evidence debt, see register below |
| 45-53 | Hosted activation, credential wizard, quota UX; channel conformance, Discord hardening, Telegram, Slack, Matrix, channel repair UX | |
| 54-59 | Thread/session lifecycle, continuity, group reset/handoff, persona, workspace binding, integration adapter plane | |
| 60-72 | Real calendar/mail closure (Feishu/Lark), attachments, inbox triage, routines, webhooks, catalog, exec-profile UX, operator shell productization, evidence bundle, launch-gate validator | R72 delivered the *validator*; the launch-gate *run* is still open, see Roadmap 77 |

## Release Evidence Debt Register

These roadmaps are implementation-complete but their release evidence is not
closed. Additionally, **every existing soak record predates the Rust port**
(Go daemon, commit `5ad95ba`, host `zentalk-1`, 2026-05-01), so under the
release-truth standard none of it certifies the shipped Rust implementation.

| Roadmap | Open evidence |
|---|---|
| 37 Hosted secrets & connector isolation | final soak evidence |
| 39 Production install/upgrade/backup/soak | final release soak evidence |
| 42 Integration health & permission diagnostics | stable-host / real-account release evidence |
| 43 Hosted operational profile & recovery | full-duration hosted daemon soak (`hosted_soak_pending`) |
| 72 Public release soak & launch gate | operator-run hosted soak + real-account evidence index; ship/no-ship decision |

Closing this register is Roadmaps 76 and 77.

---

## Roadmap 73: Documentation And Execution-Record Truth Closure

Status: `[x] complete`

### Goal

Make every in-repo claim about the control plane match the Rust reality, so an
engineer joining after the migration cannot be misled by Go-era text.

### Context

- Project `CLAUDE.md` still documents a "Go 1.24" daemon in `daemon/` with
  `daemon/internal/...` package paths; that tree no longer exists. The
  `Makefile` targets already run cargo.
- `crates/surface/api/src/` carries stale wave-8 TODO comments, e.g.
  `state.rs` claims "the kura-reminders crate does not exist" while the crate
  exists, compiles, and is wired in `crates/surface/app/src/lib.rs:338`.
  Stale markers also remain in `types.rs`, `middleware.rs`, `routes/auth.rs`,
  and `routes/setupwizard.rs`.
- The pre-replacement roadmap document carried `[ ] proposed` for 28 shipped
  roadmaps; the old task registry linked to a nonexistent absolute path and
  described R17/R18/R20 as in-progress/planned.

### Tasks

- [x] Replace `daemon-roadmaps.md` and `daemon-tasks.md` with current-state
      documents; retire the stale R1-72 documents and superseded roadmap-split
      planning docs to git history (this change)
- [x] Rewrite the daemon section of `CLAUDE.md` for the Rust workspace
      (crate layout under `crates/`, cargo/make commands, entry point
      `kura-cli`, app assembly in `crates/surface/app/src/lib.rs`)
- [x] Audit every `TODO: PLACEHOLDER` / `TODO: MISSING` marker in
      `crates/surface/api/src/`; delete markers that describe closed gaps,
      keep only ones matching a real open seam, and reference this roadmap
      from any that remain
- [x] Sweep `docs/` for `daemon/internal/`, `cd daemon && go test`, and other
      Go-era paths; fix or annotate as historical
- [x] Confirm `docs/specs/README.md` mapping and speckit-flow docs describe
      the current (non-speckit-tooling) authoring flow

### Definition Of Done

- No non-archive document or code comment claims the control plane is Go or
  references the removed `daemon/` tree as live code.
- A reviewer can grep `daemon/internal|go test` under `docs/`, `CLAUDE.md`,
  and `crates/` and find only archive files and deliberate historical notes.

### Explicitly Out Of Scope

- Any behavior change; this roadmap is documentation and comments only.

---

## Roadmap 74: Rust API Surface Parity Closure

Status: `[x] complete (2026-08-16)`

### Goal

Make the Rust daemon's HTTP surface actually match the Go daemon it replaced,
so SDK/web/TUI clients and the launch-gate workflow stop 404ing against
shipped functionality.

### Context

A 2026-08-16 route-table audit (Go table recovered from commit `16ac318^`)
found the wave-8 "16 route families done" claim in `crates/MIGRATION.md` was
an overstatement — the Go daemon served 22 families. The domain managers for
the missing surfaces were fully ported and wired into `AppState`, but no
route ever called them. Additionally, the ported `protected()` auth
middleware is not attached to the production router: even with an auth
manager configured, every route is served unauthenticated (Go wrapped every
protected route at registration).

### Tasks

- [x] Port the six Roadmap 65-72 route families: `/v1/triage/policies`,
      `/v1/routines`, `/v1/catalog/items`, `/v1/execution/profiles` +
      `/v1/execution/explain`, `/v1/support/evidence-bundles`,
      `/v1/release/launch-gate` — with Go status-code/error-mapping parity,
      behavioral tests, and the `LaunchGateEvidence` `serde(default)` wire
      fix (zero-value decode parity with Go)
- [x] Port `/v1/config` (config inspection, Roadmap 1)
- [x] Port `/v1/sessions` list/get/reset/events (Roadmaps 1/6)
- [x] Port `/v1/llm/dispatches`, `/v1/llm/dispatches/stream` (Roadmap 3,
      SSE started/delta/terminal frames; `CreateDispatchInput` and
      `CheckInput` gained `serde(default)` for Go zero-value decode parity)
- [x] Port `/v1/sandboxes/executions`, `/v1/sandboxes/profiles` (+reload),
      `/v1/sandboxes/explain` (Roadmap 16)
- [x] Port the four additional families the parity gate surfaced:
      `/v1/capabilities` (Roadmap 2), `/v1/connectors` incl. the
      `ingress/messages` inbound pipeline (Roadmaps 2/8/11),
      `/v1/policy/approvals` incl. consumer-policy sync, sandbox enrichment,
      and computer-use resume on approval (Roadmap 4), `/v1/providers` incl.
      managed auth, models, default-model, and checks (Roadmaps 9/10)
- [x] Attach `protected()` to the production router with Go's exact
      unauthenticated allowlist (`/healthz`, `/version`, `/v1/system/info`,
      pairing start/complete, signature-authed `/v1/triggers/webhook/`),
      verified by test
- [x] Route-table parity gate test (`crates/surface/api/tests/route_parity.rs`)
      probing every pattern in the recorded Go route table against the
      mounted router; it fails on unexplained gaps

### Definition Of Done (met)

- Every route in the Go daemon's final route table exists in the Rust router
  with matching methods and status semantics, or carries a recorded
  intentional-divergence note — enforced by the parity gate test.
- Authentication enforcement parity: the unauthenticated allowlist matches Go
  `server.go`, verified by test.

### Residual Risk (rolled into Roadmap 75)

- Per-handler tenant-context integration is documented per module rather than
  implemented: hosted credential-permission checks, tenant-scoped list and
  tenant-safe persistence variants, and the tenant-gated connector
  diagnostics/setup/smoke reads answer the Go no-tenant-context behavior.
  These paths are inert until identity-resolved tenant contexts are exercised
  by a hosted deployment, and must be closed before the Roadmap 76 hosted
  soak.

### Explicitly Out Of Scope

- In-manager hook wiring (Roadmap 75) and any new product surfaces.

---

## Roadmap 75: Deferred Hook Wiring Closure

Status: `[x] complete (2026-08-16)` — see the recorded decisions below; the
hosted-deployment remainder of the Roadmap 74 tenant residual moved to
Roadmap 76 as a pre-soak prerequisite.

### Goal

Replace the remaining permissive in-manager defaults with real implementations
or explicit recorded risk acceptances, so defense-in-depth gates actually
defend before release evidence is gathered on top of them.

### Context

Route-level `protected()` middleware remains the authoritative authorization
boundary; the in-manager hooks below are defense-in-depth and currently
permissive in the production assembly (`crates/surface/app/src/lib.rs`):

- `kura_webhook::Manager` is constructed with `quota: None`, falling back to
  `AllowAllQuota` (`crates/domains/webhook/src/lib.rs:195`). The webhook
  ingress `/v1/triggers/webhook/` is signature-authed rather than
  `protected()`, which makes an unbounded quota gate a real exposure, not
  just defense-in-depth.
- `kura_catalog::Manager` is constructed with `checker: None` (falls back to
  `AllMet`) and `permissions: None` — the sandbox/secret/policy wiring that
  spec 053 explicitly deferred as follow-on.
- Spec 052's deferred non-webhook inbound trigger sources remain unbuilt
  (decide: implement or formally re-scope).

### Tasks

- [x] Webhook `QuotaGate` backed by the billing plane
      (`WebhookQuotaGateImpl` in `crates/surface/app/src/adapters.rs`):
      tenant-scoped triggers reserve + commit one workflow-launch unit;
      denials return the billing reason code, publish
      `webhook.trigger_quota_denied`, and land in the durable QuotaDenial
      ledger; hosted tenants fail closed when quota state is unavailable.
      Recorded decision: tenant-less local triggers stay allowed — quota is
      a hosted per-tenant bound.
- [x] Catalog `RequirementChecker` backed by the sandbox plane
      (`CatalogSandboxRequirementChecker`): a requirement key names a sandbox
      backend that must report available; unknown keys are unmet (fail
      closed). Catalog `PermissionGate` backed by the identity plane
      (`CatalogTenantPermissionGate`): tenant-scoped enablement requires an
      Active tenant; unknown/disabled tenants are denied. Recorded decision:
      fine-grained permission strings stay with the authoritative
      protected() middleware.
- [x] Hook-seam inventory (grep `unwrap_or_else(|| Box::new` across crates):
      webhook firer/quota — wired; catalog checker/permissions — wired;
      execprofile health/reqs/perms — wired (sandbox health + sandbox
      requirement checker + tenant gate); evidence collector/perms — wired
      (routine collector + `EvidenceSupportPermissionGate`: named actor +
      Active tenant). Accepted as-is (recorded): activation/scheduler clock
      seams (test determinism only) and the matrix `FakeTransport`
      constructor default (production wiring passes the real transport).
- [x] Spec 052 non-webhook trigger sources — recorded decision: remain out
      of scope for the non-knowledge program. IM ingress is served by the
      channel connectors; any other third-party event source rides the
      signed webhook plane. Generic inbound trigger adapters join the
      post-launch backlog.
- [x] Roadmap 74 tenant residual, data-isolation slice: llm dispatches use
      tenant-scoped list/get and tenant-safe persistence; the sessions list
      is tenant-filtered; connector diagnostics/setup/smoke hosted reads are
      fully implemented behind the tenant context + connectors-manage
      permission. Remaining hosted-deployment items moved to Roadmap 76
      (pre-soak): providers hosted credential-permission checks and
      per-tenant managed-auth variants, tenant-context overrides in
      catalog/evidence/webhook handlers, and by-id tenant guards.
- [x] Behavioral tests for each newly wired gate
      (`crates/surface/app/src/adapters.rs` hook_wiring_tests: requirement
      matching + fail-closed, active/disabled/unknown-tenant gating,
      local-allow + hosted fail-closed + deny-event assertions)

### Definition Of Done

- No production assembly path constructs a manager with a permissive default
  hook unless an accepted-risk note in this document says so and why.
- The webhook ingress denies over-quota deliveries and emits an auditable
  event for the denial.

### Explicitly Out Of Scope

- New product surfaces; this roadmap only closes seams the shipped specs
  already declared as follow-ons.

---

## Roadmap 76: Rust-Era Release Evidence Re-Run

Status: `[ ] planned` — depends on Roadmaps 74 and 75 (soak must cover the
full API surface and final wiring)

### Goal

Re-establish the entire release evidence base on the Rust daemon and close
the R37/39/42/43 evidence debt register.

### Context

The production operations baseline (install, upgrade, backup, restore,
rollback, the reusable 24-hour soak harness, fault drills, real-account smoke
policy, resource-growth checks) is implemented; operator entry point
[`production-operations.md`](production-operations.md). All recorded evidence
is Go-era and therefore void for the Rust binary.

### Tasks

- [ ] Close the hosted tenant-context remainder before the hosted soak:
      providers hosted credential-permission checks and per-tenant
      managed-auth variants, tenant-context overrides in
      catalog/evidence/webhook handlers, by-id tenant guards, and the four
      recorded handler divergences (llm setup-wizard gate, approval thread
      projection, ingress credential audit, session profile projections)
- [~] Stability hardening before the soak: the systematic Go-null sweep is
      done (every raw `serde_json::from_str` over a `_raw`/`_json` text
      column in the store now routes through the null-tolerant
      `decode_json_field`, alongside the earlier decode_vec/decode_map and
      serde `null_default` fixes); the 3x restart drill on the real test
      data dir passed 2026-08-17 with recovery in 18s/10s/9s (classified
      recovered; bound is 5 minutes). Recorded observation: boot blocks on
      synchronous MCP websocket reconnects during restore (~10-20s with one
      unreachable persisted server) — restore should mark remote MCP
      servers for lazy start instead of blocking the listener; tracked as a
      pre-soak follow-up together with the clippy `unwrap_used` audit over
      non-test daemon paths
- [x] First-release schema baseline (2026-08-17): migrations v1-v55
      collapsed into the single `baseline_v1_first_release` (the exact
      schema they produced, dumped from a migrated database — 464
      statements); fresh databases create the baseline directly;
      development databases at the legacy head (v55) are re-stamped in
      place and anything older must re-initialize; migrationfixture's
      pre-tenant v21 staging retired (no pre-baseline lineage ships);
      future migrations append as v2+
- [~] Install and auto-upgrade productization: `scripts/install.sh`
      (build + install `kura` onto PATH + data-dir init) and
      `scripts/upgrade.sh` (preflight → backup → build+install → restart →
      postflight, with the restore script as the rollback path) are in.
      CI and packaging exist as GitHub Actions (2026-08-17):
      `.github/workflows/ci.yml` runs the Rust workspace suite, contract
      tests, the route-parity gate, and the TypeScript client builds on
      every push/PR to main (ubuntu + macos); `.github/workflows/release.yml`
      builds `kura` for macOS (arm64/x64) and Linux (x64/arm64) on `v*`
      tags and publishes tarballs + SHA256SUMS as a GitHub Release — which
      also becomes the release feed for the daemon-surfaced update check
      (remaining task, post-first-tag)
- [x] Verify the soak harness, fault drills, and ops tooling run unmodified
      against the Rust daemon (2026-08-16: `scripts/production/run-soak.sh`
      targeted-validation run passed against the Rust binary on the real
      `~/.kura-test` data dir). The first real-data boot surfaced a Go
      wire-compat defect class, fixed at root before any soak counts:
      Go-marshaled `null` for nil slices/maps in persisted JSON (store
      decoders + serde `null_default` annotations), 29 serde enum wire
      values mangled by `rename_all = "snake_case"` acronym handling (every
      `string_enum!` macro now renames each variant to its exact Go
      literal), the missing Go `AppendEvent` default-tenant auto-bind (T077
      CHECK constraint), and an MCP websocket transport
      runtime-inside-runtime boot panic (shared never-dropped IO runtime +
      `block_in_place`). Full-duration soaks must run on the fixed build.
- [ ] 24-hour test-environment soak of the Rust daemon with fake-backend
      fault drills and resource-growth checks (closes R39 evidence)
- [ ] Full-duration hosted daemon soak on a stable host (closes R43
      `hosted_soak_pending`)
- [ ] Hosted secrets / connector isolation soak evidence (closes R37)
- [ ] Integration health and permission diagnostics evidence on a stable host
      with real accounts (closes R42)
- [ ] Record every run through
      [`release-truth-checklist.md`](release-truth-checklist.md) with commit
      hash, host, dates, and artifacts linked from this document

### Definition Of Done

- The Release Evidence Debt Register above shrinks to only the Roadmap 77
  launch-gate row, each closed row linking Rust-era evidence.
- Every recorded run identifies the exact Rust commit it certifies.

### Explicitly Out Of Scope

- The public launch-gate run itself (Roadmap 77).

---

## Roadmap 77: Public Launch Gate Execution

Status: `[ ] planned` — depends on Roadmaps 74, 75, and 76

### Goal

Execute the launch gate that Roadmap 72 codified: assemble the real evidence
index, run the validator, and record a ship / no-ship decision for public
release.

### Context

`ValidateLaunchGate` and `POST /v1/release/launch-gate` exist and enforce:
required workload coverage, at least 3 channels, real calendar and mail
providers, soak, support-bundle, and redaction evidence
(see [`release-readiness.md`](release-readiness.md) and
`specs/057-public-release-soak-and-launch-gate/`). The gate is a no-ship
gate; missing evidence must fail it, not be waived.

### Tasks

- [ ] Operator-run real-account workloads across at least 3 channels
      (Discord, Telegram, Slack; Matrix as available) on the Rust daemon
- [ ] Real-account Feishu/Lark calendar and mail workloads, including
      attachment transfer and inbox triage paths
- [ ] Support evidence bundle generation and redaction verification on the
      release candidate
- [ ] Assemble `LaunchGateEvidence` from Roadmap 76 artifacts plus the runs
      above; execute `POST /v1/release/launch-gate`
- [ ] Record the ship / no-ship decision, the evidence index location, and —
      on no-ship — the enumerated blockers as new roadmap tasks

### Definition Of Done

- A dated launch-gate decision for a named Rust commit is recorded in this
  document and in the release evidence index.
- On ship: the Roadmap 78 entry gate is formally open. On no-ship: every
  blocker is a tracked task with an owner roadmap.

---

## Roadmap 78+: Context, Knowledge, And Memory Program

Status: `[~] in progress` — the implementation gate was opened by operator
decision on 2026-08-17 (ahead of the Roadmap 77 ship decision; the
launch-gate evidence work continues in parallel as operator-run activity).
The memory design root is
[TencentCloud/TencentDB-Agent-Memory](https://github.com/TencentCloud/TencentDB-Agent-Memory)
— layered L0-L3 memory with deterministic drill-down, uniform governed
memory assets (memory/skills/wiki/codegraph under one envelope), async
consolidation pipelines, layered retrieval with BM25+vector+RRF fallback,
assets-as-tools, and dual-layer white-box storage — adapted onto
Kura's planes per the revised `docs/specs/058`.

### Goal

Deliver the product-thesis pillars that the entire non-knowledge program
deliberately excluded: layered memory, knowledge retrieval, context
engineering, agent-managed skills, and audited self-improvement
(see [`../product/product-outline.md`](../product/product-outline.md)).

### Binding Constraints (from the product outline)

- Memory writes must be attributable, scoped, and reversible.
- Recalled memory is evidence, not truth; important decisions must link back
  to source.
- No self-modifying agent behavior without review and audit.
- No general-purpose knowledge graph as the primary storage model.

### Program Shape

> **Planning-flow change (2026-08-17, operator):** the numbered-spec
> authoring flow is retired for new work. Features are planned directly in
> design docs and implemented; specs 001-062 remain as historical record.

1. **Memory plane foundation** — complete 2026-08-17 (Roadmap 78, from
   spec 058): layered assets, governed write lifecycle, LLM consolidation,
   live capture at chat/ingress/workflow.
2. **Agent pluginization** — inserted ahead of all further capability work
   by operator decision 2026-08-17; reference architecture
   [deepseek-ai/deepseek-harness](https://github.com/deepseek-ai/deepseek-harness).
   Design + status: [`../harness/plugin-architecture.md`](../harness/plugin-architecture.md).
   Phase 1 (plugin kernel, builtin plugin assembly, `/v1/plugins`) landed
   2026-08-17; phase 2 is the hookable agent loop; phase 3 is
   out-of-process plugin providers.
3. **Session/context management as plugins** — slice 1 complete
   2026-08-17 (`6b22ac5`, `ce2635e`): the `session-strategy` builtin shapes
   the window at `chat/pre-dispatch` (frame-preserving elision,
   compression-to-memory), and the `context` builtin injects the memory
   bootstrap with a full AssemblyRecord.
4. **Knowledge retrieval** — slice 1 complete 2026-08-17
   (`695bafe`, `b3cf38b`, `7fa2f46`): BM25 + recency + vector ranking fused
   with RRF (k=60) over Ready L1 atoms, the `Embedder` seam, and
   `POST /v1/retrieval/queries`.
5. **Agent-managed skills** — slice 1 complete 2026-08-17 (`b41928a`): a
   skill proposal is a `kind=skill` L1 memory asset riding the existing
   memory review queue, plus the Ready-only publication bridge.
6. **Self-improvement** — slice 1 complete 2026-08-17 (`9be4164`): bounded,
   rate-limited proposals over one plugin-profile config value, each
   carrying evidence and a recorded rollback snapshot.

Every program item has landed slice 1; none is closed. The second slice wave
for all six, together with the multi-tenant assembly, built-in tool, and
server-deployment work surfaced by the 2026-09-18 audit, is planned in
[`../harness/agent-deepening-program.md`](../harness/agent-deepening-program.md).

---

## Deferred, Non-Gating Work

Carried follow-ons that do not block the launch gate; schedule after
Roadmap 77 unless a launch blocker pulls them forward:

- **Sandbox backend expansion** (from Roadmap 20): additional consumer
  families onto sandbox execution, stronger backends beyond `docker`,
  VM-grade isolation, remote execution control planes.
- **Client-surface polish** beyond the operator-shell scope shipped in R70
  (the Rust TUI and web shell evolve by product need, not by this program).

## Relationship To Other Planning Artifacts

- Git history of this file (pre-2026-08-16) — frozen execution record for
  Roadmaps 1-72.
- [`daemon-tasks.md`](daemon-tasks.md) — task registry for the roadmaps in
  this document (the R1-72 registry lives in that file's git history).
- [`../specs/README.md`](../specs/README.md) — upstream spec index and
  spec→roadmap mapping; continues at 058 for the knowledge program.
- [`release-truth-checklist.md`](release-truth-checklist.md) — mandatory for
  every evidence claim in Roadmaps 76 and 77.
- `specs/<NNN>-.../` — per-slice working areas once implementation begins.
