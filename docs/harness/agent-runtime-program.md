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
- [x] **1.4** Gateway OAuth sign-up wired to 1.1.
      *Done 2026-11-02* in `kura-gateway` (`-mode shared`; design in that
      repo's docs/design.md §16). The cell pilot is unchanged and stays
      the default.
      - *Sign-in and sign-up:*
        - GitHub and Google sign in through the authorization code flow
          with PKCE;
        - the subject is the IdP's stable id (`github:<id>`,
          `google:<sub>`) and is the provisioning idempotency key;
        - sign-up calls `POST /v1/provisioning/users` with the operator
          token, and the minted user token is sealed in the gateway DB;
        - sessions are server-side, and the cookie holds only a random id.
      - *Proxying:* every `/v1/*` request goes to the shared daemon with
        the user's own token. Client identity headers are stripped, and the
        cell-mode deny list still applies.
      - *Model key:* the settings page sets the user's model key as the
        tenant secret `llm.openai_compatible.api_key`, so the key never
        reaches the gateway DB.
      - *Operator controls:*
        - sign-up can be closed (`-signup closed`) and is rate-limited per
          hour;
        - a concurrent duplicate sign-up revokes its surplus token;
        - `admin accounts` lists accounts, and `admin disable` disables
          one and ends its sessions.
      - *Verified* by gateway unit tests and a local two-stranger end to
        end. The run used the real daemon in shared mode, the real
        gateway, a fake GitHub (`tools/oauth-smoke`) and a fake model:
        - both sign up and chat, plain and streamed, each on their own
          key, never the operator's;
        - reminders, memory and retrieval stay per tenant, and selecting
          another tenant is 403;
        - `kura_llm_tokens_total{tenant}` equals the usage on each user's
          key;
        - no key or token appears in any log.
      - *Found on the way and fixed:*
        - reminders were never bound to a tenant, so every tenant could
          list, read and act on every reminder (now bound, backfilled by
          schema v8, and filtered; shared-hosting.md §4.5);
        - the session summarizer's and consolidator's dispatches ran on the
          tenant's key but were not counted in `kura_llm_tokens_total`. The
          dispatcher now records spend for every dispatch.
      - *Not covered:*
        - real OAuth apps and deployment to the SG hosts (needs client ids
          and the operator's go-ahead);
        - the turn token budget (4.1) still charges chat turns only, not
          the background summarizer calls.
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
- [x] **3.3** MCP stdio servers and per-tenant browser contexts in the same
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
      *Browser done 2026-11-02* (decision: a throwaway browser per tenant,
      no saved logins). Design in `tenant-browser.md`.
      - *What tenants get:* the agent drives the browser through chat
        tools (`browser.open/read/click/type/back/close`). Each tenant gets
        one Playwright/Chromium worker in its own isolated container,
        started as an attached sandbox execution with an in-process-only
        image and limits (1 GiB, 512 pids). The worker image is
        `capabilities/browser/Dockerfile`, Chromium only (1.2 GB); CI
        builds it, runs the protocol under the isolated flags, and pushes
        `kura-browser-worker`.
      - *Bounds:* the egress proxy is the browser's only network.
        `KURA_BROWSER_MAX_SESSIONS` (4) caps browsers daemon-wide;
        `KURA_BROWSER_IDLE_SECONDS` (600) closes idle ones and removes
        their containers. Only `http`/`https` URLs open.
      - *Measured on sg-2:* session start ~1.3 s on runc and ~2.2 s on
        runsc; actions 10–150 ms; about 200 MiB per session.
      - *Found on the way and fixed:* the sandbox ended every attached
        execution at the profile's batch timeout (30 s). So MCP stdio
        servers, on the host and in tenant containers, were killed and
        auto-restarted every 30 s. An attached session is now bounded
        only by a timeout its caller sets.
      - *Found on the way and fixed (2):* under runsc, docker's embedded
        DNS does not answer inside gVisor. A tenant container could not
        resolve the proxy's name, so no networked run (browser included)
        got out. The proxy must be an IP under runsc, and the daemon now
        refuses to start otherwise. 3.4's "egress under runsc" gap is now
        verified.
      - *Verified end to end on dopejs-sg-2* (`scripts/tenant-browser-e2e/`),
        runc and runsc, all checks passing:
        - a tenant's chat calls `browser.open` and gets the page text;
        - one container per tenant, under the runtime, internal network
          only, read-only, no capabilities;
        - private addresses are refused by the proxy, metadata is
          unreachable, and with the proxy down nothing loads;
        - the idle container is removed.
      - *Not covered:* screenshots for the model (the loop is text only),
        downloads, and saved logins (the decision excluded them).
      *Earlier note (kept for the history):* per-tenant browser contexts,
      sequenced after 3.5. A browser
      needs network, and tenant containers get network only through 3.5's
      egress proxy. Until then tenants have no browser path at all: the
      computer-use session/action routes are operator-only in shared mode
      (pinned in `hosting.rs` `OPERATOR_ROUTES`), there is no browser chat
      tool, and workflow browser steps run through operator-only
      `POST /v1/runs`. The operator's browser worker remains a host process.
- [x] **3.4** Measure gVisor on the cell hosts; adopt as a second backend only
      if overhead fits.
      *Done 2026-11-02*; numbers and setup in
      `sandbox-execution-plane.md` §9, harness in
      `scripts/sandbox-runtime-bench/`. Measured on dopejs-sg-2 with gVisor
      `release-20261026.0` on systrap (the VM has no nested
      virtualization) and kura's isolated flags:
      - container start, CPU work and the kura daemon's boot are unchanged;
      - syscall-heavy work is 3–10× slower (5 ms per process spawn, 10×
        on `stat`, 6× on small workspace files, 2.9× on a Python start);
      - each container uses about 17 MiB more.
      *Decision:* it fits short agent tool runs and is the right choice
      for strangers' code. It is offered as an operator choice, not made
      the default. `KURA_SANDBOX_DOCKER_RUNTIME=runsc` runs every isolated
      container under it. The boot stops if docker lacks the runtime,
      rather than falling back to runc.
      *Verified:* unit tests for the argv and the config check. On sg-2 the
      Docker-gated isolation suite ran with `KURA_DOCKER_TEST_RUNTIME=runsc`:
      16 of 17 passed. The run's kernel is gVisor's, with the same uid,
      read-only root and workspace. The egress-proxy test needs a full
      `kura` binary as its sidecar (`KURA_EGRESS_PROXY_BIN`), which was
      not built there; CI runs it, on runc.
      *Host state:* sg-2 has runsc registered next to runc, the default.
      The pilot's `smoke` cell was not restarted.
      *Not covered:*
      - the egress-proxy path under runsc (proven on runc only; verified
        in 3.3, which found that runsc needs the proxy addressed by IP);
      - the KVM platform (needs a host with nested virtualization);
      - running the cell pilot's daemons under runsc (`kura-node` would
        need a runtime option; the daemon image boots under it);
      - dependency installs and builds, which the numbers say will be
        several times slower.
- [x] **3.5** G11 DNS rebinding (from 1.5): tenant workloads reach the network
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
      default, never the operator's `egress` config. *Topology done
      2026-10-31:* `KURA_SANDBOX_TENANT_NETWORK` (must be docker
      `--internal`, checked at boot, or tenant network stays off) plus
      `KURA_SANDBOX_EGRESS_PROXY`. An isolated run asking for full network
      joins that network, gets `HTTP(S)_PROXY` set to the proxy and DNS
      pointed at a dead upstream, and needs no operator approval because
      the proxy is the policy. Per-run host lists, ports and loopback stay
      refused. Proven in CI with the real binary as the sidecar: proxied
      HTTP works, metadata is refused by the proxy, bypassing the proxy has
      no route, and outside DNS fails. *Not covered:* daemon-side clients
      that fetch a tenant-influenced URL still resolve twice (none exist
      yet); the proxy enforces one policy for all tenants, with no
      per-tenant allow lists or attribution.

Done when a tenant can run code and MCP tools, a hostile workload cannot read
another tenant's workspace or the daemon's data dir (test proves it), and
resource exhaustion in one run does not degrade others beyond the limits.

- [x] **3.6 Which tenant went where** (from the 2026-11-02 gap review:
      "the egress proxy is one policy with no attribution"). All tenant
      containers share one internal network, so the proxy could not tell
      them apart.
      - The daemon and the proxy share a key
        (`KURA_SANDBOX_EGRESS_KEY_FILE`, `kura egress-proxy
        --tenant-key-file`). Each tenant's containers get a proxy URL
        carrying `tenant id : HMAC(key, tenant id)`.
      - The proxy refuses a request without a valid credential (`407` with
        a challenge). It logs every connection under its tenant when it
        opens and again when it closes, with bytes and duration. It caps a
        tenant at `--per-tenant-connections` (default 64 of 256 slots).
      - Chromium ignores proxy credentials in a URL, so the browser worker
        hands them to Playwright, which answers the `407`.
      - Without a key, behavior is unchanged and requests are logged as
        `unattributed`. Roll out the daemon first, then the proxy.
      *Verified:*
      - unit tests: credential derivation, the proxy's `407` / `429` /
        slot release, per-tenant URLs in the container flags, and the key
        needing egress;
      - `scripts/tenant-egress-e2e/` on dopejs-sg-2, under runc and
        runsc. A tenant's browser turn loads a page through the
        authenticated proxy. The proxy logs `tenant=… connect
        example.com:443 … opened`, then `closed up=2553 down=6954`. No
        credential, or another tenant's name with this credential, gets
        `407`. The private-address policy still applies, and the refusal
        carries the tenant.
      *Found by the end to end:* the first version logged a connection
      only when it closed, so a browser's kept-alive tunnel was invisible
      while it was in use.
      *Not done:* per-tenant destination policy, i.e. different allow
      lists per tenant. That is a product decision (what may a tenant
      reach), and the credential now makes it possible without changing
      the topology.

## Stage 2b — Runtime Core: Long Tasks

Sequenced after Stage 3 because resume semantics depend on where tools run.

- [x] **2.5 In-window summary (G3).** Elided messages are replaced by a
      model-written summary under its own budget; the summary is recorded in
      the `AssemblyRecord` like any other inclusion.
      *Shipped opt-in* (`summaryBudgetTokens`; see plugin-architecture.md,
      session strategy). Rolling and asynchronous: the summary is written
      behind the reply and used from the next dispatch, so the first turn
      that crosses the budget still sees only the marker. Coverage is found
      by a content digest of the last folded-in messages, since message ids
      do not reach the hook. *Not covered:* the summary is a system message
      written from content that includes tool output (G9 framing lands in
      2.8); every elided window still captures its whole span to L0 again
      (pre-existing, unchanged here).
- [x] **2.6 Parallel tool calls (G4).** Calls declared read-only run
      concurrently (bounded); calls with side effects stay serial and in
      order. Approval and quota gates are evaluated per call before any call
      in the batch starts.
      *Shipped* (`toolMaxParallel`, default 4; see plugin-architecture.md,
      chat). The gate is met by construction rather than a two-phase call:
      only calls that need no approval and reserve no quota may join a
      batch (`memory.lookup`; MCP tools declared `readOnlyHint` and allowed
      outright on the surface), and that is decided for every call of the
      round before any starts. *Not covered:* `readOnlyHint` is the
      server's claim, trusted for ordering only (it never relaxes
      authorization); a rule changed to `approval_required` between that
      check and the call makes that one call wait for approval while its
      read-only siblings run; tool-profile tools stay serial until their
      quota reservation moves ahead of the batch.
- [x] **2.7 Turn resume (G5).** Persist round state after each settled tool
      call; on restart, resume from the last settled call. Side-effecting
      tools carry an idempotency key so a replayed call is detected, not
      repeated.
      *Shipped* (`turnResume`, default on; see plugin-architecture.md, chat).
      The idempotency key is the turn key (tenant, `X-Request-Id`, thread,
      question) plus the call id, and detection happens in the loop: every
      call is journaled as started before it runs, so on resume a call with
      side effects that started and never settled is reported to the model
      as interrupted instead of repeated, and a granted approval is never
      asked again because that call is never re-entered. Resume is driven
      by the client retrying, not by the daemon at boot: a resumed turn's
      answer must reach whoever asked, and an unattended restart would
      spend tokens for nobody. *Not covered:* the key is not passed to tools,
      so a remote service cannot deduplicate on its side (no tool has an
      idempotent API to pass it to yet); a call interrupted while waiting
      for approval is reported as interrupted, and the model must call it
      again (a new approval); scheduled and IM turns use request ids that
      are not retried, so they do not resume; a crash between recording the
      answer and deleting the checkpoint replays the settled calls and asks
      the model once more.
- [x] **2.8** Structured output (G7), steering a running turn (G8), and
      untrusted-content framing for tool output (G9).
      *G7 provider-agnostic path done* (see plugin-architecture.md, chat):
      `responseFormat` on the non-streamed query, schema checked on arrival
      against an enforceable subset, schema in the system message, the
      final answer validated in the daemon with one correction round, and
      `structuredOutput` returned parsed. *Native path done:* the schema
      also goes to the OpenAI-compatible wire as `response_format`
      (`json_schema`, not strict; only on rounds offering no tools) and to
      the Responses wire as `text.format` (with tools); a 400 naming the
      field drops it for the life of the client. Validation stays in the
      daemon either way. *Not covered:* streamed queries; Anthropic's
      native structured output; the format is not recorded on the
      persisted dispatch, only in its system message; the native-format
      fallback is not logged or counted.
      *G9 done* (`toolOutputFraming`, default on; see plugin-architecture.md,
      chat): results are enclosed in `<tool_output>` with their own tags
      escaped, and the system message says the content is data. The 2.5
      summary reads framed results and its prompt already reports
      instructions in tool output as content. *Not covered:* ~~memory
      injected by the context hook is not framed~~ (closed 2026-11-02: memory
      items are framed in `<memory>` after one notice, `memoryFraming`, off is
      the rollback); text a provider-side tool returns never
      passes through this path.
      *G8 done 2026-11-01* (see plugin-architecture.md, chat): a running
      non-streamed query is reached by its tenant and `X-Request-Id` at
      `POST /v1/chat/turns/{requestId}/messages` and `/cancel`. The loop
      reads a `kura_core::TurnInbox` before every model call, and a message
      that arrives as the model answers makes that answer interim; the last
      check and closing the inbox are one step, so a message is read or
      refused (`turn_finished`), never accepted and dropped. Bounded at 8
      unread messages of 32 KiB. Found on the way: the dispatcher raced an
      already-cancelled token against a fresh attempt, so a cancelled
      round could still start a provider request; it now does not start
      one. *Not covered:* ~~streamed queries (one dispatch, no loop)~~
      (closed 2026-11-02, below); ~~a message is not in the 2.7 checkpoint, so a turn resumed after a
      crash has lost the messages it was sent~~ (closed 2026-11-02: a
      message is journaled when the loop reads it, before the model call
      that reads it, and a resumed turn puts it back in the same place;
      older checkpoints still read); reaching a turn running in
      another daemon process.

- [x] **2.9 Streamed turns run the agent loop** (found in the 2026-11-02
      gap review). A streamed query was one dispatch: no tools, so IM
      connectors that stream their replies (the streaming path) offered an
      agent with no memory lookup, MCP or browser. 2.6–2.8 did not apply
      to them either.
      *Done 2026-11-02:*
      - `Service::stream` runs the same turn as `query` (`turn_traced`).
      - Each round dispatches streaming into a channel that the turn
        drains into the caller's emitter inside the same `block_on`, so
        order is kept and nothing arrives after the turn ends.
      - A caller that stops listening cancels the turn.
      - A new round is a new `dispatchId`, and its `reply` starts over
        (interim text, as in a query). The SSE route (`RoundReply`) and
        the TUI (renders `reply`) follow this; IM already rendered the
        round's `reply`. Schema descriptions and the SDK type say so.
      - Steering now reaches streamed turns too.
      - Structured output is still refused on streams: text goes out
        before it could be checked.
      *Changed for clients:*
      - a streamed turn's `chat.query.started` no longer carries
        `requestTurnId` (recorded after the turn now; the terminal event
        has it);
      - if the model's first round writes nothing and calls a tool,
        `started` arrives with the first text of a later round.
      *Not covered:* no frame announces a tool call while it runs, so the
      stream is silent during tool calls.

## Stage 4 — Shared Hosting M4: Fairness and Scale

- [x] **4.1** Per-tenant rate, concurrency and token limits; fair run queue.
      *Done 2026-11-02*, design in `turn-admission.md`. Every turn takes a
      permit in `kura_chat::Service` (`admission.rs`) before it runs, and
      holds it to the end. This covers chat query/stream, IM, swarm
      children, activation and skill proposals.
      - *Concurrency:* over the global or per-tenant cap, the turn waits
        with a bounded queue and wait. A freed slot goes to the next
        tenant in round-robin, and one tenant's turns run in arrival
        order.
      - *Turn rate and token budget:* refused at once with 429 and
        `Retry-After`. Tokens are charged from provider-reported usage
        when a dispatch settles.
      - *Defaults:*
        - global 64 running and 256 waiting, sized from the 4.3 numbers
          and the 512 blocking threads;
        - shared mode per tenant: 8 turns, 30 turns a minute, 16 waiting;
        - token budget off: tenants dispatch with their own key, so it
          protects them from a runaway loop rather than the platform's
          spend;
        - the operator and the daemon's own work are exempt from the
          per-tenant limits.
      - *Config:* `entries.chat.config.limits`. `enabled: false` is the
        rollback. An unknown key fails the boot.
      - *Stream route:* now admits before it answers, so a refusal is a
        429, not a 200 followed by a failed stream.
      - *Metrics:* `kura_chat_turn_admissions_total{outcome}` and
        `kura_chat_turn_queue_wait_seconds`.
      - *Not covered:*
        - the limiter is per process;
        - `POST /v1/llm/dispatches` is a raw dispatch outside the gate
          (operator-only when shared);
        - a future tool that runs a sub-turn from inside a turn would need
          its parent's permit, or it waits out the timeout.
- [x] **4.2** Indexed token lookup; no store write per request (the
      `upsert_access_token` in `protected`); reader pool on by default.
      *Done 2026-11-01:* the auth manager keeps a `token_hash -> token_id`
      index (one place, `State::put_token`, maintains it) and answers a
      token used within `LAST_USED_RESOLUTION` (60 s) under the read lock;
      it used to scan every token under the write lock. `protected` writes
      only when `last_used_at` moved, through `touch_access_token`, which
      sets `last_used_at`/`updated_at` and never status. *Found on the
      way:* the old whole-row upsert, from a snapshot taken at
      authentication, could put a token revoked in between back to active
      in the store (in memory it stayed revoked, so the token came back on
      restart); `/v1/auth/me` had the same upsert and now relies on
      `protected`. `store.readers` defaults to 2 (`0`, in config or
      `KURA_STORE_READERS`, is the rollback). *Changed for clients:*
      `lastUsedAt` is accurate to a minute. *Not covered:* ~~an expiry
      noticed during authentication is still not persisted (as before)~~
      (closed 2026-11-02: the request that finds a token overdue records
      `expired` in the store, only over `active`; the Go
      `tenant.access_denied` audit event for it is still not written).
- [x] **4.3** Load test with parallel tools (2.6) and swarm enabled; the
      single writer connection is the expected bottleneck — measure via
      `kura_store_lock_wait_seconds` before changing it.
      *Done 2026-11-01:* `crates/surface/app/tests/agent_load.rs` runs the
      real assembly with bearer-token tenants, a local OpenAI-compatible
      model (25 ms per answer) whose first round asks for three parallel
      `memory.lookup` calls, and a three-goal swarm per tenant. It asserts
      structure (every turn answers after its lookups, every child
      completes, no write fails) and prints timings; CI runs 4×2×3.
      `kura_store_lock_wait_seconds` turned out to see readers and
      `pool.write()` only — ~95 handler writes lock `state.store`
      directly and the manager-owned connections wait inside SQLite — so
      two probes measure those instead: one acquiring the API writer mutex,
      one writing on its own connection.
      *Measured* (release build, this machine; model floor 50 ms a turn):

      | Concurrent sessions | Turn p50 / p95 | Writer mutex p95 / max | Own-connection write p95 / max | Reader wait, total |
      |---|---|---|---|---|
      | 32 (160 turns, 552 tool calls) | 131 / 205 ms | 12 / 24 ms | 12 / 69 ms | 0.15 ms over 1288 reads |
      | 128 (384 turns, 1296 tool calls) | 497 / 702 ms | 37 / 143 ms | 59 / 252 ms | 0.27 ms over 3024 reads |

      The writer is the bottleneck, as expected: reads are free with the
      pool, while write waits grow faster than the load (4× sessions, 3–4×
      the tail), and a turn makes many writes. Not changed here; 4.1 sizes
      admission from these numbers.
      *Found on the way:* the chat service built a tokio runtime per call,
      and a provider's pooled HTTP connections live on the runtime that
      opened them, so a later turn could be handed a dead connection and
      fail as `transport` ("runtime dropped the dispatch task"). It happened
      on 5 of 24 turns at 8 sessions; against a real keep-alive provider it
      would be intermittent. The bridge runtime is now one per process, like
      the mcp and consolidator runtimes. Transport errors now carry their
      causes; reqwest's own message stopped at "error sending request".
      *Limit:* above ~128 simultaneous connects the fake model's listen
      backlog (macOS `somaxconn` 128) resets connections; that is the
      harness, not the daemon.
- [x] **4.5 Write path under load** (from the 2026-11-02 gap review: "the
      single writer is the bottleneck"). Measured before changing anything,
      with a new commit counter, `kura_store_commits_total`, installed on
      every connection by the one place they are all opened
      (`SQLiteStore::configure`).
      *Findings* (the 4.3 load test; macOS figures in parentheses):
      - A chat turn made 27 commits. Event appends were 9.6 of them, the
        dispatch was 8 (each round's upsert then a separate tenant-bind
        `UPDATE`), turn checkpoints 4.6, and the rest memory, projections
        and swarm documents.
      - At 128 concurrent turns the Mac was idle: 1.7 s of CPU in 1.7 s of
        wall time on 14 cores. Two causes:
        - SQLite's default busy handler waits by sleeping 1–100 ms between
          tries, so writers slept through locks that were free after a
          fraction of a millisecond;
        - every chat read and write went through one connection behind one
          mutex (`secondary`), so reads queued behind other turns' writes
          and their file-lock waits.
      - On the cell host (dopejs-sg-2, cloud disk) every commit fsyncs
        under `synchronous=FULL`: an ordinary write takes ~8 ms at p50, and
        the host tops out near 440 commits/s.
      *Done:*
      - Each round's dispatch row and its tenant binding are written in one
        transaction (`upsert_llm_dispatch_for_tenant`), down to 23 commits a
        turn. This also closes a window where the row existed without its
        tenant.
      - Every connection gets a fine-grained busy handler: 50 µs doubling
        to 1 ms, 5 s in all as before. At 128 sessions (Mac), writer-mutex
        wait p95 went from 60 to 10 ms, and turn p50 from 455 to 373 ms.
      - The chat service reads on the store pool's query-only readers and
        writes on its own connection (`PooledChatStore`). A reader is
        query-only, so a "read" that writes fails loudly: that is how
        `active_agent_profile_selection` (which creates the default
        profile) was found to be a write.
      - `store.synchronous` / `KURA_STORE_SYNCHRONOUS`: `full` (default,
        unchanged) or `normal`. Under WAL, `normal` never corrupts the
        database and loses nothing on a daemon crash, but the last commits
        before a power loss or OS crash can be lost. The boot refuses
        anything else (no `off`).
      *Measured on dopejs-sg-2* (2 vCPU; same load, 3 tool calls a turn):

      | Sessions | `full` turn p50 / p95, run | `normal` turn p50 / p95, run |
      |---|---|---|
      | 8 | 589 ms / 1.20 s, 2.6 s | 189 ms / 338 ms, 0.97 s |
      | 32 | 1.73 s / 3.37 s, 10.7 s | 392 ms / 585 ms, 2.6 s |
      | 128 | 7.25 s / 10.6 s, 25.1 s | 1.71 s / 2.47 s, 6.4 s |

      `normal` is about 4× at every size. The default stays `full` because
      it is a durability choice; the shared host should set `normal`.
      *Not done, in order of the measured share:*
      - batch event appends into one transaction per few milliseconds (42%
        of commits), which delays an event's arrival in the ledger;
      - fewer checkpoint writes per turn;
      - more than one replica needs a store that is not one file on one
        host (Postgres), plus a shared turn limiter.
- [x] **4.4** Collapse the per-route tenant helpers (H4) into one extractor.
      *Done 2026-11-01:* `middleware::ActingTenant` (infallible
      `FromRequestParts`) replaces the twelve per-family copies
      (`acting_tenant` ×4, `context_tenant` ×2, `route_tenant`,
      `resolve_tenant`, `tenant_id_from`, `tenant_id`, `webhook_tenant`,
      `routes::scoped_tenant`) with three rules: `id()` (resolved or empty),
      `or_requested()` (resolved wins, request value only without one) and
      `scoped()` (a different requested tenant is 403). Each family keeps
      the rule it had, so no response changes; 106 handlers take it, and the
      audit gate counts it as a tenancy marker. *Not covered:* the 189
      remaining `Option<Extension<TenantContext>>` parameters hand the
      whole context to permission and ownership checks and were left as
      they are. *Found, then fixed 2026-11-02:*
      - `GET /v1/live-validations/kill-switches` let `?tenantId=` override
        the resolved tenant, so a member of another tenant could list that
        tenant's switches in single mode. It now uses the caller's tenant,
        and naming another is a 403.
      - Setting a switch had the same hole and worse: any member allowed
        to reconcile could set another tenant's tenant-scoped switch, and
        an enabled switch aborts that tenant's pending validations. The
        manager now refuses a tenant switch for any tenant but the
        caller's.
      - *Not changed:* global switches can still be set by anyone with
        reconcile permission. On a single daemon nothing distinguishes an
        operator from the tenants' owners, and in shared mode the route is
        operator-only.

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
