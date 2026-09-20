# Agent Deepening Program

Planning date: 2026-09-18. This document is the planning record for the
second slice wave of [Roadmap 78+](../runtime/daemon-roadmaps.md) (Context,
Knowledge, and Memory Program), plus the follow-ups worth adopting from
OpenClaw releases 2026.8.1 through 2026.9.4.

Operator decisions taken at planning time:

- **Capability lines first.** Roadmap 76 (Rust-era release evidence) and
  Roadmap 77 (launch gate) continue as parallel operator-run activity and do
  not block this program.
- **Ops and security hardening is in scope but sequenced last** (Stage 7).

**Revised the same day, after the Stage 8–10 audit:** the "capability lines
first" ordering no longer holds unmodified. Stage 8 turned out to contain
data-isolation defects and Stage 10.1 a shipped artifact that cannot start;
both now precede the capability lines. Everything else keeps the original
ordering. See Sequencing for the revised order and the reasoning.

## Current Truth

Every Roadmap 78 program item landed slice 1 on 2026-08-17. The design docs
still describe some shipped work as pending; Stage 0 fixes that. What is
actually in `main`:

| Program item | Shipped | Where |
|---|---|---|
| 1 Memory plane | L0–L3 governed assets, write lifecycle, async consolidation, live capture | `crates/domains/memory` |
| 2 Pluginization | Phase 1 kernel, phase 2 hookable loop, phase 3 external stdio plugins + seam RPC | `crates/foundation/plugin`, `crates/surface/app/src/plugins.rs` |
| 3 Session/context as plugins | `session-strategy` (frame-preserving elision, compression-to-memory), `context` (bootstrap injection, AssemblyRecord) | `crates/domains/session`, `crates/domains/context` |
| 4 Knowledge retrieval | BM25 + recency + vector, RRF fusion (k=60), `Embedder` seam, `POST /v1/retrieval/queries` | `crates/domains/context/src/lib.rs:214` |
| 5 Agent-managed skills | Proposal = `kind=skill` L1 asset riding the memory review queue; publication bridge | `crates/surface/api/src/routes/skill_proposals.rs` (`b41928a`) |
| 6 Self-improvement | Bounded proposals over one plugin-profile config value, rate-bounded, rollback snapshot required | `crates/domains/improvement` (`9be4164`) |

The shape of the capability work is therefore **deepening six open lines**,
not opening new ones.

A 2026-09-18 audit added three areas the original plan did not cover:
multi-tenant assembly (Stage 8), built-in tools (Stage 9), and server
deployment readiness (Stage 10). Stage 8 contains data-isolation defects and
**should run before the capability lines** — see Sequencing.

**Status 2026-09-19: every stage (0–10) is closed.** Each closure entry
below names what shipped, how it was verified, and what was deliberately
left out; the Known Defects table records what was found on the way (D1–D11)
and which are fixed. Items that could not be verified on the authoring host
say so in their entry (Docker image, TLS overlay, a CI Chromium run).
Remaining follow-ups that are *not* half-done stages, recorded for the next
program: D8 (DNS rebinding), artifact capture for generated images/video
(9.3/9.5), `download` in the browser worker (9.4), and routing manager-owned
secondary connections through the pool (10.2).

## Known Defects

Verified against `main` at planning time. Each is a defect, not a missing
feature: the design intent exists and the implementation does not match it.

| # | Severity | Defect | Evidence |
|---|---|---|---|
| D1 | High | Tenant task-local is never installed by the API middleware, so the entire `kura-tenancy` fail-closed accessor layer is unreachable from HTTP | `middleware.rs:135` inserts an axum `Extension`; the only `tenantctx::scope` call sites in `crates/surface/` are `routes/evaluation.rs:564` and one audit emit at `middleware.rs:248` |
| D2 | High | Five route families carry no tenant, owner, or actor at all | `improvement.rs`, `routine.rs`, `triage.rs`, `workflows.rs`, `policy.rs` — no `Extension`/`tenant`/`principal` reference in any of them |
| D3 | High | The Docker image cannot start: a glibc binary is copied into Alpine | `deploy/docker/Dockerfile` builds on `rust:1.85-bookworm` with no `--target *-musl`, runs on `alpine:3.20`; no `.cargo/config.toml`; CI never builds the image and `release.yml` ships `-gnu` only |
| D4 | High | **Fixed 2026-09-19 (10.2).** All database access serialised through one mutex over one connection, nullifying WAL | `state.rs:149` is `Arc<Mutex<SQLiteStore>>`; `store/src/lib.rs:115` holds a single `rusqlite::Connection`; `PRAGMA journal_mode = WAL` at `:166` buys nothing with one connection |
| D5 | Medium | `tenantctx.rs` module docs claim the middleware installs the context; it does not | `crates/iam/identity/src/tenantctx.rs` header vs. `middleware.rs:82-140` |
| D6 | Medium | **Fixed 2026-09-19 (10.3).** No metrics, tracing, or OpenTelemetry — telemetry was a 227-line text logger | `crates/foundation/telemetry/src/lib.rs`; no related dependency in `crates/Cargo.toml` |
| D10 | High | No live HTTP LLM provider: the dispatcher registers only the Claude/Codex CLI bridges and echo (`plugins.rs` `build_llm`). `openai_compatible` exists as a `kura_providers` profile with base URL and key, but nothing implements `kura_llm::Provider` for it, so a configured HTTP model fails with `provider not found`. Found 2026-09-19 while scoping 9.0 on a stale base; upstream had already fixed it (`5b4e4d9`, `llm_bridge.rs`), which the merge on 2026-09-20 adopted. | `crates/surface/app/src/plugins.rs` `build_llm`; `crates/modeling/managedproviders/src/` has no HTTP bridge |
| D11 | High | Chat dispatches were never tenant-bound: `persist_dispatch` upserted the row (no tenant column in the upsert) and nothing called `bind_row_tenant`, so `GET /v1/llm/dispatches` under a bearer token listed nothing and a tenant could not see its own dispatch history. Found 2026-09-19 by the backup-under-load test; fixed the same day (`ChatStore::bind_llm_dispatch_tenant`, called from `persist_dispatch`; e2e test `chat_dispatches_are_listed_for_their_own_tenant_only`). | `crates/domains/chat/src/service.rs` `persist_dispatch`; `crates/domains/chat/src/store.rs` |
| D9 | High | The production backup script copied only `daemon.sqlite`; in WAL mode committed data lives in the `-wal` file until checkpoint, so backups had zero tables and still passed `PRAGMA integrity_check`. Every pre-existing backup/restore evidence record is unverified. **Fixed 2026-09-19** (`.backup` + migration-head validation); recorded because the evidence debt it created is not. | `scripts/production/backup-test-state.sh`; verified against `~/.kura-test` (4 KB main file, 3.2 MB WAL) |
| D8 | Medium | Egress policy does not defend against DNS rebinding: `check_url_resolved` validates the addresses it resolves, but the HTTP client resolves again for the connection and may get a different answer. Closing it means pinning the checked address through to the socket, per client. Recorded 2026-09-18 with Stage 7.3; not fixed. | `crates/foundation/egress/src/lib.rs` module docs |
| D7 | Medium | The CI clippy step can never fail: its output is piped to `tail`, so the job takes `tail`'s exit code | `.github/workflows/ci.yml`, "Clippy (deny new warnings in changed surface)". `cargo clippy --workspace --all-targets` currently errors in `kura-mcp`'s `transport_test`/`manager_test` ("this loop never actually loops") and CI is green regardless. Found 2026-09-18 while verifying Stage 8; not fixed — the lint failures are pre-existing and unrelated to this program, and turning the gate on is a change that should land with them fixed, not before. |

D2 requires a precise statement of exposure: every affected route sits behind
`protected()`, so it is **not** an unauthenticated door. It is a horizontal
privilege defect — any authenticated principal in a multi-user deployment
reaches every other principal's routines, triage policies, workflows,
improvement proposals, and **policy approvals**. `/v1/improvement/proposals/{id}/apply`
rewrites `plugins.json` for the whole daemon; `/v1/policy/approvals` is the
approval gate itself. On a single-user host the exposure is nil, which is why
it has not surfaced.

## Stage 0 — Document Truth Correction

Small, and first: the planning docs mislead a reader about what is done.

- [x] `plugin-architecture.md` lists "symbolic tool-log compression" and
      "binding-aware loadouts" as later context slices; both shipped
      (`92191d5`, `81a0529`). Correct the later-slice lists.
- [x] `daemon-roadmaps.md` Roadmap 78 program items 2–6 carry no slice
      status. Record slice 1 completion per item with its commit.
- [x] `openclaw-architecture-gaps.md` is an architecture-level comparison
      from 2026-08 and does not cover release deltas. Add a pointer to this
      document for the feature-level tracking.

**Definition of done:** an engineer reading `docs/` cannot conclude that a
shipped slice is pending. **Closed 2026-09-18.**

## Stage 1 — Retrieval Correctness and Scale

Highest priority, because the current implementation has a scaling cliff on
the reply path.

### The problem

`crates/surface/app/src/plugins.rs:811` loads **every** Ready L1 atom for the
tenant into the candidate corpus on every turn, and
`crates/domains/context/src/lib.rs:396` calls `embedder.embed()` once **per
document per turn**. With the in-process hashed-trigram default this is
merely wasteful. With an external neural embedder — which is the entire point
of the `context.embedder` seam shipped in phase 3 slice 2 — it becomes N
line-JSON RPC round-trips per turn, synchronously, on the reply path.

Secondary: `rank_of` and `candidates.contains` make fusion O(n²).

### Tasks

- [x] **1.1 Persist embeddings, embed the query only.** Done 2026-09-18.
      Schema v3 adds `memory_asset_embeddings(asset_id, fingerprint, …)` with
      a foreign key onto `memory_assets` (`ON DELETE CASCADE`). `Embedder`
      gains `fingerprint()` — two providers that could disagree on a vector
      must report different keys, since mixing spaces produces nonsense rather
      than an error. `retrieve_fused_with_vectors` /
      `retrieve_and_assemble_with_vectors` take precomputed corpus vectors;
      the context plugin reads them from the index and writes back misses.
      *Verified:* `kura-context`'s `precomputed_vectors_embed_only_the_query`
      (counting provider: exactly one call, and it is the query);
      `kura-app`'s `retrieval_caches_corpus_embeddings_and_revocation_clears_them`
      (index empty → populated after one turn → reused on the next);
      `kura-store`'s `memory_asset_embeddings_upgrade_from_v2_in_place`
      (a v2 deployment gains the table on upgrade, not just fresh installs).
      *Deviation from the original plan:* the vector is computed lazily on
      first retrieval rather than at the Ready transition. Same steady-state
      cost, one mechanism instead of two, and no new hook on the write path.
      *Rollback:* schema v3 is additive, but a v3 database refuses to open on
      a v2 daemon (`schema version … is newer than supported`). Rolling the
      daemon back therefore needs the data directory restored from backup —
      this is the first schema change of the program and the constraint is
      structural, not specific to this table.
- [x] **1.2 Bound the retrieval corpus.** Done 2026-09-18, **scope corrected**:
      the original text said "prune in the store, not in memory", which does
      not apply — `memory.list()` serves the corpus from the in-memory manager
      and already filters by tenant, layer and status, so there is no DAO in
      that path to filter. What was actually missing is the bound:
      `RETRIEVAL_MAX_CORPUS` (2000) keeps the newest atoms and records the
      remainder as `over_candidate_limit` in the AssemblyRecord, so truncation
      is never silent.
- [x] **1.3 Replace the O(n²) fusion internals** with rank index maps. Done
      2026-09-18: `rank_of`'s linear `position()` scan and
      `candidates.contains` are now one table read each. All ten retrieval
      tests pass unchanged, so ranking behaviour is identical.
- [x] **1.4 Retrieve over L2/L3, not just L1.** Done 2026-09-19. The
      retrieval corpus is now L1 + L2 + L3 in both paths (the context plugin
      and `/v1/retrieval/queries`). The real gap this closes: bootstrap
      injects L2/L3 newest-first under its own budget, so a scenario or
      persona that did not fit — or was simply not recent — was unreachable no
      matter how well the query matched it. Retrieval is the second chance.
      Assets bootstrap already injected are skipped and **recorded** as
      `already_injected` rather than silently dropped: "it was already in the
      window" is a different fact from "it did not match".
      `RetrievalDoc` gains `layer`, so the AssemblyRecord and the
      `Memory[l2 …]` citation name the real layer instead of hard-coding `l1`.
      *Found and fixed while here:* `/v1/retrieval/queries` was still
      re-embedding the whole corpus per request — Stage 1.1 fixed the plugin
      path and missed this one. Both now share `corpus_vectors` in the API
      crate, so they cannot drift.
      *Verified:* end-to-end test with a bootstrap budget that fits only the
      newest scenario; the older matching one is recalled, the newest appears
      exactly once, and the skip is in the record.
- [x] **1.6 Assembly-record read API.** Done 2026-09-19.
      `GET /v1/context/assemblies` — newest first, filterable by thread, the
      record verbatim. Built on the `context.assembled` events the plugin
      already persists rather than a new table: the event *is* the record.
      *Structural boundary, stated in the route docs and the schema:* an
      assembly cannot carry a dispatch id. The hook runs at
      `chat/pre-dispatch`, before the dispatch record exists — that ordering
      is the "model-visible = logged" invariant. Correlation is by tenant and
      thread in time order. A resolved tenant overrides the `tenantId` query
      parameter, tested.
      Full stack: schema, contract fixture, SDK `listContextAssemblies`.

**Re-cut 2026-09-19:** 1.5 ("give the model a memory lookup tool") has moved
to Stage 9 as **9.0b**. It was mis-placed: it is a *tool*, and the audit that
day established that the live chat path has no tool calling at all (see the
Stage 9 prerequisite). Leaving it here would have left Stage 1 half-done on
an item that cannot be built until that prerequisite lands.

**Retrieval benchmark (the verification this stage requires), run 2026-09-19:**
10 000 Ready L1 atoms, corpus bounded to 2 000 by 1.2, hashed-trigram
embedder, debug build: cold turn 305 ms (embeds the corpus once and persists
it), warm turns 193 ms / 155 ms (query only). The index held exactly 2 000
vectors after the cold turn. Test:
`retrieval_benchmark_10k_atoms_warm_turn_is_not_slower_than_cold` in
`kura-app`, which asserts the ordering so the number cannot regress silently.

**Stage 1 closed 2026-09-19** — every listed task met, including the
benchmark the first closure attempt skipped. 1.5 lives in Stage 9 as 9.0b
because it is a tool and depends on 9.0; that move is a dependency, and it is
stated here so nobody has to discover it.

**Verification:** `cargo test --workspace`, `make daemon-contract-test`, plus
a retrieval benchmark at 10k Ready atoms comparing turn latency before and
after 1.1/1.2.

## Stage 2 — Memory Observability and Lifecycle

Adopted from OpenClaw 2026.9.4 ("knowing what was saved or forgotten") and
2026.9.1 (`memory reset` rebuilds derived indexes without deleting sessions).

- [x] **2.1 A "what is remembered" read surface.** Done 2026-09-18.
      `GET /v1/memory/overview` reports counts per `(layer, status)`, the size
      of the derived retrieval index, and two recency lists — what was just
      remembered and what was just forgotten. "Forgotten" covers revoked,
      expired **and** superseded: all three remove an asset from recall, and
      the latter two are the ones nobody watched happen.
      Full stack: schema (`schemas/api/memory-overview.schema.json`) →
      contract fixture → SDK `getMemoryOverview` → `MemoryOverviewView` in the
      operator shell, with a `/memory/overview` navigation entry.
      *One addition beyond the plan:* the view derives and shows the gap
      between Ready assets and indexed vectors. An index that has silently
      stopped being written is otherwise indistinguishable from a healthy one
      until someone subtracts two numbers.
      *Audit F6, fixed 2026-09-19:* the first closure shipped
      `MemoryOverviewView` as a component nobody mounted — `App.tsx` imports
      none of `surfaces.tsx`, so it and the pre-existing memory views were
      unreachable. `MemoryOverviewPanel` now mounts it in the operator shell
      with the tenant-scoped data flow every other panel uses, fetched in
      `refreshShell` (best-effort: a daemon without the memory plugin must not
      take the shell down) and with the rebuild action wired to
      `rebuildMemoryIndexes`. App tests cover the mount.
- [x] **2.2 Index rebuild command.** Done 2026-09-18.
      `POST /v1/memory/indexes/rebuild` drops the tenant's derived vectors;
      they are recomputed lazily by subsequent retrievals, so this is the whole
      repair rather than step one of two. Assets and conversation truth are
      untouched — the point of a rebuild rather than a reset.
      *Tenant-scoped deliberately:* the unscoped `clear_memory_asset_embeddings`
      stays in the store but is not what the API calls. An operator repairing
      their own index must not discard every other tenant's.
      *Boundary condition worth recording:* `upsert_memory_asset` stores an
      empty tenant as SQL NULL, so an empty tenant selects the NULL rows (the
      single-user assembly's own data) rather than `= ''` (nothing) or all rows
      (every tenant's). All three overview/rebuild queries share that rule.
- [x] **2.3 Revocation must clear derived indexes.** Done 2026-09-18, in the
      same slice as 1.1 as planned. Enforced at `finish_mutation` in the memory
      route family — the single choke point every asset mutation already
      passes through (revoke, reject, supersede, retention expiry) — rather
      than at each call site. Any status that is neither Ready nor Pending
      drops the asset's cached vectors.
      *Verified by reversion:* removing the call makes
      `retrieval_caches_corpus_embeddings_and_revocation_clears_them` fail on
      "revocation must clear the derived index".

**Tradeoff, stated plainly:** Stage 1.1 trades an invariant that currently
holds for free against per-turn latency that currently scales linearly with
corpus size. That is the right trade at scale, but it is a real loss of
safety-by-construction, so 2.3 ships in the same slice as 1.1.

**Stage 2 closed 2026-09-19** (after the F6 fix; the first closure was premature).

## Stage 3 — Skill Lifecycle

Slice 1 gave propose → review → publish. What OpenClaw 2026.9.4 shipped and
we have not is the *acquisition* half.

- [x] **3.1 Distill skill proposals from past conversations.** Done
      2026-09-19. `POST /v1/skills/proposals/distill {threadId, guidance?}`
      loads the source thread's continuity (safe content), and runs the
      distillation **as a chat turn** on `skill-distill:<threadId>` — a real
      dispatch, hooked, persisted, readable — rather than a background job.
      Operator `guidance` is placed first in the prompt and may be as concrete
      as a corrected draft; replying on the distill thread and calling again
      is the steering loop. The model's JSON draft becomes an ordinary Pending
      `kind=skill` proposal with evidence links to both threads, so review and
      publication are unchanged. A parse failure returns the dispatch id and
      the reason rather than a proposal.
- [x] **3.2 Skill search.** Done 2026-09-19. `GET /v1/skills/search?q=`
      reuses the Stage 1 fused ranker over the installed registry (no second
      ranker); vectors are computed per call because the corpus is tens of
      skills and the derived index is keyed to memory assets. Each hit carries
      the skill's usage record.
- [x] **3.3 Skill versioning via supersede.** Done 2026-09-19. A republish
      appends a version to the **same** catalog item instead of registering a
      second one, and the bundle's frontmatter now carries
      `provenance: memory:<asset>`, `version`, and `supersedes: <previous
      provenance>`, so any SKILL.md traces to the approved asset it came from
      and the one it replaced.
- [x] **3.4 Usage feedback into the evaluation plane.** Done 2026-09-19,
      **scope stated honestly:** invocations are counted automatically (the
      `chat/turn-end` payload now carries the turn's selected skills and a
      `skills` hook records them); `helpful` / `corrected` are **explicit**
      feedback via `POST /v1/skills/{id}/feedback`, because inferring a
      correction from the next message is a guess and this record is meant
      to be trusted. Search results carry the record, which is the "signal the
      catalog carries". Schemas, contract fixtures, SDK.

**Stage 3 closed 2026-09-19.**

## Stage 4 — Concurrent Sub-Agents

OpenClaw 2026.9.2 made Swarm the default. Kura's `crates/domains/orchestration`
is a workflow planner — a sequential DAG with handoffs — not concurrent
sub-agents.

- [x] **4.1 Quota bound.** Tool-plane half landed 2026-09-18; concurrency half landed 2026-09-19 with `SwarmQuotaGateImpl`: every child of a fan-out reserves one `RUN_LAUNCHES` unit **before it spawns**, commits on a result, releases on failure, and fails closed when the quota plane is unavailable. As with tools, `ChildQuotaGate` has no permissive default and the only always-allow gate is named `ExplicitlyUnboundedChildQuota`.
      `kura_tools::ToolRuntime` is the guarded path every tool call takes, and
      the order is structural rather than a convention each family remembers:
      quota reserve -> egress check (resolving, because this is call time) ->
      provider -> **commit on success, release on failure**. A call that
      produced no answer must not consume the tenant's budget; that is the
      easiest thing to get wrong and has its own test.
      `ToolQuotaGateImpl` binds it to billing's existing `RUNTIME_TOOL_CALLS`
      category (daily, one unit, reserved "before invocation") — **no new
      category, no contract change.** The gate fails closed: an unavailable
      quota plane denies rather than granting unbounded third-party spend.
      *Shaped by Roadmap 75's finding, not merely aware of it:* that roadmap
      recorded `quota: None -> AllowAllQuota` on the webhook manager as "a real
      exposure, not just defense-in-depth". So `QuotaGate` has **no permissive
      default implementation**; the only always-allow gate is named
      `ExplicitlyUnboundedQuota` so it cannot be mistaken for one, and the
      `tools` plugin declares `requires: &["billing"]` so the plane does not
      assemble without the quota plane at all. An assembly test asserts that
      dependency and fails when it is removed (verified by removing it).
      *Still open — the concurrency half:* Stage 4's sub-agent fan-out has its
      own spend multiplication and policy-gate races. This closes the bound
      Stage 9 needed, not Stage 4.
- [x] **4.2 Sub-agent execution on the hook seam.** Done 2026-09-19. A child
      turn *is* `chat::Service::query` on its own thread
      (`swarm:<run>:<index>`), so it passes through `chat/turn-start`,
      `chat/pre-dispatch` and `chat/turn-end` and persists its own dispatch
      record — "model-visible = logged" holds per child without the swarm
      crate knowing hooks exist. The app test registers a counting `chat/pre-dispatch`
      hook before the launch and asserts it fired once per child. Concurrency is bounded by `maxConcurrentChildren`
      (scoped-thread batches), children per run by `maxChildrenPerRun`.
      **Opt-in, not default-on** (OpenClaw 2026.9.2 chose default-on): the
      `swarm` plugin refuses every launch with 403 until
      `swarm.config.enabled = true`, tested.
- [x] **4.3 Structured results and live progress events.** Done 2026-09-19.
      `SwarmRun` carries every child's terminal state (`completed | failed |
      quota_denied | cancelled`) with dispatch id, output preview and error;
      `run.summary()` is the structured answer. Events `swarm.run_queued`,
      `run_started`, `child_started`, `child_completed`, `child_failed`,
      `child_quota_denied`, `run_completed` are persisted and published as
      each transition happens. Runs are persisted as manager documents and
      restored at boot; children a restart interrupted are marked cancelled
      rather than left pending forever. `GET|POST /v1/swarm/runs`,
      `GET /v1/swarm/runs/{id}` with document-level tenancy; schema, contract
      fixture, SDK `launch/list/getSwarmRun`.

**Stage 4 closed 2026-09-19.**

**Risk:** this is the item most likely to be cut. If Stage 1–3 slip, drop
Stage 4 rather than shipping unbounded concurrency.

## Stage 5 — Session Frames and Thread Segmentation

Both are already named as later slices in `plugin-architecture.md`.

- [x] **5.1 Session frame objects.** Done 2026-09-19. `SessionFrame`
      (goal + constraints) persisted as a manager document keyed by thread,
      `GET|PUT|DELETE /v1/threads/{id}/frame` with document-level tenancy,
      SDK `get/set/clearSessionFrame`, schema + contract fixture. The
      session-strategy plugin injects it **first** at `chat/pre-dispatch` as a
      system message — the class `shape_window` never elides — and replaces
      rather than stacks it on later turns. App test: with a 120-char budget
      the history is elided and the frame is still message zero.
- [x] **5.2 Channel-native thread segmentation policies.** Done 2026-09-19.
      Built on the mechanism the threads engine already had: a thread's
      continuity is scoped to its current *session segment* and `reset_thread`
      opens a new one. The policy (`session-strategy.config.channelSegmentation`,
      keyed by connector id with `"*"` default, `idleGapSeconds`) is evaluated
      by a new `chat/turn-start` hook — before continuity is assembled, so the
      turn sees the fresh segment — and applied through
      `apply_thread_lifecycle_action(Reset)`, so an automatic boundary leaves
      the same audited lifecycle action and reset evidence as an operator
      reset (`reason_code = idle_gap_segmentation`). The turn-start payload
      gained `channelScopeRef` and `sourceTimestamp` to make that decidable.
      Personal threads are never auto-segmented. App test: a thread idle 2 h
      under a 1 h policy opens a new segment and lands in `Reset`; one active
      60 s ago keeps its segment.

**Stage 5 closed 2026-09-19.**

## Stage 6 — Self-Improvement Surface

- [x] **6.1 Widen the target set.** Done 2026-09-19. Two constants became
      tunables — `context.retrievalMaxCorpus` and `context.vectorMinSimilarity`
      — and proposals are validated against an **allowlist**
      (`KNOWN_TARGETS`: the context budgets, corpus bound, similarity
      threshold, ref threshold, and the session-strategy budgets) with a
      numeric-value check. An allowlist rather than "any profile key" because
      `apply` rewrites `plugins.json` for the whole daemon: a key the agent
      invented would land a silent no-op, or a typo that disables a plugin.
- [x] **6.2 Require an evaluation result on every proposal.** Done
      2026-09-19. `ProposeInput.evaluation {kind, id, metric, baseline,
      observed}` is required (`EvaluationRequired`); for `replay_attempt` the
      route confirms the attempt exists in the evaluation plane. "Predicted
      effect" is now an assertion resting on a measurement the record names.

**Stage 6 closed 2026-09-19.**

## Stage 7 — Ops and Security Hardening

Sequenced after the capability lines by operator decision. These are
prerequisites for the Roadmap 77 launch gate, not for this program.

- [x] **7.1 Config write path with compare-and-set.** Done 2026-09-19.
      `GET|PUT /v1/config/file` — the operator-owned `config.json` itself,
      deliberately distinct from the effective `/v1/config` (which is
      defaults + env + resolved secrets and must never be written back).
      CAS via `expectSha256` (409 with the live hash on mismatch), `dryRun`,
      and `strict` (unknown keys become errors instead of the silent no-op
      that `kura_config::load` otherwise makes of them). Atomic temp+rename,
      the same shape `kura config set` already used. Owner-only.
      **The hot-apply boundary is published as an empty list**
      (`HOT_APPLY_SETTINGS`), and every write reports `restartRequired: true`
      with the changed sections named. Nothing hot-applies today; the full
      restart is said out loud rather than a partial one implied.
      *Rule enforced, not stated:* inline secret values
      (`llm.openaiCompatible.apiKey`, the connector tokens) are refused with
      400 — the daemon must never be the thing that puts a key on disk. A
      hand-edited file carrying one is still read, and redacted on the way
      out. Validation in `kura_config::validate_file_config_json` round-trips
      the decoded `FileConfig` so the unknown-key check needs no
      hand-maintained key list. Schema, contract fixture, SDK
      `getConfigFile`/`writeConfigFile`.
- [x] **7.2 Upgrade rehearsal in isolated candidate state.** Done
      2026-09-19. `kura daemon rehearse-upgrade [--keep]` →
      `kura_app::rehearse_upgrade`: snapshots `daemon.sqlite` with
      `VACUUM INTO`, copies `plugins.json`, `plugins/`, `config.json` and
      `tenant-secret-values/` into a 0700 scratch dir, then runs the real
      `App::new` against the copy — migrations, plugin profile, every manager.
      The source is opened with a new `open_unmigrated` and never migrated;
      the report re-reads its version afterwards and fails if it moved.
      Checks: candidate at `CURRENT_SCHEMA_VERSION`, `integrity_check == ok`,
      **table count > 0** (an intact empty database is what a lost snapshot
      looks like), plugin warnings and disabled plugins surfaced. A failing
      rehearsal keeps the scratch dir for inspection; a passing one cleans up.
      The report states activation and rollback in words, because the answer
      flips at one moment: before the real daemon starts on the new binary,
      rollback is nothing; after, it is restore from a proper snapshot.
      *Verified:* a v(N-1) source rehearses to head while staying at v(N-1);
      a malformed `plugins.json` fails the rehearsal instead of the real boot.
      **Found while building it — D9.** `scripts/production/backup-test-state.sh`
      did `cp daemon.sqlite`. The daemon runs WAL mode, so committed data
      lives in `daemon.sqlite-wal` until a checkpoint; the test data dir had a
      4 KB main file beside a 3.2 MB WAL. Verified: the script's backup had
      **zero tables** and no `schema_migrations`, and `PRAGMA integrity_check`
      said `ok` on it. Every Roadmap 39 backup/restore evidence record is
      therefore unverified. Fixed to `.backup` (the online backup API, reads
      through the WAL) plus a validation that the backup's migration head
      matches the source — the check that would have caught this.
- [x] **7.3 Outbound-request (SSRF) policy.** Done 2026-09-18 —
      new `kura-egress` foundation crate, wired as `config.egress` and applied
      to tool-profile `baseUrl` at create and edit.
      **Scope corrected:** the audit said "all three egress points — browser,
      webhook, outbound fetch", copied from OpenClaw's list. Checked against
      this codebase: **the webhook plane is ingress-only** (`TargetKind` is
      `routine|workflow|run`; firing launches an internal workflow, never an
      outbound HTTP call) and the browser capability is a README stub. The real
      egress today is the LLM provider `baseURL`, the IM connectors' fixed
      vendor endpoints, and MCP streamable-http — **none of which takes a
      model-controlled URL.** So this is not three holes being plugged; it is
      the gate built before model-influenced URLs arrive with 9.2.
      *Defaults deny* loopback, RFC1918, carrier-grade NAT and unique-local,
      and **unconditionally** the link-local range — `169.254.169.254` is the
      cloud metadata service and no agent has a legitimate reason to read it,
      so it is not behind the private-network flag.
      *Stated boundary, asserted by a test rather than left implied:*
      `check_url` is syntactic and does **not** resolve; `check_url_resolved`
      does. The split is deliberate — one call that sometimes resolves is how a
      validation call gets mistaken for a security boundary. DNS rebinding is
      **not** defended against; that needs the checked address pinned through
      to the socket, which belongs in the HTTP client layer and is recorded as
      an open gap below rather than implied here.
      *Covered by tests:* IPv4-mapped metadata (`::ffff:169.254.169.254`),
      subdomain and case coverage in the hostname blocklist, scheme allowlist,
      and the `https:///internal` normalisation surprise (the `url` crate turns
      the first path segment into the host).
- [x] **7.4 Keep cross-channel delivery default-deny.** Recorded
      2026-09-19 in `docs/providers/tool-provider-architecture.md`, where a
      `message.send` capability would appear. Verified first, then recorded:
      the reply path copies `connector_id`/`channel_id` from the inbound
      message (`im/src/lib.rs:1659`, `:2485`), proactive delivery targets are
      operator-configured by result class, and the chat path has no tool
      calling — so today there is no model-directed send to deny. The
      decision binds the future capability: default-deny across connector
      bindings unless an operator-written preference names the target.

**Stage 7 closed 2026-09-19.**

## Stage 8 — Multi-Tenant Assembly Closure

The tenancy *planes* are built and tested; the *assembly* stops at
single-user. This stage closes D1, D2, D3 (tenant half), and D5.

Recorded history: Roadmap 74 left "per-handler tenant-context integration" as
a residual, Roadmap 75 moved the remainder to Roadmap 76 as a pre-soak
prerequisite. That framing understates it — the residual is not a set of
hosted-credential edge cases, it is the generic enforcement path.

- [x] **8.1 Install the tenant task-local in `protected()`.** Done
      2026-09-18. `protected()` now drives the downstream handler inside
      `tenantctx::scope`, using the same resolved value it attaches as an
      extension, so the two cannot disagree. This makes the existing
      `kura-tenancy` accessors live — 1365 lines of already-tested
      fail-closed behavior that previously could not fire.
      *Verified:* `middleware::tenant_task_local_tests` — one test asserting
      `require()` returns the tenant inside an arbitrary handler (it fails
      with `ERR:tenant context required` when the fix is reverted, confirmed
      by reverting it), one asserting the no-identity-manager assembly still
      fails closed rather than defaulting to an ambient tenant. Full
      workspace suite, contract tests, and the route-parity gate green.
      *Correction to the original DoD:* `routes/evaluation.rs`'s
      `with_tenant_context` is **not** removed. It already prefers an
      existing task-local, so it degrades to a passthrough in production,
      and it still serves tests that build requests without the middleware.
      Removing it would be regression risk for no functional gain.
      *Known limit:* a tokio task-local is not inherited by `tokio::spawn`.
      Work detached from the request task must carry the tenant explicitly —
      recorded in the `protected()` doc comment and a follow-up for 8.2's
      audit of the affected handlers.
      *Rollback:* revert the one-line scope; handlers that read the
      `Extension` directly are unaffected either way.
- [x] **8.2 Scope the blind route families.** Done 2026-09-18.
      - `policy.rs` — the engine holds approvals in memory for the whole
        daemon, so the list intersects it with `list_approvals_for_tenant_raw`
        and writes bind the `approvals`/`decisions` rows; by-id and resolve are
        covered by `ByIDTenantGuardLayer`.
      - `routine.rs` / `triage.rs` — both are backed by `manager_documents`,
        which already had a `tenant_id` column that the managers were writing
        as `""`. Added `bind_manager_document_tenant`,
        `lookup_manager_document_tenant`, and
        `list_manager_document_ids_for_tenant` to the store, plus three shared
        helpers in `routes/mod.rs`. The managers rewrite the document on every
        save, so ownership is re-bound after each mutation.
      - `workflows.rs` — addressed under `/v1/runs/{run_id}`, so the run is the
        tenant-owned resource; one `ByIDTenantGuardLayer` on `/v1/runs/` covers
        all four routes. Workflow rows are also bound so
        `kura-tenancy`'s `list_workflows_for_tenant` is not silently empty.
      - `sandboxes.rs` — **surfaced by the 8.4 gate, not by the original
        audit**, which had waved it through on the strength of its
        `requested_by` attribution. Executions are per-principal (filtered list
        + by-id guard + row binding); profiles are operator configuration and
        their reload carries the 8.3 guard.
      *Reclassified:* `improvement.rs` moved to 8.3. Its apply rewrites
      `plugins.json` for the whole daemon, so it is not per-tenant data and
      tenant filtering would have been the wrong model.
      *Verified (corrected by audit F2, 2026-09-19):* the first closure
      claimed a two-tenant test per family with reversion for each. In fact
      `sandboxes.rs` had none and reversion had been run for two families.
      Now: six two-tenant tests, and reversion verified for all six (policy,
      workflows, routine, triage, sandboxes each fail with their guard
      neutralised). Writing the sandboxes test also caught a real trap — the
      manager persists through a *second* store on the data dir
      (`asm.secondary` in production), which the test must mirror.
- [x] **8.3 Decide and record the scope of the global-by-design families.**
      Done 2026-09-18. `require_daemon_global_operator` gates
      `PUT /v1/plugins/profile`, the four `/v1/improvement/proposals` routes,
      `POST /v1/capabilities`, and `POST /v1/sandboxes/profiles/reload`.
      Denials are 403 (nothing to disclose, unlike the by-id 404).
      **Recorded decision:** the bar is `Role::Owner`, not a new `Permission`
      variant — no existing permission expresses "may change the daemon's
      assembly", and adding one is a cross-language contract change that
      deserves a deliberate decision rather than arriving as a side effect.
      `config.rs` is read-only with redactions and needs no further guard.
- [x] **8.4 A negative-path route audit gate.** Done 2026-09-18 —
      `crates/surface/api/tests/route_tenancy_audit.rs`. Every route-family
      module must carry a tenancy mechanism or appear in `GLOBAL_BY_DESIGN`
      with a recorded reason; markers inside `#[cfg(test)]` do not count. A
      second test fails if an exemption names a module that no longer exists,
      so the list cannot silently widen. **It earned its place on the first
      run** by catching `sandboxes.rs`, which the manual audit had missed.
- [x] **8.5 End-to-end multi-tenant verification.** Done 2026-09-18 —
      `crates/surface/app/tests/multi_tenant_e2e.rs`. Builds a real `App`,
      seeds two tenants with their own principals, memberships, grants and
      tokens, and drives plain HTTP requests carrying **only a bearer token**.
      Three tests: the token alone resolves a tenant (and an unauthenticated
      request is still 401, so the isolation below is not vacuous); two tenants
      in one process cannot enumerate, read, or act on each other's approvals
      or triage policies; and several sessions per tenant stay within their
      tenant.
      *Coverage boundary, stated because it matters:* these tests still pass
      with the 8.1 `tenantctx::scope` call removed, because every route closed
      in 8.2 reads the `TenantContext` extension rather than the task-local.
      They prove D2; D1's proof remains the middleware unit test, which was
      verified by reversion. See 8.7.
- [x] **8.7 Resolve the two tenancy mechanisms.** Decided and recorded
      2026-09-19 in `crates/iam/tenancy/src/lib.rs`: the API-layer helpers
      (`bind_document_tenant`, `guard_document_tenant`,
      `tenant_visible_document_ids`, `ByIDTenantGuardLayer`, the per-family
      list intersections) are **the** production path; `kura-tenancy` is kept
      as the executable specification of the cross-tenant semantics the store
      primitives must honour, and its structs are not to be wired. The
      alternative — re-plumbing a shared store handle through 16 accessors —
      was rejected for having no behavioural gain. One mechanism, one spec.
- [x] **8.6 Correct the `tenantctx.rs` module documentation** (D5). Done
      2026-09-18; the header now names `protected()` as the installer and
      states the `tokio::spawn` propagation limit.

**Definition of done:** a multi-user deployment cannot reach another
principal's data through any route family, proven by 8.5 and enforced
against regression by 8.4. Audit 2026-09-19 found the first closure unmet (no sandboxes two-tenant test, reversion for 2 of 6, 8.7 open); all three fixed the same day. **Stage 8 closed 2026-09-19.**

## Stage 9 — Built-in Tools

> **Prerequisite discovered 2026-09-19, not in the original plan.** The live
> chat path has **no tool calling**: `chat/src/service.rs` (1647 lines) has
> zero occurrences of "tool", and `kura-llm`'s dispatch types carry no tool
> definitions or tool-call requests. The workflow planner is deterministic
> keyword matching (`orchestration` has zero `llm` references). A second,
> tool-capable agent loop exists — `crates/engine/core` ("mirrors
> `codex-rs/core`": `ToolRegistry`, turn loop) over `crates/engine/model-provider`
> (`ToolSpec`, OpenAI streaming `tool_calls`) — but **nothing depends on it**.
>
> Consequence: the tool plane built in 9.1b is a complete, tested mechanism
> that the conversational agent cannot decide to use. 9.2–9.5 must not start
> until 9.0 lands. This was a planning error: the tool-provider design assumed
> tool calling existed and did not check.

- [x] **9.0 Tool calling in the live chat path.** Closed 2026-09-20 on
      the **upstream** design, option (a). While this program was being
      built on a stale base, `origin/main` (commits `d34e740`…`387d15a`,
      2026-09-02) landed tool calling by driving the loop `kura-core`
      already had: `kura-llm` carries `tools`/`tool_calls` (persisted as
      `tools_json`/`tool_calls_json`, schema v4), `chat/src/round.rs` makes
      one dispatcher round a `ModelProvider`, and `ToolSource` supplies a
      `kura_core::ToolRegistry` per turn; MCP tools ride `kura_mcp::McpTool`
      with approval waits. The option-(b) loop written here was dropped at
      the merge rather than kept as a second loop. What this program adds
      on top: `ToolSource::registry(&ToolTurn)` (the tenant, thread and
      agent profile the tools are resolved for), the `chat/tool-call` hook
      point, `chat.tool.called` events and tool-call metrics (all in the
      `Observed` wrapper of `crates/surface/app/src/tool_host.rs`), a
      per-tenant token-spend metric, and a `chat` plugin config for
      `toolMaxRounds`. *Recorded omission:* the `toolTrace` field the
      option-(b) response carried was dropped with it; tool calls are
      observable through `chat.tool.called` events and `/metrics`.
      *Found on the way (D10):* there was no live HTTP LLM provider on the
      stale base; upstream fixed it independently (`5b4e4d9`,
      `engine/model-provider/src/llm_bridge.rs`), so the provider crate
      written here was dropped at the merge.
- [x] **9.0b Memory lookup tool** (re-cut from 1.5). Closed 2026-09-19:
      `memory.lookup` in `crates/surface/app/src/tool_host.rs` — a query
      runs the same `run_query` as `/v1/retrieval/queries` (Ready, private/
      team, tenant-scoped, same derived vector index); an `assetId` fetch
      applies the same visibility rules, so a lookup by id cannot reach past
      what retrieval would rank (test seeds a restricted atom and asserts it
      is not recalled). Hits cite `Memory[<layer> <assetId>]`.

Kura today is a tool *host* with no batteries: the MCP plane, the skill
`execution.*` escape hatch, and the runtime's `LocalTool`/`McpTool`/`DomainTool`
call ledger all exist, but no web search, image generation, video generation,
or real browser ships with it. `crates/domains/computeruse` defaults to
`MemoryDriver` (`manager.rs:58`) and no real driver exists anywhere in
`crates/`; every directory under `capabilities/` is a README placeholder.

The design question this stage must answer first: **built-in tools should be
default plugins, not kernel features.** Phase 3 already supports external
stdio plugins with seam dispatch, and that is the right tier — it keeps the
kernel trust boundary intact and makes each tool disableable and replaceable.

- [x] **9.1 Tool provider architecture.** Designed 2026-09-18 —
      [`../providers/tool-provider-architecture.md`](../providers/tool-provider-architecture.md).
      Four planes (capability / family / auth mode / profile), mirroring the
      LLM provider split but with a **data-driven registry**: `LlmConfig`
      carries one struct field per provider, which would make every new tool
      vendor a recompile. Credentials live only in the tenant secret plane as
      a `secret_ref`; the profile has no credential column by construction.
      Egress and spend are declared in the profile, not assumed. Third-party
      providers ride the existing external-plugin seam, so they need no daemon
      change.
      *The design is deliberately vendor-agnostic:* choosing a search vendor
      is a later, smaller decision that becomes filling in one profile.
- [x] **9.1b Implement the capability seam and profile storage.** Mechanism
      landed 2026-09-18; reverted to open 2026-09-19 by audit (`ToolRuntime`
      had no production caller); **re-closed 2026-09-19** once the chat tool
      host became its caller (`AppToolHost::call_capability` →
      `ToolRuntime::search`/`invoke`). New `kura-tools` crate (capability / family / auth mode /
      profile, the `SearchProvider` seam, the `builtin_stub` family, and
      `check_profile`); schema v4 `tool_profiles`; the `tools` builtin plugin
      restoring profiles at boot; `/v1/tools/{capabilities,profiles,…}`;
      schemas + contract fixtures; SDK types and methods.
      *The credential rule is enforced in three places, not stated once:* the
      struct has no field that could hold a value (unit test asserts the
      serialized field set), the API responses carry only `secretRef` +
      `secretConfigured` (route test), and the JSON schema is
      `additionalProperties: false` with no value-shaped property (**a contract
      test feeds it a profile carrying `apiKey` and asserts rejection**).
      *"One default per capability" is unrepresentable rather than validated:*
      a partial unique index refuses a second default even if a caller bypasses
      the manager, proven by a store test.
      *Provable without a vendor account, as designed:* every test above runs
      against `builtin_stub`, whose URLs are in the RFC 2606 reserved domain so
      it cannot become an accidental egress.
      *Deliberate omission:* the quota gate is **not** wired yet — `ToolLimits`
      carries `maxCallsPerDay` and the design requires the reservation, but no
      networked family exists to bound. It lands with 4.1, before 9.2.
- [x] **9.2 Web search** — closed 2026-09-19 via the `mcp_backed` family
      (operator decision: users configure their own provider; provider-hosted
      search was rejected). A profile names the user's MCP server and tool;
      the model is offered the tool's real `inputSchema` as `web.search`;
      calls are authorized under the MCP exposure rules for the `chat`
      surface, then run through `ToolRuntime::invoke` (quota → provider →
      commit/release). A profile whose server is not live is not offered.
      `builtin_stub` proves the path end to end in the assembly test. MCP
      `Tool` now retains `inputSchema` (rows discovered before this field
      refresh on next discovery).
      *Sharp edge recorded in the design:* search results are
      attacker-influenced URLs. The capability returns URLs and snippets;
      fetching one is a separate, separately-gated action through the same
      SSRF blocklist, or the tool is a confused deputy.
- [x] **9.3 Image generation** — closed 2026-09-19: same `mcp_backed` path
      as 9.2 under the `image.generate` capability; the user's MCP image
      server's `inputSchema` and result are passed through verbatim (MCP
      `content` blocks are rendered as text, other outputs as JSON).
      *Deliberate omission, recorded:* results are not yet copied into the
      artifact plane (`crates/domains/artifacts`); the model sees whatever the
      server returns (typically a URL or a text handle). Artifact capture is a
      follow-up, not part of this closure.
- [x] **9.4 Real browser driver** — closed 2026-09-19. `SubprocessDriver`
      (`crates/domains/computeruse/src/subprocess_driver.rs`) implements the
      existing `Driver` trait over a line-JSON stdio protocol
      (`capabilities/browser/PROTOCOL.md`): per-request timeout, kill and
      respawn on exit/hang/malformed output, every outcome reported to the
      capability supervisor as health or failure (visible with backoff and
      circuit-break at `/v1/capabilities`). Enabled by
      `entries.computer-use.config.driver = "subprocess"`; manager, policy
      gating and artifact recorder unchanged. The reference worker
      `capabilities/browser/worker.mjs` drives Chromium through Playwright
      (navigate/back/forward/wait/screenshot/snapshot/click/input/select,
      PNG screenshots and text snapshots as captures).
      *Verified:* driver supervision against a protocol-conformant fake
      worker (round trip with base64 capture decoding, crash → respawn,
      hang → timeout, garbage → failure, missing binary); assembly wiring
      test; and a manual smoke of `worker.mjs` on this machine against a
      real headless Chromium (Playwright 1.63, macOS: `input`, `screenshot`
      7 KB PNG, `snapshot`, `close`). No CI job runs Chromium.
      *Recorded omission:* `download` returns `unsupported_action` in the
      worker; the in-memory driver's synthetic download stays for tests.
- [x] **9.5 Video generation** — closed 2026-09-19 with 9.3: `video.generate`
      through the same `mcp_backed` path; same artifact omission recorded.

**Tradeoff:** every tool here is an egress point and a cost centre. None
lands before 7.3 (SSRF blocklist) and the quota bound from 4.1, or we ship
an unbounded spend and exfiltration surface.

**Note on `capabilities/`:** five placeholder READMEs describing reserved
workers is worse than an empty directory — it reads as existing structure.
Either 9.4 fills `browser/` or the placeholders are deleted.

## Stage 10 — Server Deployment Readiness

`docs/runtime/production-install.md` scopes itself to "tenant-scoped
single-node baseline"; the systemd unit is the most mature artifact in the
tree. The gap to a team server is D3, D4, D6, and transport security.

- [x] **10.1 Fix the Docker image (D3).** Done 2026-09-18. Runtime stage
      moved from `alpine:3.20` to `debian:bookworm-slim`, matching the build
      stage's libc and the `-gnu` targets `release.yml` ships; the image also
      now carries `KURA_VERSION`, which the Alpine version silently dropped.
      A `docker-image` CI job builds the image and polls `/healthz` for 30s,
      so the defect cannot return silently.
      *Not verified locally:* no Docker on the authoring host — the CI job is
      the verification, and it is the reason the job is part of this task
      rather than a follow-up.
- [x] **10.2 Replace the single-connection store (D4)** — closed 2026-09-19.
      `kura_store::StorePool` keeps the one existing connection as the
      writer and opens `store.readers` query-only reader connections
      (`PRAGMA query_only = ON`: a reader physically cannot write, so a
      misrouted mutation fails instead of racing). `AppState.store_pool`
      serves 97 read-only handler call sites (list/get/lookup across
      resources, evaluation, auth, channel management, integrations,
      workspace bindings, policy, runs, sessions, memory, retrieval,
      providers, and the by-id tenant guard); mutations keep the writer.
      Every route test and assembly test now runs with `readers = 2`, so a
      write on the read path cannot ship. Wait time is measured per role
      and exported (`kura_store_lock_wait_seconds{role}`).
      *Measured* (`crates/surface/app/tests/store_pool_load.rs`, 8 concurrent
      sessions × 3 rounds of chat + 4 list reads, this machine): with
      `readers = 4` all 72 reads were served by readers with 0.57 ms total
      wait; with `readers = 0` the same 72 reads waited 29.6 ms in total on
      the shared writer (max 1.9 ms). The test asserts the structural
      properties, not the timings.
      *Rollback as designed:* `readers = 0` (the default) is the pre-pool
      behaviour exactly; the pool ships dark until `store.readers` /
      `KURA_STORE_READERS` is set.
      *Recorded limit:* the ~55 manager-owned secondary connections and the
      `state.store` writes are unchanged; the pool addresses the API read
      path, which is where concurrent users queue.
- [x] **10.3 Observability (D6)** — closed 2026-09-19. `kura_telemetry::metrics`
      is a dependency-free registry rendered as Prometheus text at
      `GET /metrics` (a protected route: the exposition carries tenant ids).
      Series: `kura_http_requests_total{route,method,status}` and
      `kura_http_request_duration_seconds{route}` (route *template*, never a
      raw path), `kura_llm_dispatch_duration_seconds{provider,outcome}` and
      `kura_llm_dispatches_total`, `kura_llm_tokens_total{tenant,provider,kind}`
      (recorded in the chat service, which knows the tenant),
      `kura_store_lock_wait_seconds{role}`, `kura_hook_duration_seconds{point}`,
      `kura_chat_tool_calls_total{name,outcome}` and its duration. Every
      response carries `x-request-id` (echoed or generated) and the daemon
      writes one structured `http.request` access-log line per request with
      that id, the tenant, the route template, status and latency; the
      access log and instrumented crates share one global logger installed
      at serve time. Route test `observability_tests` covers counting,
      request ids and the exposition.
- [x] **10.4 Transport security** — decided and shipped 2026-09-19:
      terminate at a reverse proxy, never in-process. `deploy/docker/Caddyfile`
      and `docker-compose.tls.yml` put Caddy (automatic Let's Encrypt) in
      front of the daemon, remove the daemon's host port, keep SSE streams
      unbuffered, forward `X-Request-Id` into the daemon's access log, and
      set HSTS; `deploy/README.md` documents it and states the rule: a team
      deployment reachable beyond loopback needs TLS *and* bearer auth.
      *Not verified here:* no Docker on this host; the overlay is a
      configuration artifact, exercised the first time the operator runs it.
- [x] **10.5 Access-token durability** — confirmed and tested 2026-09-19.
      Tokens issued through `POST /v1/auth/tokens` are persisted by the route
      (`upsert_access_token`) and restored into the auth manager at boot;
      `protected()` also persists on first use. `access_tokens_survive_a_restart_and_revocations_stick`
      builds a second `App` over the same data directory and shows an issued
      token still authenticates, the seeded token too, and a token revoked
      before the restart is still refused.
- [x] **10.6 Backup and restore under multi-user load** — closed 2026-09-19.
      `backup_restore_load.rs`: two tenants run chat turns continuously while
      an online snapshot (`snapshot_to`, the same SQLite backup API the
      production script uses since D9) is taken; the snapshot opens as a
      fresh data directory with `readers = 2`, reports the current schema
      version and its tables, lists exactly each tenant's sessions (5/3) with
      no leakage, and pre-backup tokens authenticate. The runbook gained a
      section on online backups under load and the pool. Finding D11 fell
      out of this test.

**Explicitly still out of scope:** multi-node. Single-node with a real
connection pool, TLS, and observability is the target; horizontal scale-out
is a separate program and needs a storage engine decision this document does
not make.

## Explicitly Out of Scope

- Native apps (Android/iOS/macOS/Linux desktop), Apple Watch — outside the
  current client surface (`web`, `kura-tui`, operator shell).
- Cloud worker matrices (Daytona, Azure, Windows/WSL2 workers). `BackendKind`
  already declares `Ssh` and `Remote`
  (`crates/domains/sandbox/src/lib.rs:62`), but the remote execution control
  plane is a recorded deferred, non-gating item from Roadmap 20.
- Multi-node / horizontal scale-out (see Stage 10).
- Node/Bun/SQLite runtime work — irrelevant to a Rust control plane.
- Per-channel formatting parity with OpenClaw's connector matrix.

## Sequencing

**Revised 2026-09-18.** The original decision was "capability lines first,
ops last". The audit changed the inputs: Stage 8 contains data-isolation
defects (D1, D2) and Stage 10.1 is a 30-minute fix for a shipped artifact
that cannot start. Both now precede the capability lines. This revises, and
is meant to be read as revising, the earlier sequencing decision.

```
Stage 0   docs truth                    small, unblocks accurate reading
Stage 10.1 Docker image fix (D3)        cheap, fixes a broken shipped artifact
Stage 8   multi-tenant assembly         data-isolation defects; 8.4 stops regression
  ├ 8.1 task-local          (D1)
  ├ 8.2 five blind families (D2)
  ├ 8.3 global-by-design guards
  ├ 8.4 route audit gate
  ├ 8.5 e2e two-tenant verification
  └ 8.6 doc fix             (D5)
Stage 1 + 2.3  retrieval scale + revocation invariant (one slice)
Stage 2   memory observability and rebuild
Stage 7.3 SSRF blocklist                prerequisite for Stage 9
Stage 9   built-in tools                9.1 seam → 9.2 search → 9.3 image → 9.4 browser
Stage 3   skill lifecycle
Stage 10.2-10.6  store pool, observability, TLS       after 8.5 pins correctness
Stage 5   session frames                interleavable
Stage 6   self-improvement              needs Stage 1 and 3 targets
Stage 7.1/7.2/7.4  config CAS, upgrade rollback, delivery default-deny
Stage 4   concurrent sub-agents         first to cut under pressure
Stage 9.5 video generation              second to cut
```

Dependency edges that must not be reordered:

- **2.3 ships with 1.1.** Persisting embeddings breaks the revocation
  invariant that currently holds for free.
- **7.3 and 4.1 precede Stage 9.** Tools are egress and spend; the blocklist
  and the quota bound land first.
- **8.5 precedes 10.2.** Multi-tenant correctness gets pinned by tests before
  the concurrency model moves underneath it.
- **8.4 precedes the rest of Stage 8 closing.** Without the audit gate the
  next route family repeats D2.

## Verification Baseline

`cargo test --workspace` and `make daemon-contract-test` green at each closing
commit, with schema changes landing in `schemas/` before client regeneration.
Stage-specific additions:

| Stage | Additional gate |
|---|---|
| 1 | Retrieval benchmark at 10k Ready atoms, before/after turn latency |
| 4 | Quota-exhaustion test before the concurrency path defaults on |
| 8 | Two-tenant end-to-end suite (8.5) + the route audit gate (8.4) |
| 9 | Each tool exercised against a policy denial and a quota denial — `tool_host_tests::a_tool_call_meets_quota_and_policy_denials_as_tool_errors` (quota gate denial → tool error; `chat/tool-call` veto → tool error, host never called) plus the service-level veto test |
| 10.1 | CI builds the image and passes a `/healthz` smoke |
| 10.2 | Concurrent-session load test; store lock wait time recorded via 10.3 — `store_pool_load.rs` (numbers in the 10.2 entry) |
| 10.5 | Restart test: issued token authenticates, revoked token stays revoked |
| 10.6 | Online backup under two-tenant load restores with exact per-tenant counts |

## Rollback Posture

Every stage is designed to be revertible in isolation:

- Stage 1.1/1.2 — additive cache and DAO filter; reverting restores inline
  computation over the full corpus.
- Stage 3 — distillation produces ordinary proposals; the publication bridge
  and its Ready-only guard are unchanged.
- Stage 4 — ships behind an explicit opt-in, **not** default-on as OpenClaw
  chose; the default assembly stays behavior-identical.
- Stage 7.1 — config writes land behind a feature flag until the
  hot-apply/restart boundary table is published.
- Stage 8.1 — reverting the layer is safe: handlers reading the `Extension`
  directly behave identically with or without the task-local.
- Stage 9 — each tool is a plugin; disabling its profile entry removes it.
- Stage 10.2 — pool size 1 reproduces the current single-connection behavior
  exactly, so the pool can ship dark and be enabled by config.

## Risks

- **Stage 8 is a behavior change for any existing multi-user data.** Rows
  written before tenant scoping was enforced may carry NULL or a default
  tenant. The backfill path (`state.tenant_migration_status`, already handled
  by `protected()` with a 503) exists; Stage 8 must confirm it covers the
  five families in 8.2.
- **Stage 10.2 touches every persistence call site.** It is the one item here
  that cannot be delivered as a thin slice, which is why it sits behind 8.5's
  test coverage.
- **Stage 9 changes the product's cost profile.** Search and image generation
  are per-call spend against third-party providers; without 4.1's quota bound
  a single agent loop can run up an unbounded bill.

## Merge record (2026-09-20)

The program was built on `ddc5351`, twenty commits behind `origin/main`.
The merge (`merge/agent-deepening`) kept upstream's design wherever the two
overlapped — tool calling through `kura-core`, the `openai_compatible`
provider through `llm_bridge.rs`, upstream's schema v3 (`model_role_bindings`)
and v4 (`llm_dispatch_tools`) — and renumbered this program's migrations
after them: v5 `memory_asset_embeddings`, v6 `tool_profiles`. Everything
else here (Stages 0–8, 9.0b–9.5, 10.x, the defects table) carried over as
written. Databases created by the pre-merge branch (its v3–v5) are not
upgradable and must be re-initialised; none existed outside this machine.
