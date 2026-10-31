# Agent Runtime Program

Planning date: 2026-10-28. This document is the planning record for the work
that follows the [Agent Deepening Program](agent-deepening-program.md) (closed
2026-09-19) and the first milestone of
[Shared Hosting](shared-hosting.md) (M1, 2026-10-08). It merges three
sources into one ordered plan:

- the remaining Shared Hosting milestones (M2–M4);
- the follow-ups the deepening program recorded but did not schedule;
- an audit of the agent runtime core (context, caching, tool execution,
  durability, tracing) done on 2026-10-27/28.

Operator decisions taken at planning time (2026-10-28):

- **Gateway authentication (shared-hosting open question 1):** the
  provisioning API mints a per-user token that the gateway stores encrypted.
  The daemon keeps full authorization; a leaked gateway key is not a
  skeleton key. No gateway-signed identity header.
- **M3 execution backend (open question 2):** Docker-per-run first, behind a
  backend interface; gVisor (`runsc`) is a second backend, adopted only after
  it is measured on the cell hosts.
- **Roadmaps 76/77** are re-scoped from "public launch gate" to "evidence
  required before the hosted tier opens to strangers". They stay operator-run
  and do not block this program.
- **Order:** Stage 0 → Stage 1 (M2) → Stage 2a → Stage 3 (M3) → Stage 2b →
  Stage 4 (M4). See Sequencing.

## Current Truth

What `main` (`2f904fa`) already has, so nothing below re-plans shipped work:

| Area | Shipped | Where |
|---|---|---|
| Agent loop | model → tool → model rounds through `kura-core`, hard round cap, hookable (`chat/pre-dispatch`, `turn-start`, `turn-end`) | `crates/engine/core`, `crates/domains/chat` |
| Tools | MCP-backed tools behind the policy/approval gate, built-in tools (memory lookup, web search, image generation, browser worker) | `crates/domains/mcp`, `crates/surface/app/src/tool_host.rs` |
| Context | `session-strategy` frame-preserving elision with compression-to-memory; `context` memory bootstrap with an `AssemblyRecord` | `crates/domains/session`, `crates/domains/context` |
| Retrieval | BM25 + recency + vector, RRF; `context.embedder` seam (external plugin) with hashed n-gram fallback | `crates/domains/context`, `crates/surface/app/src/external.rs` |
| Observability | Prometheus `/metrics` (HTTP, LLM dispatch, tokens per tenant, store lock wait, hooks, tool calls), `x-request-id`, structured access log | `crates/foundation/telemetry` |
| Multi-tenancy | tenant task-local + `TenantContext`, two-tenant e2e tests, `KURA_HOSTING=shared` deny-by-default route policy | `crates/surface/api/src/{middleware,hosting}.rs` |
| Durability | run checkpoints from the runtime ledger, restored on boot | `crates/engine/checkpoints` |

## Gaps

Verified against `2f904fa`. Each row names the evidence; nothing here is
inferred from crate names alone.

| # | Gap | Evidence |
|---|---|---|
| G1 | Context budgets are counted in **characters**, not tokens, and are not derived from the target model's window | `crates/domains/session/src/lib.rs:46-49` (`*_budget_chars`) |
| G2 | No prompt caching: the stable prefix (system frame + memory bootstrap) is re-sent at full price every round | no `cache_control` / cache marker anywhere in `crates/` |
| G3 | Elided messages leave only a marker in the window; no in-window summary | `crates/domains/session/src/lib.rs:16-27` |
| G4 | Tool calls in one round run **serially** | `crates/domains/chat/src/round.rs:239` |
| G5 | A turn interrupted mid tool loop (crash, restart) has no defined resume semantics; checkpoints cover the runtime ledger, not the chat round | `crates/engine/checkpoints/src/lib.rs` (174 lines) |
| G6 | No per-turn trace: metrics are aggregates; one turn's rounds, dispatches and tool calls cannot be reconstructed as a tree | `crates/foundation/telemetry` has counters/histograms only |
| G7 | No structured output (`response_format` / schema-constrained replies) | no reference in `crates/engine`, `crates/modeling` |
| G8 | No way to steer a running turn (inject a message, change direction); only cancellation | `CancellationToken` only |
| G9 | Tool output is not marked untrusted to the model (prompt-injection posture) | — |
| G10 | The `embed` model role is configurable but no built-in embedder consumes it; without an external plugin, retrieval runs on the hashed n-gram fallback | `crates/foundation/config/src/types.rs:178`, `crates/domains/context/src/lib.rs:356` |
| G11 | Egress check does not pin the resolved address to the socket (DNS rebinding) | deepening program D8 |

Hygiene found in the same audit:

| # | Item | Evidence |
|---|---|---|
| H1 | Test temp dirs leaked under `$TMPDIR`; fix exists uncommitted (`tempfile` in tests, `scripts/cargo-test-isolated.sh`) | working tree at planning time |
| H2 | CI runs bare `cargo test`, local `make daemon-test` runs the isolated wrapper | `.github/workflows/ci.yml` |
| H3 | `go.work` / `go.work.sum` still reference the deleted Go `daemon/` | repo root |
| H4 | Route handlers fall back to a request-supplied `tenantId` when no `TenantContext` is present, via helpers copied per route family. Unreachable in the production assembly (auth and identity are always installed, `crates/surface/app/src/lib.rs:356`), and in shared mode `shared_route_policy` already refuses any request without a resolved tenant (`crates/surface/api/src/hosting.rs:191`); the only test of that covered operator routes, not tenant routes | `routes/{catalog,evidence,memory,mcp}.rs` and others (296 `Option<Extension<TenantContext>>` sites) |

## Stage 0 — Hygiene

- [x] **0.1** Land H1: tests own their temp dirs; `make daemon-test` /
      `rs-test` / `daemon-contract-test` run through the isolated wrapper.
      Done when the full workspace passes through the wrapper and leaves no
      `kura-*` directory behind.
- [x] **0.2** CI uses the same wrapper (H2).
- [x] **0.3** Remove `go.work` / `go.work.sum` (H3).
- [x] **0.4** Pin the shared-mode guarantee behind H4 with a test: every
      `TENANT_ROUTES` entry, called without a resolved tenant on a shared
      daemon, is refused with `hosting_operator_only`. No production change —
      the policy layer already enforces it. Collapsing the copied per-route
      helpers into one extractor is deferred to Stage 4 (touches every route
      family; no behavior change).

Rollback: each item is one commit; reverting restores the previous behavior.

**Stage 0 closed 2026-10-28** (`37af95f`, `975e7c2`, `1554b3f`, and the 0.4
commit). Verified by `make daemon-test` through the wrapper on macOS: 306 test
binaries, 1741 passed, 0 failed; the run directory was removed afterwards. The
first run surfaced a wrapper bug — macOS `TMPDIR` ends in `/`, so the joined
path held `//` and `kura-sandbox`'s isolated-home test compared it against the
canonical path — fixed in the wrapper before closure. CI's first run through
the wrapper (Linux and macOS) is the remaining confirmation for 0.2.

## Stage 1 — Shared Hosting M2: Provisioning and BYOK

Design detail lives in [shared-hosting.md](shared-hosting.md) §3; this stage
schedules it.

- [x] **1.1** Operator-only provisioning API: principal + personal tenant +
      membership + token in one call, idempotent on the external user id.
      Closed 2026-10-28: `POST /v1/provisioning/users`, design in
      [shared-hosting.md](shared-hosting.md) §6. Verified by identity unit
      tests (idempotency, interrupted-identity repair, no revival of inactive
      identities, audit fail-closed) and an app-level e2e test that validates
      the live response against `provision-user.response.schema.json`.
- [x] **1.2** Per-user gateway token (decision above): minted by 1.1,
      revocable, scoped to the one tenant. Closed 2026-10-28 with 1.1: the
      e2e test proves the token resolves as the new user, cannot select the
      operator tenant, and cannot provision. A per-principal token cap is M4.
- [x] **1.3** Per-tenant LLM credentials selected at dispatch; a tenant
      without a credential gets a clear "not configured" error, never the
      operator's key. Closed 2026-10-29: design in
      [shared-hosting.md](shared-hosting.md) §7 (dispatcher credential
      policy; key stored as the tenant secret `llm.openai_compatible.api_key`
      through the tenant-secret routes; other operator providers refused to
      tenants). The billing-boundary test is the app e2e
      `each_tenant_chats_on_its_own_key_and_never_on_the_operators`.
      `kura_llm_tokens_total{tenant}` already labels the requesting tenant,
      which is now also the key owner.
- [ ] **1.4** Gateway OAuth sign-up wired to 1.1.
- ~~**1.5** G11 DNS rebinding~~ — **moved to Stage 3 (3.5) on 2026-10-28.**
      The premise was wrong: in M2 no tenant controls an outbound
      destination. The resolving egress check fires only for networked tool
      families and none is networked today (MCP-backed tools network inside
      the MCP server process, `crates/domains/tools/src/lib.rs:108-115`); the
      daemon's own clients (LLM providers, MCP http/ws transports, channels)
      reach operator-configured or fixed hosts, and MCP server creation is
      operator-only in shared mode. Pinning inside the daemon's HTTP client
      would also not cover MCP servers, which resolve for themselves. 1.3 keeps
      it that way by giving tenants a key, never a base URL.

Done when two strangers sign up through the gateway, each runs chat, memory,
reminders and retrieval on their own key, the two-tenant e2e suite covers the
new routes, and `kura_llm_tokens_total{tenant}` attributes spend correctly.
Rollback: unset `KURA_HOSTING`; provisioning rows are ordinary tenant rows.

## Stage 2a — Runtime Core: Measure, Then Save

- [x] **2.1 Turn trace (G6).** One span tree per turn: turn → round →
      dispatch → tool call, carrying `x-request-id`, tenant, token usage and
      outcome. Readable through the API by turn id. Use `tracing` with an
      optional OTLP exporter; off-by-default export, always-on in-process
      recording bounded by retention.
      *Closed 2026-10-29.* `kura_telemetry::trace::TraceStore` records every
      chat turn (query and stream, however it ends) in process, bounded at
      512 turns × 256 spans. `turnId` is on the chat response;
      `GET /v1/turn-traces/{turnId}` and `GET /v1/turn-traces?requestId=`
      read it back, tenant-scoped (another tenant's turn is a 404; the
      operator reads all). A round span covers its dispatch and the tool
      calls it asked for. Traces carry no message text, tool arguments or
      tool output. **Deviation:** no `tracing` crate: the workspace has none,
      and the turn already has one explicit owner (`Service::run_agent_turn`)
      to record from, so spans are written there rather than through a
      global subscriber. The span list is flat with parent ids, so OTLP export
      can be added over finished `TurnTrace`s without re-instrumenting; it is
      not built (it was off-by-default anyway). Not covered: turns from
      outside chat (workflows, scheduler) and token cache-read counts (2.3
      adds them to `Usage`, and the dispatch span picks them up).
- [x] **2.2 Token budgets (G1).** Budgets in tokens, derived from the target
      model's context window; count with provider-reported usage where
      available and a calibrated estimator otherwise. Existing `*_chars`
      config keeps working (converted) for one release.
      *Session window done 2026-10-29* (plugin-architecture.md, "Token
      budgets"): `budgetUnit: chars|tokens|shadow`, token budgets capped by
      an operator-declared `modelContextWindows` less an output reserve,
      char budgets converted at 4:1, `shadow` to compare on live traffic.
      Default stays `chars` until shadow data says otherwise.
      *Context plugin done 2026-10-30* (plugin-architecture.md, "Token
      budgets" under the context plugin): the same `budgetUnit` with
      `memoryBudgetTokens` / `retrievalBudgetTokens`, each injected message
      charged at its rendered size, and every `AssemblyRecord` carrying
      token usage in all modes. *Estimator calibration 2026-10-30:* every
      dispatch span carries `estimatedInputTokens` next to the provider's
      `inputTokens`, and `kura_token_estimate_input_tokens_total{provider,
      source=estimated|reported}` sums both over completed dispatches with
      reported usage, so estimated/reported is the per-provider correction
      factor. **Deviation:** the factor is measured, not applied
      automatically. A self-adjusting estimator would move elision points
      between turns with no config change to explain it; the operator
      applies it through the token budgets instead. No built-in model-window
      table, deliberately.
- [ ] **2.3 Prompt caching (G2).** Keep the system frame + memory bootstrap a
      byte-stable prefix across rounds; emit the provider's cache marker where
      supported. Verified by 2.1 showing cache-read tokens on round ≥ 2.
      *Usage plumbing done 2026-10-30.* Found on the way: none of the three
      HTTP wires surfaced usage at all, so on a real provider every dispatch
      recorded zero tokens, 2.1 traces showed none, and 2.2's calibration
      metric had no samples. Now Anthropic (`message_start` /
      `message_delta`), chat-completions (`stream_options.include_usage`,
      dropped for the client's lifetime the first time a gateway answers 400
      naming it) and Responses (`response.completed`) report input, output
      and the cache split through a `ResponseEvent::Usage`. `Usage` gains
      `cacheReadInputTokens` / `cacheCreationInputTokens`, parts of
      `inputTokens` (Anthropic's uncached-only `input_tokens` is summed back
      to the whole prompt), omitted when zero so existing payloads are
      unchanged. Dispatch spans, turn usage and `kura_llm_tokens_total{kind=
      cache_read|cache_write}` carry them. Not covered: the borrowed Claude
      CLI path still reports input/output only.
      *Cache markers 2026-10-30.* OpenAI-shaped wires cache prefixes
      automatically and need no marker; the cached part now shows in usage.
      Anthropic gets two `cache_control` breakpoints: the last `system` block
      (or the last tool with no system prompt), whose prefix — tools, then
      the deterministic, timestamp-free frame — repeats across turns; and the
      last message block when tools are offered, so each tool-loop round
      re-reads the one before. Without tools a turn is one dispatch and the
      tail is not marked: a cache write costs more than plain input and
      nothing would read it. A 400 naming `cache_control` (a compatible
      gateway without caching) is retried once unmarked and the client stops
      marking. **Not verified live:** the round ≥ 2 cache-read check needs a
      real Anthropic key and is the operator's to run (the dispatch span's
      `cacheReadInputTokens`). **Open:** recalled memory sits ahead of the
      history and changes with each query, so across turns only the frame is
      reused; moving recall after the history would let the history cache
      too, but changes what the model sees and needs its own evaluation.
- [x] **2.4 Built-in embedder (G10).** When the `embed` role is routed, use
      it as the embedder; hashed n-gram stays the fallback. Re-embedding the
      corpus is an explicit operator action, not a boot side effect.
      *Done 2026-10-30:* `OpenAiCompatibleEmbeddingClient`
      (`crates/engine/model-provider/src/embeddings.rs`) and `RoleEmbedder`
      (`crates/surface/app/src/role_embedder.rs`), resolved at boot from the
      same store-then-config binding `GET /v1/models/roles` reports. The
      reply path embeds ≤16 misses per retrieval; the operator re-embed is
      `POST /v1/memory/indexes/rebuild?reembed=true`. Metrics
      `kura_retrieval_embeddings_total`, `kura_embedder_requests_total`.
      Fixed on the way: the external embedder's failure fallback cached
      hashed vectors under the plugin's fingerprint. Limits: OpenAI-compatible
      wire only (Anthropic has no embeddings API; Responses/Codex accounts are
      refused with a log); a role change needs a restart; the 0.25 vector
      threshold is untuned for neural models.

## Stage 3 — Shared Hosting M3: Isolated Execution

- [x] **3.1** Execution backend interface; Docker-per-run backend: own uid,
      only the tenant's workspace mounted, cgroup CPU/memory/pids limits,
      egress through `kura-egress`, DNS pinned, no host network.
      *Done 2026-10-30:* `ExecutionBackend` seam with `tenant_isolated()`,
      and the `tenant_isolated` profile
      (`crates/domains/sandbox/src/isolation.rs`; details in
      `sandbox-execution-plane.md` §5). Deviation: isolated runs get **no**
      network. A network request is denied rather than routed, and egress
      plus DNS pinning move to 3.5, which builds the proxy they need. Proven
      by real-container tests in CI's Linux job.
- [x] **3.2** Shared mode refuses to start execution on any backend that is
      not isolated (no plain subprocess path for tenants).
      *Done 2026-10-31:* `Manager::require_tenant_isolation`, installed at boot
      when `KURA_HOSTING=shared`. Any work not provably the operator's goes
      only to a backend whose `tenant_isolated()` is true, or is denied with
      `hosting:isolation_required`. "Provably the operator's" means the
      operator tenant's owner acting through a request, or in-process work
      owned by the operator tenant. A tenant's work, a non-owner member of
      the operator tenant, and work with no tenant all count as tenant work.
      The check is in `prepare`, so it covers the attached path (MCP stdio)
      as well. In-process callers name the owner with
      `ExecutionRequest.owner_tenant_id`, which is never read from JSON and
      must agree with the request context (`tenant:mismatch`). MCP passes the
      server's owner. Consequence until 3.3: on a shared daemon, a tenant's
      MCP stdio server and any unowned server are refused rather than run on
      the host.
- [ ] **3.3** MCP stdio servers and per-tenant browser contexts in the same
      plane.
      *MCP stdio done 2026-10-31.* An attached execution on the isolated
      backend runs `docker run -i` with live stdin/stdout, so an MCP stdio
      server on `tenant_isolated` runs in its tenant's container. MCP passes
      the owner and no host working dir or derived roots. Roots or network
      the server declares are still refused with the sandbox's reason. The
      container is removed when the session ends or is cancelled. The
      operator chooses an image that has the server's runtime, via
      `KURA_SANDBOX_DOCKER_IMAGE`; per-server images are not built yet.
      Fixed along the way: container env values no longer travel on the
      `docker` argv, which any host user can read with `ps`. `-e KEY` takes
      them from the client's environment, and client-steering names
      (`PATH`, `HOME`, `DOCKER_*`) go inline to the container only.
      *Open:* per-tenant browser contexts, sequenced after 3.5. A browser
      needs network, and tenant containers get network only through 3.5's
      egress proxy. Until then tenants have no browser path at all: the
      computer-use session/action routes are operator-only in shared mode
      (pinned in `hosting.rs` `OPERATOR_ROUTES`), there is no browser chat
      tool, and workflow browser steps run through operator-only
      `POST /v1/runs`. The operator's browser worker remains a host process.
- [ ] **3.4** Measure gVisor on the cell hosts; adopt as a second backend only
      if overhead fits.
- [ ] **3.5** G11 DNS rebinding (from 1.5): tenant workloads reach the network
      only through an egress proxy that resolves once, checks the address
      with `kura-egress`, and connects to that address. Daemon-side clients
      that take a tenant-influenced URL (none in M2) resolve through a
      policy-checking resolver for the same reason.
      *Proxy core done 2026-10-31:* `kura-egress-proxy` (CONNECT and
      absolute-form HTTP) and `kura egress-proxy`. It resolves once through
      `EgressPolicy::resolve_pinned`, checks every address, and dials that
      address. It refuses ports off the allow list (default 80/443), strips
      proxy credentials, and bounds head size, head time and connections.
      Metric `kura_egress_proxy_requests_total`. Its policy is the strict
      default, never the operator's `egress` config. *Open:* the topology
      that makes it the only exit (tenant containers on an `--internal`
      docker network, the proxy as the sidecar bridging it), and isolated
      runs that ask for network being routed there instead of refused.

Done when a tenant can run code and MCP tools, a hostile workload cannot read
another tenant's workspace or the daemon's data dir (test proves it), and
resource exhaustion in one run does not degrade others beyond the limits.

## Stage 2b — Runtime Core: Long Tasks

Sequenced after Stage 3 because resume semantics depend on where tools run.

- [ ] **2.5 In-window summary (G3).** Elided messages are replaced by a
      model-written summary under its own budget; the summary is recorded in
      the `AssemblyRecord` like any other inclusion.
- [ ] **2.6 Parallel tool calls (G4).** Calls declared read-only run
      concurrently (bounded); calls with side effects stay serial and in
      order. Approval and quota gates are evaluated per call before any call
      in the batch starts.
- [ ] **2.7 Turn resume (G5).** Persist round state after each settled tool
      call; on restart, resume from the last settled call. Side-effecting
      tools carry an idempotency key so a replayed call is detected, not
      repeated.
- [ ] **2.8** Structured output (G7), steering a running turn (G8), and
      untrusted-content framing for tool output (G9).

## Stage 4 — Shared Hosting M4: Fairness and Scale

- [ ] **4.1** Per-tenant rate, concurrency and token limits; fair run queue.
- [ ] **4.2** Indexed token lookup; no store write per request (the
      `upsert_access_token` in `protected`); reader pool on by default.
- [ ] **4.3** Load test with parallel tools (2.6) and swarm enabled; the
      single writer connection is the expected bottleneck — measure via
      `kura_store_lock_wait_seconds` before changing it.
- [ ] **4.4** Collapse the per-route tenant helpers (H4) into one extractor.

## Parallel Track — Hosted Launch Evidence (Roadmaps 76/77)

Operator-run, re-scoped per the decision above: the evidence register in
[daemon-roadmaps.md](../runtime/daemon-roadmaps.md) must be closed before the
hosted tier opens to strangers (end of Stage 3), not before a public binary
release.

## Sequencing

1. **Stage 0 first** — small, and 0.4 must hold before anyone but the
   operator can authenticate.
2. **Stage 1 (M2)** — the next shared-hosting milestone; it changes who can
   reach the daemon, so it follows the hygiene.
3. **Stage 2a** — cheap, and 2.1 is the instrument every later stage is
   verified with; 2.2/2.3 cut cost before tenant volume arrives.
4. **Stage 3 (M3)** — restores code execution for tenants; fixes where tools
   run, which 2.7 depends on.
5. **Stage 2b** — the long-task work, now on a settled execution plane.
6. **Stage 4 (M4)** — scale once the workload shape (parallel tools, swarm)
   exists to measure.

## Risks

- **2.2 changes what the model sees.** Converting budgets alters elision
  points; ship behind the existing strategy config with the old char budget
  as an explicit fallback, and compare `AssemblyRecord`s before switching.
- **2.6 and 2.7 interact with approvals.** A parallel batch must not start a
  call whose sibling is still waiting for human approval; a resumed turn must
  not re-ask an approval already granted. Both need tests before shipping.
- **Stage 3 widens the attack surface by design.** It ships only with the
  cross-tenant read test and the resource-exhaustion test passing.
- **Stage 1.3 is a billing boundary.** A tenant falling through to the
  operator's credential is a cost leak; the test must prove it cannot.
